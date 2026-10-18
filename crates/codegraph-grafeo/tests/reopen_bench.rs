//! Reopen characterization bench (issue #485) — isolates the persisted-graph
//! cache warm-reopen cost. Ignored by default; run with:
//!
//! ```text
//! cargo test --release -p codegraph-grafeo --test reopen_bench -- --ignored --nocapture
//! ```
//!
//! Builds a synthetic graph shaped like a real codegraph graph (Schema /
//! Property / CodeList / Domain labels, ~20 properties per node, the
//! schema_ddl indexed properties among them, N/3 edges) at the two measured
//! ladder sizes (1,414 and 3,901 nodes), checkpoints it, and times each
//! reopen phase separately.

use codegraph_grafeo::GrafeoEngine;
use codegraph_grafeo::schema_ddl;
use std::fmt::Write as _;
use std::time::Instant;

const NODES_SMALL: usize = 1_414;
const NODES_LARGE: usize = 3_901;
const MATCH_ITERATIONS: usize = 200;

fn q(s: &str) -> String {
    format!("'{s}'")
}

fn label_mix(i: usize) -> &'static str {
    match i % 20 {
        0..=3 => "Schema",
        4..=14 => "Property",
        15..=17 => "CodeList",
        _ => "Domain",
    }
}

fn node_props(i: usize) -> Vec<(String, String)> {
    let title = format!("Type{i}");
    let name = format!("field{i}");
    let schema_id = format!("ns/Type{i}");
    let bools = [
        ("is_entity", i.is_multiple_of(2)),
        ("is_codelist", false),
        ("is_primitive_wrapper", false),
        ("has_all_of", i.is_multiple_of(7)),
        ("has_one_of", false),
        ("has_any_of", false),
        ("has_definitions", i.is_multiple_of(3)),
    ];
    match label_mix(i) {
        "Schema" => {
            let mut p: Vec<(String, String)> = vec![
                ("schema_id".into(), q(&schema_id)),
                ("title".into(), q(&title)),
                (
                    "description".into(),
                    q(&format!("synthetic schema {i} for the reopen bench")),
                ),
                ("schema_type".into(), q("object")),
                ("classification".into(), q("entity")),
                ("pg_type".into(), q("TABLE")),
                ("rust_type".into(), q(&title)),
                ("sea_orm_type".into(), q("Entity")),
                ("domain".into(), q("common")),
                ("namespace".into(), q("ns")),
                ("rel_path".into(), q(&format!("common/Type{i}.json"))),
                ("rust_type_name".into(), q(&title)),
                ("pg_table_name".into(), q(&format!("type_{i}"))),
                ("api_path_segment".into(), q(&format!("type-{i}"))),
                ("custom_annotations".into(), q("{}")),
            ];
            for (k, v) in bools {
                p.push((k.to_string(), v.to_string()));
            }
            p
        }
        "Property" => vec![
            ("name".into(), q(&name)),
            ("prop_type".into(), q("string")),
            ("description".into(), q(&format!("synthetic property {i}"))),
            ("format".into(), q("uuid")),
            ("is_required".into(), "true".into()),
            ("is_nullable".into(), "false".into()),
            ("is_array".into(), i.is_multiple_of(5).to_string()),
            ("pattern".into(), q("")),
            ("min_length".into(), q("1")),
            ("max_length".into(), q("64")),
            ("min_items".into(), q("0")),
            ("max_items".into(), q("10")),
            ("minimum".into(), q("0")),
            ("maximum".into(), q("100")),
            ("pg_column_name".into(), q(&format!("field_{i}_column"))),
            ("pg_column_type".into(), q("UUID")),
            ("rust_field_name".into(), q(&name)),
            ("rust_field_type".into(), q("Uuid")),
            ("sea_orm_type".into(), q("Uuid")),
            ("render_strategy".into(), q("scalar")),
            ("ref_target".into(), q(&title)),
            ("classification".into(), q("primitive")),
            ("_schema_title".into(), q(&format!("Type{}", i % 50))),
            ("_schema_id".into(), q(&format!("ns/Type{}", i % 50))),
        ],
        "CodeList" => vec![
            ("name".into(), q(&format!("clist{i}"))),
            ("description".into(), q(&format!("synthetic codelist {i}"))),
            ("pg_table_name".into(), q(&format!("clist_{i}"))),
            ("render_as".into(), q("enum")),
            ("check_expression".into(), q("")),
            ("_codelist_name".into(), q(&format!("clist{}", i % 50))),
        ],
        _ => vec![("name".into(), q(&format!("domain{}", i % 9)))],
    }
}

fn count_nodes(db: &grafeo::GrafeoDB) -> usize {
    let session = db.session();
    let res = session
        .execute("MATCH (n) RETURN count(n)")
        .expect("count nodes");
    res.scalar::<i64>().expect("count value") as usize
}

fn count_edges(db: &grafeo::GrafeoDB) -> usize {
    let session = db.session();
    let res = session
        .execute("MATCH ()-[e]->() RETURN count(e)")
        .expect("count edges");
    res.scalar::<i64>().expect("count value") as usize
}

fn insert_nodes(session: &grafeo::Session, n: usize) {
    for i in 0..n {
        let label = label_mix(i);
        let props = node_props(i);
        let gql = format!(
            "INSERT (:{label} {{{set}}})",
            set = props
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        if let Err(e) = session.execute(&gql) {
            panic!("node insert {i} failed: {e}\n{gql}");
        }
    }
}

fn insert_edges(session: &grafeo::Session, n_nodes: usize) {
    let schema = |e: usize| (e / 4) * 20 + (e % 4);
    let property = |e: usize| (e / 11) * 20 + 4 + (e % 11);
    let n_edges = n_nodes / 3;
    for e in 0..n_edges {
        let src = format!("ns/Type{}", schema(e) % n_nodes);
        let dst = format!("field{}", property(e) % n_nodes);
        let gql = format!(
            "MATCH (a:Schema {{schema_id: {src_q}}}), (b:Property {{name: {dst_q}}}) INSERT (a)-[:HasProperty {{sort_order: {e}}}]->(b)",
            src_q = q(&src),
            dst_q = q(&dst),
        );
        if let Err(e) = session.execute(&gql) {
            panic!("edge insert {e} failed: {e}\n{gql}");
        }
    }
}

fn run_match_workload(db: &grafeo::GrafeoDB) -> (f64, usize) {
    let session = db.session();
    let start = Instant::now();
    let mut hits = 0usize;
    for i in 0..MATCH_ITERATIONS {
        let t = format!("Type{}", i % 50);
        let gql = format!("MATCH (n:Schema {{title: {}}}) RETURN n.schema_id", q(&t));
        let res = session.execute(&gql).expect("match by title");
        hits += res.row_count();

        let f = format!("field{}", i * 3);
        let gql = format!(
            "MATCH (n:Property {{name: {}}}) RETURN n.pg_column_name",
            q(&f)
        );
        let res = session.execute(&gql).expect("match by name");
        hits += res.row_count();

        let gql = format!(
            "MATCH (n:Property {{_schema_title: {}}}) RETURN n.name",
            q(&t)
        );
        let res = session.execute(&gql).expect("match by _schema_title");
        hits += res.row_count();
    }
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;
    (elapsed, hits)
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}

struct PhaseReport {
    size: usize,
    node_count: usize,
    edge_count: usize,
    graph_file_bytes: u64,
    ingest_ms: f64,
    checkpoint_ms: f64,
    reopen_ro_ms: f64,
    reopen_ro_no_index_ms: f64,
    file_read_ms: f64,
    restore_indexes_ms: f64,
    match_baseline_ms: f64,
    match_reopened_ms: f64,
    match_reindexed_ms: f64,
    match_reopened_hits: usize,
    reopen_write_mode_ms: f64,
}

fn run_size(n_nodes: usize) -> PhaseReport {
    let dir = tempfile::tempdir().expect("tempdir");
    let graph_path = dir.path().join("graph.grafeo");

    // Build + checkpoint (models the cold run's write side).
    let ingest_start = Instant::now();
    let engine = GrafeoEngine::persistent(&graph_path).expect("persistent engine");
    let session = engine.db().session();
    insert_nodes(&session, n_nodes);
    insert_edges(&session, n_nodes);
    let ingest_ms = ingest_start.elapsed().as_secs_f64() * 1_000.0;

    let node_count = count_nodes(engine.db());
    let edge_count = count_edges(engine.db());

    // Baseline query workload with indexes live (in-memory equivalent).
    let (match_baseline_ms, _) = run_match_workload(engine.db());

    let cp_start = Instant::now();
    engine.checkpoint().expect("checkpoint");
    let checkpoint_ms = ms(cp_start.elapsed());
    drop(engine);

    let graph_file_bytes = std::fs::metadata(&graph_path).map(|m| m.len()).unwrap_or(0);

    // Raw file read = I/O floor for any reopen.
    let fr_start = Instant::now();
    let bytes = std::fs::read(&graph_path).expect("read graph file");
    let file_read_ms = ms(fr_start.elapsed());
    assert!(!bytes.is_empty());

    // Full warm reopen (production path), best of 3.
    let mut reopen_ro_ms = f64::MAX;
    for _ in 0..3 {
        let start = Instant::now();
        let reopened = GrafeoEngine::open_read_only(&graph_path).expect("read-only open");
        reopen_ro_ms = reopen_ro_ms.min(ms(start.elapsed()));
        assert_eq!(count_nodes(reopened.db()), node_count);
        drop(reopened);
    }

    // No-index graph (raw DB, same storage config, no create_property_index)
    // to split any index-rebuild share out of the reopen.
    let no_index_path = dir.path().join("no_index.grafeo");
    {
        let config = grafeo::Config::persistent(&no_index_path)
            .with_storage_format(grafeo_engine::config::StorageFormat::SingleFile)
            .without_query_timeout();
        let raw = grafeo::GrafeoDB::with_config(config).expect("raw persistent db");
        let s = raw.session();
        insert_nodes(&s, n_nodes);
        insert_edges(&s, n_nodes);
        raw.wal_checkpoint().expect("raw checkpoint");
        drop(raw);
    }
    let mut reopen_ro_no_index_ms = f64::MAX;
    for _ in 0..3 {
        let start = Instant::now();
        let config = grafeo::Config::read_only(&no_index_path);
        let raw = grafeo::GrafeoDB::with_config(config).expect("raw read-only open");
        reopen_ro_no_index_ms = reopen_ro_no_index_ms.min(ms(start.elapsed()));
        assert_eq!(count_nodes(&raw), node_count);
        drop(raw);
    }

    // Reopened read-only DB: the fix (issue #485) restores codegraph's
    // property-index set at open; measure the query penalty without it by
    // dropping the indexes explicitly.
    let reopened = GrafeoEngine::open_read_only(&graph_path).expect("read-only open");
    assert!(
        schema_ddl::indexed_properties()
            .iter()
            .all(|p| reopened.db().has_property_index(p)),
        "read-only reopen must restore the codegraph property-index set (issue #485)"
    );
    for prop in schema_ddl::indexed_properties() {
        reopened.db().drop_property_index(prop);
    }
    let (match_reopened_ms, match_hits) = run_match_workload(reopened.db());

    let ri_start = Instant::now();
    for prop in schema_ddl::indexed_properties() {
        reopened.db().create_property_index(prop);
    }
    let restore_indexes_ms = ms(ri_start.elapsed());
    assert!(reopened.db().has_property_index("title"));
    let (match_reindexed_ms, _) = run_match_workload(reopened.db());
    drop(reopened);

    // Write-mode reopen (the pre-#275 style warm path): replays any sidecar
    // WAL after loading sections.
    let mut reopen_write_mode_ms = f64::MAX;
    for _ in 0..3 {
        let start = Instant::now();
        let engine = GrafeoEngine::persistent(&graph_path).expect("persistent reopen");
        reopen_write_mode_ms = reopen_write_mode_ms.min(ms(start.elapsed()));
        assert_eq!(count_nodes(engine.db()), node_count);
        drop(engine);
    }

    PhaseReport {
        size: n_nodes,
        node_count,
        edge_count,
        graph_file_bytes,
        ingest_ms,
        checkpoint_ms,
        reopen_ro_ms,
        reopen_ro_no_index_ms,
        file_read_ms,
        restore_indexes_ms,
        match_baseline_ms,
        match_reopened_ms,
        match_reindexed_ms,
        match_reopened_hits: match_hits,
        reopen_write_mode_ms,
    }
}

fn print_report(r: &PhaseReport) {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\n=== reopen bench: {size} nodes (built {nodes} nodes / {edges} edges, file {kib} KiB) ===",
        size = r.size,
        nodes = r.node_count,
        edges = r.edge_count,
        kib = r.graph_file_bytes / 1024
    );
    let _ = writeln!(
        out,
        "  ingest (GQL nodes+edges)      {:>10.1} ms",
        r.ingest_ms
    );
    let _ = writeln!(
        out,
        "  checkpoint                    {:>10.1} ms",
        r.checkpoint_ms
    );
    let _ = writeln!(
        out,
        "  reopen open_read_only (min3)  {:>10.1} ms",
        r.reopen_ro_ms
    );
    let _ = writeln!(
        out,
        "  reopen no-index raw (min3)    {:>10.1} ms",
        r.reopen_ro_no_index_ms
    );
    let _ = writeln!(
        out,
        "  reopen write-mode (min3)      {:>10.1} ms",
        r.reopen_write_mode_ms
    );
    let _ = writeln!(
        out,
        "  raw file read (I/O floor)     {:>10.1} ms",
        r.file_read_ms
    );
    let _ = writeln!(
        out,
        "  restore 21 property indexes   {:>10.1} ms",
        r.restore_indexes_ms
    );
    let _ = writeln!(
        out,
        "  match workload x{}: indexed-mem {:>8.1} ms | reopened {:>8.1} ms ({} rows) | reindexed {:>8.1} ms",
        MATCH_ITERATIONS * 3,
        r.match_baseline_ms,
        r.match_reopened_ms,
        r.match_reopened_hits,
        r.match_reindexed_ms
    );
    println!("{out}");
}

#[test]
#[ignore = "characterization bench (issue #485); run with --release --ignored --nocapture"]
fn reopen_bench_ladder() {
    let small = run_size(NODES_SMALL);
    print_report(&small);
    let large = run_size(NODES_LARGE);
    print_report(&large);

    let node_ratio = large.node_count as f64 / small.node_count as f64;
    println!("\n=== scaling (large/small) ===");
    println!("  nodes                        x{node_ratio:.2}");
    println!(
        "  checkpoint                   x{:.2}",
        large.checkpoint_ms / small.checkpoint_ms
    );
    println!(
        "  reopen open_read_only        x{:.2}",
        large.reopen_ro_ms / small.reopen_ro_ms
    );
    println!(
        "  reopen no-index raw          x{:.2}",
        large.reopen_ro_no_index_ms / small.reopen_ro_no_index_ms
    );
    println!(
        "  match workload reopened      x{:.2}",
        large.match_reopened_ms / small.match_reopened_ms
    );
    println!(
        "  match workload reindexed     x{:.2}",
        large.match_reindexed_ms / small.match_reindexed_ms
    );
}

#[test]
#[ignore = "characterization bench (issue #485); run with --release --ignored --nocapture"]
fn reopen_bench_small_only() {
    let r = run_size(NODES_SMALL);
    print_report(&r);
}

#[test]
fn read_only_reopen_restores_codegraph_property_indexes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let graph_path = dir.path().join("graph.grafeo");

    {
        let engine = GrafeoEngine::persistent(&graph_path).expect("persistent engine");
        let session = engine.db().session();
        insert_nodes(&session, 40);
        engine.checkpoint().expect("checkpoint");
    }

    let reopened = GrafeoEngine::open_read_only(&graph_path).expect("read-only open");
    for prop in schema_ddl::indexed_properties() {
        assert!(
            reopened.db().has_property_index(prop),
            "property index '{prop}' must be restored on read-only reopen (issue #485)"
        );
    }
    assert_eq!(count_nodes(reopened.db()), 40);
}
