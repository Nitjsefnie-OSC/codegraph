//! Cross-run generation determinism gate (issue #332).
//!
//! Identical inputs must produce byte-identical generated trees — identical
//! relative path sets, identical file contents, and identical migration
//! filename sequences — no matter how many times or in how many processes
//! the pipeline runs. Anything else breaks snapshot reviews, `git diff`
//! hygiene on generated output, downstream byte-diff verification, and
//! migration-order-sensitive consumers.
//!
//! WHY CROSS-PROCESS: Rust's `HashMap`/`HashSet` iteration order depends on
//! the per-`RandomState` SipHash keys, which are randomized per process.
//! Two runs inside one process share the process's hash seeds, so
//! in-process stability does NOT prove cross-process stability — any
//! unordered container iteration that leaks into emission only manifests
//! when a fresh process (fresh seed) runs the pipeline. The cross-process
//! tests therefore re-exec this test binary (`DETERMINISM_WORKER=1`, the
//! standard self-spawn pattern) so every generation happens under an
//! independent seed.
//!
//! Fixture choice: `tests/fixtures/rosetta_bridge/` (2 rosetta files, one
//! cross-domain import) is the smallest fixture that drives the full
//! ingest → classify → generate pipeline including migration-number
//! allocation and domain topological ordering. Each run takes a few seconds
//! in debug mode, keeping the always-on pair-of-runs and the 3 spawned
//! cross-process runs well under a couple of minutes total.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Fixture a generation run should use.
#[derive(Clone, Copy, PartialEq)]
enum Fixture {
    /// `tests/fixtures/rosetta_bridge` — small, always-on.
    RosettaBridge,
    /// `tests/fixtures/flywheel` — the 11-file consumer corpus (heavy).
    Flywheel,
}

impl Fixture {
    fn dir(self) -> PathBuf {
        match self {
            Fixture::RosettaBridge => {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rosetta_bridge")
            }
            Fixture::Flywheel => {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/flywheel")
            }
        }
    }

    /// Model files in consumer dependency order (mirrors the fixtures'
    /// justfile sequences: base namespaces before importers).
    fn model_names(self) -> &'static [&'static str] {
        match self {
            Fixture::RosettaBridge => &["partners", "store"],
            Fixture::Flywheel => &[
                "common",
                "reference",
                "profiles",
                "accounts",
                "company",
                "taxonomy",
                "qanda",
                "jobs",
                "marketplace",
                "inbox",
                "pricing",
            ],
        }
    }

    fn label(self) -> &'static str {
        match self {
            Fixture::RosettaBridge => "bridge",
            Fixture::Flywheel => "flywheel",
        }
    }
}

/// Recursively collect `relative path -> bytes` for every file under `root`.
fn collect_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
                    .replace('\\', "/");
                let bytes = fs::read(&path).unwrap();
                out.insert(rel, bytes);
            }
        }
    }
    walk(root, root, &mut out);
    out
}

/// The migration filename sequence of a generated tree (sorted). Migration
/// sequence numbers are allocated from the generation order (issue #332), so
/// two runs of the same input must agree on the full filename list — a
/// divergence here means emission order (or the entity set) varied.
fn migration_sequence(tree: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    tree.keys()
        .filter(|p| p.starts_with("migrations/") && p.ends_with(".sql"))
        .cloned()
        .collect()
}

/// Run the full pipeline (ingest + classify + generate) for `fixture` into
/// `output_dir` via the stable `driver::run` entry point.
///
/// `codegraph_rev` is pinned explicitly so the generated Cargo.tomls (and
/// the `.codegraph-manifest.json` `codegraphCommit`) do not depend on the
/// spawning process's cwd or the repository's git state mid-test.
async fn run_generation(
    fixture: Fixture,
    output_dir: &Path,
) -> Result<(), codegraph::error::Error> {
    let fixture_dir = fixture.dir();
    let config_path = fixture_dir.join("domains.toml");
    let rosetta_files: Vec<PathBuf> = fixture
        .model_names()
        .iter()
        .map(|name| fixture_dir.join("model").join(format!("{name}.rosetta")))
        .collect();

    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &config_path,
        output: output_dir,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(PathBuf::from("definitely-absent-profiles.toml")),
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
        codegraph_rev: Some("determinism-test-pin".to_string()),
        check: false,
        ux_rules: None,
    })
    .await
}

/// Assert two generated trees are identical: same relative path sets, same
/// migration filename sequences, byte-identical contents. Returns the
/// migration sequences on success for cross-run comparison.
fn assert_trees_identical(a: &BTreeMap<String, Vec<u8>>, b: &BTreeMap<String, Vec<u8>>) {
    let a_names: Vec<&String> = a.keys().collect();
    let b_names: Vec<&String> = b.keys().collect();
    assert_eq!(
        a_names, b_names,
        "generated file sets differ between runs (paths only in one tree are listed)"
    );
    assert_eq!(
        migration_sequence(a),
        migration_sequence(b),
        "migration filename sequences differ between runs"
    );
    let mut diffs: Vec<String> = Vec::new();
    for (name, bytes_a) in a {
        let bytes_b = &b[name];
        if bytes_a != bytes_b {
            diffs.push(name.clone());
        }
    }
    assert!(
        diffs.is_empty(),
        "generation is not deterministic — {} files differ between runs:\n{}",
        diffs.len(),
        diffs
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

// ── Self-spawn worker protocol ──────────────────────────────────────────

const WORKER_ENV: &str = "DETERMINISM_WORKER";
const WORKER_FIXTURE_ENV: &str = "DETERMINISM_FIXTURE";
const WORKER_OUT_ENV: &str = "DETERMINISM_OUTPUT_DIR";

/// Worker entry point: re-executed by the cross-process tests with
/// `DETERMINISM_WORKER=1` plus the fixture label and output dir in the
/// environment. A no-op (passes immediately) under a normal test run.
#[test]
fn determinism_worker() {
    let Ok(out) = std::env::var(WORKER_OUT_ENV) else {
        return;
    };
    let fixture_label = std::env::var(WORKER_FIXTURE_ENV).unwrap_or_else(|_| "bridge".into());
    let fixture = match fixture_label.as_str() {
        "flywheel" => Fixture::Flywheel,
        _ => Fixture::RosettaBridge,
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(run_generation(fixture, Path::new(&out)))
        .expect("worker generation failed");
}

/// Spawn this test binary once as a worker that generates `fixture` into
/// `out` under a fresh process (fresh RandomState seed).
fn spawn_worker(fixture: Fixture, out: &Path) {
    let exe = std::env::current_exe().expect("current_exe");
    let output = Command::new(exe)
        .args([
            "--exact",
            "determinism_worker",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(WORKER_ENV, "1")
        .env(WORKER_FIXTURE_ENV, fixture.label())
        .env(WORKER_OUT_ENV, out)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to spawn determinism worker");
    assert!(
        output.status.success(),
        "determinism worker failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Always-on tests ─────────────────────────────────────────────────────

/// In-process pair of runs: same hash seeds, sequential. This pins the
/// deterministic *data flow* (ordering, migration allocation); it can never
/// prove cross-process stability because both runs share the process's
/// RandomState seeds — see the cross-process test below.
#[tokio::test]
async fn generated_output_is_byte_identical_across_runs() {
    let dir_a = tempfile::TempDir::new().unwrap();
    let dir_b = tempfile::TempDir::new().unwrap();

    run_generation(Fixture::RosettaBridge, &dir_a.path().join("out"))
        .await
        .unwrap();
    run_generation(Fixture::RosettaBridge, &dir_b.path().join("out"))
        .await
        .unwrap();

    let tree_a = collect_tree(&dir_a.path().join("out"));
    let tree_b = collect_tree(&dir_b.path().join("out"));
    assert!(!tree_a.is_empty(), "first run generated nothing");
    assert_trees_identical(&tree_a, &tree_b);
}

/// Cross-process: N=3 independent processes (independent RandomState seeds)
/// each generate the rosetta_bridge fixture; all three trees must be
/// byte-identical. This is the honest determinism gate — unordered
/// HashMap/HashSet iteration leaking into emission manifests only under
/// per-process seeds.
#[test]
fn generated_output_is_byte_identical_across_processes() {
    const N: usize = 3;
    let parent = tempfile::TempDir::new().unwrap();
    let mut trees = Vec::new();
    for i in 0..N {
        let out = parent.path().join(format!("run{i}"));
        spawn_worker(Fixture::RosettaBridge, &out);
        trees.push(collect_tree(&out));
    }
    for tree in trees.iter().skip(1) {
        assert_trees_identical(&trees[0], tree);
    }
}

// ── Heavy variant ───────────────────────────────────────────────────────

/// Same cross-process gate over the flywheel consumer corpus (11 rosetta
/// files, ~156 constructs). HEAVY: a single debug-mode generation takes on
/// the order of 30 minutes (`flywheel_corpus_tests` documents the same
/// cost), and this spawns TWO runs — budget roughly an hour. Run explicitly:
///
/// ```text
/// cargo test -p codegraph --test determinism_tests -- --ignored --nocapture
/// ```
#[test]
#[ignore = "heavy: 2 spawned debug-mode runs over the flywheel corpus, ~30 min each (~1 h total)"]
fn flywheel_corpus_is_byte_identical_across_processes() {
    let parent = tempfile::TempDir::new().unwrap();
    let mut trees = Vec::new();
    for i in 0..2 {
        let out = parent.path().join(format!("run{i}"));
        spawn_worker(Fixture::Flywheel, &out);
        trees.push(collect_tree(&out));
    }
    assert_trees_identical(&trees[0], &trees[1]);
}

// ── In-process two-thread pin (JSON fixture, committed earlier) ─────────

#[path = "test_framework/mod.rs"]
mod test_framework;

/// Distinct OS threads carry distinct RandomState seeds, so this catches
/// seed-dependent emission *within* one process (a cheaper cousin of the
/// cross-process gate) over the JSON-schema fixture with the plan-driven
/// default profile. The rosetta fixtures above additionally exercise the
/// driver's ingest/classify passes.
#[tokio::test(flavor = "multi_thread")]
async fn two_thread_generation_is_byte_identical() {
    let dir_a = tempfile::TempDir::new().unwrap();
    let dir_b = tempfile::TempDir::new().unwrap();
    let path_a = dir_a.path().to_path_buf();
    let path_b = dir_b.path().to_path_buf();

    let (res_a, res_b) = tokio::join!(
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(run_json_schema_generation(&path_a))
        }),
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(run_json_schema_generation(&path_b))
        }),
    );
    res_a.unwrap();
    res_b.unwrap();

    let tree_a = collect_tree(dir_a.path());
    let tree_b = collect_tree(dir_b.path());
    assert_trees_identical(&tree_a, &tree_b);
}

/// Run the JSON-schema fixture pipeline into `output_dir` (mirrors
/// `grafeo_e2e_tests::generate_full_app` minus the gRPC generators).
async fn run_json_schema_generation(output_dir: &Path) {
    let config =
        codegraph_config::config::parse_domain_config(Path::new("tests/fixtures/domains.toml"))
            .unwrap();
    let classifier = codegraph_classifier::config::parse_classifier_config(Path::new(
        "tests/fixtures/classifier.toml",
    ))
    .unwrap();
    let entity_names: std::collections::HashSet<String> = config
        .domains
        .values()
        .flat_map(|d| d.entities.iter().cloned())
        .collect();
    let engine = codegraph_grafeo::GrafeoEngine::in_memory().unwrap();
    codegraph::ingest::async_ingest::ingest_schemas(
        &engine,
        Path::new("tests/fixtures/schemas"),
        &classifier,
        &entity_names,
        &codegraph_config::UiOverrideConfig::default(),
        &config.defaults.type_suffix,
    )
    .await
    .unwrap();

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
    let profiles_path = workspace_root.join("profiles.toml");

    let registry = codegraph::profile::CapabilityRegistry::new();
    let mut resolved =
        codegraph::profile::load_and_resolve_profile(&profiles_path, "default", None).unwrap();
    for section in resolved.sections.values_mut() {
        section.generators.retain(|g| !g.starts_with("grpc_"));
    }
    let plan = codegraph::profile::BuildPlan::from_profile(&resolved, &registry).unwrap();

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
            schema_base_dir: Path::new(""),
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
        "generation reported errors: {:?}",
        report.errors
    );
}
