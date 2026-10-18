use std::collections::{HashMap, HashSet};

use codegraph_config::ux::{Align, Dimension, Display, ToneMap};
use codegraph_config::{DomainConfig, IfmlComponentMappings, SemanticRole};
use codegraph_core::traits::GraphQuerier;
use rex_ifml::ComponentSpec;
use serde::Serialize;

use crate::ProjectConfig;
use crate::ifml::api_paths::{ResolvedApi, id_param_from, resolve_entity_api};
use crate::ifml::context::{IfmlComponent, IfmlViewContainer};

use super::load::form_payload_block;
use super::render::{
    build_submit, button_context, container_context, humanize_event_label, modal_context,
    primary_event_requires, render_chart, render_event, render_form, render_table, sanitize_ident,
};
use super::roles::{
    component_role, is_collection, is_form_component, kind_of, mapped_fields, mapping_context,
    semantic_container_role, semantic_view_role,
};
use super::ux::{
    UxGeneration, apply_table_ux, page_ux_context, resolve_column_ux, timeline_from_plan,
    ux_props_for_entity,
};
use super::workflow::{component_workflow, js_quote, workflow_for_entity};

#[derive(Debug, Clone, Serialize)]
pub struct PageSvelteContext {
    pub api_version: String,
    pub(super) name: String,
    pub(super) label: String,
    pub(super) components: Vec<PageComponentContext>,
    /// Body render groups: the view's own components first, then one group
    /// per nested view container. Each xor group renders its container label
    /// heading before its content (only when the presentation wrapper is
    /// active — no-pack output stays byte-identical).
    pub(super) groups: Vec<RenderGroup>,
    pub(super) params: Vec<crate::ifml::context::ParameterDef>,
    pub(super) view_events: Vec<RenderEvent>,
    pub(super) imports: Vec<RenderImport>,
    pub(super) needs_goto: bool,
    pub(super) needs_on_mount: bool,
    /// Whether any component carries a transition handler (`invalidateAll`
    /// import).
    pub(super) needs_invalidate: bool,
    pub(super) has_submit: bool,
    /// Semantic slot role of the view: `modal-view` for modals, else
    /// `shell` for landmarks.
    pub(super) view_role: Option<SemanticRole>,
    /// Semantic slot role of the container grouping: `presentation-container`
    /// for xor/wizard containers.
    pub(super) container_role: Option<SemanticRole>,
    /// Roles allowed to view this page; empty when unrestricted. v1 keeps a
    /// single enforcement point in `+page.ts` — markup is role-agnostic.
    pub roles: Vec<String>,
    /// Capabilities required to view this page; empty when unrestricted.
    pub requires: Vec<String>,
    /// View-level markup gate for interactive controls; empty (no gate)
    /// for unguarded views — byte-identical output.
    pub(super) control_gate: ControlGateContext,
    /// Modal wrapper for `modal: true` views with a resolved mapping or a
    /// non-empty mapping pack; `None` renders the plain page.
    pub(super) modal: Option<RenderModal>,
    /// Presentation-container wrapper for xor view containers with a
    /// resolved mapping or a non-empty mapping pack; `None` renders the
    /// plain page.
    pub(super) container: Option<RenderContainer>,
    /// View parameter names; non-empty emits the page-level `viewParams`
    /// derived const (query-param resolution — SvelteKit views have no
    /// dynamic segments here, so route params are always empty).
    pub(super) view_params: Vec<String>,
    /// View-level guard lowered from the persisted expression AST
    /// (issue #278). `None` — and therefore no rendered guard — unless the
    /// `expr_ir` feature flag is ON and the condition lowers cleanly.
    pub(super) guard_expr: Option<String>,
    /// ux-rules page-level formatting baseline (issue #300): locale + money
    /// `Intl.NumberFormat` options for the fallback-table script helpers.
    /// `None` when the `ux_rules` plane is off — the key is skipped so
    /// flag-off output stays byte-identical. IFML page templates render
    /// with the component context only (no `project` in scope — see
    /// `render_template`), so the format baseline is threaded explicitly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) ux: Option<PageUxContext>,
}

/// Locale/currency baseline for the fallback-table formatting helpers
/// (issue #300), threaded from `ProjectConfig.ux.format`.
#[derive(Debug, Clone, Serialize)]
pub struct PageUxContext {
    /// BCP-47 locale for the page's `Intl` formatters.
    pub locale: String,
    /// Ready-to-render `Intl.NumberFormat` options object literal for money
    /// cells (`{ style: 'currency', currency: 'NZD' }`), or `{}` when no
    /// currency is configured (money degrades to grouped decimals).
    pub money_options: String,
}

/// Markup gate for a view's interactive controls (form submit/cancel buttons
/// and mapped action-control buttons): unauthorized users see nothing
/// instead of a dead button. Mirrors the `+page.ts` load guard exactly —
/// the requires-check AND the roles-check, each skipped when empty. The
/// load guard stays authoritative (it redirects); this is cosmetic hiding.
/// Whole-component mapped invocations are never wrapped — the mapped
/// component owns its internals.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ControlGateContext {
    /// `$lib/roles` import names the gate needs (`currentRoles`, `can`).
    pub imports: Vec<String>,
    /// Ready-to-render const declarations injected into the script
    /// (`const viewRequires = [...];`, `const viewRoles = [...];`,
    /// `const roles = currentRoles();`).
    pub consts: String,
    /// Ready-to-render opening marker `{#if <expr>}`; empty when inactive.
    pub open: String,
    /// Ready-to-render closing marker `{/if}`; empty when inactive.
    pub close: String,
}

impl ControlGateContext {
    /// The gate for a view: inactive unless the view is guarded AND a
    /// fallback form branch will render (mapped components own their
    /// internals and are never wrapped).
    fn for_view(vc: &IfmlViewContainer, components: &[PageComponentContext]) -> Self {
        let gateable = components.iter().any(|comp| {
            (comp.form.is_some() || comp.component_type == "form") && comp.mapping.is_none()
        });
        if !gateable {
            return Self::default();
        }
        Self::for_guards(&vc.requires, &vc.roles)
    }

    /// The gate for a guard pair: requires-check first, roles-check second,
    /// ANDed — mirroring the load guard's evaluation order and semantics.
    pub(super) fn for_guards(requires: &[String], roles: &[String]) -> Self {
        if requires.is_empty() && roles.is_empty() {
            return Self::default();
        }
        let mut consts: Vec<String> = Vec::new();
        let mut checks: Vec<String> = Vec::new();
        if !requires.is_empty() {
            let list = requires
                .iter()
                .map(|c| js_quote(c))
                .collect::<Vec<_>>()
                .join(", ");
            consts.push(format!("const viewRequires = [{list}];"));
            checks.push("viewRequires.some((c) => can(c))".to_string());
        }
        if !roles.is_empty() {
            let list = roles
                .iter()
                .map(|r| js_quote(r))
                .collect::<Vec<_>>()
                .join(", ");
            consts.push(format!("const viewRoles = [{list}];"));
            consts.push("const roles = currentRoles();".to_string());
            checks.push("roles.some((r) => viewRoles.includes(r))".to_string());
        }
        // Import order mirrors the load template: currentRoles first, can second.
        let mut imports = Vec::new();
        if !roles.is_empty() {
            imports.push("currentRoles".to_string());
        }
        if !requires.is_empty() {
            imports.push("can".to_string());
        }
        Self {
            imports,
            consts: consts.join("\n\t"),
            open: format!("{{#if {}}}", checks.join(" && ")),
            close: "{/if}".to_string(),
        }
    }

    /// This gate AND-composed with an event-level `requires` check:
    /// `{#if viewChecks && ['Cap'].some((c) => can(c))}`. An empty requires
    /// list clones the gate unchanged; an inactive view gate yields the
    /// event check alone.
    fn compose_with_event(&self, requires: &[String]) -> Self {
        if requires.is_empty() {
            return self.clone();
        }
        let list = requires
            .iter()
            .map(|c| js_quote(c))
            .collect::<Vec<_>>()
            .join(", ");
        let check = format!("[{list}].some((c) => can(c))");
        let inner = if self.open.is_empty() {
            check
        } else {
            let view_expr = self
                .open
                .strip_prefix("{#if ")
                .and_then(|s| s.strip_suffix('}'))
                .unwrap_or_default();
            format!("{view_expr} && {check}")
        };
        let mut imports = self.imports.clone();
        if !imports.iter().any(|name| name == "can") {
            imports.push("can".to_string());
        }
        Self {
            imports,
            consts: self.consts.clone(),
            open: format!("{{#if {inner}}}"),
            close: "{/if}".to_string(),
        }
    }
}

/// One body render group: an optional container label heading followed by
/// the group's components. Group 0 is the view's own components (no
/// heading); further groups are the view's nested containers in
/// declaration order.
#[derive(Debug, Clone, Serialize)]
pub struct RenderGroup {
    pub heading: Option<String>,
    pub components: Vec<PageComponentContext>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderImport {
    pub export_name: String,
    pub import_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageComponentContext {
    pub(crate) name: String,
    /// Sanitized JS identifier (const/handler names).
    pub(crate) js_name: String,
    pub(crate) component_type: String,
    /// Semantic slot role of the whole component (`collection`, `display`,
    /// `selection-field`); forms keep `None` — their inputs carry roles.
    pub(crate) role: Option<SemanticRole>,
    pub(crate) entity: String,
    pub(crate) fields: Vec<String>,
    pub(crate) fields_with_types: Vec<(String, String)>,
    pub(crate) filter: String,
    pub(crate) table: Option<RenderTable>,
    pub(crate) form: Option<RenderForm>,
    pub(crate) chart: Option<RenderChart>,
    pub(crate) mapping: Option<RenderMapping>,
    pub(crate) events: Vec<RenderEvent>,
    /// Ready-to-render event callback props for mapped components,
    /// e.g. `onselect={comp_grid_select}` (Svelte 5 event-property form —
    /// `on:select` directives are not forwarded to components).
    pub(crate) event_props: Vec<String>,
    /// Ready-to-render data prop for mapped components, e.g. `data={data.items}`.
    pub(crate) data_prop: String,
    /// Ready-to-render fields prop for mapped components.
    pub(crate) fields_prop: String,
    /// Ready-to-render submit callback prop for mapped form components.
    pub(crate) submit_prop: Option<String>,
    /// Joined validation expressions for mapped form components
    /// (`data-validate` attribute).
    pub(crate) data_validate: Option<String>,
    /// First validation message for mapped form components
    /// (`data-validate-message` attribute).
    pub(crate) data_validate_message: Option<String>,
    pub(crate) api: Option<ResolvedApi>,
    /// View parameter carrying the entity id (edit mode), when any.
    pub(crate) id_param: Option<String>,
    /// Fields passed to a mapped component: declared fields when present,
    /// else derived from the typed spec (table columns / form fields).
    pub(crate) mapped_fields: Vec<String>,
    /// Handler for row-click navigation on list/table fallback markup.
    pub(crate) row_handler: Option<String>,
    pub(crate) submit: Option<RenderSubmit>,
    pub(crate) submit_handler: Option<String>,
    /// Mapped submit button replacing the hardcoded fallback `<button>`.
    pub(crate) submit_button: Option<RenderButton>,
    /// Mapped cancel/back/click buttons rendered after the form.
    pub(crate) buttons: Vec<RenderButton>,
    /// Workflow state display for the bound entity, resolved from the
    /// owning domain's config; `None` renders no badge (byte-identical).
    pub(crate) workflow: Option<RenderWorkflow>,
    /// Load fetch wiring for this component (mirrors the `+page.ts`
    /// contract: first list/details/form wins, details/form need an id
    /// param). Gates mapped-branch badges and transition buttons on the
    /// component's value path actually being fetch-backed.
    pub(crate) fetch_list: bool,
    pub(crate) fetch_item: bool,
    pub(crate) fetch_form: bool,
    /// Fetch URL (JS template literal) for the component's transition
    /// handler: `{base}/${id}/actions/transition`; `None` when the component
    /// carries no transition buttons.
    pub(crate) transition_url_expr: Option<String>,
    /// Markup gate for the submit control: the view gate AND-composed with
    /// the primary save/submit event's `requires` check. Equals the view
    /// gate (byte-identical) when no event-level requires exist.
    pub(crate) submit_gate: ControlGateContext,
    /// Whether the fallback form branch renders: emits the typed
    /// `{js_name}_form_state` const used by value bindings and the
    /// workflow badge.
    pub(crate) form_state: bool,
    /// Ready-to-render typed payload construction lines for the submit
    /// handler (`const payload: ...` + per-field coercions); empty keeps
    /// the untyped `formData` body (byte-identical).
    pub(crate) form_payload: String,
    /// Whether the component belongs to a nested view container: suppresses
    /// the page-level `<h1>` heading inside the group.
    pub(crate) in_container: bool,
    /// ux-resolved columns for a spec-less list component (issue #300):
    /// the same [`RenderColumn`] shape as typed tables, keyed aligned with
    /// `fields`. Empty (and skipped from the context) when the ux plane is
    /// off — the template keeps its `comp.fields` loop, byte-identical.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) ux_list_columns: Vec<RenderColumn>,
    /// Uniform timeline view of the collection layout (issue #301):
    /// `Some` exactly when the active fallback branch renders a timeline —
    /// typed tables project their `RenderTable::layout` timeline here,
    /// spec-less lists resolve theirs directly. Mapped components never
    /// resolve one (whole-component mappings replace the fallback by
    /// design). `None` (skipped) renders the table — flag-off and
    /// rule-less pages stay byte-identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) timeline: Option<RenderTimeline>,
    /// Secondary navigate events (issue #301) for fallback collections
    /// carrying more than one: the first navigate event stays inline (the
    /// row/item click handler), the rest disclose into the per-row actions
    /// menu. Empty (and skipped from the context) when the component has
    /// at most one navigate event, is mapped, or the ux plane is off —
    /// single-event markup stays byte-identical.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) row_menu_events: Vec<RenderRowMenuEvent>,
}

/// Workflow config for a component's bound entity, pre-rendered into the
/// state badge markup for the component's markup context.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RenderWorkflow {
    pub status_field: String,
    pub states: Vec<String>,
    pub terminal_states: Vec<String>,
    pub initial_state: String,
    /// Raw from → [targets] edges from the config; `None` targets resolve
    /// per the empty-map rule (any non-terminal state). Skipped in output.
    #[serde(skip_serializing)]
    pub transition_map: HashMap<String, Vec<String>>,
    /// Whether the config generates workflow action endpoints: gates the
    /// transition buttons and handler (the POST target must exist).
    #[serde(skip_serializing)]
    pub generate_action_endpoints: bool,
    /// Ready-to-render transition buttons for details/form components:
    /// one per valid (from → to) edge, disabled client-side unless the
    /// current state matches the from-state (empty map: unless terminal).
    pub transitions: Vec<RenderTransition>,
    /// Ready-to-render badge: `<span class="workflow-state"
    /// data-testid="{component}-state" data-workflow-state={...}>...</span>`
    /// plus the terminal marker attribute when terminal states are known.
    pub badge_html: String,
    /// Whether the badge reads a per-row `item` binding (collections): the
    /// mapped-component sibling renders inside `{#each data.items as item}`.
    pub each: bool,
}

/// One workflow transition button: humanized target label, e2e testid
/// (`{component}-transition-{target}`), the (from → to) edge, the
/// client-side disabled expression over the component's state value path,
/// and the ready-to-render `<button>` markup.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RenderTransition {
    pub label: String,
    pub testid: String,
    pub from: String,
    pub to: String,
    pub disabled_expr: String,
    pub html: String,
}

/// Submit wiring for a form component: fetch + success navigation.
#[derive(Debug, Clone, Serialize)]
pub struct RenderSubmit {
    pub(super) handler_name: String,
    /// URL expression (JS literal) passed to fetch. Edit views PUT here;
    /// create views POST here directly.
    pub(super) url_expr: String,
    pub(super) method: String,
    /// Create-mode collection URL expression. Views carrying an id param
    /// branch at runtime: `isEdit` PUTs [`Self::url_expr`] (item), otherwise
    /// POSTs this (collection). `None` for views without an id param, which
    /// always POST the collection.
    pub(super) create_url_expr: Option<String>,
    /// Name of the id param gating the edit branch (`!!viewParams.<name>`).
    pub(super) edit_param: Option<String>,
    /// Navigation target URL expression from the view's save event.
    pub(super) navigate_url: Option<String>,
    /// Emit the conservative client-side check that surfaces the first
    /// failing validation message before the network request.
    pub(super) client_validate: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderMapping {
    pub(super) import_name: String,
    pub(super) import_path: String,
    pub(super) testid: Option<String>,
    pub(super) row_testid: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderEvent {
    pub handler_name: String,
    pub event_type: String,
    /// "navigate" | "refresh" | "action" | "stay"
    pub action_kind: String,
    pub target: String,
    /// JS string expression evaluating to the navigation target URL.
    pub url_expr: String,
    /// Capability requirements on the event (`requires: [Cap]`); empty when
    /// unguarded. Gates the event's control behind `can(...)`.
    pub requires: Vec<String>,
    /// Semantic slot role: `action-control` for save/submit/cancel/back/click
    /// button-style events.
    pub role: Option<SemanticRole>,
}

/// Modal wrapper for a `modal: true` view: a mapped `modal-view` component
/// (`<Dialog bind:open={dialog_open}>`) or the built-in div fallback.
#[derive(Debug, Clone, Serialize)]
pub struct RenderModal {
    /// Full opening markup line, e.g.
    /// `<Dialog bind:open={dialog_open} testid="x-modal">` or
    /// `<div class="modal" role="dialog" data-testid="x-modal">`.
    pub open_line: String,
    /// Full closing markup, e.g. `</Dialog>` or `</div>`.
    pub close_line: String,
    /// Testid of the generated close button.
    pub close_testid: String,
    /// Component import when the wrapper is a mapped dialog; `None` for the
    /// built-in div fallback.
    pub import: Option<RenderImport>,
}

/// Presentation-container wrapper for an xor view container: a mapped
/// `presentation-container` component (`<Card testid="...">`) or the
/// built-in section fallback. Children render inline inside the wrapper.
#[derive(Debug, Clone, Serialize)]
pub struct RenderContainer {
    /// Full opening markup line, e.g. `<Card testid="card">` or
    /// `<section data-testid="checkout-container">`.
    pub open_line: String,
    /// Full closing markup, e.g. `</Card>` or `</section>`.
    pub close_line: String,
    /// Testid carried by the wrapper.
    pub testid: String,
    /// Component import when the wrapper is a mapped container; `None` for
    /// the built-in section fallback.
    pub import: Option<RenderImport>,
    /// Ready-to-render container label heading rendered right after the
    /// opening line (`<h2 class="container-label" …>`); `None` when the
    /// wrapper groups nested containers that carry their own headings.
    pub label_heading: Option<String>,
}

/// One navigation entry in the landmark shell layout: `href_attr` is the
/// ready-to-render `href={...}` attribute using the same URL expression the
/// page goto handlers emit.
#[derive(Debug, Clone, Serialize)]
pub struct RenderNavItem {
    pub label: String,
    pub href_attr: String,
}

/// Landmark shell nav context: a mapped `shell` component wrapping nav
/// items built from the landmark views' navigate events.
#[derive(Debug, Clone, Serialize)]
pub struct RenderShellNav {
    pub import: RenderImport,
    pub testid: Option<String>,
    pub items: Vec<RenderNavItem>,
}

/// Render context for the framework layout shell
/// (`ifml/{fw}/layout.tera` → `src/routes/+layout.svelte`).
#[derive(Debug, Clone, Serialize)]
pub struct LayoutSvelteContext {
    pub shell: RenderShellNav,
}

/// A mapped action-control button replacing the hardcoded fallback `<button>`
/// (form submit) or rendering a cancel/back/click action.
#[derive(Debug, Clone, Serialize)]
pub struct RenderButton {
    pub import_name: String,
    pub import_path: String,
    /// Humanized event action: Save / Cancel / Submit / Back.
    pub label: String,
    /// Ready-to-render `onclick={handler}` prop; `None` when no handler fn
    /// is generated.
    pub onclick_prop: Option<String>,
    /// Ready-to-render `disabled={submitting}` prop for submit buttons.
    pub disabled_prop: Option<String>,
    /// Ready-to-render `testid="..."` prop.
    pub testid_prop: String,
    /// Capability requirements of the source event; composed with the view
    /// gate into `gate_open`/`gate_close` by the page post-pass.
    pub event_requires: Vec<String>,
    /// Ready-to-render gate markers around the button: the view gate
    /// AND-composed with the event's `requires` check; empty when no gate
    /// applies.
    pub gate_open: String,
    pub gate_close: String,
}

/// Typed-table render context derived from a `ComponentSpec::Table`
#[derive(Debug, Clone, Serialize)]
pub struct RenderTable {
    pub(super) pagination: bool,
    /// `pagination` when the list spec enables pagination.
    pub(super) role: Option<SemanticRole>,
    pub(super) columns: Vec<RenderColumn>,
    /// Collection layout (issue #301): the default `Table`, or a timeline
    /// resolved ONLY from an explicit matching `[[collection]]`
    /// `display = "timeline"` rule through the shared plan machinery
    /// ([`crate::ux::plan`]). The `Table` variant is skipped from the
    /// serialized context, so flag-off / rule-less rendering never sees
    /// the key and stays byte-identical (a single template gate).
    #[serde(skip_serializing_if = "TableLayout::is_table")]
    pub(super) layout: TableLayout,
}

/// Collection layout for a fallback table/list (issue #301).
///
/// Timeline is strictly opt-in: only an explicit matching `[[collection]]`
/// rule produces it. The plan output is reused verbatim — order_by was
/// already validated against the entity's TimePoint fields by
/// [`crate::ux::plan::build_ux_plan`], so this type never re-validates.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "layout", rename_all = "kebab-case")]
pub enum TableLayout {
    /// The default table rendering (skipped from the serialized context).
    Table,
    /// Chronological rendering replacing the table block: newest-first
    /// `<ol>` items with a `<time>` head, title, preview fields and the
    /// workflow badge per item.
    Timeline {
        /// Data path each item sorts by (the rule's `order_by`).
        order_binding: String,
        /// Title field per item (rule value or the plan's column-order
        /// first field); `None` renders no title line.
        title_binding: Option<String>,
        /// Extra fields shown per item, resolved through the SAME
        /// per-column ux tier as #300 table cells.
        preview: Vec<RenderColumn>,
    },
}

impl TableLayout {
    /// True for the default table layout — the skip-serialization gate.
    pub fn is_table(&self) -> bool {
        matches!(self, TableLayout::Table)
    }
}

/// Template-facing timeline payload for a fallback collection (issue
/// #301). Typed tables project their [`RenderTable::layout`] timeline
/// here; spec-less lists resolve theirs directly — one uniform component
/// field renders both branches.
#[derive(Debug, Clone, Serialize)]
pub struct RenderTimeline {
    /// Data path each item sorts by (newest first).
    pub order_binding: String,
    /// Title field per item; `None` renders no title line.
    pub title_binding: Option<String>,
    /// Preview fields rendered through the same per-column ux resolution
    /// as #300 cells (chips/copy/money/quantity/date helpers).
    pub preview: Vec<RenderColumn>,
}

/// One secondary event disclosed into the per-row actions menu (issue
/// #301, IFML Pass-3 flavor). The menu item calls the SAME named handler
/// function the inline placement would call — parity by construction.
#[derive(Debug, Clone, Serialize)]
pub struct RenderRowMenuEvent {
    /// The shared handler fn name (`comp_grid_archive`).
    pub handler_name: String,
    /// Humanized event-type label (`Archive`).
    pub label: String,
}

/// One typed table column; `binding` is the ready-to-emit data path
/// (`property` for field/lookup columns, the rendered expression for
/// expression columns)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RenderColumn {
    pub(super) label: String,
    pub(super) kind: String,
    pub(super) binding: String,
    pub(super) lookup: String,
    pub(super) expr: String,
    /// ux-rules resolved presentation (issue #300). `None` when the plane
    /// is off — the key is skipped so flag-off contexts and rendered
    /// markup stay byte-identical (the template's ux branches are then
    /// never taken).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) ux: Option<RenderColumnUx>,
}

/// ux-rules resolved presentation for one fallback-table column (issue
/// #300), carried as the shared ux vocabulary (`codegraph_config::ux`)
/// serialized kebab-case for the template's cell branches.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RenderColumnUx {
    /// Resolved dimension, kebab-case (`text`, `quantity`, `money`,
    /// `time-point`, `status-category`, `identifier`, `reference`, `flag`).
    pub dimension: String,
    /// Resolved display, kebab-case (`raw`, `chip`, `copy-chip`, `link`).
    pub display: String,
    /// Cell/header alignment, kebab-case (`left`, `right`).
    pub align: String,
    /// Status keyword → badge-variant map driving `toneFor`; skipped when
    /// empty so templates treat absence as "no tone entries".
    #[serde(skip_serializing_if = "ToneMap::is_empty")]
    pub tone: ToneMap,
}

impl RenderColumnUx {
    /// The fixed presentation for a `lookup` column (tier 0): the DSL
    /// names the rendering explicitly (`via status_labels`), so the column
    /// is a StatusCategory chip with the pack keyword tone map — closing
    /// the gap where lookup columns rendered raw. Deliberately ABOVE the
    /// rule tier: no `[[column]]` rule downgrades an explicit lookup.
    pub(super) fn for_lookup() -> Self {
        Self {
            dimension: Dimension::StatusCategory.as_str().to_string(),
            display: "chip".to_string(),
            align: "left".to_string(),
            tone: ToneMap::default(),
        }
    }

    /// Project a resolved [`ColumnPlan`] onto the render context.
    pub(super) fn from_plan(plan: &crate::ux::plan::ColumnPlan) -> Self {
        Self {
            dimension: plan.dimension.as_str().to_string(),
            display: match plan.display {
                Display::Raw => "raw",
                Display::Chip => "chip",
                Display::CopyChip => "copy-chip",
                Display::Link => "link",
            }
            .to_string(),
            align: match plan.align {
                Align::Left => "left",
                Align::Right => "right",
            }
            .to_string(),
            tone: plan.tone.clone(),
        }
    }
}

/// Typed-form render context derived from a `ComponentSpec::Form`
#[derive(Debug, Clone, Serialize)]
pub struct RenderForm {
    pub(super) fields: Vec<RenderInputField>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderInputField {
    pub(super) name: String,
    pub(super) input_type: String,
    /// Per-input slot role: `selection-field` for dropdown/radio inputs.
    pub(super) input_role: Option<SemanticRole>,
    pub(super) is_textarea: bool,
    pub(super) is_select: bool,
    pub(super) is_radio: bool,
    pub(super) required: bool,
    pub(super) values: Vec<String>,
    /// Validation expressions joined for a `data-validate` attribute.
    pub(super) data_validate: String,
    /// Message positionally paired with the first validation; rendered as
    /// `data-validate-message` next to `data-validate`.
    pub(super) message: Option<String>,
}

/// Typed-chart render context derived from a `ComponentSpec::Chart`
#[derive(Debug, Clone, Serialize)]
pub struct RenderChart {
    pub(super) kind: String,
    pub(super) label_field: Option<String>,
    pub(super) value_fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageLoadContext {
    pub api_version: String,
    pub(super) name: String,
    pub(super) components: Vec<PageLoadComponentContext>,
    pub(super) has_fetch: bool,
    /// Whether any component carries a workflow: the load result types its
    /// values `any` instead of `unknown` so workflow value paths in the
    /// page markup typecheck. Absent for byte-identical non-workflow loads.
    pub(super) has_workflow: bool,
    /// All view parameters, resolved from `url.searchParams` (views have no
    /// dynamic route segments); entries with a DSL default carry the JS
    /// literal as the final `??` fallback. Resolved params are returned to
    /// the page as `result.params`.
    pub(super) view_params: Vec<RenderViewParam>,
    /// Roles allowed to view this page; non-empty emits the load-level
    /// role guard plus the `$lib/roles` helper import.
    pub(super) view_roles: Vec<String>,
    /// Capabilities required to view this page; non-empty emits the
    /// load-level `can()` guard.
    pub(super) view_requires: Vec<String>,
    /// Ready-to-render guard const declarations (`const viewRoles = ...;`),
    /// emitted between the imports and the load function.
    pub(super) guard_consts: String,
    /// Integrated route-tree mode (issue #468 W1b/W-a): the view lives in
    /// the ui app's `(views)` group and the `(views)/+layout.server.ts`
    /// load resolves the session's role tokens — so the guard decides
    /// SERVER-side via `parent()` roles (a denied persona is redirected
    /// during SSR, before any data fetch). Standalone keeps the
    /// browser-gated guard (no layout provides server roles).
    pub(super) integrated: bool,
    /// Redirect target when a guard fails: the first unguarded view's
    /// route, else `/`.
    pub(super) denial_target: String,
}

/// A view parameter resolved in the load function from the query string.
#[derive(Debug, Clone, Serialize)]
pub struct RenderViewParam {
    pub(super) name: String,
    /// JS literal emitted as the final `??` fallback.
    pub(super) default: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageLoadComponentContext {
    /// Sanitized JS identifier for local variable names.
    pub(super) name: String,
    pub(super) component_type: String,
    pub(super) entity: String,
    /// Legacy lowercase entity route (used by frameworks without a resolved
    /// API model).
    pub(super) route_name: String,
    pub(super) api: Option<ResolvedApi>,
    pub(super) id_param: Option<String>,
    /// Whether the bound entity carries a workflow: the load merges the
    /// `/workflow` state into the payload so badges can read the
    /// authoritative current state.
    pub(super) workflow: bool,
    pub(super) paginate: bool,
    pub(super) fetch_list: bool,
    pub(super) fetch_item: bool,
    pub(super) fetch_form: bool,
}

/// The lowered TypeScript guard for a view's `condition:`, or `None`.
///
/// Gated on the `expr_ir` feature flag: OFF (default) always yields `None`
/// so generated output stays byte-identical. ON, the guard is lowered from
/// the persisted expression AST (`expr_json`); a condition outside the
/// closed subset is a hard error naming the view (rexlang philosophy —
/// no silent passthrough).
pub(crate) fn page_guard_expr(
    vc: &IfmlViewContainer,
    project: &ProjectConfig,
) -> crate::error::Result<Option<String>> {
    if !project.codegen.expr_ir {
        return Ok(None);
    }
    let Some(expr_json) = vc.conditional_expr_json.as_deref() else {
        return Ok(None);
    };
    crate::ifml::expr_ts::lower_ifml_json(expr_json, &vc.name)
        .map(Some)
        .map_err(|e| crate::error::Error::Config(e.to_string()))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn build_page_context(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    api_version: &str,
    vc: &IfmlViewContainer,
    mappings: Option<&IfmlComponentMappings>,
    modal_targets: &HashSet<String>,
    ux: Option<&UxGeneration<'_>>,
    project: &ProjectConfig,
) -> crate::error::Result<PageSvelteContext> {
    let id_param = id_param_from(&vc.params);
    let mut api_cache: HashMap<String, Option<ResolvedApi>> = HashMap::new();
    let container = container_context(vc, mappings);

    let mut components = Vec::new();
    for c in &vc.components {
        let ctx = page_component_context(
            db,
            config,
            api_version,
            vc,
            c,
            id_param.as_deref(),
            mappings,
            &mut api_cache,
            modal_targets,
            ux,
        )
        .await;
        components.push(ctx);
    }

    // Body render groups: the view's own components, then one group per
    // nested container in declaration order. Headings (and the in-container
    // h1 suppression) are gated on the presentation wrapper being active —
    // without a mapping pack the page stays plain (byte-identical with
    // pre-nesting output).
    let mut groups = vec![RenderGroup {
        heading: None,
        components,
    }];
    if container.is_some() {
        for group_decl in &vc.containers {
            let heading = group_decl.is_xor.then(|| {
                let lower = group_decl.name.to_lowercase();
                let label = group_decl
                    .label
                    .clone()
                    .unwrap_or_else(|| group_decl.name.clone());
                format!("<h2 class=\"container-label\" data-testid=\"{lower}-label\">{label}</h2>")
            });
            let mut group_components = Vec::new();
            for c in &group_decl.components {
                let mut ctx = page_component_context(
                    db,
                    config,
                    api_version,
                    vc,
                    c,
                    id_param.as_deref(),
                    mappings,
                    &mut api_cache,
                    modal_targets,
                    ux,
                )
                .await;
                ctx.in_container = true;
                group_components.push(ctx);
            }
            groups.push(RenderGroup {
                heading,
                components: group_components,
            });
        }
    }
    let components: Vec<PageComponentContext> = groups
        .iter()
        .flat_map(|group| group.components.iter().cloned())
        .collect();

    let view_events: Vec<RenderEvent> = vc
        .events
        .iter()
        .map(|e| render_event(e, modal_targets))
        .collect();

    let mut imports: Vec<RenderImport> = Vec::new();
    let mut seen_imports: HashMap<String, String> = HashMap::new();
    let modal = modal_context(vc, mappings);
    if let Some(imp) = modal.as_ref().and_then(|m| m.import.as_ref()) {
        seen_imports.insert(imp.import_path.clone(), imp.export_name.clone());
        imports.push(RenderImport {
            export_name: imp.export_name.clone(),
            import_path: imp.import_path.clone(),
        });
    }
    if let Some(imp) = container.as_ref().and_then(|c| c.import.as_ref()) {
        let export = imp.export_name.clone();
        if seen_imports
            .insert(imp.import_path.clone(), export.clone())
            .is_none()
        {
            imports.push(RenderImport {
                export_name: export,
                import_path: imp.import_path.clone(),
            });
        }
    }
    let mut needs_goto = view_events.iter().any(|e| e.action_kind == "navigate");
    let mut has_submit = false;
    for comp in &components {
        for evt in &comp.events {
            if evt.action_kind == "navigate" {
                needs_goto = true;
            }
        }
        if let Some(ref submit) = comp.submit {
            has_submit = true;
            if submit.navigate_url.is_some() {
                needs_goto = true;
            }
        }
        if let Some(ref mapping) = comp.mapping {
            let export = mapping.import_name.clone();
            if seen_imports
                .insert(mapping.import_path.clone(), export.clone())
                .is_none()
            {
                imports.push(RenderImport {
                    export_name: export,
                    import_path: mapping.import_path.clone(),
                });
            }
        }
        for btn in comp.submit_button.iter().chain(comp.buttons.iter()) {
            if seen_imports
                .insert(btn.import_path.clone(), btn.import_name.clone())
                .is_none()
            {
                imports.push(RenderImport {
                    export_name: btn.import_name.clone(),
                    import_path: btn.import_path.clone(),
                });
            }
        }
    }
    let needs_on_mount = view_events
        .iter()
        .any(|e| e.action_kind == "navigate" && e.event_type == "load");
    let mut control_gate = ControlGateContext::for_view(vc, &components);

    // Load fetch wiring (same rules the load context applies) plus
    // event-level capability gates. Fetch flags gate the mapped-branch
    // badge and transition buttons on the component's value path actually
    // being fetch-backed; the transition handler URL resolves from the
    // component's API path and the view's id param. Event gates compose
    // the view gate with each component's save/submit requires (submit
    // control) and each secondary button's event requires; buttons without
    // requires reuse the view gate verbatim (byte-identical).
    let id_param = id_param_from(&vc.params);
    let mut fetch_state = FetchState::default();
    let mut needs_can_import = false;
    for group in &mut groups {
        for comp in &mut group.components {
            let is_list = comp.table.is_some() || comp.component_type == "list";
            let is_details = comp.component_type == "details";
            let is_form = comp.form.is_some() || comp.component_type == "form";
            let has_api = comp.api.is_some() && !comp.entity.is_empty();
            let (fetch_list, fetch_item, fetch_form) =
                fetch_state.next(is_list, is_details, is_form, has_api, id_param.as_deref());
            comp.fetch_list = fetch_list;
            comp.fetch_item = fetch_item;
            comp.fetch_form = fetch_form;

            if let Some(workflow) = &comp.workflow {
                let fetch_backed = match comp.component_type.as_str() {
                    "form" => fetch_form,
                    "details" => fetch_item,
                    _ => fetch_list,
                };
                if fetch_backed
                    && workflow.generate_action_endpoints
                    && !workflow.transitions.is_empty()
                    && let Some(api) = &comp.api
                    && let Some(param) = &id_param
                {
                    comp.transition_url_expr = Some(format!(
                        "`{}/${{viewParams.{param}}}/actions/transition`",
                        api.base_path
                    ));
                }
            }

            let primary = primary_event_requires(&comp.events);
            comp.submit_gate = control_gate.compose_with_event(&primary);
            if !primary.is_empty() {
                needs_can_import = true;
            }
            for btn in &mut comp.buttons {
                if btn.event_requires.is_empty() {
                    btn.gate_open = control_gate.open.clone();
                    btn.gate_close = control_gate.close.clone();
                } else {
                    needs_can_import = true;
                    let gate = control_gate.compose_with_event(&btn.event_requires);
                    btn.gate_open = gate.open;
                    btn.gate_close = gate.close;
                }
            }
        }
    }
    if needs_can_import && !control_gate.imports.iter().any(|name| name == "can") {
        control_gate.imports.push("can".to_string());
    }
    let components: Vec<PageComponentContext> = groups
        .iter()
        .flat_map(|group| group.components.iter().cloned())
        .collect();
    let needs_invalidate = components
        .iter()
        .any(|comp| comp.transition_url_expr.is_some());

    Ok(PageSvelteContext {
        api_version: api_version.to_string(),
        name: vc.name.clone(),
        label: vc.label.clone().unwrap_or_else(|| vc.name.clone()),
        components,
        params: vc.params.clone(),
        view_params: {
            let mut seen: HashSet<String> = HashSet::new();
            vc.params
                .iter()
                .filter(|p| seen.insert(p.name.clone()))
                .map(|p| p.name.clone())
                .collect()
        },
        view_events,
        imports,
        needs_goto,
        needs_on_mount,
        needs_invalidate,
        has_submit,
        groups,
        view_role: semantic_view_role(vc),
        container_role: semantic_container_role(vc),
        roles: vc.roles.clone(),
        requires: vc.requires.clone(),
        control_gate,
        modal,
        container,
        ux: ux.map(|uxg| page_ux_context(uxg.rules)),
        guard_expr: page_guard_expr(vc, project)?,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn page_component_context(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    api_version: &str,
    vc: &IfmlViewContainer,
    c: &IfmlComponent,
    id_param: Option<&str>,
    mappings: Option<&IfmlComponentMappings>,
    api_cache: &mut HashMap<String, Option<ResolvedApi>>,
    modal_targets: &HashSet<String>,
    ux: Option<&UxGeneration<'_>>,
) -> PageComponentContext {
    let (mut table, form, chart) = match c.spec {
        Some(ComponentSpec::Table(ref spec)) => (Some(render_table(spec)), None, None),
        Some(ComponentSpec::Form(ref spec)) => (None, Some(render_form(spec)), None),
        Some(ComponentSpec::Chart(ref spec)) => (None, None, Some(render_chart(spec))),
        None => (None, None, None),
    };

    // ux-rules fallback-table resolution (issue #300): only when the plane
    // is active — otherwise the ux context keys stay absent and the
    // rendered page is byte-identical. Field columns resolve through the
    // bound property's graph metadata (mirroring
    // `ifml_control_inference.rs`), then the shared Pass-1 + rule tier in
    // `crate::ux::plan`; lookup columns pin StatusCategory + chip above
    // both tiers; expression columns stay Text (the IFML AST carries no
    // return type).
    //
    // Issue #301 adds the collection layout: the plan for the bound entity
    // (resolved ONCE per generation, see `resolve_generation_ux`)
    // contributes a Timeline layout ONLY from an explicit matching
    // `[[collection]] display = "timeline"` rule. Plan output is reused
    // verbatim — never re-validated here.
    let (ux_list_columns, timeline) = match ux {
        Some(uxg) => {
            let rules = uxg.rules;
            let props = ux_props_for_entity(db, c.entity.as_deref().unwrap_or_default()).await;
            let status_field = c
                .entity
                .as_deref()
                .and_then(|entity| workflow_for_entity(config, entity))
                .map(|wf| wf.status_field);
            let status_field = status_field.as_deref();
            if let Some(table) = table.as_mut() {
                apply_table_ux(table, rules, &props, &c.fields_with_types, status_field);
            }
            let ux_list_columns = if c.spec.is_none() && c.component_type == "list" {
                c.fields
                    .iter()
                    .map(|field| RenderColumn {
                        label: field.clone(),
                        kind: "field".to_string(),
                        binding: field.clone(),
                        lookup: String::new(),
                        expr: String::new(),
                        ux: Some(resolve_column_ux(
                            rules,
                            "field",
                            field,
                            &props,
                            &c.fields_with_types,
                            status_field,
                        )),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let timeline = timeline_from_plan(
                uxg,
                c.entity.as_deref().unwrap_or_default(),
                rules,
                &props,
                &c.fields_with_types,
                status_field,
            );
            if let Some(payload) = &timeline
                && let Some(table) = table.as_mut()
            {
                table.layout = TableLayout::Timeline {
                    order_binding: payload.order_binding.clone(),
                    title_binding: payload.title_binding.clone(),
                    preview: payload.preview.clone(),
                };
            }
            (ux_list_columns, timeline)
        }
        None => (Vec::new(), None),
    };

    let kind = kind_of(c);
    let slot_role = component_role(c);
    let mapping = mappings
        .and_then(|m| m.resolve_slot(&vc.name, &c.name, &c.component_type, &kind, slot_role))
        .map(mapping_context);
    // Mapped components replace the fallback wholesale (issue #301): no
    // timeline layout, no row-actions menu — the mapped component owns its
    // presentation.
    let mut timeline = timeline;
    if mapping.is_some() {
        if let Some(table) = table.as_mut() {
            table.layout = TableLayout::Table;
        }
        timeline = None;
    }
    let events: Vec<RenderEvent> = c
        .events
        .iter()
        .map(|e| render_event(e, modal_targets))
        .collect();
    // IFML Pass-3 event tiering (issue #301): the first navigate event
    // stays inline (the row/item click handler, exactly as before);
    // additional secondary events disclose into the per-row actions menu.
    // Handler functions are emitted once per event regardless, so every
    // menu item calls the identical body its inline placement would.
    let row_menu_events = if ux.is_some() && mapping.is_none() && is_collection(c) {
        events
            .iter()
            .filter(|e| e.action_kind == "navigate")
            .skip(1)
            .map(|e| RenderRowMenuEvent {
                handler_name: e.handler_name.clone(),
                label: humanize_event_label(&e.event_type),
            })
            .collect()
    } else {
        Vec::new()
    };
    let event_props: Vec<String> = events
        .iter()
        .filter(|e| {
            e.action_kind == "navigate" && e.event_type != "submit" && e.event_type != "save"
        })
        .map(|e| format!("on{}={{{}}}", e.event_type, e.handler_name))
        .collect();

    let entity = c.entity.clone().unwrap_or_default();
    let api = match api_cache.get(&entity) {
        Some(resolved) => resolved.clone(),
        None => {
            let resolved = if entity.is_empty() {
                None
            } else {
                resolve_entity_api(db, config, &entity, api_version).await
            };
            api_cache.insert(entity.clone(), resolved.clone());
            resolved
        }
    };

    let mapped_fields = mapped_fields(c, table.as_ref(), form.as_ref(), chart.as_ref());

    let row_handler = if is_collection(c) {
        events
            .iter()
            .find(|e| e.action_kind == "navigate")
            .map(|e| e.handler_name.clone())
    } else {
        None
    };

    let submit = build_submit(c, api.as_ref(), id_param, &events);
    let submit_handler = submit.as_ref().map(|s| s.handler_name.clone());
    let submit_prop = submit_handler
        .as_ref()
        .map(|handler| format!("onsubmit={{{handler}}}"));
    let button_mapping = if mapping.is_none() {
        mappings.and_then(|m| {
            m.resolve_slot(
                &vc.name,
                &c.name,
                &c.component_type,
                &kind,
                Some(SemanticRole::ActionControl),
            )
        })
    } else {
        None
    };
    let (submit_button, buttons) = button_context(c, button_mapping, submit.as_ref(), &events);
    // Validation expressions carry model-authored quotes (`code != ""`);
    // emitted into a double-quoted HTML attribute they must be escaped or
    // the attribute terminates mid-expression (issue #468 W1b: the
    // ruleregistryeditor page did exactly that).
    let escape_attr = |s: String| s.replace('"', "&quot;");
    let data_validate = form.as_ref().map(|form| {
        escape_attr(
            form.fields
                .iter()
                .filter(|f| !f.data_validate.is_empty())
                .map(|f| f.data_validate.clone())
                .collect::<Vec<_>>()
                .join(" && "),
        )
    });
    let data_validate_message = form.as_ref().and_then(|form| {
        form.fields
            .iter()
            .find(|f| !f.data_validate.is_empty())
            .and_then(|f| f.message.clone())
            .map(escape_attr)
    });
    let data_prop = if is_collection(c) || chart.is_some() {
        "data={data.items}".to_string()
    } else if is_form_component(c) {
        "item={data.formData}".to_string()
    } else {
        "item={data.item}".to_string()
    };
    let fields_prop = format!(
        "fields={{[{}]}}",
        mapped_fields
            .iter()
            .map(|f| format!("'{f}'"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let workflow = component_workflow(config, c);
    let form_state = is_form_component(c) && mapping.is_none();
    let form_payload = if form_state {
        form_payload_block(c, form.as_ref())
    } else {
        String::new()
    };

    PageComponentContext {
        name: c.name.clone(),
        js_name: sanitize_ident(&c.name),
        component_type: c.component_type.clone(),
        role: slot_role,
        entity,
        fields: c.fields.clone(),
        fields_with_types: c.fields_with_types.clone(),
        filter: c.filter.clone().unwrap_or_default(),
        mapped_fields,
        mapping,
        events,
        event_props,
        data_prop,
        fields_prop,
        submit_prop,
        data_validate,
        data_validate_message,
        api,
        id_param: id_param.map(str::to_string),
        row_handler,
        submit,
        submit_handler,
        submit_button,
        buttons,
        workflow,
        fetch_list: false,
        fetch_item: false,
        fetch_form: false,
        transition_url_expr: None,
        submit_gate: ControlGateContext::default(),
        form_state,
        form_payload,
        in_container: false,
        table,
        form,
        chart,
        ux_list_columns,
        timeline,
        row_menu_events,
    }
}

/// Running view-level fetch state: which component kinds have already
/// claimed the load's single fetch per kind.
#[derive(Default)]
pub(super) struct FetchState {
    has_list: bool,
    has_details: bool,
    has_form: bool,
}

impl FetchState {
    /// The load fetch flags for one component: the first list/details/form
    /// contributes a fetch; details and form fetches additionally require
    /// an id param.
    pub(super) fn next(
        &mut self,
        is_list: bool,
        is_details: bool,
        is_form: bool,
        has_api: bool,
        id_param: Option<&str>,
    ) -> (bool, bool, bool) {
        let mut fetch_list = false;
        let mut fetch_item = false;
        let mut fetch_form = false;
        if has_api {
            if is_list && !self.has_list {
                fetch_list = true;
            } else if is_details && !self.has_details && id_param.is_some() {
                fetch_item = true;
            } else if is_form && !self.has_form && id_param.is_some() {
                fetch_form = true;
            }
        }
        self.has_list |= fetch_list;
        self.has_details |= fetch_item;
        self.has_form |= fetch_form;
        (fetch_list, fetch_item, fetch_form)
    }
}
