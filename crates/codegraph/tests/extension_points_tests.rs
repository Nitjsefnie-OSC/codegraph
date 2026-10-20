//! Push-integration plane tests (issue #503).
//!
//! `--extension-points` generates the full integration infrastructure —
//! catalog/tables/config structs and the `IntegrationDispatcher` — but
//! nothing ever called `notify`, so a catalog-registered integration
//! worker never learned that anything happened. This suite pins:
//!
//! - command-layer notify call sites for covered entities (create/update/
//!   delete), gated on the parsed extension-points config so absent config
//!   ⇒ byte-identical output
//! - the documented POST payload shape (`{event, entity_id, org_id,
//!   installation_id, correlation_id}`, `event = "{entity_type}.{event}"`,
//!   entity_type = the full `domain.table` ref the installations SQL
//!   matches against `ep.allowed_entities`)
//! - the dispatcher's topology/provider legs: SeaORM + reqwest (monolith),
//!   cornucopia `ClientSource` + `GenericClient` (cornucopia monolith and
//!   workers), `worker::Fetch` (wasm32 workers)
//!
//! The closed event vocabulary is `created` / `updated` / `deleted` /
//! `state_changed`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use codegraph_ext_points::parse_extension_points_str;

use codegraph_grafeo::GrafeoEngine;

/// Extension point covering the fixture entity (CandidateType → table
/// `candidate` in domain `recruiting`), push direction — the shape the
/// command-layer call sites key on.
const EXT_POINTS_TOML: &str = r#"
[payroll-sync]
name = "Payroll Sync"
description = "Synchronise employee and pay data"
cardinality = "exclusive"
entities = ["recruiting.candidate"]
directions = ["push"]
"#;

/// Ingest the fixture JSON schemas (schema entities must exist for the
/// command generator).
async fn ingest_fixture_schemas(
    engine: &GrafeoEngine,
    config: &codegraph_config::config::DomainConfig,
) {
    let classifier = codegraph_classifier::config::parse_classifier_config(Path::new(
        "tests/fixtures/classifier.toml",
    ))
    .unwrap();
    let entity_names: std::collections::HashSet<String> = config
        .domains
        .values()
        .flat_map(|d| d.entities.iter().cloned())
        .collect();
    codegraph::ingest::async_ingest::ingest_schemas(
        engine,
        Path::new("tests/fixtures/schemas"),
        &classifier,
        &entity_names,
        &codegraph_config::UiOverrideConfig::default(),
        &config.defaults.type_suffix,
    )
    .await
    .unwrap();
}

fn profiles_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .join("profiles.toml")
}

/// Run the full generator suite (minus gRPC) over the fixture graph.
async fn run_generation(
    engine: &GrafeoEngine,
    ext_points: Option<&codegraph_ext_points::ExtensionPointsConfig>,
    output_dir: &Path,
    domain_types_dir: &Path,
    hooks_tmp: &Path,
) {
    let tera = codegraph::generate::template_engine::create_tera(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("templates"),
    )
    .unwrap();

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let type_contracts_path = workspace_root
        .join("crates")
        .join("codegraph-type-contracts");
    let workflow_path = workspace_root.join("crates").join("codegraph-workflow");
    let profiles = profiles_path();

    let registry = codegraph::profile::CapabilityRegistry::new();
    let mut resolved =
        codegraph::profile::load_and_resolve_profile(&profiles, "default", None).unwrap();
    for section in resolved.sections.values_mut() {
        section.generators.retain(|g| !g.starts_with("grpc_"));
    }
    let plan = codegraph::profile::BuildPlan::from_profile(&resolved, &registry).unwrap();

    let project_config = codegraph::generate::ProjectConfig {
        identity: codegraph::generate::IdentityConfig {
            app_name: "test-app".into(),
            domain_types_crate: "domain_types".into(),
            generator_name: "codegraph-test".into(),
            ..Default::default()
        },
        paths: codegraph::generate::PathsConfig {
            type_contracts_base: type_contracts_path.to_string_lossy().to_string(),
            codegraph_workflow_base: workflow_path.to_string_lossy().to_string(),
            domain_types_base: "domain-types".into(),
            ..Default::default()
        },
        codegen: codegraph::generate::CodegenConfig {
            types_import_prefix: "crate::types".into(),
            ..Default::default()
        },
        ..Default::default()
    };

    let config =
        codegraph_config::config::parse_domain_config(Path::new("tests/fixtures/domains.toml"))
            .unwrap();

    let report =
        codegraph::generate::run_generators_with_opts(codegraph::generate::GeneratorOpts {
            db: engine,
            config: &config,
            output_dir,
            tera: &tera,
            ui_overrides: &Default::default(),
            ui_domains: &Default::default(),
            schema_base_dir: Path::new(""),
            seed_config: None,
            domain_types_base: Some(domain_types_dir),
            hooks_base: Some(hooks_tmp),
            ext_points,
            build_plan: Some(&plan),
            ifml_frameworks: vec![],
            ifml_components: None,
            ux_rules: None,
            project_config: Some(&project_config),
            emdash_plugins: None,
            domain_config_dir: None,
        })
        .await
        .unwrap();
    assert!(!report.has_errors(), "generation reported errors");
}

fn read_command_module(output_dir: &Path) -> String {
    fs::read_to_string(
        output_dir
            .join("src")
            .join("domain")
            .join("recruiting")
            .join("candidate")
            .join("command.rs"),
    )
    .expect("candidate command module must be generated")
}

// ── Command-layer call sites (the B5-style payoff) ──

#[tokio::test]
async fn extension_point_coverage_emits_notify_call_sites() {
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("out");
    let domain_types_dir = dir.path().join("domain-types");
    fs::create_dir_all(&output_dir).unwrap();

    let engine = GrafeoEngine::in_memory().unwrap();
    let config =
        codegraph_config::config::parse_domain_config(Path::new("tests/fixtures/domains.toml"))
            .unwrap();
    ingest_fixture_schemas(&engine, &config).await;

    let ext = parse_extension_points_str(EXT_POINTS_TOML).unwrap();

    let hooks_tmp = tempfile::tempdir().unwrap();
    run_generation(
        &engine,
        Some(&ext),
        &output_dir,
        &domain_types_dir,
        hooks_tmp.path(),
    )
    .await;

    // The dispatcher module exists (the plane is installed)…
    assert!(
        output_dir
            .join("src")
            .join("integration_dispatch.rs")
            .exists(),
        "integration_dispatch.rs must be generated when extension points are configured"
    );

    // …and the covered entity's command layer FIRES it: one notify call
    // site per EMITTED mutation, entity_type = the full domain.table ref,
    // closed event vocabulary. The fixture entity carries create/update
    // (no delete op) — the delete site must follow the delete fn.
    let command = read_command_module(&output_dir);
    let has_delete = command.contains("pub async fn delete");
    for (op_fn, event) in [
        ("pub async fn create", "created"),
        ("pub async fn update", "updated"),
        ("pub async fn delete", "deleted"),
    ] {
        assert_eq!(
            command.contains(op_fn),
            command.contains(&format!(
                "dispatcher.notify(organization_id, \"recruiting.candidate\", \"{event}\""
            )),
            "the `{event}` notify site must exactly follow the `{op_fn}` fn"
        );
    }
    assert!(
        command
            .contains("dispatcher.notify(organization_id, \"recruiting.candidate\", \"created\""),
        "command module must carry the create notify site:\n{command}"
    );
    let _ = has_delete;
}

/// Presence gating (the byte-identity contract): with NO extension-points
/// config the command layer carries no dispatcher field and no notify call
/// sites, and the integration plane is not generated at all.
#[tokio::test]
async fn without_extension_points_output_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("out");
    let domain_types_dir = dir.path().join("domain-types");
    fs::create_dir_all(&output_dir).unwrap();

    let engine = GrafeoEngine::in_memory().unwrap();
    let config =
        codegraph_config::config::parse_domain_config(Path::new("tests/fixtures/domains.toml"))
            .unwrap();
    ingest_fixture_schemas(&engine, &config).await;

    let hooks_tmp = tempfile::tempdir().unwrap();
    run_generation(
        &engine,
        None,
        &output_dir,
        &domain_types_dir,
        hooks_tmp.path(),
    )
    .await;

    assert!(
        !output_dir
            .join("src")
            .join("integration_dispatch.rs")
            .exists(),
        "integration_dispatch.rs must not be generated without extension points"
    );
    let command = read_command_module(&output_dir);
    assert!(
        !command.contains("dispatcher"),
        "command module must not reference a dispatcher without extension points:\n{command}"
    );
    assert!(
        !command.contains("notify("),
        "command module must not carry notify call sites without extension points:\n{command}"
    );
}

// ── Dispatcher contract ──

fn test_project(provider: &str) -> codegraph::generate::ProjectConfig {
    let mut project = codegraph::generate::ProjectConfig::default();
    project.identity.generator_name = "codegraph-test".into();
    if provider == "cornucopia" {
        project.database.persistence_provider =
            codegraph::generate::profile::PersistenceProvider::Cornucopia;
    }
    project
}

fn render_dispatcher(provider: &str) -> String {
    let tera = codegraph::generate::template_engine::create_tera(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("templates"),
    )
    .unwrap();
    let ctx: HashMap<String, String> = HashMap::new();
    codegraph::generate::render_template_with_project(
        &tera,
        "integration/dispatcher.tera",
        &ctx,
        &test_project(provider),
    )
    .expect("dispatcher.tera must render")
}

/// The POST payload is the documented contract: event namespaced by the
/// full `domain.table` entity_type, plus the four identity fields.
#[test]
fn dispatcher_payload_shape_is_the_documented_contract() {
    let dispatcher = render_dispatcher("sea_orm");
    for fragment in [
        "serde_json::json!({",
        "format!(\"{}.{}\", entity_type, event)",
        "\"entity_id\": entity_id.to_string()",
        "\"org_id\": org_id.to_string()",
        "\"installation_id\": installation_id.to_string()",
        "\"correlation_id\": correlation_id.to_string()",
    ] {
        assert!(
            dispatcher.contains(fragment),
            "dispatcher payload must carry `{fragment}`:\n{dispatcher}"
        );
    }
}

/// The dispatcher must be constructible in EVERY topology the call sites
/// fire in: cornucopia via the emitted `ClientSource`/`GenericClient`
/// handle (never a SeaORM handle there), and the wasm32 worker slice via
/// `worker::Fetch` (reqwest does not link wasm32).
#[test]
fn dispatcher_supports_cornucopia_and_wasm_legs() {
    let cornucopia = render_dispatcher("cornucopia");
    assert!(
        cornucopia.contains("ClientSource"),
        "cornucopia dispatcher must take a ClientSource handle:\n{cornucopia}"
    );
    assert!(
        !cornucopia.contains("DatabaseConnection"),
        "cornucopia dispatcher must not reference a SeaORM handle:\n{cornucopia}"
    );

    for dispatcher in [render_dispatcher("sea_orm"), cornucopia] {
        assert!(
            dispatcher.contains("#[cfg(not(target_arch = \"wasm32\"))]"),
            "native HTTP leg must be reqwest behind the native cfg:\n{dispatcher}"
        );
        assert!(
            dispatcher.contains("reqwest"),
            "native HTTP leg must use reqwest:\n{dispatcher}"
        );
        assert!(
            dispatcher.contains("#[cfg(target_arch = \"wasm32\")]"),
            "wasm HTTP leg must be cfg-gated:\n{dispatcher}"
        );
        assert!(
            dispatcher.contains("worker::Fetch"),
            "wasm HTTP leg must use worker::Fetch:\n{dispatcher}"
        );
    }
}
