use std::collections::HashMap;

use codegraph_core::error::GraphError;
use codegraph_core::types::{
    EdgeProperties, EdgeType, RegulatoryEdgeKind, RegulatoryKind, RegulatoryOwner,
};

use codegraph_type_contracts::RefClassificationKind;

use crate::engine::GrafeoEngine;

/// Session wrapper that bumps `ingest_gql_calls` on every execution so the
/// ingest and read GQL volumes split at the wrapper boundary (issue #473).
/// Method signatures mirror `grafeo::Session`'s execute API one-for-one so
/// ingest call sites compile unchanged.
pub(super) struct CountingSession {
    inner: grafeo::session::Session,
}

impl CountingSession {
    pub(super) fn new(inner: grafeo::session::Session) -> Self {
        Self { inner }
    }

    pub(super) fn execute(&self, query: &str) -> grafeo::Result<grafeo::QueryResult> {
        crate::perf::bump_ingest_gql();
        self.inner.execute(query)
    }

    pub(super) fn execute_with_params(
        &self,
        query: &str,
        params: HashMap<String, grafeo::Value>,
    ) -> grafeo::Result<grafeo::QueryResult> {
        crate::perf::bump_ingest_gql();
        self.inner.execute_with_params(query, params)
    }
}

/// Escape a value for a GQL string literal: backslashes first (the GQL
/// parser processes backslash escapes, so a literal `\` must be doubled or
/// sequences like `\"` inside JSON payloads are silently corrupted on
/// store), then single quotes.
pub(crate) fn escape_gql(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// The GQL for one regulatory reference edge (issue #265). Owners match by
/// their natural keys (Schema title, Condition name, Regulatory name +
/// kind); targets are always Regulatory nodes matched by name + kind.
pub(super) fn regulatory_reference_gql(
    owner: &RegulatoryOwner,
    target: &str,
    target_kind: RegulatoryKind,
    edge_kind: RegulatoryEdgeKind,
    ref_path: Option<&str>,
) -> String {
    let owner_match = match owner {
        RegulatoryOwner::Schema(title) => format!("(a:Schema {{title: '{}'}})", escape_gql(title)),
        RegulatoryOwner::Condition(name) => {
            format!("(a:Condition {{name: '{}'}})", escape_gql(name))
        }
        RegulatoryOwner::Regulatory { name, kind } => format!(
            "(a:Regulatory {{name: '{}', kind: '{}'}})",
            escape_gql(name),
            kind.as_str()
        ),
        RegulatoryOwner::Function(name) => format!("(a:Function {{name: '{}'}})", escape_gql(name)),
        RegulatoryOwner::Rule(name) => format!("(a:Rule {{name: '{}'}})", escape_gql(name)),
    };
    let props_str = match ref_path {
        Some(path) => format!(" {{ref_path: '{}'}}", escape_gql(path)),
        None => String::new(),
    };
    format!(
        "MATCH {owner_match}, (b:Regulatory {{name: '{target}', kind: '{kind}'}}) \
         INSERT (a)-[:{edge}{props}]->(b)",
        target = escape_gql(target),
        kind = target_kind.as_str(),
        edge = edge_kind.as_str(),
        props = props_str,
    )
}

/// The regulatory edge kind for an EdgeType (issue #265).
pub(super) fn edge_kind_ref(edge_type: &EdgeType) -> RegulatoryEdgeKind {
    match edge_type {
        EdgeType::HasRuleSource => RegulatoryEdgeKind::RuleSource,
        EdgeType::CorpusInBody => RegulatoryEdgeKind::CorpusInBody,
        EdgeType::DerivesFrom => RegulatoryEdgeKind::DerivesFrom,
        _ => RegulatoryEdgeKind::Reference,
    }
}

/// Decode the `from_id`/`to_id` encoding `ingest_edge` accepts for the
/// regulatory reference edge types (`regowner:schema:<title>` /
/// `regowner:condition:<name>` / `regowner:regulatory:<kind>:<name>` →
/// `reg:<kind>:<name>`); `None` when malformed.
pub(super) fn decode_regulatory_edge_ids(
    from_id: &str,
    to_id: &str,
) -> Option<(RegulatoryOwner, String, RegulatoryKind)> {
    let rest = from_id.strip_prefix("regowner:")?;
    let (owner_str, owner_payload) = rest.split_once(':')?;
    let owner = match owner_str {
        "schema" => RegulatoryOwner::Schema(owner_payload.to_string()),
        "condition" => RegulatoryOwner::Condition(owner_payload.to_string()),
        "regulatory" => {
            let (kind_str, name) = owner_payload.split_once(':')?;
            RegulatoryOwner::Regulatory {
                name: name.to_string(),
                kind: RegulatoryKind::parse_kind(kind_str)?,
            }
        }
        "function" => RegulatoryOwner::Function(owner_payload.to_string()),
        "rule" => RegulatoryOwner::Rule(owner_payload.to_string()),
        _ => return None,
    };
    let target_rest = to_id.strip_prefix("reg:")?;
    let (kind_str, target) = target_rest.split_once(':')?;
    Some((
        owner,
        target.to_string(),
        RegulatoryKind::parse_kind(kind_str)?,
    ))
}

/// Strip a leading API-metamodel node prefix (`ar:` / `ao:` / `pl:` / `pm:` /
/// `ed:`) from an id, returning the id unchanged when no prefix is present.
/// Interaction/HttpEndpoint ids (`ia:` / `he:`) are deliberately not stripped —
/// those nodes store the full prefixed id as their `name`.
pub(super) fn strip_api_prefix(id: &str) -> &str {
    for prefix in ["ar:", "ao:", "pl:", "pm:", "ed:"] {
        if let Some(rest) = id.strip_prefix(prefix) {
            return rest;
        }
    }
    id
}

pub(super) fn classification_kind_to_str(kind: &RefClassificationKind) -> String {
    match kind {
        RefClassificationKind::PrimitiveWrapper => "primitive_wrapper",
        RefClassificationKind::ArrayWrapper => "array_wrapper",
        RefClassificationKind::RangeWrapper => "range_wrapper",
        RefClassificationKind::CodelistReference => "codelist",
        RefClassificationKind::CodelistCheck => "codelist_check",
        RefClassificationKind::InlineEnum => "inline_enum",
        RefClassificationKind::EntityReference => "entity_reference",
        RefClassificationKind::ValueObject => "value_object",
        RefClassificationKind::CompositeWrapper => "composite_wrapper",
        RefClassificationKind::StructuredWrapper => "structured_wrapper",
        RefClassificationKind::MediaWrapper => "media_wrapper",
    }
    .to_string()
}

pub(super) fn serde_enum_str<T: serde::Serialize>(v: &T) -> String {
    let json = serde_json::to_string(v).unwrap_or_default();
    json.trim_matches('"').to_string()
}

/// Format an Option<String> as a GQL value: either 'escaped' or null.
pub(super) fn opt_str(s: &Option<String>) -> String {
    match s {
        Some(v) => format!("'{}'", escape_gql(v)),
        None => "null".to_string(),
    }
}

pub(super) fn build_edge_props_string(props: Option<&EdgeProperties>) -> String {
    let Some(p) = props else {
        return String::new();
    };
    let mut fields = Vec::new();
    if let Some(v) = &p.sort_order {
        fields.push(format!("sort_order: {v}"));
    }
    if let Some(v) = &p.ref_path {
        fields.push(format!("ref_path: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.resolved_classification {
        fields.push(format!("resolved_classification: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.composition_type {
        fields.push(format!("composition_type: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.dependency_type {
        fields.push(format!("dependency_type: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.render_as {
        fields.push(format!("render_as: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.role {
        fields.push(format!("role: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.def_name {
        fields.push(format!("def_name: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.target_param_binding {
        fields.push(format!("target_param_binding: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.source_param {
        fields.push(format!("source_param: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.event_type {
        fields.push(format!("event_type: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.outcome {
        fields.push(format!("outcome: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.component_type {
        fields.push(format!("component_type: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.direction {
        fields.push(format!("direction: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.expression {
        fields.push(format!("expression: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.effect {
        fields.push(format!("effect: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.when_expr {
        fields.push(format!("when_expr: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.obligations {
        fields.push(format!("obligations: '{}'", escape_gql(v)));
    }
    if let Some(v) = &p.rule_source {
        fields.push(format!("rule_source: '{}'", escape_gql(v)));
    }
    if let Some(v) = p.import_wildcard {
        fields.push(format!("wildcard: {v}"));
    }
    if let Some(v) = &p.import_alias {
        fields.push(format!("alias: '{}'", escape_gql(v)));
    }
    if fields.is_empty() {
        String::new()
    } else {
        format!(" {{{}}}", fields.join(", "))
    }
}

/// Split a compound ID of the form `"part1::part2"`, returning an error
/// that names the edge label on failure.
pub(super) fn split_compound_id<'a>(
    id: &'a str,
    edge_label: &str,
) -> Result<(&'a str, &'a str), GraphError> {
    id.split_once("::").ok_or_else(|| {
        GraphError::Ingest(format!("{edge_label} id must be 'part1::part2', got: {id}"))
    })
}

/// Convert an Option<String> to a grafeo::Value for parameterized queries.
pub(super) fn opt_to_grafeo_value(s: &Option<String>) -> grafeo::Value {
    match s {
        Some(v) => grafeo::Value::String(v.clone().into()),
        None => grafeo::Value::Null,
    }
}

/// Convert a bool to the grafeo::Value representation Grafeo expects.
pub(super) fn bool_to_grafeo_value(b: bool) -> grafeo::Value {
    grafeo::Value::Bool(b)
}

pub(super) fn count_from_gql(engine: &GrafeoEngine, gql: &str) -> Result<usize, GraphError> {
    let session = CountingSession::new(engine.db().session());
    let result = session
        .execute(gql)
        .map_err(|e| GraphError::Query(e.to_string()))?;
    let rows = result.rows();
    if rows.is_empty() {
        return Ok(0);
    }
    rows[0][0]
        .as_int64()
        .map(|v| v as usize)
        .ok_or_else(|| GraphError::Query("count query did not return an integer".into()))
}
