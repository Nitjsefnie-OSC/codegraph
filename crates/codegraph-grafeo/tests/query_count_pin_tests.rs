//! Issue #389 pin — process-isolated integration test (own binary, own
//! process-global counter): concurrent in-lib graph tests can no longer
//! pollute the count (the +2 flakes observed on PR CIs #494/#495).

//! Issue #389: pins the GQL query volume of one `get_composition_tree`
//! call over a representative model (multi-property entity with sibling
//! VO children of the same type, a codelist property, a scalar entity
//! ref, a junction array, and an allOf extender). If you change the
//! recursion, re-measure and update the pinned number — the count must
//! only ever go DOWN.

use codegraph_grafeo::{query_count, reset_query_count};

/// Process-local lock for the two counter-sensitive tests: the counter is
/// process-global and this binary's tests run on parallel threads (the
/// in-crate COUNTER_LOCK is #[cfg(test)]-only; same mechanism, per-process
/// instance is fine since each test binary gets its own process).
static COUNTER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

use codegraph_core::traits::{GraphIngestor, GraphQuerier};
use codegraph_core::types::{CodeList, PropertyNode, SchemaNode};
use codegraph_core::types::{EdgeProperties, EdgeType};
use codegraph_grafeo::GrafeoEngine;
use codegraph_type_contracts::RefClassificationKind;

fn schema(title: &str, domain: &str, is_entity: bool) -> SchemaNode {
    SchemaNode {
        namespace: None,
        schema_id: format!("{domain}/{title}"),
        title: title.to_string(),
        description: None,
        schema_type: "object".to_string(),
        classification: if is_entity { "entity" } else { "value_object" }.to_string(),
        domain: Some(domain.to_string()),
        rel_path: format!("{domain}/{title}.json"),
        pg_type: "TABLE".to_string(),
        rust_type: title.to_string(),
        sea_orm_type: "Entity".to_string(),
        rust_type_name: title.to_string(),
        pg_table_name: title.to_string(),
        api_path_segment: title.to_string(),
        parent_schema: None,
        is_entity,
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

fn codelist_schema(title: &str) -> SchemaNode {
    let mut s = schema(title, "common", false);
    s.is_codelist = true;
    s
}

fn property(name: &str, is_required: bool) -> PropertyNode {
    PropertyNode {
        name: name.to_string(),
        prop_type: "string".to_string(),
        description: None,
        format: None,
        is_required,
        is_nullable: false,
        is_id: false,
        is_array: false,
        min_items: None,
        max_items: None,
        pattern: None,
        min_length: None,
        max_length: None,
        minimum: None,
        maximum: None,
        pg_column_name: name.to_string(),
        pg_column_type: "TEXT".to_string(),
        rust_field_name: name.to_string(),
        rust_field_type: "String".to_string(),
        sea_orm_type: "String".to_string(),
        render_strategy: "scalar".to_string(),
        ref_target: None,
        classification: None,
        projection: None,
        classification_kind: None,
        ui_override_detail: None,
        ui_override_list_cell: None,
        ui_override_form: None,
        ui_override_inline: None,
        type_expr: None,
    }
}

fn classified(
    name: &str,
    required: bool,
    kind: RefClassificationKind,
    ref_target: &str,
    is_array: bool,
) -> PropertyNode {
    let mut p = property(name, required);
    p.is_array = is_array;
    p.ref_target = Some(ref_target.to_string());
    p.classification_kind = Some(kind);
    p
}

/// Representative model:
///
/// ```text
/// CaseType (entity, allOf → CaseBaseType)
///   homeAddress    → AddressType (VO, sibling #1)
///   postalAddress  → AddressType (VO, sibling #2 — same type re-visited)
///   owner          → OrgType (scalar entity ref)
///   status         → StatusCode (codelist w/ UsesCodeList edge)
///   title          → plain required column
///   watchers       → TagType[] (junction array)
/// AddressType (VO)
///   city           → plain column
/// CaseBaseType (entity)
///   baseField      → plain column
/// ```
async fn representative_engine() -> GrafeoEngine {
    let engine = GrafeoEngine::in_memory().unwrap();
    for (title, entity) in [
        ("CaseType", true),
        ("CaseBaseType", true),
        ("AddressType", false),
        ("OrgType", true),
        ("TagType", true),
    ] {
        engine
            .ingest_schema(&schema(title, "common", entity))
            .await
            .unwrap();
    }
    engine
        .ingest_schema(&codelist_schema("StatusCode"))
        .await
        .unwrap();

    for prop in [
        classified(
            "homeAddress",
            false,
            RefClassificationKind::ValueObject,
            "AddressType",
            false,
        ),
        classified(
            "postalAddress",
            false,
            RefClassificationKind::ValueObject,
            "AddressType",
            false,
        ),
        classified(
            "owner",
            false,
            RefClassificationKind::EntityReference,
            "OrgType",
            false,
        ),
        classified(
            "status",
            true,
            RefClassificationKind::CodelistReference,
            "StatusCode",
            false,
        ),
        property("title", true),
        classified(
            "watchers",
            false,
            RefClassificationKind::EntityReference,
            "TagType",
            true,
        ),
    ] {
        engine
            .ingest_property("CaseType", "common/CaseType", &prop)
            .await
            .unwrap();
    }
    engine
        .ingest_property("AddressType", "common/AddressType", &property("city", true))
        .await
        .unwrap();
    engine
        .ingest_property(
            "CaseBaseType",
            "common/CaseBaseType",
            &property("baseField", true),
        )
        .await
        .unwrap();

    // ReferencesSchema edges (prop::Title → domain/Title).
    for (prop, target) in [
        ("homeAddress", "AddressType"),
        ("postalAddress", "AddressType"),
        ("owner", "OrgType"),
        ("status", "StatusCode"),
        ("watchers", "TagType"),
    ] {
        engine
            .ingest_edge(
                &format!("{prop}::CaseType"),
                &format!("common/{target}"),
                EdgeType::ReferencesSchema,
                None,
            )
            .await
            .unwrap();
    }

    // UsesCodeList edge (status → StatusCodeList).
    engine
        .ingest_codelist(&CodeList {
            name: "StatusCodeList".to_string(),
            description: None,
            pg_table_name: "status_code_list".to_string(),
            render_as: "dropdown".to_string(),
            check_expression: None,
        })
        .await
        .unwrap();
    engine
        .ingest_edge(
            "status::CaseType",
            "StatusCodeList",
            EdgeType::UsesCodeList,
            Some(&EdgeProperties {
                render_as: Some("dropdown".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    // allOf extender: CaseType → CaseBaseType.
    engine
        .ingest_edge(
            "CaseType",
            "CaseBaseType",
            EdgeType::ExtendsSchema,
            Some(&EdgeProperties {
                composition_type: Some("allOf".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    // Junction array lowering: watchers → ItemsOf → TagType (the target
    // resolution path used by push_junction_array_child).
    engine
        .ingest_edge(
            "watchers::CaseType",
            "common/TagType",
            EdgeType::ItemsOf,
            None,
        )
        .await
        .unwrap();

    engine
}

#[tokio::test]
async fn composition_tree_query_count_pin() {
    // QUERY_COUNT is process-global: hold the counter lock so no
    // concurrent in-crate read test lands inside the measurement.
    let _counter = COUNTER_LOCK.lock().await;
    let engine = representative_engine().await;

    reset_query_count();
    let tree = engine.get_composition_tree("CaseType").await.unwrap();

    // Structural sanity: the fixture exercises every branch.
    assert_eq!(tree.root.schema_title, "CaseType");
    // 4 scalar/VO columns (homeAddress + postalAddress fold into children;
    // watchers folds into a junction child; owner/status/title remain).
    assert_eq!(tree.root.columns.len(), 3, "owner/status/title columns");
    assert_eq!(tree.root.jsonb_columns.len(), 0);
    // 2 VO children + 1 junction child + 1 allOf child.
    assert_eq!(tree.root.children.len(), 4);
    let vo_children = tree
        .root
        .children
        .iter()
        .filter(|c| c.schema_title == "AddressType")
        .count();
    assert_eq!(vo_children, 2, "sibling VOs each get their own child");
    let case_base = tree
        .root
        .children
        .iter()
        .find(|c| c.schema_title == "CaseBaseType")
        .expect("allOf extender child");
    assert_eq!(case_base.columns.len(), 1);

    // PIN: measured GQL executions for this tree build. See the module
    // docs before touching this number. Baseline before the memoized
    // loadout (issue #389): 58. #446 (scalar entity references are
    // never demoted to the jsonb plane) skips the target-schema lookup
    // for the fixture's one scalar entity ref: 47 → 46.
    assert_eq!(query_count(), 46);
}

/// Issue #350: the batch ref-target resolution must return identical
/// data to the N single-key `get_property_ref_target` calls it
/// replaces (same keys, same targets, including `None` for the
/// negative space). Executes GQL, so it holds the counter lock like
/// every other in-crate read test.
#[tokio::test]
async fn list_all_property_ref_targets_matches_point_lookups() {
    let _counter = COUNTER_LOCK.lock().await;
    let engine = representative_engine().await;

    let batch = engine.list_all_property_ref_targets().await.unwrap();
    for (pname, stitle) in [
        ("homeAddress", "CaseType"),
        ("postalAddress", "CaseType"),
        ("owner", "CaseType"),
        ("status", "CaseType"),
        ("watchers", "CaseType"),
    ] {
        let point = engine.get_property_ref_target(pname, stitle).await.unwrap();
        assert_eq!(
            batch
                .get(&(pname.to_string(), stitle.to_string()))
                .cloned()
                .flatten(),
            point,
            "{pname}/{stitle}: batch must match the point lookup"
        );
    }
    // Negative space: `title` has no reference edge — the batch carries
    // None so the point query never runs for it again.
    let plain = engine
        .get_property_ref_target("title", "CaseType")
        .await
        .unwrap();
    assert_eq!(plain, None);
    assert_eq!(
        batch
            .get(&("title".to_string(), "CaseType".to_string()))
            .cloned()
            .flatten(),
        None
    );
    // The property universe: 6 CaseType props + 1 AddressType + 1
    // CaseBaseType.
    assert_eq!(batch.len(), 8);
}
