//! Issue #473: ingest + generation GQL counter pins.
//!
//! Lives as an integration test (own process) because the counters are
//! process-global: lib tests run alongside the #389 composition pin, which
//! ingests and reads concurrently and would break exact in-process deltas.
//! Within this binary the tests serialize on a mutex, so every delta is
//! exact.
//!
//! Issue #482 adds the natural-key registry contract pins: an edge between
//! two same-run nodes performs ZERO ingest GQL (direct
//! `create_edge_with_props`), while an edge to a node NOT ingested through
//! the registry falls back to the MATCH-based GQL statement (miss counter,
//! graph outcome unchanged).

use std::sync::{Mutex, MutexGuard};

use codegraph_core::traits::{GraphIngestor, GraphQuerier};
use codegraph_core::types::{EdgeProperties, EdgeType, NamespaceNode, PropertyNode, SchemaNode};
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

fn namespace(fqn: &str) -> NamespaceNode {
    NamespaceNode {
        fqn: fqn.to_string(),
        parent: None,
        source: Some("config".to_string()),
    }
}

fn property(name: &str) -> PropertyNode {
    PropertyNode {
        name: name.to_string(),
        prop_type: "string".to_string(),
        description: None,
        format: None,
        is_required: true,
        is_nullable: false,
        is_id: false,
        is_array: false,
        min_items: None,
        max_items: None,
        pattern: None,
        min_length: None,
        max_length: None,
        minimum: None,
        maximum: None,
        pg_column_name: name.to_string(),
        pg_column_type: "TEXT".to_string(),
        rust_field_name: name.to_string(),
        rust_field_type: "String".to_string(),
        sea_orm_type: "String".to_string(),
        render_strategy: "scalar".to_string(),
        ref_target: None,
        classification: None,
        projection: None,
        classification_kind: None,
        ui_override_detail: None,
        ui_override_list_cell: None,
        ui_override_form: None,
        ui_override_inline: None,
        type_expr: None,
    }
}

fn delta(after: PerfSnapshot, before: PerfSnapshot) -> PerfSnapshot {
    PerfSnapshot {
        ingest_node_inserts: after.ingest_node_inserts - before.ingest_node_inserts,
        ingest_edge_inserts: after.ingest_edge_inserts - before.ingest_edge_inserts,
        ingest_gql_calls: after.ingest_gql_calls - before.ingest_gql_calls,
        read_gql_calls: after.read_gql_calls - before.read_gql_calls,
        perf_registry_edge_hits: after.perf_registry_edge_hits - before.perf_registry_edge_hits,
        perf_registry_edge_misses: after.perf_registry_edge_misses
            - before.perf_registry_edge_misses,
    }
}

#[test]
fn node_ingest_is_direct_and_registered_edges_are_zero_gql() {
    let _guard = serial();
    block_on(async {
        perf_reset();
        let engine = GrafeoEngine::in_memory().unwrap();

        let before = perf_snapshot();
        engine.ingest_schema(&schema("PerfAType")).await.unwrap();
        engine.ingest_schema(&schema("PerfBType")).await.unwrap();
        engine
            .ingest_property("PerfBType", "common/PerfBType", &property("perf_prop"))
            .await
            .unwrap();
        let after_nodes = perf_snapshot();

        engine
            .ingest_edge(
                "perf_prop::PerfBType",
                "common/PerfBType",
                EdgeType::ReferencesSchema,
                None,
            )
            .await
            .unwrap();
        let after_edge = perf_snapshot();

        let node_delta = delta(after_nodes, before);
        assert_eq!(node_delta.ingest_node_inserts, 3);
        assert_eq!(node_delta.ingest_edge_inserts, 0);
        // Direct node creates (issue #482): zero GQL, NodeIds captured
        // (the property's HasProperty edge is direct too — schema
        // registered).
        assert_eq!(node_delta.ingest_gql_calls, 0);
        assert_eq!(node_delta.perf_registry_edge_hits, 1);
        assert_eq!(node_delta.read_gql_calls, 0);

        let edge_delta = delta(after_edge, after_nodes);
        assert_eq!(edge_delta.ingest_edge_inserts, 1);
        assert_eq!(edge_delta.ingest_node_inserts, 0);
        // Registry hit: the edge is written via the direct session API.
        assert_eq!(edge_delta.ingest_gql_calls, 0);
        assert_eq!(edge_delta.perf_registry_edge_hits, 1);
        assert_eq!(edge_delta.perf_registry_edge_misses, 0);
        assert_eq!(edge_delta.read_gql_calls, 0);
    });
}

#[test]
fn registry_hit_edge_performs_zero_ingest_gql_and_is_readable() {
    let _guard = serial();
    block_on(async {
        perf_reset();
        let engine = GrafeoEngine::in_memory().unwrap();
        engine.ingest_schema(&schema("PerfRType")).await.unwrap();
        engine.ingest_schema(&schema("PerfSType")).await.unwrap();

        // ExtendsSchema between two same-run nodes (with an edge property
        // that must survive the direct path): zero ingest GQL, then a
        // QUERIER read sees the edge (read-your-writes through the graph).
        let before = perf_snapshot();
        engine
            .ingest_edge(
                "PerfSType",
                "PerfRType",
                EdgeType::ExtendsSchema,
                Some(&EdgeProperties {
                    composition_type: Some("allOf".to_string()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        let after_edge = perf_snapshot();

        let d = delta(after_edge, before);
        assert_eq!(d.perf_registry_edge_hits, 1);
        assert_eq!(d.perf_registry_edge_misses, 0);
        assert_eq!(d.ingest_gql_calls, 0, "direct edge: zero ingest GQL");

        let parents = engine.get_allof_targets("PerfSType").await.unwrap();
        assert_eq!(parents, vec!["PerfRType".to_string()]);
    });
}

#[test]
fn unregistered_endpoint_falls_back_and_edge_still_links() {
    let _guard = serial();
    block_on(async {
        perf_reset();
        let engine = GrafeoEngine::in_memory().unwrap();
        engine.ingest_schema(&schema("PerfKType")).await.unwrap();

        // A Schema created OUTSIDE the ingestor (the pre-existing-graph
        // shape): not in the registry, so the edge must fall back to the
        // MATCH-based GQL statement — and still link correctly.
        let session = engine.db().session();
        session
            .execute(
                "INSERT (:Schema {title: 'PerfLegacyType', schema_id: 'legacy/PerfLegacyType', \
                 schema_type: 'object', classification: 'entity', pg_type: 'TABLE', \
                 rust_type: 'PerfLegacyType', sea_orm_type: 'Entity', rel_path: 'x', \
                 rust_type_name: 'PerfLegacyType', pg_table_name: 'perf_legacy_type', \
                 api_path_segment: 'perf-legacy-type', is_entity: true, is_codelist: false, \
                 is_primitive_wrapper: false, has_all_of: false, has_one_of: false, \
                 has_any_of: false, has_definitions: false, custom_annotations: '{}'})",
            )
            .unwrap();

        let before = perf_snapshot();
        engine
            .ingest_edge(
                "PerfKType",
                "PerfLegacyType",
                EdgeType::ExtendsSchema,
                Some(&EdgeProperties {
                    composition_type: Some("allOf".to_string()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        let after = perf_snapshot();

        let d = delta(after, before);
        assert_eq!(d.ingest_edge_inserts, 1);
        assert_eq!(d.perf_registry_edge_misses, 1, "fallback counts a miss");
        assert_eq!(d.perf_registry_edge_hits, 0);
        assert!(
            d.ingest_gql_calls > 0,
            "fallback edge goes through GQL ({d:?})"
        );

        // Graph outcome: the fallback edge (and its property) is readable
        // through the querier.
        let parents = engine.get_allof_targets("PerfKType").await.unwrap();
        assert_eq!(parents, vec!["PerfLegacyType".to_string()]);
    });
}

#[test]
fn innamespace_edges_hit_the_registry_and_stay_querier_visible() {
    let _guard = serial();
    block_on(async {
        perf_reset();
        let engine = GrafeoEngine::in_memory().unwrap();
        let mut ns_schema = schema("PerfNsType");
        ns_schema.schema_id = "cdm::PerfNsType".to_string();
        engine.ingest_schema(&ns_schema).await.unwrap();
        engine
            .ingest_namespace(&namespace("cdm.base"))
            .await
            .unwrap();

        let before = perf_snapshot();
        engine
            .ingest_edge("cdm::PerfNsType", "cdm.base", EdgeType::InNamespace, None)
            .await
            .unwrap();
        let after = perf_snapshot();

        let d = delta(after, before);
        assert_eq!(d.perf_registry_edge_hits, 1, "namespace-plane pair hit");
        assert_eq!(d.ingest_gql_calls, 0, "zero GQL on the direct path");

        // Read-your-writes: the querier resolves the InNamespace edge.
        let members = engine
            .list_schemas_by_namespace("cdm.base", false)
            .await
            .unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].title, "PerfNsType");
    });
}

#[test]
fn innamespace_fallback_uses_labeled_statements_per_plane() {
    let _guard = serial();
    block_on(async {
        perf_reset();
        let engine = GrafeoEngine::in_memory().unwrap();

        // Namespace-plane fallback: raw GQL Schema + Namespace (unregistered).
        let session = engine.db().session();
        session
            .execute(
                "INSERT (:Schema {title: 'PerfNs2Type', schema_id: 'cdm::PerfNs2Type', \
                 schema_type: 'object', classification: 'entity', pg_type: 'TABLE', \
                 rust_type: 'PerfNs2Type', sea_orm_type: 'Entity', rel_path: 'x', \
                 rust_type_name: 'PerfNs2Type', pg_table_name: 'perf_ns2_type', \
                 api_path_segment: 'perf-ns2-type', is_entity: true, is_codelist: false, \
                 is_primitive_wrapper: false, has_all_of: false, has_one_of: false, \
                 has_any_of: false, has_definitions: false, custom_annotations: '{}'})",
            )
            .unwrap();
        session
            .execute("INSERT (:Namespace {fqn: 'cdm.legacy', parent: null, source: null})")
            .unwrap();
        engine
            .ingest_edge(
                "cdm::PerfNs2Type",
                "cdm.legacy",
                EdgeType::InNamespace,
                None,
            )
            .await
            .unwrap();
        let members = engine
            .list_schemas_by_namespace("cdm.legacy", false)
            .await
            .unwrap();
        assert_eq!(members.len(), 1, "namespace-plane fallback links");
        assert_eq!(members[0].schema_id, "cdm::PerfNs2Type");

        // AT-Protocol-plane fallback: raw GQL Lexicon + AtprotoNamespace.
        session
            .execute(
                "INSERT (:Lexicon {nsid: 'com.example.perf', lex_type: 'object', \
                 key_strategy: 'tid', revision: null, description: null, domain: 'atproto'})",
            )
            .unwrap();
        session
            .execute(
                "INSERT (:AtprotoNamespace {authority: 'example.com', segment: 'perf', \
                 domain: 'atproto'})",
            )
            .unwrap();
        engine
            .ingest_edge(
                "com.example.perf",
                "example.com",
                EdgeType::InNamespace,
                None,
            )
            .await
            .unwrap();
        let rows = session
            .execute(
                "MATCH (:Lexicon {nsid: 'com.example.perf'})-[:InNamespace]->\
                 (:AtprotoNamespace {authority: 'example.com'}) \
                 RETURN 1 AS linked",
            )
            .unwrap();
        assert_eq!(
            rows.rows().len(),
            1,
            "atproto-plane fallback links (labeled statements, no OR scan)"
        );
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
        // A fallback edge (unregistered endpoint) so the ingest GQL counter
        // is exercised alongside the registry counters.
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
        assert!(
            live.perf_registry_edge_misses > 0,
            "precondition: fallback miss recorded ({live:?})"
        );

        perf_reset();
        assert_eq!(perf_snapshot(), PerfSnapshot::default());
    });
}
