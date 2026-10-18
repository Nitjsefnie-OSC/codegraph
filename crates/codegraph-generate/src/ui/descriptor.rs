use crate::ProjectConfig;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;
use serde::Serialize;

use crate::error::Result;
use crate::render_template_with_project;
use crate::traits::{EntityGenerator, EntityGeneratorKind, GeneratedFile};
use codegraph_config::DomainConfig;
use codegraph_config::config::{UiDomainConfig, UiOverrideConfig};

use crate::api::api_model::{resolve_entity_operations, resolve_path_segment};

use super::common::{collect_child_sections, collect_ui_fields, resolve_display_field};
use super::wizard_detect::{
    ChildInfo, WizardCandidate, detect_wizard_candidate, humanize, snake_case,
};

/// Extended field data for descriptor template rendering.
#[derive(Debug, Clone, Serialize)]
pub struct DescriptorField {
    // Core field data (from UiField)
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
    pub description: String,
    pub codelist_values: Vec<String>,

    // Descriptor-specific: grouping
    pub group: Option<String>,

    // Validation metadata
    pub has_validation: bool,
    pub validation_max_length: Option<u64>,
    pub validation_min_length: Option<u64>,
    pub validation_pattern: Option<String>,
    pub validation_minimum: Option<String>,
    pub validation_maximum: Option<String>,
    pub validation_format: Option<String>,

    // List visibility
    pub has_list: bool,
    pub is_sortable: bool,
    pub is_badge: bool,

    // Entity reference
    pub ref_entity: Option<String>,
    pub ref_domain: Option<String>,
    /// The referenced entity's resolved display field (`'id'` when none).
    pub ref_display_field: String,

    // Codelist
    pub codelist_source: String,
    pub codelist_name: Option<String>,

    // Overrides (from PropertyNode UI override fields)
    pub has_overrides: bool,
    pub override_detail: Option<String>,
    pub override_list_cell: Option<String>,
    pub override_form: Option<String>,
    pub override_inline: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DescriptorFieldGroup {
    pub name: String,
    pub label: String,
    pub collapsible: bool,
}

#[derive(Debug, Serialize)]
pub struct DescriptorChild {
    pub name: String,
    pub label: String,
    pub entity_name: String,
    pub relationship: String,
    pub inline: bool,
    /// The child entity's resolved display field (`None` falls back to
    /// `'id'` in the template — `displayField` stays a string for the
    /// `@crewbase/entities` consumer contract).
    pub display_field: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowTransitionDef {
    pub from: String,
    pub to: String,
    pub label: String,
    pub confirm: bool,
}

#[derive(Debug, Serialize)]
pub struct DescriptorContext {
    pub entity_name: String,
    pub domain: String,
    pub path_segment: String,
    pub operations: Vec<String>,
    pub has_fts: bool,
    pub fields: Vec<DescriptorField>,
    pub groups: Vec<DescriptorFieldGroup>,
    pub children: Vec<DescriptorChild>,
    pub has_workflow: bool,
    pub workflow_field: String,
    pub workflow_transitions: Vec<WorkflowTransitionDef>,
    pub wizard: Option<WizardCandidate>,
}

pub struct UiDescriptorGenerator {
    output_dir: PathBuf,
    ui_domains: UiDomainConfig,
}

impl UiDescriptorGenerator {
    pub fn new(
        output_dir: &Path,
        ui_overrides: UiOverrideConfig,
        ui_domains: UiDomainConfig,
    ) -> Self {
        let _ = ui_overrides; // no longer needed at generation time; resolved during ingestion
        Self {
            output_dir: output_dir.to_path_buf(),
            ui_domains,
        }
    }
}

#[async_trait]
impl EntityGenerator for UiDescriptorGenerator {
    fn kind(&self) -> EntityGeneratorKind {
        EntityGeneratorKind::UiDescriptor
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

        // Operations
        let operations = resolve_entity_operations(db, config, &domain, &entity_name).await;

        // FTS detection
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

        // DTO config
        let dto_config = entity_cfg.map(|ec| &ec.dto);
        let immutable_fields: Vec<String> = dto_config
            .map(|d| d.immutable_fields.clone())
            .unwrap_or_default();
        let list_include: Vec<String> = dto_config
            .map(|d| d.list_include.clone())
            .unwrap_or_default();
        let dto_groups = dto_config.map(|d| d.groups.clone()).unwrap_or_default();

        // Collect base UI fields
        let ui_fields =
            collect_ui_fields(db, schema_title, &immutable_fields, Some(&domain), config).await?;

        // Get raw properties for validation metadata and UI override data
        let properties = db.get_properties(schema_title).await?;

        // Entity-ref fields: resolve the target's schema TITLE via the graph
        // (identical across the JSON and mox pipelines — raw `ref_target`
        // strings are pipeline-specific: "WorkerType.json#" vs "WorkerType").
        // Keyed by UI field name so the field closure stays synchronous.
        let mut ref_entities: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        for f in ui_fields.iter().filter(|f| f.is_entity_ref) {
            let prop = properties.iter().find(|p| {
                p.rust_field_name == f.name
                    || p.name == f.name
                    || codegraph_core::types::resolve_field(p).rust_field_name == f.name
            });
            let resolved = match prop {
                Some(p) => {
                    let graph_target = if p.is_array {
                        db.get_array_item_schema(&p.name, schema_title).await
                    } else {
                        db.get_property_ref_target(&p.name, schema_title).await
                    }
                    .ok()
                    .flatten();
                    match graph_target {
                        Some(target) => Some(target.title.clone()),
                        // No graph edge — normalize the raw $ref stem.
                        None => p.ref_target.as_ref().map(|t| {
                            let last = t.rsplit('/').next().unwrap_or(t.as_str());
                            last.strip_suffix(".json#")
                                .or_else(|| last.strip_suffix(".json"))
                                .unwrap_or(last)
                                .to_string()
                        }),
                    }
                }
                None => None,
            };
            ref_entities.insert(f.name.clone(), resolved);
        }

        // Resolve the display field of each distinct entity-ref target once:
        // `displayField` tells the runtime how to label a referenced record,
        // so it must come from the TARGET's fields, not a hardcoded
        // 'display_name'. Unresolvable targets fall back to 'id'.
        let mut ref_display_fields: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut ref_target_titles: Vec<&String> = ref_entities.values().flatten().collect();
        ref_target_titles.sort();
        ref_target_titles.dedup();
        for title in ref_target_titles {
            let target_fields = collect_ui_fields(db, title, &[], None, config)
                .await
                .unwrap_or_default();
            let resolved = resolve_display_field(&target_fields).unwrap_or_else(|| "id".into());
            ref_display_fields.insert(title.clone(), resolved);
        }

        // Build descriptor fields with validation + overrides
        let fields: Vec<DescriptorField> = ui_fields
            .iter()
            .map(|f| {
                // Find matching PropertyNode for validation data. Entity-ref
                // UI fields carry the resolved `_id` name while the raw
                // property keeps the authored name — match through
                // `resolve_field` too (issue: JSON/mox descriptor parity).
                let prop = properties.iter().find(|p| {
                    p.rust_field_name == f.name
                        || p.name == f.name
                        || codegraph_core::types::resolve_field(p).rust_field_name == f.name
                });

                // Determine group from dto.groups config
                let group = dto_groups
                    .iter()
                    .find(|(_, fields)| fields.contains(&f.name))
                    .map(|(group_name, _)| group_name.clone());

                // Validation from PropertyNode
                let max_length = prop.and_then(|p| p.max_length);
                let min_length = prop.and_then(|p| p.min_length);
                let pattern = prop.and_then(|p| p.pattern.clone());
                let minimum = prop.and_then(|p| p.minimum.map(|d| d.to_string()));
                let maximum = prop.and_then(|p| p.maximum.map(|d| d.to_string()));
                let format = prop.and_then(|p| p.format.clone());
                let has_validation = max_length.is_some()
                    || min_length.is_some()
                    || pattern.is_some()
                    || minimum.is_some()
                    || maximum.is_some()
                    || format.is_some();

                // List visibility
                let in_list = if !list_include.is_empty() {
                    list_include.contains(&f.name)
                } else {
                    true // default: all visible if no include list
                };

                // Overrides from PropertyNode (single resolution point — set during ingestion)
                let (
                    has_overrides,
                    override_detail,
                    override_list_cell,
                    override_form,
                    override_inline,
                ) = {
                    let has = prop
                        .map(|p| {
                            p.ui_override_detail.is_some()
                                || p.ui_override_list_cell.is_some()
                                || p.ui_override_form.is_some()
                                || p.ui_override_inline.is_some()
                        })
                        .unwrap_or(false);
                    (
                        has,
                        prop.and_then(|p| p.ui_override_detail.clone()),
                        prop.and_then(|p| p.ui_override_list_cell.clone()),
                        prop.and_then(|p| p.ui_override_form.clone()),
                        prop.and_then(|p| p.ui_override_inline.clone()),
                    )
                };

                DescriptorField {
                    name: f.name.clone(),
                    label: f.label.clone(),
                    ts_type: f.ts_type.clone(),
                    input_type: f.input_type.clone(),
                    is_required: f.is_required,
                    is_array: f.is_array,
                    is_entity_ref: f.is_entity_ref,
                    is_immutable: f.is_immutable,
                    is_codelist: f.is_codelist,
                    is_range: f.is_range,
                    description: f.description.clone(),
                    codelist_values: f.codelist_values.clone(),
                    group,
                    has_validation,
                    validation_max_length: max_length,
                    validation_min_length: min_length,
                    validation_pattern: pattern,
                    validation_minimum: minimum,
                    validation_maximum: maximum,
                    validation_format: format,
                    has_list: in_list,
                    is_sortable: f.is_required, // sortable if required (heuristic)
                    is_badge: f.is_codelist,    // badge for codelist fields
                    ref_entity: if f.is_entity_ref {
                        ref_entities.get(&f.name).cloned().flatten()
                    } else {
                        None
                    },
                    ref_domain: if f.is_entity_ref {
                        Some(domain.clone())
                    } else {
                        None
                    },
                    ref_display_field: if f.is_entity_ref {
                        ref_entities
                            .get(&f.name)
                            .cloned()
                            .flatten()
                            .and_then(|t| ref_display_fields.get(&t).cloned())
                            .unwrap_or_else(|| "id".into())
                    } else {
                        "id".into()
                    },
                    codelist_source: if f.codelist_values.is_empty() {
                        "codelist".into()
                    } else {
                        "inline".into()
                    },
                    codelist_name: if f.is_codelist && f.codelist_values.is_empty() {
                        prop.and_then(|p| p.ref_target.clone())
                    } else {
                        None
                    },
                    has_overrides,
                    override_detail,
                    override_list_cell,
                    override_form,
                    override_inline,
                }
            })
            .collect();

        // Groups from dto.groups
        let groups: Vec<DescriptorFieldGroup> = dto_groups
            .keys()
            .map(|name| DescriptorFieldGroup {
                name: name.clone(),
                label: humanize(name),
                collapsible: name != "default",
            })
            .collect();

        // Children
        let child_sections = collect_child_sections(db, schema_title, config, &domain).await?;
        let children: Vec<DescriptorChild> = child_sections
            .iter()
            .map(|cs| DescriptorChild {
                name: snake_case(&cs.entity_name),
                label: cs.label.clone(),
                entity_name: cs.entity_name.clone(),
                relationship: "one-to-many".into(),
                inline: false,
                display_field: cs.display_field.clone(),
            })
            .collect();

        // Workflow
        let workflow_config = entity_cfg.and_then(|ec| ec.workflow.as_ref());
        let has_workflow = workflow_config
            .map(|wf| wf.generate_action_endpoints)
            .unwrap_or(false);
        let workflow_field = workflow_config
            .map(|w| w.status_field.clone())
            .unwrap_or_default();
        let workflow_transitions: Vec<WorkflowTransitionDef> = workflow_config
            .map(|w| {
                // `transitions` is a TOML map (HashMap): iterate deterministically
                // so generated descriptors are byte-stable for identical config.
                let mut transitions: Vec<WorkflowTransitionDef> = w
                    .transitions
                    .iter()
                    .flat_map(|(from, tos)| {
                        tos.iter().map(move |to| WorkflowTransitionDef {
                            from: from.clone(),
                            to: to.clone(),
                            label: humanize(to),
                            confirm: true,
                        })
                    })
                    .collect();
                transitions.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
                transitions
            })
            .unwrap_or_default();

        // Wizard detection: build ChildInfo from child sections + graph queries
        let mut child_infos = Vec::new();
        for c in &children {
            let referencing = db
                .get_referencing_schemas(&c.entity_name)
                .await
                .unwrap_or_default();
            let is_tightly_coupled = referencing.len() <= 1;
            let child_children = db
                .get_child_schemas(&c.entity_name)
                .await
                .unwrap_or_default();
            child_infos.push(ChildInfo {
                name: c.entity_name.clone(),
                relationship: c.relationship.clone(),
                is_tightly_coupled,
                has_own_children: !child_children.is_empty(),
            });
        }
        let field_group_names: Vec<String> = if groups.is_empty() {
            vec!["default".into()]
        } else {
            groups.iter().map(|g| g.name.clone()).collect()
        };
        let ui_entity_override = self.ui_domains.get_entity(&domain, schema_title);
        let wizard = detect_wizard_candidate(
            schema_title,
            &child_infos,
            &field_group_names,
            ui_entity_override,
        );

        let context = DescriptorContext {
            entity_name,
            domain: domain.clone(),
            path_segment: path_segment.clone(),
            operations,
            has_fts,
            fields,
            groups,
            children,
            has_workflow,
            workflow_field,
            workflow_transitions,
            wizard,
        };

        let content = render_template_with_project(tera, "ui/descriptor.tera", &context, project)?;

        let path = self
            .output_dir
            .join("ui")
            .join("src")
            .join("routes")
            .join(&domain)
            .join(&path_segment)
            .join("descriptor.ts");

        Ok(vec![GeneratedFile { path, content }])
    }
}
