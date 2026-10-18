//! Shared setup/helpers for the Grafeo E2E suite.
//!
//! Moved verbatim from `grafeo_e2e_tests.rs` (issue #376); visibility raised
//! to `pub(crate)` so the sibling test modules can use them.

use std::collections::HashSet;
use std::path::Path;

use codegraph::generate::template_engine::create_tera;
use codegraph_config::config::parse_domain_config_str;
use codegraph_grafeo::GrafeoEngine;
/// Extract entity type names from a DomainConfig into a HashSet.
pub(crate) fn entity_names_from_config(
    config: &codegraph_config::config::DomainConfig,
) -> HashSet<String> {
    config
        .domains
        .values()
        .flat_map(|d| d.entities.iter().cloned())
        .collect()
}

/// Set up Grafeo engine with ingested fixture schemas.
pub(crate) async fn setup_grafeo() -> (GrafeoEngine, codegraph_config::config::DomainConfig) {
    setup_grafeo_from(Path::new("tests/fixtures")).await
}

/// [`setup_grafeo`] against an arbitrary fixture directory (domains.toml,
/// classifier.toml, schemas/ directly underneath).
pub(crate) async fn setup_grafeo_from(
    fixture_dir: &Path,
) -> (GrafeoEngine, codegraph_config::config::DomainConfig) {
    let config =
        codegraph_config::config::parse_domain_config(&fixture_dir.join("domains.toml")).unwrap();
    let classifier =
        codegraph_classifier::config::parse_classifier_config(&fixture_dir.join("classifier.toml"))
            .unwrap();
    let entity_names = entity_names_from_config(&config);
    let engine = GrafeoEngine::in_memory().unwrap();

    codegraph::ingest::async_ingest::ingest_schemas(
        &engine,
        &fixture_dir.join("schemas"),
        &classifier,
        &entity_names,
        &codegraph_config::UiOverrideConfig::default(),
        &config.defaults.type_suffix,
    )
    .await
    .unwrap();

    (engine, config)
}

pub(crate) async fn generate_full_app(output_dir: &std::path::Path) {
    generate_full_app_from(output_dir, Path::new("tests/fixtures")).await
}

/// [`generate_full_app`] against an arbitrary fixture directory.
pub(crate) async fn generate_full_app_from(output_dir: &std::path::Path, fixture_dir: &Path) {
    let (engine, config) = setup_grafeo_from(fixture_dir).await;
    let tera =
        create_tera(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates")).unwrap();

    // Absolute paths to workspace crates so the generated Cargo.toml can use
    // path deps (production wires these via profiles.toml meta).
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let type_contracts_path = workspace_root
        .join("crates")
        .join("codegraph-type-contracts");
    let workflow_path = workspace_root.join("crates").join("codegraph-workflow");
    let profiles_path = workspace_root.join("profiles.toml");

    // Build a BuildPlan like the driver does, minus the gRPC generators:
    // their output is compiled through tonic-build (needs protoc) and has
    // its own compile coverage in grpc_compile_tests.rs, which skips
    // gracefully when protoc is absent. The repository emitter is the
    // target of this gate, not the gRPC stack.
    let registry = codegraph::profile::CapabilityRegistry::new();
    let mut resolved =
        codegraph::profile::load_and_resolve_profile(&profiles_path, "default", None).unwrap();
    for section in resolved.sections.values_mut() {
        section.generators.retain(|g| !g.starts_with("grpc_"));
    }
    let plan = codegraph::profile::BuildPlan::from_profile(&resolved, &registry).unwrap();

    // The domain-types crate is generated INSIDE the app output so the
    // scaffolded `domain-types = { path = "domain-types" }` dependency
    // resolves (it auto-joins the generated workspace as a path-dep member).
    // Production redirects it via profiles meta `domain_types_base`.
    let domain_types_dir = output_dir.join("domain-types");
    let hooks_tmp = tempfile::TempDir::new().unwrap();

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
            types_import_prefix: config.defaults.types_import_prefix.clone(),
            ..Default::default()
        },
        cargo: codegraph::generate::CargoConfig {
            // The scaffold only emits codegraph-workflow / type-contracts deps as
            // git+rev deps gated on `codegraph_rev`; with the rev left empty they
            // are omitted. Inject them as local path deps instead (rendered raw
            // into [dependencies] by extra_dependencies) so `cargo check` never
            // touches the network.
            extra_dependencies: format!(
                "codegraph-workflow = {{ path = \"{}\" }}\n\
                 codegraph-type-contracts = {{ path = \"{}\" }}",
                workflow_path.display(),
                type_contracts_path.display(),
            ),
            ..Default::default()
        },
        ..Default::default()
    };

    let ui_overrides = codegraph_config::UiOverrideConfig::default();
    let ui_domains = codegraph_config::UiDomainConfig::default();
    let report =
        codegraph::generate::run_generators_with_opts(codegraph::generate::GeneratorOpts {
            db: &engine,
            config: &config,
            output_dir,
            tera: &tera,
            ui_overrides: &ui_overrides,
            ui_domains: &ui_domains,
            schema_base_dir: std::path::Path::new(""),
            seed_config: None,
            domain_types_base: Some(&domain_types_dir),
            hooks_base: Some(hooks_tmp.path()),
            ext_points: None,
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

    assert!(
        !report.has_errors(),
        "Expected no generation errors, got: {:?}",
        report
            .errors
            .iter()
            .map(|e| format!("{}/{}: {}", e.entity, e.generator, e.source))
            .collect::<Vec<_>>()
    );

    // The `repository` generator (part of the default plan) emits
    // `repository_impl.rs` for every entity via RepositoryImplEmitter::emit,
    // including the include-path fetch methods resolved for each handler.
    // Assert the gate actually covers the emitter — if the generator stops
    // producing these files, this test would silently stop compiling
    // repository output.
    let repository_impl_count =
        count_files_named(&output_dir.join("src").join("domain"), "repository_impl.rs");
    assert!(
        repository_impl_count > 0,
        "compile gate must emit at least one repository_impl.rs, \
         otherwise it no longer covers the repository emitter"
    );

    // Generate mod.rs files for all directories under src/
    generate_mod_files_recursive(&output_dir.join("src"));

    // Generate top-level test entry points for tests/{domain}/ subdirectories
    let tests_dir = output_dir.join("tests");
    if tests_dir.is_dir() {
        for entry in std::fs::read_dir(&tests_dir).unwrap().flatten() {
            if entry.file_type().unwrap().is_dir() {
                let domain_name = entry.file_name().to_string_lossy().to_string();
                let mut mods: Vec<String> = std::fs::read_dir(entry.path())
                    .unwrap()
                    .flatten()
                    .filter_map(|f| {
                        let name = f.file_name().to_string_lossy().to_string();
                        name.strip_suffix(".rs").map(|s| s.to_string())
                    })
                    .collect();
                mods.sort();
                let content = format!(
                    "// Generated by hr-graph. DO NOT EDIT.\n\n{}\n",
                    mods.iter()
                        .map(|m| format!("#[path = \"{domain_name}/{m}.rs\"]\nmod {m};"))
                        .collect::<Vec<_>>()
                        .join("\n")
                );
                std::fs::write(tests_dir.join(format!("{domain_name}.rs")), content).unwrap();
            }
        }
    }
}

/// Recursively count files with the given name under `dir`.
pub(crate) fn count_files_named(dir: &std::path::Path, name: &str) -> usize {
    let mut count = 0;
    if !dir.is_dir() {
        return 0;
    }
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        if entry.file_type().unwrap().is_dir() {
            count += count_files_named(&entry.path(), name);
        } else if entry.file_name().to_string_lossy() == name {
            count += 1;
        }
    }
    count
}

/// Recursively generate `mod.rs` files for directories that need them.
/// Skips directories that already contain `main.rs` (crate root).
pub(crate) fn generate_mod_files_recursive(dir: &std::path::Path) {
    if !dir.is_dir() {
        return;
    }

    let mut modules = Vec::new();

    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();

        if entry.file_type().unwrap().is_dir() {
            generate_mod_files_recursive(&entry.path());
            modules.push(name);
        } else if let Some(stem) = name.strip_suffix(".rs")
            && stem != "mod"
            && stem != "main"
            && stem != "app_state"
        {
            modules.push(stem.to_string());
        }
    }

    // Only create mod.rs for subdirectories (not the crate root which has main.rs)
    if !modules.is_empty() && !dir.join("mod.rs").exists() && !dir.join("main.rs").exists() {
        modules.sort();
        let content = format!(
            "// Generated by hr-graph. DO NOT EDIT.\n\n{}\n",
            modules
                .iter()
                .map(|m| format!("pub mod {m};"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        std::fs::write(dir.join("mod.rs"), content).unwrap();
    }
}

/// Helper: inline domain config with `allow_include` for CandidateType.
pub(crate) fn include_domain_config() -> codegraph_config::DomainConfig {
    let config_str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.common]
label = "Common"
schema_dir = "common"
postgres_schema = "common"
entities = []

[domains.recruiting]
label = "Recruiting"
schema_dir = "recruiting"
postgres_schema = "recruiting"
depends_on = ["common"]
entities = ["CandidateType", "ApplicationType"]

[domains.recruiting.entity_config.CandidateType]
role = "root"
allow_include = ["application"]
"#;
    parse_domain_config_str(config_str).unwrap()
}
