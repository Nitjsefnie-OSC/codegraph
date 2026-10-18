//! Function node family + func codegen tests (issue #263).
//!
//! The rosetta bridge lands sigil `Function` elements as structured
//! `FunctionNode`s (dispatch head, typed inputs/output, aliases,
//! `set`/`add` operations with `->` paths, post-conditions, transform
//! annotations) instead of needs_review; the `functions` generator (gated
//! behind `rosetta_backend`) emits per-domain Rust free functions with
//! transpiled bodies, aliases as let-bindings, dispatch heads as `match`,
//! and `debug_assert!` post-conditions under the `function_postconditions`
//! feature.

use std::path::Path;

use codegraph_backend::{BackendConfig, create_backend};
use codegraph_config::config::{DomainConfig, parse_domain_config};
use codegraph_core::traits::GraphQuerier;

// ── Fixture (the shared rosetta_bridge store model + its functions) ────

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
async fn function_nodes_ingest_with_stats_and_needs_review_shrinks() {
    let (_backend, outcome, _dir) = bridge_store().await;
    let stats = &outcome.stats;
    // OrderScore, ValueForStatus, ShippedValue, ApprovedValue.
    assert_eq!(stats.functions_ingested, 4, "stats: {stats}");
    assert!(
        !stats
            .needs_review_names
            .iter()
            .any(|n| n.starts_with("func ")),
        "functions left needs_review in #263: {:?}",
        stats.needs_review_names
    );
}

#[tokio::test]
async fn functions_never_land_in_bridged_titles() {
    // Equivalence-of-concerns: functions are computation plane — they
    // create NO SchemaNodes, so classification and DDL stay untouched.
    let (backend, outcome, _dir) = bridge_store().await;
    for name in [
        "OrderScore",
        "ValueForStatus",
        "ShippedValue",
        "ApprovedValue",
    ] {
        assert!(
            !outcome.stats.bridged_titles.iter().any(|t| t == name),
            "{name} must not bridge as a schema"
        );
    }
    let schemas = backend.querier().list_schemas(None).await.unwrap();
    for name in [
        "OrderScore",
        "ValueForStatus",
        "ShippedValue",
        "ApprovedValue",
    ] {
        assert!(
            !schemas.iter().any(|s| s.title == name),
            "{name} must not become a schema node"
        );
    }
}

// ── Node read-back: structured payloads ────────────────────────────────

#[tokio::test]
async fn function_nodes_read_back_with_structured_payloads() {
    let (backend, _outcome, _dir) = bridge_store().await;
    let functions = backend.querier().list_functions().await.unwrap();
    // list_functions orders by (domain, name) — both fixture functions
    // live in `store`.
    let by_name = |name: &str| {
        functions
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("{name} missing: {functions:?}"))
    };

    // Simple derived value: alias, two ops, post-condition, [enrich].
    let score = by_name("OrderScore");
    assert_eq!(score.domain.as_deref(), Some("store"));
    assert_eq!(
        score.definition.as_deref(),
        Some("a simple derived value with an alias and a post-condition")
    );
    assert!(score.dispatch.is_none());
    assert_eq!(score.inputs.len(), 1);
    assert_eq!(score.inputs[0].name, "order");
    assert_eq!(score.inputs[0].type_ref, "OrderType");
    assert!(!score.inputs[0].is_array);
    assert!(!score.inputs[0].is_optional);
    let output = score.output.as_ref().expect("OrderScore output");
    assert_eq!(
        (output.name.as_str(), output.type_ref.as_str()),
        ("result", "number")
    );
    assert_eq!(score.aliases.len(), 1);
    assert_eq!(score.aliases[0].name, "base");
    assert!(
        score.aliases[0].expr_json.contains("FeatureCall"),
        "alias payload is the Expr::to_json shape: {}",
        score.aliases[0].expr_json
    );
    assert_eq!(score.operations.len(), 2);
    assert!(!score.operations[0].is_add);
    assert_eq!(score.operations[0].assign_root, "result");
    assert!(score.operations[0].path.is_empty());
    assert!(score.operations[1].is_add, "the count op is an `add`");
    assert_eq!(score.post_conditions.len(), 1);
    assert_eq!(score.post_conditions[0].name, "ResultPresent");
    assert_eq!(
        score.post_conditions[0].definition.as_deref(),
        Some("the result is present")
    );
    assert_eq!(score.transform_annotations.len(), 1);
    assert_eq!(
        score.transform_annotations[0].kind,
        codegraph_core::types::FunctionTransformKind::Enrich
    );
    assert_eq!(score.transform_annotations[0].reference, None);
    assert_eq!(
        score.properties.get("origin").and_then(|v| v.as_str()),
        Some("rosetta")
    );

    // Dispatch head: (status: OrderStatus->Draft).
    let dispatch_parent = by_name("ValueForStatus");
    let dispatch = dispatch_parent.dispatch.as_ref().expect("dispatch head");
    assert_eq!(
        (
            dispatch.attribute.as_str(),
            dispatch.enumeration.as_str(),
            dispatch.value.as_str()
        ),
        ("status", "OrderStatus", "Draft")
    );
    assert_eq!(dispatch_parent.inputs.len(), 2);
    assert_eq!(dispatch_parent.inputs[1].type_ref, "OrderStatus");
    assert!(dispatch_parent.extends.is_none());

    // Extends resolution: children carry the parent name.
    let shipped = by_name("ShippedValue");
    assert_eq!(shipped.extends.as_deref(), Some("ValueForStatus"));
    assert_eq!(shipped.aliases.len(), 1);
    assert_eq!(shipped.aliases[0].name, "net");
    let approved = by_name("ApprovedValue");
    assert_eq!(approved.extends.as_deref(), Some("ValueForStatus"));
    assert_eq!(
        approved.dispatch.as_ref().map(|d| d.value.as_str()),
        Some("Approved")
    );
}

#[tokio::test]
async fn function_extends_edges_read_back() {
    let (backend, _outcome, _dir) = bridge_store().await;
    let edges = backend.querier().list_function_extends().await.unwrap();
    assert_eq!(
        edges,
        vec![
            ("ApprovedValue".to_string(), "ValueForStatus".to_string()),
            ("ShippedValue".to_string(), "ValueForStatus".to_string()),
        ],
        "both children link to their parent, child-ordered"
    );
}

#[tokio::test]
async fn mock_engine_lands_the_same_function_surface() {
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
    assert_eq!(outcome.stats.functions_ingested, 4);
    assert_eq!(engine.list_functions().await.unwrap().len(), 4);
    let edges = engine.list_function_extends().await.unwrap();
    assert_eq!(edges.len(), 2);
}

// ── Extends cycle = hard error naming the cycle ─────────────────────────

const CYCLE_MODEL: &str = r#"
namespace cycle.dom

type Thing:
	id string (1..1)

func Alpha extends Beta:
	output:
		result number (1..1)

	set result:
		1.0

func Beta extends Alpha:
	output:
		result number (1..1)

	set result:
		2.0
"#;

#[tokio::test]
async fn function_extends_cycle_is_a_hard_error_naming_the_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let model_path = dir.path().join("cycle.rosetta");
    std::fs::write(&model_path, CYCLE_MODEL).unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(
        &domains_path,
        r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.cycle]
label = "Cycle"
schema_dir = "cycle"
postgres_schema = "cycle"
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
    .expect_err("an extends cycle must hard-error");
    let msg = err.to_string();
    assert!(msg.contains("cycle"), "error names the failure: {msg}");
    assert!(
        msg.contains("Alpha -> Beta -> Alpha"),
        "error describes the cycle path: {msg}"
    );
}

// ── Function docReference → RegulatoryReference edges ───────────────────

const DOCREF_MODEL: &str = r#"
namespace docref.dom

body RegElement RegBody <"the body">

type DocType:
	id string (1..1)

func Documented:
	[docReference RegBody]
	inputs:
		item DocType (1..1)
	output:
		result number (1..1)

	set result:
		1.0
"#;

#[tokio::test]
async fn function_doc_references_link_to_regulatory_nodes() {
    let dir = tempfile::tempdir().unwrap();
    let model_path = dir.path().join("docref.rosetta");
    std::fs::write(&model_path, DOCREF_MODEL).unwrap();
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
    assert_eq!(outcome.stats.functions_ingested, 1);
    assert_eq!(outcome.stats.regulatory_edges, 1, "the func docReference");

    let refs = backend
        .querier()
        .list_regulatory_references()
        .await
        .unwrap();
    assert!(
        refs.iter().any(|r| r.owner == "Documented"
            && r.owner_label == "Function"
            && r.target == "RegBody"
            && r.edge_kind == codegraph_core::types::RegulatoryEdgeKind::Reference),
        "RegulatoryReference edge FROM the FunctionNode: {refs:?}"
    );
}

// ── functions generator (rosetta_backend gate) ──────────────────────────

const GENERATOR_MODEL: &str = r#"
namespace gen.store

enum OrderStatus:
	Draft
	Shipped

type OrderType:
	total number (1..1)
	tags string (0..*)

func OrderScore: <"a simple derived value">
	[enrich]
	inputs:
		order OrderType (1..1)
	output:
		result number (1..1)

	alias base: order -> total

	set result:
		base + order -> total

	add result:
		order -> tags count

	post-condition ResultPresent: <"the result is present">
		result exists

func ValueForStatus(status: OrderStatus->Draft):
	inputs:
		order OrderType (1..1)
		status OrderStatus (1..1)
	output:
		result number (1..1)

	set result:
		1.0

func ShippedValue(status: OrderStatus->Shipped) extends ValueForStatus:
	inputs:
		order OrderType (1..1)
		status OrderStatus (1..1)
	output:
		result number (1..1)

	alias net: order -> total

	set result:
		net * 2.0
"#;

fn profiles_toml(functions: bool, postconditions: bool) -> String {
    let mut generators = "[\"dto\"".to_string();
    if functions {
        generators.push_str(", \"functions\"");
    }
    generators.push(']');
    format!(
        r#"
[profiles.default.meta]
name = "rosetta-func"
version = "1.0.0"
description = "functions gate test"

[profiles.default.features]
rosetta_backend = true
function_postconditions = {postconditions}

[profiles.default.api]
generators = {generators}
"#
    )
}

async fn run_generator_pipeline(
    dir: &Path,
    model_text: &str,
    functions: bool,
    postconditions: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let model = dir.join("gen.rosetta");
    std::fs::write(&model, model_text)?;
    let domains = dir.join("domains.toml");
    std::fs::write(&domains, DOMAINS_TOML)?;
    let profiles = dir.join("profiles.toml");
    std::fs::write(&profiles, profiles_toml(functions, postconditions))?;
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

fn read_functions_rs(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join("generated/src/domain/store/functions.rs")).ok()
}

#[tokio::test]
async fn functions_generator_emits_golden_rust_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    run_generator_pipeline(dir.path(), GENERATOR_MODEL, true, false)
        .await
        .expect("pipeline with rosetta_backend + functions succeeds");

    let content = read_functions_rs(dir.path()).expect("functions.rs emitted");
    // Signatures: builtin number → f64, snake_case fn names.
    assert!(
        content.contains("pub fn order_score(order: OrderType) -> f64 {"),
        "{content}"
    );
    assert!(
        content.contains("pub fn value_for_status(order: OrderType, status: OrderStatus) -> f64 {"),
        "{content}"
    );
    // Transform annotation metadata.
    assert!(content.contains("/// Transform: [enrich]"), "{content}");
    // Alias as a let-binding with a transpiled `->` expression.
    assert!(content.contains("let base = order.total;"), "{content}");
    // Dispatch head → match with the own arm, the extending child arm,
    // and the default arm.
    assert!(content.contains("match status {"), "{content}");
    assert!(content.contains("OrderStatus::Draft => {"), "{content}");
    assert!(
        content.contains("OrderStatus::Shipped => shipped_value(order, status),"),
        "{content}"
    );
    assert!(
        content.contains(
            "_ => todo!(\"function 'ValueForStatus': unhandled OrderStatus dispatch value\"),"
        ),
        "{content}"
    );
    // Output accumulates through a default-initialized mutable local.
    assert!(
        content.contains("let mut result = f64::default();"),
        "{content}"
    );
    // Post-conditions are ABSENT while function_postconditions is off —
    // but the marker convention keeps the record for the untranspilable
    // Count op (no collection knowledge for function bodies).
    assert!(
        !content.contains("debug_assert!"),
        "post-conditions are config-gated: {content}"
    );
    assert!(
        content.contains(
            "// TODO(#263): transpile operation 'add result' (unsupported: Count: count requires a collection field)"
        ),
        "{content}"
    );
    // The child's own emitted fn keeps its alias + extends provenance.
    assert!(content.contains("let net = order.total;"), "{content}");
    assert!(
        content.contains("pub fn shipped_value(order: OrderType, status: OrderStatus) -> f64 {"),
        "{content}"
    );
}

#[tokio::test]
async fn post_conditions_emit_debug_assert_when_the_feature_is_on() {
    let dir = tempfile::tempdir().unwrap();
    run_generator_pipeline(dir.path(), GENERATOR_MODEL, true, true)
        .await
        .expect("pipeline with function_postconditions succeeds");

    let content = read_functions_rs(dir.path()).expect("functions.rs emitted");
    assert!(
        content.contains("debug_assert!(result.is_some(), \"ResultPresent\");"),
        "{content}"
    );
}

#[tokio::test]
async fn functions_generator_off_emits_nothing() {
    let dir = tempfile::tempdir().unwrap();
    run_generator_pipeline(dir.path(), GENERATOR_MODEL, false, false)
        .await
        .expect("pipeline without functions listed succeeds");
    assert!(
        read_functions_rs(dir.path()).is_none(),
        "functions.rs must not exist when the generator is not listed"
    );
}

#[tokio::test]
async fn functions_listed_while_rosetta_backend_off_is_a_configuration_error() {
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
name = "rosetta-func-misconfig"
version = "1.0.0"
description = "functions misconfiguration gate"

[profiles.default.features]
rosetta_backend = false

[profiles.default.api]
generators = ["dto", "functions"]
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
        "listing functions with rosetta_backend off must fail plan validation"
    );
    assert!(
        read_functions_rs(dir.path()).is_none(),
        "no output may be written when plan validation fails"
    );
}

// ── Equivalence of concerns: functions alter no other output ────────────

/// The generator model minus every func — the same data model the
/// equivalence run bridges.
const GENERATOR_MODEL_NO_FUNCS: &str = r#"
namespace gen.store

enum OrderStatus:
	Draft
	Shipped

type OrderType:
	total number (1..1)
	tags string (0..*)
"#;

#[tokio::test]
async fn functions_alter_no_output_beyond_their_own_module() {
    // Two runs with the SAME profiles (functions listed): one bridges the
    // model with functions, one without. Every generated file must be
    // byte-identical except the functions module itself — pinning that
    // function ingestion does not perturb classification, DDL, entities,
    // or any other generator's view of the graph.
    let dir_with = tempfile::tempdir().unwrap();
    let dir_without = tempfile::tempdir().unwrap();
    run_generator_pipeline(dir_with.path(), GENERATOR_MODEL, true, false)
        .await
        .expect("with-functions run");
    run_generator_pipeline(dir_without.path(), GENERATOR_MODEL_NO_FUNCS, true, false)
        .await
        .expect("without-functions run");

    let out_with = dir_with.path().join("generated");
    let out_without = dir_without.path().join("generated");
    assert!(
        read_functions_rs(dir_with.path()).is_some(),
        "the with-functions run emits the module"
    );
    assert!(
        read_functions_rs(dir_without.path()).is_none(),
        "a model without funcs emits no module"
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
            None => diff.push(format!("only in with-functions run: {rel}")),
            Some(other) => {
                if other != content {
                    diff.push(format!("content drift: {rel}"));
                }
            }
        }
    }
    for rel in without.keys() {
        if !with.contains_key(rel) {
            diff.push(format!("only in without-functions run: {rel}"));
        }
    }
    // The ONLY permitted differences: the functions module itself plus the
    // two mechanical registries that index emitted files — the domain
    // mod.rs (declares the new module) and the run manifest (inventories
    // it). Everything classification/DDL/entity-shaped must be identical.
    let permitted: std::collections::BTreeSet<String> = [
        "src/domain/store/functions.rs".to_string(),
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
        "functions must not alter any other generated file: {unexpected:?} (full diff: {diff:?})"
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
