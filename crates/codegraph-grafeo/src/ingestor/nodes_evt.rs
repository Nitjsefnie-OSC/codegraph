use codegraph_core::error::GraphError;
use codegraph_core::types::EvtModelGraph;

use super::gql::CountingSession;
use super::gql::{escape_gql, opt_str};
use crate::engine::GrafeoEngine;

fn json_string<T: serde::Serialize>(value: &T) -> Result<String, GraphError> {
    serde_json::to_string(value).map_err(|e| GraphError::Ingest(e.to_string()))
}

impl GrafeoEngine {
    pub(super) async fn insert_evt_model(&self, model: &EvtModelGraph) -> Result<(), GraphError> {
        let session = CountingSession::new(self.db().session());

        for event in &model.events {
            let fields_json = json_string(&event.fields)?;
            let gql = format!(
                "INSERT (:EvtEvent {{ source_path: '{}', name: '{}', version: {}, \
                 fields_json: '{}', ordinal: {} }})",
                escape_gql(&event.source_path),
                escape_gql(&event.name),
                opt_str(&event.version),
                escape_gql(&fields_json),
                event.ordinal,
            );
            session
                .execute(&gql)
                .map_err(|e| GraphError::Ingest(format!("ingest_evt_model event failed: {e}")))?;
        }

        for channel in &model.channels {
            let publishes_json = json_string(&channel.publishes)?;
            let gql = format!(
                "INSERT (:EvtChannel {{ source_path: '{}', name: '{}', \
                 publishes_json: '{}', ordinal: {} }})",
                escape_gql(&channel.source_path),
                escape_gql(&channel.name),
                escape_gql(&publishes_json),
                channel.ordinal,
            );
            session
                .execute(&gql)
                .map_err(|e| GraphError::Ingest(format!("ingest_evt_model channel failed: {e}")))?;
        }

        for subscription in &model.subscriptions {
            let events_json = json_string(&subscription.events)?;
            let gql = format!(
                "INSERT (:EvtSubscription {{ source_path: '{}', name: '{}', \
                 events_json: '{}', consumer: '{}', ordinal: {} }})",
                escape_gql(&subscription.source_path),
                escape_gql(&subscription.name),
                escape_gql(&events_json),
                escape_gql(&subscription.consumer),
                subscription.ordinal,
            );
            session.execute(&gql).map_err(|e| {
                GraphError::Ingest(format!("ingest_evt_model subscription failed: {e}"))
            })?;
        }

        Ok(())
    }
}
