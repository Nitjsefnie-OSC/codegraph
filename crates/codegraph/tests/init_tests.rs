//! Project-init lifecycle integration tests: `init`, `doctor`, `add domain`,
//! and the embedded rev accessor. All hermetic (tempfile, no network).
//!
//! Init is mox-first (issue #231): the scaffold emits `model/<domain>.mox`
//! per domain and NO `schemas/` directory or `classifier.toml`.

use std::fs;
use std::path::{Path, PathBuf};

use codegraph::init::commands::{DoctorArgs, InitArgs, cmd_add_domain, cmd_doctor, cmd_init};
use codegraph::init::{ProjectFeatures, ProjectTemplateContext};
use codegraph::profile::{BuildPlan, CapabilityRegistry, load_and_resolve_profile};
use tempfile::TempDir;

/// Absolute repo root (`<repo>/crates/codegraph` → two parents up).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("codegraph crate lives at <repo>/crates/codegraph")
        .to_path_buf()
}

fn features() -> ProjectFeatures {
    ProjectFeatures {
        grpc: false,
        ifml: false,
        ops: true,
        rosetta: false,
    }
}

fn init_args(dir: &Path, name: &str, codegraph_path: Option<PathBuf>, force: bool) -> InitArgs {
    InitArgs {
        name: Some(name.to_string()),
        output_dir: dir.to_path_buf(),
        domains: vec!["common".to_string()],
        database_target: "postgres".to_string(),
        persistence_provider: "sea_orm".to_string(),
        deployment_topology: "monolith".to_string(),
        grpc: false,
        ifml: false,
        ops: true,
        rosetta: false,
        rev: Some("abc123".to_string()),
        codegraph_path,
        force,
        template_dirs: vec![],
    }
}

fn assert_scaffold_files_exist(project: &Path, domains: &[&str]) {
    let domain_names: Vec<String> = domains.iter().map(|d| d.to_string()).collect();
    let ctx = ProjectTemplateContext::new(
        "demo-app",
        &domain_names,
        "abc123",
        None,
        "postgres",
        "sea_orm",
        "monolith",
        features(),
    );
    let mut expected = ctx.file_tree();
    // 15 template entries minus the per-domain model expansion: 14 fixed
    // outputs + one model/<domain>.mox per domain.
    assert_eq!(
        expected.len(),
        14 + domains.len(),
        "file tree should be 14 fixed outputs + one model file per domain"
    );
    expected.sort();
    for rel in &expected {
        assert!(project.join(rel).is_file(), "missing {}", rel.display());
    }
    let mut actual: Vec<PathBuf> = walkdir::WalkDir::new(project)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .map(|p| p.strip_prefix(project).unwrap().to_path_buf())
        .collect();
    actual.sort();
    assert_eq!(actual, expected, "unexpected extra files: {actual:?}");
}

fn assert_mox_first_layout(project: &Path) {
    assert!(
        !project.join("schemas").exists(),
        "mox-first scaffold must not create a schemas/ directory"
    );
    assert!(
        !project.join("classifier.toml").exists(),
        "mox-first scaffold must not create classifier.toml"
    );
}

fn assert_starter_model_compiles(project: &Path, domain: &str) {
    let path = project.join("model").join(format!("{domain}.mox"));
    let content = fs::read_to_string(&path).unwrap();
    let compilation = rex_driver::compile_files(
        &[(path.display().to_string(), content.clone())],
        &rex_driver::DomainImports::default(),
    );
    assert!(
        compilation.model.is_some(),
        "starter model {domain}.mox must compile: {:?}",
        compilation.diagnostics
    );
    let model = compilation.model.unwrap();
    assert_eq!(
        model.packages[0].name, domain,
        "package name must match the domain"
    );
    let classes: Vec<&str> = model.packages[0]
        .classes
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    let capitalized = format!(
        "{}{}",
        domain.chars().next().unwrap().to_uppercase(),
        domain.chars().skip(1).collect::<String>()
    );
    let todo_list = format!("{capitalized}TodoListType");
    let todo_item = format!("{capitalized}TodoItemType");
    assert!(
        classes.contains(&todo_list.as_str()) && classes.contains(&todo_item.as_str()),
        "starter model must carry the TODO starter classes, got {classes:?}"
    );
}

#[test]
fn init_scaffolds_expected_file_tree() {
    let dir = TempDir::new().unwrap();
    let root = repo_root().canonicalize().unwrap();
    cmd_init(&init_args(
        dir.path(),
        "demo-app",
        Some(root.clone()),
        false,
    ))
    .unwrap();

    let project = dir.path().join("demo-app");
    assert_scaffold_files_exist(&project, &["common"]);
    assert_mox_first_layout(&project);

    let domains =
        codegraph_config::config::parse_domain_config(&project.join("domains.toml")).unwrap();
    assert!(domains.domains.contains_key("common"));

    let resolved =
        load_and_resolve_profile(&project.join("profiles.toml"), "default", None).unwrap();
    let plan = BuildPlan::from_profile(&resolved, &CapabilityRegistry::new()).unwrap();
    assert!(!plan.entity_generators.is_empty());

    assert_starter_model_compiles(&project, "common");

    let workspace = fs::read_to_string(project.join("Cargo.toml")).unwrap();
    let expected_path = format!("path = \"{}\"", root.join("crates/codegraph").display());
    assert!(
        workspace.contains(&expected_path),
        "workspace Cargo.toml should pin the codegraph crate via path:\n{workspace}"
    );
    assert!(
        !workspace.contains("magick93"),
        "path mode must not reference the git repo:\n{workspace}"
    );

    // A --codegraph-path scaffold is "asked for path": profiles.toml opts the
    // generated app into path deps too (issue #347).
    let profiles_raw = fs::read_to_string(project.join("profiles.toml")).unwrap();
    assert!(
        profiles_raw.contains("dependency_strategy = \"path\""),
        "profiles.toml must carry dependency_strategy = \"path\" when the \
         scaffold was given --codegraph-path:\n{profiles_raw}"
    );

    let main = fs::read_to_string(project.join("demo-app-graph/src/main.rs")).unwrap();
    assert!(
        main.contains("CODEGRAPH_REV: &str = \"abc123\""),
        "wrapper should stamp the rev:\n{main}"
    );
}

#[test]
fn init_scaffolds_one_model_file_per_domain() {
    let dir = TempDir::new().unwrap();
    let mut args = init_args(dir.path(), "demo-app", None, false);
    args.domains = vec!["common".to_string(), "billing".to_string()];
    cmd_init(&args).unwrap();

    let project = dir.path().join("demo-app");
    assert_scaffold_files_exist(&project, &["common", "billing"]);
    assert_mox_first_layout(&project);
    assert_starter_model_compiles(&project, "common");
    assert_starter_model_compiles(&project, "billing");

    let domains =
        codegraph_config::config::parse_domain_config(&project.join("domains.toml")).unwrap();
    assert!(domains.domains.contains_key("common"));
    assert!(domains.domains.contains_key("billing"));
}

/// The generated domains.toml must not pin `entities` — mox is
/// author-declarative and the bridge decides entity vs VO.
#[test]
fn init_domains_toml_has_no_entities_key() {
    let dir = TempDir::new().unwrap();
    cmd_init(&init_args(dir.path(), "demo-app", None, false)).unwrap();

    let raw = fs::read_to_string(dir.path().join("demo-app/domains.toml")).unwrap();
    assert!(
        !raw.contains("entities"),
        "domains.toml must not carry an entities key:\n{raw}"
    );
}

/// The generated wrapper + justfile must be mox-first: Run/Classify/Doctor
/// take --mox-files and no longer default --schemas/--classifier.
#[test]
fn init_wrapper_and_justfile_are_mox_first() {
    let dir = TempDir::new().unwrap();
    cmd_init(&init_args(dir.path(), "demo-app", None, false)).unwrap();
    let project = dir.path().join("demo-app");

    let main = fs::read_to_string(project.join("demo-app-graph/src/main.rs")).unwrap();
    assert!(
        !main.contains("default_value = \"schemas\""),
        "wrapper Run must not default --schemas:\n{main}"
    );
    assert!(
        !main.contains("default_value = \"classifier.toml\""),
        "wrapper must not default --classifier:\n{main}"
    );
    assert!(
        main.contains("mox_files"),
        "wrapper must accept --mox-files:\n{main}"
    );

    let justfile = fs::read_to_string(project.join("justfile")).unwrap();
    assert!(
        justfile.contains("--mox-files model/common.mox"),
        "justfile recipes must pass --mox-files per domain:\n{justfile}"
    );
    assert!(
        !justfile.contains("--classifier"),
        "justfile must not reference --classifier:\n{justfile}"
    );
    assert!(
        justfile.contains("Regenerate code from your .mox model"),
        "justfile generate comment must reference the .mox model:\n{justfile}"
    );

    // CI workflow is mox-first too (issue #346): --mox-files per domain, no
    // rosetta flags, and the plain postgres service (byte-identical to the
    // pre-rosetta shape).
    let ci = fs::read_to_string(project.join(".github/workflows/ci.yml")).unwrap();
    assert!(
        ci.contains("--mox-files model/common.mox"),
        "ci.yml generate step must pass --mox-files per domain:\n{ci}"
    );
    assert!(
        !ci.contains("--rosetta-files"),
        "mox-first ci.yml must not reference --rosetta-files:\n{ci}"
    );
    assert!(
        !ci.contains("APP_DATABASE_URL"),
        "mox-first ci.yml keeps its historical shape (no APP_DATABASE_URL):\n{ci}"
    );

    let domains_toml = fs::read_to_string(project.join("domains.toml")).unwrap();
    assert!(
        domains_toml.contains("model/common.mox"),
        "domains.toml should point at the model file:\n{domains_toml}"
    );

    let readme = fs::read_to_string(project.join("README.md")).unwrap();
    assert!(
        readme.contains(".mox model"),
        "README should describe the .mox-first scaffold:\n{readme}"
    );
    assert!(
        !readme.contains("classifier.toml"),
        "README layout must not list classifier.toml:\n{readme}"
    );
}

/// The ops manifest emitted by init must be mox-first: `mox_files` set, no
/// `schemas_dir`/`classifier` keys, and it must load through the harness's
/// `OpsConfig` (contract from PR #250).
#[test]
fn init_ops_manifest_is_mox_first() {
    let dir = TempDir::new().unwrap();
    let mut args = init_args(dir.path(), "demo-app", None, false);
    args.domains = vec!["common".to_string(), "billing".to_string()];
    cmd_init(&args).unwrap();

    let project = dir.path().join("demo-app");
    let raw = fs::read_to_string(project.join("codegraph-ops.toml")).unwrap();
    assert!(
        raw.contains(r#"mox_files = ["model/common.mox", "model/billing.mox"]"#),
        "manifest must list one mox file per domain in domain order:\n{raw}"
    );
    assert!(
        !raw.contains("schemas_dir"),
        "manifest must not carry schemas_dir:\n{raw}"
    );
    assert!(
        !raw.contains("classifier"),
        "manifest must not carry classifier:\n{raw}"
    );

    let manifest_path = project.join("codegraph-ops.toml");
    let cfg = codegraph_ops::OpsConfig::load(&manifest_path)
        .expect("emitted manifest must load via OpsConfig::load");
    assert_eq!(
        cfg.manifest.mox_files,
        vec![
            "model/common.mox".to_string(),
            "model/billing.mox".to_string()
        ]
    );
}

/// THE acceptance gate: a fresh scaffold must generate with ONLY --mox-files
/// + --config (no --schemas, no --classifier), producing DDL for both starter
/// classes with the refers-derived FK.
#[tokio::test]
async fn init_scaffold_runs_mox_first() {
    let dir = TempDir::new().unwrap();
    let root = repo_root().canonicalize().unwrap();
    cmd_init(&init_args(dir.path(), "demo-app", Some(root), false)).unwrap();
    let project = dir.path().join("demo-app");

    let mox_files = vec![project.join("model/common.mox")];
    let output = project.join("generated");
    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &project.join("domains.toml"),
        output: &output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(project.join("profiles.toml")),
        no_post_gen: true,
        template_dir: &[],
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
        codegraph_rev: None,
        check: false,
        ux_rules: None,
    })
    .await
    .unwrap();

    let generated: Vec<PathBuf> = walkdir::WalkDir::new(&output)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .collect();
    assert!(
        !generated.is_empty(),
        "mox-first generation must produce files"
    );

    let migrations_dir = output.join("migrations");
    let mut ddl = String::new();
    for entry in fs::read_dir(&migrations_dir).unwrap_or_else(|e| {
        panic!(
            "migrations dir missing under {}: {e}",
            migrations_dir.display()
        )
    }) {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) == Some("sql") {
            ddl.push_str(&fs::read_to_string(&path).unwrap());
            ddl.push('\n');
        }
    }
    assert!(
        ddl.contains("todo_list"),
        "DDL must contain todo_list:\n{ddl}"
    );
    assert!(
        ddl.contains("todo_item"),
        "DDL must contain todo_item:\n{ddl}"
    );
    assert!(
        ddl.contains("todo_list_id"),
        "starter refers must produce a todo_list_id FK on todo_item:\n{ddl}"
    );

    // Path dependency strategy (issue #347): the scaffold was created with
    // --codegraph-path, so profiles.toml carries dependency_strategy = "path"
    // and the generated app + testkit manifests must reference the codegraph
    // crates inside the generating checkout — no git+rev pins anywhere.
    let generated_cargo =
        fs::read_to_string(output.join("Cargo.toml")).expect("generated Cargo.toml");
    let checkout_root = repo_root();
    assert!(
        generated_cargo.contains(&format!(
            "codegraph-workflow = {{ path = \"{}/crates/codegraph-workflow\" }}",
            checkout_root.display()
        )),
        "generated Cargo.toml must reference codegraph-workflow by path:\n{generated_cargo}"
    );
    assert!(
        generated_cargo.contains(&format!(
            "codegraph-type-contracts = {{ path = \"{}/crates/codegraph-type-contracts\" }}",
            checkout_root.display()
        )),
        "generated Cargo.toml must reference codegraph-type-contracts by path:\n{generated_cargo}"
    );
    assert!(
        !generated_cargo.contains("magick93/codegraph.git"),
        "path strategy must not reference the git repo:\n{generated_cargo}"
    );
    let testkit_cargo =
        fs::read_to_string(output.join("testkit/Cargo.toml")).expect("testkit Cargo.toml");
    assert!(
        testkit_cargo.contains(&format!(
            "codegraph-ops = {{ path = \"{}/crates/codegraph-ops\" }}",
            checkout_root.display()
        )),
        "testkit Cargo.toml must reference codegraph-ops by path:\n{testkit_cargo}"
    );
}

#[test]
fn init_git_rev_mode_pins_rev_in_workspace() {
    let dir = TempDir::new().unwrap();
    cmd_init(&init_args(dir.path(), "demo-app", None, false)).unwrap();

    let workspace = fs::read_to_string(dir.path().join("demo-app/Cargo.toml")).unwrap();
    let git_pins = workspace
        .matches("git = \"https://github.com/magick93/codegraph.git\"")
        .count();
    let rev_pins = workspace.matches("rev = \"abc123\"").count();
    assert_eq!(
        git_pins, 11,
        "all 11 codegraph workspace deps should pin the git repo:\n{workspace}"
    );
    assert_eq!(
        rev_pins, 11,
        "all 11 codegraph workspace deps should pin rev abc123:\n{workspace}"
    );

    let main = fs::read_to_string(dir.path().join("demo-app/demo-app-graph/src/main.rs")).unwrap();
    assert!(
        main.contains("CODEGRAPH_REV: &str = \"abc123\""),
        "wrapper should stamp the rev:\n{main}"
    );

    // Default scaffolds (no --codegraph-path) stay on the rev strategy: no
    // dependency_strategy line at all (issue #347 byte-identity).
    let profiles_raw = fs::read_to_string(dir.path().join("demo-app/profiles.toml")).unwrap();
    assert!(
        !profiles_raw.contains("dependency_strategy"),
        "default scaffold must not emit a dependency_strategy line:\n{profiles_raw}"
    );
}

#[test]
fn init_refuses_overwrite_without_force() {
    let dir = TempDir::new().unwrap();
    cmd_init(&init_args(dir.path(), "demo-app", None, false)).unwrap();
    let err = cmd_init(&init_args(dir.path(), "demo-app", None, false)).unwrap_err();
    assert!(
        format!("{err}").contains("force"),
        "overwrite refusal should mention --force: {err}"
    );
}

#[test]
fn init_force_overwrites() {
    let dir = TempDir::new().unwrap();
    cmd_init(&init_args(dir.path(), "demo-app", None, false)).unwrap();
    cmd_init(&init_args(dir.path(), "demo-app", None, true)).unwrap();
    assert!(dir.path().join("demo-app/Cargo.toml").is_file());
}

#[test]
fn init_normalizes_project_name() {
    let dir = TempDir::new().unwrap();
    cmd_init(&init_args(dir.path(), "My Cool App", None, false)).unwrap();
    assert!(dir.path().join("my-cool-app/Cargo.toml").is_file());
    assert!(!dir.path().join("My Cool App").exists());
}

/// Doctor with the wrapper's new default args (mox files set, no
/// --schemas/--classifier) must pass on a fresh scaffold with ZERO model
/// warnings — the intentional new-project shape is warning-free.
#[test]
fn doctor_fresh_scaffold_has_zero_model_warnings() {
    let dir = TempDir::new().unwrap();
    cmd_init(&init_args(dir.path(), "demo-app", Some(repo_root()), false)).unwrap();
    let project = dir.path().join("demo-app");

    let summary = cmd_doctor(&DoctorArgs {
        config: project.join("domains.toml"),
        schemas: None,
        classifier: None,
        profiles_config: Some(project.join("profiles.toml")),
        mox_files: vec![project.join("model/common.mox")],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    })
    .unwrap();
    assert_eq!(
        summary.hard_failures, 0,
        "fresh scaffold must have no hard failures"
    );
    assert_eq!(
        summary.model_warnings, 0,
        "fresh scaffold doctor must produce zero model warnings"
    );
}

/// A schemas directory that exists but is empty is still a misconfiguration
/// in mox mode — the WARN stays.
#[test]
fn doctor_empty_schemas_dir_in_mox_mode_still_warns() {
    let dir = TempDir::new().unwrap();
    let config = copy_doctor_fixture_files(&dir);
    let schemas = dir.path().join("schemas");
    fs::create_dir_all(&schemas).unwrap();
    let mox = dir.path().join("model.mox");
    fs::write(&mox, DOCTOR_MOX).unwrap();

    let summary = cmd_doctor(&DoctorArgs {
        config,
        schemas: Some(schemas),
        classifier: None,
        profiles_config: None,
        mox_files: vec![mox],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    })
    .unwrap();
    assert_eq!(
        summary.model_warnings, 1,
        "empty schemas dir in mox mode must still warn"
    );
}

/// classifier.toml is only required when JSON schemas are present: absent
/// classifier + a schemas dir with JSON is a hard failure.
#[test]
fn doctor_classifier_missing_with_json_schemas_is_hard_failure() {
    let dir = TempDir::new().unwrap();
    let config = copy_fixture_domains(dir.path());
    let schemas = dir.path().join("schemas/common");
    fs::create_dir_all(&schemas).unwrap();
    fs::write(
        schemas.parent().unwrap().join("common/Widget.json"),
        r#"{ "title": "Widget", "type": "object" }"#,
    )
    .unwrap();

    let err = cmd_doctor(&DoctorArgs {
        config,
        schemas: Some(dir.path().join("schemas")),
        classifier: None,
        profiles_config: None,
        mox_files: vec![],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    })
    .unwrap_err();
    assert!(
        format!("{err}").contains("hard check"),
        "classifier missing with JSON schemas present should be a hard failure: {err}"
    );
}

/// Doctor's mox compile must accept MULTIPLE --mox-files (one per domain).
#[test]
fn doctor_validates_multiple_mox_files() {
    let dir = TempDir::new().unwrap();
    let mut args = init_args(dir.path(), "demo-app", None, false);
    args.domains = vec!["common".to_string(), "billing".to_string()];
    cmd_init(&args).unwrap();
    let project = dir.path().join("demo-app");

    let summary = cmd_doctor(&DoctorArgs {
        config: project.join("domains.toml"),
        schemas: None,
        classifier: None,
        profiles_config: None,
        mox_files: vec![
            project.join("model/common.mox"),
            project.join("model/billing.mox"),
        ],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    })
    .unwrap();
    assert_eq!(summary.hard_failures, 0);
    assert_eq!(summary.model_warnings, 0);
}

#[test]
fn doctor_fails_on_missing_schemas() {
    let dir = TempDir::new().unwrap();
    let config = dir.path().join("domains.toml");
    let classifier = dir.path().join("classifier.toml");
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    fs::copy(fixtures.join("domains.toml"), &config).unwrap();
    fs::copy(fixtures.join("classifier.toml"), &classifier).unwrap();

    let err = cmd_doctor(&DoctorArgs {
        config,
        schemas: Some(dir.path().join("schemas")),
        classifier: Some(classifier),
        profiles_config: None,
        mox_files: vec![],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    })
    .unwrap_err();
    assert!(
        format!("{err}").contains("hard check"),
        "missing schemas dir should be a hard failure: {err}"
    );
}

fn copy_fixture_domains(dir: &Path) -> PathBuf {
    let config = dir.join("domains.toml");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/domains.toml");
    fs::copy(fixture, &config).unwrap();
    config
}

const DOCTOR_MOX: &str = "package recruiting\n\nclass CandidateType {\n    String name\n}\n";

fn copy_doctor_fixture_files(dir: &TempDir) -> PathBuf {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    fs::copy(
        fixtures.join("classifier.toml"),
        dir.path().join("classifier.toml"),
    )
    .unwrap();
    copy_fixture_domains(dir.path())
}

#[test]
fn doctor_mox_mode_validates_packages_and_allows_missing_schemas() {
    let dir = TempDir::new().unwrap();
    let config = copy_doctor_fixture_files(&dir);
    let mox = dir.path().join("model.mox");
    fs::write(&mox, DOCTOR_MOX).unwrap();

    // The schemas dir does not exist; in mox mode that is the intentional
    // new-project shape (info, no warning); the compiling package matches
    // the fixture's recruiting domain.
    let summary = cmd_doctor(&DoctorArgs {
        config,
        schemas: Some(dir.path().join("schemas")),
        classifier: Some(dir.path().join("classifier.toml")),
        profiles_config: None,
        mox_files: vec![mox],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    })
    .unwrap();
    assert_eq!(summary.hard_failures, 0);
}

#[test]
fn doctor_mox_package_without_domain_entry_is_a_hard_failure() {
    let dir = TempDir::new().unwrap();
    let config = copy_doctor_fixture_files(&dir);
    let mox = dir.path().join("model.mox");
    fs::write(
        &mox,
        "package unknown_package\n\nclass Widget {\n    String name\n}\n",
    )
    .unwrap();

    let err = cmd_doctor(&DoctorArgs {
        config,
        schemas: Some(dir.path().join("schemas")),
        classifier: Some(dir.path().join("classifier.toml")),
        profiles_config: None,
        mox_files: vec![mox],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    })
    .unwrap_err();
    assert!(
        format!("{err}").contains("hard check"),
        "unmatched mox package should be a hard failure: {err}"
    );
}

#[test]
fn doctor_broken_mox_file_is_a_hard_failure() {
    let dir = TempDir::new().unwrap();
    let config = copy_doctor_fixture_files(&dir);
    let mox = dir.path().join("broken.mox");
    fs::write(
        &mox,
        "package recruiting\n\nclass Broken {\n    String\n}\n",
    )
    .unwrap();

    let err = cmd_doctor(&DoctorArgs {
        config,
        schemas: Some(dir.path().join("schemas")),
        classifier: Some(dir.path().join("classifier.toml")),
        profiles_config: None,
        mox_files: vec![mox],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    })
    .unwrap_err();
    assert!(
        format!("{err}").contains("hard check"),
        "broken mox should be a hard failure: {err}"
    );
}

#[test]
fn add_domain_appends_and_creates_mox_starter() {
    let dir = TempDir::new().unwrap();
    let config = copy_fixture_domains(dir.path());

    cmd_add_domain(&config, "billing", false).unwrap();

    let parsed = codegraph_config::config::parse_domain_config(&config).unwrap();
    assert!(parsed.domains.contains_key("billing"));
    assert_eq!(parsed.domains["billing"].label, "Billing");
    assert_eq!(parsed.domains["billing"].schema_dir, "billing");
    assert_eq!(parsed.domains["billing"].postgres_schema, "billing");

    let model = dir.path().join("model/billing.mox");
    assert!(model.is_file(), "add domain must create model/billing.mox");
    let content = fs::read_to_string(&model).unwrap();
    let compilation = rex_driver::compile_files(
        &[("model/billing.mox".to_string(), content)],
        &rex_driver::DomainImports::default(),
    );
    assert!(
        compilation.model.is_some(),
        "added domain starter must compile: {:?}",
        compilation.diagnostics
    );
    assert_eq!(compilation.model.unwrap().packages[0].name, "billing");

    assert!(
        !dir.path().join("schemas/billing").exists(),
        "add domain must not create a schemas/ directory"
    );

    let err = cmd_add_domain(&config, "billing", false).unwrap_err();
    assert!(
        format!("{err}").contains("already exists"),
        "duplicate domain should be rejected: {err}"
    );
}

#[test]
fn add_domain_rejects_duplicate() {
    let dir = TempDir::new().unwrap();
    let config = copy_fixture_domains(dir.path());

    cmd_add_domain(&config, "billing", false).unwrap();
    let err = cmd_add_domain(&config, "billing", false).unwrap_err();
    assert!(
        format!("{err}").contains("already exists"),
        "duplicate domain should be rejected: {err}"
    );
}

#[test]
fn add_domain_normalizes_name() {
    let dir = TempDir::new().unwrap();
    let config = copy_fixture_domains(dir.path());

    cmd_add_domain(&config, "Billing Accounts", false).unwrap();

    let parsed = codegraph_config::config::parse_domain_config(&config).unwrap();
    assert!(parsed.domains.contains_key("billing_accounts"));
    let model = dir.path().join("model/billing_accounts.mox");
    assert!(model.is_file());
    let content = fs::read_to_string(&model).unwrap();
    let compilation = rex_driver::compile_files(
        &[("model/billing_accounts.mox".to_string(), content)],
        &rex_driver::DomainImports::default(),
    );
    assert!(
        compilation.model.is_some(),
        "normalized domain starter must compile: {:?}",
        compilation.diagnostics
    );
    assert_eq!(
        compilation.model.unwrap().packages[0].name,
        "billing_accounts"
    );
}

#[test]
fn rev_accessor_is_hex_or_empty() {
    let rev = codegraph::rev::codegraph_rev();
    if !rev.is_empty() {
        assert_eq!(rev.len(), 40, "git SHAs are 40 hex chars: {rev}");
        assert!(
            rev.chars().all(|c| c.is_ascii_hexdigit()),
            "rev should be hex: {rev}"
        );
    }
}

// ── doctor: mox `import schema` validation (issue #230) ──

fn write_doctor_import_fixture(dir: &TempDir, mox_body: &str, json: Option<&str>) {
    let config = copy_doctor_fixture_files(dir);
    assert!(config.is_file());
    if let Some(json) = json {
        let schema = dir.path().join("schemas/item.json");
        fs::create_dir_all(schema.parent().unwrap()).unwrap();
        fs::write(schema, json).unwrap();
    }
    let mox = dir.path().join("model.mox");
    fs::write(&mox, mox_body).unwrap();
}

const IMPORT_MOX: &str = "package recruiting\n\nimport schema \"schemas/item.json\" as Item\n\nclass CandidateType {\n    String name\n}\n";

#[test]
fn doctor_valid_import_passes() {
    let dir = TempDir::new().unwrap();
    write_doctor_import_fixture(
        &dir,
        IMPORT_MOX,
        Some(r#"{ "title": "ItemType", "type": "object" }"#),
    );

    cmd_doctor(&DoctorArgs {
        config: dir.path().join("domains.toml"),
        schemas: Some(dir.path().join("schemas")),
        classifier: Some(dir.path().join("classifier.toml")),
        profiles_config: None,
        mox_files: vec![dir.path().join("model.mox")],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    })
    .unwrap();
}

#[test]
fn doctor_missing_import_target_is_a_hard_failure() {
    let dir = TempDir::new().unwrap();
    write_doctor_import_fixture(&dir, IMPORT_MOX, None);

    let err = cmd_doctor(&DoctorArgs {
        config: dir.path().join("domains.toml"),
        schemas: Some(dir.path().join("schemas")),
        classifier: Some(dir.path().join("classifier.toml")),
        profiles_config: None,
        mox_files: vec![dir.path().join("model.mox")],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    })
    .unwrap_err();
    assert!(
        format!("{err}").contains("hard check"),
        "missing import target should be a hard failure: {err}"
    );
}

#[test]
fn doctor_invalid_import_json_is_a_hard_failure() {
    let dir = TempDir::new().unwrap();
    write_doctor_import_fixture(&dir, IMPORT_MOX, Some("{ not json"));

    let err = cmd_doctor(&DoctorArgs {
        config: dir.path().join("domains.toml"),
        schemas: Some(dir.path().join("schemas")),
        classifier: Some(dir.path().join("classifier.toml")),
        profiles_config: None,
        mox_files: vec![dir.path().join("model.mox")],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    })
    .unwrap_err();
    assert!(
        format!("{err}").contains("hard check"),
        "invalid import JSON should be a hard failure: {err}"
    );
}

// ── Rosetta project lifecycle (issue #260) ──

fn init_rosetta_args(dir: &Path, name: &str, domains: &[&str]) -> InitArgs {
    let mut args = init_args(dir, name, None, false);
    args.rosetta = true;
    args.domains = domains.iter().map(|d| d.to_string()).collect();
    args
}

/// `--rosetta` scaffolds a rosetta-first project: `model/<domain>.rosetta`
/// starters (sigil-verified), no `.mox`, no schemas/, no classifier.toml,
/// `rosetta_backend = true` in profiles.toml, and rosetta-first justfile +
/// ops manifest wiring. Default (non---rosetta) output is untouched: every
/// pre-existing test above pins it.
#[test]
fn init_rosetta_scaffolds_expected_tree() {
    let dir = TempDir::new().unwrap();
    let mut args = init_rosetta_args(dir.path(), "demo-app", &["common", "billing"]);
    args.rev = Some("abc123".to_string());
    cmd_init(&args).unwrap();
    let project = dir.path().join("demo-app");

    // One .rosetta starter per domain, no .mox anywhere.
    assert!(project.join("model/common.rosetta").is_file());
    assert!(project.join("model/billing.rosetta").is_file());
    assert!(!project.join("model/common.mox").exists());
    assert_mox_first_layout(&project);

    // Starter verifies through the sigil pipeline and carries the
    // `{app_name}.{domain}` namespace + starter type/enum.
    for domain in ["common", "billing"] {
        let path = project.join("model").join(format!("{domain}.rosetta"));
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.contains(&format!("namespace demo_app.{domain}")),
            "namespace must be {{app_name}}.{{domain}}:\n{content}"
        );
        assert!(content.contains("version \"1.0.0\""), "{content}");
        let pascal = "common" == domain;
        assert!(
            content.contains(&format!(
                "type {}Type:",
                if pascal { "Common" } else { "Billing" }
            )),
            "starter type must exist:\n{content}"
        );
        let check = codegraph::init::rosetta_model::verify_rosetta_sources(&[
            codegraph::init::rosetta_model::RosettaFileCheck {
                name: path.display().to_string(),
                text: content,
            },
        ]);
        assert!(
            check.hard_errors.is_empty(),
            "{domain} starter must pass sigil verification: {:?}",
            check.hard_errors
        );
    }

    // domains.toml parses, no entities key, points at the .rosetta models.
    let domains =
        codegraph_config::config::parse_domain_config(&project.join("domains.toml")).unwrap();
    assert!(domains.domains.contains_key("common"));
    assert!(domains.domains.contains_key("billing"));
    let domains_raw = fs::read_to_string(project.join("domains.toml")).unwrap();
    assert!(
        domains_raw.contains("model/common.rosetta"),
        "domains.toml should point at the rosetta model:\n{domains_raw}"
    );
    assert!(!domains_raw.contains(".mox"), "{domains_raw}");

    // profiles.toml carries the rosetta feature and still resolves.
    let profiles_raw = fs::read_to_string(project.join("profiles.toml")).unwrap();
    assert!(
        profiles_raw.contains("rosetta_backend = true"),
        "profiles.toml must enable rosetta_backend:\n{profiles_raw}"
    );
    let resolved =
        load_and_resolve_profile(&project.join("profiles.toml"), "default", None).unwrap();
    BuildPlan::from_profile(&resolved, &CapabilityRegistry::new()).unwrap();

    // justfile recipes pass --rosetta-files per domain, not --mox-files.
    let justfile = fs::read_to_string(project.join("justfile")).unwrap();
    assert!(
        justfile.contains("--rosetta-files model/common.rosetta"),
        "justfile recipes must pass --rosetta-files per domain:\n{justfile}"
    );
    assert!(!justfile.contains("--mox-files"), "{justfile}");

    // Ops manifest seeds rosetta_files and loads via OpsConfig::load.
    let manifest_raw = fs::read_to_string(project.join("codegraph-ops.toml")).unwrap();
    assert!(
        manifest_raw
            .contains(r#"rosetta_files = ["model/common.rosetta", "model/billing.rosetta"]"#),
        "manifest must list one rosetta file per domain in domain order:\n{manifest_raw}"
    );
    assert!(!manifest_raw.contains("mox_files"), "{manifest_raw}");
    let cfg = codegraph_ops::OpsConfig::load(&project.join("codegraph-ops.toml"))
        .expect("emitted manifest must load via OpsConfig::load");
    assert_eq!(
        cfg.manifest.rosetta_files,
        vec![
            "model/common.rosetta".to_string(),
            "model/billing.rosetta".to_string()
        ]
    );
}

/// CI workflow coherence for rosetta-first scaffolds (issue #346): the
/// generate step must pass --rosetta-files per domain (mirroring the
/// justfile's per-domain loop), never --mox-files; the ops-api service must
/// be a pgmq-bundled postgres image (the generated domain-event triggers
/// require the pgmq extension — plain postgres inserts 500); and the
/// app_user serving pool URL must be pinned to the migration's
/// `app_user`/`app_user_pass` convention.
#[test]
fn init_rosetta_ci_is_rosetta_first() {
    let dir = TempDir::new().unwrap();
    let mut args = init_rosetta_args(dir.path(), "demo-app", &["common", "billing"]);
    args.rev = Some("abc123".to_string());
    cmd_init(&args).unwrap();
    let project = dir.path().join("demo-app");

    let ci = fs::read_to_string(project.join(".github/workflows/ci.yml")).unwrap();
    assert!(
        ci.contains("--rosetta-files model/common.rosetta"),
        "ci.yml generate step must pass --rosetta-files per domain:\n{ci}"
    );
    assert!(
        ci.contains("--rosetta-files model/billing.rosetta"),
        "ci.yml generate step must pass --rosetta-files per domain:\n{ci}"
    );
    assert!(
        !ci.contains("--mox-files"),
        "rosetta-first ci.yml must not reference --mox-files:\n{ci}"
    );
    assert!(
        ci.contains("pgmq"),
        "ops-api postgres service must be a pgmq-capable image \
         (domain-event triggers need the pgmq extension):\n{ci}"
    );
    assert!(
        ci.contains("APP_DATABASE_URL"),
        "ci.yml must set the app_user serving pool URL:\n{ci}"
    );
}

/// THE rosetta acceptance gate: a fresh `init --rosetta` scaffold must
/// generate with ONLY --rosetta-files + --config (no --schemas, no
/// --classifier), producing DDL for the starter type and its status
/// codelist.
#[tokio::test]
async fn init_rosetta_scaffold_runs_rosetta_first() {
    let dir = TempDir::new().unwrap();
    let root = repo_root().canonicalize().unwrap();
    let mut args = init_rosetta_args(dir.path(), "demo-app", &["common"]);
    args.codegraph_path = Some(root);
    cmd_init(&args).unwrap();
    let project = dir.path().join("demo-app");

    let rosetta_files = vec![project.join("model/common.rosetta")];
    let output = project.join("generated");
    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &project.join("domains.toml"),
        output: &output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(project.join("profiles.toml")),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &[],
        rosetta_files: &rosetta_files,
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

    let generated: Vec<PathBuf> = walkdir::WalkDir::new(&output)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .collect();
    assert!(
        !generated.is_empty(),
        "rosetta-first generation must produce files"
    );

    let migrations_dir = output.join("migrations");
    let mut ddl = String::new();
    for entry in fs::read_dir(&migrations_dir).unwrap_or_else(|e| {
        panic!(
            "migrations dir missing under {}: {e}",
            migrations_dir.display()
        )
    }) {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) == Some("sql") {
            ddl.push_str(&fs::read_to_string(&path).unwrap());
            ddl.push('\n');
        }
    }
    assert!(
        ddl.contains("CREATE TABLE IF NOT EXISTS common.common "),
        "DDL must contain the starter type's table (CommonType strips the \
         Type suffix → table common, the codelist is common.common_status):\n{ddl}"
    );
    assert!(
        ddl.contains("common_status"),
        "DDL must contain the starter enum's codelist table (CommonStatus → common_status):\n{ddl}"
    );
}

/// Doctor with --rosetta-files only (the wrapper's new default on rosetta
/// scaffolds) must pass with zero hard failures and zero model warnings.
#[test]
fn doctor_rosetta_starter_has_zero_model_warnings() {
    let dir = TempDir::new().unwrap();
    let mut args = init_rosetta_args(dir.path(), "demo-app", &["common"]);
    args.codegraph_path = Some(repo_root());
    cmd_init(&args).unwrap();
    let project = dir.path().join("demo-app");

    let summary = cmd_doctor(&DoctorArgs {
        config: project.join("domains.toml"),
        schemas: None,
        classifier: None,
        profiles_config: Some(project.join("profiles.toml")),
        mox_files: vec![],
        rosetta_files: vec![project.join("model/common.rosetta")],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    })
    .unwrap();
    assert_eq!(summary.hard_failures, 0);
    assert_eq!(summary.model_warnings, 0);
}

/// A rosetta file failing sigil verification (parse/lower/resolve) is a
/// hard failure, mirroring check_mox_files semantics.
#[test]
fn doctor_broken_rosetta_file_is_a_hard_failure() {
    let dir = TempDir::new().unwrap();
    let config = copy_fixture_domains(dir.path());
    let broken = dir.path().join("model/broken.rosetta");
    fs::create_dir_all(broken.parent().unwrap()).unwrap();
    fs::write(&broken, "namespace recruiting\n\ntype Broken:\n\tname\n").unwrap();

    let err = cmd_doctor(&DoctorArgs {
        config,
        schemas: None,
        classifier: None,
        profiles_config: None,
        mox_files: vec![],
        rosetta_files: vec![broken],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    })
    .unwrap_err();
    assert!(
        format!("{err}").contains("hard check"),
        "broken rosetta file should be a hard failure: {err}"
    );
}

/// A namespace whose last segment matches no domains.toml key is a WARNING
/// (not a hard failure): compute_generation_order silently drops such
/// schemas, so the project would generate nothing for it.
#[test]
fn doctor_rosetta_namespace_without_domain_entry_warns() {
    let dir = TempDir::new().unwrap();
    let config = copy_fixture_domains(dir.path());
    let file = dir.path().join("model/orphan.rosetta");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        &file,
        "namespace nz.example.orphan\nversion \"1.0.0\"\n\ntype OrphanType:\n\tname string (1..1)\n",
    )
    .unwrap();

    let summary = cmd_doctor(&DoctorArgs {
        config,
        schemas: None,
        classifier: None,
        profiles_config: None,
        mox_files: vec![],
        rosetta_files: vec![file],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    })
    .unwrap();
    assert_eq!(
        summary.hard_failures, 0,
        "namespace mismatch warns, not fails"
    );
    assert!(
        summary.soft_warnings >= 1 && summary.model_warnings >= 1,
        "unmatched namespace must warn: {summary:?}"
    );
}

/// `import <ns>.*` line-scan: an imported namespace with no file among
/// --rosetta-files whose last segment also matches no domain key warns.
#[test]
fn doctor_rosetta_import_without_matching_file_warns() {
    let dir = TempDir::new().unwrap();
    let config = copy_fixture_domains(dir.path());
    let file = dir.path().join("model/recruiting.rosetta");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        &file,
        concat!(
            "namespace nz.example.recruiting\nversion \"1.0.0\"\n\n",
            "import nz.example.ghost.*\n\ntype CandidateType:\n\tname string (1..1)\n"
        ),
    )
    .unwrap();

    let summary = cmd_doctor(&DoctorArgs {
        config,
        schemas: None,
        classifier: None,
        profiles_config: None,
        mox_files: vec![],
        rosetta_files: vec![file],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    })
    .unwrap();
    assert_eq!(summary.hard_failures, 0);
    assert!(
        summary.soft_warnings >= 1,
        "import of an unprovided, domain-less namespace must warn: {summary:?}"
    );
}

/// `add domain` on a rosetta-first project (auto-detected via
/// model/*.rosetta, or forced with --rosetta) creates a sigil-verified
/// .rosetta starter and appends the domains.toml entry.
#[test]
fn add_domain_rosetta_mode_creates_rosetta_starter() {
    let dir = TempDir::new().unwrap();
    let args = init_rosetta_args(dir.path(), "demo-app", &["common"]);
    cmd_init(&args).unwrap();
    let project = dir.path().join("demo-app");
    let config = project.join("domains.toml");

    // Auto-detect: model/common.rosetta present.
    cmd_add_domain(&config, "billing", false).unwrap();

    let parsed = codegraph_config::config::parse_domain_config(&config).unwrap();
    assert!(parsed.domains.contains_key("billing"));

    let model = project.join("model/billing.rosetta");
    assert!(
        model.is_file(),
        "add domain must create model/billing.rosetta"
    );
    assert!(
        !project.join("model/billing.mox").exists(),
        "rosetta mode must not create a .mox starter"
    );
    let content = fs::read_to_string(&model).unwrap();
    assert!(
        content.contains("namespace demo_app.billing"),
        "added starter must carry the project-namespaced namespace:\n{content}"
    );
    let check = codegraph::init::rosetta_model::verify_rosetta_sources(&[
        codegraph::init::rosetta_model::RosettaFileCheck {
            name: "model/billing.rosetta".to_string(),
            text: content,
        },
    ]);
    assert!(
        check.hard_errors.is_empty(),
        "added starter must pass sigil verification: {:?}",
        check.hard_errors
    );

    assert!(
        !project.join("schemas/billing").exists(),
        "add domain must not create a schemas/ directory"
    );

    let err = cmd_add_domain(&config, "billing", false).unwrap_err();
    assert!(
        format!("{err}").contains("already exists"),
        "duplicate domain should be rejected: {err}"
    );
}

// ── doctor --deploy-files (rexlang #49) ────────────────────────────────────

/// A self-contained standalone deployment (no imports): compiles clean on
/// a monolith scaffold — the topology cross-check agrees (standalone
/// profile, monolith plan), and the fresh scaffold's domains.toml declares
/// no entities so coverage is silent.
#[test]
fn doctor_deploy_files_compile_clean_with_matching_topology() {
    let dir = TempDir::new().unwrap();
    cmd_init(&init_args(dir.path(), "demo-app", None, false)).unwrap();
    let project = dir.path().join("demo-app");
    let deploy = project.join("svc.deploy");
    fs::write(
        &deploy,
        concat!(
            "application Shop {\n",
            "    component api: api { replicas: 2 }\n",
            "    component db: database { engine: sqlite }\n",
            "    connects api -> db\n",
            "}\n",
            "\n",
            "profile local {\n",
            "    target: standalone\n",
            "}\n",
            "\n",
            "deployment dev for Shop {\n",
            "    use local\n",
            "}\n",
        ),
    )
    .unwrap();

    let summary = cmd_doctor(&DoctorArgs {
        config: project.join("domains.toml"),
        schemas: None,
        classifier: None,
        profiles_config: Some(project.join("profiles.toml")),
        mox_files: vec![project.join("model/common.mox")],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![deploy],
        ifml_files: vec![],
    })
    .unwrap();
    assert_eq!(
        summary.hard_failures, 0,
        "a clean deploy agreeing with the monolith plan must not fail"
    );
}

/// Cross-check (a), profile side: a cloudflareWorkers profile with the
/// monolith topology is a HARD failure — the plan cannot host the
/// declared components.
#[test]
fn doctor_deploy_workers_profile_conflicts_with_monolith_topology() {
    let dir = TempDir::new().unwrap();
    cmd_init(&init_args(dir.path(), "demo-app", None, false)).unwrap();
    let project = dir.path().join("demo-app");
    let deploy = project.join("svc.deploy");
    fs::write(
        &deploy,
        concat!(
            "application Shop {\n",
            "    component api: api { replicas: 2 }\n",
            "    component db: database { engine: d1 }\n",
            "    connects api -> db\n",
            "}\n",
            "\n",
            "profile edge {\n",
            "    target: cloudflareWorkers\n",
            "}\n",
            "\n",
            "deployment prod for Shop {\n",
            "    use edge\n",
            "}\n",
        ),
    )
    .unwrap();

    let err = cmd_doctor(&DoctorArgs {
        config: project.join("domains.toml"),
        schemas: None,
        classifier: None,
        profiles_config: Some(project.join("profiles.toml")),
        mox_files: vec![project.join("model/common.mox")],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![deploy],
        ifml_files: vec![],
    })
    .unwrap_err();
    assert!(
        format!("{err}").contains("hard check"),
        "workers profile on a monolith plan must be a hard failure: {err}"
    );
}

/// Cross-check (a), topology side: the Workers topology with no
/// cloudflareWorkers profile in any deploy model is a HARD failure.
#[test]
fn doctor_workers_topology_without_deploy_profile_is_hard_failure() {
    let dir = TempDir::new().unwrap();
    let mut args = init_args(dir.path(), "demo-app", None, false);
    args.deployment_topology = "workers".to_string();
    cmd_init(&args).unwrap();
    let project = dir.path().join("demo-app");
    let deploy = project.join("svc.deploy");
    fs::write(
        &deploy,
        concat!(
            "application Shop {\n",
            "    component api: api { replicas: 2 }\n",
            "    component db: database { engine: sqlite }\n",
            "    connects api -> db\n",
            "}\n",
            "\n",
            "profile local {\n",
            "    target: standalone\n",
            "}\n",
            "\n",
            "deployment dev for Shop {\n",
            "    use local\n",
            "}\n",
        ),
    )
    .unwrap();

    let err = cmd_doctor(&DoctorArgs {
        config: project.join("domains.toml"),
        schemas: None,
        classifier: None,
        profiles_config: Some(project.join("profiles.toml")),
        mox_files: vec![project.join("model/common.mox")],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![deploy],
        ifml_files: vec![],
    })
    .unwrap_err();
    assert!(
        format!("{err}").contains("hard check"),
        "workers topology without a workers profile must be a hard failure: {err}"
    );
}

/// Cross-check (b): a domains.toml domain with entities but no matching
/// application component WARNs (soft) — never a hard failure.
#[test]
fn doctor_deploy_component_coverage_gap_warns_but_does_not_fail() {
    let dir = TempDir::new().unwrap();
    cmd_init(&init_args(dir.path(), "demo-app", None, false)).unwrap();
    let project = dir.path().join("demo-app");
    let config = project.join("domains.toml");
    let toml = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("{toml}\nentities = [\"Widget\"]\n")).unwrap();
    // One application whose components do NOT carry the domain's name:
    // the coverage gap warns.
    let deploy = project.join("svc.deploy");
    fs::write(
        &deploy,
        concat!(
            "application Shop {\n",
            "    component api: api { replicas: 2 }\n",
            "    component db: database { engine: sqlite }\n",
            "    connects api -> db\n",
            "}\n",
            "\n",
            "profile local {\n",
            "    target: standalone\n",
            "}\n",
            "\n",
            "deployment dev for Shop {\n",
            "    use local\n",
            "}\n",
        ),
    )
    .unwrap();

    let summary = cmd_doctor(&DoctorArgs {
        config,
        schemas: None,
        classifier: None,
        profiles_config: Some(project.join("profiles.toml")),
        mox_files: vec![project.join("model/common.mox")],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![deploy],
        ifml_files: vec![],
    })
    .unwrap();
    assert_eq!(
        summary.hard_failures, 0,
        "a coverage gap is advisory, never a hard failure"
    );
    assert!(
        summary.soft_warnings >= 1,
        "the coverage gap must warn (soft)"
    );
}

// ── doctor --ifml-files (issue #468 phase 1) ───────────────────────────────

const DOCTOR_IFML: &str = concat!(
    "domain \"portal\" { schema \"portal\"; }\n",
    "\n",
    "import \"portal.actor\";\n",
    "\n",
    "view \"Dashboard\" {\n",
    "    label \"Dashboard\";\n",
    "\n",
    "    component \"summary\" {\n",
    "        type: details;\n",
    "        data: Thing;\n",
    "        fields: [title];\n",
    "\n",
    "        on edit -> navigate(\"Editor\");\n",
    "    }\n",
    "}\n",
);

const DOCTOR_IFML_ACTOR: &str = concat!(
    "import \"portal.mox\"\n",
    "\n",
    "actors Portal {\n",
    "    actor Admin\n",
    "\n",
    "    capability ManageThing on Thing\n",
    "\n",
    "    grant Admin {\n",
    "        permit ManageThing\n",
    "    }\n",
    "}\n",
);

const DOCTOR_IFML_MOX: &str = concat!(
    "package portal\n",
    "\n",
    "class Thing {\n",
    "    String title\n",
    "}\n",
);

/// A well-formed .ifml with a resolvable policy import parses clean and
/// contributes zero hard failures.
#[test]
fn doctor_ifml_files_parse_clean_and_resolve_policy() {
    let dir = TempDir::new().unwrap();
    cmd_init(&init_args(dir.path(), "demo-app", None, false)).unwrap();
    let project = dir.path().join("demo-app");
    fs::write(project.join("portal.mox"), DOCTOR_IFML_MOX).unwrap();
    fs::write(project.join("portal.actor"), DOCTOR_IFML_ACTOR).unwrap();
    let ifml = project.join("app.ifml");
    fs::write(&ifml, DOCTOR_IFML).unwrap();

    let summary = cmd_doctor(&DoctorArgs {
        config: project.join("domains.toml"),
        schemas: None,
        classifier: None,
        profiles_config: Some(project.join("profiles.toml")),
        mox_files: vec![project.join("model/common.mox")],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![ifml],
    })
    .unwrap();
    assert_eq!(
        summary.hard_failures, 0,
        "a clean ifml model with a compiling policy import must not fail"
    );
}

/// An `import "….actor"` whose file is missing is a HARD failure: the run
/// path warns-and-continues past it, so doctor is the only gate.
#[test]
fn doctor_ifml_missing_policy_import_is_hard_failure() {
    let dir = TempDir::new().unwrap();
    cmd_init(&init_args(dir.path(), "demo-app", None, false)).unwrap();
    let project = dir.path().join("demo-app");
    let ifml = project.join("app.ifml");
    fs::write(&ifml, DOCTOR_IFML).unwrap();

    let err = cmd_doctor(&DoctorArgs {
        config: project.join("domains.toml"),
        schemas: None,
        classifier: None,
        profiles_config: Some(project.join("profiles.toml")),
        mox_files: vec![project.join("model/common.mox")],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![ifml],
    })
    .unwrap_err();
    assert!(
        format!("{err}").contains("hard check"),
        "a missing policy import must be a hard failure: {err}"
    );
}

/// A .ifml that does not parse is a hard failure.
#[test]
fn doctor_ifml_parse_error_is_hard_failure() {
    let dir = TempDir::new().unwrap();
    cmd_init(&init_args(dir.path(), "demo-app", None, false)).unwrap();
    let project = dir.path().join("demo-app");
    let ifml = project.join("broken.ifml");
    fs::write(&ifml, "view { this is not ifml").unwrap();

    let err = cmd_doctor(&DoctorArgs {
        config: project.join("domains.toml"),
        schemas: None,
        classifier: None,
        profiles_config: Some(project.join("profiles.toml")),
        mox_files: vec![project.join("model/common.mox")],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![ifml],
    })
    .unwrap_err();
    assert!(
        format!("{err}").contains("hard check"),
        "an unparseable ifml file must be a hard failure: {err}"
    );
}
