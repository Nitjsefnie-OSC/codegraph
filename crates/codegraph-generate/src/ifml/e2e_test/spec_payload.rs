//! Spec payload models for the IFML e2e generator (issue #317).
//!
//! The per-view test assembly the generator builds from the graph BEFORE
//! any TypeScript is rendered: one [`ViewTestSpec`] per view plus the
//! sibling workflow/ux payloads. The render functions consume them, and —
//! since #317 — the POM page-class builder consumes the same payloads to
//! gate its spec-driven surface (flow methods exist only for flows the
//! specs exercise, keeping schema-less runs free of navigation surface).

use super::super::route_generator::RenderWorkflow;

pub(crate) use super::fixtures::Fixture;

/// The (from → to) edge a spec can safely exercise: with a populated
/// transitions map, the initial state's first (sorted) target; with an
/// empty map, the first non-terminal state other than the initial one.
pub(crate) fn pick_transition(workflow: &RenderWorkflow) -> Option<(String, String)> {
    if workflow.transition_map.is_empty() {
        return workflow
            .states
            .iter()
            .find(|s| !workflow.terminal_states.contains(s) && **s != workflow.initial_state)
            .map(|s| (workflow.initial_state.clone(), s.clone()));
    }
    let targets = workflow.transition_map.get(&workflow.initial_state)?;
    let mut sorted: Vec<&String> = targets.iter().collect();
    sorted.sort();
    sorted
        .first()
        .map(|to| (workflow.initial_state.clone(), (*to).clone()))
}

// ── Test models ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct ViewTestSpec {
    pub view_name: String,
    pub label: String,
    pub route: String,
    pub render: Option<RenderTest>,
    pub click_throughs: Vec<ClickThroughTest>,
    pub validations: Vec<ValidationTest>,
    pub round_trips: Vec<RoundTripTest>,
    /// Actor-persona guard tests (policy-gated): per human actor, one test
    /// asserting the view renders when permitted or redirects to the denial
    /// target when not.
    pub personas: Vec<PersonaTest>,
}

/// One actor-persona guard test: seeds `__USER_ROLES__` (and
/// `__USER_CAPABILITIES__` for capability-only views) before navigating,
/// then asserts the permitted outcome (page renders) or the denied one
/// (redirect to the denial target).
#[derive(Debug, Clone)]
pub struct PersonaTest {
    pub actor: String,
    pub permitted: bool,
    pub route: String,
    pub label: String,
    pub assert_heading: bool,
    pub primary_testid: Option<String>,
    pub denial_target: String,
    /// Capabilities seeded into `__USER_CAPABILITIES__` for capability-only
    /// views (requires without roles); `None` skips the seeding.
    pub capabilities: Option<Vec<String>>,
    /// Submit-control testid asserted visible for permitted personas when
    /// the view carries an unmapped form (the page markup gates it behind
    /// the same guard checks); `None` skips the assertion.
    pub control_testid: Option<String>,
    /// The component owning [`PersonaTest::control_testid`] — names the
    /// POM accessor (`{component}Submit`) the persona assertion drives.
    /// `None` whenever `control_testid` is `None` (#317).
    pub control_component: Option<String>,
}

#[derive(Debug)]
pub struct RenderTest {
    pub label: String,
    pub route: String,
    pub primary_testid: String,
    pub assert_heading: bool,
    /// Landmark shell nav testid (resolved `shell` mapping) asserted visible
    /// on every page that renders inside the layout.
    pub nav_testid: Option<String>,
    /// Mapped presentation-container wrapper testid for xor view containers.
    pub container_testid: Option<String>,
}

#[derive(Debug)]
pub struct ClickThroughTest {
    pub title: String,
    pub source_route: String,
    /// The component whose row is clicked — names the POM flow surface.
    pub source_component: String,
    /// The navigation target view — names the `navigateTo{Target}` /
    /// `close{Target}Modal` POM methods (#317).
    pub target_view: String,
    pub row_testid: String,
    pub fixture: Fixture,
    pub target_pattern: String,
    /// Assertions for navigation into a `modal: true` target: wrapper
    /// visibility, close-button click, and the URL pattern after close.
    pub modal: Option<ModalCloseAssertions>,
}

#[derive(Debug)]
pub struct ModalCloseAssertions {
    pub wrapper_testid: String,
    pub close_testid: String,
    pub back_pattern: String,
}

#[derive(Debug)]
pub struct ValidationTest {
    /// The form component under test — names the POM accessors (#317).
    pub component: String,
    pub route: String,
    pub form_testid: String,
    pub submit_testid: String,
    pub field: String,
}

#[derive(Debug)]
pub struct RoundTripTest {
    /// The form component under test — names the POM accessors (#317).
    pub component: String,
    pub route: String,
    pub id_param: String,
    pub form_testid: String,
    pub submit_testid: String,
    pub field: String,
    pub original: String,
    pub updated: String,
    pub fixture: Fixture,
    pub target_pattern: String,
}

/// One workflow state assertion inside a view's `{view}.workflow.spec.ts`:
/// create a fixture via the API, open the view, and assert the state badge
/// shows the configured initial state (mirrors the non-IFML
/// `{entity}.workflow.test.ts` convention). Details/form components also
/// carry a transition round trip when a valid (from → to) edge exists.
#[derive(Debug)]
pub struct WorkflowTest {
    /// Human-readable component name used in the test title.
    pub component_name: String,
    pub route: String,
    pub id_param: Option<String>,
    pub state_testid: String,
    pub initial_state: String,
    /// Collection badges render per row (`{#each}`): state assertions use
    /// `.first()` to stay strict-mode-safe.
    pub is_collection: bool,
    /// Transition round trip (details/form only): initial state → first
    /// valid target via the `{component}-transition-{target}` button.
    pub transition: Option<TransitionStep>,
    pub fixture: Fixture,
}

/// The transition a workflow spec exercises: click the enabled
/// `{component}-transition-{target}` button, assert the badge shows the
/// target state, and confirm persistence via a GET.
#[derive(Debug)]
pub struct TransitionStep {
    pub from: String,
    pub to: String,
    pub to_testid: String,
}

/// The ux-rules rendering tests for one view (issue #303), rendered into
/// `{view}.ux.spec.ts`. Every check is generated only for markup the
/// fallback template actually renders (chips, numeric alignment, Intl
/// formatting, copy-chip, overflow menu, timeline) — no vacuous tests.
#[derive(Debug)]
pub struct UxViewTest {
    /// The fallback list component carrying the asserted markup.
    pub component_name: String,
    pub route: String,
    pub fixture: Fixture,
    pub table_testid: String,
    pub row_testid: String,
    /// Chip columns: seeded value, its rendered text, and the expected
    /// `data-chip-variant` (the column's ToneMap lookup).
    pub chip_checks: Vec<UxChipCheck>,
    /// Numeric/date column cells: nth-child position, alignment class, and
    /// the Intl formatter the page runs for the column's dimension.
    pub column_checks: Vec<UxColumnCheck>,
    /// Copy-chip column: click writes the cell value to the clipboard.
    pub copy_check: Option<UxCopyCheck>,
    /// Overflow menu (second+ navigate events disclosed by tiering).
    pub menu: Option<UxMenuCheck>,
    /// Timeline layout entries (explicit `[[collection]]` rule).
    pub timeline: Option<UxTimelineCheck>,
    /// Locale/currency baseline feeding the in-spec Intl computations.
    pub locale: String,
    /// Ready-to-render money options object literal (`{}` when the pack
    /// carries no currency).
    pub money_options: String,
}

/// One chip assertion: create a row, then expect the row's `{comp}-chip`
/// carrying `text` to show `variant`. `override_literal` is `Some` when the
/// row is created with `field` overridden to it; `None` for workflow status
/// chips (issue #311): the create DTO excludes the status field, so the
/// row's chip is pinned to the initial state the DDL default materializes.
#[derive(Debug)]
pub struct UxChipCheck {
    pub field: String,
    pub override_literal: Option<String>,
    pub text: String,
    pub variant: String,
}

/// One numeric/date cell assertion: the cell at `cell_index` (0-based within
/// the row) renders `literal` through the Intl formatter of `format`; the
/// `text-right` header/cell class asserts fire only on right-aligned
/// columns (mirroring the template).
#[derive(Debug)]
pub struct UxColumnCheck {
    /// Header name when the column is right-aligned (asserts `text-right`
    /// on the `<th>`).
    pub header: Option<String>,
    pub cell_index: usize,
    pub format: UxCellFormat,
    pub assert_right: bool,
}

/// The Intl formatter a column's dimension renders through (mirroring the
/// page's `formatMoney`/`formatNumber`/`formatDate`).
#[derive(Debug)]
pub enum UxCellFormat {
    Money { literal: String },
    Quantity { literal: String },
    Date { literal: String },
}

/// One copy-chip assertion: clicking `{comp}-copy` writes the created
/// entity's `field` value to the clipboard.
#[derive(Debug)]
pub struct UxCopyCheck {
    pub field: String,
}

/// Overflow-menu assertions: open the trigger, expect the menu, and follow
/// its first item to the navigation target.
#[derive(Debug)]
pub struct UxMenuCheck {
    pub trigger_testid: String,
    pub menu_testid: String,
    pub target_pattern: String,
}

/// Timeline assertions: the `{comp}-timeline` root renders items newest-first
/// with real `<time>` content matching the fixture's order value.
#[derive(Debug)]
pub struct UxTimelineCheck {
    pub root_testid: String,
    pub item_testid: String,
    /// The fixture's value for the timeline `order_by` field, when present.
    pub order_literal: Option<String>,
}

// ── Navigation journey models (issue #468 W5, `ifml_e2e_auth`-gated) ────────

/// The control an IFML navigate event binds to, as the fallback markup wires
/// it: a collection's first navigate event drives its row click; a form's
/// save/submit drives the submit control (after fixture fills). Events with
/// no fallback control (row-menu disclosures, cancel/back on unmapped
/// forms, details/chart events) never become hops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JourneyHopKind {
    Row,
    Submit,
}

/// One drivable navigation hop: the event's control plus the landing URL.
#[derive(Debug, Clone)]
pub struct JourneyHop {
    /// The component owning the control — names the POM accessors.
    pub source_component: String,
    pub kind: JourneyHopKind,
    /// The view the hop lands on.
    pub target_view: String,
    /// `RegExp` source matching the landing URL (target route + bindings).
    pub target_pattern: String,
    /// API fixture seeded so the control has data (row hops); `None` for
    /// submit hops.
    pub seed: Option<Fixture>,
    /// POM fill method + literal pairs a submit hop runs before clicking
    /// (typed form fields); empty for row hops.
    pub fills: Vec<(String, String)>,
}

/// One maximal navigate() path from a source view (issue #468 W5): a wizard
/// reads as a walk — one journey per path, prefixes deduped by construction
/// (the DFS only records leaves).
#[derive(Debug)]
pub struct JourneyPath {
    /// The view sequence INCLUDING the source view; titles + persona gating
    /// read from it. `steps.len() == views.len() - 1`.
    pub views: Vec<String>,
    /// One hop per edge, in walk order.
    pub steps: Vec<JourneyStep>,
}

/// One step of a journey path: the hop payload plus the resolved arrival
/// assertions (POM method on the source page class; the target surface the
/// target page class actually exposes).
#[derive(Debug)]
pub struct JourneyStep {
    pub hop: JourneyHop,
    /// The POM method on the SOURCE view's page class driving the hop
    /// (`journeyTo{Target}`), collision-suffixed like the flow methods.
    pub method: String,
    /// Whether an arrival surface assertion is rendered (the target page
    /// class exposes one); `arrival_heading` picks `heading()` over
    /// `primaryRoot()` (the collection convention).
    pub arrival: bool,
    pub arrival_heading: bool,
    /// xor wizard step names asserted visible after arriving on an xor
    /// target (`step('Name')`).
    pub target_steps: Vec<String>,
}

/// One `{view}.journey.spec.ts` payload: the maximal navigate() walks from
/// one source view, driven by one persona permitted on every walked view.
#[derive(Debug)]
pub struct ViewJourney {
    pub view_name: String,
    pub label: String,
    /// The persona driving every path (`None` never renders — paths without
    /// a permitted persona are dropped at assembly).
    pub persona: Option<String>,
    /// The source view's id param; set when the journey must open with a
    /// seeded entity id.
    pub open_param: Option<String>,
    /// The API fixture seeding `open_param`.
    pub open_seed: Option<Fixture>,
    /// Row-data seeds (distinct entities across the journey's row hops).
    pub seeds: Vec<Fixture>,
    pub paths: Vec<JourneyPath>,
}
