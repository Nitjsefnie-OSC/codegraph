use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::PropertyNode;
use serde::Serialize;

use crate::ProjectConfig;
use crate::api::api_model::resolve_entity_operations;
use crate::api::include_path::{ResolvedIncludePath, resolve_include_paths_for_topology};
use crate::error::Result;
use crate::render_template_with_project;
use crate::traits::{EntityGenerator, EntityGeneratorKind, GeneratedFile};
use crate::type_registry;
use codegraph_config::DomainConfig;
use codegraph_type_contracts::RefClassificationKind;

#[derive(Debug, Serialize)]
pub struct DtoContext {
    pub module_name: String,
    pub entity_name: String,
    pub domain: String,
    pub fields: Vec<DtoField>,
    pub immutable_fields: Vec<String>,
    pub workflow_excluded_fields: Vec<String>,
    pub list_exclude: Vec<String>,
    pub list_include: Vec<String>,
    pub has_list_fields: bool,
    pub operations: Vec<String>,
    /// First-level child DTOs only — used for parent struct field references.
    pub child_dtos: Vec<ChildDtoContext>,
    /// All child DTOs flattened (including deeply nested) — used for struct definitions.
    pub all_child_dtos: Vec<ChildDtoContext>,
    pub codelist_imports: Vec<String>,
    /// Codelist imports for the update DTO (parent-level fields only, excludes child DTO fields).
    pub codelist_imports_update: Vec<String>,
    /// Whether this entity has a workflow (adds workflow_state section to response).
    pub has_workflow: bool,
    /// Whether this entity has an approval status field.
    pub has_approval_status: bool,
    /// Import paths for structured JSONB wrapper types used by DTO fields.
    pub structured_imports: Vec<String>,
    /// Whether to emit garde validation attributes on DTO fields.
    pub has_validate: bool,
    /// Entity fields the ingested mox domain model marks as derived
    /// (computed). They are marked read-only in the response DTO and
    /// excluded from the create/update DTOs (issue #218, #195 decision).
    pub derived_fields: Vec<String>,
    /// Mox derived features that name NO stored field: exposed as read-only
    /// response fields with computed accessors (issue #501 slice 1). Empty
    /// when no mox was ingested — output stays byte-identical.
    pub mox_derived_extras: Vec<MoxDerivedExtra>,
}

/// One non-stored mox derived feature, ready for response-DTO emission.
#[derive(Debug, Clone, Serialize)]
pub struct MoxDerivedExtra {
    /// The mox feature name as written (for diagnostics).
    pub source_name: String,
    /// snake_case Rust field / accessor name.
    pub name: String,
    /// Rust field type; `None` when the mox type has no primitive Rust
    /// mapping (the accessor is then a `todo!` stub and no field is exposed).
    pub rust_type: Option<String>,
    /// Lowered accessor body — a Rust expression, or a `todo!(...)` stub
    /// naming what did not lower (the module is the record, never a guess).
    pub body: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DtoField {
    pub name: String,
    pub rust_type: String,
    pub is_required: bool,
    pub is_array: bool,
    pub description: String,
    pub render_strategy: String,
    pub is_entity_ref: bool,
    pub is_hierarchy_field: bool,
    // Validation fields
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub minimum: Option<rust_decimal::Decimal>,
    pub maximum: Option<rust_decimal::Decimal>,
    pub pattern: Option<String>,
    pub format: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChildDtoContext {
    pub field_name: String,
    pub struct_name: String,
    pub fields: Vec<DtoField>,
    pub is_array: bool,
    /// Nested child DTOs (ValueObject properties within this child DTO)
    pub child_dtos: Vec<ChildDtoContext>,
}

/// Maximum nesting depth for recursive child DTO building.
const MAX_CHILD_DTO_DEPTH: usize = 10;

/// Recursively build a `ChildDtoContext` for a ValueObject property.
async fn build_child_dto(
    db: &dyn GraphQuerier,
    prop: &codegraph_core::types::PropertyNode,
    parent_schema_title: &str,
    parent_struct_name: &str,
    visited: &mut std::collections::HashSet<String>,
    depth: usize,
    suffix: &str,
) -> Option<ChildDtoContext> {
    if depth >= MAX_CHILD_DTO_DEPTH {
        return None;
    }

    let target = if prop.is_array {
        db.get_array_item_schema(&prop.name, parent_schema_title)
            .await
            .ok()
            .flatten()
    } else {
        db.get_property_ref_target(&prop.name, parent_schema_title)
            .await
            .ok()
            .flatten()
    };

    let target_schema = target?;

    // Cycle guard
    if !visited.insert(target_schema.title.clone()) {
        return None;
    }

    let raw_child_props = db
        .get_properties(&target_schema.title)
        .await
        .unwrap_or_default();
    let child_props = {
        let mut seen = std::collections::HashSet::new();
        raw_child_props
            .into_iter()
            .filter(|p| p.rust_field_name != "id" && seen.insert(p.rust_field_name.clone()))
            .collect::<Vec<_>>()
    };

    let child_struct_name = format!(
        "{}{}",
        parent_struct_name,
        codegraph_naming::strip_suffix(&target_schema.rust_type_name, suffix)
    );

    let mut child_fields: Vec<DtoField> = Vec::new();
    let mut nested_child_dtos: Vec<ChildDtoContext> = Vec::new();

    // Composite range: collapse start/end fields into a single range column (same as parent DTOs)
    let consumed_fields: std::collections::HashSet<String> = db
        .get_consumed_fields(&target_schema.title)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(prop, _role)| prop.name)
        .collect();

    // Add composite range field to child DTO (e.g. affiliation_period: String)
    if let Ok(Some(range)) = db.get_composite_range(&target_schema.title).await {
        child_fields.push(DtoField {
            name: range.pg_column_name,
            rust_type: "String".to_string(),
            is_required: false,
            is_array: false,
            description: String::new(),
            render_strategy: "composite_range".to_string(),
            is_entity_ref: false,
            is_hierarchy_field: false,
            min_length: None,
            max_length: None,
            minimum: None,
            maximum: None,
            pattern: None,
            format: None,
        });
    }

    for c in child_props
        .iter()
        .filter(|c| !consumed_fields.contains(&c.name))
    {
        match c.effective_kind() {
            Some(RefClassificationKind::CompositeWrapper)
            | Some(RefClassificationKind::MediaWrapper) => {
                if let Ok(comp_cols) = db
                    .get_composite_columns(&c.name, &target_schema.title)
                    .await
                {
                    for col in &comp_cols {
                        let rust_type = col
                            .dto_rust_type
                            .clone()
                            .unwrap_or_else(|| col.rust_type.clone());
                        child_fields.push(DtoField {
                            name: format!("{}{}", c.rust_field_name, col.suffix),
                            rust_type,
                            is_required: c.is_required,
                            is_array: false,
                            description: String::new(),
                            render_strategy: "composite_column".to_string(),
                            is_entity_ref: false,
                            is_hierarchy_field: false,
                            min_length: None,
                            max_length: None,
                            minimum: None,
                            maximum: None,
                            pattern: None,
                            format: None,
                        });
                    }
                }
            }
            Some(RefClassificationKind::EntityReference) => {
                let fd = codegraph_core::types::resolve_field(c);
                child_fields.push(DtoField {
                    name: fd.rust_field_name.clone(),
                    rust_type: "uuid::Uuid".to_string(),
                    is_required: c.is_required,
                    is_array: false,
                    description: String::new(),
                    render_strategy: "entity_ref".to_string(),
                    is_entity_ref: true,
                    is_hierarchy_field: false,
                    min_length: None,
                    max_length: None,
                    minimum: None,
                    maximum: None,
                    pattern: None,
                    format: None,
                });
            }
            Some(RefClassificationKind::StructuredWrapper) => {
                let fd = codegraph_core::types::resolve_field(c);
                // StructuredWrappers are stored as a single JSONB column inline.
                child_fields.push(DtoField {
                    name: fd.rust_field_name.clone(),
                    rust_type: "serde_json::Value".to_string(),
                    is_required: c.is_required,
                    is_array: false,
                    description: String::new(),
                    render_strategy: "direct_column".to_string(),
                    is_entity_ref: false,
                    is_hierarchy_field: false,
                    min_length: None,
                    max_length: None,
                    minimum: None,
                    maximum: None,
                    pattern: None,
                    format: None,
                });
            }
            Some(RefClassificationKind::ValueObject) => {
                // Recurse: nested VOs become nested child DTOs
                let nested_dto = Box::pin(build_child_dto(
                    db,
                    c,
                    &target_schema.title,
                    &child_struct_name,
                    visited,
                    depth + 1,
                    suffix,
                ))
                .await;
                if let Some(nested) = nested_dto {
                    nested_child_dtos.push(nested);
                }
            }
            Some(RefClassificationKind::CodelistReference)
            | Some(RefClassificationKind::CodelistCheck) => {
                if c.is_array {
                    // Codelist array → nested child DTO with single "code" field
                    let nested_struct = format!(
                        "{}{}",
                        child_struct_name,
                        codegraph_naming::to_pascal_case(&c.rust_field_name)
                    );
                    let code_type = codelist_enum_name_from_ref(&c.ref_target)
                        .unwrap_or_else(|| "String".to_string());
                    nested_child_dtos.push(ChildDtoContext {
                        field_name: c.rust_field_name.clone(),
                        struct_name: nested_struct,
                        fields: vec![DtoField {
                            name: "code".to_string(),
                            rust_type: code_type,
                            is_required: true,
                            is_array: false,
                            description: String::new(),
                            render_strategy: "codelist".to_string(),
                            is_entity_ref: false,
                            is_hierarchy_field: false,
                            min_length: None,
                            max_length: None,
                            minimum: None,
                            maximum: None,
                            pattern: None,
                            format: None,
                        }],
                        is_array: true,
                        child_dtos: vec![],
                    });
                } else {
                    let fd = codegraph_core::types::resolve_field(c);
                    let rust_type = codelist_enum_name_from_ref(&c.ref_target)
                        .unwrap_or_else(|| "String".to_string());
                    child_fields.push(DtoField {
                        name: fd.rust_field_name.clone(),
                        rust_type,
                        is_required: c.is_required,
                        is_array: false,
                        description: String::new(),
                        render_strategy: "codelist".to_string(),
                        is_entity_ref: false,
                        is_hierarchy_field: false,
                        min_length: None,
                        max_length: None,
                        minimum: None,
                        maximum: None,
                        pattern: None,
                        format: None,
                    });
                }
            }
            Some(RefClassificationKind::PrimitiveWrapper)
            | Some(RefClassificationKind::ArrayWrapper)
            | Some(RefClassificationKind::RangeWrapper)
            | Some(RefClassificationKind::InlineEnum) => {
                let fd = codegraph_core::types::resolve_field(c);
                let len_ok = supports_garde_length(&c.rust_field_type);
                child_fields.push(DtoField {
                    name: fd.rust_field_name.clone(),
                    rust_type: c.rust_field_type.clone(),
                    is_required: c.is_required,
                    is_array: false,
                    description: String::new(),
                    render_strategy: "direct_column".to_string(),
                    is_entity_ref: false,
                    is_hierarchy_field: false,
                    min_length: if len_ok { c.min_length } else { None },
                    max_length: if len_ok { c.max_length } else { None },
                    minimum: c.minimum,
                    maximum: c.maximum,
                    pattern: c.pattern.clone(),
                    format: c.format.clone(),
                });
            }
            None => {
                let t = &c.rust_field_type;
                if t.contains("::")
                    || t.starts_with("Vec<")
                    || matches!(
                        t.as_str(),
                        "String" | "bool" | "i16" | "i32" | "i64" | "f32" | "f64" | "u32" | "u64"
                    )
                {
                    let fd = codegraph_core::types::resolve_field(c);
                    let len_ok = supports_garde_length(t);
                    child_fields.push(DtoField {
                        name: fd.rust_field_name.clone(),
                        rust_type: t.clone(),
                        is_required: c.is_required,
                        is_array: false,
                        description: String::new(),
                        render_strategy: "direct_column".to_string(),
                        is_entity_ref: false,
                        is_hierarchy_field: false,
                        min_length: if len_ok { c.min_length } else { None },
                        max_length: if len_ok { c.max_length } else { None },
                        minimum: c.minimum,
                        maximum: c.maximum,
                        pattern: c.pattern.clone(),
                        format: c.format.clone(),
                    });
                }
            }
        }
    }

    // Deduplicate child fields by name
    {
        let mut seen_fields = std::collections::HashSet::new();
        child_fields.retain(|f| seen_fields.insert(f.name.clone()));
    }

    Some(ChildDtoContext {
        field_name: prop.rust_field_name.clone(),
        struct_name: child_struct_name,
        fields: child_fields,
        is_array: prop.is_array,
        child_dtos: nested_child_dtos,
    })
}

/// Flatten nested child DTOs into a single list (depth-first).
/// The template iterates a flat list and emits struct definitions for each.
/// Each entry retains its `child_dtos` so templates can emit nested child fields.
fn flatten_child_dtos(children: Vec<ChildDtoContext>) -> Vec<ChildDtoContext> {
    let mut result = Vec::new();
    for child in children {
        let nested = child.child_dtos.clone();
        result.push(child);
        result.extend(flatten_child_dtos(nested));
    }
    result
}

/// Build the template context for DTO generation. Shared between the app DTO generator
/// and the domain-types DTO generator.
pub async fn build_dto_context(
    db: &dyn GraphQuerier,
    schema_title: &str,
    domain: &str,
    config: &DomainConfig,
) -> Result<DtoContext> {
    let schema = db
        .get_schema_in_domain(schema_title, domain)
        .await?
        .ok_or_else(|| crate::error::Error::SchemaNotFound(schema_title.into()))?;

    let entity_name = schema.rust_type_name.clone();
    let module_name = schema.pg_table_name.clone();
    let domain = domain.to_string();

    // Get entity config from domains.toml
    let entity_cfg = config
        .domains
        .get(&domain)
        .and_then(|d| d.get_entity_config(&entity_name));

    // DDD design surface (issue #449): a design covering this title maps
    // its repository built-ins onto the DTO operation set — no `save` in
    // the design means no create/update DTO files. dto_response always
    // emits. Empty surface → schema-derived operations, byte-identical.
    let design_surface = crate::ddd::design::DddDesignSurface::from_graph(db).await?;
    let mut operations = resolve_entity_operations(db, config, &domain, &entity_name).await;
    if let Some(design_ops) = design_surface.operations_for(schema_title) {
        operations = design_ops;
    }

    let dto_config = entity_cfg.map(|ec| &ec.dto);
    let mut immutable_fields = dto_config
        .map(|d| d.immutable_fields.clone())
        .unwrap_or_default();
    let list_exclude = dto_config
        .map(|d| d.list_exclude.clone())
        .unwrap_or_default();
    let list_include = dto_config
        .map(|d| d.list_include.clone())
        .unwrap_or_default();

    // Workflow status fields are excluded from Create/Update DTOs —
    // status is set to initial_state on create and changed via workflow transitions.
    let workflow = entity_cfg.and_then(|ec| ec.workflow.as_ref());
    let mut workflow_excluded_fields = Vec::new();
    if let Some(wf) = workflow {
        workflow_excluded_fields.push(wf.status_field.clone());
        if let Some(ref approval_field) = wf.approval_status_field {
            workflow_excluded_fields.push(approval_field.clone());
        }
        // Also mark as immutable so they're excluded from Update DTO
        for f in &workflow_excluded_fields {
            if !immutable_fields.contains(f) {
                immutable_fields.push(f.clone());
            }
        }
    }

    let hierarchy_field_name = entity_cfg
        .and_then(|ec| ec.hierarchy_field.as_ref())
        .cloned();

    let all_props = db.get_properties(schema_title).await?;

    // Deduplicate properties by field name — allOf composition can produce
    // duplicate HasProperty edges (parent + child both contribute the same field).
    let mut props = {
        let mut seen = std::collections::HashSet::new();
        all_props
            .into_iter()
            .filter(|p| seen.insert(p.rust_field_name.clone()))
            .collect::<Vec<_>>()
    };
    // For codelist entities with no graph properties (enum-only JSON schema),
    // inject the three columns created by the codelist DDL template.
    codegraph_core::types::inject_codelist_properties(&mut props, schema.is_codelist, &domain);

    // Media fields are excluded from Create/Update DTOs — uploads happen via
    // separate media endpoints, not the JSON CRUD body.
    for prop in &props {
        if prop.effective_kind() == Some(RefClassificationKind::MediaWrapper)
            && let Ok(comp_cols) = db.get_composite_columns(&prop.name, schema_title).await
        {
            for col in &comp_cols {
                let field_name = format!("{}{}", prop.rust_field_name, col.suffix);
                if !workflow_excluded_fields.contains(&field_name) {
                    workflow_excluded_fields.push(field_name.clone());
                }
                if !immutable_fields.contains(&field_name) {
                    immutable_fields.push(field_name);
                }
            }
        }
    }

    // Consumed fields from composite range collapsing — skip these from DTOs
    let consumed_fields: std::collections::HashSet<String> = db
        .get_consumed_fields(schema_title)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(prop, _role)| prop.name)
        .collect();

    // Collect all raw field names to detect collisions when stripping _code suffix.
    // E.g. if both "language_code" and "language" exist, don't strip "language_code".
    let _all_field_names: std::collections::HashSet<String> =
        props.iter().map(|p| p.rust_field_name.clone()).collect();

    // Build entity titles set so we can detect VO properties that target
    // entities (these become FK columns in the DDL, not child tables).
    let entity_titles: std::collections::HashSet<String> = config
        .domains
        .values()
        .flat_map(|d| d.entities.iter().cloned())
        .collect();

    let mut fields = Vec::new();
    let mut child_dtos = Vec::new();
    let mut seen_child_structs = std::collections::HashSet::new();

    for prop in &props {
        // Skip fields consumed by composite ranges
        if consumed_fields.contains(&prop.name) {
            continue;
        }
        let is_entity_ref = prop.effective_kind() == Some(RefClassificationKind::EntityReference);

        if prop.effective_kind() == Some(RefClassificationKind::CompositeWrapper)
            || prop.effective_kind() == Some(RefClassificationKind::MediaWrapper)
        {
            if let Ok(comp_cols) = db.get_composite_columns(&prop.name, schema_title).await {
                for col in &comp_cols {
                    let rust_type = col
                        .dto_rust_type
                        .clone()
                        .unwrap_or_else(|| col.rust_type.clone());
                    fields.push(DtoField {
                        name: format!("{}{}", prop.rust_field_name, col.suffix),
                        rust_type,
                        is_required: prop.is_required,
                        is_array: false,
                        description: String::new(),
                        render_strategy: "composite_column".to_string(),
                        is_entity_ref: false,
                        is_hierarchy_field: false,
                        min_length: None,
                        max_length: None,
                        minimum: None,
                        maximum: None,
                        pattern: None,
                        format: None,
                    });
                }
            }
            continue;
        }

        if prop.effective_kind() == Some(RefClassificationKind::ValueObject) {
            // When a non-array VO targets a known entity, the DDL emits an
            // FK column instead of a child table. Emit a UUID field to match.
            let is_entity_fk = if !prop.is_array {
                db.get_property_ref_target(&prop.name, schema_title)
                    .await
                    .ok()
                    .flatten()
                    .map(|t| entity_titles.contains(&t.title))
                    .unwrap_or(false)
            } else {
                false
            };
            if is_entity_fk {
                let fd = codegraph_core::types::resolve_field(prop);
                fields.push(DtoField {
                    name: fd.rust_field_name.clone(),
                    rust_type: "Uuid".to_string(),
                    is_required: false,
                    is_array: false,
                    description: prop.description.clone().unwrap_or_default(),
                    render_strategy: "entity_ref".to_string(),
                    is_entity_ref: true,
                    is_hierarchy_field: false,
                    min_length: None,
                    max_length: None,
                    minimum: None,
                    maximum: None,
                    pattern: None,
                    format: None,
                });
            } else {
                let mut visited = std::collections::HashSet::new();
                visited.insert(schema_title.to_string());
                if let Some(child_dto) = Box::pin(build_child_dto(
                    db,
                    prop,
                    schema_title,
                    &entity_name,
                    &mut visited,
                    0,
                    &config.defaults.type_suffix,
                ))
                .await
                    && seen_child_structs.insert(child_dto.struct_name.clone())
                {
                    child_dtos.push(child_dto);
                }
            }
            continue;
        }

        // Array of entity refs:
        //  - Junction (no back-reference on the target schema): the DTO carries
        //    a Vec<uuid::Uuid> of related ids, persisted into the junction table.
        //  - FK-on-child (target has a `<parent>_id` column, e.g. party.case_id
        //    for case.party_ids): no field on the parent DTO — children are
        //    managed through their own endpoints.
        if prop.is_array && prop.effective_kind() == Some(RefClassificationKind::EntityReference) {
            let target = db
                .get_array_item_schema(&prop.name, schema_title)
                .await
                .ok()
                .flatten();
            let back_ref = format!(
                "{}_id",
                codegraph_naming::truncate_pg_identifier(&module_name)
            );
            let has_back_ref = match &target {
                Some(t) => db
                    .get_properties(&t.title)
                    .await
                    .map(|ps| {
                        ps.iter().any(|p| {
                            p.pg_column_name == back_ref
                                || p.pg_column_name
                                    == format!(
                                        "{}_id",
                                        codegraph_naming::to_snake_case(schema_title)
                                    )
                        })
                    })
                    .unwrap_or(false),
                None => false,
            };
            if !has_back_ref {
                let field_def = codegraph_core::types::resolve_field(prop);
                fields.push(DtoField {
                    name: field_def.rust_field_name.clone(),
                    rust_type: "uuid::Uuid".to_string(),
                    is_required: prop.is_required,
                    is_array: true,
                    description: prop.description.clone().unwrap_or_default(),
                    render_strategy: "entity_ref".to_string(),
                    is_entity_ref: true,
                    is_hierarchy_field: false,
                    min_length: None,
                    max_length: None,
                    minimum: None,
                    maximum: None,
                    pattern: None,
                    format: None,
                });
            }
            continue;
        }

        // Codelist array properties → synthetic child DTO with a single "code" field.
        if prop.is_array
            && matches!(
                prop.effective_kind(),
                Some(RefClassificationKind::CodelistReference)
                    | Some(RefClassificationKind::CodelistCheck)
            )
        {
            let child_struct_name = format!(
                "{}{}",
                entity_name,
                codegraph_naming::to_pascal_case(&prop.rust_field_name)
            );
            let code_type = codelist_enum_name_from_ref(&prop.ref_target)
                .unwrap_or_else(|| "String".to_string());
            if seen_child_structs.insert(child_struct_name.clone()) {
                child_dtos.push(ChildDtoContext {
                    field_name: prop.rust_field_name.clone(),
                    struct_name: child_struct_name,
                    fields: vec![DtoField {
                        name: "code".to_string(),
                        rust_type: code_type,
                        is_required: true,
                        is_array: false,
                        description: prop.description.clone().unwrap_or_default(),
                        render_strategy: "codelist".to_string(),
                        is_entity_ref: false,
                        is_hierarchy_field: false,
                        min_length: None,
                        max_length: None,
                        minimum: None,
                        maximum: None,
                        pattern: None,
                        format: None,
                    }],
                    is_array: true,
                    child_dtos: vec![],
                });
            }
            continue;
        }

        // Map codelist references to the generated codelist enum type
        // (e.g. GenderCodeList) for compile-time validation
        let rust_type = match prop.effective_kind() {
            Some(RefClassificationKind::CodelistReference)
            | Some(RefClassificationKind::CodelistCheck) => {
                codelist_enum_name_from_ref(&prop.ref_target)
                    .unwrap_or_else(|| "String".to_string())
            }
            Some(RefClassificationKind::RangeWrapper) => {
                // Range types mapped to JSONB; use the graph's rust_field_type
                // (serde_json::Value) so DTOs match entity model types.
                prop.rust_field_type.clone()
            }
            _ => prop.rust_field_type.clone(),
        };
        // When is_array is true, the template wraps the type in Vec<>.
        // The ingester already sets rust_field_type to Vec<T> for arrays,
        // so strip the outer Vec<> to avoid double-wrapping (Vec<Vec<T>>).
        let rust_type = if prop.is_array {
            strip_vec_wrapper(&rust_type)
        } else {
            rust_type
        };

        let field_def = codegraph_core::types::resolve_field(prop);
        // Use field_def.rust_field_name for DTO field names
        let len_ok = supports_garde_length(&rust_type);

        fields.push(DtoField {
            name: field_def.rust_field_name.clone(),
            rust_type,
            // Entity references in DTOs respect the schema's required/optional
            // constraint — required FKs produce non-Option uuid::Uuid fields.
            is_required: prop.is_required,
            is_array: prop.is_array,
            description: prop.description.as_deref().unwrap_or("").to_string(),
            render_strategy: prop.render_strategy.clone(),
            is_entity_ref,
            is_hierarchy_field: hierarchy_field_name.as_deref() == Some(&field_def.rust_field_name),
            min_length: if len_ok { prop.min_length } else { None },
            max_length: if len_ok { prop.max_length } else { None },
            minimum: prop.minimum,
            maximum: prop.maximum,
            pattern: prop.pattern.clone(),
            format: prop.format.clone(),
        });
    }

    // Add synthetic hierarchy field (self-referential FK) when configured.
    // This column is added by the DDL generator but doesn't exist in the schema.
    if let Some(ref hf) = hierarchy_field_name {
        fields.push(DtoField {
            name: hf.clone(),
            rust_type: "uuid::Uuid".to_string(),
            is_required: false,
            is_array: false,
            description: String::new(),
            render_strategy: String::new(),
            is_entity_ref: false,
            is_hierarchy_field: true,
            min_length: None,
            max_length: None,
            minimum: None,
            maximum: None,
            pattern: None,
            format: None,
        });
    }

    // Deduplicate parent-level fields by name
    {
        let mut seen_fields = std::collections::HashSet::new();
        fields.retain(|f| seen_fields.insert(f.name.clone()));
    }

    // Inject hierarchy_field as a synthetic optional UUID field for self-referential
    // tree/hierarchy relationships (e.g. parent_organization_id, reports_to_position_id).
    if let Some(hf) = entity_cfg.and_then(|ec| ec.hierarchy_field.clone())
        && !fields.iter().any(|f| f.name == hf)
    {
        fields.push(DtoField {
            name: hf,
            rust_type: "uuid::Uuid".to_string(),
            is_required: false,
            is_array: false,
            description: "Parent hierarchy reference.".to_string(),
            render_strategy: "hierarchy".to_string(),
            is_entity_ref: false,
            is_hierarchy_field: true,
            min_length: None,
            max_length: None,
            minimum: None,
            maximum: None,
            pattern: None,
            format: None,
        });
    }

    let has_list_fields = !list_include.is_empty();

    // Collect codelist imports: scan fields for types matching known codelist names
    let codelist_names: std::collections::HashSet<String> = db
        .list_codelists()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|cl| cl.name)
        .collect();

    // Flatten nested child DTOs so we scan ALL nesting levels for codelist imports
    // and generate struct definitions for every level.
    let all_child_dtos = flatten_child_dtos(child_dtos.clone());

    // Scan both top-level fields and all child DTO fields (at every nesting level)
    let all_dto_fields = fields
        .iter()
        .chain(all_child_dtos.iter().flat_map(|c| c.fields.iter()));
    let mut codelist_imports: Vec<String> = all_dto_fields
        .filter_map(|f| {
            // Check the raw type and Vec<Type> inner type
            let ty = &f.rust_type;
            let inner = ty
                .strip_prefix("Vec<")
                .and_then(|s| s.strip_suffix('>'))
                .unwrap_or(ty);
            if codelist_names.contains(inner) {
                Some(inner.to_string())
            } else {
                None
            }
        })
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    codelist_imports.sort();

    // Codelist imports for the update DTO: only parent-level fields (child structs
    // are imported from dto_create, so their codelist types aren't needed here).
    let mut codelist_imports_update: Vec<String> = fields
        .iter()
        .filter_map(|f| {
            let ty = &f.rust_type;
            let inner = ty
                .strip_prefix("Vec<")
                .and_then(|s| s.strip_suffix('>'))
                .unwrap_or(ty);
            if codelist_names.contains(inner) {
                Some(inner.to_string())
            } else {
                None
            }
        })
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    codelist_imports_update.sort();

    let has_workflow = workflow
        .map(|wf| wf.generate_action_endpoints)
        .unwrap_or(false);
    let has_approval_status = workflow
        .and_then(|wf| wf.approval_status_field.as_ref())
        .is_some();

    // Collect structured wrapper imports by scanning all DTO fields (parent + child)
    // for types classified as StructuredWrapper. Parent properties have classification
    // info; for child DTO fields, walk their source schemas to check classification.
    let mut structured_type_names: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    // 1. Direct parent-level structured wrapper properties
    for p in &props {
        if p.effective_kind() == Some(RefClassificationKind::StructuredWrapper) {
            // Strip Vec<> and Option<> wrappers for array and optional properties
            let mut ty = strip_vec_wrapper(&p.rust_field_type);
            ty = strip_option_wrapper(&ty);
            if ty != "serde_json::Value" && !ty.is_empty() {
                structured_type_names.insert(ty);
            }
        }
    }

    // 2. Walk child VO schemas recursively to find StructuredWrapper properties
    //    at any nesting depth (mirrors the child DTO building recursion).
    let mut vo_visit_queue: Vec<(String, String)> = Vec::new(); // (prop_name, parent_schema)
    let mut vo_visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    vo_visited.insert(schema_title.to_string());
    for p in &props {
        if p.effective_kind() == Some(RefClassificationKind::ValueObject) {
            vo_visit_queue.push((p.name.clone(), schema_title.to_string()));
        }
    }
    while let Some((prop_name, parent)) = vo_visit_queue.pop() {
        let target = match db.get_property_ref_target(&prop_name, &parent).await {
            Ok(Some(t)) => Some(t),
            _ => db
                .get_array_item_schema(&prop_name, &parent)
                .await
                .ok()
                .flatten(),
        };
        if let Some(ts) = target {
            if !vo_visited.insert(ts.title.clone()) {
                continue;
            }
            if let Ok(child_props) = db.get_properties(&ts.title).await {
                for cp in &child_props {
                    if cp.effective_kind() == Some(RefClassificationKind::StructuredWrapper) {
                        let mut ty = strip_vec_wrapper(&cp.rust_field_type);
                        ty = strip_option_wrapper(&ty);
                        if ty != "serde_json::Value" && !ty.is_empty() {
                            structured_type_names.insert(ty);
                        }
                    }
                    if cp.effective_kind() == Some(RefClassificationKind::ValueObject) {
                        vo_visit_queue.push((cp.name.clone(), ts.title.clone()));
                    }
                }
            }
        }
    }

    let import_prefix = &config.defaults.types_import_prefix;
    let structured_imports: Vec<String> = structured_type_names
        .into_iter()
        .map(|t| format!("use {import_prefix}::{t};"))
        .collect();

    // Mox derived features (issues #218, #501): features the domain model
    // marks as computed are read-only response fields, omitted from
    // create/update inputs. Matched by class NAME against this schema
    // (title or entity name). Features naming an existing stored field
    // annotate that field (the #218 behavior); features naming NO stored
    // field are exposed as new read-only response fields with computed
    // accessors (#501 slice 1). When no mox was ingested both lists are
    // empty and output is byte-identical.
    let derived_all: Vec<codegraph_core::types::MoxDerivedFeatureNode> = db
        .get_mox_derived_features()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.class == schema_title || d.class == entity_name)
        .collect();
    let mut derived_fields: Vec<String> = derived_all
        .iter()
        .map(|d| codegraph_naming::to_snake_case(&d.name))
        .filter(|name| fields.iter().any(|f| f.name == *name))
        .collect();
    derived_fields.sort();
    derived_fields.dedup();
    for name in &derived_fields {
        if !workflow_excluded_fields.contains(name) {
            workflow_excluded_fields.push(name.clone());
        }
        if !immutable_fields.contains(name) {
            immutable_fields.push(name.clone());
        }
    }

    // Non-stored derived features (#501 slice 1, quality bar #508): a
    // read-only response field plus a fixed-signature accessor lowered
    // from the `expr` body. A body that refuses to lower is a GENERATION
    // ERROR (issue #508 R3 — never an emitted `todo!` panic); body-less
    // features keep the documented seam.
    let field_bindings: std::collections::HashMap<String, crate::mox_expr::FieldBinding> = fields
        .iter()
        .map(|f| {
            (
                f.name.clone(),
                crate::mox_expr::FieldBinding::new(
                    format!("self.{}", f.name),
                    !crate::mox_expr::is_copy_rust_type(&f.rust_type),
                    f.rust_type.clone(),
                ),
            )
        })
        .collect();
    let empty_arena = crate::mox_expr::ArenaIndex::new();
    let mut mox_derived_extras: Vec<MoxDerivedExtra> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for feature in &derived_all {
        let name =
            codegraph_naming::escape_rust_keyword(&codegraph_naming::to_snake_case(&feature.name));
        if fields.iter().any(|f| f.name == name) || derived_fields.contains(&name) {
            continue;
        }
        let rust_type =
            crate::mox_expr::mox_primitive_rust_type(&feature.type_ref).map(str::to_string);
        let body = match feature.expr.as_deref() {
            Some(expr) => {
                match crate::mox_expr::lower_expr_to_rust(expr, &field_bindings, &empty_arena) {
                    Ok(code) => code,
                    Err(e) => {
                        failures.push(format!(
                            "derived feature `{}` on `{}`: {} ({})",
                            feature.name, schema_title, e.detail, e.kind
                        ));
                        continue;
                    }
                }
            }
            None => format!(
                "todo!(\"body-less derived feature '{}' — declare an expr body\")",
                feature.name
            ),
        };
        mox_derived_extras.push(MoxDerivedExtra {
            source_name: feature.name.clone(),
            name,
            rust_type,
            body,
        });
    }
    if !failures.is_empty() {
        return Err(crate::error::Error::Validation(format!(
            "mox expr bodies outside the lowered subset (issue #508 R3):\n  - {}",
            failures.join("\n  - ")
        )));
    }
    mox_derived_extras.sort_by(|a, b| a.name.cmp(&b.name));
    mox_derived_extras.dedup_by(|a, b| a.name == b.name);

    Ok(DtoContext {
        module_name,
        entity_name,
        domain,
        fields,
        immutable_fields,
        workflow_excluded_fields,
        list_exclude,
        list_include,
        has_list_fields,
        operations,
        child_dtos,
        all_child_dtos,
        codelist_imports,
        codelist_imports_update,
        has_workflow,
        has_approval_status,
        structured_imports,
        has_validate: true,
        derived_fields,
        mox_derived_extras,
    })
}

pub struct DtoGenerator {
    output_dir: PathBuf,
}

impl DtoGenerator {
    pub fn new(output_dir: &Path) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
        }
    }
}

#[async_trait]
impl EntityGenerator for DtoGenerator {
    fn kind(&self) -> EntityGeneratorKind {
        EntityGeneratorKind::Dto
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
        let ctx = build_dto_context(db, schema_title, domain, config).await?;

        if ctx.module_name.is_empty() {
            return Ok(Vec::new());
        }

        // Issue #268 (namespace_layout): namespaced DTOs emit under
        // `src/domain/{ns_dir}/{module}/` and register their types on the
        // namespace-derived module path, so handler/import consumers
        // resolve them there. Flat (gate off / namespace-less) keeps the
        // domain-derived layout byte-identical.
        let schema = db
            .get_schema_in_domain(schema_title, domain)
            .await?
            .ok_or_else(|| crate::error::Error::SchemaNotFound(schema_title.into()))?;
        let ns_dir = crate::namespace_dir_prefix(schema.namespace.as_deref(), project);
        let ns_rust = crate::namespace_rust_prefix(schema.namespace.as_deref(), project);
        let domain_dir_segment = ns_dir.as_deref().unwrap_or(&ctx.domain);

        let base_dir = self
            .output_dir
            .join("src")
            .join("domain")
            .join(domain_dir_segment)
            .join(&ctx.module_name);

        let mut files = Vec::new();

        if ctx.operations.contains(&"create".to_string()) {
            let content = render_template_with_project(tera, "ddd/dto_create.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: base_dir.join("dto_create.rs"),
                content,
            });
        }

        if ctx.operations.contains(&"update".to_string()) {
            let content = render_template_with_project(tera, "ddd/dto_update.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: base_dir.join("dto_update.rs"),
                content,
            });
        }

        let response = render_template_with_project(tera, "ddd/dto_response.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: base_dir.join("dto_response.rs"),
            content: response,
        });

        // Register all DTO types produced by this generator for cross-generator import resolution.
        let module_path = || -> Vec<String> {
            // Issue #268: the registered path must match the emitted file
            // location — namespace segments when gated, domain segment flat.
            let base: Vec<String> = match &ns_rust {
                Some(ns) => {
                    let mut segs: Vec<String> = format!("crate::domain::{ns}")
                        .split("::")
                        .map(str::to_string)
                        .collect();
                    segs.push(ctx.module_name.clone());
                    segs
                }
                None => vec![
                    "crate".into(),
                    "domain".into(),
                    ctx.domain.clone(),
                    ctx.module_name.clone(),
                ],
            };
            base
        };
        type_registry::register_type(
            &format!("{}Response", ctx.entity_name),
            [module_path(), vec!["dto_response".into()]].concat(),
        );
        type_registry::register_type(
            &format!("{}LinkedResponse", ctx.entity_name),
            [module_path(), vec!["dto_response".into()]].concat(),
        );
        if ctx.operations.contains(&"create".to_string()) {
            type_registry::register_type(
                &format!("Create{}Request", ctx.entity_name),
                [module_path(), vec!["dto_create".into()]].concat(),
            );
        }
        if ctx.operations.contains(&"update".to_string()) {
            type_registry::register_type(
                &format!("Update{}Request", ctx.entity_name),
                [module_path(), vec!["dto_update".into()]].concat(),
            );
        }
        // Register child DTO types (first-level children are referenced by parent DTOs).
        for child in &ctx.all_child_dtos {
            type_registry::register_type(
                &child.struct_name,
                [module_path(), vec!["dto_response".into()]].concat(),
            );
            // Also register the Response variant — child DTOs are used as
            // Option<{struct_name}Response> in include-path fields.
            type_registry::register_type(
                &format!("{}Response", child.struct_name),
                [module_path(), vec!["dto_response".into()]].concat(),
            );
        }

        // Generate include DTOs if allow_include is configured.
        // Skip non-root entities (child, value_object) unless they have explicit
        // allow_include configuration — auto-discovered includes only make sense
        // for root entities.
        let entity_cfg = config
            .domains
            .get(domain)
            .and_then(|d| d.get_entity_config(schema_title));
        let has_explicit_include = entity_cfg
            .and_then(|ec| ec.allow_include.as_ref())
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        let is_root = entity_cfg
            .and_then(|ec| ec.role.as_deref())
            .map(|r| r == "root")
            .unwrap_or(true);
        if has_explicit_include || is_root {
            let include_paths = if let Some(ec) = entity_cfg {
                resolve_include_paths_for_topology(
                    db,
                    config,
                    domain,
                    schema_title,
                    ec.allow_include.as_ref(),
                    project.is_workers_topology(),
                )
                .await?
            } else {
                Vec::new()
            };

            if !include_paths.is_empty() {
                let mut dto_files = self
                    .build_include_dtos(
                        db,
                        schema_title,
                        domain,
                        config,
                        tera,
                        project,
                        &include_paths,
                    )
                    .await?;
                files.append(&mut dto_files);
            }
        }

        Ok(files)
    }
}

impl DtoGenerator {
    #[allow(clippy::too_many_arguments)]
    /// Generate include DTO files for eager-loading via `?include=` query parameter.
    async fn build_include_dtos(
        &self,
        db: &dyn GraphQuerier,
        schema_title: &str,
        domain: &str,
        _config: &DomainConfig,
        tera: &tera::Tera,
        project: &ProjectConfig,
        include_paths: &[ResolvedIncludePath],
    ) -> Result<Vec<GeneratedFile>> {
        let schema = db
            .get_schema_in_domain(schema_title, domain)
            .await?
            .ok_or_else(|| crate::error::Error::SchemaNotFound(schema_title.into()))?;

        // Issue #268: gated module dir + registered path segments must
        // match the dto_response registration above.
        let ns_dir = crate::namespace_dir_prefix(schema.namespace.as_deref(), project);
        let ns_rust = crate::namespace_rust_prefix(schema.namespace.as_deref(), project);
        let domain_dir_segment = ns_dir.clone().unwrap_or_else(|| domain.to_string());

        let entity_name = schema.rust_type_name.clone();
        let module_name = schema.pg_table_name.clone();

        let has_includes = !include_paths.is_empty();
        let has_dot_paths = include_paths.iter().any(|p| p.segments.len() > 1);

        // Build include_fields for the template, deduplicating by alias to prevent
        // duplicate struct field names when multiple include paths resolve to the same alias.
        // Dot-notation paths are grouped by their first segment key (e.g. "deployment.position"
        // and "deployment.organization" both become "deployment") — only the first path per
        // group contributes an IncludedData field, matching UI access patterns.
        let include_fields = build_include_fields(include_paths);

        // Query all properties for dot-notation intermediate entity fields
        let all_props = db.list_all_properties().await?;

        // Build enriched_types for dot-notation paths, grouped by first segment.
        // Paths like "deployment.position" and "deployment.organization" share the same
        // intermediate entity and produce a single combined DTO (DeploymentCombinedResponse)
        // with one optional nested field per leaf.
        let enriched_types = build_enriched_types(db, domain, include_paths, &all_props).await?;

        // Register include DTO types for cross-generator import resolution.
        register_include_types(
            &entity_name,
            domain,
            &module_name,
            include_paths,
            ns_rust.as_deref(),
        );

        // Collect all type names referenced by include fields for cross-entity import resolution.
        let mut ref_type_names = collect_ref_type_names(&include_fields, &enriched_types);
        // Also add framework types referenced by the template.
        ref_type_names.push(format!("{}LinkedResponse", entity_name));
        ref_type_names.push("Meta".into());
        let caller_ns = ns_rust.clone();
        let caller_module: Vec<String> = match &caller_ns {
            Some(ns) => {
                let mut segs: Vec<String> = format!("crate::domain::{ns}")
                    .split("::")
                    .map(str::to_string)
                    .collect();
                segs.push(module_name.clone());
                segs.push("dto_included".into());
                segs
            }
            None => vec![
                "crate".into(),
                "domain".into(),
                domain.into(),
                module_name.clone(),
                "dto_included".into(),
            ],
        };
        let imports = type_registry::resolve_imports(&ref_type_names, &caller_module);

        // Collect codelist enum types referenced by compound DTO base fields.
        let codelist_imports = collect_codelist_imports(&enriched_types);

        // Only emit `use uuid::Uuid;` when an enriched base field actually
        // references the bare `Uuid` type (otherwise the import is dead code).
        let needs_uuid = needs_uuid_import(&enriched_types);

        let ctx = serde_json::json!({
            "entity_name": entity_name,
            "module_name": module_name,
            "domain": domain,
            "has_includes": has_includes,
            "has_dot_paths": has_dot_paths,
            "include_fields": include_fields,
            "enriched_types": enriched_types,
            "imports": imports,
            "codelist_imports": codelist_imports,
            "needs_uuid": needs_uuid,
        });

        let content = render_template_with_project(tera, "ddd/dto_included.tera", &ctx, project)?;

        Ok(vec![GeneratedFile {
            path: self
                .output_dir
                .join("src")
                .join("domain")
                .join(domain_dir_segment)
                .join(&module_name)
                .join("dto_included.rs"),
            content,
        }])
    }
}

/// Resolve the field alias for an include path: for dot-notation paths use the
/// first segment (e.g. "deployment" for "deployment.position"), otherwise the
/// single-segment alias directly.
fn field_alias_for(path: &ResolvedIncludePath) -> String {
    if path.segments.len() > 1 {
        path.segments[0].module_name.clone()
    } else {
        path.alias.clone()
    }
}

/// Build include_fields for the dto_included template.
fn build_include_fields(include_paths: &[ResolvedIncludePath]) -> Vec<serde_json::Value> {
    let mut seen_aliases = std::collections::HashSet::new();
    include_paths
        .iter()
        .filter(|path| seen_aliases.insert(field_alias_for(path)))
        .map(|path| {
            let (rust_type, inner_type, is_vec) = if path.segments.len() == 1 {
                let seg = &path.segments[0];
                // When the segment has a child_table_override (VO→entity),
                // the response type is the child DTO (e.g. WorkerPersonLegalResponse),
                // not the entity's response type (PersonResponse).
                let resp_type = seg
                    .child_table_override
                    .as_ref()
                    .map(|o| o.response_type.clone())
                    .unwrap_or_else(|| format!("{}Response", seg.entity_name));
                if seg.is_array {
                    (format!("Option<Vec<{}>>", resp_type), resp_type, true)
                } else {
                    (format!("Option<{}>", resp_type), resp_type, false)
                }
            } else {
                // Dot-notation paths use the combined DTO type name.
                let combined_name = format!("{}CombinedResponse", path.segments[0].entity_name);
                (format!("Option<{}>", combined_name), combined_name, false)
            };

            let field_alias = field_alias_for(path);
            // For dot-notation paths, field_alias is the first segment (e.g. "deployment")
            // which doesn't need serde renaming — the Rust field name is the JSON key.
            // Only apply rename when field_alias has underscore-for-dot substitution
            // that changes the Rust identifier from the API key (single-segment paths).
            let needs_serde_rename = path.segments.len() == 1 && field_alias != path.alias;
            serde_json::json!({
                "alias": path.alias,
                "field_alias": field_alias,
                "serde_rename": if needs_serde_rename { Some(&path.alias) } else { None },
                "rust_type": rust_type,
                "inner_type": inner_type,
                "is_vec": is_vec,
                "is_dot_path": path.segments.len() > 1,
            })
        })
        .collect()
}

async fn build_enriched_types(
    db: &dyn GraphQuerier,
    domain: &str,
    include_paths: &[ResolvedIncludePath],
    all_props: &HashMap<String, Vec<PropertyNode>>,
) -> Result<Vec<serde_json::Value>> {
    if !include_paths.iter().any(|p| p.segments.len() > 1) {
        return Ok(Vec::new());
    }
    // Group dot-notation paths by first segment.
    let mut by_first_seg: std::collections::HashMap<String, Vec<&ResolvedIncludePath>> =
        std::collections::HashMap::new();
    for path in include_paths {
        if path.segments.len() > 1 {
            let key = path.segments[0].module_name.clone();
            by_first_seg.entry(key).or_default().push(path);
        }
    }

    let mut enriched = Vec::new();
    for group_paths in by_first_seg.values() {
        enriched.push(build_combined_group(db, domain, group_paths, all_props).await?);
    }
    Ok(enriched)
}

async fn build_combined_group(
    db: &dyn GraphQuerier,
    domain: &str,
    group_paths: &[&ResolvedIncludePath],
    all_props: &HashMap<String, Vec<PropertyNode>>,
) -> Result<serde_json::Value> {
    let first_path = group_paths[0];
    let intermediate = &first_path.segments[0];

    // Build the combined type name: DeploymentCombinedResponse
    let combined_name = format!("{}CombinedResponse", intermediate.entity_name);

    let mut base_fields: Vec<serde_json::Value> = Vec::new();
    base_fields.push(serde_json::json!({"name": "id", "rust_type": "Uuid", "is_optional": false}));

    let props_key = match db
        .get_schema_in_domain(&intermediate.schema_title, domain)
        .await?
    {
        Some(s) => Some(s.title),
        None => {
            let with_type = format!("{}Type", intermediate.entity_name);
            db.get_schema_in_domain(&with_type, domain)
                .await?
                .map(|s| s.title)
        }
    };
    if let Some(ref key) = props_key
        && let Some(props) = all_props.get(key)
    {
        for prop in props {
            if prop.rust_field_name == "id"
                || prop.rust_field_name == "created_at"
                || prop.rust_field_name == "updated_at"
            {
                continue;
            }
            // Skip ValueObject properties (not direct columns).
            if matches!(
                prop.effective_kind(),
                Some(RefClassificationKind::ValueObject)
            ) {
                continue;
            }
            let is_optional = prop.is_nullable || !prop.is_required;
            let field_type = dot_field_type(prop, is_optional);
            let fd = codegraph_core::types::resolve_field(prop);
            base_fields.push(serde_json::json!({
                "name": fd.rust_field_name,
                "rust_type": field_type,
                "is_optional": is_optional,
            }));
        }
    }

    base_fields.push(serde_json::json!({"name": "created_at", "rust_type": "chrono::DateTime<chrono::Utc>", "is_optional": false}));
    base_fields.push(serde_json::json!({"name": "updated_at", "rust_type": "chrono::DateTime<chrono::Utc>", "is_optional": false}));

    // Add one nested field per leaf in the group.
    let mut nested_fields: Vec<serde_json::Value> = Vec::new();
    for leaf_path in group_paths {
        let leaf = &leaf_path.segments[leaf_path.segments.len() - 1];
        nested_fields.push(serde_json::json!({
            "alias": leaf.module_name,
            "rust_type": format!("Option<{}Response>", leaf.entity_name),
            "inner_type": format!("{}Response", leaf.entity_name),
            "is_vec": false,
        }));
    }

    Ok(serde_json::json!({
        "type_name": combined_name,
        "base_fields": base_fields,
        "nested_fields": nested_fields,
    }))
}

fn dot_field_type(prop: &PropertyNode, is_optional: bool) -> String {
    if matches!(
        prop.effective_kind(),
        Some(RefClassificationKind::StructuredWrapper)
    ) {
        let base = if prop.is_array {
            "Vec<serde_json::Value>"
        } else {
            "serde_json::Value"
        };
        if is_optional {
            format!("Option<{base}>")
        } else {
            base.to_string()
        }
    } else if matches!(
        prop.effective_kind(),
        Some(RefClassificationKind::EntityReference)
    ) {
        if is_optional {
            "Option<uuid::Uuid>".to_string()
        } else {
            "uuid::Uuid".to_string()
        }
    } else if matches!(
        prop.effective_kind(),
        Some(RefClassificationKind::CodelistReference | RefClassificationKind::CodelistCheck)
    ) {
        let enum_type =
            codelist_enum_name_from_ref(&prop.ref_target).unwrap_or_else(|| "String".to_string());
        if is_optional {
            format!("Option<{}>", enum_type)
        } else {
            enum_type
        }
    } else {
        let raw = prop.rust_field_type.clone();
        if is_optional && !raw.starts_with("Option<") && !raw.starts_with("Vec<Option<") {
            format!("Option<{}>", raw)
        } else {
            raw
        }
    }
}

/// True when the field's Rust type can carry garde `length` semantics
/// (strings and collections — the types implementing garde's `Simple`
/// length rule). `Option<...>` wrappers are unwrapped first: the DTO
/// template validates inner values via `#[garde(inner(length(...)))]`.
/// Without this gate, JSON `minLength` on e.g. a `format: date-time`
/// property emits `#[garde(length(...))]` on a `chrono::DateTime<Utc>`
/// field, which does not compile.
fn supports_garde_length(rust_type: &str) -> bool {
    let t = rust_type.trim();
    let t = t
        .strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
        .map(str::trim)
        .unwrap_or(t);
    t.starts_with("String")
        || t.starts_with("Vec<")
        || t.starts_with("VecDeque<")
        || t.contains("HashMap<")
        || t.contains("BTreeMap<")
        || t.contains("HashSet<")
        || t.contains("BTreeSet<")
        || t.contains("LinkedList<")
        || t.contains("BinaryHeap<")
}

#[allow(clippy::too_many_arguments)]
fn register_include_types(
    entity_name: &str,
    domain: &str,
    module_name: &str,
    include_paths: &[ResolvedIncludePath],
    ns_rust: Option<&str>,
) {
    // Issue #268: this entity's OWN module registrations follow its
    // namespace-derived path under namespace_layout; cross-target
    // registrations (single-segment paths resolving to the TARGET entity's
    // dto_response module) stay domain-flat — correct whenever the target
    // is namespace-less; fully-namespaced include targets are a #268
    // follow-up (they need the target's namespace at include-path
    // resolution time).
    let own_ns: Vec<String> = match ns_rust {
        Some(ns) => format!("crate::domain::{ns}")
            .split("::")
            .map(str::to_string)
            .collect(),
        None => vec!["crate".into(), "domain".into(), domain.into()],
    };
    let mut own_path = own_ns.clone();
    own_path.push(module_name.to_string());
    own_path.push("dto_included".into());
    let mut own_response_path = own_ns;
    own_response_path.push(module_name.to_string());
    own_response_path.push("dto_response".into());
    type_registry::register_type(
        &format!("{}WithIncludeResponse", entity_name),
        own_path.clone(),
    );
    type_registry::register_type(&format!("{}IncludedData", entity_name), own_path.clone());
    for path in include_paths {
        let type_name = if path.segments.len() > 1 {
            format!("{}CombinedResponse", path.segments[0].entity_name)
        } else {
            path.response_rust_type.clone()
        };
        // Register the include response type under the module that
        // DEFINES it, or import resolution poisons the registry with a
        // self-referencing path (first registration wins):
        // - multi-segment combined DTOs are defined in this entity's
        //   dto_included module;
        // - VO→entity child-table overrides resolve to this entity's
        //   dto_response module, where its child DTOs are registered;
        // - plain single-segment paths resolve to the target entity's
        //   dto_response module ({Target}Response is defined there).
        let defining_module = match path.segments.len() {
            1 => {
                if path.segments[0].child_table_override.is_some() {
                    // This entity's dto_response module, where its
                    // child DTOs are registered.
                    own_response_path.clone()
                } else {
                    let seg = &path.segments[0];
                    vec![
                        "crate".into(),
                        "domain".into(),
                        seg.domain.clone(),
                        seg.module_name.clone(),
                        "dto_response".into(),
                    ]
                }
            }
            _ => own_path.clone(),
        };
        type_registry::register_type(&type_name, defining_module);
    }
}

fn collect_ref_type_names(
    include_fields: &[serde_json::Value],
    enriched_types: &[serde_json::Value],
) -> Vec<String> {
    let mut ref_type_names: Vec<String> = Vec::new();
    for field in include_fields {
        if let Some(inner) = field["inner_type"].as_str() {
            ref_type_names.push(inner.to_string());
        }
    }
    for et in enriched_types {
        if let Some(nested) = et["nested_fields"].as_array() {
            for nf in nested {
                if let Some(inner) = nf["inner_type"].as_str() {
                    ref_type_names.push(inner.to_string());
                }
            }
        }
        // Also add base_fields' response type references (e.g.,
        // CertificationResponse, IdentifierResponse) so they are
        // imported.  Strip Option<...> and Vec<...> wrappers to
        // extract the inner type name.
        if let Some(base) = et["base_fields"].as_array() {
            for bf in base {
                if let Some(rt) = bf["rust_type"].as_str() {
                    let inner = rt
                        .strip_prefix("Option<")
                        .or_else(|| rt.strip_prefix("Vec<"))
                        .and_then(|s| s.strip_suffix('>'))
                        .unwrap_or(rt);
                    // Skip codelist enum types — they are referenced via
                    // `pub use crate::codelist::…` re-exports, and resolving
                    // them to their dto_response module would emit an
                    // import the generated body never uses.
                    let is_codelist = (inner.ends_with("CodeList") || inner.ends_with("Code"))
                        && !inner.contains('<');
                    if !is_codelist {
                        // Add unwrapped types (e.g. "JobResponse") and
                        // wrapped types (e.g. "CertificationResponse" from
                        // "Vec<CertificationResponse>") to the import list.
                        ref_type_names.push(inner.to_string());
                    }
                }
            }
        }
    }
    ref_type_names
}

fn collect_codelist_imports(enriched_types: &[serde_json::Value]) -> Vec<String> {
    let mut codelist_imports: Vec<String> = Vec::new();
    for et in enriched_types {
        if let Some(base_fields) = et["base_fields"].as_array() {
            for bf in base_fields {
                if let Some(rust_type) = bf["rust_type"].as_str() {
                    let ty = rust_type
                        .strip_prefix("Option<")
                        .and_then(|s| s.strip_suffix('>'))
                        .unwrap_or(rust_type);
                    // Codelist enum types end with "CodeList" or "StatusCode" etc.
                    if (ty.ends_with("CodeList") || ty.ends_with("Code"))
                        && !ty.contains('<')
                        && !codelist_imports.contains(&ty.to_string())
                    {
                        codelist_imports.push(ty.to_string());
                    }
                }
            }
        }
    }
    codelist_imports
}

fn needs_uuid_import(enriched_types: &[serde_json::Value]) -> bool {
    enriched_types.iter().any(|et| {
        et["base_fields"].as_array().is_some_and(|bfs| {
            bfs.iter().any(|bf| {
                bf["rust_type"]
                    .as_str()
                    .map(|rt| rt == "Uuid" || rt == "Option<Uuid>")
                    .unwrap_or(false)
            })
        })
    })
}

/// Strips `_code` suffix from a codelist field name, unless the result
/// would be a Rust keyword (e.g. `type_code` → `type` is invalid).
/// Strip outer `Vec<T>` wrapper, returning `T`. If the type doesn't start
/// with `Vec<`, returns it unchanged.
pub(crate) fn strip_vec_wrapper(ty: &str) -> String {
    if let Some(inner) = ty.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')) {
        inner.to_string()
    } else {
        ty.to_string()
    }
}

pub(crate) fn strip_option_wrapper(ty: &str) -> String {
    if let Some(inner) = ty.strip_prefix("Option<").and_then(|s| s.strip_suffix('>')) {
        inner.to_string()
    } else {
        ty.to_string()
    }
}

pub fn strip_code_suffix_safe(name: &str) -> String {
    const RUST_KEYWORDS: &[&str] = &[
        "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn",
        "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
        "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe",
        "use", "where", "while", "async", "await", "dyn", "abstract", "become", "box", "do",
        "final", "macro", "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
    ];
    match name.strip_suffix("_code") {
        Some(stripped) if !RUST_KEYWORDS.contains(&stripped) => stripped.to_string(),
        _ => name.to_string(),
    }
}

/// Extract the codelist enum name from a `ref_target` value.
///
/// Handles both clean names (`"GenderCodeList"`) and path-style references
/// (`"common/json/codelist/GenderCodeList.json"`).
/// Returns `None` when `ref_target` is `None` or empty.
pub(crate) fn codelist_enum_name_from_ref(ref_target: &Option<String>) -> Option<String> {
    codegraph_core::types::codelist_enum_name_from_ref(ref_target)
}
