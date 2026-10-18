//! Phase 0 red pin for IFML test-side auth bootstrap + journey specs
//! (issue #463 Phase 2, `ifml_e2e_auth`).
//!
//! Pins the generated auth-testing surface for IFML apps:
//!
//! - `tests/e2e/auth.setup.ts` — a Playwright `globalSetup` that provisions
//!   one API key per human actor via `public.create_api_key` (psql over
//!   `DATABASE_URL`) and writes `.auth/{persona}.json` (apiKey, roles,
//!   capabilities); `playwright.config.ts` wires it as `globalSetup`;
//! - `tests/ifml/support/personas.ts` — per-persona `test.use(...)` fixtures
//!   carrying the storage state, the Bearer header, and the
//!   `__USER_ROLES__` / `__USER_CAPABILITIES__` seeding in ONE place (the
//!   per-spec inline `addInitScript` stubs disappear from the base specs);
//! - `{view}.auth.spec.ts` — the auth family: unauthenticated/garbage-key
//!   behavior, denial = redirect AND denial presentation, and control-gating
//!   visibility per capability;
//! - `tests/ifml/*.journey.spec.ts` — IR-derived journeys: the workflow
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

view "Home" {
    label "Home";
    landmark: true;

    component "home_grid" {
        type: list;
        data: Request;
        fields: [title, status];
        pagination: true;

        on select(row) -> navigate("RefundRequestList");
    }
}

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

        on save -> navigate("RequestWizard");
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

view "RefundRequestList" {
    label "Refund Requests";

    component "refund_grid" {
        type: list;
        data: Request;
        fields: [title, status];
        pagination: true;

        on select(row) -> navigate("RefundRequestDetail", {
            id: row.id
        });
    }
}

view "RefundRequestDetail" {
    params { id: Uuid };
    label "Refund Request";

    component "refund_editor" {
        type: form;
        data: Request;
        mode: edit;

        field title -> input text {
            required: true;
        }

        on save -> navigate("RefundRequestForm", {
            id: params.id
        });
    }
}

view "RefundRequestForm" {
    params { id: Uuid };
    label "Edit Refund Request";

    component "refund_form" {
        type: form;
        data: Request;
        mode: edit;

        field title -> input text {
            required: true;
        }
    }
}

view "RequestWizard" {
    label "Request Wizard";
    xor: true;

    container "Plan" {
        default: true;

        component "wizard_plan" {
            type: form;
            data: Request;

            field title -> input text {
                required: true;
            }
        }
    }

    container "Compare" {
        component "wizard_compare" {
            type: details;
            data: Request;
            fields: [title, status];
        }
    }
}

view "AgentBoard" {
    label "Agent Board";
    roles: [agent];

    component "agent_grid" {
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

/// Generate the fixture and return the emitted INTEGRATED ui test tree
/// (issue #468 W1b: the workspace default profile builds the entity ui app
/// alongside the IFML generators).
async fn generate_fixture() -> (Fixture, PathBuf) {
    let fixture = write_fixture();
    let output = fixture.root.path().join("generated");
    let frameworks = vec!["svelte".to_string()];
    codegraph::driver::run(run_args(&fixture, &frameworks, &output))
        .await
        .expect("pipeline run must succeed");
    (fixture, output.join("ui"))
}

fn read(base: &Path, rel: &str) -> String {
    fs::read_to_string(base.join(rel))
        .unwrap_or_else(|e| panic!("expected {rel} to be emitted: {e}"))
}

/// `tests/ifml/support/auth.setup.ts`: the globalSetup provisioning one API
/// key per human actor and writing the per-persona storage files.
/// INTEGRATED mode (issue #468 W1b): the workspace default profile carries
/// the entity ui generators, so the IFML tree integrates into `ui/` and the
/// bootstrap lives under `tests/ifml/support/` — clear of the entity suite's
/// `tests/e2e/` fixtures.
#[tokio::test]
async fn auth_setup_provisions_api_keys_per_persona() {
    let (_fixture, ui) = generate_fixture().await;

    let setup = read(&ui, "tests/ifml/support/auth.setup.ts");
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

    // Integrated mode: the ui tree's entity-owned playwright config runs —
    // the IFML config itself is suppressed. The entity config must carry the
    // ifml project so the IFML specs ride the same playwright run.
    let config = read(&ui, "playwright.config.ts");
    assert!(
        config.contains("name: 'ifml'") && config.contains("/ifml\\/.+\\.spec\\.ts/"),
        "the integrated playwright config must carry the ifml project:\n{config}"
    );
    assert!(
        !ui.join("tests").join("e2e").join("auth.setup.ts").exists(),
        "the IFML bootstrap must not collide with the entity tests/e2e tree"
    );
    assert!(
        !ui.join("tests").join("e2e").join("personas.ts").exists(),
        "the IFML personas module must not collide with tests/e2e/fixtures/personas.ts"
    );
}

/// `tests/ifml/support/personas.ts`: per-persona `test.use` fixtures —
/// storage state, Bearer header, and role/capability seeding in one place;
/// the per-spec inline `addInitScript` stubs are gone from the base specs.
#[tokio::test]
async fn persona_fixture_replaces_inline_init_script_stubs() {
    let (_fixture, ui) = generate_fixture().await;

    let personas = read(&ui, "tests/ifml/support/personas.ts");
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
        let body = read(&ui, spec);
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
    let (_fixture, ui) = generate_fixture().await;

    // Capability-guarded view: Admin passes, Agent is denied.
    let queue = read(&ui, "tests/ifml/review-queue.auth.spec.ts");
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
    let list = read(&ui, "tests/ifml/request-list.auth.spec.ts");
    assert!(
        list.contains("agent"),
        "the roles-guarded view's auth spec must exercise the denied persona:\n{list}"
    );
}

/// The journey family: workflow handoff across personas + nav-click journey
/// through the shell.
#[tokio::test]
async fn journey_specs_cover_workflow_handoff_and_shell_nav() {
    let (_fixture, ui) = generate_fixture().await;

    let workflow = read(&ui, "tests/ifml/request-workflow.journey.spec.ts");
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

    let nav = read(&ui, "tests/ifml/shell-nav.journey.spec.ts");
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
    let (_fixture, ui) = generate_fixture().await;

    let list_page = read(&ui, "tests/pages/request-list-page.ts");
    assert!(
        list_page.contains("deleteViaMenu"),
        "the list page class must delete via UxTable.deleteViaMenu:\n{list_page}"
    );
    assert!(
        list_page.contains("Delete"),
        "the list page class must expose a Delete()-style method:\n{list_page}"
    );

    let form_page = read(&ui, "tests/pages/request-form-page.ts");
    assert!(
        form_page.contains("Cancel"),
        "the form page class must expose a Cancel()-style method:\n{form_page}"
    );
}

/// Issue #468 W5 (a): the navigate() graph becomes journey specs — one
/// `{view}.journey.spec.ts` per source view, walking the maximal chain as
/// steps that assert each hop's URL through the page classes.
#[tokio::test]
async fn journey_spec_walks_the_home_navigation_chain() {
    let (_fixture, ui) = generate_fixture().await;

    let home = read(&ui, "tests/ifml/home.journey.spec.ts");
    assert!(
        home.contains(
            "journey: Home → RefundRequestList → RefundRequestDetail → RefundRequestForm"
        ),
        "the Home journey must walk the full chain as one test:\n{home}"
    );
    // Every hop drives through a page-class journey method — never a raw
    // goto() or testid in the spec body.
    assert!(
        home.contains("await ui.journeyToRefundRequestList();"),
        "the first hop must drive the source page class:\n{home}"
    );
    assert!(
        home.contains("new RefundRequestListPage(page)"),
        "arrival assertions drive the target page class:\n{home}"
    );
    assert!(
        home.contains("toHaveURL(new RegExp('/refundrequestlist$'))"),
        "steps must assert the target view's route:\n{home}"
    );
    assert!(
        !home.contains("goto("),
        "journeys walk by clicking, never goto():\n{home}"
    );
    // The hop methods live on the source page classes (auth-gated surface).
    let home_page = read(&ui, "tests/pages/home-page.ts");
    assert!(
        home_page.contains("async journeyToRefundRequestList()"),
        "the source page class must carry the hop method:\n{home_page}"
    );
    assert!(
        home_page.contains("waitForURL(new RegExp('/refundrequestlist$'))"),
        "the hop method must wait for the landing route:\n{home_page}"
    );

    // Multi-hop chaining: the intermediate views get their own walks, and
    // prefixes are not re-emitted as separate journeys.
    let list = read(&ui, "tests/ifml/refund-request-list.journey.spec.ts");
    assert!(
        list.contains("journeyToRefundRequestDetail")
            && list.contains("journeyToRefundRequestForm"),
        "the list journey must chain into the detail and form views:\n{list}"
    );
    assert!(
        list.matches("test('journey:").count() == 1,
        "one journey per maximal path (no prefix duplicates):\n{list}"
    );
}

/// Issue #468 W5 (b): personas LACKING a guarded view's roles/capabilities
/// get explicit gating tests — the denial redirect AND the hidden surface.
#[tokio::test]
async fn gating_spec_asserts_the_denied_persona_is_redirected() {
    let (_fixture, ui) = generate_fixture().await;

    let queue = read(&ui, "tests/ifml/review-queue.auth.spec.ts");
    // The fixture policy: Admin holds ApproveRequest, Agent does not — a
    // differing-access pair, so the gating family must render.
    assert!(
        queue.contains("test.describe('Review Queue gated from agent'"),
        "the gating describe must name the denied persona:\n{queue}"
    );
    assert!(
        queue.contains("asPersona(agent);"),
        "the gating tests drive as the denied persona:\n{queue}"
    );
    assert!(
        queue.contains("await ui.expectDenied();"),
        "the gating test must assert the denial redirect:\n{queue}"
    );
    assert!(
        queue.contains("await ui.expectDeniedPresentation('"),
        "the gating test must assert the guarded surface stays hidden:\n{queue}"
    );
    assert!(
        queue.contains("ApproveRequest gating"),
        "the hidden-surface reason must name the gating capability:\n{queue}"
    );

    // The hidden-surface assertion resolves on the page class (issue #468
    // W5 closed the missing-method gap for the whole auth family).
    let queue_page = read(&ui, "tests/pages/review-queue-page.ts");
    assert!(
        queue_page.contains("async expectDeniedPresentation(reason: string)"),
        "the page class must render the hidden-surface assertion:\n{queue_page}"
    );
}

/// Issue #468 W5 (c): xor wizard views expose a `step(name)` helper — one
/// record entry per nested container — and journeys arriving on an xor
/// target assert each step's visibility.
#[tokio::test]
async fn wizard_page_class_exposes_step_helpers() {
    let (_fixture, ui) = generate_fixture().await;

    let wizard = read(&ui, "tests/pages/request-wizard-page.ts");
    assert!(
        wizard.contains("step(name: string): Locator"),
        "the xor view's page class must expose the step helper:\n{wizard}"
    );
    for container in ["Plan", "Compare"] {
        assert!(
            wizard.contains(&format!("'{container}':")),
            "the step record must carry the {container} container:\n{wizard}"
        );
    }

    // A journey arriving on the wizard asserts the steps through it.
    let form_journey = read(&ui, "tests/ifml/request-form.journey.spec.ts");
    assert!(
        form_journey.contains("wizard.step('Plan')")
            && form_journey.contains("wizard.step('Compare')"),
        "the journey into the wizard must assert the wizard steps:\n{form_journey}"
    );
}

/// Issue #468 hydration follow-up: in integrated mode the `(views)` group
/// carries the role-hydration layouts — the server load derives role tokens
/// from the session's BaseJump account_role (the same get_accounts source
/// the entity `(app)` guard layout reads), and the universal load publishes
/// them onto `globalThis.__USER_ROLES__` BEFORE the child +page.ts guards
/// run. Mapping convention: the fixture policy has no Owner/Member actor,
/// so owner falls back to the FIRST policy actor (Admin) and member to the
/// LAST (Agent); each mapping carries the policy token AND its snake_case
/// form. The server load never redirects — persona contexts carry no
/// session on purpose and must reach the guarded pages.
#[tokio::test]
async fn hydration_layouts_map_basejump_roles_to_policy_tokens() {
    let (_fixture, ui) = generate_fixture().await;

    let server = read(&ui, "src/routes/(views)/+layout.server.ts");
    assert!(
        server.contains("get_accounts") && server.contains("account_role"),
        "the server layout must derive roles from the BaseJump account:\n{server}"
    );
    assert!(
        server.contains("const OWNER_ROLE_TOKENS: string[] = ['Admin', 'admin'];"),
        "owner maps to the first policy actor (token + snake form):\n{server}"
    );
    assert!(
        server.contains("const MEMBER_ROLE_TOKENS: string[] = ['Agent', 'agent'];"),
        "member maps to the last policy actor (token + snake form):\n{server}"
    );
    assert!(
        !server.contains("redirect("),
        "the hydration layout must NOT redirect — personas have no session \
         by design and the client guards deny instead:\n{server}"
    );
    assert!(
        server.contains("roles: null"),
        "a sessionless visit hydrates nothing (roles: null):\n{server}"
    );

    let universal = read(&ui, "src/routes/(views)/+layout.ts");
    assert!(
        universal.contains("(globalThis as any).__USER_ROLES__ = data.roles;"),
        "the universal layout must publish roles onto the guard surface:\n{universal}"
    );
    assert!(
        universal.contains("if (typeof window !== 'undefined' && data.roles) {"),
        "null roles must never clobber persona init-script seeding:\n{universal}"
    );
}

/// The persona fixtures seed BOTH token forms, so a snake_case-guarded view
/// (`roles: [agent]`, the crewbase convention) admits the Agent persona and
/// denies the rest — the allow/deny pair the gating specs assert.
#[tokio::test]
async fn persona_seeding_matches_snake_case_role_guards() {
    let (_fixture, ui) = generate_fixture().await;

    let personas = read(&ui, "tests/ifml/support/personas.ts");
    assert!(
        personas.contains("roles: ['Agent', 'agent']"),
        "personas must seed the policy token AND its snake_case form:\n{personas}"
    );
    assert!(personas.contains("roles: ['Admin', 'admin']"), "{personas}");

    // The guard admits the agent persona (its seeded token matches the
    // snake_case view role) and gates the admin persona out.
    let board = read(&ui, "tests/ifml/agent-board.auth.spec.ts");
    assert!(
        board.contains("test.describe('Agent Board as agent'"),
        "the snake-token view must admit the agent persona:\n{board}"
    );
    assert!(
        board.contains("test.describe('Agent Board gated from admin'"),
        "the owner-tier persona must be gated from the member-tier view:\n{board}"
    );
}
