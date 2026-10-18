//! Events-plane ingest tests (issue #454 Phase 2a): the rex-ir to
//! `EvtModelGraph` conversion (declaration order, every payload, dense
//! per-kind ordinals), payload field-type title resolution (primitives
//! silent, exact, +type_suffix, miss counted), the end-to-end
//! `ingest_evt_files` run over a fixture dir (mox import, `import schema`,
//! and `import sigil` with the sibling candidate pool), hard-error
//! semantics, and the graph-cache inputs group.

use std::path::{Path, PathBuf};

use codegraph_core::mock::MockEngine;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::{
    EvtChannelNode, EvtEventField, EvtEventNode, EvtSubscriptionNode, SchemaNode,
};
use rex_ir::events::{ChannelDef, EventDef, EventField, EventModel, SubscriptionDef};
use rex_ir::{PrimitiveType, TypeRef};

// ── conversion ─────────────────────────────────────────────────────────

/// Two events (one version-pinned, one not) with primitive, enum, and
/// class-typed payload fields; one channel publishing both; two
/// subscriptions with different consumers.
fn sample_model() -> EventModel {
    let class = |package: &str, name: &str| TypeRef::Class {
        package: package.to_string(),
        name: name.to_string(),
    };
    let enum_ = |package: &str, name: &str| TypeRef::Enum {
        package: package.to_string(),
        name: name.to_string(),
    };
    EventModel::new()
        .event(
            EventDef::new("OrderPlaced")
                .version("1.0.0")
                .field(EventField::new(
                    "orderId",
                    TypeRef::Primitive(PrimitiveType::String),
                ))
                .field(EventField::new(
                    "status",
                    enum_("nz.example.orders", "OrderStatus"),
                )),
        )
        .event(EventDef::new("OrderCancelled").field(EventField::new(
            "order",
            class("nz.example.orders", "Order"),
        )))
        .channel(
            ChannelDef::new("orders")
                .publishes("OrderPlaced")
                .publishes("OrderCancelled"),
        )
        .subscription(
            SubscriptionDef::new("billing")
                .event("OrderPlaced")
                .event("OrderCancelled")
                .consumer("billing_service"),
        )
        .subscription(
            SubscriptionDef::new("analytics")
                .event("OrderPlaced")
                .consumer("analytics_service"),
        )
}

/// `OrderStatus` resolves exactly, `Order` via the +`Type` suffix;
/// everything else misses.
fn sample_resolver() -> impl Fn(&TypeRef) -> Option<String> {
    |ty: &TypeRef| match ty {
        TypeRef::Primitive(_) => None,
        _ => {
            let name = ty.qualified_name().unwrap();
            let short = name
                .rsplit("::")
                .next()
                .unwrap()
                .rsplit('.')
                .next()
                .unwrap();
            match short {
                "OrderStatus" => Some("OrderStatus".to_string()),
                "Order" => Some("OrderType".to_string()),
                _ => None,
            }
        }
    }
}

fn type_json(value: &TypeRef) -> serde_json::Value {
    serde_json::to_value(value).unwrap()
}

fn event(
    source_path: &str,
    name: &str,
    version: Option<&str>,
    fields: Vec<(&str, &TypeRef, Option<&str>)>,
    ordinal: usize,
) -> EvtEventNode {
    EvtEventNode {
        source_path: source_path.to_string(),
        name: name.to_string(),
        version: version.map(str::to_string),
        fields: fields
            .into_iter()
            .map(|(name, ty, resolved)| EvtEventField {
                name: name.to_string(),
                type_json: type_json(ty),
                resolved_title: resolved.map(str::to_string),
            })
            .collect(),
        ordinal,
    }
}

#[test]
fn conversion_maps_declaration_order_and_every_payload() {
    let model = sample_model();
    let graph = codegraph::ingest::evt_ingest::evt_model_graph_from_rex(
        &model,
        "orders.evt",
        &sample_resolver(),
    );

    assert_eq!(graph.source_path, "orders.evt");

    // Events in declaration order: version pinned verbatim (present and
    // absent), fields in order with their TypeRef JSON and resolved
    // titles, ordinals dense from 0.
    let status = TypeRef::Enum {
        package: "nz.example.orders".to_string(),
        name: "OrderStatus".to_string(),
    };
    let order = TypeRef::Class {
        package: "nz.example.orders".to_string(),
        name: "Order".to_string(),
    };
    assert_eq!(
        graph.events,
        vec![
            event(
                "orders.evt",
                "OrderPlaced",
                Some("1.0.0"),
                vec![
                    ("orderId", &TypeRef::Primitive(PrimitiveType::String), None,),
                    ("status", &status, Some("OrderStatus")),
                ],
                0,
            ),
            event(
                "orders.evt",
                "OrderCancelled",
                None,
                vec![("order", &order, Some("OrderType"))],
                1,
            ),
        ]
    );

    // Channels: publishes entries verbatim in declaration order.
    assert_eq!(
        graph.channels,
        vec![EvtChannelNode {
            source_path: "orders.evt".to_string(),
            name: "orders".to_string(),
            publishes: vec!["OrderPlaced".to_string(), "OrderCancelled".to_string()],
            ordinal: 0,
        }]
    );

    // Subscriptions: event lists and consumer verbatim.
    assert_eq!(
        graph.subscriptions,
        vec![
            EvtSubscriptionNode {
                source_path: "orders.evt".to_string(),
                name: "billing".to_string(),
                events: vec!["OrderPlaced".to_string(), "OrderCancelled".to_string()],
                consumer: "billing_service".to_string(),
                ordinal: 0,
            },
            EvtSubscriptionNode {
                source_path: "orders.evt".to_string(),
                name: "analytics".to_string(),
                events: vec!["OrderPlaced".to_string()],
                consumer: "analytics_service".to_string(),
                ordinal: 1,
            },
        ]
    );
}

// ── title resolution ───────────────────────────────────────────────────

#[test]
fn resolve_type_title_hits_exact_then_suffix_and_leaves_primitives_silent() {
    let titles: std::collections::HashSet<String> = ["OrderStatus", "WidgetType"]
        .into_iter()
        .map(str::to_string)
        .collect();
    let resolve =
        |ty: &TypeRef| codegraph::ingest::evt_ingest::resolve_type_title(ty, "Type", &titles);

    // Primitives resolve to None SILENTLY — the same None as a miss at
    // this layer; only the ingest caller separates them (by variant), so
    // a primitive payload never warns and never counts.
    assert_eq!(
        resolve(&TypeRef::Primitive(PrimitiveType::String)),
        None,
        "primitive → None (no title)"
    );
    // Exact hit, through both qualifier styles.
    assert_eq!(
        resolve(&TypeRef::Enum {
            package: "nz.example.orders".to_string(),
            name: "OrderStatus".to_string(),
        }),
        Some("OrderStatus".to_string())
    );
    assert_eq!(
        resolve(&TypeRef::Class {
            package: "shop.catalog".to_string(),
            name: "Widget".to_string(),
        }),
        Some("WidgetType".to_string()),
        "+type_suffix hit"
    );
    // Miss.
    assert_eq!(
        resolve(&TypeRef::Class {
            package: "shop".to_string(),
            name: "Ghost".to_string(),
        }),
        None
    );
}

// ── end-to-end ingest ──────────────────────────────────────────────────

const LIB_MOX: &str = concat!(
    "package shop\n",
    "\n",
    "import schema \"x.json\" as Widget\n",
    "import sigil \"y.rosetta\"\n",
    "\n",
    "class Product {\n",
    "    String title\n",
    "}\n",
    "\n",
    "enum Kind {\n",
    "    A as \"a\" = 0\n",
    "}\n",
    "\n",
    "class Ghost {\n",
    "    String name\n",
    "}\n",
);

/// Imports a namespace that only exists in the SIBLING pool file
/// (`z.rosetta`, never named by any import): a clean compile proves the
/// candidate-pool discovery ran.
const Y_ROSETTA: &str = concat!(
    "namespace shop.notes\n",
    "version \"1.0.0\"\n",
    "\n",
    "import shop.other.*\n",
    "\n",
    "type Note:\n",
    "    label string (1..1)\n",
    "    tag shop.other.Tag (1..1)\n",
);

const Z_ROSETTA: &str = concat!(
    "namespace shop.other\n",
    "version \"1.0.0\"\n",
    "\n",
    "type Tag:\n",
    "    name string (1..1)\n",
);

const X_JSON: &str = r#"{"title": "Widget"}"#;

const ORDERS_EVT: &str = concat!(
    "import \"lib.mox\"\n",
    "\n",
    "event ProductShipped version \"2.0.0\" {\n",
    "    label: String;\n",
    "    product: Product;\n",
    "    kind: Kind;\n",
    "    ghost: Ghost;\n",
    "}\n",
    "\n",
    "event ProductAudited {\n",
    "    product: Product;\n",
    "}\n",
    "\n",
    "channel fulfilment {\n",
    "    publishes ProductShipped;\n",
    "    publishes ProductAudited;\n",
    "}\n",
    "\n",
    "subscription billing {\n",
    "    events [ ProductShipped ProductAudited ]\n",
    "    consumer billing_service\n",
    "}\n",
);

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.shop]
label = "Shop"
schema_dir = "shop"
postgres_schema = "shop"
"#;

fn entity_schema(title: &str) -> SchemaNode {
    SchemaNode {
        namespace: None,
        schema_id: format!("shop/json/{title}.json"),
        title: title.to_string(),
        description: None,
        schema_type: "object".to_string(),
        classification: "entity_reference".to_string(),
        domain: Some("shop".to_string()),
        rel_path: format!("shop/json/{title}.json"),
        pg_type: "UUID".to_string(),
        rust_type: "Uuid".to_string(),
        sea_orm_type: "Uuid".to_string(),
        rust_type_name: title.strip_suffix("Type").unwrap_or(title).to_string(),
        pg_table_name: title.strip_suffix("Type").unwrap_or(title).to_lowercase(),
        api_path_segment: title.strip_suffix("Type").unwrap_or(title).to_lowercase(),
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

/// Writes the fixture dir and returns (orders.evt path, domains.toml path).
fn write_fixture(dir: &Path) -> (PathBuf, PathBuf) {
    std::fs::write(dir.join("lib.mox"), LIB_MOX).unwrap();
    std::fs::write(dir.join("x.json"), X_JSON).unwrap();
    std::fs::write(dir.join("y.rosetta"), Y_ROSETTA).unwrap();
    std::fs::write(dir.join("z.rosetta"), Z_ROSETTA).unwrap();
    let evt = dir.join("orders.evt");
    std::fs::write(&evt, ORDERS_EVT).unwrap();
    let domains = dir.join("domains.toml");
    std::fs::write(&domains, DOMAINS_TOML).unwrap();
    (evt, domains)
}

#[tokio::test]
async fn ingest_evt_file_end_to_end_with_mox_schema_and_sigil_imports() {
    let dir = tempfile::tempdir().unwrap();
    let (evt, domains_path) = write_fixture(dir.path());
    let domain_config = codegraph_config::config::parse_domain_config(&domains_path).unwrap();

    // The schema graph carries the mox-bridge titles: Product resolves via
    // the +Type suffix, Kind exactly; Ghost is deliberately ABSENT (a
    // graph-level miss warns and counts but is not a compile error — the
    // type is valid in the mox domain).
    let mock = MockEngine::builder()
        .with_schema(entity_schema("ProductType"))
        .with_schema(entity_schema("Kind"))
        .build();

    let stats =
        codegraph::ingest::evt_ingest::ingest_evt_files(&mock, &mock, &[evt], &domain_config)
            .await
            .unwrap();

    assert_eq!(stats.files, 1);
    assert_eq!(stats.events, 2);
    assert_eq!(stats.channels, 1);
    assert_eq!(stats.subscriptions, 1);
    assert_eq!(
        stats.unresolved_types, 1,
        "only the Ghost miss counts — String is a silent primitive"
    );

    let models = mock.get_evt_models().await.unwrap();
    assert_eq!(models.len(), 1);
    let graph = &models[0];
    assert_eq!(graph.events.len(), 2);
    assert_eq!(graph.channels.len(), 1);
    assert_eq!(graph.subscriptions.len(), 1);
    assert_eq!(
        graph.channels[0].publishes,
        ["ProductShipped", "ProductAudited"]
    );
    assert_eq!(graph.subscriptions[0].consumer, "billing_service");

    // Title resolution against the ingested schema titles: exact first,
    // then +type_suffix; primitives and misses keep None.
    let shipped = graph
        .events
        .iter()
        .find(|e| e.name == "ProductShipped")
        .expect("ProductShipped event");
    assert_eq!(shipped.version.as_deref(), Some("2.0.0"));
    let label = shipped.fields.iter().find(|f| f.name == "label").unwrap();
    assert_eq!(label.resolved_title, None, "primitive → None");
    let product = shipped.fields.iter().find(|f| f.name == "product").unwrap();
    assert_eq!(product.resolved_title.as_deref(), Some("ProductType"));
    let kind = shipped.fields.iter().find(|f| f.name == "kind").unwrap();
    assert_eq!(kind.resolved_title.as_deref(), Some("Kind"));
    let ghost = shipped.fields.iter().find(|f| f.name == "ghost").unwrap();
    assert_eq!(ghost.resolved_title, None, "graph-level miss → None");
}

#[tokio::test]
async fn unknown_field_type_is_a_hard_error_with_the_diagnostic_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.mox"), LIB_MOX).unwrap();
    std::fs::write(dir.path().join("x.json"), X_JSON).unwrap();
    std::fs::write(dir.path().join("y.rosetta"), Y_ROSETTA).unwrap();
    std::fs::write(dir.path().join("z.rosetta"), Z_ROSETTA).unwrap();
    let evt = dir.path().join("bad.evt");
    std::fs::write(
        &evt,
        concat!(
            "import \"lib.mox\"\n",
            "\n",
            "event Bad {\n",
            "    x: Mystery;\n",
            "}\n",
        ),
    )
    .unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(&domains_path, DOMAINS_TOML).unwrap();
    let domain_config = codegraph_config::config::parse_domain_config(&domains_path).unwrap();

    let mock = MockEngine::new();
    let err = codegraph::ingest::evt_ingest::ingest_evt_files(&mock, &mock, &[evt], &domain_config)
        .await
        .unwrap_err();

    match &err {
        codegraph::error::Error::EvtModel { file, reason } => {
            assert!(file.ends_with("bad.evt"), "{file}");
            assert!(
                reason.contains("unknown type 'Mystery'"),
                "the rendered diagnostic carries rexlang's wording: {reason}"
            );
        }
        other => panic!("expected Error::EvtModel, got {other:?}"),
    }

    let models = mock.get_evt_models().await.unwrap();
    assert!(
        models.is_empty(),
        "a broken contract never half-ingests: {} models landed",
        models.len()
    );
}

#[tokio::test]
async fn missing_evt_file_is_a_hard_error() {
    let dir = tempfile::tempdir().unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(&domains_path, DOMAINS_TOML).unwrap();
    let domain_config = codegraph_config::config::parse_domain_config(&domains_path).unwrap();

    let mock = MockEngine::new();
    let err = codegraph::ingest::evt_ingest::ingest_evt_files(
        &mock,
        &mock,
        &[dir.path().join("absent.evt")],
        &domain_config,
    )
    .await
    .unwrap_err();

    match &err {
        codegraph::error::Error::EvtModel { file, reason } => {
            assert!(file.ends_with("absent.evt"), "{file}");
            assert!(reason.contains("could not be read"), "{reason}");
        }
        other => panic!("expected Error::EvtModel, got {other:?}"),
    }
}

// ── graph-cache inputs ─────────────────────────────────────────────────

#[test]
fn evt_files_are_part_of_the_graph_cache_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(&domains_path, DOMAINS_TOML).unwrap();
    let evt = dir.path().join("orders.evt");
    std::fs::write(&evt, ORDERS_EVT).unwrap();

    let inputs = codegraph::artifact::collect_run_inputs(
        None,
        None,
        &domains_path,
        &[],
        &[],
        &[],
        std::slice::from_ref(&evt),
        &[],
        &[],
        &[],
    )
    .unwrap();
    assert!(
        inputs
            .iter()
            .any(|f| f.path == evt.display().to_string() && f.bytes == ORDERS_EVT.as_bytes()),
        "the .evt file group rides the inputs list: {:?}",
        inputs.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    let before = codegraph::artifact::inputs_hash(&inputs);
    std::fs::write(
        &evt,
        ORDERS_EVT.replace("ProductAudited", "ProductReviewed"),
    )
    .unwrap();
    let after = codegraph::artifact::inputs_hash(
        &codegraph::artifact::collect_run_inputs(
            None,
            None,
            &domains_path,
            &[],
            &[],
            &[],
            std::slice::from_ref(&evt),
            &[],
            &[],
            &[],
        )
        .unwrap(),
    );
    assert_ne!(
        before, after,
        "editing a .evt file must bust the persisted-graph cache"
    );

    // A run without evt files is unaffected (the group is empty).
    let without = codegraph::artifact::collect_run_inputs(
        None,
        None,
        &domains_path,
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();
    assert!(
        !without.iter().any(|f| f.path.contains(".evt")),
        "no evt entries without evt files"
    );
}
