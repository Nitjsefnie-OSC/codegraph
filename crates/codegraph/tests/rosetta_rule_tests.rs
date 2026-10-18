//! Rule node family + rules codegen tests (issue #264).
//!
//! The rosetta bridge lands sigil `Rule` elements (reporting/eligibility)
//! as structured `RuleNode`s (kind, `from` input type, canonical
//! `Expr::to_json` body payload) instead of needs_review; input types
//! attach via `RuleAppliesTo` edges, rule doc references via
//! `RegulatoryOwner::Rule`, and rule-source `[ruleReference R]` bindings
//! promote to `RuleReference` edges when R resolves within the run. The
//! `rules` generator (gated behind `rosetta_backend`) emits per-domain
//! Rust: reporting rules as computed-field functions attached to their
//! input entity, eligibility rules as endpoint guards naming the
//! ApiOperations they gate; untranspilable bodies keep TODO(#264)
//! markers.

use std::path::Path;

use codegraph_backend::{BackendConfig, create_backend};
use codegraph_config::config::{DomainConfig, parse_domain_config};
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::{RegulatoryEdgeKind, RegulatoryKind, RuleKind};

// ── Fixture (the shared rosetta_bridge store model + its rules) ────────

fn store_model() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rosetta_bridge/model/store.rosetta")
}

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.partners]
label = "Partners"
schema_dir = "partners"
postgres_schema = "partners"

[domains.store]
label = "Store"
schema_dir = "store"
postgres_schema = "store"
"#;

async fn bridge_store() -> (
    codegraph_backend::Backend,
    codegraph::ingest::rosetta_ingest::RosettaIngestOutcome,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(&domains_path, DOMAINS_TOML).unwrap();
    let domain_config: DomainConfig = parse_domain_config(&domains_path).unwrap();

    let model_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rosetta_bridge/model");
    let backend = create_backend(&BackendConfig::default()).await.unwrap();
    let outcome = codegraph::ingest::rosetta_ingest::ingest_rosetta_files(
        backend.ingestor(),
        backend.querier(),
        &[model_dir.join("partners.rosetta"), store_model()],
        &domain_config,
        "Type",
    )
    .await
    .unwrap();
    (backend, outcome, dir)
}

// ── Bridge stats + needs_review shrink ─────────────────────────────────

#[tokio::test]
async fn rule_nodes_ingest_with_stats_and_needs_review_shrinks() {
    let (_backend, outcome, _dir) = bridge_store().await;
    let stats = &outcome.stats;
    // HighValueTotal + TierForTotal (reporting), EligibleOrder
    // (eligibility).
    assert_eq!(stats.rules_ingested, 3, "stats: {stats}");
    assert!(
        !stats
            .needs_review_names
            .iter()
            .any(|n| n.starts_with("rule ")),
        "rules left needs_review in #264: {:?}",
        stats.needs_review_names
    );
    // All three rules declare `from OrderType`, which bridges this run.
    assert_eq!(stats.rule_applies_to, 3, "stats: {stats}");
    // StoreSource.OrderType.total [ruleReference HighValueTotal]; the
    // `+ label` attribute carries no reference.
    assert_eq!(stats.rule_reference_edges, 1, "stats: {stats}");
    assert_eq!(stats.rule_reference_skips, 0, "stats: {stats}");
}

#[tokio::test]
async fn rules_never_land_in_bridged_titles() {
    // Equivalence of concerns: rules are computation plane — they create
    // NO SchemaNodes, so classification and DDL stay untouched.
    let (backend, outcome, _dir) = bridge_store().await;
    for name in [
        "HighValueTotal",
        "TierForTotal",
        "EligibleOrder",
        "StoreSource",
    ] {
        assert!(
            !outcome.stats.bridged_titles.iter().any(|t| t == name),
            "{name} must not bridge as a schema"
        );
        let schemas = backend.querier().list_schemas(None).await.unwrap();
        assert!(
            !schemas.iter().any(|s| s.title == name),
            "{name} must not become a schema node"
        );
    }
}

// ── Node read-back: structured payloads ────────────────────────────────

#[tokio::test]
async fn rule_nodes_read_back_with_structured_payloads() {
    let (backend, _outcome, _dir) = bridge_store().await;
    let rules = backend.querier().list_rules().await.unwrap();
    // list_rules orders by (domain, name) — all fixture rules live in
    // `store`.
    let by_name = |name: &str| {
        rules
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("{name} missing: {rules:?}"))
    };

    // Transpilable reporting rule: definition, input type, Comparison
    // payload.
    let high = by_name("HighValueTotal");
    assert_eq!(high.domain.as_deref(), Some("store"));
    assert_eq!(high.kind, RuleKind::Reporting);
    assert_eq!(
        high.definition.as_deref(),
        Some("orders above the reporting threshold")
    );
    assert_eq!(high.input_type.as_deref(), Some("OrderType"));
    let payload: serde_json::Value = serde_json::from_str(&high.expr_json).unwrap();
    assert_eq!(payload.get("kind").and_then(|k| k.as_str()), Some("Binary"));
    assert_eq!(payload.get("op").and_then(|o| o.as_str()), Some(">"));
    assert_eq!(
        high.properties.get("origin").and_then(|v| v.as_str()),
        Some("rosetta")
    );

    // Eligibility rule.
    let eligible = by_name("EligibleOrder");
    assert_eq!(eligible.kind, RuleKind::Eligibility);
    assert_eq!(eligible.input_type.as_deref(), Some("OrderType"));
    assert_eq!(
        eligible.definition.as_deref(),
        Some("orders that may be reported")
    );

    // The untranspilable rule still lands (the transpiler rejection is a
    // GENERATION-time concern): reference-guard Switch payload.
    let tier = by_name("TierForTotal");
    assert_eq!(tier.kind, RuleKind::Reporting);
    let payload: serde_json::Value = serde_json::from_str(&tier.expr_json).unwrap();
    assert_eq!(payload.get("kind").and_then(|k| k.as_str()), Some("Switch"));
}

#[tokio::test]
async fn rule_applies_to_edges_read_back() {
    let (backend, _outcome, _dir) = bridge_store().await;
    let edges = backend.querier().list_rule_applies_to().await.unwrap();
    assert_eq!(
        edges,
        vec![
            ("EligibleOrder".to_string(), "OrderType".to_string()),
            ("HighValueTotal".to_string(), "OrderType".to_string()),
            ("TierForTotal".to_string(), "OrderType".to_string()),
        ],
        "every fixture rule attaches to its input entity, rule-ordered"
    );
}

#[tokio::test]
async fn rule_source_references_promote_to_edges() {
    let (backend, _outcome, _dir) = bridge_store().await;
    let refs = backend.querier().list_rule_references().await.unwrap();
    assert_eq!(
        refs,
        vec![codegraph_core::types::RuleRefRecord {
            schema_title: "OrderType".to_string(),
            attribute: "total".to_string(),
            rule: "HighValueTotal".to_string(),
            rule_source: "StoreSource".to_string(),
        }],
        "the StoreSource class binding promotes to a RuleReference edge"
    );
}

#[tokio::test]
async fn unknown_rule_references_are_resolution_errors() {
    // Sigil resolution validates `[ruleReference R]` targets (E0101): a
    // reference naming no workspace rule hard-errors before anything
    // bridges — a broken reference can never half-bridge. The bridge's
    // out-of-run skip counter stays as defense-in-depth for rules that
    // resolve but live outside the bridged file set (the FunctionExtends
    // precedent).
    const MODEL: &str = r#"
namespace skip.dom

type Thing:
	id string (1..1)

reporting rule Present from Thing:
	id exists

rule source OutsideSource {
	Thing:
		+ id [ruleReference MissingRule]
}
"#;
    let dir = tempfile::tempdir().unwrap();
    let model_path = dir.path().join("skip.rosetta");
    std::fs::write(&model_path, MODEL).unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(
        &domains_path,
        r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.skip]
label = "Skip"
schema_dir = "skip"
postgres_schema = "skip"
"#,
    )
    .unwrap();
    let backend = create_backend(&BackendConfig::default()).await.unwrap();
    let domain_config: DomainConfig = parse_domain_config(&domains_path).unwrap();
    let err = codegraph::ingest::rosetta_ingest::ingest_rosetta_files(
        backend.ingestor(),
        backend.querier(),
        &[model_path],
        &domain_config,
        "Type",
    )
    .await
    .expect_err("an unknown rule reference must hard-error at resolution");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown rule 'MissingRule'"),
        "error names the bad reference: {msg}"
    );
}

// ── Rule docReference → RegulatoryReference edges ───────────────────────

#[tokio::test]
async fn rule_doc_references_link_to_regulatory_nodes() {
    const MODEL: &str = r#"
namespace docref.dom

body RegElement RegBody <"the body">

type DocType:
	id string (1..1)

reporting rule Documented from DocType:
	[docReference RegBody]
	id exists
"#;
    let dir = tempfile::tempdir().unwrap();
    let model_path = dir.path().join("docref.rosetta");
    std::fs::write(&model_path, MODEL).unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(
        &domains_path,
        r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.docref]
label = "Docref"
schema_dir = "docref"
postgres_schema = "docref"
"#,
    )
    .unwrap();
    let backend = create_backend(&BackendConfig::default()).await.unwrap();
    let domain_config: DomainConfig = parse_domain_config(&domains_path).unwrap();
    let outcome = codegraph::ingest::rosetta_ingest::ingest_rosetta_files(
        backend.ingestor(),
        backend.querier(),
        &[model_path],
        &domain_config,
        "Type",
    )
    .await
    .unwrap();
    assert_eq!(outcome.stats.rules_ingested, 1);
    assert_eq!(outcome.stats.regulatory_edges, 1, "the rule docReference");

    let refs = backend
        .querier()
        .list_regulatory_references()
        .await
        .unwrap();
    assert!(
        refs.iter().any(|r| r.owner == "Documented"
            && r.owner_label == "Rule"
            && r.target == "RegBody"
            && r.target_kind == RegulatoryKind::Body
            && r.edge_kind == RegulatoryEdgeKind::Reference),
        "RegulatoryReference edge FROM the RuleNode: {refs:?}"
    );
}

#[tokio::test]
async fn mock_engine_lands_the_same_rule_surface() {
    let dir = tempfile::tempdir().unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(&domains_path, DOMAINS_TOML).unwrap();
    let domain_config: DomainConfig = parse_domain_config(&domains_path).unwrap();

    let engine = codegraph_core::mock::MockEngine::new();
    let model_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rosetta_bridge/model");
    let outcome = codegraph::ingest::rosetta_ingest::ingest_rosetta_files(
        &engine,
        &engine,
        &[model_dir.join("partners.rosetta"), store_model()],
        &domain_config,
        "Type",
    )
    .await
    .unwrap();
    assert_eq!(outcome.stats.rules_ingested, 3);
    assert_eq!(engine.list_rules().await.unwrap().len(), 3);
    assert_eq!(engine.list_rule_applies_to().await.unwrap().len(), 3);
    assert_eq!(engine.list_rule_references().await.unwrap().len(), 1);
}

// ── rules generator (rosetta_backend gate) ──────────────────────────────

const GENERATOR_MODEL: &str = r#"
namespace gen.store

enum OrderStatus:
	Draft
	Shipped

type OrderType:
	total number (1..1)
	label string (0..1)
	tags string (1..*)
	status OrderStatus (1..1)

reporting rule HighValueTotal from OrderType: <"orders above the threshold">
	total > 100.0

reporting rule TagCount from OrderType:
	tags count

reporting rule TierForTotal from OrderType:
	status switch
		Draft then 1,
		default 0

eligibility rule EligibleOrder from OrderType: <"orders that may be reported">
	total > 0.0 and label exists
"#;

fn profiles_toml(rules: bool) -> String {
    let mut generators = "[\"dto\"".to_string();
    if rules {
        generators.push_str(", \"rules\"");
    }
    generators.push(']');
    format!(
        r#"
[profiles.default.meta]
name = "rosetta-rules"
version = "1.0.0"
description = "rules gate test"

[profiles.default.features]
rosetta_backend = true

[profiles.default.api]
generators = {generators}
"#
    )
}

async fn run_generator_pipeline(
    dir: &Path,
    model_text: &str,
    rules: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let model = dir.join("gen.rosetta");
    std::fs::write(&model, model_text)?;
    let domains = dir.join("domains.toml");
    std::fs::write(&domains, DOMAINS_TOML)?;
    let profiles = dir.join("profiles.toml");
    std::fs::write(&profiles, profiles_toml(rules))?;
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

fn read_rules_rs(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join("generated/src/domain/store/rules.rs")).ok()
}

#[tokio::test]
async fn rules_generator_emits_golden_rust_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    run_generator_pipeline(dir.path(), GENERATOR_MODEL, true)
        .await
        .expect("pipeline with rosetta_backend + rules succeeds");

    let content = read_rules_rs(dir.path()).expect("rules.rs emitted");

    // Reporting rule → computed-field function: borrowed input, inferred
    // bool return, transpiled body over the `input` receiver.
    assert!(
        content.contains("pub fn high_value_total(input: &OrderType) -> bool {"),
        "{content}"
    );
    assert!(content.contains("input.total > 100.0"), "{content}");

    // Collection knowledge flows from the entity's PropertyNodes: `tags`
    // is an array, so count transpiles (i64).
    assert!(
        content.contains("pub fn tag_count(input: &OrderType) -> i64 {"),
        "{content}"
    );

    // The reference-guard switch cannot transpile (documented gap): the
    // rule emits a TODO(#264) marker instead of a guessed function.
    assert!(
        content.contains(
            "// TODO(#264): transpile rule 'TierForTotal' (unsupported: Switch: reference guard \
             'Draft' needs enum/choice type knowledge (documented gap))"
        ),
        "{content}"
    );

    // Eligibility rule → endpoint guard fn + ApiOperation wiring doc
    // (resolve_entity_operations falls back to the configured defaults:
    // create/read/update/delete/list over the normalized resource).
    assert!(
        content.contains("pub fn eligible_order(input: &OrderType) -> bool {"),
        "{content}"
    );
    assert!(
        content.contains("input.total > 0.0) && input.label.is_some()"),
        "{content}"
    );
    assert!(
        content
            .contains("`create_Order`, `read_Order`, `update_Order`, `delete_Order`, `list_Order`"),
        "{content}"
    );
    assert!(
        content.contains("/// orders that may be reported"),
        "{content}"
    );
}

#[tokio::test]
async fn rules_generator_off_emits_nothing() {
    let dir = tempfile::tempdir().unwrap();
    run_generator_pipeline(dir.path(), GENERATOR_MODEL, false)
        .await
        .expect("pipeline without rules listed succeeds");
    assert!(
        read_rules_rs(dir.path()).is_none(),
        "rules.rs must not exist when the generator is not listed"
    );
}

#[tokio::test]
async fn rules_listed_while_rosetta_backend_off_is_a_configuration_error() {
    // Byte-identity protection: the capability gate hard-errors when the
    // generator is listed but its feature is off.
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("gen.rosetta");
    std::fs::write(&model, GENERATOR_MODEL).unwrap();
    let domains = dir.path().join("domains.toml");
    std::fs::write(&domains, DOMAINS_TOML).unwrap();
    let profiles = dir.path().join("profiles.toml");
    std::fs::write(
        &profiles,
        r#"
[profiles.default.meta]
name = "rosetta-rules-misconfig"
version = "1.0.0"
description = "rules misconfiguration gate"

[profiles.default.features]
rosetta_backend = false

[profiles.default.api]
generators = ["dto", "rules"]
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
        "listing rules with rosetta_backend off must fail plan validation"
    );
    assert!(
        read_rules_rs(dir.path()).is_none(),
        "no output may be written when plan validation fails"
    );
}

// ── Equivalence of concerns: rules alter no other output ────────────────

/// The generator model minus every rule and the rule source — the same
/// data model the equivalence run bridges.
const GENERATOR_MODEL_NO_RULES: &str = r#"
namespace gen.store

enum OrderStatus:
	Draft
	Shipped

type OrderType:
	total number (1..1)
	label string (0..1)
	tags string (1..*)
	status OrderStatus (1..1)
"#;

#[tokio::test]
async fn rules_alter_no_output_beyond_their_own_module() {
    // Two runs with the SAME profiles (rules listed): one bridges the
    // model with rules, one without. Every generated file must be
    // byte-identical except the rules module itself — pinning that rule
    // ingestion does not perturb classification, DDL, entities, or any
    // other generator's view of the graph.
    let dir_with = tempfile::tempdir().unwrap();
    let dir_without = tempfile::tempdir().unwrap();
    run_generator_pipeline(dir_with.path(), GENERATOR_MODEL, true)
        .await
        .expect("with-rules run");
    run_generator_pipeline(dir_without.path(), GENERATOR_MODEL_NO_RULES, true)
        .await
        .expect("without-rules run");

    let out_with = dir_with.path().join("generated");
    let out_without = dir_without.path().join("generated");
    assert!(
        read_rules_rs(dir_with.path()).is_some(),
        "the with-rules run emits the module"
    );
    assert!(
        read_rules_rs(dir_without.path()).is_none(),
        "a model without rules emits no module"
    );

    let collect = |root: &Path| -> std::collections::BTreeMap<String, String> {
        let mut files = std::collections::BTreeMap::new();
        for path in walk(root) {
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            files.insert(rel, content);
        }
        files
    };
    let with = collect(&out_with);
    let without = collect(&out_without);

    let mut diff = Vec::new();
    for (rel, content) in &with {
        match without.get(rel) {
            None => diff.push(format!("only in with-rules run: {rel}")),
            Some(other) => {
                if other != content {
                    diff.push(format!("content drift: {rel}"));
                }
            }
        }
    }
    for rel in without.keys() {
        if !with.contains_key(rel) {
            diff.push(format!("only in without-rules run: {rel}"));
        }
    }
    // The ONLY permitted differences: the rules module itself plus the
    // two mechanical registries that index emitted files — the domain
    // mod.rs (declares the new module) and the run manifest (inventories
    // it). Everything classification/DDL/entity-shaped must be identical.
    let permitted: std::collections::BTreeSet<String> = [
        "src/domain/store/rules.rs".to_string(),
        "src/domain/store/mod.rs".to_string(),
        ".codegraph-manifest.json".to_string(),
    ]
    .into_iter()
    .collect();
    let unexpected: Vec<String> = diff
        .iter()
        .filter(|entry| {
            !permitted.contains(
                entry
                    .rsplit_once(": ")
                    .map(|(_, file)| file)
                    .unwrap_or(entry),
            )
        })
        .cloned()
        .collect();
    assert!(
        unexpected.is_empty(),
        "rules must not alter any other generated file: {unexpected:?} (full diff: {diff:?})"
    );
}

fn walk(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}
