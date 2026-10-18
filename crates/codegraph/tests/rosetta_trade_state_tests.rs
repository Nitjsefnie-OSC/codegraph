//! WP-A spike: can codegraph handle the REAL FINOS CDM TradeState pattern?
//!
//! The fixture under `tests/fixtures/rosetta_trade_state/` is authored from
//! the actual FINOS common-domain-model `rosetta-source` tree (master):
//! `TradeState`, `State`/`ClosedState`/`ClosedStateEnum`, `Reset`,
//! `Instruction`/`PrimitiveInstruction`, `BusinessEvent`, the instruction
//! types, and the real `Create_*` / `Filter*` / `Qualify_Novation` /
//! `ChangeCounterparty` functions, plus the verbatim `base-desc` regulatory
//! bodies (ICMA/GMRA). Where the real CDM uses a construct sigil at the
//! pinned rev rejects, the fragment documents the trim inline (every trim
//! carries a `TRIM:` note in the model text).
//!
//! These tests pin SPIKE EVIDENCE, not production behavior: the pipeline
//! accepts a real-CDM-shaped model end to end, the graph carries the
//! structured surfaces, and the generated artifacts show the G1..G5 gap
//! shapes described in `docs/trade-state-spike.md`.

use std::path::Path;
use std::path::PathBuf;

use codegraph_backend::{BackendConfig, create_backend};
use codegraph_config::config::{DomainConfig, parse_domain_config};
use codegraph_core::types::ConditionKind;

fn fixture_model() -> Vec<PathBuf> {
    let dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rosetta_trade_state/model");
    [
        "base-desc.rosetta",
        "base-datetime-type.rosetta",
        "base-staticdata-party-type.rosetta",
        "base-staticdata-identifier-type.rosetta",
        "product-template-type.rosetta",
        "event-common-enum.rosetta",
        "event-common-type.rosetta",
        "event-common-func.rosetta",
    ]
    .iter()
    .map(|f| dir.join(f))
    .collect()
}

fn fixture_domains() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rosetta_trade_state/domains.toml")
}

const PROFILES_TOML: &str = r#"
[profiles.spike.meta]
name = "trade-state-spike"
version = "1.0.0"
description = "WP-A spike: rosetta_backend + functions + condition_validations over the CDM TradeState fragment"

[profiles.spike.features]
rosetta_backend = true
function_postconditions = true

[profiles.spike.api]
generators = [
    "ddl", "sea_orm_entity", "codelist", "dto", "repository", "command",
    "query", "event", "handler", "workflow_action", "media_route", "test",
    "lifecycle_trait", "domain_types_dto", "domain_types_query_service",
    "errors", "router", "links",
    "openapi", "scaffold", "basejump_setup", "pgmq_setup",
    "platform_schema", "workflow_seed", "hook_registry", "domain_types_scaffold",
    "report_views",
    "webhook_dispatch", "webhook_endpoint_api",
    "functions", "condition_validations",
]
"#;

async fn bridge_fixture() -> codegraph_backend::Backend {
    let domain_config: DomainConfig = parse_domain_config(&fixture_domains()).unwrap();
    let backend = create_backend(&BackendConfig::default()).await.unwrap();
    codegraph::ingest::rosetta_ingest::ingest_rosetta_files(
        backend.ingestor(),
        backend.querier(),
        &fixture_model(),
        &domain_config,
        "Type",
    )
    .await
    .expect("the real-CDM TradeState fragment must ingest cleanly");
    backend
}

// ── 1. Full pipeline over ONLY the fixture ─────────────────────────────

#[tokio::test]
async fn full_pipeline_runs_clean_over_the_real_cdm_fragment() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("generated");
    let profiles = dir.path().join("profiles.toml");
    std::fs::write(&profiles, PROFILES_TOML).unwrap();

    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &fixture_domains(),
        output: &output,
        extension_points_path: None,
        profile_name: "spike",
        variant: None,
        profiles_config_path: Some(profiles),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &[],
        rosetta_files: &fixture_model(),
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
    .await
    .expect("full pipeline over the real-CDM fragment must succeed");

    let generated = output.clone();
    // Data plane: TradeState as an entity (DDL + SeaORM entity + handler).
    let has = |p: &str| generated.join(p).exists();
    assert!(
        has("src/entity/common_trade_state.rs"),
        "sea_orm entity for TradeState missing"
    );
    assert!(
        has("src/api/common/trade_state_handler.rs"),
        "API handler for TradeState missing"
    );
    assert!(
        has("src/domain/common/trade_state/repository.rs"),
        "repository for TradeState missing"
    );
    // Function plane + conditions plane (rosetta_backend gated).
    assert!(
        has("src/domain/common/functions.rs"),
        "functions.rs missing"
    );
    assert!(
        has("src/domain/common/validations.rs"),
        "condition validations missing"
    );
}

// ── 2. TradeState + ClosedStateEnum in the graph, with namespaces ──────

#[tokio::test]
async fn trade_state_lands_as_a_namespaced_schema_and_closed_state_as_a_codelist() {
    let backend = bridge_fixture().await;
    let schemas = backend.querier().list_schemas(None).await.unwrap();

    let trade_state = schemas
        .iter()
        .find(|s| s.title == "TradeState")
        .expect("TradeState schema node missing");
    assert_eq!(
        trade_state.namespace.as_deref(),
        Some("cdm.event.common"),
        "TradeState carries its real CDM namespace"
    );
    assert_eq!(
        trade_state.schema_id, "cdm.event.common/TradeState",
        "the rosetta bridge keeps the mox `package/Name` id convention (diverges from the JSON path's `<ns>::<Name>` — see the spike doc's gap ledger)"
    );
    assert_eq!(trade_state.domain.as_deref(), Some("common"));

    // The state lineage plane: State, ClosedState, and the enum behind it.
    for title in ["State", "ClosedState"] {
        let node = schemas
            .iter()
            .find(|s| s.title == title)
            .unwrap_or_else(|| panic!("{title} schema node missing"));
        assert_eq!(node.namespace.as_deref(), Some("cdm.event.common"));
    }

    let codelists = backend.querier().list_codelists().await.unwrap();
    let closed_state = codelists
        .iter()
        .find(|c| c.name == "ClosedStateEnum")
        .expect("ClosedStateEnum codelist missing");
    assert_eq!(
        closed_state.pg_table_name, "closed_state_enum",
        "pg table name is the bare title (the domain-schema prefix is applied at DDL emission)"
    );

    // Namespace plane (#268): the declared CDM namespaces, rosetta-sourced.
    let namespaces = backend.querier().list_namespaces().await.unwrap();
    let ns = |fqn: &str| {
        namespaces
            .iter()
            .find(|n| n.fqn == fqn)
            .unwrap_or_else(|| panic!("namespace {fqn} missing: {namespaces:?}"))
    };
    assert_eq!(ns("cdm.event.common").source.as_deref(), Some("rosetta"));
    assert_eq!(ns("cdm.base").source.as_deref(), Some("rosetta"));

    // Regulatory plane (#265): the verbatim base-desc bodies + the
    // Identifier type's real multi-line [docReference ICMA GMRA ...].
    let refs = backend
        .querier()
        .list_regulatory_references()
        .await
        .unwrap();
    assert!(
        refs.iter()
            .any(|r| r.target == "GMRA" || r.target == "ICMA"),
        "the real ICMA/GMRA docReference must resolve now the base-desc bodies are declared: {refs:?}"
    );
}

// ── 3. The real primitive functions land structured ────────────────────

#[tokio::test]
async fn real_cdm_functions_land_with_structured_io_and_ops() {
    let backend = bridge_fixture().await;
    let functions = backend.querier().list_functions().await.unwrap();
    let by_name = |name: &str| {
        functions
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| {
                panic!(
                    "{name} missing: {:?}",
                    functions.iter().map(|f| f.name.clone()).collect::<Vec<_>>()
                )
            })
    };

    // Create_Reset (verbatim real CDM): instruction + before:TradeState in,
    // TradeState out, `set reset: tradeState` + `add reset -> resetHistory`.
    let create_reset = by_name("Create_Reset");
    assert_eq!(create_reset.inputs.len(), 2);
    assert_eq!(create_reset.inputs[0].name, "instruction");
    assert_eq!(create_reset.inputs[0].type_ref, "ResetInstruction");
    assert_eq!(
        create_reset.inputs[1].name, "tradeState",
        "input names keep the source Rosetta casing; the transpiler snake_cases at emission"
    );
    assert_eq!(create_reset.inputs[1].type_ref, "TradeState");
    let out = create_reset.output.as_ref().expect("Create_Reset output");
    assert_eq!(
        (out.name.as_str(), out.type_ref.as_str()),
        ("reset", "TradeState")
    );
    assert_eq!(create_reset.operations.len(), 2);
    let set_op = &create_reset.operations[0];
    assert!(!set_op.is_add);
    assert_eq!(set_op.assign_root, "reset");
    assert!(
        set_op.path.is_empty(),
        "`set reset: tradeState` has no path"
    );
    let add_op = &create_reset.operations[1];
    assert!(add_op.is_add);
    assert_eq!(add_op.assign_root, "reset");
    assert_eq!(add_op.path, vec!["resetHistory"]);

    // Create_Exercise (the real fan-out): instruction + before inputs, a
    // 1..* TradeState output, and TWO add ops accumulating into it.
    let create_exercise = by_name("Create_Exercise");
    assert_eq!(
        create_exercise
            .inputs
            .iter()
            .map(|i| (i.name.as_str(), i.type_ref.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("exerciseInstruction", "ExerciseInstruction"),
            ("originalTrade", "TradeState"),
        ],
        "the real instruction+before input pattern (source casing preserved)"
    );
    let exercise_out = create_exercise
        .output
        .as_ref()
        .expect("Create_Exercise output");
    assert_eq!(exercise_out.type_ref, "TradeState");
    assert!(
        exercise_out.is_array,
        "the real output cardinality is (1..*)"
    );
    assert_eq!(
        create_exercise
            .operations
            .iter()
            .filter(|op| op.is_add && op.assign_root == "exercise")
            .count(),
        2,
        "the 1→N fan-out is expressed as two `add exercise` ops"
    );

    // Real CDM has NO dispatch heads in these funcs (no `(attr: Enum->Value)`
    // syntax anywhere in the fragment) and Qualify_Novation carries the
    // [qualification BusinessEvent] surface.
    let qualify = by_name("Qualify_Novation");
    assert!(qualify.dispatch.is_none());
    assert_eq!(qualify.inputs[0].type_ref, "BusinessEvent");
    assert!(
        functions.iter().all(|f| f.dispatch.is_none()),
        "the CDM fragment is dispatch-free — dispatch transpilation is unexercised by real CDM"
    );
}

// ── 4. Condition nodes + validations emission ──────────────────────────

#[tokio::test]
async fn fragment_conditions_land_as_nodes_and_emit_into_validations() {
    let backend = bridge_fixture().await;
    let conditions = backend.querier().list_conditions().await.unwrap();
    let named = |name: &str| {
        conditions
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("condition {name} missing: {conditions:?}"))
    };

    // Real CDM conditions survive with their canonical expr payloads.
    let averaging = named("AveragingMethodologyExists");
    assert_eq!(averaging.owner_title, "Reset");
    assert_eq!(averaging.kind, ConditionKind::Condition);
    assert!(
        averaging
            .expr_json
            .as_deref()
            .unwrap_or_default()
            .contains("AveragingCalculation")
            || averaging
                .expr_json
                .as_deref()
                .unwrap_or_default()
                .contains("averagingMethodology"),
        "the payload names the guarded attribute: {:?}",
        averaging.expr_json
    );
    assert_eq!(named("NewTrade").owner_title, "Instruction");
    assert_eq!(named("ClosedStateExists").owner_title, "State");
    assert_eq!(named("IsOptionPayout").owner_title, "ExerciseInstruction");

    // Choice types derive one_of nodes over their option titles.
    let transfer_one_of = named("Transfer_one_of");
    assert_eq!(transfer_one_of.kind, ConditionKind::OneOf);
    assert_eq!(
        transfer_one_of.options,
        vec![
            "ScheduledTransfer",
            "UnscheduledTransfer",
            "ContingentTransfer"
        ]
    );

    // Full pipeline: the condition_validations generator emits the
    // transpilable ones and TODO-marks the rest (#262).
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("generated");
    let profiles = dir.path().join("profiles.toml");
    std::fs::write(&profiles, PROFILES_TOML).unwrap();
    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &fixture_domains(),
        output: &output,
        extension_points_path: None,
        profile_name: "spike",
        variant: None,
        profiles_config_path: Some(profiles),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &[],
        rosetta_files: &fixture_model(),
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
    .await
    .expect("pipeline run for validations emission");

    let validations = std::fs::read_to_string(output.join("src/domain/common/validations.rs"))
        .expect("common/validations.rs emitted");
    assert!(
        validations.contains("pub fn validate_averaging_methodology_exists("),
        "the real Reset condition transpiles: {validations}"
    );
    assert!(
        validations.contains("pub fn validate_event_date("),
        "the real BusinessEvent condition transpiles"
    );
    assert!(
        validations.contains("Trade.counterparty allows at least 2"),
        "the real (2..2) counterparty cardinality becomes an item-count validation"
    );
    // Post-#283 guarded-optional lowering: 7 of the 9 real conditions
    // emit real checks.
    assert!(
        validations.contains("pub fn validate_new_trade("),
        "chain-exists condition (Instruction.NewTrade) transpiles post-#283"
    );
    assert!(
        validations.contains(
            "dto.primitive_instruction.as_ref().map(|v| v.execution.is_some()).unwrap_or(false)"
        ),
        "exists through an optional (choice) receiver uses the canonical map shape"
    );
    assert!(
        validations.contains("pub fn validate_closed_state_exists(")
            && validations
                .contains("dto.position_state.as_ref() == Some(&PositionStatusEnum::Closed)"),
        "optional enum equality lowers to as_ref() == Some(&Variant)"
    );
    assert!(
        validations.contains("pub fn validate_corporate_action(")
            && validations.contains(
                "dto.intent.as_ref() == Some(&EventIntentEnum::CorporateActionAdjustment)"
            ),
        "required-path enum equality with a qualified literal lowers"
    );
    assert!(
        validations.contains("pub fn validate_exclusive_split_primitive("),
        "OnlyExists over a chain lowers through the same rule"
    );
    assert!(
        std::fs::read_to_string(output.join("src/domain/datetime/validations.rs"))
            .expect("datetime/validations.rs")
            .contains("pub fn validate_adjusted_date("),
        "absent/exists family transpiles (AdjustedDate)"
    );
    // Honest refusals keep precise markers: the Reference-guard switch
    // (optional choice argument) and the root Choice operation.
    assert!(
        validations.contains("TODO(#262): transpile condition 'IsOptionPayout'")
            && validations.contains("exercise_option"),
        "switch over an optional choice field stays TODO'd, not silently dropped"
    );
    assert!(
        std::fs::read_to_string(output.join("src/domain/identifier/validations.rs"))
            .expect("identifier/validations.rs")
            .contains("TODO(#262): transpile condition 'IssuerChoice'"),
        "root Choice operations remain the documented stub"
    );
}

// ── 5. Generated functions module: copy-from-before, no dispatch ───────

#[tokio::test]
async fn generated_functions_show_copy_from_before_and_no_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("generated");
    let profiles = dir.path().join("profiles.toml");
    std::fs::write(&profiles, PROFILES_TOML).unwrap();
    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &fixture_domains(),
        output: &output,
        extension_points_path: None,
        profile_name: "spike",
        variant: None,
        profiles_config_path: Some(profiles),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &[],
        rosetta_files: &fixture_model(),
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
    .await
    .expect("pipeline run for functions emission");

    let content = std::fs::read_to_string(output.join("src/domain/common/functions.rs"))
        .expect("common/functions.rs emitted");

    // The real Create_Reset emits copy-from-before + lineage append.
    assert!(
        content.contains("pub fn create_reset(instruction: ResetInstruction, trade_state: TradeState) -> TradeState {"),
        "signature: {content}"
    );
    assert!(
        content.contains("reset = trade_state;"),
        "`set reset: tradeState` → copy-from-before assignment"
    );
    // Post-#284 slice B: the copy-from-before op SEEDS the output local
    // instead of assigning over `Default::default()` — value semantics,
    // no silently-dropped "before" fields.
    assert!(
        content.contains("let mut reset = trade_state;"),
        "wholesale copy-from-before seeds the output local (issue #284): {content}"
    );
    assert!(
        content.contains("reset.reset_history.push(instruction.reset);"),
        "`add reset -> resetHistory` → push into the lineage list"
    );

    // The fan-out signature: instruction + owned-by-value before state.
    assert!(
        content.contains(
            "pub fn create_exercise(exercise_instruction: ExerciseInstruction, original_trade: TradeState)"
        ),
        "Create_Exercise keeps the real instruction+before owned inputs"
    );

    // Real CDM funcs are dispatch-free: no `match` transpilation appears.
    assert!(
        !content.contains("match "),
        "a dispatch-free fragment must not produce dispatch arms"
    );

    // Real-world composition degrades with explicit TODO markers (the
    // gap-ledger evidence, not silent guesses).
    assert!(
        content.contains("TODO(#263)"),
        "untranspilable real constructs carry TODO markers"
    );
}

// ── 6. Operations config: the G1 mitigation surface ────────────────────

#[tokio::test]
async fn trade_state_operations_config_yields_no_update_or_delete_surface() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("generated");
    let profiles = dir.path().join("profiles.toml");
    std::fs::write(&profiles, PROFILES_TOML).unwrap();
    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &fixture_domains(),
        output: &output,
        extension_points_path: None,
        profile_name: "spike",
        variant: None,
        profiles_config_path: Some(profiles),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &[],
        rosetta_files: &fixture_model(),
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
    .await
    .expect("pipeline run for operations evidence");

    let generated = output.clone();
    let router =
        std::fs::read_to_string(generated.join("src/api/common/router.rs")).expect("router");
    let routes = router
        .split("fn trade_state_routes()")
        .nth(1)
        .expect("trade_state_routes present")
        .split("\nfn ")
        .next()
        .unwrap()
        .to_string();
    assert!(routes.contains("trade_state_handler::list"));
    assert!(routes.contains("trade_state_handler::create"));
    assert!(routes.contains("trade_state_handler::get_by_id"));
    assert!(
        !routes.contains("trade_state_handler::update"),
        "operations=[create,read,list] must emit no update route: {routes}"
    );
    assert!(
        !routes.contains("trade_state_handler::delete"),
        "operations=[create,read,list] must emit no delete route: {routes}"
    );

    // The repository contract is equally narrow.
    let repo =
        std::fs::read_to_string(generated.join("src/domain/common/trade_state/repository.rs"))
            .expect("repository");
    assert!(repo.contains("async fn create("));
    assert!(repo.contains("async fn find_by_id("));
    assert!(repo.contains("async fn list("));
    assert!(
        !repo.contains("async fn update("),
        "no update in the repo contract"
    );
    assert!(
        !repo.contains("async fn delete("),
        "no delete in the repo contract"
    );

    // Post-#284 the DDL matches the narrowed API surface: operations
    // excluding update/delete infer append-only persistence (issue #284),
    // closing the G1 gap the config-only mitigation could not.
    let mut ddl = String::new();
    for entry in std::fs::read_dir(generated.join("migrations")).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| {
                n.ends_with("_common_trade_state.sql")
                    && !n.contains("rls")
                    && !n.contains("trigger")
            })
            .unwrap_or(false)
        {
            ddl = std::fs::read_to_string(path).unwrap();
            break;
        }
    }
    assert!(
        ddl.contains("GRANT SELECT, INSERT ON TABLE common.trade_state"),
        "grants narrowed to the append-only surface (issue #284): {ddl}"
    );
    assert!(
        !ddl.contains("GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE common.trade_state"),
        "UPDATE/DELETE grants must be gone once the API surface omits them"
    );
    assert!(
        !ddl.contains("updated_at"),
        "append-only tables have no updated_at column (issue #284): {ddl}"
    );
}
