use codegraph_core::error::GraphError;
use codegraph_core::types::{ActorPolicyModel, PolicyNode};

use super::gql::CountingSession;
use super::gql::{escape_gql, opt_str};
use crate::engine::GrafeoEngine;

impl GrafeoEngine {
    pub(super) async fn insert_policy(&self, policy: &PolicyNode) -> Result<(), GraphError> {
        let session = CountingSession::new(self.db().session());
        let kind_json =
            serde_json::to_string(&policy.kind).map_err(|e| GraphError::Ingest(e.to_string()))?;
        let domain = policy.domain.clone().unwrap_or_default();
        let gql = format!(
            "INSERT (:Policy {{ \
                name: '{}', kind_json: '{}', target_schema: '{}', domain: '{}' \
            }})",
            escape_gql(&policy.name),
            escape_gql(&kind_json),
            escape_gql(&policy.target_schema),
            escape_gql(&domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_policy failed: {e}")))?;
        Ok(())
    }

    // ── Authorization metamodel ──────────────────────────────────────

    pub(super) async fn insert_actor_policy(
        &self,
        model: &ActorPolicyModel,
    ) -> Result<(), GraphError> {
        let session = CountingSession::new(self.db().session());

        for actor in &model.actors {
            let gql = format!(
                "INSERT (:Actor {{ name: '{}', kind: {}, extends: {}, block: {} }})",
                escape_gql(&actor.name),
                opt_str(&actor.kind),
                opt_str(&actor.extends),
                opt_str(&actor.block),
            );
            session.execute(&gql).map_err(|e| {
                GraphError::Ingest(format!("ingest_actor_policy actor failed: {e}"))
            })?;
        }

        for capability in &model.capabilities {
            let gql = format!(
                "INSERT (:Capability {{ name: '{}', class: '{}', block: {} }})",
                escape_gql(&capability.name),
                escape_gql(&capability.class),
                opt_str(&capability.block),
            );
            session.execute(&gql).map_err(|e| {
                GraphError::Ingest(format!("ingest_actor_policy capability failed: {e}"))
            })?;
        }

        for grant in &model.grants {
            let obligations_json = serde_json::to_string(&grant.obligations)
                .map_err(|e| GraphError::Ingest(e.to_string()))?;
            let gql = format!(
                "MATCH (a:Actor {{name: '{actor}'}}), (c:Capability {{name: '{capability}'}}) \
                 INSERT (a)-[:Grant {{ effect: '{effect}', when_expr: {when}, obligations: '{obligations}' }}]->(c)",
                actor = escape_gql(&grant.actor),
                capability = escape_gql(&grant.capability),
                effect = escape_gql(&grant.effect),
                when = opt_str(&grant.when),
                obligations = escape_gql(&obligations_json),
            );
            session.execute(&gql).map_err(|e| {
                GraphError::Ingest(format!("ingest_actor_policy grant failed: {e}"))
            })?;
        }

        let blocks_json = serde_json::to_string(&model.policy.blocks)
            .map_err(|e| GraphError::Ingest(e.to_string()))?;
        let never_both_json = serde_json::to_string(&model.policy.never_both)
            .map_err(|e| GraphError::Ingest(e.to_string()))?;
        let purposes_json = serde_json::to_string(&model.policy.purposes)
            .map_err(|e| GraphError::Ingest(e.to_string()))?;
        let delegations_json = serde_json::to_string(&model.policy.delegations)
            .map_err(|e| GraphError::Ingest(e.to_string()))?;
        session
            .execute("MERGE (:ActorPolicy {name: 'actor_policy'})")
            .map_err(|e| GraphError::Ingest(format!("ingest_actor_policy merge failed: {e}")))?;
        let set_gql = format!(
            "MATCH (p:ActorPolicy {{name: 'actor_policy'}}) SET p.blocks = '{}'",
            escape_gql(&blocks_json),
        );
        session
            .execute(&set_gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_actor_policy blocks failed: {e}")))?;
        let set_gql = format!(
            "MATCH (p:ActorPolicy {{name: 'actor_policy'}}) SET p.never_both = '{}'",
            escape_gql(&never_both_json),
        );
        session.execute(&set_gql).map_err(|e| {
            GraphError::Ingest(format!("ingest_actor_policy never_both failed: {e}"))
        })?;
        let set_gql = format!(
            "MATCH (p:ActorPolicy {{name: 'actor_policy'}}) SET p.purposes = '{}'",
            escape_gql(&purposes_json),
        );
        session
            .execute(&set_gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_actor_policy purposes failed: {e}")))?;
        let set_gql = format!(
            "MATCH (p:ActorPolicy {{name: 'actor_policy'}}) SET p.delegations = '{}'",
            escape_gql(&delegations_json),
        );
        session.execute(&set_gql).map_err(|e| {
            GraphError::Ingest(format!("ingest_actor_policy delegations failed: {e}"))
        })?;

        Ok(())
    }
}
