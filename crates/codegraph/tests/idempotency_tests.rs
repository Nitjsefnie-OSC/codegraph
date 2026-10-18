//! Regeneration idempotency gates (issue #354, pinning #332).
//!
//! Regenerating over an unchanged (or whitespace-only-touched) model must be
//! a no-op: same file paths in AND out, byte-identical contents (mtimes
//! excluded). While ordering is unstable, nondeterministic emission leaks
//! into content and this fails — which is exactly what these tests are here
//! to catch (they pin the A1 determinism fix).
//!
//! Conventions for the Postgres test mirror
//! `crates/codegraph-ops/tests/db_integration.rs`: `#[ignore]`d, env-gated
//! via `DATABASE_URL` (default `postgres://postgres:postgres@localhost:5432/postgres`),
//! `psql` on PATH required, early-return skip notices for missing tooling.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use codegraph_ops::db::{psql_exec, psql_exec_file, psql_query};
use codegraph_ops::env::ensure_psql;
use codegraph_ops::pg::PgTarget;

use sha2::{Digest, Sha256};

/// The always-on fixture: `rosetta_bridge` (2 model files) — small, fast,
/// and drives the full ingest → classify → generate pipeline including
/// migration-number allocation.
fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rosetta_bridge")
}

/// A writable copy of the fixture (models + domains.toml) — the noop-edit
/// test mutates a model and must never touch the committed fixture.
fn copy_fixture(dest: &Path) {
    fs::copy(
        fixture_dir().join("domains.toml"),
        dest.join("domains.toml"),
    )
    .unwrap();
    fs::create_dir_all(dest.join("model")).unwrap();
    for entry in fs::read_dir(fixture_dir().join("model")).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), dest.join("model").join(entry.file_name())).unwrap();
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
                out.insert(rel, fs::read(&path).unwrap());
            }
        }
    }
    walk(root, root, &mut out);
    out
}

/// Run the full pipeline over the fixture copy in `fixture_root` into
/// `output_dir` via the stable `driver::run` entry point (`codegraph_rev`
/// pinned so the manifests don't depend on git state).
async fn run_generation(fixture_root: &Path, output_dir: &Path) {
    let config_path = fixture_root.join("domains.toml");
    let mut models: Vec<PathBuf> = fs::read_dir(fixture_root.join("model"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    models.sort();
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
        rosetta_files: &models,
        ddd_files: &[],
        evt_files: &[],
        deploy_files: &[],
        ifml_framework: &[],
        ifml_components: None,
        ifml_design_system: None,
        codegraph_rev: Some("idempotency-test-pin".to_string()),
        check: false,
        ux_rules: None,
    })
    .await
    .unwrap();
}

/// Assert two generated trees have identical path sets and byte-identical
/// contents (mtimes deliberately excluded).
fn assert_zero_diff(a: &BTreeMap<String, Vec<u8>>, b: &BTreeMap<String, Vec<u8>>) {
    let only_a: Vec<&String> = a.keys().filter(|k| !b.contains_key(*k)).collect();
    let only_b: Vec<&String> = b.keys().filter(|k| !a.contains_key(*k)).collect();
    assert!(
        only_a.is_empty() && only_b.is_empty(),
        "file sets differ after regeneration — only in first: {only_a:?}, only in second: {only_b:?}"
    );
    let changed: Vec<String> = a
        .iter()
        .filter(|(k, v)| b.get(*k).is_some_and(|b| b != *v))
        .map(|(k, _)| k.clone())
        .collect();
    assert!(
        changed.is_empty(),
        "{} file(s) changed across regeneration:\n{}",
        changed.len(),
        changed.join("\n")
    );
}

/// Regeneration after a whitespace-only model edit must be a zero diff in
/// the SAME output directory. Fails while unstable ordering leaks into
/// content (the E3 pin on A1).
#[tokio::test]
async fn regeneration_after_noop_edit_is_zero_diff() {
    let dir = tempfile::TempDir::new().unwrap();
    copy_fixture(dir.path());
    let output = dir.path().join("generated");

    run_generation(dir.path(), &output).await;
    let before = collect_tree(&output);
    assert!(!before.is_empty(), "first generation produced nothing");

    // Whitespace-only edit (a trailing blank line): parse-identical model.
    let store = dir.path().join("model/store.rosetta");
    let mut text = fs::read_to_string(&store).unwrap();
    text.push('\n');
    fs::write(&store, text).unwrap();

    // Regenerate into the SAME directory.
    run_generation(dir.path(), &output).await;
    let after = collect_tree(&output);

    assert_zero_diff(&before, &after);
}

// ── Cross-process generate-twice ────────────────────────────────────────

const WORKER_ENV: &str = "IDEMPOTENCY_WORKER";
const WORKER_OUT_ENV: &str = "IDEMPOTENCY_OUTPUT_DIR";

/// Worker entry point: re-executed by [`generate_twice_is_diff_empty`] with
/// `IDEMPOTENCY_WORKER=1`; a no-op under a normal test run.
#[test]
fn idempotency_worker() {
    let Ok(out) = std::env::var(WORKER_OUT_ENV) else {
        return;
    };
    let out = PathBuf::from(out);
    let fixture = out.join("fixture");
    fs::create_dir_all(&fixture).unwrap();
    copy_fixture(&fixture);
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        run_generation(&fixture, &out.join("generated")).await;
    });
}

/// Two independent processes (fresh RandomState seeds — the honest variant)
/// generate the same fixture; the trees must be byte-identical.
#[test]
fn generate_twice_is_diff_empty() {
    let exe = std::env::current_exe().unwrap();
    let parent = tempfile::TempDir::new().unwrap();
    let mut trees = Vec::new();
    for i in 0..2 {
        let out = parent.path().join(format!("run{i}"));
        let output = Command::new(&exe)
            .args([
                "--exact",
                "idempotency_worker",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(WORKER_ENV, "1")
            .env(WORKER_OUT_ENV, &out)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("failed to spawn idempotency worker");
        assert!(
            output.status.success(),
            "idempotency worker failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        trees.push(collect_tree(&out.join("generated")));
    }
    assert_zero_diff(&trees[0], &trees[1]);
}

// ── Postgres: apply the generated migrations twice, compare fingerprints ─

fn target() -> PgTarget {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/postgres".to_string());
    PgTarget::from_url(&url).expect("invalid DATABASE_URL")
}

/// True when psql is unavailable — the test returns early so the suite
/// passes trivially on machines without postgres tooling.
fn skip_if_no_psql() -> bool {
    if ensure_psql().is_err() {
        eprintln!("skipping: psql not found");
        true
    } else {
        false
    }
}

/// True when pg_dump is unavailable (needed for the schema fingerprint).
fn skip_if_no_pg_dump() -> bool {
    let ok = Command::new("pg_dump")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("skipping: pg_dump not found");
    }
    !ok
}

/// True when the target looks like a Supabase-compatible Postgres: the
/// generated platform band (basejump via pg_tle, pgmq queues, the `http`
/// extension) needs them. A vanilla `postgres:16` container — e.g. the CI
/// fallback on 127.0.0.1:15432 — lacks them and the test skips.
async fn supabase_compatible(t: &PgTarget) -> bool {
    let out = psql_query(
        t,
        "SELECT \
         EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = 'auth') AND \
         EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = 'pgmq') AND \
         EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = 'pg_tle') AND \
         EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = 'http') AND \
         EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = 'uuid-ossp');",
    )
    .await
    .unwrap_or_default();
    out.trim() == "t"
}

async fn recreate_database(admin: &PgTarget, db: &str) {
    psql_exec(
        admin,
        &format!("DROP DATABASE IF EXISTS {db} WITH (FORCE);"),
    )
    .await
    .unwrap_or_else(|e| panic!("drop database {db} failed: {e}"));
    psql_exec(admin, &format!("CREATE DATABASE {db};"))
        .await
        .unwrap_or_else(|e| panic!("create database {db} failed: {e}"));
}

/// Sorted list of the generated migration files (application order).
fn sorted_migrations(output: &Path) -> Vec<PathBuf> {
    let dir = output.join("migrations");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("migrations dir {} missing: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no generated migrations found");
    files
}

/// Schemas the generated band creates — the fingerprint surface.
const FINGERPRINT_SCHEMAS: &[&str] = &[
    "basejump",
    "platform",
    "platform_integrations",
    "api_keys_private",
    "partners",
    "store",
];

/// `pg_dump --schema-only` over the fingerprint schemas, hashed. The
/// `\restrict`/`\unrestrict` psql meta-command lines carry a per-dump random
/// token (pg_dump security backports) and are stripped before hashing; the
/// remainder of the dump for identical schema state is byte-identical.
async fn schema_fingerprint(t: &PgTarget) -> String {
    let mut cmd = Command::new("pg_dump");
    cmd.arg("--schema-only")
        .arg("--no-password")
        .arg("--dbname")
        .arg(t.url());
    for schema in FINGERPRINT_SCHEMAS {
        cmd.args(["--schema", schema]);
    }
    let out = cmd.output().expect("failed to spawn pg_dump");
    assert!(
        out.status.success(),
        "pg_dump failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let mut hasher = Sha256::new();
    for line in text.lines() {
        if line.starts_with("\\restrict ") || line.starts_with("\\unrestrict ") {
            continue;
        }
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

/// Apply the generated migrations, fingerprint the schema; reset to a fresh
/// database, apply the SAME files again, fingerprint again — the two
/// fingerprints must be equal. Pins that migration content and application
/// order are deterministic (the DB-level view of A1).
///
/// Requires a Supabase-compatible Postgres (the generated platform band
/// installs basejump/pg_tle/pgmq/http) reachable at `DATABASE_URL` (default
/// `postgres://postgres:postgres@localhost:5432/postgres`), plus `psql` and
/// `pg_dump` on PATH. Runs in its own scratch database (pid-suffixed,
/// dropped with FORCE afterwards) so the target stays untouched.
#[tokio::test]
#[ignore = "requires a Supabase-compatible Postgres (set DATABASE_URL or default localhost:5432), psql + pg_dump on PATH"]
async fn migrate_twice_schema_fingerprint_is_equal() {
    if skip_if_no_psql() || skip_if_no_pg_dump() {
        return;
    }
    let admin = target();
    if !supabase_compatible(&admin).await {
        eprintln!("skipping: target lacks Supabase-compatible auth schema / pgmq / pg_tle / http");
        return;
    }

    // Generate the rosetta_bridge fixture once.
    let dir = tempfile::TempDir::new().unwrap();
    copy_fixture(dir.path());
    let output = dir.path().join("generated");
    run_generation(dir.path(), &output).await;
    let migrations = sorted_migrations(&output);

    let db_name = format!("codegraph_idempotency_{}", std::process::id());
    let mut fingerprints = Vec::new();
    for pass in 0..2 {
        recreate_database(&admin, &db_name).await;
        let db_target = PgTarget {
            db: db_name.clone(),
            ..admin.clone()
        };
        for file in &migrations {
            psql_exec_file(&db_target, file)
                .await
                .unwrap_or_else(|e| panic!("pass {pass}: applying {}: {e}", file.display()));
        }
        fingerprints.push(schema_fingerprint(&db_target).await);
    }
    let _ = psql_exec(
        &admin,
        &format!("DROP DATABASE IF EXISTS {db_name} WITH (FORCE);"),
    )
    .await;

    assert_eq!(
        fingerprints[0], fingerprints[1],
        "two applications of the same generated migrations produced different schemas"
    );
}
