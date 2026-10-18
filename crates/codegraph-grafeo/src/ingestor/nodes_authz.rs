use std::collections::HashMap;

use codegraph_core::error::GraphError;
use codegraph_core::types::{ActorPolicyModel, PolicyNode};

use super::gql::CountingSession;
use super::gql::{escape_gql, opt_str, opt_to_grafeo_value};
use super::registry;
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
            let props = HashMap::from([
                (
                    "name".to_string(),
                    grafeo::Value::String(actor.name.clone().into()),
                ),
                ("kind".to_string(), opt_to_grafeo_value(&actor.kind)),
                ("extends".to_string(), opt_to_grafeo_value(&actor.extends)),
                ("block".to_string(), opt_to_grafeo_value(&actor.block)),
            ]);
            self.create_registered_node("Actor", &[registry::keys::actor(&actor.name)], &props)?;
        }

        for capability in &model.capabilities {
            let props = HashMap::from([
                (
                    "name".to_string(),
                    grafeo::Value::String(capability.name.clone().into()),
                ),
                (
                    "class".to_string(),
                    grafeo::Value::String(capability.class.clone().into()),
                ),
                ("block".to_string(), opt_to_grafeo_value(&capability.block)),
            ]);
            self.create_registered_node(
                "Capability",
                &[registry::keys::capability(&capability.name)],
                &props,
            )?;
        }

        for grant in &model.grants {
            let obligations_json = serde_json::to_string(&grant.obligations)
                .map_err(|e| GraphError::Ingest(e.to_string()))?;
            if let (Some(actor_node), Some(capability_node)) = (
                self.lookup_node(&registry::keys::actor(&grant.actor)),
                self.lookup_node(&registry::keys::capability(&grant.capability)),
            ) {
                crate::perf::bump_registry_edge_hit();
                let props: Vec<(&str, grafeo::Value)> = vec![
                    ("effect", grafeo::Value::String(grant.effect.clone().into())),
                    ("when_expr", opt_to_grafeo_value(&grant.when)),
                    (
                        "obligations",
                        grafeo::Value::String(obligations_json.into()),
                    ),
                ];
                self.create_edge_direct(actor_node, capability_node, "Grant", &props)?;
            } else {
                crate::perf::bump_registry_edge_miss();
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
