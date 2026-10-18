use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use codegraph_core::error::GraphError;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::{
    DataBindingResolution, EventNode, NavigationFlowRecord, ViewContainerNode,
    resolve_effective_permits,
};
use rex_ifml::ComponentSpec;

use super::context::*;
use super::dependency_graph;

/// Parse a `ViewComponentNode.spec` JSON string into a typed spec.
///
/// Returns `None` when the raw value is absent or fails to deserialize —
/// a malformed spec never fails generation.
fn parse_component_spec(raw: Option<&str>) -> Option<ComponentSpec> {
    let raw = raw?;
    match serde_json::from_str::<ComponentSpec>(raw) {
        Ok(spec) => Some(spec),
        Err(err) => {
            tracing::warn!(%err, "ignoring unparseable IFML component spec");
            None
        }
    }
}

/// Strip the owning-view namespace from a stored view-level component
/// name. View-level components ingest as `{view}/{comp}` (so same-named
/// components in different views stay distinct graph nodes); generation
/// works with the bare name. Nested-container components persist bare and
/// pass through unchanged.
fn bare_component_name<'a>(stored: &'a str, container_name: &str) -> &'a str {
    stored
        .strip_prefix(container_name)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(stored)
}

/// Strip the owning-view namespace from a component event name. Events
/// hang off the namespaced component id, so a view-level `history`'s
/// select event persists as `comp_{view}/history_select`; the rendered
/// handler name stays `comp_history_select`.
fn bare_event_name(raw: &str, container_name: &str) -> String {
    raw.strip_prefix(&format!("comp_{container_name}/"))
        .map(|rest| format!("comp_{rest}"))
        .unwrap_or_else(|| raw.to_string())
}

/// Parse the persisted `target_param_binding` JSON into a map.
///
/// Absent or malformed bindings yield an empty map — a malformed binding
/// never fails generation.
fn parse_param_binding(raw: Option<&str>) -> HashMap<String, String> {
    let Some(raw) = raw else {
        return HashMap::new();
    };
    match serde_json::from_str::<HashMap<String, String>>(raw) {
        Ok(map) => map,
        Err(err) => {
            tracing::warn!(%err, "ignoring unparseable IFML target_param_binding");
            HashMap::new()
        }
    }
}

/// Per-model lookup tables resolving persisted event actions and data
/// bindings without re-querying the graph per event or component.
struct ActionIndex {
    flows: Vec<NavigationFlowRecord>,
    dataflow_targets: HashMap<String, String>,
    action_triggers: HashMap<String, String>,
}

impl ActionIndex {
    fn new(
        flows: Vec<NavigationFlowRecord>,
        data_flows: Vec<(String, String, Option<String>, Option<String>)>,
        action_triggers: Vec<(String, String)>,
    ) -> Self {
        let dataflow_targets = data_flows
            .into_iter()
            .map(|(source, target, _, _)| (source, target))
            .collect();
        let action_triggers = action_triggers.into_iter().collect();
        Self {
            flows,
            dataflow_targets,
            action_triggers,
        }
    }

    fn flow_for(&self, event_name: &str) -> Option<&NavigationFlowRecord> {
        self.flows.iter().find(|f| f.event == event_name)
    }

    /// Resolve the action persisted for an event: NavigationFlow → Navigate,
    /// DataFlow → Refresh, TriggersAction → Action, else Stay.
    fn action_for(&self, event_name: &str) -> IfmlAction {
        if let Some(flow) = self.flow_for(event_name) {
            return IfmlAction::Navigate {
                target: flow.target.clone(),
                binding: parse_param_binding(flow.target_param_binding.as_deref()),
            };
        }
        if let Some(target) = self.dataflow_targets.get(event_name) {
            return IfmlAction::Refresh {
                target: target.clone(),
                binding: HashMap::new(),
            };
        }
        if let Some(name) = self.action_triggers.get(event_name) {
            return IfmlAction::Action(name.clone());
        }
        IfmlAction::Stay
    }

    fn build_event(&self, evt: EventNode) -> IfmlEvent {
        IfmlEvent {
            action: self.action_for(&evt.name),
            name: evt.name,
            event_type: evt.event_type,
            params: evt.params.unwrap_or_default(),
            requires: evt.requires,
        }
    }

    fn navigation_edges(&self) -> Vec<NavigationEdge> {
        self.flows
            .iter()
            .map(|flow| {
                // Component-sourced flows carry the stored (view-namespaced)
                // component name; render contexts match components by bare
                // name, so strip the owning-view prefix.
                let source_component = (flow.source != flow.source_container).then(|| {
                    flow.source
                        .strip_prefix(&format!("{}/", flow.source_container))
                        .unwrap_or(&flow.source)
                        .to_string()
                });
                NavigationEdge {
                    source_container: flow.source_container.clone(),
                    source_event: bare_event_name(&flow.event, &flow.source_container),
                    target_container: flow.target.clone(),
                    parameter_binding: parse_param_binding(flow.target_param_binding.as_deref()),
                    conditional_expression: None,
                    source_component,
                }
            })
            .collect()
    }
}

/// Trait for querying the IFML model from the graph
#[async_trait]
pub trait IfmlQuerier: Send + Sync {
    async fn get_ifml_model(&self) -> Result<IfmlModel, GraphError>;
    async fn get_view_containers(&self) -> Result<Vec<IfmlViewContainer>, GraphError>;
    async fn get_view_container(&self, name: &str)
    -> Result<Option<IfmlViewContainer>, GraphError>;
    async fn get_navigation_edges(&self) -> Result<Vec<NavigationEdge>, GraphError>;
    async fn get_data_flows(&self) -> Result<Vec<DataFlowEdge>, GraphError>;
    async fn get_actions(&self) -> Result<Vec<IfmlActionDef>, GraphError>;
    async fn compute_generation_order(&self) -> Result<Vec<String>, GraphError>;
}

impl<'a> IfmlGraphQuerier<'a> {
    async fn action_index(&self) -> Result<ActionIndex, GraphError> {
        let flows = self.db.get_ifml_navigation_flows().await?;
        let data_flows = self.db.get_ifml_data_flows().await?;
        let action_triggers = self.db.get_ifml_action_triggers().await?;
        Ok(ActionIndex::new(flows, data_flows, action_triggers))
    }

    /// The ingested actor policy as a render-ready context: effective
    /// capabilities per actor via `resolve_effective_permits` (extends
    /// chain, forbid-wins), plus the capability inventory. `None` when the
    /// graph carries no policy.
    async fn policy_context(&self) -> Result<Option<PolicyContext>, GraphError> {
        let actors = self.db.get_actors().await?;
        let capabilities = self.db.get_capabilities().await?;
        if actors.is_empty() && capabilities.is_empty() {
            return Ok(None);
        }
        let grants = self.db.get_grants().await?;
        // Actors repeat across policies (every domain's `.actor` re-imports
        // the core ladder), so merge by NAME with the union of effective
        // permits — one entry per actor, not one per ingested policy.
        let mut merged: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
            Default::default();
        for actor in &actors {
            let entry = merged.entry(actor.name.clone()).or_default();
            for permit in resolve_effective_permits(&actors, &grants, &actor.name) {
                if permit.effect == "permit" {
                    entry.insert(permit.capability);
                }
            }
        }
        let actor_caps: Vec<(String, Vec<String>)> = merged
            .into_iter()
            .map(|(name, caps)| (name, caps.into_iter().collect()))
            .collect();
        let mut capability_names: Vec<String> =
            capabilities.into_iter().map(|cap| cap.name).collect();
        capability_names.sort();
        capability_names.dedup();
        Ok(Some(PolicyContext {
            actors: actor_caps,
            capabilities: capability_names,
        }))
    }

    async fn get_components_for(
        &self,
        container_name: &str,
        index: &ActionIndex,
        bindings: &[DataBindingResolution],
    ) -> Result<Vec<IfmlComponent>, GraphError> {
        let raw = self.db.get_ifml_view_components(container_name).await?;
        let mut components = Vec::new();
        for comp in &raw {
            // The stored name doubles as the graph key: namespaced for
            // view-level components (`{view}/{comp}`), bare for
            // nested-container components. Events and data bindings
            // resolve against the stored key; the render context carries
            // the bare name.
            let comp_id = format!("comp:{}", comp.name);
            let raw_events = self.db.get_ifml_events(&comp_id).await?;
            let events: Vec<IfmlEvent> = raw_events
                .into_iter()
                .map(|evt| {
                    let mut event = index.build_event(evt);
                    event.name = bare_event_name(&event.name, container_name);
                    event
                })
                .collect();

            let mut properties = HashMap::new();
            properties.insert("type".to_string(), comp.component_type.clone());
            if let Some(ref mode) = comp.mode {
                properties.insert("mode".to_string(), mode.clone());
            }

            let binding = bindings.iter().find(|b| b.component == comp.name);
            let fields_with_types = self.resolve_field_types(comp, binding).await?;

            components.push(IfmlComponent {
                name: bare_component_name(&comp.name, container_name).to_string(),
                component_type: comp.component_type.clone(),
                mode: comp.mode.clone(),
                entity: comp.entity.clone(),
                fields: comp.fields.clone().unwrap_or_default(),
                fields_with_types,
                filter: comp.filter.clone(),
                properties,
                events,
                parts: Vec::new(),
                spec: parse_component_spec(comp.spec.as_deref()),
            });
        }
        Ok(components)
    }

    async fn resolve_field_types(
        &self,
        comp: &codegraph_core::types::ViewComponentNode,
        binding: Option<&DataBindingResolution>,
    ) -> Result<Vec<(String, String)>, GraphError> {
        let Some(ref entity) = comp.entity else {
            return Ok(Vec::new());
        };
        let title = match binding {
            Some(resolution) => resolution.entity_title.clone(),
            None => self.resolve_schema_title_by_name(entity).await?,
        };
        let props = self.db.get_properties(&title).await?;
        let props_by_name: HashMap<String, String> = props
            .iter()
            .map(|p| (p.name.clone(), p.rust_field_type.clone()))
            .collect();
        // Typed form specs declare fields outside `fields`; fall back to the
        // spec's field names so fixtures and payload typing resolve too.
        let field_names = match comp.fields.clone().unwrap_or_default() {
            fields if !fields.is_empty() => fields,
            _ => match parse_component_spec(comp.spec.as_deref()) {
                Some(rex_ifml::ComponentSpec::Form(form)) => {
                    form.fields.iter().map(|f| f.name.clone()).collect()
                }
                _ => Vec::new(),
            },
        };
        Ok(field_names
            .iter()
            .filter_map(|f| props_by_name.get(f).map(|t| (f.clone(), t.clone())))
            .collect())
    }

    async fn resolve_schema_title_by_name(&self, entity: &str) -> Result<String, GraphError> {
        if self.db.get_schema(entity).await?.is_some() {
            return Ok(entity.to_string());
        }
        let suffixed = format!("{entity}Type");
        if self.db.get_schema(&suffixed).await?.is_some() {
            return Ok(suffixed);
        }
        Ok(entity.to_string())
    }
}

/// Implementation that queries the Grafeo graph
pub struct IfmlGraphQuerier<'a> {
    db: &'a dyn GraphQuerier,
}

impl<'a> IfmlGraphQuerier<'a> {
    pub fn new(db: &'a dyn GraphQuerier) -> Self {
        Self { db }
    }
    /// Assemble one container subtree (params, components, events, flags
    /// plus nested containers) from the adjacency map. Traversal is an
    /// explicit post-order stack so children are built before their parents
    /// without async recursion; children keep name order for deterministic
    /// output.
    async fn load_container_tree(
        &self,
        root: &ViewContainerNode,
        children_of: &HashMap<String, Vec<ViewContainerNode>>,
        index: &ActionIndex,
        bindings: &[DataBindingResolution],
    ) -> Result<IfmlViewContainer, GraphError> {
        let mut built: HashMap<String, IfmlViewContainer> = HashMap::new();
        let mut stack: Vec<(ViewContainerNode, bool)> = vec![(root.clone(), false)];
        while let Some((node, expanded)) = stack.pop() {
            if expanded {
                let mut containers = Vec::new();
                if let Some(children) = children_of.get(&node.name) {
                    for child in children {
                        if let Some(built_child) = built.remove(&child.name) {
                            containers.push(built_child);
                        }
                    }
                }
                let container = self
                    .build_container(&node, containers, index, bindings)
                    .await?;
                built.insert(node.name.clone(), container);
            } else {
                stack.push((node.clone(), true));
                if let Some(children) = children_of.get(&node.name) {
                    for child in children {
                        stack.push((child.clone(), false));
                    }
                }
            }
        }
        built
            .remove(&root.name)
            .ok_or_else(|| GraphError::NotFound(format!("container tree root {}", root.name)))
    }

    /// Resolve one container node (no nesting) into its render context.
    async fn build_container(
        &self,
        vc: &ViewContainerNode,
        containers: Vec<IfmlViewContainer>,
        index: &ActionIndex,
        bindings: &[DataBindingResolution],
    ) -> Result<IfmlViewContainer, GraphError> {
        let params: Vec<ParameterDef> = self
            .db
            .get_parameters_for_view(&vc.name)
            .await?
            .into_iter()
            .map(|p| ParameterDef {
                name: p.name,
                type_ref: p.type_ref,
                default: None,
            })
            .collect();
        let components = self.get_components_for(&vc.name, index, bindings).await?;
        let raw_events = self.db.get_ifml_events(&format!("vc:{}", vc.name)).await?;
        let events: Vec<IfmlEvent> = raw_events
            .into_iter()
            .map(|evt| index.build_event(evt))
            .collect();
        Ok(IfmlViewContainer {
            name: vc.name.clone(),
            label: vc.label.clone(),
            is_xor: vc.is_xor,
            is_default: vc.is_default,
            is_landmark: vc.is_landmark,
            is_modal: vc.is_modal,
            conditional_expression: vc.conditional_expression.clone(),
            conditional_expr_json: vc.expr_json.clone(),
            roles: vc.roles.clone().unwrap_or_default(),
            requires: vc.requires.clone().unwrap_or_default(),
            params,
            components,
            events,
            containers,
        })
    }
}

#[async_trait]
impl<'a> IfmlQuerier for IfmlGraphQuerier<'a> {
    async fn get_ifml_model(&self) -> Result<IfmlModel, GraphError> {
        let view_containers = self.get_view_containers().await?;
        let actions = self.get_actions().await?;
        let navigation_edges = self.get_navigation_edges().await?;
        let data_flows = self.get_data_flows().await?;
        let generation_order = self.compute_generation_order().await?;
        let policy = self.policy_context().await?;

        Ok(IfmlModel {
            view_containers,
            actions,
            navigation_edges,
            data_flows,
            generation_order,
            policy,
        })
    }

    async fn get_view_containers(&self) -> Result<Vec<IfmlViewContainer>, GraphError> {
        let index = self.action_index().await?;
        let bindings = self.db.get_data_bindings().await?;
        let raw_containers = self.db.get_ifml_view_containers().await?;

        // A container with an incoming ContainsViewContainer edge is nested;
        // only roots surface as top-level view containers.
        let mut nested: HashSet<String> = HashSet::new();
        let mut children_of: HashMap<String, Vec<ViewContainerNode>> = HashMap::new();
        for vc in &raw_containers {
            let children = self.db.get_ifml_container_children(&vc.name).await?;
            for child in &children {
                nested.insert(child.name.clone());
                children_of
                    .entry(vc.name.clone())
                    .or_default()
                    .push(child.clone());
            }
        }

        let mut containers = Vec::new();
        for vc in &raw_containers {
            if nested.contains(&vc.name) {
                continue;
            }
            containers.push(
                self.load_container_tree(vc, &children_of, &index, &bindings)
                    .await?,
            );
        }

        Ok(containers)
    }

    async fn get_view_container(
        &self,
        name: &str,
    ) -> Result<Option<IfmlViewContainer>, GraphError> {
        let containers = self.get_view_containers().await?;
        Ok(containers.into_iter().find(|c| c.name == name))
    }

    async fn get_navigation_edges(&self) -> Result<Vec<NavigationEdge>, GraphError> {
        let index = self.action_index().await?;
        Ok(index.navigation_edges())
    }

    async fn get_data_flows(&self) -> Result<Vec<DataFlowEdge>, GraphError> {
        let raw = self.db.get_ifml_data_flows().await?;
        Ok(raw
            .into_iter()
            .map(
                |(source, target, source_param, target_param)| DataFlowEdge {
                    source_element: source,
                    target_element: target,
                    source_param,
                    target_param,
                },
            )
            .collect())
    }

    async fn get_actions(&self) -> Result<Vec<IfmlActionDef>, GraphError> {
        let raw = self.db.get_ifml_actions().await?;
        Ok(raw
            .into_iter()
            .map(|a| IfmlActionDef {
                name: a.name,
                properties: HashMap::new(),
                events: Vec::new(),
            })
            .collect())
    }

    async fn compute_generation_order(&self) -> Result<Vec<String>, GraphError> {
        let nav = self.get_navigation_edges().await?;
        if !nav.is_empty() {
            let pairs: Vec<(String, String)> = nav
                .iter()
                .map(|e| (e.source_container.clone(), e.target_container.clone()))
                .collect();
            return Ok(dependency_graph::compute_view_generation_order(&pairs));
        }

        let containers = self.get_view_containers().await?;
        let mut names: Vec<String> = containers.into_iter().map(|c| c.name).collect();
        names.sort();
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codegraph_core::mock::MockEngine;
    use codegraph_core::traits::GraphIngestor;
    use codegraph_core::types::{
        EdgeProperties, EdgeType, ParameterDefinitionNode, ViewComponentNode, ViewContainerNode,
    };
    use rex_ifml::{ChartKind, ColumnDef, InputFieldType};

    async fn ingest_view_container(db: &MockEngine, name: &str, landmark: bool) {
        db.ingest_view_container(&ViewContainerNode {
            name: name.to_string(),
            label: None,
            is_xor: false,
            is_default: false,
            is_landmark: landmark,
            is_modal: false,
            conditional_expression: None,
            expr_json: None,
            domain: None,
            module_uses: None,
            roles: None,
            requires: None,
        })
        .await
        .unwrap();
    }

    async fn ingest_component(db: &MockEngine, parent: &str, name: &str, component_type: &str) {
        db.ingest_view_component(&ViewComponentNode {
            name: name.to_string(),
            component_type: component_type.to_string(),
            mode: None,
            entity: Some("Customer".to_string()),
            fields: Some(vec!["name".to_string()]),
            filter: None,
            api_operation: None,
            spec: None,
            conditional_expression: None,
            expr_json: None,
            domain: None,
        })
        .await
        .unwrap();
        db.ingest_edge(
            &format!("vc:{parent}"),
            &format!("comp:{name}"),
            EdgeType::ContainsViewComponent,
            None,
        )
        .await
        .unwrap();
    }

    async fn ingest_event(db: &MockEngine, parent: &str, name: &str, event_type: &str) {
        db.ingest_event(&EventNode {
            name: name.to_string(),
            event_type: event_type.to_string(),
            params: None,
            conditional_expression: None,
            expr_json: None,
            requires: Vec::new(),
            domain: None,
        })
        .await
        .unwrap();
        db.ingest_edge(
            &format!("comp:{parent}"),
            &format!("evt:{name}"),
            EdgeType::HasEvent,
            None,
        )
        .await
        .unwrap();
    }

    /// CustomerList(grid) --select--> CustomerDetail with a param binding.
    async fn ingest_customer_list_to_detail(db: &MockEngine) {
        ingest_view_container(db, "CustomerList", true).await;
        ingest_view_container(db, "CustomerDetail", false).await;
        ingest_component(db, "CustomerList", "grid", "list").await;
        ingest_event(db, "grid", "comp_grid_select", "select").await;
        db.ingest_edge(
            "evt:comp_grid_select",
            "vc:CustomerDetail",
            EdgeType::NavigationFlow,
            Some(&EdgeProperties {
                target_param_binding: Some(r#"{"customerId": "row.id"}"#.to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn component_event_navigate_action_is_resolved_with_binding() {
        let engine = MockEngine::new();
        ingest_customer_list_to_detail(&engine).await;

        let querier = IfmlGraphQuerier::new(&engine);
        let containers = querier.get_view_containers().await.unwrap();
        let list = containers
            .iter()
            .find(|c| c.name == "CustomerList")
            .expect("CustomerList container");
        let event = &list.components[0].events[0];
        assert_eq!(event.name, "comp_grid_select");
        match &event.action {
            IfmlAction::Navigate { target, binding } => {
                assert_eq!(target, "CustomerDetail");
                assert_eq!(
                    binding.get("customerId").map(String::as_str),
                    Some("row.id")
                );
            }
            other => panic!("expected Navigate action, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn component_event_navigation_resolves_to_parent_view() {
        let engine = MockEngine::new();
        ingest_customer_list_to_detail(&engine).await;

        let querier = IfmlGraphQuerier::new(&engine);
        let edges = querier.get_navigation_edges().await.unwrap();
        assert_eq!(edges.len(), 1);
        let edge = &edges[0];
        assert_eq!(edge.source_container, "CustomerList");
        assert_eq!(edge.source_component.as_deref(), Some("grid"));
        assert_eq!(edge.source_event, "comp_grid_select");
        assert_eq!(edge.target_container, "CustomerDetail");
        assert_eq!(
            edge.parameter_binding.get("customerId").map(String::as_str),
            Some("row.id")
        );
    }

    #[tokio::test]
    async fn view_params_are_populated_from_parameter_definitions() {
        let engine = MockEngine::new();
        ingest_customer_list_to_detail(&engine).await;
        engine
            .ingest_parameter_definition(&ParameterDefinitionNode {
                name: "customerId".to_string(),
                direction: "in".to_string(),
                type_ref: "Uuid".to_string(),
                domain: None,
            })
            .await
            .unwrap();
        engine
            .ingest_edge(
                "vc:CustomerDetail",
                "param:customerId",
                EdgeType::HasParameter,
                Some(&EdgeProperties {
                    direction: Some("in".to_string()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();

        let querier = IfmlGraphQuerier::new(&engine);
        let detail = querier
            .get_view_container("CustomerDetail")
            .await
            .unwrap()
            .expect("CustomerDetail container");
        assert_eq!(detail.params.len(), 1);
        assert_eq!(detail.params[0].name, "customerId");
        assert_eq!(detail.params[0].type_ref, "Uuid");

        let list = querier
            .get_view_container("CustomerList")
            .await
            .unwrap()
            .expect("CustomerList container");
        assert!(list.params.is_empty());
    }

    #[tokio::test]
    async fn action_invocation_refresh_and_stay_actions_are_resolved() {
        let engine = MockEngine::new();
        ingest_view_container(&engine, "Dashboard", false).await;
        ingest_component(&engine, "Dashboard", "form", "form").await;
        ingest_component(&engine, "Dashboard", "chart", "chart").await;
        ingest_event(&engine, "form", "comp_form_save", "save").await;
        ingest_event(&engine, "form", "comp_form_cancel", "cancel").await;
        ingest_event(&engine, "chart", "comp_chart_load", "load").await;

        engine
            .ingest_action_node(&codegraph_core::types::ActionNode {
                name: "UpdateCustomer".to_string(),
                domain: None,
            })
            .await
            .unwrap();
        engine
            .ingest_edge(
                "evt:comp_form_save",
                "action:UpdateCustomer",
                EdgeType::TriggersAction,
                None,
            )
            .await
            .unwrap();
        engine
            .ingest_edge(
                "evt:comp_chart_load",
                "comp:chart",
                EdgeType::DataFlow,
                None,
            )
            .await
            .unwrap();

        let querier = IfmlGraphQuerier::new(&engine);
        let dashboard = querier
            .get_view_container("Dashboard")
            .await
            .unwrap()
            .expect("Dashboard container");
        let action_of = |name: &str| {
            dashboard
                .components
                .iter()
                .flat_map(|c| c.events.iter())
                .find(|e| e.name == name)
                .map(|e| e.action.clone())
                .unwrap_or_else(|| panic!("event {name} not found"))
        };
        assert_eq!(
            action_of("comp_form_save"),
            IfmlAction::Action("UpdateCustomer".to_string())
        );
        assert_eq!(action_of("comp_form_cancel"), IfmlAction::Stay);
        assert_eq!(
            action_of("comp_chart_load"),
            IfmlAction::Refresh {
                target: "chart".to_string(),
                binding: HashMap::new()
            }
        );
    }

    #[tokio::test]
    async fn view_roles_round_trip_through_mock_engine() {
        let engine = MockEngine::new();
        engine
            .ingest_view_container(&ViewContainerNode {
                name: "AdminConsole".to_string(),
                label: None,
                is_xor: false,
                is_default: false,
                is_landmark: false,
                is_modal: false,
                conditional_expression: None,
                expr_json: None,
                domain: None,
                module_uses: None,
                roles: Some(vec!["admin".to_string(), "manager".to_string()]),
                requires: Some(vec!["manage_refunds".to_string()]),
            })
            .await
            .unwrap();
        ingest_view_container(&engine, "Public", false).await;

        let querier = IfmlGraphQuerier::new(&engine);
        let containers = querier.get_view_containers().await.unwrap();
        let admin = containers
            .iter()
            .find(|c| c.name == "AdminConsole")
            .expect("AdminConsole container");
        assert_eq!(
            admin.roles,
            vec!["admin".to_string(), "manager".to_string()]
        );
        assert_eq!(admin.requires, vec!["manage_refunds".to_string()]);
        let public = containers
            .iter()
            .find(|c| c.name == "Public")
            .expect("Public container");
        assert!(public.roles.is_empty());
        assert!(public.requires.is_empty());
    }

    #[tokio::test]
    async fn policy_context_resolves_effective_capabilities_per_actor() {
        use codegraph_core::types::{
            ActorNode, ActorPolicyModel, ActorPolicyNode, CapabilityNode, GrantEdge,
        };
        let engine = MockEngine::new();
        engine
            .ingest_actor_policy(&ActorPolicyModel {
                actors: vec![
                    ActorNode {
                        name: "Admin".to_string(),
                        kind: Some("human".to_string()),
                        extends: None,
                        block: Some("core".to_string()),
                    },
                    ActorNode {
                        name: "Manager".to_string(),
                        kind: Some("human".to_string()),
                        extends: Some("Admin".to_string()),
                        block: Some("core".to_string()),
                    },
                ],
                capabilities: vec![
                    CapabilityNode {
                        name: "manage_refunds".to_string(),
                        class: "Refund".to_string(),
                        block: None,
                    },
                    CapabilityNode {
                        name: "approve_expense".to_string(),
                        class: "Expense".to_string(),
                        block: None,
                    },
                ],
                grants: vec![
                    GrantEdge {
                        actor: "Admin".to_string(),
                        capability: "manage_refunds".to_string(),
                        effect: "permit".to_string(),
                        when: None,
                        expr_json: None,
                        obligations: vec![],
                    },
                    GrantEdge {
                        actor: "Admin".to_string(),
                        capability: "approve_expense".to_string(),
                        effect: "permit".to_string(),
                        when: None,
                        expr_json: None,
                        obligations: vec![],
                    },
                    GrantEdge {
                        actor: "Manager".to_string(),
                        capability: "approve_expense".to_string(),
                        effect: "forbid".to_string(),
                        when: None,
                        expr_json: None,
                        obligations: vec![],
                    },
                ],
                policy: ActorPolicyNode {
                    blocks: vec!["core".to_string()],
                    never_both: vec![],
                    purposes: vec![],
                    delegations: vec![],
                },
            })
            .await
            .unwrap();

        let querier = IfmlGraphQuerier::new(&engine);
        let model = querier.get_ifml_model().await.unwrap();
        let policy = model.policy.expect("policy context");
        assert_eq!(
            policy.actors,
            vec![
                (
                    "Admin".to_string(),
                    vec!["approve_expense".to_string(), "manage_refunds".to_string()]
                ),
                ("Manager".to_string(), vec!["manage_refunds".to_string()]),
            ]
        );
        assert_eq!(
            policy.capabilities,
            vec!["approve_expense".to_string(), "manage_refunds".to_string()]
        );
    }

    async fn ingest_nested_container(
        db: &MockEngine,
        parent: &str,
        name: &str,
        label: Option<&str>,
        is_default: bool,
    ) {
        db.ingest_view_container(&ViewContainerNode {
            name: name.to_string(),
            label: label.map(str::to_string),
            is_xor: false,
            is_default,
            is_landmark: false,
            is_modal: false,
            conditional_expression: None,
            expr_json: None,
            domain: None,
            module_uses: None,
            roles: None,
            requires: None,
        })
        .await
        .unwrap();
        db.ingest_edge(
            &format!("vc:{parent}"),
            &format!("vc:{name}"),
            EdgeType::ContainsViewContainer,
            None,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn nested_containers_nest_under_their_view_not_top_level() {
        let engine = MockEngine::new();
        ingest_view_container(&engine, "Dashboard", true).await;
        ingest_nested_container(&engine, "Dashboard", "Sidebar", Some("Navigation"), true).await;

        let querier = IfmlGraphQuerier::new(&engine);
        let containers = querier.get_view_containers().await.unwrap();
        let mut top_names: Vec<&str> = containers.iter().map(|c| c.name.as_str()).collect();
        top_names.sort();
        assert_eq!(
            top_names,
            vec!["Dashboard"],
            "nested containers must not appear as top-level view containers: {top_names:?}"
        );
        assert_eq!(containers[0].containers.len(), 1, "{containers:?}");
        let sidebar = &containers[0].containers[0];
        assert_eq!(sidebar.name, "Sidebar");
        assert_eq!(sidebar.label.as_deref(), Some("Navigation"));
        assert!(sidebar.is_default, "container default flag must round-trip");
    }

    #[tokio::test]
    async fn nested_container_contents_attach_to_the_container() {
        let engine = MockEngine::new();
        ingest_view_container(&engine, "Dashboard", true).await;
        ingest_nested_container(&engine, "Dashboard", "Sidebar", None, false).await;
        ingest_component(&engine, "Sidebar", "nav", "menu").await;
        ingest_component(&engine, "Dashboard", "grid", "list").await;
        engine
            .ingest_event(&EventNode {
                name: "vc_Sidebar_toggle".to_string(),
                event_type: "click".to_string(),
                params: None,
                conditional_expression: None,
                expr_json: None,
                requires: Vec::new(),
                domain: None,
            })
            .await
            .unwrap();
        engine
            .ingest_edge(
                "vc:Sidebar",
                "evt:vc_Sidebar_toggle",
                EdgeType::HasEvent,
                None,
            )
            .await
            .unwrap();

        let querier = IfmlGraphQuerier::new(&engine);
        let containers = querier.get_view_containers().await.unwrap();
        assert_eq!(
            containers.len(),
            1,
            "only the view itself is a top-level entry: {containers:?}"
        );
        let dashboard = &containers[0];
        assert_eq!(
            dashboard
                .components
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["grid"],
            "view-level components stay on the view"
        );
        assert_eq!(dashboard.containers.len(), 1, "{dashboard:?}");
        let sidebar = &dashboard.containers[0];
        assert_eq!(
            sidebar
                .components
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["nav"],
            "container components attach to their owning container"
        );
        assert_eq!(
            sidebar
                .events
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["vc_Sidebar_toggle"],
            "container events attach to their owning container"
        );
    }

    #[tokio::test]
    async fn nested_container_conditional_expression_round_trips() {
        let engine = MockEngine::new();
        ingest_view_container(&engine, "Dashboard", true).await;
        engine
            .ingest_view_container(&ViewContainerNode {
                name: "Promo".to_string(),
                label: Some("Promo".to_string()),
                is_xor: false,
                is_default: false,
                is_landmark: false,
                is_modal: false,
                conditional_expression: Some("user.subscribed == true".to_string()),
                expr_json: None,
                domain: None,
                module_uses: None,
                roles: None,
                requires: None,
            })
            .await
            .unwrap();
        engine
            .ingest_edge(
                "vc:Dashboard",
                "vc:Promo",
                EdgeType::ContainsViewContainer,
                None,
            )
            .await
            .unwrap();

        let querier = IfmlGraphQuerier::new(&engine);
        let containers = querier.get_view_containers().await.unwrap();
        assert_eq!(containers.len(), 1);
        let promo = &containers[0].containers[0];
        assert_eq!(promo.name, "Promo");
        assert_eq!(
            promo.conditional_expression.as_deref(),
            Some("user.subscribed == true"),
            "container guards must round-trip into the view tree"
        );
    }

    #[tokio::test]
    async fn policy_context_is_none_without_policy() {
        let engine = MockEngine::new();
        ingest_view_container(&engine, "Public", false).await;
        let querier = IfmlGraphQuerier::new(&engine);
        let model = querier.get_ifml_model().await.unwrap();
        assert!(model.policy.is_none());
    }

    #[tokio::test]
    async fn same_named_components_in_different_views_stay_isolated() {
        // The wellness/compensation collision shape: two views in different
        // domains each declare a view-level component named "history". The
        // ingest driver stores view-level components under `{view}/{comp}`,
        // so each view must surface ONLY its own component, its own
        // events (under bare handler names), and its own entity binding.
        let engine = MockEngine::new();
        ingest_view_container(&engine, "ActivityDashboard", true).await;
        ingest_view_container(&engine, "CompensationHistory", true).await;

        // View-level components, persisted exactly as the ingest driver
        // stores them: namespaced node names, `HasEvent` parents and event
        // node names derived from the namespaced id.
        for (view, entity, event_type) in [
            ("ActivityDashboard", "WellnessActivity", "create"),
            ("CompensationHistory", "Reward", "select"),
        ] {
            let comp_name = format!("{view}/history");
            engine
                .ingest_view_component(&ViewComponentNode {
                    name: comp_name.clone(),
                    component_type: "list".to_string(),
                    mode: None,
                    entity: Some(entity.to_string()),
                    fields: Some(vec!["name".to_string()]),
                    filter: None,
                    api_operation: None,
                    spec: None,
                    conditional_expression: None,
                    expr_json: None,
                    domain: None,
                })
                .await
                .unwrap();
            engine
                .ingest_edge(
                    &format!("vc:{view}"),
                    &format!("comp:{comp_name}"),
                    EdgeType::ContainsViewComponent,
                    None,
                )
                .await
                .unwrap();
            let event_name = format!("comp_{comp_name}_{event_type}");
            engine
                .ingest_event(&EventNode {
                    name: event_name.clone(),
                    event_type: event_type.to_string(),
                    params: None,
                    conditional_expression: None,
                    expr_json: None,
                    requires: Vec::new(),
                    domain: None,
                })
                .await
                .unwrap();
            engine
                .ingest_edge(
                    &format!("comp:{comp_name}"),
                    &format!("evt:{event_name}"),
                    EdgeType::HasEvent,
                    None,
                )
                .await
                .unwrap();
        }
        // The compensation history navigates on select.
        engine
            .ingest_edge(
                "evt:comp_CompensationHistory/history_select",
                "vc:RewardEditor",
                EdgeType::NavigationFlow,
                None,
            )
            .await
            .unwrap();

        let querier = IfmlGraphQuerier::new(&engine);
        let containers = querier.get_view_containers().await.unwrap();

        let dashboard = containers
            .iter()
            .find(|c| c.name == "ActivityDashboard")
            .expect("ActivityDashboard container");
        assert_eq!(
            dashboard
                .components
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["history"],
            "only the view's own component surfaces, under its bare name"
        );
        let history = &dashboard.components[0];
        assert_eq!(history.entity.as_deref(), Some("WellnessActivity"));
        assert_eq!(
            history
                .events
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["comp_history_create"],
            "only the view's own event surfaces, under its bare handler name"
        );

        let compensation = containers
            .iter()
            .find(|c| c.name == "CompensationHistory")
            .expect("CompensationHistory container");
        assert_eq!(compensation.components.len(), 1);
        let comp_history = &compensation.components[0];
        assert_eq!(comp_history.name, "history");
        assert_eq!(comp_history.entity.as_deref(), Some("Reward"));
        assert_eq!(
            comp_history
                .events
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["comp_history_select"],
        );

        let edges = querier.get_navigation_edges().await.unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source_container, "CompensationHistory");
        assert_eq!(edges[0].source_component.as_deref(), Some("history"));
        assert_eq!(edges[0].source_event, "comp_history_select");
        assert_eq!(edges[0].target_container, "RewardEditor");
    }

    #[test]
    fn parse_component_spec_none_when_absent() {
        assert!(parse_component_spec(None).is_none());
        assert!(parse_component_spec(Some("")).is_none());
    }

    #[test]
    fn parse_component_spec_none_on_garbage() {
        assert!(parse_component_spec(Some("not json")).is_none());
        assert!(parse_component_spec(Some("{\"Unknown\": {}}")).is_none());
    }

    #[test]
    fn parse_component_spec_table() {
        let raw = r#"{"type":"table","value":{"columns":[
            {"type":"field","value":{"label":"Name","field":{"entity":"Customer","property":"name"}}},
            {"type":"lookup","value":{"label":"Status","field":{"entity":"Customer","property":"status"},"lookup":"status_labels"}},
            {"type":"expression","value":{"label":"Tenure","expr":{"type":"call","value":{"name":"tenure_years","args":[{"type":"fieldExpr","value":{"object":{"type":"ident","value":"Customer"},"field":"hire_date"}}]}}}}
        ],"pagination":true}}"#;
        let spec = parse_component_spec(Some(raw)).expect("table spec should parse");
        let ComponentSpec::Table(table) = spec else {
            panic!("expected table spec, got {spec:?}");
        };
        assert!(table.pagination);
        assert_eq!(table.columns.len(), 3);
        assert!(matches!(table.columns[0], ColumnDef::Field { .. }));
        assert!(matches!(table.columns[1], ColumnDef::Lookup { .. }));
        assert!(matches!(table.columns[2], ColumnDef::Expression { .. }));
    }

    #[test]
    fn parse_component_spec_form_and_chart() {
        let form_raw = r#"{"type":"form","value":{"fields":[
            {"name":"email","input":{"type":"email"},"required":true,"validations":[],"values":[]}
        ]}}"#;
        let spec = parse_component_spec(Some(form_raw)).expect("form spec should parse");
        let ComponentSpec::Form(form) = spec else {
            panic!("expected form spec, got {spec:?}");
        };
        assert_eq!(form.fields[0].input, InputFieldType::Email);
        assert!(form.fields[0].required);

        let chart_raw = r#"{"type":"chart","value":{"kind":"bar","labelField":"region","valueFields":["revenue"]}}"#;
        let spec = parse_component_spec(Some(chart_raw)).expect("chart spec should parse");
        let ComponentSpec::Chart(chart) = spec else {
            panic!("expected chart spec, got {spec:?}");
        };
        assert_eq!(chart.kind, ChartKind::Bar);
        assert_eq!(chart.label_field.as_deref(), Some("region"));
    }
}
