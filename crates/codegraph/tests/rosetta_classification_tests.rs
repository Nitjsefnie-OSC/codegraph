//! Rosetta classification interplay tests (issue #258).
//!
//! The #254 decision: Rosetta types are AUTO-SCORED by the AutoClassifier
//! like JSON schemas — provenance is NOT authoritative. These tests pin:
//! - rosetta scoring parity with the JSON path on an equivalent model;
//! - `force_entities` / `force_value_objects` / legacy `entities` lists in
//!   domains.toml are honored for rosetta titles (observable end-to-end in
//!   DDL: FK columns are only emitted to entity targets, ddl.rs);
//! - choices (all-(0..1) attributes) score as value objects;
//! - `SchemaClassificationData.source` stays unset for rosetta nodes — the
//!   mox bypass (`override:source=mox`) never fires on them.

use std::path::PathBuf;

use codegraph_backend::{BackendConfig, create_backend};
use codegraph_config::config::parse_domain_config;

const MODEL: &str = r#"
namespace pipeline.store
version "1.0.0"

type Product: <"A product">
	productId string (1..1)
	label string (0..1)
	price number (1..1)
	currency Currency (1..1)
	holder Holder (0..1)

enum Currency:
	EUR
	USD

type Holder:
	holderId string (1..1)

type Note: <"A free-form note nobody references">
	noteId string (1..1)

choice ListingType:
	Product
	Holder
"#;

/// The same data model authored as JSON schemas (no `entities` declared —
/// scoring must partition identically to the rosetta run).
const PRODUCT_JSON: &str = r#"
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Product",
  "description": "A product",
  "type": "object",
  "properties": {
    "productId": { "type": "string" },
    "label": { "type": "string" },
    "price": { "type": "number" },
    "currency": { "$ref": "Currency.json" },
    "holder": { "$ref": "Holder.json" }
  },
  "required": ["productId", "price", "currency"]
}
"#;

const HOLDER_JSON: &str = r#"
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Holder",
  "description": "A holder",
  "type": "object",
  "properties": {
    "holderId": { "type": "string" }
  },
  "required": ["holderId"]
}
"#;

const NOTE_JSON: &str = r#"
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Note",
  "description": "A free-form note nobody references",
  "type": "object",
  "properties": {
    "noteId": { "type": "string" }
  },
  "required": ["noteId"]
}
"#;

const CURRENCY_JSON: &str = r#"
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Currency",
  "description": "ISO currencies",
  "type": "string",
  "enum": ["EUR", "USD"]
}
"#;

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.store]
label = "Store"
schema_dir = "store"
postgres_schema = "store"
"#;

fn write_model(dir: &std::path::Path) -> (PathBuf, PathBuf) {
    let model = dir.join("store.rosetta");
    std::fs::write(&model, MODEL).unwrap();
    let domains = dir.join("domains.toml");
    std::fs::write(&domains, DOMAINS_TOML).unwrap();
    (domains, model)
}

/// Score a rosetta model exactly the way the driver does: ingest through
/// the bridge, pull classification data from the graph, run the
/// AutoClassifier per domain. Returns (entity titles, VO titles).
async fn score_rosetta(extra_domain_lines: &str) -> (Vec<String>, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let (domains, model) = write_model(dir.path());
    if !extra_domain_lines.is_empty() {
        let mut toml = std::fs::read_to_string(&domains).unwrap();
        toml.push_str(extra_domain_lines);
        std::fs::write(&domains, toml).unwrap();
    }

    let be = create_backend(&BackendConfig::default()).await.unwrap();
    let domain_config = parse_domain_config(&domains).unwrap();
    codegraph::ingest::rosetta_ingest::ingest_rosetta_files(
        be.ingestor(),
        be.querier(),
        &[model],
        &domain_config,
        &domain_config.defaults.type_suffix,
    )
    .await
    .unwrap();

    let classifier_config = codegraph_classifier::config::parse_classifier_config_str("").unwrap();
    let classifier_types: std::collections::HashSet<String> = classifier_config
        .primitive_wrappers
        .keys()
        .cloned()
        .chain(classifier_config.array_wrappers.keys().cloned())
        .chain(classifier_config.range_wrappers.keys().cloned())
        .chain(
            classifier_config
                .composite_wrappers
                .iter()
                .map(|cw| cw.schema.clone()),
        )
        .collect();
    let all_data = be.querier().get_classification_data().await.unwrap();
    let auto_classifier = codegraph::classify::AutoClassifier::new(
        classifier_types,
        classifier_config.naming_rules.clone(),
    );
    let entry = &domain_config.domains["store"];
    let domain_schemas: Vec<_> = all_data
        .iter()
        .filter(|d| d.domain.as_deref() == Some("store"))
        .cloned()
        .collect();
    let result = auto_classifier.classify_domain("store", entry, &domain_schemas);
    (
        result.entities.iter().map(|s| s.title.clone()).collect(),
        result
            .value_objects
            .iter()
            .map(|s| s.title.clone())
            .collect(),
    )
}

#[tokio::test]
async fn rosetta_scores_partition_types_and_choices() {
    let (entities, value_objects) = score_rosetta("").await;
    // Nothing in this small model carries enough signal to be promoted by
    // scoring alone — same as the JSON path with no entities declared.
    // The choice must be a VO (all-(0..1) attributes).
    assert!(
        value_objects.contains(&"ListingType".to_string()),
        "choice should score VO; entities={entities:?} vos={value_objects:?}"
    );
    assert!(
        !entities.contains(&"ListingType".to_string()),
        "choice must not score as entity"
    );
    // The JSON-path parity test below pins the rest of the partition.
}

#[tokio::test]
async fn rosetta_scoring_matches_the_json_path_on_an_equivalent_model() {
    let (rosetta_entities, rosetta_vos) = score_rosetta("").await;

    // Same model authored as JSON schemas. Layout matches production:
    // <schemas-root>/<domain>/json/*.json with the config at the root.
    let dir = tempfile::tempdir().unwrap();
    let schemas_root = dir.path().join("schemas");
    let json_dir = schemas_root.join("store/json");
    std::fs::create_dir_all(json_dir.join("codelist")).unwrap();
    std::fs::write(schemas_root.join("domains.toml"), DOMAINS_TOML).unwrap();
    std::fs::write(
        schemas_root.join("classifier.toml"),
        "inline_enum_threshold = 20\n",
    )
    .unwrap();
    std::fs::write(json_dir.join("Product.json"), PRODUCT_JSON).unwrap();
    std::fs::write(json_dir.join("Holder.json"), HOLDER_JSON).unwrap();
    std::fs::write(json_dir.join("Note.json"), NOTE_JSON).unwrap();
    std::fs::write(json_dir.join("codelist/Currency.json"), CURRENCY_JSON).unwrap();

    let be = create_backend(&BackendConfig::default()).await.unwrap();
    let domain_config = parse_domain_config(&schemas_root.join("domains.toml")).unwrap();
    let classifier = codegraph_classifier::config::parse_classifier_config(
        &schemas_root.join("classifier.toml"),
    )
    .unwrap();
    codegraph::ingest::async_ingest::ingest_schemas(
        be.ingestor(),
        &schemas_root,
        &classifier,
        &std::collections::HashSet::new(),
        &codegraph_config::config::UiOverrideConfig::default(),
        &domain_config.defaults.type_suffix,
    )
    .await
    .unwrap();

    let classifier_types: std::collections::HashSet<String> = classifier
        .primitive_wrappers
        .keys()
        .cloned()
        .chain(classifier.array_wrappers.keys().cloned())
        .chain(classifier.range_wrappers.keys().cloned())
        .chain(
            classifier
                .composite_wrappers
                .iter()
                .map(|cw| cw.schema.clone()),
        )
        .collect();
    let all_data = be.querier().get_classification_data().await.unwrap();
    let auto =
        codegraph::classify::AutoClassifier::new(classifier_types, classifier.naming_rules.clone());
    let entry = &domain_config.domains["store"];
    let domain_schemas: Vec<_> = all_data
        .iter()
        .filter(|d| d.domain.as_deref() == Some("store"))
        .cloned()
        .collect();
    let json = auto.classify_domain("store", entry, &domain_schemas);
    let json_entities: Vec<String> = json.entities.iter().map(|s| s.title.clone()).collect();
    let json_vos: Vec<String> = json.value_objects.iter().map(|s| s.title.clone()).collect();

    // The partitions agree, restricted to the kinds both paths express
    // (ListingType is rosetta-choice-specific; JSON's oneOf representation
    // differs by disposition).
    let shared = ["Product", "Holder", "Note", "Currency"];
    for title in shared {
        let title = title.to_string();
        assert_eq!(
            rosetta_entities.contains(&title),
            json_entities.contains(&title),
            "{title} entity decision diverges: rosetta {:?} vs json {:?}",
            rosetta_entities,
            json_entities
        );
        assert_eq!(
            rosetta_vos.contains(&title),
            json_vos.contains(&title),
            "{title} VO decision diverges: rosetta {:?} vs json {:?}",
            rosetta_vos,
            json_vos
        );
    }
}

#[tokio::test]
async fn legacy_entities_list_flips_rosetta_titles_end_to_end() {
    // Entity-gated observables, end to end through the driver:
    // - auto-scored VO target: Product's DDL has NO holder column/FK;
    // - legacy `entities = ["Holder"]`: Holder is flipped to entity (the
    //   driver merges the list into entity_names, reclassify_with_entities
    //   processes rosetta nodes) AND the FK is retained (generated_table_set
    //   keys off the same list, ddl.rs).
    let dir = tempfile::tempdir().unwrap();
    let (domains, model) = write_model(dir.path());
    let output = dir.path().join("generated");
    let rosetta = vec![model];
    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &domains,
        output: &output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(PathBuf::from("definitely-absent-profiles.toml")),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &[],
        rosetta_files: &rosetta,
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
    .unwrap();

    let read = |root: &std::path::Path, suffix: &str| {
        for entry in walkdir::WalkDir::new(root) {
            let entry = entry.unwrap();
            if entry.file_type().is_file() {
                let rel = entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/");
                if rel.starts_with("migrations/")
                    && rel.ends_with(suffix)
                    && !rel.ends_with("_rls.sql")
                    && !rel.ends_with("_trigger.sql")
                    && !rel.ends_with("_fts.sql")
                {
                    return std::fs::read_to_string(entry.path()).unwrap();
                }
            }
        }
        panic!("no migration matching {suffix}");
    };

    let baseline = read(&output, "_store_product.sql");
    assert!(
        !baseline.contains("REFERENCES store.holder"),
        "auto-scored VO target must not get an FK to its table"
    );
    // VO targets compose as an inline child table instead (WP1.7): Holder's
    // own field lands in the child table, not as an FK column on product.
    assert!(
        baseline.contains("product_holder"),
        "auto-scored VO target should compose as a child table"
    );

    // Legacy entities list flips Holder to entity, end to end.
    let dir2 = tempfile::tempdir().unwrap();
    let (domains2, model2) = write_model(dir2.path());
    let mut toml = std::fs::read_to_string(&domains2).unwrap();
    toml.push_str("\nentities = [\"Holder\"]\n");
    std::fs::write(&domains2, toml).unwrap();
    let output2 = dir2.path().join("generated");
    let rosetta2 = vec![model2];
    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &domains2,
        output: &output2,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(PathBuf::from("definitely-absent-profiles.toml")),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &[],
        rosetta_files: &rosetta2,
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
    .unwrap();
    let forced = read(&output2, "_store_product.sql");
    assert!(
        forced.contains("holder_id UUID"),
        "forced entity target must get the FK column"
    );
    assert!(
        forced.contains("REFERENCES store.holder"),
        "forced entity target must get the FK constraint"
    );
}

#[tokio::test]
async fn force_entities_and_force_value_objects_flip_rosetta_scores() {
    // force_entities: Note scores VO by default (in_degree 0, 1 field) and
    // must be promoted by the override.
    let (entities, vos) = score_rosetta("\nforce_entities = [\"Note\"]\n").await;
    assert!(
        entities.contains(&"Note".to_string()),
        "force_entities must promote Note; entities={entities:?} vos={vos:?}"
    );

    // force_value_objects beats scoring: Holder earns +1 entity from
    // in_degree=1, but the override pins it VO.
    let (entities, vos) = score_rosetta("\nforce_value_objects = [\"Holder\"]\n").await;
    assert!(
        !entities.contains(&"Holder".to_string()),
        "force_value_objects must demote Holder; entities={entities:?}"
    );
    assert!(vos.contains(&"Holder".to_string()));
}

#[test]
fn rosetta_never_trips_the_mox_scoring_bypass() {
    // The bypass keys on SchemaClassificationData.source == "mox". The
    // bridge sets origin=rosetta and leaves source unset — assert on the
    // AutoClassifier directly: a rosetta-shaped record must score through
    // the normal path (no override reason), while a mox-shaped one bypasses.
    use codegraph_core::types::{MOX_SOURCE, SchemaClassificationData};
    use std::collections::{HashMap, HashSet};

    let classifier = codegraph::classify::AutoClassifier::new(HashSet::new(), HashMap::new());

    // Real DomainEntry via the parser (no literal construction churn).
    let dir = tempfile::tempdir().unwrap();
    let domains = dir.path().join("domains.toml");
    std::fs::write(&domains, DOMAINS_TOML).unwrap();
    let config = codegraph_config::config::parse_domain_config(&domains).unwrap();
    let entry = &config.domains["store"];

    let rosetta_data = SchemaClassificationData {
        namespace: None,
        title: "Product".to_string(),
        domain: Some("store".to_string()),
        rel_path: "rosetta.bridge/Product".to_string(),
        schema_type: "object".to_string(),
        is_codelist: false,
        is_primitive_wrapper: false,
        has_all_of: false,
        composes_noun_type: false,
        field_count: 4,
        required_field_count: 2,
        ref_count: 2,
        in_degree: 2,
        is_enum: false,
        is_string_type: false,
        is_entity: false,
        source: None,
    };
    let result = classifier.classify_domain("store", entry, std::slice::from_ref(&rosetta_data));
    let scored: Vec<_> = result
        .entities
        .iter()
        .chain(result.value_objects.iter())
        .collect();
    let product = scored
        .iter()
        .find(|s| s.title == "Product")
        .expect("rosetta schema must be scored");
    assert!(
        !product
            .reasons
            .iter()
            .any(|r| r.contains("override:source=mox")),
        "rosetta nodes must go through scoring, not the mox bypass"
    );

    // Contrast: the same record with source=mox IS bypassed.
    let mox_data = SchemaClassificationData {
        source: Some(MOX_SOURCE.to_string()),
        ..rosetta_data
    };
    let result = classifier.classify_domain("store", entry, &[mox_data]);
    let scored: Vec<_> = result
        .entities
        .iter()
        .chain(result.value_objects.iter())
        .collect();
    let product = scored
        .iter()
        .find(|s| s.title == "Product")
        .expect("mox schema must be scored");
    assert!(
        product
            .reasons
            .iter()
            .any(|r| r.contains("override:source=mox")),
        "the mox bypass must keep working for mox-sourced nodes"
    );
}
