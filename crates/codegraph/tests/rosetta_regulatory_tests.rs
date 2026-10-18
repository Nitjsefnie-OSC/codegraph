//! Regulatory reference plane tests (issue #265).
//!
//! The rosetta bridge lands the regulatory node family (report/body/
//! corpus/segment/rule source/rule schema/meta type as ONE parameterized
//! `RegulatoryNode`) plus the edges the sigil model actually expresses
//! (`RegulatoryReference`/`HasRuleSource`/`CorpusInBody`/`DerivesFrom`);
//! the `regulatory_reports` generator (gated behind `rosetta_backend`)
//! emits per-corpus report skeletons with TODO(#263)/TODO(#264) seams.

use codegraph_backend::{BackendConfig, create_backend};
use codegraph_config::config::{DomainConfig, parse_domain_config};
use codegraph_core::mock::MockEngine;
use codegraph_core::traits::{GraphIngestor, GraphQuerier};
use codegraph_core::types::{RegulatoryEdgeKind, RegulatoryKind, RegulatoryOwner};

// ── Fixture ────────────────────────────────────────────────────────────

fn fixture_model() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rosetta_regulatory/model/regulatory.rosetta")
}

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.reg]
label = "Regulatory"
schema_dir = "reg"
postgres_schema = "reg"
"#;

async fn bridge_fixture() -> (
    codegraph_backend::Backend,
    codegraph::ingest::rosetta_ingest::RosettaIngestOutcome,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(&domains_path, DOMAINS_TOML).unwrap();
    let domain_config: DomainConfig = parse_domain_config(&domains_path).unwrap();

    let backend = create_backend(&BackendConfig::default()).await.unwrap();
    let outcome = codegraph::ingest::rosetta_ingest::ingest_rosetta_files(
        backend.ingestor(),
        backend.querier(),
        &[fixture_model()],
        &domain_config,
        "Type",
    )
    .await
    .unwrap();
    (backend, outcome, dir)
}

// ── Bridge stats + needs_review semantics ──────────────────────────────

#[tokio::test]
async fn regulatory_nodes_ingest_with_stats_and_needs_review_shrinks() {
    let (_backend, outcome, _dir) = bridge_fixture().await;
    let stats = &outcome.stats;
    // 1 body + 2 corpora + 2 segments + 1 metaType + 1 schema + 1 rule
    // source + 2 reports.
    assert_eq!(stats.regulatory_nodes, 10, "stats: {stats}");
    // Type docReference (body+corpus+segment) + condition docReference
    // (body+corpus) + report 1 (body+corpus+segment+source) + report 2
    // (body+corpus) + corpus parent (ESMA→body).
    assert_eq!(stats.regulatory_edges, 12, "stats: {stats}");

    // The regulatory families left needs_review, #263 landed function
    // nodes (IngestOrders), and #264 landed the two rules — nothing
    // remains out-of-plane.
    assert_eq!(
        stats.needs_review, 0,
        "names: {:?}",
        stats.needs_review_names
    );
    assert_eq!(stats.functions_ingested, 1, "stats: {stats}");
    // The two fixture rules (PositiveTotal eligibility + ReportedStatus
    // reporting) land as RuleNodes; PositiveTotal's `from OrderType`
    // matches the bridged schema.
    assert_eq!(stats.rules_ingested, 2, "stats: {stats}");
    assert_eq!(stats.rule_applies_to, 2, "stats: {stats}");
    // AgencySource's `[ruleReference ReportedStatus]` promotes to a real
    // edge; `- total` carries none.
    assert_eq!(stats.rule_reference_edges, 1, "stats: {stats}");
    assert!(
        stats
            .needs_review_names
            .iter()
            .all(|n| !n.starts_with("rule ")
                && !n.starts_with("report ")
                && !n.starts_with("body ")
                && !n.starts_with("corpus ")
                && !n.starts_with("segment ")
                && !n.starts_with("schema ")
                && !n.starts_with("meta-type ")
                && !n.starts_with("func ")
                && !n.starts_with("external-rule-source "))
    );
}

// ── Node read-back (grafeo backend, as rosetta_condition_tests) ────────

#[tokio::test]
async fn regulatory_nodes_read_back_by_kind() {
    let (backend, _outcome, _dir) = bridge_fixture().await;
    let nodes = backend.querier().list_regulatory().await.unwrap();
    assert_eq!(nodes.len(), 10);
    // Order comes back kind+name sorted; index by (kind, name).
    let by_key = |kind: RegulatoryKind, name: &str| {
        nodes
            .iter()
            .find(|n| n.kind == kind && n.name == name)
            .unwrap_or_else(|| panic!("{kind:?} {name} missing: {nodes:?}"))
    };

    let body = by_key(RegulatoryKind::Body, "RegAgencyBody");
    assert_eq!(
        body.properties.get("body_type").and_then(|v| v.as_str()),
        Some("RegAgency")
    );
    assert_eq!(body.definition.as_deref(), Some("the agency body"));
    assert_eq!(
        body.properties.get("origin").and_then(|v| v.as_str()),
        Some("rosetta")
    );

    let esma = by_key(RegulatoryKind::Corpus, "ESMA");
    assert_eq!(esma.label.as_deref(), Some("esma"));
    assert_eq!(
        esma.properties.get("parent_body").and_then(|v| v.as_str()),
        Some("RegAgencyBody")
    );
    let cftc = by_key(RegulatoryKind::Corpus, "CFTC");
    assert_eq!(
        cftc.properties.get("parent_body"),
        Some(&serde_json::Value::Null)
    );

    let meta = by_key(RegulatoryKind::MetaType, "Calculation");
    assert_eq!(
        meta.properties.get("type_ref").and_then(|v| v.as_str()),
        Some("number"),
        "builtin type refs stay a properties string (no node to derive from)"
    );

    let schema = by_key(RegulatoryKind::RuleSchema, "FpMLSchema");
    assert_eq!(
        schema.properties.get("format").and_then(|v| v.as_str()),
        Some("FpML")
    );
    let transforms = schema
        .properties
        .get("transform_annotations")
        .and_then(|v| v.as_array())
        .expect("transform annotations captured on the rule schema node");
    assert_eq!(transforms.len(), 1);
    assert_eq!(
        transforms[0].get("function").and_then(|v| v.as_str()),
        Some("IngestOrders")
    );
    assert_eq!(
        transforms[0].get("kind").and_then(|v| v.as_str()),
        Some("ingest")
    );

    let source = by_key(RegulatoryKind::RuleSource, "AgencySource");
    let classes = source
        .properties
        .get("classes")
        .and_then(|v| v.as_array())
        .expect("rule source classes persist");
    assert_eq!(
        classes[0].get("data").and_then(|v| v.as_str()),
        Some("OrderType")
    );
    let attributes = classes[0]
        .get("attributes")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(attributes.len(), 2);
    assert_eq!(
        attributes[0].get("attribute").and_then(|v| v.as_str()),
        Some("status")
    );
    assert_eq!(
        attributes[0].get("add"),
        Some(&serde_json::Value::Bool(true))
    );

    // Anonymous reports get a deterministic synthesized name embedding the
    // regulatory reference + timing.
    let report = by_key(
        RegulatoryKind::Report,
        "Report RegAgencyBody ESMA Section1 \"1.a\" (T+1)",
    );
    assert_eq!(
        report.properties.get("timing").and_then(|v| v.as_str()),
        Some("T+1")
    );
    assert_eq!(
        report.properties.get("input_type").and_then(|v| v.as_str()),
        Some("OrderType")
    );
    assert_eq!(
        report
            .properties
            .get("rule_source")
            .and_then(|v| v.as_str()),
        Some("AgencySource")
    );
}

// ── Edge read-back ─────────────────────────────────────────────────────

#[tokio::test]
async fn regulatory_references_read_back() {
    let (backend, _outcome, _dir) = bridge_fixture().await;
    let refs = backend
        .querier()
        .list_regulatory_references()
        .await
        .unwrap();
    let has = |owner: &str,
               owner_label: &str,
               target: &str,
               target_kind: RegulatoryKind,
               edge: RegulatoryEdgeKind,
               ref_path: Option<&str>| {
        refs.iter().any(|r| {
            r.owner == owner
                && r.owner_label == owner_label
                && r.target == target
                && r.target_kind == target_kind
                && r.edge_kind == edge
                && r.ref_path.as_deref() == ref_path
        })
    };

    // Type-level docReference → body + corpus + segment. Body/corpus
    // edges carry no ref path (the docReference `provision` trailer is
    // not exercised: the pinned sigil rev's greedy item parser consumes
    // `provision "..."` as a segment pair — an upstream quirk, skipped
    // here because no declared Segment named `provision` exists).
    assert!(has(
        "OrderType",
        "Schema",
        "RegAgencyBody",
        RegulatoryKind::Body,
        RegulatoryEdgeKind::Reference,
        None
    ));
    assert!(has(
        "OrderType",
        "Schema",
        "ESMA",
        RegulatoryKind::Corpus,
        RegulatoryEdgeKind::Reference,
        None
    ));
    assert!(has(
        "OrderType",
        "Schema",
        "Section1",
        RegulatoryKind::Segment,
        RegulatoryEdgeKind::Reference,
        Some("1.a")
    ));

    // Condition-level docReference → corpus (Condition owner).
    assert!(has(
        "PositiveTotal",
        "Condition",
        "RegAgencyBody",
        RegulatoryKind::Body,
        RegulatoryEdgeKind::Reference,
        None
    ));
    assert!(has(
        "PositiveTotal",
        "Condition",
        "CFTC",
        RegulatoryKind::Corpus,
        RegulatoryEdgeKind::Reference,
        None
    ));

    // Report 1: regulatory ref + rule source (Regulatory owner).
    assert!(has(
        "Report RegAgencyBody ESMA Section1 \"1.a\" (T+1)",
        "Regulatory",
        "Section1",
        RegulatoryKind::Segment,
        RegulatoryEdgeKind::Reference,
        Some("1.a")
    ));
    assert!(has(
        "Report RegAgencyBody ESMA Section1 \"1.a\" (T+1)",
        "Regulatory",
        "AgencySource",
        RegulatoryKind::RuleSource,
        RegulatoryEdgeKind::RuleSource,
        None
    ));

    // Report 2: body + corpus only.
    assert!(has(
        "Report RegAgencyBody CFTC (real-time)",
        "Regulatory",
        "RegAgencyBody",
        RegulatoryKind::Body,
        RegulatoryEdgeKind::Reference,
        None
    ));

    // Corpus parent body.
    assert!(has(
        "ESMA",
        "Regulatory",
        "RegAgencyBody",
        RegulatoryKind::Body,
        RegulatoryEdgeKind::CorpusInBody,
        None
    ));

    // No DerivesFrom edges: the metaType's type_ref is builtin (`number`)
    // and the rule source has no `extends`.
    assert!(
        !refs
            .iter()
            .any(|r| r.edge_kind == RegulatoryEdgeKind::DerivesFrom)
    );
}

#[tokio::test]
async fn doc_reference_targets_with_no_declared_element_are_skipped() {
    // The fixture only carries resolvable targets; this test pins the
    // best-effort contract on the mock engine: an edge naming an
    // un-declared regulatory node is dropped silently (sigil passes doc
    // references through unvalidated).
    let engine = MockEngine::new();
    engine
        .ingest_regulatory_reference(
            &RegulatoryOwner::Schema("OrderType".to_string()),
            "NeverDeclared",
            RegulatoryKind::Body,
            RegulatoryEdgeKind::Reference,
            None,
        )
        .await
        .unwrap();
    assert!(
        engine
            .list_regulatory_references()
            .await
            .unwrap()
            .is_empty()
    );
}

// ── Mock engine parity (same bridge, mock backend) ─────────────────────

#[tokio::test]
async fn bridge_lands_regulatory_nodes_in_the_mock_engine() {
    let dir = tempfile::tempdir().unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(&domains_path, DOMAINS_TOML).unwrap();
    let domain_config: DomainConfig = parse_domain_config(&domains_path).unwrap();

    let engine = MockEngine::new();
    let outcome = codegraph::ingest::rosetta_ingest::ingest_rosetta_files(
        &engine,
        &engine,
        &[fixture_model()],
        &domain_config,
        "Type",
    )
    .await
    .unwrap();

    assert_eq!(outcome.stats.regulatory_nodes, 10);
    let nodes = engine.list_regulatory().await.unwrap();
    assert_eq!(nodes.len(), 10);
    let refs = engine.list_regulatory_references().await.unwrap();
    assert_eq!(refs.len(), 12);
}

// ── regulatory_reports generator (rosetta_backend gate) ────────────────

const GENERATOR_MODEL_COPY: &str =
    include_str!("fixtures/rosetta_regulatory/model/regulatory.rosetta");

fn profiles_toml(regulatory_reports: bool) -> String {
    let mut toml = r#"
[profiles.default.meta]
name = "rosetta-reg"
version = "1.0.0"
description = "regulatory_reports gate test"

[profiles.default.features]
rosetta_backend = true

[profiles.default.api]
generators = ["dto", "regulatory_reports"]
"#
    .to_string();
    if !regulatory_reports {
        toml = toml.replace("\"dto\", \"regulatory_reports\"", "\"dto\"");
    }
    toml
}

async fn run_generator_pipeline(
    dir: &std::path::Path,
    regulatory_reports: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let model = dir.join("regulatory.rosetta");
    std::fs::write(&model, GENERATOR_MODEL_COPY)?;
    let domains = dir.join("domains.toml");
    std::fs::write(&domains, DOMAINS_TOML)?;
    let profiles = dir.join("profiles.toml");
    std::fs::write(&profiles, profiles_toml(regulatory_reports))?;
    let output = dir.join("generated");

    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &domains,
        output: &output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(profiles),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &[],
        rosetta_files: &[model],
        ddd_files: &[],
        evt_files: &[],
        deploy_files: &[],
        ifml_framework: &[],
        ifml_components: None,
        ifml_design_system: None,
        codegraph_rev: None,
        check: false,
        ux_rules: None,
    })
    .await?;
    Ok(())
}

fn read_regulatory_reports(dir: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(dir.join("generated/src/domain/reg/regulatory_reports.rs")).ok()
}

#[tokio::test]
async fn regulatory_reports_emits_corpus_dispatch_and_transform_hooks_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    run_generator_pipeline(dir.path(), true)
        .await
        .expect("pipeline with rosetta_backend + regulatory_reports succeeds");

    let content = read_regulatory_reports(dir.path()).expect("regulatory_reports.rs emitted");
    // Per-corpus module + dispatch table seeded from the report segments.
    assert!(content.contains("pub mod esma {"), "{content}");
    assert!(
        content.contains("pub fn esma_report_dispatch(segment: &str) -> Option<&'static str> {"),
        "{content}"
    );
    assert!(content.contains("\"1.a\" => {"), "{content}");
    // Rules landed (#264): the dispatch arm resolves the rules bound
    // through the report's rule source; the genuinely-deferred seams
    // (endpoint handler wiring, transform-hook wire format) keep their
    // markers. No stale seams may remain.
    assert!(
        content.contains("with source AgencySource — rules: ReportedStatus."),
        "{content}"
    );
    assert!(
        content.contains("Rule fns: super::rules::{reported_status} (via the `rules` generator)."),
        "{content}"
    );
    assert!(content.contains("TODO(#264)"), "{content}");
    assert!(
        content.contains("Function `IngestOrders` is bridged; its body emits via the `functions` generator (`ingest_orders`)."),
        "{content}"
    );
    assert!(
        !content.contains("TODO(#263)"),
        "no #263 seams may remain: {content}"
    );
    assert!(
        !content.contains("signature registry"),
        "the registry language is retired: {content}"
    );
    // The report's `with source` binding is documented at its arm.
    assert!(content.contains("with source AgencySource"), "{content}");
    // Rule source surface: bindings_registered counts the resolvable
    // RuleReference bindings (ReportedStatus via AgencySource.OrderType).
    assert!(content.contains("pub mod agency_source {"), "{content}");
    assert!(
        content.contains("+ status -> rules [ReportedStatus]"),
        "{content}"
    );
    assert!(
        content.contains("pub fn bindings_registered() -> usize {"),
        "{content}"
    );
    assert!(
        content.contains("        1"),
        "the binding count: {content}"
    );
    // Transform hook captured from `[ingest FpMLSchema]` on IngestOrders.
    assert!(content.contains("pub mod fp_ml_schema {"), "{content}");
    assert!(
        content.contains("pub fn ingest_orders_hook() {"),
        "{content}"
    );
    // The emitted module must be plain Rust: dispatch arms stay inert
    // (scaffolding, no payloads).
    assert!(content.contains("_ => None,"), "{content}");
}

#[tokio::test]
async fn regulatory_reports_feature_listed_without_generator_emits_nothing() {
    let dir = tempfile::tempdir().unwrap();
    run_generator_pipeline(dir.path(), false)
        .await
        .expect("pipeline without regulatory_reports listed succeeds");

    assert!(
        read_regulatory_reports(dir.path()).is_none(),
        "regulatory_reports.rs must not exist when the generator is not listed"
    );
}

#[tokio::test]
async fn regulatory_reports_listed_while_rosetta_backend_off_is_a_configuration_error() {
    // Byte-identity protection: the capability gate hard-errors when the
    // generator is listed but its feature is off, instead of silently
    // running (or silently diverging).
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("regulatory.rosetta");
    std::fs::write(&model, GENERATOR_MODEL_COPY).unwrap();
    let domains = dir.path().join("domains.toml");
    std::fs::write(&domains, DOMAINS_TOML).unwrap();
    let profiles = dir.path().join("profiles.toml");
    std::fs::write(
        &profiles,
        r#"
[profiles.default.meta]
name = "rosetta-reg-misconfig"
version = "1.0.0"
description = "regulatory_reports misconfiguration gate"

[profiles.default.features]
rosetta_backend = false

[profiles.default.api]
generators = ["dto", "regulatory_reports"]
"#,
    )
    .unwrap();

    let result = codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &domains,
        output: &dir.path().join("generated"),
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(profiles),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &[],
        rosetta_files: &[model],
        ddd_files: &[],
        evt_files: &[],
        deploy_files: &[],
        ifml_framework: &[],
        ifml_components: None,
        ifml_design_system: None,
        codegraph_rev: None,
        check: false,
        ux_rules: None,
    })
    .await;

    assert!(
        result.is_err(),
        "listing regulatory_reports with rosetta_backend off must fail plan validation"
    );
    assert!(
        read_regulatory_reports(dir.path()).is_none(),
        "no output may be written when plan validation fails"
    );
}
