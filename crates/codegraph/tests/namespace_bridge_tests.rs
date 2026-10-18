//! Issue #268 — namespace source bridging + namespace-aware generation.
//!
//! Per-source bridging (mock + grafeo parity where the harness allows):
//! - `.mox` `package <dotted.name>` → NamespaceNode + NamespaceParent
//!   chains + SchemaNode.namespace + InNamespace edges (package-less mox
//!   models stay namespace-less — byte-identity pin);
//! - `.rosetta` `namespace a.b` + `import a.b.* as x` → nodes + wildcard/
//!   alias NamespaceImports edges + SchemaNode.namespace;
//! - JSON `$id`/`$namespace` inference (documented derivation; schemas
//!   without either stay namespace-less — the back-compat marker);
//! - classification over a domain whose schemas span namespaces;
//! - generation order consuming `namespace_generation_order` when
//!   namespaces exist (imported-before-importer across sources);
//! - the `namespace_layout` gate: gated paths ON vs flat OFF, and API URL
//!   segments invariant under namespaces.

use std::fs;
use std::path::{Path, PathBuf};

use codegraph_config::config::{DomainConfig, parse_domain_config_str};
use codegraph_core::mock::MockEngine;
use codegraph_core::traits::{GraphIngestor, GraphQuerier};
use codegraph_core::types::{NamespaceImport, NamespaceNode};
use codegraph_grafeo::GrafeoEngine;

// ── mox bridge ──────────────────────────────────────────────────────────

const MOX_NAMESPACED: &str = r#"
package cdm.base.datetime

enum OrderStatus {
    Draft as "Draft" = 0
    Open as "Open" = 1
}

class PartyType {
    String [1] name
    refers PartyType [0..1] parent
}
"#;

/// Same classes, NO package declaration — the byte-identity pin: nothing
/// namespace-shaped may land in the graph.
const MOX_PACKAGELESS: &str = r#"
enum OrderStatus {
    Draft as "Draft" = 0
    Open as "Open" = 1
}

class PartyType {
    String [1] name
}
"#;

const BRIDGE_DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.datetime]
label = "Datetime"
schema_dir = "datetime"
postgres_schema = "datetime"
"#;

async fn ingest_mox(
    db: &(impl GraphIngestor + GraphQuerier),
    source: &str,
) -> codegraph::ingest::mox_ingest::MoxIngestStats {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.mox");
    fs::write(&path, source).unwrap();
    let config: DomainConfig = parse_domain_config_str(BRIDGE_DOMAINS_TOML).unwrap();
    let stats = codegraph::ingest::mox_ingest::ingest_mox_files(
        db,
        db,
        &[path],
        &config,
        &config.defaults.type_suffix,
    )
    .await
    .unwrap()
    .stats;
    dir.close().unwrap();
    stats
}

async fn assert_mox_namespaced_graph(db: &(impl GraphIngestor + GraphQuerier), stats_ns: usize) {
    let namespaces = db.list_namespaces().await.unwrap();
    let fqns: Vec<&str> = namespaces.iter().map(|n| n.fqn.as_str()).collect();
    assert_eq!(
        fqns,
        vec!["cdm", "cdm.base", "cdm.base.datetime"],
        "dotted parent chain, sorted by fqn"
    );
    let leaf = namespaces.last().unwrap();
    assert_eq!(leaf.parent.as_deref(), Some("cdm.base"));
    assert_eq!(leaf.source.as_deref(), Some("mox"));
    // Every bridged schema joins the namespace.
    let members = db
        .list_schemas_by_namespace("cdm.base.datetime", false)
        .await
        .unwrap();
    assert!(
        members.iter().any(|s| s.title == "PartyType"),
        "PartyType must join the namespace: {members:?}"
    );
    assert!(
        members.iter().any(|s| s.title == "OrderStatus"),
        "the bridged enum codelist schema joins too: {members:?}"
    );
    assert_eq!(stats_ns, 3, "namespace nodes incl. dotted parents");
}

#[tokio::test]
async fn mox_package_bridges_namespace_on_mock() {
    let engine = MockEngine::new();
    let stats = ingest_mox(&engine, MOX_NAMESPACED).await;
    assert_mox_namespaced_graph(&engine, stats.namespaces).await;
}

#[tokio::test]
async fn mox_package_bridges_namespace_on_grafeo() {
    let engine = GrafeoEngine::in_memory().unwrap();
    let stats = ingest_mox(&engine, MOX_NAMESPACED).await;
    assert_mox_namespaced_graph(&engine, stats.namespaces).await;
}

#[tokio::test]
async fn mox_without_package_stays_namespaceless_on_mock() {
    let engine = MockEngine::new();
    assert_mox_packageless(&engine).await;
}

#[tokio::test]
async fn mox_without_package_stays_namespaceless_on_grafeo() {
    let engine = GrafeoEngine::in_memory().unwrap();
    assert_mox_packageless(&engine).await;
}

/// The rex grammar REQUIRES a `package` declaration: a package-less file
/// fails to compile, so a namespace-less mox model cannot exist — every
/// compilable mox model carries package (= namespace) provenance. The
/// byte-identity pin therefore lives on the JSON surface (schemas without
/// `$id`/`$namespace` stay namespace-less, see the tests below) and on the
/// equivalence gate (mox WITH a package vs namespace-less JSON must stay
/// byte-identical in generated output, since the flat gate ignores
/// namespaces). Here we pin the upstream contract: the package-less file
/// bridges nothing and creates no namespace nodes.
async fn assert_mox_packageless(db: &(impl GraphIngestor + GraphQuerier)) {
    let stats = ingest_mox(db, MOX_PACKAGELESS).await;
    assert_eq!(stats.namespaces, 0, "no namespace nodes");
    assert_eq!(
        stats.classes, 0,
        "compile failed upstream — nothing bridges"
    );
    assert!(stats.skipped > 0, "the file counts as skipped");
    let schemas = db.list_schemas(None).await.unwrap();
    assert!(
        schemas.iter().all(|s| s.namespace.is_none()),
        "no schema may carry a namespace: {schemas:?}"
    );
    assert!(
        db.list_namespaces().await.unwrap().is_empty(),
        "namespace-less model must not create namespace nodes"
    );
}

// ── rosetta bridge ──────────────────────────────────────────────────────

const ROS_INNER: &str = r#"
namespace biz.core
version "1.0.0"

enum Side:
	Buy
	Sell

type RefType:
	refId string (1..1)
"#;

const ROS_OUTER: &str = r#"
namespace biz.trade
version "1.0.0"

import biz.core.*
import biz.core as core

enum TradeStatus:
	New
	Done

type TradeType:
	side Side (1..1)
	core RefType (0..1) [metadata id]
"#;

#[tokio::test]
async fn rosetta_namespace_and_imports_bridge() {
    let engine = GrafeoEngine::in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let inner = dir.path().join("core.rosetta");
    let outer = dir.path().join("trade.rosetta");
    fs::write(&inner, ROS_INNER).unwrap();
    fs::write(&outer, ROS_OUTER).unwrap();
    let toml = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.trade]
label = "Trade"
schema_dir = "trade"
postgres_schema = "trade"
"#;
    let config_path = dir.path().join("domains.toml");
    fs::write(&config_path, toml).unwrap();
    let config = codegraph_config::config::parse_domain_config(&config_path).unwrap();
    let outcome = codegraph::ingest::rosetta_ingest::ingest_rosetta_files(
        &engine,
        &engine,
        &[inner, outer],
        &config,
        "Type",
    )
    .await
    .unwrap();

    // Nodes: biz, biz.core, biz.trade (declaring files) — the import
    // target is already declared, so it adds nothing new.
    let namespaces = engine.list_namespaces().await.unwrap();
    let fqns: Vec<&str> = namespaces.iter().map(|n| n.fqn.as_str()).collect();
    assert_eq!(fqns, vec!["biz", "biz.core", "biz.trade"], "{namespaces:?}");
    let trade = namespaces.iter().find(|n| n.fqn == "biz.trade").unwrap();
    assert_eq!(trade.source.as_deref(), Some("rosetta"));
    assert_eq!(outcome.stats.namespaces, 3);

    // Imports: wildcard + aliased edge to the SAME target — both kept,
    // payloads round-trip.
    let imports = engine.get_namespace_imports("biz.trade").await.unwrap();
    assert_eq!(imports.len(), 2, "{imports:?}");
    assert!(imports.iter().all(|i| i.to_ns == "biz.core"));
    assert!(imports.iter().any(|i| i.wildcard && i.alias.is_none()));
    assert!(
        imports
            .iter()
            .any(|i| !i.wildcard && i.alias.as_deref() == Some("core"))
    );
    assert_eq!(outcome.stats.namespace_imports, 2);

    // Bridged schemas carry the namespace and join it.
    let trade_schema = engine.get_schema("TradeType").await.unwrap().unwrap();
    assert_eq!(trade_schema.namespace.as_deref(), Some("biz.trade"));
    let core_schema = engine.get_schema("RefType").await.unwrap().unwrap();
    assert_eq!(core_schema.namespace.as_deref(), Some("biz.core"));
    let members = engine
        .list_schemas_by_namespace("biz.core", false)
        .await
        .unwrap();
    assert!(members.iter().any(|s| s.title == "RefType"), "{members:?}");
}

// ── JSON $id/$namespace inference ───────────────────────────────────────

const JSON_NAMESPACED_ID: &str = r#"{
  "$id": "https://cdm.example/cdm/base/datetime/PartyType.json",
  "title": "PartyType",
  "type": "object",
  "properties": { "name": { "type": "string" } },
  "required": ["name"]
}"#;

const JSON_NAMESPACED_KEY: &str = r#"{
  "$namespace": "cdm.money",
  "title": "MoneyType",
  "type": "object",
  "properties": { "amount": { "type": "number" } },
  "required": ["amount"]
}"#;

const JSON_PLAIN: &str = r#"{
  "title": "PlainType",
  "type": "object",
  "properties": { "label": { "type": "string" } },
  "required": ["label"]
}"#;

/// Writes `<root>/cdm/json/*.json` and returns `<root>` — the schema dir
/// is the parent so the dir-derived domain (segment before `/json/`) is
/// `cdm`, matching the domains.toml used by the classification test.
fn write_json_schemas(root: &Path) -> PathBuf {
    let dir = root.join("cdm/json");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("PartyType.json"), JSON_NAMESPACED_ID).unwrap();
    fs::write(dir.join("MoneyType.json"), JSON_NAMESPACED_KEY).unwrap();
    fs::write(dir.join("PlainType.json"), JSON_PLAIN).unwrap();
    root.to_path_buf()
}

async fn ingest_json_fixture(db: &(impl GraphIngestor + GraphQuerier), schemas_dir: &Path) {
    let classifier = codegraph_classifier::config::parse_classifier_config_str("").unwrap();
    codegraph::ingest::async_ingest::ingest_schemas(
        db,
        schemas_dir,
        &classifier,
        &Default::default(),
        &Default::default(),
        "Type",
    )
    .await
    .unwrap();
}

async fn assert_json_namespace_graph(db: &(impl GraphIngestor + GraphQuerier)) {
    let namespaces = db.list_namespaces().await.unwrap();
    let fqns: Vec<&str> = namespaces.iter().map(|n| n.fqn.as_str()).collect();
    assert_eq!(
        fqns,
        vec!["cdm", "cdm.base", "cdm.base.datetime", "cdm.money"],
        "$id-derived chain + explicit $namespace: {namespaces:?}"
    );
    // Derivation provenance.
    let leaf = namespaces
        .iter()
        .find(|n| n.fqn == "cdm.base.datetime")
        .unwrap();
    assert_eq!(leaf.source.as_deref(), Some("json"));
    let money = namespaces.iter().find(|n| n.fqn == "cdm.money").unwrap();
    assert_eq!(money.parent.as_deref(), Some("cdm"));

    let party = db.get_schema("PartyType").await.unwrap().unwrap();
    assert_eq!(
        party.namespace.as_deref(),
        Some("cdm.base.datetime"),
        "$id path minus filename minus host"
    );
    let money_schema = db.get_schema("MoneyType").await.unwrap().unwrap();
    assert_eq!(money_schema.namespace.as_deref(), Some("cdm.money"));
    let plain = db.get_schema("PlainType").await.unwrap().unwrap();
    assert_eq!(
        plain.namespace, None,
        "no $id/$namespace = namespace-less (back-compat)"
    );
    let members = db
        .list_schemas_by_namespace("cdm.money", false)
        .await
        .unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].title, "MoneyType");
}

#[tokio::test]
async fn json_namespace_from_id_and_namespace_key_on_mock() {
    let engine = MockEngine::new();
    let dir = tempfile::tempdir().unwrap();
    let schemas = write_json_schemas(dir.path());
    ingest_json_fixture(&engine, &schemas).await;
    assert_json_namespace_graph(&engine).await;
}

#[tokio::test]
async fn json_namespace_from_id_and_namespace_key_on_grafeo() {
    let engine = GrafeoEngine::in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let schemas = write_json_schemas(dir.path());
    ingest_json_fixture(&engine, &schemas).await;
    assert_json_namespace_graph(&engine).await;
}

// ── mox/JSON namespace parity for equivalent models ─────────────────────

/// The same single-class model authored twice: `.mox` package `todo` vs a
/// JSON `$id` whose path ends in `/todo/`. Both must land the schema in
/// namespace `todo` with the same membership shape.
#[tokio::test]
async fn namespace_parity_between_equivalent_mox_and_json_models() {
    let mox = r#"
package todo

class TaskType {
    String [1] title
}
"#;
    let json = r#"{
  "$id": "https://app.example/todo/TaskType.json",
  "title": "TaskType",
  "type": "object",
  "properties": { "title": { "type": "string" } },
  "required": ["title"]
}"#;

    // mox side.
    let mox_shape = {
        let engine = GrafeoEngine::in_memory().unwrap();
        let _ = ingest_mox(&engine, mox).await;
        assert_eq!(
            engine
                .get_schema("TaskType")
                .await
                .unwrap()
                .unwrap()
                .namespace,
            Some("todo".to_string())
        );
        let fqns: Vec<String> = engine
            .list_namespaces()
            .await
            .unwrap()
            .iter()
            .map(|n| n.fqn.clone())
            .collect();
        let titles: Vec<String> = engine
            .list_schemas_by_namespace("todo", false)
            .await
            .unwrap()
            .iter()
            .map(|s| s.title.clone())
            .collect();
        (fqns, titles)
    };

    // JSON side.
    let json_shape = {
        let engine = GrafeoEngine::in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let json_dir = dir.path().join("todo/json");
        fs::create_dir_all(&json_dir).unwrap();
        fs::write(json_dir.join("TaskType.json"), json).unwrap();
        ingest_json_fixture(&engine, &dir.path().join("todo")).await;
        assert_eq!(
            engine
                .get_schema("TaskType")
                .await
                .unwrap()
                .unwrap()
                .namespace,
            Some("todo".to_string())
        );
        let fqns: Vec<String> = engine
            .list_namespaces()
            .await
            .unwrap()
            .iter()
            .map(|n| n.fqn.clone())
            .collect();
        let titles: Vec<String> = engine
            .list_schemas_by_namespace("todo", false)
            .await
            .unwrap()
            .iter()
            .map(|s| s.title.clone())
            .collect();
        (fqns, titles)
    };

    assert_eq!(
        mox_shape, json_shape,
        "equivalent models must produce equivalent namespace shapes"
    );
}

// ── generation order consumes namespace_generation_order ────────────────

#[tokio::test]
async fn generation_order_respects_namespace_imports() {
    let engine = MockEngine::new();
    for (fqn, parent) in [
        ("alpha", None),
        ("alpha.leaf", Some("alpha")),
        ("billing", None),
    ] {
        engine
            .ingest_namespace(&NamespaceNode {
                fqn: fqn.to_string(),
                parent: parent.map(|p| p.to_string()),
                source: None,
            })
            .await
            .unwrap();
    }
    // billing imports alpha.leaf ⇒ alpha.leaf's entities generate first.
    engine
        .ingest_namespace_import(&NamespaceImport {
            from_ns: "billing".into(),
            to_ns: "alpha.leaf".into(),
            wildcard: true,
            alias: None,
        })
        .await
        .unwrap();

    let config = parse_domain_config_str(
        r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.billing]
label = "Billing"
schema_dir = "billing"
postgres_schema = "billing"
"#,
    )
    .unwrap();

    async fn namespaced_titles(db: &dyn GraphQuerier, config: &DomainConfig) -> Vec<String> {
        codegraph_generate::compute_generation_order(db, config)
            .await
            .unwrap_or_else(|e| panic!("generation order failed: {e}"))
            .iter()
            .map(|e| e.schema_title.clone())
            .collect::<Vec<_>>()
    }

    // Namespaced graph: imported-before-importer BEATS the title sort.
    for (title, ns) in [("ZebraType", "billing"), ("AppleType", "alpha.leaf")] {
        engine
            .ingest_schema(&bridge_schema(title, Some(ns)))
            .await
            .unwrap();
        engine
            .ingest_edge(
                title,
                ns,
                codegraph_core::types::EdgeType::InNamespace,
                None,
            )
            .await
            .unwrap();
    }
    let titles = namespaced_titles(&engine, &config).await;
    assert_eq!(
        titles,
        vec!["AppleType", "ZebraType"],
        "AppleType (alpha.leaf, imported) must precede ZebraType (billing, the importer)"
    );

    // Namespace-less graph: pure title order (back-compat pin).
    let plain = MockEngine::new();
    plain
        .ingest_schema(&bridge_schema("ZebraType", None))
        .await
        .unwrap();
    plain
        .ingest_schema(&bridge_schema("AppleType", None))
        .await
        .unwrap();
    let titles = namespaced_titles(&plain, &config).await;
    assert_eq!(titles, vec!["AppleType", "ZebraType"]);
}

/// A minimal entity schema for the generation-order tests.
fn bridge_schema(title: &str, ns: Option<&str>) -> codegraph_core::types::SchemaNode {
    codegraph_core::types::SchemaNode {
        schema_id: format!("{}/{title}", ns.unwrap_or("plain")),
        title: title.to_string(),
        description: None,
        schema_type: "object".to_string(),
        classification: "entity_reference".to_string(),
        domain: Some("billing".to_string()),
        namespace: ns.map(|n| n.to_string()),
        rel_path: format!("{}/{}.json", ns.unwrap_or("plain"), title),
        pg_type: "UUID".to_string(),
        rust_type: "Uuid".to_string(),
        sea_orm_type: "Uuid".to_string(),
        rust_type_name: title.to_string(),
        pg_table_name: title.to_lowercase(),
        api_path_segment: title.to_lowercase(),
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

// ── classification spans namespaces within a domain ─────────────────────

#[tokio::test]
async fn namespaced_domain_classifies_across_namespaces() {
    let engine = GrafeoEngine::in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let schemas = write_json_schemas(dir.path());
    ingest_json_fixture(&engine, &schemas).await;

    let config: DomainConfig = parse_domain_config_str(
        r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.cdm]
label = "CDM"
schema_dir = "cdm"
postgres_schema = "cdm"
"#,
    )
    .unwrap();
    let all_data = engine.get_classification_data().await.unwrap();
    let domain_schemas: Vec<_> = all_data
        .iter()
        .filter(|d| d.domain.as_deref() == Some("cdm"))
        .cloned()
        .collect();
    assert_eq!(
        domain_schemas.len(),
        3,
        "all three schemas belong to the domain regardless of namespace"
    );
    // The namespace rides the classification data (issue #267 contract).
    let party = domain_schemas
        .iter()
        .find(|d| d.title == "PartyType")
        .unwrap();
    assert_eq!(party.namespace.as_deref(), Some("cdm.base.datetime"));

    let result = codegraph::classify::AutoClassifier::new(Default::default(), Default::default())
        .classify_domain("cdm", &config.domains["cdm"], &domain_schemas);
    let classified: Vec<String> = result
        .entities
        .iter()
        .map(|s| s.title.clone())
        .chain(result.value_objects.iter().map(|s| s.title.clone()))
        .collect();
    assert!(
        classified.contains(&"PartyType".to_string())
            && classified.contains(&"MoneyType".to_string()),
        "namespaced schemas classify alongside namespace-less ones: {classified:?}"
    );
}

// ── namespace_layout gate: path emission ON vs flat OFF ─────────────────

const GATED_PROFILES_TOML: &str = r#"
[profiles.ns.meta]
name = "ns"
version = "1.0.0"
description = "namespace-layout gate fixture"

[profiles.ns.features]
namespace_layout = true

[profiles.ns.api]
generators = ["ddl", "sea_orm_entity", "dto", "repository", "handler"]
output = "generated/"
"#;

const UNGATED_PROFILES_TOML: &str = r#"
[profiles.ns.meta]
name = "ns"
version = "1.0.0"
description = "flat (gate-off) fixture"

[profiles.ns.features]

[profiles.ns.api]
generators = ["ddl", "sea_orm_entity", "dto", "repository", "handler"]
output = "generated/"
"#;

/// Write the shared gated-test fixture: a namespaced mox model + config.
fn write_gate_fixture(root: &Path) -> (PathBuf, Vec<PathBuf>) {
    fs::write(root.join("domains.toml"), BRIDGE_DOMAINS_TOML).unwrap();
    let model = root.join("model.mox");
    fs::write(&model, MOX_NAMESPACED).unwrap();
    (root.join("generated"), vec![model])
}

fn collect_rel_files(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

async fn run_gate(root: &Path, output: &Path, mox: &[PathBuf], gated: bool) {
    let profiles = root.join("profiles.toml");
    fs::write(
        &profiles,
        if gated {
            GATED_PROFILES_TOML
        } else {
            UNGATED_PROFILES_TOML
        },
    )
    .unwrap();
    let mut args = codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &root.join("domains.toml"),
        output,
        extension_points_path: None,
        profile_name: "ns",
        variant: None,
        profiles_config_path: Some(profiles),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: mox,
        rosetta_files: &[],
        ddd_files: &[],
        evt_files: &[],
        deploy_files: &[],
        ifml_framework: &[],
        ifml_components: None,
        ifml_design_system: None,
        codegraph_rev: None,
        check: false,
        ux_rules: None,
    };
    args.profile_name = "ns";
    codegraph::driver::run(args).await.unwrap();
}

/// Gated ON: entity + DTO + repository outputs land under
/// `cdm/base/datetime/` (the namespace-derived module path).
#[tokio::test]
async fn namespace_layout_gate_emits_namespace_paths() {
    let root = tempfile::tempdir().unwrap();
    let (output, mox) = write_gate_fixture(root.path());
    run_gate(root.path(), &output, &mox, true).await;

    let files = collect_rel_files(&output);
    assert!(
        files
            .iter()
            .any(|f| f == "src/entity/cdm/base/datetime/datetime_party.rs"),
        "entity module under the namespace path; files: {files:?}"
    );
    assert!(
        files
            .iter()
            .any(|f| f.starts_with("src/domain/cdm/base/datetime/party/")),
        "dto + repository under the namespace path; files: {files:?}"
    );
    assert!(
        !files.iter().any(|f| f == "src/entity/datetime_party.rs"),
        "the entity must NOT also emit flat under the gate"
    );
}

/// Gate OFF (default): flat domain layout — the byte-identity contract.
#[tokio::test]
async fn namespace_layout_off_keeps_flat_paths() {
    let root = tempfile::tempdir().unwrap();
    let (output, mox) = write_gate_fixture(root.path());
    run_gate(root.path(), &output, &mox, false).await;

    let files = collect_rel_files(&output);
    assert!(
        files.iter().any(|f| f == "src/entity/datetime_party.rs"),
        "flat entity module; files: {files:?}"
    );
    assert!(
        files
            .iter()
            .any(|f| f.starts_with("src/domain/datetime/party/")),
        "flat dto + repository; files: {files:?}"
    );
    assert!(
        !files.iter().any(|f| f.contains("cdm/base")),
        "no namespace-derived paths when the gate is off: {files:?}"
    );
}

// ── API URL segments invariant under namespaces ─────────────────────────

/// SvelteKit/API path segments stay title/api_path_segment-based (issue
/// #268 explicit non-goal): the handler routes declare the SAME URL paths
/// whether or not the namespace gate is on.
#[tokio::test]
async fn api_url_segments_invariant_under_namespace_layout() {
    fn route_paths(dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                route_paths(&path, out);
            } else if let Ok(content) = fs::read_to_string(&path) {
                for part in content.split("path = \"").skip(1) {
                    let end = part.find('"').unwrap_or(0);
                    out.push(part[..end].to_string());
                }
            }
        }
    }
    async fn routes(gated: bool) -> Vec<String> {
        let root = tempfile::tempdir().unwrap();
        let (output, mox) = write_gate_fixture(root.path());
        run_gate(root.path(), &output, &mox, gated).await;
        let mut out = Vec::new();
        route_paths(&output.join("src/api"), &mut out);
        out.sort();
        out.dedup();
        out
    }
    let flat = routes(false).await;
    let namespaced = routes(true).await;
    assert!(!flat.is_empty(), "handlers must declare route paths");
    assert_eq!(flat, namespaced, "URL segments are namespace-invariant");
}
