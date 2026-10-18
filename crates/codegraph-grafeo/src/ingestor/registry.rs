//! Natural-key → NodeId registry (issue #482).
//!
//! Node ingest methods create nodes via the grafeo direct session API
//! (`create_node_with_props` — zero GQL, zero MATCH, no auto-commit
//! pipeline) and record the returned [`NodeId`] under EVERY natural-key
//! form the edge MATCH statements in `edges.rs` resolve that node by
//! (`schema_title:{title}`, `schema_id:{id}`, `property:{name}::{title}`,
//! …). `insert_edge` then looks both endpoints up: a HIT writes the edge
//! with `create_edge_with_props` (zero GQL); a MISS falls back to the
//! existing GQL statement verbatim — never an error, never a mis-link.
//!
//! Scope: the registry is populated only by THIS run's node ingests, so a
//! hit is always a same-run node. Edges to pre-existing nodes (doctor /
//! add-domain flows onto an existing persistent graph) take the GQL
//! fallback, which is exactly the pre-#482 behaviour.

use std::collections::HashMap;
use std::sync::Mutex;

use codegraph_core::error::GraphError;
use grafeo::NodeId;

use crate::engine::GrafeoEngine;

/// Natural-key registry: `"{kind}:{value}"` → NodeId of the node created
/// this run. Mutex only for future-proofing; ingest is single-threaded.
#[derive(Default)]
pub(crate) struct NodeRegistry {
    by_key: Mutex<HashMap<String, NodeId>>,
}

impl NodeRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Record `id` under every key form. Later registrations win — matches
    /// the "latest insert" resolution a fresh GQL MATCH would (ambiguously)
    /// return.
    pub(crate) fn register(&self, keys: &[String], id: NodeId) {
        let mut map = self.by_key.lock().unwrap_or_else(|e| e.into_inner());
        for key in keys {
            map.insert(key.clone(), id);
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<NodeId> {
        let map = self.by_key.lock().unwrap_or_else(|e| e.into_inner());
        map.get(key).copied()
    }
}

/// Registry key forms. Each constructor mirrors ONE endpoint MATCH form in
/// `ingestor/edges.rs` verbatim; the edge side builds its keys from the
/// same `from_id`/`to_id` encodings (or the same split compound parts) so
/// a key mismatch is impossible by construction — and even if an encoding
/// drifts, the lookup misses and the GQL fallback keeps the graph correct.
pub(super) mod keys {
    /// `MATCH (s:Schema {title: …})` — Schema by title.
    pub(crate) fn schema_title(title: &str) -> String {
        format!("schema_title:{title}")
    }

    /// `MATCH (s:Schema {schema_id: …})` — Schema by id (`ns::Name` or
    /// legacy `dir/Name`).
    pub(crate) fn schema_id(id: &str) -> String {
        format!("schema_id:{id}")
    }

    /// `MATCH (p:Property {name, _schema_title})` — the compound edge id is
    /// exactly `"{name}::{schema_title}"` (`split_compound_id` contract).
    pub(crate) fn property_compound(name: &str, schema_title: &str) -> String {
        format!("property:{name}::{schema_title}")
    }

    /// Property compound passed verbatim as an edge id.
    pub(crate) fn property(compound_id: &str) -> String {
        format!("property:{compound_id}")
    }

    /// `MATCH (c:CodeList {name: …})`.
    pub(crate) fn codelist(name: &str) -> String {
        format!("codelist:{name}")
    }

    /// `MATCH (v:EnumValue {value, _codelist_name})` — compound edge id is
    /// `"{value}::{codelist_name}"`.
    pub(crate) fn enum_value_compound(value: &str, codelist_name: &str) -> String {
        format!("enum_value:{value}::{codelist_name}")
    }

    /// EnumValue compound passed verbatim as an edge id.
    pub(crate) fn enum_value(compound_id: &str) -> String {
        format!("enum_value:{compound_id}")
    }

    /// `MATCH (r:CompositeRange {pg_column_name: …})`.
    pub(crate) fn composite_range(pg_column_name: &str) -> String {
        format!("composite_range:{pg_column_name}")
    }

    /// `MATCH (n:Namespace {fqn: …})`.
    pub(crate) fn namespace(fqn: &str) -> String {
        format!("namespace:{fqn}")
    }

    /// `MATCH (a:AtprotoNamespace {authority: …})`.
    pub(crate) fn atproto_namespace(authority: &str) -> String {
        format!("atproto_ns:{authority}")
    }

    /// `MATCH (l:Lexicon {nsid: …})`.
    pub(crate) fn lexicon(nsid: &str) -> String {
        format!("lexicon:{nsid}")
    }

    /// `MATCH (c:Collection {nsid: …})`.
    pub(crate) fn collection(nsid: &str) -> String {
        format!("collection:{nsid}")
    }

    /// `MATCH (r:Repository {did: …})`.
    pub(crate) fn repository(did: &str) -> String {
        format!("repository:{did}")
    }

    /// `MATCH (e:Event {name: …})`.
    pub(crate) fn event(name: &str) -> String {
        format!("event:{name}")
    }

    /// `MATCH (a:ActionNode {name: …})`.
    pub(crate) fn action(name: &str) -> String {
        format!("action:{name}")
    }

    /// `MATCH (m:ModuleDefinition {name: …})`.
    pub(crate) fn module(name: &str) -> String {
        format!("module:{name}")
    }

    /// `MATCH (d:DataBinding {name: …})`.
    pub(crate) fn data_binding(name: &str) -> String {
        format!("data_binding:{name}")
    }

    /// `MATCH (v:ViewComponent {name: …})`.
    pub(crate) fn view_component(name: &str) -> String {
        format!("view_component:{name}")
    }

    /// `MATCH (r:ApiResource {name: …})` — edges strip the `ar:` prefix.
    pub(crate) fn api_resource(name: &str) -> String {
        format!("api_resource:{name}")
    }

    /// `MATCH (o:ApiOperation {name: …})` — edges strip the `ao:` prefix.
    pub(crate) fn api_operation(name: &str) -> String {
        format!("api_operation:{name}")
    }

    /// `MATCH (i:Interaction {name: …})` — Interaction stores the FULL
    /// prefixed id as its name (no strip).
    pub(crate) fn interaction(full_id: &str) -> String {
        format!("interaction:{full_id}")
    }

    /// `MATCH (h:HttpEndpoint {name: …})` — full prefixed id (no strip).
    pub(crate) fn http_endpoint(full_id: &str) -> String {
        format!("http_endpoint:{full_id}")
    }

    /// `MATCH (p:Pipeline {name: …})` — edges strip the `pl:` prefix.
    pub(crate) fn pipeline(name: &str) -> String {
        format!("pipeline:{name}")
    }

    /// `MATCH (e:ErrorDefinition {code: …})`.
    pub(crate) fn error_definition(code: &str) -> String {
        format!("error_definition:{code}")
    }

    /// `MATCH (p:Permission {name: …})`.
    pub(crate) fn permission(name: &str) -> String {
        format!("permission:{name}")
    }

    /// `MATCH (a:Actor {name: …})`.
    pub(crate) fn actor(name: &str) -> String {
        format!("actor:{name}")
    }

    /// `MATCH (c:Capability {name: …})`.
    pub(crate) fn capability(name: &str) -> String {
        format!("capability:{name}")
    }

    /// `MATCH (f:Function {name: …})`.
    pub(crate) fn function(name: &str) -> String {
        format!("function:{name}")
    }

    /// `MATCH (r:Rule {name: …})`.
    pub(crate) fn rule(name: &str) -> String {
        format!("rule:{name}")
    }
}

impl GrafeoEngine {
    /// Create a node via the direct session API (zero GQL) and register it
    /// under `keys`. Property map is the same `HashMap<String, Value>` the
    /// parameterized GQL INSERT would have bound, so the stored property
    /// set is identical.
    pub(crate) fn create_registered_node(
        &self,
        label: &str,
        keys: &[String],
        props: &HashMap<String, grafeo::Value>,
    ) -> Result<NodeId, GraphError> {
        let session = self.db().session();
        let id = session
            .create_node_with_props(
                std::slice::from_ref(&label),
                props.iter().map(|(k, v)| (k.as_str(), v.clone())),
            )
            .map_err(|e| {
                GraphError::Ingest(format!("direct insert of {label} node failed: {e}"))
            })?;
        self.node_registry.register(keys, id);
        Ok(id)
    }

    /// Look one node up by registry key (no counter side effects).
    pub(crate) fn lookup_node(&self, key: &str) -> Option<NodeId> {
        self.node_registry.get(key)
    }

    /// Create an edge via the direct session API (zero GQL). `props` is the
    /// same property set the GQL INSERT would carry (see
    /// `edges::edge_props_pairs`).
    pub(crate) fn create_edge_direct(
        &self,
        src: NodeId,
        dst: NodeId,
        edge_type: &str,
        props: &[(&str, grafeo::Value)],
    ) -> Result<(), GraphError> {
        let session = self.db().session();
        session
            .create_edge_with_props(
                src,
                dst,
                edge_type,
                props.iter().map(|(k, v)| (*k, v.clone())),
            )
            .map_err(|e| {
                GraphError::Ingest(format!("direct insert of {edge_type} edge failed: {e}"))
            })?;
        Ok(())
    }
}
