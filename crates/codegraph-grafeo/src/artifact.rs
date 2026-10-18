//! L2 deterministic IR artifact document (issue #275).
//!
//! The document is a canonical, hashable JSON dump of the full graph:
//! every node (label, content-derived key, sorted properties) and every
//! edge (type, endpoint keys, sorted properties), with all collections in
//! a content-determined order. Export → import → export is byte-identical
//! and the sha256 of the canonical bytes is stable across runs on
//! identical inputs. The format is pinned by the conformance cases in
//! `kit` (`tests/fixtures/artifact_kit/graph_document_v1.md`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use codegraph_core::error::GraphError;
use grafeo_common::types::Value;
use grafeo_core::graph::Direction;
use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::engine::GrafeoEngine;
use crate::schema_ddl::GRAPH_FORMAT_VERSION as FORMAT_VERSION;

/// Errors from the deterministic artifact layer. Every failure mode is a
/// named variant so conformance cases and callers can match on it.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("artifact format version {found} is newer than supported version {supported}")]
    FormatVersionTooNew { found: u32, supported: u32 },
    #[error("artifact format version {found} is older than supported version {supported}")]
    FormatVersionTooOld { found: u32, supported: u32 },
    #[error("duplicate node identity {key}; export requires unique node content per label")]
    DuplicateNodeIdentity { key: String },
    #[error("duplicate edge {edge_type} {from} -> {to} with identical properties")]
    DuplicateEdgeIdentity {
        edge_type: String,
        from: String,
        to: String,
    },
    #[error("edge references unknown node key {key}")]
    UnknownEdgeEndpoint { key: String },
    #[error("unsupported property value of type {kind} on {owner}")]
    UnsupportedValue { owner: String, kind: String },
    #[error("node {key} carries multiple labels; the artifact format is single-label")]
    MultiLabelNode { key: String },
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("graph error: {0}")]
    Graph(#[from] codegraph_core::error::GraphError),
}

impl ArtifactError {
    /// Stable diagnostic name for conformance-case matching.
    pub fn diagnostic(&self) -> &'static str {
        match self {
            ArtifactError::FormatVersionTooNew { .. } => "format_version_too_new",
            ArtifactError::FormatVersionTooOld { .. } => "format_version_too_old",
            ArtifactError::DuplicateNodeIdentity { .. } => "duplicate_node_identity",
            ArtifactError::DuplicateEdgeIdentity { .. } => "duplicate_edge_identity",
            ArtifactError::UnknownEdgeEndpoint { .. } => "unknown_edge_endpoint",
            ArtifactError::UnsupportedValue { .. } => "unsupported_value",
            ArtifactError::MultiLabelNode { .. } => "multi_label_node",
            ArtifactError::Io(_) => "io",
            ArtifactError::Json(_) => "json",
            ArtifactError::Graph(_) => "graph",
        }
    }
}

/// The artifact document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphDocument {
    #[serde(rename = "formatVersion")]
    pub format_version: u32,
    pub nodes: Vec<NodeRecord>,
    pub edges: Vec<EdgeRecord>,
}

/// One node: label, content-derived key, sorted properties.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeRecord {
    pub label: String,
    pub key: String,
    pub properties: BTreeMap<String, PropValue>,
}

/// One edge: type, endpoint node keys, sorted properties.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeRecord {
    #[serde(rename = "type")]
    pub edge_type: String,
    pub from: String,
    pub to: String,
    pub properties: BTreeMap<String, PropValue>,
}

/// A canonicalizable property value. Graph properties persist as a flat
/// bag of scalars; lists and maps are supported for completeness. Null
/// properties are omitted by the exporter (they carry no information).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PropValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<PropValue>),
    Map(BTreeMap<String, PropValue>),
}

/// Summary of a successful [`export_ir`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedArtifact {
    pub sha256: String,
    pub node_count: usize,
    pub edge_count: usize,
}

/// Dump the graph at `engine` into a canonical document written to `path`.
pub fn export_ir(engine: &GrafeoEngine, path: &Path) -> Result<ExportedArtifact, ArtifactError> {
    let doc = build_document(engine)?;
    let bytes = canonical_bytes(&doc)?;
    std::fs::write(path, &bytes)?;
    Ok(ExportedArtifact {
        sha256: sha256_hex(&bytes),
        node_count: doc.nodes.len(),
        edge_count: doc.edges.len(),
    })
}

/// Read a document from `path` and rebuild the graph into a fresh engine
/// (schema DDL runs first; IF NOT EXISTS keeps it conflict-free).
pub fn import_ir(path: &Path) -> Result<GrafeoEngine, ArtifactError> {
    let bytes = std::fs::read(path)?;
    let doc = parse_document(&bytes)?;
    let engine = GrafeoEngine::in_memory()?;
    apply_document(&engine, &doc)?;
    Ok(engine)
}

/// Canonical serialization: serde_json of the document struct (field order
/// fixed by declaration, property maps sorted by `BTreeMap`).
pub fn canonical_bytes(doc: &GraphDocument) -> Result<Vec<u8>, ArtifactError> {
    Ok(serde_json::to_vec(doc)?)
}

/// Decode and validate a document: a `formatVersion` the running binary
/// cannot represent is rejected with a named error variant.
pub fn parse_document(bytes: &[u8]) -> Result<GraphDocument, ArtifactError> {
    let doc: GraphDocument = serde_json::from_slice(bytes)?;
    if doc.format_version > FORMAT_VERSION {
        return Err(ArtifactError::FormatVersionTooNew {
            found: doc.format_version,
            supported: FORMAT_VERSION,
        });
    }
    if doc.format_version < FORMAT_VERSION {
        return Err(ArtifactError::FormatVersionTooOld {
            found: doc.format_version,
            supported: FORMAT_VERSION,
        });
    }
    Ok(doc)
}

/// Order the document canonically: nodes by (label, key), edges by
/// (type, from, to, canonical properties). Idempotent on exported
/// documents; used by the conformance kit to normalize accepted variants.
pub fn normalize(doc: &mut GraphDocument) {
    doc.nodes
        .sort_by(|a, b| (&a.label, &a.key).cmp(&(&b.label, &b.key)));
    doc.edges.sort_by_cached_key(|e| {
        (
            e.edge_type.clone(),
            e.from.clone(),
            e.to.clone(),
            props_sort_key(e),
        )
    });
}

fn props_sort_key(edge: &EdgeRecord) -> String {
    serde_json::to_string(&edge.properties).unwrap_or_default()
}

fn build_document(engine: &GrafeoEngine) -> Result<GraphDocument, ArtifactError> {
    let store = engine.db().store();

    let mut nodes = Vec::new();
    let mut node_id_to_key: HashMap<grafeo::NodeId, String> = HashMap::new();
    let mut seen_keys: HashSet<String> = HashSet::new();
    for id in store.node_ids() {
        let node = store.get_node(id).ok_or_else(|| {
            ArtifactError::Graph(GraphError::Query(format!(
                "node {id:?} vanished during export"
            )))
        })?;
        let mut labels = node
            .labels
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>();
        labels.sort();
        if labels.len() != 1 {
            return Err(ArtifactError::MultiLabelNode {
                key: labels.join(":"),
            });
        }
        let label = labels.remove(0);
        let mut properties: BTreeMap<String, PropValue> = BTreeMap::new();
        for (key, value) in node.properties.to_btree_map() {
            let owner = format!("node {label}#{:?}", id);
            let prop = value_to_prop(&value, &owner)?;
            if !matches!(prop, PropValue::Null) {
                properties.insert(key.to_string(), prop);
            }
        }
        let canonical = serde_json::to_vec(&properties)?;
        let key = format!("{label}::{}", short_digest(&canonical));
        if !seen_keys.insert(key.clone()) {
            return Err(ArtifactError::DuplicateNodeIdentity { key });
        }
        node_id_to_key.insert(id, key.clone());
        nodes.push(NodeRecord {
            label,
            key,
            properties,
        });
    }
    nodes.sort_by(|a, b| (&a.label, &a.key).cmp(&(&b.label, &b.key)));

    let mut edges = Vec::new();
    let mut seen_edges: HashSet<String> = HashSet::new();
    for id in store.node_ids() {
        for (_dst, eid) in store.edges_from(id, Direction::Outgoing) {
            let edge = store.get_edge(eid).ok_or_else(|| {
                ArtifactError::Graph(GraphError::Query(format!(
                    "edge {eid:?} vanished during export"
                )))
            })?;
            let edge_type = edge.edge_type.to_string();
            let mut properties: BTreeMap<String, PropValue> = BTreeMap::new();
            for (key, value) in edge.properties.to_btree_map() {
                let owner = format!("edge {edge_type}#{:?}", eid);
                let prop = value_to_prop(&value, &owner)?;
                if !matches!(prop, PropValue::Null) {
                    properties.insert(key.to_string(), prop);
                }
            }
            let from = node_id_to_key.get(&edge.src).cloned().ok_or_else(|| {
                ArtifactError::UnknownEdgeEndpoint {
                    key: format!("{:?}", edge.src),
                }
            })?;
            let to = node_id_to_key.get(&edge.dst).cloned().ok_or_else(|| {
                ArtifactError::UnknownEdgeEndpoint {
                    key: format!("{:?}", edge.dst),
                }
            })?;
            let identity = format!(
                "{edge_type}\u{1}{from}\u{1}{to}\u{1}{}",
                serde_json::to_string(&properties)?
            );
            if !seen_edges.insert(identity) {
                return Err(ArtifactError::DuplicateEdgeIdentity {
                    edge_type,
                    from,
                    to,
                });
            }
            edges.push(EdgeRecord {
                edge_type,
                from,
                to,
                properties,
            });
        }
    }
    edges.sort_by_cached_key(|e| {
        (
            e.edge_type.clone(),
            e.from.clone(),
            e.to.clone(),
            props_sort_key(e),
        )
    });

    Ok(GraphDocument {
        format_version: FORMAT_VERSION,
        nodes,
        edges,
    })
}

fn apply_document(engine: &GrafeoEngine, doc: &GraphDocument) -> Result<(), ArtifactError> {
    let session = engine.db().session();
    let mut key_to_id: HashMap<&str, grafeo::NodeId> = HashMap::with_capacity(doc.nodes.len());
    for node in &doc.nodes {
        let props: Vec<(&str, Value)> = node
            .properties
            .iter()
            .map(|(key, value)| (key.as_str(), prop_to_value(value)))
            .collect();
        let id = session
            .create_node_with_props(&[node.label.as_str()], props)
            .map_err(|e| ingest_error(format!("create node {}", node.key), e))?;
        key_to_id.insert(node.key.as_str(), id);
    }
    for edge in &doc.edges {
        let from = key_to_id.get(edge.from.as_str()).ok_or_else(|| {
            ArtifactError::UnknownEdgeEndpoint {
                key: edge.from.clone(),
            }
        })?;
        let to =
            key_to_id
                .get(edge.to.as_str())
                .ok_or_else(|| ArtifactError::UnknownEdgeEndpoint {
                    key: edge.to.clone(),
                })?;
        let props: Vec<(&str, Value)> = edge
            .properties
            .iter()
            .map(|(key, value)| (key.as_str(), prop_to_value(value)))
            .collect();
        session
            .create_edge_with_props(*from, *to, &edge.edge_type, props)
            .map_err(|e| ingest_error(format!("create edge {} -> {}", edge.from, edge.to), e))?;
    }
    Ok(())
}

fn ingest_error(context: String, err: grafeo::Error) -> ArtifactError {
    ArtifactError::Graph(GraphError::Ingest(format!("{context}: {err}")))
}

fn value_to_prop(value: &Value, owner: &str) -> Result<PropValue, ArtifactError> {
    Ok(match value {
        Value::Null => PropValue::Null,
        Value::Bool(b) => PropValue::Bool(*b),
        Value::Int64(i) => PropValue::Int(*i),
        Value::Float64(f) => {
            if !f.is_finite() {
                return Err(ArtifactError::UnsupportedValue {
                    owner: owner.to_string(),
                    kind: "non-finite float".to_string(),
                });
            }
            PropValue::Float(*f)
        }
        Value::String(s) => PropValue::Str(s.to_string()),
        Value::List(items) => PropValue::List(
            items
                .iter()
                .map(|v| value_to_prop(v, owner))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Value::Map(map) => PropValue::Map(
            map.iter()
                .map(|(k, v)| value_to_prop(v, owner).map(|p| (k.to_string(), p)))
                .collect::<Result<BTreeMap<_, _>, _>>()?,
        ),
        other => {
            return Err(ArtifactError::UnsupportedValue {
                owner: owner.to_string(),
                kind: value_kind(other),
            });
        }
    })
}

fn value_kind(value: &Value) -> String {
    let debug = format!("{value:?}");
    debug
        .split(['(', ' ', '{'])
        .next()
        .unwrap_or("unknown")
        .to_string()
}

fn prop_to_value(prop: &PropValue) -> Value {
    match prop {
        PropValue::Null => Value::Null,
        PropValue::Bool(b) => Value::Bool(*b),
        PropValue::Int(i) => Value::Int64(*i),
        PropValue::Float(f) => Value::Float64(*f),
        PropValue::Str(s) => Value::String(s.as_str().into()),
        PropValue::List(items) => {
            Value::List(items.iter().map(prop_to_value).collect::<Vec<_>>().into())
        }
        PropValue::Map(entries) => Value::Map(std::sync::Arc::new(
            entries
                .iter()
                .map(|(k, v)| (k.as_str().into(), prop_to_value(v)))
                .collect(),
        )),
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn short_digest(bytes: &[u8]) -> String {
    sha256_hex(bytes)[..16].to_string()
}
