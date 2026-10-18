//! Tests for the `ops` global generator.
//!
//! Verifies the generated `codegraph-ops.toml` manifest and `testkit/` crate
//! are produced by the full generation pipeline and that the manifest
//! serializes back into the `OpsManifest` type used by the codegraph-ops
//! harness.

#[path = "test_framework/mod.rs"]
mod test_framework;

use std::path::Path;

use codegraph::generate::traits::GlobalGenerator;
use codegraph_core::types::{PropertyNode, SchemaNode};
use test_framework::GeneratorTest;
use test_framework::validators::file_presence::FilePresenceValidator;
use test_framework::validators::string_pattern::StringPatternValidator;

/// Minimal mock engine + config, same pattern as profile_smoke_tests.
/// No `depends_on` so the domain registry stays acyclic with one domain.
fn mock_test_setup() -> (
    codegraph_core::mock::MockEngine,
    codegraph_config::DomainConfig,
    tera::Tera,
    tempfile::TempDir,
) {
    let schema = SchemaNode {
        namespace: None,
        schema_id: "recruiting/json/CandidateType.json".to_string(),
        title: "CandidateType".to_string(),
        description: Some("A candidate for a position".to_string()),
        schema_type: "object".to_string(),
        classification: "entity_reference".to_string(),
        domain: Some("recruiting".to_string()),
        rel_path: "recruiting/json/CandidateType.json".to_string(),
        pg_type: "UUID".to_string(),
        rust_type: "Uuid".to_string(),
        sea_orm_type: "Uuid".to_string(),
        rust_type_name: "Candidate".to_string(),
        pg_table_name: "candidate".to_string(),
        api_path_segment: "candidates".to_string(),
        parent_schema: None,
        is_entity: true,
        is_codelist: false,
        is_primitive_wrapper: false,
        has_all_of: true,
        has_one_of: false,
        has_any_of: false,
        has_definitions: true,
        custom_annotations: Default::default(),
        access: None,
        annotations: None,
    };

    let props = vec![PropertyNode {
        is_id: false,
        name: "givenName".to_string(),
        prop_type: "string".to_string(),
        description: Some("First name".to_string()),
        format: None,
        is_required: true,
        is_nullable: false,
        is_array: false,
        min_items: None,
        max_items: None,
        pattern: None,
        min_length: None,
        max_length: None,
        minimum: None,
        maximum: None,
        pg_column_name: "given_name".to_string(),
        pg_column_type: "TEXT".to_string(),
        rust_field_name: "given_name".to_string(),
        rust_field_type: "String".to_string(),
        sea_orm_type: "Text".to_string(),
        render_strategy: "direct_column".to_string(),
        ref_target: None,
        classification: Some("primitive_wrapper".to_string()),
        projection: None,
        classification_kind: None,
        ui_override_detail: None,
        ui_override_list_cell: None,
        ui_override_form: None,
        ui_override_inline: None,
        type_expr: None,
    }];

    let engine = codegraph_core::mock::MockEngine::builder()
        .with_schema(schema)
        .with_properties("CandidateType", props)
        .build();

    let config = codegraph_config::config::parse_domain_config_str(
        r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.recruiting]
label = "Recruiting"
schema_dir = "recruiting"
postgres_schema = "recruiting"
entities = ["CandidateType"]

[domains.recruiting.entity_config.CandidateType]
operations = ["create", "read", "update", "delete", "list"]
"#,
    )
    .unwrap();

    let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    let tera = codegraph::generate::template_engine::create_tera(&template_dir).unwrap();

    let output_dir = tempfile::TempDir::new().unwrap();

    (engine, config, tera, output_dir)
}

/// The full pipeline (no build plan → all generators) must produce the ops
/// manifest plus the testkit crate, with manifest content present.
#[test]
fn ops_generator_produces_manifest_and_testkit_in_full_pipeline() {
    let (engine, config, tera, output_dir) = mock_test_setup();

    let test = GeneratorTest {
        db: &engine,
        config: &config,
        tera: &tera,
        output_dir: output_dir.path(),
        validators: vec![
            Box::new(FilePresenceValidator {
                label: "ops_check".to_string(),
                required_paths: vec![
                    "codegraph-ops.toml".to_string(),
                    "testkit/Cargo.toml".to_string(),
                    "testkit/src/main.rs".to_string(),
                ],
            }),
            Box::new(StringPatternValidator {
                label: "ops_manifest_content".to_string(),
                required_patterns: vec![
                    "app_name".to_string(),
                    "capabilities".to_string(),
                    "output_dir = \".\"".to_string(),
                ],
                forbidden_patterns: vec![],
            }),
        ],
    };

    let files = test.run().expect("generation failed");
    assert!(!files.is_empty(), "pipeline should produce files");

    let manifest = files
        .iter()
        .find(|f| f.path == Path::new("codegraph-ops.toml"))
        .expect("codegraph-ops.toml should be collected");
    assert!(
        manifest.content.contains("DO NOT EDIT"),
        "manifest should carry the DO NOT EDIT header"
    );
}

/// Direct generator call: the manifest must round-trip through the
/// `OpsManifest` serde types used by the codegraph-ops harness.
#[tokio::test]
async fn ops_generator_direct_call_manifest_roundtrips() {
    let (engine, config, tera, output_dir) = mock_test_setup();

    let generator = codegraph::generate::ops::OpsManifestGenerator::new(
        output_dir.path(),
        true, // has_cli
        true, // has_ui
        true, // has_admin_cli
        true, // has_grpc
    );
    let files = generator
        .generate(
            &engine,
            &config,
            &[],
            &tera,
            &codegraph::generate::ProjectConfig::default(),
        )
        .await
        .expect("ops generator failed");

    assert_eq!(files.len(), 3, "manifest + testkit Cargo.toml + main.rs");

    let manifest = files
        .iter()
        .find(|f| f.path.ends_with("codegraph-ops.toml"))
        .expect("codegraph-ops.toml");
    let parsed: codegraph_config::ops_manifest::OpsManifest =
        toml::from_str(&manifest.content).expect("manifest should parse as OpsManifest");

    assert_eq!(parsed.app_name, "app");
    assert_eq!(parsed.output_dir, Path::new("."));
    assert_eq!(parsed.servers.api_port, 3000);
    assert_eq!(parsed.servers.ui_port, 5173);
    assert_eq!(parsed.servers.bind_addr, "0.0.0.0");
    assert_eq!(parsed.database.api.host, "localhost");
    assert_eq!(parsed.database.api.port, 5432);
    let e2e = parsed.database.e2e.as_ref().expect("e2e db target");
    assert_eq!(e2e.port, 54322);
    assert!(parsed.database.e2e_app.is_none());
    assert!(parsed.supabase.is_none());
    assert!(parsed.hurl.is_none());
    assert!(parsed.hooks.is_empty());
    assert!(parsed.extensions.is_empty());
    assert!(parsed.capabilities.has_cli);
    assert!(parsed.capabilities.has_ui);
    assert!(parsed.capabilities.has_admin_cli);
    assert!(parsed.capabilities.has_grpc);
    assert_eq!(parsed.capabilities.database_target, "postgres");
    assert_eq!(parsed.capabilities.persistence_provider, "sea_orm");
    // rosetta_files mirrors mox_files on the generated manifest: present in
    // the type (issue #260) and empty here — the init scaffold's
    // ops_manifest.tera seeds the per-domain entries.
    assert!(parsed.mox_files.is_empty());
    assert!(parsed.rosetta_files.is_empty());

    let cargo = files
        .iter()
        .find(|f| f.path.ends_with("testkit/Cargo.toml"))
        .expect("testkit/Cargo.toml");
    assert!(
        cargo.content.contains("codegraph-ops"),
        "testkit Cargo.toml should depend on codegraph-ops. Got:\n{}",
        cargo.content
    );
    assert!(
        cargo.content.contains("tokio"),
        "testkit Cargo.toml should depend on tokio"
    );

    let main = files
        .iter()
        .find(|f| f.path.ends_with("testkit/src/main.rs"))
        .expect("testkit/src/main.rs");
    assert!(
        main.content.contains("codegraph_ops"),
        "testkit main.rs should reference codegraph_ops. Got:\n{}",
        main.content
    );
}

/// The testkit Cargo.toml must pin the codegraph-ops dependency to the
/// codegraph git rev when `project.cargo.codegraph_rev` is set (external consumers
/// depend on codegraph crates via git, so the relative path fallback would
/// not exist in their repo).
#[tokio::test]
async fn testkit_cargo_uses_git_rev_when_pinned() {
    let (engine, config, tera, output_dir) = mock_test_setup();

    let generator = codegraph::generate::ops::OpsManifestGenerator::new(
        output_dir.path(),
        true, // has_cli
        true, // has_ui
        true, // has_admin_cli
        true, // has_grpc
    );
    let project = codegraph::generate::ProjectConfig {
        cargo: codegraph::generate::CargoConfig {
            codegraph_rev: "abc123".into(),
            ..Default::default()
        },
        ..codegraph::generate::ProjectConfig::default()
    };
    let files = generator
        .generate(&engine, &config, &[], &tera, &project)
        .await
        .expect("ops generator failed");

    let cargo = files
        .iter()
        .find(|f| f.path.ends_with("testkit/Cargo.toml"))
        .expect("testkit/Cargo.toml");
    assert!(
        cargo
            .content
            .contains(r#"git = "https://github.com/magick93/codegraph.git""#),
        "testkit Cargo.toml should pin codegraph-ops via git. Got:\n{}",
        cargo.content
    );
    assert!(
        cargo.content.contains(r#"rev = "abc123""#),
        "testkit Cargo.toml should pin the codegraph rev. Got:\n{}",
        cargo.content
    );
    assert!(
        !cargo.content.contains("path ="),
        "testkit Cargo.toml should not use a path dependency when a rev is pinned. Got:\n{}",
        cargo.content
    );
}

/// With an empty `project.cargo.codegraph_rev` (the default), the testkit Cargo.toml
/// falls back to the path dependency into the codegraph workspace.
#[tokio::test]
async fn testkit_cargo_uses_path_when_rev_empty() {
    let (engine, config, tera, output_dir) = mock_test_setup();

    let generator = codegraph::generate::ops::OpsManifestGenerator::new(
        output_dir.path(),
        true, // has_cli
        true, // has_ui
        true, // has_admin_cli
        true, // has_grpc
    );
    let files = generator
        .generate(
            &engine,
            &config,
            &[],
            &tera,
            &codegraph::generate::ProjectConfig::default(),
        )
        .await
        .expect("ops generator failed");

    let cargo = files
        .iter()
        .find(|f| f.path.ends_with("testkit/Cargo.toml"))
        .expect("testkit/Cargo.toml");
    assert!(
        cargo.content.contains("path ="),
        "testkit Cargo.toml should use a path dependency when no rev is pinned. Got:\n{}",
        cargo.content
    );
    assert!(
        !cargo.content.contains("rev ="),
        "testkit Cargo.toml should not pin a git rev when codegraph_rev is empty. Got:\n{}",
        cargo.content
    );
}

/// The emitted testkit crate must actually compile. The codegraph-ops path
/// dependency is rewritten to an absolute path so the test is hermetic
/// regardless of where the tempdir lives (the generated relative path only
/// resolves when the output sits inside the codegraph workspace).
#[tokio::test]
#[ignore = "slow: compiles codegraph-ops; run in CI"]
async fn testkit_crate_compiles() {
    let (engine, config, tera, output_dir) = mock_test_setup();

    let generator = codegraph::generate::ops::OpsManifestGenerator::new(
        output_dir.path(),
        true, // has_cli
        true, // has_ui
        true, // has_admin_cli
        true, // has_grpc
    );
    let files = generator
        .generate(
            &engine,
            &config,
            &[],
            &tera,
            &codegraph::generate::ProjectConfig::default(),
        )
        .await
        .expect("ops generator failed");

    let manifest = files
        .iter()
        .find(|f| f.path.ends_with("codegraph-ops.toml"))
        .expect("codegraph-ops.toml");
    let cargo = files
        .iter()
        .find(|f| f.path.ends_with("testkit/Cargo.toml"))
        .expect("testkit/Cargo.toml");
    let main = files
        .iter()
        .find(|f| f.path.ends_with("testkit/src/main.rs"))
        .expect("testkit/src/main.rs");

    let ops_abs = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("codegraph-ops"))
        .expect("codegraph workspace crates dir");
    let ops_abs = ops_abs
        .canonicalize()
        .expect("codegraph-ops crate should exist in the workspace");

    let cargo_content = cargo
        .content
        .lines()
        .map(|line| {
            if line.starts_with("codegraph-ops") {
                format!("codegraph-ops = {{ path = \"{}\" }}", ops_abs.display())
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let root = output_dir.path();
    std::fs::write(root.join("codegraph-ops.toml"), &manifest.content).expect("write manifest");
    std::fs::create_dir_all(root.join("testkit").join("src")).expect("mkdir testkit/src");
    std::fs::write(root.join("testkit").join("Cargo.toml"), &cargo_content)
        .expect("write Cargo.toml");
    std::fs::write(
        root.join("testkit").join("src").join("main.rs"),
        &main.content,
    )
    .expect("write main.rs");

    let status = std::process::Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(root.join("testkit").join("Cargo.toml"))
        .arg("--target-dir")
        .arg(root.join("target"))
        .status()
        .expect("spawn cargo build");
    assert!(status.success(), "cargo build of generated testkit failed");
}

/// The manifest should seed a smoke entity from the generation order so a
/// fresh manifest exercises the api suite out of the box.
#[tokio::test]
async fn ops_generator_seeds_smoke_from_generation_order() {
    let (engine, config, tera, output_dir) = mock_test_setup();

    let generator = codegraph::generate::ops::OpsManifestGenerator::new(
        output_dir.path(),
        true,
        true,
        true,
        true,
    );
    let order = vec![codegraph::generate::GenerationEntry {
        schema_title: "CandidateType".to_string(),
        domain: "recruiting".to_string(),
        pg_schema: "recruiting".to_string(),
        is_cyclic: false,
    }];
    let files = generator
        .generate(
            &engine,
            &config,
            &order,
            &tera,
            &codegraph::generate::ProjectConfig::default(),
        )
        .await
        .expect("ops generator failed");

    let manifest = files
        .iter()
        .find(|f| f.path.ends_with("codegraph-ops.toml"))
        .expect("codegraph-ops.toml");
    let parsed: codegraph_config::ops_manifest::OpsManifest =
        toml::from_str(&manifest.content).expect("manifest should parse");

    let smoke = parsed.smoke.expect("smoke should be seeded");
    assert_eq!(smoke.entity, "recruiting/candidate");
    assert_eq!(smoke.create_body, "{}");
}

/// Capability flags must mirror the build plan: only the grpc scaffold flag
/// is off when grpc is not in the plan.
#[tokio::test]
async fn ops_generator_capability_flags_reflect_constructor() {
    let (engine, config, tera, output_dir) = mock_test_setup();

    let generator = codegraph::generate::ops::OpsManifestGenerator::new(
        output_dir.path(),
        false, // has_cli
        false, // has_ui
        false, // has_admin_cli
        false, // has_grpc
    );
    let files = generator
        .generate(
            &engine,
            &config,
            &[],
            &tera,
            &codegraph::generate::ProjectConfig::default(),
        )
        .await
        .expect("ops generator failed");

    let manifest = files
        .iter()
        .find(|f| f.path.ends_with("codegraph-ops.toml"))
        .expect("codegraph-ops.toml");
    let parsed: codegraph_config::ops_manifest::OpsManifest =
        toml::from_str(&manifest.content).expect("manifest should parse");

    assert!(!parsed.capabilities.has_cli);
    assert!(!parsed.capabilities.has_ui);
    assert!(!parsed.capabilities.has_admin_cli);
    assert!(!parsed.capabilities.has_grpc);
}

/// End-to-end contract: the emitted manifest must load through the harness's
/// own `OpsConfig` (paths resolved, db targets wrapped, api URL derived).
#[tokio::test]
async fn emitted_manifest_loads_through_ops_config() {
    let (engine, config, tera, output_dir) = mock_test_setup();

    let generator = codegraph::generate::ops::OpsManifestGenerator::new(
        output_dir.path(),
        true,
        true,
        true,
        true,
    );
    let files = generator
        .generate(
            &engine,
            &config,
            &[],
            &tera,
            &codegraph::generate::ProjectConfig::default(),
        )
        .await
        .expect("ops generator failed");

    // Write the emitted manifest to disk so OpsConfig::load can read it.
    let manifest_path = output_dir.path().join("codegraph-ops.toml");
    let manifest = files
        .iter()
        .find(|f| f.path.ends_with("codegraph-ops.toml"))
        .expect("codegraph-ops.toml");
    std::fs::write(&manifest_path, &manifest.content).expect("write manifest");

    let cfg = codegraph_ops::OpsConfig::load(&manifest_path).expect("OpsConfig::load");

    assert_eq!(cfg.app_binary_name(), "app");
    assert_eq!(cfg.api_url(), "http://localhost:3000");
    assert_eq!(cfg.api_db.port, 5432);
    assert!(cfg.e2e_db.is_some());
    assert!(cfg.e2e_app_db.is_none());
    assert!(cfg.supabase_dir.is_none());
    assert!(cfg.hurl_dir.is_none());
    assert!(cfg.hooks.is_empty());
}

// ---------------------------------------------------------------------------
// `run --check` / `generate --check` post-generate compile gate (issue #336)
// ---------------------------------------------------------------------------

/// `--check` plumbing: RunArgs carries the flag, defaults to false, and the
/// flag is settable through to the driver. Clap-level parsing (`--check`
/// accepted on `run` and `generate`) is exercised by the CLI `--help` smoke
/// check; this pins the driver-level contract without a full pipeline run.
#[test]
fn check_flag_plumbs_through_run_args() {
    let dir = tempfile::TempDir::new().unwrap();
    let config_path = dir.path().join("domains.toml");
    let template_dirs: Vec<std::path::PathBuf> = Vec::new();
    let ifml_files: Vec<std::path::PathBuf> = Vec::new();

    let args = codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &config_path,
        output: dir.path(),
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: None,
        no_post_gen: false,
        template_dir: &template_dirs,
        ifml_files: &ifml_files,
        openapi_files: &[],
        mox_files: &[],
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
    };
    assert!(!args.check, "--check must default to false on RunArgs");

    let flagged = codegraph::driver::RunArgs {
        check: true,
        ..args
    };
    assert!(flagged.check, "--check must be settable on RunArgs");
}

/// The gate function: a compiling crate passes; corrupting one generated
/// .rs file makes it fail with the rustc error surfaced in the message.
///
/// Always-on (NOT `#[ignore]`d): the fixture is a zero-dependency crate, so
/// `cargo check` never touches the registry or the network — it only needs
/// the locally installed toolchain (resolved via `$CARGO` under `cargo
/// test`, else `$PATH`).
#[test]
fn check_gate_fails_non_zero_on_broken_output() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"cg_check_gate_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    let main_rs = dir.path().join("src").join("main.rs");
    std::fs::write(&main_rs, "fn main() { println!(\"ok\"); }\n").unwrap();

    // Good output passes the gate.
    codegraph::driver::check_generated_output(dir.path())
        .expect("clean crate must pass the compile gate");

    // Corrupt the generated .rs file: the gate must fail and surface the
    // rustc diagnostic.
    std::fs::write(
        &main_rs,
        "fn main() { let _broken: u32 = \"not a number\"; }\n",
    )
    .unwrap();
    let err = codegraph::driver::check_generated_output(dir.path())
        .expect_err("broken crate must fail the compile gate");
    let msg = err.to_string();
    assert!(
        msg.contains("cargo check"),
        "failure must name the gate command: {msg}"
    );
    assert!(
        msg.contains("mismatched types"),
        "rustc error must be surfaced in the failure: {msg}"
    );
}

/// Full-pipeline e2e: `run` over a mox fixture generates an app that passes
/// the gate, and corrupting a generated .rs file afterwards makes the gate
/// (invoked through the driver plumbing) fail on the real generated tree.
/// Modeled on `testkit_crate_compiles`; the flag-off side (no cargo
/// invocation) is pinned by `check_flag_plumbs_through_run_args` (default
/// false) plus the `if check` gate in the driver.
///
/// Uses a minimal real profile with `dependency_strategy = "path"` so the
/// generated app resolves codegraph deps into this checkout (offline);
/// plan-less runs are NOT checkable today (they emit an incoherent root
/// Cargo.toml — see the report on issue #336).
#[tokio::test]
#[ignore = "slow: full generate + cargo check of the generated app tree (builds the full dep tree; needs a warm cargo cache)"]
async fn check_gate_passes_on_good_output() {
    let dir = tempfile::TempDir::new().unwrap();
    let mox = dir.path().join("model.mox");
    let config = dir.path().join("domains.toml");
    let profiles = dir.path().join("profiles.toml");
    // Absolute path into this checkout so the generated domain-types crate
    // resolves codegraph-type-contracts as an offline path dep (same trick
    // as `testkit_crate_compiles`).
    let type_contracts = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(|p| p.join("crates/codegraph-type-contracts"))
        .expect("codegraph workspace crates dir");
    std::fs::write(
        &mox,
        "package common\n\n/// A named collection of todo items.\nclass TodoListType {\n    String name\n}\n",
    )
    .unwrap();
    std::fs::write(
        &config,
        "[domains.common]\nlabel = \"Common\"\nschema_dir = \"common\"\npostgres_schema = \"common\"\n",
    )
    .unwrap();
    std::fs::write(
        &profiles,
        format!(
            r#"[profiles.default.meta]
name = "default"
version = "0.1.0"
description = "Minimal check-gate fixture profile"
app_name = "app"
generator_name = "app-graph"
api_title = "App API"
domain_types_crate = "app_domain_types"
domain_types_base = "crates/domain-types"
type_contracts_base = "{}"

[profiles.default.features]
database_target = "postgres"
persistence_provider = "sea_orm"
deployment_topology = "monolith"
dependency_strategy = "path"
ops_backend = false
grpc_backend = false
ifml_backend = false
has_admin_cli = true
ux_rules = true

[profiles.default.api]
generators = [
    "ddl", "sea_orm_entity", "codelist", "dto", "repository", "command",
    "query", "event", "handler", "workflow_action", "media_route", "test",
    "lifecycle_trait", "domain_types_dto", "domain_types_query_service",
    "router", "links", "errors",
    "openapi", "scaffold", "basejump_setup", "pgmq_setup",
    "platform_schema", "workflow_seed", "hook_registry", "domain_types_scaffold",
    "report_views",
    "webhook_dispatch", "webhook_endpoint_api",
]
output = "generated/"
scripts.post_gen = []

[profiles.default.cli]
generators = ["cli_command", "cli_domain", "cli_scaffold"]
output = "generated/"
"#,
            type_contracts.display()
        ),
    )
    .unwrap();

    let output = dir.path().join("generated");
    let mox_files = vec![mox.clone()];
    let template_dirs: Vec<std::path::PathBuf> = Vec::new();

    // Good output + gate on: the run must pass end to end.
    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &config,
        output: &output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(profiles.clone()),
        no_post_gen: false,
        template_dir: &template_dirs,
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &mox_files,
        rosetta_files: &[],
        ddd_files: &[],
        evt_files: &[],
        deploy_files: &[],
        ifml_framework: &[],
        ifml_components: None,
        ifml_design_system: None,
        ux_rules: None,
        codegraph_rev: None,
        check: true,
    })
    .await
    .expect("run with --check must pass on good output");
    assert!(
        output.join("Cargo.toml").exists(),
        "the fixture must generate a Cargo.toml for the gate to check"
    );

    // Corrupt one generated .rs file: the gate must now fail with the rustc
    // error surfaced. Prefer the crate entrypoint — some emitted .rs files
    // are not reachable from the module tree and cargo never compiles them.
    let main_rs = output.join("src").join("main.rs");
    let lib_rs = if main_rs.exists() {
        main_rs
    } else {
        std::fs::read_dir(output.join("src"))
            .expect("generated src dir")
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|ext| ext == "rs"))
            .expect("at least one generated .rs file")
    };
    let original = std::fs::read_to_string(&lib_rs).unwrap();
    std::fs::write(
        &lib_rs,
        format!("{original}\nfn __corrupted() {{ let _x: u32 = \"no\"; }}\n"),
    )
    .unwrap();
    let err = codegraph::driver::check_generated_output(&output)
        .expect_err("gate must fail on corrupted generated code");
    assert!(
        err.to_string().contains("mismatched types"),
        "rustc error must be surfaced: {err}"
    );
}

/// Rosetta-first contract (issue #260): a manifest carrying rosetta_files
/// parses through the harness's `OpsConfig::load` and preserves the
/// per-domain model list in manifest order.
#[test]
fn ops_manifest_rosetta_files_roundtrip_through_ops_config() {
    let raw = r#"
app_name = "demo-app"
rosetta_files = ["model/common.rosetta", "model/billing.rosetta"]
domain_config = "domains.toml"
output_dir = "generated"

[servers]
api_port = 3000
ui_port = 5173
bind_addr = "0.0.0.0"

[database.api]
host = "localhost"
port = 5432
user = "postgres"
password = "postgres"
database = "postgres"
"#;
    let dir = tempfile::TempDir::new().unwrap();
    let manifest_path = dir.path().join("codegraph-ops.toml");
    std::fs::write(&manifest_path, raw).unwrap();

    let cfg = codegraph_ops::OpsConfig::load(&manifest_path).expect("OpsConfig::load");
    assert_eq!(
        cfg.manifest.rosetta_files,
        vec![
            "model/common.rosetta".to_string(),
            "model/billing.rosetta".to_string()
        ]
    );
    assert!(cfg.manifest.mox_files.is_empty());
}
