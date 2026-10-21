use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::{ParentCandidate, PolicyKind};
use serde::Serialize;

use crate::ProjectConfig;
use crate::error::Result;
use crate::filter_fields::{
    FilterFieldInfo, NestedFilterFieldInfo, resolve_filter_fields, resolve_nested_filter_fields,
};
use crate::render_template_with_project;
use crate::traits::{EntityGenerator, EntityGeneratorKind, GeneratedFile};
use crate::type_registry;
use codegraph_config::{DomainConfig, EntityConfig};

use super::include_path::{ResolvedIncludePath, resolve_include_paths_for_topology};
use super::router::{ChildInfo, CrossRefInfo};

use super::api_model::{resolve_entity_operations, resolve_path_segment};

/// A mox operation routed as an action endpoint (issue #508 R2):
/// `POST {id}/op/{snake}`. Only qualifying ops route — bodied AND with
/// all-primitive signature slots; body-less ops never route.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MoxOpRoute {
    /// Operation name as written in mox.
    pub name: String,
    /// Rust method name (and route segment).
    pub snake: String,
    /// Parameters: (snake name, Rust type).
    pub params: Vec<MoxOpParam>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MoxOpParam {
    pub name: String,
    pub rust_type: String,
}

/// Qualifying mox ops for `schema_title` (issue #508 R2): a body
/// (`rust` or `expr`) and only primitive signature slots.
pub async fn routable_mox_ops(
    db: &dyn GraphQuerier,
    schema_title: &str,
    entity_name: &str,
) -> Vec<MoxOpRoute> {
    let all = db.get_mox_operations().await.unwrap_or_default();
    all.iter()
        .filter(|op| op.class == schema_title || op.class == entity_name)
        .filter(|op| op.bodies.contains_key("rust") || op.bodies.contains_key("expr"))
        .filter_map(|op| {
            let primitive = |t: &str| crate::mox_expr::mox_primitive_rust_type(t).is_some();
            if !primitive(&op.return_type) || !op.params.iter().all(|p| primitive(&p.type_)) {
                return None;
            }
            Some(MoxOpRoute {
                name: op.name.clone(),
                snake: codegraph_naming::to_snake_case(&op.name),
                params: op
                    .params
                    .iter()
                    .map(|p| MoxOpParam {
                        name: codegraph_naming::to_snake_case(&p.name),
                        rust_type: crate::mox_expr::mox_primitive_rust_type(&p.type_)
                            .unwrap_or("String")
                            .to_string(),
                    })
                    .collect(),
            })
        })
        .collect()
}

/// Mirror field names in emission order (issue #508 R2): the Response →
/// mirror copy list — mirrors the mox_types emission (`id` first when no
/// property carries it).
pub async fn mox_mirror_fields(db: &dyn GraphQuerier, schema_title: &str) -> Vec<String> {
    let props = db.get_properties(schema_title).await.unwrap_or_default();
    let mut fields: Vec<String> = props.iter().map(|p| p.rust_field_name.clone()).collect();
    if !fields.iter().any(|f| f == "id") {
        fields.insert(0, "id".to_string());
    }
    fields
}

#[derive(Debug, Serialize)]
pub struct HandlerContext {
    pub entity_name: String,
    pub module_name: String,
    pub domain: String,
    pub path_segment: String,
    pub tag: String,
    pub operations: Vec<String>,
    pub has_create: bool,
    pub has_read: bool,
    pub has_update: bool,
    pub has_delete: bool,
    pub has_list: bool,
    pub parent_ref: Option<String>,
    pub parent_entity: Option<String>,
    pub parent_path_segment: Option<String>,
    pub parent_module_name: Option<String>,
    /// tera 2's `default` filter only fires on undefined (not null), so None
    /// must OMIT the key for `parent_domain | default(value=domain)` routes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_domain: Option<String>,
    pub role: String,
    /// Named path parameter for this entity's ID (e.g. `"worker_id"`).
    pub param_name: String,
    /// Named path parameter for the parent's ID (e.g. `"worker_id"` when this entity is a child of Worker).
    pub parent_param_name: Option<String>,
    /// Named path parameter for the grandparent's ID (depth-2 children only).
    pub grandparent_param_name: Option<String>,
    /// Path segment for the grandparent entity (depth-2 children only, e.g. `"workers"`).
    pub grandparent_path_segment: Option<String>,
    /// Domain of the grandparent entity (depth-2 children only).
    /// Skipped when None: tera 2's `default` only fires on undefined keys.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_domain: Option<String>,
    pub children: Vec<ChildInfo>,
    pub cross_refs: Vec<CrossRefInfo>,
    /// When set, the list endpoint supports ?status= filtering on this column.
    pub status_field: Option<String>,
    /// Whether this entity has workflow support.
    pub has_workflow: bool,
    /// Whether this entity has full-text search enabled.
    pub has_fts: bool,
    /// REST surface for full-text search: "query_param", "dedicated", or "both".
    pub fts_rest_mode: String,
    /// Whether this entity has semantic search (pgvector embeddings) enabled.
    pub has_embeddings: bool,
    /// Fields exposed as JSON:API `?filter[field]=value` query params.
    pub filter_fields: Vec<FilterFieldInfo>,
    /// Nested (child/grandchild) fields exposed as `?filter[child.column]=value`.
    pub nested_filter_fields: Vec<NestedFilterFieldInfo>,
    /// Maximum number of items allowed in a bulk create request.
    pub max_bulk_size: usize,
    /// Number of ancestor path parameters captured before this handler's own routes.
    /// 0 for root entities, 1 for children, 2 for grandchildren, etc.
    pub ancestor_path_params: usize,
    /// When set, this entity supports the /tree endpoint (self-referencing hierarchy).
    pub hierarchy_field: Option<String>,
    /// When true, the find_tree method returns Vec<serde_json::Value> instead of Vec<Response>.
    #[serde(default)]
    pub tree_include: bool,
    /// When true, the get_by_id handler returns typed WithIncludeResponse instead of serde_json::Value.
    pub has_include: bool,
    /// Resolved include paths for `?include=` query parameter.
    pub include_paths: Vec<ResolvedIncludePath>,
    /// Resolved `use` import statements for types referenced by this handler.
    /// Populated via `type_registry::resolve_imports()` instead of hard-coded template paths.
    pub handler_imports: Vec<String>,
    /// When true, the entity has a soft-delete audit policy and the query layer
    /// expects an `include_deleted: bool` argument on all read methods.
    pub is_auditable: bool,
    /// Sortable-field allow-list for `?sort=` on the list endpoint
    /// (issue #306), derived from the ux plan. Empty = the ux sort plane
    /// is inactive: no sort params are emitted and the handler stays
    /// byte-identical.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ux_sort_fields: Vec<String>,
    /// Mox operations routed as action endpoints (issue #508 R2). Empty =
    /// no op routes and the handler stays byte-identical.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mox_ops: Vec<MoxOpRoute>,
    /// Response → mirror copy list for op handlers (issue #508 R2).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mox_mirror_fields: Vec<String>,
}

pub struct HandlerGenerator {
    output_dir: PathBuf,
    parent_candidates: Vec<ParentCandidate>,
}

impl HandlerGenerator {
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
impl EntityGenerator for HandlerGenerator {
    fn kind(&self) -> EntityGeneratorKind {
        EntityGeneratorKind::Handler
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        schema_title: &str,
        domain: &str,
        config: &DomainConfig,
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        let schema = db
            .get_schema_in_domain(schema_title, domain)
            .await?
            .ok_or_else(|| crate::error::Error::SchemaNotFound(schema_title.into()))?;

        let entity_name = schema.rust_type_name.clone();
        let module_name = schema.pg_table_name.clone();
        let domain = domain.to_string();

        if module_name.is_empty() {
            return Ok(Vec::new());
        }

        let entity_cfg = config
            .domains
            .get(&domain)
            .and_then(|d| d.get_entity_config(&entity_name));

        let path_segment = resolve_path_segment(entity_cfg, &schema);

        let operations = resolve_entity_operations(db, config, &domain, &entity_name).await;

        let tag = entity_cfg
            .and_then(|ec| ec.tag.clone())
            .unwrap_or_else(|| entity_name.clone());

        let parent_entity = entity_cfg.and_then(|ec| ec.parent.clone());

        let workflow = entity_cfg.and_then(|ec| ec.workflow.as_ref());
        let status_field = workflow.map(|wf| wf.status_field.clone());
        let has_workflow = workflow
            .map(|wf| wf.generate_action_endpoints)
            .unwrap_or(false);

        let search = entity_cfg.map(|ec| &ec.search);
        let has_fts = search
            .and_then(|s| s.fts_columns.as_ref())
            .map(|cols| !cols.is_empty())
            .unwrap_or(false);
        let has_embeddings = search
            .map(|s| !s.embedding_columns.is_empty())
            .unwrap_or(false);
        let fts_rest_mode = entity_cfg
            .map(|ec| ec.search.fts_rest_mode.clone())
            .unwrap_or_else(|| "query_param".to_string());

        let filter_fields = resolve_filter_fields(
            db,
            schema_title,
            entity_cfg
                .and_then(|ec| ec.filter_fields.as_ref())
                .map(|v| v.as_slice()),
        )
        .await?;

        let max_bulk_size = entity_cfg
            .and_then(|ec| ec.max_bulk_size)
            .unwrap_or(config.defaults.max_bulk_size);

        // Resolve parent/child relationships from parent_candidates.
        let stripped_title =
            super::router::strip_suffix(schema_title, &config.defaults.type_suffix);
        let topo = resolve_handler_topology(
            db,
            config,
            &domain,
            &entity_name,
            stripped_title,
            entity_cfg,
            &self.parent_candidates,
        )
        .await?;

        // Compute ancestor_path_params: how many {id} path segments Axum
        // accumulates before reaching this handler.  We walk up the resolved
        // parent chain, checking each ancestor's role in the domain it belongs to.
        let ancestor_path_params = compute_ancestor_path_params(
            config,
            &domain,
            stripped_title,
            entity_cfg,
            &self.parent_candidates,
            topo.parent_domain.as_deref(),
            &topo.role,
        );

        let param_name = super::router::param_name_from_path_segment(&path_segment);
        let parent_param_name = topo
            .parent_path_segment
            .as_deref()
            .map(super::router::param_name_from_path_segment);
        // For depth-2, resolve grandparent's param name, path segment, and domain.
        let (grandparent_param_name, grandparent_path_segment, grandparent_domain) =
            resolve_grandparent(
                db,
                config,
                &domain,
                entity_cfg,
                &self.parent_candidates,
                stripped_title,
                ancestor_path_params,
            )
            .await;

        // Resolve include paths from config. Skip non-root entities unless they
        // have explicit allow_include configuration.
        let include_paths =
            resolve_include_paths(db, config, &domain, schema_title, entity_cfg, project).await?;

        let nested_filter_fields =
            resolve_nested_filter_fields(db, schema_title, &module_name, &domain, config).await?;

        // Resolve handler imports via TypeRegistry instead of hard-coded template paths.
        let has_include = !include_paths.is_empty();
        let handler_imports = build_handler_imports(
            &operations,
            &entity_name,
            has_include,
            &include_paths,
            &domain,
            &module_name,
        );

        // Soft-delete audit policy: when the entity tracks deleted rows, the
        // query layer exposes an `include_deleted` argument on read methods.
        // Handlers must thread it through (passing `false` — APIs exclude
        // soft-deleted rows by default). Mirrors the logic in the query
        // generator (ddd/query.rs) so handler/query signatures stay in sync.
        let is_auditable = resolve_is_auditable(db, schema_title, config, &domain).await?;

        // ux sort plane (issue #306): the `?sort=` allow-list, resolved from
        // the SAME ux plan the list page renders. Empty when the flag is off
        // or no column is sortable — the handler then emits nothing.
        let ux_sort_fields = if operations.iter().any(|op| op == "list") {
            crate::ux::sort::resolve_ux_sort_plan(db, config, project, schema_title, &domain)
                .await?
                .fields
        } else {
            Vec::new()
        };

        // Mox op action routes + mirror copy list (issue #508 R2).
        let mox_ops = routable_mox_ops(db, schema_title, &entity_name).await;
        let mox_mirror_fields = mox_mirror_fields(db, schema_title).await;

        let ctx = HandlerContext {
            has_create: operations.contains(&"create".to_string()),
            has_read: operations.contains(&"read".to_string()),
            has_update: operations.contains(&"update".to_string()),
            has_delete: operations.contains(&"delete".to_string()),
            has_list: operations.contains(&"list".to_string()),
            entity_name,
            module_name: module_name.clone(),
            domain: domain.clone(),
            path_segment,
            tag,
            operations,
            mox_ops,
            mox_mirror_fields,
            parent_ref: topo.parent_ref,
            parent_entity,
            parent_path_segment: topo.parent_path_segment,
            parent_module_name: topo.parent_module_name,
            parent_domain: topo.parent_domain,
            role: topo.role,
            param_name,
            parent_param_name,
            grandparent_param_name,
            grandparent_path_segment,
            grandparent_domain,
            children: topo.children,
            cross_refs: topo.cross_refs,
            status_field,
            has_workflow,
            has_fts,
            fts_rest_mode,
            has_embeddings,
            filter_fields,
            nested_filter_fields,
            max_bulk_size,
            ancestor_path_params,
            hierarchy_field: entity_cfg.and_then(|ec| ec.hierarchy_field.clone()),
            tree_include: entity_cfg
                .and_then(|ec| ec.tree_include.as_ref())
                .map(|v| !v.is_empty())
                .unwrap_or(false),
            has_include,
            include_paths: include_paths.clone(),
            handler_imports,
            is_auditable,
            ux_sort_fields,
        };

        let content = render_template_with_project(tera, "api/handler.tera", &ctx, project)?;
        Ok(vec![GeneratedFile {
            path: self
                .output_dir
                .join("src")
                .join("api")
                .join(&domain)
                .join(format!("{}_handler.rs", module_name)),
            content,
        }])
    }
}

/// Resolved parent/child topology for a handler, gathered from manual config
/// and graph parent candidates.
struct HandlerTopology {
    parent_ref: Option<String>,
    parent_path_segment: Option<String>,
    parent_module_name: Option<String>,
    parent_domain: Option<String>,
    role: String,
    children: Vec<ChildInfo>,
    cross_refs: Vec<CrossRefInfo>,
}

impl Default for HandlerTopology {
    fn default() -> Self {
        Self {
            parent_ref: None,
            parent_path_segment: None,
            parent_module_name: None,
            parent_domain: None,
            role: "root".into(),
            children: vec![],
            cross_refs: vec![],
        }
    }
}

async fn resolve_handler_topology(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    domain: &str,
    entity_name: &str,
    stripped_title: &str,
    entity_cfg: Option<&EntityConfig>,
    parent_candidates: &[ParentCandidate],
) -> Result<HandlerTopology> {
    let mut topo = HandlerTopology {
        parent_ref: entity_cfg.and_then(|ec| ec.parent_ref.clone()),
        role: entity_cfg
            .and_then(|ec| ec.role.clone())
            .unwrap_or_else(|| "root".into()),
        ..Default::default()
    };

    // 1. Check manual config for parent (takes priority over graph)
    apply_config_parent(db, config, domain, entity_cfg, &mut topo).await;

    // 2. Fall back to graph parent_candidates if manual config didn't resolve parent.
    // Only nest when the parent is in the same domain (matching router behavior).
    apply_graph_parent(
        db,
        config,
        domain,
        stripped_title,
        parent_candidates,
        &mut topo,
    )
    .await;

    // Check if this entity is a parent (graph detection)
    collect_graph_children(
        db,
        config,
        domain,
        stripped_title,
        parent_candidates,
        &mut topo.children,
    )
    .await;

    // Fallback: if entity is a parent in config but wasn't matched by parent_candidates
    if topo.children.is_empty() {
        collect_config_children(db, config, domain, stripped_title, &mut topo.children).await;
    }

    // Detect cross-aggregate entity references.
    topo.cross_refs =
        detect_cross_refs(db, config, domain, entity_name, entity_cfg, &topo.children).await;

    // Graph/config discovery order is not stable across processes (HashMap
    // iteration, unordered MATCH rows); sort so emitted handler code
    // (`with_child` links, route registrations) is byte-identical between runs.
    topo.children
        .sort_by(|a, b| a.path_segment.cmp(&b.path_segment));

    Ok(topo)
}

async fn apply_config_parent(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    domain: &str,
    entity_cfg: Option<&EntityConfig>,
    topo: &mut HandlerTopology,
) {
    if let Some(ec) = entity_cfg
        && ec.role.as_deref() == Some("child")
        && let Some(parent_title) = &ec.parent
    {
        let parent_name = super::router::strip_suffix(parent_title, &config.defaults.type_suffix);
        if topo.parent_ref.is_none() {
            topo.parent_ref = Some(format!(
                "{}_id",
                codegraph_naming::to_snake_case(parent_name)
            ));
        }
        if let Ok(Some(parent_schema)) = db.get_schema_in_domain(parent_title, domain).await {
            topo.parent_path_segment = Some(resolve_path_segment(None, &parent_schema));
            topo.parent_module_name = Some(parent_schema.pg_table_name.clone());
            topo.parent_domain = if config
                .domains
                .get(domain)
                .map(|d| d.entities.contains(parent_title))
                .unwrap_or(false)
            {
                Some(domain.to_string())
            } else {
                parent_schema.domain.or_else(|| Some(domain.to_string()))
            };
        } else {
            topo.parent_path_segment = Some(codegraph_naming::to_kebab_case(parent_name));
            topo.parent_module_name = Some(codegraph_naming::to_snake_case(parent_name));
            topo.parent_domain = Some(domain.to_string());
        }
        topo.role = "child".to_string();
    }
}

async fn apply_graph_parent(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    domain: &str,
    stripped_title: &str,
    parent_candidates: &[ParentCandidate],
    topo: &mut HandlerTopology,
) {
    if topo.parent_path_segment.is_some() || topo.role == "root" {
        return;
    }
    for pc in parent_candidates {
        let child_name = super::router::strip_suffix(&pc.child_title, &config.defaults.type_suffix);
        if child_name == stripped_title {
            // Check if parent is in the same domain: either explicitly listed
            // or its schema is classified into this domain.
            let parent_in_domain = config
                .domains
                .get(domain)
                .map(|d| d.entities.contains(&pc.parent_title))
                .unwrap_or(false)
                || db
                    .get_schema_in_domain(&pc.parent_title, domain)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|s| s.domain.as_ref().map(|d| d == domain))
                    .unwrap_or(false);
            if !parent_in_domain {
                // Parent is in another domain — keep FK column but don't nest
                if topo.parent_ref.is_none() {
                    topo.parent_ref = Some(crate::fk_column_for_candidate(
                        pc,
                        &config.defaults.type_suffix,
                    ));
                }
                break;
            }
            let parent_name =
                super::router::strip_suffix(&pc.parent_title, &config.defaults.type_suffix);
            if topo.parent_ref.is_none() {
                topo.parent_ref = Some(crate::fk_column_for_candidate(
                    pc,
                    &config.defaults.type_suffix,
                ));
            }
            if let Ok(Some(parent_schema)) = db.get_schema_in_domain(&pc.parent_title, domain).await
            {
                topo.parent_path_segment = Some(resolve_path_segment(None, &parent_schema));
                topo.parent_module_name = Some(parent_schema.pg_table_name);
                topo.parent_domain = Some(domain.to_string());
            } else {
                topo.parent_path_segment = Some(codegraph_naming::to_kebab_case(parent_name));
                topo.parent_module_name = Some(codegraph_naming::to_snake_case(parent_name));
                topo.parent_domain = Some(domain.to_string());
            }
            topo.role = "child".to_string();
            break;
        }
    }
}

async fn collect_graph_children(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    domain: &str,
    stripped_title: &str,
    parent_candidates: &[ParentCandidate],
    resolved_children: &mut Vec<ChildInfo>,
) {
    for pc in parent_candidates {
        let parent_name =
            super::router::strip_suffix(&pc.parent_title, &config.defaults.type_suffix);
        if parent_name == stripped_title {
            let child_name =
                super::router::strip_suffix(&pc.child_title, &config.defaults.type_suffix);
            if let Ok(Some(child_schema)) = db.get_schema_in_domain(&pc.child_title, domain).await {
                resolved_children.push(ChildInfo {
                    entity_name: child_schema.rust_type_name.clone(),
                    module_name: child_schema.pg_table_name.clone(),
                    path_segment: resolve_path_segment(None, &child_schema),
                });
            } else {
                resolved_children.push(ChildInfo {
                    entity_name: child_name.to_string(),
                    module_name: codegraph_naming::to_snake_case(child_name),
                    path_segment: codegraph_naming::to_kebab_case(child_name),
                });
            }
        }
    }
}

async fn collect_config_children(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    domain: &str,
    stripped_title: &str,
    resolved_children: &mut Vec<ChildInfo>,
) {
    if let Some(domain_entry) = config.domains.get(domain) {
        // Iterate entity_config entries to find children of this entity
        {
            for (other_title, other_cfg) in &domain_entry.entity_config {
                if other_cfg.role.as_deref() == Some("child")
                    && let Some(parent_title) = &other_cfg.parent
                    && super::router::strip_suffix(parent_title, &config.defaults.type_suffix)
                        == stripped_title
                {
                    let child_name =
                        super::router::strip_suffix(other_title, &config.defaults.type_suffix);
                    if let Ok(Some(child_schema)) =
                        db.get_schema_in_domain(other_title, domain).await
                    {
                        resolved_children.push(ChildInfo {
                            entity_name: child_schema.rust_type_name.clone(),
                            module_name: child_schema.pg_table_name.clone(),
                            path_segment: resolve_path_segment(None, &child_schema),
                        });
                    } else {
                        resolved_children.push(ChildInfo {
                            entity_name: child_name.to_string(),
                            module_name: codegraph_naming::to_snake_case(child_name),
                            path_segment: codegraph_naming::to_kebab_case(child_name),
                        });
                    }
                }
            }
        }
    }
}

async fn detect_cross_refs(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    domain: &str,
    entity_name: &str,
    entity_cfg: Option<&EntityConfig>,
    resolved_children: &[ChildInfo],
) -> Vec<CrossRefInfo> {
    let mut resolved_cross_refs: Vec<CrossRefInfo> = vec![];
    {
        let schema_title_with_type = format!("{}Type", entity_name);
        if let Ok(referenced) = db.get_referenced_schemas(&schema_title_with_type).await {
            let parent_name = entity_cfg.and_then(|ec| ec.parent.as_deref()).unwrap_or("");
            let child_names: std::collections::HashSet<&str> = resolved_children
                .iter()
                .map(|c| c.entity_name.as_str())
                .collect();

            for ref_schema_node in &referenced {
                let ref_title = &ref_schema_node.title;
                let ref_entity_name =
                    super::router::strip_suffix(ref_title, &config.defaults.type_suffix);

                if ref_entity_name == entity_name {
                    continue;
                }
                if ref_entity_name == parent_name {
                    continue;
                }
                if child_names.contains(ref_entity_name) {
                    continue;
                }

                // Only include refs that are entities in any domain
                if let Ok(Some(ref_schema)) = db.get_schema_in_domain(ref_title, domain).await {
                    if ref_schema.pg_table_name.is_empty() {
                        continue;
                    }
                    // Find domain of referenced entity
                    let ref_domain = ref_schema
                        .domain
                        .clone()
                        .unwrap_or_else(|| domain.to_string());
                    let fk_col = codegraph_naming::to_snake_case(ref_entity_name) + "_id";
                    let link_rel = codegraph_naming::to_snake_case(ref_entity_name);
                    resolved_cross_refs.push(CrossRefInfo {
                        entity_name: ref_entity_name.to_string(),
                        module_name: ref_schema.pg_table_name.clone(),
                        domain: ref_domain,
                        path_segment: resolve_path_segment(None, &ref_schema),
                        fk_column: fk_col,
                        link_rel,
                    });
                }
            }
        }
    }
    resolved_cross_refs
}

fn compute_ancestor_path_params(
    config: &DomainConfig,
    domain: &str,
    stripped_title: &str,
    entity_cfg: Option<&EntityConfig>,
    parent_candidates: &[ParentCandidate],
    resolved_parent_domain: Option<&str>,
    resolved_role: &str,
) -> usize {
    if resolved_role != "child" {
        return 0;
    }
    let parent_title_for_lookup = entity_cfg.and_then(|ec| ec.parent.clone()).or_else(|| {
        parent_candidates
            .iter()
            .find(|pc| {
                super::router::strip_suffix(&pc.child_title, &config.defaults.type_suffix)
                    == stripped_title
            })
            .map(|pc| pc.parent_title.clone())
    });
    let mut depth = 1usize;
    if let Some(parent_title) = parent_title_for_lookup {
        // Prefer the current entity's domain for looking up the parent's config,
        // since the parent may exist in multiple domains with different roles.
        let parent_dom = if config
            .domains
            .get(domain)
            .map(|d| d.entities.contains(&parent_title))
            .unwrap_or(false)
        {
            domain
        } else {
            resolved_parent_domain.unwrap_or(domain)
        };
        // Check if parent has explicit role=root in its domain config
        let parent_explicit_root = config
            .domains
            .get(parent_dom)
            .and_then(|d| d.get_entity_config(&parent_title))
            .map(|ec| ec.role.as_deref() == Some("root"))
            .unwrap_or(false);
        if !parent_explicit_root {
            // Check if parent is a child (either via manual config or graph)
            let parent_is_child_in_config = config
                .domains
                .get(parent_dom)
                .and_then(|d| d.get_entity_config(&parent_title))
                .map(|ec| ec.role.as_deref() == Some("child"))
                .unwrap_or(false);
            let parent_is_child_in_graph = parent_candidates
                .iter()
                .any(|pc| pc.child_title == parent_title);
            if parent_is_child_in_config || parent_is_child_in_graph {
                depth += 1;
            }
        }
    }
    depth
}

async fn resolve_grandparent(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    domain: &str,
    entity_cfg: Option<&EntityConfig>,
    parent_candidates: &[ParentCandidate],
    stripped_title: &str,
    ancestor_path_params: usize,
) -> (Option<String>, Option<String>, Option<String>) {
    if ancestor_path_params < 2 {
        return (None, None, None);
    }
    let parent_title = entity_cfg.and_then(|ec| ec.parent.clone()).or_else(|| {
        parent_candidates
            .iter()
            .find(|pc| {
                super::router::strip_suffix(&pc.child_title, &config.defaults.type_suffix)
                    == stripped_title
            })
            .map(|pc| pc.parent_title.clone())
    });
    let gp_title = parent_title.and_then(|pt| {
        config
            .domains
            .get(domain)
            .and_then(|d| d.get_entity_config(&pt))
            .and_then(|ec| ec.parent.clone())
            .or_else(|| {
                let parent_stripped =
                    super::router::strip_suffix(&pt, &config.defaults.type_suffix);
                parent_candidates
                    .iter()
                    .find(|pc| {
                        super::router::strip_suffix(&pc.child_title, &config.defaults.type_suffix)
                            == parent_stripped
                    })
                    .map(|pc| pc.parent_title.clone())
            })
    });
    if let Some(ref gpt) = gp_title {
        if let Ok(Some(gp_schema)) = db.get_schema_in_domain(gpt, domain).await {
            let gp_seg = resolve_path_segment(None, &gp_schema);
            let gp_param = super::router::param_name_from_path_segment(&gp_seg);
            // Grandparent domain: look up which domain owns it.
            let gp_domain = config
                .domains
                .iter()
                .find_map(|(d, dc)| {
                    if dc.get_entity_config(gpt).is_some() {
                        Some(d.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| domain.to_string());
            (Some(gp_param), Some(gp_seg), Some(gp_domain))
        } else {
            let gp_name = super::router::strip_suffix(gpt, &config.defaults.type_suffix);
            let gp_seg = codegraph_naming::to_kebab_case(gp_name);
            let gp_param = super::router::param_name_from_path_segment(&gp_seg);
            (Some(gp_param), Some(gp_seg), Some(domain.to_string()))
        }
    } else {
        (None, None, None)
    }
}

async fn resolve_include_paths(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    domain: &str,
    schema_title: &str,
    entity_cfg: Option<&EntityConfig>,
    project: &ProjectConfig,
) -> Result<Vec<ResolvedIncludePath>> {
    // Resolve include paths from config. Skip non-root entities unless they
    // have explicit allow_include configuration.
    let has_explicit_include = entity_cfg
        .and_then(|ec| ec.allow_include.as_ref())
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    let is_root = entity_cfg
        .and_then(|ec| ec.role.as_deref())
        .map(|r| r == "root")
        .unwrap_or(true);
    let include_paths = if has_explicit_include || is_root {
        if let Some(ec) = entity_cfg {
            resolve_include_paths_for_topology(
                db,
                config,
                domain,
                schema_title,
                ec.allow_include.as_ref(),
                project.is_workers_topology(),
            )
            .await?
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    // Deduplicate include paths by alias to prevent duplicate struct fields
    // in the generated handler code (auto-discover can produce the same child
    // entity through multiple FK relationships).
    let include_paths = {
        let mut seen = std::collections::HashSet::new();
        include_paths
            .into_iter()
            .filter(|path| seen.insert(path.alias.clone()))
            .collect::<Vec<_>>()
    };
    Ok(include_paths)
}

fn build_handler_imports(
    operations: &[String],
    entity_name: &str,
    has_include: bool,
    include_paths: &[ResolvedIncludePath],
    domain: &str,
    module_name: &str,
) -> Vec<String> {
    let mut handler_refs: Vec<String> = vec![
        "AppState".into(),
        "AppError".into(),
        "ApiKeyInfo".into(),
        format!("{}Response", entity_name),
        format!("{}LinkedResponse", entity_name),
    ];
    if operations.contains(&"create".to_string()) {
        handler_refs.push("BulkItemError".into());
    }
    if operations.contains(&"create".to_string()) {
        handler_refs.push(format!("Create{}Request", entity_name));
    }
    if operations.contains(&"update".to_string()) {
        handler_refs.push(format!("Update{}Request", entity_name));
    }
    if has_include {
        handler_refs.push(format!("{}WithIncludeResponse", entity_name));
        handler_refs.push(format!("{}IncludedData", entity_name));
        // Add combined response types for dot-notation paths and VO response
        // types for child_table_override paths — the handler's match arms
        // reference these types directly in merge patterns.
        for path in include_paths {
            if path.segments.len() > 1 {
                handler_refs.push(format!("{}CombinedResponse", path.segments[0].entity_name));
            }
            // The override response type is only referenced textually in the
            // dot-path merge arm (handler.tera), so only register it for
            // multi-segment paths; single-segment includes use inference.
            if path.segments.len() > 1
                && let Some(over) = path
                    .segments
                    .first()
                    .and_then(|s| s.child_table_override.as_ref())
            {
                handler_refs.push(over.response_type.clone());
            }
        }
    }
    // Note: Create{entity_name}Body and {entity_name}BulkCreateResponse are
    // defined inline by handler.tera and must NOT be registered in the type
    // registry or added to handler_refs. Doing so causes E0432/E0255 when
    // multiple domains share an entity name (e.g. "Order" in screening AND
    // assessments): the second handler's registration silently fails (name
    // collision), resolve_imports generates wrong cross-module imports for
    // types that should be local, producing duplicate definitions and
    // non-existent type references in the generated code.
    let handler_caller: Vec<String> = vec![
        "crate".into(),
        "api".into(),
        domain.to_string(),
        format!("{}_handler", module_name),
    ];
    type_registry::resolve_imports(&handler_refs, &handler_caller)
}

async fn resolve_is_auditable(
    db: &dyn GraphQuerier,
    schema_title: &str,
    config: &DomainConfig,
    domain: &str,
) -> Result<bool> {
    let policies = db.get_policies_for_schema(schema_title).await?;
    // Issue #449: a `.ddd` design overrides both the operation set and the
    // auditable flag — the handler's call shape must match the design-mapped
    // query layer exactly (include_deleted on read methods exists only when
    // the query layer accepts it), so mirror ddd/query.rs's resolution.
    let design_surface = crate::ddd::design::DddDesignSurface::from_graph(db).await?;
    let mut operations =
        crate::api::api_model::resolve_entity_operations(db, config, domain, schema_title).await;
    if let Some(design_ops) = design_surface.operations_for(schema_title) {
        operations = design_ops;
    }
    // Append-only snapshot semantics (issue #284) — same inference as
    // db/ddl.rs: insert-only entities carry no audit columns, so the API
    // layer never passes `include_deleted` to the query/repository traits.
    let append_only = config
        .domains
        .get(domain)
        .and_then(|d| d.get_entity_config(schema_title))
        .as_ref()
        .is_some_and(|ec| ec.is_append_only())
        || !operations.iter().any(|op| op == "update" || op == "delete");
    let is_auditable = (if let Some(design) = design_surface.design_for(schema_title) {
        design.flags.auditable
    } else if policies.is_empty() {
        config
            .domains
            .get(domain)
            .and_then(|d| d.auditable)
            .unwrap_or(true)
    } else {
        policies
            .iter()
            .any(|p| matches!(&p.kind, PolicyKind::Audit(a) if a.track_deleted))
    }) && !append_only;
    Ok(is_auditable)
}

/// Template pins for the `?sort=` list surface (issue #306): the
/// generated handler parses/validates `sort`/`order` against the plan's
/// allow-list and names the valid fields in the 400; flag-off renders no
/// sort surface at all.
#[cfg(test)]
mod ux_sort_handler_template_tests {
    use super::*;
    use serde_json::json;

    fn base_ctx(ux_sort_fields: Vec<&str>) -> serde_json::Value {
        json!({
            "entity_name": "Task",
            "module_name": "task",
            "domain": "common",
            "path_segment": "tasks",
            "tag": "Task",
            "operations": ["list", "read"],
            "has_create": false,
            "has_read": true,
            "has_update": false,
            "has_delete": false,
            "has_list": true,
            "role": "root",
            "param_name": "task_id",
            "children": [],
            "cross_refs": [],
            "status_field": null,
            "has_workflow": false,
            "has_fts": false,
            "fts_rest_mode": "query_param",
            "has_embeddings": false,
            "filter_fields": [],
            "nested_filter_fields": [],
            "max_bulk_size": 100,
            "ancestor_path_params": 0,
            "hierarchy_field": null,
            "tree_include": false,
            "has_include": false,
            "include_paths": [],
            "handler_imports": [],
            "is_auditable": false,
            "ux_sort_fields": ux_sort_fields,
        })
    }

    fn render(ctx: &serde_json::Value) -> String {
        let tera = crate::template_engine::create_tera(std::path::Path::new("."))
            .expect("embedded templates");
        crate::render_template_with_project(
            &tera,
            "api/handler.tera",
            ctx,
            &ProjectConfig::default(),
        )
        .expect("handler renders")
    }

    #[test]
    fn flag_on_handler_validates_sort_params_against_allow_list() {
        let out = render(&base_ctx(vec!["name", "total_amount"]));

        // ListParams gains optional sort/order with serde defaults (absent
        // params keep the default created_at DESC ordering).
        assert!(
            out.contains("#[serde(default)]\n    pub sort: Option<String>,"),
            "{out}"
        );
        assert!(
            out.contains("#[serde(default)]\n    pub order: Option<String>,"),
            "{out}"
        );
        // The allow-list const carries the plan's sortable fields.
        assert!(
            out.contains("const ALLOWED_SORT_FIELDS: &[&str] = &["),
            "{out}"
        );
        assert!(out.contains("\"name\","), "{out}");
        assert!(out.contains("\"total_amount\","), "{out}");
        // 400 texts name the valid sortable fields (include-path style).
        assert!(
            out.contains("Unknown sort field: {sort}. Valid sortable fields: {}"),
            "{out}"
        );
        assert!(
            out.contains("order requires sort; valid sortable fields: {}"),
            "{out}"
        );
        // `order` is restricted to asc|desc.
        assert!(
            out.contains("Invalid order: {other}. Valid values: asc, desc"),
            "{out}"
        );
        // The validated spec threads into the query layer.
        assert!(out.contains("let ux_sort: Option<(String, bool)>"), "{out}");
        assert!(
            out.contains(
                "list_filtered(params.page, params.page_size, &filters, ux_sort, api_key_info"
            ),
            "{out}"
        );
    }

    #[test]
    fn flag_off_handler_emits_no_sort_surface() {
        let out = render(&base_ctx(Vec::new()));
        for needle in [
            "ALLOWED_SORT_FIELDS",
            "pub sort:",
            "pub order:",
            "Unknown sort field",
            "Invalid order",
            "order requires sort",
            "ux_sort",
        ] {
            assert!(
                !out.contains(needle),
                "flag-off handler must not contain {needle:?}:\n{out}"
            );
        }
        // The pre-#306 call shape stays intact.
        assert!(
            out.contains("list_filtered(params.page, params.page_size, &filters, api_key_info"),
            "{out}"
        );
    }

    /// Issue #479: create/update bodies extract through a
    /// `serde_path_to_error`-wrapped extractor so 422s name
    /// `path.to.field: cause` instead of axum's opaque untagged-enum
    /// rejection. The create body branches on the payload shape BEFORE
    /// deserializing (the untagged enum discards per-variant serde
    /// errors, so path-tracking the enum is not enough); the extractor is
    /// emitted only when a write body exists.
    #[test]
    fn write_bodies_extract_through_serde_path_to_error() {
        // has_create renders the extractor + the create signature.
        let mut ctx = base_ctx(Vec::new());
        ctx["has_create"] = json!(true);
        let out = render(&ctx);
        for needle in [
            // The shared rejection mapper names the failing path + cause.
            "fn json_body_rejection(e: serde_path_to_error::Error<serde_json::Error>) -> AppError {",
            "field: e.path().to_string(),",
            "message: e.inner().to_string(),",
            // Non-JSON content types keep the 415 contract axum's Json had.
            "StatusCode::UNSUPPORTED_MEDIA_TYPE",
            "code: \"UNSUPPORTED_MEDIA_TYPE\"",
            // The create extractor branches on the payload shape and
            // path-tracks EACH attempt (Bulk|Single semantics preserved).
            "pub struct CreateBody(pub CreateTaskBody);",
            "let body = if value.is_array() {",
            "Vec<CreateTaskRequest> =\n                serde_path_to_error::deserialize(value)",
            "CreateTaskBody::Bulk(items)",
            "CreateTaskBody::Single(item)",
            "CreateBody(body): CreateBody,",
            "#[serde(untagged)]",
        ] {
            assert!(
                out.contains(needle),
                "handler must contain {needle:?}:\n{out}"
            );
        }
        // Update rides the generic path-tracked extractor.
        let mut ctx = base_ctx(Vec::new());
        ctx["has_update"] = json!(true);
        let out = render(&ctx);
        assert!(out.contains("pub struct JsonPathBody<T>(pub T);"), "{out}");
        assert!(
            out.contains("JsonPathBody(body): JsonPathBody<UpdateTaskRequest>,"),
            "{out}"
        );
        // Read-only entities emit neither extractor nor wrapper noise.
        let out = render(&base_ctx(Vec::new()));
        for needle in ["JsonPathBody", "CreateBody", "json_body_rejection"] {
            assert!(
                !out.contains(needle),
                "read-only handler must not emit {needle:?}:\n{out}"
            );
        }
    }
}
