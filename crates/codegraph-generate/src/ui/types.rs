use std::collections::HashSet;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;
use serde::Serialize;

use super::common::collect_ui_fields;
use crate::GenerationEntry;
use crate::ProjectConfig;
use crate::api::api_model::resolve_entity_operations;
use crate::error::Result;
use crate::render_template_with_project;
use crate::traits::{GeneratedFile, GlobalGenerator, GlobalGeneratorKind};
use codegraph_config::DomainConfig;

#[derive(Debug, Serialize)]
pub struct UiTypesContext {
    pub entities: Vec<UiEntityType>,
    pub nested_types: Vec<UiNestedType>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UiNestedType {
    pub name: String,
    pub fields: Vec<UiTypeField>,
    /// The VO's display field for `#lib/format.ts`'s typed renderer
    /// (issue #468 W2): first readable name-ish string scalar, else None
    /// (the renderer falls back to the dynamic `formatFallback`).
    #[serde(default)]
    pub display_field: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UiEntityType {
    pub name: String,
    pub module_name: String,
    pub domain: String,
    pub response_fields: Vec<UiTypeField>,
    pub create_fields: Vec<UiTypeField>,
    pub update_fields: Vec<UiTypeField>,
    pub has_create: bool,
    pub has_update: bool,
    pub has_workflow: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UiTypeField {
    pub name: String,
    pub ts_type: String,
    pub is_required: bool,
    pub is_array: bool,
    pub description: String,
    /// When set, this field is a nested ValueObject referencing another type.
    /// The template uses this to add created_at/updated_at timestamps to the nested interface.
    #[serde(default)]
    pub nested_type_name: Option<String>,
}

pub struct UiTypeGenerator {
    output_dir: PathBuf,
}

impl UiTypeGenerator {
    pub fn new(output_dir: &Path) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
        }
    }
}

#[async_trait]
impl GlobalGenerator for UiTypeGenerator {
    fn kind(&self) -> GlobalGeneratorKind {
        GlobalGeneratorKind::UiTypes
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        config: &DomainConfig,
        generation_order: &[GenerationEntry],
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        let mut entities = Vec::new();

        for entry in generation_order {
            let schema = match db
                .get_schema_in_domain(&entry.schema_title, &entry.domain)
                .await?
            {
                Some(s) => s,
                None => continue,
            };

            let entity_name = schema.rust_type_name.clone();
            let module_name = schema.pg_table_name.clone();
            let domain = entry.domain.clone();

            if module_name.is_empty() {
                continue;
            }

            let entity_cfg = config
                .domains
                .get(&domain)
                .and_then(|d| d.get_entity_config(&entity_name));

            let operations = resolve_entity_operations(db, config, &domain, &entity_name).await;

            let dto_config = entity_cfg.map(|ec| &ec.dto);
            let immutable_fields: Vec<String> = dto_config
                .map(|d| d.immutable_fields.clone())
                .unwrap_or_default();

            let workflow = entity_cfg.and_then(|ec| ec.workflow.as_ref());
            let has_workflow = workflow
                .map(|wf| wf.generate_action_endpoints)
                .unwrap_or(false);

            let mut all_excluded: Vec<String> = immutable_fields.clone();
            if let Some(wf) = workflow {
                all_excluded.push(wf.status_field.clone());
                if let Some(ref approval_field) = wf.approval_status_field {
                    all_excluded.push(approval_field.clone());
                }
            }

            let ui_fields = collect_ui_fields(
                db,
                &entry.schema_title,
                &immutable_fields,
                Some(&domain),
                config,
            )
            .await?;
            let mut response_fields = Vec::new();
            let mut create_fields = Vec::new();
            let mut update_fields = Vec::new();

            for ui_field in &ui_fields {
                let type_field = UiTypeField {
                    name: ui_field.name.clone(),
                    ts_type: ui_field.ts_type.clone(),
                    is_required: ui_field.is_required,
                    is_array: ui_field.is_array,
                    description: ui_field.description.clone(),
                    nested_type_name: ui_field.nested_type_name.clone(),
                };

                response_fields.push(type_field.clone());

                if !all_excluded.contains(&ui_field.name) {
                    create_fields.push(type_field.clone());
                }

                if !ui_field.is_immutable && !all_excluded.contains(&ui_field.name) {
                    update_fields.push(type_field);
                }
            }

            entities.push(UiEntityType {
                name: entity_name,
                module_name,
                domain,
                response_fields,
                create_fields,
                update_fields,
                has_create: operations.contains(&"create".to_string()),
                has_update: operations.contains(&"update".to_string()),
                has_workflow,
            });
        }

        entities.sort_by(|a, b| a.name.cmp(&b.name));

        // Collect nested ValueObject type definitions.
        let mut visited_types = HashSet::new();
        let mut nested_types = Vec::new();
        for entity in &entities {
            for field in &entity.response_fields {
                if let Some(ref schema_title_for_ref) = field.nested_type_name {
                    if !visited_types.insert(schema_title_for_ref.clone()) {
                        continue;
                    }
                    // Resolve the nested type's TS interface name and fields.
                    let nested_ts_name = field.ts_type.clone();
                    if let Some(nt_fields) = collect_nested_type_fields(
                        db,
                        schema_title_for_ref,
                        Some(&entity.domain),
                        config,
                    )
                    .await
                    {
                        let display_field = resolve_vo_display_field(&nt_fields);
                        nested_types.push(UiNestedType {
                            name: nested_ts_name,
                            fields: nt_fields,
                            display_field,
                        });
                    }
                }
            }
        }

        let ctx = UiTypesContext {
            entities,
            nested_types,
        };
        let content = render_template_with_project(tera, "ui/scaffold/types.tera", &ctx, project)?;

        let format_content =
            render_template_with_project(tera, "ui/format.ts.tera", &ctx, project)?;

        Ok(vec![
            GeneratedFile {
                path: self
                    .output_dir
                    .join("ui")
                    .join("src")
                    .join("lib")
                    .join("api")
                    .join("types.ts"),
                content,
            },
            GeneratedFile {
                path: self
                    .output_dir
                    .join("ui")
                    .join("src")
                    .join("lib")
                    .join("format.ts"),
                content: format_content,
            },
        ])
    }
}

/// The `#lib/format.ts` display-field pick for one value object (issue #468
/// W2): first readable name-ish string scalar (skipping audit stamps and
/// timestamps), else None — the emitted renderer falls back to the dynamic
/// `formatFallback` so it stays total.
fn resolve_vo_display_field(fields: &[UiTypeField]) -> Option<String> {
    const AUDIT: [&str; 4] = ["id", "created_at", "updated_at", "deleted_at"];
    const PREFERRED: [&str; 6] = [
        "display_name",
        "full_name",
        "name",
        "title",
        "label",
        "code",
    ];
    for pref in PREFERRED {
        if let Some(f) = fields.iter().find(|f| f.name == pref)
            && !f.is_array
            && f.ts_type == "string"
        {
            return Some(f.name.clone());
        }
    }
    fields
        .iter()
        .find(|f| {
            !AUDIT.contains(&f.name.as_str())
                && !f.is_array
                && f.ts_type == "string"
                && !f.name.ends_with("_at")
        })
        .map(|f| f.name.clone())
}

/// Collect fields for a nested ValueObject type by querying its schema from the graph.
/// Returns `None` if the schema can't be resolved.
async fn collect_nested_type_fields(
    db: &dyn GraphQuerier,
    schema_title: &str,
    domain: Option<&str>,
    config: &DomainConfig,
) -> Option<Vec<UiTypeField>> {
    let fields = collect_ui_fields(db, schema_title, &[], domain, config)
        .await
        .ok()?;
    Some(
        fields
            .into_iter()
            .map(|f| UiTypeField {
                name: f.name,
                ts_type: f.ts_type,
                is_required: f.is_required,
                is_array: f.is_array,
                description: f.description,
                nested_type_name: f.nested_type_name,
            })
            .collect(),
    )
}
