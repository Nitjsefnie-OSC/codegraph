use serde::Serialize;

use super::page::{ChildSection, UiField};
use super::store::UiParentInfo;

#[derive(Debug, Serialize)]
pub struct EntityRefDep {
    /// Rust field name on the main entity (e.g., "tenant_id" or "parties").
    pub field_name: String,
    /// Key into `depIds`, unique across the whole dependency closure.
    pub dep_id: String,
    /// API path for the referenced entity (e.g., "/platform/tenant").
    pub api_path: String,
    /// Whether the reference is a junction array (array-of-entity-ref).
    pub is_array: bool,
}

/// One broken required-FK leg on a dependency step (cycle breaking).
///
/// Required-FK cycles no longer abort the `beforeAll` setup: the DFS breaks
/// the cycle at its last-discovered leg by omitting that FK from the create
/// body. If the API genuinely requires the FK, the create 4xxs and the
/// emitted spec throws with a message naming the cycle — a modeling error.
#[derive(Debug, Clone, Serialize)]
pub struct CycleBreakCtx {
    /// FK column omitted from this step's create body.
    pub fk: String,
    /// Entity title whose create omits the FK (the first-created participant).
    pub on_entity: String,
    /// The discovery path of the cycle, e.g. `Job -> Task -> Job`.
    pub cycle_path: String,
}

/// One entity creation step in the dependency `beforeAll`.
/// Steps are ordered leaf-first so an entity's FK deps already exist.
#[derive(Debug, Serialize)]
pub struct DependencyStep {
    /// Key into `depIds`.
    pub dep_id: String,
    /// API path for `createEntityAsAcme`.
    pub api_path: String,
    /// JS object body WITHOUT outer braces; excludes FK fields (see `fk_map`).
    pub fields_json: String,
    /// `[(fk_column, referenced_dep_id)]` — FK columns filled from created deps.
    pub fk_map: Vec<[String; 2]>,
    /// Whether this dep is referenced through a junction array.
    pub is_array: bool,
    /// True when the dependency entity has no `create` operation — its POST
    /// route does not exist (405). The emitted spec tolerates the failure
    /// (warn + leave the FK unset) because such rows may pre-exist via seed
    /// data; required-FK entities still fail their own create loudly.
    pub optional: bool,
    /// Required-FK legs omitted from `fk_map` to break a dependency cycle.
    /// Empty unless this step participates in a required-FK cycle.
    pub cycle_breaks: Vec<CycleBreakCtx>,
    /// Pre-joined `cycle_path`s of `cycle_breaks` (`"; "` separated) for the
    /// break-point failure message — the template engine has no `map` filter.
    pub cycle_break_paths: String,
}

/// Configuration for generated include E2E tests.
#[derive(Debug, Serialize)]
pub struct E2eIncludeConfig {
    /// Entity creation steps, ordered by dependency (deps first, main last).
    pub setup_steps: Vec<IncludeSetupStep>,
    /// depIds key of the main entity (last step).
    pub main_entity_id_ref: String,
    /// Include paths to test via get_by_id.
    pub test_paths: Vec<IncludeTestPath>,
    /// Whether multiple single-segment paths exist (for multi-include test).
    pub has_multi_include: bool,
    /// Whether to generate list-with-include tests.
    pub test_list_include: bool,
}

/// One entity creation step for include test setup.
#[derive(Debug, Serialize)]
pub struct IncludeSetupStep {
    /// depIds key, e.g. "person" or "candidate"
    pub dep_id: String,
    /// API path for createEntityAsAcme, e.g. "/api/common/person"
    pub api_path: String,
    /// JS object entries for required fields (without outer braces)
    pub fields_json: String,
    /// FK mappings: (field_on_this_entity, depId_of_target)
    pub fk_map: Vec<[String; 2]>,
}

/// An include path to test.
#[derive(Debug, Serialize)]
pub struct IncludeTestPath {
    /// Query parameter value, e.g. "person" or "deployment.position"
    pub alias: String,
    /// depIds key of the target (last segment) entity
    pub target_dep_id: String,
    /// Whether this is a dot-notation path
    pub is_dot_path: bool,
    /// Whether the relationship is 1:many
    pub is_array: bool,
}

#[derive(Debug, Serialize)]
pub struct UiE2eTestContext {
    pub entity_name: String,
    pub entity_label: String,
    pub module_name: String,
    pub domain: String,
    pub path_segment: String,
    pub has_create: bool,
    pub has_read: bool,
    pub has_update: bool,
    pub has_delete: bool,
    pub has_list: bool,
    pub has_workflow: bool,
    pub workflow_states: Vec<String>,
    pub initial_state: String,
    pub terminal_states: Vec<String>,
    pub fields: Vec<UiField>,
    pub create_fields: Vec<UiField>,
    pub required_create_fields: Vec<UiField>,
    pub update_fields: Vec<UiField>,
    pub first_list_column: Option<String>,
    pub has_fts: bool,
    pub fts_search_field: String,
    /// Main-entity required refs (consumed by `testData()`).
    pub entity_ref_deps: Vec<EntityRefDep>,
    /// Ordered leaf-first dependency creation steps (consumed by `beforeAll`).
    pub dependency_steps: Vec<DependencyStep>,
    /// Actionable messages when a required dep cannot be satisfied.
    pub dependency_errors: Vec<String>,
    pub has_entity_ref_deps: bool,
    /// Required `_id` columns the name convention claims that have NO
    /// creatable entity target (no matching schema, or the target has no
    /// create API). `testData` keeps their text sentinel and the payload
    /// templates emit `// fixture: convention ref '<field>' has no creatable
    /// target` next to it, naming the gap.
    pub convention_ref_gaps: Vec<String>,
    /// Child sections (child entities) displayed on the detail page
    pub child_sections: Vec<ChildSection>,
    pub has_child_sections: bool,
    /// Named path parameter for this entity's ID (e.g. `"worker_id"`).
    pub param_name: String,
    /// Set when this entity is a child nested under a parent.
    pub parent: Option<UiParentInfo>,
    /// JS object literal body for creating the parent entity via API (only set when parent is Some)
    pub parent_test_data_json: String,
    /// JS object literal for creating the grandparent entity (only set for depth-2 nesting)
    pub grandparent_test_data_json: String,
    /// Include E2E test configuration. None when include is not configured.
    pub e2e_include: Option<E2eIncludeConfig>,
    /// ux-rules list-rendering spec contract (issue #302). `None` when the
    /// `ux_rules` plane is inactive for this entity (flag off, plan-less,
    /// or no list/create output) — no `.ux.test.ts` file is emitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ux_spec: Option<UxE2eSpecCtx>,
    /// POM (Playwright Object Model) inputs (issue #316). Populated for
    /// every spec-emitting (any-op) entity — spec-infra gated, NOT
    /// ux-flag gated; `None` only when the entity emits no spec files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pom: Option<super::pom_ctx::PomCtx>,
}

/// One ux column slot mirrored into the `{seg}.ux.test.ts` spec (issue #302).
#[derive(Debug, Serialize)]
pub struct UxE2eColumnCtx {
    /// Response property the cell reads (the plan's column key).
    pub key: String,
    /// Resolved dimension, kebab-case.
    pub dimension: String,
    /// Resolved display, kebab-case.
    pub display: String,
    /// Column alignment: `left` or `right`.
    pub align: String,
    pub sortable: bool,
    pub truncate_tooltip: bool,
    /// 1-based `td` position in a body row — data columns render in
    /// `column_order` sequence and the actions cell trails them.
    pub td_index: usize,
    /// Audit stamp (`created_at`/`updated_at`/`deleted_at`/`*_by`) — these
    /// trail every other column in `column_order`.
    pub is_audit: bool,
}

/// A chip column with a known fixture label (the codelist's first value or
/// the workflow initial state). Boolean chip columns never match — they
/// render through the boolean Badge branch, which carries no chip testid.
#[derive(Debug, Serialize)]
pub struct UxE2eChipCheck {
    /// The exact chip text the fixture row renders.
    pub text: String,
}

/// The copy-chip column the clipboard/tooltip assertions read.
#[derive(Debug, Serialize)]
pub struct UxE2eCopyCheck {
    pub key: String,
    pub td_index: usize,
    /// JS expression evaluating to the full cell value on the fixture row
    /// (`createdId`, or a string literal). Random-valued fixtures (plain
    /// uuid literals) never qualify.
    pub expected_expr: String,
    /// The cell renders through a Tooltip (the trigger carries the chip).
    pub truncate_tooltip: bool,
}

/// One formatting assertion: the cell text must equal the SAME
/// `Intl.*Format` output the page computes for the fixture value.
#[derive(Debug, Serialize)]
pub struct UxE2eFormatCheck {
    /// `money` | `quantity` | `time-point` (drives the formatter).
    pub kind: String,
    pub td_index: usize,
    /// JS literal of the fixture value (a number, or a quoted instant).
    pub fixture_literal: String,
}

/// A right-aligned column: th and td must carry `text-right tabular-nums`.
#[derive(Debug, Serialize)]
pub struct UxE2eAlignCheck {
    pub key: String,
    pub td_index: usize,
    /// The th renders a sort button — the only per-column th hook.
    pub sortable: bool,
}

/// The list-sort assertions (issue #306 contract surfaced in the spec).
#[derive(Debug, Serialize)]
pub struct UxE2eSortCtx {
    /// Sortable keys in `column_order` sequence — the th order and the
    /// API allow-list must both follow it.
    pub fields: Vec<String>,
    /// The allow-list text the 400 message enumerates.
    pub valid_fields_text: String,
    /// First sortable column with two distinct stable fixture values —
    /// drives the `?sort=&order=` first-row flip assertion. `None` when no
    /// sortable column has a controlled fixture pair.
    pub flip: Option<UxE2eSortFlip>,
}

/// The asc/desc first-row flip fixture pair for one sortable column.
#[derive(Debug, Serialize)]
pub struct UxE2eSortFlip {
    pub key: String,
    pub td_index: usize,
    /// Distinct alternative fixture literal for the second row.
    pub alt_literal: String,
}

/// Row-action overflow assertions (menu-driven Delete + confirm-cancel).
#[derive(Debug, Serialize)]
pub struct UxE2eActionsCtx {
    /// The menu carries a Delete that requires confirmation — the
    /// dialog-cancel-keeps-the-row block is emitted.
    pub confirm_delete: bool,
}

/// The readable-lead assertion: the FIRST rendered column shows the
/// human-readable fixture text, not the system id (plan rows 17/25).
#[derive(Debug, Serialize)]
pub struct UxE2eFirstColumnCtx {
    pub key: String,
    /// Exact first-cell text on the fixture row (a Text/Raw column only —
    /// the readable-lead field by construction).
    pub expected_literal: String,
}

/// The ux-rules spec contract for one entity (issue #302).
///
/// Every assertion block in `ux.test.tera` is gated on one of these
/// fields being non-empty/`Some` — the spec contains only tests for
/// features the entity's list page actually renders.
#[derive(Debug, Serialize)]
pub struct UxE2eSpecCtx {
    /// Full `column_order` sequence (the table-mode header-count check).
    pub columns: Vec<UxE2eColumnCtx>,
    /// The collection renders as a table (chips/format/align/sort/zebra
    /// blocks are table-only — timelines render title + preview entries).
    pub has_table: bool,
    /// Timeline collection parameters (the timeline block).
    pub timeline: Option<super::page::UxTimelineCtx>,
    /// Timeline `order_by` fixture literal, when the field is fixture-
    /// controlled — the spec asserts its formatted date renders.
    pub timeline_order_fixture: Option<String>,
    pub actions: Option<UxE2eActionsCtx>,
    pub chip_checks: Vec<UxE2eChipCheck>,
    pub copy_check: Option<UxE2eCopyCheck>,
    pub format_checks: Vec<UxE2eFormatCheck>,
    pub align_checks: Vec<UxE2eAlignCheck>,
    pub sort: Option<UxE2eSortCtx>,
    pub first_column: Option<UxE2eFirstColumnCtx>,
    /// Alternate row shading is rendered (table mode).
    pub zebra: bool,
    /// BCP-47 locale mirror — the spec computes expected strings with the
    /// same `Intl` calls the page runs.
    pub locale: String,
    pub currency: Option<String>,
}
