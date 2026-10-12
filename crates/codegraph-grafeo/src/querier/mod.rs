mod api_model;
mod atproto;
mod composition;
mod ddd;
mod deploy;
mod evt;
mod governance;
mod ifml;
mod query;
mod schema;

use std::collections::HashMap;

use async_trait::async_trait;
use codegraph_core::error::GraphError;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::{
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
    ViewComponentNode, ViewContainerNode, descendants, topological_namespace_order,
    topological_order,
};

use self::query::{query_gql, query_gql_params, query_many, query_many_params};
use crate::conversions::{RowReader, row_to_property_node};
use crate::engine::GrafeoEngine;

/// The RETURN clause for all SchemaNode queries — keeps the 25 columns in one place.
pub(super) const SCHEMA_RETURN_COLS: &str = "\
    s.schema_id, s.title, s.description, \
    s.schema_type, s.classification, s.domain, s.namespace, s.rel_path, s.pg_type, s.rust_type, \
    s.sea_orm_type, s.rust_type_name, s.pg_table_name, s.api_path_segment, \
    s.parent_schema, s.is_entity, s.is_codelist, s.is_primitive_wrapper, \
    s.has_all_of, s.has_one_of, s.has_any_of, s.has_definitions, s.custom_annotations, \
    s.access, s.annotations";

/// The RETURN clause for all PropertyNode queries — keeps the columns in one place.
pub(super) const PROPERTY_RETURN_COLS: &str = "\
    p.name, p.prop_type, p.description, p.format, \
    p.is_required, p.is_nullable, p.is_id, p.is_array, p.pattern, \
    p.min_length, p.max_length, p.minimum, p.maximum, \n    p.min_items, p.max_items, \
    p.pg_column_name, p.pg_column_type, p.rust_field_name, p.rust_field_type, \
    p.sea_orm_type, p.render_strategy, p.ref_target, p.classification, \
    p.classification_kind";

impl GrafeoEngine {
    pub(super) async fn query_generation_order(&self) -> Result<Vec<String>, GraphError> {
        // Get all schema titles
        let all_result = query_gql(self, "MATCH (s:Schema) RETURN s.title")?;
        let reader = RowReader::from_columns(&all_result.columns);
        let all_titles: Vec<String> = all_result
            .rows
            .iter()
            .map(|row| reader.get_string(row, "s.title"))
            .collect::<Result<_, _>>()?;

        // Get DependsOn edges
        let edge_result = query_gql(
            self,
            "MATCH (a:Schema)-[:DependsOn]->(b:Schema) RETURN a.title, b.title",
        )?;
        let edge_reader = RowReader::from_columns(&edge_result.columns);
        let edges: Vec<(String, String)> = edge_result
            .rows
            .iter()
            .map(|row| {
                Ok((
                    edge_reader.get_string(row, "a.title")?,
                    edge_reader.get_string(row, "b.title")?,
                ))
            })
            .collect::<Result<Vec<(String, String)>, GraphError>>()?;

        // Pure ordering (dependency before dependent, lexicographic
        // tie-break); a dependency cycle is an error naming the members.
        topological_order(&all_titles, &edges).map_err(GraphError::Query)
    }

    pub(super) async fn query_all_schema_references(
        &self,
    ) -> Result<Vec<(String, String)>, GraphError> {
        query_many(
            self,
            "MATCH (s:Schema)-[:HasProperty]->(:Property)-[:ReferencesSchema]->(t:Schema) \
               RETURN DISTINCT s.title, t.title",
            |reader, row| {
                Ok((
                    reader.get_string(row, "s.title")?,
                    reader.get_string(row, "t.title")?,
                ))
            },
        )
        .await
    }

    /// Titles targeted by any property reference edge (`ReferencesSchema`
    /// or `ItemsOf`, issue #460) — the contained value-object surface.
    pub(super) async fn query_property_target_titles(
        &self,
    ) -> Result<std::collections::HashSet<String>, GraphError> {
        let mut out = std::collections::HashSet::new();
        for edge in ["ReferencesSchema", "ItemsOf"] {
            let gql = format!(
                "MATCH (:Schema)-[:HasProperty]->(:Property)-[:{edge}]->(t:Schema) \
                 RETURN DISTINCT t.title"
            );
            let result = query_gql(self, &gql)?;
            let reader = RowReader::from_columns(&result.columns);
            for row in &result.rows {
                out.insert(reader.get_string(row, "t.title")?);
            }
        }
        Ok(out)
    }

    pub(super) async fn query_all_properties(
        &self,
    ) -> Result<HashMap<String, Vec<PropertyNode>>, GraphError> {
        let gql = format!(
            "MATCH (s:Schema)-[:HasProperty]->(p:Property) RETURN s.title, {PROPERTY_RETURN_COLS}"
        );
        let result = query_gql(self, &gql)?;
        let reader = RowReader::from_columns(&result.columns);
        let mut map: HashMap<String, Vec<PropertyNode>> = HashMap::new();
        for row in &result.rows {
            let schema_title = reader.get_string(row, "s.title")?;
            let prop = row_to_property_node(&reader, row)?;
            map.entry(schema_title).or_default().push(prop);
        }
        Ok(map)
    }
}

impl GrafeoEngine {
    // ── Namespace plane query methods (issue #267) ─────────────────────

    pub(super) async fn query_namespaces(&self) -> Result<Vec<NamespaceNode>, GraphError> {
        query_many(
            self,
            "MATCH (n:Namespace) RETURN n.fqn, n.parent, n.source ORDER BY n.fqn",
            |reader, row| {
                Ok(NamespaceNode {
                    fqn: reader.get_string(row, "n.fqn")?,
                    parent: reader.get_opt_string(row, "n.parent")?,
                    source: reader.get_opt_string(row, "n.source")?,
                })
            },
        )
        .await
    }

    pub(super) async fn query_schemas_by_namespace(
        &self,
        fqn: &str,
        recursive: bool,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        // The closure of descendant namespaces is computed in Rust from all
        // NamespaceParent edges (GQL-side transitive closure is not assumed),
        // then schemas are collected per namespace and deduplicated.
        let mut wanted: Vec<String> = vec![fqn.to_string()];
        if recursive {
            let result = query_gql(
                self,
                "MATCH (c:Namespace)-[:NamespaceParent]->(p:Namespace) \
                 RETURN c.fqn, p.fqn",
            )?;
            let reader = RowReader::from_columns(&result.columns);
            let edges: Vec<(String, String)> = result
                .rows
                .iter()
                .map(|row| {
                    Ok((
                        reader.get_string(row, "c.fqn")?,
                        reader.get_string(row, "p.fqn")?,
                    ))
                })
                .collect::<Result<Vec<(String, String)>, GraphError>>()?;
            wanted.extend(descendants(fqn, &edges));
        }
        wanted.sort();

        let mut out: Vec<SchemaNode> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for ns in &wanted {
            let params = HashMap::from([("fqn".to_string(), grafeo::Value::String(ns.into()))]);
            let result = query_gql_params(
                self,
                "MATCH (s:Schema)-[:InNamespace]->(n:Namespace {fqn: $fqn}) \
                 RETURN DISTINCT s.schema_id",
                params,
            )?;
            let reader = RowReader::from_columns(&result.columns);
            for row in &result.rows {
                let schema_id = reader.get_string(row, "s.schema_id")?;
                if seen.insert(schema_id.clone())
                    && let Some(schema) = self.query_schema_by_id(&schema_id).await?
                {
                    out.push(schema);
                }
            }
        }
        out.sort_by(|a, b| a.schema_id.cmp(&b.schema_id));
        Ok(out)
    }

    pub(super) async fn query_namespace_imports(
        &self,
        fqn: &str,
    ) -> Result<Vec<NamespaceImport>, GraphError> {
        let params = HashMap::from([("fqn".to_string(), grafeo::Value::String(fqn.into()))]);
        query_many_params(
            self,
            "MATCH (a:Namespace {fqn: $fqn})-[e:NamespaceImports]->(b:Namespace) \
             RETURN b.fqn AS to_ns, e.wildcard AS wildcard, e.alias AS alias \
             ORDER BY to_ns, alias",
            params,
            |reader, row| {
                Ok(NamespaceImport {
                    from_ns: fqn.to_string(),
                    to_ns: reader.get_string(row, "to_ns")?,
                    wildcard: reader.get_bool(row, "wildcard").unwrap_or(false),
                    alias: reader.get_opt_string(row, "alias")?,
                })
            },
        )
        .await
    }

    pub(super) async fn query_namespace_generation_order(&self) -> Result<Vec<String>, GraphError> {
        let all = query_gql(self, "MATCH (n:Namespace) RETURN n.fqn")?;
        let reader = RowReader::from_columns(&all.columns);
        let fqns: Vec<String> = all
            .rows
            .iter()
            .map(|row| reader.get_string(row, "n.fqn"))
            .collect::<Result<_, _>>()?;
        let imports = query_gql(
            self,
            "MATCH (a:Namespace)-[:NamespaceImports]->(b:Namespace) \
             RETURN DISTINCT a.fqn, b.fqn",
        )?;
        let edge_reader = RowReader::from_columns(&imports.columns);
        let pairs: Vec<(String, String)> = imports
            .rows
            .iter()
            .map(|row| {
                Ok((
                    edge_reader.get_string(row, "a.fqn")?,
                    edge_reader.get_string(row, "b.fqn")?,
                ))
            })
            .collect::<Result<Vec<(String, String)>, GraphError>>()?;
        topological_namespace_order(&fqns, &pairs).map_err(GraphError::Query)
    }
}

#[async_trait]
impl GraphQuerier for GrafeoEngine {
    async fn get_schema(&self, title: &str) -> Result<Option<SchemaNode>, GraphError> {
        self.query_schema(title).await
    }

    async fn get_schema_by_id(&self, schema_id: &str) -> Result<Option<SchemaNode>, GraphError> {
        self.query_schema_by_id(schema_id).await
    }

    async fn get_schema_in_domain(
        &self,
        title: &str,
        domain: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        self.query_schema_in_domain(title, domain).await
    }

    async fn list_schemas(&self, domain: Option<&str>) -> Result<Vec<SchemaNode>, GraphError> {
        self.query_schemas(domain).await
    }

    async fn get_properties(&self, schema_title: &str) -> Result<Vec<PropertyNode>, GraphError> {
        self.query_properties(schema_title).await
    }

    async fn get_properties_in_domain(
        &self,
        schema_title: &str,
        domain: &str,
    ) -> Result<Vec<PropertyNode>, GraphError> {
        self.query_properties_in_domain(schema_title, domain).await
    }

    async fn get_child_schemas(&self, schema_title: &str) -> Result<Vec<SchemaNode>, GraphError> {
        self.query_child_schemas(schema_title).await
    }

    async fn get_classification_data(&self) -> Result<Vec<SchemaClassificationData>, GraphError> {
        self.query_classification_data().await
    }

    async fn get_entity_names(&self) -> Result<Vec<String>, GraphError> {
        self.query_entity_names().await
    }

    async fn get_entity_schema_map(&self) -> Result<HashMap<String, String>, GraphError> {
        self.query_entity_schema_map().await
    }

    async fn get_value_object_schemas(&self) -> Result<Vec<SchemaNode>, GraphError> {
        self.query_value_object_schemas().await
    }

    async fn get_parent_candidates(&self) -> Result<Vec<ParentCandidate>, GraphError> {
        self.query_parent_candidates().await
    }

    async fn get_codelist(&self, name: &str) -> Result<Option<CodeList>, GraphError> {
        self.query_codelist(name).await
    }

    async fn list_codelists(&self) -> Result<Vec<CodeList>, GraphError> {
        self.query_codelists().await
    }

    async fn get_enum_values(&self, codelist_name: &str) -> Result<Vec<EnumValue>, GraphError> {
        self.query_enum_values(codelist_name).await
    }

    async fn get_composite_columns(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Vec<CompositeColumn>, GraphError> {
        self.query_composite_columns(property_name, schema_title)
            .await
    }

    async fn get_structured_sub_fields(
        &self,
        schema_title: &str,
    ) -> Result<Vec<StructuredSubField>, GraphError> {
        self.query_structured_sub_fields(schema_title).await
    }

    async fn get_composite_range(
        &self,
        schema_title: &str,
    ) -> Result<Option<CompositeRange>, GraphError> {
        self.query_composite_range(schema_title).await
    }

    async fn get_consumed_fields(
        &self,
        schema_title: &str,
    ) -> Result<Vec<(PropertyNode, String)>, GraphError> {
        self.query_consumed_fields(schema_title).await
    }

    async fn get_codelist_for_property(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Option<(CodeList, String)>, GraphError> {
        self.query_codelist_for_property(property_name, schema_title)
            .await
    }

    async fn get_required_extensions(
        &self,
        schema_title: &str,
    ) -> Result<Vec<Extension>, GraphError> {
        self.query_required_extensions(schema_title).await
    }

    async fn get_composition_tree(
        &self,
        schema_title: &str,
    ) -> Result<CompositionTree, GraphError> {
        self.query_composition_tree(schema_title).await
    }

    async fn get_allof_targets(&self, schema_title: &str) -> Result<Vec<String>, GraphError> {
        self.query_allof_targets(schema_title).await
    }

    async fn get_schemas_that_extend(
        &self,
        parent_title: &str,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        self.query_schemas_that_extend(parent_title).await
    }

    async fn get_referencing_schemas(&self, schema_title: &str) -> Result<Vec<String>, GraphError> {
        self.query_referencing_schemas(schema_title).await
    }

    async fn get_referenced_schemas(
        &self,
        schema_title: &str,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        self.query_referenced_schemas(schema_title).await
    }

    async fn get_property_ref_target(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        self.query_property_ref_target(property_name, schema_title)
            .await
    }

    async fn get_property_ref_target_by_id(
        &self,
        property_name: &str,
        schema_id: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        self.query_property_ref_target_by_id(property_name, schema_id)
            .await
    }

    async fn get_properties_by_schema_id(
        &self,
        schema_id: &str,
    ) -> Result<Vec<PropertyNode>, GraphError> {
        self.query_properties_by_schema_id(schema_id).await
    }

    async fn get_array_item_schema(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        self.query_array_item_schema(property_name, schema_title)
            .await
    }

    async fn get_generation_order(&self) -> Result<Vec<String>, GraphError> {
        self.query_generation_order().await
    }

    async fn list_all_schema_references(&self) -> Result<Vec<(String, String)>, GraphError> {
        self.query_all_schema_references().await
    }

    async fn list_property_target_titles(
        &self,
    ) -> Result<std::collections::HashSet<String>, GraphError> {
        self.query_property_target_titles().await
    }

    async fn list_all_properties(&self) -> Result<HashMap<String, Vec<PropertyNode>>, GraphError> {
        self.query_all_properties().await
    }

    async fn get_ifml_view_containers(&self) -> Result<Vec<ViewContainerNode>, GraphError> {
        self.query_ifml_view_containers().await
    }

    async fn get_ifml_modules(&self) -> Result<Vec<ModuleDefinitionNode>, GraphError> {
        self.query_ifml_modules().await
    }

    async fn get_ifml_container_children(
        &self,
        parent: &str,
    ) -> Result<Vec<ViewContainerNode>, GraphError> {
        self.query_ifml_container_children(parent).await
    }

    async fn get_ifml_view_components(
        &self,
        container_name: &str,
    ) -> Result<Vec<ViewComponentNode>, GraphError> {
        self.query_ifml_view_components(container_name).await
    }

    async fn get_ifml_events(&self, parent_id: &str) -> Result<Vec<EventNode>, GraphError> {
        self.query_ifml_events(parent_id).await
    }

    async fn get_ifml_navigation_flows(&self) -> Result<Vec<NavigationFlowRecord>, GraphError> {
        self.query_ifml_navigation_flows().await
    }

    async fn get_ifml_action_triggers(&self) -> Result<Vec<(String, String)>, GraphError> {
        self.query_ifml_action_triggers().await
    }

    async fn get_ifml_data_flows(
        &self,
    ) -> Result<Vec<(String, String, Option<String>, Option<String>)>, GraphError> {
        self.query_ifml_data_flows().await
    }

    async fn get_ifml_actions(&self) -> Result<Vec<ActionNode>, GraphError> {
        self.query_ifml_actions().await
    }

    async fn get_ifml_parameters(&self) -> Result<Vec<ParameterDefinitionNode>, GraphError> {
        self.query_ifml_parameters().await
    }

    async fn get_parameters_for_view(
        &self,
        container_name: &str,
    ) -> Result<Vec<ParameterDefinitionNode>, GraphError> {
        self.query_parameters_for_view(container_name).await
    }

    async fn get_data_bindings(&self) -> Result<Vec<DataBindingResolution>, GraphError> {
        self.query_data_bindings().await
    }

    async fn get_atproto_namespaces(&self) -> Result<Vec<AtprotoNamespaceNode>, GraphError> {
        self.query_atproto_namespaces().await
    }

    async fn get_lexicons(&self, domain: &str) -> Result<Vec<LexiconNode>, GraphError> {
        self.query_lexicons(domain).await
    }

    async fn get_lexicon_by_schema(
        &self,
        schema_title: &str,
    ) -> Result<Option<LexiconNode>, GraphError> {
        self.query_lexicon_by_schema(schema_title).await
    }

    async fn get_collections(&self, domain: &str) -> Result<Vec<CollectionNode>, GraphError> {
        self.query_collections(domain).await
    }

    async fn get_repositories(&self) -> Result<Vec<RepositoryNode>, GraphError> {
        self.query_repositories().await
    }

    async fn get_lexicon_references(&self, nsid: &str) -> Result<Vec<LexiconNode>, GraphError> {
        self.query_lexicon_references(nsid).await
    }

    async fn get_api_resources(&self) -> Result<Vec<ApiResourceNode>, GraphError> {
        self.query_api_resources().await
    }

    async fn get_api_resource(&self, name: &str) -> Result<Option<ApiResourceNode>, GraphError> {
        self.query_api_resource(name).await
    }

    async fn get_api_operations(
        &self,
        resource_name: &str,
    ) -> Result<Vec<ApiOperationNode>, GraphError> {
        self.query_api_operations(resource_name).await
    }

    async fn get_api_operation(&self, name: &str) -> Result<Option<ApiOperationNode>, GraphError> {
        self.query_api_operation(name).await
    }

    async fn get_http_endpoint_for_operation(
        &self,
        operation_name: &str,
    ) -> Result<Option<HttpEndpointNode>, GraphError> {
        self.query_http_endpoint_for_operation(operation_name).await
    }

    async fn get_interactions(
        &self,
        _operation_name: &str,
    ) -> Result<Vec<InteractionNode>, GraphError> {
        self.query_interactions(_operation_name).await
    }

    async fn get_http_endpoints(&self) -> Result<Vec<HttpEndpointNode>, GraphError> {
        self.query_http_endpoints().await
    }

    async fn get_error_definitions(&self) -> Result<Vec<ErrorDefinitionNode>, GraphError> {
        self.query_error_definitions().await
    }

    async fn get_permissions(&self) -> Result<Vec<PermissionNode>, GraphError> {
        self.query_permissions().await
    }

    async fn get_pipelines(&self) -> Result<Vec<PipelineNode>, GraphError> {
        self.query_pipelines().await
    }

    async fn get_pipeline_for_endpoint(
        &self,
        endpoint_path: &str,
    ) -> Result<Option<PipelineNode>, GraphError> {
        self.query_pipeline_for_endpoint(endpoint_path).await
    }

    async fn get_policies_for_schema(
        &self,
        schema_title: &str,
    ) -> Result<Vec<PolicyNode>, GraphError> {
        self.query_policies_for_schema(schema_title).await
    }

    async fn get_relationships_for_schema(
        &self,
        schema_title: &str,
    ) -> Result<Vec<RelationshipNode>, GraphError> {
        self.query_relationships_for_schema(schema_title).await
    }

    async fn get_relationship_by_name(
        &self,
        name: &str,
    ) -> Result<Option<RelationshipNode>, GraphError> {
        self.query_relationship_by_name(name).await
    }

    async fn list_all_policies(&self) -> Result<Vec<PolicyNode>, GraphError> {
        self.query_all_policies().await
    }

    async fn list_all_relationships(&self) -> Result<Vec<RelationshipNode>, GraphError> {
        self.query_all_relationships().await
    }

    async fn get_security_identity(
        &self,
        subject: &str,
    ) -> Result<Option<SecurityIdentityNode>, GraphError> {
        self.query_security_identity(subject).await
    }

    async fn get_memberships_for_identity(
        &self,
        identity_name: &str,
    ) -> Result<Vec<MembershipNode>, GraphError> {
        self.query_memberships_for_identity(identity_name).await
    }

    async fn get_tenant(&self, name: &str) -> Result<Option<TenantNode>, GraphError> {
        self.query_tenant(name).await
    }

    async fn list_all_tenants(&self) -> Result<Vec<TenantNode>, GraphError> {
        self.query_all_tenants().await
    }

    async fn get_actors(&self) -> Result<Vec<ActorNode>, GraphError> {
        self.query_actors().await
    }

    async fn get_capabilities(&self) -> Result<Vec<CapabilityNode>, GraphError> {
        self.query_capabilities().await
    }

    async fn get_grants(&self) -> Result<Vec<GrantEdge>, GraphError> {
        self.query_grants().await
    }

    async fn get_actor_policy(&self) -> Result<Option<ActorPolicyNode>, GraphError> {
        self.query_actor_policy().await
    }

    async fn effective_permits(&self, actor: &str) -> Result<Vec<Permit>, GraphError> {
        self.query_effective_permits(actor).await
    }

    async fn get_ddd_models(&self) -> Result<Vec<DddModelGraph>, GraphError> {
        self.query_ddd_models().await
    }

    async fn get_evt_models(&self) -> Result<Vec<EvtModelGraph>, GraphError> {
        self.query_evt_models().await
    }

    async fn get_deploy_models(&self) -> Result<Vec<DeployModelGraph>, GraphError> {
        self.query_deploy_models().await
    }

    async fn composition_table_conflict(
        &self,
        schema_title: &str,
    ) -> Result<Option<(String, String)>, GraphError> {
        self.query_composition_table_conflict(schema_title).await
    }

    async fn get_mox_vocabularies(&self) -> Result<Vec<MoxVocabularyNode>, GraphError> {
        self.query_mox_vocabularies().await
    }

    async fn get_mox_operations(&self) -> Result<Vec<MoxOperationNode>, GraphError> {
        self.query_mox_operations().await
    }

    async fn get_mox_derived_features(&self) -> Result<Vec<MoxDerivedFeatureNode>, GraphError> {
        self.query_mox_derived_features().await
    }

    async fn get_mox_derived_features_for_schema(
        &self,
        schema_title: &str,
    ) -> Result<Vec<MoxDerivedFeatureNode>, GraphError> {
        self.query_mox_derived_features_for_schema(schema_title)
            .await
    }

    async fn get_conditions_for_schema(
        &self,
        schema_title: &str,
    ) -> Result<Vec<ConditionNode>, GraphError> {
        self.query_conditions_for_schema(schema_title).await
    }

    async fn list_conditions(&self) -> Result<Vec<ConditionNode>, GraphError> {
        self.query_conditions().await
    }

    async fn list_regulatory(&self) -> Result<Vec<RegulatoryNode>, GraphError> {
        self.query_regulatory().await
    }

    async fn list_regulatory_references(&self) -> Result<Vec<RegulatoryRefRecord>, GraphError> {
        self.query_regulatory_references().await
    }

    async fn list_functions(&self) -> Result<Vec<FunctionNode>, GraphError> {
        self.query_functions().await
    }

    async fn list_function_extends(&self) -> Result<Vec<(String, String)>, GraphError> {
        self.query_function_extends().await
    }

    async fn list_rules(&self) -> Result<Vec<RuleNode>, GraphError> {
        self.query_rules().await
    }

    async fn list_rule_applies_to(&self) -> Result<Vec<(String, String)>, GraphError> {
        self.query_rule_applies_to().await
    }

    async fn list_rule_references(&self) -> Result<Vec<RuleRefRecord>, GraphError> {
        self.query_rule_references().await
    }

    async fn list_namespaces(&self) -> Result<Vec<NamespaceNode>, GraphError> {
        self.query_namespaces().await
    }

    async fn list_schemas_by_namespace(
        &self,
        fqn: &str,
        recursive: bool,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        self.query_schemas_by_namespace(fqn, recursive).await
    }

    async fn get_namespace_imports(&self, fqn: &str) -> Result<Vec<NamespaceImport>, GraphError> {
        self.query_namespace_imports(fqn).await
    }

    async fn namespace_generation_order(&self) -> Result<Vec<String>, GraphError> {
        self.query_namespace_generation_order().await
    }
}
