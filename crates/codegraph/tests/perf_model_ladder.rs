//! Synthetic consumer-scale workload ladder (issue #470, epic #469).
//!
//! A parametrized deterministic mox model generator plus a driver-level
//! timing harness, so ingest growth can be bisected to the smallest size
//! that reproduces the suspected superlinear (O(nodes×edges)) cold-ingest
//! behavior at consumer scale (9 mox packages, ~38 entities).
//!
//! Model shape per package `pkg{p}`: `{PascalPkg}EntityType{i}` entities
//! with K mixed scalar features (String/int/decimal/boolean/date), one
//! enum-typed codelist feature (the mox `enum` → codelist bridge makes
//! codelists trivial to include), a same-package `refers` wrap
//! (i → i+1 mod E), one cross-package `refers` to package (p+1) mod P, and
//! one `contains` value-object class. Class names are package-prefixed so
//! the ONE union namespace the multi-package compile lowers against stays
//! unambiguous.
//!
//! Tests:
//! - `ladder_gate_3x8_runs` — PR-CI gate: 3×8×6 through the FULL
//!   `driver::run` pipeline (mox-only), asserting migrations + DTO +
//!   repository artifacts.
//! - `ladder_step` (ignored) — one ladder rung: cold run, then two
//!   `run_with_graph_cache` invocations sharing a cache dir, printing
//!   machine-readable `perf_ladder` TSV lines (collected by
//!   `scripts/perf/ladder.sh`).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use codegraph::driver::{self, RunArgs};

// ── Paths ────────────────────────────────────────────────────────────────

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("codegraph crate lives at <repo>/crates/codegraph")
        .to_path_buf()
}

/// Persistent perf workspace (NOT a tempdir — the ladder script and
/// follow-up runs need the models and caches to survive the process).
fn perf_root() -> PathBuf {
    repo_root().join("target").join("perf")
}

// ── Deterministic model generator ────────────────────────────────────────

struct ModelLayout {
    config: PathBuf,
    profiles: PathBuf,
    mox_files: Vec<PathBuf>,
}

fn pascal_pkg(p: usize) -> String {
    format!("Pkg{p}")
}

/// The K scalar features cycle through these mox types; `decimal` rides a
/// `wraps String { format "decimal" }` datatype (no Decimal primitive in
/// rex).
fn scalar_feature(j: usize, pascal: &str) -> String {
    match j % 5 {
        0 => format!("    String field{j}"),
        1 => format!("    int [0..1] field{j}"),
        2 => format!("    {pascal}Amount [0..1] field{j}"),
        3 => format!("    boolean [0..1] field{j}"),
        _ => format!("    date [0..1] field{j}"),
    }
}

fn package_mox(p: usize, pkgs: usize, entities: usize, props: usize) -> String {
    let pascal = pascal_pkg(p);
    let next_pascal = pascal_pkg((p + 1) % pkgs);
    let mut src = String::new();
    src.push_str(&format!("package pkg{p}\n\n"));
    src.push_str(&format!(
        "type {pascal}Amount wraps String {{\n    format \"decimal\"\n}}\n\n"
    ));
    src.push_str(&format!(
        "enum {pascal}StatusKind {{\n    Alpha as \"A\" = 0\n    Beta as \"B\" = 1\n    Gamma as \"G\" = 2\n}}\n\n"
    ));
    for i in 0..entities {
        src.push_str(&format!("class {pascal}EntityType{i} {{\n"));
        for j in 0..props {
            src.push_str(&scalar_feature(j, &pascal));
            src.push('\n');
        }
        if props >= 2 {
            src.push_str(&format!("    {pascal}StatusKind status\n"));
        }
        src.push_str(&format!(
            "    refers {pascal}EntityType{} [0..1] next_peer\n",
            (i + 1) % entities
        ));
        src.push_str(&format!(
            "    refers {next_pascal}EntityType0 [0..1] cross_peer\n"
        ));
        src.push_str(&format!("    contains {pascal}DetailType[] details\n"));
        src.push_str("}\n\n");
    }
    src.push_str(&format!(
        "class {pascal}DetailType {{\n    String [1] detail_label\n    int [0..1] detail_quantity\n    boolean [0..1] detail_flag\n}}\n"
    ));
    src
}

fn domains_toml(pkgs: usize) -> String {
    let mut toml = String::from(
        "[defaults]\noperations = [\"create\", \"read\", \"update\", \"delete\", \"list\"]\n\n",
    );
    for p in 0..pkgs {
        toml.push_str(&format!(
            "[domains.pkg{p}]\nlabel = \"Pkg{p}\"\nschema_dir = \"pkg{p}\"\npostgres_schema = \"pkg{p}\"\n\n"
        ));
    }
    toml
}

fn profiles_toml() -> String {
    r#"
[profiles.default.meta]
name = "default"
version = "0.1.0"
description = "Synthetic ladder profile (issue #470)"
app_name = "perf-ladder"

[profiles.default.features]
database_target = "postgres"
persistence_provider = "sea_orm"
deployment_topology = "monolith"

[profiles.default.api]
generators = [
    # Entity
    "ddl", "sea_orm_entity", "codelist", "dto", "repository",
    "command", "query", "handler",
    # Domain
    "errors", "router",
    # Global
    "scaffold", "openapi", "platform_schema",
]
output = "generated/"
scripts.post_gen = []
"#
    .to_string()
}

/// Write the model for (P, E, K) under `target/perf/model-{P}x{E}/`,
/// idempotent overwrite.
fn write_model(pkgs: usize, entities: usize, props: usize) -> ModelLayout {
    let dir = perf_root().join(format!("model-{pkgs}x{entities}"));
    fs::create_dir_all(&dir).expect("model dir must be creatable");
    let mut mox_files = Vec::new();
    for p in 0..pkgs {
        let path = dir.join(format!("pkg{p}.mox"));
        fs::write(&path, package_mox(p, pkgs, entities, props)).expect("mox file must be writable");
        mox_files.push(path);
    }
    mox_files.sort();
    let config = dir.join("domains.toml");
    fs::write(&config, domains_toml(pkgs)).expect("domains.toml must be writable");
    let profiles = dir.join("profiles.toml");
    fs::write(&profiles, profiles_toml()).expect("profiles.toml must be writable");
    ModelLayout {
        config,
        profiles,
        mox_files,
    }
}

fn run_args<'a>(layout: &'a ModelLayout, output: &'a Path) -> RunArgs<'a> {
    RunArgs {
        schemas: None,
        classifier: None,
        config_path: &layout.config,
        output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(layout.profiles.clone()),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &layout.mox_files,
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

fn count_files(dir: &Path) -> usize {
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                count += 1;
            }
        }
    }
    count
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v| v > 0)
        .unwrap_or(default)
}

// ── Gate test (PR-CI, debug-fast) ────────────────────────────────────────

fn migration_for_table(migrations: &Path, table: &str) -> PathBuf {
    fs::read_dir(migrations)
        .expect("migrations dir must exist")
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(&format!("_{table}.sql")))
        })
        .unwrap_or_else(|| {
            panic!(
                "no migration for table {table} under {}",
                migrations.display()
            )
        })
}

#[tokio::test]
async fn ladder_gate_3x8_runs() {
    let layout = write_model(3, 8, 6);
    let output = perf_root().join("out-3x8x6-gate");
    let _ = fs::remove_dir_all(&output);

    driver::run(run_args(&layout, &output))
        .await
        .expect("3x8 gate run must succeed");

    let migrations = output.join("migrations");
    assert!(migrations.is_dir(), "migrations dir must exist");
    for p in 0..3 {
        for i in [0, 7] {
            let table = format!("pkg{p}_entity_type{i}");
            let sql = fs::read_to_string(migration_for_table(&migrations, &table))
                .unwrap_or_else(|e| panic!("read migration for {table}: {e}"));
            assert!(sql.contains("CREATE TABLE"), "{table}: {sql}");
        }
    }

    let module = output.join("src/domain/pkg0/pkg0_entity_type0");
    assert!(
        module.join("dto_response.rs").is_file(),
        "dto_response.rs must be emitted"
    );
    assert!(
        module.join("repository.rs").is_file(),
        "repository.rs must be emitted"
    );
}

// ── Ladder rung (ignored; driven by scripts/perf/ladder.sh) ──────────────

/// One ladder rung. Sizes come from CODEGRAPH_PERF_PKGS /
/// CODEGRAPH_PERF_ENTITIES / CODEGRAPH_PERF_PROPS (default 3/8/6). Prints
/// one `perf_ladder` TSV line per phase:
///
/// ```text
/// perf_ladder\t{P}x{E}x{K}\tcold\t{seconds:.2}\t{files}
/// perf_ladder\t{P}x{E}x{K}\twarm1\t{seconds:.2}\t{files}
/// perf_ladder\t{P}x{E}x{K}\twarm2\t{seconds:.2}\t{files}
/// ```
///
/// warm1/warm2 share `target/perf/cache-{P}x{E}x{K}`; warm1 may still
/// ingest (fresh cache), warm2 must reuse the persisted graph. Run via
/// `scripts/perf/ladder.sh` (or directly with the env vars below).
#[ignore]
#[tokio::test]
async fn ladder_step() {
    let pkgs = env_usize("CODEGRAPH_PERF_PKGS", 3);
    let entities = env_usize("CODEGRAPH_PERF_ENTITIES", 8);
    let props = env_usize("CODEGRAPH_PERF_PROPS", 6);
    let size = format!("{pkgs}x{entities}x{props}");
    let layout = write_model(pkgs, entities, props);

    let cold_out = perf_root().join(format!("out-{size}-cold"));
    let _ = fs::remove_dir_all(&cold_out);
    let started = Instant::now();
    driver::run(run_args(&layout, &cold_out))
        .await
        .unwrap_or_else(|e| panic!("cold run {size} failed: {e}"));
    let cold_files = count_files(&cold_out);
    println!(
        "perf_ladder\t{size}\tcold\t{:.2}\t{cold_files}",
        started.elapsed().as_secs_f64()
    );

    let cache = perf_root().join(format!("cache-{size}"));
    let warm_out = perf_root().join(format!("out-{size}-warm"));
    let _ = fs::remove_dir_all(&warm_out);
    let started = Instant::now();
    let _warm1 = driver::run_with_graph_cache(run_args(&layout, &warm_out), Some(&cache))
        .await
        .unwrap_or_else(|e| panic!("warm1 run {size} failed: {e}"));
    let warm1_files = count_files(&warm_out);
    println!(
        "perf_ladder\t{size}\twarm1\t{:.2}\t{warm1_files}",
        started.elapsed().as_secs_f64()
    );

    let started = Instant::now();
    let warm2 = driver::run_with_graph_cache(run_args(&layout, &warm_out), Some(&cache))
        .await
        .unwrap_or_else(|e| panic!("warm2 run {size} failed: {e}"));
    assert!(
        warm2.graph_cache_reused,
        "warm2 must reuse the persisted graph (inputs unchanged between warm1 and warm2)"
    );
    let warm2_files = count_files(&warm_out);
    println!(
        "perf_ladder\t{size}\twarm2\t{:.2}\t{warm2_files}",
        started.elapsed().as_secs_f64()
    );
}
