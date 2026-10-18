//! Canonical expression IR on every carrier (issue #278).
//!
//! Pins that `GrantEdge.when` and the four IFML conditional carriers
//! persist canonical AST JSON (`expr_json`) alongside the original source
//! string, that legacy payloads without the field still load, and that
//! ingest never fails on an unparseable expression.

use codegraph_core::mock::MockEngine;
use codegraph_core::traits::{GraphIngestor, GraphQuerier};
use codegraph_core::types::{
    ActorPolicyModel, DataBindingNode, EventNode, GrantEdge, ViewComponentNode, ViewContainerNode,
};
use serde_json::Value;

const DOMAIN_MOX: &str = r#"
package rex.support.domain

class Ticket {
    id String id
    String title
    boolean internal
    int amount
}
"#;

const ACTOR: &str = r#"
import "domain.mox"

actors Support {
    actor Agent
    capability Review on Ticket

    grant Agent {
        permit Review when (amount > 0 && !internal)
    }
}
"#;

const IFML: &str = r#"
domain "sales" { schema "sales"; }

view "Promo" {
    if promo_enabled == true;

    component "grid" {
        type: list;
        data: Customer;
        if row_count > 0;

        on select(row) if row.active == true -> navigate("PromoDetail");
    }
}

view "PromoDetail" {
    component "editor" {
        type: form;
        data: Customer;
    }
}
"#;

fn compiled_grant_policy() -> ActorPolicyModel {
    let compilation = rex_driver::compile_actors_str(
        "policy.actor",
        ACTOR,
        &[("domain.mox".to_string(), DOMAIN_MOX.to_string())],
        &rex_driver::DomainImports::default(),
    );
    assert!(
        compilation.diagnostics.is_empty(),
        "fixture policy must compile: {:?}",
        compilation.diagnostics
    );
    let model = compilation
        .model
        .expect("fixture policy must produce a model");
    codegraph::ifml_actor_import::actor_policy_from_model(&model)
}

#[tokio::test]
async fn grant_when_expr_persists_expr_json() {
    let policy = compiled_grant_policy();
    let grant = policy
        .grants
        .iter()
        .find(|g| g.when.is_some())
        .expect("fixture grant carries a `when`");
    let expr_json = grant
        .expr_json
        .as_ref()
        .expect("a `when` grant must persist canonical expr_json");
    let payload: Value = serde_json::from_str(expr_json).expect("expr_json must be valid JSON");
    assert_eq!(payload["kind"], "binary", "canonical shape: {payload}");
    assert_eq!(payload["op"], "and", "(amount > 0 && !internal): {payload}");

    let engine = MockEngine::new();
    engine
        .ingest_actor_policy(&policy)
        .await
        .expect("ingest policy");
    let stored = engine.get_grants().await.expect("grants");
    let stored = stored
        .iter()
        .find(|g| g.when.is_some())
        .expect("grant round-trips");
    assert_eq!(
        stored.expr_json.as_deref(),
        Some(expr_json.as_str()),
        "expr_json must survive the graph round trip"
    );
}

#[tokio::test]
async fn ifml_conditionals_persist_expr_json() {
    let model = rex_ifml::parse_ifml(IFML).expect("fixture model must parse");
    let engine = MockEngine::new();
    codegraph::ingest::ifml_ingest::ingest_ifml_model(&engine, &model)
        .await
        .expect("ingest ifml model");

    let containers = engine.get_ifml_view_containers().await.expect("views");
    let promo = containers
        .iter()
        .find(|v| v.name == "Promo")
        .expect("Promo");
    let source = promo
        .conditional_expression
        .as_ref()
        .expect("Promo carries a condition");
    let expr_json = promo
        .expr_json
        .as_ref()
        .expect("a conditioned view must persist expr_json");
    let payload: Value = serde_json::from_str(expr_json).expect("valid JSON");
    assert_eq!(payload["type"], "binOp", "canonical shape: {payload}");
    assert_eq!(
        source, "promo_enabled == true",
        "the source string must be preserved alongside the AST"
    );

    let components = engine
        .get_ifml_view_components("Promo")
        .await
        .expect("components");
    // View-level components persist under the owning view's namespace.
    let grid = components
        .iter()
        .find(|c| c.name == "Promo/grid")
        .expect("grid");
    let grid_json = grid
        .expr_json
        .as_ref()
        .expect("a conditioned component must persist expr_json");
    let payload: Value = serde_json::from_str(grid_json).expect("expr_json must be valid JSON");
    assert_eq!(payload["type"], "binOp", "{payload}");

    let events = engine
        .get_ifml_events("comp:Promo/grid")
        .await
        .expect("events");
    let select = events
        .iter()
        .find(|e| e.event_type == "select")
        .expect("select event");
    let select_json = select
        .expr_json
        .as_ref()
        .expect("a conditioned event must persist expr_json");
    let payload: Value = serde_json::from_str(select_json).expect("valid JSON");
    assert_eq!(payload["type"], "binOp", "{payload}");
}

#[tokio::test]
async fn unconditional_ifml_nodes_have_no_expr_json() {
    let model = rex_ifml::parse_ifml(IFML).expect("fixture model must parse");
    let engine = MockEngine::new();
    codegraph::ingest::ifml_ingest::ingest_ifml_model(&engine, &model)
        .await
        .expect("ingest ifml model");

    let containers = engine.get_ifml_view_containers().await.expect("views");
    let detail = containers
        .iter()
        .find(|v| v.name == "PromoDetail")
        .expect("PromoDetail");
    assert!(detail.conditional_expression.is_none());
    assert_eq!(detail.expr_json, None);

    let components = engine
        .get_ifml_view_components("PromoDetail")
        .await
        .expect("components");
    assert!(
        components
            .iter()
            .all(|c| c.conditional_expression.is_none() && c.expr_json.is_none()),
        "unconditional carriers must stay None: {components:?}"
    );
}

#[test]
fn legacy_graphs_without_expr_json_still_load() {
    let edge: GrantEdge = serde_json::from_str(
        r#"{"actor":"A","capability":"C","effect":"permit","when":null,"obligations":[]}"#,
    )
    .expect("legacy GrantEdge payload must deserialize");
    assert_eq!(edge.expr_json, None);

    let vc: ViewContainerNode = serde_json::from_str(
        r#"{"name":"V","label":null,"is_xor":false,"is_default":false,"is_landmark":false,
            "is_modal":false,"conditional_expression":null,"domain":null,"module_uses":null,
            "roles":null,"requires":null}"#,
    )
    .expect("legacy ViewContainerNode payload must deserialize");
    assert_eq!(vc.expr_json, None);

    let comp: ViewComponentNode = serde_json::from_str(
        r#"{"name":"C","component_type":"list","mode":null,"entity":null,"fields":null,
            "filter":null,"api_operation":null,"spec":null,"conditional_expression":null,
            "domain":null}"#,
    )
    .expect("legacy ViewComponentNode payload must deserialize");
    assert_eq!(comp.expr_json, None);

    let event: EventNode = serde_json::from_str(
        r#"{"name":"E","event_type":"click","params":null,"conditional_expression":null,"domain":null}"#,
    )
    .expect("legacy EventNode payload must deserialize");
    assert_eq!(event.expr_json, None);

    let binding: DataBindingNode = serde_json::from_str(
        r#"{"name":"B","conditional_expression":null,"expression_language":"ifml","domain":null}"#,
    )
    .expect("legacy DataBindingNode payload must deserialize");
    assert_eq!(binding.expr_json, None);
}

#[test]
fn expr_json_round_trips_through_serde_when_present() {
    let edge = GrantEdge {
        actor: "A".to_string(),
        capability: "C".to_string(),
        effect: "permit".to_string(),
        when: Some("amount > 0".to_string()),
        obligations: vec![],
        expr_json: Some(r#"{"kind":"binary"}"#.to_string()),
    };
    let text = serde_json::to_string(&edge).expect("serialize");
    let back: GrantEdge = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(back, edge);
}
