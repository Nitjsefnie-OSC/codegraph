//! The generated UI's testid vocabulary, from one Rust source (issue #468 W3).
//!
//! [`crate::ux::plan::ids`] is the canonical table; this module projects it
//! into (a) per-render Tera contexts (`tid.*`) so page/form/child-section
//! templates interpolate the same strings the Rust table computes — page
//! markup cannot drift from `ids::` because it renders FROM it — and (b)
//! the `ui/src/lib/testids.ts` module (feature `testid_module`) that the
//! generated POMs and specs import, so the test surface is bound to the
//! same source at TypeScript compile time.

use serde::Serialize;

use crate::ux::plan::ids;

/// One entity module's static testids (snake_case module prefix).
///
/// Dynamic compositions (`{module}-field-{field}`, `{module}-sort-{col}`,
/// `{module}-action-{action}`) stay template-composed from `module_name` —
/// their variable parts are data, not vocabulary.
#[derive(Debug, Clone, Serialize)]
pub struct TidMap {
    pub search: String,
    pub create_btn: String,
    pub empty: String,
    pub no_results: String,
    pub table: String,
    pub pagination: String,
    /// The list-page row-delete dialog (normalized from `…-delete-confirm`).
    pub delete_dialog: String,
    /// Its confirm button — `{dialog}-confirm`, the one dialog-button rule.
    pub delete_dialog_confirm: String,
    pub form: String,
    pub cancel_btn: String,
    pub submit_btn: String,
    pub edit_btn: String,
    pub delete_btn: String,
    pub actions: String,
    pub actions_menu: String,
    pub action_delete: String,
    pub action_edit: String,
    pub chip: String,
    pub copy: String,
    pub timeline: String,
    pub timeline_item: String,
    pub timeline_meta: String,
    pub timeline_workflow_state: String,
    /// `child-section-{module}` — the detail page's child accordion root.
    pub child_section: String,
    pub child_section_add_btn: String,
    pub manage_btn: String,
}

/// Shared unprefixed testids (same on every page that emits them).
#[derive(Debug, Clone, Serialize)]
pub struct SharedTids {
    pub confirm_dialog: String,
    pub confirm_dialog_confirm: String,
    pub workflow_panel: String,
    pub workflow_state: String,
}

pub fn shared() -> SharedTids {
    SharedTids {
        confirm_dialog: ids::confirm_dialog(),
        confirm_dialog_confirm: ids::confirm_dialog_confirm(),
        workflow_panel: ids::workflow_panel(),
        workflow_state: ids::workflow_state(),
    }
}

/// Build the map for one entity module. Single source: every string comes
/// from [`ids`]; templates and the TS module cannot disagree with it.
pub fn for_module(module: &str) -> TidMap {
    TidMap {
        search: ids::entity_search(module),
        create_btn: ids::entity_create_btn(module),
        empty: ids::entity_empty(module),
        no_results: ids::entity_no_results(module),
        table: ids::entity_table(module),
        pagination: ids::entity_pagination(module),
        delete_dialog: ids::entity_delete_dialog(module),
        delete_dialog_confirm: ids::entity_delete_dialog_confirm(module),
        form: ids::entity_form(module),
        cancel_btn: ids::entity_cancel_btn(module),
        submit_btn: ids::entity_submit_btn(module),
        edit_btn: ids::entity_edit_btn(module),
        delete_btn: ids::entity_delete_btn(module),
        actions: ids::actions(module),
        actions_menu: ids::actions_menu(module),
        action_delete: ids::action_delete(module),
        action_edit: ids::action_edit(module),
        chip: ids::chip(module),
        copy: ids::copy(module),
        timeline: ids::timeline(module),
        timeline_item: ids::timeline_item(module),
        timeline_meta: ids::timeline_meta(module),
        timeline_workflow_state: ids::timeline_workflow_state(module),
        child_section: format!("child-section-{module}"),
        child_section_add_btn: format!("child-section-{module}-add-btn"),
        manage_btn: ids::manage(module),
    }
}
