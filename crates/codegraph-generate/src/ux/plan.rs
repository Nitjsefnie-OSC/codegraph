//! UX plan builder (ux-rules epic, phase 2 — issue #296).
//!
//! [`build_ux_plan`] turns per-entity UI inputs ([`UxPlanInput`]) plus a
//! merged [`UxRules`] document into a [`UxPlan`]: the per-column
//! presentation contract, the column order, the collection shape (table or
//! timeline), the row-action partition, and row visuals. The plan is pure
//! data — the UI/IFML generators (#297+) consume it; nothing here renders.
//!
//! Passes:
//!
//! - **Pass 1 (inference)** is [`crate::ux::dimension::infer_dimension`].
//! - **Pass 2 (rules + ordering)**: per field, the first matching
//!   `[[column]]` rule pins display/align/tone/sortable and may override
//!   the inferred dimension (which also cancels any money hint for the
//!   column); no match falls back to the pack default for the inferred
//!   dimension. `column_order` puts the first human-readable field first
//!   and audit stamps last unless the author pinned the list order.
//! - **Pass 3 (actions)**: `Open`/`Edit`/`Delete` partition into
//!   inline/overflow at `[actions] inline_max` (default 1: `Open` inline).
//! - **Pass 4 (visuals)**: zebra rows, inactive shading when soft-delete
//!   or workflow terminal states exist, vertical centering.
//!
//! Timeline is STRICTLY opt-in: only an explicit matching `[[collection]]`
//! `display = "timeline"` rule produces [`CollectionPlan::Timeline`]; the
//! heuristic never switches a table into a timeline (a diagnostic suggests
//! it instead — see [`crate::ux::diagnostics`]).
//!
//! The `ids` module is the single source of the `data-testid` constants
//! shared with the e2e generator (consumers: #302/#304).

use std::collections::BTreeMap;

use codegraph_config::ux::{
    ColumnRule, Dimension, Display, FormatConfig, ToneMap, UxRules, glob_match,
};
use codegraph_core::types::PropertyNode;

use crate::error::{Error, Result};
use crate::ui::page::UiField;

use super::dimension::{DimensionHints, infer_dimension_with_hints};

/// Stable `data-testid` constants for generated UI, shared by the UI
/// generators, the e2e/ux spec emitters (#302/#304), and the POM
/// generator (#316/#317) so generated specs never drift from generated
/// markup.
///
/// # The contract
///
/// Tera templates keep their literals — Tera cannot call Rust. The
/// relationship is pinned in the other direction: the GENERATED SPECS
/// locate markup through these literals (and the byte-identity suites pin
/// the specs), while the POM builds its locators from `ids::`. So a
/// template edit that drifts a testid breaks the generated specs, and the
/// POM — reading the same table — stays in sync by construction.
///
/// | id | Owning template / literal source | Consumer |
/// |----|----------------------------------|----------|
/// | `{module}-search` | `ui/list_page.tera:334` | search specs, POM |
/// | `{module}-create-btn` | `ui/list_page.tera:339` | crud spec, POM |
/// | `{module}-empty` | `ui/list_page.tera:348` | crud spec (table-or-empty), POM |
/// | `{module}-no-results` | `ui/list_page.tera:367` | search spec, POM |
/// | `{module}-table` | `ui/list_page.tera:372` | crud spec `:190`, ux spec, POM |
/// | `{module}-sort-{field}` | `ui/list_page.tera:379` | ux spec sort blocks |
/// | `{module}-pagination` | `ui/list_page.tera:447` | ux spec hydration waits, POM |
/// | `{module}-delete-dialog` | `ui/list_page.tera`, `ui/child_section_component.tera` | ux spec, POM |
/// | `{module}-delete-dialog-confirm` | `ui/list_page.tera`, `ui/child_section_component.tera` | ux spec delete flow, POM |
/// | `{module}-form` | `ui/entity_form.tera:140` | validation/crud specs, POM |
/// | `{module}-cancel-btn` | `ui/entity_form.tera:255` | POM cancel flow |
/// | `{module}-submit-btn` | `ui/entity_form.tera:258` | crud spec `:204`, POM submit |
/// | `{module}-edit-btn` | `ui/detail_page.tera:245` | crud spec edit nav, POM |
/// | `{module}-delete-btn` | `ui/detail_page.tera:253` | crud spec delete flow, POM |
/// | `{module}-field-{field}` | `ui/detail_page.tera:281` | crud spec `:281`, POM detail read |
/// | `confirm-dialog` | `ui/detail_page.tera:368` | crud spec `:507`, POM |
/// | `confirm-dialog-confirm` | `ui/detail_page.tera:380` | crud spec `:508`, POM |
/// | `workflow-panel` | `ui/scaffold/workflow_panel.tera:31` | workflow spec `:136-158`, POM |
/// | `workflow-state` | `ui/scaffold/workflow_panel.tera:36` | workflow spec `:138/:158`, POM |
/// | `{module}-row` | (no entity markup testid yet — entity specs locate rows via the `{module}-table` root; kept for family uniformity with `{comp}-row`) | POM row targeting, future |
/// | `{prefix}-chip` / `{prefix}-copy` / `{prefix}-actions` / `{prefix}-actions-menu` | `ui/_ux_cell.tera:20/:28/:82/:90` (prefix = module) | ux spec chip/copy/action blocks, POM |
/// | `{prefix}-timeline` / `{prefix}-timeline-item` | `ui/list_timeline.tera:4/:6` (prefix = module) | ux spec timeline block, POM |
///
/// IFML family (prefix = the IFML component name; fallback markup in
/// `templates/ifml/svelte/page.tera`, mapped components carry the
/// `ifml-components.toml` testids resolved by
/// [`crate::ifml::selectors::ComponentSelectors`]):
///
/// | id | Owning source | Consumer |
/// |----|---------------|----------|
/// | `{comp}-table` | `ifml/svelte/page.tera:326/:415` + `selectors()` fallback | ifml e2e specs, POM |
/// | `{comp}-row` | `ifml/svelte/page.tera:336/:425` + `selectors()` fallback | ifml click-through specs, POM |
/// | `{comp}-form` | `ifml/svelte/page.tera:359/:450` + `selectors()` fallback | ifml validation/CRUD specs, POM |
/// | `{comp}-submit` | `ifml/svelte/page.tera:395/:460` + `selectors()` fallback | ifml CRUD specs, POM |
/// | `{comp}-error` | `ifml/svelte/page.tera:393/:458` | ifml validation specs, POM |
/// | `{comp}-details` | `ifml/svelte/page.tera:474` + `selectors()` fallback | ifml specs, POM |
/// | `{comp}-state` | `route_generator::workflow_badge_html` | ifml workflow specs, POM |
/// | `{comp}-transition-{to}` | `route_generator.rs:724` (kebab via `codegraph_naming`) | ifml workflow specs, POM |
/// | `{comp}-chip` / `{comp}-copy` / `{comp}-actions` / `{comp}-actions-menu` | `ifml/svelte/page.tera` ux cell branches | ifml ux specs, POM |
/// | `{comp}-timeline` / `{comp}-timeline-item` | `ifml/svelte/page.tera:295/:297` | ifml timeline specs, POM |
///
/// Template line numbers track the `ux-rules` branch state; they are
/// orientation, not a pin — the literals themselves are pinned by the
/// generated specs.
pub mod ids {
    /// Row-actions cell / wrapper.
    pub const ACTIONS: &str = "actions";
    /// The overflow menu behind actions beyond `inline_max`.
    pub const ACTIONS_MENU: &str = "actions-menu";
    /// Chip-rendered column values.
    pub const CHIP: &str = "chip";
    /// Copy-to-clipboard affordance on copy-chip columns.
    pub const COPY: &str = "copy";
    /// Timeline collection root.
    pub const TIMELINE: &str = "timeline";
    /// One entry on a timeline collection.
    pub const TIMELINE_ITEM: &str = "timeline-item";

    // ── Entity family (prefix = the entity's snake_case module name) ────

    /// `{module}-search` — the list page's FTS search input.
    pub fn entity_search(module: &str) -> String {
        format!("{module}-search")
    }

    /// `{module}-create-btn` — the list page's create button.
    pub fn entity_create_btn(module: &str) -> String {
        format!("{module}-create-btn")
    }

    /// `{module}-empty` — the empty-state placeholder (no data, no query).
    pub fn entity_empty(module: &str) -> String {
        format!("{module}-empty")
    }

    /// `{module}-no-results` — the empty-state placeholder (active query).
    pub fn entity_no_results(module: &str) -> String {
        format!("{module}-no-results")
    }

    /// `{module}-table` — the list page table root.
    pub fn entity_table(module: &str) -> String {
        format!("{module}-table")
    }

    /// `{module}-row` — a body row.
    ///
    /// Entity markup does not stamp this testid yet: entity specs locate
    /// rows via the [`entity_table`] root (`tbody tr`). Kept for family
    /// uniformity with the IFML `{comp}-row` fallback; do not assert
    /// against it until markup stamps it.
    pub fn entity_row(module: &str) -> String {
        format!("{module}-row")
    }

    /// `{module}-sort-{field}` — a column header's sort button.
    pub fn entity_sort(module: &str, field: &str) -> String {
        format!("{module}-sort-{field}")
    }

    /// `{module}-pagination` — the results count / pager strip.
    pub fn entity_pagination(module: &str) -> String {
        format!("{module}-pagination")
    }

    /// `{module}-delete-confirm` — the list page's row-delete dialog.
    ///
    /// Renamed `{module}-delete-dialog` (issue #468 finding 3): the only
    /// composition rule for a dialog's confirm button is `{dialog}-confirm`
    /// (the unprefixed `confirm-dialog` → `confirm-dialog-confirm` pair);
    /// `…-delete-confirm` + `…-delete-confirm-confirm` read as a double
    /// confirm leaked from template concatenation.
    pub fn entity_delete_dialog(module: &str) -> String {
        format!("{module}-delete-dialog")
    }

    /// `{module}-delete-dialog-confirm` — its confirm button.
    #[deprecated(
        since = "0.1.0",
        note = "use entity_delete_dialog + the {dialog}-confirm rule"
    )]
    pub fn entity_delete_confirm(module: &str) -> String {
        format!("{module}-delete-confirm")
    }

    /// `{module}-delete-confirm-confirm` — its confirm button.
    #[deprecated(
        since = "0.1.0",
        note = "use entity_delete_dialog_confirm (issue #468)"
    )]
    pub fn entity_delete_confirm_confirm(module: &str) -> String {
        format!("{module}-delete-confirm-confirm")
    }

    /// `{module}-delete-dialog-confirm` — the confirm button inside
    /// [`entity_delete_dialog`], by the one dialog-button rule.
    pub fn entity_delete_dialog_confirm(module: &str) -> String {
        format!("{module}-delete-dialog-confirm")
    }

    /// `{module}-form` — the create/edit form root.
    pub fn entity_form(module: &str) -> String {
        format!("{module}-form")
    }

    /// `{module}-cancel-btn` — the form's cancel button.
    pub fn entity_cancel_btn(module: &str) -> String {
        format!("{module}-cancel-btn")
    }

    /// `{module}-submit-btn` — the form's submit button.
    pub fn entity_submit_btn(module: &str) -> String {
        format!("{module}-submit-btn")
    }

    /// `{module}-edit-btn` — the detail page's edit button.
    pub fn entity_edit_btn(module: &str) -> String {
        format!("{module}-edit-btn")
    }

    /// `{module}-delete-btn` — the detail page's delete button (opens
    /// [`confirm_dialog`]).
    pub fn entity_delete_btn(module: &str) -> String {
        format!("{module}-delete-btn")
    }

    /// `{module}-field-{field}` — a detail page field value.
    pub fn entity_field(module: &str, field: &str) -> String {
        format!("{module}-field-{field}")
    }

    // ── Detail-page delete dialog (unprefixed, shared per page) ─────────

    /// `confirm-dialog` — the detail page's delete dialog.
    pub fn confirm_dialog() -> String {
        "confirm-dialog".to_string()
    }

    /// `confirm-dialog-confirm` — its confirm button.
    pub fn confirm_dialog_confirm() -> String {
        "confirm-dialog-confirm".to_string()
    }

    // ── Workflow panel (unprefixed, shared across entities) ─────────────

    /// `workflow-panel` — the detail page's workflow panel.
    pub fn workflow_panel() -> String {
        "workflow-panel".to_string()
    }

    /// `workflow-state` — the panel's current-state badge.
    pub fn workflow_state() -> String {
        "workflow-state".to_string()
    }

    // ── IFML family (prefix = the IFML component name) ──────────────────

    /// `{comp}-table` — fallback collection markup root.
    pub fn ifml_table(comp: &str) -> String {
        format!("{comp}-table")
    }

    /// `{comp}-row` — a fallback collection body row.
    pub fn ifml_row(comp: &str) -> String {
        format!("{comp}-row")
    }

    /// `{comp}-form` — fallback form markup root.
    pub fn ifml_form(comp: &str) -> String {
        format!("{comp}-form")
    }

    /// `{comp}-submit` — the fallback form submit button.
    pub fn ifml_submit(comp: &str) -> String {
        format!("{comp}-submit")
    }

    /// `{comp}-error` — the fallback form validation error line.
    pub fn ifml_error(comp: &str) -> String {
        format!("{comp}-error")
    }

    /// `{comp}-details` — fallback details markup root.
    pub fn ifml_details(comp: &str) -> String {
        format!("{comp}-details")
    }

    /// `{comp}-state` — the workflow state badge.
    pub fn ifml_state(comp: &str) -> String {
        format!("{comp}-state")
    }

    /// `{comp}-transition-{to}` — a workflow transition button; `to_kebab`
    /// is the target state in kebab-case (`codegraph_naming::to_kebab_case`,
    /// as [`crate::ifml::route_generator`] renders it).
    pub fn ifml_transition(comp: &str, to_kebab: &str) -> String {
        format!("{comp}-transition-{to_kebab}")
    }

    // ── Shared fragments (prefix = entity module name OR IFML comp name) ─

    /// `{prefix}-chip` — a chip-rendered cell value.
    pub fn chip(prefix: &str) -> String {
        format!("{prefix}-{CHIP}")
    }

    /// `{prefix}-copy` — a copy-chip affordance.
    pub fn copy(prefix: &str) -> String {
        format!("{prefix}-{COPY}")
    }

    /// `{prefix}-actions` — the row-actions cell.
    pub fn actions(prefix: &str) -> String {
        format!("{prefix}-{ACTIONS}")
    }

    /// `{prefix}-actions-menu` — the row-actions overflow menu.
    pub fn actions_menu(prefix: &str) -> String {
        format!("{prefix}-{ACTIONS_MENU}")
    }

    /// `{prefix}-timeline` — a timeline collection root.
    pub fn timeline(prefix: &str) -> String {
        format!("{prefix}-{TIMELINE}")
    }

    /// `{prefix}-timeline-item` — one timeline entry.
    pub fn timeline_item(prefix: &str) -> String {
        format!("{prefix}-{TIMELINE_ITEM}")
    }

    /// `{prefix}-timeline-meta` — one entry's meta line.
    pub fn timeline_meta(prefix: &str) -> String {
        format!("{prefix}-timeline-meta")
    }

    /// `{prefix}-workflow-state` — a timeline row's state badge (the
    /// detail panel's badge is the unprefixed [`workflow_state`]).
    pub fn timeline_workflow_state(prefix: &str) -> String {
        format!("{prefix}-workflow-state")
    }

    /// `{prefix}-action-delete` — the row-actions delete entry.
    pub fn action_delete(prefix: &str) -> String {
        format!("{prefix}-action-delete")
    }

    /// `{prefix}-action-edit` — the row-actions edit entry.
    pub fn action_edit(prefix: &str) -> String {
        format!("{prefix}-action-edit")
    }

    /// `manage-{prefix}-btn` — the child-section manage toggle.
    pub fn manage(prefix: &str) -> String {
        format!("manage-{prefix}-btn")
    }
}

/// The resolved UX contract for one entity's list/table rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct UxPlan {
    /// Per-column presentation, keyed by property/field name.
    pub columns: BTreeMap<String, ColumnPlan>,
    /// Rendering order of the field names (a permutation of the input
    /// field names — no field is dropped). Input order verbatim when the
    /// author pinned the list order.
    pub column_order: Vec<String>,
    /// How the entity's collection renders (table or opt-in timeline).
    pub collection: CollectionPlan,
    /// Row-action partition and confirmation set.
    pub actions: ActionPlan,
    /// Row-level visual defaults.
    pub visuals: RowVisuals,
    /// Resolved locale/currency baseline for templates.
    pub format: FormatConfig,
}

/// Presentation contract for one column slot.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnPlan {
    /// The final dimension (inferred, or rule-overridden).
    pub dimension: Dimension,
    /// How the value renders.
    pub display: Display,
    /// Column alignment.
    pub align: Align,
    /// Locale/currency settings for this column's values.
    pub format: FormatConfig,
    /// Status keyword → badge variant (applies to chip rendering).
    pub tone: ToneMap,
    /// Long values truncate with a tooltip (Identifier default).
    pub truncate_tooltip: bool,
    /// Whether the column is sortable in tables.
    pub sortable: bool,
}

/// Column alignment (mirror of `codegraph_config::ux::Align`, re-exported
/// here for template-consumer ergonomics).
pub use codegraph_config::ux::Align;

/// How an entity's collection renders.
#[derive(Debug, Clone, PartialEq)]
pub enum CollectionPlan {
    /// The default table rendering.
    Table,
    /// Chronological rendering — ONLY from an explicit matching
    /// `[[collection]] display = "timeline"` rule.
    Timeline {
        /// Datetime field the timeline sorts by (validated against the
        /// entity's inferred TimePoint fields).
        order_by: String,
        /// Title field of each timeline entry (rule value, or the
        /// `column_order`-first field).
        title_field: Option<String>,
        /// Extra fields shown on each entry (rule values, or up to three
        /// default preview fields).
        preview: Vec<String>,
    },
}

/// Row-action partition: inline actions and overflow-menu actions.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionPlan {
    /// Actions rendered inline (at most `[actions] inline_max`).
    pub primary: Vec<ActionSpec>,
    /// Actions collapsed into the overflow menu.
    pub menu: Vec<ActionSpec>,
    /// Action names (lowercase) that require user confirmation.
    pub confirm: Vec<String>,
}

/// One row action with its user-facing label.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionSpec {
    /// The action.
    pub action: RowAction,
    /// Human-facing label (`"Open"` / `"Edit"` / `"Delete"`).
    pub label: String,
}

/// The canonical row actions, in canonical order: `Open` (primary), then
/// `Edit`, then `Delete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    /// Navigate to the detail view.
    Open,
    /// Enter the edit form.
    Edit,
    /// Delete the row.
    Delete,
}

impl RowAction {
    /// Canonical order: `Open` first (it earns the inline slot at
    /// `inline_max = 1`), then `Edit`, then `Delete`.
    pub const CANONICAL: [RowAction; 3] = [RowAction::Open, RowAction::Edit, RowAction::Delete];

    /// Lowercase action name (matched against `[actions] confirm`).
    pub fn as_str(self) -> &'static str {
        match self {
            RowAction::Open => "open",
            RowAction::Edit => "edit",
            RowAction::Delete => "delete",
        }
    }

    /// User-facing label.
    pub fn label(self) -> &'static str {
        match self {
            RowAction::Open => "Open",
            RowAction::Edit => "Edit",
            RowAction::Delete => "Delete",
        }
    }
}

/// Row-level visual defaults for table/timeline rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct RowVisuals {
    /// Alternate row shading.
    pub zebra: bool,
    /// Shade soft-deleted / workflow-terminal rows.
    pub inactive_shading: bool,
    /// Vertical alignment of row cells.
    pub vertical_align: VerticalAlign,
}

/// Vertical alignment of row cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAlign {
    /// Cells center vertically (default).
    Center,
    /// Cells align to the top.
    Top,
}

/// Generator-supplied inputs for one entity's UX plan.
///
/// Deliberately decoupled from graph internals: the UI generator (and
/// later the IFML generators) fill this from data they already collected,
/// so [`build_ux_plan`] stays unit-testable without a graph.
pub struct UxPlanInput<'a> {
    /// The entity title (matched against `[[collection]] entity_pattern`).
    pub entity_title: &'a str,
    /// The entity's collected UI fields, in list order.
    pub fields: &'a [UiField],
    /// Graph properties by field name — may be partial or empty (synthetic
    /// columns infer from the [`UiField`] alone).
    pub prop_by_name: BTreeMap<&'a str, &'a PropertyNode>,
    /// The entity's workflow status field, if a workflow is configured.
    pub workflow_status_field: Option<&'a str>,
    /// The workflow's terminal state names (drives inactive shading).
    pub workflow_terminal_states: &'a [String],
    /// A SoftDeleteMarker policy role is present on the entity.
    pub has_soft_delete: bool,
    /// The author pinned the list order via dto list_include/list_exclude —
    /// `column_order` keeps the input order verbatim.
    pub user_pinned_list_order: bool,
}

/// Build the UX plan for one entity.
///
/// Returns `None` when `rules` is `None` (the `ux_rules` feature flag is
/// off — callers pass the project's rules only when enabled), so flag-off
/// callers skip the plan entirely and their output stays byte-identical.
///
/// Errors when an explicit timeline rule's `order_by` does not resolve to
/// one of the entity's inferred TimePoint fields (the error names the
/// candidate fields).
pub fn build_ux_plan(rules: Option<&UxRules>, input: &UxPlanInput<'_>) -> Result<Option<UxPlan>> {
    let Some(rules) = rules else {
        return Ok(None);
    };

    // Pass 2: per-column rules over Pass-1 inference.
    let mut columns = BTreeMap::new();
    let mut inferred_by_name: BTreeMap<&str, Dimension> = BTreeMap::new();
    for field in input.fields {
        let prop = prop_for(input, field);
        let resolution = resolve_column(rules, prop, field, input.workflow_status_field);
        inferred_by_name.insert(field.name.as_str(), resolution.inferred);
        columns.insert(
            field.name.clone(),
            resolution.into_column_plan(rules.format.clone()),
        );
    }

    let column_order = compute_column_order(input, &columns);
    let collection = resolve_collection(rules, input, &inferred_by_name, &columns, &column_order)?;

    // Pass 3: action partition.
    let actions = build_action_plan(&rules.actions);

    // Pass 4: row visuals.
    let visuals = RowVisuals {
        zebra: true,
        inactive_shading: input.has_soft_delete || !input.workflow_terminal_states.is_empty(),
        vertical_align: VerticalAlign::Center,
    };

    Ok(Some(UxPlan {
        columns,
        column_order,
        collection,
        actions,
        visuals,
        format: rules.format.clone(),
    }))
}

fn prop_for<'a>(input: &UxPlanInput<'a>, field: &UiField) -> Option<&'a PropertyNode> {
    input.prop_by_name.get(field.name.as_str()).copied()
}

/// The outcome of resolving one field through Pass-1 inference plus the
/// rule tier. `pub(crate)` so [`crate::ux::diagnostics`] shares the exact
/// same resolution (no logic drift between plan and diagnostics).
pub(crate) struct ColumnResolution {
    /// The Pass-1 inferred dimension.
    pub inferred: Dimension,
    /// The final dimension (rule override wins over inference).
    pub dimension: Dimension,
    /// Rule-pinned display, if a rule matched and pinned one.
    pub rule_display: Option<Display>,
    /// Rule-pinned alignment.
    pub rule_align: Option<Align>,
    /// Rule-pinned tone map.
    pub rule_tone: Option<ToneMap>,
    /// Rule-pinned sortability.
    pub rule_sortable: Option<bool>,
    /// Hints collected during inference (money keyword guesses).
    pub hints: DimensionHints,
    /// A matching rule PINNED the dimension — cancels the money hint for
    /// this column (the author made the call).
    pub dimension_pinned: bool,
}

impl ColumnResolution {
    /// Fold into the pack-default column plan, applying rule payloads.
    ///
    /// `pub(crate)` so the IFML route generator (issue #300) resolves its
    /// fallback-table columns through the exact same inference + rule tier
    /// + pack-default fold — no second fold path to drift.
    pub(crate) fn into_column_plan(self, format: FormatConfig) -> ColumnPlan {
        let (default_display, default_align, sortable_base, truncate_default) =
            dimension_defaults(self.dimension);
        let display = self.rule_display.unwrap_or(default_display);
        ColumnPlan {
            dimension: self.dimension,
            display,
            align: self.rule_align.unwrap_or(default_align),
            format,
            tone: self
                .rule_tone
                .clone()
                .unwrap_or_else(|| default_tone(display)),
            truncate_tooltip: truncate_default,
            sortable: self.rule_sortable.unwrap_or(sortable_base),
        }
    }
}

/// Pass-1 inference plus first-match-wins rule resolution for one field.
pub(crate) fn resolve_column(
    rules: &UxRules,
    prop: Option<&PropertyNode>,
    field: &UiField,
    workflow_status_field: Option<&str>,
) -> ColumnResolution {
    let (inferred, hints) = infer_dimension_with_hints(prop, field, workflow_status_field);
    let rule = rules
        .columns
        .iter()
        .find(|rule| column_rule_matches(rule, field, prop, inferred));
    ColumnResolution {
        inferred,
        dimension: rule.and_then(|r| r.dimension).unwrap_or(inferred),
        rule_display: rule.and_then(|r| r.display),
        rule_align: rule.and_then(|r| r.align),
        rule_tone: rule.and_then(|r| r.tone.clone()),
        rule_sortable: rule.and_then(|r| r.sortable),
        hints,
        // A genuine override — the rule's dimension differs from the
        // inference — cancels the money hint. A rule that CONFIRMS the
        // inferred dimension (e.g. the pack's money rule) is not an
        // override: the keyword-vs-quantity ambiguity stays worth
        // surfacing.
        dimension_pinned: rule
            .and_then(|r| r.dimension)
            .is_some_and(|d| d != inferred),
    }
}

/// AND-semantics selector match over the set selectors a rule declares.
///
/// The `dimension` key is dual-purpose: on a selector-only rule (pack
/// style, `dimension = "money"`) it IS the selector, matched against the
/// INFERRED dimension. When the rule also carries a classification /
/// pg_type / name_pattern selector, those establish the match and the
/// dimension key becomes payload — that is the ONLY way a rule can
/// override the inferred dimension (`name_pattern = "*_amount"` +
/// `dimension = "quantity"`), and with strict AND semantics it would be
/// unreachable. A rule with no selectors matches every column.
fn column_rule_matches(
    rule: &ColumnRule,
    field: &UiField,
    prop: Option<&PropertyNode>,
    inferred: Dimension,
) -> bool {
    let has_other_selectors =
        rule.classification.is_some() || rule.pg_type.is_some() || rule.name_pattern.is_some();
    if rule.dimension.is_some() && !has_other_selectors && rule.dimension != Some(inferred) {
        return false;
    }
    if let Some(classification) = rule.classification.as_deref()
        && !classification_matches(prop, classification)
    {
        return false;
    }
    if let Some(pg_type) = rule.pg_type.as_deref()
        && !pg_type_matches(&field.pg_type, pg_type)
    {
        return false;
    }
    if let Some(pattern) = rule.name_pattern.as_deref()
        && !glob_match(pattern, &field.name)
    {
        return false;
    }
    true
}

/// Best-effort classification match: the property's classification-kind
/// name (lowercased, `_`/`-` stripped) and the selector must name each
/// other as substrings ("codelist" matches `CodelistReference`); an empty
/// prop map simply never matches.
fn classification_matches(prop: Option<&PropertyNode>, selector: &str) -> bool {
    let Some(kind) = prop.and_then(|p| p.classification_kind.as_ref()) else {
        return false;
    };
    let kind_name = format!("{kind:?}").to_lowercase().replace(['_', '-'], "");
    let selector = selector.to_lowercase().replace(['_', '-'], "");
    kind_name.contains(&selector) || selector.contains(&kind_name)
}

/// Case-insensitive prefix match on the element pg type (arrays infer
/// their element): "numeric" matches `NUMERIC(10,2)`, "bool" matches
/// `BOOLEAN`.
fn pg_type_matches(field_pg_type: &str, selector: &str) -> bool {
    let element = field_pg_type
        .strip_suffix("[]")
        .unwrap_or(field_pg_type)
        .to_lowercase();
    let selector = selector.trim().to_lowercase();
    !selector.is_empty() && element.starts_with(&selector)
}

/// Pack defaults for a dimension, mirroring `ux_default.toml`:
/// quantity/money right+raw, time-point raw left, status-category chip,
/// identifier copy-chip + truncate-tooltip, reference link, flag chip.
/// Sortability allow-list: Quantity, Money, TimePoint, Text.
fn dimension_defaults(dimension: Dimension) -> (Display, Align, bool, bool) {
    match dimension {
        Dimension::Text => (Display::Raw, Align::Left, true, false),
        Dimension::Quantity => (Display::Raw, Align::Right, true, false),
        Dimension::Money => (Display::Raw, Align::Right, true, false),
        Dimension::TimePoint => (Display::Raw, Align::Left, true, false),
        Dimension::StatusCategory => (Display::Chip, Align::Left, false, false),
        Dimension::Identifier => (Display::CopyChip, Align::Left, false, true),
        Dimension::Reference => (Display::Link, Align::Left, false, false),
        Dimension::Flag => (Display::Chip, Align::Left, false, false),
    }
}

/// Chip columns get the workflow-panel keyword tone map by default;
/// non-chip columns carry an empty map (tone is unused there).
fn default_tone(display: Display) -> ToneMap {
    if matches!(display, Display::Chip | Display::CopyChip) {
        ToneMap::default()
    } else {
        ToneMap(BTreeMap::new())
    }
}

/// Audit-stamp names sort last in `column_order`:
/// `created_at` / `updated_at` / `deleted_at` and `*_by`.
fn is_audit_field(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == "created_at"
        || lower == "updated_at"
        || lower == "deleted_at"
        || lower.ends_with("_by")
}

/// `column_order`: the first human-readable field (a Text-dimension field
/// named name/title/label-ish — exact `name` > `title` > `label`, then
/// contains, case-insensitive) leads, audit stamps trail, everything else
/// keeps input order. No field is dropped. When the author pinned the list
/// order, the input order is kept verbatim.
fn compute_column_order(
    input: &UxPlanInput<'_>,
    columns: &BTreeMap<String, ColumnPlan>,
) -> Vec<String> {
    let names: Vec<String> = input.fields.iter().map(|f| f.name.clone()).collect();
    if input.user_pinned_list_order {
        return names;
    }

    let readable = names
        .iter()
        .filter(|n| {
            columns
                .get(*n)
                .is_some_and(|plan| plan.dimension == Dimension::Text && !is_audit_field(n))
        })
        .map(String::as_str)
        .collect::<Vec<_>>();

    let exact = |keyword: &str| {
        readable
            .iter()
            .find(|n| n.eq_ignore_ascii_case(keyword))
            .copied()
    };
    let contains = |keyword: &str| {
        readable
            .iter()
            .find(|n| n.to_lowercase().contains(keyword))
            .copied()
    };
    let lead = exact("name")
        .or_else(|| exact("title"))
        .or_else(|| exact("label"))
        .or_else(|| contains("name"))
        .or_else(|| contains("title"))
        .or_else(|| contains("label"));

    let mut ordered = Vec::with_capacity(names.len());
    if let Some(lead) = lead {
        ordered.push(lead.to_string());
    }
    for name in &names {
        if Some(name.as_str()) != lead && !is_audit_field(name) {
            ordered.push(name.clone());
        }
    }
    for name in &names {
        if is_audit_field(name) {
            ordered.push(name.clone());
        }
    }
    ordered
}

/// The first `[[collection]]` rule whose `entity_pattern` matches the
/// entity title wins (an absent pattern matches every entity). Timeline
/// only when that rule says `display = "timeline"`; anything else —
/// including no rule at all — is a table. NEVER inferred.
fn resolve_collection(
    rules: &UxRules,
    input: &UxPlanInput<'_>,
    inferred_by_name: &BTreeMap<&str, Dimension>,
    columns: &BTreeMap<String, ColumnPlan>,
    column_order: &[String],
) -> Result<CollectionPlan> {
    let matched = rules.collections.iter().find(|rule| {
        rule.entity_pattern
            .as_deref()
            .is_none_or(|pattern| glob_match(pattern, input.entity_title))
    });

    let Some(rule) = matched else {
        return Ok(CollectionPlan::Table);
    };
    if rule.display.as_deref() != Some("timeline") {
        return Ok(CollectionPlan::Table);
    }

    let Some(order_by) = rule.order_by.as_deref() else {
        // Parse-validated for TOML-authored rules; still a hard error for
        // programmatically built rules.
        return Err(Error::Config(format!(
            "ux-rules: [[collection]] entity_pattern = {:?} sets display = \"timeline\" without order_by\n\
             hint: set order_by = \"<datetime property>\"; the timeline sorts by it",
            rule.entity_pattern.as_deref().unwrap_or("*")
        )));
    };

    let time_point_fields: Vec<&str> = input
        .fields
        .iter()
        .filter(|f| inferred_by_name.get(f.name.as_str()) == Some(&Dimension::TimePoint))
        .map(|f| f.name.as_str())
        .collect();
    if !time_point_fields.contains(&order_by) {
        let candidates = if time_point_fields.is_empty() {
            format!("entity {:?} has no time-point fields", input.entity_title)
        } else {
            format!(
                "entity {:?} time-point fields are: {}",
                input.entity_title,
                time_point_fields.join(", ")
            )
        };
        return Err(Error::Config(format!(
            "ux-rules: [[collection]] entity_pattern = {:?} timeline order_by {:?} does not name a \
             time-point field; {candidates}",
            rule.entity_pattern.as_deref().unwrap_or("*"),
            order_by
        )));
    }

    let title_field = match rule.title_field.as_deref() {
        Some(title) if !title.is_empty() => Some(title.to_string()),
        _ => column_order.first().cloned(),
    };
    let preview = if rule.preview.is_empty() {
        default_preview(columns, column_order, title_field.as_deref())
    } else {
        rule.preview.clone()
    };

    Ok(CollectionPlan::Timeline {
        order_by: order_by.to_string(),
        title_field,
        preview,
    })
}

/// Default timeline preview: the first three `column_order` fields that
/// are Text/Quantity and neither the title field nor an audit stamp.
fn default_preview(
    columns: &BTreeMap<String, ColumnPlan>,
    column_order: &[String],
    title_field: Option<&str>,
) -> Vec<String> {
    column_order
        .iter()
        .filter(|name| {
            columns
                .get(*name)
                .is_some_and(|plan| matches!(plan.dimension, Dimension::Text | Dimension::Quantity))
        })
        .filter(|name| Some(name.as_str()) != title_field && !is_audit_field(name))
        .take(3)
        .cloned()
        .collect()
}

/// Pass 3: partition the canonical actions at `inline_max` (clamped to at
/// least 1 so `Open` always survives inline) and resolve the confirmation
/// list (pack default `["delete"]` when unset; entries lowercase).
fn build_action_plan(rules: &codegraph_config::ux::ActionRules) -> ActionPlan {
    let inline_max = rules.inline_max.max(1);
    let mut primary = Vec::new();
    let mut menu = Vec::new();
    for action in RowAction::CANONICAL {
        let spec = ActionSpec {
            action,
            label: action.label().to_string(),
        };
        if primary.len() < inline_max {
            primary.push(spec);
        } else {
            menu.push(spec);
        }
    }
    let confirm: Vec<String> = if rules.confirm.is_empty() {
        vec![RowAction::Delete.as_str().to_string()]
    } else {
        rules
            .confirm
            .iter()
            .map(|c| c.trim().to_lowercase())
            .collect()
    };
    ActionPlan {
        primary,
        menu,
        confirm,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codegraph_config::{builtin_ux_rules, parse_ux_rules_str};
    use codegraph_type_contracts::RefClassificationKind;

    fn field(name: &str, pg_type: &str, ts_type: &str, input_type: &str) -> UiField {
        UiField {
            name: name.to_string(),
            label: String::new(),
            ts_type: ts_type.to_string(),
            input_type: input_type.to_string(),
            is_required: false,
            is_array: false,
            is_entity_ref: false,
            is_immutable: false,
            is_codelist: false,
            is_range: false,
            codelist_values: vec![],
            description: String::new(),
            pg_type: pg_type.to_string(),
            open_end: false,
            ref_api_path: None,
            structured_sub_fields: vec![],
            nested_type_name: None,
        }
    }

    fn text_field(name: &str) -> UiField {
        field(name, "TEXT", "string", "text")
    }

    fn codelist_field(name: &str) -> UiField {
        let mut f = field(name, "TEXT", "string", "select");
        f.is_codelist = true;
        f.codelist_values = vec!["draft".into(), "approved".into()];
        f
    }

    fn entity_ref_field(name: &str) -> UiField {
        let mut f = field(name, "UUID", "string", "text");
        f.is_entity_ref = true;
        f
    }

    fn test_input<'a>(entity_title: &'a str, fields: &'a [UiField]) -> UxPlanInput<'a> {
        UxPlanInput {
            entity_title,
            fields,
            prop_by_name: BTreeMap::new(),
            workflow_status_field: None,
            workflow_terminal_states: &[],
            has_soft_delete: false,
            user_pinned_list_order: false,
        }
    }

    fn pack() -> UxRules {
        builtin_ux_rules().unwrap().rules
    }

    fn ux_rules(toml: &str) -> UxRules {
        parse_ux_rules_str(toml).unwrap().rules
    }

    const PACK: &str = codegraph_config::ux::BUILT_IN_UX_PACK;

    #[test]
    fn none_rules_yield_none_plan() {
        let fields = vec![text_field("name")];
        let plan = build_ux_plan(None, &test_input("Refund", &fields)).unwrap();
        assert!(plan.is_none(), "flag off ⇒ no plan, no consumption");
    }

    #[test]
    fn pack_defaults_per_dimension() {
        let fields = vec![
            field("id", "UUID", "string", "text"),
            field("headcount", "INTEGER", "number", "number"),
            field("total_amount", "NUMERIC(10,2)", "string", "number"),
            field("due_at", "TIMESTAMPTZ", "string", "datetime-local"),
            codelist_field("status"),
            field("scheme_id", "TEXT", "string", "text"),
            entity_ref_field("assignee"),
            field("active", "BOOLEAN", "boolean", "checkbox"),
        ];
        let plan = build_ux_plan(Some(&pack()), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();

        let id = &plan.columns["id"];
        assert_eq!(id.dimension, Dimension::Identifier);
        assert_eq!(id.display, Display::CopyChip);
        assert!(id.truncate_tooltip, "identifier truncates with a tooltip");
        assert!(!id.sortable);

        let qty = &plan.columns["headcount"];
        assert_eq!(qty.dimension, Dimension::Quantity);
        assert_eq!(qty.align, Align::Right);
        assert_eq!(qty.display, Display::Raw);
        assert!(qty.sortable);

        let money = &plan.columns["total_amount"];
        assert_eq!(money.dimension, Dimension::Money);
        assert_eq!(money.align, Align::Right);
        assert!(money.sortable);

        let time = &plan.columns["due_at"];
        assert_eq!(time.dimension, Dimension::TimePoint);
        assert_eq!(time.align, Align::Left);
        assert_eq!(time.display, Display::Raw);
        assert!(time.sortable);

        let status = &plan.columns["status"];
        assert_eq!(status.dimension, Dimension::StatusCategory);
        assert_eq!(status.display, Display::Chip);
        assert_eq!(status.tone.lookup("active"), "default");
        assert_eq!(status.tone.lookup("pending"), "secondary");
        assert!(!status.sortable);

        let reference = &plan.columns["assignee"];
        assert_eq!(reference.display, Display::Link);
        assert!(!reference.sortable);

        let flag = &plan.columns["active"];
        assert_eq!(flag.dimension, Dimension::Flag);
        assert_eq!(flag.display, Display::Chip);
        assert!(!flag.sortable);

        let text = &plan.columns["scheme_id"];
        assert_eq!(text.dimension, Dimension::Text);
        assert_eq!(text.display, Display::Raw);
        assert_eq!(text.align, Align::Left);
        assert!(text.sortable);
    }

    #[test]
    fn rule_override_beats_inference_and_kills_money_hint() {
        let rules = ux_rules(
            "[[column]]\nname_pattern = \"*_amount\"\ndimension = \"quantity\"\ndisplay = \"raw\"\n",
        );
        let fields = vec![field("total_amount", "NUMERIC(10,2)", "string", "number")];
        let plan = build_ux_plan(Some(&rules), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();
        let column = &plan.columns["total_amount"];
        assert_eq!(column.dimension, Dimension::Quantity, "rule pins quantity");
        assert_eq!(column.display, Display::Raw);

        // The keyword money hint is cancelled: the author pinned intent.
        let diag = super::super::diagnostics::collect_diagnostics(
            &rules,
            &test_input("Refund", &fields),
            &plan,
        );
        assert!(diag.money_hints.is_empty(), "{:?}", diag.money_hints);
    }

    #[test]
    fn first_match_wins_over_merged_columns_vec() {
        // Merged project-ahead-of-pack shape: project rule first.
        let project = ux_rules("[[column]]\nname_pattern = \"*_amount\"\ndisplay = \"chip\"\n");
        let pack_rule = ux_rules("[[column]]\ndimension = \"money\"\ndisplay = \"copy-chip\"\n");
        let merged = UxRules {
            columns: project
                .columns
                .iter()
                .chain(pack_rule.columns.iter())
                .cloned()
                .collect(),
            ..UxRules::default()
        };
        let fields = vec![field("total_amount", "NUMERIC(10,2)", "string", "number")];
        let plan = build_ux_plan(Some(&merged), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();
        let column = &plan.columns["total_amount"];
        assert_eq!(column.display, Display::Chip, "first matching rule wins");
        // The shadowed rule never contributes its payload; the Money pack
        // default (raw) applies for what the winner left unset.
        assert_eq!(column.align, Align::Right);
    }

    #[test]
    fn selector_match_on_classification_and_pg_type() {
        // A CodelistReference prop backs the status field; "codelist"
        // substring-matches the kind name (best-effort).
        let prop = PropertyNode {
            name: "status".into(),
            prop_type: "string".into(),
            description: None,
            format: None,
            is_required: false,
            is_nullable: false,
            is_id: false,
            type_expr: None,
            is_array: false,
            min_items: None,
            max_items: None,
            pattern: None,
            min_length: None,
            max_length: None,
            minimum: None,
            maximum: None,
            pg_column_name: "status".into(),
            pg_column_type: "TEXT".into(),
            rust_field_name: "status".into(),
            rust_field_type: "String".into(),
            sea_orm_type: "String".into(),
            render_strategy: "scalar".into(),
            ref_target: None,
            classification: None,
            projection: None,
            classification_kind: Some(RefClassificationKind::CodelistReference),
            ui_override_detail: None,
            ui_override_list_cell: None,
            ui_override_form: None,
            ui_override_inline: None,
        };
        let mut prop_by_name = BTreeMap::new();
        prop_by_name.insert("status", &prop);
        let rules = ux_rules("[[column]]\nclassification = \"codelist\"\ndisplay = \"link\"\n");
        let fields = vec![codelist_field("status"), text_field("name")];
        let input = UxPlanInput {
            prop_by_name,
            ..test_input("Refund", &fields)
        };
        let plan = build_ux_plan(Some(&rules), &input).unwrap().unwrap();
        assert_eq!(
            plan.columns["status"].display,
            Display::Link,
            "classification selector matched the codelist prop"
        );
        assert_eq!(
            plan.columns["name"].display,
            Display::Raw,
            "no prop, no match"
        );

        // pg_type prefix match: "numeric" matches NUMERIC(10,2).
        let pg_rules = ux_rules("[[column]]\npg_type = \"numeric\"\ndisplay = \"copy-chip\"\n");
        let fields = vec![
            text_field("name"),
            field("total", "NUMERIC(10,2)", "string", "number"),
        ];
        let plan = build_ux_plan(Some(&pg_rules), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();
        assert_eq!(
            plan.columns["name"].display,
            Display::Raw,
            "TEXT is not numeric"
        );
        assert_eq!(
            plan.columns["total"].display,
            Display::CopyChip,
            "pg_type prefix match pins the payload"
        );
    }

    #[test]
    fn sortable_allow_list_and_rule_opt_out() {
        let fields = vec![text_field("description"), codelist_field("status")];
        let plan = build_ux_plan(Some(&pack()), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();
        assert!(plan.columns["description"].sortable);
        assert!(!plan.columns["status"].sortable);

        let opt_in = ux_rules("[[column]]\ndimension = \"status-category\"\nsortable = true\n");
        let plan = build_ux_plan(Some(&opt_in), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();
        assert!(plan.columns["status"].sortable, "rule opt-IN wins");

        let opt_out = ux_rules("[[column]]\ndimension = \"text\"\nsortable = false\n");
        let plan = build_ux_plan(Some(&opt_out), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();
        assert!(!plan.columns["description"].sortable, "rule opt-OUT wins");
    }

    #[test]
    fn column_order_readable_first_audit_last_nothing_dropped() {
        let fields = vec![
            field("id", "UUID", "string", "text"),
            field("created_at", "TIMESTAMPTZ", "string", "datetime-local"),
            codelist_field("status"),
            text_field("company_name"),
            field("updated_by", "TEXT", "string", "text"),
            field("headcount", "INTEGER", "number", "number"),
        ];
        let plan = build_ux_plan(Some(&pack()), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();
        assert_eq!(
            plan.column_order,
            vec![
                "company_name".to_string(),
                "id".to_string(),
                "status".to_string(),
                "headcount".to_string(),
                "created_at".to_string(),
                "updated_by".to_string(),
            ]
        );
    }

    #[test]
    fn column_order_prefers_exact_name_then_title_then_label() {
        // Exact `name` beats an earlier contains-"name" candidate.
        let fields = vec![text_field("display_name"), text_field("name")];
        let plan = build_ux_plan(Some(&pack()), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();
        assert_eq!(plan.column_order[0], "name", "exact `name` first");

        // Exact `title` beats a contains-"label" candidate.
        let fields = vec![text_field("owner_label"), text_field("title")];
        let plan = build_ux_plan(Some(&pack()), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();
        assert_eq!(
            plan.column_order[0], "title",
            "exact `title` beats contains-label"
        );

        // No exact match: contains order is name > title > label.
        let fields = vec![text_field("owner_label"), text_field("job")];
        let plan = build_ux_plan(Some(&pack()), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();
        assert_eq!(
            plan.column_order[0], "owner_label",
            "contains-label wins when no exact match"
        );
    }

    #[test]
    fn pinned_list_order_kept_verbatim() {
        let fields = vec![
            field("created_at", "TIMESTAMPTZ", "string", "datetime-local"),
            text_field("name"),
            field("id", "UUID", "string", "text"),
        ];
        let mut input = test_input("Refund", &fields);
        input.user_pinned_list_order = true;
        let plan = build_ux_plan(Some(&pack()), &input).unwrap().unwrap();
        assert_eq!(
            plan.column_order,
            vec![
                "created_at".to_string(),
                "name".to_string(),
                "id".to_string()
            ]
        );
    }

    #[test]
    fn timeline_only_from_explicit_matching_rule() {
        let rules = ux_rules(
            "[[collection]]\nentity_pattern = \"Refund*\"\ndisplay = \"timeline\"\norder_by = \"created_at\"\n",
        );
        let fields = vec![
            text_field("reference"),
            text_field("description"),
            field("headcount", "INTEGER", "number", "number"),
            field("created_at", "TIMESTAMPTZ", "string", "datetime-local"),
        ];
        let plan = build_ux_plan(Some(&rules), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();
        let CollectionPlan::Timeline {
            order_by,
            title_field,
            preview,
        } = plan.collection
        else {
            panic!("explicit rule must produce a timeline");
        };
        assert_eq!(order_by, "created_at");
        assert_eq!(
            title_field.as_deref(),
            Some("reference"),
            "column_order-first default"
        );
        // Defaults: first Text/Quantity column_order fields that are not
        // the title and not audit stamps.
        assert_eq!(
            preview,
            vec!["description".to_string(), "headcount".to_string()]
        );

        // A non-matching entity stays a table — the heuristic NEVER infers.
        let plan = build_ux_plan(Some(&rules), &test_input("Payout", &fields))
            .unwrap()
            .unwrap();
        assert_eq!(plan.collection, CollectionPlan::Table);
    }

    #[test]
    fn table_rule_shadows_later_timeline_rule() {
        let rules = ux_rules(
            "[[collection]]\nentity_pattern = \"Refund*\"\ndisplay = \"table\"\n\n\
             [[collection]]\nentity_pattern = \"*\"\ndisplay = \"timeline\"\norder_by = \"created_at\"\n",
        );
        let fields = vec![field(
            "created_at",
            "TIMESTAMPTZ",
            "string",
            "datetime-local",
        )];
        let plan = build_ux_plan(Some(&rules), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();
        assert_eq!(
            plan.collection,
            CollectionPlan::Table,
            "first-match-wins opt-out"
        );

        let plan = build_ux_plan(Some(&rules), &test_input("Payout", &fields))
            .unwrap()
            .unwrap();
        assert!(
            matches!(plan.collection, CollectionPlan::Timeline { .. }),
            "the bare-star timeline still reaches non-shadowed entities"
        );
    }

    #[test]
    fn unresolvable_order_by_errors_naming_candidates() {
        let rules = ux_rules(
            "[[collection]]\nentity_pattern = \"Refund*\"\ndisplay = \"timeline\"\norder_by = \"deleted_at\"\n",
        );
        let fields = vec![
            field("created_at", "TIMESTAMPTZ", "string", "datetime-local"),
            field("updated_at", "TIMESTAMPTZ", "string", "datetime-local"),
        ];
        let err = build_ux_plan(Some(&rules), &test_input("Refund", &fields))
            .expect_err("unresolvable order_by must fail");
        let message = err.to_string();
        assert!(message.contains("deleted_at"), "{message}");
        assert!(message.contains("Refund"), "{message}");
        assert!(
            message.contains("created_at") && message.contains("updated_at"),
            "{message}"
        );

        // No TimePoint fields at all: the error says so.
        let fields = vec![text_field("name")];
        let err = build_ux_plan(Some(&rules), &test_input("Refund", &fields))
            .expect_err("no candidates must fail");
        assert!(err.to_string().contains("no time-point fields"), "{err}");

        // An existing non-time field is also unresolvable.
        let rules = ux_rules(
            "[[collection]]\nentity_pattern = \"Refund*\"\ndisplay = \"timeline\"\norder_by = \"name\"\n",
        );
        let fields = vec![
            text_field("name"),
            field("created_at", "DATE", "string", "date"),
        ];
        let err = build_ux_plan(Some(&rules), &test_input("Refund", &fields))
            .expect_err("non-time order_by must fail");
        assert!(err.to_string().contains("created_at"), "{err}");
    }

    #[test]
    fn explicit_title_and_preview_flow_through() {
        let rules = ux_rules(
            "[[collection]]\nentity_pattern = \"Refund*\"\ndisplay = \"timeline\"\norder_by = \"created_at\"\ntitle_field = \"reference\"\npreview = [\"status\"]\n",
        );
        let fields = vec![
            text_field("reference"),
            codelist_field("status"),
            field("created_at", "TIMESTAMPTZ", "string", "datetime-local"),
        ];
        let plan = build_ux_plan(Some(&rules), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();
        let CollectionPlan::Timeline {
            title_field,
            preview,
            ..
        } = plan.collection
        else {
            panic!("timeline expected");
        };
        assert_eq!(title_field.as_deref(), Some("reference"));
        assert_eq!(
            preview,
            vec!["status".to_string()],
            "author values replace defaults"
        );
    }

    #[test]
    fn actions_partition_at_inline_max_one() {
        let plan = build_ux_plan(Some(&pack()), &test_input("Refund", &[]))
            .unwrap()
            .unwrap();
        assert_eq!(
            plan.actions
                .primary
                .iter()
                .map(|s| s.action)
                .collect::<Vec<_>>(),
            vec![RowAction::Open],
            "Open earns the single inline slot"
        );
        assert_eq!(
            plan.actions
                .menu
                .iter()
                .map(|s| s.action)
                .collect::<Vec<_>>(),
            vec![RowAction::Edit, RowAction::Delete]
        );
        assert_eq!(plan.actions.confirm, vec!["delete".to_string()]);
        assert_eq!(plan.actions.primary[0].label, "Open");
    }

    #[test]
    fn inline_max_budget_and_confirm_replace() {
        let rules = ux_rules("[actions]\ninline_max = 2\nconfirm = [\"Refund\", \"Void\"]\n");
        let plan = build_ux_plan(Some(&rules), &test_input("Refund", &[]))
            .unwrap()
            .unwrap();
        assert_eq!(
            plan.actions
                .primary
                .iter()
                .map(|s| s.action)
                .collect::<Vec<_>>(),
            vec![RowAction::Open, RowAction::Edit]
        );
        assert_eq!(
            plan.actions
                .menu
                .iter()
                .map(|s| s.action)
                .collect::<Vec<_>>(),
            vec![RowAction::Delete]
        );
        assert_eq!(
            plan.actions.confirm,
            vec!["refund".to_string(), "void".to_string()],
            "project confirm replaces the pack default, lowercased"
        );
    }

    #[test]
    fn row_visuals_defaults_and_inactive_shading_signals() {
        let plan = build_ux_plan(Some(&pack()), &test_input("Refund", &[]))
            .unwrap()
            .unwrap();
        assert!(plan.visuals.zebra);
        assert!(!plan.visuals.inactive_shading);
        assert_eq!(plan.visuals.vertical_align, VerticalAlign::Center);

        let mut input = test_input("Refund", &[]);
        input.has_soft_delete = true;
        let plan = build_ux_plan(Some(&pack()), &input).unwrap().unwrap();
        assert!(
            plan.visuals.inactive_shading,
            "soft delete signals inactive rows"
        );

        let input = UxPlanInput {
            workflow_terminal_states: &["rejected".to_string()],
            ..test_input("Refund", &[])
        };
        let plan = build_ux_plan(Some(&pack()), &input).unwrap().unwrap();
        assert!(
            plan.visuals.inactive_shading,
            "terminal workflow states do too"
        );
    }

    #[test]
    fn format_flows_to_plan_and_columns() {
        let parsed =
            parse_ux_rules_str("[format]\nlocale = \"de-DE\"\ncurrency = \"EUR\"\n").unwrap();
        let fields = vec![field("total_amount", "NUMERIC(10,2)", "string", "number")];
        let plan = build_ux_plan(Some(&parsed.rules), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();
        assert_eq!(plan.format.locale, "de-DE");
        assert_eq!(plan.format.currency.as_deref(), Some("EUR"));
        assert_eq!(plan.columns["total_amount"].format, plan.format);

        let plan = build_ux_plan(Some(&pack()), &test_input("Refund", &fields))
            .unwrap()
            .unwrap();
        assert_eq!(plan.format.locale, "en-NZ", "pack baseline wins when unset");
        assert_eq!(plan.format.currency.as_deref(), Some("NZD"));
    }

    #[test]
    fn pack_document_pinned_constants_exist() {
        // The literal pack keeps the contract the defaults mirror; parse it
        // so a pack edit that breaks the default contract is caught here.
        let parsed = parse_ux_rules_str(PACK).unwrap();
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.rules.actions.inline_max, 1);
        assert_eq!(parsed.rules.actions.confirm, vec!["delete".to_string()]);
        assert!(
            parsed.rules.collections.is_empty(),
            "timeline is opt-in only"
        );
        assert_eq!(ids::ACTIONS, "actions");
        assert_eq!(ids::ACTIONS_MENU, "actions-menu");
        assert_eq!(ids::CHIP, "chip");
        assert_eq!(ids::COPY, "copy");
        assert_eq!(ids::TIMELINE, "timeline");
        assert_eq!(ids::TIMELINE_ITEM, "timeline-item");
    }

    #[test]
    fn ids_full_table_matches_template_literals() {
        // Entity family — literals as stamped by ui/list_page.tera,
        // ui/entity_form.tera, ui/detail_page.tera, and
        // ui/scaffold/workflow_panel.tera.
        assert_eq!(ids::entity_search("todo_item"), "todo_item-search");
        assert_eq!(ids::entity_create_btn("todo_item"), "todo_item-create-btn");
        assert_eq!(ids::entity_empty("todo_item"), "todo_item-empty");
        assert_eq!(ids::entity_no_results("todo_item"), "todo_item-no-results");
        assert_eq!(ids::entity_table("todo_item"), "todo_item-table");
        assert_eq!(ids::entity_row("todo_item"), "todo_item-row");
        assert_eq!(ids::entity_sort("todo_item", "name"), "todo_item-sort-name");
        assert_eq!(ids::entity_pagination("todo_item"), "todo_item-pagination");
        // The delete dialog follows the ONE dialog-button rule
        // (`{dialog}-confirm`); the old `-delete-confirm-confirm` double is
        // gone everywhere (issue #468 finding 3).
        assert_eq!(
            ids::entity_delete_dialog("todo_item"),
            "todo_item-delete-dialog"
        );
        assert_eq!(
            ids::entity_delete_dialog_confirm("todo_item"),
            "todo_item-delete-dialog-confirm"
        );
        assert_eq!(ids::action_delete("todo_item"), "todo_item-action-delete");
        assert_eq!(ids::action_edit("todo_item"), "todo_item-action-edit");
        assert_eq!(ids::manage("todo_item"), "manage-todo_item-btn");
        assert_eq!(ids::entity_form("todo_item"), "todo_item-form");
        assert_eq!(ids::entity_cancel_btn("todo_item"), "todo_item-cancel-btn");
        assert_eq!(ids::entity_submit_btn("todo_item"), "todo_item-submit-btn");
        assert_eq!(ids::entity_edit_btn("todo_item"), "todo_item-edit-btn");
        assert_eq!(ids::entity_delete_btn("todo_item"), "todo_item-delete-btn");
        assert_eq!(
            ids::entity_field("todo_item", "title"),
            "todo_item-field-title"
        );
        assert_eq!(ids::confirm_dialog(), "confirm-dialog");
        assert_eq!(ids::confirm_dialog_confirm(), "confirm-dialog-confirm");
        assert_eq!(ids::workflow_panel(), "workflow-panel");
        assert_eq!(ids::workflow_state(), "workflow-state");

        // IFML family — literals as stamped by ifml/svelte/page.tera and
        // the selectors() fallbacks.
        assert_eq!(ids::ifml_table("grid"), "grid-table");
        assert_eq!(ids::ifml_row("grid"), "grid-row");
        assert_eq!(ids::ifml_form("editor"), "editor-form");
        assert_eq!(ids::ifml_submit("editor"), "editor-submit");
        assert_eq!(ids::ifml_error("editor"), "editor-error");
        assert_eq!(ids::ifml_details("summary"), "summary-details");
        assert_eq!(ids::ifml_state("grid"), "grid-state");
        assert_eq!(
            ids::ifml_transition("editor", "approved"),
            "editor-transition-approved"
        );

        // Shared fragments carry either prefix.
        assert_eq!(ids::chip("todo_item"), "todo_item-chip");
        assert_eq!(ids::copy("grid"), "grid-copy");
        assert_eq!(ids::actions("todo_item"), "todo_item-actions");
        assert_eq!(ids::actions_menu("grid"), "grid-actions-menu");
        assert_eq!(ids::timeline("todo_item"), "todo_item-timeline");
        assert_eq!(ids::timeline_item("grid"), "grid-timeline-item");
    }
}
