//! Tests for the `ui_testids` global generator (issue #468 W3): the
//! `ui/src/lib/testids.ts` projection of `ux::plan::ids`.

use codegraph::generate::ProjectConfig;
use codegraph::generate::traits::GlobalGenerator;
use codegraph::generate::ui::testids_gen::UiTestidsGenerator;
use codegraph_config::config::parse_domain_config_str;
use codegraph_core::mock::MockEngine;
use codegraph_core::types::{PropertyNode, SchemaNode};

fn schema(title: &str, table: &str, domain: &str) -> SchemaNode {
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
        rust_type_name: title.trim_end_matches("Type").into(),
        pg_table_name: table.into(),
        api_path_segment: table.into(),
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

fn config() -> codegraph_config::DomainConfig {
    parse_domain_config_str(
        "[defaults]\noperations = [\"create\", \"read\", \"update\", \"delete\", \"list\"]\n\n\
         [domains.hr]\nlabel = \"hr\"\nschema_dir = \"hr\"\npostgres_schema = \"hr\"\n\
         entities = [\"PersonType\", \"WorkerType\"]\n\n\
         [domains.hr.entity_config.PersonType]\nrole = \"root\"\n\n\
         [domains.hr.entity_config.WorkerType]\nrole = \"root\"\n",
    )
    .expect("fixture domains.toml must parse")
}

fn generate(project: &ProjectConfig) -> Vec<(String, String)> {
    let engine = MockEngine::builder()
        .with_schema(schema("PersonType", "person", "hr"))
        .with_schema(schema("WorkerType", "worker", "hr"))
        .with_properties("PersonType", vec![scalar("full_name", "TEXT", true)])
        .with_properties("WorkerType", vec![scalar("hire_date", "DATE", false)])
        .build();
    let output = tempfile::TempDir::new().unwrap();
    let generation_order = vec![
        codegraph::generate::GenerationEntry {
            schema_title: "PersonType".into(),
            domain: "hr".into(),
            pg_schema: "hr".into(),
            is_cyclic: false,
        },
        codegraph::generate::GenerationEntry {
            schema_title: "WorkerType".into(),
            domain: "hr".into(),
            pg_schema: "hr".into(),
            is_cyclic: false,
        },
    ];
    let tera = {
        let template_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
        codegraph::generate::template_engine::create_tera(&template_dir).unwrap()
    };
    let files = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(UiTestidsGenerator::new(output.path()).generate(
            &engine,
            &config(),
            &generation_order,
            &tera,
            project,
        ))
        .expect("UiTestidsGenerator failed");
    files
        .into_iter()
        .map(|f| (f.path.to_string_lossy().to_string(), f.content))
        .collect()
}

#[test]
fn testid_module_emits_typed_module_from_the_graph() {
    let files = generate(&ProjectConfig {
        codegen: codegraph::generate::CodegenConfig {
            testid_module: true,
            ..Default::default()
        },
        ..ProjectConfig::default()
    });
    let (_, module) = files
        .iter()
        .find(|(p, _)| p.ends_with("ui/src/lib/testids.ts"))
        .expect("testids.ts emitted");

    // Per-entity entries with the entity's own field unions.
    assert!(
        module.contains("person: entity('person', [\n    'full_name',\n  ] as const),"),
        "{module}"
    );
    assert!(
        module.contains("worker: entity('worker', [\n    'hire_date',\n  ] as const),"),
        "{module}"
    );
    // The normalized dialog vocabulary (issue #468 finding 3).
    assert!(
        module.contains("deleteDialog: `${m}-delete-dialog`"),
        "{module}"
    );
    assert!(
        module.contains("deleteDialogConfirm: `${m}-delete-dialog-confirm`"),
        "{module}"
    );
    assert!(!module.contains("confirm-confirm"), "{module}");
    // The shared table + the runtime accessor the POM's cross-entity
    // locators use.
    assert!(
        module.contains("confirmDialog: 'confirm-dialog'"),
        "{module}"
    );
    assert!(module.contains("export function tidFor("), "{module}");
}

#[test]
fn feature_off_emits_nothing() {
    let files = generate(&ProjectConfig::default());
    assert!(
        files.is_empty(),
        "flag-off must emit nothing, got: {files:?}"
    );
}
