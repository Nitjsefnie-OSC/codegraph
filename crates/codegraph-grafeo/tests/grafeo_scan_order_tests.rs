//! Issue #452: the grafeo querier boundary returns total orders, not raw
//! FxHashMap scan order. Each probe ingests in a shuffled (non-sorted)
//! sequence and pins the documented key for the method:
//!
//! * `list_codelists` — by `name`
//! * `list_all_schema_references` — by `(source, target)`
//! * `get_parent_candidates` — by `(child, parent, field)`

use codegraph_core::traits::{GraphIngestor, GraphQuerier};
use codegraph_core::types::*;
use codegraph_grafeo::GrafeoEngine;

fn make_schema(title: &str, domain: &str, is_entity: bool) -> SchemaNode {
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

fn make_property(name: &str, is_array: bool) -> PropertyNode {
    PropertyNode {
        name: name.to_string(),
        prop_type: "string".to_string(),
        description: None,
        format: None,
        is_required: true,
        is_nullable: false,
        is_id: false,
        is_array,
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

#[tokio::test]
async fn list_codelists_returns_name_sorted_total_order() {
    let engine = GrafeoEngine::in_memory().unwrap();
    for name in [
        "ZebraCodeList",
        "MidCodeList",
        "AlphaCodeList",
        "BetaCodeList",
    ] {
        let codelist = CodeList {
            name: name.to_string(),
            description: None,
            pg_table_name: name.to_lowercase(),
            render_as: "dropdown".to_string(),
            check_expression: None,
        };
        engine.ingest_codelist(&codelist).await.unwrap();
    }

    let codelists = engine.list_codelists().await.unwrap();
    let names: Vec<&str> = codelists.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "AlphaCodeList",
            "BetaCodeList",
            "MidCodeList",
            "ZebraCodeList"
        ]
    );
}

#[tokio::test]
async fn list_all_schema_references_returns_pair_sorted_total_order() {
    let engine = GrafeoEngine::in_memory().unwrap();
    for (title, domain) in [
        ("ZedType", "alpha"),
        ("StudentType", "school"),
        ("AlphaType", "alpha"),
        ("CourseType", "school"),
    ] {
        engine
            .ingest_schema(&make_schema(title, domain, true))
            .await
            .unwrap();
    }

    let domain_of: &[(&str, &str)] = &[
        ("ZedType", "alpha"),
        ("StudentType", "school"),
        ("AlphaType", "alpha"),
        ("CourseType", "school"),
    ];
    let schema_id = |title: &str| {
        let domain = domain_of
            .iter()
            .find(|(t, _)| *t == title)
            .map(|(_, d)| *d)
            .unwrap_or_default();
        format!("{domain}/{title}")
    };

    // Shuffled (source, target) insertion sequence.
    let refs = [
        ("StudentType", "CourseType"),
        ("ZedType", "AlphaType"),
        ("CourseType", "AlphaType"),
    ];
    for (src, tgt) in refs {
        let mut prop = make_property(&tgt.to_lowercase(), false);
        prop.ref_target = Some(tgt.to_string());
        engine
            .ingest_property(src, &format!("ref/{src}"), &prop)
            .await
            .unwrap();
        engine
            .ingest_edge(
                &format!("{}::{src}", tgt.to_lowercase()),
                &schema_id(tgt),
                EdgeType::ReferencesSchema,
                None,
            )
            .await
            .unwrap();
    }

    let references = engine.list_all_schema_references().await.unwrap();
    let pairs: Vec<(String, String)> = references;
    assert_eq!(
        pairs,
        [
            ("CourseType".to_string(), "AlphaType".to_string()),
            ("StudentType".to_string(), "CourseType".to_string()),
            ("ZedType".to_string(), "AlphaType".to_string()),
        ]
    );
}

#[tokio::test]
async fn get_parent_candidates_returns_child_parent_field_sorted_total_order() {
    let engine = GrafeoEngine::in_memory().unwrap();
    for (title, domain) in [
        ("CheetahType", "zoo"),
        ("AlphaType", "zoo"),
        ("BetaType", "zoo"),
        ("ZedType", "zoo"),
        ("AardvarkType", "zoo"),
        ("MidType", "zoo"),
        ("OrgType", "zoo"),
        ("MemberType", "zoo"),
    ] {
        engine
            .ingest_schema(&make_schema(title, domain, true))
            .await
            .unwrap();
    }

    // Scalar-ref candidates, inserted in a shuffled sequence.
    let scalar = [
        ("CheetahType", "AlphaType", "alpha_ref"),
        ("BetaType", "ZedType", "zed_ref"),
        ("AardvarkType", "MidType", "mid_ref"),
    ];
    for (child, parent, field) in scalar {
        let mut prop = make_property(field, false);
        prop.ref_target = Some(parent.to_string());
        engine
            .ingest_property(child, &format!("ref/{child}"), &prop)
            .await
            .unwrap();
        engine
            .ingest_edge(
                &format!("{field}::{child}"),
                &format!("zoo/{parent}"),
                EdgeType::ReferencesSchema,
                None,
            )
            .await
            .unwrap();
    }

    // Array-items candidate: OrgType.members -> MemberType (ItemsOf).
    let mut members = make_property("members", true);
    members.ref_target = Some("MemberType".to_string());
    engine
        .ingest_property("OrgType", "ref/OrgType", &members)
        .await
        .unwrap();
    engine
        .ingest_edge(
            "members::OrgType",
            "zoo/MemberType",
            EdgeType::ItemsOf,
            None,
        )
        .await
        .unwrap();

    let candidates = engine.get_parent_candidates().await.unwrap();
    let keys: Vec<(String, String, String)> = candidates
        .iter()
        .map(|c| {
            (
                c.child_title.clone(),
                c.parent_title.clone(),
                c.field_name.clone(),
            )
        })
        .collect();
    assert_eq!(
        keys,
        [
            (
                "AardvarkType".to_string(),
                "MidType".to_string(),
                "mid_ref".to_string()
            ),
            (
                "BetaType".to_string(),
                "ZedType".to_string(),
                "zed_ref".to_string()
            ),
            (
                "CheetahType".to_string(),
                "AlphaType".to_string(),
                "alpha_ref".to_string()
            ),
            (
                "MemberType".to_string(),
                "OrgType".to_string(),
                "members".to_string()
            ),
        ]
    );
}
