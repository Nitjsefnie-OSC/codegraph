use super::MockEngine;
use super::builder::strip_api_prefix;
use crate::error::GraphError;
use crate::traits::GraphIngestor;
use crate::types::*;
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
impl GraphIngestor for MockEngine {
    async fn ingest_schema(&self, node: &SchemaNode) -> Result<String, GraphError> {
        let id = node.schema_id.clone();
        self.schemas
            .lock()
            .unwrap()
            .insert(node.title.clone(), node.clone());
        Ok(id)
    }

    async fn ingest_property(
        &self,
        schema_title: &str,
        _schema_id: &str,
        prop: &PropertyNode,
    ) -> Result<(), GraphError> {
        self.properties
            .lock()
            .unwrap()
            .entry(schema_title.to_string())
            .or_default()
            .push(prop.clone());
        Ok(())
    }

    async fn ingest_codelist(&self, codelist: &CodeList) -> Result<(), GraphError> {
        self.codelists
            .lock()
            .unwrap()
            .insert(codelist.name.clone(), codelist.clone());
        Ok(())
    }

    async fn ingest_enum_value(
        &self,
        codelist_name: &str,
        value: &EnumValue,
    ) -> Result<(), GraphError> {
        self.enum_values
            .lock()
            .unwrap()
            .entry(codelist_name.to_string())
            .or_default()
            .push(value.clone());
        Ok(())
    }

    async fn ingest_composite_column(&self, _col: &CompositeColumn) -> Result<(), GraphError> {
        Ok(())
    }

    async fn ingest_composite_range(&self, _range: &CompositeRange) -> Result<(), GraphError> {
        Ok(())
    }

    async fn ingest_extension(&self, _name: &str) -> Result<(), GraphError> {
        Ok(())
    }

    async fn ingest_edge(
        &self,
        from_id: &str,
        to_id: &str,
        edge_type: EdgeType,
        props: Option<&EdgeProperties>,
    ) -> Result<(), GraphError> {
        match edge_type {
            EdgeType::ContainsViewComponent => {
                let mut map = self.view_container_components.lock().unwrap();
                map.entry(strip_ifml_prefix(from_id).to_string())
                    .or_default()
                    .push(strip_ifml_prefix(to_id).to_string());
            }
            EdgeType::ContainsViewContainer => {
                let mut map = self.view_container_children.lock().unwrap();
                map.entry(strip_ifml_prefix(from_id).to_string())
                    .or_default()
                    .push(strip_ifml_prefix(to_id).to_string());
            }
            EdgeType::HasEvent => {
                let mut map = self.events_by_parent.lock().unwrap();
                map.entry(from_id.to_string())
                    .or_default()
                    .push(strip_ifml_prefix(to_id).to_string());
            }
            EdgeType::NavigationFlow => {
                let binding = props.and_then(|p| p.target_param_binding.clone());
                self.navigation_flows.lock().unwrap().push((
                    from_id.to_string(),
                    to_id.to_string(),
                    binding,
                ));
            }
            EdgeType::HasParameter => {
                let param = strip_ifml_prefix(to_id).to_string();
                let mut map = self.params_by_parent.lock().unwrap();
                map.entry(strip_ifml_prefix(from_id).to_string())
                    .or_default()
                    .push(param);
            }
            EdgeType::TriggersAction => {
                self.action_triggers.lock().unwrap().push((
                    strip_ifml_prefix(from_id).to_string(),
                    strip_ifml_prefix(to_id).to_string(),
                ));
            }
            EdgeType::DataFlow => {
                let props = props.cloned().unwrap_or_default();
                self.data_flows.lock().unwrap().push((
                    from_id.to_string(),
                    to_id.to_string(),
                    props.source_param,
                    props.target_param_binding,
                ));
            }
            EdgeType::HasDataBinding => {
                self.data_binding_edges.lock().unwrap().push((
                    strip_ifml_prefix(from_id).to_string(),
                    strip_ifml_prefix(to_id).to_string(),
                ));
            }
            EdgeType::BindsToEntity => {
                self.binding_entity_edges
                    .lock()
                    .unwrap()
                    .push((strip_ifml_prefix(from_id).to_string(), to_id.to_string()));
            }
            EdgeType::BindsToProperty => {
                let property = to_id.split_once("::").map(|(p, _)| p).unwrap_or(to_id);
                self.binding_property_edges
                    .lock()
                    .unwrap()
                    .push((strip_ifml_prefix(from_id).to_string(), property.to_string()));
            }
            EdgeType::HasInteraction => {
                self.op_interaction
                    .lock()
                    .unwrap()
                    .insert(strip_api_prefix(from_id).to_string(), to_id.to_string());
            }
            EdgeType::HasOperation => {
                self.resource_operations
                    .lock()
                    .unwrap()
                    .entry(strip_api_prefix(from_id).to_string())
                    .or_default()
                    .push(strip_api_prefix(to_id).to_string());
            }
            EdgeType::BindsHttpEndpoint => {
                self.interaction_endpoint
                    .lock()
                    .unwrap()
                    .insert(strip_ifml_prefix(from_id).to_string(), to_id.to_string());
            }
            EdgeType::FunctionExtends => {
                self.function_extends
                    .lock()
                    .unwrap()
                    .push((from_id.to_string(), to_id.to_string()));
            }
            EdgeType::RuleAppliesTo => {
                self.rule_applies_to
                    .lock()
                    .unwrap()
                    .push((from_id.to_string(), to_id.to_string()));
            }
            EdgeType::RuleReference => {
                let props = props.cloned().unwrap_or_default();
                self.rule_refs.lock().unwrap().push(RuleRefRecord {
                    schema_title: from_id.to_string(),
                    attribute: props.ref_path.unwrap_or_default(),
                    rule: to_id.to_string(),
                    rule_source: props.rule_source.unwrap_or_default(),
                });
            }
            // Namespace plane (issue #267): InNamespace links a Schema (by
            // schema_id) to a Namespace (by fqn).
            EdgeType::InNamespace => {
                // The AT-Protocol projection also uses InNamespace (Lexicon
                // nsid → AtprotoNamespace authority); those edges carry no
                // schema-side meaning here and are ignored.
                if let Some(ns) = self.type_namespaces.lock().unwrap().get(to_id).cloned() {
                    self.schema_in_namespace
                        .lock()
                        .unwrap()
                        .insert(from_id.to_string(), ns.fqn);
                }
            }
            EdgeType::NamespaceParent => {
                self.namespace_parent_edges
                    .lock()
                    .unwrap()
                    .push((from_id.to_string(), to_id.to_string()));
            }
            EdgeType::NamespaceImports => {
                let props = props.cloned().unwrap_or_default();
                self.namespace_imports
                    .lock()
                    .unwrap()
                    .push(NamespaceImport {
                        from_ns: from_id.to_string(),
                        to_ns: to_id.to_string(),
                        wildcard: props.import_wildcard.unwrap_or(false),
                        alias: props.import_alias,
                    });
            }
            // Derived, never ingested (issue #267).
            EdgeType::NamespaceDepends => {}
            _ => {}
        }
        Ok(())
    }

    async fn ingest_view_container(&self, node: &ViewContainerNode) -> Result<String, GraphError> {
        let id = format!("vc:{}", node.name);
        self.view_containers
            .lock()
            .unwrap()
            .insert(node.name.clone(), node.clone());
        Ok(id)
    }

    async fn ingest_module_definition(
        &self,
        node: &ModuleDefinitionNode,
    ) -> Result<String, GraphError> {
        let id = format!("module:{}", node.name);
        self.module_definitions
            .lock()
            .unwrap()
            .insert(node.name.clone(), node.clone());
        Ok(id)
    }

    async fn ingest_view_component(&self, node: &ViewComponentNode) -> Result<String, GraphError> {
        let id = format!("comp:{}", node.name);
        self.view_components
            .lock()
            .unwrap()
            .insert(node.name.clone(), node.clone());
        Ok(id)
    }

    async fn ingest_event(&self, node: &EventNode) -> Result<String, GraphError> {
        let id = format!("evt:{}", node.name);
        self.events
            .lock()
            .unwrap()
            .insert(node.name.clone(), node.clone());
        Ok(id)
    }

    async fn ingest_action_node(&self, node: &ActionNode) -> Result<String, GraphError> {
        let id = format!("action:{}", node.name);
        self.action_nodes
            .lock()
            .unwrap()
            .insert(node.name.clone(), node.clone());
        Ok(id)
    }

    async fn ingest_parameter_definition(
        &self,
        node: &ParameterDefinitionNode,
    ) -> Result<String, GraphError> {
        let id = format!("param:{}", node.name);
        self.parameter_definitions
            .lock()
            .unwrap()
            .insert(node.name.clone(), node.clone());
        Ok(id)
    }

    async fn ingest_data_binding(&self, node: &DataBindingNode) -> Result<String, GraphError> {
        let id = format!("db:{}", node.name);
        self.data_bindings
            .lock()
            .unwrap()
            .insert(node.name.clone(), node.clone());
        Ok(id)
    }

    async fn ingest_atproto_namespace(
        &self,
        node: &AtprotoNamespaceNode,
    ) -> Result<String, GraphError> {
        let authority = node.authority.clone();
        self.atproto_namespaces
            .lock()
            .unwrap()
            .insert(authority.clone(), node.clone());
        Ok(authority)
    }

    async fn ingest_namespace(&self, node: &NamespaceNode) -> Result<String, GraphError> {
        self.type_namespaces
            .lock()
            .unwrap()
            .insert(node.fqn.clone(), node.clone());
        if let Some(parent) = &node.parent {
            self.namespace_parent_edges
                .lock()
                .unwrap()
                .push((node.fqn.clone(), parent.clone()));
        }
        Ok(node.fqn.clone())
    }

    async fn ingest_namespace_import(&self, import: &NamespaceImport) -> Result<(), GraphError> {
        self.namespace_imports.lock().unwrap().push(import.clone());
        Ok(())
    }

    async fn ingest_lexicon(&self, node: &LexiconNode) -> Result<String, GraphError> {
        let nsid = node.nsid.clone();
        self.lexicons
            .lock()
            .unwrap()
            .insert(nsid.clone(), node.clone());
        Ok(nsid)
    }

    async fn ingest_collection(&self, node: &CollectionNode) -> Result<String, GraphError> {
        let nsid = node.nsid.clone();
        self.collections
            .lock()
            .unwrap()
            .insert(nsid.clone(), node.clone());
        Ok(nsid)
    }

    async fn ingest_repository(&self, node: &RepositoryNode) -> Result<String, GraphError> {
        let did = node.did.clone();
        self.repositories
            .lock()
            .unwrap()
            .insert(did.clone(), node.clone());
        Ok(did)
    }

    // ── API metamodel ingestion ───────────────────────────────────────

    async fn ingest_api_resource(&self, node: &ApiResourceNode) -> Result<String, GraphError> {
        let id = format!("ar:{}", node.name);
        self.api_resources
            .lock()
            .unwrap()
            .insert(id.clone(), node.clone());
        Ok(id)
    }

    async fn ingest_api_operation(&self, node: &ApiOperationNode) -> Result<String, GraphError> {
        let id = format!("ao:{}", node.name);
        self.api_operations
            .lock()
            .unwrap()
            .insert(id.clone(), node.clone());
        Ok(id)
    }

    async fn ingest_interaction(&self, node: &InteractionNode) -> Result<String, GraphError> {
        let id = format!("ia:{}", Uuid::new_v4());
        self.interactions
            .lock()
            .unwrap()
            .insert(id.clone(), node.clone());
        Ok(id)
    }

    async fn ingest_http_endpoint(&self, node: &HttpEndpointNode) -> Result<String, GraphError> {
        let id = format!("he:{}", Uuid::new_v4());
        self.http_endpoints
            .lock()
            .unwrap()
            .insert(id.clone(), node.clone());
        Ok(id)
    }

    async fn ingest_pipeline(&self, node: &PipelineNode) -> Result<String, GraphError> {
        let id = format!("pl:{}", node.name);
        self.pipelines
            .lock()
            .unwrap()
            .insert(id.clone(), node.clone());
        Ok(id)
    }

    async fn ingest_error_definition(
        &self,
        node: &ErrorDefinitionNode,
    ) -> Result<String, GraphError> {
        let id = format!("ed:{}", node.code);
        self.error_definitions
            .lock()
            .unwrap()
            .insert(id.clone(), node.clone());
        Ok(id)
    }

    async fn ingest_permission(&self, node: &PermissionNode) -> Result<String, GraphError> {
        let id = format!("pm:{}", node.name);
        self.permissions
            .lock()
            .unwrap()
            .insert(id.clone(), node.clone());
        Ok(id)
    }

    // ── Persistence metamodel ingestion ───────────────────────────────

    async fn ingest_policy(&self, policy: &PolicyNode) -> Result<(), GraphError> {
        self.policies
            .lock()
            .unwrap()
            .insert(policy.name.clone(), policy.clone());
        Ok(())
    }

    async fn ingest_relationship(&self, rel: &RelationshipNode) -> Result<(), GraphError> {
        self.relationships
            .lock()
            .unwrap()
            .insert(rel.name.clone(), rel.clone());
        Ok(())
    }

    async fn ingest_relationships(&self, rels: &[RelationshipNode]) -> Result<(), GraphError> {
        for rel in rels {
            self.relationships
                .lock()
                .unwrap()
                .insert(rel.name.clone(), rel.clone());
        }
        Ok(())
    }

    async fn ingest_security_identity(&self, id: &SecurityIdentityNode) -> Result<(), GraphError> {
        self.security_identities
            .lock()
            .unwrap()
            .insert(id.name.clone(), id.clone());
        Ok(())
    }

    async fn ingest_membership(&self, m: &MembershipNode) -> Result<(), GraphError> {
        self.memberships.lock().unwrap().push(m.clone());
        Ok(())
    }

    async fn ingest_tenant(&self, t: &TenantNode) -> Result<(), GraphError> {
        self.tenants
            .lock()
            .unwrap()
            .insert(t.name.clone(), t.clone());
        Ok(())
    }

    async fn ingest_condition(&self, node: &ConditionNode) -> Result<(), GraphError> {
        self.conditions.lock().unwrap().push(node.clone());
        Ok(())
    }

    async fn ingest_regulatory(&self, node: &RegulatoryNode) -> Result<(), GraphError> {
        self.regulatory.lock().unwrap().push(node.clone());
        Ok(())
    }

    async fn ingest_regulatory_reference(
        &self,
        owner: &RegulatoryOwner,
        target: &str,
        target_kind: RegulatoryKind,
        edge_kind: RegulatoryEdgeKind,
        ref_path: Option<&str>,
    ) -> Result<(), GraphError> {
        // Best-effort, mirroring the GQL MATCH semantics: no target node,
        // no edge.
        let known = self
            .regulatory
            .lock()
            .unwrap()
            .iter()
            .any(|n| n.name == target && n.kind == target_kind);
        if !known {
            return Ok(());
        }
        let (owner_name, owner_label) = match owner {
            RegulatoryOwner::Schema(title) => (title.clone(), "Schema".to_string()),
            RegulatoryOwner::Condition(name) => (name.clone(), "Condition".to_string()),
            RegulatoryOwner::Regulatory { name, .. } => (name.clone(), "Regulatory".to_string()),
            RegulatoryOwner::Function(name) => (name.clone(), "Function".to_string()),
            RegulatoryOwner::Rule(name) => (name.clone(), "Rule".to_string()),
        };
        self.regulatory_refs
            .lock()
            .unwrap()
            .push(RegulatoryRefRecord {
                owner: owner_name,
                owner_label,
                target: target.to_string(),
                target_kind,
                edge_kind,
                ref_path: ref_path.map(str::to_string),
            });
        Ok(())
    }

    async fn ingest_function(&self, node: &FunctionNode) -> Result<(), GraphError> {
        self.functions.lock().unwrap().push(node.clone());
        Ok(())
    }

    async fn ingest_rule(&self, node: &RuleNode) -> Result<(), GraphError> {
        self.rules.lock().unwrap().push(node.clone());
        Ok(())
    }

    async fn ingest_actor_policy(&self, model: &ActorPolicyModel) -> Result<(), GraphError> {
        {
            let mut actors = self.actors.lock().unwrap();
            for actor in &model.actors {
                actors.insert(actor.name.clone(), actor.clone());
            }
        }
        {
            let mut capabilities = self.capabilities.lock().unwrap();
            for capability in &model.capabilities {
                capabilities.insert(capability.name.clone(), capability.clone());
            }
        }
        self.grants
            .lock()
            .unwrap()
            .extend(model.grants.iter().cloned());
        *self.actor_policy.lock().unwrap() = Some(model.policy.clone());
        Ok(())
    }

    async fn ingest_ddd_model(&self, model: &DddModelGraph) -> Result<(), GraphError> {
        self.ddd_models.lock().unwrap().push(model.clone());
        Ok(())
    }

    async fn ingest_evt_model(&self, model: &EvtModelGraph) -> Result<(), GraphError> {
        self.evt_models.lock().unwrap().push(model.clone());
        Ok(())
    }

    async fn ingest_deploy_model(&self, model: &DeployModelGraph) -> Result<(), GraphError> {
        self.deploy_models.lock().unwrap().push(model.clone());
        Ok(())
    }

    async fn finalize(&self) -> Result<IngestStats, GraphError> {
        let schemas = self.schemas.lock().unwrap();
        let properties = self.properties.lock().unwrap();
        let codelists = self.codelists.lock().unwrap();
        let enum_values = self.enum_values.lock().unwrap();
        let vc = self.view_containers.lock().unwrap();
        let vcomp = self.view_components.lock().unwrap();
        let evt = self.events.lock().unwrap();
        let act = self.action_nodes.lock().unwrap();
        let param = self.parameter_definitions.lock().unwrap();

        let ifml_count = vc.len() + vcomp.len() + evt.len() + act.len() + param.len();
        let api_res = self.api_resources.lock().unwrap();

        Ok(IngestStats {
            schema_count: schemas.len(),
            property_count: properties.values().map(|v| v.len()).sum(),
            codelist_count: codelists.len(),
            enum_value_count: enum_values.values().map(|v| v.len()).sum(),
            ifml_node_count: ifml_count,
            api_resource_count: api_res.len(),
            duration: self.start_time.elapsed(),
            ..Default::default()
        })
    }

    async fn update_entity_flag(&self, title: &str, is_entity: bool) -> Result<(), GraphError> {
        let mut schemas = self.schemas.lock().unwrap();
        if let Some(schema) = schemas.get_mut(title) {
            schema.is_entity = is_entity;
        }
        Ok(())
    }

    async fn update_property_classification(
        &self,
        _schema_title: &str,
        _property_name: &str,
        _kind: &str,
    ) -> Result<(), GraphError> {
        // Mock: no-op for property classification updates
        Ok(())
    }
}
