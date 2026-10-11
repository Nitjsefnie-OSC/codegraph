use codegraph_core::error::GraphError;
use codegraph_core::types::DeployModelGraph;

use super::gql::escape_gql;
use crate::engine::GrafeoEngine;

impl GrafeoEngine {
    pub(super) async fn insert_deploy_model(
        &self,
        model: &DeployModelGraph,
    ) -> Result<(), GraphError> {
        let session = self.db().session();

        // Upsert by `source_path` (the node's unique key): the model is ONE
        // node, so a delete-then-insert replaces a prior ingest of the same
        // file wholesale instead of duplicating it.
        let delete = format!(
            "MATCH (m:DeployModel {{source_path: '{}'}}) DELETE m",
            escape_gql(&model.source_path),
        );
        session
            .execute(&delete)
            .map_err(|e| GraphError::Ingest(format!("ingest_deploy_model delete failed: {e}")))?;

        let model_json = serde_json::to_string(model).map_err(|e| {
            GraphError::Ingest(format!("ingest_deploy_model serialize failed: {e}"))
        })?;
        let gql = format!(
            "INSERT (:DeployModel {{ source_path: '{}', model_json: '{}' }})",
            escape_gql(&model.source_path),
            escape_gql(&model_json),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_deploy_model insert failed: {e}")))?;

        Ok(())
    }
}
