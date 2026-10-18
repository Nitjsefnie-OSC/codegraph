use crate::ProjectConfig;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;
use serde::Serialize;

use crate::error::Result;
use crate::render_template_with_project;
use crate::traits::{EntityGenerator, EntityGeneratorKind, GeneratedFile};
use codegraph_config::DomainConfig;
use codegraph_core::types::PropertyNode;
use codegraph_core::types::resolve_field;
use codegraph_type_contracts::RefClassificationKind;

use crate::api::api_model::{resolve_entity_operations, resolve_path_segment};

use super::common::{collect_child_sections, collect_ui_fields};
use super::page::UiField;

/// Maps a PropertyNode to a UiField for use in templates.
#[allow(clippy::too_many_arguments)]
pub fn ui_field_from_property(
    prop: &PropertyNode,
    is_entity_ref: bool,
    is_codelist: bool,
    codelist_values: &[String],
    immutable_fields: &[String],
    pg_type: &str,
    is_range: bool,
    open_end: bool,
) -> UiField {
    let resolved = resolve_field(prop);
    let ts_type = rust_type_to_ts(&prop.rust_field_type, is_entity_ref);
    let input_type = classify_input_type(prop, is_entity_ref, is_codelist);

    // Strip the Rust `r#` prefix — it is only needed for Rust identifiers,
    // not for TypeScript / Svelte property access.
    let ts_field_name = resolved
        .rust_field_name
        .strip_prefix("r#")
        .unwrap_or(&resolved.rust_field_name)
        .to_string();
    // rust_field_name is sanitized at ingestion (no _code suffix),
    // so it matches the DTO field name directly.
    let ts_field_name = if is_entity_ref && !prop.is_array && !ts_field_name.ends_with("_id") {
        // resolve_field already appends _id for scalar EntityReference
        // columns; junction arrays keep raw names.
        format!("{}_id", ts_field_name)
    } else {
        ts_field_name
    };
    let label = field_name_to_label(&ts_field_name);

    UiField {
        name: ts_field_name,
        label,
        ts_type,
        input_type,
        is_required: prop.is_required,
        is_array: prop.is_array,
        is_entity_ref,
        is_immutable: immutable_fields.contains(&resolved.rust_field_name),
        is_codelist,
        is_range,
        codelist_values: codelist_values.to_vec(),
        description: prop.description.as_deref().unwrap_or("").to_string(),
        pg_type: pg_type.to_string(),
        open_end,
        ref_api_path: None, // Populated later by collect_ui_fields
        structured_sub_fields: vec![],
        nested_type_name: None,
    }
}

pub fn rust_type_to_ts(rust_type: &str, is_entity_ref: bool) -> String {
    if is_entity_ref {
        return "string".to_string();
    }
    match rust_type {
        "String" => "string".to_string(),
        "bool" => "boolean".to_string(),
        "i16" | "i32" | "i64" | "f32" | "f64" | "u32" | "u64" => "number".to_string(),
        t if t.contains("Decimal") => "string".to_string(),
        t if t.contains("NaiveDate") => "string".to_string(),
        t if t.contains("DateTime") => "string".to_string(),
        t if t.contains("Uuid") => "string".to_string(),
        t if t.starts_with("Vec<") => "Array<string>".to_string(),
        _ => "string".to_string(),
    }
}

pub fn classify_input_type(prop: &PropertyNode, is_entity_ref: bool, is_codelist: bool) -> String {
    if is_codelist {
        return "select".to_string();
    }
    if is_entity_ref {
        return "text".to_string();
    }
    // Range types: TSTZRANGE, DATERANGE, INT4RANGE, INT8RANGE
    if prop.pg_column_type.contains("RANGE") {
        return "date-range".to_string();
    }
    // Array types (non-codelist)
    if prop.is_array && !is_codelist {
        return "array".to_string();
    }
    match prop.effective_kind() {
        Some(RefClassificationKind::InlineEnum) => "select".to_string(),
        _ => match prop.rust_field_type.as_str() {
            "bool" => "checkbox".to_string(),
            t if t.contains("NaiveDate") => "date".to_string(),
            t if t.contains("DateTime") => "datetime-local".to_string(),
            "i16" | "i32" | "i64" | "f32" | "f64" | "u32" | "u64" => "number".to_string(),
            t if t.contains("Decimal") => "number".to_string(),
            _ if prop.pg_column_type.contains("geometry")
                || prop.pg_column_type.contains("GEOMETRY") =>
            {
                "geometry".to_string()
            }
            _ => "text".to_string(),
        },
    }
}

pub fn field_name_to_label(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    upper + chars.as_str()
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Serialize)]
pub struct UiFormContext {
    pub entity_name: String,
    pub module_name: String,
    pub domain: String,
    pub path_segment: String,
    pub fields: Vec<UiField>,
    pub create_fields: Vec<UiField>,
    pub update_fields: Vec<UiField>,
    pub has_create: bool,
    pub has_update: bool,
    pub has_child_sections: bool,
    /// This entity's testid vocabulary, projected from `ux::plan::ids`
    /// (issue #468 W3).
    pub tid: crate::ui::testids::TidMap,
}

pub struct UiFormGenerator {
    output_dir: PathBuf,
}

impl UiFormGenerator {
    pub fn new(output_dir: &Path) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
        }
    }
}

#[async_trait]
impl EntityGenerator for UiFormGenerator {
    fn kind(&self) -> EntityGeneratorKind {
        EntityGeneratorKind::UiForm
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

        let has_create = operations.contains(&"create".to_string());
        let has_update = operations.contains(&"update".to_string());

        if !has_create && !has_update {
            return Ok(Vec::new());
        }

        let dto_config = entity_cfg.map(|ec| &ec.dto);
        let immutable_fields: Vec<String> = dto_config
            .map(|d| d.immutable_fields.clone())
            .unwrap_or_default();

        // Workflow-excluded fields
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
            tid: crate::ui::testids::for_module(&module_name.clone()),
            entity_name: entity_name.clone(),
            module_name,
            domain,
            path_segment,
            fields,
            create_fields,
            update_fields,
            has_create,
            has_update,
            has_child_sections,
        };

        let content = render_template_with_project(tera, "ui/entity_form.tera", &ctx, project)?;
        Ok(vec![GeneratedFile {
            path: self
                .output_dir
                .join("ui")
                .join("src")
                .join("lib")
                .join("components")
                .join("forms")
                .join(format!("{}Form.svelte", entity_name)),
            content,
        }])
    }
}
