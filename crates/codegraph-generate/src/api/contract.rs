use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_config::DomainConfig;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::ParentCandidate;
use serde::Serialize;

use crate::api::router::{RouterContext, RouterEntity, build_router_context};
use crate::error::Result;
use crate::traits::{
    DomainGenerator, DomainGeneratorKind, GeneratedFile, GlobalGenerator, GlobalGeneratorKind,
};
use crate::{ProjectConfig, render_template_with_project};

/// One HTTP endpoint in the plugin API contract.
///
/// `path` is the full path relative to the gateway origin (e.g.
/// `/api/v1/community_graph/trust-connection/{trust_connection_id}`), so a
/// plugin can build the fetch URL as `{gatewayUrl}{path}` with no string
/// assembly. `operation` mirrors the router template's route intent
/// (`list`, `create`, `read`, `update`, `delete`, `search`, …).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiContractRoute {
    pub method: String,
    pub path: String,
    pub operation: String,
}

/// The REST surface of a single entity within a domain.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiContractEntity {
    pub entity: String,
    pub module: String,
    pub path_segment: String,
    /// Full entity base path (e.g. `/api/v1/events/public-event`).
    pub base_path: String,
    pub routes: Vec<ApiContractRoute>,
}

/// Machine-readable API contract for one domain, emitted under
/// `api-contracts/{domain}.json` + `api-contracts/{domain}.ts`.
///
/// The domain base path is `/api/v1/{domain}` with the *verbatim* domain key
/// from `domains.toml` — this is what the gateway routes
/// (`/api/v1/{domain}/{*path}`) and the monolith routers mount, and it is the
/// exact string snake_case domains (`community_graph`, `provider_portal`)
/// must keep (issue #54).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiContract {
    pub domain: String,
    /// Exact gateway base path, e.g. `/api/v1/community_graph`.
    pub base_path: String,
    /// Entities keyed by path segment (BTreeMap → deterministic output).
    pub entities: BTreeMap<String, ApiContractEntity>,
}

/// Emit a per-domain machine-readable API contract (`.ts` + `.json`) that
/// mirrors the generated REST routers exactly. See issue #54 — plugin
/// packages and their tests must consume this contract instead of hardcoding
/// `/api/v1/{domain}/{entity}` paths, which drifted for the snake_case
/// domains (`community_graph`, `provider_portal`).
pub struct ApiContractGenerator {
    output_dir: PathBuf,
    parent_candidates: Vec<ParentCandidate>,
}

impl ApiContractGenerator {
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
impl DomainGenerator for ApiContractGenerator {
    fn kind(&self) -> DomainGeneratorKind {
        DomainGeneratorKind::ApiContract
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
        // Reuse the router generator's context resolution so the contract
        // reflects the exact entity set + path segments the router mounts.
        let ctx = build_router_context(
            db,
            domain,
            entity_titles,
            config,
            &self.parent_candidates,
            project,
        )
        .await?;
        let contract = build_api_contract(&ctx);

        let ts = render_template_with_project(tera, "api/contract.tera", &contract, project)?;
        let json = serde_json::to_string_pretty(&contract)
            .map_err(|e| crate::error::Error::Template(format!("serialize api contract: {e}")))?;

        let base = self.output_dir.join("api-contracts");
        Ok(vec![
            GeneratedFile {
                path: base.join(format!("{domain}.ts")),
                content: ts,
            },
            GeneratedFile {
                path: base.join(format!("{domain}.json")),
                content: format!("{json}\n"),
            },
        ])
    }
}

/// Global aggregator: emits `api-contracts/index.ts` re-exporting every
/// domain contract (and `api-contracts/README.md`).
pub struct ApiContractIndexGenerator {
    output_dir: PathBuf,
}

impl ApiContractIndexGenerator {
    pub fn new(output_dir: &Path) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
        }
    }
}

#[async_trait]
impl GlobalGenerator for ApiContractIndexGenerator {
    fn kind(&self) -> GlobalGeneratorKind {
        GlobalGeneratorKind::ApiContractIndex
    }

    async fn generate(
        &self,
        _db: &dyn GraphQuerier,
        config: &DomainConfig,
        generation_order: &[crate::GenerationEntry],
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        let domains: Vec<String> = crate::all_domains_for_generation(config, generation_order)
            .into_iter()
            .map(|(d, _)| d)
            .collect();
        let index = render_template_with_project(
            tera,
            "api/contract_index.tera",
            &serde_json::json!({ "domains": domains }),
            project,
        )?;
        let readme = render_template_with_project(
            tera,
            "api/contract_readme.tera",
            &serde_json::json!({}),
            project,
        )?;
        let base = self.output_dir.join("api-contracts");
        Ok(vec![
            GeneratedFile {
                path: base.join("index.ts"),
                content: index,
            },
            GeneratedFile {
                path: base.join("README.md"),
                content: readme,
            },
        ])
    }
}

/// Build the [`ApiContract`] for a domain from its resolved router context.
///
/// Routes are derived from the exact same fields the `api/router.tera`
/// template branches on (`has_create`, `has_update`, …, `media_fields`,
/// `children`), with nested child entities flattened into full paths.
fn build_api_contract(ctx: &RouterContext) -> ApiContract {
    let by_name: HashMap<&str, &RouterEntity> = ctx
        .entities
        .iter()
        .map(|e| (e.entity_name.as_str(), e))
        .collect();

    let mut entities = BTreeMap::new();
    for entity in &ctx.entities {
        let base_path = full_base_path(entity, &by_name, &ctx.domain);
        entities.insert(
            entity.path_segment.clone(),
            ApiContractEntity {
                entity: entity.entity_name.clone(),
                module: entity.module_name.clone(),
                path_segment: entity.path_segment.clone(),
                base_path: base_path.clone(),
                routes: entity_routes(entity, &base_path),
            },
        );
    }

    ApiContract {
        domain: ctx.domain.clone(),
        base_path: format!("/api/v1/{}", ctx.domain),
        entities,
    }
}

/// Full base path for an entity, walking parent links for nested routes.
///
/// Matches the router template's nesting: a child is mounted at
/// `{parent_base}/{parent_param}/{child_path_segment}`, so its full base path
/// is `{parent_base}/{{{parent.param_name}}}/{child.path_segment}`.
fn full_base_path(
    entity: &RouterEntity,
    by_name: &HashMap<&str, &RouterEntity>,
    domain: &str,
) -> String {
    if let Some(parent) = &entity.parent {
        let parent_entity = by_name
            .get(parent.entity_name.as_str())
            .expect("router parent must be in the same domain");
        let parent_base = full_base_path(parent_entity, by_name, domain);
        format!(
            "{}/{{{}}}/{}",
            parent_base, parent_entity.param_name, entity.path_segment
        )
    } else {
        format!("/api/v1/{}/{}", domain, entity.path_segment)
    }
}

/// Resolve the HTTP surface of a single entity, mirroring `api/router.tera`.
///
/// Every branch here must track the router template: same conditions, same
/// path shapes (including the literal `{id}` media route and the
/// `{param_name}` id route). The unit tests below pin both against the
/// template's rendered output.
fn entity_routes(entity: &RouterEntity, base_path: &str) -> Vec<ApiContractRoute> {
    let mut routes = Vec::new();

    // Collection: GET list always; POST create when enabled.
    routes.push(route("GET", base_path, "list"));
    if entity.has_create {
        routes.push(route("POST", base_path, "create"));
    }

    // Item route: GET read always; PUT/DELETE when enabled.
    let id_path = format!("{}/{{{}}}", base_path, entity.param_name);
    routes.push(route("GET", &id_path, "read"));
    if entity.has_update {
        routes.push(route("PUT", &id_path, "update"));
    }
    if entity.has_delete {
        routes.push(route("DELETE", &id_path, "delete"));
    }

    // Hierarchy tree.
    if entity.hierarchy_field.is_some() {
        routes.push(route("GET", &format!("{}/tree", id_path), "tree"));
    }

    // Embeddings.
    if entity.has_embeddings {
        routes.push(route(
            "POST",
            &format!("{}/semantic-search", base_path),
            "semantic_search",
        ));
    }

    // FTS search (dedicated endpoint only — query-param mode stays on list).
    if entity.has_fts && entity.fts_rest_mode != "query_param" {
        routes.push(route("GET", &format!("{}/search", base_path), "search"));
    }

    // Workflow actions (mirrors the router template's workflow block).
    if entity.has_workflow {
        routes.push(route(
            "POST",
            &format!("{}/actions/transition", id_path),
            "workflow_transition",
        ));
        if entity.has_approval_status {
            routes.push(route(
                "POST",
                &format!("{}/actions/approve", id_path),
                "workflow_approve",
            ));
            routes.push(route(
                "POST",
                &format!("{}/actions/reject", id_path),
                "workflow_reject",
            ));
        }
        routes.push(route(
            "POST",
            &format!("{}/actions/delegate", id_path),
            "workflow_delegate",
        ));
        routes.push(route(
            "GET",
            &format!("{}/workflow", id_path),
            "workflow_state",
        ));
        routes.push(route(
            "GET",
            &format!("{}/workflow/history", id_path),
            "workflow_history",
        ));
    }

    // Media fields (the router template hardcodes the literal `{id}` segment).
    for mf in &entity.media_fields {
        let media_path = format!("{}/{{id}}/{}", base_path, mf);
        routes.push(route("PUT", &media_path, &format!("media_upload_{mf}")));
        routes.push(route("GET", &media_path, &format!("media_download_{mf}")));
        routes.push(route("DELETE", &media_path, &format!("media_delete_{mf}")));
    }

    routes
}

fn route(method: &str, path: &str, operation: &str) -> ApiContractRoute {
    ApiContractRoute {
        method: method.to_string(),
        path: path.to_string(),
        operation: operation.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::router::{ChildInfo, ParentInfo, RouterEntity, param_name_from_path_segment};

    fn entity(name: &str, module: &str, path_segment: &str) -> RouterEntity {
        RouterEntity {
            mox_ops: Vec::new(),
            entity_name: name.into(),
            module_name: module.into(),
            path_segment: path_segment.into(),
            has_create: true,
            has_update: true,
            has_delete: true,
            has_read: true,
            has_list: true,
            has_workflow: false,
            has_approval_status: false,
            has_embeddings: false,
            has_fts: false,
            fts_rest_mode: "query_param".into(),
            role: "root".into(),
            param_name: param_name_from_path_segment(path_segment),
            parent: None,
            children: vec![],
            cross_refs: vec![],
            media_fields: vec![],
            hierarchy_field: None,
            pipeline_middleware: vec![],
            has_pipeline_layer: false,
            has_permissions: false,
            permission_scope: String::new(),
            permission_record_scoped: false,
            api_key_scope: module.into(),
        }
    }

    fn test_router_entity() -> RouterEntity {
        let mut e = entity("PublicEvent", "public_event", "public-event");
        e.has_fts = true;
        e.fts_rest_mode = "dedicated".into();
        e
    }

    #[test]
    fn route_matrix_matches_router_template_surface() {
        // Mirrors api/router.tera for a full-featured entity: create/read/
        // update/delete/list + dedicated FTS search.
        let e = test_router_entity();
        let routes = entity_routes(&e, "/api/v1/events/public-event");
        let flat: Vec<(String, String, String)> = routes
            .iter()
            .map(|r| (r.method.clone(), r.path.clone(), r.operation.clone()))
            .collect();
        assert_eq!(
            flat,
            vec![
                (
                    "GET".into(),
                    "/api/v1/events/public-event".into(),
                    "list".into()
                ),
                (
                    "POST".into(),
                    "/api/v1/events/public-event".into(),
                    "create".into()
                ),
                (
                    "GET".into(),
                    "/api/v1/events/public-event/{public_event_id}".into(),
                    "read".into()
                ),
                (
                    "PUT".into(),
                    "/api/v1/events/public-event/{public_event_id}".into(),
                    "update".into()
                ),
                (
                    "DELETE".into(),
                    "/api/v1/events/public-event/{public_event_id}".into(),
                    "delete".into()
                ),
                (
                    "GET".into(),
                    "/api/v1/events/public-event/search".into(),
                    "search".into()
                ),
            ]
        );
    }

    #[test]
    fn workflow_routes_expand_when_workflow_enabled() {
        let mut e = test_router_entity();
        e.has_workflow = true;
        e.has_approval_status = true;
        let routes = entity_routes(&e, "/api/v1/events/public-event");
        let paths: Vec<&str> = routes.iter().map(|r| r.path.as_str()).collect();
        for expected in [
            "/api/v1/events/public-event/{public_event_id}/actions/transition",
            "/api/v1/events/public-event/{public_event_id}/actions/approve",
            "/api/v1/events/public-event/{public_event_id}/actions/reject",
            "/api/v1/events/public-event/{public_event_id}/actions/delegate",
            "/api/v1/events/public-event/{public_event_id}/workflow",
            "/api/v1/events/public-event/{public_event_id}/workflow/history",
        ] {
            assert!(
                paths.contains(&expected),
                "missing workflow route {expected}"
            );
        }
    }

    #[test]
    fn media_routes_use_literal_id_segment() {
        let mut e = test_router_entity();
        e.media_fields = vec!["avatar".into()];
        let routes = entity_routes(&e, "/api/v1/events/public-event");
        let paths: Vec<&str> = routes.iter().map(|r| r.path.as_str()).collect();
        {
            let expected = "/api/v1/events/public-event/{id}/avatar";
            assert!(paths.contains(&expected), "missing media route {expected}");
        }
        // The router template hardcodes `{id}` for media routes, NOT the
        // entity param name.
        assert!(
            !paths.iter().any(|p| p.contains("{public_event_id}/avatar")),
            "media route must use literal {{id}} segment"
        );
    }

    #[test]
    fn snake_case_domain_base_path_is_verbatim() {
        // community_graph / provider_portal mount under their verbatim domain
        // key — the contract must preserve the underscore (issue #54).
        let ctx = RouterContext {
            domain: "community_graph".into(),
            entities: vec![entity("Relationship", "relationship", "relationship")],
            has_permission_middleware: false,
            has_custom_routes: false,
        };
        let contract = build_api_contract(&ctx);
        assert_eq!(contract.base_path, "/api/v1/community_graph");
        let rel = contract.entities.get("relationship").unwrap();
        assert_eq!(rel.base_path, "/api/v1/community_graph/relationship");
        assert!(
            rel.routes
                .iter()
                .any(|r| r.path == "/api/v1/community_graph/relationship"),
            "entity list route must use the snake_case base path"
        );
    }

    #[test]
    fn nested_child_base_path_walks_parent() {
        let mut parent = entity("Person", "person", "person");
        parent.param_name = "person_id".into();
        let mut child = entity("Note", "note", "note");
        child.role = "child".into();
        child.parent = Some(ParentInfo {
            entity_name: "Person".into(),
            module_name: "person".into(),
            path_segment: "person".into(),
            fk_column: "person_id".into(),
        });
        parent.children.push(ChildInfo {
            entity_name: "Note".into(),
            module_name: "note".into(),
            path_segment: "note".into(),
        });

        let ctx = RouterContext {
            domain: "crm".into(),
            entities: vec![parent, child],
            has_permission_middleware: false,
            has_custom_routes: false,
        };
        let contract = build_api_contract(&ctx);
        let note = contract.entities.get("note").unwrap();
        assert_eq!(note.base_path, "/api/v1/crm/person/{person_id}/note");
        assert!(
            note.routes
                .iter()
                .any(|r| r.path == "/api/v1/crm/person/{person_id}/note"),
            "nested list route must include the parent id segment"
        );
    }

    #[test]
    fn contract_serializes_with_camel_case_keys() {
        let ctx = RouterContext {
            domain: "events".into(),
            entities: vec![test_router_entity()],
            has_permission_middleware: false,
            has_custom_routes: false,
        };
        let json = serde_json::to_value(build_api_contract(&ctx)).unwrap();
        assert_eq!(json["basePath"], "/api/v1/events");
        assert_eq!(
            json["entities"]["public-event"]["pathSegment"],
            "public-event"
        );
        assert_eq!(
            json["entities"]["public-event"]["routes"][0]["method"],
            "GET"
        );
    }
}
