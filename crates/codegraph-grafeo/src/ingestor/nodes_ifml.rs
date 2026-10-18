use std::collections::HashMap;

use codegraph_core::error::GraphError;
use codegraph_core::types::{
    ActionNode, DataBindingNode, EventNode, ModuleDefinitionNode, ParameterDefinitionNode,
    ViewComponentNode, ViewContainerNode,
};

use super::gql::CountingSession;
use super::gql::{escape_gql, opt_str, opt_to_grafeo_value};
use super::registry;
use crate::engine::GrafeoEngine;

impl GrafeoEngine {
    pub(super) async fn insert_view_container(
        &self,
        node: &ViewContainerNode,
    ) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
        let id = format!("vc:{}", node.name);
        let module_uses_json = node
            .module_uses
            .as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_default());
        let roles_json = node
            .roles
            .as_ref()
            .map(|r| serde_json::to_string(r).unwrap_or_default());
        let requires_json = node
            .requires
            .as_ref()
            .map(|r| serde_json::to_string(r).unwrap_or_default());
        let gql = format!(
            "MERGE (vc:ViewContainer {{name: '{}'}}) \
             SET vc.label = {}, vc.is_xor = {}, vc.is_default = {}, \
                 vc.is_landmark = {}, vc.is_modal = {}, vc.conditional_expression = {}, \
                 vc.domain = {}, vc.module_uses = {}, vc.roles = {}, vc.requires = {}",
            escape_gql(&node.name),
            opt_str(&node.label),
            node.is_xor,
            node.is_default,
            node.is_landmark,
            node.is_modal,
            opt_str(&node.conditional_expression),
            opt_str(&node.domain),
            opt_str(&module_uses_json),
            opt_str(&roles_json),
            opt_str(&requires_json),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_view_container failed: {e}")))?;
        Ok(id)
    }

    pub(super) async fn insert_module_definition(
        &self,
        node: &ModuleDefinitionNode,
    ) -> Result<String, GraphError> {
        let id = format!("module:{}", node.name);
        let json = |v: &Option<String>| v.clone();
        let module_uses_json = node
            .module_uses
            .as_ref()
            .map(|u| serde_json::to_string(u).unwrap_or_default());
        let props = HashMap::from([
            (
                "name".to_string(),
                grafeo::Value::String(node.name.clone().into()),
            ),
            ("domain".to_string(), opt_to_grafeo_value(&node.domain)),
            (
                "inputs_json".to_string(),
                opt_to_grafeo_value(&json(&node.inputs_json)),
            ),
            (
                "outputs_json".to_string(),
                opt_to_grafeo_value(&json(&node.outputs_json)),
            ),
            (
                "properties_json".to_string(),
                opt_to_grafeo_value(&json(&node.properties_json)),
            ),
            (
                "module_uses".to_string(),
                opt_to_grafeo_value(&module_uses_json),
            ),
        ]);
        self.create_registered_node(
            "ModuleDefinition",
            &[registry::keys::module(&node.name)],
            &props,
        )?;
        Ok(id)
    }

    pub(super) async fn insert_view_component(
        &self,
        node: &ViewComponentNode,
    ) -> Result<String, GraphError> {
        let id = format!("comp:{}", node.name);
        let fields_json = node
            .fields
            .as_ref()
            .map(|f| serde_json::to_string(f).unwrap_or_default());
        let props = HashMap::from([
            (
                "name".to_string(),
                grafeo::Value::String(node.name.clone().into()),
            ),
            (
                "component_type".to_string(),
                grafeo::Value::String(node.component_type.clone().into()),
            ),
            ("mode".to_string(), opt_to_grafeo_value(&node.mode)),
            ("entity".to_string(), opt_to_grafeo_value(&node.entity)),
            ("fields".to_string(), opt_to_grafeo_value(&fields_json)),
            ("filter".to_string(), opt_to_grafeo_value(&node.filter)),
            (
                "api_operation".to_string(),
                opt_to_grafeo_value(&node.api_operation),
            ),
            ("spec".to_string(), opt_to_grafeo_value(&node.spec)),
            (
                "conditional_expression".to_string(),
                opt_to_grafeo_value(&node.conditional_expression),
            ),
            ("domain".to_string(), opt_to_grafeo_value(&node.domain)),
        ]);
        self.create_registered_node(
            "ViewComponent",
            &[registry::keys::view_component(&node.name)],
            &props,
        )?;
        Ok(id)
    }

    pub(super) async fn insert_event(&self, node: &EventNode) -> Result<String, GraphError> {
        let id = format!("evt:{}", node.name);
        let params_json = node
            .params
            .as_ref()
            .map(|p| serde_json::to_string(p).unwrap_or_default());
        let requires_json = if node.requires.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&node.requires).unwrap_or_default())
        };
        let props = HashMap::from([
            (
                "name".to_string(),
                grafeo::Value::String(node.name.clone().into()),
            ),
            (
                "event_type".to_string(),
                grafeo::Value::String(node.event_type.clone().into()),
            ),
            ("params".to_string(), opt_to_grafeo_value(&params_json)),
            (
                "conditional_expression".to_string(),
                opt_to_grafeo_value(&node.conditional_expression),
            ),
            ("requires".to_string(), opt_to_grafeo_value(&requires_json)),
            ("domain".to_string(), opt_to_grafeo_value(&node.domain)),
        ]);
        self.create_registered_node("Event", &[registry::keys::event(&node.name)], &props)?;
        Ok(id)
    }

    pub(super) async fn insert_action_node(&self, node: &ActionNode) -> Result<String, GraphError> {
        let id = format!("action:{}", node.name);
        let props = HashMap::from([
            (
                "name".to_string(),
                grafeo::Value::String(node.name.clone().into()),
            ),
            ("domain".to_string(), opt_to_grafeo_value(&node.domain)),
        ]);
        self.create_registered_node("ActionNode", &[registry::keys::action(&node.name)], &props)?;
        Ok(id)
    }

    pub(super) async fn insert_parameter_definition(
        &self,
        node: &ParameterDefinitionNode,
    ) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
        let id = format!("param:{}", node.name);
        let exists = session
            .execute(&format!(
                "MATCH (p:ParameterDefinition {{name: '{}'}}) RETURN p.name LIMIT 1",
                escape_gql(&node.name)
            ))
            .map_err(|e| GraphError::Ingest(format!("ingest_parameter_definition failed: {e}")))?
            .rows()
            .is_empty();
        let gql = if !exists {
            format!(
                "MATCH (p:ParameterDefinition {{name: '{}'}}) \
                 SET p.direction = '{}', p.type_ref = '{}', p.domain = {}",
                escape_gql(&node.name),
                escape_gql(&node.direction),
                escape_gql(&node.type_ref),
                opt_str(&node.domain),
            )
        } else {
            format!(
                "INSERT (:ParameterDefinition {{ \
                    name: '{}', direction: '{}', type_ref: '{}', domain: {} \
                }})",
                escape_gql(&node.name),
                escape_gql(&node.direction),
                escape_gql(&node.type_ref),
                opt_str(&node.domain),
            )
        };
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_parameter_definition failed: {e}")))?;
        Ok(id)
    }

    pub(super) async fn insert_data_binding(
        &self,
        node: &DataBindingNode,
    ) -> Result<String, GraphError> {
        let id = format!("db:{}", node.name);
        let props = HashMap::from([
            (
                "name".to_string(),
                grafeo::Value::String(node.name.clone().into()),
            ),
            (
                "conditional_expression".to_string(),
                opt_to_grafeo_value(&node.conditional_expression),
            ),
            (
                "expression_language".to_string(),
                grafeo::Value::String(node.expression_language.clone().into()),
            ),
            ("domain".to_string(), opt_to_grafeo_value(&node.domain)),
        ]);
        self.create_registered_node(
            "DataBinding",
            &[registry::keys::data_binding(&node.name)],
            &props,
        )?;
        Ok(id)
    }
}
