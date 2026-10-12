use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;

use crate::error::GraphError;
use crate::traits::GraphQuerier;
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
    ViewComponentNode, ViewContainerNode, ref_target_candidate_title,
};

/// Cached codelist-for-property value: `Option<(CodeList, render_as)>`.
type CodelistPropertyVal = Option<(CodeList, String)>;

/// A caching wrapper around a `&dyn GraphQuerier`.
///
/// Caches the results of frequently-called query methods (get_schema,
/// get_properties, etc.) so that multiple generators querying the same
/// entity do not re-execute identical graph queries.
///
/// The cache is populated lazily on first access and lives for the
/// duration of the wrapper (typically one full generation run).
pub struct CachingQuerier<'a> {
    inner: &'a dyn GraphQuerier,
    schema_cache: RwLock<HashMap<String, Option<SchemaNode>>>,
    schema_id_cache: RwLock<HashMap<String, Option<SchemaNode>>>,
    properties_cache: RwLock<HashMap<String, Vec<PropertyNode>>>,
    child_schemas_cache: RwLock<HashMap<String, Vec<SchemaNode>>>,
    composite_columns_cache: RwLock<HashMap<(String, String), Vec<CompositeColumn>>>,
    structured_sub_fields_cache: RwLock<HashMap<String, Vec<StructuredSubField>>>,
    composite_range_cache: RwLock<HashMap<String, Option<CompositeRange>>>,
    consumed_fields_cache: RwLock<HashMap<String, Vec<(PropertyNode, String)>>>,
    codelist_for_property_cache: RwLock<HashMap<(String, String), CodelistPropertyVal>>,
    codelist_cache: RwLock<HashMap<String, Option<CodeList>>>,
    enum_values_cache: RwLock<HashMap<String, Vec<EnumValue>>>,
    extensions_cache: RwLock<HashMap<String, Vec<Extension>>>,
    composition_tree_cache: RwLock<HashMap<String, CompositionTree>>,
    allof_targets_cache: RwLock<HashMap<String, Vec<String>>>,
    schemas_that_extend_cache: RwLock<HashMap<String, Vec<SchemaNode>>>,
    referencing_cache: RwLock<HashMap<String, Vec<String>>>,
    referenced_cache: RwLock<HashMap<String, Vec<SchemaNode>>>,
    property_ref_target_cache: RwLock<HashMap<(String, String), Option<SchemaNode>>>,
    property_ref_target_by_id_cache: RwLock<HashMap<(String, String), Option<SchemaNode>>>,
    array_item_schema_cache: RwLock<HashMap<(String, String), Option<SchemaNode>>>,
    api_resources_cache: RwLock<Option<Vec<ApiResourceNode>>>,
    api_operations_cache: RwLock<HashMap<String, Vec<ApiOperationNode>>>,
    policies_cache: RwLock<HashMap<String, Vec<PolicyNode>>>,
    relationships_cache: RwLock<HashMap<String, Vec<RelationshipNode>>>,
    all_policies_cache: RwLock<Option<Vec<PolicyNode>>>,
    all_relationships_cache: RwLock<Option<Vec<RelationshipNode>>>,
    ddd_models_cache: RwLock<Option<Vec<DddModelGraph>>>,
    evt_models_cache: RwLock<Option<Vec<EvtModelGraph>>>,
    deploy_models_cache: RwLock<Option<Vec<DeployModelGraph>>>,
    /// Populated for future tenant-scoped queries; not read yet.
    #[allow(dead_code)]
    tenants_cache: RwLock<HashMap<String, TenantNode>>,
}

impl<'a> CachingQuerier<'a> {
    pub fn new(inner: &'a dyn GraphQuerier) -> Self {
        Self {
            inner,
            schema_cache: RwLock::new(HashMap::new()),
            schema_id_cache: RwLock::new(HashMap::new()),
            properties_cache: RwLock::new(HashMap::new()),
            child_schemas_cache: RwLock::new(HashMap::new()),
            composite_columns_cache: RwLock::new(HashMap::new()),
            structured_sub_fields_cache: RwLock::new(HashMap::new()),
            composite_range_cache: RwLock::new(HashMap::new()),
            consumed_fields_cache: RwLock::new(HashMap::new()),
            codelist_for_property_cache: RwLock::new(HashMap::new()),
            codelist_cache: RwLock::new(HashMap::new()),
            enum_values_cache: RwLock::new(HashMap::new()),
            extensions_cache: RwLock::new(HashMap::new()),
            composition_tree_cache: RwLock::new(HashMap::new()),
            allof_targets_cache: RwLock::new(HashMap::new()),
            schemas_that_extend_cache: RwLock::new(HashMap::new()),
            referencing_cache: RwLock::new(HashMap::new()),
            referenced_cache: RwLock::new(HashMap::new()),
            property_ref_target_cache: RwLock::new(HashMap::new()),
            property_ref_target_by_id_cache: RwLock::new(HashMap::new()),
            array_item_schema_cache: RwLock::new(HashMap::new()),
            api_resources_cache: RwLock::new(None),
            api_operations_cache: RwLock::new(HashMap::new()),
            policies_cache: RwLock::new(HashMap::new()),
            relationships_cache: RwLock::new(HashMap::new()),
            all_policies_cache: RwLock::new(None),
            all_relationships_cache: RwLock::new(None),
            ddd_models_cache: RwLock::new(None),
            evt_models_cache: RwLock::new(None),
            deploy_models_cache: RwLock::new(None),
            tenants_cache: RwLock::new(HashMap::new()),
        }
    }

    /// Pre-warm the cache by bulk-loading all schemas, properties, and reference
    /// edges in a small number of graph queries. This avoids hundreds of
    /// individual queries during generation.
    pub async fn warm(&self) -> Result<(), GraphError> {
        // 1. Bulk-load all schemas → schema_cache
        let all_schemas = self.inner.list_schemas(None).await?;
        {
            let mut cache = self.schema_cache.write().unwrap();
            for schema in &all_schemas {
                cache.insert(schema.title.clone(), Some(schema.clone()));
            }
        }

        // 1b. Build schema_id_cache from all_schemas
        {
            let mut cache = self.schema_id_cache.write().unwrap();
            for schema in &all_schemas {
                cache.insert(schema.schema_id.clone(), Some(schema.clone()));
            }
        }

        // 2. Bulk-load all properties → properties_cache
        let all_props = self.inner.list_all_properties().await?;
        let schema_by_title: HashMap<&str, &SchemaNode> =
            all_schemas.iter().map(|s| (s.title.as_str(), s)).collect();
        {
            let mut cache = self.properties_cache.write().unwrap();
            // Insert empty vecs for schemas that have no properties
            for schema in &all_schemas {
                cache.entry(schema.title.clone()).or_default();
            }
            for (title, props) in &all_props {
                cache.insert(title.clone(), props.clone());
            }
        }

        // 3. Bulk-load all schema references → referenced_cache
        let all_refs = self.inner.list_all_schema_references().await?;
        {
            let mut cache = self.referenced_cache.write().unwrap();
            // Build per-source map of SchemaNodes
            let mut ref_map: HashMap<String, Vec<SchemaNode>> = HashMap::new();
            for (src, tgt) in &all_refs {
                if let Some(schema) = schema_by_title.get(tgt.as_str()) {
                    ref_map
                        .entry(src.clone())
                        .or_default()
                        .push((*schema).clone());
                }
            }
            for (src, targets) in ref_map {
                cache.insert(src, targets);
            }
            // Also pre-populate empty entries for schemas with no refs
            for schema in &all_schemas {
                cache.entry(schema.title.clone()).or_default();
            }
        }

        // 4. Build child_schemas_cache: inline #/$defs children (the
        //    `parent_schema` back-pointer) PLUS derived refers children
        //    (array-of-entity-ref properties whose ref target is an entity,
        //    issue #312) — mirroring `get_child_schemas`' two routes so the
        //    pre-warmed cache agrees with the lazy inner calls.
        {
            let schema_by_title: HashMap<&str, &SchemaNode> =
                all_schemas.iter().map(|s| (s.title.as_str(), s)).collect();
            let mut cache = self.child_schemas_cache.write().unwrap();
            let mut children_map: HashMap<String, Vec<SchemaNode>> = HashMap::new();
            for schema in &all_schemas {
                if let Some(ref parent) = schema.parent_schema {
                    children_map
                        .entry(parent.clone())
                        .or_default()
                        .push(schema.clone());
                }
            }
            for (title, props) in &all_props {
                for prop in props {
                    if !prop.is_array {
                        continue;
                    }
                    let Some(ref target) = prop.ref_target else {
                        continue;
                    };
                    let candidate = ref_target_candidate_title(target);
                    if candidate == title {
                        continue;
                    }
                    if let Some(child) = schema_by_title.get(candidate)
                        && child.is_entity
                    {
                        let entry = children_map.entry(title.clone()).or_default();
                        if !entry.iter().any(|c| c.title == child.title) {
                            entry.push((*child).clone());
                        }
                    }
                }
            }
            for (parent, mut children) in children_map {
                children.sort_by(|a, b| a.title.cmp(&b.title));
                cache.insert(parent, children);
            }
        }

        // 5. Build referencing_cache (reverse of referenced)
        {
            let mut cache = self.referencing_cache.write().unwrap();
            let mut rev_map: HashMap<String, Vec<String>> = HashMap::new();
            for (src, tgt) in &all_refs {
                rev_map.entry(tgt.clone()).or_default().push(src.clone());
            }
            for (tgt, sources) in rev_map {
                cache.insert(tgt, sources);
            }
        }

        // 6. Bulk-load all policies
        {
            let policies = self.inner.list_all_policies().await?;
            *self.all_policies_cache.write().unwrap() = Some(policies);
        }

        // 7. Bulk-load all relationships
        {
            let rels = self.inner.list_all_relationships().await?;
            *self.all_relationships_cache.write().unwrap() = Some(rels);
        }

        Ok(())
    }
}

/// Helper macro to implement a cached method with a single string key.
macro_rules! cached_single {
    ($self:ident, $cache:ident, $key:expr, $inner_call:expr) => {{
        {
            let cache = $self.$cache.read().unwrap();
            if let Some(val) = cache.get($key) {
                return Ok(val.clone());
            }
        }
        let result = $inner_call.await?;
        {
            let mut cache = $self.$cache.write().unwrap();
            cache.insert($key.to_string(), result.clone());
        }
        Ok(result)
    }};
}

/// Helper macro for cached methods with a two-string key.
macro_rules! cached_pair {
    ($self:ident, $cache:ident, $k1:expr, $k2:expr, $inner_call:expr) => {{
        let key = ($k1.to_string(), $k2.to_string());
        {
            let cache = $self.$cache.read().unwrap();
            if let Some(val) = cache.get(&key) {
                return Ok(val.clone());
            }
        }
        let result = $inner_call.await?;
        {
            let mut cache = $self.$cache.write().unwrap();
            cache.insert(key, result.clone());
        }
        Ok(result)
    }};
}

#[async_trait]
impl GraphQuerier for CachingQuerier<'_> {
    async fn get_schema(&self, title: &str) -> Result<Option<SchemaNode>, GraphError> {
        cached_single!(self, schema_cache, title, self.inner.get_schema(title))
    }

    async fn get_schema_by_id(&self, schema_id: &str) -> Result<Option<SchemaNode>, GraphError> {
        {
            let cache = self.schema_id_cache.read().unwrap();
            if let Some(val) = cache.get(schema_id) {
                return Ok(val.clone());
            }
        }
        let result = self.inner.get_schema_by_id(schema_id).await?;
        {
            let mut cache = self.schema_id_cache.write().unwrap();
            cache.insert(schema_id.to_string(), result.clone());
        }
        Ok(result)
    }

    async fn get_schema_in_domain(
        &self,
        title: &str,
        domain: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        self.inner.get_schema_in_domain(title, domain).await
    }

    async fn list_schemas(&self, domain: Option<&str>) -> Result<Vec<SchemaNode>, GraphError> {
        self.inner.list_schemas(domain).await
    }

    async fn get_properties(&self, schema_title: &str) -> Result<Vec<PropertyNode>, GraphError> {
        cached_single!(
            self,
            properties_cache,
            schema_title,
            self.inner.get_properties(schema_title)
        )
    }

    async fn get_child_schemas(&self, schema_title: &str) -> Result<Vec<SchemaNode>, GraphError> {
        cached_single!(
            self,
            child_schemas_cache,
            schema_title,
            self.inner.get_child_schemas(schema_title)
        )
    }

    async fn get_classification_data(&self) -> Result<Vec<SchemaClassificationData>, GraphError> {
        self.inner.get_classification_data().await
    }

    async fn get_entity_names(&self) -> Result<Vec<String>, GraphError> {
        self.inner.get_entity_names().await
    }

    async fn get_entity_schema_map(&self) -> Result<HashMap<String, String>, GraphError> {
        self.inner.get_entity_schema_map().await
    }

    async fn get_value_object_schemas(&self) -> Result<Vec<SchemaNode>, GraphError> {
        self.inner.get_value_object_schemas().await
    }

    async fn get_parent_candidates(&self) -> Result<Vec<ParentCandidate>, GraphError> {
        self.inner.get_parent_candidates().await
    }

    async fn get_codelist(&self, name: &str) -> Result<Option<CodeList>, GraphError> {
        cached_single!(self, codelist_cache, name, self.inner.get_codelist(name))
    }

    async fn list_codelists(&self) -> Result<Vec<CodeList>, GraphError> {
        self.inner.list_codelists().await
    }

    async fn get_enum_values(&self, codelist_name: &str) -> Result<Vec<EnumValue>, GraphError> {
        cached_single!(
            self,
            enum_values_cache,
            codelist_name,
            self.inner.get_enum_values(codelist_name)
        )
    }

    async fn get_composite_columns(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Vec<CompositeColumn>, GraphError> {
        cached_pair!(
            self,
            composite_columns_cache,
            property_name,
            schema_title,
            self.inner
                .get_composite_columns(property_name, schema_title)
        )
    }

    async fn get_structured_sub_fields(
        &self,
        schema_title: &str,
    ) -> Result<Vec<StructuredSubField>, GraphError> {
        cached_single!(
            self,
            structured_sub_fields_cache,
            schema_title,
            self.inner.get_structured_sub_fields(schema_title)
        )
    }

    async fn get_composite_range(
        &self,
        schema_title: &str,
    ) -> Result<Option<CompositeRange>, GraphError> {
        cached_single!(
            self,
            composite_range_cache,
            schema_title,
            self.inner.get_composite_range(schema_title)
        )
    }

    async fn get_consumed_fields(
        &self,
        schema_title: &str,
    ) -> Result<Vec<(PropertyNode, String)>, GraphError> {
        cached_single!(
            self,
            consumed_fields_cache,
            schema_title,
            self.inner.get_consumed_fields(schema_title)
        )
    }

    async fn get_codelist_for_property(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Option<(CodeList, String)>, GraphError> {
        cached_pair!(
            self,
            codelist_for_property_cache,
            property_name,
            schema_title,
            self.inner
                .get_codelist_for_property(property_name, schema_title)
        )
    }

    async fn get_required_extensions(
        &self,
        schema_title: &str,
    ) -> Result<Vec<Extension>, GraphError> {
        cached_single!(
            self,
            extensions_cache,
            schema_title,
            self.inner.get_required_extensions(schema_title)
        )
    }

    async fn get_composition_tree(
        &self,
        schema_title: &str,
    ) -> Result<CompositionTree, GraphError> {
        cached_single!(
            self,
            composition_tree_cache,
            schema_title,
            self.inner.get_composition_tree(schema_title)
        )
    }

    async fn get_allof_targets(&self, schema_title: &str) -> Result<Vec<String>, GraphError> {
        cached_single!(
            self,
            allof_targets_cache,
            schema_title,
            self.inner.get_allof_targets(schema_title)
        )
    }

    async fn get_referencing_schemas(&self, schema_title: &str) -> Result<Vec<String>, GraphError> {
        cached_single!(
            self,
            referencing_cache,
            schema_title,
            self.inner.get_referencing_schemas(schema_title)
        )
    }

    async fn get_schemas_that_extend(
        &self,
        parent_title: &str,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        cached_single!(
            self,
            schemas_that_extend_cache,
            parent_title,
            self.inner.get_schemas_that_extend(parent_title)
        )
    }

    async fn get_referenced_schemas(
        &self,
        schema_title: &str,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        cached_single!(
            self,
            referenced_cache,
            schema_title,
            self.inner.get_referenced_schemas(schema_title)
        )
    }

    async fn get_property_ref_target(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        cached_pair!(
            self,
            property_ref_target_cache,
            property_name,
            schema_title,
            self.inner
                .get_property_ref_target(property_name, schema_title)
        )
    }

    async fn get_property_ref_target_by_id(
        &self,
        property_name: &str,
        schema_id: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        cached_pair!(
            self,
            property_ref_target_by_id_cache,
            property_name,
            schema_id,
            self.inner
                .get_property_ref_target_by_id(property_name, schema_id)
        )
    }

    async fn get_properties_by_schema_id(
        &self,
        schema_id: &str,
    ) -> Result<Vec<PropertyNode>, GraphError> {
        self.inner.get_properties_by_schema_id(schema_id).await
    }

    async fn get_array_item_schema(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        cached_pair!(
            self,
            array_item_schema_cache,
            property_name,
            schema_title,
            self.inner
                .get_array_item_schema(property_name, schema_title)
        )
    }

    async fn get_generation_order(&self) -> Result<Vec<String>, GraphError> {
        self.inner.get_generation_order().await
    }

    async fn list_all_schema_references(&self) -> Result<Vec<(String, String)>, GraphError> {
        self.inner.list_all_schema_references().await
    }

    async fn list_property_target_titles(
        &self,
    ) -> Result<std::collections::HashSet<String>, GraphError> {
        self.inner.list_property_target_titles().await
    }

    async fn list_all_properties(&self) -> Result<HashMap<String, Vec<PropertyNode>>, GraphError> {
        self.inner.list_all_properties().await
    }

    // ── IFML query delegation ──────────────────────────────────────────

    async fn get_ifml_view_containers(&self) -> Result<Vec<ViewContainerNode>, GraphError> {
        self.inner.get_ifml_view_containers().await
    }

    async fn get_ifml_modules(&self) -> Result<Vec<ModuleDefinitionNode>, GraphError> {
        self.inner.get_ifml_modules().await
    }

    async fn get_ifml_view_components(
        &self,
        container_name: &str,
    ) -> Result<Vec<ViewComponentNode>, GraphError> {
        self.inner.get_ifml_view_components(container_name).await
    }

    async fn get_ifml_container_children(
        &self,
        parent: &str,
    ) -> Result<Vec<ViewContainerNode>, GraphError> {
        self.inner.get_ifml_container_children(parent).await
    }

    async fn get_ifml_events(&self, parent_id: &str) -> Result<Vec<EventNode>, GraphError> {
        self.inner.get_ifml_events(parent_id).await
    }

    async fn get_ifml_navigation_flows(&self) -> Result<Vec<NavigationFlowRecord>, GraphError> {
        self.inner.get_ifml_navigation_flows().await
    }

    async fn get_ifml_data_flows(
        &self,
    ) -> Result<Vec<(String, String, Option<String>, Option<String>)>, GraphError> {
        self.inner.get_ifml_data_flows().await
    }

    async fn get_ifml_actions(&self) -> Result<Vec<ActionNode>, GraphError> {
        self.inner.get_ifml_actions().await
    }

    async fn get_ifml_action_triggers(&self) -> Result<Vec<(String, String)>, GraphError> {
        self.inner.get_ifml_action_triggers().await
    }

    async fn get_ifml_parameters(&self) -> Result<Vec<ParameterDefinitionNode>, GraphError> {
        self.inner.get_ifml_parameters().await
    }

    async fn get_parameters_for_view(
        &self,
        container_name: &str,
    ) -> Result<Vec<ParameterDefinitionNode>, GraphError> {
        self.inner.get_parameters_for_view(container_name).await
    }

    async fn get_data_bindings(&self) -> Result<Vec<DataBindingResolution>, GraphError> {
        self.inner.get_data_bindings().await
    }

    // ── Constraint plane query delegation (issue #261) ─────────────────

    async fn get_conditions_for_schema(
        &self,
        schema_title: &str,
    ) -> Result<Vec<ConditionNode>, GraphError> {
        self.inner.get_conditions_for_schema(schema_title).await
    }

    async fn list_conditions(&self) -> Result<Vec<ConditionNode>, GraphError> {
        self.inner.list_conditions().await
    }

    // ── Regulatory reference plane delegation (issue #265) ─────────────

    async fn list_regulatory(&self) -> Result<Vec<RegulatoryNode>, GraphError> {
        self.inner.list_regulatory().await
    }

    async fn list_regulatory_references(&self) -> Result<Vec<RegulatoryRefRecord>, GraphError> {
        self.inner.list_regulatory_references().await
    }

    // ── Computation plane delegation (issue #263) ──────────────────────

    async fn list_functions(&self) -> Result<Vec<FunctionNode>, GraphError> {
        self.inner.list_functions().await
    }

    async fn list_function_extends(&self) -> Result<Vec<(String, String)>, GraphError> {
        self.inner.list_function_extends().await
    }

    // ── Rule plane delegation (issue #264) ─────────────────────────────

    async fn list_rules(&self) -> Result<Vec<RuleNode>, GraphError> {
        self.inner.list_rules().await
    }

    async fn list_rule_applies_to(&self) -> Result<Vec<(String, String)>, GraphError> {
        self.inner.list_rule_applies_to().await
    }

    async fn list_rule_references(&self) -> Result<Vec<RuleRefRecord>, GraphError> {
        self.inner.list_rule_references().await
    }

    // ── AT Protocol query delegation ───────────────────────────────────

    async fn get_lexicons(&self, domain: &str) -> Result<Vec<LexiconNode>, GraphError> {
        self.inner.get_lexicons(domain).await
    }

    async fn get_lexicon_by_schema(
        &self,
        schema_title: &str,
    ) -> Result<Option<LexiconNode>, GraphError> {
        self.inner.get_lexicon_by_schema(schema_title).await
    }

    async fn get_collections(&self, domain: &str) -> Result<Vec<CollectionNode>, GraphError> {
        self.inner.get_collections(domain).await
    }

    async fn get_repositories(&self) -> Result<Vec<RepositoryNode>, GraphError> {
        self.inner.get_repositories().await
    }

    async fn get_atproto_namespaces(&self) -> Result<Vec<AtprotoNamespaceNode>, GraphError> {
        self.inner.get_atproto_namespaces().await
    }

    async fn get_lexicon_references(&self, nsid: &str) -> Result<Vec<LexiconNode>, GraphError> {
        self.inner.get_lexicon_references(nsid).await
    }

    // ── Namespace plane delegation (issue #267) ────────────────────────

    async fn list_namespaces(&self) -> Result<Vec<NamespaceNode>, GraphError> {
        self.inner.list_namespaces().await
    }

    async fn list_schemas_by_namespace(
        &self,
        fqn: &str,
        recursive: bool,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        self.inner.list_schemas_by_namespace(fqn, recursive).await
    }

    async fn get_namespace_imports(&self, fqn: &str) -> Result<Vec<NamespaceImport>, GraphError> {
        self.inner.get_namespace_imports(fqn).await
    }

    async fn namespace_generation_order(&self) -> Result<Vec<String>, GraphError> {
        self.inner.namespace_generation_order().await
    }

    // ── API metamodel query delegation ──────────────────────────────────

    async fn get_api_resources(&self) -> Result<Vec<ApiResourceNode>, GraphError> {
        if let Some(cached) = self.api_resources_cache.read().unwrap().as_ref() {
            return Ok(cached.clone());
        }
        let result = self.inner.get_api_resources().await?;
        self.api_resources_cache
            .write()
            .unwrap()
            .replace(result.clone());
        Ok(result)
    }

    async fn get_api_resource(&self, name: &str) -> Result<Option<ApiResourceNode>, GraphError> {
        self.inner.get_api_resource(name).await
    }

    async fn get_api_operation(&self, name: &str) -> Result<Option<ApiOperationNode>, GraphError> {
        self.inner.get_api_operation(name).await
    }

    async fn get_http_endpoint_for_operation(
        &self,
        operation_name: &str,
    ) -> Result<Option<HttpEndpointNode>, GraphError> {
        self.inner
            .get_http_endpoint_for_operation(operation_name)
            .await
    }

    async fn get_api_operations(
        &self,
        resource_name: &str,
    ) -> Result<Vec<ApiOperationNode>, GraphError> {
        if let Some(cached) = self.api_operations_cache.read().unwrap().get(resource_name) {
            return Ok(cached.clone());
        }
        let result = self.inner.get_api_operations(resource_name).await?;
        self.api_operations_cache
            .write()
            .unwrap()
            .insert(resource_name.to_string(), result.clone());
        Ok(result)
    }

    async fn get_interactions(
        &self,
        operation_name: &str,
    ) -> Result<Vec<InteractionNode>, GraphError> {
        self.inner.get_interactions(operation_name).await
    }

    async fn get_http_endpoints(&self) -> Result<Vec<HttpEndpointNode>, GraphError> {
        self.inner.get_http_endpoints().await
    }

    async fn get_error_definitions(&self) -> Result<Vec<ErrorDefinitionNode>, GraphError> {
        self.inner.get_error_definitions().await
    }

    async fn get_permissions(&self) -> Result<Vec<PermissionNode>, GraphError> {
        self.inner.get_permissions().await
    }

    async fn get_pipelines(&self) -> Result<Vec<PipelineNode>, GraphError> {
        self.inner.get_pipelines().await
    }

    async fn get_pipeline_for_endpoint(
        &self,
        endpoint_path: &str,
    ) -> Result<Option<PipelineNode>, GraphError> {
        self.inner.get_pipeline_for_endpoint(endpoint_path).await
    }

    // ── Persistence metamodel queries ──────────────────────────────────

    async fn get_policies_for_schema(
        &self,
        schema_title: &str,
    ) -> Result<Vec<PolicyNode>, GraphError> {
        {
            let cache = self.policies_cache.read().unwrap();
            if let Some(policies) = cache.get(schema_title) {
                return Ok(policies.clone());
            }
        }
        let policies = self.inner.get_policies_for_schema(schema_title).await?;
        self.policies_cache
            .write()
            .unwrap()
            .insert(schema_title.to_string(), policies.clone());
        Ok(policies)
    }

    async fn get_relationships_for_schema(
        &self,
        schema_title: &str,
    ) -> Result<Vec<RelationshipNode>, GraphError> {
        {
            let cache = self.relationships_cache.read().unwrap();
            if let Some(rels) = cache.get(schema_title) {
                return Ok(rels.clone());
            }
        }
        let rels = self
            .inner
            .get_relationships_for_schema(schema_title)
            .await?;
        self.relationships_cache
            .write()
            .unwrap()
            .insert(schema_title.to_string(), rels.clone());
        Ok(rels)
    }

    async fn get_relationship_by_name(
        &self,
        name: &str,
    ) -> Result<Option<RelationshipNode>, GraphError> {
        self.inner.get_relationship_by_name(name).await
    }

    async fn list_all_policies(&self) -> Result<Vec<PolicyNode>, GraphError> {
        {
            let cache = self.all_policies_cache.read().unwrap();
            if let Some(policies) = cache.as_ref() {
                return Ok(policies.clone());
            }
        }
        let policies = self.inner.list_all_policies().await?;
        *self.all_policies_cache.write().unwrap() = Some(policies.clone());
        Ok(policies)
    }

    async fn list_all_relationships(&self) -> Result<Vec<RelationshipNode>, GraphError> {
        {
            let cache = self.all_relationships_cache.read().unwrap();
            if let Some(rels) = cache.as_ref() {
                return Ok(rels.clone());
            }
        }
        let rels = self.inner.list_all_relationships().await?;
        *self.all_relationships_cache.write().unwrap() = Some(rels.clone());
        Ok(rels)
    }

    // ── Security metamodel queries ─────────────────────────────────────

    async fn get_security_identity(
        &self,
        subject: &str,
    ) -> Result<Option<SecurityIdentityNode>, GraphError> {
        self.inner.get_security_identity(subject).await
    }

    async fn get_memberships_for_identity(
        &self,
        identity_name: &str,
    ) -> Result<Vec<MembershipNode>, GraphError> {
        self.inner.get_memberships_for_identity(identity_name).await
    }

    async fn get_tenant(&self, name: &str) -> Result<Option<TenantNode>, GraphError> {
        self.inner.get_tenant(name).await
    }

    async fn list_all_tenants(&self) -> Result<Vec<TenantNode>, GraphError> {
        self.inner.list_all_tenants().await
    }

    // ── Authorization metamodel queries ────────────────────────────────

    async fn get_actors(&self) -> Result<Vec<ActorNode>, GraphError> {
        self.inner.get_actors().await
    }

    async fn get_capabilities(&self) -> Result<Vec<CapabilityNode>, GraphError> {
        self.inner.get_capabilities().await
    }

    async fn get_grants(&self) -> Result<Vec<GrantEdge>, GraphError> {
        self.inner.get_grants().await
    }

    async fn get_actor_policy(&self) -> Result<Option<ActorPolicyNode>, GraphError> {
        self.inner.get_actor_policy().await
    }

    async fn effective_permits(&self, actor: &str) -> Result<Vec<Permit>, GraphError> {
        self.inner.effective_permits(actor).await
    }

    async fn get_ddd_models(&self) -> Result<Vec<DddModelGraph>, GraphError> {
        if let Some(models) = self.ddd_models_cache.read().unwrap().clone() {
            return Ok(models);
        }
        let models = self.inner.get_ddd_models().await?;
        *self.ddd_models_cache.write().unwrap() = Some(models.clone());
        Ok(models)
    }

    async fn get_evt_models(&self) -> Result<Vec<EvtModelGraph>, GraphError> {
        if let Some(models) = self.evt_models_cache.read().unwrap().clone() {
            return Ok(models);
        }
        let models = self.inner.get_evt_models().await?;
        *self.evt_models_cache.write().unwrap() = Some(models.clone());
        Ok(models)
    }

    async fn get_deploy_models(&self) -> Result<Vec<DeployModelGraph>, GraphError> {
        if let Some(models) = self.deploy_models_cache.read().unwrap().clone() {
            return Ok(models);
        }
        let models = self.inner.get_deploy_models().await?;
        *self.deploy_models_cache.write().unwrap() = Some(models.clone());
        Ok(models)
    }

    async fn composition_table_conflict(
        &self,
        schema_title: &str,
    ) -> Result<Option<(String, String)>, GraphError> {
        self.inner.composition_table_conflict(schema_title).await
    }

    // ── mox domain queries ─────────────────────────────────────────────

    async fn get_mox_vocabularies(&self) -> Result<Vec<MoxVocabularyNode>, GraphError> {
        self.inner.get_mox_vocabularies().await
    }

    async fn get_mox_operations(&self) -> Result<Vec<MoxOperationNode>, GraphError> {
        self.inner.get_mox_operations().await
    }

    async fn get_mox_derived_features(&self) -> Result<Vec<MoxDerivedFeatureNode>, GraphError> {
        self.inner.get_mox_derived_features().await
    }

    async fn get_mox_derived_features_for_schema(
        &self,
        schema_title: &str,
    ) -> Result<Vec<MoxDerivedFeatureNode>, GraphError> {
        self.inner
            .get_mox_derived_features_for_schema(schema_title)
            .await
    }
}
