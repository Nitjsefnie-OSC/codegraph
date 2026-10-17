//! Tests for generated Playwright CRUD-test entity-ref dependency setup.
//!
//! Regression coverage for the `beforeAll` dep setup that previously emitted
//! empty bodies (`.schema.json` stems not stripped), skipped ref fields, never
//! created transitive required FKs, and silently swallowed failures.
//!
//! Slices:
//! - A: scalar same-domain + cross-domain `.schema.json` refs resolve
//! - B: array refs assign an array id and testData sends the array
//! - C: required-only transitive closure is ordered leaf-first with FK wiring
//! - D: unsatisfiable required deps throw; required-FK cycles (self, mutual,
//!   longer) are BROKEN instead of aborting the whole describe block;
//!   optional refs are not created; cleanup is reversed and array-aware

use std::path::Path;

use codegraph::generate::ProjectConfig;
use codegraph::generate::traits::EntityGenerator;
use codegraph::generate::ui::e2e_test::UiE2eTestGenerator;
use codegraph_config::config::parse_domain_config_str;
use codegraph_core::mock::MockEngine;
use codegraph_core::types::{PropertyNode, SchemaNode};
use codegraph_type_contracts::RefClassificationKind;

// ── Fixture helpers ─────────────────────────────────────────────────────

fn schema(
    title: &str,
    table: &str,
    rust_type_name: &str,
    domain: &str,
    segment: &str,
) -> SchemaNode {
    SchemaNode {
        namespace: None,
        custom_annotations: Default::default(),
        access: None,
        annotations: None,
        schema_id: format!("{domain}/json/{table}.schema.json"),
        title: title.into(),
        description: None,
        schema_type: "object".into(),
        classification: "entity_reference".into(),
        domain: Some(domain.into()),
        rel_path: format!("{domain}/json/{table}.schema.json"),
        pg_type: "UUID".into(),
        rust_type: "Uuid".into(),
        sea_orm_type: "Uuid".into(),
        rust_type_name: rust_type_name.into(),
        pg_table_name: table.into(),
        api_path_segment: segment.into(),
        parent_schema: None,
        is_entity: true,
        is_codelist: false,
        is_primitive_wrapper: false,
        has_all_of: false,
        has_one_of: false,
        has_any_of: false,
        has_definitions: false,
    }
}

fn scalar(name: &str, pg_type: &str, is_required: bool) -> PropertyNode {
    PropertyNode {
        is_id: false,
        name: name.into(),
        prop_type: "string".into(),
        description: None,
        format: None,
        is_required,
        is_nullable: !is_required,
        is_array: false,
        min_items: None,
        max_items: None,
        pattern: None,
        min_length: None,
        max_length: None,
        minimum: None,
        maximum: None,
        pg_column_name: name.into(),
        pg_column_type: pg_type.into(),
        rust_field_name: name.into(),
        rust_field_type: "String".into(),
        sea_orm_type: "Text".into(),
        render_strategy: "direct_column".into(),
        ref_target: None,
        classification: Some("primitive_wrapper".into()),
        projection: None,
        classification_kind: None,
        ui_override_detail: None,
        ui_override_list_cell: None,
        ui_override_form: None,
        ui_override_inline: None,
        type_expr: None,
    }
}

/// A plain `format: uuid` scalar column that is NOT a `$ref`/graph edge —
/// e.g. `party.case_id`. The residual defect: these were never resolved as
/// entity refs and serialized as `'Test Case Id'` literals.
fn plain_uuid(name: &str, is_required: bool) -> PropertyNode {
    PropertyNode {
        is_id: false,
        name: name.into(),
        prop_type: "string".into(),
        description: None,
        format: Some("uuid".into()),
        is_required,
        is_nullable: !is_required,
        is_array: false,
        min_items: None,
        max_items: None,
        pattern: None,
        min_length: None,
        max_length: None,
        minimum: None,
        maximum: None,
        pg_column_name: name.into(),
        pg_column_type: "UUID".into(),
        rust_field_name: name.into(),
        rust_field_type: "Uuid".into(),
        sea_orm_type: "Uuid".into(),
        render_strategy: "direct_column".into(),
        ref_target: None,
        classification: Some("primitive_wrapper".into()),
        projection: None,
        classification_kind: None,
        ui_override_detail: None,
        ui_override_list_cell: None,
        ui_override_form: None,
        ui_override_inline: None,
        type_expr: None,
    }
}

fn entity_ref(name: &str, ref_target: &str, is_array: bool, is_required: bool) -> PropertyNode {
    PropertyNode {
        is_id: false,
        name: name.into(),
        prop_type: if is_array {
            "array".into()
        } else {
            "object".into()
        },
        description: None,
        format: None,
        is_required,
        is_nullable: !is_required,
        is_array,
        min_items: None,
        max_items: None,
        pattern: None,
        min_length: None,
        max_length: None,
        minimum: None,
        maximum: None,
        pg_column_name: name.into(),
        pg_column_type: "UUID".into(),
        rust_field_name: name.into(),
        rust_field_type: if is_array {
            "Vec<Uuid>".into()
        } else {
            "Uuid".into()
        },
        sea_orm_type: "Uuid".into(),
        render_strategy: "entity_reference".into(),
        ref_target: Some(ref_target.into()),
        classification: Some("entity_reference".into()),
        projection: None,
        classification_kind: Some(RefClassificationKind::EntityReference),
        ui_override_detail: None,
        ui_override_list_cell: None,
        ui_override_form: None,
        ui_override_inline: None,
        type_expr: None,
    }
}

fn config(entities: &[(&str, &str)]) -> codegraph_config::DomainConfig {
    // entities: (domain, title) pairs; all get full CRUD operations.
    let mut domains: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for (domain, title) in entities {
        domains.entry(domain).or_default().push(title);
    }
    let mut toml = String::from(
        "[defaults]\noperations = [\"create\", \"read\", \"update\", \"delete\", \"list\"]\n\n",
    );
    for (domain, titles) in domains {
        let list = titles
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(", ");
        toml.push_str(&format!(
            "[domains.{domain}]\nlabel = \"{domain}\"\nschema_dir = \"{domain}\"\npostgres_schema = \"{domain}\"\nentities = [{list}]\n\n"
        ));
        for title in titles {
            toml.push_str(&format!(
                "[domains.{domain}.entity_config.{title}]\nrole = \"root\"\n\n"
            ));
        }
    }
    parse_domain_config_str(&toml).expect("fixture domains.toml must parse")
}

fn tera() -> tera::Tera {
    let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    codegraph::generate::template_engine::create_tera(&template_dir).unwrap()
}

fn generate(
    engine: &MockEngine,
    config: &codegraph_config::DomainConfig,
    title: &str,
    domain: &str,
) -> String {
    let output = tempfile::TempDir::new().unwrap();
    let generator = UiE2eTestGenerator::new(output.path());
    let files = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(generator.generate(
            engine,
            title,
            domain,
            config,
            &tera(),
            &ProjectConfig::default(),
        ))
        .expect("UiE2eTestGenerator failed");
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(".owner.crud.test.ts"))
        .expect("owner.crud.test.ts should be generated")
        .content
        .clone()
}

fn owner_crud(
    engine: &MockEngine,
    config: &codegraph_config::DomainConfig,
    title: &str,
    domain: &str,
) -> String {
    generate(engine, config, title, domain)
}

fn after_all_block(content: &str) -> &str {
    match content.find("test.afterAll") {
        Some(idx) => &content[idx..],
        None => "",
    }
}

fn before_all_block(content: &str) -> &str {
    let Some(start) = content.find("test.beforeAll") else {
        return "";
    };
    // Bounded slice: the main-entity creates in the test bodies also call
    // createEntityAsAcme, so assertions scoped to `beforeAll` must stop at
    // the next hook.
    let end = content[start..]
        .find("test.afterAll")
        .map(|off| start + off)
        .unwrap_or(content.len());
    &content[start..end]
}

// ── Slice A: scalar refs resolve via graph + `.schema.json` fallback ────

#[test]
fn scalar_refs_resolve_to_non_empty_bodies_and_api_paths() {
    let person = schema("PersonType", "person", "Person", "hr", "person");
    let organization = schema(
        "OrganizationType",
        "organization",
        "Organization",
        "platform",
        "organization",
    );
    let worker = schema("WorkerType", "worker", "Worker", "hr", "worker");
    let engine = MockEngine::builder()
        .with_schema(worker)
        .with_schema(person.clone())
        .with_schema(organization.clone())
        .with_ref_target("person", "WorkerType", person)
        .with_ref_target("organization", "WorkerType", organization)
        .with_properties(
            "WorkerType",
            vec![
                scalar("name", "TEXT", true),
                entity_ref("person", "hr/json/person.schema.json", false, true),
                entity_ref(
                    "organization",
                    "../../platform/json/organization.schema.json",
                    false,
                    true,
                ),
            ],
        )
        .with_properties("PersonType", vec![scalar("full_name", "TEXT", true)])
        .with_properties("OrganizationType", vec![scalar("legal_name", "TEXT", true)])
        .build();
    let config = config(&[
        ("hr", "WorkerType"),
        ("hr", "PersonType"),
        ("platform", "OrganizationType"),
    ]);

    let content = owner_crud(&engine, &config, "WorkerType", "hr");

    // Non-empty dependency bodies (previously `{ }`).
    assert!(
        content.contains(
            "createEntityAsAcme(orgContext, '/hr/person', { 'full_name': 'Test Full Name' })"
        ),
        "person dep body must be non-empty:\n{content}"
    );
    // Same-domain scalar ref assigns a scalar id.
    assert!(
        content.contains("depIds['person_id'] = dep_1['id'] as string;"),
        "scalar ref must assign depIds keyed by the FK field:\n{content}"
    );
    // Cross-domain `.schema.json` ref resolves to `/<domain>/<segment>`.
    assert!(
        content.contains("createEntityAsAcme(orgContext, '/platform/organization'"),
        "cross-domain .schema.json ref must resolve via the graph:\n{content}"
    );
    assert!(
        content.contains("depIds['organization_id'] = dep_2['id'] as string;"),
        "cross-domain scalar ref must assign a depIds key:\n{content}"
    );
    // testData wires the main-entity field from depIds.
    assert!(
        content.contains("...(depIds['person_id'] ? { 'person_id': depIds['person_id'] } : {}),"),
        "testData must emit the entity-ref branch from entity_ref_deps:\n{content}"
    );
    // Required steps must not be wrapped in the old swallowing try/catch.
    assert!(
        !before_all_block(&content).contains("} catch (_e) {"),
        "required dep creation must not swallow failures:\n{content}"
    );
}

// ── Slice B: array refs ─────────────────────────────────────────────────

#[test]
fn array_ref_assigns_array_and_test_data_sends_it() {
    let party = schema("PartyType", "party", "Party", "hr", "party");
    let worker = schema("WorkerType", "worker", "Worker", "hr", "worker");
    let engine = MockEngine::builder()
        .with_schema(worker)
        .with_schema(party.clone())
        .with_ref_target("parties", "WorkerType", party)
        .with_properties(
            "WorkerType",
            vec![
                scalar("name", "TEXT", true),
                entity_ref("parties", "hr/json/party.schema.json", true, true),
            ],
        )
        .with_properties("PartyType", vec![scalar("party_name", "TEXT", true)])
        .build();
    let config = config(&[("hr", "WorkerType"), ("hr", "PartyType")]);

    let content = owner_crud(&engine, &config, "WorkerType", "hr");

    assert!(
        content.contains("depIds['parties'] = [dep_1['id'] as string];"),
        "array ref must assign an array id:\n{content}"
    );
    assert!(
        content.contains("...(depIds['parties'] ? { 'parties': depIds['parties'] } : {}),"),
        "testData must send the array through depIds:\n{content}"
    );
    assert!(
        after_all_block(&content).contains("Array.isArray(ids) ? ids : [ids]"),
        "cleanup must iterate array ids:\n{content}"
    );
}

// ── Slice C: required-only transitive closure ───────────────────────────

#[test]
fn required_closure_is_ordered_leaf_first_with_fk_wiring() {
    let tenant = schema("TenantType", "tenant", "Tenant", "platform", "tenant");
    let case = schema("CaseType", "case", "Case", "hr", "case");
    let party = schema("PartyType", "party", "Party", "hr", "party");
    let enrollment = schema(
        "EnrollmentType",
        "enrollment",
        "Enrollment",
        "hr",
        "enrollment",
    );
    let engine = MockEngine::builder()
        .with_schema(enrollment)
        .with_schema(party.clone())
        .with_schema(case.clone())
        .with_schema(tenant.clone())
        .with_ref_target("party", "EnrollmentType", party)
        .with_ref_target("case", "PartyType", case)
        .with_ref_target("tenant", "PartyType", tenant.clone())
        .with_ref_target("tenant", "CaseType", tenant)
        .with_properties(
            "EnrollmentType",
            vec![
                scalar("name", "TEXT", true),
                entity_ref("party", "hr/json/party.schema.json", false, true),
            ],
        )
        .with_properties(
            "PartyType",
            vec![
                scalar("name", "TEXT", true),
                entity_ref("case", "hr/json/case.schema.json", false, true),
                entity_ref("tenant", "platform/json/tenant.schema.json", false, true),
            ],
        )
        .with_properties(
            "CaseType",
            vec![
                scalar("name", "TEXT", true),
                entity_ref("tenant", "platform/json/tenant.schema.json", false, true),
            ],
        )
        .with_properties("TenantType", vec![scalar("name", "TEXT", true)])
        .build();
    let config = config(&[
        ("hr", "EnrollmentType"),
        ("hr", "PartyType"),
        ("hr", "CaseType"),
        ("platform", "TenantType"),
    ]);

    let content = owner_crud(&engine, &config, "EnrollmentType", "hr");

    let tenant_idx = content
        .find("depIds['tenant_id'] = dep_")
        .expect("tenant dep must be created");
    let case_idx = content
        .find("depIds['case_id'] = dep_")
        .expect("case dep must be created");
    let party_idx = content
        .find("depIds['party_id'] = dep_")
        .expect("party dep must be created");
    assert!(
        tenant_idx < case_idx && case_idx < party_idx,
        "closure must be ordered tenant→case→party:\n{content}"
    );

    assert!(
        content.contains("'tenant_id': depIds['tenant_id']"),
        "case body must fill its tenant FK:\n{content}"
    );
    assert!(
        content.contains("'case_id': depIds['case_id']"),
        "party body must fill its case FK:\n{content}"
    );
    assert!(
        content.contains("'tenant_id': depIds['tenant_id']"),
        "party body must fill its tenant FK:\n{content}"
    );

    // Cleanup runs in reverse creation order.
    let after = after_all_block(&content);
    let after_party = after.find("depIds['party_id']").expect("party cleanup");
    let after_case = after.find("depIds['case_id']").expect("case cleanup");
    let after_tenant = after.find("depIds['tenant_id']").expect("tenant cleanup");
    assert!(
        after_party < after_case && after_case < after_tenant,
        "cleanup must delete in reverse order:\n{after}"
    );
}

// ── Slice D: fail loudly, skip optionals, array-aware cleanup ───────────

#[test]
fn unsatisfiable_required_field_throws() {
    let person = schema("PersonType", "person", "Person", "hr", "person");
    let worker = schema("WorkerType", "worker", "Worker", "hr", "worker");
    let engine = MockEngine::builder()
        .with_schema(worker)
        .with_schema(person.clone())
        .with_ref_target("person", "WorkerType", person)
        .with_properties(
            "WorkerType",
            vec![
                scalar("name", "TEXT", true),
                entity_ref("person", "hr/json/person.schema.json", false, true),
            ],
        )
        // Geometry has no fabricable test value → unsatisfiable required field.
        .with_properties("PersonType", vec![scalar("shape", "GEOMETRY", true)])
        .build();
    let config = config(&[("hr", "WorkerType"), ("hr", "PersonType")]);

    let content = owner_crud(&engine, &config, "WorkerType", "hr");

    assert!(
        content.contains("throw new Error("),
        "unsatisfiable required dep must throw loudly:\n{content}"
    );
    assert!(
        content.contains("required field 'shape' on PersonType"),
        "throw message must be actionable:\n{content}"
    );
    assert!(
        !before_all_block(&content).contains("} catch (_e) {"),
        "required steps must not swallow failures:\n{content}"
    );
}

#[test]
fn required_mutual_cycle_is_broken_with_an_omitted_leg() {
    // A requires B, B requires A: the setup must NOT abort — the DFS breaks
    // the cycle at its last-discovered leg (A's `b_id`), so A is created
    // first without it and B fills `a_id` from the created A.
    let b = schema("BType", "b", "B", "hr", "b");
    let a = schema("AType", "a", "A", "hr", "a");
    let engine = MockEngine::builder()
        .with_schema(a.clone())
        .with_schema(b.clone())
        .with_ref_target("b", "AType", b)
        .with_ref_target("a", "BType", a)
        .with_properties(
            "AType",
            vec![
                scalar("name", "TEXT", true),
                entity_ref("b", "hr/json/b.schema.json", false, true),
            ],
        )
        .with_properties(
            "BType",
            vec![
                scalar("name", "TEXT", true),
                entity_ref("a", "hr/json/a.schema.json", false, true),
            ],
        )
        .build();
    let config = config(&[("hr", "AType"), ("hr", "BType")]);

    let content = owner_crud(&engine, &config, "AType", "hr");

    assert!(
        !content.contains("required dependency cycle detected"),
        "mutual cycle must not abort the setup anymore:\n{content}"
    );
    let before = before_all_block(&content);
    assert!(
        before.contains(
            "// fixture: breaking required-FK cycle BType -> AType -> BType by omitting 'b_id' on AType"
        ),
        "the break must be named in a comment:\n{before}"
    );
    // Both participants are created; the omitted leg is the break-point's.
    assert!(
        before.contains("createEntityAsAcme(orgContext, '/hr/a'")
            && before.contains("createEntityAsAcme(orgContext, '/hr/b'"),
        "both cycle participants must be created:\n{before}"
    );
    assert!(
        !before.contains("'b_id': depIds['b_id']") && before.contains("'a_id': depIds['a_id']"),
        "exactly the break-point leg (b_id on A) must be omitted:\n{before}"
    );
    // A (break-point) is created before B; B fills its a_id from depIds.
    let a_idx = before
        .find("depIds['a_id'] = dep_")
        .expect("A dep must be created");
    let b_idx = before
        .find("depIds['b_id'] = dep_")
        .expect("B dep must be created");
    assert!(
        a_idx < b_idx,
        "break-point must be created first:\n{before}"
    );
    // If the API genuinely requires the omitted FK, the create 4xxs — the
    // emitted catch must rethrow with a message naming the cycle, not pass.
    assert!(
        before.contains("cycle-break create for 'a_id' failed"),
        "break-point create failure must surface a clear error:\n{before}"
    );
    assert!(
        before.contains("make one ref optional in the model"),
        "the error must point at the modeling fix:\n{before}"
    );
}

#[test]
fn self_reference_leg_is_omitted_not_thrown() {
    // Job.job_id → Job (parent job): the base case of cycle breaking. The
    // closure must not recurse on the self-edge, no dep step is created, and
    // the conditional testData spread simply omits the leg.
    let job = schema("JobType", "job", "Job", "hr", "job");
    let engine = MockEngine::builder()
        .with_schema(job.clone())
        .with_ref_target("job", "JobType", job)
        .with_properties(
            "JobType",
            vec![
                scalar("name", "TEXT", true),
                entity_ref("job", "hr/json/job.schema.json", false, true),
            ],
        )
        .build();
    let config = config(&[("hr", "JobType")]);

    let content = owner_crud(&engine, &config, "JobType", "hr");

    assert!(
        !content.contains("required dependency cycle detected"),
        "self-reference must not abort the setup anymore:\n{content}"
    );
    let before = before_all_block(&content);
    assert!(
        !before.contains("createEntityAsAcme(orgContext, '/hr/job'"),
        "no dep step may be created for the self-reference:\n{before}"
    );
    assert!(
        !before.contains("breaking required-FK cycle"),
        "a self-reference is omitted silently, not 'broken':\n{before}"
    );
    assert!(
        content.contains("...(depIds['job_id'] ? { 'job_id': depIds['job_id'] } : {}),"),
        "the conditional spread stays — the leg is set only if Job is \
         created transitively as someone else's dep:\n{content}"
    );
}

#[test]
fn three_entity_cycle_creates_in_working_order_with_one_omitted_leg() {
    // A -> B -> C -> A: the DFS breaks exactly one leg (the last-discovered,
    // A's `b_id`) and still orders the remaining creates leaf-first:
    // A (break-point, no b_id) → C (fills a_id) → B (fills c_id).
    let b = schema("BType", "b", "B", "hr", "b");
    let c = schema("CType", "c", "C", "hr", "c");
    let a = schema("AType", "a", "A", "hr", "a");
    let engine = MockEngine::builder()
        .with_schema(a.clone())
        .with_schema(b.clone())
        .with_schema(c.clone())
        .with_ref_target("b", "AType", b)
        .with_ref_target("c", "BType", c)
        .with_ref_target("a", "CType", a)
        .with_properties(
            "AType",
            vec![
                scalar("name", "TEXT", true),
                entity_ref("b", "hr/json/b.schema.json", false, true),
            ],
        )
        .with_properties(
            "BType",
            vec![
                scalar("name", "TEXT", true),
                entity_ref("c", "hr/json/c.schema.json", false, true),
            ],
        )
        .with_properties(
            "CType",
            vec![
                scalar("name", "TEXT", true),
                entity_ref("a", "hr/json/a.schema.json", false, true),
            ],
        )
        .build();
    let config = config(&[("hr", "AType"), ("hr", "BType"), ("hr", "CType")]);

    let content = owner_crud(&engine, &config, "AType", "hr");

    assert!(
        !content.contains("required dependency cycle detected"),
        "three-entity cycle must not abort the setup anymore:\n{content}"
    );
    let before = before_all_block(&content);
    let break_comments = before.matches("breaking required-FK cycle").count();
    assert_eq!(
        break_comments, 1,
        "exactly one leg must be broken:\n{before}"
    );
    assert!(
        before.contains(
            "// fixture: breaking required-FK cycle BType -> CType -> AType -> BType by omitting 'b_id' on AType"
        ),
        "the break comment must name the full cycle path:\n{before}"
    );
    // Working creation order with the break-point first.
    let a_idx = before
        .find("depIds['a_id'] = dep_")
        .expect("A dep must be created");
    let c_idx = before
        .find("depIds['c_id'] = dep_")
        .expect("C dep must be created");
    let b_idx = before
        .find("depIds['b_id'] = dep_")
        .expect("B dep must be created");
    assert!(
        a_idx < c_idx && c_idx < b_idx,
        "creates must run A (break-point) → C → B:\n{before}"
    );
    // Exactly one omitted leg: A's b_id is gone, the other two FKs wire up.
    assert!(
        !before.contains("'b_id': depIds['b_id']")
            && before.contains("'a_id': depIds['a_id']")
            && before.contains("'c_id': depIds['c_id']"),
        "exactly the break-point leg (b_id on A) must be omitted:\n{before}"
    );
}

#[test]
fn optional_refs_are_not_created() {
    let person = schema("PersonType", "person", "Person", "hr", "person");
    let worker = schema("WorkerType", "worker", "Worker", "hr", "worker");
    let engine = MockEngine::builder()
        .with_schema(worker)
        .with_schema(person.clone())
        .with_ref_target("person", "WorkerType", person)
        .with_properties(
            "WorkerType",
            vec![
                scalar("name", "TEXT", true),
                entity_ref("person", "hr/json/person.schema.json", false, false),
            ],
        )
        .with_properties("PersonType", vec![scalar("full_name", "TEXT", true)])
        .build();
    let config = config(&[("hr", "WorkerType"), ("hr", "PersonType")]);

    let content = owner_crud(&engine, &config, "WorkerType", "hr");

    assert!(
        !content.contains("createEntityAsAcme(orgContext, '/hr/person'"),
        "optional refs must not be created:\n{content}"
    );
    assert!(
        !content.contains("dep_1"),
        "optional refs must not produce dependency steps:\n{content}"
    );
}

// ── Slice E: convention-resolved plain-uuid FK columns ──────────────────

#[test]
fn convention_uuid_fk_resolves_to_entity() {
    let case = schema("CaseType", "case", "Case", "hr", "case");
    let party = schema("PartyType", "party", "Party", "hr", "party");
    let engine = MockEngine::builder()
        .with_schema(party)
        .with_schema(case)
        .with_properties(
            "PartyType",
            vec![scalar("name", "TEXT", true), plain_uuid("case_id", true)],
        )
        .with_properties("CaseType", vec![scalar("name", "TEXT", true)])
        .build();
    let config = config(&[("hr", "PartyType"), ("hr", "CaseType")]);

    let content = owner_crud(&engine, &config, "PartyType", "hr");

    assert!(
        content.contains("createEntityAsAcme(orgContext, '/hr/case'"),
        "required plain-uuid case_id must resolve to the Case entity:\n{content}"
    );
    assert!(
        content.contains("depIds['case_id'] = dep_"),
        "convention ref must assign a depIds key:\n{content}"
    );
    assert!(
        content.contains("...(depIds['case_id'] ? { 'case_id': depIds['case_id'] } : {}),"),
        "testData must emit the entity-ref branch from the convention ref:\n{content}"
    );
    assert!(
        !content.contains("'Test Case Id'"),
        "resolved convention ref must not emit a literal:\n{content}"
    );
}

#[test]
fn convention_uuid_fk_optional_not_created() {
    let case = schema("CaseType", "case", "Case", "hr", "case");
    let party = schema("PartyType", "party", "Party", "hr", "party");
    let engine = MockEngine::builder()
        .with_schema(party)
        .with_schema(case)
        .with_properties(
            "PartyType",
            vec![scalar("name", "TEXT", true), plain_uuid("case_id", false)],
        )
        .with_properties("CaseType", vec![scalar("name", "TEXT", true)])
        .build();
    let config = config(&[("hr", "PartyType"), ("hr", "CaseType")]);

    let content = owner_crud(&engine, &config, "PartyType", "hr");

    assert!(
        !content.contains("createEntityAsAcme(orgContext, '/hr/case'"),
        "optional convention ref must not be created:\n{content}"
    );
    assert!(
        !content.contains("depIds['case_id']"),
        "optional convention ref must not produce a dep:\n{content}"
    );
}

#[test]
fn convention_uuid_fk_ambiguous_not_resolved() {
    // Two entities normalize to the same `case` stem in different domains;
    // the main entity is in neither domain, so the match stays ambiguous.
    let case_hr = schema("CaseType", "case", "Case", "hr", "case");
    let case_platform = schema("CaseRecordType", "case", "CaseRecord", "platform", "case");
    let party = schema("PartyType", "party", "Party", "identity", "party");
    let engine = MockEngine::builder()
        .with_schema(party)
        .with_schema(case_hr)
        .with_schema(case_platform)
        .with_properties(
            "PartyType",
            vec![scalar("name", "TEXT", true), plain_uuid("case_id", true)],
        )
        .with_properties("CaseType", vec![scalar("name", "TEXT", true)])
        .with_properties("CaseRecordType", vec![scalar("name", "TEXT", true)])
        .build();
    let config = config(&[
        ("identity", "PartyType"),
        ("hr", "CaseType"),
        ("platform", "CaseRecordType"),
    ]);

    let content = owner_crud(&engine, &config, "PartyType", "identity");

    assert!(
        !content.contains("createEntityAsAcme(orgContext, '/hr/case'")
            && !content.contains("createEntityAsAcme(orgContext, '/platform/case'"),
        "ambiguous convention ref must not create a dependency:\n{content}"
    );
    assert!(
        !content.contains("depIds['case_id']"),
        "ambiguous convention ref must not produce a dep:\n{content}"
    );
    assert!(
        !content.contains("'Test Case Id'"),
        "unresolved uuid must fall through to the uuid literal:\n{content}"
    );
}

#[test]
fn unresolved_required_uuid_emits_valid_uuid_literal() {
    let review = schema(
        "ReviewDecisionType",
        "review_decision",
        "ReviewDecision",
        "compliance",
        "review-decision",
    );
    let engine = MockEngine::builder()
        .with_schema(review)
        .with_properties(
            "ReviewDecisionType",
            vec![
                scalar("name", "TEXT", true),
                plain_uuid("reviewer_id", true),
            ],
        )
        .build();
    let config = config(&[("compliance", "ReviewDecisionType")]);

    let content = owner_crud(&engine, &config, "ReviewDecisionType", "compliance");

    assert!(
        !content.contains("'Test Reviewer Id'"),
        "unresolved required uuid must not emit a placeholder string:\n{content}"
    );
    assert!(
        content.contains("00000000-0000-4000-8000-"),
        "unresolved required uuid must emit a UUID-shaped literal:\n{content}"
    );
}

// ── Slice F: TEXT-typed convention FKs (the crewbase `worker_id` shape) ──
//
// A required `_id` column the model declares as a plain string (mox
// `String workerId` → pg TEXT, no `format: uuid`) is still a name-derived
// entity ref: it must flow through the SAME path as schema refs — omitted
// from testData, dep-created in beforeAll, conditional spread in the
// payload — instead of falling through to the `'Test Worker Id'` text
// sentinel (the 422 class: one mistyped field rejects every variant of the
// untagged `CreateXBody` enum).

/// A plain TEXT `_id` column with no `$ref`/graph edge and no uuid format.
fn text_fk(name: &str, is_required: bool) -> PropertyNode {
    PropertyNode {
        rust_field_type: "String".into(),
        ..scalar(name, "TEXT", is_required)
    }
}

#[test]
fn text_convention_fk_flows_through_the_schema_ref_path() {
    let widget = schema("WidgetType", "widget", "Widget", "hr", "widget");
    let holder = schema("HolderType", "holder", "Holder", "hr", "holder");
    let engine = MockEngine::builder()
        .with_schema(widget)
        .with_schema(holder)
        .with_properties("WidgetType", vec![scalar("name", "TEXT", true)])
        .with_properties(
            "HolderType",
            vec![scalar("name", "TEXT", true), text_fk("widget_id", true)],
        )
        .build();
    let config = config(&[("hr", "WidgetType"), ("hr", "HolderType")]);

    let content = owner_crud(&engine, &config, "HolderType", "hr");

    // Dep-created in beforeAll, leaf-first (the target before the payload).
    let before_all = before_all_block(&content);
    assert!(
        before_all.contains("createEntityAsAcme(orgContext, '/hr/widget'"),
        "text convention FK must dep-create its target:\n{before_all}"
    );
    assert!(
        before_all.contains("depIds['widget_id'] = dep_"),
        "the dep step must fill the convention ref's depIds key:\n{before_all}"
    );
    // Conditional spread in the payload; NO text sentinel anywhere.
    assert!(
        content.contains("...(depIds['widget_id'] ? { 'widget_id': depIds['widget_id'] } : {}),"),
        "testData must emit the entity-ref spread for the convention ref:\n{content}"
    );
    assert!(
        !content.contains("'Test Widget Id'"),
        "resolved convention FK must not emit a text sentinel:\n{content}"
    );
    assert!(
        content.contains("// 'widget_id': entity ref — emitted by _dep_setup.tera"),
        "testData must omit the field exactly like a schema ref:\n{content}"
    );
}

#[test]
fn text_convention_fk_without_creatable_target_keeps_sentinel_and_names_gap() {
    let widget = schema("WidgetType", "widget", "Widget", "hr", "widget");
    let holder = schema("HolderType", "holder", "Holder", "hr", "holder");
    let engine = MockEngine::builder()
        .with_schema(widget)
        .with_schema(holder)
        .with_properties("WidgetType", vec![scalar("name", "TEXT", true)])
        .with_properties(
            "HolderType",
            vec![scalar("name", "TEXT", true), text_fk("widget_id", true)],
        )
        .build();
    // The target entity has NO create operation — nothing to dep-create.
    let toml = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.hr]
label = "hr"
schema_dir = "hr"
postgres_schema = "hr"
entities = ["WidgetType", "HolderType"]

[domains.hr.entity_config.WidgetType]
role = "root"
operations = ["read", "list"]

[domains.hr.entity_config.HolderType]
role = "root"
"#;
    let config = parse_domain_config_str(toml).unwrap();

    let content = owner_crud(&engine, &config, "HolderType", "hr");

    assert!(
        !before_all_block(&content).contains("createEntityAsAcme(orgContext, '/hr/widget'"),
        "an uncreatable target must not be dep-created:\n{content}"
    );
    assert!(
        !content.contains("depIds['widget_id']"),
        "an uncreatable target must not produce a dep or a spread:\n{content}"
    );
    assert!(
        content.contains("'Test Widget Id'"),
        "the text sentinel is kept for an uncreatable target:\n{content}"
    );
    assert!(
        content.contains("// fixture: convention ref 'widget_id' has no creatable target"),
        "the emitted payload must name the gap:\n{content}"
    );
}

#[test]
fn text_convention_fk_with_no_matching_entity_names_gap() {
    // `widget_id` present, but NO widget entity exists anywhere — the name
    // convention claims a target that is not schema-backed.
    let holder = schema("HolderType", "holder", "Holder", "hr", "holder");
    let engine = MockEngine::builder()
        .with_schema(holder)
        .with_properties(
            "HolderType",
            vec![scalar("name", "TEXT", true), text_fk("widget_id", true)],
        )
        .build();
    let config = config(&[("hr", "HolderType")]);

    let content = owner_crud(&engine, &config, "HolderType", "hr");

    assert!(
        content.contains("'Test Widget Id'"),
        "no target schema — the sentinel is kept:\n{content}"
    );
    assert!(
        content.contains("// fixture: convention ref 'widget_id' has no creatable target"),
        "the emitted payload must name the gap:\n{content}"
    );
    assert!(
        !content.contains("depIds['widget_id']"),
        "no target schema — no dep:\n{content}"
    );
}

// ── Payload typing (issue #468 W2): payloads are keyed to the request ───

#[test]
fn owner_crud_test_data_is_key_typed_to_the_create_request() {
    let person = schema("PersonType", "person", "Person", "hr", "person");
    let engine = MockEngine::builder()
        .with_schema(person)
        .with_properties("PersonType", vec![scalar("full_name", "TEXT", true)])
        .build();
    let config = config(&[("hr", "PersonType")]);

    let content = owner_crud(&engine, &config, "PersonType", "hr");

    assert!(
        content.contains("Record<keyof Create"),
        "testData/updatedData must be keyed to the generated create request:\n{content}"
    );
    assert!(
        !content.contains("function testData(): Record<string, unknown>"),
        "create-capable entities must not fall back to the free-form record:\n{content}"
    );
}

// ── Slice E2: type-driven test-data sentinels (untagged-enum 422s) ──────
//
// The generated API body is `#[serde(untagged)] enum CreateXBody` — one
// mistyped field (e.g. a NUMERIC column sent as `'Test Full Time
// Equivalent'`) rejects EVERY variant with `data did not match any
// variant`. Sentinels must be derived from the field's pg column type.

/// A DATE column (the temporal scalar sibling of `timestamp`).
fn date_field(name: &str, is_required: bool) -> PropertyNode {
    PropertyNode {
        is_id: false,
        rust_field_type: "NaiveDate".into(),
        ..scalar(name, "DATE", is_required)
    }
}

#[test]
fn numeric_columns_get_numeric_sentinels_not_text() {
    // The reported crewbase 422 shape: FTE (pg NUMERIC) sent as
    // 'Test Full Time Equivalent'.
    let job = schema("JobType", "job", "Job", "core", "job");
    let engine = MockEngine::builder()
        .with_schema(job)
        .with_properties(
            "JobType",
            vec![
                scalar("title", "TEXT", true),
                numeric("full_time_equivalent", "NUMERIC(5,2)", true),
                numeric("headcount", "INTEGER", true),
            ],
        )
        .build();
    let config = config(&[("core", "JobType")]);

    let content = owner_crud(&engine, &config, "JobType", "core");

    assert!(
        content.contains("'full_time_equivalent': 42.5,"),
        "NUMERIC column must emit the decimal sentinel:\n{content}"
    );
    assert!(
        !content.contains("'Test Full Time Equivalent'"),
        "a NUMERIC column must never get the readable text sentinel:\n{content}"
    );
    assert!(
        content.contains("'headcount': 42,"),
        "INTEGER column must emit the integer sentinel:\n{content}"
    );
    assert!(!content.contains("'Test Headcount'"), "{content}");
}

#[test]
fn date_and_timestamp_columns_get_iso_sentinels() {
    let job = schema("JobType", "job", "Job", "core", "job");
    let engine = MockEngine::builder()
        .with_schema(job)
        .with_properties(
            "JobType",
            vec![
                scalar("title", "TEXT", true),
                date_field("effective_date", true),
                timestamp("opened_at", true),
            ],
        )
        .build();
    let config = config(&[("core", "JobType")]);

    let content = owner_crud(&engine, &config, "JobType", "core");

    assert!(
        content.contains("'effective_date': '2026-01-15',"),
        "DATE column must emit the date sentinel:\n{content}"
    );
    assert!(
        content.contains("'opened_at': '2026-01-15T08:00:00Z',"),
        "TIMESTAMPTZ column must emit the datetime sentinel:\n{content}"
    );
    assert!(
        !content.contains("'Test Effective Date'") && !content.contains("'Test Opened At'"),
        "temporal columns must never get the readable text sentinel:\n{content}"
    );
    assert!(
        !content.contains("'2025-01-15") && !content.contains("'2025-06-"),
        "the retired 2025 sentinels must be gone:\n{content}"
    );
}

#[test]
fn range_and_typed_array_columns_get_shape_correct_literals() {
    let booking = schema("BookingType", "booking", "Booking", "core", "booking");
    let engine = MockEngine::builder()
        .with_schema(booking)
        .with_properties(
            "BookingType",
            vec![
                scalar("title", "TEXT", true),
                with_pg(scalar("validity", "String", true), "DATERANGE"),
                with_pg(scalar("window", "String", true), "TSTZRANGE"),
                with_pg(scalar("tags", "Vec<String>", true), "TEXT[]"),
                with_pg(numeric("scores", "INTEGER[]", true), "INTEGER[]"),
            ],
        )
        .build();
    let config = config(&[("core", "BookingType")]);

    let content = owner_crud(&engine, &config, "BookingType", "core");

    assert!(
        content.contains("'validity': '[2026-01-01,2026-12-31)',"),
        "DATERANGE must emit the half-open date-range literal:\n{content}"
    );
    assert!(
        content.contains("'window': '[2026-01-01T00:00:00Z,2026-12-31T23:59:59Z)',"),
        "TSTZRANGE must emit the half-open instant-range literal:\n{content}"
    );
    assert!(
        content.contains("'scores': [42],"),
        "INTEGER[] must emit a numeric array:\n{content}"
    );
    // The retired closed-bracket instant range is gone.
    assert!(
        !content.contains("2025-12-31T23:59:59Z]"),
        "range literals must be half-open:\n{content}"
    );
}

/// A `scalar` retyped to a specific pg column type (ranges / typed arrays).
fn with_pg(mut prop: PropertyNode, pg_type: &str) -> PropertyNode {
    prop.pg_column_type = pg_type.into();
    if pg_type.ends_with("[]") {
        prop.is_array = true;
    }
    prop
}

// ── Slice F: ux-rules spec emission + per-block gating (issue #302) ─────

/// A codelist-backed column (CodelistReference prop + graph enum values).
fn codelist(name: &str, ref_target: &str, is_required: bool) -> PropertyNode {
    PropertyNode {
        is_id: false,
        name: name.into(),
        prop_type: "string".into(),
        description: None,
        format: None,
        is_required,
        is_nullable: !is_required,
        is_array: false,
        min_items: None,
        max_items: None,
        pattern: None,
        min_length: None,
        max_length: None,
        minimum: None,
        maximum: None,
        pg_column_name: name.into(),
        pg_column_type: "TEXT".into(),
        rust_field_name: name.into(),
        rust_field_type: "String".into(),
        sea_orm_type: "Text".into(),
        render_strategy: "direct_column".into(),
        ref_target: Some(ref_target.into()),
        classification: Some("codelist_reference".into()),
        projection: None,
        classification_kind: Some(RefClassificationKind::CodelistReference),
        type_expr: None,
        ui_override_detail: None,
        ui_override_list_cell: None,
        ui_override_form: None,
        ui_override_inline: None,
    }
}

/// A numeric column (quantity / money depending on the name).
fn numeric(name: &str, pg_type: &str, is_required: bool) -> PropertyNode {
    PropertyNode {
        is_id: false,
        rust_field_type: if pg_type.starts_with("NUMERIC") {
            "Decimal".into()
        } else {
            "i32".into()
        },
        ..scalar(name, pg_type, is_required)
    }
}

/// A timestamp column.
fn timestamp(name: &str, is_required: bool) -> PropertyNode {
    PropertyNode {
        is_id: false,
        rust_field_type: "DateTime<Utc>".into(),
        ..scalar(name, "TIMESTAMPTZ", is_required)
    }
}

fn enum_value(value: &str) -> codegraph_core::types::EnumValue {
    codegraph_core::types::EnumValue {
        value: value.into(),
        display_name: None,
        sort_order: 0,
    }
}

/// The canonical ux fixture entity: id, readable lead name, codelist
/// status, quantity, money, time point and an audit stamp.
fn refund_engine() -> MockEngine {
    let refund = schema("RefundType", "refund", "Refund", "hr", "refund");
    MockEngine::builder()
        .with_schema(refund)
        .with_properties(
            "RefundType",
            vec![
                plain_uuid("id", true),
                scalar("name", "TEXT", true),
                codelist("status", "hr/json/refund_status.json", true),
                numeric("headcount", "INTEGER", true),
                numeric("total_amount", "NUMERIC(10,2)", true),
                timestamp("due_at", true),
                timestamp("created_at", true),
            ],
        )
        .with_enum_values(
            "refund_status",
            vec![enum_value("draft"), enum_value("approved")],
        )
        .build()
}

/// Generate all files for one entity (the ux spec, the POM page class and
/// the shared kernel among them).
fn all_files(
    engine: &MockEngine,
    config: &codegraph_config::DomainConfig,
    project: &codegraph::generate::ProjectConfig,
    title: &str,
    domain: &str,
) -> Vec<(String, String)> {
    let output = tempfile::TempDir::new().unwrap();
    let generator = UiE2eTestGenerator::new(output.path());
    let files = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(generator.generate(engine, title, domain, config, &tera(), project))
        .expect("UiE2eTestGenerator failed");
    files
        .into_iter()
        .map(|f| (f.path.to_string_lossy().to_string(), f.content))
        .collect()
}

fn ux_project() -> codegraph::generate::ProjectConfig {
    let rules = codegraph_config::builtin_ux_rules().unwrap().rules;
    codegraph::generate::ProjectConfig {
        ux: codegraph::generate::UxConfig { ux: Some(rules) },
        ..ProjectConfig::default()
    }
}

/// Generate and return the `.ux.test.ts` content, or `None` when the spec
/// was not emitted.
fn ux_spec(
    engine: &MockEngine,
    config: &codegraph_config::DomainConfig,
    project: &codegraph::generate::ProjectConfig,
    title: &str,
    domain: &str,
) -> Option<String> {
    all_files(engine, config, project, title, domain)
        .into_iter()
        .find(|(p, _)| p.ends_with(".ux.test.ts"))
        .map(|(_, c)| c)
}

#[test]
fn ux_spec_absent_when_flag_off() {
    let engine = refund_engine();
    let config = config(&[("hr", "RefundType")]);

    let files = {
        let output = tempfile::TempDir::new().unwrap();
        let generator = UiE2eTestGenerator::new(output.path());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(generator.generate(
                &engine,
                "RefundType",
                "hr",
                &config,
                &tera(),
                &ProjectConfig::default(),
            ))
            .unwrap()
    };

    assert!(
        !files
            .iter()
            .any(|f| f.path.to_string_lossy().ends_with(".ux.test.ts")),
        "flag off must not emit a ux spec:\n{files:?}"
    );
}

#[test]
fn ux_spec_emitted_when_plan_and_list_output_present() {
    let engine = refund_engine();
    let config = config(&[("hr", "RefundType")]);

    let content = ux_spec(&engine, &config, &ux_project(), "RefundType", "hr")
        .expect("flag on + list output must emit the ux spec");

    // The spec drives the list page through the POM page class (#316).
    assert!(
        content.contains("import { RefundPage } from './refund.page';"),
        "{content}"
    );
    assert!(content.contains("new RefundPage(page)"), "{content}");
    // Fixture creation follows the shared API conventions.
    assert!(
        content.contains("createEntityAsAcme(orgContext, BASE_PATH"),
        "{content}"
    );
}

// ── POM emission (issue #316) ───────────────────────────────────────────

#[test]
fn pom_page_and_kernel_emitted_for_any_op_entities() {
    let engine = refund_engine();
    let config = config(&[("hr", "RefundType")]);

    let files = all_files(
        &engine,
        &config,
        &ProjectConfig::default(),
        "RefundType",
        "hr",
    );
    let page = files
        .iter()
        .find(|(p, _)| p.ends_with("refund.page.ts"))
        .expect("spec-emitting entity must emit its page class");
    let kernel = files
        .iter()
        .find(|(p, _)| p.ends_with("_support/pom.ts"))
        .expect("the shared POM kernel must be emitted");

    // Page class: routes + form fills + the kernel import (the UxTable
    // import rides the ux mirror — see pom_page_ux_surface_is_plan_driven).
    assert!(
        page.1.contains("export class RefundPage extends BasePage"),
        "{}",
        page.1
    );
    assert!(
        page.1
            .contains("import { BasePage } from '../_support/pom';"),
        "{}",
        page.1
    );
    assert!(
        page.1.contains("static readonly apiPath = '/hr/refund';"),
        "{}",
        page.1
    );
    assert!(
        page.1.contains("base = '/hr/refund'"),
        "top-level entities default their base route:\n{}",
        page.1
    );
    assert!(
        page.1.contains("async fillName(value: string)"),
        "{}",
        page.1
    );
    assert!(
        page.1.contains("static readonly detailUrlPattern"),
        "{}",
        page.1
    );
    // Kernel: BasePage + UxTable with the ids::-mirrored literal families.
    assert!(kernel.1.contains("export class BasePage"), "{}", kernel.1);
    assert!(kernel.1.contains("export class UxTable"), "{}", kernel.1);
    assert!(
        kernel
            .1
            .contains("[data-testid=\"${this.cfg.module}-${suffix}\"]"),
        "{}",
        kernel.1
    );
    for fragment in [
        "tid('chip')",
        "tid('copy')",
        "tid('actions-menu')",
        "tid('timeline-item')",
    ] {
        assert!(
            kernel.1.contains(fragment),
            "missing {fragment}:\n{}",
            kernel.1
        );
    }
}

#[test]
fn pom_kernel_emitted_once_per_generator_across_entities() {
    let engine = refund_engine();
    let config = config(&[("hr", "RefundType")]);

    let output = tempfile::TempDir::new().unwrap();
    let generator = UiE2eTestGenerator::new(output.path());
    let run = |generator: &UiE2eTestGenerator| {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(generator.generate(
                &engine,
                "RefundType",
                "hr",
                &config,
                &tera(),
                &ProjectConfig::default(),
            ))
            .unwrap()
    };
    let first = run(&generator);
    let second = run(&generator);

    assert!(
        first
            .iter()
            .any(|f| f.path.to_string_lossy().ends_with("_support/pom.ts")),
        "the first spec-emitting entity renders the kernel"
    );
    assert!(
        !second
            .iter()
            .any(|f| f.path.to_string_lossy().ends_with("_support/pom.ts")),
        "subsequent entities must not duplicate the kernel"
    );
}

#[test]
fn pom_page_ux_surface_is_plan_driven() {
    let engine = refund_engine();
    let config = config(&[("hr", "RefundType")]);

    // Flag ON: sortable/copy/actions/zebra forwarders ride the plan.
    let files = all_files(&engine, &config, &ux_project(), "RefundType", "hr");
    let page = &files
        .iter()
        .find(|(p, _)| p.ends_with("refund.page.ts"))
        .expect("page class")
        .1;
    assert!(
        page.contains("readonly table = new UxTable(this.page, {"),
        "{page}"
    );
    assert!(page.contains("locale: 'en-NZ'"), "{page}");
    assert!(page.contains("currency: 'NZD'"), "{page}");
    for forwarder in [
        "async sortBy(field: string, dir: 'asc' | 'desc')",
        "async copyCell(row: Locator, expected: string)",
        "async openActions(row?: Locator)",
        "async expectZebra()",
        "async expectFormatted(",
        "chipFor(text: string)",
    ] {
        assert!(page.contains(forwarder), "missing {forwarder}:\n{page}");
    }

    // Flag OFF: no ux mirror, no table — but the page class still exists
    // (the POM is spec-infra, NOT ux-gated).
    let files = all_files(
        &engine,
        &config,
        &ProjectConfig::default(),
        "RefundType",
        "hr",
    );
    let page = &files
        .iter()
        .find(|(p, _)| p.ends_with("refund.page.ts"))
        .expect("page class")
        .1;
    assert!(
        !page.contains("UxTable"),
        "flag off must not build a table:\n{page}"
    );
    assert!(
        !page.contains("async sortBy("),
        "flag off must not expose ux forwarders:\n{page}"
    );
}

#[test]
fn ux_spec_no_list_or_no_create_emits_no_file() {
    // operations without `list` — the list page contract is absent.
    let engine = refund_engine();
    let mut toml =
        String::from("[defaults]\noperations = [\"create\", \"read\", \"update\", \"delete\"]\n\n");
    toml.push_str(
        "[domains.hr]\nlabel = \"hr\"\nschema_dir = \"hr\"\npostgres_schema = \"hr\"\nentities = [\"RefundType\"]\n\n[domains.hr.entity_config.RefundType]\nrole = \"root\"\n",
    );
    let config = parse_domain_config_str(&toml).unwrap();

    let files = {
        let output = tempfile::TempDir::new().unwrap();
        let generator = UiE2eTestGenerator::new(output.path());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(generator.generate(
                &engine,
                "RefundType",
                "hr",
                &config,
                &tera(),
                &ux_project(),
            ))
            .unwrap()
    };
    assert!(
        !files
            .iter()
            .any(|f| f.path.to_string_lossy().ends_with(".ux.test.ts")),
        "no list output must not emit a ux spec:\n{files:?}"
    );
}

#[test]
fn ux_spec_without_codelists_has_no_chip_block() {
    let refund = schema("RefundType", "refund", "Refund", "hr", "refund");
    let engine = MockEngine::builder()
        .with_schema(refund)
        .with_properties(
            "RefundType",
            vec![
                plain_uuid("id", true),
                scalar("name", "TEXT", true),
                numeric("headcount", "INTEGER", true),
            ],
        )
        .build();
    let config = config(&[("hr", "RefundType")]);

    let content =
        ux_spec(&engine, &config, &ux_project(), "RefundType", "hr").expect("spec emitted");

    assert!(
        !content.contains("status chips render"),
        "no codelist ⇒ no chip block:\n{content}"
    );
    assert!(
        !content.contains("-chip\""),
        "no codelist ⇒ no chip testid in the spec:\n{content}"
    );
    // Other blocks still fire.
    assert!(
        content.contains("sortable headers toggle aria-sort"),
        "{content}"
    );
}

#[test]
fn ux_spec_codelist_column_emits_chip_block_with_known_value() {
    let engine = refund_engine();
    let config = config(&[("hr", "RefundType")]);

    let content =
        ux_spec(&engine, &config, &ux_project(), "RefundType", "hr").expect("spec emitted");

    assert!(content.contains("status chips render"), "{content}");
    assert!(
        content.contains("ui.chipFor('draft')"),
        "fixture uses the codelist's first value through the POM:\n{content}"
    );
}

#[test]
fn ux_spec_timeline_rule_emits_timeline_block_skips_table_blocks() {
    let refund = schema("RefundType", "refund", "Refund", "hr", "refund");
    let engine = MockEngine::builder()
        .with_schema(refund)
        .with_properties(
            "RefundType",
            vec![
                plain_uuid("id", true),
                scalar("name", "TEXT", true),
                timestamp("due_at", true),
            ],
        )
        .build();
    let config = config(&[("hr", "RefundType")]);
    let rules = codegraph_config::parse_ux_rules_str(
        "[[collection]]\nentity_pattern = \"Refund*\"\ndisplay = \"timeline\"\norder_by = \"due_at\"\n",
    )
    .unwrap()
    .rules;
    let project = codegraph::generate::ProjectConfig {
        ux: codegraph::generate::UxConfig { ux: Some(rules) },
        ..ProjectConfig::default()
    };

    let content = ux_spec(&engine, &config, &project, "RefundType", "hr")
        .expect("timeline entity emits the ux spec");

    assert!(
        content.contains("timeline renders entries newest-first"),
        "{content}"
    );
    assert!(
        content.contains("ui.timelineRoot()"),
        "timeline access flows through the POM:\n{content}"
    );
    // Table-only blocks stay out.
    assert!(
        !content.contains("list renders the ux table contract"),
        "{content}"
    );
    assert!(
        !content.contains("zebra shading"),
        "zebra is table-only:\n{content}"
    );
    assert!(
        !content.contains("aria-sort"),
        "timelines keep their fixed DESC order — no sort block:\n{content}"
    );
}

#[test]
fn ux_spec_sort_block_pins_allow_list_and_flip() {
    let engine = refund_engine();
    let config = config(&[("hr", "RefundType")]);

    let content =
        ux_spec(&engine, &config, &ux_project(), "RefundType", "hr").expect("spec emitted");

    assert!(
        content.contains("sortable headers toggle aria-sort"),
        "{content}"
    );
    // The allow-list follows the plan's column_order (name leads, audit
    // stamp last) — mirrored from the API 400 message.
    assert!(
        content.contains("name, headcount, total_amount, due_at, created_at"),
        "{content}"
    );
    assert!(content.contains("sort=__nope__"), "{content}");
    assert!(content.contains("Unknown sort field"), "{content}");
    // The flip overrides the readable lead field with a distinct value.
    assert!(
        content.contains("{ 'name': 'Test Name B' }"),
        "asc/desc flip creates two distinct fixture rows:\n{content}"
    );
    assert!(content.contains("order=asc"), "{content}");
    assert!(content.contains("order=desc"), "{content}");
}

#[test]
fn ux_spec_copy_chip_block_uses_created_row_id() {
    let engine = refund_engine();
    let config = config(&[("hr", "RefundType")]);

    let content =
        ux_spec(&engine, &config, &ux_project(), "RefundType", "hr").expect("spec emitted");

    assert!(
        content.contains("copy-chip copies the identifier"),
        "{content}"
    );
    assert!(
        content.contains("ui.copyCell(row, createdId)"),
        "clipboard polling lives in the kernel copyCell:\n{content}"
    );
    assert!(content.contains("ui.copyTrigger(row).hover()"), "{content}");
    // The identifier cell truncates → tooltip assertion rides along.
    assert!(content.contains("ui.tooltipFor(createdId)"), "{content}");
}

#[test]
fn ux_spec_alignment_and_format_blocks_reference_plan_columns() {
    let engine = refund_engine();
    let config = config(&[("hr", "RefundType")]);

    let content =
        ux_spec(&engine, &config, &ux_project(), "RefundType", "hr").expect("spec emitted");

    assert!(content.contains("numeric columns right-align"), "{content}");
    // Money/quantity/time-point expectations run through the kernel's
    // Intl mirror — the SAME formatters the page computes. Money is a
    // NUMERIC column → the type-driven 42.5 sentinel (not the integer 42).
    assert!(content.contains("'money', 42.5)"), "{content}");
    assert!(content.contains("'quantity', 42)"), "{content}");
    assert!(
        content.contains("'time-point', '2026-01-15T08:00:00Z')"),
        "{content}"
    );
    // The readable lead column assertion pins column order row 17.
    assert!(content.contains("ui.firstCell()"), "{content}");
    assert!(
        content.contains("toHaveText('Test Name')"),
        "first rendered column is the readable field:\n{content}"
    );
}

#[test]
fn ux_spec_random_valued_columns_never_get_copy_or_flip_assertions() {
    // A required plain uuid that is NOT the id column resolves a random
    // fixture value — the spec must not predict it.
    let refund = schema("RefundType", "refund", "Refund", "hr", "refund");
    let engine = MockEngine::builder()
        .with_schema(refund)
        .with_properties(
            "RefundType",
            vec![
                scalar("name", "TEXT", true),
                plain_uuid("external_ref", true),
            ],
        )
        .build();
    let config = config(&[("hr", "RefundType")]);

    let content =
        ux_spec(&engine, &config, &ux_project(), "RefundType", "hr").expect("spec emitted");

    assert!(
        !content.contains("copy-chip copies the identifier"),
        "random uuid fixture ⇒ no clipboard prediction:\n{content}"
    );
}

// ── One testid vocabulary (issue #468 W3): the POM imports the module ───

#[test]
fn testid_module_pom_resolves_locators_through_the_module() {
    let person = schema("PersonType", "person", "Person", "hr", "person");
    let engine = MockEngine::builder()
        .with_schema(person)
        .with_properties("PersonType", vec![scalar("full_name", "TEXT", true)])
        .build();
    let config = config(&[("hr", "PersonType")]);
    let project = codegraph::generate::ProjectConfig {
        codegen: codegraph::generate::CodegenConfig {
            testid_module: true,
            ..Default::default()
        },
        ..ProjectConfig::default()
    };

    let files = all_files(&engine, &config, &project, "PersonType", "hr");
    let page = files
        .iter()
        .find(|(p, _)| p.ends_with("person.page.ts"))
        .expect("POM page emitted")
        .1
        .clone();

    // The module import rides the flag.
    assert!(
        page.contains("import { shared, testids, tidFor } from '#lib/testids.js';"),
        "{page}"
    );
    // Static locators resolve typed per-entity.
    assert!(page.contains("testids.person.table}"), "{page}");
    assert!(page.contains("testids.person.deleteBtn}"), "{page}");
    // The field locator goes through the typed union.
    assert!(page.contains("testids.person.field(name)"), "{page}");
    // Shared dialogs resolve through the shared table — and the old
    // unprefixed literals are gone from the delete flow.
    assert!(page.contains("shared.confirmDialog}"), "{page}");
    assert!(!page.contains("${this.module}-table"), "{page}");
    assert!(!page.contains("confirm-dialog-confirm"), "{page}");

    // Flag OFF (default): literals as before, no import.
    let files = all_files(
        &engine,
        &config,
        &ProjectConfig::default(),
        "PersonType",
        "hr",
    );
    let page = &files
        .iter()
        .find(|(p, _)| p.ends_with("person.page.ts"))
        .expect("POM page emitted")
        .1;
    assert!(!page.contains("#lib/testids.js"), "{page}");
    assert!(page.contains("${this.module}-table"), "{page}");
    assert!(page.contains("data-testid=\"confirm-dialog\""), "{page}");
}
