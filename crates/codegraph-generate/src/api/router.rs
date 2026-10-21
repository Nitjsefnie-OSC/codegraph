use crate::ProjectConfig;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::{Access, ParentCandidate};
use serde::Serialize;

use crate::error::Result;
use crate::render_template_with_project;
use crate::traits::{DomainGenerator, DomainGeneratorKind, GeneratedFile};
use codegraph_config::DomainConfig;

use super::api_model::{resolve_entity_operations, resolve_path_segment};

/// Route-auth skip decision (issue #279): an entity's routes mount WITHOUT
/// the permission layers only when the `public_operations_rls` feature is
/// ON, the schema carries `access = Public`, and the entity's
/// `public_operations` config covers its ENTIRE effective operation set —
/// a partially-public entity keeps auth on the remainder. Flag off, no
/// access flag, or `Private` all keep today's behavior (byte-identity).
pub fn route_auth_is_public(
    public_operations_rls: bool,
    access: Option<Access>,
    config_public_operations: Option<&[String]>,
    effective_operations: &[String],
) -> bool {
    if !public_operations_rls || access != Some(Access::Public) {
        return false;
    }
    let Some(public_ops) = config_public_operations else {
        return false;
    };
    effective_operations
        .iter()
        .all(|op| public_ops.contains(op))
}

#[derive(Debug, Clone, Serialize)]
pub struct ParentInfo {
    pub entity_name: String,
    pub module_name: String,
    pub path_segment: String,
    pub fk_column: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChildInfo {
    pub entity_name: String,
    pub module_name: String,
    pub path_segment: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CrossRefInfo {
    pub entity_name: String,
    pub module_name: String,
    pub domain: String,
    pub path_segment: String,
    pub fk_column: String,
    pub link_rel: String,
}

#[derive(Debug, Serialize)]
pub struct RouterContext {
    pub domain: String,
    pub entities: Vec<RouterEntity>,
    pub has_permission_middleware: bool,
    /// True when this entity-less domain delegates its router to a hand-written
    /// `handwritten_routes.rs` module (`custom_routes = true` in domains.toml).
    /// The rendered router then calls `handwritten_routes::router()` (sibling
    /// module declared by mod.rs) instead of returning a bare `Router::new()`.
    pub has_custom_routes: bool,
}

#[derive(Debug, Serialize)]
pub struct RouterEntity {
    /// Mox op action routes (issue #508 R2) — POST `{id}/op/{snake}`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mox_ops: Vec<crate::api::handler::MoxOpRoute>,
    pub entity_name: String,
    pub module_name: String,
    pub path_segment: String,
    pub has_create: bool,
    pub has_update: bool,
    pub has_delete: bool,
    /// Whether the read (`GET /{id}`) route has a handler to point at —
    /// an empty design-mapped operation set (value designs, no-repository
    /// entities) emits no read handler, so the route must be gated too.
    pub has_read: bool,
    /// Whether the list (`GET /`) route has a handler to point at.
    pub has_list: bool,
    pub has_workflow: bool,
    pub has_approval_status: bool,
    pub has_embeddings: bool,
    /// Whether this entity has full-text search enabled.
    pub has_fts: bool,
    /// REST surface for full-text search: "query_param", "dedicated", or "both".
    pub fts_rest_mode: String,
    pub role: String,
    /// Named path parameter for this entity's ID (e.g. `"worker_id"`).
    pub param_name: String,
    pub parent: Option<ParentInfo>,
    pub children: Vec<ChildInfo>,
    pub cross_refs: Vec<CrossRefInfo>,
    pub media_fields: Vec<String>,
    /// When set, this entity supports the /tree endpoint (self-referencing hierarchy).
    pub hierarchy_field: Option<String>,
    /// Middleware names from the Pipeline that applies to this entity's endpoints.
    pub pipeline_middleware: Vec<String>,
    /// True when a non-empty pipeline middleware list exists.
    pub has_pipeline_layer: bool,
    /// True when the entity has `permissions.scope` set (permission gating on).
    pub has_permissions: bool,
    /// Permission scope prefix, e.g. "support:support-plan". Ops are appended
    /// by the generated permission layer.
    pub permission_scope: String,
    /// Whether read-by-id / update / delete are record-scoped.
    pub permission_record_scoped: bool,
    /// Entity part of the API-key scope string (`{domain}.{entity}.read|write`)
    /// enforced by the generated route-level scope guard. Defaults to the
    /// entity's module name; overridable via `entity_config.<E>.api_key_scope`.
    pub api_key_scope: String,
}

pub struct RouterGenerator {
    output_dir: PathBuf,
    parent_candidates: Vec<ParentCandidate>,
}

/// Derive a named path parameter from a URL path segment.
///
/// Singularises the segment and appends `_id`:
/// - `"workers"` → `"worker_id"`
/// - `"military-service"` → `"military_service_id"`
/// - `"employment-permits"` → `"employment_permit_id"`
/// - `"education"` → `"education_id"` (already singular)
pub fn param_name_from_path_segment(segment: &str) -> String {
    let s = segment.replace('-', "_");
    let singular = if s.ends_with("ies") && s.len() > 3 {
        format!("{}y", &s[..s.len() - 3])
    } else if s.ends_with("sses") {
        // addresses → address (strip "es")
        s[..s.len() - 2].to_string()
    } else if s.ends_with("ses") && s.len() > 3 {
        // statuses → status, processes → process (strip "es")
        s[..s.len() - 2].to_string()
    } else if s.ends_with('s') && !s.ends_with("ss") {
        s[..s.len() - 1].to_string()
    } else {
        s
    };
    format!("{singular}_id")
}

/// Strip a configurable suffix from schema titles (e.g. "PersonType" → "Person").
pub fn strip_suffix<'a>(title: &'a str, suffix: &str) -> &'a str {
    title.strip_suffix(suffix).unwrap_or(title)
}

/// Strip the "Type" suffix from schema titles (e.g. "PersonType" → "Person").
///
/// Convenience wrapper — prefer `strip_suffix(title, &config.defaults.type_suffix)` in generators.
pub fn strip_type_suffix(title: &str) -> &str {
    strip_suffix(title, "Type")
}

/// Calculate the nesting depth of an entity given its raw schema title (with "Type" suffix).
pub(crate) fn calculate_nesting_depth(
    entity_name: &str,
    parent_candidates: &[ParentCandidate],
) -> usize {
    let parent_map: HashMap<&str, &str> = parent_candidates
        .iter()
        .map(|c| (c.child_title.as_str(), c.parent_title.as_str()))
        .collect();
    let mut depth = 1;
    let mut current = entity_name;
    let mut visited = std::collections::HashSet::new();
    visited.insert(current);
    while let Some(&parent) = parent_map.get(current) {
        if !visited.insert(parent) {
            break; // cycle detected — stop counting
        }
        depth += 1;
        current = parent;
    }
    depth
}

impl RouterGenerator {
    pub fn new(output_dir: &Path) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
            parent_candidates: Vec::new(),
        }
    }

    pub fn with_parent_candidates(mut self, candidates: Vec<ParentCandidate>) -> Self {
        self.parent_candidates = candidates;
        self
    }
}

#[async_trait]
impl DomainGenerator for RouterGenerator {
    fn kind(&self) -> DomainGeneratorKind {
        DomainGeneratorKind::Router
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        domain: &str,
        entity_titles: &[String],
        config: &DomainConfig,
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        // Entity-less `custom_routes` domains (e.g. `platform`) are served by
        // the hand-written extension crate (`cosmos_extensions::http::platform`)
        // in the monolith topology — the server nests `platform_routes` from
        // `scaffold/server.tera` instead of a generated `api/{domain}/router.rs`.
        // Emit no router file so `api/{domain}/` disappears from the tree; the
        // workers topology keeps the cornucopia `handwritten_routes.rs` pattern.
        let is_custom_routes_domain = config
            .domains
            .get(domain)
            .map(|d| d.custom_routes)
            .unwrap_or(false);
        if is_custom_routes_domain
            && project.deployment.deployment_topology
                == crate::profile::DeploymentTopology::Monolith
        {
            return Ok(Vec::new());
        }
        let mut entities = Vec::new();
        // Maps pg_table_name → entity index (for dedup)
        let mut module_to_idx: HashMap<String, usize> = HashMap::new();
        // Maps schema title (e.g. "LER-RSType") → entity index.
        // Used in the second pass to look up child/parent entities by title
        // rather than by strip_type_suffix(title), which breaks for hyphenated
        // titles like "LER-RSType" where rust_type_name == "LERRS" != "LER-RS".
        let mut title_to_entity_idx: HashMap<String, usize> = HashMap::new();

        for title in entity_titles {
            let entity_cfg = config
                .domains
                .get(domain)
                .and_then(|d| d.get_entity_config(title));
            let generation_mode = entity_cfg
                .and_then(|ec| ec.generation_mode.as_deref())
                .unwrap_or(&config.defaults.generation_mode);
            if generation_mode == "handler_only" || generation_mode == "ddd_only" {
                continue;
            }

            if let Ok(Some(schema)) = db.get_schema_in_domain(title, domain).await
                && !schema.pg_table_name.is_empty()
            {
                // Dedup by module name to prevent duplicate route functions
                if let Some(&existing_idx) = module_to_idx.get(&schema.pg_table_name) {
                    title_to_entity_idx.insert(title.clone(), existing_idx);
                    continue;
                }
                let entity_name = &schema.rust_type_name;
                let operations = resolve_entity_operations(db, config, domain, entity_name).await;

                let workflow = entity_cfg.and_then(|ec| ec.workflow.as_ref());
                let has_workflow = workflow
                    .map(|wf| wf.generate_action_endpoints)
                    .unwrap_or(false);
                let has_approval_status = workflow
                    .and_then(|wf| wf.approval_status_field.as_ref())
                    .is_some();

                let has_embeddings = entity_cfg
                    .map(|ec| !ec.search.embedding_columns.is_empty())
                    .unwrap_or(false);
                let has_fts = entity_cfg
                    .and_then(|ec| ec.search.fts_columns.as_ref())
                    .map(|cols| !cols.is_empty())
                    .unwrap_or(false);
                let fts_rest_mode = entity_cfg
                    .map(|ec| ec.search.fts_rest_mode.clone())
                    .unwrap_or_else(|| "query_param".to_string());

                let media_fields: Vec<String> = db
                    .get_properties(title)
                    .await
                    .unwrap_or_default()
                    .iter()
                    .filter(|p| {
                        p.effective_kind()
                            == Some(codegraph_type_contracts::RefClassificationKind::MediaWrapper)
                    })
                    .map(|p| p.pg_column_name.clone())
                    .collect();

                let permissions = entity_cfg
                    .map(|ec| ec.permissions.clone())
                    .unwrap_or_default();
                let permission_scope = permissions.scope.clone().unwrap_or_default();
                let public_skip = route_auth_is_public(
                    project.integration.public_operations_rls,
                    schema.access,
                    entity_cfg.and_then(|ec| ec.public_operations.as_deref()),
                    &operations,
                );
                let has_permissions = !permission_scope.is_empty() && !public_skip;

                let entity_idx = entities.len();
                module_to_idx.insert(schema.pg_table_name.clone(), entity_idx);
                title_to_entity_idx.insert(title.clone(), entity_idx);

                entities.push(RouterEntity {
                    mox_ops: crate::api::handler::routable_mox_ops(db, title, entity_name).await,
                    entity_name: entity_name.clone(),
                    module_name: schema.pg_table_name.clone(),
                    path_segment: resolve_path_segment(entity_cfg, &schema),
                    has_create: operations.contains(&"create".to_string()),
                    has_update: operations.contains(&"update".to_string()),
                    has_delete: operations.contains(&"delete".to_string()),
                    has_read: operations.contains(&"read".to_string()),
                    has_list: operations.contains(&"list".to_string()),
                    has_workflow,
                    has_approval_status,
                    has_embeddings,
                    has_fts,
                    fts_rest_mode,
                    role: entity_cfg
                        .and_then(|ec| ec.role.clone())
                        .unwrap_or_else(|| "root".into()),
                    param_name: param_name_from_path_segment(&resolve_path_segment(
                        entity_cfg, &schema,
                    )),
                    parent: None,
                    children: vec![],
                    cross_refs: vec![],
                    media_fields,
                    hierarchy_field: entity_cfg.and_then(|ec| ec.hierarchy_field.clone()),
                    pipeline_middleware: Vec::new(),
                    has_pipeline_layer: false,
                    has_permissions,
                    permission_scope,
                    permission_record_scoped: permissions.record_scoped,
                    api_key_scope: entity_cfg
                        .and_then(|ec| ec.api_key_scope.clone())
                        .unwrap_or_else(|| schema.pg_table_name.clone()),
                });
            }
        }

        // Second pass: populate parent/child relationships.
        // Manual config takes priority over graph detection.
        // Second pass: populate parent/child relationships.
        // Manual config takes priority over graph detection.
        {
            // Source 1: manual config from domains.toml (role/parent/parent_ref)
            for title in entity_titles {
                if let Some(ec) = config
                    .domains
                    .get(domain)
                    .and_then(|d| d.get_entity_config(title))
                    && ec.role.as_deref() == Some("child")
                    && let Some(parent_title) = &ec.parent
                {
                    // Use title_to_entity_idx so hyphenated schema titles
                    // like "LER-RSType" (rust_type_name "LERRS") resolve
                    // correctly instead of failing via strip_type_suffix.
                    if let (Some(&ci), Some(&pi)) = (
                        title_to_entity_idx.get(title.as_str()),
                        title_to_entity_idx.get(parent_title.as_str()),
                    ) && entities[ci].parent.is_none()
                    {
                        let parent_name = strip_suffix(parent_title, &config.defaults.type_suffix);
                        let fk_column = ec.parent_ref.clone().unwrap_or_else(|| {
                            format!("{}_id", codegraph_naming::to_snake_case(parent_name))
                        });
                        let parent_module = entities[pi].module_name.clone();
                        let parent_path = entities[pi].path_segment.clone();
                        let parent_entity = entities[pi].entity_name.clone();

                        entities[ci].role = "child".to_string();
                        entities[ci].parent = Some(ParentInfo {
                            entity_name: parent_entity,
                            module_name: parent_module,
                            path_segment: parent_path,
                            fk_column,
                        });

                        let child_entity = entities[ci].entity_name.clone();
                        let child_module = entities[ci].module_name.clone();
                        let child_path = entities[ci].path_segment.clone();
                        entities[pi].children.push(ChildInfo {
                            entity_name: child_entity,
                            module_name: child_module,
                            path_segment: child_path,
                        });
                    }
                }
            }

            // Source 2: graph-detected parent_candidates (only for entities
            // not already assigned by manual config above)
            for pc in &self.parent_candidates {
                let child_idx = title_to_entity_idx.get(pc.child_title.as_str()).copied();
                let parent_idx = title_to_entity_idx.get(pc.parent_title.as_str()).copied();

                if let (Some(ci), Some(pi)) = (child_idx, parent_idx)
                    && entities[ci].parent.is_none()
                    && entities[ci].role != "root"
                {
                    let fk_column =
                        crate::fk_column_for_candidate(pc, &config.defaults.type_suffix);
                    let parent_module = entities[pi].module_name.clone();
                    let parent_path = entities[pi].path_segment.clone();
                    let parent_entity = entities[pi].entity_name.clone();

                    entities[ci].role = "child".to_string();
                    entities[ci].parent = Some(ParentInfo {
                        entity_name: parent_entity,
                        module_name: parent_module,
                        path_segment: parent_path,
                        fk_column,
                    });

                    let child_entity = entities[ci].entity_name.clone();
                    let child_module = entities[ci].module_name.clone();
                    let child_path = entities[ci].path_segment.clone();
                    entities[pi].children.push(ChildInfo {
                        entity_name: child_entity,
                        module_name: child_module,
                        path_segment: child_path,
                    });
                }
            }
        }

        // Warn on entities with nesting depth > 3 (long URLs).
        for entity in &entities {
            if entity.role == "child" {
                let title = format!("{}Type", entity.entity_name);
                let depth = calculate_nesting_depth(&title, &self.parent_candidates);
                if depth > 3 {
                    tracing::warn!(
                        "Entity '{}' nesting depth is {} (max 3). URL will be long: consider restructuring.",
                        entity.entity_name,
                        depth
                    );
                }
            }
        }

        // Third pass: detect cross-aggregate entity references.
        {
            let index: HashMap<String, usize> = entities
                .iter()
                .enumerate()
                .map(|(i, e)| (e.entity_name.clone(), i))
                .collect();

            let mut cross_refs_by_idx: HashMap<usize, Vec<CrossRefInfo>> = HashMap::new();

            for (i, entity) in entities.iter().enumerate() {
                let schema_title = format!("{}Type", entity.entity_name);
                let referenced = match db.get_referenced_schemas(&schema_title).await {
                    Ok(refs) => refs,
                    Err(_) => continue,
                };

                let parent_name = entity
                    .parent
                    .as_ref()
                    .map(|p| p.entity_name.as_str())
                    .unwrap_or("");
                let child_names: std::collections::HashSet<&str> = entity
                    .children
                    .iter()
                    .map(|c| c.entity_name.as_str())
                    .collect();

                for ref_schema_node in &referenced {
                    let ref_title = &ref_schema_node.title;
                    let ref_entity_name = strip_suffix(ref_title, &config.defaults.type_suffix);

                    // Skip self
                    if ref_entity_name == entity.entity_name {
                        continue;
                    }
                    // Skip parent
                    if ref_entity_name == parent_name {
                        continue;
                    }
                    // Skip children
                    if child_names.contains(ref_entity_name) {
                        continue;
                    }
                    // Only link to entities present in this domain's entity list
                    let Some(&ref_idx) = index.get(ref_entity_name) else {
                        continue;
                    };

                    let ref_entity = &entities[ref_idx];
                    let fk_col = codegraph_naming::to_snake_case(ref_entity_name) + "_id";
                    let link_rel = codegraph_naming::to_snake_case(ref_entity_name);
                    cross_refs_by_idx.entry(i).or_default().push(CrossRefInfo {
                        entity_name: ref_entity.entity_name.clone(),
                        module_name: ref_entity.module_name.clone(),
                        domain: domain.to_string(),
                        path_segment: ref_entity.path_segment.clone(),
                        fk_column: fk_col,
                        link_rel,
                    });
                }
            }

            for (i, refs) in cross_refs_by_idx {
                entities[i].cross_refs = refs;
            }
        }

        let perms = db.get_permissions().await.unwrap_or_default();
        let has_permission_middleware = !perms.is_empty();

        let _pipelines = db.get_pipelines().await.unwrap_or_default();
        let endpoints = db.get_http_endpoints().await.unwrap_or_default();

        for entity in &mut entities {
            let base_path = format!("/api/v1/{}/{}", domain, entity.path_segment);
            if let Some(endpoint) = endpoints
                .iter()
                .find(|ep| ep.path_template.starts_with(&base_path))
                && let Ok(Some(pipeline)) =
                    db.get_pipeline_for_endpoint(&endpoint.path_template).await
            {
                entity.pipeline_middleware = pipeline.middleware.unwrap_or_default();
                entity.has_pipeline_layer = !entity.pipeline_middleware.is_empty();
            }
        }

        let ctx = RouterContext {
            domain: domain.to_string(),
            entities,
            has_permission_middleware,
            has_custom_routes: config
                .domains
                .get(domain)
                .map(|d| d.custom_routes)
                .unwrap_or(false),
        };

        let content = render_template_with_project(tera, "api/router.tera", &ctx, project)?;
        Ok(vec![GeneratedFile {
            path: self
                .output_dir
                .join("src")
                .join("api")
                .join(domain)
                .join("router.rs"),
            content,
        }])
    }
}

/// Build the [`RouterContext`] for a domain — the single source of truth for
/// which entities the domain's REST router mounts and at what URL path
/// segments.
///
/// Shared by the `router` generator (renders `src/api/{domain}/router.rs`)
/// and the `api_contract` generator (renders the machine-readable plugin API
/// contract under `api-contracts/{domain}.{ts,json}`) so the two can never
/// drift: both consume the exact same resolved entity + path_segment set.
pub async fn build_router_context(
    db: &dyn GraphQuerier,
    domain: &str,
    entity_titles: &[String],
    config: &DomainConfig,
    parent_candidates: &[ParentCandidate],
    project: &ProjectConfig,
) -> Result<RouterContext> {
    let mut entities = Vec::new();
    // Maps pg_table_name → entity index (for dedup)
    let mut module_to_idx: HashMap<String, usize> = HashMap::new();
    // Maps schema title (e.g. "LER-RSType") → entity index.
    // Used in the second pass to look up child/parent entities by title
    // rather than by strip_type_suffix(title), which breaks for hyphenated
    // titles like "LER-RSType" where rust_type_name == "LERRS" != "LER-RS".
    let mut title_to_entity_idx: HashMap<String, usize> = HashMap::new();

    for title in entity_titles {
        let entity_cfg = config
            .domains
            .get(domain)
            .and_then(|d| d.get_entity_config(title));
        let generation_mode = entity_cfg
            .and_then(|ec| ec.generation_mode.as_deref())
            .unwrap_or(&config.defaults.generation_mode);
        if generation_mode == "handler_only" || generation_mode == "ddd_only" {
            continue;
        }

        if let Ok(Some(schema)) = db.get_schema_in_domain(title, domain).await
            && !schema.pg_table_name.is_empty()
        {
            // Dedup by module name to prevent duplicate route functions
            if let Some(&existing_idx) = module_to_idx.get(&schema.pg_table_name) {
                title_to_entity_idx.insert(title.clone(), existing_idx);
                continue;
            }
            let entity_name = &schema.rust_type_name;
            let operations = resolve_entity_operations(db, config, domain, entity_name).await;

            let workflow = entity_cfg.and_then(|ec| ec.workflow.as_ref());
            let has_workflow = workflow
                .map(|wf| wf.generate_action_endpoints)
                .unwrap_or(false);
            let has_approval_status = workflow
                .and_then(|wf| wf.approval_status_field.as_ref())
                .is_some();

            let has_embeddings = entity_cfg
                .map(|ec| !ec.search.embedding_columns.is_empty())
                .unwrap_or(false);
            let fts_rest_mode = entity_cfg
                .map(|ec| ec.search.fts_rest_mode.clone())
                .unwrap_or_else(|| "query_param".to_string());

            let search = entity_cfg.map(|ec| &ec.search);
            let has_fts = search
                .and_then(|s| s.fts_columns.as_ref())
                .map(|cols| !cols.is_empty())
                .unwrap_or(false);

            let media_fields: Vec<String> = db
                .get_properties(title)
                .await
                .unwrap_or_default()
                .iter()
                .filter(|p| {
                    p.effective_kind()
                        == Some(codegraph_type_contracts::RefClassificationKind::MediaWrapper)
                })
                .map(|p| p.pg_column_name.clone())
                .collect();

            let permissions = entity_cfg
                .map(|ec| ec.permissions.clone())
                .unwrap_or_default();
            let permission_scope = permissions.scope.clone().unwrap_or_default();
            let public_skip = route_auth_is_public(
                project.integration.public_operations_rls,
                schema.access,
                entity_cfg.and_then(|ec| ec.public_operations.as_deref()),
                &operations,
            );
            let has_permissions = !permission_scope.is_empty() && !public_skip;

            let entity_idx = entities.len();
            module_to_idx.insert(schema.pg_table_name.clone(), entity_idx);
            title_to_entity_idx.insert(title.clone(), entity_idx);

            entities.push(RouterEntity {
                mox_ops: crate::api::handler::routable_mox_ops(db, title, entity_name).await,
                entity_name: entity_name.clone(),
                module_name: schema.pg_table_name.clone(),
                path_segment: resolve_path_segment(entity_cfg, &schema),
                has_create: operations.contains(&"create".to_string()),
                has_update: operations.contains(&"update".to_string()),
                has_delete: operations.contains(&"delete".to_string()),
                has_read: operations.contains(&"read".to_string()),
                has_list: operations.contains(&"list".to_string()),
                has_workflow,
                has_approval_status,
                has_embeddings,
                has_fts,
                fts_rest_mode,
                role: entity_cfg
                    .and_then(|ec| ec.role.clone())
                    .unwrap_or_else(|| "root".into()),
                param_name: param_name_from_path_segment(&resolve_path_segment(
                    entity_cfg, &schema,
                )),
                parent: None,
                children: vec![],
                cross_refs: vec![],
                media_fields,
                hierarchy_field: entity_cfg.and_then(|ec| ec.hierarchy_field.clone()),
                pipeline_middleware: Vec::new(),
                has_pipeline_layer: false,
                has_permissions,
                permission_scope,
                permission_record_scoped: permissions.record_scoped,
                api_key_scope: entity_cfg
                    .and_then(|ec| ec.api_key_scope.clone())
                    .unwrap_or_else(|| schema.pg_table_name.clone()),
            });
        }
    }

    // Second pass: populate parent/child relationships.
    // Manual config takes priority over graph detection.
    {
        // Source 1: manual config from domains.toml (role/parent/parent_ref)
        for title in entity_titles {
            if let Some(ec) = config
                .domains
                .get(domain)
                .and_then(|d| d.get_entity_config(title))
                && ec.role.as_deref() == Some("child")
                && let Some(parent_title) = &ec.parent
            {
                // Use title_to_entity_idx so hyphenated schema titles
                // like "LER-RSType" (rust_type_name "LERRS") resolve
                // correctly instead of failing via strip_type_suffix.
                if let (Some(&ci), Some(&pi)) = (
                    title_to_entity_idx.get(title.as_str()),
                    title_to_entity_idx.get(parent_title.as_str()),
                ) && entities[ci].parent.is_none()
                {
                    let parent_name = strip_suffix(parent_title, &config.defaults.type_suffix);
                    let fk_column = ec.parent_ref.clone().unwrap_or_else(|| {
                        format!("{}_id", codegraph_naming::to_snake_case(parent_name))
                    });
                    let parent_module = entities[pi].module_name.clone();
                    let parent_path = entities[pi].path_segment.clone();
                    let parent_entity = entities[pi].entity_name.clone();

                    entities[ci].role = "child".to_string();
                    entities[ci].parent = Some(ParentInfo {
                        entity_name: parent_entity,
                        module_name: parent_module,
                        path_segment: parent_path,
                        fk_column,
                    });

                    let child_entity = entities[ci].entity_name.clone();
                    let child_module = entities[ci].module_name.clone();
                    let child_path = entities[ci].path_segment.clone();
                    entities[pi].children.push(ChildInfo {
                        entity_name: child_entity,
                        module_name: child_module,
                        path_segment: child_path,
                    });
                }
            }
        }

        // Source 2: graph-detected parent_candidates (only for entities
        // not already assigned by manual config above)
        for pc in parent_candidates {
            let child_idx = title_to_entity_idx.get(pc.child_title.as_str()).copied();
            let parent_idx = title_to_entity_idx.get(pc.parent_title.as_str()).copied();

            if let (Some(ci), Some(pi)) = (child_idx, parent_idx)
                && entities[ci].parent.is_none()
                && entities[ci].role != "root"
            {
                let fk_column = crate::fk_column_for_candidate(pc, &config.defaults.type_suffix);
                let parent_module = entities[pi].module_name.clone();
                let parent_path = entities[pi].path_segment.clone();
                let parent_entity = entities[pi].entity_name.clone();

                entities[ci].role = "child".to_string();
                entities[ci].parent = Some(ParentInfo {
                    entity_name: parent_entity,
                    module_name: parent_module,
                    path_segment: parent_path,
                    fk_column,
                });

                let child_entity = entities[ci].entity_name.clone();
                let child_module = entities[ci].module_name.clone();
                let child_path = entities[ci].path_segment.clone();
                entities[pi].children.push(ChildInfo {
                    entity_name: child_entity,
                    module_name: child_module,
                    path_segment: child_path,
                });
            }
        }
    }

    // Warn on entities with nesting depth > 3 (long URLs).
    for entity in &entities {
        if entity.role == "child" {
            let title = format!("{}Type", entity.entity_name);
            let depth = calculate_nesting_depth(&title, parent_candidates);
            if depth > 3 {
                tracing::warn!(
                    "Entity '{}' nesting depth is {} (max 3). URL will be long: consider restructuring.",
                    entity.entity_name,
                    depth
                );
            }
        }
    }

    // Third pass: detect cross-aggregate entity references.
    {
        let index: HashMap<String, usize> = entities
            .iter()
            .enumerate()
            .map(|(i, e)| (e.entity_name.clone(), i))
            .collect();

        let mut cross_refs_by_idx: HashMap<usize, Vec<CrossRefInfo>> = HashMap::new();

        for (i, entity) in entities.iter().enumerate() {
            let schema_title = format!("{}Type", entity.entity_name);
            let referenced = match db.get_referenced_schemas(&schema_title).await {
                Ok(refs) => refs,
                Err(_) => continue,
            };

            let parent_name = entity
                .parent
                .as_ref()
                .map(|p| p.entity_name.as_str())
                .unwrap_or("");
            let child_names: std::collections::HashSet<&str> = entity
                .children
                .iter()
                .map(|c| c.entity_name.as_str())
                .collect();

            for ref_schema_node in &referenced {
                let ref_title = &ref_schema_node.title;
                let ref_entity_name = strip_suffix(ref_title, &config.defaults.type_suffix);

                // Skip self
                if ref_entity_name == entity.entity_name {
                    continue;
                }
                // Skip parent
                if ref_entity_name == parent_name {
                    continue;
                }
                // Skip children
                if child_names.contains(ref_entity_name) {
                    continue;
                }
                // Only link to entities present in this domain's entity list
                let Some(&ref_idx) = index.get(ref_entity_name) else {
                    continue;
                };

                let ref_entity = &entities[ref_idx];
                let fk_col = codegraph_naming::to_snake_case(ref_entity_name) + "_id";
                let link_rel = codegraph_naming::to_snake_case(ref_entity_name);
                cross_refs_by_idx.entry(i).or_default().push(CrossRefInfo {
                    entity_name: ref_entity.entity_name.clone(),
                    module_name: ref_entity.module_name.clone(),
                    domain: domain.to_string(),
                    path_segment: ref_entity.path_segment.clone(),
                    fk_column: fk_col,
                    link_rel,
                });
            }
        }

        for (i, refs) in cross_refs_by_idx {
            entities[i].cross_refs = refs;
        }
    }

    let perms = db.get_permissions().await.unwrap_or_default();
    let has_permission_middleware = !perms.is_empty();

    let _pipelines = db.get_pipelines().await.unwrap_or_default();
    let endpoints = db.get_http_endpoints().await.unwrap_or_default();

    for entity in &mut entities {
        let base_path = format!("/api/v1/{}/{}", domain, entity.path_segment);
        if let Some(endpoint) = endpoints
            .iter()
            .find(|ep| ep.path_template.starts_with(&base_path))
            && let Ok(Some(pipeline)) = db.get_pipeline_for_endpoint(&endpoint.path_template).await
        {
            entity.pipeline_middleware = pipeline.middleware.unwrap_or_default();
            entity.has_pipeline_layer = !entity.pipeline_middleware.is_empty();
        }
    }

    Ok(RouterContext {
        domain: domain.to_string(),
        entities,
        has_permission_middleware,
        has_custom_routes: config
            .domains
            .get(domain)
            .map(|d| d.custom_routes)
            .unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codegraph_core::types::{DetectionSource, ParentCandidate};

    #[test]
    fn test_nesting_depth_calculation() {
        let candidates = vec![
            ParentCandidate {
                child_title: "BType".into(),
                parent_title: "AType".into(),
                field_name: "a_id".into(),
                source: DetectionSource::ScalarRef,
            },
            ParentCandidate {
                child_title: "CType".into(),
                parent_title: "BType".into(),
                field_name: "b_id".into(),
                source: DetectionSource::ScalarRef,
            },
            ParentCandidate {
                child_title: "DType".into(),
                parent_title: "CType".into(),
                field_name: "c_id".into(),
                source: DetectionSource::ScalarRef,
            },
        ];
        assert_eq!(calculate_nesting_depth("AType", &candidates), 1); // root
        assert_eq!(calculate_nesting_depth("BType", &candidates), 2); // child
        assert_eq!(calculate_nesting_depth("CType", &candidates), 3); // grandchild
        assert_eq!(calculate_nesting_depth("DType", &candidates), 4); // too deep
    }

    #[test]
    fn test_strip_suffix() {
        assert_eq!(strip_suffix("PersonType", "Type"), "Person");
        assert_eq!(strip_suffix("CompensationType", "Type"), "Compensation");
        assert_eq!(strip_suffix("Worker", "Type"), "Worker");
        assert_eq!(strip_suffix("Type", "Type"), "");
        assert_eq!(strip_suffix("", "Type"), "");
        assert_eq!(strip_suffix("PersonView", "View"), "Person");
        assert_eq!(strip_suffix("CustomEntitySuf", "Suf"), "CustomEntity");
    }

    #[allow(deprecated)]
    #[test]
    fn test_strip_type_suffix_backward_compat() {
        assert_eq!(strip_type_suffix("PersonType"), "Person");
        assert_eq!(strip_type_suffix("CompensationType"), "Compensation");
        assert_eq!(strip_type_suffix("Worker"), "Worker");
        assert_eq!(strip_type_suffix("Type"), "");
        assert_eq!(strip_type_suffix(""), "");
    }

    #[test]
    fn test_nesting_depth_root_entity() {
        let candidates = vec![];
        assert_eq!(calculate_nesting_depth("PersonType", &candidates), 1);
    }

    #[test]
    fn test_nesting_depth_no_cycle_on_unrelated() {
        let candidates = vec![ParentCandidate {
            child_title: "BType".into(),
            parent_title: "AType".into(),
            field_name: "a_id".into(),
            source: DetectionSource::ScalarRef,
        }];
        // Unrelated entity not in candidates
        assert_eq!(calculate_nesting_depth("XType", &candidates), 1);
    }

    #[test]
    fn test_parent_info_fields() {
        let info = ParentInfo {
            entity_name: "Compensation".into(),
            module_name: "compensation".into(),
            path_segment: "compensation".into(),
            fk_column: "compensation_type_id".into(),
        };
        assert_eq!(info.fk_column, "compensation_type_id");
    }

    #[test]
    fn test_child_info_fields() {
        let info = ChildInfo {
            entity_name: "Reward".into(),
            module_name: "reward".into(),
            path_segment: "reward".into(),
        };
        assert_eq!(info.path_segment, "reward");
    }

    #[test]
    fn test_cross_ref_info_fields() {
        let info = CrossRefInfo {
            entity_name: "Person".into(),
            domain: "common".into(),
            path_segment: "person".into(),
            fk_column: "person_id".into(),
            link_rel: "person".into(),
            module_name: "person_type".into(),
        };
        assert_eq!(info.link_rel, "person");
        assert_eq!(info.domain, "common");
    }

    #[test]
    fn test_nesting_depth_terminates_on_cycle() {
        let candidates = vec![
            ParentCandidate {
                child_title: "AType".into(),
                parent_title: "BType".into(),
                field_name: "b_id".into(),
                source: DetectionSource::ScalarRef,
            },
            ParentCandidate {
                child_title: "BType".into(),
                parent_title: "AType".into(),
                field_name: "a_id".into(),
                source: DetectionSource::ScalarRef,
            },
        ];
        // Must terminate (not infinite loop) and return a finite depth
        let depth = calculate_nesting_depth("AType", &candidates);
        assert!(
            depth <= 3,
            "Cycle should not produce depth > 3, got {depth}"
        );
    }

    #[test]
    fn param_name_simple_plural() {
        assert_eq!(param_name_from_path_segment("workers"), "worker_id");
    }

    #[test]
    fn param_name_kebab_case() {
        assert_eq!(
            param_name_from_path_segment("employment-permits"),
            "employment_permit_id"
        );
    }

    #[test]
    fn param_name_ies_suffix() {
        assert_eq!(param_name_from_path_segment("companies"), "company_id");
        assert_eq!(param_name_from_path_segment("policies"), "policy_id");
    }

    #[test]
    fn param_name_statuses() {
        assert_eq!(param_name_from_path_segment("statuses"), "status_id");
    }

    #[test]
    fn param_name_addresses() {
        assert_eq!(param_name_from_path_segment("addresses"), "address_id");
    }

    #[test]
    fn param_name_processes() {
        assert_eq!(param_name_from_path_segment("processes"), "process_id");
    }

    #[test]
    fn param_name_singular_passthrough() {
        assert_eq!(param_name_from_path_segment("person"), "person_id");
    }

    #[test]
    fn param_name_ss_suffix_kept() {
        assert_eq!(param_name_from_path_segment("access"), "access_id");
    }

    fn test_router_context() -> RouterContext {
        RouterContext {
            domain: "events".to_string(),
            entities: vec![RouterEntity {
                mox_ops: Vec::new(),
                entity_name: "PublicEvent".to_string(),
                module_name: "public_event".to_string(),
                path_segment: "public-event".to_string(),
                has_create: true,
                has_update: true,
                has_delete: true,
                has_read: true,
                has_list: true,
                has_workflow: false,
                has_approval_status: false,
                has_embeddings: false,
                has_fts: true,
                fts_rest_mode: "dedicated".to_string(),
                role: "root".to_string(),
                param_name: "public_event_id".to_string(),
                parent: None,
                children: vec![],
                cross_refs: vec![],
                media_fields: vec![],
                hierarchy_field: None,
                pipeline_middleware: vec![],
                has_pipeline_layer: false,
                has_permissions: true,
                permission_scope: "events:public-event".to_string(),
                permission_record_scoped: false,
                api_key_scope: "public_event".to_string(),
            }],
            has_permission_middleware: true,
            has_custom_routes: false,
        }
    }

    /// Every entity router must mount the generated API-key scope guard wired to
    /// API-key scope enforcement is DB-level (#169): the router must NOT emit
    /// any per-route scope middleware — the RESTRICTIVE scope_enforced_* RLS
    /// policies raise P0403 INSUFFICIENT_SCOPE in-DB and the generated error
    /// mapper maps that to HTTP 403.
    #[test]
    fn router_omits_scope_guard_enforced_in_db() {
        let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
        let tera = crate::template_engine::create_tera(&template_dir).unwrap();
        let project = ProjectConfig::default();

        let rendered = render_template_with_project(
            &tera,
            "api/router.tera",
            &test_router_context(),
            &project,
        )
        .unwrap();

        assert!(
            !rendered.contains("require_scope_for_request"),
            "router must not reference the retired scope guard"
        );
        assert!(
            !rendered.contains("scope_guard"),
            "router must not emit per-entity scope guard fns"
        );
        // Permission gating (JWT roles until phase 5 moves it into RLS) stays.
        assert!(
            rendered.contains("crate::middleware::permission::require_permission"),
            "permission middleware must survive"
        );
    }

    /// No scope middleware regardless of permission gating.
    #[test]
    fn router_renders_scope_guard_without_permissions() {
        let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
        let tera = crate::template_engine::create_tera(&template_dir).unwrap();
        let project = ProjectConfig::default();

        let mut ctx = test_router_context();
        ctx.entities[0].has_permissions = false;
        ctx.has_permission_middleware = false;

        let rendered =
            render_template_with_project(&tera, "api/router.tera", &ctx, &project).unwrap();

        assert!(!rendered.contains("public_event_scope_guard"));
        assert!(!rendered.contains("require_scope_for_request"));
        assert!(!rendered.contains("public_event_permission"));
    }
}
