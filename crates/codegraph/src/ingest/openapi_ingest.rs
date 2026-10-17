use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use codegraph_core::traits::GraphIngestor;
use codegraph_core::types::{
    ApiOperationNode, ApiResourceNode, EdgeType, HttpEndpointNode, InteractionNode,
};
use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Default)]
pub struct OpenApiIngestStats {
    pub resources: usize,
    pub operations: usize,
    pub interactions: usize,
    pub endpoints: usize,
}

impl fmt::Display for OpenApiIngestStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "OpenAPI: {} resources, {} operations, {} interactions, {} endpoints",
            self.resources, self.operations, self.interactions, self.endpoints
        )
    }
}

/// Ingest an OpenAPI 3.0/3.1 spec file (JSON) into the graph.
#[cfg_attr(feature = "hotpath", hotpath::measure)]
pub async fn ingest_openapi_file(
    db: &dyn GraphIngestor,
    path: &Path,
) -> Result<OpenApiIngestStats> {
    let raw = std::fs::read_to_string(path)?;
    let doc: serde_json::Value = serde_json::from_str(&raw)?;
    let stats = ingest_openapi_spec(db, &doc).await?;
    tracing::info!(file = %path.display(), "OpenAPI ingestion complete: {stats}");
    Ok(stats)
}

/// Ingest an OpenAPI 3.0/3.1 document (parsed as JSON) into the graph.
pub async fn ingest_openapi_spec(
    db: &dyn GraphIngestor,
    doc: &serde_json::Value,
) -> Result<OpenApiIngestStats> {
    let spec: OpenApiDoc =
        serde_json::from_value(doc.clone()).map_err(|e| Error::Config(format!("{e}")))?;
    let mut stats = OpenApiIngestStats::default();

    match spec.openapi.as_deref() {
        Some(v) if v.starts_with("3.") => {}
        Some(v) => {
            tracing::warn!(version = %v, "OpenAPI version is not 3.0/3.1; attempting best-effort parse");
        }
        None => {
            tracing::warn!(
                "OpenAPI document has no 'openapi' version field; attempting best-effort parse"
            );
        }
    }

    let mut seen_operations: HashMap<String, String> = HashMap::new();

    // One ApiResource per unique resource name (paths like /v1/customers and
    // /v1/customers/{customerId} share the "customers" resource). The
    // path_segment is the shortest path for that resource (deterministic).
    let mut resource_titles: HashMap<String, (String, String)> = HashMap::new();
    for (path, path_item) in &spec.paths {
        let resource_name = resource_name_from_path(path);
        let schema_title = path_item
            .success_schema_title()
            .unwrap_or_else(|| "".to_string());
        resource_titles
            .entry(resource_name)
            .and_modify(|(title, seg)| {
                if path.len() < seg.len() {
                    *seg = path.clone();
                }
                if title.is_empty() && !schema_title.is_empty() {
                    *title = schema_title.clone();
                }
            })
            .or_insert_with(|| (schema_title, path.clone()));
    }

    for (resource_name, (schema_title, path_segment)) in &resource_titles {
        let resource_id = db
            .ingest_api_resource(&ApiResourceNode {
                name: resource_name.clone(),
                schema_title: schema_title.clone(),
                domain: "external".to_string(),
                label: Some(resource_name.clone()),
                path_segment: path_segment.clone(),
            })
            .await
            .map_err(Error::Graph)?;
        stats.resources += 1;

        if !schema_title.is_empty() {
            db.ingest_edge(&resource_id, schema_title, EdgeType::BindsToSchema, None)
                .await
                .map_err(Error::Graph)?;
        }

        for (path, path_item) in &spec.paths {
            if resource_name_from_path(path) != *resource_name {
                continue;
            }
            for (method, operation) in path_item.operations() {
                let op_name = operation
                    .operation_id
                    .clone()
                    .unwrap_or_else(|| format!("{}_{}", method, resource_name));
                if let Some(prev_path) = seen_operations.get(&op_name) {
                    tracing::warn!(
                        operation = %op_name,
                        first_seen = %prev_path,
                        current_path = %path,
                        "duplicate OpenAPI operationId skipped"
                    );
                    continue;
                }
                seen_operations.insert(op_name.clone(), path.clone());

                let kind = operation_kind(method, path);
                let output_schema = operation
                    .success_schema_title()
                    .unwrap_or_else(|| schema_title.clone());
                let input_schema = operation.request_body_schema_title();

                let op_id = db
                    .ingest_api_operation(&ApiOperationNode {
                        name: op_name.clone(),
                        kind: kind.to_string(),
                        input_schema,
                        output_schema,
                        paging: kind == OperationKind::List,
                        sorting: kind == OperationKind::List,
                        filtering: kind == OperationKind::List,
                        domain: Some("external".to_string()),
                    })
                    .await
                    .map_err(Error::Graph)?;
                stats.operations += 1;

                db.ingest_edge(&resource_id, &op_id, EdgeType::HasOperation, None)
                    .await
                    .map_err(Error::Graph)?;

                let interaction_id = db
                    .ingest_interaction(&InteractionNode {
                        transport: "http".to_string(),
                        domain: Some("external".to_string()),
                    })
                    .await
                    .map_err(Error::Graph)?;
                stats.interactions += 1;

                db.ingest_edge(&op_id, &interaction_id, EdgeType::HasInteraction, None)
                    .await
                    .map_err(Error::Graph)?;

                let endpoint_id = db
                    .ingest_http_endpoint(&HttpEndpointNode {
                        method: method.to_uppercase(),
                        path_template: path.clone(),
                        domain: Some("external".to_string()),
                    })
                    .await
                    .map_err(Error::Graph)?;
                stats.endpoints += 1;

                db.ingest_edge(
                    &interaction_id,
                    &endpoint_id,
                    EdgeType::BindsHttpEndpoint,
                    None,
                )
                .await
                .map_err(Error::Graph)?;
            }
        }
    }

    Ok(stats)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationKind {
    List,
    Read,
    Create,
    Update,
    Delete,
}

impl fmt::Display for OperationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            OperationKind::List => "list",
            OperationKind::Read => "read",
            OperationKind::Create => "create",
            OperationKind::Update => "update",
            OperationKind::Delete => "delete",
        };
        write!(f, "{s}")
    }
}

fn operation_kind(method: &str, path: &str) -> OperationKind {
    let has_id = path_segments(path).any(|s| s.starts_with('{'));
    match method {
        "get" => {
            if has_id {
                OperationKind::Read
            } else {
                OperationKind::List
            }
        }
        "post" => OperationKind::Create,
        "put" => OperationKind::Update,
        "patch" => OperationKind::Update,
        "delete" => OperationKind::Delete,
        _ => OperationKind::List,
    }
}

fn path_segments(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|s| !s.is_empty())
}

/// Resource name: the last path segment, trimming placeholder segments
/// (`{customerId}`) and trailing slashes.
fn resource_name_from_path(path: &str) -> String {
    path_segments(path)
        .filter(|s| !s.starts_with('{'))
        .last()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "root".to_string())
}

/// Resolve a schema `$ref` to a bare title: `#/components/schemas/X` → `X`.
/// Non-ref schemas and non-component refs resolve to `None` (no recursion).
fn ref_schema_title(value: &serde_json::Value) -> Option<String> {
    let reference = value.get("$ref")?.as_str()?;
    let last = reference.rsplit('/').next()?;
    if last.is_empty() {
        None
    } else {
        Some(last.to_string())
    }
}

fn response_schema_title(responses: &HashMap<String, Response>) -> Option<String> {
    let mut success_keys: Vec<&String> = responses.keys().filter(|k| k.starts_with('2')).collect();
    success_keys.sort();
    for key in success_keys {
        let response = responses.get(key)?;
        if let Some(title) = response.schema_title() {
            return Some(title);
        }
    }
    None
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Default)]
struct OpenApiDoc {
    #[serde(default)]
    openapi: Option<String>,
    #[serde(default)]
    paths: HashMap<String, PathItem>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Default)]
struct PathItem {
    #[serde(default)]
    get: Option<Operation>,
    #[serde(default)]
    post: Option<Operation>,
    #[serde(default)]
    put: Option<Operation>,
    #[serde(default)]
    patch: Option<Operation>,
    #[serde(default)]
    delete: Option<Operation>,
}

impl PathItem {
    fn operations(&self) -> Vec<(&str, &Operation)> {
        let mut ops = Vec::new();
        if let Some(op) = &self.get {
            ops.push(("get", op));
        }
        if let Some(op) = &self.post {
            ops.push(("post", op));
        }
        if let Some(op) = &self.put {
            ops.push(("put", op));
        }
        if let Some(op) = &self.patch {
            ops.push(("patch", op));
        }
        if let Some(op) = &self.delete {
            ops.push(("delete", op));
        }
        ops
    }

    fn success_schema_title(&self) -> Option<String> {
        for (_, op) in self.operations() {
            if let Some(title) = op.success_schema_title() {
                return Some(title);
            }
        }
        None
    }
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Default)]
struct Operation {
    #[serde(default, rename = "operationId")]
    operation_id: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    parameters: Vec<serde_json::Value>,
    #[serde(default, rename = "requestBody")]
    request_body: Option<serde_json::Value>,
    #[serde(default)]
    responses: HashMap<String, Response>,
}

impl Operation {
    fn success_schema_title(&self) -> Option<String> {
        response_schema_title(&self.responses)
    }

    fn request_body_schema_title(&self) -> Option<String> {
        let body = self.request_body.as_ref()?;
        let content = body.get("content")?;
        let (_, media) = content.as_object()?.iter().next()?;
        let schema = media.get("schema")?;
        ref_schema_title(schema)
    }
}

#[derive(Debug, Deserialize, Default)]
struct Response {
    #[serde(default)]
    content: Option<HashMap<String, MediaType>>,
}

impl Response {
    fn schema_title(&self) -> Option<String> {
        let content = self.content.as_ref()?;
        for media in content.values() {
            if let Some(schema) = &media.schema
                && let Some(title) = ref_schema_title(schema)
            {
                return Some(title);
            }
        }
        None
    }
}

#[derive(Debug, Deserialize, Default)]
struct MediaType {
    #[serde(default)]
    schema: Option<serde_json::Value>,
}
