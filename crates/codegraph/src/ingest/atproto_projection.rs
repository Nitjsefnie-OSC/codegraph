use std::collections::HashMap;

use codegraph_config::DomainConfig;
use codegraph_core::error::GraphError;
use codegraph_core::traits::{GraphIngestor, GraphQuerier};
use codegraph_core::types::{
    AtprotoNamespaceNode, CollectionNode, EdgeType, LexiconNode, RepositoryNode, SchemaNode,
};
use codegraph_naming::{strip_suffix, to_snake_case};
use codegraph_type_contracts::RefClassificationKind;

use crate::generate::ProjectConfig;
#[cfg(test)]
use crate::generate::{AtprotoConfig, IntegrationFlags};

#[cfg_attr(feature = "hotpath", hotpath::measure)]
pub async fn project_atproto_lexicons(
    ingestor: &dyn GraphIngestor,
    querier: &dyn GraphQuerier,
    config: &DomainConfig,
    project: &ProjectConfig,
) -> Result<(), GraphError> {
    let authority = &project.atproto.atproto_authority;
    if authority.is_empty() {
        return Ok(());
    }

    let type_suffix = &config.defaults.type_suffix;

    let mut sorted_domains: Vec<&String> = config.domains.keys().collect();
    sorted_domains.sort();

    let mut title_to_nsid: HashMap<String, String> = HashMap::new();
    let mut collection_nsids: Vec<String> = Vec::new();

    for domain_name in &sorted_domains {
        let domain_slug = to_snake_case(domain_name);

        let namespace = AtprotoNamespaceNode {
            authority: authority.clone(),
            segment: domain_slug.clone(),
            domain: "atproto".to_string(),
        };
        ingestor.ingest_atproto_namespace(&namespace).await?;

        let schemas = querier.list_schemas(Some(domain_name)).await?;

        let entities: Vec<&SchemaNode> = schemas.iter().filter(|s| s.is_entity).collect();
        let vos: Vec<&SchemaNode> = schemas
            .iter()
            .filter(|s| !s.is_entity && !s.is_codelist && s.schema_type == "object")
            .collect();
        let codelists: Vec<&SchemaNode> = schemas.iter().filter(|s| s.is_codelist).collect();

        for entity in &entities {
            let entity_stripped = strip_suffix(&entity.title, type_suffix);
            let nsid = format!(
                "{}.{}.{}",
                authority,
                domain_slug,
                to_snake_case(&entity_stripped)
            );

            let lexicon = LexiconNode {
                nsid: nsid.clone(),
                lex_type: "record".to_string(),
                key_strategy: "tid".to_string(),
                revision: Some(1),
                description: entity.description.clone(),
                domain: domain_name.to_string(),
            };
            ingestor.ingest_lexicon(&lexicon).await?;

            let collection = CollectionNode {
                nsid: nsid.clone(),
                key_strategy: "tid".to_string(),
                domain: domain_name.to_string(),
            };
            ingestor.ingest_collection(&collection).await?;
            collection_nsids.push(nsid.clone());

            ingestor
                .ingest_edge(&entity.title, &nsid, EdgeType::ProjectsToLexicon, None)
                .await?;
            ingestor
                .ingest_edge(&nsid, &nsid, EdgeType::DefinesCollection, None)
                .await?;
            ingestor
                .ingest_edge(&nsid, authority, EdgeType::InNamespace, None)
                .await?;

            title_to_nsid.insert(entity.title.clone(), nsid);
        }

        for vo in &vos {
            let vo_stripped = strip_suffix(&vo.title, type_suffix);
            let nsid = format!(
                "{}.{}.{}",
                authority,
                domain_slug,
                to_snake_case(&vo_stripped)
            );

            let lexicon = LexiconNode {
                nsid: nsid.clone(),
                lex_type: "object".to_string(),
                key_strategy: String::new(),
                revision: Some(1),
                description: vo.description.clone(),
                domain: domain_name.to_string(),
            };
            ingestor.ingest_lexicon(&lexicon).await?;
            ingestor
                .ingest_edge(&vo.title, &nsid, EdgeType::ProjectsToLexicon, None)
                .await?;
            ingestor
                .ingest_edge(&nsid, authority, EdgeType::InNamespace, None)
                .await?;

            title_to_nsid.insert(vo.title.clone(), nsid);
        }

        for cl in &codelists {
            let cl_stripped = strip_suffix(&cl.title, type_suffix);
            let nsid = format!(
                "{}.{}.{}",
                authority,
                domain_slug,
                to_snake_case(&cl_stripped)
            );

            let lexicon = LexiconNode {
                nsid: nsid.clone(),
                lex_type: "string".to_string(),
                key_strategy: "nsid".to_string(),
                revision: Some(1),
                description: cl.description.clone(),
                domain: domain_name.to_string(),
            };
            ingestor.ingest_lexicon(&lexicon).await?;
            ingestor
                .ingest_edge(&cl.title, &nsid, EdgeType::ProjectsToLexicon, None)
                .await?;
            ingestor
                .ingest_edge(&nsid, authority, EdgeType::InNamespace, None)
                .await?;

            title_to_nsid.insert(cl.title.clone(), nsid);
        }

        for entity in &entities {
            let props = querier.get_properties(&entity.title).await?;
            let source_nsid = match title_to_nsid.get(&entity.title).cloned() {
                Some(nsid) => nsid,
                None => continue,
            };

            for prop in &props {
                if prop.classification_kind == Some(RefClassificationKind::EntityReference)
                    && let Some(ref_target) = &prop.ref_target
                {
                    let ref_stem = extract_ref_stem(ref_target);
                    if let Some(target_schema) =
                        find_schema_by_title_or_stem(&schemas, ref_stem, type_suffix)
                        && let Some(target_nsid) = title_to_nsid.get(&target_schema.title)
                    {
                        ingestor
                            .ingest_edge(
                                &source_nsid,
                                target_nsid,
                                EdgeType::LexiconReferences,
                                None,
                            )
                            .await?;
                    }
                }
            }
        }
    }

    if project.atproto.atproto_tenancy == "shared_pds" {
        let org_name = authority.rsplit('.').next().unwrap_or(authority);
        let repo = RepositoryNode {
            did: format!("did:plc:{}", authority),
            handle: Some(format!("{}.bsky.social", authority)),
            pds_endpoint: format!("https://pds.{}.bsky.social", authority),
            org_name: org_name.to_string(),
            tenancy_mode: project.atproto.atproto_tenancy.clone(),
        };
        let repo_did = ingestor.ingest_repository(&repo).await?;

        for nsid in &collection_nsids {
            ingestor
                .ingest_edge(nsid, &repo_did, EdgeType::StoredInRepository, None)
                .await?;
        }
    }

    ingestor.finalize().await?;
    Ok(())
}

fn extract_ref_stem(ref_path: &str) -> &str {
    let path = ref_path.strip_suffix('#').unwrap_or(ref_path);
    let filename = path.rsplit('/').next().unwrap_or(path);
    if ref_path.starts_with("#/") {
        return filename;
    }
    filename.strip_suffix(".json").unwrap_or(filename)
}

fn find_schema_by_title_or_stem<'a>(
    schemas: &'a [SchemaNode],
    stem: &str,
    type_suffix: &str,
) -> Option<&'a SchemaNode> {
    schemas.iter().find(|s| {
        s.title == stem
            || strip_suffix(&s.title, type_suffix) == stem
            || s.title == format!("{stem}{type_suffix}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codegraph_config::config::{DefaultsConfig, DomainEntry};
    use codegraph_core::mock::MockEngine;
    use codegraph_core::traits::GraphIngestor;
    use codegraph_core::types::SchemaNode;
    use std::collections::HashMap;

    fn make_schema_node(
        title: &str,
        domain: &str,
        is_entity: bool,
        is_codelist: bool,
        schema_type: &str,
    ) -> SchemaNode {
        let snake = to_snake_case(title);
        SchemaNode {
            namespace: None,
            schema_id: format!("{}/{}.json", domain, title),
            title: title.to_string(),
            description: None,
            schema_type: schema_type.to_string(),
            classification: String::new(),
            domain: Some(domain.to_string()),
            rel_path: format!("{}/{}.json", domain, title),
            pg_type: "UUID".to_string(),
            rust_type: "Uuid".to_string(),
            sea_orm_type: "Uuid".to_string(),
            rust_type_name: title.to_string(),
            pg_table_name: snake.clone(),
            api_path_segment: snake,
            parent_schema: None,
            is_entity,
            is_codelist,
            is_primitive_wrapper: false,
            has_all_of: false,
            has_one_of: false,
            has_any_of: false,
            has_definitions: false,
            custom_annotations: Default::default(),
            access: None,
            annotations: None,
        }
    }

    fn make_entity(title: &str, domain: &str) -> SchemaNode {
        make_schema_node(title, domain, true, false, "object")
    }

    fn make_vo(title: &str, domain: &str) -> SchemaNode {
        make_schema_node(title, domain, false, false, "object")
    }

    fn make_codelist(title: &str, domain: &str) -> SchemaNode {
        make_schema_node(title, domain, false, true, "string")
    }

    fn make_project_config(authority: &str) -> ProjectConfig {
        ProjectConfig {
            atproto: AtprotoConfig {
                atproto_authority: authority.to_string(),
                atproto_tenancy: "shared_pds".to_string(),
                ..Default::default()
            },
            integration: IntegrationFlags {
                has_atproto: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn make_domain_config() -> DomainConfig {
        let mut domains = HashMap::new();
        domains.insert(
            "grants".to_string(),
            DomainEntry {
                label: "Grants".to_string(),
                schema_dir: "grants".to_string(),
                postgres_schema: "grants".to_string(),
                depends_on: vec![],
                dependencies: Vec::new(),
                entities: vec![],
                entity_config: HashMap::new(),
                auto_discover: None,
                exclude_entities: vec![],
                force_entities: vec![],
                force_value_objects: vec![],
                exclude: vec![],
                auditable: None,
                tier: "extended".to_string(),
                worker_name: None,
                custom_domain: None,
                service_bindings: None,
                hyperdrive_binding: None,
                cron_triggers: None,
                remote_include_mode: None,
                webhooks: None,
                queue_name: None,
                queue_binding: None,
                queue_max_retries: None,
                queue_max_concurrency: None,
                observability: None,
                custom_routes: false,
            },
        );
        DomainConfig {
            namespaces: HashMap::new(),
            defaults: DefaultsConfig::default(),
            rbac: None,
            domains,
        }
    }

    #[tokio::test]
    async fn skips_when_authority_empty() {
        let engine = MockEngine::new();
        let config = make_domain_config();
        let project = ProjectConfig {
            atproto: AtprotoConfig {
                atproto_authority: String::new(),
                ..Default::default()
            },
            ..Default::default()
        };

        project_atproto_lexicons(&engine, &engine, &config, &project)
            .await
            .unwrap();

        let namespaces = engine.get_atproto_namespaces().await.unwrap();
        assert!(namespaces.is_empty());
    }

    #[tokio::test]
    async fn project_atproto_creates_namespace() {
        let engine = MockEngine::new();
        let config = make_domain_config();
        let project = make_project_config("nz.gravy");

        project_atproto_lexicons(&engine, &engine, &config, &project)
            .await
            .unwrap();

        let namespaces = engine.get_atproto_namespaces().await.unwrap();
        assert_eq!(namespaces.len(), 1);
        assert_eq!(namespaces[0].authority, "nz.gravy");
        assert_eq!(namespaces[0].segment, "grants");
        assert_eq!(namespaces[0].domain, "atproto");
    }

    #[tokio::test]
    async fn project_atproto_creates_entity_lexicons_and_collections() {
        let engine = MockEngine::new();

        engine
            .ingest_schema(&make_entity("GrantType", "grants"))
            .await
            .unwrap();

        let config = make_domain_config();
        let project = make_project_config("nz.gravy");

        project_atproto_lexicons(&engine, &engine, &config, &project)
            .await
            .unwrap();

        let lexicons = engine.get_lexicons("grants").await.unwrap();
        assert_eq!(lexicons.len(), 1);
        let grant_lex = &lexicons[0];
        assert_eq!(grant_lex.nsid, "nz.gravy.grants.grant");
        assert_eq!(grant_lex.lex_type, "record");
        assert_eq!(grant_lex.key_strategy, "tid");
        assert_eq!(grant_lex.domain, "grants");
        assert_eq!(grant_lex.revision, Some(1));

        let collections = engine.get_collections("grants").await.unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].nsid, "nz.gravy.grants.grant");
        assert_eq!(collections[0].key_strategy, "tid");
    }

    #[tokio::test]
    async fn project_atproto_creates_value_object_lexicons() {
        let engine = MockEngine::new();

        engine
            .ingest_schema(&make_entity("GrantType", "grants"))
            .await
            .unwrap();
        engine
            .ingest_schema(&make_vo("AmountType", "grants"))
            .await
            .unwrap();

        let config = make_domain_config();
        let project = make_project_config("nz.gravy");

        project_atproto_lexicons(&engine, &engine, &config, &project)
            .await
            .unwrap();

        let lexicons = engine.get_lexicons("grants").await.unwrap();
        assert_eq!(lexicons.len(), 2);

        let vo_lex = lexicons.iter().find(|l| l.lex_type == "object").unwrap();
        assert_eq!(vo_lex.nsid, "nz.gravy.grants.amount");
        assert_eq!(vo_lex.lex_type, "object");
        assert!(vo_lex.key_strategy.is_empty());
    }

    #[tokio::test]
    async fn project_atproto_creates_codelist_lexicons() {
        let engine = MockEngine::new();

        engine
            .ingest_schema(&make_entity("GrantType", "grants"))
            .await
            .unwrap();
        engine
            .ingest_schema(&make_codelist("StatusType", "grants"))
            .await
            .unwrap();

        let config = make_domain_config();
        let project = make_project_config("nz.gravy");

        project_atproto_lexicons(&engine, &engine, &config, &project)
            .await
            .unwrap();

        let lexicons = engine.get_lexicons("grants").await.unwrap();
        assert_eq!(lexicons.len(), 2);

        let cl_lex = lexicons.iter().find(|l| l.lex_type == "string").unwrap();
        assert_eq!(cl_lex.nsid, "nz.gravy.grants.status");
        assert_eq!(cl_lex.lex_type, "string");
        assert_eq!(cl_lex.key_strategy, "nsid");
    }

    #[tokio::test]
    async fn project_atproto_creates_repository_for_shared_pds() {
        let engine = MockEngine::new();

        engine
            .ingest_schema(&make_entity("GrantType", "grants"))
            .await
            .unwrap();

        let config = make_domain_config();
        let project = make_project_config("nz.gravy");

        project_atproto_lexicons(&engine, &engine, &config, &project)
            .await
            .unwrap();

        let repos = engine.get_repositories().await.unwrap();
        assert_eq!(repos.len(), 1);
        let repo = &repos[0];
        assert_eq!(repo.did, "did:plc:nz.gravy");
        assert_eq!(repo.handle, Some("nz.gravy.bsky.social".to_string()));
        assert_eq!(repo.pds_endpoint, "https://pds.nz.gravy.bsky.social");
        assert_eq!(repo.org_name, "gravy");
        assert_eq!(repo.tenancy_mode, "shared_pds");
    }

    #[tokio::test]
    async fn project_atproto_only_collections_for_entities() {
        let engine = MockEngine::new();

        engine
            .ingest_schema(&make_entity("GrantType", "grants"))
            .await
            .unwrap();
        engine
            .ingest_schema(&make_vo("AmountType", "grants"))
            .await
            .unwrap();
        engine
            .ingest_schema(&make_codelist("StatusType", "grants"))
            .await
            .unwrap();

        let config = make_domain_config();
        let project = make_project_config("nz.gravy");

        project_atproto_lexicons(&engine, &engine, &config, &project)
            .await
            .unwrap();

        let collections = engine.get_collections("grants").await.unwrap();
        assert_eq!(collections.len(), 1, "only entities get collection nodes");
        assert_eq!(collections[0].nsid, "nz.gravy.grants.grant");
    }

    #[tokio::test]
    async fn project_atproto_multiple_entities_in_domain() {
        let engine = MockEngine::new();

        engine
            .ingest_schema(&make_entity("GrantType", "grants"))
            .await
            .unwrap();
        engine
            .ingest_schema(&make_entity("ApplicantType", "grants"))
            .await
            .unwrap();

        let config = make_domain_config();
        let project = make_project_config("nz.gravy");

        project_atproto_lexicons(&engine, &engine, &config, &project)
            .await
            .unwrap();

        let lexicons = engine.get_lexicons("grants").await.unwrap();
        assert_eq!(lexicons.len(), 2);
        let nsids: Vec<&str> = lexicons.iter().map(|l| l.nsid.as_str()).collect();
        assert!(nsids.contains(&"nz.gravy.grants.grant"));
        assert!(nsids.contains(&"nz.gravy.grants.applicant"));

        let collections = engine.get_collections("grants").await.unwrap();
        assert_eq!(collections.len(), 2);
    }

    #[tokio::test]
    async fn project_atproto_strips_type_suffix_from_titles() {
        let engine = MockEngine::new();

        engine
            .ingest_schema(&make_entity("ApplicationFormType", "grants"))
            .await
            .unwrap();

        let mut config = make_domain_config();
        config.defaults.type_suffix = "Type".to_string();
        let project = make_project_config("nz.gravy");

        project_atproto_lexicons(&engine, &engine, &config, &project)
            .await
            .unwrap();

        let lexicons = engine.get_lexicons("grants").await.unwrap();
        assert_eq!(lexicons.len(), 1);
        assert_eq!(lexicons[0].nsid, "nz.gravy.grants.application_form");
    }

    #[tokio::test]
    async fn project_atproto_respects_authority_with_dots() {
        let engine = MockEngine::new();

        engine
            .ingest_schema(&make_entity("GrantType", "grants"))
            .await
            .unwrap();

        let config = make_domain_config();
        let project = make_project_config("com.example.app");

        project_atproto_lexicons(&engine, &engine, &config, &project)
            .await
            .unwrap();

        let lexicons = engine.get_lexicons("grants").await.unwrap();
        assert_eq!(lexicons[0].nsid, "com.example.app.grants.grant");

        let namespaces = engine.get_atproto_namespaces().await.unwrap();
        assert_eq!(namespaces[0].authority, "com.example.app");
    }
}
