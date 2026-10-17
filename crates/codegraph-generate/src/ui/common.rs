use std::collections::HashSet;

use codegraph_config::DomainConfig;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::PropertyNode;
use codegraph_naming;
use codegraph_type_contracts::RefClassificationKind;

use super::form::{field_name_to_label, ui_field_from_property};
use super::page::{ChildSection, UiField};
use crate::api::api_model::{resolve_path_segment, resolve_path_segment_with_config};
use crate::api::router;
use crate::error::Result;

/// Collects UI fields from graph properties, applying standard classification
/// and codelist value resolution. Shared across page, form, and type generators.
pub async fn collect_ui_fields(
    db: &dyn GraphQuerier,
    schema_title: &str,
    immutable_fields: &[String],
    current_domain: Option<&str>,
    config: &DomainConfig,
) -> Result<Vec<UiField>> {
    let all_props = match current_domain {
        Some(domain) => db.get_properties_in_domain(schema_title, domain).await?,
        None => db.get_properties(schema_title).await?,
    };
    let props = {
        let mut seen = std::collections::HashSet::new();
        all_props
            .into_iter()
            .filter(|p| seen.insert(p.rust_field_name.clone()))
            .collect::<Vec<_>>()
    };
    // Query composite range for this schema (if any)
    let composite_range = db.get_composite_range(schema_title).await.ok().flatten();

    let mut fields = Vec::new();

    for prop in &props {
        if prop.effective_kind() == Some(RefClassificationKind::ValueObject) {
            // Resolve the target schema and emit a nested type reference.
            let target_schema = if prop.is_array {
                db.get_array_item_schema(&prop.name, schema_title).await?
            } else {
                db.get_property_ref_target(&prop.name, schema_title).await?
            };
            if let Some(target) = target_schema {
                let source_entity_name = router::strip_suffix(schema_title, "Type");
                // TS interface name for the parent type to reference, e.g. "WorkerPersonLegalResponse"
                let ts_type_name = format!(
                    "{}{}Response",
                    codegraph_naming::to_pascal_case(source_entity_name),
                    router::strip_suffix(&target.rust_type_name, "Type"),
                );
                // Schema title for the type generator to resolve sub-fields, e.g. "PersonLegalType"
                let schema_title_for_ref = target.title.clone();
                // Strip Rust r# prefix — it is only needed for Rust identifiers,
                // not for TypeScript / Svelte property access.
                let ts_name = prop
                    .rust_field_name
                    .strip_prefix("r#")
                    .unwrap_or(&prop.rust_field_name)
                    .to_string();
                // Query structured sub-fields so the form generator renders nested
                // ValueObjects (e.g. JurisdictionType → city, country_code, ...)
                // using StructuredWrapperField instead of a plain text input.
                let raw_sub_fields = db
                    .get_structured_sub_fields(&schema_title_for_ref)
                    .await
                    .unwrap_or_default();
                let sub_fields: Vec<super::page::UiSubField> = raw_sub_fields
                    .iter()
                    .enumerate()
                    .map(|(i, sf)| {
                        let snake_name = codegraph_naming::to_snake_case(&sf.name);
                        let label = super::form::field_name_to_label(&snake_name);
                        super::page::UiSubField {
                            name: sf.name.clone(),
                            snake_name,
                            label,
                            is_required: sf.is_required,
                            description: sf.description.clone(),
                            show_by_default: sf.is_required || i < 2,
                        }
                    })
                    .collect();
                fields.push(UiField {
                    name: ts_name.clone(),
                    label: field_name_to_label(&ts_name),
                    ts_type: ts_type_name,
                    input_type: "text".to_string(),
                    is_required: prop.is_required,
                    is_array: prop.is_array,
                    is_entity_ref: false,
                    is_immutable: false,
                    is_codelist: false,
                    is_range: false,
                    codelist_values: vec![],
                    description: prop.description.clone().unwrap_or_default(),
                    pg_type: prop.pg_column_type.clone(),
                    open_end: false,
                    ref_api_path: None,
                    structured_sub_fields: sub_fields,
                    nested_type_name: Some(schema_title_for_ref),
                });
            }
            continue;
        }

        // Expand CompositeWrappers (e.g., AmountType → amount + amount_currency)
        // into their constituent UI fields so they appear on forms.
        if prop.effective_kind() == Some(RefClassificationKind::CompositeWrapper) {
            if let Ok(comp_cols) = db.get_composite_columns(&prop.name, schema_title).await {
                for col in &comp_cols {
                    let base_name = prop
                        .rust_field_name
                        .strip_prefix("r#")
                        .unwrap_or(&prop.rust_field_name);
                    let field_name = format!("{}{}", base_name, col.suffix);
                    let label = super::form::field_name_to_label(&field_name);
                    let is_codelist = col.dto_rust_type.is_some();
                    let codelist_values = if is_codelist {
                        if let Some(ref dto_type) = col.dto_rust_type {
                            // dto_rust_type is the codelist enum name (e.g., "CurrencyCodeList")
                            // which matches the graph's CodeList node name.
                            db.get_enum_values(dto_type)
                                .await
                                .unwrap_or_default()
                                .into_iter()
                                .map(|ev| ev.value)
                                .collect()
                        } else {
                            Vec::new()
                        }
                    } else {
                        Vec::new()
                    };
                    let input_type = if is_codelist {
                        "select".to_string()
                    } else if col.rust_type.contains("Decimal") || col.rust_type.contains("f64") {
                        "number".to_string()
                    } else {
                        "text".to_string()
                    };
                    fields.push(UiField {
                        name: field_name,
                        label,
                        ts_type: "string".to_string(),
                        input_type,
                        is_required: prop.is_required,
                        is_array: false,
                        is_entity_ref: false,
                        is_immutable: false,
                        is_codelist,
                        is_range: false,
                        codelist_values,
                        description: String::new(),
                        pg_type: col.pg_type.clone(),
                        open_end: false,
                        ref_api_path: None,
                        structured_sub_fields: vec![],
                        nested_type_name: None,
                    });
                }
            }
            continue;
        }

        // StructuredWrapper: single JSONB column — query sub-fields from graph
        // and embed them in UiField for the template to render generically.
        if prop.effective_kind() == Some(RefClassificationKind::StructuredWrapper) {
            // Derive the wrapper schema title from the property's ref_target.
            // ref_target formats: "../../common/json/base/IdentifierType.json"
            //                     "#/definitions/IdentifierType"
            //                     "IdentifierType"
            let wrapper_schema = prop
                .ref_target
                .as_ref()
                .map(|t| {
                    let last = t.rsplit('/').next().unwrap_or(t.as_str());
                    last.strip_suffix(".json#")
                        .or_else(|| last.strip_suffix(".json"))
                        .unwrap_or(last)
                        .to_string()
                })
                .unwrap_or_default();

            let raw_sub_fields = db
                .get_structured_sub_fields(&wrapper_schema)
                .await
                .unwrap_or_default();

            let sub_fields: Vec<super::page::UiSubField> = raw_sub_fields
                .iter()
                .enumerate()
                .map(|(i, sf)| {
                    let snake_name = codegraph_naming::to_snake_case(&sf.name);
                    let label = super::form::field_name_to_label(&snake_name);
                    super::page::UiSubField {
                        name: sf.name.clone(),
                        snake_name,
                        label,
                        is_required: sf.is_required,
                        description: sf.description.clone(),
                        show_by_default: sf.is_required || i < 2,
                    }
                })
                .collect();

            let mut field = ui_field_from_property(
                prop,
                false, // is_entity_ref
                false, // is_codelist
                &[],   // codelist_values
                immutable_fields,
                &prop.pg_column_type,
                false, // is_range
                false, // open_end
            );
            field.structured_sub_fields = sub_fields;
            fields.push(field);
            continue;
        }

        let is_entity_ref = prop.effective_kind() == Some(RefClassificationKind::EntityReference);
        let is_codelist = matches!(
            prop.effective_kind(),
            Some(RefClassificationKind::CodelistReference)
                | Some(RefClassificationKind::CodelistCheck)
        );

        let codelist_values = resolve_codelist_values(db, prop, is_codelist).await;

        // Determine pg_type, is_range, open_end from PropertyNode and composite range
        let pg_type = prop.pg_column_type.clone();
        let is_range = pg_type.contains("RANGE");
        let open_end = if is_range {
            composite_range
                .as_ref()
                .map(|r| r.open_end)
                .unwrap_or(false)
        } else {
            false
        };

        let mut field = ui_field_from_property(
            prop,
            is_entity_ref,
            is_codelist,
            &codelist_values,
            immutable_fields,
            &pg_type,
            is_range,
            open_end,
        );

        // Resolve entity reference API paths for test dependency creation
        if is_entity_ref {
            // Prefer the graph-resolved target schema: it handles `.schema.json`
            // ref stems and filename/title divergence (e.g. `eta.schema.json`
            // vs title "Estimated Time of Completion") that string parsing
            // cannot. Fall back to parsing the raw `$ref` string when no graph
            // edge is recorded.
            let mut resolved = if prop.is_array {
                db.get_array_item_schema(&prop.name, schema_title)
                    .await
                    .ok()
                    .flatten()
            } else {
                db.get_property_ref_target(&prop.name, schema_title)
                    .await
                    .ok()
                    .flatten()
            };
            if resolved.is_none()
                && let Some(ref target) = prop.ref_target
            {
                // Extract the type name from various ref formats:
                //   "../../common/json/organization.schema.json" -> "organization"
                //   "../../common/json/OrganizationType.json#"     -> "OrganizationType"
                //   "#/definitions/AssessmentAccessType"           -> "AssessmentAccessType"
                //   "SomeType"                                     -> "SomeType"
                let last_segment = target.rsplit('/').next().unwrap_or(target);
                let ref_schema_title = last_segment
                    .strip_suffix(".schema.json")
                    .or_else(|| last_segment.strip_suffix(".json#"))
                    .or_else(|| last_segment.strip_suffix(".json"))
                    .unwrap_or(last_segment);
                if let Ok(Some(ref_schema)) = db
                    .get_schema_in_domain(ref_schema_title, current_domain.unwrap_or(""))
                    .await
                {
                    resolved = Some(ref_schema);
                }
                if resolved.is_none()
                    && let Ok(Some(ref_schema)) = db.get_schema(ref_schema_title).await
                {
                    resolved = Some(ref_schema);
                }
            }
            // If the resolved schema is in a different domain, check if the
            // same type exists in the current domain and prefer it.
            if let (Some(cur_domain), Some(found)) = (current_domain, &resolved)
                && found.domain.as_deref() != Some(cur_domain)
                && let Ok(schemas) = db.list_schemas(Some(cur_domain)).await
                && let Some(same_domain) = schemas.iter().find(|s| s.title == found.title)
            {
                resolved = Some(same_domain.clone());
            }
            if let Some(ref_schema) = resolved
                && let Some(ref domain) = ref_schema.domain
            {
                field.ref_api_path = Some(format!(
                    "/{}/{}",
                    domain,
                    resolve_path_segment_with_config(None, &ref_schema, config)
                ));
            }
        }

        fields.push(field);
    }

    // Deduplicate by field name — CompositeWrapper expansion may produce
    // a field with the same name as a direct property (e.g., "language").
    let mut seen = std::collections::HashSet::new();
    fields.retain(|f| seen.insert(f.name.clone()));

    // If no UI fields were found from graph properties, check if this is a
    // codelist entity (enum-only schema with no properties). Inject synthetic
    // fields for code, display_name, and sort_order so the form renders inputs
    // and the CRUD tests can create the entity.
    if fields.is_empty()
        && let Some(domain) = current_domain
        && let Ok(Some(schema)) = db.get_schema_in_domain(schema_title, domain).await
        && schema.is_codelist
        && domain == "common"
    {
        let mut inject = |name: &str, label: &str, input_type: &str, is_required: bool| {
            fields.push(UiField {
                name: name.to_string(),
                label: label.to_string(),
                ts_type: "string".to_string(),
                input_type: input_type.to_string(),
                is_required,
                is_array: false,
                is_entity_ref: false,
                is_immutable: false,
                is_codelist: false,
                is_range: false,
                codelist_values: vec![],
                description: String::new(),
                pg_type: "TEXT".to_string(),
                open_end: false,
                ref_api_path: None,
                structured_sub_fields: vec![],
                nested_type_name: None,
            });
        };
        inject("code", "Code", "code", true);
        inject("display_name", "Display Name", "text", true);
        inject("sort_order", "Sort Order", "number", false);
    }

    Ok(fields)
}

const PREFERRED_DISPLAY_FIELDS: [&str; 6] = [
    "display_name",
    "full_name",
    "name",
    "title",
    "label",
    "code",
];

const NON_DISPLAY_FIELD_NAMES: [&str; 4] = ["id", "created_at", "updated_at", "deleted_at"];

fn is_display_string_scalar(field: &UiField) -> bool {
    field.ts_type == "string"
        && !field.is_array
        && !field.is_range
        && !field.is_entity_ref
        && field.structured_sub_fields.is_empty()
        && field.nested_type_name.is_none()
}

pub(crate) fn resolve_display_field(fields: &[UiField]) -> Option<String> {
    for candidate in PREFERRED_DISPLAY_FIELDS {
        if let Some(field) = fields
            .iter()
            .find(|f| f.name == candidate && is_display_string_scalar(f))
        {
            return Some(field.name.clone());
        }
    }
    fields
        .iter()
        .find(|f| {
            f.is_required
                && is_display_string_scalar(f)
                && !NON_DISPLAY_FIELD_NAMES.contains(&f.name.as_str())
        })
        .map(|f| f.name.clone())
}

/// Resolves direct child tables for an entity by scanning entity_config entries
/// with `role = "child"` and `parent` matching the given schema. Returns sorted
/// `ChildSection` entries for use in detail-page accordion sections.
pub async fn collect_child_sections(
    db: &dyn GraphQuerier,
    schema_title: &str,
    domain_config: &DomainConfig,
    current_domain: &str,
) -> Result<Vec<ChildSection>> {
    let mut sections = Vec::new();
    let mut visited = HashSet::new();
    visited.insert(schema_title.to_string());

    for (domain_name, domain_entry) in &domain_config.domains {
        for (config_key, entity_cfg) in &domain_entry.entity_config {
            // Only interested in child entities whose parent matches our schema
            let is_child = entity_cfg
                .role
                .as_deref()
                .map(|r| r == "child")
                .unwrap_or(false);
            if !is_child {
                continue;
            }
            let parent_matches = entity_cfg
                .parent
                .as_deref()
                .map(|p| p == schema_title)
                .unwrap_or(false);
            if !parent_matches {
                continue;
            }

            // Prevent cycles
            if !visited.insert(config_key.clone()) {
                continue;
            }

            // Resolve the child schema from the graph
            let child_schema = match db.get_schema_in_domain(config_key, current_domain).await? {
                Some(s) => s,
                None => continue,
            };

            let entity_name = child_schema.rust_type_name.clone();
            let module_name = child_schema.pg_table_name.clone();
            if module_name.is_empty() {
                continue;
            }

            let path_segment = resolve_path_segment(Some(entity_cfg), &child_schema);
            let label = field_name_to_label(&entity_name);

            // Collect scalar fields for the child
            let immutable_fields: Vec<String> = Vec::new();
            let fields = collect_ui_fields(
                db,
                config_key,
                &immutable_fields,
                Some(domain_name),
                domain_config,
            )
            .await?;

            // Check if this child has its own children (for nested accordions)
            let grandchildren = db.get_child_schemas(config_key).await.unwrap_or_default();
            let has_children = !grandchildren.is_empty();

            sections.push(ChildSection {
                entity_name,
                module_name: module_name.clone(),
                label,
                path_segment,
                domain: domain_name.clone(),
                has_children,
                display_field: resolve_display_field(&fields),
                fields,
                tid: crate::ui::testids::for_module(&module_name),
            });
        }
    }

    // Also check graph-level child schemas (inline $defs) that may not be in
    // entity_config but are structurally children.
    if let Ok(graph_children) = db.get_child_schemas(schema_title).await {
        for child in graph_children {
            if !visited.insert(child.title.clone()) {
                continue;
            }
            // Only include children that have a table name (entities, not VOs)
            if child.pg_table_name.is_empty() {
                continue;
            }
            let entity_name = child.rust_type_name.clone();
            let label = field_name_to_label(&entity_name);
            let path_segment = resolve_path_segment(None, &child);
            let immutable_fields: Vec<String> = Vec::new();
            let fields = collect_ui_fields(
                db,
                &child.title,
                &immutable_fields,
                Some(current_domain),
                domain_config,
            )
            .await?;

            let grandchildren = db.get_child_schemas(&child.title).await.unwrap_or_default();
            let child_module = child.pg_table_name.clone();

            sections.push(ChildSection {
                entity_name,
                tid: crate::ui::testids::for_module(&child_module),
                module_name: child_module,
                label,
                path_segment,
                domain: child
                    .domain
                    .clone()
                    .unwrap_or_else(|| current_domain.to_string()),
                has_children: !grandchildren.is_empty(),
                display_field: resolve_display_field(&fields),
                fields,
            });
        }
    }

    sections.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(sections)
}

async fn resolve_codelist_values(
    db: &dyn GraphQuerier,
    prop: &PropertyNode,
    is_codelist: bool,
) -> Vec<String> {
    if !is_codelist {
        return Vec::new();
    }
    if let Some(ref target) = prop.ref_target {
        let filename = target.rsplit('/').next().unwrap_or(target);
        let cl_name = filename
            .strip_suffix(".json#")
            .or_else(|| filename.strip_suffix(".json"))
            .unwrap_or(filename);
        match db.get_enum_values(cl_name).await {
            Ok(values) => values.into_iter().map(|ev| ev.value).collect(),
            Err(e) => {
                eprintln!("warning: failed to resolve codelist values for '{cl_name}': {e}");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod display_field_tests {
    use super::*;

    fn field(name: &str, ts_type: &str, is_required: bool) -> UiField {
        UiField {
            name: name.to_string(),
            label: name.to_string(),
            ts_type: ts_type.to_string(),
            input_type: "text".to_string(),
            is_required,
            is_array: false,
            is_entity_ref: false,
            is_immutable: false,
            is_codelist: false,
            is_range: false,
            codelist_values: vec![],
            description: String::new(),
            pg_type: "TEXT".to_string(),
            open_end: false,
            ref_api_path: None,
            structured_sub_fields: vec![],
            nested_type_name: None,
        }
    }

    #[test]
    fn prefers_canonical_names_in_order() {
        let fields = vec![
            field("title", "string", true),
            field("name", "string", true),
            field("display_name", "string", true),
        ];
        assert_eq!(
            resolve_display_field(&fields).as_deref(),
            Some("display_name")
        );
        let fields = vec![field("name", "string", true), field("code", "string", true)];
        assert_eq!(resolve_display_field(&fields).as_deref(), Some("name"));
    }

    #[test]
    fn preferred_candidate_must_be_string_scalar() {
        let fields = vec![field("name", "number", true), field("code", "string", true)];
        assert_eq!(resolve_display_field(&fields).as_deref(), Some("code"));
    }

    #[test]
    fn falls_back_to_first_required_non_audit_string_scalar() {
        let fields = vec![
            field("created_at", "string", true),
            field("sort_order", "number", true),
            field("note", "string", true),
            field("remark", "string", false),
        ];
        assert_eq!(resolve_display_field(&fields).as_deref(), Some("note"));
    }

    #[test]
    fn skips_arrays_ranges_refs_and_wrappers_in_fallback() {
        let mut array_field = field("tags", "Array<string>", true);
        array_field.is_array = true;
        let mut range_field = field("validity", "string", true);
        range_field.is_range = true;
        let mut ref_field = field("owner_id", "string", true);
        ref_field.is_entity_ref = true;
        let mut wrapper = field("identifier", "string", true);
        wrapper.structured_sub_fields = vec![super::super::page::UiSubField {
            name: "value".into(),
            snake_name: "value".into(),
            label: "Value".into(),
            is_required: true,
            description: String::new(),
            show_by_default: true,
        }];
        let fields = vec![array_field, range_field, ref_field, wrapper];
        assert_eq!(resolve_display_field(&fields), None);
    }

    #[test]
    fn none_when_no_readable_field_exists() {
        let fields = vec![
            field("id", "string", true),
            field("quantity", "number", true),
        ];
        assert_eq!(resolve_display_field(&fields), None);
        assert_eq!(resolve_display_field(&[]), None);
    }
}
