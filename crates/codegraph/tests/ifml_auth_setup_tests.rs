//! Phase 0 red pin for IFML test-side auth bootstrap + journey specs
//! (issue #463 Phase 2, `ifml_e2e_auth`).
//!
//! Pins the generated auth-testing surface for IFML apps:
//!
//! - `tests/e2e/auth.setup.ts` — a Playwright `globalSetup` that provisions
//!   one API key per human actor via `public.create_api_key` (psql over
//!   `DATABASE_URL`) and writes `.auth/{persona}.json` (apiKey, roles,
//!   capabilities); `playwright.config.ts` wires it as `globalSetup`;
//! - `tests/e2e/personas.ts` — per-persona `test.use(...)` fixtures carrying
//!   the storage state, the Bearer header, and the `__USER_ROLES__` /
//!   `__USER_CAPABILITIES__` seeding in ONE place (the per-spec inline
//!   `addInitScript` stubs disappear from the base specs);
//! - `{view}.auth.spec.ts` — the auth family: unauthenticated/garbage-key
//!   behavior, denial = redirect AND denial presentation, and control-gating
//!   visibility per capability;
//! - `tests/journeys/*.journey.spec.ts` — IR-derived journeys: the workflow
//!   handoff (persona A creates → persona B transitions via the POM
//!   transition map → state persisted via GET) and the nav-click journey
//!   through the shell (`routes.ts` links clicked, not `open()`);
//! - POM extensions: `{comp}Delete()` via `UxTable.deleteViaMenu` + confirm
//!   and `{comp}Cancel()` on the page classes.
//!
//! Node-free: runs the full pipeline over a small fixture (policy with two
//! human actors + a capability, view `roles:` + `requires:`, a
//! workflow-configured entity) and asserts the emitted artifacts. RED at
//! HEAD: none of these exist yet.
//!
//! ```text
//! cargo test -p codegraph --test ifml_auth_setup_tests
//! ```

use std::fs;
use std::path::{Path, PathBuf};

const PORTAL_MOX: &str = r#"package portal

/// Request lifecycle.
enum RequestStatus {
    Draft as "Draft" = 0
    Submitted as "Submitted" = 1
    Approved as "Approved" = 2
}

/// A reviewable request.
class Request {
    /// Request title.
    String title
    /// Lifecycle status.
    RequestStatus status
}
"#;

const PORTAL_ACTOR: &str = r#"import "portal.mox"

actors Portal {
    actor Admin
    actor Agent

    capability ApproveRequest on Request

    grant Admin {
        permit ApproveRequest
    }
}
"#;

const APP_IFML: &str = r#"domain "portal" { schema "portal"; }

import "portal.actor";

view "RequestList" {
    label "Requests";
    landmark: true;
    roles: [Admin];

    component "grid" {
        type: list;
        data: Request;
        fields: [title, status];
        pagination: true;

        on select(row) -> navigate("RequestForm", {
            id: row.id
        });
    }
}

view "RequestForm" {
    params { id: Uuid };
    label "Edit Request";

    component "editor" {
        type: form;
        data: Request;
        mode: edit;

        field title -> input text {
            required: true;
        }

        on save -> navigate("RequestList");
        on cancel -> navigate("RequestList");
    }
}

view "ReviewQueue" {
    label "Review Queue";
    requires: [ApproveRequest];

    component "queue" {
        type: list;
        data: Request;
        fields: [title, status];
        pagination: true;
    }
}
"#;

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]
api_version = "v1"

[domains.portal]
label = "Portal"
schema_dir = "portal"
postgres_schema = "portal"
entities = ["Request"]

[domains.portal.entity_config.Request]
role = "root"

[domains.portal.entity_config.Request.workflow]
status_field = "status"
states = ["Draft", "Submitted", "Approved"]
initial_state = "Draft"
terminal_states = ["Approved"]
generate_action_endpoints = true

[domains.portal.entity_config.Request.workflow.transitions]
Draft = ["Submitted"]
Submitted = ["Approved"]
"#;

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
    mox: PathBuf,
    ifml: PathBuf,
}

fn write_fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("domains.toml"), DOMAINS_TOML).unwrap();
    fs::write(root.path().join("portal.mox"), PORTAL_MOX).unwrap();
    fs::write(root.path().join("portal.actor"), PORTAL_ACTOR).unwrap();
    let ifml = root.path().join("app.ifml");
    fs::write(&ifml, APP_IFML).unwrap();
    Fixture {
        config: root.path().join("domains.toml"),
        mox: root.path().join("portal.mox"),
        ifml,
        root,
    }
}

fn run_args<'a>(
    fixture: &'a Fixture,
    frameworks: &'a [String],
    output: &'a Path,
) -> codegraph::driver::RunArgs<'a> {
    codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &fixture.config,
        output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        // The workspace profiles.toml: pins that `ifml_e2e_auth` is ON in
        // the default profile (the "default/fullstack ON" decision).
        profiles_config_path: Some(PathBuf::from("../../profiles.toml")),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: std::slice::from_ref(&fixture.ifml),
        openapi_files: &[],
        mox_files: std::slice::from_ref(&fixture.mox),
        rosetta_files: &[],
        ddd_files: &[],
        evt_files: &[],
        deploy_files: &[],
        ifml_framework: frameworks,
        ifml_components: None,
        ifml_design_system: None,
        codegraph_rev: None,
        check: false,
        ux_rules: None,
    }
}

/// Generate the fixture and return the emitted svelte test tree.
async fn generate_fixture() -> (Fixture, PathBuf) {
    let fixture = write_fixture();
    let output = fixture.root.path().join("generated");
    let frameworks = vec!["svelte".to_string()];
    codegraph::driver::run(run_args(&fixture, &frameworks, &output))
        .await
        .expect("pipeline run must succeed");
    (fixture, output.join("svelte"))
}

fn read(base: &Path, rel: &str) -> String {
    fs::read_to_string(base.join(rel))
        .unwrap_or_else(|e| panic!("expected {rel} to be emitted: {e}"))
}

/// `tests/e2e/auth.setup.ts`: the globalSetup provisioning one API key per
/// human actor and writing the per-persona storage files.
#[tokio::test]
async fn auth_setup_provisions_api_keys_per_persona() {
    let (_fixture, svelte) = generate_fixture().await;

    let setup = read(&svelte, "tests/e2e/auth.setup.ts");
    assert!(
        setup.contains("create_api_key"),
        "auth.setup.ts must provision keys via public.create_api_key:\n{setup}"
    );
    assert!(
        setup.contains("DATABASE_URL"),
        "auth.setup.ts must reach the database over DATABASE_URL:\n{setup}"
    );
    for persona in ["admin", "agent"] {
        assert!(
            setup.contains(&format!(".auth/{persona}.json")),
            "auth.setup.ts must write the {persona} storage state:\n{setup}"
        );
    }
    // Capabilities ride the policy, not hardcoding: the Admin persona's file
    // must carry the ApproveRequest capability, Agent's must not.
    assert!(
        setup.contains("ApproveRequest"),
        "auth.setup.ts must derive capabilities from the policy context:\n{setup}"
    );

    let config = read(&svelte, "playwright.config.ts");
    assert!(
        config.contains("globalSetup"),
        "playwright.config.ts must wire the auth globalSetup:\n{config}"
    );
    assert!(
        config.contains("tests/e2e/auth.setup.ts"),
        "playwright.config.ts must point globalSetup at tests/e2e/auth.setup.ts:\n{config}"
    );
}

/// `tests/e2e/personas.ts`: per-persona `test.use` fixtures — storage state,
/// Bearer header, and role/capability seeding in one place; the per-spec
/// inline `addInitScript` stubs are gone from the base specs.
#[tokio::test]
async fn persona_fixture_replaces_inline_init_script_stubs() {
    let (_fixture, svelte) = generate_fixture().await;

    let personas = read(&svelte, "tests/e2e/personas.ts");
    assert!(
        personas.contains("test.use"),
        "personas.ts must expose test.use helpers:\n{personas}"
    );
    assert!(
        personas.contains("Authorization") && personas.contains("Bearer"),
        "personas.ts must carry the per-persona Bearer header:\n{personas}"
    );
    assert!(
        personas.contains("__USER_ROLES__") && personas.contains("__USER_CAPABILITIES__"),
        "personas.ts must seed roles + capabilities in one place:\n{personas}"
    );
    for persona in ["admin", "agent"] {
        assert!(
            personas.contains(persona),
            "personas.ts must expose the {persona} persona:\n{personas}"
        );
    }

    for spec in [
        "tests/ifml/request-list.spec.ts",
        "tests/ifml/review-queue.spec.ts",
    ] {
        let body = read(&svelte, spec);
        assert!(
            !body.contains("addInitScript"),
            "{spec} must drive personas through the fixture, not inline addInitScript:\n{body}"
        );
    }
}

/// The `{view}.auth.spec.ts` family: unauthenticated/garbage-key behavior,
/// denial = redirect AND denial presentation, control-gating per capability.
#[tokio::test]
async fn auth_specs_assert_denial_and_control_gating() {
    let (_fixture, svelte) = generate_fixture().await;

    // Capability-guarded view: Admin passes, Agent is denied.
    let queue = read(&svelte, "tests/ifml/review-queue.auth.spec.ts");
    assert!(
        queue.contains("redirect"),
        "the auth spec must assert the denial redirect:\n{queue}"
    );
    assert!(
        queue.contains("denied") || queue.contains("denial"),
        "the auth spec must assert the denial presentation, not just the redirect:\n{queue}"
    );
    assert!(
        queue.contains("ApproveRequest"),
        "the auth spec must gate controls per capability:\n{queue}"
    );
    assert!(
        queue.contains("garbage") || queue.contains("unauthenticated"),
        "the auth spec must cover unauthenticated/garbage-key behavior:\n{queue}"
    );

    // Role-guarded view: the roles check has its own auth spec coverage.
    let list = read(&svelte, "tests/ifml/request-list.auth.spec.ts");
    assert!(
        list.contains("agent"),
        "the roles-guarded view's auth spec must exercise the denied persona:\n{list}"
    );
}

/// The journey family: workflow handoff across personas + nav-click journey
/// through the shell.
#[tokio::test]
async fn journey_specs_cover_workflow_handoff_and_shell_nav() {
    let (_fixture, svelte) = generate_fixture().await;

    let workflow = read(&svelte, "tests/journeys/request-workflow.journey.spec.ts");
    assert!(
        workflow.contains("admin") && workflow.contains("agent"),
        "the workflow journey must hand off between two personas:\n{workflow}"
    );
    assert!(
        workflow.to_lowercase().contains("transition"),
        "the workflow journey must transition via the POM transition map:\n{workflow}"
    );
    assert!(
        workflow.contains("Bearer") || workflow.contains("apiKey"),
        "the workflow journey's create step must authenticate via the persona API key:\n{workflow}"
    );

    let nav = read(&svelte, "tests/journeys/shell-nav.journey.spec.ts");
    assert!(
        nav.contains("routes"),
        "the nav journey must click through the shell's routes.ts links:\n{nav}"
    );
    assert!(
        nav.contains("getByRole('link')"),
        "the nav journey must click real links, not goto()/open() URLs:\n{nav}"
    );
}

/// POM extensions (presence-gated like the existing families): delete via
/// the overflow menu + confirm, and form cancel.
#[tokio::test]
async fn page_classes_gain_delete_and_cancel() {
    let (_fixture, svelte) = generate_fixture().await;

    let list_page = read(&svelte, "tests/pages/request-list-page.ts");
    assert!(
        list_page.contains("deleteViaMenu"),
        "the list page class must delete via UxTable.deleteViaMenu:\n{list_page}"
    );
    assert!(
        list_page.contains("Delete"),
        "the list page class must expose a Delete()-style method:\n{list_page}"
    );

    let form_page = read(&svelte, "tests/pages/request-form-page.ts");
    assert!(
        form_page.contains("Cancel"),
        "the form page class must expose a Cancel()-style method:\n{form_page}"
    );
}
