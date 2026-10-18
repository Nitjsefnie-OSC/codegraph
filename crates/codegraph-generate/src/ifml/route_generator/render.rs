use std::collections::{HashMap, HashSet};

use codegraph_config::{IfmlComponentMapping, IfmlComponentMappings, SemanticRole};
use rex_ifml::{
    BinOp, ChartKind, ChartSpec, ColumnDef, ComponentSpec, Expression, FormSpec, TableSpec, UnaryOp,
};

use crate::ifml::api_paths::ResolvedApi;
use crate::ifml::context::{IfmlAction, IfmlComponent, IfmlEvent, IfmlViewContainer};
use crate::ifml::control_core;

use super::context::{
    RenderButton, RenderChart, RenderColumn, RenderContainer, RenderEvent, RenderForm,
    RenderImport, RenderInputField, RenderModal, RenderNavItem, RenderShellNav, RenderSubmit,
    RenderTable, TableLayout,
};
use super::roles::{event_role, input_field_role, is_form_component};

/// Build the submit wiring for a form component: POST for create views, PUT
/// for edit views (view carries an id param); success navigates per the
/// view's save event.
pub(super) fn build_submit(
    c: &IfmlComponent,
    api: Option<&ResolvedApi>,
    id_param: Option<&str>,
    events: &[RenderEvent],
) -> Option<RenderSubmit> {
    if !is_form_component(c) {
        return None;
    }
    let api = api?;
    let handler_name = format!("submit_{}", sanitize_ident(&c.name));
    let (url_expr, method, create_url_expr, edit_param) = match id_param {
        Some(param) if api.has_update => (
            format!("`{}/${{viewParams.{param}}}`", api.base_path),
            "PUT".to_string(),
            Some(format!("\"{}\"", api.base_path)),
            Some(param.to_string()),
        ),
        _ => (
            format!("\"{}\"", api.base_path),
            "POST".to_string(),
            None,
            None,
        ),
    };
    let navigate_url = events
        .iter()
        .find(|e| {
            e.action_kind == "navigate" && (e.event_type == "save" || e.event_type == "submit")
        })
        .map(|e| e.url_expr.clone());
    Some(RenderSubmit {
        handler_name,
        url_expr,
        method,
        create_url_expr,
        edit_param,
        navigate_url,
        client_validate: form_has_messages(c),
    })
}

/// True when any typed form field pairs a validation with a message —
/// the signal for the submit handler's client-side message check.
fn form_has_messages(c: &IfmlComponent) -> bool {
    matches!(&c.spec, Some(ComponentSpec::Form(spec)) if spec.fields.iter().any(|f| {
        !f.validations.is_empty() && !f.messages.is_empty()
    }))
}

pub(super) fn render_event(evt: &IfmlEvent, modal_targets: &HashSet<String>) -> RenderEvent {
    let (action_kind, target, binding) = match &evt.action {
        IfmlAction::Navigate { target, binding } => ("navigate", target.clone(), binding),
        IfmlAction::Refresh { target, binding } => ("refresh", target.clone(), binding),
        IfmlAction::Action(name) => ("action", name.clone(), &HashMap::new()),
        IfmlAction::Stay => ("stay", String::new(), &HashMap::new()),
    };
    let url_expr = if action_kind == "navigate" {
        let expr = nav_url_expr(&target, binding);
        if modal_targets.contains(&target) {
            with_dialog_param(&expr)
        } else {
            expr
        }
    } else {
        String::new()
    };
    RenderEvent {
        handler_name: sanitize_ident(&evt.name),
        event_type: evt.event_type.clone(),
        action_kind: action_kind.to_string(),
        target,
        url_expr,
        requires: evt.requires.clone(),
        role: event_role(&evt.event_type),
    }
}

/// Append the `dialog=open` query param marking navigation into a modal
/// view: template-literal URLs get `&dialog=open`, plain literals get
/// `?dialog=open`.
fn with_dialog_param(url_expr: &str) -> String {
    if let Some(inner) = url_expr.strip_prefix('`').and_then(|s| s.strip_suffix('`')) {
        format!("`{inner}&dialog=open`")
    } else if let Some(inner) = url_expr.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        format!("\"{inner}?dialog=open\"")
    } else {
        url_expr.to_string()
    }
}

/// Whether a `modal: true` view renders a modal wrapper: a mapped
/// `modal-view` component when one resolves, else the built-in div fallback
/// whenever a non-empty mapping pack is present. Without mappings the view
/// renders as a plain page (byte-identical output).
pub(crate) fn modal_wrapper_active(
    is_modal: bool,
    _view: &str,
    mappings: Option<&IfmlComponentMappings>,
) -> bool {
    is_modal && mappings.is_some_and(|m| !m.components.is_empty())
}

/// The modal wrapper's `data-testid`: the resolved modal-view mapping's
/// `testids.root`, else `{view}-modal`.
pub(crate) fn modal_wrapper_testid(view: &str, mappings: Option<&IfmlComponentMappings>) -> String {
    mappings
        .and_then(|m| m.resolve_by_role(view, SemanticRole::ModalView))
        .and_then(|m| m.testid("root").map(str::to_string))
        .unwrap_or_else(|| format!("{}-modal", view.to_lowercase()))
}

/// Modal wrapper context for a view container; `None` renders the plain page.
pub(super) fn modal_context(
    vc: &IfmlViewContainer,
    mappings: Option<&IfmlComponentMappings>,
) -> Option<RenderModal> {
    if !modal_wrapper_active(vc.is_modal, &vc.name, mappings) {
        return None;
    }
    let view_lower = vc.name.to_lowercase();
    let close_testid = format!("{view_lower}-modal-close");
    let mapping = mappings.and_then(|m| m.resolve_by_role(&vc.name, SemanticRole::ModalView));
    let (open_line, close_line, import) = match mapping {
        Some(m) => {
            let export = m.export_name();
            let testid = modal_wrapper_testid(&vc.name, mappings);
            (
                format!("<{export} bind:open={{dialog_open}} testid=\"{testid}\">"),
                format!("</{export}>"),
                Some(RenderImport {
                    export_name: export.to_string(),
                    import_path: m.path.clone(),
                }),
            )
        }
        None => (
            format!("<div class=\"modal\" role=\"dialog\" data-testid=\"{view_lower}-modal\">"),
            "</div>".to_string(),
            None,
        ),
    };
    Some(RenderModal {
        open_line,
        close_line,
        close_testid,
        import,
    })
}

/// Whether an xor view container renders a presentation-container wrapper:
/// a mapped `presentation-container` component when one resolves, else the
/// built-in section fallback whenever a non-empty mapping pack is present.
/// Without mappings the view renders as a plain page (byte-identical output).
pub(crate) fn container_wrapper_active(
    is_xor: bool,
    mappings: Option<&IfmlComponentMappings>,
) -> bool {
    is_xor && mappings.is_some_and(|m| !m.components.is_empty())
}

/// Resolve the presentation-container mapping for an xor view container:
/// name tier first (the container name), then the role tier.
fn resolve_container_mapping<'m>(
    vc: &IfmlViewContainer,
    mappings: Option<&'m IfmlComponentMappings>,
) -> Option<&'m IfmlComponentMapping> {
    mappings.and_then(|m| {
        m.resolve_slot(
            &vc.name,
            &vc.name,
            "",
            "",
            Some(SemanticRole::PresentationContainer),
        )
    })
}

/// The mapped container wrapper's `data-testid`: the resolved mapping's
/// `testids.root`, else `{view}-container`. `None` when the view is not xor
/// or no `presentation-container` mapping resolves (the e2e assertion gate).
pub(crate) fn mapped_container_testid(
    vc_name: &str,
    is_xor: bool,
    mappings: Option<&IfmlComponentMappings>,
) -> Option<String> {
    if !is_xor {
        return None;
    }
    let fallback = format!("{}-container", vc_name.to_lowercase());
    mappings
        .and_then(|m| {
            m.resolve_slot(
                vc_name,
                vc_name,
                "",
                "",
                Some(SemanticRole::PresentationContainer),
            )
        })
        .map(|m| m.testid("root").map(str::to_string).unwrap_or(fallback))
}

/// Presentation-container wrapper context for an xor view container; `None`
/// renders the plain page. Also active (without a label heading) when the
/// view nests xor containers that share the one wrapper.
pub(super) fn container_context(
    vc: &IfmlViewContainer,
    mappings: Option<&IfmlComponentMappings>,
) -> Option<RenderContainer> {
    let has_xor_children = vc.containers.iter().any(|c| c.is_xor);
    if !container_wrapper_active(vc.is_xor || has_xor_children, mappings) {
        return None;
    }
    let view_lower = vc.name.to_lowercase();
    let section_testid = format!("{view_lower}-container");
    let label_heading = vc.is_xor.then(|| {
        let label = vc.label.clone().unwrap_or_else(|| vc.name.clone());
        format!("<h2 class=\"container-label\" data-testid=\"{section_testid}-label\">{label}</h2>")
    });
    match resolve_container_mapping(vc, mappings) {
        Some(m) => {
            let export = m.export_name();
            let testid = m
                .testid("root")
                .map(str::to_string)
                .unwrap_or_else(|| section_testid.clone());
            Some(RenderContainer {
                open_line: format!("<{export} testid=\"{testid}\">"),
                close_line: format!("</{export}>"),
                testid,
                import: Some(RenderImport {
                    export_name: export.to_string(),
                    import_path: m.path.clone(),
                }),
                label_heading,
            })
        }
        None => Some(RenderContainer {
            open_line: format!("<section data-testid=\"{section_testid}\">"),
            close_line: "</section>".to_string(),
            testid: section_testid,
            import: None,
            label_heading,
        }),
    }
}

/// Root identifier of a binding value expression (`row` in `row.id`);
/// `None` for literals and empty expressions.
fn binding_root_ident(expr: &str) -> Option<&str> {
    let root: String = expr
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if root.is_empty() || root.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        None
    } else {
        Some(&expr[..root.len()])
    }
}

/// Whether a binding value expression is rooted in an event parameter —
/// such identifiers are out of scope in the landmark layout, so the nav
/// link must drop the pair instead of rendering a dead reference.
fn references_event_param(expr: &str, params: &[String]) -> bool {
    binding_root_ident(expr).is_some_and(|root| params.iter().any(|p| p == root))
}

/// Shell nav context for the landmark layout: resolves the `shell` mapping
/// against the landmark views and builds nav items from their navigate
/// events (same URL resolution as the page goto handlers, minus bindings
/// rooted in event params). `None` when no landmark views exist or no
/// `shell` mapping resolves — no layout is emitted then.
pub(crate) fn shell_nav(
    vcs: &[IfmlViewContainer],
    mappings: Option<&IfmlComponentMappings>,
) -> Option<RenderShellNav> {
    let labels: HashMap<&str, &str> = vcs
        .iter()
        .map(|vc| (vc.name.as_str(), vc.label.as_deref().unwrap_or(&vc.name)))
        .collect();
    let landmarks = vcs.iter().filter(|vc| vc.is_landmark && !vc.is_modal);
    let mapping = mappings.and_then(|m| {
        landmarks
            .clone()
            .find_map(|vc| m.resolve_by_role(&vc.name, SemanticRole::Shell))
    })?;
    let mut items: Vec<RenderNavItem> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for vc in landmarks {
        for evt in vc
            .events
            .iter()
            .chain(vc.components.iter().flat_map(|c| c.events.iter()))
        {
            if let IfmlAction::Navigate { target, binding } = &evt.action {
                let scoped: HashMap<String, String> = binding
                    .iter()
                    .filter(|(_, expr)| !references_event_param(expr, &evt.params))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let url_expr = nav_url_expr(target, &scoped);
                let label = labels.get(target.as_str()).copied().unwrap_or(target);
                if seen.insert((label.to_string(), url_expr.clone())) {
                    items.push(RenderNavItem {
                        label: label.to_string(),
                        href_attr: format!("href={{{url_expr}}}"),
                    });
                }
            }
        }
    }
    Some(RenderShellNav {
        import: RenderImport {
            export_name: mapping.export_name().to_string(),
            import_path: mapping.path.clone(),
        },
        testid: mapping.testid("root").map(str::to_string),
        items,
    })
}

/// Mapped action-control buttons for a component's fallback markup: the
/// form's save/submit button plus cancel/back/click buttons. A `None`
/// mapping keeps the hardcoded fallback markup (byte-identical output).
pub(super) fn button_context(
    c: &IfmlComponent,
    mapping: Option<&IfmlComponentMapping>,
    submit: Option<&RenderSubmit>,
    events: &[RenderEvent],
) -> (Option<RenderButton>, Vec<RenderButton>) {
    let Some(m) = mapping else {
        return (None, Vec::new());
    };
    let export = m.export_name().to_string();
    let import_path = m.path.clone();
    let testid_prop = |fallback: String| {
        let testid = m.testid("root").map(str::to_string).unwrap_or(fallback);
        format!("testid=\"{testid}\"")
    };
    let onclick_prop = |handler: &str| format!("onclick={{{handler}}}");
    let submit_button = RenderButton {
        import_name: export.clone(),
        import_path: import_path.clone(),
        label: primary_button_label(events),
        onclick_prop: submit.map(|s| onclick_prop(&s.handler_name)),
        disabled_prop: submit
            .is_some()
            .then(|| "disabled={submitting}".to_string()),
        testid_prop: testid_prop(format!("{}-submit", c.name)),
        event_requires: primary_event_requires(events),
        gate_open: String::new(),
        gate_close: String::new(),
    };
    let buttons = events
        .iter()
        .filter(|e| {
            e.action_kind == "navigate"
                && matches!(e.event_type.as_str(), "cancel" | "back" | "click")
        })
        .map(|e| {
            // Secondary buttons must not share the submit button's root
            // testid (duplicate selectors); prefer a per-event mapping key,
            // else the component-scoped slot name the fallback markup uses.
            let testid = m
                .testid(&e.event_type)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{}-{}", c.name, e.event_type));
            RenderButton {
                import_name: export.clone(),
                import_path: import_path.clone(),
                label: humanize_event_label(&e.event_type),
                onclick_prop: Some(onclick_prop(&e.handler_name)),
                disabled_prop: None,
                testid_prop: format!("testid=\"{testid}\""),
                event_requires: e.requires.clone(),
                gate_open: String::new(),
                gate_close: String::new(),
            }
        })
        .collect();
    (Some(submit_button), buttons)
}

/// Capability requirements of the save/submit event driving the primary
/// form button; empty when unguarded.
pub(super) fn primary_event_requires(events: &[RenderEvent]) -> Vec<String> {
    events
        .iter()
        .find(|e| e.event_type == "save" || e.event_type == "submit")
        .map(|e| e.requires.clone())
        .unwrap_or_default()
}

/// Label for the primary form button: the save/submit event's humanized
/// action, else "Submit".
fn primary_button_label(events: &[RenderEvent]) -> String {
    events
        .iter()
        .find(|e| e.event_type == "save" || e.event_type == "submit")
        .map(|e| humanize_event_label(&e.event_type))
        .unwrap_or_else(|| "Submit".to_string())
}

pub(super) fn humanize_event_label(event_type: &str) -> String {
    let mut chars = event_type.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// Build a JS template-literal URL for a navigation target: the target view's
/// generated route (`/{name-lowercase}`) plus query params from the binding
/// map (`?key=${expr}`). Binding expressions are emitted verbatim from the
/// model; keys are sorted for deterministic output.
pub(super) fn nav_url_expr(target: &str, binding: &HashMap<String, String>) -> String {
    let path = format!("/{}", target.to_lowercase());
    if binding.is_empty() {
        return format!("\"{path}\"");
    }
    let mut pairs: Vec<(&String, &String)> = binding.iter().collect();
    pairs.sort();
    let query: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}=${{{v}}}")).collect();
    format!("`{path}?{}`", query.join("&"))
}

pub(super) fn sanitize_ident(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect();
    if cleaned.is_empty() {
        "component".to_string()
    } else if cleaned.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

pub(super) fn render_table(spec: &TableSpec) -> RenderTable {
    RenderTable {
        pagination: spec.pagination,
        role: if spec.pagination {
            Some(SemanticRole::Pagination)
        } else {
            None
        },
        columns: spec.columns.iter().map(render_column).collect(),
        layout: TableLayout::Table,
    }
}

fn render_column(col: &ColumnDef) -> RenderColumn {
    match col {
        ColumnDef::Field { label, field } => RenderColumn {
            label: label.clone(),
            kind: "field".to_string(),
            binding: field.property.clone(),
            lookup: String::new(),
            expr: String::new(),
            ux: None,
        },
        ColumnDef::Lookup {
            label,
            field,
            lookup,
        } => RenderColumn {
            label: label.clone(),
            kind: "lookup".to_string(),
            binding: field.property.clone(),
            lookup: lookup.clone(),
            expr: String::new(),
            ux: None,
        },
        ColumnDef::Expression { label, expr } => RenderColumn {
            label: label.clone(),
            kind: "expr".to_string(),
            binding: render_expression(expr),
            lookup: String::new(),
            expr: render_expression(expr),
            ux: None,
        },
    }
}

pub(super) fn render_form(spec: &FormSpec) -> RenderForm {
    RenderForm {
        fields: spec
            .fields
            .iter()
            .map(|field| {
                let html = control_core::html_input_for_dsl(&field.input);
                let validations: Vec<String> =
                    field.validations.iter().map(render_expression).collect();
                let message = if validations.is_empty() {
                    None
                } else {
                    field.messages.first().cloned()
                };
                RenderInputField {
                    name: field.name.clone(),
                    input_role: input_field_role(&html.input_type),
                    input_type: html.input_type,
                    is_textarea: html.is_textarea,
                    is_select: html.is_select,
                    is_radio: html.is_radio,
                    required: field.required,
                    values: field.values.clone(),
                    // Escaped for the double-quoted HTML attribute (model
                    // expressions carry quotes; issue #468 W1b).
                    data_validate: validations.join(" && ").replace('"', "&quot;"),
                    message,
                }
            })
            .collect(),
    }
}

pub(super) fn render_chart(spec: &ChartSpec) -> RenderChart {
    RenderChart {
        kind: match spec.kind {
            ChartKind::Bar => "bar",
            ChartKind::Line => "line",
            ChartKind::Pie => "pie",
            ChartKind::Radar => "radar",
            ChartKind::Metric => "metric",
        }
        .to_string(),
        label_field: spec.label_field.clone(),
        value_fields: spec.value_fields.clone(),
    }
}

/// Render an IFML expression as a plain-text placeholder binding
/// (not evaluated — the generated markup keeps it verbatim).
pub(super) fn render_expression(expr: &Expression) -> String {
    match expr {
        Expression::Ident(name) => name.clone(),
        Expression::StringLit(value) => format!("\"{}\"", value.replace('"', "\\\"")),
        Expression::NumLit(value) => format!("{}", value.value()),
        Expression::BoolLit(value) => value.to_string(),
        Expression::FieldExpr { object, field } => {
            format!("{}.{}", render_expression(object), field)
        }
        Expression::BinOp { left, op, right } => format!(
            "{} {} {}",
            render_expression(left),
            bin_op_symbol(op),
            render_expression(right)
        ),
        Expression::UnaryOp { op, operand } => match op {
            UnaryOp::Not => format!("!{}", render_expression(operand)),
            UnaryOp::Neg => format!("-{}", render_expression(operand)),
        },
        Expression::Group(inner) => format!("({})", render_expression(inner)),
        Expression::Call { name, args } => {
            let rendered: Vec<String> = args.iter().map(render_expression).collect();
            format!("{name}({})", rendered.join(", "))
        }
    }
}

fn bin_op_symbol(op: &BinOp) -> &'static str {
    match op {
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::RegexMatch => "=~",
        BinOp::NegRegex => "!~",
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}
