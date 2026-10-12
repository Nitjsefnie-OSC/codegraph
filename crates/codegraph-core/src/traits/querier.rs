use crate::error::GraphError;
use crate::types::{
    ActionNode, ActorNode, ActorPolicyNode, ApiOperationNode, ApiResourceNode,
    AtprotoNamespaceNode, CapabilityNode, CodeList, CollectionNode, CompositeColumn,
    CompositeRange, CompositionTree, ConditionNode, DataBindingResolution, DddModelGraph,
    DeployModelGraph, EnumValue, ErrorDefinitionNode, EventNode, EvtModelGraph, Extension,
    FunctionNode, GrantEdge, HttpEndpointNode, InteractionNode, LexiconNode, MembershipNode,
    ModuleDefinitionNode, MoxDerivedFeatureNode, MoxOperationNode, MoxVocabularyNode,
    NamespaceImport, NamespaceNode, NavigationFlowRecord, ParameterDefinitionNode, ParentCandidate,
    PermissionNode, Permit, PipelineNode, PolicyNode, PropertyNode, RegulatoryNode,
    RegulatoryRefRecord, RelationshipNode, RepositoryNode, RuleNode, RuleRefRecord,
    SchemaClassificationData, SchemaNode, SecurityIdentityNode, StructuredSubField, TenantNode,
    ViewComponentNode, ViewContainerNode,
};
use async_trait::async_trait;
use std::collections::HashMap;

#[async_trait]
pub trait GraphQuerier: Send + Sync {
    async fn get_schema(&self, title: &str) -> Result<Option<SchemaNode>, GraphError>;
    async fn get_schema_by_id(&self, _schema_id: &str) -> Result<Option<SchemaNode>, GraphError> {
        Ok(None)
    }

    async fn get_schema_in_domain(
        &self,
        title: &str,
        _domain: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        self.get_schema(title).await
    }

    async fn list_schemas(&self, domain: Option<&str>) -> Result<Vec<SchemaNode>, GraphError>;
    async fn get_properties(&self, schema_title: &str) -> Result<Vec<PropertyNode>, GraphError>;

    /// Fetch properties for a schema, restricting to a specific domain when
    /// multiple schemas share the same title across domains.
    /// Default implementation falls back to `get_properties`.
    async fn get_properties_in_domain(
        &self,
        schema_title: &str,
        _domain: &str,
    ) -> Result<Vec<PropertyNode>, GraphError> {
        self.get_properties(schema_title).await
    }

    /// Resolve the schemas that structurally hang off `schema_title` as
    /// children. Two structural routes resolve, unioned, deduped by title
    /// (route 1 wins), and sorted by title:
    ///
    /// 1. **Inline `#/$defs` children** — schemas whose `parent_schema`
    ///    back-pointer names this schema (JSON-Schema inline definition
    ///    shape).
    /// 2. **Derived `refers` children** (issue #312) — entities targeted by
    ///    this schema's array-of-entity-ref properties (the
    ///    `Schema -[:HasProperty]-> Property {is_array: true}
    ///    -[:ItemsOf]-> Schema` chain). The array-of-entity-ref lowering
    ///    puts the FK on the CHILD table (`{parent}_id`), so the target is
    ///    the child. Only entity targets resolve here: value-object
    ///    (`contains`) arrays materialize as composition child tables and
    ///    codelist arrays as codelist FKs — different planes — and
    ///    self-references are excluded.
    ///
    /// This is the GRAPH route only. `domains.toml` `entity_config`
    /// (`role = "child"` + `parent`/`parent_ref`) stays authoritative:
    /// consumers merge config-declared children first and skip graph
    /// children already covered by them.
    async fn get_child_schemas(&self, schema_title: &str) -> Result<Vec<SchemaNode>, GraphError>;

    async fn get_classification_data(&self) -> Result<Vec<SchemaClassificationData>, GraphError>;
    async fn get_entity_names(&self) -> Result<Vec<String>, GraphError>;
    async fn get_entity_schema_map(&self) -> Result<HashMap<String, String>, GraphError>;
    async fn get_value_object_schemas(&self) -> Result<Vec<SchemaNode>, GraphError>;
    async fn get_parent_candidates(&self) -> Result<Vec<ParentCandidate>, GraphError>;

    async fn get_codelist(&self, name: &str) -> Result<Option<CodeList>, GraphError>;
    async fn list_codelists(&self) -> Result<Vec<CodeList>, GraphError>;
    async fn get_enum_values(&self, codelist_name: &str) -> Result<Vec<EnumValue>, GraphError>;

    async fn get_composite_columns(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Vec<CompositeColumn>, GraphError>;
    async fn get_structured_sub_fields(
        &self,
        schema_title: &str,
    ) -> Result<Vec<StructuredSubField>, GraphError>;
    async fn get_composite_range(
        &self,
        schema_title: &str,
    ) -> Result<Option<CompositeRange>, GraphError>;
    async fn get_consumed_fields(
        &self,
        schema_title: &str,
    ) -> Result<Vec<(PropertyNode, String)>, GraphError>;
    async fn get_codelist_for_property(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Option<(CodeList, String)>, GraphError>;

    async fn get_required_extensions(
        &self,
        schema_title: &str,
    ) -> Result<Vec<Extension>, GraphError>;

    async fn get_composition_tree(&self, schema_title: &str)
    -> Result<CompositionTree, GraphError>;
    async fn get_allof_targets(&self, schema_title: &str) -> Result<Vec<String>, GraphError>;
    /// Get all schemas that have an ExtendsSchema edge pointing to the given parent
    /// title — the reverse of `get_allof_targets`. Returns full SchemaNode objects
    /// so callers can inspect `is_entity`, `pg_table_name`, etc.
    ///
    /// For example, if both PersonType (entity) and PersonLegalType (VO) allOf-compose
    /// PersonBaseType, then `get_schemas_that_extend("PersonBaseType")` returns both.
    async fn get_schemas_that_extend(
        &self,
        _parent_title: &str,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        Ok(Vec::new())
    }
    async fn get_referencing_schemas(&self, schema_title: &str) -> Result<Vec<String>, GraphError>;
    async fn get_referenced_schemas(
        &self,
        schema_title: &str,
    ) -> Result<Vec<SchemaNode>, GraphError>;
    async fn get_property_ref_target(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Option<SchemaNode>, GraphError>;

    /// Like get_property_ref_target but matches by _schema_id (unique) instead of _schema_title.
    async fn get_property_ref_target_by_id(
        &self,
        property_name: &str,
        schema_id: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        // Default: delegate to title-based version
        self.get_property_ref_target(property_name, schema_id).await
    }

    /// Get properties associated with a schema by its unique schema_id (not title).
    async fn get_properties_by_schema_id(
        &self,
        _schema_id: &str,
    ) -> Result<Vec<PropertyNode>, GraphError> {
        // Default: return empty
        Ok(Vec::new())
    }

    async fn get_array_item_schema(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Option<SchemaNode>, GraphError>;

    async fn get_generation_order(&self) -> Result<Vec<String>, GraphError>;

    /// Bulk-fetch all schema→schema reference edges in one query.
    ///
    /// Returns `(source_title, target_title)` pairs for every
    /// `Schema -[:HasProperty]-> Property -[:ReferencesSchema]-> Schema` path.
    /// Implementations should use a single graph query instead of per-entity calls.
    /// Default implementation falls back to per-entity `get_referenced_schemas`.
    async fn list_all_schema_references(&self) -> Result<Vec<(String, String)>, GraphError> {
        let schemas = self.list_schemas(None).await?;
        let mut refs = Vec::new();
        for s in &schemas {
            if let Ok(targets) = self.get_referenced_schemas(&s.title).await {
                for t in &targets {
                    refs.push((s.title.clone(), t.title.clone()));
                }
            }
        }
        Ok(refs)
    }

    /// Titles that appear as the TARGET of another schema's property
    /// reference — the union of `ReferencesSchema` and `ItemsOf` edge
    /// targets (issue #460). This is the contained value-object surface:
    /// a non-entity schema in this set projects as a `{parent}_{feature}`
    /// child table of its owner, never as a standalone table.
    ///
    /// Default implementation falls back to bulk properties + inline
    /// `ref_target` (arrays whose engines resolve targets only through
    /// edges need their own override).
    async fn list_property_target_titles(
        &self,
    ) -> Result<std::collections::HashSet<String>, GraphError> {
        let mut out = std::collections::HashSet::new();
        for props in self.list_all_properties().await?.into_values() {
            for p in props {
                if let Some(t) = p.ref_target {
                    out.insert(t);
                }
            }
        }
        Ok(out)
    }

    /// Bulk-fetch all properties keyed by schema title.
    ///
    /// Default implementation falls back to per-schema `get_properties`.
    async fn list_all_properties(&self) -> Result<HashMap<String, Vec<PropertyNode>>, GraphError> {
        let schemas = self.list_schemas(None).await?;
        let mut map = HashMap::new();
        for s in &schemas {
            let props = self.get_properties(&s.title).await?;
            if !props.is_empty() {
                map.insert(s.title.clone(), props);
            }
        }
        Ok(map)
    }

    // ── IFML query methods (default: no IFML data) ─────────────────────

    /// Get all IFML ViewContainer nodes.
    async fn get_ifml_view_containers(&self) -> Result<Vec<ViewContainerNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Get all IFML ModuleDefinition nodes (DSL `module` declarations).
    async fn get_ifml_modules(&self) -> Result<Vec<ModuleDefinitionNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Get all ViewComponent nodes inside a container.
    async fn get_ifml_view_components(
        &self,
        _container_name: &str,
    ) -> Result<Vec<ViewComponentNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Get the ViewContainer nodes nested inside a parent ViewContainer via
    /// ContainsViewContainer edges.
    async fn get_ifml_container_children(
        &self,
        _parent: &str,
    ) -> Result<Vec<ViewContainerNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Get all Event nodes for a given parent element.
    async fn get_ifml_events(&self, _parent_id: &str) -> Result<Vec<EventNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Get NavigationFlow edges with full fidelity: the element the event
    /// hangs off (`source`), the owning ViewContainer (`source_container`),
    /// the event name, the target ViewContainer, and the persisted
    /// `target_param_binding` JSON.
    async fn get_ifml_navigation_flows(&self) -> Result<Vec<NavigationFlowRecord>, GraphError> {
        Ok(Vec::new())
    }

    /// Get DataFlow edges: (source, target, source_param, target_param).
    async fn get_ifml_data_flows(
        &self,
    ) -> Result<Vec<(String, String, Option<String>, Option<String>)>, GraphError> {
        Ok(Vec::new())
    }

    /// Get all ActionNode definitions.
    async fn get_ifml_actions(&self) -> Result<Vec<ActionNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Get TriggersAction edges: (event_name, action_name).
    async fn get_ifml_action_triggers(&self) -> Result<Vec<(String, String)>, GraphError> {
        Ok(Vec::new())
    }

    /// Get all ParameterDefinition nodes.
    async fn get_ifml_parameters(&self) -> Result<Vec<ParameterDefinitionNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Get the ParameterDefinition nodes bound to a ViewContainer via
    /// HasParameter edges.
    async fn get_parameters_for_view(
        &self,
        _container_name: &str,
    ) -> Result<Vec<ParameterDefinitionNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Get resolved data bindings: component → entity title + bound fields.
    async fn get_data_bindings(&self) -> Result<Vec<DataBindingResolution>, GraphError> {
        Ok(Vec::new())
    }

    // ── AT Protocol query methods (default: no AT Protocol data) ────────

    /// Get all Lexicon nodes for a domain.
    #[allow(unused_variables)]
    async fn get_lexicons(&self, domain: &str) -> Result<Vec<LexiconNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Get the Lexicon projected from a schema.
    #[allow(unused_variables)]
    async fn get_lexicon_by_schema(
        &self,
        schema_title: &str,
    ) -> Result<Option<LexiconNode>, GraphError> {
        Ok(None)
    }

    /// Get all Collection nodes for a domain.
    #[allow(unused_variables)]
    async fn get_collections(&self, domain: &str) -> Result<Vec<CollectionNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Get all Repository nodes.
    #[allow(unused_variables)]
    async fn get_repositories(&self) -> Result<Vec<RepositoryNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Get all AT-Protocol namespace nodes (renamed from `get_namespaces`,
    /// issue #267 collision resolution).
    #[allow(unused_variables)]
    async fn get_atproto_namespaces(&self) -> Result<Vec<AtprotoNamespaceNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Get Lexicons that this Lexicon references (for ref/union types).
    #[allow(unused_variables)]
    async fn get_lexicon_references(&self, nsid: &str) -> Result<Vec<LexiconNode>, GraphError> {
        Ok(Vec::new())
    }

    // ── API metamodel query methods ─────────────────────────────────────

    async fn get_api_resources(&self) -> Result<Vec<ApiResourceNode>, GraphError> {
        Ok(Vec::new())
    }

    async fn get_api_resource(&self, _name: &str) -> Result<Option<ApiResourceNode>, GraphError> {
        Ok(None)
    }

    async fn get_api_operation(&self, _name: &str) -> Result<Option<ApiOperationNode>, GraphError> {
        Ok(None)
    }

    async fn get_http_endpoint_for_operation(
        &self,
        _operation_name: &str,
    ) -> Result<Option<HttpEndpointNode>, GraphError> {
        Ok(None)
    }

    async fn get_api_operations(
        &self,
        _resource_name: &str,
    ) -> Result<Vec<ApiOperationNode>, GraphError> {
        Ok(Vec::new())
    }

    async fn get_interactions(
        &self,
        _operation_name: &str,
    ) -> Result<Vec<InteractionNode>, GraphError> {
        Ok(Vec::new())
    }

    async fn get_http_endpoints(&self) -> Result<Vec<HttpEndpointNode>, GraphError> {
        Ok(Vec::new())
    }

    async fn get_error_definitions(&self) -> Result<Vec<ErrorDefinitionNode>, GraphError> {
        Ok(Vec::new())
    }

    async fn get_permissions(&self) -> Result<Vec<PermissionNode>, GraphError> {
        Ok(Vec::new())
    }

    async fn get_pipelines(&self) -> Result<Vec<PipelineNode>, GraphError> {
        Ok(Vec::new())
    }

    async fn get_pipeline_for_endpoint(
        &self,
        _endpoint_path: &str,
    ) -> Result<Option<PipelineNode>, GraphError> {
        Ok(None)
    }

    // --- Persistence Metamodel queries ---

    async fn get_policies_for_schema(
        &self,
        schema_title: &str,
    ) -> Result<Vec<PolicyNode>, GraphError> {
        let _ = schema_title;
        Ok(Vec::new())
    }

    async fn get_relationships_for_schema(
        &self,
        schema_title: &str,
    ) -> Result<Vec<RelationshipNode>, GraphError> {
        let _ = schema_title;
        Ok(Vec::new())
    }

    async fn get_relationship_by_name(
        &self,
        name: &str,
    ) -> Result<Option<RelationshipNode>, GraphError> {
        let _ = name;
        Ok(None)
    }

    async fn list_all_policies(&self) -> Result<Vec<PolicyNode>, GraphError> {
        Ok(Vec::new())
    }

    async fn list_all_relationships(&self) -> Result<Vec<RelationshipNode>, GraphError> {
        Ok(Vec::new())
    }

    // --- Security Metamodel queries ---

    async fn get_security_identity(
        &self,
        subject: &str,
    ) -> Result<Option<SecurityIdentityNode>, GraphError> {
        let _ = subject;
        Ok(None)
    }

    async fn get_memberships_for_identity(
        &self,
        identity_name: &str,
    ) -> Result<Vec<MembershipNode>, GraphError> {
        let _ = identity_name;
        Ok(Vec::new())
    }

    async fn get_tenant(&self, name: &str) -> Result<Option<TenantNode>, GraphError> {
        let _ = name;
        Ok(None)
    }

    async fn list_all_tenants(&self) -> Result<Vec<TenantNode>, GraphError> {
        Ok(Vec::new())
    }

    // ── Authorization metamodel query methods ─────────────────────────

    async fn get_actors(&self) -> Result<Vec<ActorNode>, GraphError> {
        Ok(Vec::new())
    }

    async fn get_capabilities(&self) -> Result<Vec<CapabilityNode>, GraphError> {
        Ok(Vec::new())
    }

    async fn get_grants(&self) -> Result<Vec<GrantEdge>, GraphError> {
        Ok(Vec::new())
    }

    async fn get_actor_policy(&self) -> Result<Option<ActorPolicyNode>, GraphError> {
        Ok(None)
    }

    /// Effective grant decisions for an actor after resolving its `extends`
    /// chain: union of all chain grants with forbid-wins per capability
    /// (see `resolve_effective_permits` for the exact rules).
    async fn effective_permits(&self, _actor: &str) -> Result<Vec<Permit>, GraphError> {
        Ok(Vec::new())
    }

    // ── DDD design plane query methods (issue #449) ───────────────────

    /// Every ingested `.ddd` design model, reassembled to the exact ingest
    /// shape: applications sorted by name, modules/designs/services/
    /// searches by their `ordinal`, repositories sorted by name and
    /// attached to their application, repository operations by `ordinal`.
    async fn get_ddd_models(&self) -> Result<Vec<DddModelGraph>, GraphError> {
        Ok(Vec::new())
    }

    // ── Event-contract plane query methods (issue #454) ──────────────

    /// Every ingested `.evt` event-contract model, reassembled to the exact
    /// ingest shape: models grouped by `source_path` (sorted
    /// lexicographically), events/channels/subscriptions within each model
    /// by their `ordinal`.
    async fn get_evt_models(&self) -> Result<Vec<EvtModelGraph>, GraphError> {
        Ok(Vec::new())
    }

    // ── Deployment plane query methods (rexlang #49) ─────────────────

    /// Every ingested `.deploy` deployment model, reassembled to the exact
    /// ingest shape: models grouped by `source_path` (sorted
    /// lexicographically), applications/profiles/deployments within each
    /// model by their declaration order.
    async fn get_deploy_models(&self) -> Result<Vec<DeployModelGraph>, GraphError> {
        Ok(Vec::new())
    }

    /// When a composition parent projects a `{parent}_{feature}` child
    /// table whose name COLLIDES with this schema's own `pg_table_name`,
    /// returns `(parent_title, projected_table)`: the parent's child-table
    /// projection must win (it carries the parent FK + audit columns —
    /// cornucopia's build-time SQL validation type-checks queries against
    /// the LIVE database), so the DDL plane emits the child-shaped table as
    /// the standalone migration and the parent skips its duplicate block.
    /// `None` when nothing collides: contains-only VOs keep BOTH surfaces
    /// (their standalone title-based table AND the parent's child table).
    /// `$title` matches a schema by title or pg_table_name.
    async fn composition_table_conflict(
        &self,
        _schema_title: &str,
    ) -> Result<Option<(String, String)>, GraphError> {
        Ok(None)
    }

    // ── mox domain query methods (default: no mox data) ──────────────

    /// All ingested mox vocabularies, with facets and vendored entries.
    async fn get_mox_vocabularies(&self) -> Result<Vec<MoxVocabularyNode>, GraphError> {
        Ok(Vec::new())
    }

    /// All ingested mox class operations (bodies carried verbatim as text).
    async fn get_mox_operations(&self) -> Result<Vec<MoxOperationNode>, GraphError> {
        Ok(Vec::new())
    }

    /// All ingested mox derived features.
    async fn get_mox_derived_features(&self) -> Result<Vec<MoxDerivedFeatureNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Mox derived features linked to a schema entity via BelongsToClass
    /// edges (i.e. those whose class name matched a schema-ingested entity
    /// at ingest time).
    async fn get_mox_derived_features_for_schema(
        &self,
        schema_title: &str,
    ) -> Result<Vec<MoxDerivedFeatureNode>, GraphError> {
        Ok(self
            .get_mox_derived_features()
            .await?
            .into_iter()
            .filter(|d| d.class == schema_title)
            .collect())
    }

    // ── Constraint plane query methods (default: no condition data) ──

    /// All ConditionNodes attached to a schema via HasCondition edges
    /// (named conditions AND bridge-derived one_of nodes).
    async fn get_conditions_for_schema(
        &self,
        _schema_title: &str,
    ) -> Result<Vec<ConditionNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Every ConditionNode in the graph.
    async fn list_conditions(&self) -> Result<Vec<ConditionNode>, GraphError> {
        Ok(Vec::new())
    }

    // ── Regulatory reference plane query methods (issue #265) ─────────

    /// Every regulatory reference metadata node in the graph, ordered by
    /// (kind, name).
    async fn list_regulatory(&self) -> Result<Vec<RegulatoryNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Every regulatory reference edge, read back uniformly across the
    /// four edge families (`RegulatoryReference`/`HasRuleSource`/
    /// `CorpusInBody`/`DerivesFrom`), ordered by (owner, target).
    async fn list_regulatory_references(&self) -> Result<Vec<RegulatoryRefRecord>, GraphError> {
        Ok(Vec::new())
    }

    // ── Computation plane query methods (issue #263) ───────────────────

    /// Every FunctionNode in the graph, ordered by (domain, name).
    async fn list_functions(&self) -> Result<Vec<FunctionNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Every `FunctionExtends` edge as `(child, parent)` pairs (issue
    /// #263), ordered by child name.
    async fn list_function_extends(&self) -> Result<Vec<(String, String)>, GraphError> {
        Ok(Vec::new())
    }

    // ── Rule plane query methods (issue #264) ──────────────────────────

    /// Every RuleNode in the graph, ordered by (domain, name).
    async fn list_rules(&self) -> Result<Vec<RuleNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Every `RuleAppliesTo` edge as `(rule name, schema title)` pairs
    /// (issue #264), ordered by rule name.
    async fn list_rule_applies_to(&self) -> Result<Vec<(String, String)>, GraphError> {
        Ok(Vec::new())
    }

    /// Every rule-source `RuleReference` binding (issue #264), ordered by
    /// (rule source, schema title, attribute).
    async fn list_rule_references(&self) -> Result<Vec<RuleRefRecord>, GraphError> {
        Ok(Vec::new())
    }

    // ── Namespace plane query methods (issue #267; default: no data) ──

    /// Every NamespaceNode in the graph, ordered by `fqn` (stable).
    async fn list_namespaces(&self) -> Result<Vec<NamespaceNode>, GraphError> {
        Ok(Vec::new())
    }

    /// Schemas linked (via `InNamespace` edges) to the namespace `fqn`.
    /// With `recursive = true` the namespace's descendants (through
    /// `NamespaceParent` edges) are included. Ordered by `schema_id`.
    async fn list_schemas_by_namespace(
        &self,
        _fqn: &str,
        _recursive: bool,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        Ok(Vec::new())
    }

    /// The `NamespaceImports` edges originating at `fqn`, ordered by
    /// (target fqn, alias).
    async fn get_namespace_imports(&self, _fqn: &str) -> Result<Vec<NamespaceImport>, GraphError> {
        Ok(Vec::new())
    }

    /// Deterministic topological order of all namespaces by their imports
    /// (imported before importer; lexicographic tie-break on fqn). A cycle
    /// is an error naming the cycle members.
    async fn namespace_generation_order(&self) -> Result<Vec<String>, GraphError> {
        Ok(Vec::new())
    }
}

/// Check if a VO (value object) schema extends an entity through its allOf
/// composition chain. Traverses: VO → its allOf parents → schemas that extend
/// those parents → first entity found (excluding the VO itself).
///
/// Example: PersonLegalType (VO) allOf → [PersonBaseType, PersonLegalInclusion].
/// PersonType (entity) also allOf → PersonBaseType and PersonLegalInclusion.
/// `find_entity_extended_by_vo(db, "PersonLegalType")` returns PersonType.
///
/// Entities that extend the VO **directly** take priority: the allOf-parent walk
/// is ambiguous for shared base VOs (e.g. EventBaseType allOf → PersonBaseType,
/// which nearly every entity extends), so it can resolve to an unrelated entity.
pub async fn find_entity_extended_by_vo(
    db: &dyn GraphQuerier,
    vo_title: &str,
) -> Result<Option<SchemaNode>, GraphError> {
    // Tier 1: entities that extend the VO itself.
    if let Ok(extenders) = db.get_schemas_that_extend(vo_title).await {
        for extender in &extenders {
            if extender.title != vo_title
                && extender.is_entity
                && !extender.pg_table_name.is_empty()
            {
                if let Some(auth) = db.get_schema_by_id(&extender.schema_id).await? {
                    return Ok(Some(auth));
                }
                return Ok(Some(extender.clone()));
            }
        }
    }

    // Tier 2: fall back to the allOf-parent walk.
    let allof_targets = db.get_allof_targets(vo_title).await?;
    for parent_def in &allof_targets {
        if let Ok(extenders) = db.get_schemas_that_extend(parent_def).await {
            for extender in &extenders {
                if extender.title != vo_title
                    && extender.is_entity
                    && !extender.pg_table_name.is_empty()
                {
                    if let Some(auth) = db.get_schema_by_id(&extender.schema_id).await? {
                        return Ok(Some(auth));
                    }
                    return Ok(Some(extender.clone()));
                }
            }
        }
    }
    Ok(None)
}
