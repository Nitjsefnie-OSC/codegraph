use crate::ProjectConfig;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;
use serde::Serialize;

use crate::api::api_model::{
    resolve_entity_operations, resolve_path_segment, resolve_path_segment_with_config,
};
use crate::error::Result;
use crate::render_template_with_project;
use crate::traits::{EntityGenerator, EntityGeneratorKind, GeneratedFile};
use crate::ux::plan::{ActionSpec, RowAction, build_ux_plan};
use codegraph_config::DomainConfig;
use codegraph_config::ux::{Align, Display};

use super::common::{collect_child_sections, collect_ui_fields};
use super::store::UiParentInfo;

#[derive(Debug, Serialize)]
pub struct UiPageContext {
    pub entity_name: String,
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
    pub has_approval_status: bool,
    pub has_fts: bool,
    pub fields: Vec<UiField>,
    pub list_fields: Vec<UiField>,
    pub child_sections: Vec<ChildSection>,
    pub has_child_sections: bool,
    /// Named path parameter for this entity's ID (e.g. `"worker_id"`).
    pub param_name: String,
    /// Set when this entity is a child nested under a parent.
    pub parent: Option<UiParentInfo>,
    /// Config-driven extension points mounted on the detail page
    /// (`ui_detail_extensions`, #162 phase 3). Empty = no extension blocks.
    pub detail_extensions: Vec<UiDetailExtension>,
    /// ux-rules resolved list columns, in `column_order` sequence (issue
    /// #297). Empty = no resolved columns for this entity (e.g. codelists);
    /// the plane may still be active, so the field is always serialized
    /// when the context exists — a `for` over an absent variable is a
    /// render error, while an empty one falls through to the template's
    /// `list_fields` fallback.
    pub ux_columns: Vec<UxColumnCtx>,
    /// ux-rules row-action partition (present only with a plan). Serialized
    /// as null (not omitted) when there is no plan: tera 2 hard-errors on
    /// absent-variable dereferences like `{% if ux_actions.child_menu %}`
    /// where tera 1 treated them as falsy, and a null root keeps that
    /// falsy behavior byte-identical.
    pub ux_actions: Option<UxActionsCtx>,
    /// ux-rules locale/visual baseline (present only with a plan). Null —
    /// not omitted — when there is no plan, for the same tera 2 reason as
    /// `ux_actions`.
    pub ux: Option<UxSettingsCtx>,
    /// ux-rules list-sort contract (issue #306): the `?sort=` allow-list,
    /// present only when the plan exposes at least one sortable column and
    /// the collection renders as a table. Skipped from the serialized
    /// context otherwise, so timeline/flag-off renders stay byte-identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ux_sort: Option<UxSortCtx>,
    /// This entity's testid vocabulary, projected from `ux::plan::ids`
    /// (issue #468 W3). Templates interpolate `tid.*` instead of carrying
    /// literals, so page markup renders FROM the canonical table.
    pub tid: crate::ui::testids::TidMap,
    /// Unprefixed shared testids (`confirm-dialog`, workflow panel).
    pub shared_tid: crate::ui::testids::SharedTids,
}

/// One consumer-owned panel mounted on a generated detail page.
#[derive(Debug, Clone, Serialize)]
pub struct UiDetailExtension {
    /// kebab-case name from config (e.g. "ird-registration-panel") — used
    /// for the conventional `data-testid`.
    pub name: String,
    /// PascalCase component name (e.g. "IrdRegistrationPanel").
    pub component: String,
}

/// A sub-field definition for a StructuredWrapper type, embedded in UiField
/// at generation time. Consumed by the StructuredWrapperField Svelte component.
#[derive(Debug, Clone, Serialize)]
pub struct UiSubField {
    /// camelCase name from JSON schema (e.g. "schemeId")
    pub name: String,
    /// snake_case name (e.g. "scheme_id")
    pub snake_name: String,
    /// Human-readable label (e.g. "Scheme ID")
    pub label: String,
    pub is_required: bool,
    pub description: String,
    /// True for required fields and the first optional field (index < 2 in ordered result).
    /// Drives which sub-fields are visible before the expand toggle in the UI.
    pub show_by_default: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UiField {
    pub name: String,
    pub label: String,
    pub ts_type: String,
    pub input_type: String,
    pub is_required: bool,
    pub is_array: bool,
    pub is_entity_ref: bool,
    pub is_immutable: bool,
    pub is_codelist: bool,
    pub is_range: bool,
    pub codelist_values: Vec<String>,
    pub description: String,
    /// The Postgres column type (e.g., "TEXT", "TSTZRANGE", "TEXT[]").
    pub pg_type: String,
    /// Whether the range end is open (unbounded upper bound).
    pub open_end: bool,
    /// For entity_ref fields: the full API path prefix (e.g., "/common/organization")
    pub ref_api_path: Option<String>,
    /// Non-empty when this field is a StructuredWrapper (e.g. IdentifierType).
    /// Contains sub-field definitions queried from the graph at generation time.
    pub structured_sub_fields: Vec<UiSubField>,
    /// When set, this field is a nested ValueObject referencing another type.
    /// The `ts_type` holds the nested interface name (e.g. "WorkerPersonLegalResponse").
    #[serde(default)]
    pub nested_type_name: Option<String>,
}

/// ux-rules resolved column slot for the list template (issue #297).
///
/// Populated only when the `ux_rules` plane is active; the field is skipped
/// from the serialized context otherwise, so flag-off renders stay
/// byte-identical. Entries follow `plan.column_order` (readable identifier
/// first, audit stamps last).
#[derive(Debug, Clone, Serialize)]
pub struct UxColumnCtx {
    /// Field name — the response property the cell reads.
    pub key: String,
    /// Resolved dimension, kebab-case (`money`, `time-point`, ...).
    pub dimension: String,
    /// Resolved display, kebab-case (`raw`, `chip`, `copy-chip`, `link`).
    pub display: String,
    /// Column alignment: `left` or `right`.
    pub align: String,
    /// Status keyword → badge variant for chip rendering.
    pub tone: BTreeMap<String, String>,
    /// The cell truncates and surfaces the full value through a Tooltip.
    pub truncate_tooltip: bool,
    /// The column is sortable in tables — the header renders a sort
    /// button and the list endpoint accepts `?sort=<key>` (issue #306).
    pub sortable: bool,
}

/// One row action for the list template (issue #297).
#[derive(Debug, Clone, Serialize)]
pub struct UxActionCtx {
    /// Lowercase action name (`open` | `edit` | `delete`).
    pub action: String,
    /// Human-facing fallback label from the plan.
    pub label: String,
}

/// Row-action partition for the list template (issue #297): the plan's
/// primary/menu split plus the confirmation set, filtered to the entity's
/// enabled operations.
#[derive(Debug, Clone, Serialize)]
pub struct UxActionsCtx {
    /// Actions rendered inline in the actions cell.
    pub primary: Vec<UxActionCtx>,
    /// Actions collapsed into the overflow menu.
    pub menu: Vec<UxActionCtx>,
    /// Lowercase action names that require user confirmation.
    pub confirm: Vec<String>,
    /// Convenience flag: `delete` is in `confirm` (drives the AlertDialog
    /// branch in the template).
    pub confirm_delete: bool,
    /// Actions collapsed into CHILD-SECTION item menus (issue #299): the
    /// plan's Edit/Delete pair whichever side of the row partition they
    /// sit on, still filtered to the entity's enabled operations — child
    /// items always tier their secondary actions behind the per-item menu
    /// while the `Manage →` link stays the inline affordance. Empty ⇒ the
    /// child section keeps the pre-#299 flat buttons.
    pub child_menu: Vec<UxActionCtx>,
}

/// Locale/visual baseline for the list template (issue #297).
#[derive(Debug, Clone, Serialize)]
pub struct UxSettingsCtx {
    /// BCP-47 locale for `Intl` number/date formatting.
    pub locale: String,
    /// ISO-4217 currency code for money formatting (`None` = plain decimals).
    pub currency: Option<String>,
    /// Alternate row shading.
    pub zebra: bool,
    /// Shade soft-deleted / workflow-terminal rows.
    pub inactive_shading: bool,
    /// Row-cell vertical alignment: `center` or `top`.
    pub vertical_align: String,
    /// Soft-delete marker field (e.g. `deleted_at`), when the entity has one.
    pub soft_delete_field: Option<String>,
    /// The workflow status field, when a workflow is configured.
    pub workflow_status_field: Option<String>,
    /// Timeline collection layout (issue #298), resolved from
    /// `CollectionPlan::Timeline`. `None` = the default table layout; the
    /// key is skipped from the serialized context entirely, so table-mode
    /// and flag-off contexts stay byte-identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeline: Option<UxTimelineCtx>,
}

/// Timeline collection parameters for the list template (issue #298).
///
/// Only ever produced by an explicit `[[collection]] display = "timeline"`
/// rule — tables never morph (the plan heuristic suggests instead).
#[derive(Debug, Clone, Serialize)]
pub struct UxTimelineCtx {
    /// Datetime field the timeline sorts by (rendered newest first).
    pub order_by: String,
    /// Title field of each entry; the template falls back to `id` when
    /// the plan could not resolve one.
    pub title_field: Option<String>,
    /// Extra fields shown on each entry, rendered through the shared
    /// cell formatter (chips, money, ...).
    pub preview: Vec<String>,
}

/// List-sort contract for the list template (issue #306): the `?sort=`
/// allow-list derived from the plan, present only when at least one
/// column is sortable and the collection renders as a table.
#[derive(Debug, Clone, Serialize)]
pub struct UxSortCtx {
    /// Sortable field keys, in `column_order` sequence — the same values
    /// the list endpoint's allow-list validates against.
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChildSection {
    pub entity_name: String,
    pub module_name: String,
    pub label: String,
    pub path_segment: String,
    pub domain: String,
    pub has_children: bool,
    pub display_field: Option<String>,
    pub fields: Vec<UiField>,
    /// The child entity's testid vocabulary (issue #468 W3).
    pub tid: crate::ui::testids::TidMap,
}

pub struct UiPageGenerator {
    output_dir: PathBuf,
    parent_candidates: Vec<codegraph_core::types::ParentCandidate>,
    /// ux-rules diagnostic lines already reported this run — dedupes the
    /// per-entity warning channel so a run-wide finding (e.g. the
    /// `[actions] inline_max` accounting) prints once, not per entity.
    reported_ux_lines: Mutex<std::collections::HashSet<String>>,
}

impl UiPageGenerator {
    pub fn new(output_dir: &Path) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
            parent_candidates: Vec::new(),
            reported_ux_lines: Mutex::new(std::collections::HashSet::new()),
        }
    }

    pub fn with_parent_candidates(
        mut self,
        candidates: Vec<codegraph_core::types::ParentCandidate>,
    ) -> Self {
        self.parent_candidates = candidates;
        self
    }

    /// Print advisory ux-rules diagnostic lines through the generator
    /// warning channel (stderr, mirroring the other generators), deduped
    /// per run. Quiet when the plan applies cleanly.
    fn report_ux_diagnostics(&self, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        let mut seen = self
            .reported_ux_lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for line in lines {
            if seen.insert(line.clone()) {
                eprintln!("warning: ux-rules: {line}");
            }
        }
    }
}

#[async_trait]
impl EntityGenerator for UiPageGenerator {
    fn kind(&self) -> EntityGeneratorKind {
        EntityGeneratorKind::UiPage
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        schema_title: &str,
        domain: &str,
        config: &DomainConfig,
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        let schema = db
            .get_schema_in_domain(schema_title, domain)
            .await?
            .ok_or_else(|| crate::error::Error::SchemaNotFound(schema_title.into()))?;

        let entity_name = schema.rust_type_name.clone();
        let module_name = schema.pg_table_name.clone();
        let domain = domain.to_string();
        if module_name.is_empty() {
            return Ok(Vec::new());
        }

        let entity_cfg = config
            .domains
            .get(&domain)
            .and_then(|d| d.get_entity_config(&entity_name));

        let path_segment = resolve_path_segment(entity_cfg, &schema);

        let operations = resolve_entity_operations(db, config, &domain, &entity_name).await;

        let dto_config = entity_cfg.map(|ec| &ec.dto);
        let immutable_fields: Vec<String> = dto_config
            .map(|d| d.immutable_fields.clone())
            .unwrap_or_default();
        let list_exclude: Vec<String> = dto_config
            .map(|d| d.list_exclude.clone())
            .unwrap_or_default();
        let list_include: Vec<String> = dto_config
            .map(|d| d.list_include.clone())
            .unwrap_or_default();

        let workflow = entity_cfg.and_then(|ec| ec.workflow.as_ref());
        let has_workflow = workflow
            .map(|wf| wf.generate_action_endpoints)
            .unwrap_or(false);
        let workflow_states = workflow.map(|wf| wf.states.clone()).unwrap_or_default();
        let initial_state = workflow
            .map(|wf| wf.initial_state.clone())
            .unwrap_or_default();
        let terminal_states = workflow
            .map(|wf| wf.terminal_states.clone())
            .unwrap_or_default();
        let has_approval_status = workflow
            .and_then(|wf| wf.approval_status_field.as_ref())
            .is_some();

        let has_fts = entity_cfg
            .map(|ec| {
                !ec.search.fts_weights.is_empty()
                    || ec
                        .search
                        .fts_columns
                        .as_ref()
                        .map(|c| !c.is_empty())
                        .unwrap_or(false)
            })
            .unwrap_or(false);

        let fields =
            collect_ui_fields(db, schema_title, &immutable_fields, Some(&domain), config).await?;

        // Build list fields: if list_include is set, use those; otherwise all fields minus list_exclude
        let list_fields: Vec<UiField> = if !list_include.is_empty() {
            fields
                .iter()
                .filter(|f| list_include.contains(&f.name))
                .cloned()
                .collect()
        } else {
            fields
                .iter()
                .filter(|f| !list_exclude.contains(&f.name))
                .cloned()
                .collect()
        };

        let child_sections = collect_child_sections(db, schema_title, config, &domain).await?;
        let has_child_sections = !child_sections.is_empty();

        // ux-rules list contract (issue #297): resolve the plan over the
        // collected list fields when the plane is active, then map it onto
        // the template context. No rules ⇒ defaults keep output identical.
        let (ux_columns, ux_actions, ux, ux_sort, ux_diag) = resolve_ux_context(
            db,
            config,
            project,
            &UxEntityInfo {
                schema_title,
                domain: &domain,
                list_fields: &list_fields,
                operations: &operations,
                user_pinned_list_order: !list_include.is_empty() || !list_exclude.is_empty(),
            },
        )
        .await?;
        self.report_ux_diagnostics(&ux_diag);

        // Resolve parent info for child entities.
        // Manual config takes priority over graph detection.
        let parent =
            resolve_parent_info(&self.parent_candidates, db, config, &domain, schema_title).await;

        let detail_extensions = entity_cfg
            .map(|ec| {
                ec.ui_detail_extensions
                    .iter()
                    .map(|name| UiDetailExtension {
                        name: name.clone(),
                        component: codegraph_naming::to_pascal_case(name),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let ctx = UiPageContext {
            tid: crate::ui::testids::for_module(&module_name),
            entity_name,
            module_name,
            domain: domain.clone(),
            path_segment: path_segment.clone(),
            has_create: operations.contains(&"create".to_string()),
            has_read: operations.contains(&"read".to_string()),
            has_update: operations.contains(&"update".to_string()),
            has_delete: operations.contains(&"delete".to_string()),
            has_list: operations.contains(&"list".to_string()),
            has_workflow,
            workflow_states,
            initial_state,
            terminal_states,
            has_approval_status,
            has_fts,
            fields,
            list_fields,
            child_sections,
            has_child_sections,
            param_name: crate::api::router::param_name_from_path_segment(&path_segment),
            parent: parent.clone(),
            detail_extensions,
            ux_columns,
            ux_actions,
            ux,
            ux_sort,
            shared_tid: crate::ui::testids::shared(),
        };

        let routes_base = self
            .output_dir
            .join("ui")
            .join("src")
            .join("routes")
            .join("(app)");
        let routes_dir = routes_dir_for(&routes_base, parent.as_ref(), &domain, &path_segment);

        emit_ui_pages(tera, &ctx, project, &routes_dir)
    }
}

/// Build the ux-rules list-render context for one entity (issue #297).
///
/// When the `ux_rules` plane is inactive (`project.ux` is `None`) the
/// defaults keep every ux key out of the serialized context, so the
/// rendered list page stays byte-identical. Otherwise [`build_ux_plan`]
/// resolves the per-column contract over the collected [`UiField`]s and
/// the plan maps onto template-shaped context data:
///
/// - `ux_columns` follows `plan.column_order` (readable identifier first,
///   audit stamps last — or input order when the author pinned it),
/// - actions are filtered to the entity's enabled operations (`Open`
///   needs `read`, `Edit` `update`, `Delete` `delete`),
/// - inactive-shading inputs (soft-delete marker field, workflow status
///   field) ride along so the template can mark rows,
/// - advisory diagnostics come back for the caller's warning channel.

// Entity facts the ux context resolver needs, bundled to keep the
// resolver's signature small.
#[derive(Clone, Copy)]
struct UxEntityInfo<'a> {
    schema_title: &'a str,
    domain: &'a str,
    list_fields: &'a [UiField],
    operations: &'a [String],
    user_pinned_list_order: bool,
}

async fn resolve_ux_context(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    project: &ProjectConfig,
    info: &UxEntityInfo<'_>,
) -> Result<(
    Vec<UxColumnCtx>,
    Option<UxActionsCtx>,
    Option<UxSettingsCtx>,
    Option<UxSortCtx>,
    Vec<String>,
)> {
    let UxEntityInfo {
        schema_title,
        domain,
        list_fields,
        operations,
        user_pinned_list_order,
    } = *info;
    let Some(rules) = project.ux.ux.as_ref() else {
        return Ok((Vec::new(), None, None, None, Vec::new()));
    };

    // Shared plan-input collection (issue #306): the same assembly feeds
    // the page context and the API-side sort allow-list, so the rendered
    // sort buttons and the endpoint's validation can never disagree.
    let plan_ctx = crate::ux::sort::collect_ux_plan_context(
        db,
        config,
        schema_title,
        domain,
        list_fields.to_vec(),
        user_pinned_list_order,
    )
    .await?;
    let soft_delete_field = plan_ctx.soft_delete_field().map(str::to_string);
    let workflow_status_field = plan_ctx.workflow_status_field().map(str::to_string);
    let input = plan_ctx.plan_input(schema_title);

    let plan = match build_ux_plan(Some(rules), &input)? {
        Some(plan) => plan,
        None => return Ok((Vec::new(), None, None, None, Vec::new())),
    };

    let diag = crate::ux::diagnostics::collect_diagnostics(rules, &input, &plan);
    let diag_lines = crate::ux::diagnostics::report(&diag);

    let mut ux_columns = Vec::with_capacity(plan.column_order.len());
    for name in &plan.column_order {
        let Some(col) = plan.columns.get(name) else {
            continue;
        };
        ux_columns.push(UxColumnCtx {
            key: name.clone(),
            dimension: col.dimension.as_str().to_string(),
            display: match col.display {
                Display::Raw => "raw",
                Display::Chip => "chip",
                Display::CopyChip => "copy-chip",
                Display::Link => "link",
            }
            .to_string(),
            align: match col.align {
                Align::Left => "left",
                Align::Right => "right",
            }
            .to_string(),
            tone: col.tone.0.clone(),
            truncate_tooltip: col.truncate_tooltip,
            sortable: col.sortable,
        });
    }

    // Actions are real UI affordances: an action an entity cannot serve
    // (no `update` op, for instance) renders a dead route — drop it.
    let action_allowed = |action: RowAction| match action {
        RowAction::Open => operations.iter().any(|op| op == "read"),
        RowAction::Edit => operations.iter().any(|op| op == "update"),
        RowAction::Delete => operations.iter().any(|op| op == "delete"),
    };
    let map_action = |spec: &ActionSpec| UxActionCtx {
        action: spec.action.as_str().to_string(),
        label: spec.label.clone(),
    };
    let primary: Vec<UxActionCtx> = plan
        .actions
        .primary
        .iter()
        .filter(|spec| action_allowed(spec.action))
        .map(map_action)
        .collect();
    let menu: Vec<UxActionCtx> = plan
        .actions
        .menu
        .iter()
        .filter(|spec| action_allowed(spec.action))
        .map(map_action)
        .collect();
    let confirm_delete = plan.actions.confirm.iter().any(|c| c == "delete");
    // Issue #299: child sections collapse Edit/Delete behind the per-item
    // menu regardless of the row partition (the Manage link owns the
    // inline slot); canonical order keeps Edit before Delete.
    let child_menu: Vec<UxActionCtx> = primary
        .iter()
        .chain(menu.iter())
        .filter(|spec| matches!(spec.action.as_str(), "edit" | "delete"))
        .cloned()
        .collect();
    let ux_actions = if primary.is_empty() && menu.is_empty() {
        None
    } else {
        Some(UxActionsCtx {
            primary,
            menu,
            confirm: plan.actions.confirm.clone(),
            confirm_delete,
            child_menu,
        })
    };

    let timeline = match &plan.collection {
        crate::ux::plan::CollectionPlan::Timeline {
            order_by,
            title_field,
            preview,
        } => Some(UxTimelineCtx {
            order_by: order_by.clone(),
            title_field: title_field.clone(),
            preview: preview.clone(),
        }),
        crate::ux::plan::CollectionPlan::Table => None,
    };

    // Issue #306: table layouts expose the plan's sortable columns through
    // the `?sort=` allow-list; timelines keep their fixed DESC order (a
    // ledger item — sort UI is deliberately table-only in v1).
    let ux_sort = match plan.collection {
        crate::ux::plan::CollectionPlan::Timeline { .. } => None,
        crate::ux::plan::CollectionPlan::Table => {
            let sort = crate::ux::sort::sort_plan_from_plan(&plan);
            if sort.is_empty() {
                None
            } else {
                Some(UxSortCtx {
                    fields: sort.fields,
                })
            }
        }
    };

    let ux = UxSettingsCtx {
        locale: plan.format.locale.clone(),
        currency: plan.format.currency.clone(),
        zebra: plan.visuals.zebra,
        inactive_shading: plan.visuals.inactive_shading,
        vertical_align: match plan.visuals.vertical_align {
            crate::ux::plan::VerticalAlign::Center => "center",
            crate::ux::plan::VerticalAlign::Top => "top",
        }
        .to_string(),
        soft_delete_field,
        workflow_status_field,
        timeline,
    };

    Ok((ux_columns, ux_actions, Some(ux), ux_sort, diag_lines))
}

async fn resolve_parent_info(
    parent_candidates: &[codegraph_core::types::ParentCandidate],
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    domain: &str,
    schema_title: &str,
) -> Option<UiParentInfo> {
    let stripped = crate::api::router::strip_suffix(schema_title, &config.defaults.type_suffix);
    let mut result = None;

    // 1. Check manual config first
    if let Some(ec) = config
        .domains
        .get(domain)
        .and_then(|d| d.get_entity_config(schema_title))
        && ec.role.as_deref() == Some("child")
        && let Some(ref parent_title) = ec.parent
        && let Ok(Some(parent_schema)) = db.get_schema_in_domain(parent_title, domain).await
    {
        let parent_domain = if config
            .domains
            .get(domain)
            .map(|d| d.entities.contains(parent_title))
            .unwrap_or(false)
        {
            domain.to_string()
        } else {
            parent_schema
                .domain
                .clone()
                .unwrap_or_else(|| domain.to_string())
        };
        let gp =
            super::store::resolve_grandparent(parent_title, domain, config, parent_candidates, db)
                .await
                .map(Box::new);
        result = Some(UiParentInfo {
            param_name: crate::api::router::param_name_from_path_segment(
                &resolve_path_segment_with_config(None, &parent_schema, config),
            ),
            domain: parent_domain,
            path_segment: resolve_path_segment_with_config(None, &parent_schema, config),
            module_name: parent_schema.pg_table_name.clone(),
            entity_name: parent_schema.rust_type_name.clone(),
            grandparent: gp,
        });
    }

    // 2. Fall back to graph parent_candidates (only if entity is not explicitly root)
    let page_effective_role = config
        .domains
        .get(domain)
        .and_then(|d| d.get_entity_config(schema_title))
        .and_then(|ec| ec.role.as_deref())
        .unwrap_or("root");
    if result.is_none() && page_effective_role != "root" {
        for pc in parent_candidates {
            let child_name =
                crate::api::router::strip_suffix(&pc.child_title, &config.defaults.type_suffix);
            if child_name == stripped {
                let in_explicit = config
                    .domains
                    .get(domain)
                    .map(|d| d.entities.contains(&pc.parent_title))
                    .unwrap_or(false);
                let parent_in_domain = in_explicit
                    || db
                        .get_schema_in_domain(&pc.parent_title, domain)
                        .await
                        .ok()
                        .flatten()
                        .and_then(|s| s.domain.as_ref().map(|d| *d == domain))
                        .unwrap_or(false);
                if !parent_in_domain {
                    break;
                }
                if let Ok(Some(parent_schema)) =
                    db.get_schema_in_domain(&pc.parent_title, domain).await
                {
                    let gp = super::store::resolve_grandparent(
                        &pc.parent_title,
                        domain,
                        config,
                        parent_candidates,
                        db,
                    )
                    .await
                    .map(Box::new);
                    result = Some(UiParentInfo {
                        param_name: crate::api::router::param_name_from_path_segment(
                            &resolve_path_segment_with_config(None, &parent_schema, config),
                        ),
                        domain: domain.to_string(),
                        path_segment: resolve_path_segment_with_config(
                            None,
                            &parent_schema,
                            config,
                        ),
                        module_name: parent_schema.pg_table_name.clone(),
                        entity_name: parent_schema.rust_type_name.clone(),
                        grandparent: gp,
                    });
                }
                break;
            }
        }
    }
    result
}

fn routes_dir_for(
    routes_base: &Path,
    parent: Option<&UiParentInfo>,
    domain: &str,
    path_segment: &str,
) -> PathBuf {
    if let Some(p) = parent {
        if let Some(ref gp) = p.grandparent {
            // Depth-2: grandparent/[gp_param]/parent/[parent_param]/child
            routes_base
                .join(&gp.domain)
                .join(&gp.path_segment)
                .join(format!("[{}]", gp.param_name))
                .join(&p.path_segment)
                .join(format!("[{}]", p.param_name))
                .join(path_segment)
        } else {
            routes_base
                .join(&p.domain)
                .join(&p.path_segment)
                .join(format!("[{}]", p.param_name))
                .join(path_segment)
        }
    } else {
        routes_base.join(domain).join(path_segment)
    }
}

fn emit_ui_pages(
    tera: &tera::Tera,
    ctx: &UiPageContext,
    project: &ProjectConfig,
    routes_dir: &Path,
) -> Result<Vec<GeneratedFile>> {
    let mut files = Vec::new();

    // List page
    if ctx.has_list {
        let content = render_template_with_project(tera, "ui/list_page.tera", ctx, project)?;
        files.push(GeneratedFile {
            path: routes_dir.join("+page.svelte"),
            content,
        });
        let load = render_template_with_project(tera, "ui/list_load.tera", ctx, project)?;
        files.push(GeneratedFile {
            path: routes_dir.join("+page.server.ts"),
            content: load,
        });
    }

    // Detail page
    if ctx.has_read {
        let content = render_template_with_project(tera, "ui/detail_page.tera", ctx, project)?;
        files.push(GeneratedFile {
            path: routes_dir
                .join(format!("[{}]", ctx.param_name))
                .join("+page.svelte"),
            content,
        });
        let load = render_template_with_project(tera, "ui/detail_load.tera", ctx, project)?;
        files.push(GeneratedFile {
            path: routes_dir
                .join(format!("[{}]", ctx.param_name))
                .join("+page.server.ts"),
            content: load,
        });
    }

    // Create page
    if ctx.has_create {
        let content = render_template_with_project(tera, "ui/form_page.tera", ctx, project)?;
        files.push(GeneratedFile {
            path: routes_dir.join("new").join("+page.svelte"),
            content,
        });
    }

    // Edit page
    if ctx.has_update {
        let content = render_template_with_project(tera, "ui/edit_page.tera", ctx, project)?;
        files.push(GeneratedFile {
            path: routes_dir
                .join(format!("[{}]", ctx.param_name))
                .join("edit")
                .join("+page.svelte"),
            content,
        });
        let load = render_template_with_project(tera, "ui/edit_load.tera", ctx, project)?;
        files.push(GeneratedFile {
            path: routes_dir
                .join(format!("[{}]", ctx.param_name))
                .join("edit")
                .join("+page.server.ts"),
            content: load,
        });
    }

    Ok(files)
}

#[cfg(test)]
mod ux_list_template_tests {
    use super::*;
    use serde_json::json;

    fn base_ctx() -> serde_json::Value {
        let mut ctx = json!({
            "entity_name": "Task",
            "module_name": "task",
            "domain": "common",
            "path_segment": "tasks",
            "param_name": "task_id",
            "has_list": true,
            "has_create": true,
            "has_read": true,
            "has_update": true,
            "has_delete": true,
            "has_workflow": false,
            "has_fts": false,
            "list_fields": [{"name": "name", "label": "Name"}],
            "terminal_states": ["archived"],
            "parent": null,
            // Production serializes these as null when the ux plane is
            // inactive (tera 2 errors on absent-variable dereferences).
            "ux_columns": [],
            "ux_actions": null,
            "ux": null,
        });
        // The testid vocabulary rides the context like production renders
        // it (issue #468 W3): projected from `ux::plan::ids` for the module.
        let obj = ctx.as_object_mut().unwrap();
        obj.insert(
            "tid".into(),
            serde_json::to_value(crate::ui::testids::for_module("task")).unwrap(),
        );
        obj.insert(
            "shared_tid".into(),
            serde_json::to_value(crate::ui::testids::shared()).unwrap(),
        );
        ctx
    }

    fn render(ctx: &serde_json::Value) -> String {
        let tera = crate::template_engine::create_tera(std::path::Path::new("."))
            .expect("embedded templates");
        crate::render_template_with_project(
            &tera,
            "ui/list_page.tera",
            ctx,
            &ProjectConfig::default(),
        )
        .expect("list page renders")
    }

    /// Flag OFF (no ux keys in the context): none of the ux markup may
    /// leak, and the pre-#297 list-page shape is intact.
    #[test]
    fn flag_off_list_page_has_no_ux_markup() {
        let out = render(&base_ctx());
        for needle in [
            "-chip",
            "-actions",
            "-copy",
            "Intl.NumberFormat",
            "Intl.DateTimeFormat",
            "data-inactive",
            "Tooltip",
            "DropdownMenu",
            "AlertDialog",
            "invalidateAll",
            "rowClass",
            "tabular-nums",
        ] {
            assert!(
                !out.contains(needle),
                "flag-off output must not contain {needle:?}:\n{out}"
            );
        }
        // The pre-existing behaviors stay byte-shaped.
        assert!(
            out.contains("class=\"cursor-pointer hover:bg-muted/50\""),
            "{out}"
        );
        assert!(
            out.contains("<Table.Head>{col.label}</Table.Head>"),
            "{out}"
        );
        assert!(out.contains("task-table"), "{out}");
        assert!(out.contains("isBooleanField"), "{out}");
    }

    /// Flag ON: every #297 content pin is present in the rendered page.
    #[test]
    fn flag_on_list_page_carries_ux_pins() {
        let mut ctx = base_ctx();
        let obj = ctx.as_object_mut().unwrap();
        obj.insert("ux_columns".into(), json!([
            {"key": "name", "dimension": "text", "display": "raw", "align": "left", "tone": {}, "truncate_tooltip": false, "sortable": true},
            {"key": "total_amount", "dimension": "money", "display": "raw", "align": "right", "tone": {}, "truncate_tooltip": false, "sortable": true},
            {"key": "status", "dimension": "status-category", "display": "chip", "align": "left", "tone": {"active": "default", "draft": "secondary"}, "truncate_tooltip": false, "sortable": false},
            {"key": "id", "dimension": "identifier", "display": "copy-chip", "align": "left", "tone": {}, "truncate_tooltip": true, "sortable": false}
        ]));
        obj.insert("ux_actions".into(), json!({
            "primary": [{"action": "open", "label": "Open"}],
            "menu": [{"action": "edit", "label": "Edit"}, {"action": "delete", "label": "Delete"}],
            "confirm": ["delete"],
            "confirm_delete": true
        }));
        obj.insert(
            "ux".into(),
            json!({
                "locale": "en-NZ",
                "currency": "NZD",
                "zebra": true,
                "inactive_shading": true,
                "vertical_align": "center",
                "soft_delete_field": "deleted_at",
                "workflow_status_field": null
            }),
        );
        obj.insert(
            "ux_sort".into(),
            json!({"fields": ["name", "total_amount"]}),
        );

        let out = render(&ctx);
        // Chips with tone variants.
        assert!(out.contains(r#"data-testid="task-chip""#), "{out}");
        assert!(out.contains("variant={toneFor(col, value)}"), "{out}");
        // Numeric/money alignment on header AND cells.
        assert_eq!(
            out.matches("text-right tabular-nums").count(),
            2,
            "one header + one cell occurrence:\n{out}"
        );
        // Localized money/number/date formatting.
        assert!(out.contains("Intl.NumberFormat"), "{out}");
        assert!(out.contains("currency: 'NZD'"), "{out}");
        assert!(
            out.contains(
                "Intl.DateTimeFormat('en-NZ', { dateStyle: 'medium', timeStyle: 'short' })"
            ),
            "{out}"
        );
        // Copy chips + row actions dropdown + confirm-guarded delete.
        assert!(out.contains(r#"data-testid="task-copy""#), "{out}");
        assert!(out.contains(r#"data-testid="task-actions""#), "{out}");
        assert!(out.contains(r#"data-testid="task-actions-menu""#), "{out}");
        assert!(out.contains("AlertDialog"), "{out}");
        assert!(
            out.contains(r#"data-testid="task-delete-dialog""#),
            "{out}"
        );
        // Tooltip wrapping for truncated cells.
        assert!(out.contains("Tooltip.Root"), "{out}");
        // Zebra rows + inactive-row branch.
        assert!(out.contains("bg-muted/50"), "{out}");
        assert!(out.contains("data-inactive="), "{out}");
        assert!(
            out.contains("if (row['deleted_at'] != null) return true;"),
            "{out}"
        );
        // Readable-first column order: `name` leads, `id` trails.
        let name_pos = out.find("key: 'name'").expect("name column");
        let id_pos = out.find("key: 'id'").expect("id column");
        assert!(
            name_pos < id_pos,
            "readable-first column order violated:\n{out}"
        );
    }

    /// Column-header sorting (issue #306): sortable headers render as
    /// toggle buttons with `aria-sort` on the `th` and a direction
    /// indicator; sort state threads through pagination/search (page
    /// resets, `q` preserved); non-sortable headers keep the plain label.
    #[test]
    fn flag_on_sortable_headers_render_toggle_buttons_with_aria_sort() {
        let mut ctx = base_ctx();
        let obj = ctx.as_object_mut().unwrap();
        obj.insert("ux_columns".into(), json!([
            {"key": "name", "dimension": "text", "display": "raw", "align": "left", "tone": {}, "truncate_tooltip": false, "sortable": true},
            {"key": "status", "dimension": "status-category", "display": "chip", "align": "left", "tone": {}, "truncate_tooltip": false, "sortable": false}
        ]));
        obj.insert(
            "ux".into(),
            json!({
                "locale": "en-NZ", "currency": null, "zebra": false,
                "inactive_shading": false, "vertical_align": "center",
                "soft_delete_field": null, "workflow_status_field": null
            }),
        );
        obj.insert("ux_sort".into(), json!({"fields": ["name"]}));

        let out = render(&ctx);

        // aria-sort on the th, driven by the runtime sort state.
        assert!(
            out.contains("aria-sort={col.sortable ? sortStateFor(col.key) : undefined}"),
            "{out}"
        );
        // Toggle button per sortable column with a stable testid, native
        // button semantics, and the direction indicator.
        assert!(
            out.contains(r#"data-testid="task-sort-{col.key}""#),
            "{out}"
        );
        assert!(out.contains("onclick={() => toggleSort(col.key)}"), "{out}");
        assert!(out.contains("function sortIndicator("), "{out}");
        assert!(
            out.contains("'ascending' ? '▲' : state === 'descending' ? '▼' : ''"),
            "{out}"
        );
        // Asc → desc → none ladder (none drops the params = default order).
        assert!(out.contains("function toggleSort("), "{out}");
        assert!(out.contains("params.set('sort', key);"), "{out}");
        assert!(out.contains("params.set('order', 'asc');"), "{out}");
        assert!(out.contains("params.set('order', 'desc');"), "{out}");
        // FTS query preserved across sort toggles.
        assert!(
            out.contains("if (searchQuery) params.set('q', searchQuery);"),
            "{out}"
        );
        // Sort state survives pagination.
        assert!(out.contains("function withSort("), "{out}");
        assert!(out.contains("withSort(params);"), "{out}");
        // Non-sortable header keeps the plain label branch.
        assert!(out.contains("{:else}"), "{out}");
    }

    /// Without a `ux_sort` context (no sortable columns, timeline, or the
    /// flag off) no sort markup leaks.
    #[test]
    fn no_sort_context_renders_no_sort_markup() {
        let mut ctx = base_ctx();
        let obj = ctx.as_object_mut().unwrap();
        obj.insert("ux_columns".into(), json!([
            {"key": "status", "dimension": "status-category", "display": "chip", "align": "left", "tone": {}, "truncate_tooltip": false, "sortable": false}
        ]));
        obj.insert(
            "ux".into(),
            json!({
                "locale": "en-NZ", "currency": null, "zebra": false,
                "inactive_shading": false, "vertical_align": "center",
                "soft_delete_field": null, "workflow_status_field": null
            }),
        );
        let out = render(&ctx);
        for needle in [
            "aria-sort",
            "toggleSort",
            "sortIndicator",
            "withSort",
            "sortStateFor",
            "-sort-",
        ] {
            assert!(
                !out.contains(needle),
                "no-sort output must not contain {needle:?}:\n{out}"
            );
        }
    }

    /// The Flag dimension keeps the runtime boolean Badge (no regression
    /// vs today's `isBooleanField` path): boolean values hit the badge
    /// branch before the chip branch.
    #[test]
    fn flag_on_boolean_badge_branch_precedes_chips() {
        let mut ctx = base_ctx();
        let obj = ctx.as_object_mut().unwrap();
        obj.insert("ux_columns".into(), json!([
            {"key": "active", "dimension": "flag", "display": "chip", "align": "left", "tone": {}, "truncate_tooltip": false, "sortable": false}
        ]));
        obj.insert(
            "ux".into(),
            json!({
                "locale": "en-NZ", "currency": null, "zebra": false,
                "inactive_shading": false, "vertical_align": "center",
                "soft_delete_field": null, "workflow_status_field": null
            }),
        );
        let out = render(&ctx);
        let boolean_branch = out.find("isBooleanField(value)").expect("boolean branch");
        let chip_branch = out.find("col.display === 'chip'").expect("chip branch");
        assert!(
            boolean_branch < chip_branch,
            "booleans must render through the isBooleanField Badge branch"
        );
    }

    /// Currency unset (`None`): money formats as a plain locale decimal.
    #[test]
    fn flag_on_money_without_currency_uses_plain_decimal() {
        let mut ctx = base_ctx();
        let obj = ctx.as_object_mut().unwrap();
        obj.insert("ux_columns".into(), json!([
            {"key": "total_amount", "dimension": "money", "display": "raw", "align": "right", "tone": {}, "truncate_tooltip": false, "sortable": true}
        ]));
        obj.insert(
            "ux".into(),
            json!({
                "locale": "en-NZ", "currency": null, "zebra": false,
                "inactive_shading": false, "vertical_align": "center",
                "soft_delete_field": null, "workflow_status_field": null
            }),
        );
        let out = render(&ctx);
        assert!(
            out.contains("const moneyFmt = new Intl.NumberFormat('en-NZ');"),
            "{out}"
        );
        assert!(!out.contains("style: 'currency'"), "{out}");
    }

    /// Timeline ctx (issue #298): the dispatcher swaps the table for the
    /// timeline layout while the shared load logic (search input, create
    /// button, empty state, pagination) stays put.
    #[test]
    fn flag_on_timeline_ctx_swaps_table_for_timeline() {
        let mut ctx = base_ctx();
        let obj = ctx.as_object_mut().unwrap();
        obj.insert("ux_columns".into(), json!([
            {"key": "name", "dimension": "text", "display": "raw", "align": "left", "tone": {}, "truncate_tooltip": false, "sortable": true},
            {"key": "total_amount", "dimension": "money", "display": "raw", "align": "right", "tone": {}, "truncate_tooltip": false, "sortable": true},
            {"key": "status", "dimension": "status-category", "display": "chip", "align": "left", "tone": {}, "truncate_tooltip": false, "sortable": false},
            {"key": "created_at", "dimension": "time-point", "display": "raw", "align": "left", "tone": {}, "truncate_tooltip": false, "sortable": true}
        ]));
        obj.insert("ux_actions".into(), json!({
            "primary": [{"action": "open", "label": "Open"}],
            "menu": [{"action": "edit", "label": "Edit"}, {"action": "delete", "label": "Delete"}],
            "confirm": ["delete"],
            "confirm_delete": true
        }));
        obj.insert(
            "ux".into(),
            json!({
                "locale": "en-NZ", "currency": "NZD", "zebra": true,
                "inactive_shading": true, "vertical_align": "center",
                "soft_delete_field": "deleted_at", "workflow_status_field": null,
                "timeline": {
                    "order_by": "created_at",
                    "title_field": "name",
                    "preview": ["status", "total_amount"]
                }
            }),
        );
        let out = render(&ctx);

        // Timeline markup with the pinned testids.
        assert!(out.contains(r#"data-testid="task-timeline""#), "{out}");
        assert!(out.contains(r#"data-testid="task-timeline-item""#), "{out}");
        // Order field formatted through the shared Intl helper.
        assert!(out.contains("function formatDate("), "{out}");
        assert!(
            out.contains("Intl.DateTimeFormat('en-NZ', { dateStyle: 'medium' })"),
            "{out}"
        );
        assert!(out.contains("const orderKey = 'created_at';"), "{out}");
        // DESC sort of the shared rows.
        assert!(out.contains("[...displayRows].sort"), "{out}");
        assert!(out.contains("? 1 : av > bv ? -1 : 0"), "{out}");
        // Title link over the entity base path.
        assert!(out.contains("const titleKey = 'name';"), "{out}");
        assert!(out.contains("href={`${basePath}/${row['id']}`}"), "{out}");
        // Preview fields render through the shared cell formatter.
        assert!(
            out.contains("const previewKeys = ['status', 'total_amount'];"),
            "{out}"
        );
        assert!(out.contains("timeline-meta"), "{out}");
        assert!(out.contains(r#"data-testid="task-chip""#), "{out}");
        assert!(out.contains("formatMoney(value)"), "{out}");
        // Row actions dropdown + confirm-guarded delete.
        assert!(out.contains(r#"data-testid="task-actions""#), "{out}");
        assert!(out.contains(r#"data-testid="task-actions-menu""#), "{out}");
        assert!(
            out.contains(r#"data-testid="task-delete-dialog""#),
            "{out}"
        );
        // Table markup absent in timeline mode.
        assert!(!out.contains(r#"data-testid="task-table""#), "{out}");
        assert!(!out.contains("<Table.Root"), "{out}");
        assert!(!out.contains("Table.Head"), "{out}");
        assert!(!out.contains("task-timeline-meta-list"), "{out}");

        // Shared load logic survives: search, create, empty state,
        // pagination, FTS-free filtering.
        assert!(out.contains(r#"data-testid="task-search""#), "{out}");
        assert!(out.contains(r#"data-testid="task-create-btn""#), "{out}");
        assert!(out.contains(r#"data-testid="task-empty""#), "{out}");
        assert!(out.contains(r#"data-testid="task-pagination""#), "{out}");
    }

    /// Table mode with the flag ON (no `timeline` key in `ux`): the
    /// timeline layout, helpers and styles never render.
    #[test]
    fn flag_on_table_mode_has_no_timeline_markup() {
        let mut ctx = base_ctx();
        let obj = ctx.as_object_mut().unwrap();
        obj.insert("ux_columns".into(), json!([
            {"key": "created_at", "dimension": "time-point", "display": "raw", "align": "left", "tone": {}, "truncate_tooltip": false, "sortable": false}
        ]));
        obj.insert(
            "ux".into(),
            json!({
                "locale": "en-NZ", "currency": null, "zebra": true,
                "inactive_shading": false, "vertical_align": "center",
                "soft_delete_field": null, "workflow_status_field": null
            }),
        );
        let out = render(&ctx);
        for needle in [
            "task-timeline",
            "timeline-item",
            "timeline-meta",
            "timeline-rail",
            "function formatDate(",
            "const orderKey",
            "titleKey",
            "previewKeys",
            "displayRowsSorted",
            "workflowVariant",
        ] {
            assert!(
                !out.contains(needle),
                "table-mode output must not contain {needle:?}:\n{out}"
            );
        }
        assert!(out.contains(r#"data-testid="task-table""#), "{out}");
    }

    /// Workflow on a timeline entity: the badge renders with the panel's
    /// variant ladder against the configured status field.
    #[test]
    fn flag_on_timeline_workflow_badge_branch_compiles() {
        let mut ctx = base_ctx();
        let obj = ctx.as_object_mut().unwrap();
        obj.insert("has_workflow".into(), json!(true));
        obj.insert("ux_columns".into(), json!([
            {"key": "created_at", "dimension": "time-point", "display": "raw", "align": "left", "tone": {}, "truncate_tooltip": false, "sortable": false}
        ]));
        obj.insert(
            "ux".into(),
            json!({
                "locale": "en-NZ", "currency": null, "zebra": false,
                "inactive_shading": true, "vertical_align": "center",
                "soft_delete_field": "deleted_at",
                "workflow_status_field": "status",
                "timeline": {
                    "order_by": "created_at",
                    "title_field": null,
                    "preview": []
                }
            }),
        );
        let out = render(&ctx);
        assert!(out.contains("function workflowVariant("), "{out}");
        assert!(
            out.contains(r#"data-testid="task-workflow-state""#),
            "{out}"
        );
        assert!(out.contains("terminalStates.includes(state)"), "{out}");
    }
}

/// Detail-page child-section template pins (issue #299): the per-item
/// actions menu + confirmation dialog under the parent's ux plan, and the
/// byte-identity golden for the flag-off render.
#[cfg(test)]
mod ux_child_section_template_tests {
    use super::*;
    use serde_json::json;

    /// One child section attached to a `TaskType` detail page.
    fn child_sections() -> serde_json::Value {
        let mut section = json!([{
            "entity_name": "SubTaskType",
            "module_name": "sub_task",
            "label": "Sub Task",
            "path_segment": "sub-task",
            "domain": "common",
            "has_children": true,
            "fields": []
        }]);
        // Per-section testid vocabulary, like collect_child_sections.
        section[0].as_object_mut().unwrap().insert(
            "tid".into(),
            serde_json::to_value(crate::ui::testids::for_module("sub_task")).unwrap(),
        );
        section
    }

    /// Detail-page context with `ux` keys present only when `ux` is true.
    fn detail_ctx(ux: bool) -> serde_json::Value {
        let mut ctx = json!({
            "entity_name": "TaskType",
            "module_name": "task",
            "domain": "common",
            "path_segment": "task",
            "param_name": "task_id",
            "has_create": true,
            "has_read": true,
            "has_update": true,
            "has_delete": true,
            "has_list": true,
            "has_workflow": false,
            "workflow_states": [],
            "initial_state": "",
            "terminal_states": [],
            "has_approval_status": false,
            "has_fts": false,
            "fields": [],
            "list_fields": [],
            "child_sections": child_sections(),
            "has_child_sections": true,
            "parent": null,
            "detail_extensions": [],
            // Null (not absent) mirrors UiPageContext's flag-off serialization.
            "ux_actions": null,
            "ux": null,
        });
        {
            let obj = ctx.as_object_mut().unwrap();
            obj.insert(
                "tid".into(),
                serde_json::to_value(crate::ui::testids::for_module("task")).unwrap(),
            );
            obj.insert(
                "shared_tid".into(),
                serde_json::to_value(crate::ui::testids::shared()).unwrap(),
            );
        }
        if ux {
            let obj = ctx.as_object_mut().unwrap();
            obj.insert(
                "ux_actions".into(),
                json!({
                    "primary": [{"action": "open", "label": "Open"}],
                    "menu": [{"action": "edit", "label": "Edit"}, {"action": "delete", "label": "Delete"}],
                    "confirm": ["delete"],
                    "confirm_delete": true,
                    "child_menu": [{"action": "edit", "label": "Edit"}, {"action": "delete", "label": "Delete"}]
                }),
            );
        }
        ctx
    }

    fn render_detail(ctx: &serde_json::Value) -> String {
        let tera = crate::template_engine::create_tera(std::path::Path::new("."))
            .expect("embedded templates");
        crate::render_template_with_project(
            &tera,
            "ui/detail_page.tera",
            ctx,
            &ProjectConfig::default(),
        )
        .expect("detail page renders")
    }

    /// Byte-identity golden: the flag-off child-section detail render is
    /// byte-for-byte the pre-#299 output (captured from the HEAD templates
    /// before the child-section changes; see the testdata file header).
    #[test]
    fn flag_off_child_section_detail_render_is_byte_identical() {
        let golden = include_str!("testdata/pre299_flag_off_detail_render.txt");
        let out = render_detail(&detail_ctx(false));
        assert_eq!(out, golden, "flag-off detail render drifted from pre-#299");
    }

    /// Flag-off: no ux markup leaks into the child section, and the flat
    /// Edit/Delete buttons plus the inline Manage link stay.
    #[test]
    fn flag_off_child_section_keeps_flat_buttons() {
        let out = render_detail(&detail_ctx(false));
        for needle in [
            "DropdownMenu",
            "sub_task-actions",
            "sub_task-action-edit",
            "sub_task-action-delete",
            "sub_task-delete-dialog",
            "DeleteId",
            "confirmDelete",
        ] {
            assert!(
                !out.contains(needle),
                "flag-off output has {needle:?}:\n{out}"
            );
        }
        assert!(
            out.contains(r#"onclick={() => editChild('common', 'sub-task', child.id)}"#),
            "{out}"
        );
        assert!(
            out.contains(r#"onclick={() => deleteChild('common', 'sub-task', child.id)}"#),
            "{out}"
        );
        assert!(
            out.contains(r#"data-testid="manage-sub_task-btn""#),
            "{out}"
        );
    }

    /// Flag-on: Edit/Delete tier into the per-item actions menu, Delete
    /// sits behind the plan's AlertDialog confirmation, and the Manage →
    /// link remains the inline affordance. The detail HEADER Edit/Delete
    /// buttons stay primary (never moved into a menu).
    #[test]
    fn flag_on_child_section_tiers_actions_behind_menu() {
        let out = render_detail(&detail_ctx(true));

        // Per-item menu trigger + content testids.
        assert!(out.contains(r#"data-testid="sub_task-actions""#), "{out}");
        assert!(
            out.contains(r#"data-testid="sub_task-actions-menu""#),
            "{out}"
        );
        assert!(
            out.contains(r#"data-testid="sub_task-action-edit""#),
            "{out}"
        );
        assert!(
            out.contains(r#"data-testid="sub_task-action-delete""#),
            "{out}"
        );
        // Menu items keep the exact legacy Edit/Delete behaviors.
        assert!(
            out.contains(r#"onclick={() => editChild('common', 'sub-task', child.id)}"#),
            "{out}"
        );
        assert!(
            out.contains("onclick={() => sub_taskDeleteId = child.id}"),
            "{out}"
        );
        // Delete behind the per-section AlertDialog confirm.
        assert!(out.contains("DropdownMenu.Root"), "{out}");
        assert!(out.contains("open={sub_taskDeleteId !== null}"), "{out}");
        assert!(out.contains("if (!v) sub_taskDeleteId = null;"), "{out}");
        assert!(
            out.contains(r#"data-testid="sub_task-delete-dialog""#),
            "{out}"
        );
        assert!(
            out.contains(r#"data-testid="sub_task-delete-dialog-confirm""#),
            "{out}"
        );
        assert!(
            out.contains("let sub_taskDeleteId = $state<string | null>(null);"),
            "{out}"
        );
        assert!(
            out.contains("function confirmDeleteSubTaskTypeChild() {"),
            "{out}"
        );
        assert!(
            out.contains("onclick={confirmDeleteSubTaskTypeChild}"),
            "{out}"
        );
        assert!(
            out.contains("void deleteChild('common', 'sub-task', childId);"),
            "{out}"
        );
        // The flat legacy buttons are gone.
        assert!(
            !out.contains("onclick={() => deleteChild('common', 'sub-task', child.id)}"),
            "{out}"
        );
        // The Manage → link stays inline (primary affordance).
        assert!(
            out.contains(r#"data-testid="manage-sub_task-btn""#),
            "{out}"
        );
        assert!(out.contains("Manage →"), "{out}");
        assert!(
            out.contains(
                "import * as DropdownMenu from '#lib/components/ui/dropdown-menu/index.js';"
            ),
            "{out}"
        );

        // The detail HEADER keeps its primary Edit/Delete buttons.
        assert!(out.contains(r#"data-testid="task-edit-btn""#), "{out}");
        assert!(out.contains(r#"data-testid="task-delete-btn""#), "{out}");
        assert!(
            out.contains(r#"onclick={() => deleteDialogOpen = true}"#),
            "{out}"
        );
        // The header delete opens the header dialog, not the child one.
        let header_delete = out.find(r#"task-delete-btn"#).unwrap();
        let child_menu = out.find(r#"sub_task-actions""#).unwrap();
        assert!(
            header_delete < child_menu,
            "header stays the primary surface"
        );
    }

    /// Non-confirming plans keep the direct delete: with `confirm` empty
    /// the menu item calls deleteChild immediately and no child dialog is
    /// emitted.
    #[test]
    fn flag_on_non_confirm_delete_stays_direct() {
        let mut ctx = detail_ctx(true);
        ctx.as_object_mut().unwrap().insert(
            "ux_actions".into(),
            json!({
                "primary": [{"action": "open", "label": "Open"}],
                "menu": [{"action": "edit", "label": "Edit"}, {"action": "delete", "label": "Delete"}],
                "confirm": [],
                "confirm_delete": false,
                "child_menu": [{"action": "edit", "label": "Edit"}, {"action": "delete", "label": "Delete"}]
            }),
        );
        let out = render_detail(&ctx);
        assert!(
            out.contains(r#"onclick={() =>  void deleteChild('common', 'sub-task', child.id)}"#,),
            "{out}"
        );
        assert!(!out.contains("sub_task-delete-dialog"), "{out}");
        assert!(!out.contains("DeleteId = $state"), "{out}");
    }

    /// Empty `child_menu` (the parent's ops cannot serve edit/delete)
    /// falls back to the flat legacy buttons rather than emitting a dead
    /// empty menu.
    #[test]
    fn flag_on_empty_child_menu_falls_back_to_flat_buttons() {
        let mut ctx = detail_ctx(true);
        ctx.as_object_mut().unwrap().insert(
            "ux_actions".into(),
            json!({
                "primary": [{"action": "open", "label": "Open"}],
                "menu": [],
                "confirm": [],
                "confirm_delete": false,
                "child_menu": []
            }),
        );
        let out = render_detail(&ctx);
        assert!(
            out.contains(r#"onclick={() => editChild('common', 'sub-task', child.id)}"#),
            "{out}"
        );
        assert!(!out.contains("DropdownMenu.Root"), "{out}");
    }
}
