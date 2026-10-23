//! Dependency-strategy template emission (issue #347).
//!
//! The generated app, ops testkit, and domain-types Cargo.toml manifests must
//! reference codegraph crates via git+rev by default (`rev`, the historical
//! byte-identical contract) and via absolute path deps when the profile sets
//! `dependency_strategy = "path"`.

use std::path::Path;

use codegraph_generate::profile::DependencyStrategy;
use codegraph_generate::template_engine::create_tera;
use codegraph_generate::{CargoConfig, ProjectConfig, render_template_with_project};

fn template_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("templates")
}

fn project(strategy: DependencyStrategy) -> ProjectConfig {
    ProjectConfig {
        cargo: CargoConfig {
            codegraph_rev: "abc123".to_string(),
            dependency_strategy: strategy,
            codegraph_path_root: "/home/dev/codegraph".to_string(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Minimal scaffold context: only the vars `scaffold/cargo_toml.tera` reads
/// on the no-extras path (no grpc/atproto/seed/cli/test-gen branches).
#[derive(serde::Serialize)]
struct ScaffoldCtx {
    app_name: &'static str,
    has_cli: bool,
    has_seed: bool,
    has_fern: bool,
    has_test_gen: bool,
    has_atproto: bool,
    has_grpc: bool,
    has_admin_cli: bool,
    domain_types_path: &'static str,
    hooks_api_path: &'static str,
    extensions_path: &'static str,
    seed_crate_path: &'static str,
    has_webhooks: bool,
    has_reports: bool,
    has_auth_rate_limit: bool,
    has_labels: bool,
    migration_strategy: &'static str,
}

fn scaffold_ctx() -> ScaffoldCtx {
    ScaffoldCtx {
        app_name: "hr-app",
        has_cli: false,
        has_seed: false,
        has_fern: false,
        has_test_gen: false,
        has_atproto: false,
        has_grpc: false,
        has_admin_cli: false,
        domain_types_path: "",
        hooks_api_path: "",
        extensions_path: "",
        seed_crate_path: "",
        has_webhooks: false,
        has_reports: false,
        has_auth_rate_limit: false,
        has_labels: false,
        migration_strategy: "migration_crate",
    }
}

#[derive(serde::Serialize)]
struct TestkitCtx {
    codegraph_ops_path: &'static str,
}

fn render_scaffold_cargo(project: &ProjectConfig) -> String {
    let tera = create_tera(&template_dir()).unwrap();
    render_template_with_project(&tera, "scaffold/cargo_toml.tera", &scaffold_ctx(), project)
        .unwrap()
}

fn render_testkit_cargo(project: &ProjectConfig) -> String {
    let tera = create_tera(&template_dir()).unwrap();
    render_template_with_project(
        &tera,
        "ops/testkit_cargo.tera",
        &TestkitCtx {
            codegraph_ops_path: "../../../crates/codegraph-ops",
        },
        project,
    )
    .unwrap()
}

fn render_domain_types_cargo(project: &ProjectConfig) -> String {
    let tera = create_tera(&template_dir()).unwrap();
    render_template_with_project(
        &tera,
        "domain_types/cargo_toml.tera",
        &serde_json::json!({}),
        project,
    )
    .unwrap()
}

#[test]
fn domain_types_path_strategy_emits_absolute_path_dep() {
    let rendered = render_domain_types_cargo(&project(DependencyStrategy::Path));
    assert!(
        rendered.contains(
            r#"codegraph-type-contracts = { path = "/home/dev/codegraph/crates/codegraph-type-contracts" }"#
        ),
        "domain-types Cargo.toml must reference codegraph-type-contracts by path:\n{rendered}"
    );
    assert!(
        !rendered.contains("git =") && !rendered.contains("rev ="),
        "path strategy must not emit git or rev sources:\n{rendered}"
    );
}

#[test]
fn domain_types_rev_strategy_emits_git_rev_dep() {
    let rendered = render_domain_types_cargo(&project(DependencyStrategy::Rev));
    assert!(
        rendered.contains(
            r#"codegraph-type-contracts = { git = "https://github.com/magick93/codegraph.git", rev = "abc123" }"#
        ),
        "domain-types Cargo.toml must pin codegraph-type-contracts via git+rev:\n{rendered}"
    );
    assert!(
        !rendered.contains("path ="),
        "rev strategy must not emit path deps:\n{rendered}"
    );
}

#[test]
fn domain_types_explicit_path_override_wins_in_both_strategies() {
    for strategy in [DependencyStrategy::Path, DependencyStrategy::Rev] {
        let mut project = project(strategy);
        project.paths.type_contracts_base = "../custom-type-contracts".to_string();
        let rendered = render_domain_types_cargo(&project);
        assert!(
            rendered
                .contains(r#"codegraph-type-contracts = { path = "../custom-type-contracts" }"#),
            "explicit override must win for {strategy:?}:\n{rendered}"
        );
        assert!(
            !rendered.contains("git =")
                && !rendered.contains("rev =")
                && !rendered.contains("/home/dev/codegraph"),
            "explicit override must replace strategy sources for {strategy:?}:\n{rendered}"
        );
    }
}

/// Default (`rev`): both manifests pin the git repo at the stamped rev —
/// the historical byte-identical contract.
#[test]
fn rev_strategy_emits_git_rev_deps() {
    let rendered = render_scaffold_cargo(&project(DependencyStrategy::Rev));
    assert!(
        rendered.contains(
            r#"codegraph-workflow = { git = "https://github.com/magick93/codegraph.git", rev = "abc123" }"#
        ),
        "scaffold Cargo.toml must pin codegraph-workflow via git+rev:\n{rendered}"
    );
    assert!(
        rendered.contains(
            r#"codegraph-type-contracts = { git = "https://github.com/magick93/codegraph.git", rev = "abc123" }"#
        ),
        "scaffold Cargo.toml must pin codegraph-type-contracts via git+rev:\n{rendered}"
    );
    assert!(
        !rendered.contains("path = \"/home/dev/codegraph"),
        "rev strategy must not emit path deps into the checkout:\n{rendered}"
    );

    let testkit = render_testkit_cargo(&project(DependencyStrategy::Rev));
    assert!(
        testkit.contains(
            r#"codegraph-ops = { git = "https://github.com/magick93/codegraph.git", rev = "abc123" }"#
        ),
        "testkit Cargo.toml must pin codegraph-ops via git+rev:\n{testkit}"
    );
}

/// `path`: both manifests reference the crates inside the generating
/// checkout via absolute path deps (the git+rev lines must be gone).
#[test]
fn path_strategy_emits_absolute_path_deps() {
    let rendered = render_scaffold_cargo(&project(DependencyStrategy::Path));
    assert!(
        rendered.contains(
            r#"codegraph-workflow = { path = "/home/dev/codegraph/crates/codegraph-workflow" }"#
        ),
        "scaffold Cargo.toml must reference codegraph-workflow by path:\n{rendered}"
    );
    assert!(
        rendered.contains(
            r#"codegraph-type-contracts = { path = "/home/dev/codegraph/crates/codegraph-type-contracts" }"#
        ),
        "scaffold Cargo.toml must reference codegraph-type-contracts by path:\n{rendered}"
    );
    assert!(
        !rendered.contains("magick93/codegraph.git"),
        "path strategy must not reference the git repo:\n{rendered}"
    );

    let testkit = render_testkit_cargo(&project(DependencyStrategy::Path));
    assert!(
        testkit
            .contains(r#"codegraph-ops = { path = "/home/dev/codegraph/crates/codegraph-ops" }"#),
        "testkit Cargo.toml must reference codegraph-ops by path:\n{testkit}"
    );
    assert!(
        !testkit.contains("magick93/codegraph.git"),
        "path strategy must not reference the git repo:\n{testkit}"
    );
}
