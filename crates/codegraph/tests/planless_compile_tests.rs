//! Plan-less `run` emits a compilable app crate (issue #447).
//!
//! A generation without `profiles.toml` (no `BuildPlan` — e.g. plain
//! `codegraph run --rosetta-files ... --config domains.toml --output out`)
//! used to emit output that could not compile:
//!
//! 1. The separate domain-types crate (a profile concept: its crate root
//!    comes from profile meta `domain_types_base`) fell back to the OUTPUT
//!    ROOT plan-less, and its manifest/lib overwrote the app scaffold's
//!    `Cargo.toml`/`src/lib.rs`. The root manifest rendered as the
//!    domain-types manifest — no `[build-dependencies]` section — while the
//!    scaffold's `build.rs` still referenced `shadow_rs` → E0433 at build
//!    time.
//! 2. With no git checkout as cwd the pinned-rev derivation yields an empty
//!    string, so the `{% if project.codegraph_rev %}` gates drop the
//!    `codegraph-*` dependency lines entirely (the generated code references
//!    `codegraph_workflow`/`codegraph_type_contracts` no manifest declares).
//!
//! The fixes: plan-less runs skip the domain-types crate generators (their
//! output requires the profile-provided base path) and resolve the dep-pin
//! rev as CLI value → git HEAD → the binary's embedded checkout rev.

use std::fs;
use std::path::{Path, PathBuf};

/// Run the full pipeline plan-less (absent profiles.toml, default profile)
/// over the smallest fixture (`tests/fixtures/rosetta_bridge/`) into
/// `output_dir`.
async fn run_planless(output_dir: &Path, codegraph_rev: Option<String>) {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rosetta_bridge");
    let config_path = fixture_dir.join("domains.toml");
    let rosetta_files: Vec<PathBuf> = ["partners", "store"]
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
        ifml_framework: &[],
        ifml_components: None,
        ifml_design_system: None,
        codegraph_rev,
        check: false,
        ux_rules: None,
    })
    .await
    .expect("plan-less generation failed");
}

/// The plan-less output root must be the APP crate, not the domain-types
/// crate: the scaffold manifest (with its `[build-dependencies]` wiring for
/// the emitted `build.rs`) must survive the run, and the domain-types
/// crate files must not materialize inside it.
#[tokio::test]
async fn planless_output_root_manifest_is_the_app_manifest() {
    let dir = tempfile::TempDir::new().unwrap();
    let output = dir.path().join("out");
    run_planless(&output, None).await;

    let manifest = fs::read_to_string(output.join("Cargo.toml")).unwrap();
    // The domain-types clobber is gone: the manifest is the app scaffold's.
    assert!(
        !manifest.contains("name = \"domain-types\""),
        "root Cargo.toml must not be the domain-types manifest:\n{manifest}"
    );
    assert!(
        manifest.contains("[build-dependencies]"),
        "app manifest must carry the [build-dependencies] section its build.rs needs:\n{manifest}"
    );
    assert!(
        manifest.contains("shadow-rs"),
        "app manifest must declare shadow-rs for the emitted build.rs:\n{manifest}"
    );
    assert!(
        manifest.contains("axum"),
        "app manifest must declare the app's dependencies:\n{manifest}"
    );
    // The app code re-exports DTOs and `SourceContext` from the
    // domain-types crate — the manifest must path-dep it at its
    // conventional location (not the output root, which clobbered the app
    // manifest).
    assert!(
        manifest.contains("domain-types = { path = \"crates/domain-types\" }"),
        "app manifest must path-dep the domain-types crate:\n{manifest}"
    );
    assert!(
        output.join("crates/domain-types/Cargo.toml").exists()
            && output.join("crates/domain-types/src/lib.rs").exists(),
        "the domain-types crate must be emitted at crates/domain-types/"
    );
    assert!(
        output.join("build.rs").exists(),
        "the scaffold build.rs must be present"
    );

    // The app lib survives: it declares the app's module tree (the
    // domain-types lib declared `pub mod context;`/`pub mod query;` instead).
    let lib = fs::read_to_string(output.join("src/lib.rs")).unwrap();
    assert!(
        lib.contains("pub mod server;"),
        "src/lib.rs must be the app lib, not the domain-types lib:\n{lib}"
    );
    assert!(
        !output.join("src/context.rs").exists() && !output.join("src/query.rs").exists(),
        "domain-types support modules must not land in the app's src/"
    );
    let domain_types_lib_marker = "Generated crate — do not edit.";
    assert!(
        !lib.contains(domain_types_lib_marker),
        "src/lib.rs must not be overwritten by the domain-types lib:\n{lib}"
    );
}

/// The rev fallback chain (issue #447 item 2): plan-less runs with no
/// explicit rev and no git checkout as cwd must still render a non-empty
/// `codegraph-*` dependency pin, falling back to the binary's embedded
/// checkout rev. Inside this checkout the git HEAD wins; either way the
/// rendered pin must be a full hex SHA.
#[tokio::test]
async fn planless_manifest_renders_non_empty_codegraph_rev_pins() {
    let dir = tempfile::TempDir::new().unwrap();
    let output = dir.path().join("out");
    run_planless(&output, None).await;

    let manifest = fs::read_to_string(output.join("Cargo.toml")).unwrap();
    let pin_line = manifest
        .lines()
        .find(|l| l.starts_with("codegraph-workflow = "))
        .expect("root manifest must carry the codegraph-workflow dep");
    let rev = pin_line
        .split("rev = \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("codegraph-workflow pin must carry a rev");
    assert_eq!(
        rev.len(),
        40,
        "dep pin must be a full git SHA, got: {rev:?} (line: {pin_line})"
    );
    assert!(
        rev.chars().all(|c| c.is_ascii_hexdigit()),
        "dep pin must be hex: {rev:?}"
    );
}

/// Full compile gate (#336) over plan-less output: `cargo check` passes.
/// SLOW: builds the generated app's whole dependency tree and fetches the
/// `codegraph-*` git+rev pins from GitHub — needs network plus a rev that
/// exists upstream.
#[tokio::test]
#[ignore = "slow: cargo check over the generated app tree; needs network to fetch the codegraph-* git+rev pins"]
async fn planless_output_passes_the_compile_gate() {
    let dir = tempfile::TempDir::new().unwrap();
    let output = dir.path().join("out");
    // Pin a rev that exists on GitHub so the git+rev deps resolve. The
    // binary's embedded rev is right when this checkout's HEAD is pushed;
    // the master SHA is the fallback for local-only commits.
    let embedded = codegraph::rev::codegraph_rev();
    let rev = if embedded.is_empty() {
        "38abec7e417a70dc4bbacb05e628a313bc5532c3".to_string()
    } else {
        embedded.to_string()
    };
    run_planless(&output, Some(rev)).await;

    codegraph::driver::check_generated_output(&output)
        .expect("plan-less output must pass the compile gate");
}
