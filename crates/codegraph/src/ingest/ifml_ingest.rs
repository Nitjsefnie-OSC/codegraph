use codegraph_core::traits::GraphIngestor;
use codegraph_core::types::{
    ActionNode, DataBindingNode, EdgeProperties, EdgeType, EventNode, ModuleDefinitionNode,
    ModuleUseRecord, ParameterDefinitionNode, ViewComponentNode, ViewContainerNode,
};
use rex_ifml::*;

use crate::error::{Error, Result};

/// Map DSL module-use statements to their persisted graph form; `None` when
/// the declaration carries no uses.
fn module_use_records(uses: &[ModuleUse]) -> Option<Vec<ModuleUseRecord>> {
    if uses.is_empty() {
        return None;
    }
    Some(
        uses.iter()
            .map(|u| ModuleUseRecord {
                module: u.module.clone(),
                alias: u.alias.clone(),
            })
            .collect(),
    )
}

/// Render a property `ValueExpression` back to its DSL source form
/// (deterministic: spaced binary operators, direct unary prefixes, explicit
/// groups) — the persistence form for composite filter expressions.
fn render_value_expression(expr: &ValueExpression) -> String {
    match expr {
        ValueExpression::Identifier(s) => s.clone(),
        ValueExpression::String(s) => format!("\"{s}\""),
        ValueExpression::Number(n) => n.value().to_string(),
        ValueExpression::Bool(b) => b.to_string(),
        ValueExpression::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(render_value_expression)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ValueExpression::Object(members) => format!(
            "{{ {} }}",
            members
                .iter()
                .map(|m| format!("{}: {}", m.key, render_value_expression(&m.value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ValueExpression::Call(name, args) => format!(
            "{}({})",
            name,
            args.iter()
                .map(render_value_expression)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ValueExpression::FieldAccess { object, field } => {
            format!("{}.{}", render_value_expression(object), field)
        }
        ValueExpression::BinOp { left, op, right } => format!(
            "{} {} {}",
            render_value_expression(left),
            op.as_str(),
            render_value_expression(right)
        ),
        ValueExpression::UnaryOp { op, operand } => {
            format!("{}{}", op.as_str(), render_value_expression(operand))
        }
        ValueExpression::Group(inner) => format!("({})", render_value_expression(inner)),
    }
}

/// Canonical AST JSON for a DSL condition: the typed IFML `Expression`
/// serialized once at ingest (issue #278). `None` when the node is
/// unconditional or the payload fails to serialize — ingest never
/// fails on an expression.
fn expr_json(condition: Option<&Expression>) -> Option<String> {
    condition.and_then(|expr| serde_json::to_string(expr).ok())
}

/// Ingest a parsed IFML model into the graph database.
pub async fn ingest_ifml_model(
    db: &dyn GraphIngestor,
    model: &IfmlModel,
) -> Result<IfmlIngestStats> {
    let mut stats = IfmlIngestStats::default();

    // Pass 0: ingest module declarations before any view content — the
    // `HasModuleDefinition` edges from view/module `use` statements resolve
    // against these nodes (edge INSERTs silently no-op on missing targets).
    // Module-internal `use` statements (rexlang module composition) persist
    // on the module node AND as module→module edges.
    for module in &model.modules {
        let node = ModuleDefinitionNode {
            name: module.name.clone(),
            domain: model.domains.first().map(|d| d.name.clone()),
            inputs_json: Some(serde_json::to_string(&module.input_params).unwrap_or_default()),
            outputs_json: Some(serde_json::to_string(&module.output_params).unwrap_or_default()),
            properties_json: Some(serde_json::to_string(&module.properties).unwrap_or_default()),
            module_uses: module_use_records(&module.module_uses),
        };
        db.ingest_module_definition(&node).await?;
        stats.module_uses += module.module_uses.len();
        for use_ in &module.module_uses {
            db.ingest_edge(
                &format!("module:{}", module.name),
                &format!("module:{}", use_.module),
                EdgeType::HasModuleDefinition,
                None,
            )
            .await?;
        }
    }

    // Pass 1: ingest all view containers and nested containers before any
    // content. Navigation flows (and other edges) target containers that may
    // be declared later in the file, and edge INSERTs silently no-op when the
    // target node does not exist yet.
    for view in &model.views {
        let vc_id = ingest_view_container(db, view).await?;
        stats.view_containers += 1;
        stats.module_uses += view.module_uses.len();
        for use_ in &view.module_uses {
            db.ingest_edge(
                &vc_id,
                &format!("module:{}", use_.module),
                EdgeType::HasModuleDefinition,
                None,
            )
            .await?;
        }

        for container in &view.containers {
            ingest_container_tree_nodes(db, container, &mut stats).await?;
        }
    }

    // Pass 2: ingest container params, components, and events.
    for view in &model.views {
        let vc_id = format!("vc:{}", view.name);

        // Ingest container params
        for param in &view.params {
            let param_id = db
                .ingest_parameter_definition(&ParameterDefinitionNode {
                    name: param.name.clone(),
                    direction: "in".to_string(),
                    type_ref: param.type_ref.clone(),
                    domain: None,
                })
                .await
                .map_err(Error::Graph)?;
            stats.parameters += 1;

            db.ingest_edge(
                &vc_id,
                &param_id,
                EdgeType::HasParameter,
                Some(&EdgeProperties {
                    direction: Some("in".to_string()),
                    ..Default::default()
                }),
            )
            .await
            .map_err(Error::Graph)?;
        }

        // Ingest nested container contents
        for (i, container) in view.containers.iter().enumerate() {
            ingest_container_contents(db, container, i, &vc_id).await?;
        }

        // Ingest view components. View-level components persist under a
        // view-namespaced identity (`{view}/{comp}`): the graph layer
        // resolves every IFML edge endpoint and query by node name, so a
        // bare name would fuse same-named components across views (every
        // view's "history" would share one node, one event set, and one
        // data binding). Events and data bindings hang off this id and
        // inherit the namespace. Nested-container components stay bare —
        // same-named nested containers deliberately pool their contents
        // (one deduped container node).
        for comp in &view.components {
            let _comp_id = ingest_view_component(db, comp, &vc_id, Some(&view.name)).await?;
            stats.components += 1;
        }

        // Ingest view-level events
        for event in &view.events {
            handle_event(db, event, &vc_id).await?;
            stats.events += 1;
        }
    }

    // Ingest standalone actions
    for action in &model.actions {
        let action_id = db
            .ingest_action_node(&ActionNode {
                name: action.name.clone(),
                domain: None,
            })
            .await
            .map_err(Error::Graph)?;
        stats.actions += 1;

        // Ingest action body events
        for event in &action.events {
            handle_event(db, event, &action_id).await?;
            stats.events += 1;
        }
    }

    // Actor declarations are parsed and counted but not yet persisted;
    // a dedicated node type lands with the roles/permissions slice.
    stats.actors += model.actors.len();

    Ok(stats)
}

async fn ingest_view_container(db: &dyn GraphIngestor, view: &ViewDeclaration) -> Result<String> {
    let node = ViewContainerNode {
        name: view.name.clone(),
        label: view.label.clone(),
        is_xor: view.is_xor,
        is_default: false,
        is_landmark: view.is_landmark,
        is_modal: view.is_modal,
        conditional_expression: view.condition.as_ref().map(render_expression),
        expr_json: expr_json(view.condition.as_ref()),
        domain: None,
        module_uses: module_use_records(&view.module_uses),
        roles: if view.roles.is_empty() {
            None
        } else {
            Some(view.roles.clone())
        },
        requires: if view.requires.is_empty() {
            None
        } else {
            Some(view.requires.clone())
        },
    };
    let id = db
        .ingest_view_container(&node)
        .await
        .map_err(Error::Graph)?;
    Ok(id)
}

/// Ingest a container and every container nested inside it (nodes only —
/// contents are wired in pass 2), so parent lookups resolve regardless of
/// declaration order.
async fn ingest_container_tree_nodes(
    db: &dyn GraphIngestor,
    container: &ContainerDeclaration,
    stats: &mut IfmlIngestStats,
) -> Result<()> {
    let _container_id = ingest_container_node(db, container).await?;
    stats.containers += 1;
    stats.module_uses += container.module_uses.len();
    for nested in &container.containers {
        Box::pin(ingest_container_tree_nodes(db, nested, stats)).await?;
    }
    Ok(())
}

async fn ingest_container_node(
    db: &dyn GraphIngestor,
    container: &ContainerDeclaration,
) -> Result<String> {
    let node = ViewContainerNode {
        name: container.name.clone(),
        label: container.label.clone(),
        is_xor: container.is_xor,
        is_default: container.is_default,
        is_landmark: false,
        is_modal: false,
        conditional_expression: container.condition.as_ref().map(render_expression),
        expr_json: expr_json(container.condition.as_ref()),
        domain: None,
        module_uses: module_use_records(&container.module_uses),
        roles: None,
        requires: None,
    };
    db.ingest_view_container(&node).await.map_err(Error::Graph)
}

async fn ingest_container_contents(
    db: &dyn GraphIngestor,
    container: &ContainerDeclaration,
    sort_order: usize,
    parent_id: &str,
) -> Result<()> {
    let container_id = format!("vc:{}", container.name);

    // Link to parent; sort_order keeps declaration order so sibling groups
    // (e.g. xor wizard steps) render in source order.
    db.ingest_edge(
        parent_id,
        &container_id,
        EdgeType::ContainsViewContainer,
        Some(&EdgeProperties {
            sort_order: Some(i32::try_from(sort_order).unwrap_or(i32::MAX)),
            ..Default::default()
        }),
    )
    .await
    .map_err(Error::Graph)?;

    // Ingest container's components (bare names — nested-container
    // components pool under the deduped container node by design).
    for comp in &container.components {
        let comp_id = ingest_view_component(db, comp, &container_id, None).await?;
        let _ = comp_id;
    }

    // Ingest container's events
    for event in &container.events {
        handle_event(db, event, &container_id).await?;
    }

    // Recurse into nested containers
    for (i, nested) in container.containers.iter().enumerate() {
        Box::pin(ingest_container_contents(db, nested, i, &container_id)).await?;
    }

    Ok(())
}

async fn ingest_view_component(
    db: &dyn GraphIngestor,
    comp: &ComponentDeclaration,
    parent_id: &str,
    namespace: Option<&str>,
) -> Result<String> {
    let component_type = comp
        .component_type
        .as_ref()
        .map(|t| t.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let spec = comp
        .spec
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(Error::Json)?;

    let entity = comp
        .properties
        .iter()
        .find(|p| p.key == "data")
        .and_then(|p| match &p.value {
            ValueExpression::Identifier(s) => Some(s.clone()),
            _ => None,
        });

    let fields = comp
        .properties
        .iter()
        .find(|p| p.key == "fields")
        .and_then(|p| match &p.value {
            ValueExpression::Array(items) => Some(
                items
                    .iter()
                    .filter_map(|v| match v {
                        ValueExpression::Identifier(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        });

    let filter = comp
        .properties
        .iter()
        .find(|p| p.key == "filter")
        .and_then(|p| match &p.value {
            ValueExpression::Identifier(s) => Some(s.clone()),
            // Composite expressions (deep navigation chains, comparisons)
            // persist in their canonical DSL source form.
            ValueExpression::FieldAccess { .. }
            | ValueExpression::BinOp { .. }
            | ValueExpression::UnaryOp { .. }
            | ValueExpression::Group(_)
            | ValueExpression::Call(_, _) => Some(render_value_expression(&p.value)),
            _ => None,
        });

    let mode = comp
        .properties
        .iter()
        .find(|p| p.key == "mode")
        .and_then(|p| match &p.value {
            ValueExpression::Identifier(s) => Some(s.clone()),
            _ => None,
        });

    let api_operation = comp
        .properties
        .iter()
        .find(|p| p.key == "api")
        .and_then(|p| match &p.value {
            ValueExpression::Identifier(s) => Some(s.clone()),
            _ => None,
        });

    // View-level components carry the owning view as a namespace; the
    // stored name doubles as the globally-unique graph key every edge and
    // query resolves against.
    let stored_name = match namespace {
        Some(view) => format!("{view}/{}", comp.name),
        None => comp.name.clone(),
    };

    let node = ViewComponentNode {
        name: stored_name,
        component_type,
        mode,
        entity,
        fields,
        filter,
        api_operation,
        spec,
        conditional_expression: comp.condition.as_ref().map(render_expression),
        expr_json: expr_json(comp.condition.as_ref()),
        domain: None,
    };

    let id = db
        .ingest_view_component(&node)
        .await
        .map_err(Error::Graph)?;

    // Edge: parent contains component
    db.ingest_edge(parent_id, &id, EdgeType::ContainsViewComponent, None)
        .await
        .map_err(Error::Graph)?;

    // Data binding activation: when the component declares `data: <entity>`,
    // create a DataBinding node plus BindsToEntity / BindsToProperty edges.
    if let Some(ref entity) = node.entity {
        let binding_name = format!("{}:{}", node.name, entity);
        let binding_id = db
            .ingest_data_binding(&DataBindingNode {
                name: binding_name.clone(),
                conditional_expression: node.conditional_expression.clone(),
                expr_json: node.expr_json.clone(),
                expression_language: "ifml".to_string(),
                domain: None,
            })
            .await
            .map_err(Error::Graph)?;

        db.ingest_edge(&id, &binding_id, EdgeType::HasDataBinding, None)
            .await
            .map_err(Error::Graph)?;

        let schema_title = if entity.ends_with("Type") {
            entity.clone()
        } else {
            format!("{}Type", entity)
        };
        db.ingest_edge(&binding_id, &schema_title, EdgeType::BindsToEntity, None)
            .await
            .map_err(Error::Graph)?;

        if let Some(ref fields) = node.fields {
            for field in fields {
                db.ingest_edge(
                    &id,
                    &format!("{}::{}", field, schema_title),
                    EdgeType::BindsToProperty,
                    Some(&EdgeProperties {
                        role: Some("field".to_string()),
                        ..Default::default()
                    }),
                )
                .await
                .map_err(Error::Graph)?;
            }
        }
    }

    // API operation binding: when the component declares `api: <operation>`,
    // link it to the matching ApiOperation node.
    if let Some(ref op_name) = node.api_operation {
        db.ingest_edge(&id, op_name, EdgeType::BindsToOperation, None)
            .await
            .map_err(Error::Graph)?;
    }

    // Ingest events
    for event in &comp.events {
        handle_event(db, event, &id).await?;
    }

    Ok(id)
}

async fn handle_event(db: &dyn GraphIngestor, event: &EventHandler, parent_id: &str) -> Result<()> {
    let event_name = format!(
        "{}_{}",
        parent_id.replace(':', "_"),
        event.event_type.as_str()
    );
    let event_type_str = match &event.event_type {
        EventType::Select => "select",
        EventType::Submit => "submit",
        EventType::Click => "click",
        EventType::Change => "change",
        EventType::Load => "load",
        EventType::Save => "save",
        EventType::Cancel => "cancel",
        EventType::Delete => "delete",
        EventType::Confirm => "confirm",
        EventType::Back => "back",
        EventType::Custom(s) => s,
    };

    let event_node = EventNode {
        name: event_name.clone(),
        event_type: event_type_str.to_string(),
        params: if event.params.is_empty() {
            None
        } else {
            Some(event.params.clone())
        },
        conditional_expression: event.condition.as_ref().map(render_expression),
        expr_json: expr_json(event.condition.as_ref()),
        requires: event.requires.clone(),
        domain: None,
    };

    let event_id = db.ingest_event(&event_node).await.map_err(Error::Graph)?;

    // Edge: parent has event
    db.ingest_edge(parent_id, &event_id, EdgeType::HasEvent, None)
        .await
        .map_err(Error::Graph)?;

    // Handle action
    match &event.action {
        EventAction::Navigate { target, binding } => {
            let binding_str = binding.as_ref().map(|b| {
                let pairs: Vec<String> = b
                    .pairs
                    .iter()
                    .map(|(k, v)| format!("\"{}\": \"{}\"", k, render_expression(v)))
                    .collect();
                format!("{{{}}}", pairs.join(", "))
            });

            db.ingest_edge(
                &event_id,
                &format!("vc:{}", target),
                EdgeType::NavigationFlow,
                Some(&EdgeProperties {
                    target_param_binding: binding_str,
                    ..Default::default()
                }),
            )
            .await
            .map_err(Error::Graph)?;
        }
        EventAction::Refresh { target, binding: _ } => {
            // Data flow to the target component
            db.ingest_edge(
                &event_id,
                &format!("comp:{}", target),
                EdgeType::DataFlow,
                None,
            )
            .await
            .map_err(Error::Graph)?;
        }
        EventAction::ActionInvocation { name, body } => {
            let action_id = format!("action:{}", name);
            db.ingest_action_node(&ActionNode {
                name: name.clone(),
                domain: None,
            })
            .await
            .map_err(Error::Graph)?;
            db.ingest_edge(&event_id, &action_id, EdgeType::TriggersAction, None)
                .await
                .map_err(Error::Graph)?;

            // Handle nested action body events (success/error outcomes)
            if let Some(body) = body {
                for body_event in &body.handlers {
                    let outcome_str = body_event.event_type.as_str().to_string();
                    let outcome_event_id = db
                        .ingest_event(&EventNode {
                            name: format!("{}_{}", action_id.replace(':', "_"), outcome_str),
                            event_type: outcome_str.clone(),
                            params: None,
                            conditional_expression: None,
                            expr_json: None,
                            requires: Vec::new(),
                            domain: None,
                        })
                        .await
                        .map_err(Error::Graph)?;

                    db.ingest_edge(
                        &action_id,
                        &outcome_event_id,
                        EdgeType::ActionEvent,
                        Some(&EdgeProperties {
                            outcome: Some(outcome_str),
                            ..Default::default()
                        }),
                    )
                    .await
                    .map_err(Error::Graph)?;

                    // Recurse: outcome events can also have navigate/refresh
                    if let EventAction::Navigate { target, binding } = &body_event.action {
                        let binding_str = binding.as_ref().map(|b| {
                            let pairs: Vec<String> = b
                                .pairs
                                .iter()
                                .map(|(k, v)| format!("\"{}\": \"{}\"", k, render_expression(v)))
                                .collect();
                            format!("{{{}}}", pairs.join(", "))
                        });
                        db.ingest_edge(
                            &outcome_event_id,
                            &format!("vc:{}", target),
                            EdgeType::NavigationFlow,
                            Some(&EdgeProperties {
                                target_param_binding: binding_str,
                                ..Default::default()
                            }),
                        )
                        .await
                        .map_err(Error::Graph)?;
                    }
                }
            }
        }
        EventAction::Stay => {
            // No edge needed for stay
        }
    }

    Ok(())
}

#[derive(Debug, Default)]
pub struct IfmlIngestStats {
    pub view_containers: usize,
    pub containers: usize,
    pub components: usize,
    pub events: usize,
    pub parameters: usize,
    pub actions: usize,
    pub module_uses: usize,
    pub actors: usize,
    pub imported_policies: usize,
}

impl std::fmt::Display for IfmlIngestStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} views, {} nested containers, {} components, {} events, {} params, {} actions, {} module uses, {} actors, {} imported policies",
            self.view_containers,
            self.containers,
            self.components,
            self.events,
            self.parameters,
            self.actions,
            self.module_uses,
            self.actors,
            self.imported_policies,
        )
    }
}
