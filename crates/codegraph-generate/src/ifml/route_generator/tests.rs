use super::context::*;
use super::load::*;
use super::render::*;
use super::ux::*;
use super::*;
use crate::ifml::context::{IfmlAction, IfmlComponent, IfmlEvent, IfmlViewContainer};
use crate::project_config::CodegenConfig;
use crate::template_engine::create_tera;
use codegraph_config::ux::UxRules;
use codegraph_config::{DomainConfig, IfmlComponentMappings, SemanticRole};
use codegraph_core::mock::MockEngine;
use codegraph_core::types::PropertyNode;
use codegraph_type_contracts::RefClassificationKind;
use rex_ifml::InputFieldType;
use rex_ifml::PropertyRef;
use rex_ifml::{
    BinOp, ChartKind, ChartSpec, ColumnDef, ComponentSpec, Expression, FormSpec, TableSpec, UnaryOp,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::ProjectConfig;

fn table_spec() -> ComponentSpec {
    ComponentSpec::Table(TableSpec {
        columns: vec![
            ColumnDef::Field {
                label: "Name".to_string(),
                field: PropertyRef {
                    entity: "Customer".to_string(),
                    property: "name".to_string(),
                },
            },
            ColumnDef::Lookup {
                label: "Status".to_string(),
                field: PropertyRef {
                    entity: "Customer".to_string(),
                    property: "status".to_string(),
                },
                lookup: "status_labels".to_string(),
            },
            ColumnDef::Expression {
                label: "Tenure".to_string(),
                expr: Expression::Call {
                    name: "tenure_years".to_string(),
                    args: vec![Expression::FieldExpr {
                        object: Box::new(Expression::Ident("Customer".to_string())),
                        field: "hire_date".to_string(),
                    }],
                },
            },
        ],
        pagination: true,
    })
}

fn component_with_spec(spec: Option<ComponentSpec>) -> IfmlComponent {
    IfmlComponent {
        name: "grid".to_string(),
        component_type: "table".to_string(),
        mode: None,
        entity: Some("Customer".to_string()),
        fields: Vec::new(),
        fields_with_types: Vec::new(),
        filter: None,
        properties: HashMap::new(),
        events: Vec::new(),
        parts: Vec::new(),
        spec,
    }
}

#[test]
fn specless_component_yields_no_render_contexts() {
    let ctx = page_component_context_sync(&component_with_spec(None));
    assert!(ctx.table.is_none());
    assert!(ctx.form.is_none());
    assert!(ctx.chart.is_none());
    assert_eq!(ctx.component_type, "table");
}

fn page_component_context_sync(c: &IfmlComponent) -> PageComponentContext {
    page_component_context_for_tests(c, &IfmlComponentMappings::default())
}

fn test_config() -> DomainConfig {
    toml::from_str(
        r#"
[defaults]
api_version = "v1"

[domains.sales]
label = "Sales"
schema_dir = "sales"
postgres_schema = "sales"
entities = ["CustomerType"]
"#,
    )
    .unwrap()
}

fn page_component_context_for_tests(
    c: &IfmlComponent,
    mappings: &IfmlComponentMappings,
) -> PageComponentContext {
    let vc = IfmlViewContainer {
        name: "CustomerList".to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: true,
        is_modal: false,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: Vec::new(),
        components: vec![],
        events: Vec::new(),
        containers: Vec::new(),
    };
    let mut cache = HashMap::new();
    futures::executor::block_on(page_component_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc,
        c,
        None,
        Some(mappings),
        &mut cache,
        &HashSet::new(),
        None,
    ))
}

fn workflow_config() -> DomainConfig {
    toml::from_str(
        r#"
[defaults]
api_version = "v1"

[domains.sales]
label = "Sales"
schema_dir = "sales"
postgres_schema = "sales"
entities = ["CustomerType"]

[domains.sales.entity_config.CustomerType.workflow]
status_field = "status"
initial_state = "received"
states = ["received", "review", "done"]
terminal_states = ["done"]
"#,
    )
    .unwrap()
}

fn page_component_context_with_config(
    c: &IfmlComponent,
    config: &DomainConfig,
) -> PageComponentContext {
    let vc = IfmlViewContainer {
        name: "CustomerList".to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: true,
        is_modal: false,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: Vec::new(),
        components: vec![],
        events: Vec::new(),
        containers: Vec::new(),
    };
    let mut cache = HashMap::new();
    futures::executor::block_on(page_component_context(
        &MockEngine::new(),
        config,
        "v1",
        &vc,
        c,
        None,
        Some(&IfmlComponentMappings::default()),
        &mut cache,
        &HashSet::new(),
        None,
    ))
}

#[test]
fn workflow_context_populates_from_domain_config() {
    let mut c = component_with_spec(None);
    c.component_type = "list".to_string();
    let ctx = page_component_context_with_config(&c, &workflow_config());
    let wf = ctx.workflow.expect("workflow context");
    assert_eq!(wf.status_field, "status");
    assert_eq!(wf.initial_state, "received");
    assert_eq!(wf.states, vec!["received", "review", "done"]);
    assert_eq!(wf.terminal_states, vec!["done"]);
    assert!(
        wf.badge_html.contains(
            "<span class=\"workflow-state\" data-testid=\"grid-state\" data-workflow-state={item.status}"
        ),
        "{}",
        wf.badge_html
    );
    assert!(
        wf.badge_html.contains(
            " data-workflow-terminal={['done'].includes(item.status as string) ? \"true\" : \"false\"}"
        ),
        "{}",
        wf.badge_html
    );
    assert!(wf.badge_html.ends_with(">{item.status}</span>"));
}

#[test]
fn workflow_value_path_follows_component_kind() {
    let form = form_component();
    let ctx = page_component_context_with_config(&form, &workflow_config());
    let wf = ctx.workflow.expect("form workflow");
    assert!(
        wf.badge_html
            .contains("data-workflow-state={editor_form_state.workflow_state?.current_state}"),
        "form badges read the merged workflow state: {}",
        wf.badge_html
    );

    let mut details = component_with_spec(None);
    details.component_type = "details".to_string();
    let ctx = page_component_context_with_config(&details, &workflow_config());
    let wf = ctx.workflow.expect("details workflow");
    assert!(
        wf.badge_html
            .contains("data-workflow-state={data.item?.workflow_state?.current_state}"),
        "{}",
        wf.badge_html
    );
}

#[test]
fn no_workflow_leaves_context_unchanged() {
    let ctx = page_component_context_sync(&component_with_spec(Some(table_spec())));
    assert!(ctx.workflow.is_none());

    let mut chart = component_with_spec(None);
    chart.component_type = "chart".to_string();
    let ctx = page_component_context_with_config(&chart, &workflow_config());
    assert!(ctx.workflow.is_none(), "charts carry no state badge");
}

fn workflow_transitions_config() -> DomainConfig {
    toml::from_str(
        r#"
[defaults]
api_version = "v1"

[domains.sales]
label = "Sales"
schema_dir = "sales"
postgres_schema = "sales"
entities = ["CustomerType"]

[domains.sales.entity_config.CustomerType.workflow]
status_field = "status"
initial_state = "draft"
states = ["draft", "submitted", "approved", "rejected"]
terminal_states = ["approved", "rejected"]
generate_action_endpoints = true

[domains.sales.entity_config.CustomerType.workflow.transitions]
draft = ["submitted"]
submitted = ["approved", "rejected"]
"#,
    )
    .unwrap()
}

fn details_component_named(name: &str) -> IfmlComponent {
    let mut c = component_with_spec(None);
    c.name = name.to_string();
    c.component_type = "details".to_string();
    c
}

fn context_json(c: &IfmlComponent, config: &DomainConfig) -> serde_json::Value {
    let ctx = page_component_context_with_config(c, config);
    serde_json::to_value(&ctx).expect("serialize page component context")
}

// ── Transition buttons (issue #198 workflow UI v2, RED) ─────────
//
// Pin: a details/form component whose entity has a workflow WITH
// transitions carries one RenderTransition per (from → to) edge in the
// `transitions` map, each labeled by the humanized target state with
// testid `{component}-transition-{target_kebab}`, a client-side
// disabled expression (current state ≠ from), and ready-to-render
// button markup carrying `data-transition-from`/`data-transition-to`.
// The assertions read the context through serde so they compile before
// `RenderWorkflow.transitions` exists (RED: the key is missing).

#[test]
fn transition_context_built_from_transitions_map_for_details() {
    let c = details_component_named("info");
    let json = context_json(&c, &workflow_transitions_config());

    let transitions = &json["workflow"]["transitions"];
    assert!(
        transitions.is_array(),
        "details components need a transition collection: {json}"
    );
    let transitions = transitions.as_array().unwrap();
    assert_eq!(
        transitions.len(),
        3,
        "one entry per transition edge (draft→submitted, submitted→approved, submitted→rejected): {json}"
    );

    let first = &transitions[0];
    assert_eq!(first["from"], serde_json::json!("draft"));
    assert_eq!(first["to"], serde_json::json!("submitted"));
    assert_eq!(
        first["testid"],
        serde_json::json!("info-transition-submitted"),
        "testid contract: {{component}}-transition-{{target_kebab}}: {json}"
    );
    assert_eq!(
        first["label"],
        serde_json::json!("Submitted"),
        "labels are the humanized target state: {json}"
    );

    for entry in transitions {
        let disabled = entry["disabled_expr"].as_str().unwrap_or_default();
        assert!(
            disabled.contains("data.item?.workflow_state?.current_state"),
            "the disabled expression reads the merged workflow state: {entry}"
        );
        let from = entry["from"].as_str().unwrap_or_default();
        assert!(
            disabled.contains(&format!("'{from}'")),
            "the button is disabled unless the current state equals the from-state: {entry}"
        );
        let html = entry["html"].as_str().unwrap_or_default();
        let to = entry["to"].as_str().unwrap_or_default();
        let testid = entry["testid"].as_str().unwrap_or_default();
        assert!(
            html.contains(&format!("data-testid=\"{testid}\"")),
            "button markup carries the testid: {entry}"
        );
        assert!(
            html.contains(&format!("data-transition-from=\"{from}\"")),
            "button markup exposes the from-state for e2e hooks: {entry}"
        );
        assert!(
            html.contains(&format!("data-transition-to=\"{to}\"")),
            "button markup exposes the target state for e2e hooks: {entry}"
        );
        assert!(
            html.contains("disabled={"),
            "the disabled state must be a client-side binding: {entry}"
        );
        assert!(
            html.contains("onclick="),
            "clicking posts to the transition endpoint: {entry}"
        );
    }

    let rejected = transitions
        .iter()
        .find(|e| e["to"] == serde_json::json!("rejected"))
        .expect("rejected is a valid target from submitted");
    assert_eq!(rejected["from"], serde_json::json!("submitted"));
    assert_eq!(
        rejected["label"],
        serde_json::json!("Rejected"),
        "humanized target label: {rejected}"
    );
}

#[test]
fn transition_disabled_expr_follows_form_value_path() {
    let mut c = details_component_named("editor");
    c.component_type = "form".to_string();
    let json = context_json(&c, &workflow_transitions_config());

    let transitions = json["workflow"]["transitions"]
        .as_array()
        .expect("form components carry transition buttons");
    let first = &transitions[0];
    let disabled = first["disabled_expr"].as_str().unwrap_or_default();
    assert!(
        disabled.contains("editor_form_state.workflow_state?.current_state"),
        "form transition buttons read the typed form state's merged workflow state: {first}"
    );
}

#[test]
fn empty_transitions_map_targets_all_non_terminal_states() {
    let c = details_component_named("info");
    let json = context_json(&c, &workflow_config());

    let transitions = json["workflow"]["transitions"]
        .as_array()
        .expect("an empty transitions map still yields buttons");
    let targets: Vec<&str> = transitions
        .iter()
        .filter_map(|e| e["to"].as_str())
        .collect();
    assert_eq!(
        targets,
        vec!["received", "review"],
        "every non-terminal state is a valid target when the map is empty: {json}"
    );
    for entry in transitions {
        assert_eq!(
            entry["from"],
            serde_json::json!(""),
            "no from-state restriction: any non-terminal state may transition: {entry}"
        );
        let disabled = entry["disabled_expr"].as_str().unwrap_or_default();
        assert!(
            disabled.contains("'done'"),
            "buttons disable once the current state is terminal: {entry}"
        );
    }
}

#[test]
fn list_components_carry_no_transition_buttons() {
    let mut c = component_with_spec(None);
    c.component_type = "list".to_string();
    let json = context_json(&c, &workflow_transitions_config());

    assert!(
        json["workflow"]["transitions"]
            .as_array()
            .map(|t| t.is_empty())
            .unwrap_or(true),
        "list components render state badges only — no transition buttons: {json}"
    );
}

// ── Event-level capability gating (issue #208 slice, RED) ───────
//
// Pin: events carrying `requires` gate their control behind `can(...)`
// exactly like the view-level control gate, AND-composed with the view
// gate when the view is also guarded. The IfmlEvent literal below must
// gain `requires: vec!["RaiseRefund".to_string()]` when the field lands
// on the context type (Wave B struct evolution) — until then the event
// is indistinguishable from an ungated one and the gates do not render,
// which is the RED failure mode.

fn gated_save_event() -> IfmlEvent {
    IfmlEvent {
        name: "save".to_string(),
        event_type: "save".to_string(),
        params: Vec::new(),
        requires: vec!["RaiseRefund".to_string()],
        action: IfmlAction::Navigate {
            target: "CustomerList".to_string(),
            binding: HashMap::new(),
        },
    }
}

fn action_control_mappings() -> IfmlComponentMappings {
    toml::from_str(
        r#"
[[component]]
role = "action-control"
path = "$lib/components/Button.svelte"
export = "Button"
testids = { root = "ui-button" }
"#,
    )
    .unwrap()
}

#[test]
fn event_requires_gate_the_fallback_submit_control() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let vc = guarded_view(
        "RefundEdit",
        Vec::new(),
        Vec::new(),
        vec![form_component_named("editor", vec![gated_save_event()])],
    );
    let ctx = page_context_for(&vc, None);
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains(
            "{#if ['RaiseRefund'].some((c) => can(c))}<button type=\"submit\" data-testid=\"editor-submit\" disabled={submitting}>Submit</button>{/if}"
        ),
        "an event-level requires gates the submit button via can() even in unguarded views:\n{rendered}"
    );
    assert!(
        rendered.contains("import { can } from '$lib/roles';"),
        "the gate needs the can() helper import: {rendered}"
    );
}

#[test]
fn event_requires_and_compose_with_the_view_gate() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let vc = guarded_view(
        "RefundEdit",
        Vec::new(),
        vec!["ViewRefunds".to_string()],
        vec![form_component_named("editor", vec![gated_save_event()])],
    );
    let ctx = page_context_for(&vc, None);
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains(
            "{#if viewRequires.some((c) => can(c)) && ['RaiseRefund'].some((c) => can(c))}"
        ),
        "event and view gates AND-compose, mirroring the load guard:\n{rendered}"
    );
}

#[test]
fn event_requires_gate_mapped_action_control_buttons_too() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let vc = guarded_view(
        "RefundEdit",
        Vec::new(),
        Vec::new(),
        vec![form_component_named("editor", vec![gated_save_event()])],
    );
    let ctx = page_context_for(&vc, Some(&action_control_mappings()));
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains(
            "{#if ['RaiseRefund'].some((c) => can(c))}<Button onclick={submit_editor} disabled={submitting} testid=\"ui-button\">Save</Button>{/if}"
        ),
        "mapped action-control buttons carry the event-level gate:\n{rendered}"
    );
}

#[test]
fn table_spec_maps_column_kinds_and_bindings() {
    let ctx = page_component_context_sync(&component_with_spec(Some(table_spec())));
    let table = ctx.table.expect("table render context");
    assert!(table.pagination);
    assert_eq!(table.columns.len(), 3);

    assert_eq!(table.columns[0].kind, "field");
    assert_eq!(table.columns[0].binding, "name");
    assert_eq!(table.columns[1].kind, "lookup");
    assert_eq!(table.columns[1].lookup, "status_labels");
    assert_eq!(table.columns[1].binding, "status");
    assert_eq!(table.columns[2].kind, "expr");
    assert_eq!(table.columns[2].binding, "tenure_years(Customer.hire_date)");
}

// ── ux-rules fallback-table resolution (issue #300) ──

fn ux_rules(toml: &str) -> UxRules {
    codegraph_config::parse_ux_rules_str(toml).unwrap().rules
}

fn ux_prop(name: &str, pg_type: &str, kind: Option<RefClassificationKind>) -> PropertyNode {
    PropertyNode {
        name: name.to_string(),
        prop_type: "string".into(),
        description: None,
        format: None,
        is_required: false,
        is_nullable: false,
        is_id: false,
        type_expr: None,
        is_array: false,
        min_items: None,
        max_items: None,
        pattern: None,
        min_length: None,
        max_length: None,
        minimum: None,
        maximum: None,
        pg_column_name: name.into(),
        pg_column_type: pg_type.into(),
        rust_field_name: name.into(),
        rust_field_type: "String".into(),
        sea_orm_type: "String".into(),
        render_strategy: "scalar".into(),
        ref_target: None,
        classification: None,
        projection: None,
        classification_kind: kind,
        ui_override_detail: None,
        ui_override_list_cell: None,
        ui_override_form: None,
        ui_override_inline: None,
    }
}

fn ux_props(props: Vec<PropertyNode>) -> HashMap<String, PropertyNode> {
    let mut by_name = HashMap::new();
    for prop in props {
        by_name.insert(prop.name.clone(), prop);
    }
    by_name
}

fn pack() -> UxRules {
    codegraph_config::builtin_ux_rules().unwrap().rules
}

#[test]
fn lookup_column_pins_status_category_chip_above_inference_and_rules() {
    // An EntityReference property would infer Reference via Pass 1; the
    // explicit DSL lookup (`via status_labels`) still wins (tier 0).
    let props = ux_props(vec![ux_prop(
        "status",
        "UUID",
        Some(RefClassificationKind::EntityReference),
    )]);
    // A project rule that would pin the same name raw must NOT reach a
    // lookup column either.
    let rules = ux_rules("[[column]]\nname_pattern = \"status\"\ndisplay = \"raw\"\n");
    let ux = resolve_column_ux(&rules, "lookup", "status", &props, &[], None);
    assert_eq!(ux.dimension, "status-category");
    assert_eq!(ux.display, "chip");
    assert_eq!(ux.align, "left");
    assert_eq!(ux.tone.lookup("active"), "default", "pack tone map");
}

#[test]
fn rule_beats_inference_for_field_columns() {
    // `total_amount` on NUMERIC infers Money (keyword + numeric pg); the
    // project rule pins Quantity + raw instead (rules > pack > inference).
    let props = ux_props(vec![ux_prop("total_amount", "NUMERIC(10,2)", None)]);
    let rules = ux_rules(
        "[[column]]\nname_pattern = \"*_amount\"\ndimension = \"quantity\"\ndisplay = \"raw\"\nalign = \"left\"\n",
    );
    let ux = resolve_column_ux(&rules, "field", "total_amount", &props, &[], None);
    assert_eq!(ux.dimension, "quantity", "rule pins quantity");
    assert_eq!(ux.display, "raw");
    assert_eq!(ux.align, "left", "rule align replaces the pack default");
}

#[test]
fn expression_columns_stay_text() {
    // The IFML AST carries no return type for `tenure_years(...)` — the
    // column stays Text regardless of the name.
    let props = ux_props(vec![ux_prop("tenure_years", "NUMERIC(10,2)", None)]);
    let ux = resolve_column_ux(&pack(), "expr", "tenure_years", &props, &[], None);
    assert_eq!(ux.dimension, "text");
    assert_eq!(ux.display, "raw");
    assert_eq!(ux.align, "left");
}

#[test]
fn graph_metadata_drives_pass1_inference() {
    let props = ux_props(vec![
        ux_prop(
            "status",
            "TEXT",
            Some(RefClassificationKind::CodelistReference),
        ),
        ux_prop(
            "assignee",
            "UUID",
            Some(RefClassificationKind::EntityReference),
        ),
        ux_prop("id", "UUID", None),
        ux_prop("total_amount", "NUMERIC(10,2)", None),
        ux_prop("created_at", "TIMESTAMPTZ", None),
    ]);
    let status = resolve_column_ux(&pack(), "field", "status", &props, &[], None);
    assert_eq!(status.dimension, "status-category");
    assert_eq!(status.display, "chip");

    let reference = resolve_column_ux(&pack(), "field", "assignee", &props, &[], None);
    assert_eq!(reference.dimension, "reference");
    assert_eq!(reference.display, "link");

    let id = resolve_column_ux(&pack(), "field", "id", &props, &[], None);
    assert_eq!(id.dimension, "identifier");
    assert_eq!(id.display, "copy-chip");

    let money = resolve_column_ux(&pack(), "field", "total_amount", &props, &[], None);
    assert_eq!(money.dimension, "money");
    assert_eq!(money.align, "right");

    let time = resolve_column_ux(&pack(), "field", "created_at", &props, &[], None);
    assert_eq!(time.dimension, "time-point");
}

#[test]
fn workflow_status_field_lifts_to_status_category() {
    let props = ux_props(vec![ux_prop("state", "TEXT", None)]);
    let ux = resolve_column_ux(&pack(), "field", "state", &props, &[], Some("state"));
    assert_eq!(ux.dimension, "status-category");
    assert_eq!(ux.display, "chip");
}

#[test]
fn schemaless_columns_fall_back_to_component_type_pairs() {
    // No graph props: `id` with a Uuid rust type still infers Identifier
    // through the (field, rust_type) projection; a Decimal money name
    // stays Text (no pg type — honest degradation).
    let pairs = vec![
        ("id".to_string(), "Uuid".to_string()),
        ("total_amount".to_string(), "Decimal".to_string()),
    ];
    let id = resolve_column_ux(&pack(), "field", "id", &HashMap::new(), &pairs, None);
    assert_eq!(id.dimension, "identifier");
    assert_eq!(id.display, "copy-chip");
    let money = resolve_column_ux(
        &pack(),
        "field",
        "total_amount",
        &HashMap::new(),
        &pairs,
        None,
    );
    assert_eq!(money.dimension, "text");
}

#[test]
fn apply_table_ux_resolves_every_column_by_tier() {
    let mut table = render_table(&TableSpec {
        columns: vec![
            ColumnDef::Field {
                label: "Name".to_string(),
                field: PropertyRef {
                    entity: "Customer".to_string(),
                    property: "name".to_string(),
                },
            },
            ColumnDef::Lookup {
                label: "Status".to_string(),
                field: PropertyRef {
                    entity: "Customer".to_string(),
                    property: "status".to_string(),
                },
                lookup: "status_labels".to_string(),
            },
            ColumnDef::Expression {
                label: "Tenure".to_string(),
                expr: Expression::Ident("Customer".to_string()),
            },
        ],
        pagination: false,
    });
    let props = ux_props(vec![
        ux_prop("name", "TEXT", None),
        ux_prop(
            "status",
            "TEXT",
            Some(RefClassificationKind::CodelistReference),
        ),
    ]);
    apply_table_ux(&mut table, &pack(), &props, &[], Some("status"));
    assert_eq!(table.columns[0].ux.as_ref().unwrap().dimension, "text");
    assert_eq!(
        table.columns[1].ux.as_ref().unwrap().display,
        "chip",
        "lookup pins the chip"
    );
    assert_eq!(table.columns[2].ux.as_ref().unwrap().dimension, "text");
}

#[test]
fn flag_off_leaves_render_contexts_ux_free() {
    let ctx = page_component_context_sync(&component_with_spec(Some(table_spec())));
    let table = ctx.table.expect("table render context");
    assert!(
        table.columns.iter().all(|col| col.ux.is_none()),
        "flag off ⇒ no ux payloads in the context"
    );
    assert!(ctx.ux_list_columns.is_empty());

    let mut list = component_with_spec(None);
    list.component_type = "list".to_string();
    list.fields = vec!["name".to_string(), "status".to_string()];
    let ctx = page_component_context_sync(&list);
    assert!(ctx.ux_list_columns.is_empty(), "no ux columns when off");
}

#[test]
fn ux_on_resolves_specless_list_columns() {
    let mut c = component_with_spec(None);
    c.component_type = "list".to_string();
    c.fields = vec!["status".to_string(), "name".to_string()];
    let rules = pack();
    let props = ux_props(vec![
        ux_prop(
            "status",
            "TEXT",
            Some(RefClassificationKind::CodelistReference),
        ),
        ux_prop("name", "TEXT", None),
    ]);
    let status = resolve_column_ux(&rules, "field", "status", &props, &[], None);
    assert_eq!(status.display, "chip");
    assert!(status.tone.lookup("draft") == "secondary");
    let name = resolve_column_ux(&rules, "field", "name", &props, &[], None);
    assert_eq!(name.display, "raw");
}

#[test]
fn page_ux_context_formats_locale_and_money_options() {
    let rules = ux_rules("[format]\nlocale = \"de-DE\"\ncurrency = \"EUR\"\n");
    let ctx = page_ux_context(&rules);
    assert_eq!(ctx.locale, "de-DE");
    assert_eq!(ctx.money_options, "{ style: 'currency', currency: 'EUR' }");

    let ctx = page_ux_context(&pack());
    assert_eq!(ctx.locale, "en-NZ");
    assert_eq!(ctx.money_options, "{ style: 'currency', currency: 'NZD' }");

    let ctx = page_ux_context(&ux_rules("[format]\nlocale = \"de-DE\"\n"));
    assert_eq!(ctx.money_options, "{}", "no currency ⇒ plain decimals");
}

#[test]
fn page_ux_rides_the_page_context_only_when_enabled() {
    let vc = IfmlViewContainer {
        name: "CustomerList".to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: true,
        is_modal: false,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: Vec::new(),
        components: vec![],
        events: Vec::new(),
        containers: Vec::new(),
    };
    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc,
        None,
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    assert!(ctx.ux.is_none(), "flag off ⇒ no ux key on the page context");

    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc,
        None,
        &HashSet::new(),
        Some(&ux_generation_for_tests(&pack())),
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    let ux = ctx.ux.expect("ux context");
    assert_eq!(ux.locale, "en-NZ");
    assert_eq!(ux.money_options, "{ style: 'currency', currency: 'NZD' }");
}

/// A flag-on [`UxGeneration`] over an empty plan map — the shape test
/// contexts use when the timeline/layout plane itself isn't the
/// subject.
fn ux_generation_for_tests<'a>(rules: &'a UxRules) -> UxGeneration<'a> {
    static EMPTY_PLANS: std::sync::OnceLock<HashMap<String, crate::ux::plan::UxPlan>> =
        std::sync::OnceLock::new();
    UxGeneration {
        rules,
        plans: EMPTY_PLANS.get_or_init(HashMap::new),
    }
}

// ── Timeline layout + event tiering + diagnostics (issue #301) ──

fn ux_schema(title: &str) -> codegraph_core::types::SchemaNode {
    codegraph_core::types::SchemaNode {
        namespace: None,
        schema_id: format!("id:{title}"),
        title: title.to_string(),
        access: None,
        annotations: None,
        description: None,
        schema_type: "object".to_string(),
        classification: "entity".to_string(),
        domain: Some("sales".to_string()),
        rel_path: format!("{title}.json"),
        pg_type: "UUID".to_string(),
        rust_type: "Uuid".to_string(),
        sea_orm_type: "Uuid".to_string(),
        rust_type_name: title.to_string(),
        pg_table_name: codegraph_naming::to_snake_case(title),
        api_path_segment: codegraph_naming::to_kebab_case(title),
        parent_schema: None,
        is_entity: true,
        is_codelist: false,
        is_primitive_wrapper: false,
        has_all_of: false,
        has_one_of: false,
        has_any_of: false,
        has_definitions: false,
        custom_annotations: Default::default(),
    }
}

/// Graph fixture for the Customer entity: a name, a codelist status,
/// a numeric money-named amount and a timestamp — one field per
/// dimension the timeline layout touches.
fn ux_customer_db() -> MockEngine {
    MockEngine::builder()
        .with_schema(ux_schema("CustomerType"))
        .with_properties(
            "CustomerType",
            vec![
                ux_prop("name", "TEXT", None),
                ux_prop(
                    "status",
                    "TEXT",
                    Some(RefClassificationKind::CodelistReference),
                ),
                ux_prop("total_amount", "NUMERIC(10,2)", None),
                ux_prop("created_at", "TIMESTAMPTZ", None),
            ],
        )
        .build()
}

fn nav_event(name: &str, event_type: &str, target: &str) -> IfmlEvent {
    IfmlEvent {
        name: name.to_string(),
        event_type: event_type.to_string(),
        params: vec!["row".to_string()],
        requires: Vec::new(),
        action: IfmlAction::Navigate {
            target: target.to_string(),
            binding: HashMap::new(),
        },
    }
}

fn timeline_rules() -> UxRules {
    ux_rules(
        "[[collection]]\nentity_pattern = \"Customer*\"\ndisplay = \"timeline\"\n\
         order_by = \"created_at\"\ntitle_field = \"name\"\npreview = [\"status\", \"total_amount\"]\n",
    )
}

/// A typed fallback table over the Customer fixture carrying THREE
/// navigate events (select inline + archive/delete disclosed).
fn timeline_table_component() -> IfmlComponent {
    let mut c = component_with_spec(Some(ComponentSpec::Table(TableSpec {
        columns: vec![
            ColumnDef::Field {
                label: "Name".to_string(),
                field: PropertyRef {
                    entity: "Customer".to_string(),
                    property: "name".to_string(),
                },
            },
            ColumnDef::Field {
                label: "Created".to_string(),
                field: PropertyRef {
                    entity: "Customer".to_string(),
                    property: "created_at".to_string(),
                },
            },
        ],
        pagination: false,
    })));
    c.events = vec![
        nav_event("comp_grid_select", "select", "CustomerDetail"),
        nav_event("comp_grid_archive", "archive", "CustomerArchive"),
        nav_event("comp_grid_delete", "delete", "CustomerDelete"),
    ];
    c
}

/// Resolve the generation plans for a single-view fixture.
fn resolve_plans_for(
    db: &MockEngine,
    vc: &IfmlViewContainer,
    rules: &UxRules,
) -> HashMap<String, crate::ux::plan::UxPlan> {
    let model = model_of(vec![vc.clone()]);
    let (plans, _lines) = futures::executor::block_on(resolve_generation_ux(
        db,
        &test_config(),
        &model,
        Some(rules),
    ))
    .expect("generation ux resolution");
    plans
}

fn page_component_context_with_ux(
    db: &MockEngine,
    config: &DomainConfig,
    vc: &IfmlViewContainer,
    c: &IfmlComponent,
    uxgen: Option<&UxGeneration<'_>>,
) -> PageComponentContext {
    let mut cache = HashMap::new();
    futures::executor::block_on(page_component_context(
        db,
        config,
        "v1",
        vc,
        c,
        None,
        None,
        &mut cache,
        &HashSet::new(),
        uxgen,
    ))
}

#[test]
fn timeline_rule_resolves_layout_with_resolved_bindings() {
    let db = ux_customer_db();
    let rules = timeline_rules();
    let c = timeline_table_component();
    let mut vc = plain_vc("CustomerList");
    vc.components.push(c.clone());
    let plans = resolve_plans_for(&db, &vc, &rules);
    let uxgen = UxGeneration {
        rules: &rules,
        plans: &plans,
    };

    let ctx = page_component_context_with_ux(&db, &test_config(), &vc, &c, Some(&uxgen));

    // RenderTable.layout carries the timeline with the rule's
    // resolved bindings (reused plan output — no re-validation).
    let table = ctx.table.expect("table context");
    let TableLayout::Timeline {
        order_binding,
        title_binding,
        preview,
    } = &table.layout
    else {
        panic!(
            "explicit matching rule must resolve a timeline: {:?}",
            table.layout
        );
    };
    assert_eq!(order_binding, "created_at");
    assert_eq!(title_binding.as_deref(), Some("name"));
    assert_eq!(
        preview
            .iter()
            .map(|col| col.binding.as_str())
            .collect::<Vec<_>>(),
        vec!["status", "total_amount"]
    );

    // The uniform component view agrees (single template path).
    let timeline = ctx.timeline.as_ref().expect("uniform timeline view");
    assert_eq!(timeline.order_binding, "created_at");
    assert_eq!(timeline.title_binding.as_deref(), Some("name"));
    assert_eq!(timeline.preview[0].binding, "status");
    assert_eq!(
        timeline.preview[0].ux.as_ref().unwrap().display,
        "chip",
        "preview renders through the shared #300 chip tier"
    );
    assert_eq!(timeline.preview[1].binding, "total_amount");
    assert_eq!(
        timeline.preview[1].ux.as_ref().unwrap().dimension,
        "money",
        "preview renders through the shared #300 money tier"
    );
}

#[test]
fn timeline_unresolvable_order_by_errors_naming_candidates() {
    let db = ux_customer_db();
    let rules = ux_rules(
        "[[collection]]\nentity_pattern = \"Customer*\"\ndisplay = \"timeline\"\norder_by = \"deleted_at\"\n",
    );
    let mut vc = plain_vc("CustomerList");
    vc.components.push(timeline_table_component());
    let model = model_of(vec![vc]);
    let err = futures::executor::block_on(resolve_generation_ux(
        &db,
        &test_config(),
        &model,
        Some(&rules),
    ))
    .expect_err("unresolvable order_by must fail generation");
    let message = err.to_string();
    assert!(message.contains("deleted_at"), "{message}");
    assert!(message.contains("Customer"), "{message}");
    assert!(
        message.contains("created_at"),
        "the error names the candidate time-point fields: {message}"
    );
}

#[test]
fn mapped_component_beats_timeline_layout() {
    let db = ux_customer_db();
    let rules = timeline_rules();
    let c = timeline_table_component();
    let mut vc = plain_vc("CustomerList");
    vc.components.push(c.clone());
    let plans = resolve_plans_for(&db, &vc, &rules);
    let uxgen = UxGeneration {
        rules: &rules,
        plans: &plans,
    };
    let mappings: IfmlComponentMappings = toml::from_str(
        r#"
[[component]]
kind = "table"
path = "$lib/components/DataTable.svelte"
export = "DataTable"
"#,
    )
    .unwrap();

    let mut cache = HashMap::new();
    let ctx = futures::executor::block_on(page_component_context(
        &db,
        &test_config(),
        "v1",
        &vc,
        &c,
        None,
        Some(&mappings),
        &mut cache,
        &HashSet::new(),
        Some(&uxgen),
    ));

    assert!(ctx.mapping.is_some(), "whole-component mapping resolves");
    assert!(
        ctx.timeline.is_none(),
        "mapped components replace the fallback — no timeline"
    );
    assert!(
        ctx.table.as_ref().unwrap().layout.is_table(),
        "mapped components reset the table layout"
    );
    assert!(
        ctx.row_menu_events.is_empty(),
        "mapped components keep their own event surface — no actions menu"
    );
}

#[test]
fn no_timeline_rule_stays_table_and_single_event_has_no_menu() {
    let db = ux_customer_db();
    let rules = pack();
    let mut c = timeline_table_component();
    c.events = vec![nav_event("comp_grid_select", "select", "CustomerDetail")];
    let mut vc = plain_vc("CustomerList");
    vc.components.push(c.clone());
    let plans = resolve_plans_for(&db, &vc, &rules);
    let uxgen = UxGeneration {
        rules: &rules,
        plans: &plans,
    };

    let ctx = page_component_context_with_ux(&db, &test_config(), &vc, &c, Some(&uxgen));
    assert!(ctx.table.unwrap().layout.is_table(), "no rule ⇒ table");
    assert!(ctx.timeline.is_none());
    assert!(
        ctx.row_menu_events.is_empty(),
        "single navigate event ⇒ no menu, no actions testid"
    );
}

#[test]
fn event_partition_first_inline_rest_menu_with_shared_handlers() {
    let db = ux_customer_db();
    let rules = pack();
    let c = timeline_table_component();
    let mut vc = plain_vc("CustomerList");
    vc.components.push(c.clone());
    let plans = resolve_plans_for(&db, &vc, &rules);
    let uxgen = UxGeneration {
        rules: &rules,
        plans: &plans,
    };

    let ctx = page_component_context_with_ux(&db, &test_config(), &vc, &c, Some(&uxgen));
    assert_eq!(
        ctx.row_handler.as_deref(),
        Some("comp_grid_select"),
        "the FIRST navigate event stays inline"
    );
    let menu = &ctx.row_menu_events;
    assert_eq!(menu.len(), 2, "the other two events disclose");
    assert_eq!(menu[0].handler_name, "comp_grid_archive");
    assert_eq!(menu[0].label, "Archive");
    assert_eq!(menu[1].handler_name, "comp_grid_delete");
    assert_eq!(menu[1].label, "Delete");

    // Handler parity: menu items call the same named functions the
    // inline placement would call (the page template emits one
    // function per event, referenced from both placements).
    let emitted: Vec<&str> = ctx.events.iter().map(|e| e.handler_name.as_str()).collect();
    for evt in menu {
        assert!(
            emitted.contains(&evt.handler_name.as_str()),
            "menu handler {} must be an emitted event handler",
            evt.handler_name
        );
    }
}

#[test]
fn flag_off_context_json_omits_issue301_keys() {
    let c = timeline_table_component();
    let ctx = page_component_context_sync(&c);
    let json = serde_json::to_value(&ctx).unwrap();
    assert!(json.get("timeline").is_none(), "{json}");
    assert!(json.get("row_menu_events").is_none(), "{json}");
    assert!(
        json.get("table").and_then(|t| t.get("layout")).is_none(),
        "flag off ⇒ no layout key on the serialized table: {json}"
    );

    // Flag ON but no matching rule ⇒ same absence.
    let db = ux_customer_db();
    let rules = pack();
    let mut vc = plain_vc("CustomerList");
    vc.components.push(c.clone());
    let plans = resolve_plans_for(&db, &vc, &rules);
    let uxgen = UxGeneration {
        rules: &rules,
        plans: &plans,
    };
    let ctx = page_component_context_with_ux(&db, &test_config(), &vc, &c, Some(&uxgen));
    let json = serde_json::to_value(&ctx).unwrap();
    assert!(json.get("timeline").is_none(), "{json}");
    assert!(
        json.get("table").and_then(|t| t.get("layout")).is_none(),
        "table plan ⇒ layout key skipped: {json}"
    );
}

#[test]
fn specless_list_resolves_the_uniform_timeline_view() {
    let db = ux_customer_db();
    let rules = timeline_rules();
    let mut c = component_with_spec(None);
    c.component_type = "list".to_string();
    c.fields = vec![
        "name".to_string(),
        "status".to_string(),
        "created_at".to_string(),
    ];
    c.events = vec![nav_event("comp_grid_select", "select", "CustomerDetail")];
    let mut vc = plain_vc("CustomerList");
    vc.components.push(c.clone());
    let plans = resolve_plans_for(&db, &vc, &rules);
    let uxgen = UxGeneration {
        rules: &rules,
        plans: &plans,
    };

    let ctx = page_component_context_with_ux(&db, &test_config(), &vc, &c, Some(&uxgen));
    let timeline = ctx.timeline.as_ref().expect("spec-less list timeline");
    assert_eq!(timeline.order_binding, "created_at");
    assert_eq!(timeline.title_binding.as_deref(), Some("name"));
    assert_eq!(timeline.preview[0].binding, "status");
}

fn render_page_with_ux(
    db: &MockEngine,
    config: &DomainConfig,
    vc: &IfmlViewContainer,
    uxgen: Option<&UxGeneration<'_>>,
) -> String {
    let tera = create_tera(Path::new(".")).expect("tera");
    let ctx = futures::executor::block_on(build_page_context(
        db,
        config,
        "v1",
        vc,
        None,
        &HashSet::new(),
        uxgen,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render")
}

#[test]
fn timeline_render_replaces_the_table_block() {
    let db = ux_customer_db();
    let rules = timeline_rules();
    let c = timeline_table_component();
    let mut vc = plain_vc("CustomerList");
    vc.components.push(c);
    let plans = resolve_plans_for(&db, &vc, &rules);
    let uxgen = UxGeneration {
        rules: &rules,
        plans: &plans,
    };

    let rendered = render_page_with_ux(&db, &test_config(), &vc, Some(&uxgen));
    assert!(
        rendered.contains("<ol class=\"timeline\" data-testid=\"grid-timeline\">"),
        "{rendered}"
    );
    assert!(
        rendered.contains("{#each grid_timeline_items as item}"),
        "{rendered}"
    );
    assert!(
        rendered.contains("timeOf(b['created_at']) - timeOf(a['created_at'])"),
        "items sort newest first on the order binding:\n{rendered}"
    );
    assert!(
        rendered
            .contains("<li class=\"timeline-item\" data-testid=\"grid-timeline-item\" onclick={() => comp_grid_select(item)}>"),
        "the row onclick fires from the item body:\n{rendered}"
    );
    assert!(
        rendered.contains(
            "<time class=\"timeline-time\" datetime={item.created_at}>{formatDate(item.created_at)}</time>"
        ),
        "the time head renders the date-only Intl helper:\n{rendered}"
    );
    assert!(
        rendered.contains(
            "<button type=\"button\" class=\"timeline-title\" data-testid=\"grid-timeline-title\" onclick={() => comp_grid_select(item)}>{item.name}</button>"
        ),
        "the title carries the row-handler target convention:\n{rendered}"
    );
    assert!(
        rendered.contains(
            "<span class=\"timeline-meta\" data-testid=\"grid-timeline-meta\"><span class=\"chip\" data-chip={item.status} data-chip-variant={toneFor('grid.status', item.status)} data-testid=\"grid-chip\">{item.status}</span></span>"
        ),
        "preview fields render through the shared #300 chip formatter:\n{rendered}"
    );
    assert!(
        rendered.contains("{formatMoney(item.total_amount)}"),
        "{rendered}"
    );
    assert!(
        rendered.contains("'grid.status': {"),
        "preview chips join the page tone map:\n{rendered}"
    );
    assert!(
        !rendered.contains("<table data-testid=\"grid-table\""),
        "the timeline replaces the table block:\n{rendered}"
    );
}

#[test]
fn timeline_item_renders_workflow_badge_and_actions_menu() {
    let db = ux_customer_db();
    let rules = timeline_rules();
    let c = timeline_table_component();
    let mut vc = plain_vc("CustomerList");
    vc.components.push(c);
    let plans = resolve_plans_for(&db, &vc, &rules);
    let uxgen = UxGeneration {
        rules: &rules,
        plans: &plans,
    };

    let rendered = render_page_with_ux(&db, &workflow_config(), &vc, Some(&uxgen));
    assert!(
        rendered.contains("data-workflow-state={item.status}"),
        "the workflow badge renders per item:\n{rendered}"
    );
    assert!(
        rendered.contains("let grid_actions_open = $state(false);"),
        "{rendered}"
    );
    assert!(
        rendered.contains("data-testid=\"grid-actions\""),
        "{rendered}"
    );
    assert!(
        rendered.contains("data-testid=\"grid-actions-menu\""),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "onclick={(e) => { e.stopPropagation(); comp_grid_archive(item); }}>Archive</button>"
        ),
        "menu items carry the identical handler invocation:\n{rendered}"
    );
    // Handler parity: each handler body is emitted exactly once and
    // referenced from both the item click and the menu.
    assert_eq!(
        rendered
            .matches("function comp_grid_archive(row: Record<string, unknown>) {")
            .count(),
        1,
        "the secondary handler body is emitted once:\n{rendered}"
    );
    let def = rendered
        .find("function comp_grid_archive(row: Record<string, unknown>) {")
        .expect("handler definition");
    let menu_use = rendered.find("comp_grid_archive(item);").expect("menu use");
    assert!(menu_use > def, "{rendered}");
}

#[test]
fn secondary_event_menu_renders_on_plain_tables_with_inline_parity() {
    let db = ux_customer_db();
    let rules = pack();
    let c = timeline_table_component();
    let mut vc = plain_vc("CustomerList");
    vc.components.push(c);
    let plans = resolve_plans_for(&db, &vc, &rules);
    let uxgen = UxGeneration {
        rules: &rules,
        plans: &plans,
    };

    let rendered = render_page_with_ux(&db, &test_config(), &vc, Some(&uxgen));
    // The table stays; the first event remains the inline row handler.
    assert!(
        rendered.contains("<tr data-testid=\"grid-row\" onclick={() => comp_grid_select(item)}>"),
        "first event stays inline byte-equal:\n{rendered}"
    );
    assert!(
        rendered.contains("data-testid=\"grid-actions\""),
        "{rendered}"
    );
    assert!(
        rendered.contains("data-testid=\"grid-actions-menu\""),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "onclick={(e) => { e.stopPropagation(); comp_grid_archive(item); }}>Archive</button>"
        ),
        "{rendered}"
    );
    assert!(
        !rendered.contains("grid-timeline"),
        "no timeline without a rule: {rendered}"
    );
    assert_eq!(
        rendered
            .matches("function comp_grid_archive(row: Record<string, unknown>) {")
            .count(),
        1,
        "menu handlers are the shared named functions:\n{rendered}"
    );
}

#[test]
fn generation_diagnostics_are_deduped_per_entity_and_quiet_when_clean() {
    // A spec-less list whose declared fields carry the finding shapes:
    // a money keyword, a time-named field. Both hint classes fire once,
    // and a second view binding the same entity adds nothing.
    let db = ux_customer_db();
    let rules = pack();
    let mut c = component_with_spec(None);
    c.component_type = "list".to_string();
    c.fields = vec![
        "name".to_string(),
        "status".to_string(),
        "total_amount".to_string(),
        "created_at".to_string(),
    ];
    let mut vc = plain_vc("CustomerList");
    vc.components.push(c);
    let mut vc2 = plain_vc("CustomerList2");
    vc2.components.push({
        let mut c = component_with_spec(None);
        c.component_type = "list".to_string();
        c.fields = vec![
            "name".to_string(),
            "status".to_string(),
            "total_amount".to_string(),
            "created_at".to_string(),
        ];
        c
    });
    let model = model_of(vec![vc, vc2]);
    let (plans, lines) = futures::executor::block_on(resolve_generation_ux(
        &db,
        &test_config(),
        &model,
        Some(&rules),
    ))
    .expect("resolution");
    assert_eq!(plans.len(), 1, "one plan per distinct bound entity");
    let money = lines
        .iter()
        .find(|l| l.contains("total_amount"))
        .expect("money hint line");
    assert!(money.contains("total_amount"), "{money}");
    let suggestion = lines
        .iter()
        .find(|l| l.contains("timeline"))
        .expect("a time-dominated collection without a rule earns a suggestion");
    assert!(suggestion.contains("created_at"), "{suggestion}");

    // Clean projection — an entity without money keywords or
    // time-shaped fields: only the shared actions accounting line
    // remains, once.
    let mut c = component_with_spec(None);
    c.component_type = "list".to_string();
    c.fields = vec!["name".to_string(), "quantity".to_string()];
    let mut vc = plain_vc("CustomerList");
    vc.components.push(c);
    let model = model_of(vec![vc]);
    let empty_db = MockEngine::new();
    let (_plans, lines) = futures::executor::block_on(resolve_generation_ux(
        &empty_db,
        &test_config(),
        &model,
        Some(&rules),
    ))
    .expect("resolution");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("2 row action(s)"), "{lines:?}");
}

#[test]
fn flag_off_generation_diagnostics_stay_silent() {
    let db = ux_customer_db();
    let mut vc = plain_vc("CustomerList");
    vc.components.push(timeline_table_component());
    let model = model_of(vec![vc]);
    let (plans, lines) =
        futures::executor::block_on(resolve_generation_ux(&db, &test_config(), &model, None))
            .expect("resolution");
    assert!(plans.is_empty());
    assert!(lines.is_empty());
}

#[test]
fn form_payload_coerces_by_input_and_rust_types() {
    let mut c = component_with_spec(Some(ComponentSpec::Form(FormSpec {
        fields: vec![
            field_def("title", InputFieldType::Text),
            field_def("amount", InputFieldType::Number),
            field_def("urgent", InputFieldType::Checkbox),
            field_def("submittedAt", InputFieldType::DateTime),
        ],
    })));
    c.fields_with_types = vec![
        ("title".to_string(), "String".to_string()),
        ("amount".to_string(), "Option< i64 >".to_string()),
        ("urgent".to_string(), "bool".to_string()),
        (
            "submittedAt".to_string(),
            "Option< DateTime < Utc > >".to_string(),
        ),
    ];
    let form = match &c.spec {
        Some(ComponentSpec::Form(form)) => Some(render_form(form)),
        _ => None,
    };
    let block = form_payload_block(&c, form.as_ref());
    assert!(
        block.contains("\t\tpayload.title = formData.title;"),
        "{block}"
    );
    assert!(
        block.contains(
            "\t\tpayload.amount = formData.amount === '' ? null : Number(formData.amount);"
        ),
        "{block}"
    );
    assert!(
        block.contains("\t\tpayload.urgent = formData.urgent === 'on';"),
        "{block}"
    );
    assert!(
        block.contains(
            "\t\tpayload.submittedAt = formData.submittedAt === '' ? null : new Date(String(formData.submittedAt)).toISOString();"
        ),
        "{block}"
    );

    let no_types = component_with_spec(None);
    assert_eq!(
        form_payload_block(&no_types, None),
        String::new(),
        "components without form fields keep the untyped body"
    );
}

#[test]
fn form_spec_maps_input_types_and_validations() {
    let spec = ComponentSpec::Form(FormSpec {
        fields: vec![
            rex_ifml::FieldDef {
                name: "name".to_string(),
                input: InputFieldType::Text,
                required: true,
                validations: vec![Expression::BinOp {
                    left: Box::new(Expression::Call {
                        name: "len".to_string(),
                        args: vec![Expression::Ident("name".to_string())],
                    }),
                    op: BinOp::Gt,
                    right: Box::new(Expression::NumLit(2.0.into())),
                }],
                values: Vec::new(),
                messages: vec!["Name too short".to_string()],
            },
            rex_ifml::FieldDef {
                name: "start".to_string(),
                input: InputFieldType::DateTime,
                required: false,
                validations: Vec::new(),
                values: Vec::new(),
                messages: Vec::new(),
            },
            rex_ifml::FieldDef {
                name: "tier".to_string(),
                input: InputFieldType::Dropdown,
                required: false,
                validations: Vec::new(),
                values: vec!["gold".to_string(), "silver".to_string()],
                messages: Vec::new(),
            },
            rex_ifml::FieldDef {
                name: "stars".to_string(),
                input: InputFieldType::Custom("stars".to_string()),
                required: false,
                validations: Vec::new(),
                values: Vec::new(),
                messages: Vec::new(),
            },
        ],
    });
    let ctx = page_component_context_sync(&component_with_spec(Some(spec)));
    let form = ctx.form.expect("form render context");
    assert_eq!(form.fields[0].input_type, "text");
    assert!(form.fields[0].required);
    assert_eq!(form.fields[0].data_validate, "len(name) > 2");
    assert_eq!(form.fields[0].message.as_deref(), Some("Name too short"));
    assert_eq!(form.fields[1].input_type, "datetime-local");
    assert!(form.fields[2].is_select);
    assert_eq!(form.fields[2].values, vec!["gold", "silver"]);
    assert_eq!(form.fields[3].input_type, "stars");
}

#[test]
fn form_message_renders_validate_message_and_client_check() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let spec = ComponentSpec::Form(FormSpec {
        fields: vec![rex_ifml::FieldDef {
            name: "title".to_string(),
            input: InputFieldType::Text,
            required: true,
            validations: vec![Expression::BinOp {
                left: Box::new(Expression::Call {
                    name: "len".to_string(),
                    args: vec![Expression::Ident("title".to_string())],
                }),
                op: BinOp::Gt,
                right: Box::new(Expression::NumLit(2.0.into())),
            }],
            values: Vec::new(),
            messages: vec!["Title too short".to_string()],
        }],
    });
    let ctx = svelte_context(vec![page_component_context_sync(&component_with_spec(
        Some(spec),
    ))]);
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered
            .contains("data-validate=\"len(title) > 2\" data-validate-message=\"Title too short\""),
        "{rendered}"
    );
    assert!(rendered.contains("data-testid=\"grid-form\""), "{rendered}");
    assert!(
        rendered.contains("data-testid=\"grid-submit\""),
        "{rendered}"
    );
    assert!(
        rendered.contains("data-testid=\"grid-error\""),
        "{rendered}"
    );
    assert!(
        rendered.contains("form.querySelector(':invalid')"),
        "{rendered}"
    );
    assert!(
        rendered.contains("invalid?.getAttribute('data-validate-message')"),
        "{rendered}"
    );

    let plain = ComponentSpec::Form(FormSpec {
        fields: vec![rex_ifml::FieldDef {
            name: "title".to_string(),
            input: InputFieldType::Text,
            required: true,
            validations: Vec::new(),
            values: Vec::new(),
            messages: Vec::new(),
        }],
    });
    let ctx = svelte_context(vec![page_component_context_sync(&component_with_spec(
        Some(plain),
    ))]);
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(!rendered.contains("checkValidity"), "{rendered}");
    assert!(rendered.contains("data-testid=\"grid-form\""), "{rendered}");
    assert!(
        rendered.contains(
            "const formData = Object.fromEntries(new FormData(event.currentTarget as HTMLFormElement));"
        ),
        "{rendered}"
    );
}

#[test]
fn load_context_renders_param_default_fallbacks() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let list = page_component_context_sync(&component_with_spec(Some(table_spec())));
    let mut details = component_with_spec(None);
    details.component_type = "details".to_string();
    let details = page_component_context_sync(&details);
    let vc = IfmlViewContainer {
        name: "CustomerList".to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: true,
        is_modal: false,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: vec![
            super::super::context::ParameterDef {
                name: "slug".to_string(),
                type_ref: "String".to_string(),
                default: Some("'home'".to_string()),
            },
            super::super::context::ParameterDef {
                name: "customerId".to_string(),
                type_ref: "Uuid".to_string(),
                default: Some("'00000000-0000-0000-0000-000000000000'".to_string()),
            },
        ],
        components: vec![],
        events: Vec::new(),
        containers: Vec::new(),
    };
    let ctx = build_load_context("v1", &vc, &[list, details], "/", false);
    let rendered = render_template(&tera, "ifml/svelte/page_load.tera", &ctx).expect("render");
    assert!(
        rendered.contains("viewParams['slug'] = url.searchParams.get('slug') ?? 'home';"),
        "{rendered}"
    );
    assert!(
        rendered.contains("result.params = viewParams;"),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "viewParams['customerId'] = url.searchParams.get('customerId') ?? '00000000-0000-0000-0000-000000000000';"
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains("const customerId = viewParams['customerId'];"),
        "fetch ids must resolve from viewParams only (no route params): {rendered}"
    );
    assert!(
        !rendered.contains("params."),
        "route params are dead for query-param views: {rendered}"
    );

    let bare_vc = IfmlViewContainer {
        params: vec![super::super::context::ParameterDef {
            name: "customerId".to_string(),
            type_ref: "Uuid".to_string(),
            default: None,
        }],
        ..vc
    };
    let list = page_component_context_sync(&component_with_spec(Some(table_spec())));
    let ctx = build_load_context("v1", &bare_vc, &[list], "/", false);
    let rendered = render_template(&tera, "ifml/svelte/page_load.tera", &ctx).expect("render");
    assert!(
        rendered.contains("viewParams['customerId'] = url.searchParams.get('customerId') ?? '';"),
        "params without defaults fall back to the empty string: {rendered}"
    );
}

#[test]
fn paramless_view_load_stays_free_of_param_resolution() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let list = page_component_context_sync(&component_with_spec(Some(table_spec())));
    let vc = plain_vc("CustomerList");
    let ctx = build_load_context("v1", &vc, &[list], "/", false);
    let rendered = render_template(&tera, "ifml/svelte/page_load.tera", &ctx).expect("render");
    assert!(!rendered.contains("viewParams"), "{rendered}");
    assert!(!rendered.contains("result.params"), "{rendered}");
}

#[test]
fn role_guard_renders_at_top_of_load_function() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let vc = IfmlViewContainer {
        name: "AdminConsole".to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: false,
        is_modal: false,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: vec!["admin".to_string(), "manager".to_string()],
        requires: Vec::new(),
        params: Vec::new(),
        components: vec![],
        events: Vec::new(),
        containers: Vec::new(),
    };
    let ctx = build_load_context("v1", &vc, &[], "/", false);
    let rendered = render_template(&tera, "ifml/svelte/page_load.tera", &ctx).expect("render");
    assert!(
        rendered.contains("import { redirect } from '@sveltejs/kit';"),
        "{rendered}"
    );
    assert!(
        rendered.contains("import { currentRoles } from '$lib/roles';"),
        "{rendered}"
    );
    assert!(
        rendered.contains("const viewRoles = ['admin', 'manager'];"),
        "{rendered}"
    );
    assert!(
        rendered.contains("const roles = currentRoles();"),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "if (browser && viewRoles.length && !roles.some((r) => viewRoles.includes(r))) {"
        ),
        "{rendered}"
    );
    assert!(rendered.contains("throw redirect(303, '/');"), "{rendered}");
    assert!(!rendered.contains("can("), "{rendered}");
    let load_start = rendered.find("export const load").expect("load fn");
    let guard_start = rendered
        .find("const roles = currentRoles();")
        .expect("guard");
    assert!(
        guard_start > load_start,
        "guard must sit inside load: {rendered}"
    );
}

#[test]
fn no_roles_load_is_byte_identical_to_pre_guard_output() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let vc = IfmlViewContainer {
        name: "CustomerList".to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: false,
        is_modal: false,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: Vec::new(),
        components: vec![],
        events: Vec::new(),
        containers: Vec::new(),
    };
    let ctx = build_load_context("v1", &vc, &[], "/", false);
    let rendered = render_template(&tera, "ifml/svelte/page_load.tera", &ctx).expect("render");
    assert_eq!(
        rendered,
        "import type { PageLoad } from './$types';\n\
         \n\
         export const load: PageLoad = async ({ params, url, fetch }) => {\n\
         \tconst result: Record<string, unknown> = {};\n\
         \n\
         \n\
         \treturn result;\n\
         };\n"
    );
}

fn roles_helper_model(roles: Vec<String>) -> super::super::context::IfmlModel {
    model_with_guards(roles, Vec::new(), None)
}

fn model_with_guards(
    roles: Vec<String>,
    requires: Vec<String>,
    policy: Option<super::super::context::PolicyContext>,
) -> super::super::context::IfmlModel {
    let vc = IfmlViewContainer {
        name: "AdminConsole".to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: false,
        is_modal: false,
        conditional_expression: None,
        conditional_expr_json: None,
        roles,
        requires,
        params: Vec::new(),
        components: vec![],
        events: Vec::new(),
        containers: Vec::new(),
    };
    super::super::context::IfmlModel {
        view_containers: vec![vc],
        actions: Vec::new(),
        navigation_edges: Vec::new(),
        data_flows: Vec::new(),
        generation_order: Vec::new(),
        policy,
    }
}

#[test]
fn roles_helper_emitted_once_and_never_overwrites() {
    let tmp = tempfile::tempdir().unwrap();
    let generator = IfmlRouteGenerator::new(tmp.path(), "svelte");

    let file = generator
        .roles_helper(&roles_helper_model(vec!["admin".to_string()]))
        .expect("roles.ts for role-guarded model");
    assert!(file.path.ends_with("src/lib/roles.ts"), "{:?}", file.path);
    assert!(
        file.content
            .contains("export function currentRoles(): string[] {"),
        "{}",
        file.content
    );
    assert!(
        file.content
            .contains("(globalThis as any).__USER_ROLES__ ?? []"),
        "{}",
        file.content
    );
    assert!(
        !file.content.contains("can("),
        "roles-only helpers without policy must stay in the legacy shape: {}",
        file.content
    );

    std::fs::create_dir_all(file.path.parent().expect("parent dir")).unwrap();
    std::fs::write(&file.path, "custom roles helper").unwrap();
    assert!(
        generator
            .roles_helper(&roles_helper_model(vec!["admin".to_string()]))
            .is_none(),
        "existing roles.ts must never be overwritten"
    );

    assert!(
        generator
            .roles_helper(&roles_helper_model(Vec::new()))
            .is_none(),
        "no role-guarded views must mean no roles.ts"
    );

    let react = IfmlRouteGenerator::new(tmp.path(), "react");
    assert!(
        react
            .roles_helper(&roles_helper_model(vec!["admin".to_string()]))
            .is_none(),
        "guard helper is svelte-only in this slice"
    );
}

fn plain_vc(name: &str) -> IfmlViewContainer {
    IfmlViewContainer {
        name: name.to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: false,
        is_modal: false,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: Vec::new(),
        components: vec![],
        events: Vec::new(),
        containers: Vec::new(),
    }
}

fn guarded_vc(name: &str, roles: Vec<String>, requires: Vec<String>) -> IfmlViewContainer {
    IfmlViewContainer {
        name: name.to_string(),
        roles,
        requires,
        ..plain_vc(name)
    }
}

fn model_of(vcs: Vec<IfmlViewContainer>) -> super::super::context::IfmlModel {
    super::super::context::IfmlModel {
        view_containers: vcs,
        actions: Vec::new(),
        navigation_edges: Vec::new(),
        data_flows: Vec::new(),
        generation_order: Vec::new(),
        policy: None,
    }
}

#[test]
fn denial_target_prefers_first_unguarded_view() {
    assert_eq!(
        denial_target(&model_of(vec![
            guarded_vc("Admin", vec!["admin".to_string()], Vec::new()),
            plain_vc("Public"),
        ])),
        "/public"
    );
    assert_eq!(
        denial_target(&model_of(vec![
            guarded_vc("Vault", Vec::new(), vec!["open_vault".to_string()]),
            plain_vc("Public"),
        ])),
        "/public",
        "requires-only views count as guarded"
    );
    assert_eq!(
        denial_target(&model_of(vec![plain_vc("First"), plain_vc("Second")])),
        "/first",
        "graph order decides between unguarded views"
    );
    assert_eq!(
        denial_target(&model_of(vec![guarded_vc(
            "Admin",
            vec!["admin".to_string()],
            Vec::new()
        )])),
        "/",
        "no unguarded view falls back to /"
    );
    assert_eq!(denial_target(&model_of(vec![])), "/");
}

#[test]
fn capability_guard_renders_can_checks_and_denial_target() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let model = model_of(vec![
        guarded_vc(
            "RefundConsole",
            Vec::new(),
            vec!["manage_refunds".to_string()],
        ),
        plain_vc("CustomerList"),
    ]);
    let target = denial_target(&model);
    assert_eq!(target, "/customerlist");
    let ctx = build_load_context("v1", &model.view_containers[0], &[], &target, false);
    let rendered = render_template(&tera, "ifml/svelte/page_load.tera", &ctx).expect("render");
    assert!(
        rendered.contains("import { redirect } from '@sveltejs/kit';"),
        "{rendered}"
    );
    assert!(
        rendered.contains("import { can } from '$lib/roles';"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("currentRoles"),
        "capability-only views need no roles import: {rendered}"
    );
    assert!(
        rendered.contains("const viewRequires = ['manage_refunds'];"),
        "{rendered}"
    );
    assert!(
        rendered
            .contains("if (browser && viewRequires.length && !viewRequires.some((c) => can(c))) {"),
        "{rendered}"
    );
    assert!(
        rendered.contains("throw redirect(303, '/customerlist');"),
        "{rendered}"
    );
}

#[test]
fn combined_guard_renders_capability_check_before_roles_check() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let vc = guarded_vc(
        "AdminConsole",
        vec!["admin".to_string()],
        vec!["manage_refunds".to_string()],
    );
    let ctx = build_load_context("v1", &vc, &[], "/", false);
    let rendered = render_template(&tera, "ifml/svelte/page_load.tera", &ctx).expect("render");
    assert!(
        rendered.contains("import { currentRoles } from '$lib/roles';"),
        "{rendered}"
    );
    assert!(
        rendered.contains("import { can } from '$lib/roles';"),
        "{rendered}"
    );
    assert!(
        rendered.contains("const viewRoles = ['admin'];"),
        "{rendered}"
    );
    assert!(
        rendered.contains("const viewRequires = ['manage_refunds'];"),
        "{rendered}"
    );
    let cap_check = rendered
        .find("viewRequires.some((c) => can(c))")
        .expect("capability check");
    let role_check = rendered
        .find("roles.some((r) => viewRoles.includes(r))")
        .expect("roles check");
    assert!(
        cap_check < role_check,
        "capability check is primary and runs first: {rendered}"
    );
    assert_eq!(
        rendered.matches("throw redirect(303, '/');").count(),
        2,
        "both guards redirect to the denial target: {rendered}"
    );
}

fn form_component_named(name: &str, events: Vec<IfmlEvent>) -> IfmlComponent {
    let mut c = component_with_spec(Some(ComponentSpec::Form(FormSpec {
        fields: vec![field_def("name", InputFieldType::Text)],
    })));
    c.name = name.to_string();
    c.component_type = "form".to_string();
    c.events = events;
    c
}

fn guarded_view(
    name: &str,
    roles: Vec<String>,
    requires: Vec<String>,
    components: Vec<IfmlComponent>,
) -> IfmlViewContainer {
    IfmlViewContainer {
        roles,
        requires,
        components,
        ..plain_vc(name)
    }
}

fn page_context_for(
    vc: &IfmlViewContainer,
    mappings: Option<&IfmlComponentMappings>,
) -> PageSvelteContext {
    futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        vc,
        mappings,
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx")
}

#[test]
fn requires_view_gates_submit_behind_can_check() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let vc = guarded_view(
        "RefundEdit",
        Vec::new(),
        vec!["manage_refunds".to_string()],
        vec![form_component_named("editor", Vec::new())],
    );
    let ctx = page_context_for(&vc, None);
    assert!(!ctx.control_gate.open.is_empty());
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains("import { can } from '$lib/roles';"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("currentRoles"),
        "capability-only views need no roles import: {rendered}"
    );
    assert!(
        rendered.contains("const viewRequires = ['manage_refunds'];"),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "{#if viewRequires.some((c) => can(c))}<button type=\"submit\" data-testid=\"editor-submit\" disabled={submitting}>Submit</button>{/if}"
        ),
        "{rendered}"
    );
    let script_end = rendered.find("</script>").expect("script end");
    let consts = rendered.find("const viewRequires").expect("consts");
    assert!(
        consts < script_end,
        "gate consts must sit inside the script: {rendered}"
    );
}

#[test]
fn roles_only_view_gates_submit_behind_role_check() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let vc = guarded_view(
        "AdminConsole",
        vec!["admin".to_string(), "manager".to_string()],
        Vec::new(),
        vec![form_component_named("editor", Vec::new())],
    );
    let ctx = page_context_for(&vc, None);
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains("import { currentRoles } from '$lib/roles';"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("can("),
        "role-only views need no capability import: {rendered}"
    );
    assert!(
        rendered.contains("const viewRoles = ['admin', 'manager'];"),
        "{rendered}"
    );
    assert!(
        rendered.contains("const roles = currentRoles();"),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "{#if roles.some((r) => viewRoles.includes(r))}<button type=\"submit\" data-testid=\"editor-submit\""
        ),
        "{rendered}"
    );
    assert!(rendered.contains("</button>{/if}"), "{rendered}");
}

#[test]
fn combined_view_gates_submit_with_and_of_checks_matching_load_guard() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let vc = guarded_view(
        "AdminConsole",
        vec!["admin".to_string()],
        vec!["manage_refunds".to_string()],
        vec![form_component_named("editor", Vec::new())],
    );
    let ctx = page_context_for(&vc, None);
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains("import { currentRoles } from '$lib/roles';"),
        "{rendered}"
    );
    assert!(
        rendered.contains("import { can } from '$lib/roles';"),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "{#if viewRequires.some((c) => can(c)) && roles.some((r) => viewRoles.includes(r))}<button type=\"submit\""
        ),
        "markup must match the load guard's requires-AND-roles semantics: {rendered}"
    );
    let roles_import = rendered
        .find("import { currentRoles }")
        .expect("roles import");
    let can_import = rendered.find("import { can }").expect("can import");
    assert!(
        roles_import < can_import,
        "import order mirrors the load template: {rendered}"
    );
}

#[test]
fn unguarded_view_emits_no_gate_and_no_roles_import() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let vc = guarded_view(
        "CustomerEdit",
        Vec::new(),
        Vec::new(),
        vec![form_component_named("editor", Vec::new())],
    );
    let ctx = page_context_for(&vc, None);
    assert!(ctx.control_gate.open.is_empty());
    assert!(ctx.control_gate.imports.is_empty());
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(!rendered.contains("$lib/roles"), "{rendered}");
    assert!(!rendered.contains("viewRequires"), "{rendered}");
    assert!(!rendered.contains("viewRoles"), "{rendered}");
    assert!(!rendered.contains("can("), "{rendered}");
    assert!(
        rendered.contains(
            "<button type=\"submit\" data-testid=\"editor-submit\" disabled={submitting}>Submit</button>"
        ),
        "unguarded submit stays byte-identical: {rendered}"
    );
}

#[test]
fn list_only_guarded_view_emits_no_unused_gate_imports() {
    let vc = guarded_view(
        "RefundConsole",
        Vec::new(),
        vec!["manage_refunds".to_string()],
        vec![component_with_spec(Some(table_spec()))],
    );
    let ctx = page_context_for(&vc, None);
    assert!(
        ctx.control_gate.open.is_empty(),
        "no fallback form branch must mean no gate imports/consts"
    );
}

#[test]
fn mapped_submit_and_cancel_buttons_are_gated_too() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let mappings: IfmlComponentMappings = toml::from_str(
        r#"
[[component]]
role = "action-control"
path = "$lib/components/Button.svelte"
export = "Button"
testids = { root = "ui-button" }
"#,
    )
    .unwrap();
    let editor = form_component_named(
        "editor",
        vec![IfmlEvent {
            name: "comp_editor_cancel".to_string(),
            event_type: "cancel".to_string(),
            params: vec![],
            requires: Vec::new(),
            action: IfmlAction::Navigate {
                target: "CustomerList".to_string(),
                binding: HashMap::new(),
            },
        }],
    );
    let vc = guarded_view(
        "RefundEdit",
        Vec::new(),
        vec!["manage_refunds".to_string()],
        vec![editor],
    );
    let ctx = page_context_for(&vc, Some(&mappings));
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains(
            "{#if viewRequires.some((c) => can(c))}<Button onclick={submit_editor} disabled={submitting} testid=\"ui-button\">Submit</Button>{/if}"
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "{#if viewRequires.some((c) => can(c))}<Button onclick={comp_editor_cancel} testid=\"editor-cancel\">Cancel</Button>{/if}"
        ),
        "{rendered}"
    );
}

#[test]
fn gate_consts_pair_requires_and_roles_in_guard_order() {
    let gate =
        ControlGateContext::for_guards(&["manage_refunds".to_string()], &["admin".to_string()]);
    assert_eq!(
        gate.consts,
        "const viewRequires = ['manage_refunds'];\n\tconst viewRoles = ['admin'];\n\tconst roles = currentRoles();"
    );
    assert_eq!(
        gate.open,
        "{#if viewRequires.some((c) => can(c)) && roles.some((r) => viewRoles.includes(r))}"
    );
    assert_eq!(gate.close, "{/if}");
    assert_eq!(
        gate.imports,
        vec!["currentRoles".to_string(), "can".to_string()]
    );
}

#[test]
fn policy_roles_helper_embeds_role_capabilities() {
    let tmp = tempfile::tempdir().unwrap();
    let generator = IfmlRouteGenerator::new(tmp.path(), "svelte");
    let policy = super::super::context::PolicyContext {
        actors: vec![
            (
                "Admin".to_string(),
                vec!["manage_refunds".to_string(), "approve_expense".to_string()],
            ),
            ("Intern".to_string(), Vec::new()),
        ],
        capabilities: vec!["approve_expense".to_string(), "manage_refunds".to_string()],
    };
    let file = generator
        .roles_helper(&model_with_guards(
            vec!["admin".to_string()],
            Vec::new(),
            Some(policy),
        ))
        .expect("roles.ts for policy model");
    assert!(
        file.content
            .contains("const ROLE_CAPABILITIES: Record<string, string[]> = {"),
        "{}",
        file.content
    );
    assert!(
        file.content
            .contains("'Admin': ['manage_refunds', 'approve_expense'],"),
        "{}",
        file.content
    );
    assert!(file.content.contains("'Intern': [],"), "{}", file.content);
    assert!(
        file.content
            .contains("export function can(capability: string): boolean {"),
        "{}",
        file.content
    );
    assert!(
        file.content
            .contains("(globalThis as any).__USER_CAPABILITIES__ ?? []"),
        "{}",
        file.content
    );
    assert!(
        file.content.contains("ROLE_CAPABILITIES[role] ?? []"),
        "{}",
        file.content
    );
}

#[test]
fn requires_only_roles_helper_checks_user_capabilities_without_policy() {
    let tmp = tempfile::tempdir().unwrap();
    let generator = IfmlRouteGenerator::new(tmp.path(), "svelte");
    let file = generator
        .roles_helper(&model_with_guards(
            Vec::new(),
            vec!["manage_refunds".to_string()],
            None,
        ))
        .expect("roles.ts for requires-only model");
    assert!(
        file.content
            .contains("export function can(capability: string): boolean {"),
        "{}",
        file.content
    );
    assert!(
        file.content
            .contains("(globalThis as any).__USER_CAPABILITIES__ ?? []"),
        "{}",
        file.content
    );
    assert!(
        !file.content.contains("ROLE_CAPABILITIES"),
        "no policy must mean no embedded capability map: {}",
        file.content
    );
}

#[test]
fn chart_spec_maps_kind_and_axes() {
    let spec = ComponentSpec::Chart(ChartSpec {
        kind: ChartKind::Bar,
        label_field: Some("region".to_string()),
        value_fields: vec!["revenue".to_string(), "cost".to_string()],
    });
    let ctx = page_component_context_sync(&component_with_spec(Some(spec)));
    let chart = ctx.chart.expect("chart render context");
    assert_eq!(chart.kind, "bar");
    assert_eq!(chart.label_field.as_deref(), Some("region"));
    assert_eq!(chart.value_fields, vec!["revenue", "cost"]);
}

#[test]
fn render_expression_covers_operators_and_calls() {
    let expr = Expression::BinOp {
        left: Box::new(Expression::Call {
            name: "len".to_string(),
            args: vec![Expression::Ident("name".to_string())],
        }),
        op: BinOp::Gt,
        right: Box::new(Expression::NumLit(2.0.into())),
    };
    assert_eq!(render_expression(&expr), "len(name) > 2");

    let and = Expression::BinOp {
        left: Box::new(Expression::BoolLit(true)),
        op: BinOp::And,
        right: Box::new(Expression::UnaryOp {
            op: UnaryOp::Not,
            operand: Box::new(Expression::BoolLit(false)),
        }),
    };
    assert_eq!(render_expression(&and), "true && !false");

    let string = Expression::StringLit("a \"quoted\" b".to_string());
    assert_eq!(render_expression(&string), "\"a \\\"quoted\\\" b\"");
}

#[test]
fn nav_url_builds_query_from_sorted_bindings() {
    let mut binding = HashMap::new();
    binding.insert("customerId".to_string(), "row.id".to_string());
    binding.insert("region".to_string(), "\"eu\"".to_string());
    assert_eq!(
        nav_url_expr("CustomerDetail", &binding),
        "`/customerdetail?customerId=${row.id}&region=${\"eu\"}`"
    );
    assert_eq!(
        nav_url_expr("CustomerList", &HashMap::new()),
        "\"/customerlist\""
    );
}

#[test]
fn navigate_event_resolves_handler_and_url() {
    let evt = IfmlEvent {
        name: "comp_grid_select".to_string(),
        event_type: "select".to_string(),
        params: vec!["row".to_string()],
        requires: Vec::new(),
        action: IfmlAction::Navigate {
            target: "CustomerDetail".to_string(),
            binding: HashMap::new(),
        },
    };
    let rendered = render_event(&evt, &HashSet::new());
    assert_eq!(rendered.handler_name, "comp_grid_select");
    assert_eq!(rendered.action_kind, "navigate");
    assert_eq!(rendered.url_expr, "\"/customerdetail\"");
}

fn svelte_context(components: Vec<PageComponentContext>) -> PageSvelteContext {
    PageSvelteContext {
        api_version: "v1".to_string(),
        name: "View".to_string(),
        label: "View".to_string(),
        groups: vec![RenderGroup {
            heading: None,
            components: components.clone(),
        }],
        components,
        params: Vec::new(),
        view_params: Vec::new(),
        view_events: Vec::new(),
        imports: Vec::new(),
        needs_goto: false,
        needs_on_mount: false,
        needs_invalidate: false,
        has_submit: false,
        view_role: None,
        container_role: None,
        roles: Vec::new(),
        requires: Vec::new(),
        control_gate: ControlGateContext::default(),
        modal: None,
        container: None,
        ux: None,
        guard_expr: None,
    }
}

#[test]
fn template_renders_typed_table_and_form_markup() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let table_ctx = page_component_context_sync(&component_with_spec(Some(table_spec())));
    let ctx = svelte_context(vec![table_ctx]);
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains("<table data-testid=\"grid-table\" data-pagination=\"true\">"),
        "{rendered}"
    );
    assert!(
        rendered.contains("<tr data-testid=\"grid-row\">"),
        "{rendered}"
    );
    assert!(rendered.contains("<th>Name</th>"), "{rendered}");
    assert!(
        rendered.contains("<td>{item.tenure_years(Customer.hire_date)}</td>"),
        "{rendered}"
    );

    let chart = ComponentSpec::Chart(ChartSpec {
        kind: ChartKind::Pie,
        label_field: None,
        value_fields: vec!["revenue".to_string()],
    });
    let ctx = svelte_context(vec![page_component_context_sync(&component_with_spec(
        Some(chart),
    ))]);
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(rendered.contains("data-chart-kind=\"pie\""), "{rendered}");
    assert!(
        rendered.contains("data-value-fields=\"revenue\""),
        "{rendered}"
    );
}

#[test]
fn template_specless_table_component_renders_nothing() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let ctx = svelte_context(vec![page_component_context_sync(&component_with_spec(
        None,
    ))]);
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(!rendered.contains("<table"));
    assert!(!rendered.contains("<form"));
    assert!(!rendered.contains("data-chart-kind"));
    assert!(!rendered.contains("<h1>"));
    assert!(rendered.trim_end().ends_with("</svelte:head>"));
}

#[test]
fn mapped_component_renders_invocation_with_import_and_events() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let mut c = component_with_spec(Some(table_spec()));
    c.events.push(IfmlEvent {
        name: "comp_grid_select".to_string(),
        event_type: "select".to_string(),
        params: vec![],
        requires: Vec::new(),
        action: IfmlAction::Navigate {
            target: "CustomerDetail".to_string(),
            binding: HashMap::new(),
        },
    });
    let mappings: IfmlComponentMappings = toml::from_str(
        r#"
[[component]]
kind = "table"
path = "$lib/components/DataTable.svelte"
export = "DataTable"
testids = { root = "data-table", row = "data-row" }
"#,
    )
    .unwrap();
    let vc = IfmlViewContainer {
        name: "CustomerList".to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: true,
        is_modal: false,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: Vec::new(),
        components: vec![c],
        events: Vec::new(),
        containers: Vec::new(),
    };
    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc,
        Some(&mappings),
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");

    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains("import DataTable from '$lib/components/DataTable.svelte';"),
        "{rendered}"
    );
    assert!(
        rendered.contains("import { goto } from '$app/navigation';"),
        "{rendered}"
    );
    assert!(rendered.contains("<DataTable"), "{rendered}");
    assert!(rendered.contains("data={data.items}"), "{rendered}");
    assert!(
        rendered.contains("fields={['name', 'status']}"),
        "{rendered}"
    );
    assert!(
        rendered.contains("onselect={comp_grid_select}"),
        "Svelte 5 event-property form must reach the component: {rendered}"
    );
    assert!(
        rendered.contains("<h1>CustomerList</h1>"),
        "mapped component branches must keep the view heading: {rendered}"
    );
    let heading = rendered.find("<h1>").expect("heading");
    let invocation = rendered.find("<DataTable").expect("invocation");
    assert!(
        heading < invocation,
        "heading renders above the mapped component: {rendered}"
    );
    assert!(rendered.contains("testid=\"data-table\""), "{rendered}");
    assert!(rendered.contains("rowTestid=\"data-row\""), "{rendered}");
    assert!(!rendered.contains("<table"), "{rendered}");
    assert!(!rendered.contains("on:select"), "{rendered}");
}

#[test]
fn load_context_wires_first_list_with_pagination() {
    let list = page_component_context_sync(&component_with_spec(Some(table_spec())));
    let second_list = page_component_context_sync(&component_with_spec(None));
    let details = {
        let mut c = component_with_spec(None);
        c.component_type = "details".to_string();
        page_component_context_sync(&c)
    };
    let vc = IfmlViewContainer {
        name: "CustomerList".to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: true,
        is_modal: false,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: vec![super::super::context::ParameterDef {
            name: "customerId".to_string(),
            type_ref: "Uuid".to_string(),
            default: None,
        }],
        components: vec![],
        events: Vec::new(),
        containers: Vec::new(),
    };
    let ctx = build_load_context("v1", &vc, &[list, second_list, details], "/", false);
    assert!(ctx.has_fetch);
    assert_eq!(ctx.components.len(), 3);
    assert!(ctx.components[0].fetch_list);
    assert!(ctx.components[0].paginate);
    assert!(
        !ctx.components[1].fetch_list,
        "second list must not double-fetch"
    );
    assert!(ctx.components[2].fetch_item);
    assert_eq!(ctx.components[2].id_param.as_deref(), Some("customerId"));
}

fn field_def(name: &str, input: InputFieldType) -> rex_ifml::FieldDef {
    rex_ifml::FieldDef {
        name: name.to_string(),
        input,
        required: false,
        validations: Vec::new(),
        values: Vec::new(),
        messages: Vec::new(),
    }
}

#[test]
fn form_save_event_gets_action_control_role() {
    let evt = |event_type: &str| IfmlEvent {
        name: format!("comp_form_{event_type}"),
        event_type: event_type.to_string(),
        params: Vec::new(),
        requires: Vec::new(),
        action: IfmlAction::Navigate {
            target: "CustomerList".to_string(),
            binding: HashMap::new(),
        },
    };
    for event_type in ["save", "submit", "cancel", "back", "click"] {
        let rendered = render_event(&evt(event_type), &HashSet::new());
        assert_eq!(
            rendered.role,
            Some(SemanticRole::ActionControl),
            "{event_type}"
        );
    }
    assert_eq!(render_event(&evt("select"), &HashSet::new()).role, None);
    assert_eq!(render_event(&evt("load"), &HashSet::new()).role, None);
}

#[test]
fn dropdown_and_radio_inputs_carry_selection_field_role() {
    let spec = ComponentSpec::Form(FormSpec {
        fields: vec![
            field_def("tier", InputFieldType::Dropdown),
            field_def("channel", InputFieldType::RadioGroup),
            field_def("name", InputFieldType::Text),
        ],
    });
    let ctx = page_component_context_sync(&component_with_spec(Some(spec)));
    let form = ctx.form.expect("form render context");
    assert_eq!(
        form.fields[0].input_role,
        Some(SemanticRole::SelectionField)
    );
    assert_eq!(
        form.fields[1].input_role,
        Some(SemanticRole::SelectionField)
    );
    assert_eq!(form.fields[2].input_role, None);
}

#[test]
fn component_roles_classify_slots() {
    let table = component_with_spec(Some(table_spec()));
    assert_eq!(
        page_component_context_sync(&table).role,
        Some(SemanticRole::Collection)
    );

    let mut details = component_with_spec(None);
    details.component_type = "details".to_string();
    assert_eq!(
        page_component_context_sync(&details).role,
        Some(SemanticRole::Display)
    );

    let mut dropdown = component_with_spec(None);
    dropdown.component_type = "dropdown".to_string();
    assert_eq!(
        page_component_context_sync(&dropdown).role,
        Some(SemanticRole::SelectionField)
    );

    let form = component_with_spec(Some(ComponentSpec::Form(FormSpec { fields: vec![] })));
    assert_eq!(page_component_context_sync(&form).role, None);
}

#[test]
fn view_roles_mark_modal_landmark_and_xor_containers() {
    let vc = |is_modal: bool, is_landmark: bool, is_xor: bool| IfmlViewContainer {
        name: "CustomerDialog".to_string(),
        label: None,
        is_xor,
        is_default: false,
        is_landmark,
        is_modal,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: Vec::new(),
        components: vec![],
        events: Vec::new(),
        containers: Vec::new(),
    };
    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc(true, false, false),
        None,
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    assert_eq!(ctx.view_role, Some(SemanticRole::ModalView));
    assert_eq!(ctx.container_role, None);

    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc(false, true, false),
        None,
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    assert_eq!(ctx.view_role, Some(SemanticRole::Shell));

    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc(false, false, true),
        None,
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    assert_eq!(ctx.view_role, None);
    assert_eq!(
        ctx.container_role,
        Some(SemanticRole::PresentationContainer)
    );
}

#[test]
fn paginated_table_gets_pagination_role() {
    let paged = TableSpec {
        columns: vec![],
        pagination: true,
    };
    assert_eq!(render_table(&paged).role, Some(SemanticRole::Pagination));
    let plain = TableSpec {
        columns: vec![],
        pagination: false,
    };
    assert_eq!(render_table(&plain).role, None);
}

#[test]
fn role_mapping_resolves_for_collection_slot() {
    let mappings: IfmlComponentMappings = toml::from_str(
        r#"
[[component]]
role = "collection"
path = "$lib/components/Collection.svelte"
"#,
    )
    .unwrap();
    let mut c = component_with_spec(Some(table_spec()));
    c.component_type = "list".to_string();
    let ctx = page_component_context_for_tests(&c, &mappings);
    let mapping = ctx.mapping.expect("role-mapped component");
    assert_eq!(mapping.import_name, "Collection");
}

fn form_component() -> IfmlComponent {
    let mut c = component_with_spec(Some(ComponentSpec::Form(FormSpec {
        fields: vec![field_def("name", InputFieldType::Text)],
    })));
    c.name = "editor".to_string();
    c.component_type = "form".to_string();
    c
}

fn save_event() -> IfmlEvent {
    IfmlEvent {
        name: "comp_editor_save".to_string(),
        event_type: "save".to_string(),
        params: Vec::new(),
        requires: Vec::new(),
        action: IfmlAction::Navigate {
            target: "CustomerList".to_string(),
            binding: HashMap::new(),
        },
    }
}

fn form_view(is_modal: bool) -> IfmlViewContainer {
    IfmlViewContainer {
        name: if is_modal {
            "CustomerDialog".to_string()
        } else {
            "CustomerEdit".to_string()
        },
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: false,
        is_modal,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: Vec::new(),
        components: vec![form_component()],
        events: Vec::new(),
        containers: Vec::new(),
    }
}

fn button_mappings() -> IfmlComponentMappings {
    toml::from_str(
        r#"
[[component]]
role = "action-control"
path = "$lib/components/Button.svelte"
export = "Button"
testids = { root = "ui-button" }
"#,
    )
    .unwrap()
}

#[test]
fn mapped_save_event_renders_button_invocation() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let mut vc = form_view(false);
    vc.components[0].events.push(save_event());
    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc,
        Some(&button_mappings()),
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains("import Button from '$lib/components/Button.svelte';"),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "<Button onclick={submit_editor} disabled={submitting} testid=\"ui-button\">Save</Button>"
        ),
        "{rendered}"
    );
    assert!(
        !rendered.contains("<button type=\"submit\""),
        "mapped button must replace the hardcoded fallback: {rendered}"
    );
}

#[test]
fn mapped_cancel_event_renders_secondary_button() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let mut vc = form_view(false);
    vc.components[0].events.push(IfmlEvent {
        name: "comp_editor_cancel".to_string(),
        event_type: "cancel".to_string(),
        params: Vec::new(),
        requires: Vec::new(),
        action: IfmlAction::Navigate {
            target: "CustomerList".to_string(),
            binding: HashMap::new(),
        },
    });
    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc,
        Some(&button_mappings()),
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains(
            "<Button onclick={comp_editor_cancel} testid=\"editor-cancel\">Cancel</Button>"
        ),
        "secondary buttons must not share the submit button's root testid: {rendered}"
    );
}

#[test]
fn unmapped_form_keeps_hardcoded_submit_button() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let mut vc = form_view(false);
    vc.components[0].events.push(save_event());
    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc,
        Some(&IfmlComponentMappings::default()),
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains(
            "<button type=\"submit\" data-testid=\"editor-submit\" disabled={submitting}>Submit</button>"
        ),
        "{rendered}"
    );
    assert!(!rendered.contains("<Button"), "{rendered}");
}

#[test]
fn modal_view_with_mapping_renders_dialog_wrapper() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let mut vc = form_view(true);
    vc.components[0].events.push(save_event());
    let mappings: IfmlComponentMappings = toml::from_str(
        r#"
[[component]]
role = "modal-view"
path = "$lib/components/Dialog.svelte"
export = "Dialog"
testids = { root = "customer-modal" }

[[component]]
role = "action-control"
path = "$lib/components/Button.svelte"
export = "Button"
"#,
    )
    .unwrap();
    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc,
        Some(&mappings),
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains("import Dialog from '$lib/components/Dialog.svelte';"),
        "{rendered}"
    );
    assert!(
        rendered.contains("<Dialog bind:open={dialog_open} testid=\"customer-modal\">"),
        "{rendered}"
    );
    assert!(rendered.contains("</Dialog>"), "{rendered}");
    assert!(
        rendered.contains("let dialog_open = $state(true);"),
        "{rendered}"
    );
    assert!(rendered.contains("history.back();"), "{rendered}");
    assert!(
        rendered.contains("data-testid=\"customerdialog-modal-close\" onclick={close_dialog}"),
        "{rendered}"
    );
}

#[test]
fn modal_view_without_modal_mapping_renders_builtin_div_fallback() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let vc = form_view(true);
    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc,
        Some(&button_mappings()),
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered
            .contains("<div class=\"modal\" role=\"dialog\" data-testid=\"customerdialog-modal\">"),
        "{rendered}"
    );
    assert!(rendered.contains("</div>"), "{rendered}");
    assert!(!rendered.contains("bind:open"), "{rendered}");
    assert!(
        rendered.contains("let dialog_open = $state(true);"),
        "{rendered}"
    );
}

#[test]
fn modal_view_without_pack_renders_plain_page() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let vc = form_view(true);
    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc,
        None,
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(!rendered.contains("dialog_open"), "{rendered}");
    assert!(!rendered.contains("class=\"modal\""), "{rendered}");
    assert!(
        rendered.contains("<form data-testid=\"editor-form\""),
        "no-mapping modal views must render exactly as before: {rendered}"
    );
}

#[test]
fn navigation_into_modal_target_appends_dialog_param() {
    let targets: HashSet<String> = ["CustomerDialog".to_string()].into_iter().collect();
    let evt = IfmlEvent {
        name: "comp_grid_select".to_string(),
        event_type: "select".to_string(),
        params: vec!["row".to_string()],
        requires: Vec::new(),
        action: IfmlAction::Navigate {
            target: "CustomerDialog".to_string(),
            binding: HashMap::new(),
        },
    };
    assert_eq!(
        render_event(&evt, &targets).url_expr,
        "\"/customerdialog?dialog=open\""
    );

    let mut binding = HashMap::new();
    binding.insert("customerId".to_string(), "row.id".to_string());
    let bound = IfmlEvent {
        action: IfmlAction::Navigate {
            target: "CustomerDialog".to_string(),
            binding,
        },
        ..evt
    };
    assert_eq!(
        render_event(&bound, &targets).url_expr,
        "`/customerdialog?customerId=${row.id}&dialog=open`"
    );
    assert_eq!(
        render_event(&bound, &HashSet::new()).url_expr,
        "`/customerdialog?customerId=${row.id}`",
        "non-modal targets must keep byte-identical URLs"
    );
}

fn xor_view() -> IfmlViewContainer {
    IfmlViewContainer {
        name: "Checkout".to_string(),
        label: Some("Checkout".to_string()),
        is_xor: true,
        is_default: false,
        is_landmark: false,
        is_modal: false,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: Vec::new(),
        components: vec![],
        events: Vec::new(),
        containers: Vec::new(),
    }
}

fn container_mappings() -> IfmlComponentMappings {
    toml::from_str(
        r#"
[[component]]
role = "presentation-container"
path = "$lib/components/Card.svelte"
export = "Card"
testids = { root = "card" }
"#,
    )
    .unwrap()
}

#[test]
fn xor_view_with_mapping_renders_card_wrapper_around_children() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let mut vc = xor_view();
    vc.components.push(form_component());
    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc,
        Some(&container_mappings()),
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains("import Card from '$lib/components/Card.svelte';"),
        "{rendered}"
    );
    assert!(rendered.contains("<Card testid=\"card\">"), "{rendered}");
    assert!(rendered.contains("</Card>"), "{rendered}");
    assert!(
        rendered.contains("<form data-testid=\"editor-form\""),
        "children must render inside the wrapper: {rendered}"
    );
    let open = rendered.find("<Card testid=\"card\">").expect("open");
    let child = rendered.find("<form").expect("form");
    let close = rendered.rfind("</Card>").expect("close");
    assert!(open < child && child < close, "{rendered}");
}

#[test]
fn xor_view_without_container_mapping_renders_section_fallback() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let vc = xor_view();
    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc,
        Some(&button_mappings()),
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains("<section data-testid=\"checkout-container\">"),
        "{rendered}"
    );
    assert!(rendered.contains("</section>"), "{rendered}");
    assert!(!rendered.contains("<Card"), "{rendered}");
}

#[test]
fn xor_view_without_pack_renders_plain_page() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let mut vc = xor_view();
    vc.components.push(form_component());
    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc,
        None,
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(!rendered.contains("<section"), "{rendered}");
    assert!(!rendered.contains("<Card"), "{rendered}");
    assert!(
        rendered.contains("<form data-testid=\"editor-form\""),
        "no-pack xor views must render exactly as before: {rendered}"
    );
}

#[test]
fn mapped_container_testid_resolves_only_for_xor_with_mapping() {
    assert_eq!(
        mapped_container_testid("Checkout", true, Some(&container_mappings())),
        Some("card".to_string())
    );
    assert_eq!(
        mapped_container_testid("Checkout", true, Some(&button_mappings())),
        None
    );
    assert_eq!(
        mapped_container_testid("Checkout", false, Some(&container_mappings())),
        None
    );
}

fn tabs_mappings() -> IfmlComponentMappings {
    toml::from_str(
        r#"
[[component]]
role = "presentation-container"
path = "$lib/components/ui/tabs/tabs.svelte"
export = "Tabs"
testids = { root = "tabs" }
"#,
    )
    .unwrap()
}

fn xor_container_with_form(name: &str, label: &str, form_name: &str) -> IfmlViewContainer {
    IfmlViewContainer {
        name: name.to_string(),
        label: Some(label.to_string()),
        is_xor: true,
        is_default: false,
        is_landmark: false,
        is_modal: false,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: Vec::new(),
        components: vec![form_component_named(form_name, Vec::new())],
        events: Vec::new(),
        containers: Vec::new(),
    }
}

#[test]
fn sibling_xor_containers_render_inside_one_mapped_presentation_container() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let mut vc = plain_vc("Checkout");
    vc.label = Some("Checkout".to_string());
    vc.containers = vec![
        xor_container_with_form("Shipping", "Shipping", "shipping_form"),
        xor_container_with_form("Payment", "Payment", "payment_form"),
    ];
    let ctx = futures::executor::block_on(build_page_context(
        &MockEngine::new(),
        &test_config(),
        "v1",
        &vc,
        Some(&tabs_mappings()),
        &HashSet::new(),
        None,
        &ProjectConfig::default(),
    ))
    .expect("page ctx");
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains("import Tabs from '$lib/components/ui/tabs/tabs.svelte';"),
        "{rendered}"
    );
    assert_eq!(
        rendered.matches("<Tabs testid=\"tabs\">").count(),
        1,
        "sibling xor containers must share ONE mapped presentation-container wrapper: {rendered}"
    );
    assert!(
        rendered.contains("data-testid=\"shipping-label\">Shipping<"),
        "each group renders its container label with a stable testid: {rendered}"
    );
    assert!(
        rendered.contains("data-testid=\"payment-label\">Payment<"),
        "{rendered}"
    );
    assert!(
        rendered.contains("data-testid=\"shipping_form-form\""),
        "{rendered}"
    );
    assert!(
        rendered.contains("data-testid=\"payment_form-form\""),
        "{rendered}"
    );
    let open = rendered.find("<Tabs testid=\"tabs\">").expect("open");
    let shipping = rendered
        .find("data-testid=\"shipping-label\"")
        .expect("shipping group");
    let payment = rendered
        .find("data-testid=\"payment-label\"")
        .expect("payment group");
    let close = rendered.rfind("</Tabs>").expect("close");
    assert!(
        open < shipping && shipping < payment && payment < close,
        "both groups render inside the wrapper in container order: {rendered}"
    );
}

#[test]
fn shell_nav_builds_items_from_landmark_navigate_events() {
    let mut list = IfmlViewContainer {
        name: "CustomerList".to_string(),
        label: Some("Customers".to_string()),
        is_xor: false,
        is_default: false,
        is_landmark: true,
        is_modal: false,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: Vec::new(),
        components: vec![],
        events: Vec::new(),
        containers: Vec::new(),
    };
    list.events.push(IfmlEvent {
        name: "comp_grid_select".to_string(),
        event_type: "select".to_string(),
        params: vec!["row".to_string()],
        requires: Vec::new(),
        action: IfmlAction::Navigate {
            target: "CustomerDetail".to_string(),
            binding: HashMap::new(),
        },
    });
    let detail = IfmlViewContainer {
        name: "CustomerDetail".to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: false,
        is_modal: false,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: Vec::new(),
        components: vec![],
        events: Vec::new(),
        containers: Vec::new(),
    };
    let shell_mappings: IfmlComponentMappings = toml::from_str(
        r#"
[[component]]
role = "shell"
path = "$lib/components/Nav.svelte"
export = "Nav"
testids = { root = "side-nav" }
"#,
    )
    .unwrap();
    let nav = shell_nav(&[list, detail], Some(&shell_mappings)).expect("shell nav");
    assert_eq!(nav.import.export_name, "Nav");
    assert_eq!(nav.testid.as_deref(), Some("side-nav"));
    assert_eq!(nav.items.len(), 1);
    assert_eq!(nav.items[0].label, "CustomerDetail");
    assert_eq!(nav.items[0].href_attr, "href={\"/customerdetail\"}");
}

#[test]
fn shell_nav_is_none_without_landmark_or_shell_mapping() {
    let plain = IfmlViewContainer {
        name: "CustomerList".to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: false,
        is_modal: false,
        conditional_expression: None,
        conditional_expr_json: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: Vec::new(),
        components: vec![],
        events: Vec::new(),
        containers: Vec::new(),
    };
    assert!(shell_nav(std::slice::from_ref(&plain), Some(&container_mappings())).is_none());
    assert!(shell_nav(&[plain], None).is_none());
}

fn nav_shell_mappings() -> IfmlComponentMappings {
    toml::from_str(
        r#"
[[component]]
role = "shell"
path = "$lib/components/Nav.svelte"
export = "Nav"
testids = { root = "side-nav" }
"#,
    )
    .unwrap()
}

fn landmark_with_select_event(
    binding: HashMap<String, String>,
) -> super::super::context::IfmlViewContainer {
    let mut list = plain_vc("CustomerList");
    list.is_landmark = true;
    list.events.push(IfmlEvent {
        name: "comp_grid_select".to_string(),
        event_type: "select".to_string(),
        params: vec!["row".to_string()],
        requires: Vec::new(),
        action: IfmlAction::Navigate {
            target: "CustomerDetail".to_string(),
            binding,
        },
    });
    list
}

#[test]
fn shell_nav_drops_event_scoped_bindings_from_nav_hrefs() {
    let list = landmark_with_select_event(HashMap::from([(
        "customerId".to_string(),
        "row.id".to_string(),
    )]));
    let nav = shell_nav(&[list], Some(&nav_shell_mappings())).expect("shell nav");
    assert_eq!(nav.items.len(), 1);
    assert_eq!(
        nav.items[0].href_attr, "href={\"/customerdetail\"}",
        "identifiers bound only in event params (row) are not in layout scope; \
         the nav link must be a plain route link: {:?}",
        nav.items[0].href_attr
    );
}

#[test]
fn shell_nav_keeps_bindings_that_do_not_reference_event_params() {
    let list = landmark_with_select_event(HashMap::from([(
        "tab".to_string(),
        "'overview'".to_string(),
    )]));
    let nav = shell_nav(&[list], Some(&nav_shell_mappings())).expect("shell nav");
    assert_eq!(nav.items.len(), 1);
    assert_eq!(
        nav.items[0].href_attr, "href={`/customerdetail?tab=${'overview'}`}",
        "static bindings stay on the nav link: {:?}",
        nav.items[0].href_attr
    );
}

#[test]
fn layout_template_renders_shell_navigation_and_children() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let ctx = LayoutSvelteContext {
        shell: RenderShellNav {
            import: RenderImport {
                export_name: "NavigationMenu".to_string(),
                import_path: "$lib/components/ui/navigation-menu/navigation-menu.svelte"
                    .to_string(),
            },
            testid: Some("navigation-menu".to_string()),
            items: vec![
                RenderNavItem {
                    label: "Customers".to_string(),
                    href_attr: "href=\"/customerlist\"".to_string(),
                },
                RenderNavItem {
                    label: "CustomerDetail".to_string(),
                    href_attr: "href={`?customerId=${row.id}`}".to_string(),
                },
            ],
        },
    };
    let rendered = render_template(&tera, "ifml/svelte/layout.tera", &ctx).expect("render");
    assert!(
        rendered.contains(
            "import NavigationMenu from '$lib/components/ui/navigation-menu/navigation-menu.svelte';"
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains("let { children }: { children: import('svelte').Snippet } = $props();"),
        "{rendered}"
    );
    assert!(
        rendered.contains("<NavigationMenu testid=\"navigation-menu\">"),
        "{rendered}"
    );
    assert!(
        rendered.contains("<a href=\"/customerlist\">Customers</a>"),
        "{rendered}"
    );
    assert!(
        rendered.contains("<a href={`?customerId=${row.id}`}>CustomerDetail</a>"),
        "{rendered}"
    );
    assert!(rendered.contains("</NavigationMenu>"), "{rendered}");
    assert!(rendered.contains("{@render children()}"), "{rendered}");
}

fn id_param() -> super::super::context::ParameterDef {
    super::super::context::ParameterDef {
        name: "customerId".to_string(),
        type_ref: "Uuid".to_string(),
        default: None,
    }
}

#[test]
fn create_view_submit_posts_to_collection_url() {
    let ctx = page_context_for(&form_view(false), None);
    let submit = ctx.components[0].submit.as_ref().expect("form submit");
    assert_eq!(submit.method, "POST");
    assert_eq!(submit.url_expr, "\"/api/v1/customer\"");
    assert!(
        !submit.url_expr.contains("${"),
        "create submit URL must not interpolate an id: {}",
        submit.url_expr
    );
}

#[test]
fn edit_view_submit_puts_to_item_url() {
    let mut vc = form_view(false);
    vc.params = vec![id_param()];
    let ctx = page_context_for(&vc, None);
    let submit = ctx.components[0].submit.as_ref().expect("form submit");
    assert_eq!(submit.method, "PUT");
    assert_eq!(
        submit.url_expr, "`/api/v1/customer/${viewParams.customerId}`",
        "edit submit must target the item URL through viewParams"
    );
}

#[test]
fn edit_view_page_branches_to_post_collection_in_create_mode() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let mut vc = form_view(false);
    vc.params = vec![id_param()];
    vc.components[0].events.push(save_event());
    let ctx = page_context_for(&vc, None);
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &ctx).expect("render");
    assert!(
        rendered.contains("method: 'POST'"),
        "opening an id-param form view without ?id (create mode) must POST the \
         collection URL; rendered submit:\n{rendered}"
    );
    assert!(
        rendered.contains("\"/api/v1/customer\"") || rendered.contains("`/api/v1/customer`"),
        "create mode must target the collection URL literal (no trailing \
         empty item id):\n{rendered}"
    );
    assert!(
        rendered.contains("method: 'PUT'"),
        "edit mode (?id present) must keep the item PUT:\n{rendered}"
    );
}

// ── expr_ir gated page guard (issue #278) ───────────────────────────

fn conditional_vc() -> IfmlViewContainer {
    let expr = serde_json::json!({
        "type": "binOp",
        "value": {
            "left": {"type": "fieldExpr", "value": {
                "object": {"type": "ident", "value": "row"},
                "field": "active"
            }},
            "op": "eq",
            "right": {"type": "boolLit", "value": true}
        }
    });
    let mut vc = plain_vc("Promo");
    vc.conditional_expression = Some("row.active == true".to_string());
    vc.conditional_expr_json = Some(expr.to_string());
    vc
}

#[test]
fn page_guard_expr_only_when_expr_ir_flag_on() {
    let vc = conditional_vc();
    let off = ProjectConfig::default();
    assert_eq!(
        page_guard_expr(&vc, &off).expect("flag OFF cannot fail"),
        None,
        "expr_ir OFF must never lower a guard"
    );
    let on = ProjectConfig {
        codegen: CodegenConfig {
            expr_ir: true,
            ..Default::default()
        },
        ..ProjectConfig::default()
    };
    assert_eq!(
        page_guard_expr(&vc, &on)
            .expect("lowering must succeed")
            .as_deref(),
        Some("(row.active === true)"),
        "expr_ir ON must lower the persisted AST"
    );
}

#[test]
fn page_guard_is_none_without_expr_json_or_condition() {
    let project = ProjectConfig {
        codegen: CodegenConfig {
            expr_ir: true,
            ..Default::default()
        },
        ..ProjectConfig::default()
    };
    let mut unconditioned = plain_vc("Plain");
    unconditioned.conditional_expr_json = None;
    assert_eq!(
        page_guard_expr(&unconditioned, &project).expect("lowerable"),
        None
    );
    let mut source_only = plain_vc("Plain");
    source_only.conditional_expression = Some("row.active == true".to_string());
    source_only.conditional_expr_json = None;
    assert_eq!(
        page_guard_expr(&source_only, &project).expect("lowerable"),
        None,
        "without a persisted AST there is nothing to lower"
    );
}

#[test]
fn guard_template_renders_from_ast_gated() {
    let tera = create_tera(Path::new(".")).expect("tera");
    let vc = conditional_vc();

    let off_ctx = page_context_for(&vc, None);
    assert_eq!(off_ctx.guard_expr, None);
    let off = render_template(&tera, "ifml/svelte/page.tera", &off_ctx).expect("render");
    assert!(
        !off.contains("view_guard"),
        "expr_ir OFF must render no guard: {off}"
    );

    let on = ProjectConfig {
        codegen: CodegenConfig {
            expr_ir: true,
            ..Default::default()
        },
        ..ProjectConfig::default()
    };
    let mut on_ctx = page_context_for(&vc, None);
    on_ctx.guard_expr = page_guard_expr(&vc, &on).expect("lowering must succeed");
    let rendered = render_template(&tera, "ifml/svelte/page.tera", &on_ctx).expect("render");
    assert!(
        rendered.contains("const view_guard = $derived(Boolean((row.active === true)));"),
        "expr_ir ON must render the lowered guard: {rendered}"
    );
}

/// Issue #468 W1b: integrated mode scopes the shell layout to the `(views)`
/// route group (never the entity app's root layout), while standalone keeps
/// the historical `src/routes/+layout.svelte` path.
#[test]
fn route_layout_honors_the_route_root() {
    let tmp = tempfile::tempdir().unwrap();
    let standalone = IfmlRouteGenerator::new(tmp.path(), "svelte");
    assert_eq!(
        standalone.route_layout(),
        Some(std::path::PathBuf::from("src/routes/+layout.svelte"))
    );

    let integrated =
        IfmlRouteGenerator::new(tmp.path(), "svelte").with_route_root(super::ROUTE_GROUP);
    assert_eq!(
        integrated.route_layout(),
        Some(std::path::PathBuf::from(
            "src/routes/(views)/+layout.svelte"
        ))
    );

    let react = IfmlRouteGenerator::new(tmp.path(), "react").with_route_root(super::ROUTE_GROUP);
    assert_eq!(react.route_layout(), None, "layout is svelte-only");
}

// ── Role hydration (issue #468) ──────────────────────────────────────────────

fn policy_with(actors: &[(&str, &[&str])]) -> super::super::context::PolicyContext {
    super::super::context::PolicyContext {
        actors: actors
            .iter()
            .map(|(name, caps)| {
                (
                    name.to_string(),
                    caps.iter().map(|c| c.to_string()).collect(),
                )
            })
            .collect(),
        capabilities: Vec::new(),
    }
}

/// The mapping convention: basejump 'owner' → the actor whose snake_case
/// name is 'owner', else the FIRST policy actor; 'member' → snake 'member',
/// else the LAST. Both forms (policy token + snake_case) are exposed.
#[test]
fn basejump_role_tokens_follow_the_documented_convention() {
    use super::hydration::basejump_role_tokens;

    // Name match wins: Owner/Member actors map to themselves.
    let policy = policy_with(&[
        ("Admin", &["manage"]),
        ("Member", &["record"]),
        ("Owner", &["manage", "delete"]),
    ]);
    assert_eq!(
        basejump_role_tokens(&policy, "owner"),
        vec!["Owner".to_string(), "owner".to_string()]
    );
    assert_eq!(
        basejump_role_tokens(&policy, "member"),
        vec!["Member".to_string(), "member".to_string()]
    );

    // Fallback: no Owner/Member actor — owner takes the FIRST policy actor
    // (PolicyContext.actors is sorted), member the LAST.
    let fallback = policy_with(&[("Admin", &["manage"]), ("Agent", &[])]);
    assert_eq!(
        basejump_role_tokens(&fallback, "owner"),
        vec!["Admin".to_string(), "admin".to_string()]
    );
    assert_eq!(
        basejump_role_tokens(&fallback, "member"),
        vec!["Agent".to_string(), "agent".to_string()]
    );

    // Snake-named actors dedup to the single form.
    let snake = policy_with(&[("admin", &["manage"])]);
    assert_eq!(
        basejump_role_tokens(&snake, "owner"),
        vec!["admin".to_string()]
    );

    // No actors — no mapping.
    let empty = policy_with(&[]);
    assert!(basejump_role_tokens(&empty, "owner").is_empty());
}

/// A view's `roles:` token matches the actor verbatim OR snake_case (the
/// crewbase convention: `roles: [member]` vs actor `Member`).
#[test]
fn role_token_matches_both_token_forms() {
    use super::hydration::role_token_matches;
    assert!(role_token_matches("Admin", "Admin"));
    assert!(role_token_matches("admin", "Admin"));
    assert!(role_token_matches("benefits_admin", "BenefitsAdmin"));
    assert!(role_token_matches("Member", "Member"));
    assert!(!role_token_matches("admin", "Agent"));
    assert!(!role_token_matches("hr_admin", "HrReader"));
}

/// Integrated mode emits the hydration layouts over the `(views)` group;
/// standalone stays byte-identical (no hydration files, anywhere).
#[test]
fn hydration_layouts_are_integrated_only() {
    let tmp = tempfile::tempdir().unwrap();
    let guarded = model_with_guards(
        vec!["admin".to_string()],
        Vec::new(),
        Some(policy_with(&[("Admin", &["manage"])])),
    );
    let unguarded = model_with_guards(Vec::new(), Vec::new(), None);

    let standalone = IfmlRouteGenerator::new(tmp.path(), "svelte");
    assert!(
        standalone.hydration(&guarded).is_none(),
        "standalone output must stay byte-identical — no hydration files"
    );

    let integrated =
        IfmlRouteGenerator::new(tmp.path(), "svelte").with_route_root(super::ROUTE_GROUP);
    assert!(
        integrated.hydration(&unguarded).is_none(),
        "unguarded models read no role surface — nothing to hydrate"
    );

    let files = integrated
        .hydration(&guarded)
        .expect("guarded integrated model hydrates");
    let paths: Vec<String> = files
        .iter()
        .map(|f| f.path.to_string_lossy().into_owned())
        .collect();
    assert!(
        paths
            .iter()
            .any(|p| p.ends_with("src/routes/(views)/+layout.server.ts"))
            && paths
                .iter()
                .any(|p| p.ends_with("src/routes/(views)/+layout.ts")),
        "{paths:?}"
    );

    let server = &files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("+layout.server.ts"))
        .expect("server layout")
        .content;
    // Session → BaseJump → policy tokens, with the convention in-file.
    assert!(server.contains("get_accounts"), "{server}");
    assert!(server.contains("account_role === 'owner'"), "{server}");
    assert!(
        server.contains("const OWNER_ROLE_TOKENS: string[] = ['Admin', 'admin'];"),
        "{server}"
    );
    assert!(
        server.contains("const MEMBER_ROLE_TOKENS: string[] = ['Admin', 'admin'];"),
        "fallback maps member to the LAST policy actor (Admin is the only one): {server}"
    );
    // NO redirect: persona contexts carry no session and must reach the
    // guarded pages; an unauthenticated visitor keeps `roles: null`.
    assert!(!server.contains("redirect("), "{server}");
    assert!(server.contains("roles: null"), "{server}");

    let universal = &files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("/+layout.ts"))
        .expect("universal layout")
        .content;
    assert!(
        universal.contains("(globalThis as any).__USER_ROLES__ = data.roles;"),
        "{universal}"
    );
    // Persona safety: null roles must never clobber init-script seeding.
    assert!(
        universal.contains("if (typeof window !== 'undefined' && data.roles) {"),
        "{universal}"
    );
}

#[test]
fn integrated_guard_decides_server_side_via_parent_roles() {
    // Issue #468 W-a: in the integrated (views) route group the guard must
    // fire during SSR (the (views)/+layout.server.ts load resolves the
    // session's role tokens) — a denied persona is redirected before any
    // data fetch, and never sees the 404/401 boundary from a doomed fetch.
    let tera = create_tera(Path::new(".")).expect("tera");
    let vc = guarded_vc(
        "AdminConsole",
        vec!["admin".to_string()],
        vec!["manage_refunds".to_string()],
    );
    let ctx = build_load_context("v1", &vc, &[], "/", true);
    let rendered = render_template(&tera, "ifml/svelte/page_load.tera", &ctx).expect("render");
    assert!(
        rendered.contains("async ({ params, url, fetch, parent }) => {"),
        "integrated loads take parent(): {rendered}"
    );
    assert!(
        rendered.contains(
            "const parentData = (await parent()) as { roles?: string[] | null } | undefined;"
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains("const roles: string[] = parentData?.roles ?? currentRoles();"),
        "{rendered}"
    );
    assert!(
        rendered.contains("if (viewRequires.length && !viewRequires.some((c) => can(c))) {"),
        "{rendered}"
    );
    assert!(
        rendered.contains("if (viewRoles.length && !roles.some((r) => viewRoles.includes(r))) {"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("browser"),
        "the SSR guard must not be browser-gated: {rendered}"
    );
    assert!(
        !rendered.contains("import { browser } from '$app/environment';"),
        "{rendered}"
    );
}
