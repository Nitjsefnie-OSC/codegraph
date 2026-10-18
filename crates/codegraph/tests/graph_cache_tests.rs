//! L1 driver-level graph cache (issue #275): with `--graph-cache`, a second
//! consecutive `run` with unchanged inputs reopens the persisted graph
//! instead of re-ingesting; changed inputs invalidate the cache.

use std::fs;
use std::path::{Path, PathBuf};

// ── L1: driver-level graph cache ─────────────────────────────────────────

const INVENTORY_MOX: &str = r#"
package inventory

class WidgetType {
    String [1] name
    refers WidgetType [0..1] replacement
}
"#;

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.inventory]
label = "Inventory"
schema_dir = "inventory"
postgres_schema = "inventory"
"#;

struct CacheFixture {
    root: tempfile::TempDir,
    cache: tempfile::TempDir,
    config: PathBuf,
    mox: PathBuf,
    mox_files: Vec<PathBuf>,
}

fn write_cache_fixture() -> CacheFixture {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("domains.toml"), DOMAINS_TOML).unwrap();
    fs::create_dir_all(root.path().join("model")).unwrap();
    fs::write(root.path().join("model/inventory.mox"), INVENTORY_MOX).unwrap();
    CacheFixture {
        config: root.path().join("domains.toml"),
        mox: root.path().join("model/inventory.mox"),
        mox_files: vec![root.path().join("model/inventory.mox")],
        root,
        cache: tempfile::tempdir().unwrap(),
    }
}

fn cache_run_args<'a>(fx: &'a CacheFixture, output: &'a Path) -> codegraph::driver::RunArgs<'a> {
    codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &fx.config,
        output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(fx.root.path().join("profiles.toml")),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &fx.mox_files,
        rosetta_files: &[],
        ddd_files: &[],
        evt_files: &[],
        deploy_files: &[],
        ifml_framework: &[],
        ifml_components: None,
        ifml_design_system: None,
        ux_rules: None,
        codegraph_rev: None,
        check: false,
    }
}

#[tokio::test]
async fn second_run_with_unchanged_inputs_skips_reingestion() {
    let fx = write_cache_fixture();
    let output = fx.root.path().join("generated");

    let first = codegraph::driver::run_with_graph_cache(
        cache_run_args(&fx, &output),
        Some(fx.cache.path()),
    )
    .await
    .unwrap();
    assert!(!first.graph_cache_reused, "first run must ingest fresh");
    let hash = first
        .inputs_hash
        .expect("inputs hash recorded on first run");
    assert_eq!(hash.len(), 64);
    assert!(fx.cache.path().join("inputs.sha256").exists());
    assert!(fx.cache.path().join("graph.grafeo").exists());

    let second = codegraph::driver::run_with_graph_cache(
        cache_run_args(&fx, &output),
        Some(fx.cache.path()),
    )
    .await
    .unwrap();
    assert!(second.graph_cache_reused, "second run must reuse the graph");
    assert_eq!(second.inputs_hash.as_deref(), Some(hash.as_str()));
}

#[tokio::test]
async fn changed_inputs_invalidates_the_graph_cache() {
    let fx = write_cache_fixture();
    let output = fx.root.path().join("generated");

    let first = codegraph::driver::run_with_graph_cache(
        cache_run_args(&fx, &output),
        Some(fx.cache.path()),
    )
    .await
    .unwrap();
    assert!(!first.graph_cache_reused);

    fs::write(
        &fx.mox,
        "package inventory\n\nclass WidgetType {\n    String [1] name\n    String [0..1] color\n}\n",
    )
    .unwrap();

    let second = codegraph::driver::run_with_graph_cache(
        cache_run_args(&fx, &output),
        Some(fx.cache.path()),
    )
    .await
    .unwrap();
    assert!(!second.graph_cache_reused, "changed inputs must re-ingest");
    assert_ne!(second.inputs_hash.as_deref(), first.inputs_hash.as_deref());
}
