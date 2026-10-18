//! Always-on GQL/ingest performance counters (issue #473).
//!
//! Counters are process-wide `AtomicU64`s bumped at the GQL execution
//! choke points: one increment (~1ns) per query, negligible against a GQL
//! round trip, so they stay live in release/samply builds without a cargo
//! feature. Printing is env-gated (`CODEGRAPH_PERF_COUNTERS=1`) by the
//! caller; this module only exposes the snapshot/reset API.
//!
//! Ingest/read split: the ingestor executes every statement through
//! [`crate::ingestor::gql::CountingSession`] (bumps `ingest_gql_calls` at
//! the session boundary), while the querier funnels through
//! `querier::query::query_gql`/`query_gql_params` (bump `read_gql_calls`).

use std::sync::atomic::{AtomicU64, Ordering};

static INGEST_NODE_INSERTS: AtomicU64 = AtomicU64::new(0);
static INGEST_EDGE_INSERTS: AtomicU64 = AtomicU64::new(0);
static INGEST_GQL_CALLS: AtomicU64 = AtomicU64::new(0);
static READ_GQL_CALLS: AtomicU64 = AtomicU64::new(0);
static REGISTRY_EDGE_HITS: AtomicU64 = AtomicU64::new(0);
static REGISTRY_EDGE_MISSES: AtomicU64 = AtomicU64::new(0);

/// Point-in-time copy of every performance counter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PerfSnapshot {
    /// `GraphIngestor` node-ingest calls (schema, property, codelist, …).
    pub ingest_node_inserts: u64,
    /// `GraphIngestor` edge-ingest calls (`ingest_edge` +
    /// `ingest_regulatory_reference`).
    pub ingest_edge_inserts: u64,
    /// GQL statements executed on the ingest path (via `CountingSession`),
    /// including `finalize` counts.
    pub ingest_gql_calls: u64,
    /// GQL statements executed on the read path (querier choke point).
    pub read_gql_calls: u64,
    /// Edges created through the natural-key registry direct path (issue
    /// #482): both endpoints were ingested this run, so the edge was written
    /// via `Session::create_edge_with_props` with zero GQL.
    pub perf_registry_edge_hits: u64,
    /// Edges that fell back to the MATCH-based GQL statement because at
    /// least one endpoint was not registered (pre-existing graph, unknown
    /// natural-key form).
    pub perf_registry_edge_misses: u64,
}

/// Snapshot every counter (loads are `Relaxed`; counters are advisory).
pub fn perf_snapshot() -> PerfSnapshot {
    PerfSnapshot {
        ingest_node_inserts: INGEST_NODE_INSERTS.load(Ordering::Relaxed),
        ingest_edge_inserts: INGEST_EDGE_INSERTS.load(Ordering::Relaxed),
        ingest_gql_calls: INGEST_GQL_CALLS.load(Ordering::Relaxed),
        read_gql_calls: READ_GQL_CALLS.load(Ordering::Relaxed),
        perf_registry_edge_hits: REGISTRY_EDGE_HITS.load(Ordering::Relaxed),
        perf_registry_edge_misses: REGISTRY_EDGE_MISSES.load(Ordering::Relaxed),
    }
}

/// Zero every counter.
pub fn perf_reset() {
    INGEST_NODE_INSERTS.store(0, Ordering::Relaxed);
    INGEST_EDGE_INSERTS.store(0, Ordering::Relaxed);
    INGEST_GQL_CALLS.store(0, Ordering::Relaxed);
    READ_GQL_CALLS.store(0, Ordering::Relaxed);
    REGISTRY_EDGE_HITS.store(0, Ordering::Relaxed);
    REGISTRY_EDGE_MISSES.store(0, Ordering::Relaxed);
}

pub(crate) fn bump_ingest_node() {
    INGEST_NODE_INSERTS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn bump_ingest_edge() {
    INGEST_EDGE_INSERTS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn bump_ingest_gql() {
    INGEST_GQL_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn bump_read_gql() {
    READ_GQL_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn bump_registry_edge_hit() {
    REGISTRY_EDGE_HITS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn bump_registry_edge_miss() {
    REGISTRY_EDGE_MISSES.fetch_add(1, Ordering::Relaxed);
}
