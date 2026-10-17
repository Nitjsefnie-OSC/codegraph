//! Issue #473: ingest + generation GQL counter pins.
//!
//! Lives as an integration test (own process) because the counters are
//! process-global: lib tests run alongside the #389 composition pin, which
//! ingests and reads concurrently and would break exact in-process deltas.
//! Within this binary the tests serialize on a mutex, so every delta is
//! exact.

use std::sync::{Mutex, MutexGuard};

use codegraph_core::traits::{GraphIngestor, GraphQuerier};
use codegraph_core::types::{EdgeType, SchemaNode};
use codegraph_grafeo::GrafeoEngine;
use codegraph_grafeo::perf::{PerfSnapshot, perf_reset, perf_snapshot};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}

fn schema(title: &str) -> SchemaNode {
    SchemaNode {
        namespace: None,
        schema_id: format!("common/{title}"),
        title: title.to_string(),
        description: None,
        schema_type: "object".to_string(),
        classification: "entity".to_string(),
        domain: Some("common".to_string()),
        rel_path: format!("common/{title}.json"),
        pg_type: "TABLE".to_string(),
        rust_type: title.to_string(),
        sea_orm_type: "Entity".to_string(),
        rust_type_name: title.to_string(),
        pg_table_name: title.to_string(),
        api_path_segment: title.to_string(),
        parent_schema: None,
        is_entity: true,
        is_codelist: false,
        is_primitive_wrapper: false,
        has_all_of: false,
        has_one_of: false,
        has_any_of: false,
        has_definitions: false,
        custom_annotations: Default::default(),
        access: None,
        annotations: None,
    }
}

fn delta(after: PerfSnapshot, before: PerfSnapshot) -> PerfSnapshot {
    PerfSnapshot {
        ingest_node_inserts: after.ingest_node_inserts - before.ingest_node_inserts,
        ingest_edge_inserts: after.ingest_edge_inserts - before.ingest_edge_inserts,
        ingest_gql_calls: after.ingest_gql_calls - before.ingest_gql_calls,
        read_gql_calls: after.read_gql_calls - before.read_gql_calls,
    }
}

#[test]
fn edge_and_node_ingest_bump_their_own_counters() {
    let _guard = serial();
    block_on(async {
        perf_reset();
        let engine = GrafeoEngine::in_memory().unwrap();

        let before = perf_snapshot();
        engine.ingest_schema(&schema("PerfAType")).await.unwrap();
        engine.ingest_schema(&schema("PerfBType")).await.unwrap();
        let after_nodes = perf_snapshot();

        engine
            .ingest_edge(
                "perf_prop::common/PerfBType",
                "common/PerfBType",
                EdgeType::ReferencesSchema,
                None,
            )
            .await
            .unwrap();
        let after_edge = perf_snapshot();

        let node_delta = delta(after_nodes, before);
        assert_eq!(node_delta.ingest_node_inserts, 2);
        assert_eq!(node_delta.ingest_edge_inserts, 0);
        // insert_schema is one parameterized INSERT each.
        assert_eq!(node_delta.ingest_gql_calls, 2);
        assert_eq!(node_delta.read_gql_calls, 0);

        let edge_delta = delta(after_edge, after_nodes);
        assert_eq!(edge_delta.ingest_edge_inserts, 1);
        assert_eq!(edge_delta.ingest_node_inserts, 0);
        // The hot-path ReferencesSchema edge is one parameterized MATCH+INSERT.
        assert_eq!(edge_delta.ingest_gql_calls, 1);
        assert_eq!(edge_delta.read_gql_calls, 0);
    });
}

#[test]
fn querier_reads_bump_the_read_counter_only() {
    let _guard = serial();
    block_on(async {
        perf_reset();
        let engine = GrafeoEngine::in_memory().unwrap();
        engine.ingest_schema(&schema("PerfRType")).await.unwrap();

        let before = perf_snapshot();
        let found = engine.get_schema("PerfRType").await.unwrap().is_some();
        let after = perf_snapshot();

        assert!(found);
        let d = delta(after, before);
        assert_eq!(d.read_gql_calls, 1, "one get_schema = one read GQL call");
        assert_eq!(d.ingest_gql_calls, 0);
        assert_eq!(d.ingest_node_inserts, 0);
        assert_eq!(d.ingest_edge_inserts, 0);
    });
}

#[test]
fn perf_reset_zeroes_every_counter() {
    let _guard = serial();
    block_on(async {
        let engine = GrafeoEngine::in_memory().unwrap();
        engine.ingest_schema(&schema("PerfZType")).await.unwrap();
        engine
            .ingest_edge(
                "perf_prop::common/PerfZType",
                "common/PerfZType",
                EdgeType::ReferencesSchema,
                None,
            )
            .await
            .unwrap();
        let _ = engine.get_schema("PerfZType").await.unwrap();
        let live = perf_snapshot();
        assert!(
            live.ingest_gql_calls > 0 && live.read_gql_calls > 0,
            "precondition: counters live ({live:?})"
        );

        perf_reset();
        assert_eq!(perf_snapshot(), PerfSnapshot::default());
    });
}
