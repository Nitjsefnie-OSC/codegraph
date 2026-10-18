use std::path::Path;

use codegraph::profile::BuildPlan;

use crate::harness::mock_test_setup;

/// Run the routing-sensitive generator subset against the mock graph with a
/// directly-constructed build plan for the given deployment topology.
async fn run_routing_generators(
    topology: codegraph::profile::DeploymentTopology,
) -> (
    codegraph::generate::report::GenerationReport,
    tempfile::TempDir,
) {
    let (mock, config, tera, output_dir) = mock_test_setup();
    let domain_types_tmp = tempfile::TempDir::new().unwrap();
    let hooks_tmp = tempfile::TempDir::new().unwrap();

    let plan = BuildPlan {
        entity_generators: vec![
            "ddl".to_string(),
            "sea_orm_entity".to_string(),
            "handler".to_string(),
            "dto".to_string(),
            "repository".to_string(),
            "command".to_string(),
            "query".to_string(),
            "event".to_string(),
        ],
        domain_generators: vec![
            "errors".to_string(),
            "router".to_string(),
            "links".to_string(),
        ],
        global_generators: vec!["scaffold".to_string()],
        post_gen_scripts: vec![],
        ifml_frameworks: vec![],
        template_pack_path: None,
        database_target: codegraph::generate::db::dialect::DatabaseTarget::Postgres,
        has_atproto: false,
        atproto_tenancy: "shared_pds".to_string(),
        has_fern: false,
        has_emdash: false,
        fern_sdk_languages: vec!["typescript".to_string()],
        persistence_provider: codegraph::profile::PersistenceProvider::SeaOrm,
        dto_key_casing: "snake".to_string(),
        deployment_topology: topology,
        dependency_strategy: codegraph::profile::DependencyStrategy::Rev,
        namespace_layout: false,
        ux_rules: false,
        expr_ir: false,
        ifml_e2e_auth: false,
        testid_module: false,
        structural_gates: false,
        public_operations_rls: false,
        features: toml::Table::new(),
    };

    // Mirror main.rs: the project config carries the topology string so
    // templates can read `project.deployment_topology`.
    let project = codegraph::generate::ProjectConfig {
        deployment: codegraph::generate::DeploymentConfig {
            deployment_topology: topology,
        },
        ..Default::default()
    };

    let report =
        codegraph::generate::run_generators_with_opts(codegraph::generate::GeneratorOpts {
            db: &mock,
            config: &config,
            output_dir: output_dir.path(),
            tera: &tera,
            ui_overrides: &Default::default(),
            ui_domains: &Default::default(),
            schema_base_dir: Path::new(""),
            domain_types_base: Some(domain_types_tmp.path()),
            hooks_base: Some(hooks_tmp.path()),
            ext_points: None,
            seed_config: None,
            build_plan: Some(&plan),
            ifml_frameworks: vec![],
            ifml_components: None,
            ux_rules: None,
            project_config: Some(&project),
            emdash_plugins: None,
            domain_config_dir: None,
        })
        .await
        .expect("routing generator run should succeed");

    assert!(
        !report.has_errors(),
        "routing run should have no errors: {}",
        report.summary()
    );
    (report, output_dir)
}

#[tokio::test]
async fn workers_topology_routes_domain_files_into_worker_crates() {
    let (report, _output_dir) =
        run_routing_generators(codegraph::profile::DeploymentTopology::Workers).await;

    let paths: Vec<String> = report
        .files
        .iter()
        .map(|f| f.path.to_string_lossy().to_string())
        .collect();

    let any = |needle: &str| paths.iter().any(|p| p.contains(needle));

    // Routed entity generators land inside the recruiting worker crate.
    assert!(any(
        "/workers/recruiting/src/domain/recruiting/candidate/repository.rs"
    ));
    assert!(any(
        "/workers/recruiting/src/domain/recruiting/candidate/command.rs"
    ));
    assert!(any(
        "/workers/recruiting/src/domain/recruiting/candidate/dto_create.rs"
    ));
    assert!(any(
        "/workers/recruiting/src/entity/recruiting_candidate.rs"
    ));
    assert!(any(
        "/workers/recruiting/src/api/recruiting/candidate_handler.rs"
    ));

    // Routed domain generators land inside the worker crate too.
    // (errors.rs is not asserted: the mock graph has no error definitions and
    // the generator emits nothing for domains without errors.)
    assert!(any("/workers/recruiting/src/api/recruiting/router.rs"));
    assert!(any("/workers/recruiting/src/api/links.rs"));

    // No routed backend output may leak to the monolith root.
    let root_leaks = |needle: &str| {
        paths
            .iter()
            .filter(|p| p.contains(needle) && !p.contains("/workers/"))
            .count()
    };
    assert_eq!(
        root_leaks("/src/domain/recruiting/candidate/"),
        0,
        "routed domain files must not land under the output root"
    );
    assert_eq!(
        root_leaks("/src/api/recruiting/"),
        0,
        "routed api files must not land under the output root"
    );
    assert_eq!(
        root_leaks("/src/entity/"),
        0,
        "routed entity files must not land under the output root"
    );

    // Root-anchored output: shared migrations stay at the output root.
    let has_shared_migration = paths
        .iter()
        .any(|p| p.contains("/migrations/") && p.ends_with("recruiting_candidate.sql"));
    assert!(
        has_shared_migration,
        "shared DDL migrations must stay at the root"
    );
    assert!(
        paths
            .iter()
            .filter(|p| p.contains("/migrations/"))
            .all(|p| !p.contains("/workers/")),
        "no migration may be routed into a worker crate (single shared DB)"
    );

    // The monolith scaffold is gated off entirely in workers topology.
    // Worker crates legitimately carry src/main.rs / Cargo.toml under
    // workers/{domain}/ (and the workers/ workspace manifest) — the
    // monolith shapes are the ones outside workers/.
    assert!(
        !paths
            .iter()
            .any(|p| !p.contains("/workers/") && p.contains("/src/main.rs")),
        "monolith src/main.rs must not be generated in workers topology"
    );
    assert!(
        !paths
            .iter()
            .any(|p| !p.contains("/workers/") && p.contains("/src/server.rs")),
        "monolith src/server.rs must not be generated in workers topology"
    );
    assert!(
        !paths
            .iter()
            .any(|p| !p.contains("/workers/") && p.ends_with("/Cargo.toml")),
        "root monolith Cargo.toml must not be generated in workers topology"
    );

    println!("workers routing produced {} files", report.files.len());
}

#[tokio::test]
async fn monolith_topology_keeps_flat_output_layout() {
    let (report, output_dir) =
        run_routing_generators(codegraph::profile::DeploymentTopology::Monolith).await;

    let paths: Vec<String> = report
        .files
        .iter()
        .map(|f| f.path.to_string_lossy().to_string())
        .collect();

    let any = |needle: &str| paths.iter().any(|p| p.contains(needle));

    // No workers/ directory may be produced in monolith topology.
    assert!(
        !any("/workers/"),
        "monolith topology must not produce a workers/ directory"
    );

    // Backend source lives directly under the output root, as before.
    assert!(any("/src/domain/recruiting/candidate/repository.rs"));
    assert!(any("/src/domain/recruiting/candidate/command.rs"));
    assert!(any("/src/entity/recruiting_candidate.rs"));
    assert!(any("/src/api/recruiting/candidate_handler.rs"));
    assert!(any("/src/api/recruiting/router.rs"));
    assert!(any("/src/api/links.rs"));

    // Root-anchored output unchanged.
    let has_shared_migration = paths
        .iter()
        .any(|p| p.contains("/migrations/") && p.ends_with("recruiting_candidate.sql"));
    assert!(
        has_shared_migration,
        "shared DDL migrations must stay at the root"
    );

    // The scaffold is NOT gated in monolith topology.
    assert!(any("/src/main.rs"));
    assert!(any("/src/server.rs"));
    assert!(any("/Cargo.toml"));

    // And the files were actually written to disk at the monolith paths.
    assert!(output_dir.path().join("src/main.rs").exists());
    assert!(output_dir.path().join("Cargo.toml").exists());

    println!("monolith routing produced {} files", report.files.len());
}
