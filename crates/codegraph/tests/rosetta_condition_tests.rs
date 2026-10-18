//! Constraint-plane tests (issue #261).
//!
//! The rosetta bridge lands ConditionNodes (named conditions + one derived
//! one_of per choice) linked to their schemas via HasCondition edges; the
//! JSON path parses array `minItems`/`maxItems` onto PropertyNode; and the
//! `condition_validations` generator (gated behind `rosetta_backend`)
//! emits per-domain item-count validations + #262 transpile markers.

use codegraph_backend::{BackendConfig, create_backend};
use codegraph_config::config::{DomainConfig, parse_domain_config};
use codegraph_core::types::{ConditionKind, PropertyNode};

// ── Bridge fixture ─────────────────────────────────────────────────────

const MODEL: &str = r#"
namespace cond.shop
version "1.0.0"

type Order:
	orderId string (1..1)
	lines LineItem (2..5)
	total number (1..1)

	condition PositiveTotal: total > 0
	condition: total <> 0.0

enum LineItem:
	BOOK
	GAME

choice OrderType:
	Order
	LineItem
"#;

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.shop]
label = "Shop"
schema_dir = "shop"
postgres_schema = "shop"
"#;

struct Bridge {
    backend: codegraph_backend::Backend,
    outcome: codegraph::ingest::rosetta_ingest::RosettaIngestOutcome,
    _dir: tempfile::TempDir,
}

async fn bridge_fixture() -> Bridge {
    let dir = tempfile::tempdir().unwrap();
    let model_path = dir.path().join("shop.rosetta");
    std::fs::write(&model_path, MODEL).unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(&domains_path, DOMAINS_TOML).unwrap();

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
    Bridge {
        backend,
        outcome,
        _dir: dir,
    }
}

// ── Condition nodes from the bridge ────────────────────────────────────

#[tokio::test]
async fn named_and_unnamed_conditions_land_as_condition_nodes() {
    let g = bridge_fixture().await;
    let conditions = g
        .backend
        .querier()
        .get_conditions_for_schema("Order")
        .await
        .unwrap();
    assert_eq!(
        conditions.len(),
        2,
        "named + unnamed conditions both land: {conditions:?}"
    );

    // The named condition carries the canonical Expr::to_json payload.
    let named = conditions
        .iter()
        .find(|c| c.name == "PositiveTotal")
        .expect("named condition");
    assert_eq!(named.kind, ConditionKind::Condition);
    assert_eq!(named.owner_title, "Order");
    assert_eq!(named.domain.as_deref(), Some("shop"));
    assert_eq!(named.definition, None);
    let payload: serde_json::Value = serde_json::from_str(
        named
            .expr_json
            .as_deref()
            .expect("named condition carries expr_json"),
    )
    .expect("expr_json round-trips as JSON");
    assert!(
        payload.get("kind").is_some(),
        "Expr::to_json is kind-tagged: {payload}"
    );

    // The unnamed condition synthesizes `<Type>_condition_<idx>` (it is the
    // second condition in the block).
    let unnamed = conditions
        .iter()
        .find(|c| c.name == "Order_condition_1")
        .expect("unnamed condition synthesizes Order_condition_1");
    assert_eq!(unnamed.kind, ConditionKind::Condition);
    assert!(unnamed.expr_json.is_some());
}

#[tokio::test]
async fn choice_derives_one_one_of_node_with_option_titles() {
    let g = bridge_fixture().await;
    let conditions = g
        .backend
        .querier()
        .get_conditions_for_schema("OrderType")
        .await
        .unwrap();
    assert_eq!(conditions.len(), 1, "exactly ONE one_of per choice");
    let one_of = &conditions[0];
    assert_eq!(one_of.kind, ConditionKind::OneOf);
    assert_eq!(one_of.name, "OrderType_one_of");
    assert_eq!(one_of.owner_title, "OrderType");
    assert_eq!(one_of.expr_json, None, "one_of is not a sigil Expr kind");
    assert_eq!(
        one_of.options,
        vec!["Order".to_string(), "LineItem".to_string()],
        "options are the option attributes' referenced titles"
    );
}

#[tokio::test]
async fn list_conditions_totals_the_constraint_plane() {
    let g = bridge_fixture().await;
    let all = g.backend.querier().list_conditions().await.unwrap();
    // 2 on Order (named + unnamed) + 1 derived one_of on OrderType.
    assert_eq!(all.len(), 3);
    assert_eq!(g.outcome.stats.conditions_recorded, 2);
    assert_eq!(g.outcome.stats.conditions_ingested, 2);
    assert_eq!(g.outcome.stats.one_of_ingested, 1);

    // Conditions stay scoped to their owners.
    let other = g
        .backend
        .querier()
        .get_conditions_for_schema("LineItem")
        .await
        .unwrap();
    assert!(other.is_empty(), "codelist schemas own no conditions");
}

#[tokio::test]
async fn rosetta_cardinality_bounds_land_on_property_nodes() {
    let g = bridge_fixture().await;
    let props = g.backend.querier().get_properties("Order").await.unwrap();
    let lines = props.iter().find(|p| p.name == "lines").unwrap();
    assert!(lines.is_array);
    assert_eq!(
        lines.min_items,
        Some(2),
        "rosetta (2..5) maps min 2 to min_items (#261 gap-analysis finding 2)"
    );
    assert_eq!(lines.max_items, Some(5));

    // Scalars keep None — their min is requiredness, not an item bound.
    let total = props.iter().find(|p| p.name == "total").unwrap();
    assert_eq!(total.min_items, None);
    assert_eq!(total.max_items, None);
}

// ── JSON minItems graph round-trip ─────────────────────────────────────

#[tokio::test]
async fn json_min_items_round_trips_through_the_graph() {
    let schema_json = r#"{
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": "CrewType",
        "type": "object",
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "members": {
                "type": "array",
                "items": { "type": "string" },
                "minItems": 2,
                "maxItems": 7
            }
        },
        "required": ["id", "members"]
    }"#;
    let dir = tempfile::tempdir().unwrap();
    let schemas = dir.path().join("schemas");
    std::fs::create_dir_all(&schemas).unwrap();
    std::fs::write(schemas.join("CrewType.json"), schema_json).unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(&domains_path, DOMAINS_TOML).unwrap();
    let classifier_path = dir.path().join("classifier.toml");
    std::fs::write(&classifier_path, "").unwrap();

    let backend = create_backend(&BackendConfig::default()).await.unwrap();
    let domain_config = parse_domain_config(&domains_path).unwrap();
    let classifier =
        codegraph_classifier::config::parse_classifier_config(&classifier_path).unwrap();
    codegraph::ingest::async_ingest::ingest_schemas(
        backend.ingestor(),
        &schemas,
        &classifier,
        &std::collections::HashSet::from(["CrewType".to_string()]),
        &codegraph_config::config::UiOverrideConfig::default(),
        &domain_config.defaults.type_suffix,
    )
    .await
    .unwrap();

    let props: Vec<PropertyNode> = backend.querier().get_properties("CrewType").await.unwrap();
    let members = props.iter().find(|p| p.name == "members").unwrap();
    assert_eq!(
        members.min_items,
        Some(2),
        "minItems: 2 must round-trip through the graph"
    );
    assert_eq!(members.max_items, Some(7));
}

// ── condition_validations generator (rosetta_backend gate) ─────────────

const GENERATOR_MODEL: &str = r#"
namespace cond.shop
version "1.0.0"

type Product:
	productId string (1..1)
	price number (1..1)
	tags string (2..5)

	condition PositivePrice: price > 0
	condition TaggedSale: tags contains "sale"
	condition CodedProduct: productId to-enum LineItem

type Holder:
	holderId string (1..1)

choice ProductType:
	Product
	Holder
"#;

fn generator_domains_toml() -> String {
    DOMAINS_TOML.to_string()
}

fn profiles_toml(rosetta_backend: bool) -> String {
    let feature = if rosetta_backend { "true" } else { "false" };
    let mut toml = format!(
        r#"
[profiles.default.meta]
name = "rosetta-cond"
version = "1.0.0"
description = "condition_validations gate test"

[profiles.default.features]
rosetta_backend = {feature}

[profiles.default.api]
generators = ["dto", "condition_validations"]
"#
    );
    if !rosetta_backend {
        // Without the feature the generator must not even be listed
        // (the capability would hard-error otherwise).
        toml = toml.replace("\"dto\", \"condition_validations\"", "\"dto\"");
    }
    toml
}

async fn run_generator_pipeline(
    dir: &std::path::Path,
    rosetta_backend: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let model = dir.join("shop.rosetta");
    std::fs::write(&model, GENERATOR_MODEL)?;
    let domains = dir.join("domains.toml");
    std::fs::write(&domains, generator_domains_toml())?;
    let profiles = dir.join("profiles.toml");
    std::fs::write(&profiles, profiles_toml(rosetta_backend))?;
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

fn read_validations(dir: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(dir.join("generated/src/domain/shop/validations.rs")).ok()
}

#[tokio::test]
async fn condition_validations_emits_item_checks_and_transpile_markers_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    run_generator_pipeline(dir.path(), true)
        .await
        .expect("pipeline with rosetta_backend succeeds");

    let content = read_validations(dir.path()).expect("validations.rs emitted");
    // Item-count checks from the (2..5) tags attribute.
    assert!(
        content.contains("pub fn validate_product_items(dto: &CreateProductRequest)"),
        "function with crate-local DTO import conventions:\n{content}"
    );
    assert!(
        content.contains("use crate::domain::shop::product::dto_create::CreateProductRequest;",)
    );
    assert!(content.contains("dto.tags.len() < 2"), "{content}");
    assert!(content.contains("dto.tags.len() > 5"), "{content}");
    // #262 slice 2: `tags contains "sale"` now transpiles into its own
    // function (tags is an array property → collection knowledge gates the
    // word binary through).
    assert!(
        content.contains("pub fn validate_tagged_sale(dto: &CreateProductRequest)"),
        "{content}"
    );
    assert!(
        content.contains("dto.tags.contains(&\"sale\")"),
        "{content}"
    );
    // ConditionNodes that cannot transpile keep the #262 marker with the
    // asserted #261 prefix, extended with the rejection reason — here the
    // to-enum cast (codelist knowledge is not in the transpiler context).
    assert!(
        content.contains(
            "// TODO(#262): transpile condition 'CodedProduct' from its Expr::to_json payload (unsupported: ToEnum:"
        ),
        "{content}"
    );
    assert!(
        content.contains(
            "// TODO(#262): transpile one_of 'ProductType_one_of' from its options [Product, Holder]"
        ),
        "{content}"
    );
}

#[tokio::test]
async fn condition_validations_transpiles_named_conditions_and_marks_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    run_generator_pipeline(dir.path(), true)
        .await
        .expect("pipeline with rosetta_backend succeeds");

    let content = read_validations(dir.path()).expect("validations.rs emitted");
    // `condition PositivePrice: price > 0` on a required number field → a
    // real validation function body (issue #262 slice 1).
    assert!(
        content.contains(
            "pub fn validate_positive_price(dto: &CreateProductRequest) -> Result<(), String> {"
        ),
        "{content}"
    );
    assert!(content.contains("    if !(dto.price > 0) {"), "{content}");
    assert!(
        content.contains("        return Err(\"PositivePrice failed\".to_string());"),
        "{content}"
    );
    // An UNTRANSPILABLE op (`to-enum` — codelist knowledge is not in the
    // transpiler context) keeps the TODO marker, still carrying the
    // `(unsupported: kind: detail)` suffix.
    assert!(
        content.contains("(unsupported: ToEnum: to-enum needs codelist knowledge"),
        "{content}"
    );
    // The slice-1 transpiled condition stays.
    assert!(
        content.contains("pub fn validate_positive_price(dto: &CreateProductRequest)"),
        "{content}"
    );
}

#[tokio::test]
async fn condition_validations_feature_off_emits_nothing() {
    let dir = tempfile::tempdir().unwrap();
    run_generator_pipeline(dir.path(), false)
        .await
        .expect("pipeline without the feature succeeds");

    assert!(
        read_validations(dir.path()).is_none(),
        "validations.rs must not exist when rosetta_backend is off"
    );
}
