use codegraph_core::error::GraphError;
use codegraph_core::types::{
    ApiOperationNode, ApiResourceNode, AtprotoNamespaceNode, CollectionNode, ErrorDefinitionNode,
    FunctionNode, HttpEndpointNode, InteractionNode, LexiconNode, MembershipNode, MoxDomainModel,
    PermissionNode, PipelineNode, RegulatoryEdgeKind, RegulatoryKind, RegulatoryNode,
    RegulatoryOwner, RelationshipNode, RepositoryNode, RuleNode, SecurityIdentityNode, TenantNode,
};

use super::gql::CountingSession;
use super::gql::{escape_gql, opt_str, regulatory_reference_gql, serde_enum_str};
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
        let session = CountingSession::new(self.db().session());
        let payload_json = serde_json::to_string(&node).map_err(|e| {
            GraphError::Ingest(format!("ingest_function payload serialization failed: {e}"))
        })?;
        // The payload embeds nested JSON (expr payloads) whose escaped
        // quotes `\"` would terminate the GQL string literal early —
        // backslashes must be doubled BEFORE the single-quote escape.
        let payload_escaped = payload_json.replace('\\', "\\\\").replace('\'', "\\'");
        let gql = format!(
            "INSERT (:Function {{name: '{name}', domain: {domain}, \
             definition: {definition}, extends_function: {extends}, \
             payload_json: '{payload}'}})",
            name = escape_gql(&node.name),
            domain = opt_str(&node.domain),
            definition = opt_str(&node.definition),
            extends = opt_str(&node.extends),
            payload = payload_escaped,
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_function INSERT failed: {e}")))?;
        // The `FunctionExtends` edge is written separately (generic
        // `ingest_edge`) AFTER every node of the run exists — name-ordered
        // ingestion is not parent-first, so linking at node time would
        // silently drop edges to later-sorted parents.
        Ok(())
    }

    pub(super) async fn insert_rule(&self, node: &RuleNode) -> Result<(), GraphError> {
        let session = CountingSession::new(self.db().session());
        let payload_json = serde_json::to_string(&node).map_err(|e| {
            GraphError::Ingest(format!("ingest_rule payload serialization failed: {e}"))
        })?;
        // Same escaping discipline as ingest_function: the payload embeds
        // nested JSON whose escaped quotes `\"` would terminate the GQL
        // string literal early — backslashes doubled BEFORE the
        // single-quote escape.
        let payload_escaped = payload_json.replace('\\', "\\\\").replace('\'', "\\'");
        let gql = format!(
            "INSERT (:Rule {{name: '{name}', domain: {domain}, \
             definition: {definition}, kind: '{kind}', input_type: {input_type}, \
             payload_json: '{payload}'}})",
            name = escape_gql(&node.name),
            domain = opt_str(&node.domain),
            definition = opt_str(&node.definition),
            kind = node.kind.as_str(),
            input_type = opt_str(&node.input_type),
            payload = payload_escaped,
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_rule INSERT failed: {e}")))?;
        Ok(())
    }

    pub(super) async fn insert_atproto_namespace(
        &self,
        node: &AtprotoNamespaceNode,
    ) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
        let gql = format!(
            "INSERT (:AtprotoNamespace {{ authority: '{}', segment: '{}', domain: '{}' }})",
            escape_gql(&node.authority),
            escape_gql(&node.segment),
            escape_gql(&node.domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_atproto_namespace failed: {e}")))?;
        Ok(node.authority.clone())
    }

    pub(super) async fn insert_lexicon(&self, node: &LexiconNode) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
        let revision_val = match &node.revision {
            Some(v) => v.to_string(),
            None => "null".to_string(),
        };
        let gql = format!(
            "INSERT (:Lexicon {{ nsid: '{}', lex_type: '{}', key_strategy: '{}', \
             revision: {}, description: {}, domain: '{}' }})",
            escape_gql(&node.nsid),
            escape_gql(&node.lex_type),
            escape_gql(&node.key_strategy),
            revision_val,
            opt_str(&node.description),
            escape_gql(&node.domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_lexicon failed: {e}")))?;
        Ok(node.nsid.clone())
    }

    pub(super) async fn insert_collection(
        &self,
        node: &CollectionNode,
    ) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
        let gql = format!(
            "INSERT (:Collection {{ nsid: '{}', key_strategy: '{}', domain: '{}' }})",
            escape_gql(&node.nsid),
            escape_gql(&node.key_strategy),
            escape_gql(&node.domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_collection failed: {e}")))?;
        Ok(node.nsid.clone())
    }

    pub(super) async fn insert_repository(
        &self,
        node: &RepositoryNode,
    ) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
        let gql = format!(
            "INSERT (:Repository {{ did: '{}', handle: {}, pds_endpoint: '{}', \
             org_name: '{}', tenancy_mode: '{}' }})",
            escape_gql(&node.did),
            opt_str(&node.handle),
            escape_gql(&node.pds_endpoint),
            escape_gql(&node.org_name),
            escape_gql(&node.tenancy_mode),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_repository failed: {e}")))?;
        Ok(node.did.clone())
    }

    // ── API metamodel ingestion ───────────────────────────────────────

    pub(super) async fn insert_api_resource(
        &self,
        node: &ApiResourceNode,
    ) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
        let id = format!("ar:{}", node.name);
        let gql = format!(
            "INSERT (:ApiResource {{ \
                _node_id: '{}', name: '{}', schema_title: '{}', domain: '{}', \
                label: {}, path_segment: '{}' \
            }})",
            escape_gql(&id),
            escape_gql(&node.name),
            escape_gql(&node.schema_title),
            escape_gql(&node.domain),
            opt_str(&node.label),
            escape_gql(&node.path_segment),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_api_resource failed: {e}")))?;
        Ok(id)
    }

    pub(super) async fn insert_api_operation(
        &self,
        node: &ApiOperationNode,
    ) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
        let id = format!("ao:{}", node.name);
        let gql = format!(
            "INSERT (:ApiOperation {{ \
                _node_id: '{}', name: '{}', kind: '{}', input_schema: {}, output_schema: '{}', \
                paging: {}, sorting: {}, filtering: {}, domain: {} \
            }})",
            escape_gql(&id),
            escape_gql(&node.name),
            escape_gql(&node.kind),
            opt_str(&node.input_schema),
            escape_gql(&node.output_schema),
            node.paging,
            node.sorting,
            node.filtering,
            opt_str(&node.domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_api_operation failed: {e}")))?;
        Ok(id)
    }

    pub(super) async fn insert_interaction(
        &self,
        node: &InteractionNode,
    ) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
        let id = format!("ia:{}", uuid::Uuid::new_v4());
        let gql = format!(
            "INSERT (:Interaction {{ name: '{}', transport: '{}', domain: {} }})",
            escape_gql(&id),
            escape_gql(&node.transport),
            opt_str(&node.domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_interaction failed: {e}")))?;
        Ok(id)
    }

    pub(super) async fn insert_http_endpoint(
        &self,
        node: &HttpEndpointNode,
    ) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
        let id = format!("he:{}", uuid::Uuid::new_v4());
        let gql = format!(
            "INSERT (:HttpEndpoint {{ name: '{}', method: '{}', path_template: '{}', domain: {} }})",
            escape_gql(&id),
            escape_gql(&node.method),
            escape_gql(&node.path_template),
            opt_str(&node.domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_http_endpoint failed: {e}")))?;
        Ok(id)
    }

    pub(super) async fn insert_pipeline(&self, node: &PipelineNode) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
        let id = format!("pl:{}", node.name);
        let middleware_str = node
            .middleware
            .as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_default());
        let gql = format!(
            "INSERT (:Pipeline {{ _node_id: '{}', name: '{}', middleware: {}, domain: {} }})",
            escape_gql(&id),
            escape_gql(&node.name),
            opt_str(&middleware_str),
            opt_str(&node.domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_pipeline failed: {e}")))?;
        Ok(id)
    }

    pub(super) async fn insert_error_definition(
        &self,
        node: &ErrorDefinitionNode,
    ) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
        let id = format!("ed:{}", node.code);
        let gql = format!(
            "INSERT (:ErrorDefinition {{ \
                code: '{}', description: '{}', http_status: {}, domain: {} \
            }})",
            escape_gql(&node.code),
            escape_gql(&node.description),
            node.http_status,
            opt_str(&node.domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_error_definition failed: {e}")))?;
        Ok(id)
    }

    pub(super) async fn insert_permission(
        &self,
        node: &PermissionNode,
    ) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
        let id = format!("pm:{}", node.name);
        let gql = format!(
            "INSERT (:Permission {{ _node_id: '{}', name: '{}', domain: {} }})",
            escape_gql(&id),
            escape_gql(&node.name),
            opt_str(&node.domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_permission failed: {e}")))?;
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
