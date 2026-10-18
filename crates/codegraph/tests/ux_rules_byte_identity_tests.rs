//! Byte-identity canary for the ux-rules feature plane (issue #293).
//!
//! THE contract for the whole ux-rules epic: while the feature is OFF (or
//! on but unconsumed), generation output must not change by a single byte.
//! Gates live here for BOTH pipelines (issue #304 extended the canary from
//! entity-only to both):
//!
//! 1. `flag_off_pipeline_is_deterministic` — the full ENTITY pipeline
//!    (`driver::run`, mox-first init fixture) run TWICE with
//!    `ux_rules = false` produces identical output trees.
//! 2. `flag_off_output_matches_pre_feature_snapshot` — a flag-OFF entity
//!    tree hashes to the committed pre-feature snapshot
//!    (`tests/fixtures/ux_rules_pre_feature_tree.sha256`), so any
//!    generator change that leaks through the ux plane is caught.
//! 3. `ifml_flag_off_pipeline_is_deterministic` — the IFML pipeline
//!    (`driver::ifml_generate`, `ux_rules: None`) run TWICE produces
//!    identical output trees.
//! 4. `ifml_flag_off_output_matches_pre_feature_snapshot` — a flag-OFF
//!    IFML tree hashes to the committed pre-#300 snapshot
//!    (`tests/fixtures/ux_rules_pre_feature_ifml_tree.sha256`). Flag-off
//!    IFML output is pre-ux byte-identical by contract (pinned by the
//!    committed expected-markup fixture in `ifml_template_tests.rs`), so
//!    the snapshot was generated from the current code at #304 time.
//!
//! # Snapshot normalization
//!
//! Before hashing, file contents are normalized so the snapshot survives
//! rev bumps and machine-specific paths:
//!
//! - the literal current git rev and the output/repo absolute paths are
//!   replaced with `REV` / `OUT` / `REPO`;
//! - every remaining `[0-9a-f]{7,40}` hex run (greedy, leftmost — regex
//!   semantics) is replaced with `REV`.
//!
//! # Regenerating a snapshot
//!
//! ```text
//! UX_RULES_BLESS=1 CARGO_TARGET_DIR=... \
//!   cargo test -p codegraph --test ux_rules_byte_identity_tests \
//!   -- flag_off_output_matches_pre_feature_snapshot
//!   # or, for the IFML snapshot:
//!   -- ifml_flag_off_output_matches_pre_feature_snapshot
//! ```
//!
//! then commit the updated fixture. Only bless when a change to the
//! generator output is INTENDED — the whole point of this file is that an
//! accidental diff fails here first.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use codegraph::init::commands::{InitArgs, cmd_init};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// Absolute repo root (`<repo>/crates/codegraph` → two parents up).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("codegraph crate lives at <repo>/crates/codegraph")
        .to_path_buf()
}

fn init_args(dir: &Path) -> InitArgs {
    InitArgs {
        name: Some("demo-app".to_string()),
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
        codegraph_path: Some(repo_root()),
        force: false,
        template_dirs: vec![],
    }
}

/// Scaffold the init fixture and force the flag OFF in the emitted
/// profiles.toml (new scaffolds ship `ux_rules = true`; the canary pins the
/// OFF shape). Returns the `generated/` output path and the mox model.
fn fixture_with_flag_off(dir: &TempDir) -> (PathBuf, PathBuf) {
    cmd_init(&init_args(dir.path())).unwrap();
    let project = dir.path().join("demo-app");

    let profiles_path = project.join("profiles.toml");
    let profiles = fs::read_to_string(&profiles_path).unwrap();
    let flag_off = profiles.replace("ux_rules = true", "ux_rules = false");
    assert_ne!(
        profiles, flag_off,
        "scaffold profiles.toml must carry the ux_rules flag:\n{profiles}"
    );
    fs::write(&profiles_path, flag_off).unwrap();

    (project.join("generated"), project.join("model/common.mox"))
}

/// Run the full mox-first pipeline into `output` (the
/// `init_scaffold_runs_mox_first` shape: only --mox-files + config, no DB,
/// no network).
async fn run_pipeline(output: &Path, mox_file: &Path, config: &Path, profiles: &Path) {
    let mox_files = vec![mox_file.to_path_buf()];
    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: config,
        output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(profiles.to_path_buf()),
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
        ux_rules: None,
        codegraph_rev: None,
        check: false,
    })
    .await
    .unwrap();
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
                    .into_owned();
                out.insert(rel, fs::read(&path).unwrap());
            }
        }
    }
    walk(root, root, &mut out);
    out
}

/// The current git rev of the worktree (the driver stamps it into
/// generated manifests via `git rev-parse HEAD`).
fn current_git_rev() -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_root())
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn is_lower_hex(c: char) -> bool {
    matches!(c, '0'..='9' | 'a'..='f')
}

/// Replace every `[0-9a-f]{7,40}` match with REV. Greedy leftmost per
/// regex semantics: a maximal hex run longer than 40 chars yields one REV
/// per full 40-char window; a 1–6 char tail is kept literally.
fn replace_hex_runs(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        if is_lower_hex(chars[i]) {
            let start = i;
            while i < chars.len() && is_lower_hex(chars[i]) {
                i += 1;
            }
            let run_len = i - start;
            let mut consumed = 0;
            while run_len - consumed >= 7 {
                out.push_str("REV");
                consumed += (run_len - consumed).min(40);
            }
            out.extend(chars[start + consumed..start + run_len].iter());
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Normalize one file's content for hashing.
fn normalize(content: &[u8], rev: &str, output_dir: &Path, repo: &Path) -> String {
    let text = String::from_utf8_lossy(content).into_owned();
    let text = if rev.is_empty() {
        text
    } else {
        text.replace(rev, "REV")
    };
    let text = text.replace(output_dir.to_string_lossy().as_ref(), "OUT");
    let text = text.replace(repo.to_string_lossy().as_ref(), "REPO");
    replace_hex_runs(&text)
}

/// Sorted-tree hash: relative path + normalized content per file, fed into
/// one SHA-256.
fn tree_hash(
    tree: &BTreeMap<String, Vec<u8>>,
    rev: &str,
    output_dir: &Path,
    repo: &Path,
) -> String {
    let mut hasher = Sha256::new();
    for (rel, bytes) in tree {
        hasher.update(rel.as_bytes());
        hasher.update(b"\n");
        hasher.update(normalize(bytes, rev, output_dir, repo).as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

fn snapshot_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ux_rules_pre_feature_tree.sha256")
}

#[tokio::test]
async fn flag_off_pipeline_is_deterministic() {
    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();

    let (output_a, mox_a) = fixture_with_flag_off(&dir_a);
    let project_a = dir_a.path().join("demo-app");
    run_pipeline(
        &output_a,
        &mox_a,
        &project_a.join("domains.toml"),
        &project_a.join("profiles.toml"),
    )
    .await;

    let (output_b, mox_b) = fixture_with_flag_off(&dir_b);
    let project_b = dir_b.path().join("demo-app");
    run_pipeline(
        &output_b,
        &mox_b,
        &project_b.join("domains.toml"),
        &project_b.join("profiles.toml"),
    )
    .await;

    let tree_a = collect_tree(&output_a);
    let tree_b = collect_tree(&output_b);
    assert_eq!(
        tree_a.len(),
        tree_b.len(),
        "generated file sets differ between runs"
    );
    assert!(
        !tree_a.is_empty(),
        "mox-first generation must produce files"
    );

    // Manifests embed absolute paths; normalize each run's output dir
    // before comparing so only genuine content differences count.
    let a_str = output_a.to_string_lossy().into_owned();
    let b_str = output_b.to_string_lossy().into_owned();
    let mut diffs: Vec<String> = Vec::new();
    for (name, bytes_a) in &tree_a {
        let bytes_b = tree_b
            .get(name)
            .unwrap_or_else(|| panic!("file {name} missing in second run"));
        let text_a = String::from_utf8_lossy(bytes_a).replace(&a_str, "OUT");
        let text_b = String::from_utf8_lossy(bytes_b).replace(&b_str, "OUT");
        if text_a != text_b {
            diffs.push(name.clone());
        }
    }
    assert!(
        diffs.is_empty(),
        "flag-OFF generation is not deterministic — {} files differ:\n{:?}",
        diffs.len(),
        diffs
    );
}

#[tokio::test]
async fn flag_off_output_matches_pre_feature_snapshot() {
    let dir = TempDir::new().unwrap();
    let (output, mox) = fixture_with_flag_off(&dir);
    let project = dir.path().join("demo-app");
    run_pipeline(
        &output,
        &mox,
        &project.join("domains.toml"),
        &project.join("profiles.toml"),
    )
    .await;

    let tree = collect_tree(&output);
    let hash = tree_hash(&tree, &current_git_rev(), &output, &repo_root());

    if std::env::var("UX_RULES_BLESS").is_ok() {
        fs::write(snapshot_path(), &hash).unwrap();
        println!("blessed snapshot {}: {hash}", snapshot_path().display());
        return;
    }

    let expected = fs::read_to_string(snapshot_path())
        .expect("snapshot fixture must exist")
        .trim()
        .to_string();
    assert_eq!(
        hash, expected,
        "flag-OFF output diverged from the committed pre-feature snapshot.\n\
         If the generator change is intended, re-bless with UX_RULES_BLESS=1\n\
         (see the module comment for the procedure)."
    );
}

// ── IFML pipeline canary (issue #304) ───────────────────────────────────

/// The smallest IFML fixture that still exercises both view shapes the ux
/// plane renders into (fallback list + details), mirroring the
/// `SPECLESS_IFML` fixture of `ifml_template_tests.rs`. Spec-less ⇒ no
/// schemas/classifier needed — the whole canary stays fast and node-free.
const IFML_CANARY_MODEL: &str = r#"
domain "sales" {
    schema "sales";
}

view "CustomerList" {
    label "Customer Management";
    landmark: true;

    component "grid" {
        type: list;
        data: Customer;
        fields: [name, email, phone, status];

        on select(row) -> navigate("CustomerDetail", {
            customerId: row.id
        });
    }
}

view "CustomerDetail" {
    params { customerId: Uuid };

    component "info" {
        type: details;
        data: Customer;
        fields: [name, email, phone];
    }
}
"#;

fn ifml_domains_toml() -> &'static str {
    r#"
[defaults]
api_version = "v1"

[domains.sales]
label = "Sales"
schema_dir = "sales"
postgres_schema = "sales"
entities = ["CustomerType"]
"#
}

/// Write the IFML fixture into `dir`. Flag off = `ux_rules: None` (the
/// `IfmlGenerateArgs` default shape before #300). Returns the output root
/// (the tree the driver writes into) and the .ifml file path.
fn ifml_fixture(dir: &TempDir) -> (PathBuf, PathBuf) {
    let ifml_path = dir.path().join("app.ifml");
    fs::write(&ifml_path, IFML_CANARY_MODEL).unwrap();
    fs::write(dir.path().join("domains.toml"), ifml_domains_toml()).unwrap();
    let output = dir.path().join("out");
    (output, ifml_path)
}

/// Run the IFML pipeline flag-off into `output`.
async fn run_ifml_pipeline(output: &Path, ifml_file: &Path, config: &Path) {
    codegraph::driver::ifml_generate(codegraph::driver::IfmlGenerateArgs {
        config_path: config,
        output,
        ifml_files: &[ifml_file.to_path_buf()],
        schemas: None,
        classifier: None,
        frameworks: &["svelte".to_string()],
        profiles_config_path: None,
        template_dir: &[],
        ifml_components: None,
        ifml_design_system: None,
        ux_rules: None,
    })
    .await
    .unwrap();
}

fn ifml_snapshot_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ux_rules_pre_feature_ifml_tree.sha256")
}

#[tokio::test]
async fn ifml_flag_off_pipeline_is_deterministic() {
    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();

    let (output_a, ifml_a) = ifml_fixture(&dir_a);
    let (output_b, ifml_b) = ifml_fixture(&dir_b);
    run_ifml_pipeline(&output_a, &ifml_a, &dir_a.path().join("domains.toml")).await;
    run_ifml_pipeline(&output_b, &ifml_b, &dir_b.path().join("domains.toml")).await;

    let tree_a = collect_tree(&output_a);
    let tree_b = collect_tree(&output_b);
    assert_eq!(
        tree_a.len(),
        tree_b.len(),
        "IFML file sets differ between runs"
    );
    assert!(!tree_a.is_empty(), "IFML generation must produce files");

    // Normalize each run's own output dir before comparing so only
    // genuine content differences count.
    let a_str = output_a.to_string_lossy().into_owned();
    let b_str = output_b.to_string_lossy().into_owned();
    let mut diffs: Vec<String> = Vec::new();
    for (name, bytes_a) in &tree_a {
        let bytes_b = tree_b
            .get(name)
            .unwrap_or_else(|| panic!("file {name} missing in second run"));
        let text_a = String::from_utf8_lossy(bytes_a).replace(&a_str, "OUT");
        let text_b = String::from_utf8_lossy(bytes_b).replace(&b_str, "OUT");
        if text_a != text_b {
            diffs.push(name.clone());
        }
    }
    assert!(
        diffs.is_empty(),
        "flag-OFF IFML generation is not deterministic — {} files differ:\n{:?}",
        diffs.len(),
        diffs
    );
}

#[tokio::test]
async fn ifml_flag_off_output_matches_pre_feature_snapshot() {
    let dir = TempDir::new().unwrap();
    let (output, ifml) = ifml_fixture(&dir);
    run_ifml_pipeline(&output, &ifml, &dir.path().join("domains.toml")).await;

    let tree = collect_tree(&output);
    let hash = tree_hash(&tree, &current_git_rev(), &output, &repo_root());

    if std::env::var("UX_RULES_BLESS").is_ok() {
        fs::write(ifml_snapshot_path(), &hash).unwrap();
        println!(
            "blessed snapshot {}: {hash}",
            ifml_snapshot_path().display()
        );
        return;
    }

    let expected = fs::read_to_string(ifml_snapshot_path())
        .expect("snapshot fixture must exist")
        .trim()
        .to_string();
    assert_eq!(
        hash, expected,
        "flag-OFF IFML output diverged from the committed pre-#300 snapshot.\n\
         If the generator change is intended, re-bless with UX_RULES_BLESS=1\n\
         (see the module comment for the procedure)."
    );
}
