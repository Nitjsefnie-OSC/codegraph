//! Deployment-plane ingest tests (rexlang #49): the end-to-end
//! `ingest_deploy_files` run over a fixture dir (.deploy importing a .ddd
//! design with a `#Module` binding), hard-error semantics (compile error,
//! un-compilable design import, missing file), the MockEngine round trip,
//! and the graph-cache inputs group.

use std::path::{Path, PathBuf};

use codegraph_core::mock::MockEngine;
use codegraph_core::traits::GraphQuerier;

// ── fixtures ────────────────────────────────────────────────────────────

const LIB_MOX: &str = concat!(
    "package shop\n",
    "\n",
    "class Product {\n",
    "    String title\n",
    "}\n",
);

const LIB_DDD: &str = concat!(
    "import \"lib.mox\"\n",
    "\n",
    "application Shop {\n",
    "    base shop\n",
    "\n",
    "    module catalogue {\n",
    "        entity Product repository ProductRepository {\n",
    "            findById;\n",
    "            findAll;\n",
    "            save;\n",
    "            delete;\n",
    "        }\n",
    "    }\n",
    "}\n",
);

/// The design binding names the imported design's module — a clean compile
/// proves the .ddd import was resolved, compiled against its own .mox
/// import, and matched by path.
const SVC_DEPLOY: &str = concat!(
    "import \"lib.ddd\"\n",
    "\n",
    "application Shop {\n",
    "    component web: frontend\n",
    "    component api: api {\n",
    "        design \"lib.ddd#catalogue\"\n",
    "    }\n",
    "    component db: database { engine: d1 }\n",
    "    connects web -> api\n",
    "    connects api -> db\n",
    "}\n",
    "\n",
    "profile edge {\n",
    "    target: cloudflareWorkers\n",
    "    defaults { db.engine: d1 }\n",
    "    prohibit (db.engine == \"postgres\")\n",
    "    mappings { database -> cloudflare.d1 }\n",
    "}\n",
    "\n",
    "deployment prod for Shop {\n",
    "    use edge\n",
    "    configure { api.replicas: 4 }\n",
    "}\n",
);

/// Writes the fixture dir and returns the svc.deploy path.
fn write_fixture(dir: &Path) -> PathBuf {
    std::fs::write(dir.join("lib.mox"), LIB_MOX).unwrap();
    std::fs::write(dir.join("lib.ddd"), LIB_DDD).unwrap();
    let deploy = dir.join("svc.deploy");
    std::fs::write(&deploy, SVC_DEPLOY).unwrap();
    deploy
}

// ── end-to-end ingest + MockEngine round trip ──────────────────────────

#[tokio::test]
async fn ingest_deploy_file_end_to_end_with_design_import() {
    let dir = tempfile::tempdir().unwrap();
    let deploy = write_fixture(dir.path());

    let mock = MockEngine::new();
    let stats = codegraph::ingest::deploy_ingest::ingest_deploy_files(&mock, &[deploy])
        .await
        .unwrap();

    assert_eq!(stats.files, 1);
    assert_eq!(stats.applications, 1);
    assert_eq!(stats.profiles, 1);
    assert_eq!(stats.deployments, 1);

    // MockEngine round trip: get_deploy_models returns exactly what was
    // ingested (the whole model, declaration order preserved).
    let models = mock.get_deploy_models().await.unwrap();
    assert_eq!(models.len(), 1);
    let graph = &models[0];
    assert!(graph.source_path.ends_with("svc.deploy"), "{graph:?}");

    let app = &graph.applications[0];
    assert_eq!(app.name, "Shop");
    assert_eq!(
        app.components
            .iter()
            .map(|c| (c.name.as_str(), c.kind.as_str()))
            .collect::<Vec<_>>(),
        vec![("web", "frontend"), ("api", "api"), ("db", "database")],
    );
    let api = app.components.iter().find(|c| c.name == "api").unwrap();
    assert_eq!(api.ordinal, 1, "declaration order within the application");
    assert_eq!(
        api.designs,
        vec![codegraph_core::types::DeployBindingNode {
            source: "lib.ddd".to_string(),
            module: Some("catalogue".to_string()),
        }],
    );
    assert_eq!(
        app.connections
            .iter()
            .map(|c| (c.from.as_str(), c.to.as_str()))
            .collect::<Vec<_>>(),
        vec![("web", "api"), ("api", "db")],
    );

    let profile = &graph.profiles[0];
    assert_eq!(profile.name, "edge");
    assert_eq!(profile.target, "cloudflareWorkers");
    assert_eq!(profile.policies[0].expression, r#"db.engine == "postgres""#);

    let deployment = &graph.deployments[0];
    assert_eq!(deployment.application, "Shop");
    assert_eq!(deployment.profile, "edge");
    assert_eq!(deployment.configure[0].path, "api.replicas");
}

#[tokio::test]
async fn compile_error_is_a_hard_error_and_never_half_ingests() {
    let dir = tempfile::tempdir().unwrap();
    let deploy = dir.path().join("bad.deploy");
    std::fs::write(&deploy, "application Broken {").unwrap();

    let mock = MockEngine::new();
    let err = codegraph::ingest::deploy_ingest::ingest_deploy_files(&mock, &[deploy])
        .await
        .unwrap_err();

    match &err {
        codegraph::error::Error::DeployModel { file, reason } => {
            assert!(file.ends_with("bad.deploy"), "{file}");
            assert!(
                !reason.is_empty(),
                "the rendered diagnostic rides the reason"
            );
        }
        other => panic!("expected Error::DeployModel, got {other:?}"),
    }
    assert!(
        mock.get_deploy_models().await.unwrap().is_empty(),
        "a broken deployment never half-ingests"
    );
}

#[tokio::test]
async fn design_import_that_does_not_compile_is_a_hard_error() {
    let dir = tempfile::tempdir().unwrap();
    // lib.ddd imports a mox file that does not exist → the design compile
    // fails → the deploy hard-errors naming the import.
    std::fs::write(dir.path().join("lib.ddd"), LIB_DDD).unwrap();
    let deploy = dir.path().join("svc.deploy");
    std::fs::write(&deploy, SVC_DEPLOY).unwrap();

    let mock = MockEngine::new();
    let err = codegraph::ingest::deploy_ingest::ingest_deploy_files(&mock, &[deploy])
        .await
        .unwrap_err();

    match &err {
        codegraph::error::Error::DeployModel { file, reason } => {
            assert!(file.ends_with("svc.deploy"), "{file}");
            assert!(
                reason.contains("imported design 'lib.ddd' does not compile"),
                "{reason}"
            );
        }
        other => panic!("expected Error::DeployModel, got {other:?}"),
    }
    assert!(mock.get_deploy_models().await.unwrap().is_empty());
}

#[tokio::test]
async fn missing_deploy_file_is_a_hard_error() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockEngine::new();
    let err = codegraph::ingest::deploy_ingest::ingest_deploy_files(
        &mock,
        &[dir.path().join("absent.deploy")],
    )
    .await
    .unwrap_err();

    match &err {
        codegraph::error::Error::DeployModel { file, reason } => {
            assert!(file.ends_with("absent.deploy"), "{file}");
            assert!(reason.contains("could not be read"), "{reason}");
        }
        other => panic!("expected Error::DeployModel, got {other:?}"),
    }
}

// ── graph-cache inputs ─────────────────────────────────────────────────

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.shop]
label = "Shop"
schema_dir = "shop"
postgres_schema = "shop"
"#;

#[test]
fn deploy_files_are_part_of_the_graph_cache_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(&domains_path, DOMAINS_TOML).unwrap();
    let deploy = dir.path().join("svc.deploy");
    std::fs::write(&deploy, SVC_DEPLOY).unwrap();

    let inputs = codegraph::artifact::collect_run_inputs(
        None,
        None,
        &domains_path,
        &[],
        &[],
        &[],
        &[],
        std::slice::from_ref(&deploy),
        &[],
        &[],
    )
    .unwrap();
    assert!(
        inputs
            .iter()
            .any(|f| f.path == deploy.display().to_string() && f.bytes == SVC_DEPLOY.as_bytes()),
        "the .deploy file group rides the inputs list: {:?}",
        inputs.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    let before = codegraph::artifact::inputs_hash(&inputs);
    std::fs::write(
        &deploy,
        SVC_DEPLOY.replace("api.replicas: 4", "api.replicas: 8"),
    )
    .unwrap();
    let after = codegraph::artifact::inputs_hash(
        &codegraph::artifact::collect_run_inputs(
            None,
            None,
            &domains_path,
            &[],
            &[],
            &[],
            &[],
            std::slice::from_ref(&deploy),
            &[],
            &[],
        )
        .unwrap(),
    );
    assert_ne!(
        before, after,
        "editing a .deploy file must bust the persisted-graph cache"
    );

    // A run without deploy files is unaffected (the group is empty).
    let without = codegraph::artifact::collect_run_inputs(
        None,
        None,
        &domains_path,
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();
    assert!(
        !without.iter().any(|f| f.path.contains(".deploy")),
        "no deploy entries without deploy files"
    );
}
