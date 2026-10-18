use codegraph_core::error::GraphError;
use codegraph_core::types::DeployModelGraph;

use super::query::query_gql;
use crate::conversions::RowReader;
use crate::engine::GrafeoEngine;

impl GrafeoEngine {
    pub(super) async fn query_deploy_models(&self) -> Result<Vec<DeployModelGraph>, GraphError> {
        // ONE node per model, whole: `model_json` is the exact
        // `DeployModelGraph` that was ingested, so read-back is a
        // deserialize plus the source_path sort (no reassembly).
        let result = query_gql(
            self,
            "MATCH (m:DeployModel) \
             RETURN m.source_path AS source_path, m.model_json AS model_json",
        )?;
        let reader = RowReader::from_columns(&result.columns);
        let mut models: Vec<DeployModelGraph> = Vec::new();
        for row in &result.rows {
            let source_path = reader.get_string(row, "source_path")?;
            let model_json = reader.get_string(row, "model_json")?;
            let model: DeployModelGraph = serde_json::from_str(&model_json).map_err(|e| {
                GraphError::Query(format!(
                    "deploy model '{source_path}' has unreadable model_json: {e}"
                ))
            })?;
            models.push(model);
        }
        models.sort_by(|a, b| a.source_path.cmp(&b.source_path));
        Ok(models)
    }
}

#[cfg(test)]
mod round_trip_tests {
    //! rexlang #49: pins the storage contract — one `:DeployModel` node per
    //! file read back WHOLE (applications/profiles/deployments byte-equal to
    //! what was ingested), models sorted by `source_path` regardless of
    //! ingest order, and a re-ingest of the same source path REPLACING the
    //! prior model (upsert, not duplicate).

    use super::*;
    use codegraph_core::traits::{GraphIngestor, GraphQuerier};

    fn model(source_path: &str, app: &str) -> DeployModelGraph {
        serde_json::from_str(
            format!(
                r#"{{
                    "source_path": "{source_path}",
                    "applications": [{{
                        "source_path": "{source_path}",
                        "name": "{app}",
                        "components": [{{
                            "application": "{app}",
                            "name": "{app}_api",
                            "kind": "api",
                            "requires": ["edgeExecution"],
                            "settings": [
                                {{"path": "replicas", "value": {{"type": "int", "value": 2}}}}
                            ],
                            "designs": [],
                            "flows": [],
                            "ordinal": 0
                        }}],
                        "connections": []
                    }}],
                    "profiles": [{{
                        "source_path": "{source_path}",
                        "name": "edge",
                        "target": "cloudflareWorkers",
                        "defaults": [],
                        "policies": [],
                        "mappings": []
                    }}],
                    "deployments": [{{
                        "source_path": "{source_path}",
                        "name": "prod",
                        "application": "{app}",
                        "profile": "edge",
                        "configure": []
                    }}]
                }}"#
            )
            .as_str(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn models_come_back_whole_and_sorted_by_source_path() {
        let engine = GrafeoEngine::in_memory().unwrap();

        // Ingested zoo-first on purpose: read-back order is the
        // source_path sort, never ingest order.
        engine
            .ingest_deploy_model(&model("model/zoo.deploy", "Zoo"))
            .await
            .unwrap();
        engine
            .ingest_deploy_model(&model("model/alpha.deploy", "Alpha"))
            .await
            .unwrap();

        let loaded = engine.get_deploy_models().await.unwrap();
        assert_eq!(
            loaded
                .iter()
                .map(|m| m.source_path.as_str())
                .collect::<Vec<_>>(),
            vec!["model/alpha.deploy", "model/zoo.deploy"],
            "models sorted lexicographically by source_path"
        );
        assert_eq!(loaded[0], model("model/alpha.deploy", "Alpha"));
        assert_eq!(loaded[1], model("model/zoo.deploy", "Zoo"));
    }

    #[tokio::test]
    async fn reingesting_a_source_path_replaces_the_prior_model() {
        let engine = GrafeoEngine::in_memory().unwrap();
        engine
            .ingest_deploy_model(&model("model/svc.deploy", "Svc"))
            .await
            .unwrap();
        engine
            .ingest_deploy_model(&model("model/svc.deploy", "Renamed"))
            .await
            .unwrap();

        let loaded = engine.get_deploy_models().await.unwrap();
        assert_eq!(loaded.len(), 1, "upsert by source_path, no duplicate");
        assert_eq!(loaded[0].applications[0].name, "Renamed");
    }
}
