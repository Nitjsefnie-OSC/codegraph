// WP1.7 — data-plane draft sweep (see docs/rosetta/findings/wp1.7-data-plane.md).
//
// Characterization probes: ingest a representative data model (entity +
// refers + codelist + contained value object + arrays) into the graph and
// verify the draft disposition row by row ("existing graph" via
// SchemaNode/PropertyNode/CodeList + ReferencesSchema/ItemsOf edges).

use std::collections::{BTreeMap, HashMap, HashSet};

use codegraph_type_contracts::RefClassificationKind;

use crate::support;

// ── Fixture ──────────────────────────────────────────────────────────────

const ORDER_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "OrderType",
  "description": "A customer order.",
  "type": "object",
  "properties": {
    "id": {
      "description": "Primary identifier.",
      "type": "string",
      "format": "uuid"
    },
    "title": {
      "description": "Order title.",
      "type": "string"
    },
    "quantity": {
      "description": "Units ordered.",
      "type": "integer"
    },
    "price": {
      "description": "Unit price.",
      "type": "number",
      "format": "double"
    },
    "active": {
      "description": "Active flag.",
      "type": "boolean"
    },
    "due_date": {
      "description": "Due date.",
      "type": "string",
      "format": "date"
    },
    "completed_at": {
      "description": "Completion timestamp.",
      "type": "string",
      "format": "date-time"
    },
    "customer": {
      "description": "Owning customer.",
      "$ref": "CustomerType.json#"
    },
    "tags": {
      "description": "Free-form tags.",
      "type": "array",
      "items": { "type": "string" }
    },
    "history": {
      "description": "Status history codes.",
      "type": "array",
      "items": { "$ref": "codelist/OrderStatus.json#" }
    },
    "status": {
      "description": "Order status.",
      "$ref": "codelist/OrderStatus.json#"
    },
    "detail": {
      "description": "Shipping detail.",
      "$ref": "OrderDetailType.json#"
    }
  },
  "required": ["id", "title", "status", "customer"]
}"#;

const CUSTOMER_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "CustomerType",
  "description": "A customer.",
  "type": "object",
  "properties": {
    "id": {
      "description": "Primary identifier.",
      "type": "string",
      "format": "uuid"
    },
    "name": {
      "description": "Customer name.",
      "type": "string"
    }
  },
  "required": ["id", "name"]
}"#;

/// Contained value object: NOT listed in domains.toml `entities`, so the
/// classify_ref fallback (entity-reference vs value-object) makes every
/// $ref to it a ValueObject composition (same shape as WorkOrderDetailType
/// in the mox equivalence harness).
const ORDER_DETAIL_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "OrderDetailType",
  "description": "Shipping detail.",
  "type": "object",
  "properties": {
    "address_line": {
      "description": "Street address.",
      "type": "string"
    },
    "city": {
      "description": "City.",
      "type": "string"
    }
  },
  "required": ["address_line"]
}"#;

/// Codelist schema: lives under `codelist/`, which routes classify_ref to
/// CodelistReference (the path-based codelist rule).
const ORDER_STATUS_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "OrderStatus",
  "description": "Order lifecycle status.",
  "type": "string",
  "enum": ["Open", "InProgress", "Closed"],
  "enumNames": ["Open", "In progress", "Closed"]
}"#;

const DOMAIN: &str = "sweep";
const ENTITIES: &[&str] = &["OrderType", "CustomerType"];

fn fixture_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("OrderType.json", ORDER_JSON),
        ("CustomerType.json", CUSTOMER_JSON),
        ("OrderDetailType.json", ORDER_DETAIL_JSON),
        ("codelist/OrderStatus.json", ORDER_STATUS_JSON),
    ]
}

/// Property lookup by name (get_properties order is not pinned by the API).
fn props_by_name(
    props: Vec<codegraph_core::types::PropertyNode>,
) -> HashMap<String, codegraph_core::types::PropertyNode> {
    props.into_iter().map(|p| (p.name.clone(), p)).collect()
}

// ── Probe 1: entity + properties round-trip ─────────────────────────────

#[tokio::test]
async fn data_entity_round_trips_through_graph() {
    let root = tempfile::tempdir().unwrap();
    let be = support::ingest_into_graph(root.path(), DOMAIN, ENTITIES, &fixture_files()).await;
    let q = be.querier();

    // SchemaNode for the entity, in the right domain.
    let order = q
        .get_schema_in_domain("OrderType", DOMAIN)
        .await
        .unwrap()
        .expect("OrderType SchemaNode missing");
    assert_eq!(order.title, "OrderType");
    assert_eq!(order.domain.as_deref(), Some(DOMAIN));
    assert!(order.is_entity, "OrderType must be an entity");
    assert!(!order.is_codelist);
    assert_eq!(order.schema_type, "object");
    assert_eq!(order.classification, "entity_reference");
    assert_eq!(order.pg_table_name, "order");
    assert_eq!(order.rust_type_name, "Order");
    assert_eq!(order.api_path_segment, "order");

    // PropertyNodes: exact prop_type/format representation per fixture.
    let props = props_by_name(
        q.get_properties_in_domain("OrderType", DOMAIN)
            .await
            .unwrap(),
    );
    assert_eq!(
        props.len(),
        12,
        "all 12 OrderType properties must round-trip"
    );

    let id = &props["id"];
    assert_eq!(id.prop_type, "string");
    assert_eq!(id.format.as_deref(), Some("uuid"));
    assert!(id.is_required && !id.is_nullable && !id.is_array);
    assert_eq!(id.pg_column_name, "id");
    assert_eq!(id.pg_column_type, "UUID");
    assert_eq!(id.rust_field_type, "uuid::Uuid");
    assert_eq!(id.sea_orm_type, "Uuid");
    assert_eq!(
        id.classification_kind,
        Some(RefClassificationKind::PrimitiveWrapper)
    );

    let title = &props["title"];
    assert_eq!(title.prop_type, "string");
    assert_eq!(title.format, None);
    assert!(title.is_required && !title.is_nullable);
    assert_eq!(title.pg_column_type, "TEXT");
    assert_eq!(title.rust_field_type, "String");
    assert_eq!(title.sea_orm_type, "Text");

    let quantity = &props["quantity"];
    assert_eq!(quantity.prop_type, "integer");
    assert!(!quantity.is_required && quantity.is_nullable);
    assert_eq!(quantity.pg_column_type, "BIGINT");
    assert_eq!(quantity.rust_field_type, "i64");
    assert_eq!(quantity.sea_orm_type, "BigInteger");

    let price = &props["price"];
    assert_eq!(price.prop_type, "number");
    assert_eq!(price.format.as_deref(), Some("double"));
    assert_eq!(price.pg_column_type, "DOUBLE PRECISION");
    assert_eq!(price.rust_field_type, "f64");
    assert_eq!(price.sea_orm_type, "Double");

    let active = &props["active"];
    assert_eq!(active.prop_type, "boolean");
    assert_eq!(active.pg_column_type, "BOOLEAN");
    assert_eq!(active.rust_field_type, "bool");
    assert_eq!(active.sea_orm_type, "Boolean");

    let due_date = &props["due_date"];
    assert_eq!(due_date.prop_type, "string");
    assert_eq!(due_date.format.as_deref(), Some("date"));
    assert_eq!(due_date.pg_column_type, "DATE");
    assert_eq!(due_date.rust_field_type, "chrono::NaiveDate");
    assert_eq!(due_date.sea_orm_type, "Date");

    let completed_at = &props["completed_at"];
    assert_eq!(completed_at.prop_type, "string");
    assert_eq!(completed_at.format.as_deref(), Some("date-time"));
    assert_eq!(completed_at.pg_column_type, "TIMESTAMPTZ");
    assert_eq!(
        completed_at.rust_field_type,
        "chrono::DateTime<chrono::Utc>"
    );
    assert_eq!(completed_at.sea_orm_type, "TimestampWithTimeZone");

    // refers → CustomerType: the $ref property carries the ref path on
    // ref_target and an EntityReference classification. The raw prop_type
    // falls back to "object" ($ref properties have no "type" key) and the
    // scalar pg/sea columns stay empty — the FK type resolves from the
    // target at generation time.
    let customer = &props["customer"];
    assert_eq!(customer.prop_type, "object");
    assert!(customer.is_required && !customer.is_nullable);
    assert_eq!(customer.ref_target.as_deref(), Some("CustomerType.json#"));
    assert_eq!(
        customer.classification_kind,
        Some(RefClassificationKind::EntityReference)
    );
    assert_eq!(customer.rust_field_type, "CustomerType");
    assert_eq!(customer.pg_column_type, "");

    // tags: array of primitive → TEXT[] + Vec<String>, no ref.
    let tags = &props["tags"];
    assert_eq!(tags.prop_type, "array");
    assert!(tags.is_array);
    assert_eq!(tags.pg_column_type, "TEXT[]");
    assert_eq!(tags.rust_field_type, "Vec<String>");
    assert_eq!(tags.ref_target, None);

    // status: scalar $ref to a codelist → CodelistReference (pinned fully
    // in probe 3). detail: scalar $ref to a VO → ValueObject.
    assert_eq!(
        props["status"].classification_kind,
        Some(RefClassificationKind::CodelistReference)
    );
    let detail = &props["detail"];
    assert_eq!(detail.prop_type, "object");
    assert_eq!(detail.ref_target.as_deref(), Some("OrderDetailType.json#"));
    assert_eq!(
        detail.classification_kind,
        Some(RefClassificationKind::ValueObject)
    );
    assert_eq!(detail.rust_field_type, "OrderDetailType");

    // The VO schema itself round-trips as a non-entity SchemaNode.
    let vo = q
        .get_schema("OrderDetailType")
        .await
        .unwrap()
        .expect("VO schema missing");
    assert!(!vo.is_entity);
    assert_eq!(vo.classification, "value_object");
    assert_eq!(vo.pg_table_name, "order_detail");
    assert_eq!(vo.domain.as_deref(), Some(DOMAIN));
}

// ── Probe 2: ReferencesSchema / ItemsOf edges ────────────────────────────

#[tokio::test]
async fn referenceschema_and_itemsof_edges_persisted() {
    let root = tempfile::tempdir().unwrap();
    let be = support::ingest_into_graph(root.path(), DOMAIN, ENTITIES, &fixture_files()).await;
    let q = be.querier();

    // Scalar $ref (the refers case) → ReferencesSchema edge, exposed via
    // get_property_ref_target as the fully-resolved target SchemaNode.
    let customer = q
        .get_property_ref_target("customer", "OrderType")
        .await
        .unwrap()
        .expect("customer ReferencesSchema edge missing");
    assert_eq!(customer.title, "CustomerType");
    assert!(customer.is_entity);
    assert_eq!(customer.pg_table_name, "customer");
    assert_eq!(customer.domain.as_deref(), Some(DOMAIN));

    // Scalar codelist ref also rides ReferencesSchema.
    let status = q
        .get_property_ref_target("status", "OrderType")
        .await
        .unwrap()
        .expect("status ReferencesSchema edge missing");
    assert_eq!(status.title, "OrderStatus");
    assert!(status.is_codelist);

    // Scalar VO ref rides ReferencesSchema too.
    let detail = q
        .get_property_ref_target("detail", "OrderType")
        .await
        .unwrap()
        .expect("detail ReferencesSchema edge missing");
    assert_eq!(detail.title, "OrderDetailType");
    assert!(!detail.is_entity);

    // Array-of-codelist → ItemsOf edge, exposed via get_array_item_schema.
    let history_item = q
        .get_array_item_schema("history", "OrderType")
        .await
        .unwrap()
        .expect("history ItemsOf edge missing");
    assert_eq!(history_item.title, "OrderStatus");
    assert!(history_item.is_codelist);

    // Array of primitives carries NO edge (no $ref to resolve).
    assert!(
        q.get_array_item_schema("tags", "OrderType")
            .await
            .unwrap()
            .is_none()
    );

    // Bulk accessors see exactly the three scalar ReferencesSchema targets
    // (history went the ItemsOf route, so OrderStatus appears once here).
    let referenced: HashSet<String> = q
        .get_referenced_schemas("OrderType")
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.title)
        .collect();
    assert_eq!(
        referenced,
        HashSet::from([
            "CustomerType".to_string(),
            "OrderStatus".to_string(),
            "OrderDetailType".to_string(),
        ])
    );

    let mut all_refs = q.list_all_schema_references().await.unwrap();
    all_refs.sort();
    assert!(
        all_refs.contains(&("OrderType".to_string(), "CustomerType".to_string())),
        "bulk reference list must contain (OrderType, CustomerType): {all_refs:?}"
    );

    // Reverse direction: the refers target sees the referencing entity.
    let referencing = q.get_referencing_schemas("CustomerType").await.unwrap();
    assert!(
        referencing.iter().any(|t| t == "OrderType"),
        "CustomerType must be referenced by OrderType: {referencing:?}"
    );
}

// ── Probe 3: codelist round-trip ─────────────────────────────────────────

#[tokio::test]
async fn codelist_round_trips() {
    let root = tempfile::tempdir().unwrap();
    let be = support::ingest_into_graph(root.path(), DOMAIN, ENTITIES, &fixture_files()).await;
    let q = be.querier();

    // Codelist SchemaNode.
    let schema = q
        .get_schema("OrderStatus")
        .await
        .unwrap()
        .expect("codelist schema missing");
    assert!(schema.is_codelist);
    assert!(!schema.is_entity);
    assert_eq!(schema.classification, "codelist");
    assert_eq!(schema.schema_type, "string");
    assert_eq!(schema.pg_table_name, "order_status");
    assert_eq!(schema.domain.as_deref(), Some(DOMAIN));

    // CodeList node.
    let codelist = q
        .get_codelist("OrderStatus")
        .await
        .unwrap()
        .expect("CodeList node missing");
    assert_eq!(codelist.name, "OrderStatus");
    assert_eq!(codelist.pg_table_name, "order_status");
    assert_eq!(codelist.render_as, "codelist");
    assert_eq!(
        codelist.description.as_deref(),
        Some("Order lifecycle status.")
    );

    // EnumValues with display names + sort order preserved. NOTE: the
    // accessor has no ORDER BY, so the probe sorts by sort_order itself.
    let mut values = q.get_enum_values("OrderStatus").await.unwrap();
    values.sort_by_key(|v| v.sort_order);
    assert_eq!(
        values.iter().map(|v| v.value.as_str()).collect::<Vec<_>>(),
        vec!["Open", "InProgress", "Closed"]
    );
    assert_eq!(
        values
            .iter()
            .map(|v| v.display_name.clone())
            .collect::<Vec<_>>(),
        vec![
            Some("Open".to_string()),
            Some("In progress".to_string()),
            Some("Closed".to_string()),
        ]
    );
    assert_eq!(
        values.iter().map(|v| v.sort_order).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );

    // The referencing property: CodelistReference classification + the
    // codelist as ref_target + TEXT scalar storage.
    let props = props_by_name(q.get_properties("OrderType").await.unwrap());
    let status = &props["status"];
    assert_eq!(
        status.classification_kind,
        Some(RefClassificationKind::CodelistReference)
    );
    assert_eq!(
        status.ref_target.as_deref(),
        Some("codelist/OrderStatus.json#")
    );
    assert_eq!(status.pg_column_type, "TEXT");
    assert_eq!(status.rust_field_type, "String");
    assert_eq!(status.sea_orm_type, "Text");
    assert_eq!(status.render_strategy, "codelist");

    // SURPRISE (recorded in findings): get_codelist_for_property walks a
    // UsesCodeList edge that NO production ingest path ever creates — the
    // JSON-path codelist link lives on PropertyNode.ref_target +
    // classification_kind instead. The accessor is dormant for ingested
    // models.
    assert!(
        q.get_codelist_for_property("status", "OrderType")
            .await
            .unwrap()
            .is_none()
    );
}

// ── Probe 4: value object composes a child table ─────────────────────────

#[tokio::test]
async fn value_object_composes_child_table() {
    let root = tempfile::tempdir().unwrap();
    let files = support::run_pipeline(root.path(), DOMAIN, ENTITIES, &fixture_files()).await;

    let order_sql = support::file_by_suffix(&files, "sweep_order.sql")
        .expect("order DDL migration missing")
        .clone();

    // The parent table's own column list (everything before the first
    // child-table block) has NO flattened VO fields — the contrast with the
    // allOf-extends flatten pinned in WP1.1.
    let parent_block = order_sql.split("-- Child table:").next().unwrap();
    assert!(
        !parent_block.contains("address_line"),
        "VO fields must not be flattened into the parent table:\n{parent_block}"
    );

    // The contained VO composes a child table rendered inline on the parent
    // migration: own PK, {parent}_id FK back to the parent, the VO's fields.
    assert!(
        order_sql.contains("-- Child table: sweep.order_detail"),
        "parent DDL must carry the order_detail child table block:\n{order_sql}"
    );
    assert!(
        order_sql.contains("REFERENCES sweep.\"order\"(id)"),
        "child table must FK back to the parent table:\n{order_sql}"
    );

    // Array-of-codelist → junction-style child table with a code column,
    // also rendered inline on the parent migration. It has NO own generation
    // entry: a synthetic composition node, not a Schema (no
    // sweep_order_history.sql exists in the output).
    assert!(order_sql.contains("-- Child table: sweep.order_history"));
    assert!(order_sql.contains("code TEXT NOT NULL"));

    // Codelist FKs route to the common schema (deferred there if absent)…
    assert!(
        order_sql.contains("REFERENCES common.order_status(code)"),
        "codelist FK must target the common-schema codelist table:\n{order_sql}"
    );

    // …while the codelist's own migration creates the table in its own
    // domain WITHOUT a code column (non-common codelist shell: id + tenant +
    // timestamps only — the enum values live in the graph alone for this
    // shape).
    let status_sql = support::file_by_suffix(&files, "sweep_order_status.sql")
        .expect("codelist migration missing");
    assert!(
        status_sql.contains("CREATE TABLE IF NOT EXISTS sweep.order_status"),
        "{status_sql}"
    );
    assert!(
        !status_sql.contains("    code "),
        "non-common codelist shell must not carry a code column:\n{status_sql}"
    );

    // The VO ALSO gets its own generation entry (any Schema with a
    // pg_table_name does): a standalone sweep_order_detail migration.
    // The standalone carries the parent-FK COLUMN: `order` + `detail`
    // collide (child_table_name(order, detail) == the VO's own table), so
    // the standalone renders the parent's child-table shape — otherwise the
    // bare shape would shadow the projection under IF NOT EXISTS and the
    // cornucopia child queries (WHERE order_id) would not validate against
    // the live database. Non-colliding VOs keep the no-FK quirk.
    let detail_sql = support::file_by_suffix(&files, "sweep_order_detail.sql")
        .expect("VO standalone migration missing")
        .clone();
    assert!(detail_sql.contains("address_line"), "{detail_sql}");
    assert!(
        detail_sql.contains("order_id"),
        "colliding VO standalone carries the parent-FK column:\n{detail_sql}"
    );

    // The parent SeaORM entity has no flattened VO fields either.
    let order_entity = support::file_by_suffix(&files, "entity/sweep_order.rs")
        .expect("order SeaORM entity missing");
    assert!(
        !order_entity.contains("address_line"),
        "parent entity must not carry flattened VO fields"
    );
    assert!(order_entity.contains("table_name = \"order\""));
}

// ── Probe 5: generator artifact set ──────────────────────────────────────

#[tokio::test]
async fn generator_artifacts_complete() {
    let root = tempfile::tempdir().unwrap();
    let files: BTreeMap<String, String> =
        support::run_pipeline(root.path(), DOMAIN, ENTITIES, &fixture_files()).await;

    let has = |suffix: &str| {
        files
            .keys()
            .any(|k| k.ends_with(suffix) && !k.ends_with(&format!("_rls{suffix}")))
    };
    let get = |suffix: &str| support::file_by_suffix(&files, suffix).unwrap().clone();

    // DDL migrations (entity + refers target + codelist + VO child). Names
    // carry a deterministic sequence prefix, hence suffix matching.
    assert!(has("sweep_order.sql"), "order DDL missing");
    assert!(has("sweep_customer.sql"), "customer DDL missing");
    assert!(has("sweep_order_status.sql"), "codelist DDL missing");
    assert!(has("sweep_order_detail.sql"), "VO child DDL missing");

    // SeaORM entities.
    let order_entity = get("entity/sweep_order.rs");
    assert!(
        order_entity.contains("DeriveEntityModel"),
        "not a SeaORM entity"
    );
    assert!(has("entity/sweep_customer.rs"), "customer entity missing");

    // DTOs (create/update/response) under the domain tree.
    assert!(
        has("domain/sweep/order/dto_create.rs"),
        "dto_create missing"
    );
    assert!(
        has("domain/sweep/order/dto_update.rs"),
        "dto_update missing"
    );
    assert!(
        has("domain/sweep/order/dto_response.rs"),
        "dto_response missing"
    );

    // API handler.
    let handler = get("api/sweep/order_handler.rs");
    assert!(handler.contains("Order"), "handler should mention Order");
}
