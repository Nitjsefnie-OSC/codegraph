//! WP1.1 — inheritance probes (issue #254, Rune `extends` gap analysis).
//!
//! Characterization tests pinning how JSON-Schema `allOf` `$ref` composition
//! (the legacy stand-in for Rune DSL `extends`) flows from ingest into
//! generator output, and whether a generation-time inheritance merge would
//! be needed on top. Companion findings:
//! `docs/rosetta/findings/wp1.1-inheritance.md`.

use std::collections::HashSet;

use crate::support;

const ASSET_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "AssetType",
  "description": "A tracked asset.",
  "type": "object",
  "properties": {
    "id": {
      "description": "Primary identifier.",
      "type": "string",
      "format": "uuid"
    },
    "label": {
      "description": "Asset label.",
      "type": "string"
    }
  },
  "required": ["id", "label"]
}"#;

const VEHICLE_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "VehicleType",
  "description": "A vehicle.",
  "type": "object",
  "allOf": [{ "$ref": "AssetType.json" }],
  "properties": {
    "plateNo": {
      "description": "License plate.",
      "type": "string"
    }
  },
  "required": ["plateNo"]
}"#;

const BASE_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "BaseType",
  "description": "Chain root.",
  "type": "object",
  "properties": {
    "zetaField": { "description": "Root field.", "type": "string" }
  },
  "required": ["zetaField"]
}"#;

const MID_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "MidType",
  "description": "Chain middle.",
  "type": "object",
  "allOf": [{ "$ref": "BaseType.json" }],
  "properties": {
    "betaField": { "description": "Middle field.", "type": "string" }
  },
  "required": ["betaField"]
}"#;

const LEAF_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "LeafType",
  "description": "Chain leaf.",
  "type": "object",
  "allOf": [{ "$ref": "MidType.json" }],
  "properties": {
    "deltaField": { "description": "Leaf field.", "type": "string" }
  },
  "required": ["deltaField"]
}"#;

fn count_occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

fn lines_with(text: &str, needle: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.contains(needle))
        .map(|l| l.trim().to_string())
        .collect()
}

/// Column-declaration lines: trimmed lines that start with `<column> ` or
/// with the quoted form `<column>" ` (excludes COMMENT ON / CONSTRAINT /
/// index lines mentioning the name). Reserved-word columns are emitted
/// double-quoted (e.g. `"label" TEXT NOT NULL`).
fn column_declaration_lines(ddl: &str, column: &str) -> Vec<String> {
    let bare = format!("{column} ");
    let quoted = format!("{column}\" ");
    ddl.lines()
        .map(str::trim_start)
        .filter(|t| {
            t.starts_with(&bare)
                || (t.starts_with('"')
                    && t.strip_prefix('"').is_some_and(|r| r.starts_with(&quoted)))
        })
        .map(str::to_string)
        .collect()
}

fn create_table_lines(ddl: &str) -> Vec<String> {
    lines_with(ddl, "CREATE TABLE")
}

/// `pub <snake_ident>: <Type>,` lines of the generated SeaORM model, in file
/// order — the observable field order of the entity struct.
fn entity_field_order(entity_rs: &str) -> Vec<String> {
    entity_rs
        .lines()
        .filter_map(|line| {
            let t = line.trim();
            let rest = t.strip_prefix("pub ")?;
            let (name, _) = rest.split_once(':')?;
            let name = name.trim();
            if t.ends_with(',')
                && !name.is_empty()
                && name.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Probe 1 — the ingest-time allOf flatten composes inherited attributes into
/// generator output: VehicleType (allOf → AssetType, own plateNo) must carry
/// id + label + plate_no in the SeaORM entity, and the vehicle DDL table must
/// carry label + plate_no columns.
#[tokio::test]
async fn allof_flatten_composes_inherited_fields_in_generators() {
    let root = tempfile::tempdir().unwrap();
    let files = support::run_pipeline(
        root.path(),
        "gap",
        &["AssetType", "VehicleType"],
        &[
            ("AssetType.json", ASSET_JSON),
            ("VehicleType.json", VEHICLE_JSON),
        ],
    )
    .await;

    let entity = files
        .get("src/entity/gap_vehicle.rs")
        .expect("vehicle entity file emitted");
    for field in ["pub id:", "pub label:", "pub plate_no:"] {
        assert_eq!(
            count_occurrences(entity, field),
            1,
            "expected exactly one `{field}` field in vehicle entity:\n{entity}"
        );
    }

    let ddl = support::file_by_suffix(&files, "gap_vehicle.sql")
        .expect("vehicle DDL migration emitted")
        .clone();
    for column in ["label", "plate_no"] {
        let decls = column_declaration_lines(&ddl, column);
        assert!(
            !decls.is_empty(),
            "vehicle DDL must contain a `{column}` column:\n{ddl}"
        );
        eprintln!("vehicle DDL `{column}` column decls: {decls:?}");
    }
    // The vehicle table itself is `gap.vehicle` (pg_table_name carries no
    // domain prefix); the migration file is `gap_vehicle.sql`.
    assert!(
        ddl.contains("CREATE TABLE IF NOT EXISTS gap.vehicle ("),
        "vehicle DDL must create gap.vehicle"
    );
}

/// Probe 2 — a two-level allOf chain (A ← B ← C): C's generated entity and
/// DTO must contain A's + B's + C's fields. The observed field ORDER is
/// recorded (characterization; ancestor-first was the pre-run expectation).
#[tokio::test]
async fn allof_two_level_chain_ancestor_order() {
    let root = tempfile::tempdir().unwrap();
    let files = support::run_pipeline(
        root.path(),
        "gap",
        &["BaseType", "MidType", "LeafType"],
        &[
            ("BaseType.json", BASE_JSON),
            ("MidType.json", MID_JSON),
            ("LeafType.json", LEAF_JSON),
        ],
    )
    .await;

    let entity = files
        .get("src/entity/gap_leaf.rs")
        .expect("leaf entity file emitted");
    for field in ["pub zeta_field:", "pub beta_field:", "pub delta_field:"] {
        assert_eq!(
            count_occurrences(entity, field),
            1,
            "expected exactly one `{field}` in leaf entity:\n{entity}"
        );
    }

    // Plan-less runs (#447) land the domain-types crate at
    // crates/domain-types/; profiled layouts keep the historical paths.
    let dto = files
        .get("crates/domain-types/src/gap/leaf/dto_response.rs")
        .or_else(|| files.get("src/gap/leaf/dto_response.rs"))
        .or_else(|| files.get("src/domain/gap/leaf/dto_response.rs"))
        .map(String::as_str)
        .expect("leaf domain_types dto_response emitted");
    for field in ["zeta_field", "beta_field", "delta_field"] {
        assert!(
            dto.contains(field),
            "leaf dto_response must contain `{field}`:\n{dto}"
        );
    }

    let order = entity_field_order(entity);
    eprintln!("leaf entity field order: {order:?}");
    assert_eq!(
        order.first().map(String::as_str),
        Some("id"),
        "id (PK) must be the first entity field"
    );
    for field in ["zeta_field", "beta_field", "delta_field"] {
        assert!(
            order.contains(&field.to_string()),
            "entity field order must contain `{field}`: {order:?}"
        );
    }
    // Characterization: the composition order is NOT ancestor-first and NOT
    // name-sorted. Fields appear in allOf-flatten ingestion order — the
    // leaf's own fields first, then ancestors nearest-first (root last).
    // Names were chosen so alphabetical (beta, delta, zeta) differs from
    // insertion (delta, beta, zeta) to disambiguate.
    let idx = |f: &str| order.iter().position(|x| x == f).expect("field present");
    assert!(
        idx("delta_field") < idx("beta_field") && idx("beta_field") < idx("zeta_field"),
        "expected flatten-insertion field order (delta, beta, zeta), got {order:?}"
    );

    // Graph-level order behind the generated order: what `get_properties`
    // returns for the leaf schema, before and after the driver's
    // reclassify pass.
    let root2 = tempfile::tempdir().unwrap();
    let be = support::ingest_into_graph(
        root2.path(),
        "gap",
        &["BaseType", "MidType", "LeafType"],
        &[
            ("BaseType.json", BASE_JSON),
            ("MidType.json", MID_JSON),
            ("LeafType.json", LEAF_JSON),
        ],
    )
    .await;
    let q = be.querier();
    for attempt in 1..=3 {
        let names: Vec<String> = q
            .get_properties("LeafType")
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        eprintln!("LeafType get_properties attempt {attempt}: {names:?}");
    }
    let prop_names: Vec<String> = q
        .get_properties("LeafType")
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    eprintln!("LeafType graph property order (post-ingest): {prop_names:?}");

    let entity_names: HashSet<String> = ["BaseType", "MidType", "LeafType"]
        .into_iter()
        .map(String::from)
        .collect();
    codegraph::ingest::async_ingest::reclassify_with_entities(
        be.ingestor(),
        be.querier(),
        &entity_names,
    )
    .await
    .unwrap();
    let prop_names_reclassified: Vec<String> = q
        .get_properties("LeafType")
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    eprintln!("LeafType graph property order (post-reclassify): {prop_names_reclassified:?}");
    // Characterization: direct `get_properties` right after ingest returns
    // NAME-SORTED properties. The generated entity order (asserted above) is
    // flatten-insertion order instead — the pipeline's generator-visible
    // ordering does NOT match this direct-query ordering, so property order
    // out of the graph must not be relied upon by any generation-time merge.
    assert_eq!(
        prop_names_reclassified,
        vec!["betaField", "deltaField", "zetaField"],
        "direct post-ingest get_properties order (characterization)"
    );
}

/// Probe 3 — ExtendsSchema edges persist at ingest and are resolvable via the
/// GraphQuerier: VehicleType's allOf parents = [AssetType], AssetType is
/// reported as extended by VehicleType, and the composition tree already
/// carries AssetType as a child node (what the DDL/entity generators read).
#[tokio::test]
async fn extendschema_edges_persisted_in_graph() {
    let root = tempfile::tempdir().unwrap();
    let be = support::ingest_into_graph(
        root.path(),
        "gap",
        &["AssetType", "VehicleType"],
        &[
            ("AssetType.json", ASSET_JSON),
            ("VehicleType.json", VEHICLE_JSON),
        ],
    )
    .await;
    let q = be.querier();

    let prop_names: Vec<String> = q
        .get_properties("VehicleType")
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    eprintln!("VehicleType graph property order: {prop_names:?}");

    let mut parents = q.get_allof_targets("VehicleType").await.unwrap();
    parents.sort();
    assert_eq!(parents, vec!["AssetType".to_string()]);

    let extenders = q.get_schemas_that_extend("AssetType").await.unwrap();
    let extender_titles: Vec<&str> = extenders.iter().map(|s| s.title.as_str()).collect();
    assert!(
        extender_titles.contains(&"VehicleType"),
        "AssetType must be reported as extended by VehicleType, got {extender_titles:?}"
    );

    let tree = q.get_composition_tree("VehicleType").await.unwrap();
    let child_titles: Vec<&str> = tree
        .root
        .children
        .iter()
        .map(|c| c.schema_title.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        child_titles,
        vec!["AssetType"],
        "composition tree must carry the allOf parent as a child node"
    );
    let child = &tree.root.children[0];
    let child_cols: Vec<&str> = child.columns.iter().map(|c| c.name.as_str()).collect();
    assert!(
        child_cols.contains(&"id") && child_cols.contains(&"label"),
        "allOf child node must carry the parent's columns (id, label), got {child_cols:?}"
    );
}

/// Probe 4 — no double merge in output: the vehicle entity must not duplicate
/// a field declaration nor leak an allOf child-table artifact. The DDL side
/// is characterized: does a separate child table / FK for AssetType appear in
/// the vehicle migration (composition-tree child emission) or not?
#[tokio::test]
async fn no_double_merge_in_output() {
    let root = tempfile::tempdir().unwrap();
    let files = support::run_pipeline(
        root.path(),
        "gap",
        &["AssetType", "VehicleType"],
        &[
            ("AssetType.json", ASSET_JSON),
            ("VehicleType.json", VEHICLE_JSON),
        ],
    )
    .await;

    let entity = files
        .get("src/entity/gap_vehicle.rs")
        .expect("vehicle entity file emitted");
    assert_eq!(
        count_occurrences(entity, "pub plate_no:"),
        1,
        "plate_no must be declared exactly once in the vehicle entity"
    );
    let leaked: Vec<String> = entity
        .lines()
        .filter(|l| l.to_lowercase().contains("assettype"))
        .map(|l| l.trim().to_string())
        .collect();
    assert!(
        leaked.is_empty(),
        "allOf child-table artifact must not leak into the vehicle entity: {leaked:?}"
    );

    let ddl = support::file_by_suffix(&files, "gap_vehicle.sql")
        .expect("vehicle DDL migration emitted")
        .clone();

    // Characterization: the composition tree turns the allOf parent into a
    // child CompositionNode (querier push_allof_children), which the DDL
    // generator materializes as a SHADOW CHILD TABLE inside the vehicle
    // migration — duplicating the already-flattened `label` column. There is
    // no FK between the two entity tables (the child table hangs off
    // gap.vehicle, not off gap.asset).
    let tables = create_table_lines(&ddl);
    assert_eq!(
        tables,
        vec![
            "CREATE TABLE IF NOT EXISTS gap.vehicle (".to_string(),
            "CREATE TABLE IF NOT EXISTS gap.vehicle_assettype (".to_string(),
        ],
        "vehicle migration must contain the parent table plus the allOf child table"
    );
    assert_eq!(
        column_declaration_lines(&ddl, "label").len(),
        2,
        "label must be declared twice: once on gap.vehicle (flatten) and once on \
         gap.vehicle_assettype (composition-tree child table):\n{ddl}"
    );
    assert_eq!(
        column_declaration_lines(&ddl, "plate_no").len(),
        1,
        "plate_no (vehicle's own field) must be declared exactly once:\n{ddl}"
    );
    let asset_fk: Vec<String> = lines_with(&ddl, "gap.asset");
    assert!(
        asset_fk.is_empty(),
        "no FK may reference the allOf parent's own table from the vehicle migration: {asset_fk:?}"
    );
    assert!(
        ddl.contains("FOREIGN KEY (vehicle_id) REFERENCES gap.vehicle(id)"),
        "the allOf child table must hang off the extending table:\n{ddl}"
    );
}
