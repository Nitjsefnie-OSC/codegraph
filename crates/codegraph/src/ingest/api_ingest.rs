use std::fmt;

use codegraph_config::config::DomainConfig;
use codegraph_core::traits::GraphIngestor;
use codegraph_core::types::{
    ApiOperationNode, ApiResourceNode, EdgeType, ErrorDefinitionNode, HttpEndpointNode,
    InteractionNode, PermissionNode, PipelineNode,
};

use crate::error::{Error, Result};

#[derive(Debug, Default)]
pub struct ApiModelIngestStats {
    pub resources: usize,
    pub operations: usize,
    pub permissions: usize,
    pub interactions: usize,
    pub endpoints: usize,
    pub errors: usize,
}

impl fmt::Display for ApiModelIngestStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "API model: {} resources, {} operations, {} permissions, {} interactions, {} endpoints",
            self.resources, self.operations, self.permissions, self.interactions, self.endpoints
        )
    }
}

/// Ingest the API model into the graph from the domain configuration.
///
/// For every entity in every domain:
/// 1. Creates an ApiResource node (with path_segment from EntityConfig or auto-derived)
/// 2. Creates an ApiOperation node for each operation (create/read/update/delete/list)
/// 3. Creates an Interaction node linking the operation to HTTP transport
/// 4. Creates an HttpEndpoint node with method + path template
/// 5. Creates edges: BindsToSchema, HasOperation, OutputBoundTo, HasInteraction, BindsHttpEndpoint
#[cfg_attr(feature = "hotpath", hotpath::measure)]
pub async fn ingest_api_model(
    db: &dyn GraphIngestor,
    config: &DomainConfig,
) -> Result<ApiModelIngestStats> {
    let mut stats = ApiModelIngestStats::default();

    let default_pipeline_id = db
        .ingest_pipeline(&PipelineNode {
            name: "default".to_string(),
            middleware: Some(vec!["auth".to_string(), "metrics".to_string()]),
            domain: Some("common".to_string()),
        })
        .await
        .map_err(Error::Graph)?;

    let public_pipeline_id = db
        .ingest_pipeline(&PipelineNode {
            name: "public".to_string(),
            middleware: Some(vec!["metrics".to_string()]),
            domain: Some("common".to_string()),
        })
        .await
        .map_err(Error::Graph)?;

    let _admin_pipeline_id = db
        .ingest_pipeline(&PipelineNode {
            name: "admin".to_string(),
            middleware: Some(vec![
                "auth".to_string(),
                "permission".to_string(),
                "metrics".to_string(),
            ]),
            domain: Some("common".to_string()),
        })
        .await
        .map_err(Error::Graph)?;

    for (domain_name, domain_entry) in &config.domains {
        for entity_name in &domain_entry.entities {
            let ec = domain_entry.get_entity_config(entity_name);

            let path_segment = ec
                .and_then(|c| c.path_segment.as_deref())
                .unwrap_or(entity_name)
                .to_string();

            let operations = ec
                .and_then(|c| c.operations.as_ref())
                .unwrap_or(&config.defaults.operations);

            let schema_title = entity_name.clone();
            let resource_name =
                crate::generate::api::api_model::normalized_resource_name(entity_name);

            let resource_id = db
                .ingest_api_resource(&ApiResourceNode {
                    name: resource_name.to_string(),
                    schema_title: schema_title.clone(),
                    domain: domain_name.to_string(),
                    label: ec
                        .and_then(|c| c.tag.clone())
                        .or_else(|| Some(entity_name.to_string())),
                    path_segment: path_segment.clone(),
                })
                .await
                .map_err(Error::Graph)?;
            stats.resources += 1;

            db.ingest_edge(&resource_id, &schema_title, EdgeType::BindsToSchema, None)
                .await
                .map_err(Error::Graph)?;

            let op_mappings: Vec<(&str, &str, &str)> = operations
                .iter()
                .filter_map(|op| match op.as_str() {
                    "list" => Some(("list", "GET", "")),
                    "create" => Some(("create", "POST", "")),
                    "read" => Some(("read", "GET", "/{id}")),
                    "update" => Some(("update", "PUT", "/{id}")),
                    "delete" => Some(("delete", "DELETE", "/{id}")),
                    _ => None,
                })
                .collect();

            let base_path = format!("/api/v1/{}/{}", domain_name, path_segment);

            for (op_kind, method, path_suffix) in &op_mappings {
                let op_name = format!("{}_{}", op_kind, resource_name);
                let op_id = db
                    .ingest_api_operation(&ApiOperationNode {
                        name: op_name.clone(),
                        kind: op_kind.to_string(),
                        input_schema: None,
                        output_schema: schema_title.clone(),
                        paging: *op_kind == "list",
                        sorting: *op_kind == "list",
                        filtering: *op_kind == "list",
                        domain: Some(domain_name.to_string()),
                    })
                    .await
                    .map_err(Error::Graph)?;
                stats.operations += 1;

                db.ingest_edge(&resource_id, &op_id, EdgeType::HasOperation, None)
                    .await
                    .map_err(Error::Graph)?;

                db.ingest_edge(&op_id, &schema_title, EdgeType::OutputBoundTo, None)
                    .await
                    .map_err(Error::Graph)?;

                let public_ops: &[String] = ec
                    .and_then(|c| c.public_operations.as_deref())
                    .unwrap_or(&[]);
                if !public_ops.contains(&op_kind.to_string()) {
                    let perm_name = format!(
                        "{}:{}:{}",
                        domain_name,
                        resource_name.to_lowercase(),
                        op_kind
                    );
                    let perm_id = db
                        .ingest_permission(&PermissionNode {
                            name: perm_name.clone(),
                            domain: Some(domain_name.clone()),
                        })
                        .await
                        .map_err(Error::Graph)?;
                    stats.permissions += 1;

                    db.ingest_edge(&op_id, &perm_id, EdgeType::RequiresPermission, None)
                        .await
                        .map_err(Error::Graph)?;
                }

                let interaction_id = db
                    .ingest_interaction(&InteractionNode {
                        transport: "http".to_string(),
                        domain: Some(domain_name.to_string()),
                    })
                    .await
                    .map_err(Error::Graph)?;
                stats.interactions += 1;

                db.ingest_edge(&op_id, &interaction_id, EdgeType::HasInteraction, None)
                    .await
                    .map_err(Error::Graph)?;

                let path_template = format!("{}{}", base_path, path_suffix);
                let endpoint_id = db
                    .ingest_http_endpoint(&HttpEndpointNode {
                        method: method.to_string(),
                        path_template,
                        domain: Some(domain_name.to_string()),
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

                let pipeline_id = if public_ops.contains(&op_kind.to_string()) {
                    &public_pipeline_id
                } else {
                    &default_pipeline_id
                };
                db.ingest_edge(&endpoint_id, pipeline_id, EdgeType::UsesPipeline, None)
                    .await
                    .map_err(Error::Graph)?;
            }

            // Add search as a first-class operation when the entity has a dedicated
            // /search REST surface.
            let has_fts = ec
                .and_then(|c| c.search.fts_columns.as_ref())
                .map(|cols| !cols.is_empty())
                .unwrap_or(false);
            let fts_rest_mode = ec
                .map(|c| c.search.fts_rest_mode.clone())
                .unwrap_or_else(|| "query_param".to_string());
            if has_fts && (fts_rest_mode == "dedicated" || fts_rest_mode == "both") {
                let op_name = format!("search_{}", resource_name);
                let op_id = db
                    .ingest_api_operation(&ApiOperationNode {
                        name: op_name.clone(),
                        kind: "search".to_string(),
                        input_schema: None,
                        output_schema: schema_title.clone(),
                        paging: true,
                        sorting: false,
                        filtering: false,
                        domain: Some(domain_name.to_string()),
                    })
                    .await
                    .map_err(Error::Graph)?;
                stats.operations += 1;

                db.ingest_edge(&resource_id, &op_id, EdgeType::HasOperation, None)
                    .await
                    .map_err(Error::Graph)?;

                db.ingest_edge(&op_id, &schema_title, EdgeType::OutputBoundTo, None)
                    .await
                    .map_err(Error::Graph)?;

                // Interaction + HttpEndpoint for the search operation
                let interaction_id = db
                    .ingest_interaction(&InteractionNode {
                        transport: "http".to_string(),
                        domain: Some(domain_name.to_string()),
                    })
                    .await
                    .map_err(Error::Graph)?;
                stats.interactions += 1;

                db.ingest_edge(&op_id, &interaction_id, EdgeType::HasInteraction, None)
                    .await
                    .map_err(Error::Graph)?;

                let path_template = format!("{}/search", base_path);
                let endpoint_id = db
                    .ingest_http_endpoint(&HttpEndpointNode {
                        method: "GET".to_string(),
                        path_template,
                        domain: Some(domain_name.to_string()),
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

    const STANDARD_ERRORS: &[(&str, &str, i32)] = &[
        ("NOT_FOUND", "The requested resource was not found", 404),
        ("VALIDATION_ERROR", "Request validation failed", 422),
        ("UNAUTHORIZED", "Authentication required", 401),
        ("FORBIDDEN", "Insufficient permissions", 403),
        ("CONFLICT", "Resource conflict", 409),
    ];

    for (domain_name, _domain_entry) in &config.domains {
        for (code, description, http_status) in STANDARD_ERRORS {
            let node = ErrorDefinitionNode {
                code: code.to_string(),
                description: description.to_string(),
                http_status: *http_status,
                domain: Some(domain_name.clone()),
            };
            if let Err(e) = db.ingest_error_definition(&node).await {
                tracing::warn!(domain = %domain_name, error_code = %code, "failed to ingest standard error definition: {e}");
            }
            stats.errors += 1;
        }

        for entity_name in &_domain_entry.entities {
            if let Some(ec) = _domain_entry.get_entity_config(entity_name) {
                for (code, def) in &ec.errors {
                    let node = ErrorDefinitionNode {
                        code: code.clone(),
                        description: def.description.clone(),
                        http_status: def.http_status,
                        domain: Some(domain_name.clone()),
                    };
                    if let Err(e) = db.ingest_error_definition(&node).await {
                        tracing::warn!(domain = %domain_name, entity = %entity_name, error_code = %code, "failed to ingest custom error definition: {e}");
                    }
                    stats.errors += 1;
                }
            }
        }
    }

    Ok(stats)
}
