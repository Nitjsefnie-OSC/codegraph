use crate::types::*;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

mod builder;
mod ingest;
mod querier;

pub use builder::MockEngineBuilder;

type DataFlowKey = (String, String, Option<String>, Option<String>);

pub struct MockEngine {
    schemas: Mutex<HashMap<String, SchemaNode>>,
    properties: Mutex<HashMap<String, Vec<PropertyNode>>>,
    codelists: Mutex<HashMap<String, CodeList>>,
    enum_values: Mutex<HashMap<String, Vec<EnumValue>>>,
    trees: Mutex<HashMap<String, CompositionTree>>,
    composite_ranges: Mutex<HashMap<String, CompositeRange>>,
    consumed_fields: Mutex<HashMap<String, Vec<(PropertyNode, String)>>>,
    /// Maps (property_name, schema_title) -> target SchemaNode for $ref resolution
    ref_targets: Mutex<HashMap<(String, String), SchemaNode>>,
    parent_candidates: Mutex<Vec<ParentCandidate>>,
    extends_map: Mutex<HashMap<String, Vec<SchemaNode>>>,
    allof_targets: Mutex<HashMap<String, Vec<String>>>,
    view_containers: Mutex<HashMap<String, ViewContainerNode>>,
    module_definitions: Mutex<HashMap<String, ModuleDefinitionNode>>,
    view_components: Mutex<HashMap<String, ViewComponentNode>>,
    events: Mutex<HashMap<String, EventNode>>,
    action_nodes: Mutex<HashMap<String, ActionNode>>,
    parameter_definitions: Mutex<HashMap<String, ParameterDefinitionNode>>,
    /// Written by the builder; no querier reads it yet.
    #[allow(dead_code)]
    data_bindings: Mutex<HashMap<String, DataBindingNode>>,
    view_container_components: Mutex<HashMap<String, Vec<String>>>,
    /// Nested ViewContainers per parent, from ContainsViewContainer edges.
    /// Kept separate from [`Self::view_container_components`] so container
    /// children are never returned as components.
    view_container_children: Mutex<HashMap<String, Vec<String>>>,
    events_by_parent: Mutex<HashMap<String, Vec<String>>>,
    navigation_flows: Mutex<Vec<(String, String, Option<String>)>>,
    params_by_parent: Mutex<HashMap<String, Vec<String>>>,
    action_triggers: Mutex<Vec<(String, String)>>,
    data_flows: Mutex<Vec<DataFlowKey>>,
    data_binding_edges: Mutex<Vec<(String, String)>>,
    binding_entity_edges: Mutex<Vec<(String, String)>>,
    binding_property_edges: Mutex<Vec<(String, String)>>,
    op_interaction: Mutex<HashMap<String, String>>,
    interaction_endpoint: Mutex<HashMap<String, String>>,
    resource_operations: Mutex<HashMap<String, Vec<String>>>,
    atproto_namespaces: Mutex<HashMap<String, AtprotoNamespaceNode>>,
    /// Graph-wide namespaces keyed by fqn (issue #267).
    type_namespaces: Mutex<HashMap<String, NamespaceNode>>,
    /// NamespaceImports edges (issue #267).
    namespace_imports: Mutex<Vec<NamespaceImport>>,
    /// InNamespace edges: schema_id → namespace fqn (issue #267).
    schema_in_namespace: Mutex<HashMap<String, String>>,
    /// NamespaceParent edges: (child fqn, parent fqn) (issue #267).
    namespace_parent_edges: Mutex<Vec<(String, String)>>,
    lexicons: Mutex<HashMap<String, LexiconNode>>,
    collections: Mutex<HashMap<String, CollectionNode>>,
    repositories: Mutex<HashMap<String, RepositoryNode>>,
    /// Maps schema_title -> lexicon nsid for get_lexicon_by_schema lookups.
    schema_lexicons: Mutex<HashMap<String, String>>,
    api_resources: Mutex<HashMap<String, ApiResourceNode>>,
    api_operations: Mutex<HashMap<String, ApiOperationNode>>,
    interactions: Mutex<HashMap<String, InteractionNode>>,
    http_endpoints: Mutex<HashMap<String, HttpEndpointNode>>,
    pipelines: Mutex<HashMap<String, PipelineNode>>,
    error_definitions: Mutex<HashMap<String, ErrorDefinitionNode>>,
    permissions: Mutex<HashMap<String, PermissionNode>>,
    policies: Mutex<HashMap<String, PolicyNode>>,
    relationships: Mutex<HashMap<String, RelationshipNode>>,
    security_identities: Mutex<HashMap<String, SecurityIdentityNode>>,
    memberships: Mutex<Vec<MembershipNode>>,
    tenants: Mutex<HashMap<String, TenantNode>>,
    actors: Mutex<HashMap<String, ActorNode>>,
    capabilities: Mutex<HashMap<String, CapabilityNode>>,
    grants: Mutex<Vec<GrantEdge>>,
    actor_policy: Mutex<Option<ActorPolicyNode>>,
    conditions: Mutex<Vec<ConditionNode>>,
    regulatory: Mutex<Vec<RegulatoryNode>>,
    regulatory_refs: Mutex<Vec<RegulatoryRefRecord>>,
    functions: Mutex<Vec<FunctionNode>>,
    /// `(child, parent)` FunctionExtends edges (resolved at ingest time).
    function_extends: Mutex<Vec<(String, String)>>,
    rules: Mutex<Vec<RuleNode>>,
    /// `(rule name, schema title)` RuleAppliesTo edges (resolved at ingest
    /// time).
    rule_applies_to: Mutex<Vec<(String, String)>>,
    /// RuleReference bindings from rule-source classes (resolved at ingest
    /// time).
    rule_refs: Mutex<Vec<RuleRefRecord>>,
    /// Ingested `.ddd` design models (issue #449).
    ddd_models: Mutex<Vec<DddModelGraph>>,
    /// Ingested `.evt` event-contract models (issue #454).
    evt_models: Mutex<Vec<EvtModelGraph>>,
    /// Ingested `.deploy` deployment models (rexlang #49).
    deploy_models: Mutex<Vec<DeployModelGraph>>,
    start_time: Instant,
}

impl MockEngine {
    pub fn new() -> Self {
        Self {
            schemas: Mutex::new(HashMap::new()),
            properties: Mutex::new(HashMap::new()),
            codelists: Mutex::new(HashMap::new()),
            enum_values: Mutex::new(HashMap::new()),
            trees: Mutex::new(HashMap::new()),
            composite_ranges: Mutex::new(HashMap::new()),
            consumed_fields: Mutex::new(HashMap::new()),
            ref_targets: Mutex::new(HashMap::new()),
            parent_candidates: Mutex::new(Vec::new()),
            extends_map: Mutex::new(HashMap::new()),
            allof_targets: Mutex::new(HashMap::new()),
            view_containers: Mutex::new(HashMap::new()),
            module_definitions: Mutex::new(HashMap::new()),
            view_components: Mutex::new(HashMap::new()),
            events: Mutex::new(HashMap::new()),
            action_nodes: Mutex::new(HashMap::new()),
            parameter_definitions: Mutex::new(HashMap::new()),
            data_bindings: Mutex::new(HashMap::new()),
            view_container_components: Mutex::new(HashMap::new()),
            view_container_children: Mutex::new(HashMap::new()),
            events_by_parent: Mutex::new(HashMap::new()),
            navigation_flows: Mutex::new(Vec::new()),
            params_by_parent: Mutex::new(HashMap::new()),
            action_triggers: Mutex::new(Vec::new()),
            data_flows: Mutex::new(Vec::new()),
            data_binding_edges: Mutex::new(Vec::new()),
            binding_entity_edges: Mutex::new(Vec::new()),
            binding_property_edges: Mutex::new(Vec::new()),
            op_interaction: Mutex::new(HashMap::new()),
            interaction_endpoint: Mutex::new(HashMap::new()),
            resource_operations: Mutex::new(HashMap::new()),
            atproto_namespaces: Mutex::new(HashMap::new()),
            type_namespaces: Mutex::new(HashMap::new()),
            namespace_imports: Mutex::new(Vec::new()),
            schema_in_namespace: Mutex::new(HashMap::new()),
            namespace_parent_edges: Mutex::new(Vec::new()),
            lexicons: Mutex::new(HashMap::new()),
            collections: Mutex::new(HashMap::new()),
            repositories: Mutex::new(HashMap::new()),
            schema_lexicons: Mutex::new(HashMap::new()),
            api_resources: Mutex::new(HashMap::new()),
            api_operations: Mutex::new(HashMap::new()),
            interactions: Mutex::new(HashMap::new()),
            http_endpoints: Mutex::new(HashMap::new()),
            pipelines: Mutex::new(HashMap::new()),
            error_definitions: Mutex::new(HashMap::new()),
            permissions: Mutex::new(HashMap::new()),
            policies: Mutex::new(HashMap::new()),
            relationships: Mutex::new(HashMap::new()),
            security_identities: Mutex::new(HashMap::new()),
            memberships: Mutex::new(Vec::new()),
            tenants: Mutex::new(HashMap::new()),
            actors: Mutex::new(HashMap::new()),
            capabilities: Mutex::new(HashMap::new()),
            grants: Mutex::new(Vec::new()),
            actor_policy: Mutex::new(None),
            conditions: Mutex::new(Vec::new()),
            regulatory: Mutex::new(Vec::new()),
            regulatory_refs: Mutex::new(Vec::new()),
            functions: Mutex::new(Vec::new()),
            function_extends: Mutex::new(Vec::new()),
            rules: Mutex::new(Vec::new()),
            rule_applies_to: Mutex::new(Vec::new()),
            rule_refs: Mutex::new(Vec::new()),
            ddd_models: Mutex::new(Vec::new()),
            evt_models: Mutex::new(Vec::new()),
            deploy_models: Mutex::new(Vec::new()),
            start_time: Instant::now(),
        }
    }

    pub fn add_lexicon_mapping(&self, schema_title: &str, lexicon_nsid: &str) {
        self.schema_lexicons
            .lock()
            .unwrap()
            .insert(schema_title.to_string(), lexicon_nsid.to_string());
    }

    /// The namespace `fqn` plus, when `recursive`, all its descendant
    /// namespaces (via NamespaceParent edges) — the set of namespaces whose
    /// schemas `list_schemas_by_namespace` returns.
    fn namespace_closure(&self, fqn: &str, recursive: bool) -> Vec<String> {
        let mut wanted: Vec<String> = vec![fqn.to_string()];
        if recursive {
            let edges = self.namespace_parent_edges.lock().unwrap();
            // Walk down from fqn: children are edges (child → parent).
            wanted.extend(descendants(fqn, &edges));
        }
        wanted
    }

    pub fn builder() -> MockEngineBuilder {
        MockEngineBuilder::default()
    }
}

impl Default for MockEngine {
    fn default() -> Self {
        Self::new()
    }
}
