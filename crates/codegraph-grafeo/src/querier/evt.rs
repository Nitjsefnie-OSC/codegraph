use std::collections::HashMap;

use codegraph_core::error::GraphError;
use codegraph_core::types::{EvtChannelNode, EvtEventNode, EvtModelGraph, EvtSubscriptionNode};

use super::query::query_gql;
use crate::conversions::RowReader;
use crate::engine::GrafeoEngine;

fn parse_json<T: serde::de::DeserializeOwned + Default>(raw: Option<String>) -> T {
    raw.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

impl GrafeoEngine {
    pub(super) async fn query_evt_models(&self) -> Result<Vec<EvtModelGraph>, GraphError> {
        // Events per source file.
        let result = query_gql(
            self,
            "MATCH (e:EvtEvent) \
             RETURN e.source_path AS source_path, e.name AS name, e.version AS version, \
             e.fields_json AS fields_json, e.ordinal AS ordinal",
        )?;
        let reader = RowReader::from_columns(&result.columns);
        let mut events: HashMap<String, Vec<EvtEventNode>> = HashMap::new();
        for row in &result.rows {
            let source_path = reader.get_string(row, "source_path")?;
            events
                .entry(source_path.clone())
                .or_default()
                .push(EvtEventNode {
                    source_path,
                    name: reader.get_string(row, "name")?,
                    version: reader.get_opt_string(row, "version")?,
                    fields: parse_json(reader.get_opt_string(row, "fields_json")?),
                    ordinal: reader.get_usize(row, "ordinal")?,
                });
        }

        // Channels per source file.
        let result = query_gql(
            self,
            "MATCH (c:EvtChannel) \
             RETURN c.source_path AS source_path, c.name AS name, \
             c.publishes_json AS publishes_json, c.ordinal AS ordinal",
        )?;
        let reader = RowReader::from_columns(&result.columns);
        let mut channels: HashMap<String, Vec<EvtChannelNode>> = HashMap::new();
        for row in &result.rows {
            let source_path = reader.get_string(row, "source_path")?;
            channels
                .entry(source_path.clone())
                .or_default()
                .push(EvtChannelNode {
                    source_path,
                    name: reader.get_string(row, "name")?,
                    publishes: parse_json(reader.get_opt_string(row, "publishes_json")?),
                    ordinal: reader.get_usize(row, "ordinal")?,
                });
        }

        // Subscriptions per source file.
        let result = query_gql(
            self,
            "MATCH (s:EvtSubscription) \
             RETURN s.source_path AS source_path, s.name AS name, \
             s.events_json AS events_json, s.consumer AS consumer, s.ordinal AS ordinal",
        )?;
        let reader = RowReader::from_columns(&result.columns);
        let mut subscriptions: HashMap<String, Vec<EvtSubscriptionNode>> = HashMap::new();
        for row in &result.rows {
            let source_path = reader.get_string(row, "source_path")?;
            subscriptions
                .entry(source_path.clone())
                .or_default()
                .push(EvtSubscriptionNode {
                    source_path,
                    name: reader.get_string(row, "name")?,
                    events: parse_json(reader.get_opt_string(row, "events_json")?),
                    consumer: reader.get_string(row, "consumer")?,
                    ordinal: reader.get_usize(row, "ordinal")?,
                });
        }

        // One model per source file: the union of the three families'
        // source paths, sorted lexicographically; within each model every
        // family restores declaration order by `ordinal`.
        let mut paths: Vec<String> = events
            .keys()
            .chain(channels.keys())
            .chain(subscriptions.keys())
            .cloned()
            .collect();
        paths.sort();
        paths.dedup();

        let models = paths
            .into_iter()
            .map(|source_path| {
                let mut events = events.remove(&source_path).unwrap_or_default();
                events.sort_by_key(|e| e.ordinal);
                let mut channels = channels.remove(&source_path).unwrap_or_default();
                channels.sort_by_key(|c| c.ordinal);
                let mut subscriptions = subscriptions.remove(&source_path).unwrap_or_default();
                subscriptions.sort_by_key(|s| s.ordinal);
                EvtModelGraph {
                    source_path,
                    events,
                    channels,
                    subscriptions,
                }
            })
            .collect();

        Ok(models)
    }
}

#[cfg(test)]
mod reassembly_tests {
    //! Issue #454: pins the reassembly contract — nodes grouped by
    //! `source_path` (sorted lexicographically) and restored to declaration
    //! order by `ordinal` even when ingested out of order (MATCH row order
    //! is storage order, so the sort is load-bearing).

    use super::*;
    use codegraph_core::traits::{GraphIngestor, GraphQuerier};

    fn model(source_path: &str, name: &str, ordinals: &[usize]) -> EvtModelGraph {
        EvtModelGraph {
            source_path: source_path.to_string(),
            events: ordinals
                .iter()
                .map(|&ordinal| EvtEventNode {
                    source_path: source_path.to_string(),
                    name: format!("{name}Event{ordinal}"),
                    version: None,
                    fields: Vec::new(),
                    ordinal,
                })
                .collect(),
            channels: ordinals
                .iter()
                .map(|&ordinal| EvtChannelNode {
                    source_path: source_path.to_string(),
                    name: format!("{name}Channel{ordinal}"),
                    publishes: Vec::new(),
                    ordinal,
                })
                .collect(),
            subscriptions: ordinals
                .iter()
                .map(|&ordinal| EvtSubscriptionNode {
                    source_path: source_path.to_string(),
                    name: format!("{name}Subscription{ordinal}"),
                    events: Vec::new(),
                    consumer: format!("{name}Consumer{ordinal}"),
                    ordinal,
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn nodes_ingested_out_of_ordinal_order_come_back_sorted() {
        // Reads bump the process-global QUERY_COUNT: hold the counter lock
        // so this test never lands inside the composition pin's window.
        let _counter = super::super::query::COUNTER_LOCK.lock().await;
        let engine = GrafeoEngine::in_memory().unwrap();

        // Authored deliberately out of declaration order (1 before 0).
        engine
            .ingest_evt_model(&model("model/zoo.evt", "Zoo", &[1, 0]))
            .await
            .unwrap();
        // A second contract, ingested first by name — models group by
        // source_path sorted lexicographically regardless of ingest order.
        engine
            .ingest_evt_model(&model("model/alpha.evt", "Alpha", &[1, 0]))
            .await
            .unwrap();

        let loaded = engine.get_evt_models().await.unwrap();
        assert_eq!(
            loaded
                .iter()
                .map(|m| m.source_path.as_str())
                .collect::<Vec<_>>(),
            vec!["model/alpha.evt", "model/zoo.evt"],
            "models grouped by source_path, sorted lexicographically"
        );
        for m in &loaded {
            let tag = m.source_path.split('/').next_back().unwrap();
            let tag = tag.trim_end_matches(".evt");
            for (family, ordinals) in [
                (
                    "events",
                    m.events.iter().map(|e| e.ordinal).collect::<Vec<_>>(),
                ),
                (
                    "channels",
                    m.channels.iter().map(|c| c.ordinal).collect::<Vec<_>>(),
                ),
                (
                    "subscriptions",
                    m.subscriptions
                        .iter()
                        .map(|s| s.ordinal)
                        .collect::<Vec<_>>(),
                ),
            ] {
                assert_eq!(
                    ordinals,
                    vec![0, 1],
                    "{tag} {family} must restore declaration order by ordinal"
                );
            }
        }
    }
}
