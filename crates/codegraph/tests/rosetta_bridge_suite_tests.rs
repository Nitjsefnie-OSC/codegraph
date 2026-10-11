//! Rosetta bridge contract suite over a committed fixture model
//! (issue #259, Rosetta epic work package WP2).
//!
//! The fixture (`tests/fixtures/rosetta_bridge/`) is a two-file,
//! two-namespace model exercising every bridge shape: builtins, enums
//! (with displayName), enum extends, type extends with `override`, a
//! choice, a named condition, attribute metadata, and a CROSS-NAMESPACE
//! type reference (proving the one-resolve-call wiring).
//!
//! Tests copy the fixture into a tempdir before use — the committed files
//! are never mutated. The insertion-order contract (WP1.1 finding:
//! `get_properties` serves name-sorted, so insertion order is only
//! observable through generator output) is pinned via `driver::run`
//! artifacts.

use std::path::{Path, PathBuf};

use codegraph_backend::{BackendConfig, create_backend};
use codegraph_config::config::{DomainConfig, parse_domain_config};

const FIXTURE_SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rosetta_bridge");

/// Copy the committed fixture tree into a fresh tempdir so tests never
/// mutate the committed files. Returns (tempdir, domains.toml, .rosetta
/// paths in canonical order: store, partners).
fn fixture_in_tempdir() -> (tempfile::TempDir, PathBuf, Vec<PathBuf>) {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(Path::new(FIXTURE_SRC), dir.path());
    let domains = dir.path().join("domains.toml");
    let rosetta = vec![
        dir.path().join("model/store.rosetta"),
        dir.path().join("model/partners.rosetta"),
    ];
    for path in &rosetta {
        assert!(path.exists(), "fixture file missing: {}", path.display());
    }
    (dir, domains, rosetta)
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

struct BridgeGraph {
    backend: codegraph_backend::Backend,
    outcome: codegraph::ingest::rosetta_ingest::RosettaIngestOutcome,
    _dir: tempfile::TempDir,
}

async fn ingest_fixture() -> BridgeGraph {
    let (dir, domains, rosetta) = fixture_in_tempdir();
    let backend = create_backend(&BackendConfig::default()).await.unwrap();
    let domain_config: DomainConfig = parse_domain_config(&domains).unwrap();
    let outcome = codegraph::ingest::rosetta_ingest::ingest_rosetta_files(
        backend.ingestor(),
        backend.querier(),
        &rosetta,
        &domain_config,
        "Type",
    )
    .await
    .unwrap();
    BridgeGraph {
        backend,
        outcome,
        _dir: dir,
    }
}

/// driver::run over the fixture; returns (tempdir, output dir).
async fn run_fixture() -> (tempfile::TempDir, PathBuf) {
    let (dir, domains, rosetta) = fixture_in_tempdir();
    let output = dir.path().join("generated");
    let args = codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &domains,
        output: &output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(PathBuf::from("definitely-absent-profiles.toml")),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &[],
        rosetta_files: &rosetta,
        ddd_files: &[],
        evt_files: &[],
        deploy_files: &[],
        ifml_framework: &[],
        ifml_components: None,
        ifml_design_system: None,
        codegraph_rev: None,
        check: false,
        ux_rules: None,
    };
    codegraph::driver::run(args).await.unwrap();
    (dir, output)
}

/// Walk the output tree and return all file paths (relative, slash-normalized).
fn output_files(output: &Path) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for entry in walkdir::WalkDir::new(output) {
        let entry = entry.unwrap();
        if entry.file_type().is_file() {
            files.push(
                entry
                    .path()
                    .strip_prefix(output)
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/"),
            );
        }
    }
    files.sort();
    files
}

/// Extract the sequence of struct field idents: lines of the form
/// `pub <ident>:` inside the entity source.
fn pub_field_order(entity_src: &str) -> Vec<String> {
    entity_src
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let rest = trimmed.strip_prefix("pub ")?;
            let ident = rest.split(':').next()?.trim();
            (ident.chars().all(|c| c.is_ascii_lowercase() || c == '_')).then(|| ident.to_string())
        })
        .collect()
}

/// (a) Stats over the whole fixture: 2 files, 4 types + 1 choice, 2 enums
/// (both getting codelist schemas), exactly ONE extends edge (the type
/// extend PriorityOrderType←OrderType; enum extends Urgency merges values
/// WITHOUT an edge — the JSON codelist path has no extends concept for the
/// DDL machinery), 1 condition, 2 namespaces, and nothing out-of-plane.
#[tokio::test]
async fn fixture_bridges_with_expected_stats() {
    let g = ingest_fixture().await;
    let s = &g.outcome.stats;
    assert_eq!(s.files, 2, "both fixture files counted");
    // OrderType, CustomerType, PriorityOrderType + PartnerType (2nd file)
    // PLUS the choice (the `types` counter includes choices, mirroring the
    // bridge suite where choice ProductType made types == 4).
    assert_eq!(s.types, 5);
    assert_eq!(s.choices, 1, "choice OrderKind");
    assert_eq!(s.enums, 2, "OrderStatus + Urgency");
    assert_eq!(s.enum_schemas, 2, "both enums get codelist SchemaNodes");
    // Type-level extends ONLY: PriorityOrderType←OrderType. Enum extends
    // (Urgency extends OrderStatus) merges parent values first but creates
    // no ExtendsSchema edge.
    assert_eq!(s.extends, 1, "exactly the type-level extends edge");
    assert_eq!(s.conditions_recorded, 1, "PositiveTotal");
    // #268: namespace NODES, not just declaring files — each dotted
    // namespace brings its parent chain (rosetta.fixture.store →
    // rosetta.fixture → rosetta, same for .partners).
    assert_eq!(s.namespaces, 4, "2 namespaces + 2 dotted parents");
    assert_eq!(s.namespace_imports, 1, "import rosetta.fixture.partners.*");
    assert_eq!(s.needs_review, 0, "fixture is fully in-plane");
    assert_eq!(s.needs_review_names, Vec::<String>::new());
    for title in [
        "OrderType",
        "CustomerType",
        "PriorityOrderType",
        "PartnerType",
        "OrderKind",
    ] {
        assert!(
            s.bridged_titles.iter().any(|t| t == title),
            "{title} must be bridged; bridged: {:?}",
            s.bridged_titles
        );
    }
}

/// (b) The WP1.1 insertion-order contract, pinned through GENERATED
/// OUTPUT: PriorityOrderType's entity struct lists inherited OrderType
/// attributes (name-sorted within the ancestor group) BEFORE own
/// attributes, and the overridden `label` appears in its inherited slot —
/// not appended after the own group.
#[tokio::test]
async fn insertion_order_is_ancestors_first_in_generated_output() {
    let (_dir, output) = run_fixture().await;
    let files = output_files(&output);
    let entity_rel = files
        .iter()
        .find(|f| f == &&"src/entity/store_priority_order.rs".to_string())
        .unwrap_or_else(|| panic!("PriorityOrderType entity missing; files: {files:?}"));
    let src = std::fs::read_to_string(output.join(entity_rel)).unwrap();
    let fields = pub_field_order(&src);
    assert!(
        !fields.is_empty(),
        "no pub fields found in {entity_rel}:\n{src}"
    );

    // Synthetic/audit columns (present or not depending on policy effects)
    // are excluded from the data-field ordering contract.
    let synthetic: &[&str] = &[
        "platform_organization_id",
        "created_at",
        "updated_at",
        "deleted_at",
        "deleted_by",
        "updated_by",
        "is_demo_data",
    ];
    let data_fields: Vec<&String> = fields
        .iter()
        .filter(|f| !synthetic.contains(&f.as_str()))
        .collect();

    // Entity-ref attributes (customer CustomerType, partner PartnerType)
    // materialize as CHILD entities with parent FKs, not columns on the
    // parent (the JSON-path composition machinery). The remaining data
    // fields must appear EXACTLY in ancestors-first order: the inherited
    // group name-sorted (id, label, placed_on, status, tags, total) with
    // the overridden `label` in its inherited slot, then own fields
    // (priority_level) last.
    let expected = [
        "id",
        "label",
        "placed_on",
        "status",
        "tags",
        "total",
        "priority_level",
    ];
    let actual: Vec<&str> = data_fields.iter().map(|s| s.as_str()).collect();
    assert_eq!(
        actual, expected,
        "generated field order must be ancestors-first (name-sorted group) \
         then own fields, with the overridden `label` in its inherited slot; \
         fields in file: {data_fields:?}"
    );

    // The override wins at the type level: label (0..1) inherited becomes
    // `label string (1..1)` → non-Optional in the child entity.
    assert!(
        src.contains("pub label: String,"),
        "overridden label must be required (non-Option):\n{src}"
    );

    // Entity-ref attributes ride the child-entity pattern.
    for child in [
        "src/entity/store_priority_order_customer.rs",
        "src/entity/store_priority_order_partner.rs",
    ] {
        assert!(
            files.iter().any(|f| f == child),
            "entity-ref child entity {child} missing; files: {files:?}"
        );
    }
}

/// (c) Insta snapshot of the core generated artifacts for OrderType: the
/// DDL table migration + the SeaORM entity file.
#[tokio::test]
async fn core_artifacts_snapshot() {
    let (_dir, output) = run_fixture().await;
    let files = output_files(&output);

    let ddl_rel = files
        .iter()
        .find(|f| {
            f.starts_with("migrations/")
                && f.ends_with("_store_order.sql")
                && !f.ends_with("_rls.sql")
                && !f.ends_with("_trigger.sql")
                && !f.ends_with("_fts.sql")
        })
        .unwrap_or_else(|| panic!("OrderType DDL missing; files: {files:?}"));
    let entity_rel = files
        .iter()
        .find(|f| f.starts_with("src/entity/") && f.ends_with("store_order.rs"))
        .unwrap_or_else(|| panic!("OrderType entity missing; files: {files:?}"));

    #[derive(serde::Serialize)]
    struct Artifacts {
        ddl: String,
        entity: String,
    }
    let artifacts = Artifacts {
        ddl: std::fs::read_to_string(output.join(ddl_rel)).unwrap(),
        entity: std::fs::read_to_string(output.join(entity_rel)).unwrap(),
    };
    insta::assert_yaml_snapshot!(artifacts);
}

/// (d) The cross-namespace reference resolved through the ONE resolve
/// call: OrderType.partner targets PartnerType, and PartnerType exists as
/// a schema node carrying its own (different) namespace annotation.
#[tokio::test]
async fn cross_namespace_reference_resolves() {
    let g = ingest_fixture().await;
    let q = g.backend.querier();

    let props = q.get_properties("OrderType").await.unwrap();
    let partner = props
        .iter()
        .find(|p| p.name == "partner")
        .unwrap_or_else(|| {
            panic!(
                "partner property missing; got: {:?}",
                props.iter().map(|p| &p.name).collect::<Vec<_>>()
            )
        });
    assert_eq!(partner.ref_target.as_deref(), Some("PartnerType"));

    let schemas = q.list_schemas(None).await.unwrap();
    let partner_schema = schemas
        .iter()
        .find(|s| s.title == "PartnerType")
        .expect("PartnerType schema node must exist");
    assert_eq!(
        partner_schema
            .custom_annotations
            .get("rosetta_namespace")
            .and_then(|v| v.as_str()),
        Some("rosetta.fixture.partners"),
        "PartnerType keeps its own namespace, not the referencing file's"
    );
}

/// (e) The choice OrderKind gets its own artifact set under `order_kind`
/// — never colliding with type OrderType, which strips the `Type` suffix
/// to `order` (defect #9 contract: choices stay addressable under their
/// full name; both tables coexist).
#[tokio::test]
async fn fixture_choices_get_unstripped_names() {
    let (_dir, output) = run_fixture().await;
    let files = output_files(&output);

    let kind_entity = files
        .iter()
        .find(|f| f == &&"src/entity/store_order_kind.rs".to_string())
        .unwrap_or_else(|| panic!("choice OrderKind entity missing; files: {files:?}"));
    let src = std::fs::read_to_string(output.join(kind_entity)).unwrap();
    assert!(
        src.contains("order_kind"),
        "choice entity must be tabled at order_kind:\n{src}"
    );

    // No collision with the type: OrderType strips the Type suffix to
    // `order`, a distinct artifact set.
    assert!(
        files.iter().any(|f| f == "src/entity/store_order.rs"),
        "type OrderType strips the Type suffix to `order`; files: {files:?}"
    );
    assert!(
        !files.iter().any(|f| f.ends_with("store_order_type.rs")),
        "choice artifacts must not masquerade as the stripped type name; files: {files:?}"
    );
}

/// (f) Conditions and metadata survive the committed fixture: OrderType
/// carries the named condition (Expr::to_json payload) and the id
/// attribute's `[metadata id]` annotation.
#[tokio::test]
async fn conditions_and_metadata_survive_the_fixture() {
    let g = ingest_fixture().await;
    let schemas = g.backend.querier().list_schemas(None).await.unwrap();
    let order = schemas
        .iter()
        .find(|s| s.title == "OrderType")
        .expect("OrderType schema node");

    let conditions = order
        .custom_annotations
        .get("rosetta_conditions")
        .and_then(|v| v.as_array())
        .expect("conditions array");
    assert_eq!(conditions.len(), 1);
    assert_eq!(
        conditions[0].get("name").and_then(|v| v.as_str()),
        Some("PositiveTotal")
    );
    let expression = conditions[0].get("expression").expect("expression payload");
    assert!(
        expression.get("kind").is_some(),
        "Expr::to_json is kind-tagged: {expression}"
    );

    let id_annotations = order
        .custom_annotations
        .get("rosetta_attribute_annotations")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.get("annotations"))
        .and_then(|v| v.as_array())
        .expect("[metadata id] on the id attribute must be recorded");
    assert!(!id_annotations.is_empty());
}
