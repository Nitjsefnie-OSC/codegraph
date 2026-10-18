use codegraph_core::error::GraphError;
use codegraph_core::types::{
    ActionNode, DataBindingNode, EventNode, ModuleDefinitionNode, ParameterDefinitionNode,
    ViewComponentNode, ViewContainerNode,
};

use super::gql::CountingSession;
use super::gql::{escape_gql, opt_str};
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
        let session = CountingSession::new(self.db().session());
        let id = format!("module:{}", node.name);
        let json = |v: &Option<String>| v.clone();
        let module_uses_json = node
            .module_uses
            .as_ref()
            .map(|u| serde_json::to_string(u).unwrap_or_default());
        let gql = format!(
            "INSERT (:ModuleDefinition {{ \
                name: '{}', domain: {}, inputs_json: {}, outputs_json: {}, \
                properties_json: {}, module_uses: {} \
            }})",
            escape_gql(&node.name),
            opt_str(&node.domain),
            opt_str(&json(&node.inputs_json)),
            opt_str(&json(&node.outputs_json)),
            opt_str(&json(&node.properties_json)),
            opt_str(&module_uses_json),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_module_definition failed: {e}")))?;
        Ok(id)
    }

    pub(super) async fn insert_view_component(
        &self,
        node: &ViewComponentNode,
    ) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
        let id = format!("comp:{}", node.name);
        let fields_json = node
            .fields
            .as_ref()
            .map(|f| serde_json::to_string(f).unwrap_or_default());
        let gql = format!(
            "INSERT (:ViewComponent {{ \
                name: '{}', component_type: '{}', mode: {}, \
                entity: {}, fields: {}, filter: {}, api_operation: {}, \
                spec: {}, conditional_expression: {}, domain: {} \
            }})",
            escape_gql(&node.name),
            escape_gql(&node.component_type),
            opt_str(&node.mode),
            opt_str(&node.entity),
            opt_str(&fields_json),
            opt_str(&node.filter),
            opt_str(&node.api_operation),
            opt_str(&node.spec),
            opt_str(&node.conditional_expression),
            opt_str(&node.domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_view_component failed: {e}")))?;
        Ok(id)
    }

    pub(super) async fn insert_event(&self, node: &EventNode) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
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
        let gql = format!(
            "INSERT (:Event {{ \
                name: '{}', event_type: '{}', params: {}, conditional_expression: {}, requires: {}, domain: {} \
            }})",
            escape_gql(&node.name),
            escape_gql(&node.event_type),
            opt_str(&params_json),
            opt_str(&node.conditional_expression),
            opt_str(&requires_json),
            opt_str(&node.domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_event failed: {e}")))?;
        Ok(id)
    }

    pub(super) async fn insert_action_node(&self, node: &ActionNode) -> Result<String, GraphError> {
        let session = CountingSession::new(self.db().session());
        let id = format!("action:{}", node.name);
        let gql = format!(
            "INSERT (:ActionNode {{ name: '{}', domain: {} }})",
            escape_gql(&node.name),
            opt_str(&node.domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_action_node failed: {e}")))?;
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
        let session = CountingSession::new(self.db().session());
        let id = format!("db:{}", node.name);
        let gql = format!(
            "INSERT (:DataBinding {{ \
                name: '{}', conditional_expression: {}, expression_language: '{}', domain: {} \
            }})",
            escape_gql(&node.name),
            opt_str(&node.conditional_expression),
            escape_gql(&node.expression_language),
            opt_str(&node.domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_data_binding failed: {e}")))?;
        Ok(id)
    }
}
