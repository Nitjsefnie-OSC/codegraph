//! mox-first pipeline tests (issue #231): pass ordering, skip-covered
//! schema filter, and the model-source CLI contract.
//!
//! - mox-only run (no --schemas) generates output including DDL for mox
//!   classes;
//! - mixed runs let .mox win schema-title conflicts (covered JSON schemas
//!   are skipped, non-covered ones still ingest);
//! - `--schemas`-only runs are deprecated (stderr WARN), and providing
//!   neither model source is an error.

use std::fs;
use std::path::{Path, PathBuf};

use codegraph_core::traits::GraphQuerier;

const INVENTORY_MOX: &str = r#"
package inventory

class WidgetType {
    String [1] name
    int [0..1] mox_stock_level
    refers WidgetType [0..1] replacement
    contains WidgetPartType[] parts
}

class WidgetPartType {
    String [1] label
}
"#;

/// JSON schema sharing WidgetType's title with a property the .mox class
/// does not have — a discriminator for which source won the conflict.
const WIDGET_TYPE_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "WidgetType",
  "type": "object",
  "properties": {
    "id": { "description": "Primary identifier.", "type": "string", "format": "uuid" },
    "json_only_column": { "description": "Only in the JSON version.", "type": "string" }
  },
  "required": ["id"]
}"#;

const GADGET_TYPE_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "GadgetType",
  "type": "object",
  "properties": {
    "id": { "description": "Primary identifier.", "type": "string", "format": "uuid" },
    "gadget_label": { "description": "Gadget label.", "type": "string" }
  },
  "required": ["id"]
}"#;

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.inventory]
label = "Inventory"
schema_dir = "inventory"
postgres_schema = "inventory"
"#;

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
    mox: PathBuf,
    schemas: PathBuf,
}

fn write_fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("domains.toml"), DOMAINS_TOML).unwrap();
    fs::create_dir_all(root.path().join("model")).unwrap();
    fs::write(root.path().join("model/inventory.mox"), INVENTORY_MOX).unwrap();
    fs::create_dir_all(root.path().join("schemas/inventory")).unwrap();
    fs::write(
        root.path().join("schemas/inventory/WidgetType.json"),
        WIDGET_TYPE_JSON,
    )
    .unwrap();
    fs::write(
        root.path().join("schemas/inventory/GadgetType.json"),
        GADGET_TYPE_JSON,
    )
    .unwrap();
    Fixture {
        config: root.path().join("domains.toml"),
        mox: root.path().join("model/inventory.mox"),
        schemas: root.path().join("schemas"),
        root,
    }
}

impl Fixture {
    fn out(&self) -> PathBuf {
        self.root.path().join("generated")
    }
}

#[allow(clippy::too_many_arguments)]
fn run_args<'a>(
    fixture: &'a Fixture,
    schemas: Option<&'a Path>,
    classifier: Option<&'a Path>,
    mox_files: &'a [PathBuf],
    output: &'a Path,
) -> codegraph::driver::RunArgs<'a> {
    codegraph::driver::RunArgs {
        schemas,
        classifier,
        config_path: &fixture.config,
        output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        // Points at a nonexistent path so the no-plan (all generators)
        // backward-compat path runs.
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

/// Locate the main DDL migration for an inventory table (files carry a
/// sequence prefix, e.g. `000542_inventory_widget.sql`).
fn migration(table: &str, output: &Path) -> PathBuf {
    let dir = output.join("migrations");
    fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(&format!("inventory_{table}.sql")))
        })
        .unwrap_or_else(|| panic!("no migration for inventory.{table} under {}", dir.display()))
}

/// (a) mox-only run: no --schemas, no classifier — DDL is generated for the
/// mox-authored entity class.
#[tokio::test]
async fn mox_only_run_generates_ddl_for_mox_class() {
    let fixture = write_fixture();
    let output = fixture.out();
    let mox_files = vec![fixture.mox.clone()];

    codegraph::driver::run(run_args(&fixture, None, None, &mox_files, &output))
        .await
        .unwrap();

    let widget = fs::read_to_string(migration("widget", &output)).unwrap();
    assert!(widget.contains("CREATE TABLE"), "{widget}");
    assert!(widget.contains("mox_stock_level"), "{widget}");
}

/// (b) mixed run: the .mox-authored WidgetType wins the title conflict —
/// the covered JSON schema is skipped — while the non-covered GadgetType
/// JSON schema still ingests.
#[tokio::test]
async fn mixed_run_mox_wins_conflicts_and_json_fills_gaps() {
    let fixture = write_fixture();
    let output = fixture.out();
    let mox_files = vec![fixture.mox.clone()];

    codegraph::driver::run(run_args(
        &fixture,
        Some(&fixture.schemas),
        Some(Path::new("tests/fixtures/classifier.toml")),
        &mox_files,
        &output,
    ))
    .await
    .unwrap();

    // Widget table carries the mox-authored column, not the JSON-only one.
    let widget = fs::read_to_string(migration("widget", &output)).unwrap();
    assert!(widget.contains("mox_stock_level"), "{widget}");
    assert!(
        !widget.contains("json_only_column"),
        "JSON-authored WidgetType must be skipped: {widget}"
    );

    // Non-covered JSON schema still ingests and generates.
    let gadget = fs::read_to_string(migration("gadget", &output)).unwrap();
    assert!(gadget.contains("gadget_label"), "{gadget}");
}

/// (b) at the ingest layer: the skip-set removes covered titles from the
/// schema pass — the graph keeps exactly one WidgetType, sourced from mox.
#[tokio::test]
async fn skip_set_keeps_single_mox_sourced_schema_and_non_covered_json() {
    let fixture = write_fixture();
    let engine = codegraph_grafeo::GrafeoEngine::in_memory().unwrap();
    let config = codegraph_config::config::parse_domain_config(&fixture.config).unwrap();
    let mox_files = vec![fixture.mox.clone()];

    let stats = codegraph::ingest::mox_ingest::ingest_mox_files(
        &engine,
        &engine,
        &mox_files,
        &config,
        &config.defaults.type_suffix,
    )
    .await
    .unwrap()
    .stats;
    assert_eq!(stats.bridged_titles.len(), 2);
    let skip: std::collections::HashSet<String> = stats.bridged_titles.into_iter().collect();

    let classifier = codegraph_classifier::config::parse_classifier_config(Path::new(
        "tests/fixtures/classifier.toml",
    ))
    .unwrap();
    codegraph::ingest::async_ingest::ingest_schemas_with_skips(
        &engine,
        &fixture.schemas,
        &classifier,
        &std::collections::HashSet::new(),
        &codegraph_config::UiOverrideConfig::default(),
        &config.defaults.type_suffix,
        &skip,
    )
    .await
    .unwrap();

    let schemas = engine.list_schemas(None).await.unwrap();
    let widgets: Vec<_> = schemas.iter().filter(|s| s.title == "WidgetType").collect();
    assert_eq!(widgets.len(), 1, "no duplicate WidgetType: {schemas:?}");
    assert_eq!(
        widgets[0]
            .custom_annotations
            .get("source")
            .and_then(|v| v.as_str()),
        Some("mox"),
        "the mox-authored node wins"
    );

    let gadget = schemas.iter().find(|s| s.title == "GadgetType").unwrap();
    assert_ne!(
        gadget
            .custom_annotations
            .get("source")
            .and_then(|v| v.as_str()),
        Some("mox"),
        "non-covered JSON schema ingests normally"
    );
}

/// (c) the deprecation warning text is exact.
#[test]
fn schemas_only_deprecation_warning_text() {
    assert_eq!(
        codegraph::driver::schemas_deprecation_notice(),
        "WARN: --schemas is deprecated as the primary model source. Migrate to .mox files: codegraph migrate --schemas <dir> --output <dir>"
    );
}

/// (c) the both-sources notice text.
#[test]
fn mixed_sources_info_notice_text() {
    let notice = codegraph::driver::mox_primary_notice();
    assert!(notice.starts_with("INFO: .mox files are the primary model source"));
    assert!(notice.contains("--schemas fills gaps"));
}

/// (d) neither --schemas nor --mox-files is an error.
#[tokio::test]
async fn run_without_model_source_is_an_error() {
    let fixture = write_fixture();
    let output = fixture.out();

    let err = codegraph::driver::run(run_args(&fixture, None, None, &[], &output))
        .await
        .unwrap_err();
    let message = format!("{err}");
    assert!(message.contains("--mox-files"), "{message}");
    assert!(message.contains("--schemas"), "{message}");
}

/// (d) --schemas without --classifier is an error.
#[tokio::test]
async fn run_with_schemas_but_no_classifier_is_an_error() {
    let fixture = write_fixture();
    let output = fixture.out();

    let err = codegraph::driver::run(run_args(
        &fixture,
        Some(&fixture.schemas),
        None,
        &[],
        &output,
    ))
    .await
    .unwrap_err();
    assert!(
        format!("{err}").contains("--classifier is required when --schemas is provided"),
        "{err}"
    );
}
