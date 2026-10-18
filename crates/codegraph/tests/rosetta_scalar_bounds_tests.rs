//! Scalar bounds persistence round-trip + garde validation un-dead tests
//! (rosetta gap-analysis defect #1).
//!
//! PropertyNode carries min_length/max_length/minimum/maximum and the JSON
//! ingest parses them, but the Grafeo Property INSERT never persisted them —
//! they rounded-tripped as None and the dto_create.tera garde range/length
//! branches were dead code end-to-end. These tests pin both ends of the fix:
//! graph-level round-trip and generated-output garde attributes.

use std::path::PathBuf;

use codegraph_backend::{BackendConfig, create_backend};
use rust_decimal::Decimal;

const WIDGET_JSON: &str = r#"
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Widget",
  "description": "A bounded widget",
  "type": "object",
  "properties": {
    "widgetId": { "type": "string" },
    "quantity": { "type": "integer", "minimum": 2, "maximum": 10 },
    "label": { "type": "string", "minLength": 3, "maxLength": 12 }
  },
  "required": ["widgetId", "quantity", "label"]
}
"#;

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.store]
label = "Store"
schema_dir = "store"
postgres_schema = "store"
entities = ["Widget"]
"#;

const CLASSIFIER_TOML: &str = "inline_enum_threshold = 20\n";

/// Layout matches production: `<root>/domains.toml` + `<root>/<domain>/json/*.json`.
fn write_schemas_fixture(root: &std::path::Path) -> (PathBuf, PathBuf, PathBuf) {
    let json_dir = root.join("store/json");
    std::fs::create_dir_all(&json_dir).unwrap();
    let domains = root.join("domains.toml");
    std::fs::write(&domains, DOMAINS_TOML).unwrap();
    let classifier = root.join("classifier.toml");
    std::fs::write(&classifier, CLASSIFIER_TOML).unwrap();
    std::fs::write(json_dir.join("Widget.json"), WIDGET_JSON).unwrap();
    (domains, classifier, root.to_path_buf())
}

#[tokio::test]
async fn scalar_bounds_round_trip_through_the_graph() {
    let dir = tempfile::tempdir().unwrap();
    let (_, classifier_path, schemas_root) = write_schemas_fixture(dir.path());

    let backend = create_backend(&BackendConfig::default()).await.unwrap();
    let classifier =
        codegraph_classifier::config::parse_classifier_config(&classifier_path).unwrap();
    let domain_config =
        codegraph_config::config::parse_domain_config(&dir.path().join("domains.toml")).unwrap();
    codegraph::ingest::async_ingest::ingest_schemas(
        backend.ingestor(),
        &schemas_root,
        &classifier,
        &std::collections::HashSet::new(),
        &codegraph_config::config::UiOverrideConfig::default(),
        &domain_config.defaults.type_suffix,
    )
    .await
    .unwrap();

    let props = backend.querier().get_properties("Widget").await.unwrap();
    let by_name: std::collections::HashMap<&str, &codegraph_core::types::PropertyNode> =
        props.iter().map(|p| (p.name.as_str(), p)).collect();

    let quantity = by_name["quantity"];
    assert_eq!(
        quantity.minimum,
        Some(Decimal::from(2)),
        "minimum must survive the round trip"
    );
    assert_eq!(
        quantity.maximum,
        Some(Decimal::from(10)),
        "maximum must survive the round trip"
    );
    assert_eq!(quantity.min_length, None);
    assert_eq!(quantity.max_length, None);

    let label = by_name["label"];
    assert_eq!(
        label.min_length,
        Some(3),
        "min_length must survive the round trip"
    );
    assert_eq!(
        label.max_length,
        Some(12),
        "max_length must survive the round trip"
    );
    assert_eq!(label.minimum, None);
    assert_eq!(label.maximum, None);

    // Absent bounds must stay None (not error) on a boundless property.
    let widget_id = by_name["widgetId"];
    assert_eq!(widget_id.min_length, None);
    assert_eq!(widget_id.max_length, None);
    assert_eq!(widget_id.minimum, None);
    assert_eq!(widget_id.maximum, None);
}

#[tokio::test]
async fn persisted_bounds_un_dead_the_garde_branches_in_dto_create() {
    let dir = tempfile::tempdir().unwrap();
    let (domains, classifier, schemas_root) = write_schemas_fixture(dir.path());
    let output = dir.path().join("generated");

    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: Some(&schemas_root),
        classifier: Some(&classifier),
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
    })
    .await
    .unwrap();

    let dto_create = walkdir::WalkDir::new(&output)
        .into_iter()
        .filter_map(|e| e.ok())
        .find(|e| e.file_type().is_file() && e.file_name().to_string_lossy() == "dto_create.rs")
        .unwrap_or_else(|| panic!("dto_create.rs missing under {}", output.display()));
    let content = std::fs::read_to_string(dto_create.path()).unwrap();

    assert!(
        content.contains("#[garde(range(min = 2, max = 10))]"),
        "integer bounds must emit a garde range check; content:\n{content}"
    );
    assert!(
        content.contains("#[garde(length(min = 3, max = 12))]"),
        "string bounds must emit a garde length check; content:\n{content}"
    );
}
