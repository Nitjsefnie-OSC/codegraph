use std::collections::HashSet;

use codegraph_config::DomainConfig;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::{PropertyNode, SchemaNode};

use crate::api::api_model::resolve_path_segment_with_config;

use super::page::UiField;

/// Resolve the referenced schema for an entity-ref property via the graph,
/// falling back to parsing the raw `$ref` string when the graph has no edge.
/// Mirrors `collect_ui_fields` resolution: handles `.schema.json` stems,
/// filename/title divergence, and cross-domain refs.
pub(super) async fn resolve_ref_schema(
    db: &dyn GraphQuerier,
    prop: &PropertyNode,
    schema_title: &str,
    current_domain: Option<&str>,
) -> Option<SchemaNode> {
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
    // Final fallback: a required plain `format: uuid` scalar ending in `_id`
    // with no `$ref`/graph edge (e.g. `party.case_id`) resolves to an entity by
    // naming convention.
    if resolved.is_none() {
        resolved = resolve_convention_ref(db, prop, current_domain).await;
    }
    // Prefer a same-domain schema when the resolved one lives elsewhere.
    if let (Some(cur_domain), Some(found)) = (current_domain, &resolved)
        && found.domain.as_deref() != Some(cur_domain)
        && let Ok(schemas) = db.list_schemas(Some(cur_domain)).await
        && let Some(same_domain) = schemas.iter().find(|s| s.title == found.title)
    {
        resolved = Some(same_domain.clone());
    }
    resolved
}

/// Normalize an identifier for convention matching: keep only lowercase
/// alphanumerics, so `case`, `Case`, and `case_id`'s stem all collapse to
/// `case` (and `CaseType` would become `casetype`, which does not match).
fn normalize_convention_stem(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// The name-convention FK shape: a required, non-array scalar column with no
/// `$ref`/graph edge, whose `_id`-suffixed name claims an entity. The pg type
/// is deliberately NOT checked here — the model may declare the FK as a plain
/// string (`mox: String workerId` → pg TEXT, no `format: uuid`) and the
/// convention is name-derived. Identity columns (`is_id`) are never FKs.
fn is_convention_shape(prop: &PropertyNode) -> bool {
    if prop.is_array || !prop.is_required || prop.ref_target.is_some() || prop.is_id {
        return false;
    }
    let field = prop
        .rust_field_name
        .strip_prefix("r#")
        .unwrap_or(&prop.rust_field_name);
    field
        .strip_suffix("_id")
        .is_some_and(|stem| !stem.is_empty())
}

/// Resolve a required scalar FK column with no `$ref`/graph edge (e.g.
/// `case_id`) to its entity by naming convention: strip the `_id` suffix,
/// normalize, and find the unique `is_entity` schema whose normalized
/// `pg_table_name` or `rust_type_name` equals the stem. On multiple matches,
/// prefer a same-domain schema; return `None` if still ambiguous.
async fn resolve_convention_ref(
    db: &dyn GraphQuerier,
    prop: &PropertyNode,
    current_domain: Option<&str>,
) -> Option<SchemaNode> {
    if !is_convention_shape(prop) {
        return None;
    }
    let field = prop
        .rust_field_name
        .strip_prefix("r#")
        .unwrap_or(&prop.rust_field_name);
    let stem = field.strip_suffix("_id")?;
    let stem_norm = normalize_convention_stem(stem);
    let matches: Vec<SchemaNode> = db
        .list_schemas(None)
        .await
        .ok()?
        .into_iter()
        .filter(|s| {
            s.is_entity
                && (normalize_convention_stem(&s.pg_table_name) == stem_norm
                    || normalize_convention_stem(&s.rust_type_name) == stem_norm)
        })
        .collect();
    match matches.len() {
        0 => None,
        1 => matches.into_iter().next(),
        _ => {
            let same_domain: Vec<SchemaNode> = matches
                .into_iter()
                .filter(|s| s.domain.as_deref() == current_domain)
                .collect();
            if same_domain.len() == 1 {
                same_domain.into_iter().next()
            } else {
                None
            }
        }
    }
}

/// One convention-resolved entity ref: the marked field and its target.
pub(super) struct ConventionRefResolution {
    pub field_name: String,
    pub target: SchemaNode,
}

/// Post-process collected UI fields: mark required plain-uuid convention FK
/// columns (e.g. `case_id`) as entity refs so they participate in dependency
/// setup and `testData()` emits the dep branch instead of a literal.
///
/// Returns the resolutions so the caller can gate on target creatability
/// (the create-API check lives with the main-entity context, not here: the
/// transitive dep path wants every resolved target marked so the tolerant
/// optional-dep machinery applies).
pub(super) async fn apply_convention_refs(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    domain: &str,
    props: &[PropertyNode],
    fields: &mut [UiField],
) -> Vec<ConventionRefResolution> {
    let mut resolved = Vec::new();
    for field in fields.iter_mut() {
        if field.is_entity_ref || !field.is_required {
            continue;
        }
        let Some(prop) = find_ref_property(props, &field.name) else {
            continue;
        };
        if let Some(target) = resolve_convention_ref(db, prop, Some(domain)).await {
            field.is_entity_ref = true;
            field.ref_api_path = Some(api_path_for_schema(&target, config));
            resolved.push(ConventionRefResolution {
                field_name: field.name.clone(),
                target,
            });
        }
    }
    resolved
}

/// True when the field has the name-convention FK shape but has NO creatable
/// entity target — `testData` keeps the text sentinel and the templates emit
/// the `// fixture: convention ref '<field>' has no creatable target` gap
/// comment instead of silently shipping a mistyped literal.
///
/// Callers unmark resolved-but-not-creatable fields first, so a `true` here
/// means exactly: the name claims an entity, but there is nothing we can
/// dep-create for it.
pub(super) fn is_convention_ref_gap(field: &UiField, props: &[PropertyNode]) -> bool {
    if field.is_entity_ref
        || !field.is_required
        || field.is_codelist
        || field.is_array
        || !field.name.ends_with("_id")
    {
        return false;
    }
    find_ref_property(props, &field.name)
        .map(is_convention_shape)
        .unwrap_or(false)
}

/// Match a UI entity-ref field to its graph property. Entity-ref UI fields use
/// the `_id`-suffixed DTO name for scalars; arrays keep the raw field name.
pub(super) fn find_ref_property<'a>(
    props: &'a [PropertyNode],
    field_name: &str,
) -> Option<&'a PropertyNode> {
    let raw = field_name.strip_suffix("_id").unwrap_or(field_name);
    props.iter().find(|p| {
        p.rust_field_name == field_name
            || p.rust_field_name == raw
            || p.name == raw
            || p.pg_column_name == field_name
    })
}

pub(super) fn api_path_for_schema(schema: &SchemaNode, config: &DomainConfig) -> String {
    let domain = schema.domain.clone().unwrap_or_default();
    format!(
        "/{}/{}",
        domain,
        resolve_path_segment_with_config(None, schema, config)
    )
}

/// Allocate a unique `depIds` key, suffixing `_2`, `_3`, ... on collision.
pub(super) fn unique_dep_id(base: &str, used: &mut HashSet<String>) -> String {
    let mut candidate = base.to_string();
    let mut n = 2usize;
    while used.contains(&candidate) {
        candidate = format!("{}_{}", base, n);
        n += 1;
    }
    used.insert(candidate.clone());
    candidate
}
