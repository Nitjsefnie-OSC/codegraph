use crate::ProjectConfig;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;

use crate::error::Result;
use crate::render_template_with_project;
use crate::traits::{EntityGenerator, EntityGeneratorKind, GeneratedFile};
use codegraph_config::DomainConfig;

use super::common::{collect_child_sections, collect_ui_fields};
use super::form::UiFormContext;
use super::page::UiField;

pub struct CosmosEntityFormGenerator {
    output_dir: PathBuf,
}

impl CosmosEntityFormGenerator {
    pub fn new(output_dir: &Path) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
        }
    }
}

#[async_trait]
impl EntityGenerator for CosmosEntityFormGenerator {
    fn kind(&self) -> EntityGeneratorKind {
        EntityGeneratorKind::CosmosEntityForm
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
        let path_segment = schema.api_path_segment.clone();

        if module_name.is_empty() {
            return Ok(Vec::new());
        }

        let entity_cfg = config
            .domains
            .get(&domain)
            .and_then(|d| d.get_entity_config(&entity_name));

        let operations = entity_cfg
            .and_then(|ec| ec.operations.clone())
            .unwrap_or_else(|| config.defaults.operations.clone());

        let has_create = operations.contains(&"create".to_string());
        let has_update = operations.contains(&"update".to_string());

        if !has_create && !has_update {
            return Ok(Vec::new());
        }

        let dto_config = entity_cfg.map(|ec| &ec.dto);
        let immutable_fields: Vec<String> = dto_config
            .map(|d| d.immutable_fields.clone())
            .unwrap_or_default();

        let workflow = entity_cfg.and_then(|ec| ec.workflow.as_ref());
        let mut all_excluded: Vec<String> = immutable_fields.clone();
        if let Some(wf) = workflow {
            all_excluded.push(wf.status_field.clone());
            if let Some(ref approval_field) = wf.approval_status_field {
                all_excluded.push(approval_field.clone());
            }
        }

        let fields =
            collect_ui_fields(db, schema_title, &immutable_fields, Some(&domain), config).await?;

        let create_fields: Vec<UiField> = fields
            .iter()
            .filter(|f| !all_excluded.contains(&f.name))
            .cloned()
            .collect();

        let update_fields: Vec<UiField> = fields
            .iter()
            .filter(|f| !f.is_immutable && !all_excluded.contains(&f.name))
            .cloned()
            .collect();

        let child_sections = collect_child_sections(db, schema_title, config, &domain).await?;
        let has_child_sections = !child_sections.is_empty();

        let ctx = UiFormContext {
            tid: crate::ui::testids::for_module(&module_name),
            entity_name: entity_name.clone(),
            module_name: module_name.clone(),
            domain: domain.clone(),
            path_segment,
            fields,
            create_fields,
            update_fields,
            has_create,
            has_update,
            has_child_sections,
        };

        let content =
            render_template_with_project(tera, "ui/cosmos_entity_form.tera", &ctx, project)?;

        Ok(vec![GeneratedFile {
            path: self
                .output_dir
                .join("ui")
                .join("forms")
                .join(format!("{}_form.svelte", module_name)),
            content,
        }])
    }
}
