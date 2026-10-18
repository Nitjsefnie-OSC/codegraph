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

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

static INGEST_NODE_INSERTS: AtomicU64 = AtomicU64::new(0);
static INGEST_EDGE_INSERTS: AtomicU64 = AtomicU64::new(0);
static INGEST_GQL_CALLS: AtomicU64 = AtomicU64::new(0);
static READ_GQL_CALLS: AtomicU64 = AtomicU64::new(0);
static REGISTRY_EDGE_HITS: AtomicU64 = AtomicU64::new(0);
static REGISTRY_EDGE_MISSES: AtomicU64 = AtomicU64::new(0);

/// Read calls per querier method (issue #350 batch-targeting attribution):
/// method name → call count. Bumped once per trait-method invocation at the
/// `GraphQuerier` delegation adapter — inner inherent methods may run several
/// GQL statements, the METHOD count is the attribution unit.
static METHOD_READ_COUNTS: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

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

/// Attribute one querier-method invocation (issue #350). ~11k calls over a
/// 30s pass ≈ 400/s — a short mutex critical section is negligible.
pub(crate) fn bump_method_read(method: &'static str) {
    let mut counts = METHOD_READ_COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    *counts.entry(method).or_insert(0) += 1;
}

/// Snapshot the per-method read attribution: method name → call count since
/// process start (or the last [`method_read_counts_reset`]).
pub fn method_read_counts() -> BTreeMap<String, u64> {
    let counts = METHOD_READ_COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    counts.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
}

/// Zero the per-method read attribution.
pub fn method_read_counts_reset() {
    let mut counts = METHOD_READ_COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    counts.clear();
}

#[cfg(test)]
mod method_read_tests {
    use super::*;

    #[test]
    fn bump_counts_then_reset_clears() {
        method_read_counts_reset();
        bump_method_read("test_get_properties");
        bump_method_read("test_get_properties");
        bump_method_read("test_get_enum_values");
        let counts = method_read_counts();
        assert_eq!(counts.get("test_get_properties"), Some(&2));
        assert_eq!(counts.get("test_get_enum_values"), Some(&1));
        method_read_counts_reset();
        assert!(!method_read_counts().contains_key("test_get_properties"));
    }
}
