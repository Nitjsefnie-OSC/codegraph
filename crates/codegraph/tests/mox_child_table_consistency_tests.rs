//! Issue #460: mox `contains` children must project consistently across the
//! DDL, repository, and classifier planes.
//!
//! The crewbase shape that surfaced the bug: `Person contains
//! LegalDocument[] legalDocuments` while `EmploymentPermit refers
//! LegalDocument[0..1] document`. "refers wins" makes LegalDocument an
//! entity (correct), so the containment edge must project as a synthetic
//! `person_id` back-ref FK on `legal_document` itself — not as a
//! `{parent}_{feature}` child table the DDL never creates (the pre-fix 500).
//!
//! Contract pinned here:
//! 1. every SQL table the repository plane references exists in the DDL
//!    (the cross-plane consistency sweep);
//! 2. contains→entity arrays use the back-ref projection (rows live in the
//!    target entity's own table, linked by `{parent}_id`);
//! 3. contains-only value objects project ONLY as `{parent}_{feature}`
//!    child tables — no standalone bare title-based table.

use std::fs;
use std::path::{Path, PathBuf};

/// Minimal crewbase-core slice: one entity that contains an entity-targeted
/// array (LegalDocument, kept alive by EmploymentPermit's refers), plus a
/// pure contains-only VO (Certification) and a VO that refers OUT
/// (EmploymentPermit — referring out never makes a class an entity).
const CORE_MOX: &str = r#"
package core

/// A durable identifier.
type Identifier wraps String

class Person {
    id readonly Identifier personId
    contains LegalDocument[] legalDocuments
    contains Certification[] certifications
    contains EmploymentPermit[] employmentPermits
}

class LegalDocument {
    String documentType
    String documentNumber
    String issuer
    date issueDate
    date expiryDate
}

class Certification {
    String [1] name
    String evidence
}

class EmploymentPermit {
    String [1] permitType
    String[] regions
    date effectiveFrom
    refers LegalDocument [0..1] document
}
"#;

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.core]
label = "Core"
schema_dir = "core"
postgres_schema = "core"
"#;

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
    mox: PathBuf,
}

fn write_fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("domains.toml"), DOMAINS_TOML).unwrap();
    fs::create_dir_all(root.path().join("model")).unwrap();
    fs::write(root.path().join("model/core.mox"), CORE_MOX).unwrap();
    Fixture {
        config: root.path().join("domains.toml"),
        mox: root.path().join("model/core.mox"),
        root,
    }
}

impl Fixture {
    fn out(&self) -> PathBuf {
        self.root.path().join("generated")
    }
}

fn run_args<'a>(
    fixture: &'a Fixture,
    mox_files: &'a [PathBuf],
    output: &'a Path,
) -> codegraph::driver::RunArgs<'a> {
    codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &fixture.config,
        output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        // Nonexistent path → the no-plan (all generators) path runs.
        profiles_config_path: Some(fixture.root.path().join("profiles.toml")),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files,
        rosetta_files: &[],
        ddd_files: &[],
        evt_files: &[],
        deploy_files: &[],
        ifml_framework: &[],
        ifml_components: None,
        ifml_design_system: None,
        codegraph_rev: None,
        check: false,
        ux_rules: None,
    }
}

/// Run the pipeline and return (created_tables, referenced_tables) where
/// created = `CREATE TABLE` names from migrations and referenced = the
/// `{schema}.{table}` pairs the generated repository SQL targets.
fn ddl_and_repo_tables(output: &Path, schema: &str) -> (Vec<String>, Vec<String>) {
    let mut created = Vec::new();
    let migrations = output.join("migrations");
    for entry in fs::read_dir(&migrations).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("sql") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        for cap in create_table_iter(&text) {
            created.push(cap);
        }
    }

    let mut referenced = Vec::new();
    let src = output.join("src");
    for entry in walk_dir_rs(&src) {
        let text = fs::read_to_string(&entry).unwrap();
        for cap in sql_table_ref_iter(&text, schema) {
            referenced.push(cap);
        }
    }
    (created, referenced)
}

fn create_table_iter(text: &str) -> Vec<String> {
    // CREATE TABLE IF NOT EXISTS {schema}.{table} (
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find("CREATE TABLE IF NOT EXISTS ") {
        let after = &rest[pos + "CREATE TABLE IF NOT EXISTS ".len()..];
        let end = after.find('(').unwrap_or(after.len());
        out.push(after[..end].trim().replace(['\\', '"'], ""));
        rest = after;
    }
    out
}

fn sql_table_ref_iter(text: &str, schema: &str) -> Vec<String> {
    // SQL keyword followed by {schema}.{table} — covers INSERT INTO,
    // DELETE FROM, UPDATE, SELECT … FROM, and JOIN forms.
    let keywords = ["INSERT INTO", "DELETE FROM", "UPDATE", "FROM", "JOIN"];
    let mut out = Vec::new();
    for kw in keywords {
        let mut rest = text;
        let needle = kw;
        while let Some(pos) = rest.find(needle) {
            let after = rest[pos + needle.len()..].trim_start();
            let prefix = format!("{}.", schema);
            if let Some(table_part) = after.strip_prefix(&prefix) {
                let table: String = table_part
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !table.is_empty() {
                    out.push(format!("{}.{}", schema, table));
                }
            }
            rest = &rest[pos + needle.len()..];
        }
    }
    out
}

fn walk_dir_rs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_dir_rs(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    out
}

fn migration(table: &str, output: &Path) -> Option<PathBuf> {
    let dir = output.join("migrations");
    fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(&format!("core_{table}.sql")))
        })
}

/// (1) The cross-plane consistency sweep: every `{core}.{table}` the
/// generated repository SQL references must be created by the DDL. This is
/// the general net — it catches ANY plane drift, not just the mox one.
#[tokio::test]
async fn repo_sql_never_references_tables_ddl_does_not_create() {
    let fixture = write_fixture();
    let output = fixture.out();
    let mox_files = vec![fixture.mox.clone()];

    codegraph::driver::run(run_args(&fixture, &mox_files, &output))
        .await
        .unwrap();

    let (created, referenced) = ddl_and_repo_tables(&output, "core");
    let created: std::collections::HashSet<String> = created.into_iter().collect();
    let mut missing: Vec<String> = referenced
        .into_iter()
        .filter(|t| !created.contains(t))
        .collect();
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "repository SQL references tables the DDL never creates: {missing:?}"
    );
}

/// (2) contains→entity array uses the back-ref projection: rows live in the
/// target entity's own table, linked by the synthetic `person_id` FK.
#[tokio::test]
async fn contains_to_entity_array_uses_backref_projection_in_repository() {
    let fixture = write_fixture();
    let output = fixture.out();
    let mox_files = vec![fixture.mox.clone()];

    codegraph::driver::run(run_args(&fixture, &mox_files, &output))
        .await
        .unwrap();

    let person_repo =
        fs::read_to_string(output.join("src/domain/core/person/repository_impl.rs")).unwrap();

    // No phantom feature-named table anywhere in the person repository.
    assert!(
        !person_repo.contains("person_legal_documents"),
        "person repository must not reference the phantom person_legal_documents table"
    );
    // Create: nested legal documents are created-and-linked in the target table.
    assert!(
        person_repo.contains("INSERT INTO core.legal_document"),
        "person create must insert legal documents into core.legal_document"
    );
    assert!(
        person_repo.contains("person_id"),
        "person create must bind the synthetic person_id back-ref"
    );
    // Hydrate: reads come back from the target table by the back-ref FK.
    assert!(
        person_repo.contains("FROM core.legal_document WHERE person_id"),
        "person hydration must read legal documents by person_id"
    );

    // The update-replace path targets the same table.
    assert!(
        person_repo.contains("DELETE FROM core.legal_document WHERE person_id"),
        "person update-replace must delete legal documents by person_id"
    );
}

/// (2b) the DTO plane projects the back-ref child: dto_response re-exports
/// the nested child response struct, and the create command carries the
/// `legal_documents` payload field.
#[tokio::test]
async fn contains_to_entity_array_appears_in_response_dto() {
    let fixture = write_fixture();
    let output = fixture.out();
    let mox_files = vec![fixture.mox.clone()];

    codegraph::driver::run(run_args(&fixture, &mox_files, &output))
        .await
        .unwrap();

    let dto = fs::read_to_string(output.join("src/domain/core/person/dto_response.rs")).unwrap();
    assert!(
        dto.contains("PersonLegalDocumentResponse"),
        "person response must re-export the nested PersonLegalDocumentResponse: {dto}"
    );

    let person_repo =
        fs::read_to_string(output.join("src/domain/core/person/repository_impl.rs")).unwrap();
    assert!(
        person_repo.contains("legal_documents"),
        "person create/hydrate must carry the legal_documents payload field"
    );
}

/// (3) DDL shape: legal_document carries the synthetic back-ref FK with
/// cascade delete; the pure-VO child tables keep the `{parent}_{feature}`
/// projection; the parent never carries a person_legal_documents table.
#[tokio::test]
async fn ddl_projects_backref_fk_and_vo_child_tables() {
    let fixture = write_fixture();
    let output = fixture.out();
    let mox_files = vec![fixture.mox.clone()];

    codegraph::driver::run(run_args(&fixture, &mox_files, &output))
        .await
        .unwrap();

    let legal = fs::read_to_string(migration("legal_document", &output).unwrap()).unwrap();
    assert!(
        legal.contains("person_id"),
        "synthetic back-ref column: {legal}"
    );
    assert!(
        legal.contains("REFERENCES core.person"),
        "back-ref FK targets the person table: {legal}"
    );
    assert!(
        legal.contains("ON DELETE CASCADE"),
        "containment deletes cascade: {legal}"
    );

    let person = fs::read_to_string(migration("person", &output).unwrap()).unwrap();
    assert!(
        person.contains("person_certifications"),
        "contains-only VO child table: {person}"
    );
    assert!(
        person.contains("person_employment_permits"),
        "refers-out VO child table: {person}"
    );
    assert!(
        person.contains("document_id"),
        "scalar refers inside a VO child is an FK column: {person}"
    );
    assert!(
        !person.contains("person_legal_documents"),
        "the entity-targeted contains array must not create a child table: {person}"
    );
}

/// (5) The Error diagnostic itself: the consistency sweep must FLAG a
/// repository child table the DDL universe never creates — the exact
/// `person_legal_documents` shape that used to 500 at runtime. Hand-built
/// Mock graph: `Person contains LegalDocument[]` with LegalDocument
/// entity-flagged, but the generation order passed to the sweep deliberately
/// omits LegalDocument's standalone DDL — the divergence must be named.
#[tokio::test]
async fn consistency_sweep_flags_repository_tables_ddl_never_creates() {
    use codegraph_core::mock::MockEngineBuilder;
    use codegraph_core::types::{CompositionNode, CompositionTree, PropertyNode, SchemaNode};
    use codegraph_type_contracts::RefClassificationKind;

    fn entity_schema(title: &str, table: &str) -> SchemaNode {
        SchemaNode {
            namespace: None,
            schema_id: format!("core/json/{title}.json"),
            title: title.to_string(),
            description: None,
            schema_type: "object".to_string(),
            classification: "entity_reference".to_string(),
            domain: Some("core".to_string()),
            rel_path: format!("core/json/{title}.json"),
            pg_type: "UUID".to_string(),
            rust_type: format!("{table}Root"),
            sea_orm_type: "Entity".to_string(),
            rust_type_name: title.to_string(),
            pg_table_name: table.to_string(),
            api_path_segment: String::new(),
            parent_schema: None,
            is_entity: true,
            is_codelist: false,
            is_primitive_wrapper: false,
            has_all_of: false,
            has_one_of: false,
            has_any_of: false,
            has_definitions: false,
            custom_annotations: Default::default(),
            access: None,
            annotations: None,
        }
    }

    fn contains_prop(name: &str) -> PropertyNode {
        PropertyNode {
            name: name.to_string(),
            prop_type: "array".to_string(),
            description: None,
            format: None,
            is_required: false,
            is_nullable: true,
            is_id: false,
            is_array: true,
            min_items: None,
            max_items: None,
            pattern: None,
            min_length: None,
            max_length: None,
            minimum: None,
            maximum: None,
            pg_column_name: name.to_string(),
            pg_column_type: "TEXT".to_string(),
            rust_field_name: name.to_string(),
            rust_field_type: "String".to_string(),
            sea_orm_type: "String".to_string(),
            render_strategy: String::new(),
            ref_target: None,
            classification: None,
            projection: None,
            classification_kind: Some(RefClassificationKind::ValueObject),
            type_expr: None,
            ui_override_detail: None,
            ui_override_list_cell: None,
            ui_override_form: None,
            ui_override_inline: None,
        }
    }

    let legal = entity_schema("LegalDocument", "legal_document");
    let person = entity_schema("Person", "person");
    let engine = MockEngineBuilder::default()
        .with_schema(person.clone())
        .with_schema(legal.clone())
        .with_properties("Person", vec![contains_prop("legal_documents")])
        .with_ref_target("legal_documents", "Person", legal.clone())
        .with_composition_tree(
            "Person",
            CompositionTree {
                root: CompositionNode {
                    field_name: "person".to_string(),
                    schema_title: "Person".to_string(),
                    table_schema: "core".to_string(),
                    table_name: "person".to_string(),
                    fk: None,
                    is_collection: false,
                    columns: vec![],
                    jsonb_columns: vec![],
                    children: vec![],
                    composite_range: None,
                    consumed_fields: vec![],
                },
            },
        )
        .build();

    let config = codegraph_config::config::parse_domain_config_str(DOMAINS_TOML).unwrap();

    // The order deliberately omits LegalDocument: its standalone DDL table
    // never enters the DDL universe, so the repository's back-ref child
    // (`core.legal_document`) is a divergence.
    let order = vec![codegraph_generate::GenerationEntry {
        schema_title: "Person".to_string(),
        domain: "core".to_string(),
        pg_schema: "core".to_string(),
        is_cyclic: false,
    }];

    let divergences = codegraph_generate::consistency::check_child_table_consistency(
        &engine,
        &config,
        &order,
        &[],
    )
    .await
    .unwrap();

    assert!(
        divergences
            .iter()
            .any(|d| d.contains("core.legal_document") && d.contains("never creates")),
        "sweep must flag the missing back-ref target table: {divergences:?}"
    );
}

/// (4) contains-only VOs keep BOTH surfaces: their standalone title-based
/// table (first-class CRUD via their own repository) AND the
/// `{parent}_{feature}` child table the parent's repository writes. What
/// must never happen (issue #460) is the entity-flavored split — a bare
/// table for the target while the repository writes a feature-named table
/// the DDL never creates.
#[tokio::test]
async fn contains_only_vo_keeps_standalone_and_child_tables() {
    let fixture = write_fixture();
    let output = fixture.out();
    let mox_files = vec![fixture.mox.clone()];

    codegraph::driver::run(run_args(&fixture, &mox_files, &output))
        .await
        .unwrap();

    for vo in ["certification", "employment_permit"] {
        assert!(
            migration(vo, &output).is_some(),
            "contains-only VO {vo} keeps its standalone table"
        );
    }
    // The entity keeps its own table.
    assert!(migration("legal_document", &output).is_some());
    assert!(migration("person", &output).is_some());

    // The parent's child-table projection is present.
    let person = fs::read_to_string(migration("person", &output).unwrap()).unwrap();
    assert!(person.contains("person_certifications"), "{person}");
    assert!(person.contains("person_employment_permits"), "{person}");
    // And the entity-targeted contains array projects as a back-ref on the
    // target's own table — never as a phantom person_legal_documents table.
    assert!(
        !person.contains("person_legal_documents"),
        "the entity-targeted contains array must not create a child table: {person}"
    );
    let legal = fs::read_to_string(migration("legal_document", &output).unwrap()).unwrap();
    assert!(legal.contains("person_id"), "{legal}");
}
