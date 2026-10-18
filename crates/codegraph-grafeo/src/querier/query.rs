use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use codegraph_core::error::GraphError;

use crate::conversions::RowReader;
use crate::engine::GrafeoEngine;

/// Process-wide count of GQL executions (issue #389 instrumentation).
/// Every helper in this module funnels through `query_gql`/`query_gql_params`,
/// so this measures the true query volume of any graph read path.
static QUERY_COUNT: AtomicU64 = AtomicU64::new(0);

/// Total GQL executions since process start (or the last `reset_query_count`).
// Read/reset are test-only today (composition query-count pin, issue #389);
// the counter itself is live in production builds.
#[allow(dead_code)]
pub(crate) fn query_count() -> u64 {
    QUERY_COUNT.load(Ordering::Relaxed)
}

/// Zero the GQL execution counter (used by the composition query-count pin
/// test to measure a single `get_composition_tree` call in isolation).
#[allow(dead_code)]
pub(crate) fn reset_query_count() {
    QUERY_COUNT.store(0, Ordering::Relaxed);
}

/// Serialize the counter-sensitive lib tests (the #389 composition pin vs
/// any other in-crate read test): `QUERY_COUNT` is process-global, so a
/// concurrent read test executing inside the pin's reset→assert window
/// inflates the measurement. Both sides take this lock. Async-aware (the
/// tests hold it across await points).
#[cfg(test)]
pub(crate) static COUNTER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Query result wrapper holding columns and rows from Grafeo.
pub(super) struct QResult {
    pub(super) columns: Vec<String>,
    pub(super) rows: Vec<Vec<grafeo::Value>>,
}

pub(super) fn query_gql(engine: &GrafeoEngine, gql: &str) -> Result<QResult, GraphError> {
    QUERY_COUNT.fetch_add(1, Ordering::Relaxed);
    crate::perf::bump_read_gql();
    let session = engine.db().session();
    let result = session
        .execute(gql)
        .map_err(|e| GraphError::Query(format!("{e}")))?;
    let rows = result.rows().to_vec();
    Ok(QResult {
        columns: result.columns,
        rows,
    })
}

/// Execute a parameterized GQL query. Grafeo can cache query plans for
/// parameterized queries, avoiding repeated parsing of the same template.
pub(super) fn query_gql_params(
    engine: &GrafeoEngine,
    gql: &str,
    params: HashMap<String, grafeo::Value>,
) -> Result<QResult, GraphError> {
    QUERY_COUNT.fetch_add(1, Ordering::Relaxed);
    crate::perf::bump_read_gql();
    let result = engine
        .db()
        .execute_with_params(gql, params)
        .map_err(|e| GraphError::Query(format!("{e}")))?;
    let rows = result.rows().to_vec();
    Ok(QResult {
        columns: result.columns,
        rows,
    })
}

/// Run a GQL query and map every row through `map`.
pub(super) async fn query_many<T>(
    engine: &GrafeoEngine,
    gql: &str,
    map: impl Fn(&RowReader, &[grafeo::Value]) -> Result<T, GraphError>,
) -> Result<Vec<T>, GraphError> {
    let result = query_gql(engine, gql)?;
    let reader = RowReader::from_columns(&result.columns);
    result.rows.iter().map(|row| map(&reader, row)).collect()
}

/// Run a parameterized GQL query and map every row through `map`.
pub(super) async fn query_many_params<T>(
    engine: &GrafeoEngine,
    gql: &str,
    params: HashMap<String, grafeo::Value>,
    map: impl Fn(&RowReader, &[grafeo::Value]) -> Result<T, GraphError>,
) -> Result<Vec<T>, GraphError> {
    let result = query_gql_params(engine, gql, params)?;
    let reader = RowReader::from_columns(&result.columns);
    result.rows.iter().map(|row| map(&reader, row)).collect()
}

/// Run a GQL query and map the first row through `map`; an empty result set
/// yields `Ok(None)`.
pub(super) async fn query_one<T>(
    engine: &GrafeoEngine,
    gql: &str,
    map: impl Fn(&RowReader, &[grafeo::Value]) -> Result<T, GraphError>,
) -> Result<Option<T>, GraphError> {
    let result = query_gql(engine, gql)?;
    if result.rows.is_empty() {
        return Ok(None);
    }
    let reader = RowReader::from_columns(&result.columns);
    Ok(Some(map(&reader, &result.rows[0])?))
}

/// Run a parameterized GQL query and map the first row through `map`; an
/// empty result set yields `Ok(None)`.
pub(super) async fn query_one_params<T>(
    engine: &GrafeoEngine,
    gql: &str,
    params: HashMap<String, grafeo::Value>,
    map: impl Fn(&RowReader, &[grafeo::Value]) -> Result<T, GraphError>,
) -> Result<Option<T>, GraphError> {
    let result = query_gql_params(engine, gql, params)?;
    if result.rows.is_empty() {
        return Ok(None);
    }
    let reader = RowReader::from_columns(&result.columns);
    Ok(Some(map(&reader, &result.rows[0])?))
}
