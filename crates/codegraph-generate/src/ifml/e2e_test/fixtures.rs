//! Fixture payloads and TS-literal/URL-pattern helpers for the IFML e2e
//! generator (issue #317 module split).

use rex_ifml::{ComponentSpec, FormSpec};

use super::super::api_paths::{ResolvedApi, resolve_entity_api};
use super::super::context::{IfmlAction, IfmlComponent, IfmlEvent, IfmlViewContainer};
use codegraph_config::DomainConfig;
use codegraph_core::traits::GraphQuerier;

#[derive(Debug, Clone)]
pub struct Fixture {
    pub base_path: String,
    pub entries: Vec<(String, String)>,
}

impl Fixture {
    pub(crate) fn data_literal(&self) -> String {
        let inner = self
            .entries
            .iter()
            .map(|(k, v)| format!("'{k}': {v}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{{ {inner} }}")
    }
}

/// The URL route for a view, matching the route generator's svelte output
/// (`/{view-name-lowercase}`).
pub(crate) fn view_route(name: &str) -> String {
    format!("/{}", name.to_lowercase())
}

pub(crate) fn form_spec(c: &IfmlComponent) -> Option<&FormSpec> {
    match &c.spec {
        Some(ComponentSpec::Form(form)) => Some(form),
        _ => None,
    }
}

/// Resolve the API surface for an entity only when a schema for it exists in
/// the graph. Without schemas (`ifml-generate` without `--schemas`) this
/// returns `None`, which keeps API-dependent test kinds out of the output.
pub(crate) async fn schema_backed_api(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    entity: &str,
    api_version: &str,
) -> Option<ResolvedApi> {
    let in_graph = matches!(db.get_schema(entity).await, Ok(Some(_)))
        || matches!(db.get_schema(&format!("{entity}Type")).await, Ok(Some(_)));
    if !in_graph {
        return None;
    }
    resolve_entity_api(db, config, entity, api_version).await
}

/// Fixture payload entries for a component: `(field, js_literal)` pairs for
/// the component's fields, skipping primary-key and FK fields. Typed form
/// specs declare fields outside `fields`, so they are used as a fallback;
/// their per-field `values` supply the first valid value for
/// codelist/dropdown fields.
pub(crate) fn fixture_entries(c: &IfmlComponent) -> Vec<(String, String)> {
    let names: Vec<String> = if c.fields.is_empty() {
        form_spec(c)
            .map(|f| f.fields.iter().map(|f| f.name.clone()).collect())
            .unwrap_or_default()
    } else {
        c.fields.clone()
    };
    let types: std::collections::HashMap<&str, &str> = c
        .fields_with_types
        .iter()
        .map(|(f, t)| (f.as_str(), t.as_str()))
        .collect();
    let form_values: std::collections::HashMap<&str, &[String]> = form_spec(c)
        .map(|spec| {
            spec.fields
                .iter()
                .filter(|f| !f.values.is_empty())
                .map(|f| (f.name.as_str(), f.values.as_slice()))
                .collect()
        })
        .unwrap_or_default();
    names
        .iter()
        .filter(|f| f.as_str() != "id" && !f.ends_with("_id"))
        .map(|f| {
            let rust_type = types.get(f.as_str()).copied().unwrap_or("String");
            let value = match form_values.get(f.as_str()) {
                Some(values) if !values.is_empty() => format!("'{}'", js_string(&values[0])),
                _ => js_value_for_type(f, rust_type),
            };
            (f.clone(), value)
        })
        .collect()
}

/// First form field eligible for a round-trip modification: a plain string
/// field (skipping id/FK fields), with its fixture and updated values.
pub(crate) fn modifiable_field(
    c: &IfmlComponent,
    form: &FormSpec,
) -> Option<(String, String, String)> {
    let types: std::collections::HashMap<&str, &str> = c
        .fields_with_types
        .iter()
        .map(|(f, t)| (f.as_str(), t.as_str()))
        .collect();
    form.fields
        .iter()
        .filter(|f| f.name != "id" && !f.name.ends_with("_id"))
        .find(|f| {
            let rust_type = types
                .get(f.name.as_str())
                .copied()
                .unwrap_or("String")
                .to_ascii_lowercase();
            !rust_type.contains("bool")
                && ![
                    "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64",
                ]
                .iter()
                .any(|n| rust_type.contains(n))
        })
        .map(|f| {
            let original = js_value_for_type(&f.name, "String");
            let updated = format!("Updated {}", f.name);
            (f.name.clone(), original, updated)
        })
}

pub(crate) fn js_value_for_type(field: &str, rust_type: &str) -> String {
    let t = rust_type.to_ascii_lowercase();
    if t.contains("bool") {
        return "true".to_string();
    }
    if super::super::control_core::is_numeric_rust_type(&t) {
        return "42".to_string();
    }
    if t.contains("datetime") || t.contains("timestamp") {
        return "'2024-01-15T10:30:00Z'".to_string();
    }
    format!("'Test {field}'")
}

/// The first enum value of the codelist backing `field` on `entity`'s
/// schema, when the property references one. `None` keeps the generic
/// fixture value.
pub(crate) async fn codelist_fixture_value(
    db: &dyn GraphQuerier,
    entity: Option<&str>,
    field: &str,
) -> Option<String> {
    let entity = entity?;
    let schema = match db.get_schema(entity).await {
        Ok(Some(schema)) => Some(schema),
        Ok(None) => db.get_schema(&format!("{entity}Type")).await.ok()?,
        Err(_) => None,
    }?;
    let props = db.get_properties(&schema.title).await.ok()?;
    let target = props
        .iter()
        .find(|p| p.name == field)?
        .ref_target
        .as_deref()?;
    let stem = target.rsplit('/').next()?.strip_suffix(".json")?;
    let codelist = db.get_schema(stem).await.ok()??;
    if !codelist.is_codelist {
        return None;
    }
    let values = db.get_enum_values(stem).await.ok()?;
    values.into_iter().next().map(|v| v.value)
}

/// The view-level or component-level `on save`/`on submit` navigation target:
/// `(target_view, bindings)`.
pub(crate) fn save_navigation_target(
    vc: &IfmlViewContainer,
    c: &IfmlComponent,
) -> Option<(String, std::collections::HashMap<String, String>)> {
    let is_save = |e: &IfmlEvent| e.event_type == "save" || e.event_type == "submit";
    let target = |a: &IfmlAction| match a {
        IfmlAction::Navigate { target, binding } => Some((target.clone(), binding.clone())),
        _ => None,
    };
    c.events
        .iter()
        .chain(vc.events.iter())
        .find(|e| is_save(e))
        .and_then(|e| target(&e.action))
}

/// Build a `RegExp` source matching the target URL: the route plus query
/// params for each binding key (sorted, mirroring the page's `nav_url_expr`
/// ordering). Binding expressions substitute to `[^&]+`; quoted literals to
/// their escaped text.
pub(crate) fn url_pattern(
    route: &str,
    binding: &std::collections::HashMap<String, String>,
) -> String {
    let escaped_route = escape_regex(route);
    if binding.is_empty() {
        return format!("{escaped_route}$");
    }
    let mut pairs: Vec<(&String, &String)> = binding.iter().collect();
    pairs.sort();
    let params = pairs
        .iter()
        .map(|(k, v)| format!("{}={}", escape_regex(k), binding_value_pattern(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{escaped_route}\\?{params}")
}

pub(crate) fn binding_value_pattern(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        escape_regex(&trimmed[1..trimmed.len() - 1])
    } else {
        "[^&]+".to_string()
    }
}

/// URL pattern for navigation into a modal view: the `dialog=open` marker
/// is appended last, mirroring the route generator's `nav_url_expr` ordering.
pub(crate) fn url_pattern_with_dialog(
    route: &str,
    binding: &std::collections::HashMap<String, String>,
) -> String {
    if binding.is_empty() {
        return format!("{}\\?dialog=open", escape_regex(route));
    }
    format!("{}&dialog=open", url_pattern(route, binding))
}

pub(crate) fn escape_regex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if "\\.+*?()|[]{}^$#".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

pub(crate) fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

/// The inner text of a JS literal (`'draft'` → `draft`, `42` → `42`).
pub(crate) fn js_literal_inner(literal: &str) -> String {
    let trimmed = literal.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('\'') && trimmed.ends_with('\'') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn entry_inner(entries: &[(String, String)], field: &str) -> Option<String> {
    entries
        .iter()
        .find(|(name, _)| name == field)
        .map(|(_, value)| js_literal_inner(value))
        .filter(|value| !value.is_empty())
}
