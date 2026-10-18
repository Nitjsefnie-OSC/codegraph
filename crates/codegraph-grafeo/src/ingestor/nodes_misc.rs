use std::collections::HashMap;

use codegraph_core::error::GraphError;
use codegraph_core::types::{
    ApiOperationNode, ApiResourceNode, AtprotoNamespaceNode, CollectionNode, ErrorDefinitionNode,
    FunctionNode, HttpEndpointNode, InteractionNode, LexiconNode, MembershipNode, MoxDomainModel,
    PermissionNode, PipelineNode, RegulatoryEdgeKind, RegulatoryKind, RegulatoryNode,
    RegulatoryOwner, RelationshipNode, RepositoryNode, RuleNode, SecurityIdentityNode, TenantNode,
};

use super::gql::CountingSession;
use super::gql::{
    escape_gql, opt_str, opt_to_grafeo_value, regulatory_reference_gql, serde_enum_str,
};
use super::registry;
use crate::engine::GrafeoEngine;

impl GrafeoEngine {
    pub(super) async fn insert_regulatory(&self, node: &RegulatoryNode) -> Result<(), GraphError> {
        let session = CountingSession::new(self.db().session());
        let properties_json =
            serde_json::to_string(&node.properties).unwrap_or_else(|_| "{}".to_string());
        let properties = format!("'{}'", escape_gql(&properties_json));
        let gql = format!(
            "INSERT (:Regulatory {{name: '{name}', kind: '{kind}', label: {label}, \
             definition: {definition}, domain: {domain}, properties_json: {properties}}})",
            name = escape_gql(&node.name),
            kind = node.kind.as_str(),
            label = opt_str(&node.label),
            definition = opt_str(&node.definition),
            domain = opt_str(&node.domain),
            properties = properties,
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_regulatory INSERT failed: {e}")))?;
        Ok(())
    }

    pub(super) async fn insert_regulatory_reference(
        &self,
        owner: &RegulatoryOwner,
        target: &str,
        target_kind: RegulatoryKind,
        edge_kind: RegulatoryEdgeKind,
        ref_path: Option<&str>,
    ) -> Result<(), GraphError> {
        crate::perf::bump_ingest_edge();
        let session = CountingSession::new(self.db().session());
        let gql = regulatory_reference_gql(owner, target, target_kind, edge_kind, ref_path);
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_regulatory_reference failed: {e}")))?;
        Ok(())
    }

    pub(super) async fn insert_function(&self, node: &FunctionNode) -> Result<(), GraphError> {
        let payload_json = serde_json::to_string(&node).map_err(|e| {
            GraphError::Ingest(format!("ingest_function payload serialization failed: {e}"))
        })?;
        // Direct create (issue #482): real Values, no GQL escaping needed.
        let props = HashMap::from([
            (
                "name".to_string(),
                grafeo::Value::String(node.name.clone().into()),
            ),
            ("domain".to_string(), opt_to_grafeo_value(&node.domain)),
            (
                "definition".to_string(),
                opt_to_grafeo_value(&node.definition),
            ),
            (
                "extends_function".to_string(),
                opt_to_grafeo_value(&node.extends),
            ),
            (
                "payload_json".to_string(),
                grafeo::Value::String(payload_json.into()),
            ),
        ]);
        self.create_registered_node("Function", &[registry::keys::function(&node.name)], &props)?;
        // The `FunctionExtends` edge is written separately (generic
        // `ingest_edge`) AFTER every node of the run exists — name-ordered
        // ingestion is not parent-first, so linking at node time would
        // silently drop edges to later-sorted parents.
        Ok(())
    }

    pub(super) async fn insert_rule(&self, node: &RuleNode) -> Result<(), GraphError> {
        let payload_json = serde_json::to_string(&node).map_err(|e| {
            GraphError::Ingest(format!("ingest_rule payload serialization failed: {e}"))
        })?;
        // Direct create (issue #482): real Values, no GQL escaping needed.
        let props = HashMap::from([
            (
                "name".to_string(),
                grafeo::Value::String(node.name.clone().into()),
            ),
            ("domain".to_string(), opt_to_grafeo_value(&node.domain)),
            (
                "definition".to_string(),
                opt_to_grafeo_value(&node.definition),
            ),
            (
                "kind".to_string(),
                grafeo::Value::String(node.kind.as_str().into()),
            ),
            (
                "input_type".to_string(),
                opt_to_grafeo_value(&node.input_type),
            ),
            (
                "payload_json".to_string(),
                grafeo::Value::String(payload_json.into()),
            ),
        ]);
        self.create_registered_node("Rule", &[registry::keys::rule(&node.name)], &props)?;
        Ok(())
    }

    pub(super) async fn insert_atproto_namespace(
        &self,
        node: &AtprotoNamespaceNode,
    ) -> Result<String, GraphError> {
        let props = HashMap::from([
            (
                "authority".to_string(),
                grafeo::Value::String(node.authority.clone().into()),
            ),
            (
                "segment".to_string(),
                grafeo::Value::String(node.segment.clone().into()),
            ),
            (
                "domain".to_string(),
                grafeo::Value::String(node.domain.clone().into()),
            ),
        ]);
        self.create_registered_node(
            "AtprotoNamespace",
            &[registry::keys::atproto_namespace(&node.authority)],
            &props,
        )?;
        Ok(node.authority.clone())
    }

    pub(super) async fn insert_lexicon(&self, node: &LexiconNode) -> Result<String, GraphError> {
        let props = HashMap::from([
            (
                "nsid".to_string(),
                grafeo::Value::String(node.nsid.clone().into()),
            ),
            (
                "lex_type".to_string(),
                grafeo::Value::String(node.lex_type.clone().into()),
            ),
            (
                "key_strategy".to_string(),
                grafeo::Value::String(node.key_strategy.clone().into()),
            ),
            (
                "revision".to_string(),
                match node.revision {
                    Some(v) => grafeo::Value::Int64(v),
                    None => grafeo::Value::Null,
                },
            ),
            (
                "description".to_string(),
                opt_to_grafeo_value(&node.description),
            ),
            (
                "domain".to_string(),
                grafeo::Value::String(node.domain.clone().into()),
            ),
        ]);
        self.create_registered_node("Lexicon", &[registry::keys::lexicon(&node.nsid)], &props)?;
        Ok(node.nsid.clone())
    }

    pub(super) async fn insert_collection(
        &self,
        node: &CollectionNode,
    ) -> Result<String, GraphError> {
        let props = HashMap::from([
            (
                "nsid".to_string(),
                grafeo::Value::String(node.nsid.clone().into()),
            ),
            (
                "key_strategy".to_string(),
                grafeo::Value::String(node.key_strategy.clone().into()),
            ),
            (
                "domain".to_string(),
                grafeo::Value::String(node.domain.clone().into()),
            ),
        ]);
        self.create_registered_node(
            "Collection",
            &[registry::keys::collection(&node.nsid)],
            &props,
        )?;
        Ok(node.nsid.clone())
    }

    pub(super) async fn insert_repository(
        &self,
        node: &RepositoryNode,
    ) -> Result<String, GraphError> {
        let props = HashMap::from([
            (
                "did".to_string(),
                grafeo::Value::String(node.did.clone().into()),
            ),
            ("handle".to_string(), opt_to_grafeo_value(&node.handle)),
            (
                "pds_endpoint".to_string(),
                grafeo::Value::String(node.pds_endpoint.clone().into()),
            ),
            (
                "org_name".to_string(),
                grafeo::Value::String(node.org_name.clone().into()),
            ),
            (
                "tenancy_mode".to_string(),
                grafeo::Value::String(node.tenancy_mode.clone().into()),
            ),
        ]);
        self.create_registered_node(
            "Repository",
            &[registry::keys::repository(&node.did)],
            &props,
        )?;
        Ok(node.did.clone())
    }

    // ── API metamodel ingestion ───────────────────────────────────────

    pub(super) async fn insert_api_resource(
        &self,
        node: &ApiResourceNode,
    ) -> Result<String, GraphError> {
        let id = format!("ar:{}", node.name);
        let props = HashMap::from([
            (
                "_node_id".to_string(),
                grafeo::Value::String(id.clone().into()),
            ),
            (
                "name".to_string(),
                grafeo::Value::String(node.name.clone().into()),
            ),
            (
                "schema_title".to_string(),
                grafeo::Value::String(node.schema_title.clone().into()),
            ),
            (
                "domain".to_string(),
                grafeo::Value::String(node.domain.clone().into()),
            ),
            ("label".to_string(), opt_to_grafeo_value(&node.label)),
            (
                "path_segment".to_string(),
                grafeo::Value::String(node.path_segment.clone().into()),
            ),
        ]);
        self.create_registered_node(
            "ApiResource",
            &[registry::keys::api_resource(&node.name)],
            &props,
        )?;
        Ok(id)
    }

    pub(super) async fn insert_api_operation(
        &self,
        node: &ApiOperationNode,
    ) -> Result<String, GraphError> {
        let id = format!("ao:{}", node.name);
        let props = HashMap::from([
            (
                "_node_id".to_string(),
                grafeo::Value::String(id.clone().into()),
            ),
            (
                "name".to_string(),
                grafeo::Value::String(node.name.clone().into()),
            ),
            (
                "kind".to_string(),
                grafeo::Value::String(node.kind.clone().into()),
            ),
            (
                "input_schema".to_string(),
                opt_to_grafeo_value(&node.input_schema),
            ),
            (
                "output_schema".to_string(),
                grafeo::Value::String(node.output_schema.clone().into()),
            ),
            ("paging".to_string(), grafeo::Value::Bool(node.paging)),
            ("sorting".to_string(), grafeo::Value::Bool(node.sorting)),
            ("filtering".to_string(), grafeo::Value::Bool(node.filtering)),
            ("domain".to_string(), opt_to_grafeo_value(&node.domain)),
        ]);
        self.create_registered_node(
            "ApiOperation",
            &[registry::keys::api_operation(&node.name)],
            &props,
        )?;
        Ok(id)
    }

    pub(super) async fn insert_interaction(
        &self,
        node: &InteractionNode,
    ) -> Result<String, GraphError> {
        let id = format!("ia:{}", uuid::Uuid::new_v4());
        let props = HashMap::from([
            ("name".to_string(), grafeo::Value::String(id.clone().into())),
            (
                "transport".to_string(),
                grafeo::Value::String(node.transport.clone().into()),
            ),
            ("domain".to_string(), opt_to_grafeo_value(&node.domain)),
        ]);
        self.create_registered_node("Interaction", &[registry::keys::interaction(&id)], &props)?;
        Ok(id)
    }

    pub(super) async fn insert_http_endpoint(
        &self,
        node: &HttpEndpointNode,
    ) -> Result<String, GraphError> {
        let id = format!("he:{}", uuid::Uuid::new_v4());
        let props = HashMap::from([
            ("name".to_string(), grafeo::Value::String(id.clone().into())),
            (
                "method".to_string(),
                grafeo::Value::String(node.method.clone().into()),
            ),
            (
                "path_template".to_string(),
                grafeo::Value::String(node.path_template.clone().into()),
            ),
            ("domain".to_string(), opt_to_grafeo_value(&node.domain)),
        ]);
        self.create_registered_node(
            "HttpEndpoint",
            &[registry::keys::http_endpoint(&id)],
            &props,
        )?;
        Ok(id)
    }

    pub(super) async fn insert_pipeline(&self, node: &PipelineNode) -> Result<String, GraphError> {
        let id = format!("pl:{}", node.name);
        let middleware_str = node
            .middleware
            .as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_default());
        let props = HashMap::from([
            (
                "_node_id".to_string(),
                grafeo::Value::String(id.clone().into()),
            ),
            (
                "name".to_string(),
                grafeo::Value::String(node.name.clone().into()),
            ),
            (
                "middleware".to_string(),
                opt_to_grafeo_value(&middleware_str),
            ),
            ("domain".to_string(), opt_to_grafeo_value(&node.domain)),
        ]);
        self.create_registered_node("Pipeline", &[registry::keys::pipeline(&node.name)], &props)?;
        Ok(id)
    }

    pub(super) async fn insert_error_definition(
        &self,
        node: &ErrorDefinitionNode,
    ) -> Result<String, GraphError> {
        let id = format!("ed:{}", node.code);
        let props = HashMap::from([
            (
                "code".to_string(),
                grafeo::Value::String(node.code.clone().into()),
            ),
            (
                "description".to_string(),
                grafeo::Value::String(node.description.clone().into()),
            ),
            (
                "http_status".to_string(),
                grafeo::Value::Int64(i64::from(node.http_status)),
            ),
            ("domain".to_string(), opt_to_grafeo_value(&node.domain)),
        ]);
        self.create_registered_node(
            "ErrorDefinition",
            &[registry::keys::error_definition(&node.code)],
            &props,
        )?;
        Ok(id)
    }

    pub(super) async fn insert_permission(
        &self,
        node: &PermissionNode,
    ) -> Result<String, GraphError> {
        let id = format!("pm:{}", node.name);
        let props = HashMap::from([
            (
                "_node_id".to_string(),
                grafeo::Value::String(id.clone().into()),
            ),
            (
                "name".to_string(),
                grafeo::Value::String(node.name.clone().into()),
            ),
            ("domain".to_string(), opt_to_grafeo_value(&node.domain)),
        ]);
        self.create_registered_node(
            "Permission",
            &[registry::keys::permission(&node.name)],
            &props,
        )?;
        Ok(id)
    }

    // ── Persistence metamodel ────────────────────────────────────────

    pub(super) async fn insert_relationship(
        &self,
        relationship: &RelationshipNode,
    ) -> Result<(), GraphError> {
        let session = CountingSession::new(self.db().session());
        let fk_json = serde_json::to_string(&relationship.foreign_key)
            .map_err(|e| GraphError::Ingest(e.to_string()))?;
        let propagation_json = serde_json::to_string(&relationship.propagation)
            .map_err(|e| GraphError::Ingest(e.to_string()))?;
        let domain = relationship.domain.clone().unwrap_or_default();
        let gql = format!(
            "INSERT (:Relationship {{ \
                name: '{}', source_schema: '{}', target_schema: '{}', \
                cardinality: '{}', ownership: '{}', fk_json: '{}', \
                propagation_json: '{}', domain: '{}' \
            }})",
            escape_gql(&relationship.name),
            escape_gql(&relationship.source_schema),
            escape_gql(&relationship.target_schema),
            escape_gql(&serde_enum_str(&relationship.cardinality)),
            escape_gql(&serde_enum_str(&relationship.ownership)),
            escape_gql(&fk_json),
            escape_gql(&propagation_json),
            escape_gql(&domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_relationship failed: {e}")))?;
        Ok(())
    }

    pub(super) async fn insert_security_identity(
        &self,
        identity: &SecurityIdentityNode,
    ) -> Result<(), GraphError> {
        let session = CountingSession::new(self.db().session());
        let domain = identity.domain.clone().unwrap_or_default();
        let gql = format!(
            "INSERT (:SecurityIdentity {{ \
                name: '{}', subject: '{}', domain: '{}' \
            }})",
            escape_gql(&identity.name),
            escape_gql(&identity.subject),
            escape_gql(&domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_security_identity failed: {e}")))?;
        Ok(())
    }

    pub(super) async fn insert_membership(
        &self,
        membership: &MembershipNode,
    ) -> Result<(), GraphError> {
        let session = CountingSession::new(self.db().session());
        let roles_json = serde_json::to_string(&membership.roles)
            .map_err(|e| GraphError::Ingest(e.to_string()))?;
        let valid_from = membership.valid_from.clone().unwrap_or_default();
        let valid_until = membership.valid_until.clone().unwrap_or_default();
        let gql = format!(
            "INSERT (:Membership {{ \
                identity: '{}', tenant: '{}', status: '{}', roles_json: '{}', \
                valid_from: '{}', valid_until: '{}' \
            }})",
            escape_gql(&membership.identity),
            escape_gql(&membership.tenant),
            escape_gql(&serde_enum_str(&membership.status)),
            escape_gql(&roles_json),
            escape_gql(&valid_from),
            escape_gql(&valid_until),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_membership failed: {e}")))?;
        Ok(())
    }

    pub(super) async fn insert_tenant(&self, tenant: &TenantNode) -> Result<(), GraphError> {
        let session = CountingSession::new(self.db().session());
        let strategy_json = serde_json::to_string(&tenant.strategy)
            .map_err(|e| GraphError::Ingest(e.to_string()))?;
        let domain = tenant.domain.clone().unwrap_or_default();
        let gql = format!(
            "INSERT (:Tenant {{ \
                name: '{}', label: '{}', strategy_json: '{}', domain: '{}' \
            }})",
            escape_gql(&tenant.name),
            escape_gql(&tenant.label),
            escape_gql(&strategy_json),
            escape_gql(&domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_tenant failed: {e}")))?;
        Ok(())
    }

    // ── mox domain metamodel ─────────────────────────────────────────

    pub(super) async fn insert_mox_domain(&self, model: &MoxDomainModel) -> Result<(), GraphError> {
        let session = CountingSession::new(self.db().session());

        for package in &model.packages {
            let gql = format!(
                "MERGE (:MoxPackage {{ name: '{}' }})",
                escape_gql(&package.name)
            );
            session.execute(&gql).map_err(|e| {
                GraphError::Ingest(format!("ingest_mox_domain package failed: {e}"))
            })?;
        }

        for vocab in &model.vocabularies {
            let facets_json = serde_json::to_string(&vocab.facets)
                .map_err(|e| GraphError::Ingest(e.to_string()))?;
            let entries_json = serde_json::to_string(&vocab.entries)
                .map_err(|e| GraphError::Ingest(e.to_string()))?;
            let gql = format!(
                "INSERT (:Vocabulary {{ \
                    name: '{}', package: '{}', source: '{}', version: {}, \
                    key_facet: '{}', facets_json: '{}', entries_json: '{}' \
                }})",
                escape_gql(&vocab.name),
                escape_gql(&vocab.package),
                escape_gql(&vocab.source),
                opt_str(&vocab.version),
                escape_gql(&vocab.key_facet),
                escape_gql(&facets_json),
                escape_gql(&entries_json),
            );
            session.execute(&gql).map_err(|e| {
                GraphError::Ingest(format!("ingest_mox_domain vocabulary failed: {e}"))
            })?;
            let gql = format!(
                "MATCH (v:Vocabulary {{ name: '{}', package: '{}' }}), \
                        (p:MoxPackage {{ name: '{}' }}) \
                 INSERT (v)-[:VocabularyInPackage]->(p)",
                escape_gql(&vocab.name),
                escape_gql(&vocab.package),
                escape_gql(&vocab.package),
            );
            session.execute(&gql).map_err(|e| {
                GraphError::Ingest(format!("ingest_mox_domain vocab-package link failed: {e}"))
            })?;
        }

        for op in &model.operations {
            let params_json =
                serde_json::to_string(&op.params).map_err(|e| GraphError::Ingest(e.to_string()))?;
            let bodies_json =
                serde_json::to_string(&op.bodies).map_err(|e| GraphError::Ingest(e.to_string()))?;
            let gql = format!(
                "INSERT (:Operation {{ \
                    name: '{}', class: '{}', package: '{}', description: {}, \
                    return_type: '{}', params_json: '{}', bodies_json: '{}' \
                }})",
                escape_gql(&op.name),
                escape_gql(&op.class),
                escape_gql(&op.package),
                opt_str(&op.description),
                escape_gql(&op.return_type),
                escape_gql(&params_json),
                escape_gql(&bodies_json),
            );
            session.execute(&gql).map_err(|e| {
                GraphError::Ingest(format!("ingest_mox_domain operation failed: {e}"))
            })?;
        }

        for feature in &model.derived_features {
            let gql = format!(
                "INSERT (:DerivedFeature {{ \
                    name: '{}', class: '{}', package: '{}', type_ref: '{}', expr: {} \
                }})",
                escape_gql(&feature.name),
                escape_gql(&feature.class),
                escape_gql(&feature.package),
                escape_gql(&feature.type_ref),
                opt_str(&feature.expr),
            );
            session.execute(&gql).map_err(|e| {
                GraphError::Ingest(format!("ingest_mox_domain derived feature failed: {e}"))
            })?;
        }

        for (class, schema_title) in &model.class_links {
            let gql = format!(
                "MATCH (o:Operation {{ class: '{class}' }}), (s:Schema {{ title: '{title}' }}) \
                 INSERT (o)-[:BelongsToClass]->(s)",
                class = escape_gql(class),
                title = escape_gql(schema_title),
            );
            session.execute(&gql).map_err(|e| {
                GraphError::Ingest(format!(
                    "ingest_mox_domain operation class link failed: {e}"
                ))
            })?;
            let gql = format!(
                "MATCH (d:DerivedFeature {{ class: '{class}' }}), (s:Schema {{ title: '{title}' }}) \
                 INSERT (d)-[:BelongsToClass]->(s)",
                class = escape_gql(class),
                title = escape_gql(schema_title),
            );
            session.execute(&gql).map_err(|e| {
                GraphError::Ingest(format!("ingest_mox_domain derived class link failed: {e}"))
            })?;
        }

        Ok(())
    }
}
