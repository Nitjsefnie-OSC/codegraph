//! W5 generator-surface tests (issue #468): the IFML e2e generator turns
//! the views' `navigate(...)` graphs into journey specs, gates
//! roles:/requires: views per persona, and gives xor wizard views a
//! `step()` POM surface — all behind `project.codegen.ifml_e2e_auth`.
//!
//! Where `ifml_auth_setup_tests.rs` pins the presence side over the portal
//! fixture (flag on), this file pins the generator CONTRACT:
//!
//! - flag off ⇒ NO journey/gating/auth surface is emitted and stale files
//!   from a previous flag-on run are swept;
//! - journeys seed what they walk (param'd sources open with a seeded id,
//!   row hops seed rows, submit hops fill typed fields first);
//! - the gating family only renders when the policy provides personas with
//!   differing access (a single-actor policy emits the skip comment).
//!
//! Node-free: runs the full pipeline over the fixture and asserts the
//! emitted artifacts.
//!
//! ```text
//! cargo test -p codegraph --test ifml_e2e_gen_tests
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

/// A single-actor policy: no persona pair with differing access exists, so
/// the gating family must skip with a comment.
const SOLO_ACTOR: &str = r#"import "portal.mox"

actors Portal {
    actor Admin

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

/// The flag-off twin of the workspace default profile: same generator
/// selection, `ifml_e2e_auth` explicitly off (byte-identical discipline).
/// The ui + IFML generator mix puts these fixtures in INTEGRATED mode
/// (issue #468 W1b): IFML artifacts land inside the `ui/` tree.
const FLAG_OFF_PROFILES_TOML: &str = r#"
[profiles.default.features]
ux_rules = true
ifml_backend = true
ifml_e2e_auth = false

[profiles.default.ui]
generators = [
    "ui_page", "ui_form", "ui_store", "ui_e2e_test",
    "playwright-entity", "ui_descriptor", "ui-shell",
    "ui-domain-layout",
    "ui_scaffold", "ui_types", "ui_codelist", "ui_orgchart", "playwright-global",
    "ifml_skeleton", "ifml_route", "ifml_navigation", "ifml_e2e_test",
]
output = "review/generated-candidate/"
scripts.post_gen = []
"#;

/// The flag-on twin: identical selection, the W5/auth surface enabled.
const FLAG_ON_PROFILES_TOML: &str = r#"
[profiles.default.features]
ux_rules = true
ifml_backend = true
ifml_e2e_auth = true

[profiles.default.ui]
generators = [
    "ui_page", "ui_form", "ui_store", "ui_e2e_test",
    "playwright-entity", "ui_descriptor", "ui-shell",
    "ui-domain-layout",
    "ui_scaffold", "ui_types", "ui_codelist", "ui_orgchart", "playwright-global",
    "ifml_skeleton", "ifml_route", "ifml_navigation", "ifml_e2e_test",
]
output = "review/generated-candidate/"
scripts.post_gen = []
"#;

/// The STANDALONE twin (issue #468 W1b): the same IFML generators with the
/// entity ui app removed from the plan — the disjoint `{fw}/` tree with the
/// pre-W1b paths must stay byte-compatible.
const STANDALONE_PROFILES_TOML: &str = r#"
[profiles.default.features]
ux_rules = true
ifml_backend = true
ifml_e2e_auth = true

[profiles.default.ui]
generators = [
    "ifml_skeleton", "ifml_route", "ifml_navigation", "ifml_e2e_test",
]
output = "review/generated-candidate/"
scripts.post_gen = []
"#;

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
    mox: PathBuf,
    ifml: PathBuf,
    profiles: PathBuf,
}

fn write_fixture(actor: &str, profiles: &str) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("domains.toml"), DOMAINS_TOML).unwrap();
    fs::write(root.path().join("portal.mox"), PORTAL_MOX).unwrap();
    fs::write(root.path().join("portal.actor"), actor).unwrap();
    let ifml = root.path().join("app.ifml");
    fs::write(&ifml, APP_IFML).unwrap();
    let profiles_path = root.path().join("profiles.toml");
    fs::write(&profiles_path, profiles).unwrap();
    Fixture {
        config: root.path().join("domains.toml"),
        mox: root.path().join("portal.mox"),
        ifml,
        profiles: profiles_path,
        root,
    }
}

fn run_args<'a>(
    fixture: &'a Fixture,
    ifml_framework: &'a [String],
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
        profiles_config_path: Some(fixture.profiles.clone()),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: std::slice::from_ref(&fixture.ifml),
        openapi_files: &[],
        mox_files: std::slice::from_ref(&fixture.mox),
        rosetta_files: &[],
        ddd_files: &[],
        evt_files: &[],
        deploy_files: &[],
        ifml_framework,
        ifml_components: None,
        ifml_design_system: None,
        codegraph_rev: None,
        check: false,
        ux_rules: None,
    }
}

/// Generate with the auth flag ON and return the fixture + the integrated
/// ui root (the ui + IFML plan integrates the IFML tree into `ui/`).
async fn generate_flag_on() -> (Fixture, PathBuf) {
    generate_with(write_fixture(PORTAL_ACTOR, FLAG_ON_PROFILES_TOML)).await
}

async fn generate_with(fixture: Fixture) -> (Fixture, PathBuf) {
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

fn try_read(base: &Path, rel: &str) -> Option<String> {
    fs::read_to_string(base.join(rel)).ok()
}

/// Flag off ⇒ the entire W5 surface disappears: no journey specs, no auth
/// specs, no auth bootstrap, no wizard-step or hop POM methods — and stale
/// artifacts from a previous flag-on run are swept. Integrated mode: the
/// tree is `ui/`, journeys live under `tests/ifml/`, the bootstrap under
/// `tests/ifml/support/`.
#[tokio::test]
async fn flag_off_emits_no_journey_or_gating_surface() {
    let fixture = write_fixture(PORTAL_ACTOR, FLAG_OFF_PROFILES_TOML);
    let output = fixture.root.path().join("generated");
    let ui_dir = output.join("ui");

    // Simulate a previous flag-on run's leftovers.
    let ifml_tests = ui_dir.join("tests").join("ifml");
    fs::create_dir_all(&ifml_tests).unwrap();
    fs::write(ifml_tests.join("home.journey.spec.ts"), "// stale").unwrap();
    fs::write(
        ifml_tests.join("request-workflow.journey.spec.ts"),
        "// stale",
    )
    .unwrap();
    fs::write(ifml_tests.join("review-queue.auth.spec.ts"), "// stale").unwrap();
    let support = ifml_tests.join("support");
    fs::create_dir_all(&support).unwrap();
    fs::write(support.join("auth.setup.ts"), "// stale").unwrap();
    fs::write(support.join("personas.ts"), "// stale").unwrap();

    let frameworks = vec!["svelte".to_string()];
    codegraph::driver::run(run_args(&fixture, &frameworks, &output))
        .await
        .expect("flag-off run must succeed");

    // Stale journey + auth specs + bootstrap files are swept.
    assert!(
        !ifml_tests.join("home.journey.spec.ts").exists(),
        "stale view journeys must be swept flag-off"
    );
    assert!(
        !ifml_tests.join("request-workflow.journey.spec.ts").exists(),
        "stale workflow journeys must be swept flag-off"
    );
    assert!(
        !ifml_tests.join("review-queue.auth.spec.ts").exists(),
        "stale auth specs must be swept flag-off"
    );
    assert!(
        !support.join("auth.setup.ts").exists() && !support.join("personas.ts").exists(),
        "stale auth bootstrap must be swept flag-off"
    );
    // Nothing journey/gating-shaped is freshly emitted either.
    let emitted_journeys: Vec<String> = fs::read_dir(&ifml_tests)
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|name| name.ends_with(".journey.spec.ts"))
        .collect();
    assert!(
        emitted_journeys.is_empty(),
        "flag off must emit no journey specs: {emitted_journeys:?}"
    );
    assert!(try_read(&ui_dir, "tests/ifml/support/auth.setup.ts").is_none());

    // The POM carries none of the W5 surface.
    let wizard = read(&ui_dir, "tests/pages/request-wizard-page.ts");
    assert!(
        !wizard.contains("step(name: string)"),
        "flag off must keep the wizard step helper out:\n{wizard}"
    );
    let home = read(&ui_dir, "tests/pages/home-page.ts");
    assert!(
        !home.contains("journeyTo"),
        "flag off must keep the hop methods out:\n{home}"
    );
    let queue = read(&ui_dir, "tests/pages/review-queue-page.ts");
    assert!(
        !queue.contains("expectDeniedPresentation"),
        "flag off must keep the hidden-surface assertion out:\n{queue}"
    );

    // The historical base-spec output survives: personas stay inline.
    let queue_spec = read(&ui_dir, "tests/ifml/review-queue.spec.ts");
    assert!(
        queue_spec.contains("addInitScript"),
        "flag off keeps the inline persona seeding in the base specs:\n{queue_spec}"
    );

    // The entity-owned playwright config carries the ifml project (the
    // IFML config itself is suppressed: the ui config already exists).
    let config = read(&ui_dir, "playwright.config.ts");
    assert!(
        config.contains("name: 'ifml'") && config.contains("/ifml\\/.+\\.spec\\.ts/"),
        "the integrated playwright config must gain the ifml project:\n{config}"
    );
    assert!(
        !ui_dir.join("tests").join("journeys").exists(),
        "integrated mode must not resurrect the standalone journeys dir"
    );
}

/// Journeys seed what they walk: a param'd source opens with a seeded id,
/// submit hops fill typed fields before clicking, and xor arrivals assert
/// the wizard steps.
#[tokio::test]
async fn journeys_seed_param_sources_and_fill_submits() {
    let (_root, ui) = generate_flag_on().await;

    let form = read(&ui, "tests/ifml/request-form.journey.spec.ts");
    assert!(
        form.contains("request.post("),
        "the param'd source journey must seed an entity:\n{form}"
    );
    assert!(
        form.contains("await ui.open({ id: seededId });"),
        "the param'd source must open with the seeded id:\n{form}"
    );
    assert!(
        form.contains("await ui.journeyToRequestWizard();"),
        "the submit hop drives through the page class:\n{form}"
    );

    let form_page = read(&ui, "tests/pages/request-form-page.ts");
    assert!(
        form_page.contains("async journeyToRequestWizard()"),
        "the submit hop method must render:\n{form_page}"
    );
    assert!(
        form_page.contains("await this.fillEditorTitle("),
        "the submit hop must fill the typed required field first:\n{form_page}"
    );
    assert!(
        form_page.contains("await this.editorSubmit().click();"),
        "the submit hop must click the form's submit control:\n{form_page}"
    );
    assert!(
        form_page.contains("waitForURL(new RegExp('/requestwizard$'))"),
        "the hop method must wait for the landing route:\n{form_page}"
    );

    let wizard = read(&ui, "tests/pages/request-wizard-page.ts");
    assert!(
        wizard.contains("'Plan': 'wizard_plan-form'")
            && wizard.contains("'Compare': 'wizard_compare-details'"),
        "the step record must map containers to their component surfaces:\n{wizard}"
    );
}

/// Journeys drive as personas: the describe binds asPersona, and the persona
/// is one permitted on every walked view (the fixture's Home chain is
/// unguarded, so the first persona drives it).
#[tokio::test]
async fn journeys_bind_a_persona_permitted_on_every_view() {
    let (_root, ui) = generate_flag_on().await;

    let home = read(&ui, "tests/ifml/home.journey.spec.ts");
    assert!(
        home.contains("import { asPersona, admin } from './support/personas';"),
        "journeys drive through the persona fixtures (integrated: under support/):\n{home}"
    );
    assert!(
        home.contains("asPersona(admin);"),
        "the journey describe must bind the persona:\n{home}"
    );
    assert!(
        home.contains("const rowSeed0 = await request.post("),
        "row hops seed the rows they click:\n{home}"
    );

    // A guarded view in the path filters the persona choice; the
    // Admin-only-guarded RequestList journey must drive as admin (the only
    // persona with the Admin role).
    let list = read(&ui, "tests/ifml/request-list.journey.spec.ts");
    assert!(
        list.contains("asPersona(admin);"),
        "a walk through a roles-guarded view must drive as a permitted persona:\n{list}"
    );
}

/// Integrated mode (issue #468 W1b): view pages land inside the ui app's
/// `(views)` route group — sibling of the entity `(app)` group — with the
/// group-scoped shell layout, while the URL surface (routes.ts, POM routes)
/// stays group-free.
#[tokio::test]
async fn integrated_views_land_in_the_route_group() {
    let (_root, ui) = generate_flag_on().await;

    let home_page = read(&ui, "src/routes/(views)/home/+page.svelte");
    assert!(
        home_page.contains("import"),
        "the view page must be emitted inside the (views) group:\n{home_page}"
    );
    read(&ui, "src/routes/(views)/home/+page.ts");
    assert!(
        !ui.join("src/routes/+layout.svelte").exists(),
        "integrated mode must not emit a root layout over the entity app"
    );
    // Standalone route files must not appear next to the group.
    assert!(
        !ui.join("src/routes/home").exists(),
        "integrated view pages must not land at the default route root"
    );
    // The navigation map lands in the ui tree's $lib, with group-free URLs.
    let routes = read(&ui, "src/lib/routes.ts");
    assert!(
        routes.contains("'/home'") || routes.contains("\"/home\""),
        "route groups must not leak into the URL surface:\n{routes}"
    );
}

/// Standalone mode (issue #468 W1b): an ifml-only plan keeps the disjoint
/// `svelte/` tree with the pre-W1b paths — journeys in `tests/journeys/`,
/// personas at `tests/e2e/personas.ts`, and the IFML-owned playwright
/// config wired to the auth globalSetup.
#[tokio::test]
async fn standalone_ifml_only_plan_keeps_the_disjoint_tree() {
    let fixture = write_fixture(PORTAL_ACTOR, STANDALONE_PROFILES_TOML);
    let output = fixture.root.path().join("generated");
    let frameworks = vec!["svelte".to_string()];
    codegraph::driver::run(run_args(&fixture, &frameworks, &output))
        .await
        .expect("standalone run must succeed");
    let svelte = output.join("svelte");
    assert!(
        !output.join("ui").exists(),
        "an ifml-only plan must not touch a ui tree"
    );

    let home = read(&svelte, "tests/journeys/home.journey.spec.ts");
    assert!(
        home.contains("import { asPersona, admin } from '../e2e/personas';"),
        "standalone journeys import the tests/e2e personas module:\n{home}"
    );
    let setup = read(&svelte, "tests/e2e/auth.setup.ts");
    assert!(
        setup.contains("create_api_key"),
        "standalone keeps the bootstrap in tests/e2e:\n{setup}"
    );
    let personas = read(&svelte, "tests/e2e/personas.ts");
    assert!(personas.contains("asPersona"), "{personas}");
    let config = read(&svelte, "playwright.config.ts");
    assert!(
        config.contains("globalSetup: 'tests/e2e/auth.setup.ts'"),
        "standalone config wires the IFML auth globalSetup:\n{config}"
    );
    read(&svelte, "src/routes/home/+page.svelte");
}

/// The gating family renders ONLY when the policy provides personas with
/// differing access; otherwise the auth spec carries the skip comment.
#[tokio::test]
async fn single_persona_policy_skips_gating_with_comment() {
    let (_root, ui) = generate_with(write_fixture(SOLO_ACTOR, FLAG_ON_PROFILES_TOML)).await;

    let queue = read(&ui, "tests/ifml/review-queue.auth.spec.ts");
    assert!(
        queue.contains("import { admin, asPersona } from './support/personas';"),
        "integrated auth specs import the support personas module:\n{queue}"
    );
    assert!(
        queue.contains("Gating coverage (issue #468 W5) skipped"),
        "without differing access the gating family must skip with a comment:\n{queue}"
    );
    assert!(
        !queue.contains("gated from"),
        "no gating describe may render without a denied persona:\n{queue}"
    );
}

/// Issue #468 hydration: integrated mode emits the `(views)` role-hydration
/// layouts (session → BaseJump account → policy tokens → `__USER_ROLES__`);
/// standalone keeps the disjoint tree free of them — byte-identical output,
/// no hydration file at the default route root either.
#[tokio::test]
async fn hydration_layouts_are_integrated_only() {
    // Integrated flag-on: the hydration pair rides the route generator.
    let (_root, ui) = generate_flag_on().await;

    let server = read(&ui, "src/routes/(views)/+layout.server.ts");
    assert!(
        server.contains("get_accounts"),
        "the integrated server layout derives roles from the BaseJump session:\n{server}"
    );
    assert!(
        server.contains("const OWNER_ROLE_TOKENS: string[] = ['Admin', 'admin'];")
            && server.contains("const MEMBER_ROLE_TOKENS: string[] = ['Agent', 'agent'];"),
        "the mapping convention (first/last policy actor fallback) is pinned:\n{server}"
    );
    assert!(
        !server.contains("redirect("),
        "no redirect: persona contexts have no session by design:\n{server}"
    );
    let universal = read(&ui, "src/routes/(views)/+layout.ts");
    assert!(
        universal.contains("__USER_ROLES__"),
        "the universal layout hydrates the guard surface:\n{universal}"
    );

    // Standalone twin: the disjoint svelte tree gains NO hydration layout —
    // not under the route group (which doesn't exist there) and not at the
    // default route root.
    let fixture = write_fixture(PORTAL_ACTOR, STANDALONE_PROFILES_TOML);
    let output = fixture.root.path().join("generated");
    let frameworks = vec!["svelte".to_string()];
    codegraph::driver::run(run_args(&fixture, &frameworks, &output))
        .await
        .expect("standalone run must succeed");
    let svelte = output.join("svelte");
    assert!(
        !svelte.join("src/routes/(views)").exists(),
        "standalone mode must not create the (views) route group"
    );
    assert!(
        !svelte.join("src/routes/+layout.server.ts").exists()
            && !svelte.join("src/routes/+layout.ts").exists(),
        "standalone output stays byte-identical — no hydration layouts"
    );
}
