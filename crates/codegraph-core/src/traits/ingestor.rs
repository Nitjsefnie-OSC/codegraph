use crate::error::GraphError;
use crate::types::{
    ActionNode, ActorPolicyModel, ApiOperationNode, ApiResourceNode, AtprotoNamespaceNode,
    CodeList, CollectionNode, CompositeColumn, CompositeRange, ConditionNode, DataBindingNode,
    DddModelGraph, DeployModelGraph, EdgeProperties, EdgeType, EnumValue, ErrorDefinitionNode,
    EventNode, EvtModelGraph, FunctionNode, HttpEndpointNode, IngestStats, InteractionNode,
    LexiconNode, MembershipNode, ModuleDefinitionNode, MoxDomainModel, NamespaceImport,
    NamespaceNode, ParameterDefinitionNode, PermissionNode, PipelineNode, PolicyNode, PropertyNode,
    RegulatoryEdgeKind, RegulatoryKind, RegulatoryNode, RegulatoryOwner, RelationshipNode,
    RepositoryNode, RuleNode, SchemaNode, SecurityIdentityNode, TenantNode, ViewComponentNode,
    ViewContainerNode,
};
use async_trait::async_trait;

#[async_trait]
pub trait GraphIngestor: Send + Sync {
    async fn ingest_schema(&self, node: &SchemaNode) -> Result<String, GraphError>;

    async fn ingest_schemas(&self, nodes: &[SchemaNode]) -> Result<Vec<String>, GraphError> {
        let mut ids = Vec::with_capacity(nodes.len());
        for node in nodes {
            ids.push(self.ingest_schema(node).await?);
        }
        Ok(ids)
    }

    async fn ingest_property(
        &self,
        schema_title: &str,
        schema_id: &str,
        prop: &PropertyNode,
    ) -> Result<(), GraphError>;

    async fn ingest_codelist(&self, codelist: &CodeList) -> Result<(), GraphError>;

    async fn ingest_enum_value(
        &self,
        codelist_name: &str,
        value: &EnumValue,
    ) -> Result<(), GraphError>;

    async fn ingest_composite_column(&self, col: &CompositeColumn) -> Result<(), GraphError>;

    async fn ingest_composite_range(&self, range: &CompositeRange) -> Result<(), GraphError>;

    async fn ingest_extension(&self, name: &str) -> Result<(), GraphError>;

    async fn ingest_edge(
        &self,
        from_id: &str,
        to_id: &str,
        edge_type: EdgeType,
        props: Option<&EdgeProperties>,
    ) -> Result<(), GraphError>;

    async fn finalize(&self) -> Result<IngestStats, GraphError>;

    /// Update the is_entity flag on an already-ingested schema node.
    async fn ingest_view_container(&self, node: &ViewContainerNode) -> Result<String, GraphError>;

    /// Ingest a DSL `module "Name"` declaration (rex-ir `ModuleDeclaration`):
    /// the reusable pattern node view-level and module-internal `use`
    /// statements resolve against.
    async fn ingest_module_definition(
        &self,
        node: &ModuleDefinitionNode,
    ) -> Result<String, GraphError>;

    async fn ingest_view_component(&self, node: &ViewComponentNode) -> Result<String, GraphError>;

    async fn ingest_event(&self, node: &EventNode) -> Result<String, GraphError>;

    async fn ingest_action_node(&self, node: &ActionNode) -> Result<String, GraphError>;

    async fn ingest_parameter_definition(
        &self,
        node: &ParameterDefinitionNode,
    ) -> Result<String, GraphError>;

    async fn ingest_data_binding(&self, node: &DataBindingNode) -> Result<String, GraphError>;

    async fn ingest_atproto_namespace(
        &self,
        node: &AtprotoNamespaceNode,
    ) -> Result<String, GraphError>;
    async fn ingest_lexicon(&self, node: &LexiconNode) -> Result<String, GraphError>;
    async fn ingest_collection(&self, node: &CollectionNode) -> Result<String, GraphError>;
    async fn ingest_repository(&self, node: &RepositoryNode) -> Result<String, GraphError>;

    // ── Namespace plane (issue #267) ─────────────────────────────────

    /// Ingest one NamespaceNode (graph-wide namespace, issue #267). The
    /// node's `fqn` is the unique key (returned as the id). The `parent`
    /// field is persisted flat AND written as a `NamespaceParent` edge
    /// (child → parent) when present. `#268` connects the producers (mox/
    /// rosetta/`$id` sources).
    async fn ingest_namespace(&self, node: &NamespaceNode) -> Result<String, GraphError>;

    /// Ingest one NamespaceImports edge (`from_ns` imports `to_ns`), with
    /// wildcard/alias payload. Both namespaces must already exist.
    async fn ingest_namespace_import(&self, import: &NamespaceImport) -> Result<(), GraphError>;

    async fn update_entity_flag(&self, title: &str, is_entity: bool) -> Result<(), GraphError>;

    /// Update the classification kind on an already-ingested property node.
    async fn update_property_classification(
        &self,
        schema_title: &str,
        property_name: &str,
        kind: &str,
    ) -> Result<(), GraphError>;

    // ── API metamodel ingestion ───────────────────────────────────────

    async fn ingest_api_resource(&self, node: &ApiResourceNode) -> Result<String, GraphError>;

    async fn ingest_api_operation(&self, node: &ApiOperationNode) -> Result<String, GraphError>;

    async fn ingest_interaction(&self, node: &InteractionNode) -> Result<String, GraphError>;

    async fn ingest_http_endpoint(&self, node: &HttpEndpointNode) -> Result<String, GraphError>;

    async fn ingest_pipeline(&self, node: &PipelineNode) -> Result<String, GraphError>;

    async fn ingest_error_definition(
        &self,
        node: &ErrorDefinitionNode,
    ) -> Result<String, GraphError>;

    async fn ingest_permission(&self, node: &PermissionNode) -> Result<String, GraphError>;

    // --- Persistence Metamodel ---

    async fn ingest_policy(&self, policy: &PolicyNode) -> Result<(), GraphError> {
        let _ = policy;
        Ok(())
    }

    async fn ingest_policies(&self, policies: &[PolicyNode]) -> Result<(), GraphError> {
        for policy in policies {
            self.ingest_policy(policy).await?;
        }
        Ok(())
    }

    async fn ingest_relationship(&self, relationship: &RelationshipNode) -> Result<(), GraphError> {
        let _ = relationship;
        Ok(())
    }

    async fn ingest_relationships(
        &self,
        relationships: &[RelationshipNode],
    ) -> Result<(), GraphError> {
        for rel in relationships {
            self.ingest_relationship(rel).await?;
        }
        Ok(())
    }

    // --- Security Metamodel ---

    async fn ingest_security_identity(
        &self,
        identity: &SecurityIdentityNode,
    ) -> Result<(), GraphError> {
        let _ = identity;
        Ok(())
    }

    async fn ingest_membership(&self, membership: &MembershipNode) -> Result<(), GraphError> {
        let _ = membership;
        Ok(())
    }

    async fn ingest_tenant(&self, tenant: &TenantNode) -> Result<(), GraphError> {
        let _ = tenant;
        Ok(())
    }

    // ── Authorization metamodel ──────────────────────────────────────

    /// Ingest a full actor policy model: actors, capabilities, grant edges,
    /// and the model-level ActorPolicy carrier (blocks + never_both groups).
    async fn ingest_actor_policy(&self, model: &ActorPolicyModel) -> Result<(), GraphError>;

    // ── mox domain metamodel ─────────────────────────────────────────

    /// Ingest a full mox domain model: packages, vocabularies (with facets
    /// and vendored entries), class operations, derived features, and the
    /// (class → schema) BelongsToClass links resolved by name matching.
    async fn ingest_mox_domain(&self, model: &MoxDomainModel) -> Result<(), GraphError> {
        let _ = model;
        Ok(())
    }

    // ── DDD design plane (issue #449) ────────────────────────────────

    /// Ingest a full `.ddd` design model: the application, its modules, the
    /// per-class designs, repositories (+ operations), application services,
    /// and search definitions. `DddBindsClass` edges to `Schema` nodes are
    /// inserted when `resolved_title`/`entity_title` resolve; missing
    /// targets are skipped silently (advisory at this layer).
    async fn ingest_ddd_model(&self, model: &DddModelGraph) -> Result<(), GraphError>;

    // ── Event-contract plane (issue #454) ────────────────────────────

    /// Ingest a full `.evt` event-contract model: the declared events, the
    /// channels publishing them, and the subscriptions consuming them.
    /// Event references (channel `publishes`, subscription `events`) are
    /// intra-file names kept verbatim; resolution against the declaring
    /// file is a consumer concern, never a graph-plane one.
    async fn ingest_evt_model(&self, model: &EvtModelGraph) -> Result<(), GraphError>;

    // ── Deployment plane (rexlang #49) ──────────────────────────────

    /// Ingest a full `.deploy` deployment model: the declared applications
    /// (components + `connects` edges), profiles (target + defaults +
    /// policies + mappings), and deployments. Component references
    /// (`connects` endpoints, `deployment ... for`, `use`) are intra-file
    /// names kept verbatim; resolution against the declaring file is a
    /// consumer concern, never a graph-plane one.
    async fn ingest_deploy_model(&self, model: &DeployModelGraph) -> Result<(), GraphError>;

    // ── Constraint plane (issue #261) ─────────────────────────────────

    /// Ingest one ConditionNode (named condition or bridge-derived one_of)
    /// and link it to its owning schema via a `HasCondition` edge. Follows
    /// the IFML node-family precedent: a required method, implemented by
    /// every backend.
    async fn ingest_condition(&self, node: &ConditionNode) -> Result<(), GraphError>;

    // ── Regulatory reference plane (issue #265) ───────────────────────

    /// Ingest one regulatory reference metadata node (report/body/corpus/
    /// segment/rule source/rule schema/meta type). Deduplication is the
    /// bridge's job (name + kind is the natural key).
    async fn ingest_regulatory(&self, node: &RegulatoryNode) -> Result<(), GraphError>;

    // ── Computation plane (issue #263) ─────────────────────────────────

    /// Ingest one FunctionNode (rosetta func). The node carries its
    /// resolved `extends` parent name; the `FunctionExtends` edge is
    /// written separately via `ingest_edge` AFTER every function of the
    /// run exists (name-ordered ingestion is not parent-first).
    async fn ingest_function(&self, node: &FunctionNode) -> Result<(), GraphError>;

    /// Ingest one RuleNode (rosetta reporting/eligibility rule, issue
    /// #264). The `RuleAppliesTo` input edge is written separately via
    /// `ingest_edge` AFTER the input's Schema node exists (rules ingest
    /// before the schema bridging passes).
    async fn ingest_rule(&self, node: &RuleNode) -> Result<(), GraphError>;

    /// Link an owner element to a regulatory node. Edges are best-effort:
    /// when the target regulatory node (name + kind) is absent the backend
    /// is expected to succeed without writing (the rosetta bridge skips
    /// docReference targets that no declared element backs — sigil itself
    /// does not validate them).
    async fn ingest_regulatory_reference(
        &self,
        owner: &RegulatoryOwner,
        target: &str,
        target_kind: RegulatoryKind,
        edge_kind: RegulatoryEdgeKind,
        ref_path: Option<&str>,
    ) -> Result<(), GraphError>;
}
