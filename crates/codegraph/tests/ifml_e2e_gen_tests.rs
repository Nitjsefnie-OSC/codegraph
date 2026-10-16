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

/// Generate with the auth flag ON and return the fixture + svelte root.
async fn generate_flag_on() -> (Fixture, PathBuf) {
    generate_with(write_fixture(PORTAL_ACTOR, FLAG_ON_PROFILES_TOML)).await
}

async fn generate_with(fixture: Fixture) -> (Fixture, PathBuf) {
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

fn try_read(base: &Path, rel: &str) -> Option<String> {
    fs::read_to_string(base.join(rel)).ok()
}

/// Flag off ⇒ the entire W5 surface disappears: no journey specs, no auth
/// specs, no auth bootstrap, no wizard-step or hop POM methods — and stale
/// artifacts from a previous flag-on run are swept.
#[tokio::test]
async fn flag_off_emits_no_journey_or_gating_surface() {
    let fixture = write_fixture(PORTAL_ACTOR, FLAG_OFF_PROFILES_TOML);
    let output = fixture.root.path().join("generated");
    let svelte_dir = output.join("svelte");

    // Simulate a previous flag-on run's leftovers.
    let journeys = svelte_dir.join("tests").join("journeys");
    fs::create_dir_all(&journeys).unwrap();
    fs::write(journeys.join("home.journey.spec.ts"), "// stale").unwrap();
    fs::write(journeys.join("request-workflow.journey.spec.ts"), "// stale").unwrap();
    let ifml_tests = svelte_dir.join("tests").join("ifml");
    fs::create_dir_all(&ifml_tests).unwrap();
    fs::write(ifml_tests.join("review-queue.auth.spec.ts"), "// stale").unwrap();

    let frameworks = vec!["svelte".to_string()];
    codegraph::driver::run(run_args(&fixture, &frameworks, &output))
        .await
        .expect("flag-off run must succeed");

    // Stale journey + auth specs are swept.
    assert!(
        !journeys.join("home.journey.spec.ts").exists(),
        "stale view journeys must be swept flag-off"
    );
    assert!(
        !journeys.join("request-workflow.journey.spec.ts").exists(),
        "stale workflow journeys must be swept flag-off"
    );
    assert!(
        !ifml_tests.join("review-queue.auth.spec.ts").exists(),
        "stale auth specs must be swept flag-off"
    );
    // Nothing journey/gating-shaped is freshly emitted either.
    let emitted_journeys: Vec<String> = fs::read_dir(&journeys)
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect();
    assert!(
        emitted_journeys.is_empty(),
        "flag off must emit no journey specs: {emitted_journeys:?}"
    );
    assert!(try_read(&svelte_dir, "tests/e2e/auth.setup.ts").is_none());

    // The POM carries none of the W5 surface.
    let wizard = read(&svelte_dir, "tests/pages/request-wizard-page.ts");
    assert!(
        !wizard.contains("step(name: string)"),
        "flag off must keep the wizard step helper out:\n{wizard}"
    );
    let home = read(&svelte_dir, "tests/pages/home-page.ts");
    assert!(
        !home.contains("journeyTo"),
        "flag off must keep the hop methods out:\n{home}"
    );
    let queue = read(&svelte_dir, "tests/pages/review-queue-page.ts");
    assert!(
        !queue.contains("expectDeniedPresentation"),
        "flag off must keep the hidden-surface assertion out:\n{queue}"
    );

    // The historical base-spec output survives: personas stay inline.
    let queue_spec = read(&svelte_dir, "tests/ifml/review-queue.spec.ts");
    assert!(
        queue_spec.contains("addInitScript"),
        "flag off keeps the inline persona seeding in the base specs:\n{queue_spec}"
    );
}

/// Journeys seed what they walk: a param'd source opens with a seeded id,
/// submit hops fill typed fields before clicking, and xor arrivals assert
/// the wizard steps.
#[tokio::test]
async fn journeys_seed_param_sources_and_fill_submits() {
    let (_root, svelte) = generate_flag_on().await;

    let form = read(&svelte, "tests/journeys/request-form.journey.spec.ts");
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

    let form_page = read(&svelte, "tests/pages/request-form-page.ts");
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

    let wizard = read(&svelte, "tests/pages/request-wizard-page.ts");
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
    let (_root, svelte) = generate_flag_on().await;

    let home = read(&svelte, "tests/journeys/home.journey.spec.ts");
    assert!(
        home.contains("import { asPersona, admin } from '../e2e/personas';"),
        "journeys drive through the persona fixtures:\n{home}"
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
    let list = read(&svelte, "tests/journeys/request-list.journey.spec.ts");
    assert!(
        list.contains("asPersona(admin);"),
        "a walk through a roles-guarded view must drive as a permitted persona:\n{list}"
    );
}

/// The gating family renders ONLY when the policy provides personas with
/// differing access; otherwise the auth spec carries the skip comment.
#[tokio::test]
async fn single_persona_policy_skips_gating_with_comment() {
    let (_root, svelte) = generate_with(write_fixture(SOLO_ACTOR, FLAG_ON_PROFILES_TOML)).await;

    let queue = read(&svelte, "tests/ifml/review-queue.auth.spec.ts");
    assert!(
        queue.contains("Gating coverage (issue #468 W5) skipped"),
        "without differing access the gating family must skip with a comment:\n{queue}"
    );
    assert!(
        !queue.contains("gated from"),
        "no gating describe may render without a denied persona:\n{queue}"
    );
}
