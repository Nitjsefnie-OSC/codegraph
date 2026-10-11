use super::MockEngine;
use crate::error::GraphError;
use crate::traits::GraphQuerier;
use crate::types::*;
use async_trait::async_trait;
use std::collections::HashMap;

#[async_trait]
impl GraphQuerier for MockEngine {
    async fn get_schema(&self, title: &str) -> Result<Option<SchemaNode>, GraphError> {
        Ok(self.schemas.lock().unwrap().get(title).cloned())
    }

    async fn get_schema_by_id(&self, schema_id: &str) -> Result<Option<SchemaNode>, GraphError> {
        let schemas = self.schemas.lock().unwrap();
        Ok(schemas.values().find(|s| s.schema_id == schema_id).cloned())
    }

    async fn get_schema_in_domain(
        &self,
        title: &str,
        domain: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        let schemas = self.schemas.lock().unwrap();
        Ok(schemas
            .values()
            .find(|s| s.title == title && s.domain.as_deref() == Some(domain))
            .cloned())
    }

    async fn list_schemas(&self, domain: Option<&str>) -> Result<Vec<SchemaNode>, GraphError> {
        let schemas = self.schemas.lock().unwrap();
        let result: Vec<_> = schemas
            .values()
            .filter(|s| match domain {
                Some(d) => s.domain.as_deref() == Some(d),
                None => true,
            })
            .cloned()
            .collect();
        Ok(result)
    }

    async fn get_properties(&self, schema_title: &str) -> Result<Vec<PropertyNode>, GraphError> {
        Ok(self
            .properties
            .lock()
            .unwrap()
            .get(schema_title)
            .cloned()
            .unwrap_or_default())
    }

    async fn get_child_schemas(&self, schema_title: &str) -> Result<Vec<SchemaNode>, GraphError> {
        let properties = {
            let props = self.properties.lock().unwrap();
            props.get(schema_title).cloned().unwrap_or_default()
        };
        let schemas = self.schemas.lock().unwrap();
        // Route 1: inline #/$defs children (parent_schema back-pointer).
        let mut children: Vec<SchemaNode> = schemas
            .values()
            .filter(|s| s.parent_schema.as_deref() == Some(schema_title))
            .cloned()
            .collect();
        // Route 2 (issue #312): derived refers children — array-of-entity-ref
        // properties whose ref target is an entity schema (the FK-on-child
        // lowering). Mirrors the Grafeo ItemsOf route: entity targets only,
        // self-references excluded.
        let mut seen: std::collections::HashSet<String> =
            children.iter().map(|c| c.title.clone()).collect();
        for prop in &properties {
            if !prop.is_array {
                continue;
            }
            let Some(ref target) = prop.ref_target else {
                continue;
            };
            let candidate = ref_target_candidate_title(target);
            if candidate == schema_title {
                continue;
            }
            if let Some(child) = schemas.get(candidate)
                && child.is_entity
                && seen.insert(child.title.clone())
            {
                children.push(child.clone());
            }
        }
        children.sort_by(|a, b| a.title.cmp(&b.title));
        Ok(children)
    }

    async fn get_classification_data(&self) -> Result<Vec<SchemaClassificationData>, GraphError> {
        Ok(vec![])
    }

    async fn get_entity_names(&self) -> Result<Vec<String>, GraphError> {
        let schemas = self.schemas.lock().unwrap();
        Ok(schemas
            .values()
            .filter(|s| s.is_entity)
            .map(|s| s.title.clone())
            .collect())
    }

    async fn get_entity_schema_map(&self) -> Result<HashMap<String, String>, GraphError> {
        let schemas = self.schemas.lock().unwrap();
        Ok(schemas
            .values()
            .filter(|s| s.is_entity)
            .map(|s| (s.title.clone(), s.rel_path.clone()))
            .collect())
    }

    async fn get_value_object_schemas(&self) -> Result<Vec<SchemaNode>, GraphError> {
        let schemas = self.schemas.lock().unwrap();
        Ok(schemas
            .values()
            .filter(|s| !s.is_entity && !s.is_codelist && s.schema_type == "object")
            .cloned()
            .collect())
    }

    async fn get_parent_candidates(&self) -> Result<Vec<ParentCandidate>, GraphError> {
        Ok(self.parent_candidates.lock().unwrap().clone())
    }

    async fn list_property_target_titles(
        &self,
    ) -> Result<std::collections::HashSet<String>, GraphError> {
        let ref_targets = self.ref_targets.lock().unwrap();
        Ok(ref_targets.values().map(|s| s.title.clone()).collect())
    }

    async fn get_codelist(&self, name: &str) -> Result<Option<CodeList>, GraphError> {
        Ok(self.codelists.lock().unwrap().get(name).cloned())
    }

    async fn list_codelists(&self) -> Result<Vec<CodeList>, GraphError> {
        Ok(self.codelists.lock().unwrap().values().cloned().collect())
    }

    async fn get_enum_values(&self, codelist_name: &str) -> Result<Vec<EnumValue>, GraphError> {
        Ok(self
            .enum_values
            .lock()
            .unwrap()
            .get(codelist_name)
            .cloned()
            .unwrap_or_default())
    }

    async fn get_composite_columns(
        &self,
        _property_name: &str,
        _schema_title: &str,
    ) -> Result<Vec<CompositeColumn>, GraphError> {
        Ok(vec![])
    }

    async fn get_structured_sub_fields(
        &self,
        _schema_title: &str,
    ) -> Result<Vec<StructuredSubField>, GraphError> {
        Ok(vec![])
    }

    async fn get_composite_range(
        &self,
        schema_title: &str,
    ) -> Result<Option<CompositeRange>, GraphError> {
        Ok(self
            .composite_ranges
            .lock()
            .unwrap()
            .get(schema_title)
            .cloned())
    }

    async fn get_consumed_fields(
        &self,
        schema_title: &str,
    ) -> Result<Vec<(PropertyNode, String)>, GraphError> {
        Ok(self
            .consumed_fields
            .lock()
            .unwrap()
            .get(schema_title)
            .cloned()
            .unwrap_or_default())
    }

    async fn get_codelist_for_property(
        &self,
        _property_name: &str,
        _schema_title: &str,
    ) -> Result<Option<(CodeList, String)>, GraphError> {
        Ok(None)
    }

    async fn get_required_extensions(
        &self,
        _schema_title: &str,
    ) -> Result<Vec<Extension>, GraphError> {
        Ok(vec![])
    }

    async fn get_composition_tree(
        &self,
        schema_title: &str,
    ) -> Result<CompositionTree, GraphError> {
        self.trees
            .lock()
            .unwrap()
            .get(schema_title)
            .cloned()
            .ok_or_else(|| GraphError::NotFound(format!("composition tree for {schema_title}")))
    }

    async fn get_allof_targets(&self, schema_title: &str) -> Result<Vec<String>, GraphError> {
        Ok(self
            .allof_targets
            .lock()
            .unwrap()
            .get(schema_title)
            .cloned()
            .unwrap_or_default())
    }

    async fn get_schemas_that_extend(
        &self,
        parent_title: &str,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        Ok(self
            .extends_map
            .lock()
            .unwrap()
            .get(parent_title)
            .cloned()
            .unwrap_or_default())
    }

    async fn list_all_properties(&self) -> Result<HashMap<String, Vec<PropertyNode>>, GraphError> {
        Ok(self.properties.lock().unwrap().clone())
    }

    async fn get_referencing_schemas(
        &self,
        _schema_title: &str,
    ) -> Result<Vec<String>, GraphError> {
        Ok(vec![])
    }

    async fn get_referenced_schemas(
        &self,
        schema_title: &str,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        let ref_targets = self.ref_targets.lock().unwrap();
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for ((_prop_name, st), schema) in ref_targets.iter() {
            if st == schema_title && seen.insert(schema.schema_id.clone()) {
                result.push(schema.clone());
            }
        }
        Ok(result)
    }

    async fn get_property_ref_target(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        let ref_targets = self.ref_targets.lock().unwrap();
        Ok(ref_targets
            .get(&(property_name.to_string(), schema_title.to_string()))
            .cloned())
    }

    async fn get_property_ref_target_by_id(
        &self,
        property_name: &str,
        schema_id: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        let title = {
            let schemas = self.schemas.lock().unwrap();
            schemas
                .values()
                .find(|s| s.schema_id == schema_id)
                .map(|s| s.title.clone())
        };
        let Some(title) = title else {
            return Ok(None);
        };
        let ref_targets = self.ref_targets.lock().unwrap();
        Ok(ref_targets
            .get(&(property_name.to_string(), title))
            .cloned())
    }

    async fn get_properties_by_schema_id(
        &self,
        schema_id: &str,
    ) -> Result<Vec<PropertyNode>, GraphError> {
        let title = {
            let schemas = self.schemas.lock().unwrap();
            schemas
                .values()
                .find(|s| s.schema_id == schema_id)
                .map(|s| s.title.clone())
        };
        let Some(title) = title else {
            return Ok(Vec::new());
        };
        Ok(self
            .properties
            .lock()
            .unwrap()
            .get(&title)
            .cloned()
            .unwrap_or_default())
    }

    async fn get_array_item_schema(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        let ref_targets = self.ref_targets.lock().unwrap();
        Ok(ref_targets
            .get(&(property_name.to_string(), schema_title.to_string()))
            .cloned())
    }

    async fn get_ifml_view_containers(&self) -> Result<Vec<ViewContainerNode>, GraphError> {
        let map = self.view_containers.lock().unwrap();
        Ok(map.values().cloned().collect())
    }

    async fn get_ifml_modules(&self) -> Result<Vec<ModuleDefinitionNode>, GraphError> {
        Ok(self
            .module_definitions
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect())
    }

    async fn get_ifml_view_components(
        &self,
        container_name: &str,
    ) -> Result<Vec<ViewComponentNode>, GraphError> {
        let components = self.view_components.lock().unwrap();
        let map = self.view_container_components.lock().unwrap();
        let names = map.get(container_name).cloned().unwrap_or_default();
        Ok(names
            .iter()
            .filter_map(|n| components.get(n).cloned())
            .collect())
    }

    async fn get_ifml_container_children(
        &self,
        parent: &str,
    ) -> Result<Vec<ViewContainerNode>, GraphError> {
        let containers = self.view_containers.lock().unwrap();
        let map = self.view_container_children.lock().unwrap();
        let names = map
            .get(strip_ifml_prefix(parent))
            .cloned()
            .unwrap_or_default();
        Ok(names
            .iter()
            .filter_map(|n| containers.get(n).cloned())
            .collect())
    }

    async fn get_ifml_events(&self, parent_id: &str) -> Result<Vec<EventNode>, GraphError> {
        let events = self.events.lock().unwrap();
        let map = self.events_by_parent.lock().unwrap();
        let names = map.get(parent_id).cloned().unwrap_or_default();
        Ok(names
            .iter()
            .filter_map(|n| events.get(n).cloned())
            .collect())
    }

    async fn get_ifml_navigation_flows(&self) -> Result<Vec<NavigationFlowRecord>, GraphError> {
        let events_by_parent = self.events_by_parent.lock().unwrap();
        let components_by_container = self.view_container_components.lock().unwrap();
        let flows = self.navigation_flows.lock().unwrap();
        let mut result = Vec::new();
        for (event_id, target_id, binding) in flows.iter() {
            let event_name = strip_ifml_prefix(event_id).to_string();
            let parent = events_by_parent
                .iter()
                .find(|(_, evs)| evs.contains(&event_name))
                .map(|(p, _)| p.clone())
                .unwrap_or_default();
            let source = strip_ifml_prefix(&parent).to_string();
            let source_container = if let Some(component) = parent.strip_prefix("comp:") {
                components_by_container
                    .iter()
                    .find(|(_, children)| children.iter().any(|c| c == component))
                    .map(|(container, _)| container.clone())
                    .unwrap_or_else(|| source.clone())
            } else {
                source.clone()
            };
            result.push(NavigationFlowRecord {
                source,
                source_container,
                event: event_name,
                target: strip_ifml_prefix(target_id).to_string(),
                target_param_binding: binding.clone(),
            });
        }
        Ok(result)
    }

    async fn get_ifml_action_triggers(&self) -> Result<Vec<(String, String)>, GraphError> {
        Ok(self.action_triggers.lock().unwrap().clone())
    }

    async fn get_ifml_data_flows(
        &self,
    ) -> Result<Vec<(String, String, Option<String>, Option<String>)>, GraphError> {
        let flows = self.data_flows.lock().unwrap();
        Ok(flows
            .iter()
            .map(|(source, target, source_param, target_param)| {
                (
                    strip_ifml_prefix(source).to_string(),
                    strip_ifml_prefix(target).to_string(),
                    source_param.clone(),
                    target_param.clone(),
                )
            })
            .collect())
    }

    async fn get_ifml_actions(&self) -> Result<Vec<ActionNode>, GraphError> {
        let map = self.action_nodes.lock().unwrap();
        Ok(map.values().cloned().collect())
    }

    async fn get_ifml_parameters(&self) -> Result<Vec<ParameterDefinitionNode>, GraphError> {
        let map = self.parameter_definitions.lock().unwrap();
        Ok(map.values().cloned().collect())
    }

    async fn get_parameters_for_view(
        &self,
        container_name: &str,
    ) -> Result<Vec<ParameterDefinitionNode>, GraphError> {
        let definitions = self.parameter_definitions.lock().unwrap();
        let by_parent = self.params_by_parent.lock().unwrap();
        Ok(by_parent
            .get(container_name)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|name| definitions.get(name).cloned())
            .collect())
    }

    async fn get_data_bindings(&self) -> Result<Vec<DataBindingResolution>, GraphError> {
        let components = self.view_components.lock().unwrap();
        let has_db = self.data_binding_edges.lock().unwrap();
        let binds_entity = self.binding_entity_edges.lock().unwrap();
        let binds_prop = self.binding_property_edges.lock().unwrap();
        let mut result = Vec::new();
        for (comp_name, binding_name) in has_db.iter() {
            let Some(entity_title) = binds_entity
                .iter()
                .find(|(b, _)| b == binding_name)
                .map(|(_, t)| t.clone())
            else {
                continue;
            };
            let fields: Vec<String> = binds_prop
                .iter()
                .filter(|(c, _)| c == comp_name)
                .map(|(_, p)| p.clone())
                .collect();
            let api_operation = components
                .get(comp_name)
                .and_then(|c| c.api_operation.clone());
            result.push(DataBindingResolution {
                component: comp_name.clone(),
                entity_title,
                fields,
                api_operation,
            });
        }
        Ok(result)
    }

    async fn get_generation_order(&self) -> Result<Vec<String>, GraphError> {
        Ok(self.get_entity_names().await?)
    }

    // ── AT Protocol query methods ─────────────────────────────────────

    async fn get_lexicons(&self, domain: &str) -> Result<Vec<LexiconNode>, GraphError> {
        Ok(self
            .lexicons
            .lock()
            .unwrap()
            .values()
            .filter(|l| domain.is_empty() || l.domain == domain)
            .cloned()
            .collect())
    }

    async fn get_lexicon_by_schema(
        &self,
        schema_title: &str,
    ) -> Result<Option<LexiconNode>, GraphError> {
        let mapping = self.schema_lexicons.lock().unwrap();
        if let Some(nsid) = mapping.get(schema_title) {
            let lexicons = self.lexicons.lock().unwrap();
            return Ok(lexicons.get(nsid).cloned());
        }
        Ok(None)
    }

    async fn get_collections(&self, domain: &str) -> Result<Vec<CollectionNode>, GraphError> {
        Ok(self
            .collections
            .lock()
            .unwrap()
            .values()
            .filter(|c| domain.is_empty() || c.domain == domain)
            .cloned()
            .collect())
    }

    async fn get_repositories(&self) -> Result<Vec<RepositoryNode>, GraphError> {
        Ok(self
            .repositories
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect())
    }

    async fn get_atproto_namespaces(&self) -> Result<Vec<AtprotoNamespaceNode>, GraphError> {
        let mut nodes: Vec<AtprotoNamespaceNode> = self
            .atproto_namespaces
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect();
        nodes.sort_by(|a, b| a.authority.cmp(&b.authority));
        Ok(nodes)
    }

    // ── Namespace plane query methods (issue #267) ────────────────────

    async fn list_namespaces(&self) -> Result<Vec<NamespaceNode>, GraphError> {
        let mut nodes: Vec<NamespaceNode> = self
            .type_namespaces
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect();
        nodes.sort_by(|a, b| a.fqn.cmp(&b.fqn));
        Ok(nodes)
    }

    async fn list_schemas_by_namespace(
        &self,
        fqn: &str,
        recursive: bool,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        let wanted = self.namespace_closure(fqn, recursive);
        // The mock's schema map is title-keyed, so schema_id → fqn mappings
        // resolve through a value scan.
        let mapping = self.schema_in_namespace.lock().unwrap();
        let schemas = self.schemas.lock().unwrap();
        let mut out: Vec<SchemaNode> = mapping
            .iter()
            .filter(|(_, ns)| wanted.contains(ns))
            .filter_map(|(schema_id, _)| {
                schemas
                    .values()
                    .find(|s| &s.schema_id == schema_id)
                    .cloned()
            })
            .collect();
        out.sort_by(|a, b| a.schema_id.cmp(&b.schema_id));
        Ok(out)
    }

    async fn get_namespace_imports(&self, fqn: &str) -> Result<Vec<NamespaceImport>, GraphError> {
        let mut imports: Vec<NamespaceImport> = self
            .namespace_imports
            .lock()
            .unwrap()
            .iter()
            .filter(|i| i.from_ns == fqn)
            .cloned()
            .collect();
        imports.sort_by(|a, b| (&a.to_ns, &a.alias).cmp(&(&b.to_ns, &b.alias)));
        Ok(imports)
    }

    async fn namespace_generation_order(&self) -> Result<Vec<String>, GraphError> {
        let fqns: Vec<String> = self
            .type_namespaces
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let imports = self.namespace_imports.lock().unwrap();
        let pairs: Vec<(String, String)> = imports
            .iter()
            .map(|i| (i.from_ns.clone(), i.to_ns.clone()))
            .collect();
        topological_namespace_order(&fqns, &pairs).map_err(GraphError::Query)
    }

    #[allow(unused_variables)]
    async fn get_lexicon_references(&self, nsid: &str) -> Result<Vec<LexiconNode>, GraphError> {
        // TODO: look up via LexiconReferences edges.
        // MockEngine doesn't store edges, so we can't resolve this relationship.
        Ok(Vec::new())
    }

    // ── API metamodel query methods ────────────────────────────────────

    async fn get_api_resources(&self) -> Result<Vec<ApiResourceNode>, GraphError> {
        Ok(self
            .api_resources
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect())
    }

    async fn get_api_resource(&self, name: &str) -> Result<Option<ApiResourceNode>, GraphError> {
        Ok(self
            .api_resources
            .lock()
            .unwrap()
            .values()
            .find(|r| r.name == name)
            .cloned())
    }

    async fn get_api_operations(
        &self,
        resource_name: &str,
    ) -> Result<Vec<ApiOperationNode>, GraphError> {
        let ops = self.api_operations.lock().unwrap();
        let edges = self.resource_operations.lock().unwrap();
        let names = edges.get(resource_name).cloned().unwrap_or_default();
        Ok(names
            .iter()
            .filter_map(|n| ops.get(&format!("ao:{}", n)).cloned())
            .collect())
    }

    async fn get_api_operation(&self, name: &str) -> Result<Option<ApiOperationNode>, GraphError> {
        Ok(self
            .api_operations
            .lock()
            .unwrap()
            .get(&format!("ao:{}", name))
            .cloned())
    }

    async fn get_http_endpoint_for_operation(
        &self,
        operation_name: &str,
    ) -> Result<Option<HttpEndpointNode>, GraphError> {
        let op_interaction = self.op_interaction.lock().unwrap();
        let interaction_endpoint = self.interaction_endpoint.lock().unwrap();
        let Some(ia_id) = op_interaction.get(operation_name) else {
            return Ok(None);
        };
        let Some(he_id) = interaction_endpoint.get(ia_id) else {
            return Ok(None);
        };
        Ok(self.http_endpoints.lock().unwrap().get(he_id).cloned())
    }

    async fn get_interactions(
        &self,
        operation_name: &str,
    ) -> Result<Vec<InteractionNode>, GraphError> {
        let interaction_id = self
            .op_interaction
            .lock()
            .unwrap()
            .get(operation_name)
            .cloned();
        Ok(match interaction_id {
            Some(id) => self
                .interactions
                .lock()
                .unwrap()
                .get(&id)
                .cloned()
                .into_iter()
                .collect(),
            None => Vec::new(),
        })
    }

    async fn get_http_endpoints(&self) -> Result<Vec<HttpEndpointNode>, GraphError> {
        Ok(self
            .http_endpoints
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect())
    }

    async fn get_error_definitions(&self) -> Result<Vec<ErrorDefinitionNode>, GraphError> {
        Ok(self
            .error_definitions
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect())
    }

    async fn get_permissions(&self) -> Result<Vec<PermissionNode>, GraphError> {
        Ok(self.permissions.lock().unwrap().values().cloned().collect())
    }

    async fn get_pipelines(&self) -> Result<Vec<PipelineNode>, GraphError> {
        Ok(self.pipelines.lock().unwrap().values().cloned().collect())
    }

    async fn get_pipeline_for_endpoint(
        &self,
        endpoint_path: &str,
    ) -> Result<Option<PipelineNode>, GraphError> {
        let _ = endpoint_path;
        Ok(None)
    }

    // ── Persistence metamodel queries ─────────────────────────────────

    async fn get_policies_for_schema(
        &self,
        schema_title: &str,
    ) -> Result<Vec<PolicyNode>, GraphError> {
        Ok(self
            .policies
            .lock()
            .unwrap()
            .values()
            .filter(|p| p.target_schema == schema_title)
            .cloned()
            .collect())
    }

    async fn get_relationships_for_schema(
        &self,
        schema_title: &str,
    ) -> Result<Vec<RelationshipNode>, GraphError> {
        Ok(self
            .relationships
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.source_schema == schema_title || r.target_schema == schema_title)
            .cloned()
            .collect())
    }

    async fn get_relationship_by_name(
        &self,
        name: &str,
    ) -> Result<Option<RelationshipNode>, GraphError> {
        Ok(self.relationships.lock().unwrap().get(name).cloned())
    }

    async fn list_all_policies(&self) -> Result<Vec<PolicyNode>, GraphError> {
        Ok(self.policies.lock().unwrap().values().cloned().collect())
    }

    async fn list_all_relationships(&self) -> Result<Vec<RelationshipNode>, GraphError> {
        Ok(self
            .relationships
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect())
    }

    // ── Security metamodel queries ─────────────────────────────────────

    async fn get_security_identity(
        &self,
        subject: &str,
    ) -> Result<Option<SecurityIdentityNode>, GraphError> {
        Ok(self
            .security_identities
            .lock()
            .unwrap()
            .values()
            .find(|id| id.subject == subject)
            .cloned())
    }

    async fn get_memberships_for_identity(
        &self,
        identity_name: &str,
    ) -> Result<Vec<MembershipNode>, GraphError> {
        Ok(self
            .memberships
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.identity == identity_name)
            .cloned()
            .collect())
    }

    async fn get_tenant(&self, name: &str) -> Result<Option<TenantNode>, GraphError> {
        Ok(self.tenants.lock().unwrap().get(name).cloned())
    }

    async fn list_all_tenants(&self) -> Result<Vec<TenantNode>, GraphError> {
        Ok(self.tenants.lock().unwrap().values().cloned().collect())
    }

    // ── Authorization metamodel queries ─────────────────────────────

    async fn get_actors(&self) -> Result<Vec<ActorNode>, GraphError> {
        let mut actors: Vec<ActorNode> = self.actors.lock().unwrap().values().cloned().collect();
        actors.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(actors)
    }

    async fn get_capabilities(&self) -> Result<Vec<CapabilityNode>, GraphError> {
        let mut capabilities: Vec<CapabilityNode> = self
            .capabilities
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect();
        capabilities.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(capabilities)
    }

    async fn get_grants(&self) -> Result<Vec<GrantEdge>, GraphError> {
        Ok(self.grants.lock().unwrap().clone())
    }

    async fn get_actor_policy(&self) -> Result<Option<ActorPolicyNode>, GraphError> {
        Ok(self.actor_policy.lock().unwrap().clone())
    }

    async fn effective_permits(&self, actor: &str) -> Result<Vec<Permit>, GraphError> {
        let actors: Vec<ActorNode> = self.actors.lock().unwrap().values().cloned().collect();
        let grants = self.grants.lock().unwrap().clone();
        Ok(resolve_effective_permits(&actors, &grants, actor))
    }

    async fn get_ddd_models(&self) -> Result<Vec<DddModelGraph>, GraphError> {
        let mut models = self.ddd_models.lock().unwrap().clone();
        models.sort_by(|a, b| a.application.name.cmp(&b.application.name));
        Ok(models)
    }

    async fn get_evt_models(&self) -> Result<Vec<EvtModelGraph>, GraphError> {
        let mut models = self.evt_models.lock().unwrap().clone();
        models.sort_by(|a, b| a.source_path.cmp(&b.source_path));
        Ok(models)
    }

    async fn get_deploy_models(&self) -> Result<Vec<DeployModelGraph>, GraphError> {
        let mut models = self.deploy_models.lock().unwrap().clone();
        models.sort_by(|a, b| a.source_path.cmp(&b.source_path));
        Ok(models)
    }

    // ── Constraint plane queries (issue #261) ─────────────────────────

    async fn get_conditions_for_schema(
        &self,
        schema_title: &str,
    ) -> Result<Vec<ConditionNode>, GraphError> {
        let mut nodes: Vec<ConditionNode> = self
            .conditions
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.owner_title == schema_title)
            .cloned()
            .collect();
        nodes.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(nodes)
    }

    async fn list_conditions(&self) -> Result<Vec<ConditionNode>, GraphError> {
        let mut nodes: Vec<ConditionNode> = self.conditions.lock().unwrap().clone();
        nodes.sort_by(|a, b| (&a.owner_title, &a.name).cmp(&(&b.owner_title, &b.name)));
        Ok(nodes)
    }

    // ── Regulatory reference plane queries (issue #265) ───────────────

    async fn list_regulatory(&self) -> Result<Vec<RegulatoryNode>, GraphError> {
        let mut nodes: Vec<RegulatoryNode> = self.regulatory.lock().unwrap().clone();
        nodes.sort_by(|a, b| (a.kind.as_str(), &a.name).cmp(&(b.kind.as_str(), &b.name)));
        Ok(nodes)
    }

    async fn list_regulatory_references(&self) -> Result<Vec<RegulatoryRefRecord>, GraphError> {
        let mut records: Vec<RegulatoryRefRecord> = self.regulatory_refs.lock().unwrap().clone();
        records.sort_by(|a, b| (&a.owner, &a.target).cmp(&(&b.owner, &b.target)));
        Ok(records)
    }

    // ── Computation plane queries (issue #263) ─────────────────────────

    async fn list_functions(&self) -> Result<Vec<FunctionNode>, GraphError> {
        let mut nodes: Vec<FunctionNode> = self.functions.lock().unwrap().clone();
        nodes.sort_by(|a, b| (&a.domain, &a.name).cmp(&(&b.domain, &b.name)));
        Ok(nodes)
    }

    async fn list_function_extends(&self) -> Result<Vec<(String, String)>, GraphError> {
        let mut edges: Vec<(String, String)> = self.function_extends.lock().unwrap().clone();
        edges.sort();
        Ok(edges)
    }

    // ── Rule plane queries (issue #264) ────────────────────────────────

    async fn list_rules(&self) -> Result<Vec<RuleNode>, GraphError> {
        let mut nodes: Vec<RuleNode> = self.rules.lock().unwrap().clone();
        nodes.sort_by(|a, b| (&a.domain, &a.name).cmp(&(&b.domain, &b.name)));
        Ok(nodes)
    }

    async fn list_rule_applies_to(&self) -> Result<Vec<(String, String)>, GraphError> {
        let mut edges: Vec<(String, String)> = self.rule_applies_to.lock().unwrap().clone();
        edges.sort();
        Ok(edges)
    }

    async fn list_rule_references(&self) -> Result<Vec<RuleRefRecord>, GraphError> {
        let mut records: Vec<RuleRefRecord> = self.rule_refs.lock().unwrap().clone();
        records.sort_by(|a, b| {
            (&a.rule_source, &a.schema_title, &a.attribute).cmp(&(
                &b.rule_source,
                &b.schema_title,
                &b.attribute,
            ))
        });
        Ok(records)
    }
}
