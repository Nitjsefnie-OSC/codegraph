use std::collections::{HashMap, HashSet};

use crate::ifml::api_paths::id_param_from;
use crate::ifml::context::{IfmlComponent, IfmlViewContainer};
use crate::ifml::control_core;

use super::context::{
    FetchState, PageComponentContext, PageLoadComponentContext, PageLoadContext, RenderForm,
    RenderViewParam,
};
use super::render::sanitize_ident;
use super::workflow::js_quote;

/// Typed submit-payload construction for a fallback form: coerces FormData
/// string values to the bound schema's types (numbers, booleans, datetimes)
/// so the generated API accepts the body. Empty when no type information is
/// available (byte-identical untyped `formData` body).
pub(super) fn form_payload_block(c: &IfmlComponent, form: Option<&RenderForm>) -> String {
    let field_names: Vec<String> = match form {
        Some(form) if !form.fields.is_empty() => {
            form.fields.iter().map(|f| f.name.clone()).collect()
        }
        _ => c.fields.clone(),
    };
    if field_names.is_empty() {
        return String::new();
    }
    let types: HashMap<&str, &str> = c
        .fields_with_types
        .iter()
        .map(|(f, t)| (f.as_str(), t.as_str()))
        .collect();
    let input_types: HashMap<&str, &str> = form
        .map(|form| {
            form.fields
                .iter()
                .map(|f| (f.name.as_str(), f.input_type.as_str()))
                .collect()
        })
        .unwrap_or_default();
    let mut lines = Vec::new();
    for name in &field_names {
        let rust_type = types
            .get(name.as_str())
            .copied()
            .unwrap_or("String")
            .to_ascii_lowercase();
        let input_type = input_types.get(name.as_str()).copied().unwrap_or("");
        let value = format!("formData.{name}");
        let expr = if input_type == "checkbox" {
            format!("{value} === 'on'")
        } else if rust_type.contains("bool") {
            format!("{value} === '' ? null : {value} === 'true'")
        } else if is_numeric_rust_type(&rust_type) {
            format!("{value} === '' ? null : Number({value})")
        } else if control_core::is_temporal_rust_type(&rust_type)
            || matches!(input_type, "datetime-local" | "date" | "time")
        {
            format!("{value} === '' ? null : new Date(String({value})).toISOString()")
        } else {
            value
        };
        lines.push(format!("\t\tpayload.{name} = {expr};"));
    }
    let mut block = String::from("\t\tconst payload: Record<string, unknown> = {};\n");
    block.push_str(&lines.join("\n"));
    block
}

pub(crate) fn is_numeric_rust_type(rust_type: &str) -> bool {
    control_core::is_numeric_rust_type(rust_type)
}

/// Build the `+page.ts` load context. Only the first list/details/form
/// component contributes a fetch per page, mirroring the single-return load
/// contract; each fetch resolves the entity API path from the graph. View
/// parameters resolve from the query string only (views have no dynamic
/// route segments) and are returned to the page as `result.params`.
pub(super) fn build_load_context(
    api_version: &str,
    vc: &IfmlViewContainer,
    components: &[PageComponentContext],
    denial_target: &str,
    integrated: bool,
) -> PageLoadContext {
    let id_param = id_param_from(&vc.params);
    // The graph may carry duplicate HasParameter edges (see
    // ingest_parameter_definition); params dedupe by name for resolution.
    let mut seen_params: HashSet<String> = HashSet::new();
    let view_params: Vec<RenderViewParam> = vc
        .params
        .iter()
        .filter(|p| seen_params.insert(p.name.clone()))
        .map(|p| RenderViewParam {
            name: p.name.clone(),
            default: p.default.clone(),
        })
        .collect();
    let mut load_components = Vec::new();
    let mut fetch_state = FetchState::default();
    let mut has_fetch = false;

    for comp in components {
        let is_list = comp.table.is_some() || comp.component_type == "list";
        let is_details = comp.component_type == "details";
        let is_form = comp.form.is_some() || comp.component_type == "form";

        let (fetch_list, fetch_item, fetch_form) = fetch_state.next(
            is_list,
            is_details,
            is_form,
            comp.api.is_some() && !comp.entity.is_empty(),
            id_param.as_deref(),
        );
        has_fetch |= fetch_list || fetch_item || fetch_form;

        load_components.push(PageLoadComponentContext {
            name: sanitize_ident(&comp.name),
            component_type: comp.component_type.clone(),
            entity: comp.entity.clone(),
            route_name: comp.entity.to_lowercase(),
            api: comp.api.clone(),
            id_param: id_param.clone(),
            workflow: comp.workflow.is_some(),
            paginate: fetch_list && (comp.table.as_ref().map(|t| t.pagination).unwrap_or(true)),
            fetch_list,
            fetch_item,
            fetch_form,
        });
    }

    let has_workflow = load_components.iter().any(|comp| comp.workflow);
    PageLoadContext {
        api_version: api_version.to_string(),
        name: vc.name.clone(),
        components: load_components,
        has_fetch,
        has_workflow,
        view_params,
        view_roles: vc.roles.clone(),
        view_requires: vc.requires.clone(),
        guard_consts: guard_consts(vc),
        integrated,
        denial_target: denial_target.to_string(),
    }
}

/// The guard const declarations for a view, joined by newlines in
/// guard-evaluation order: capabilities first (`const viewRequires = ...;`),
/// roles second (`const viewRoles = ...;`).
fn guard_consts(vc: &IfmlViewContainer) -> String {
    let mut consts = Vec::new();
    if !vc.requires.is_empty() {
        let list = vc
            .requires
            .iter()
            .map(|c| js_quote(c))
            .collect::<Vec<_>>()
            .join(", ");
        consts.push(format!("const viewRequires = [{list}];"));
    }
    if !vc.roles.is_empty() {
        let list = vc
            .roles
            .iter()
            .map(|r| js_quote(r))
            .collect::<Vec<_>>()
            .join(", ");
        consts.push(format!("const viewRoles = [{list}];"));
    }
    consts.join("\n")
}
