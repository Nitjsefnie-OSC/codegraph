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

fn make_property(name: &str, is_required: bool) -> PropertyNode {
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

async fn seeded_engine() -> GrafeoEngine {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("PersonType", "common", true))
        .await
        .unwrap();
    engine
        .ingest_schema(&make_schema("AddressType", "common", false))
        .await
        .unwrap();
    engine
        .ingest_schema(&make_schema("PayRunType", "payroll", true))
        .await
        .unwrap();
    engine
}

// --- Task 5: Schema queries ---

#[tokio::test]
async fn test_get_schema() {
    let engine = seeded_engine().await;
    let found = engine.get_schema("PersonType").await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().title, "PersonType");

    let missing = engine.get_schema("NoSuchType").await.unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn test_list_schemas_all() {
    let engine = seeded_engine().await;
    let all = engine.list_schemas(None).await.unwrap();
    assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn test_list_schemas_by_domain() {
    let engine = seeded_engine().await;
    let common = engine.list_schemas(Some("common")).await.unwrap();
    assert_eq!(common.len(), 2);
    let payroll = engine.list_schemas(Some("payroll")).await.unwrap();
    assert_eq!(payroll.len(), 1);
}

#[tokio::test]
async fn test_get_entity_names() {
    let engine = seeded_engine().await;
    let mut names = engine.get_entity_names().await.unwrap();
    names.sort();
    assert_eq!(names, vec!["PayRunType", "PersonType"]);
}

#[tokio::test]
async fn test_get_entity_schema_map() {
    let engine = seeded_engine().await;
    let map = engine.get_entity_schema_map().await.unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(map.get("PersonType").unwrap(), "common/PersonType.json");
}

#[tokio::test]
async fn test_get_value_object_schemas() {
    let engine = seeded_engine().await;
    let vos = engine.get_value_object_schemas().await.unwrap();
    assert_eq!(vos.len(), 1);
    assert_eq!(vos[0].title, "AddressType");
}

#[tokio::test]
async fn test_get_child_schemas() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("PersonType", "common", true))
        .await
        .unwrap();

    let mut child = make_schema("PersonNameType", "common", false);
    child.parent_schema = Some("PersonType".to_string());
    engine.ingest_schema(&child).await.unwrap();

    let children = engine.get_child_schemas("PersonType").await.unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].title, "PersonNameType");
}

#[tokio::test]
async fn test_get_child_schemas_derives_refers_children() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("TodoListType", "common", true))
        .await
        .unwrap();
    engine
        .ingest_schema(&make_schema("TodoItemType", "common", true))
        .await
        .unwrap();
    engine
        .ingest_schema(&make_schema("PriorityCode", "common", false))
        .await
        .unwrap();

    // mox refers lowering: array-of-entity-ref property + ItemsOf edge to
    // the target entity (edge id convention `{prop}::{schema_title}`).
    let mut prop = make_property("items", false);
    prop.is_array = true;
    prop.ref_target = Some("TodoItemType".to_string());
    engine
        .ingest_property("TodoListType", "common/TodoListType", &prop)
        .await
        .unwrap();
    engine
        .ingest_edge(
            "items::TodoListType",
            "common/TodoItemType",
            EdgeType::ItemsOf,
            None,
        )
        .await
        .unwrap();

    // Codelist arrays are ItemsOf-lowered too, but never children.
    let mut codelist_prop = make_property("priorities", false);
    codelist_prop.is_array = true;
    codelist_prop.ref_target = Some("PriorityCode".to_string());
    engine
        .ingest_property("TodoListType", "common/TodoListType", &codelist_prop)
        .await
        .unwrap();
    engine
        .ingest_edge(
            "priorities::TodoListType",
            "common/PriorityCode",
            EdgeType::ItemsOf,
            None,
        )
        .await
        .unwrap();

    let children = engine.get_child_schemas("TodoListType").await.unwrap();
    let titles: Vec<_> = children.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, vec!["TodoItemType"]);

    // Reverse direction: the referenced entity has no children of its own.
    let reverse = engine.get_child_schemas("TodoItemType").await.unwrap();
    assert!(reverse.is_empty());
}

#[tokio::test]
async fn test_get_child_schemas_excludes_self_reference() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("NodeType", "common", true))
        .await
        .unwrap();

    let mut prop = make_property("children", false);
    prop.is_array = true;
    prop.ref_target = Some("NodeType".to_string());
    engine
        .ingest_property("NodeType", "common/NodeType", &prop)
        .await
        .unwrap();
    engine
        .ingest_edge(
            "children::NodeType",
            "common/NodeType",
            EdgeType::ItemsOf,
            None,
        )
        .await
        .unwrap();

    let children = engine.get_child_schemas("NodeType").await.unwrap();
    assert!(children.is_empty());
}

#[tokio::test]
async fn test_get_child_schemas_dedupes_inline_and_derived() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("PersonType", "common", true))
        .await
        .unwrap();

    let mut child = make_schema("ItemType", "common", true);
    child.parent_schema = Some("PersonType".to_string());
    engine.ingest_schema(&child).await.unwrap();

    let mut prop = make_property("items", false);
    prop.is_array = true;
    prop.ref_target = Some("ItemType".to_string());
    engine
        .ingest_property("PersonType", "common/PersonType", &prop)
        .await
        .unwrap();
    engine
        .ingest_edge(
            "items::PersonType",
            "common/ItemType",
            EdgeType::ItemsOf,
            None,
        )
        .await
        .unwrap();

    let children = engine.get_child_schemas("PersonType").await.unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].title, "ItemType");
}

// --- Task 6: Property, codelist, composite queries ---

#[tokio::test]
async fn test_get_properties() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("PersonType", "common", true))
        .await
        .unwrap();
    engine
        .ingest_property(
            "PersonType",
            "test/PersonType",
            &make_property("givenName", true),
        )
        .await
        .unwrap();
    engine
        .ingest_property(
            "PersonType",
            "test/PersonType",
            &make_property("familyName", true),
        )
        .await
        .unwrap();

    let props = engine.get_properties("PersonType").await.unwrap();
    assert_eq!(props.len(), 2);
}

#[tokio::test]
async fn test_get_codelist_and_enum_values() {
    let engine = GrafeoEngine::in_memory().unwrap();
    let codelist = CodeList {
        name: "GenderCodeList".to_string(),
        description: None,
        pg_table_name: "gender_code_list".to_string(),
        render_as: "dropdown".to_string(),
        check_expression: None,
    };
    engine.ingest_codelist(&codelist).await.unwrap();

    let v1 = EnumValue {
        value: "Male".to_string(),
        display_name: Some("Male".to_string()),
        sort_order: 1,
    };
    let v2 = EnumValue {
        value: "Female".to_string(),
        display_name: Some("Female".to_string()),
        sort_order: 2,
    };
    engine
        .ingest_enum_value("GenderCodeList", &v1)
        .await
        .unwrap();
    engine
        .ingest_enum_value("GenderCodeList", &v2)
        .await
        .unwrap();

    let codelists = engine.list_codelists().await.unwrap();
    assert_eq!(codelists.len(), 1);

    let found = engine.get_codelist("GenderCodeList").await.unwrap();
    assert!(found.is_some());

    let values = engine.get_enum_values("GenderCodeList").await.unwrap();
    assert_eq!(values.len(), 2);
}

#[tokio::test]
async fn test_get_property_ref_target() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("PersonType", "common", true))
        .await
        .unwrap();
    engine
        .ingest_schema(&make_schema("AddressType", "common", false))
        .await
        .unwrap();

    let mut prop = make_property("address", true);
    prop.ref_target = Some("AddressType".to_string());
    engine
        .ingest_property("PersonType", "test/PersonType", &prop)
        .await
        .unwrap();

    engine
        .ingest_edge(
            "address::PersonType",
            "common/AddressType",
            EdgeType::ReferencesSchema,
            Some(&EdgeProperties {
                ref_path: Some("#/definitions/AddressType".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    let target = engine
        .get_property_ref_target("address", "PersonType")
        .await
        .unwrap();
    assert!(target.is_some());
    assert_eq!(target.unwrap().title, "AddressType");
}

#[tokio::test]
async fn test_get_codelist_for_property() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("PersonType", "common", true))
        .await
        .unwrap();
    engine
        .ingest_property(
            "PersonType",
            "test/PersonType",
            &make_property("gender", true),
        )
        .await
        .unwrap();

    let codelist = CodeList {
        name: "GenderCodeList".to_string(),
        description: None,
        pg_table_name: "gender_code_list".to_string(),
        render_as: "dropdown".to_string(),
        check_expression: None,
    };
    engine.ingest_codelist(&codelist).await.unwrap();

    engine
        .ingest_edge(
            "gender::PersonType",
            "GenderCodeList",
            EdgeType::UsesCodeList,
            Some(&EdgeProperties {
                render_as: Some("dropdown".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    let result = engine
        .get_codelist_for_property("gender", "PersonType")
        .await
        .unwrap();
    assert!(result.is_some());
    let (cl, render_as) = result.unwrap();
    assert_eq!(cl.name, "GenderCodeList");
    assert_eq!(render_as, "dropdown");
}

#[tokio::test]
async fn test_get_array_item_schema() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("PersonType", "common", true))
        .await
        .unwrap();
    engine
        .ingest_schema(&make_schema("PersonNameType", "common", false))
        .await
        .unwrap();

    let mut prop = make_property("names", false);
    prop.is_array = true;
    engine
        .ingest_property("PersonType", "test/PersonType", &prop)
        .await
        .unwrap();

    engine
        .ingest_edge(
            "names::PersonType",
            "common/PersonNameType",
            EdgeType::ItemsOf,
            None,
        )
        .await
        .unwrap();

    let item = engine
        .get_array_item_schema("names", "PersonType")
        .await
        .unwrap();
    assert!(item.is_some());
    assert_eq!(item.unwrap().title, "PersonNameType");
}

#[tokio::test]
async fn test_get_composite_columns() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("PersonType", "common", true))
        .await
        .unwrap();
    engine
        .ingest_property(
            "PersonType",
            "test/PersonType",
            &make_property("name", true),
        )
        .await
        .unwrap();

    let col = CompositeColumn {
        suffix: "_code".to_string(),
        pg_type: "TEXT".to_string(),
        rust_type: "String".to_string(),
        sea_orm_type: "String".to_string(),
        fk_target: None,
        dto_rust_type: None,
        wrapper_schema: "AmountType".into(),
    };
    engine.ingest_composite_column(&col).await.unwrap();
    engine
        .ingest_edge(
            "name::PersonType",
            "_code::AmountType",
            EdgeType::ExpandsTo,
            Some(&EdgeProperties {
                sort_order: Some(1),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    let cols = engine
        .get_composite_columns("name", "PersonType")
        .await
        .unwrap();
    assert_eq!(cols.len(), 1);
    assert_eq!(cols[0].suffix, "_code");
}

#[tokio::test]
async fn test_get_composite_range_and_consumed_fields() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("EffectivePeriod", "common", false))
        .await
        .unwrap();
    engine
        .ingest_property(
            "EffectivePeriod",
            "test/EffectivePeriod",
            &make_property("startDate", true),
        )
        .await
        .unwrap();
    engine
        .ingest_property(
            "EffectivePeriod",
            "test/EffectivePeriod",
            &make_property("endDate", false),
        )
        .await
        .unwrap();

    let range = CompositeRange {
        pg_column_name: "effective_period".to_string(),
        pg_type: "DATERANGE".to_string(),
        rust_type: "DateRange".to_string(),
        start_field: "startDate".to_string(),
        end_field: "endDate".to_string(),
        open_end: true,
    };
    engine.ingest_composite_range(&range).await.unwrap();
    engine
        .ingest_edge(
            "EffectivePeriod",
            "effective_period",
            EdgeType::CollapsesTo,
            None,
        )
        .await
        .unwrap();
    engine
        .ingest_edge(
            "effective_period",
            "startDate::EffectivePeriod",
            EdgeType::ConsumesField,
            Some(&EdgeProperties {
                role: Some("start".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    let found_range = engine.get_composite_range("EffectivePeriod").await.unwrap();
    assert!(found_range.is_some());
    assert_eq!(found_range.unwrap().pg_column_name, "effective_period");

    let consumed = engine.get_consumed_fields("EffectivePeriod").await.unwrap();
    assert_eq!(consumed.len(), 1);
    assert_eq!(consumed[0].0.name, "startDate");
    assert_eq!(consumed[0].1, "start");
}

// --- Task 7: Graph traversal and discovery ---

#[tokio::test]
async fn test_get_allof_targets() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("PersonType", "common", true))
        .await
        .unwrap();
    engine
        .ingest_schema(&make_schema("PersonBaseType", "common", false))
        .await
        .unwrap();

    engine
        .ingest_edge(
            "PersonType",
            "PersonBaseType",
            EdgeType::ExtendsSchema,
            Some(&EdgeProperties {
                composition_type: Some("allOf".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    let targets = engine.get_allof_targets("PersonType").await.unwrap();
    assert_eq!(targets, vec!["PersonBaseType"]);
}

#[tokio::test]
async fn test_get_referencing_and_referenced_schemas() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("PersonType", "common", true))
        .await
        .unwrap();
    engine
        .ingest_schema(&make_schema("AddressType", "common", false))
        .await
        .unwrap();
    engine
        .ingest_property(
            "PersonType",
            "test/PersonType",
            &make_property("address", true),
        )
        .await
        .unwrap();

    engine
        .ingest_edge(
            "address::PersonType",
            "common/AddressType",
            EdgeType::ReferencesSchema,
            Some(&EdgeProperties {
                ref_path: Some("AddressType".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    let referenced = engine.get_referenced_schemas("PersonType").await.unwrap();
    assert_eq!(referenced.len(), 1);
    assert_eq!(referenced[0].title, "AddressType");

    let referencing = engine.get_referencing_schemas("AddressType").await.unwrap();
    assert_eq!(referencing, vec!["PersonType"]);
}

#[tokio::test]
async fn test_get_generation_order() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("PersonType", "common", true))
        .await
        .unwrap();
    engine
        .ingest_schema(&make_schema("PayRunType", "payroll", true))
        .await
        .unwrap();

    // PayRunType depends on PersonType
    engine
        .ingest_edge(
            "PayRunType",
            "PersonType",
            EdgeType::DependsOn,
            Some(&EdgeProperties {
                dependency_type: Some("ref".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    let order = engine.get_generation_order().await.unwrap();
    let person_idx = order.iter().position(|t| t == "PersonType").unwrap();
    let payrun_idx = order.iter().position(|t| t == "PayRunType").unwrap();
    assert!(
        person_idx < payrun_idx,
        "PersonType should come before PayRunType in topo order"
    );
}

#[tokio::test]
async fn test_get_parent_candidates() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("PersonType", "common", true))
        .await
        .unwrap();
    engine
        .ingest_schema(&make_schema("PersonNameType", "common", true))
        .await
        .unwrap();

    let prop = make_property("person", true);
    engine
        .ingest_property("PersonNameType", "test/PersonNameType", &prop)
        .await
        .unwrap();

    engine
        .ingest_edge(
            "person::PersonNameType",
            "common/PersonType",
            EdgeType::ReferencesSchema,
            Some(&EdgeProperties {
                ref_path: Some("PersonType".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    let candidates = engine.get_parent_candidates().await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].child_title, "PersonNameType");
    assert_eq!(candidates[0].parent_title, "PersonType");
    assert_eq!(candidates[0].field_name, "person");
}

#[tokio::test]
async fn test_get_required_extensions() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("PersonType", "common", true))
        .await
        .unwrap();

    // Insert Extension node directly via GQL
    let session = engine.db().session();
    session
        .execute("INSERT (:Extension {name: 'audit'})")
        .unwrap();

    engine
        .ingest_edge("PersonType", "audit", EdgeType::RequiresExtension, None)
        .await
        .unwrap();

    let extensions = engine.get_required_extensions("PersonType").await.unwrap();
    assert_eq!(extensions.len(), 1);
    assert_eq!(extensions[0].name, "audit");
}

#[tokio::test]
async fn test_get_classification_data() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&make_schema("PersonType", "common", true))
        .await
        .unwrap();
    engine
        .ingest_property(
            "PersonType",
            "test/PersonType",
            &make_property("givenName", true),
        )
        .await
        .unwrap();
    engine
        .ingest_property(
            "PersonType",
            "test/PersonType",
            &make_property("familyName", false),
        )
        .await
        .unwrap();

    let data = engine.get_classification_data().await.unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0].title, "PersonType");
    assert_eq!(data[0].field_count, 2);
    assert_eq!(data[0].required_field_count, 1);
    assert_eq!(data[0].schema_type, "object");
    assert!(!data[0].is_enum);
    assert!(!data[0].is_string_type);
}

// --- Task 8: Composition tree ---

#[tokio::test]
async fn test_get_composition_tree_not_found() {
    let engine = GrafeoEngine::in_memory().unwrap();
    let result = engine.get_composition_tree("NoSuchType").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_get_composition_tree_simple() {
    let engine = GrafeoEngine::in_memory().unwrap();
    let schema = make_schema("PersonType", "common", true);
    engine.ingest_schema(&schema).await.unwrap();
    engine
        .ingest_property(
            "PersonType",
            "test/PersonType",
            &make_property("givenName", true),
        )
        .await
        .unwrap();

    let tree = engine.get_composition_tree("PersonType").await.unwrap();
    assert_eq!(tree.root.schema_title, "PersonType");
    assert_eq!(tree.root.table_name, "PersonType");
    assert!(tree.root.fk.is_none(), "Root should have no FK");
    assert!(!tree.root.columns.is_empty());
    assert!(tree.root.children.is_empty());
}

// --- IFML view components ---

#[tokio::test]
async fn test_view_component_spec_round_trip() {
    let engine = GrafeoEngine::in_memory().unwrap();
    let container = ViewContainerNode {
        name: "CustomerList".to_string(),
        label: Some("Customers".to_string()),
        is_xor: false,
        is_default: false,
        is_landmark: true,
        is_modal: false,
        conditional_expression: None,
        expr_json: None,
        domain: Some("sales".to_string()),
        module_uses: None,
        roles: None,
        requires: None,
    };
    engine.ingest_view_container(&container).await.unwrap();

    let component = ViewComponentNode {
        name: "grid".to_string(),
        component_type: "list".to_string(),
        mode: None,
        entity: Some("Customer".to_string()),
        fields: Some(vec!["name".to_string(), "email".to_string()]),
        filter: None,
        api_operation: None,
        spec: Some(r#"{"columns":[{"field":"name","sortable":true}]}"#.to_string()),
        conditional_expression: None,
        expr_json: None,
        domain: Some("sales".to_string()),
    };
    engine.ingest_view_component(&component).await.unwrap();

    engine
        .ingest_edge(
            "vc:CustomerList",
            "comp:grid",
            EdgeType::ContainsViewComponent,
            None,
        )
        .await
        .unwrap();

    let loaded = engine
        .get_ifml_view_components("CustomerList")
        .await
        .unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0], component);
}

#[tokio::test]
async fn test_view_component_spec_absent_round_trip() {
    let engine = GrafeoEngine::in_memory().unwrap();
    let container = ViewContainerNode {
        name: "CustomerList".to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: false,
        is_modal: false,
        conditional_expression: None,
        expr_json: None,
        domain: None,
        module_uses: None,
        roles: None,
        requires: None,
    };
    engine.ingest_view_container(&container).await.unwrap();

    let component = ViewComponentNode {
        name: "grid".to_string(),
        component_type: "form".to_string(),
        mode: None,
        entity: None,
        fields: None,
        filter: None,
        api_operation: None,
        spec: None,
        conditional_expression: None,
        expr_json: None,
        domain: None,
    };
    engine.ingest_view_component(&component).await.unwrap();

    engine
        .ingest_edge(
            "vc:CustomerList",
            "comp:grid",
            EdgeType::ContainsViewComponent,
            None,
        )
        .await
        .unwrap();

    let loaded = engine
        .get_ifml_view_components("CustomerList")
        .await
        .unwrap();
    assert_eq!(loaded, vec![component]);
}

#[tokio::test]
async fn test_view_container_module_uses_and_roles_round_trip() {
    let engine = GrafeoEngine::in_memory().unwrap();
    let container = ViewContainerNode {
        name: "Catalog".to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: true,
        is_modal: false,
        conditional_expression: None,
        expr_json: None,
        domain: None,
        module_uses: Some(vec![
            ModuleUseRecord {
                module: "Pagination".to_string(),
                alias: Some("pager".to_string()),
            },
            ModuleUseRecord {
                module: "Footer".to_string(),
                alias: None,
            },
        ]),
        roles: Some(vec!["admin".to_string(), "manager".to_string()]),
        requires: Some(vec![
            "ResolveTicket".to_string(),
            "ApproveRefund".to_string(),
        ]),
    };
    engine.ingest_view_container(&container).await.unwrap();

    let plain = ViewContainerNode {
        name: "Plain".to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: false,
        is_modal: false,
        conditional_expression: None,
        expr_json: None,
        domain: None,
        module_uses: None,
        roles: None,
        requires: None,
    };
    engine.ingest_view_container(&plain).await.unwrap();

    let loaded = engine.get_ifml_view_containers().await.unwrap();
    assert_eq!(loaded.len(), 2);

    let catalog = loaded
        .iter()
        .find(|c| c.name == "Catalog")
        .expect("Catalog container");
    assert_eq!(
        catalog.module_uses,
        Some(vec![
            ModuleUseRecord {
                module: "Pagination".to_string(),
                alias: Some("pager".to_string()),
            },
            ModuleUseRecord {
                module: "Footer".to_string(),
                alias: None,
            },
        ])
    );
    assert_eq!(
        catalog.roles,
        Some(vec!["admin".to_string(), "manager".to_string()])
    );
    assert_eq!(
        catalog.requires,
        Some(vec![
            "ResolveTicket".to_string(),
            "ApproveRefund".to_string()
        ])
    );

    let plain_loaded = loaded
        .iter()
        .find(|c| c.name == "Plain")
        .expect("Plain container");
    assert_eq!(plain_loaded.module_uses, None);
    assert_eq!(plain_loaded.roles, None);
    assert_eq!(plain_loaded.requires, None);
}

// --- Authorization metamodel ---

#[tokio::test]
async fn test_actor_policy_round_trip_and_effective_permits() {
    let engine = GrafeoEngine::in_memory().unwrap();

    let model = ActorPolicyModel {
        actors: vec![
            ActorNode {
                name: "Admin".to_string(),
                kind: Some("human".to_string()),
                extends: None,
                block: Some("core".to_string()),
            },
            ActorNode {
                name: "Manager".to_string(),
                kind: Some("human".to_string()),
                extends: Some("Admin".to_string()),
                block: Some("core".to_string()),
            },
            ActorNode {
                name: "Auditor".to_string(),
                kind: Some("agent".to_string()),
                extends: None,
                block: Some("audit".to_string()),
            },
        ],
        capabilities: vec![
            CapabilityNode {
                name: "approve_expense".to_string(),
                class: "Expense".to_string(),
                block: Some("core".to_string()),
            },
            CapabilityNode {
                name: "view_report".to_string(),
                class: "Report".to_string(),
                block: None,
            },
        ],
        grants: vec![
            GrantEdge {
                actor: "Admin".to_string(),
                capability: "approve_expense".to_string(),
                effect: "permit".to_string(),
                when: None,
                expr_json: None,
                obligations: vec![],
            },
            GrantEdge {
                actor: "Admin".to_string(),
                capability: "view_report".to_string(),
                effect: "permit".to_string(),
                when: Some("admin.verified == true".to_string()),
                expr_json: None,
                obligations: vec!["log_access".to_string()],
            },
            GrantEdge {
                actor: "Manager".to_string(),
                capability: "approve_expense".to_string(),
                effect: "forbid".to_string(),
                when: None,
                expr_json: None,
                obligations: vec![],
            },
        ],
        policy: ActorPolicyNode {
            blocks: vec!["core".to_string(), "audit".to_string()],
            never_both: vec![NeverBothGroup {
                capabilities: vec!["approve_expense".to_string()],
            }],
            purposes: vec!["ExpenseTriage".to_string()],
            delegations: vec![DelegationRecord {
                name: "AutoApprove".to_string(),
                from_actor: "Manager".to_string(),
                to_actor: "Auditor".to_string(),
                purpose: Some("ExpenseTriage".to_string()),
                entries: vec![GrantEdge {
                    actor: "Manager".to_string(),
                    capability: "view_report".to_string(),
                    effect: "permit".to_string(),
                    when: Some("report.draft == true".to_string()),
                    expr_json: None,
                    obligations: vec!["log_access".to_string()],
                }],
            }],
        },
    };
    engine.ingest_actor_policy(&model).await.unwrap();

    let actors = engine.get_actors().await.unwrap();
    assert_eq!(actors.len(), 3);
    let manager = actors.iter().find(|a| a.name == "Manager").unwrap();
    assert_eq!(manager.extends.as_deref(), Some("Admin"));
    assert_eq!(manager.kind.as_deref(), Some("human"));
    let auditor = actors.iter().find(|a| a.name == "Auditor").unwrap();
    assert_eq!(auditor.kind.as_deref(), Some("agent"));

    let capabilities = engine.get_capabilities().await.unwrap();
    assert_eq!(capabilities.len(), 2);
    let approve = capabilities
        .iter()
        .find(|c| c.name == "approve_expense")
        .unwrap();
    assert_eq!(approve.class, "Expense");

    let grants = engine.get_grants().await.unwrap();
    assert_eq!(grants.len(), 3);
    let view_report = grants
        .iter()
        .find(|g| g.capability == "view_report")
        .unwrap();
    assert_eq!(view_report.effect, "permit");
    assert_eq!(view_report.when.as_deref(), Some("admin.verified == true"));
    assert_eq!(view_report.obligations, vec!["log_access".to_string()]);

    let policy = engine
        .get_actor_policy()
        .await
        .unwrap()
        .expect("policy node");
    assert_eq!(policy.blocks, vec!["core".to_string(), "audit".to_string()]);
    assert_eq!(policy.never_both.len(), 1);
    assert_eq!(
        policy.never_both[0].capabilities,
        vec!["approve_expense".to_string()]
    );
    assert_eq!(policy.purposes, vec!["ExpenseTriage".to_string()]);
    assert_eq!(policy.delegations.len(), 1);
    let delegation = &policy.delegations[0];
    assert_eq!(delegation.name, "AutoApprove");
    assert_eq!(delegation.from_actor, "Manager");
    assert_eq!(delegation.to_actor, "Auditor");
    assert_eq!(delegation.purpose.as_deref(), Some("ExpenseTriage"));
    assert_eq!(delegation.entries.len(), 1);
    assert_eq!(delegation.entries[0].actor, "Manager");
    assert_eq!(delegation.entries[0].capability, "view_report");
    assert_eq!(delegation.entries[0].effect, "permit");
    assert_eq!(
        delegation.entries[0].when.as_deref(),
        Some("report.draft == true")
    );
    assert_eq!(
        delegation.entries[0].obligations,
        vec!["log_access".to_string()]
    );

    // Admin: own permits only, when/obligations preserved.
    let admin_permits = engine.effective_permits("Admin").await.unwrap();
    assert_eq!(admin_permits.len(), 2);
    let admin_view = admin_permits
        .iter()
        .find(|p| p.capability == "view_report")
        .unwrap();
    assert_eq!(admin_view.when.as_deref(), Some("admin.verified == true"));
    assert_eq!(admin_view.obligations, vec!["log_access".to_string()]);

    // Manager: inherits view_report permit via extends; forbid wins for
    // approve_expense so the inherited permit is dropped.
    let manager_permits = engine.effective_permits("Manager").await.unwrap();
    assert_eq!(manager_permits.len(), 2);
    let manager_approve = manager_permits
        .iter()
        .find(|p| p.capability == "approve_expense")
        .expect("forbid entry survives");
    assert_eq!(manager_approve.effect, "forbid");
    assert!(
        manager_permits
            .iter()
            .all(|p| p.capability != "approve_expense" || p.effect == "forbid")
    );

    // Auditor: no grants at all.
    let auditor_permits = engine.effective_permits("Auditor").await.unwrap();
    assert!(auditor_permits.is_empty());
}

// --- IFML ingest idempotence ---

/// Re-ingesting a view container with a parameter (as happens when the same
/// .ifml is ingested twice into one graph) must not duplicate the
/// ParameterDefinition node or the HasParameter edge. Wave B note: the
/// generator-side defensive dedupe in
/// `crates/codegraph-generate/src/ifml/querier.rs` (get_view_containers,
/// ~line 305) can be dropped once this holds.
#[tokio::test]
async fn test_parameter_ingest_is_idempotent() {
    let engine = GrafeoEngine::in_memory().unwrap();
    let container = ViewContainerNode {
        name: "RefundRequestForm".to_string(),
        label: None,
        is_xor: false,
        is_default: false,
        is_landmark: false,
        is_modal: false,
        conditional_expression: None,
        expr_json: None,
        domain: Some("refunds".to_string()),
        module_uses: None,
        roles: None,
        requires: None,
    };
    let param = ParameterDefinitionNode {
        name: "id".to_string(),
        direction: "in".to_string(),
        type_ref: "Uuid".to_string(),
        domain: Some("refunds".to_string()),
    };

    for _ in 0..2 {
        engine.ingest_view_container(&container).await.unwrap();
        engine.ingest_parameter_definition(&param).await.unwrap();
        engine
            .ingest_edge(
                "vc:RefundRequestForm",
                "param:id",
                EdgeType::HasParameter,
                None,
            )
            .await
            .unwrap();
    }

    let all = engine.get_ifml_parameters().await.unwrap();
    assert_eq!(
        all.len(),
        1,
        "duplicate ParameterDefinition nodes must collapse: {all:?}"
    );
    let for_view = engine
        .get_parameters_for_view("RefundRequestForm")
        .await
        .unwrap();
    assert_eq!(
        for_view.len(),
        1,
        "duplicate HasParameter edges must collapse: {for_view:?}"
    );
}

// ── Constraint plane (issue #261) ─────────────────────────────────────

fn make_condition(name: &str, owner: &str, kind: ConditionKind) -> ConditionNode {
    ConditionNode {
        name: name.to_string(),
        owner_title: owner.to_string(),
        kind,
        expr_json: None,
        options: Vec::new(),
        definition: None,
        domain: Some("payroll".to_string()),
    }
}

#[tokio::test]
async fn test_condition_round_trip() {
    let engine = seeded_engine().await;
    let expr = serde_json::json!({
        "op": ">=",
        "left": { "field": "amount" },
        "right": { "number": 100 }
    });
    let named = ConditionNode {
        expr_json: Some(serde_json::to_string(&expr).unwrap()),
        definition: Some("amount at least 100".to_string()),
        ..make_condition("amount_floor", "PayRunType", ConditionKind::Condition)
    };
    let one_of = ConditionNode {
        options: vec!["PaymentTypeA".to_string(), "PaymentTypeB".to_string()],
        ..make_condition("PayRunType_one_of", "PayRunType", ConditionKind::OneOf)
    };
    engine
        .ingest_schema(&make_schema("PaymentTypeA", "payroll", false))
        .await
        .unwrap();
    engine
        .ingest_schema(&make_schema("PaymentTypeB", "payroll", false))
        .await
        .unwrap();
    engine.ingest_condition(&named).await.unwrap();
    engine.ingest_condition(&one_of).await.unwrap();

    let for_schema = engine
        .get_conditions_for_schema("PayRunType")
        .await
        .unwrap();
    assert_eq!(for_schema.len(), 2, "both condition nodes must attach");
    let named_back = for_schema
        .iter()
        .find(|c| c.kind == ConditionKind::Condition)
        .expect("named condition missing");
    assert_eq!(named_back.name, "amount_floor");
    assert_eq!(named_back.owner_title, "PayRunType");
    let payload: serde_json::Value = serde_json::from_str(
        named_back
            .expr_json
            .as_deref()
            .expect("expr_json must round-trip"),
    )
    .expect("expr_json must be valid JSON");
    assert_eq!(payload["op"], ">=");

    let one_of_back = for_schema
        .iter()
        .find(|c| c.kind == ConditionKind::OneOf)
        .expect("one_of condition missing");
    assert_eq!(one_of_back.name, "PayRunType_one_of");
    assert_eq!(one_of_back.options, vec!["PaymentTypeA", "PaymentTypeB"]);
    assert!(one_of_back.expr_json.is_none());

    // HasCondition edges actually link Schema → Condition.
    let all = engine.list_conditions().await.unwrap();
    assert_eq!(all.len(), 2);
    assert!(all.iter().all(|c| c.domain.as_deref() == Some("payroll")));

    let other = engine
        .get_conditions_for_schema("PersonType")
        .await
        .unwrap();
    assert!(other.is_empty(), "conditions must not leak across schemas");
}

#[tokio::test]
async fn test_list_conditions_empty_by_default() {
    let engine = seeded_engine().await;
    assert!(engine.list_conditions().await.unwrap().is_empty());
}

/// Issue #279: the access flag and structured annotations round-trip
/// through the grafeo node payload, and legacy nodes without them read
/// back as `None`.
#[tokio::test]
async fn test_schema_access_and_annotations_round_trip() {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine.reinit_schema().unwrap();

    let mut flagged = make_schema("NoticeType", "common", true);
    flagged.access = Some(Access::Public);
    flagged.annotations = Some(vec![Annotation {
        name: "acme.doc.tag".to_string(),
        arguments: vec![
            AnnotationArg::Named {
                name: "since".to_string(),
                value: serde_json::json!("2026-01-01"),
            },
            AnnotationArg::Literal(serde_json::json!(2)),
        ],
    }]);
    engine.ingest_schema(&flagged).await.unwrap();
    engine
        .ingest_schema(&make_schema("PlainType", "common", true))
        .await
        .unwrap();

    let back = engine
        .get_schema("NoticeType")
        .await
        .unwrap()
        .expect("flagged schema present");
    assert_eq!(back.access, Some(Access::Public));
    let annotations = back.annotations.expect("annotations round-trip");
    assert_eq!(annotations[0].name, "acme.doc.tag");
    assert_eq!(annotations[0].arguments.len(), 2);

    let plain = engine
        .get_schema("PlainType")
        .await
        .unwrap()
        .expect("plain schema present");
    assert_eq!(plain.access, None);
    assert_eq!(plain.annotations, None);
}

/// `get_interactions` is operation-scoped via the HasInteraction edge
/// (issue #387): each ApiOperation returns only its own interactions;
/// an unknown operation returns empty. Pinned on BOTH engines so the
/// mock stays in lockstep with Grafeo.
async fn exercise_interactions_scoped<E: GraphIngestor + GraphQuerier + ?Sized>(
    engine: &E,
) -> Result<(), codegraph_core::error::GraphError> {
    let resource = engine
        .ingest_api_resource(&ApiResourceNode {
            name: "candidates".to_string(),
            schema_title: "Candidate".to_string(),
            domain: "recruiting".to_string(),
            label: None,
            path_segment: "candidates".to_string(),
        })
        .await?;
    let op_create = engine
        .ingest_api_operation(&ApiOperationNode {
            name: "create_candidate".to_string(),
            kind: "create".to_string(),
            input_schema: None,
            output_schema: "Candidate".to_string(),
            paging: false,
            sorting: false,
            filtering: false,
            domain: None,
        })
        .await?;
    let op_list = engine
        .ingest_api_operation(&ApiOperationNode {
            name: "list_candidates".to_string(),
            kind: "list".to_string(),
            input_schema: None,
            output_schema: "CandidateList".to_string(),
            paging: true,
            sorting: false,
            filtering: false,
            domain: None,
        })
        .await?;
    let ia_http = engine
        .ingest_interaction(&InteractionNode {
            transport: "http".to_string(),
            domain: Some("recruiting".to_string()),
        })
        .await?;
    let ia_grpc = engine
        .ingest_interaction(&InteractionNode {
            transport: "grpc".to_string(),
            domain: None,
        })
        .await?;

    engine
        .ingest_edge(&resource, &op_create, EdgeType::HasOperation, None)
        .await?;
    engine
        .ingest_edge(&resource, &op_list, EdgeType::HasOperation, None)
        .await?;
    engine
        .ingest_edge(&op_create, &ia_http, EdgeType::HasInteraction, None)
        .await?;
    engine
        .ingest_edge(&op_list, &ia_grpc, EdgeType::HasInteraction, None)
        .await?;

    let create = engine.get_interactions("create_candidate").await?;
    assert_eq!(
        create,
        vec![InteractionNode {
            transport: "http".to_string(),
            domain: Some("recruiting".to_string()),
        }]
    );
    let list = engine.get_interactions("list_candidates").await?;
    assert_eq!(
        list,
        vec![InteractionNode {
            transport: "grpc".to_string(),
            domain: None,
        }]
    );
    assert!(
        engine
            .get_interactions("missing_operation")
            .await?
            .is_empty(),
        "unknown operation returns no interactions"
    );
    Ok(())
}

#[tokio::test]
async fn test_get_interactions_scoped_to_operation_grafeo() {
    let engine = GrafeoEngine::in_memory().unwrap();
    exercise_interactions_scoped(&engine).await.unwrap();
}

#[tokio::test]
async fn test_get_interactions_scoped_to_operation_mock() {
    let engine = codegraph_core::mock::MockEngine::new();
    exercise_interactions_scoped(&engine).await.unwrap();
}

// ── DDD design plane (issue #449) ─────────────────────────────────────

/// Count edges by label through the raw session (the DddBindsClass edge has
/// no querier surface of its own).
fn count_edges(engine: &GrafeoEngine, label: &str) -> usize {
    let session = engine.db().session();
    let result = session
        .execute(&format!("MATCH ()-[e:{label}]->() RETURN count(e) AS cnt"))
        .expect("edge count query");
    result.rows()[0][0].as_int64().expect("integer count") as usize
}

/// A full-featured DddModelGraph: two modules, designs of all three
/// stereotypes with flags, repositories with builtin + declared operations
/// (rex-ir TypeRef/Multiplicity JSON payloads), services with delegation +
/// capabilities, and a search with text/filters/sorts/document/ranking/
/// pagination. Vecs are in canonical order (ordinals, repository names).
fn ddd_fixture() -> DddModelGraph {
    let book_ref = serde_json::json!({
        "type": "class",
        "value": {"package": "nz.example.library", "name": "Book"}
    });
    let loan_ref = serde_json::json!({
        "type": "class",
        "value": {"package": "nz.example.library", "name": "Loan"}
    });
    DddModelGraph {
        source_path: "model/library.ddd".to_string(),
        application: DddApplicationNode {
            name: "Library".to_string(),
            base: Some("nz.example.library".to_string()),
            source_path: "model/library.ddd".to_string(),
        },
        modules: vec![
            DddModuleNode {
                application: "Library".to_string(),
                name: "catalogue".to_string(),
                ordinal: 0,
            },
            DddModuleNode {
                application: "Library".to_string(),
                name: "lending".to_string(),
                ordinal: 1,
            },
        ],
        designs: vec![
            DddDesignNode {
                application: "Library".to_string(),
                module: "catalogue".to_string(),
                class: "Book".to_string(),
                resolved_title: Some("Book".to_string()),
                stereotype: "entity".to_string(),
                is_abstract: false,
                flags: DddDesignFlags {
                    scaffold: true,
                    cache: true,
                    ..Default::default()
                },
                ordinal: 0,
            },
            DddDesignNode {
                application: "Library".to_string(),
                module: "catalogue".to_string(),
                class: "Money".to_string(),
                resolved_title: None,
                stereotype: "value".to_string(),
                is_abstract: false,
                flags: DddDesignFlags::default(),
                ordinal: 1,
            },
            DddDesignNode {
                application: "Library".to_string(),
                module: "catalogue".to_string(),
                class: "LoanSummary".to_string(),
                resolved_title: None,
                stereotype: "dto".to_string(),
                is_abstract: true,
                flags: DddDesignFlags {
                    auditable: true,
                    optimistic_locking: true,
                    non_persistent: true,
                    ..Default::default()
                },
                ordinal: 2,
            },
            DddDesignNode {
                application: "Library".to_string(),
                module: "lending".to_string(),
                class: "Loan".to_string(),
                resolved_title: None,
                stereotype: "entity".to_string(),
                is_abstract: false,
                flags: DddDesignFlags {
                    scaffold: true,
                    ..Default::default()
                },
                ordinal: 0,
            },
        ],
        repositories: vec![
            DddRepositoryNode {
                application: "Library".to_string(),
                name: "BookRepository".to_string(),
                design_class: "Book".to_string(),
                operations: vec![
                    DddRepositoryOperation {
                        name: "findById".to_string(),
                        builtin: Some("findById".to_string()),
                        return_type: None,
                        return_multiplicity: None,
                        params: vec![],
                        ordinal: 0,

                        is_protected: false,
                    },
                    DddRepositoryOperation {
                        name: "findAll".to_string(),
                        builtin: Some("findAll".to_string()),
                        return_type: None,
                        return_multiplicity: None,
                        params: vec![],
                        ordinal: 1,

                        is_protected: false,
                    },
                    DddRepositoryOperation {
                        name: "save".to_string(),
                        builtin: Some("save".to_string()),
                        return_type: None,
                        return_multiplicity: None,
                        params: vec![],
                        ordinal: 2,

                        is_protected: false,
                    },
                    DddRepositoryOperation {
                        name: "delete".to_string(),
                        builtin: Some("delete".to_string()),
                        return_type: None,
                        return_multiplicity: None,
                        params: vec![],
                        ordinal: 3,

                        is_protected: false,
                    },
                    DddRepositoryOperation {
                        name: "findByTitle".to_string(),
                        builtin: None,
                        return_type: Some(serde_json::json!({
                            "type": "primitive",
                            "value": "String"
                        })),
                        return_multiplicity: None,
                        params: vec![DddParam {
                            name: "title".to_string(),
                            type_json: serde_json::json!({
                                "type": "primitive",
                                "value": "String"
                            }),
                            multiplicity: None,
                        }],
                        ordinal: 4,

                        is_protected: false,
                    },
                ],
            },
            DddRepositoryNode {
                application: "Library".to_string(),
                name: "LoanRepository".to_string(),
                design_class: "Loan".to_string(),
                operations: vec![
                    DddRepositoryOperation {
                        name: "save".to_string(),
                        builtin: Some("save".to_string()),
                        return_type: None,
                        return_multiplicity: None,
                        params: vec![],
                        ordinal: 0,

                        is_protected: false,
                    },
                    DddRepositoryOperation {
                        name: "renewLoan".to_string(),
                        builtin: None,
                        return_type: Some(loan_ref.clone()),
                        return_multiplicity: Some(serde_json::json!({"type": "many"})),
                        params: vec![DddParam {
                            name: "loan".to_string(),
                            type_json: loan_ref,
                            multiplicity: Some(serde_json::json!({"type": "one"})),
                        }],
                        ordinal: 1,

                        is_protected: false,
                    },
                ],
            },
        ],
        services: vec![
            DddServiceNode {
                application: "Library".to_string(),
                module: "catalogue".to_string(),
                name: "LoanService".to_string(),
                description: Some("Manages loans".to_string()),
                dependencies: vec![
                    "LoanRepository".to_string(),
                    "NotificationService".to_string(),
                ],
                operations: vec![
                    DddServiceOperation {
                        name: "borrow".to_string(),
                        return_type: Some(serde_json::json!({
                            "type": "primitive",
                            "value": "Boolean"
                        })),
                        return_multiplicity: None,
                        params: vec![DddParam {
                            name: "book".to_string(),
                            type_json: book_ref,
                            multiplicity: None,
                        }],
                        delegation_target: None,
                        delegation_operation: None,
                        capabilities: vec!["BorrowBooks".to_string()],
                        ordinal: 0,

                        is_protected: false,
                    },
                    DddServiceOperation {
                        name: "renew".to_string(),
                        return_type: None,
                        return_multiplicity: None,
                        params: vec![],
                        delegation_target: Some("LoanRepository".to_string()),
                        delegation_operation: Some("save".to_string()),
                        capabilities: vec![],
                        ordinal: 1,

                        is_protected: false,
                    },
                ],
                ordinal: 0,
            },
            DddServiceNode {
                application: "Library".to_string(),
                module: "lending".to_string(),
                name: "ReturnService".to_string(),
                description: None,
                dependencies: vec!["LoanRepository".to_string()],
                operations: vec![DddServiceOperation {
                    name: "returnLoan".to_string(),
                    return_type: None,
                    return_multiplicity: None,
                    params: vec![],
                    delegation_target: Some("LoanRepository".to_string()),
                    delegation_operation: Some("save".to_string()),
                    capabilities: vec![],
                    ordinal: 0,

                    is_protected: false,
                }],
                ordinal: 0,
            },
        ],
        searches: vec![DddSearchNode {
            application: "Library".to_string(),
            module: "lending".to_string(),
            name: "BookSearch".to_string(),
            description: Some("Full-text catalogue search".to_string()),
            entity_class: "Book".to_string(),
            entity_title: Some("Book".to_string()),
            text: vec![
                DddSearchField {
                    property: "title".to_string(),
                    boost: Some(2.0),
                    analyzer: Some("standard".to_string()),
                },
                DddSearchField {
                    property: "synopsis".to_string(),
                    boost: None,
                    analyzer: None,
                },
            ],
            filters: vec!["category".to_string()],
            sorts: vec!["title".to_string()],
            document: vec![DddDocumentField {
                name: "label".to_string(),
                expr: r#"title + " - " + synopsis"#.to_string(),
            }],
            ranking: Some("recency".to_string()),
            analyzer: Some("english".to_string()),
            pagination: Some(DddPagination {
                limit: Some(20),
                max_limit: Some(100),
                cursor: true,
            }),
            capabilities: vec!["SearchBooks".to_string()],
            ordinal: 0,
        }],
    }
}

#[tokio::test]
async fn test_ddd_model_round_trip() {
    let engine = GrafeoEngine::in_memory().unwrap();

    // The schema the Book design / BookSearch resolve against; Money,
    // LoanSummary and Loan stay unresolved (advisory edges skip silently).
    engine
        .ingest_schema(&make_schema("Book", "library", true))
        .await
        .unwrap();

    let fixture = ddd_fixture();
    engine.ingest_ddd_model(&fixture).await.unwrap();

    let loaded = engine.get_ddd_models().await.unwrap();
    assert_eq!(loaded.len(), 1, "one model per application");
    assert_eq!(loaded[0], fixture, "deep round-trip, canonical order");

    // Structural edges land as declared.
    assert_eq!(count_edges(&engine, "DddHasModule"), 2);
    assert_eq!(count_edges(&engine, "DddHasDesign"), 4);
    assert_eq!(count_edges(&engine, "DddHasService"), 2);
    assert_eq!(count_edges(&engine, "DddHasSearch"), 1);
    assert_eq!(count_edges(&engine, "DddHasRepository"), 2);
    assert_eq!(count_edges(&engine, "DddHasOperation"), 7);
    // DddBindsClass: only the resolved titles bind (design Book + search
    // BookSearch → Schema "Book").
    assert_eq!(count_edges(&engine, "DddBindsClass"), 2);

    // Empty graphs report no models.
    let empty = GrafeoEngine::in_memory().unwrap();
    assert!(empty.get_ddd_models().await.unwrap().is_empty());
}

#[tokio::test]
async fn test_ddd_models_sorted_by_application_name() {
    let engine = GrafeoEngine::in_memory().unwrap();
    let fixture = ddd_fixture();
    engine.ingest_ddd_model(&fixture).await.unwrap();

    let other = DddModelGraph {
        source_path: "model/aardvark.ddd".to_string(),
        application: DddApplicationNode {
            name: "Aardvark".to_string(),
            base: None,
            source_path: "model/aardvark.ddd".to_string(),
        },
        modules: vec![],
        designs: vec![],
        repositories: vec![],
        services: vec![],
        searches: vec![],
    };
    engine.ingest_ddd_model(&other).await.unwrap();

    let loaded = engine.get_ddd_models().await.unwrap();
    let names: Vec<&str> = loaded.iter().map(|m| m.application.name.as_str()).collect();
    assert_eq!(names, vec!["Aardvark", "Library"]);
    assert_eq!(loaded[0], other);
    assert_eq!(loaded[1], fixture);
}

// --- Event-contract plane (issue #454) ---

/// Two contracts' worth of nodes. Billing carries the interesting shapes:
/// version Some/None, primitive + class-typed fields with resolved_title
/// Some/None, a channel with multiple publishes, and a multi-event
/// subscription. Audit covers an event with no fields at all.
fn evt_fixtures() -> Vec<EvtModelGraph> {
    let billing = EvtModelGraph {
        source_path: "events/billing.evt".to_string(),
        events: vec![
            EvtEventNode {
                source_path: "events/billing.evt".to_string(),
                name: "PaymentRequested".to_string(),
                version: Some("1.2.0".to_string()),
                fields: vec![
                    EvtEventField {
                        name: "payment_id".to_string(),
                        type_json: serde_json::json!({
                            "type": "primitive",
                            "value": "Uuid"
                        }),
                        resolved_title: None,
                    },
                    EvtEventField {
                        name: "order".to_string(),
                        type_json: serde_json::json!({
                            "type": "class",
                            "value": { "name": "Order" }
                        }),
                        resolved_title: Some("OrderType".to_string()),
                    },
                ],
                ordinal: 0,
            },
            EvtEventNode {
                source_path: "events/billing.evt".to_string(),
                name: "PaymentCompleted".to_string(),
                version: None,
                fields: vec![EvtEventField {
                    name: "receipt".to_string(),
                    type_json: serde_json::json!({
                        "type": "class",
                        "value": { "name": "Receipt" }
                    }),
                    resolved_title: None,
                }],
                ordinal: 1,
            },
        ],
        channels: vec![EvtChannelNode {
            source_path: "events/billing.evt".to_string(),
            name: "payments".to_string(),
            publishes: vec![
                "PaymentRequested".to_string(),
                "PaymentCompleted".to_string(),
            ],
            ordinal: 0,
        }],
        subscriptions: vec![EvtSubscriptionNode {
            source_path: "events/billing.evt".to_string(),
            name: "ledger-sync".to_string(),
            events: vec!["PaymentCompleted".to_string()],
            consumer: "ledger-service".to_string(),
            ordinal: 0,
        }],
    };
    let audit = EvtModelGraph {
        source_path: "events/audit.evt".to_string(),
        events: vec![EvtEventNode {
            source_path: "events/audit.evt".to_string(),
            name: "AuditTrailWritten".to_string(),
            version: None,
            fields: vec![],
            ordinal: 0,
        }],
        channels: vec![EvtChannelNode {
            source_path: "events/audit.evt".to_string(),
            name: "audit".to_string(),
            publishes: vec!["AuditTrailWritten".to_string()],
            ordinal: 0,
        }],
        subscriptions: vec![EvtSubscriptionNode {
            source_path: "events/audit.evt".to_string(),
            name: "compliance".to_string(),
            events: vec!["AuditTrailWritten".to_string()],
            consumer: "compliance-exporter".to_string(),
            ordinal: 0,
        }],
    };
    // Audit ingested LAST but sorts FIRST by source_path.
    vec![billing, audit]
}

#[tokio::test]
async fn test_evt_model_round_trip() {
    let engine = GrafeoEngine::in_memory().unwrap();

    for fixture in evt_fixtures() {
        engine.ingest_evt_model(&fixture).await.unwrap();
    }

    let loaded = engine.get_evt_models().await.unwrap();
    let paths: Vec<&str> = loaded.iter().map(|m| m.source_path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["events/audit.evt", "events/billing.evt"],
        "one model per source_path, grouped lexicographically"
    );
    assert_eq!(loaded[0], evt_fixtures()[1], "audit deep round-trip");
    assert_eq!(loaded[1], evt_fixtures()[0], "billing deep round-trip");

    // Intra-file references survive verbatim.
    let billing = &loaded[1];
    assert_eq!(
        billing.channels[0].publishes,
        vec!["PaymentRequested", "PaymentCompleted"]
    );
    assert_eq!(billing.subscriptions[0].events, vec!["PaymentCompleted"]);
    assert_eq!(billing.subscriptions[0].consumer, "ledger-service");

    // Ordinals preserved.
    assert_eq!(
        billing
            .events
            .iter()
            .map(|e| (e.name.as_str(), e.ordinal))
            .collect::<Vec<_>>(),
        vec![("PaymentRequested", 0), ("PaymentCompleted", 1)]
    );

    // Empty graphs report no models.
    let empty = GrafeoEngine::in_memory().unwrap();
    assert!(empty.get_evt_models().await.unwrap().is_empty());
}

#[tokio::test]
async fn test_composition_table_conflict_detects_name_collision() {
    let engine = GrafeoEngine::in_memory().unwrap();
    // pg_table_name = title (the make_schema convention): parent `reward`,
    // VO child `reward_target`, property column `target` — the parent's
    // `{parent}_{feature}` projection (`reward_target`) COLLIDES with the
    // child's own table name.
    // Real-graph shape: schema titles are Camel, pg_table_names snake, and
    // property `_schema_title` carries the parent's TITLE.
    let mut parent = make_schema("Reward", "compensation", true);
    parent.pg_table_name = "reward".to_string();
    engine.ingest_schema(&parent).await.unwrap();
    let mut child = make_schema("RewardTarget", "compensation", false);
    child.pg_table_name = "reward_target".to_string();
    engine.ingest_schema(&child).await.unwrap();

    // mox contains lowering: a ValueObject-classified property referencing
    // the target (edge id convention `{prop}::{schema_title}`).
    let mut prop = make_property("target", true);
    prop.classification_kind = Some(codegraph_type_contracts::RefClassificationKind::ValueObject);
    prop.ref_target = Some("RewardTarget".to_string());
    engine
        .ingest_property("Reward", "compensation/RewardTarget", &prop)
        .await
        .unwrap();
    // The mox lowering writes the child-table link as ItemsOf; the JSON
    // path writes ReferencesSchema. Both must detect.
    engine
        .ingest_edge(
            "target::Reward",
            "compensation/RewardTarget",
            EdgeType::ItemsOf,
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        engine
            .composition_table_conflict("RewardTarget")
            .await
            .unwrap(),
        Some(("Reward".to_string(), "reward_target".to_string()))
    );
    assert_eq!(
        engine.composition_table_conflict("Reward").await.unwrap(),
        None
    );

    // Non-colliding VOs keep BOTH surfaces: the parent's projection
    // (`reward_linked`) differs from the child's own table (`linked_thing`).
    let mut linked = make_schema("LinkedThing", "compensation", false);
    linked.pg_table_name = "linked_thing".to_string();
    engine.ingest_schema(&linked).await.unwrap();
    let mut linked_prop = make_property("linked", true);
    linked_prop.classification_kind =
        Some(codegraph_type_contracts::RefClassificationKind::ValueObject);
    linked_prop.ref_target = Some("LinkedThing".to_string());
    engine
        .ingest_property("Reward", "compensation/Reward", &linked_prop)
        .await
        .unwrap();
    engine
        .ingest_edge(
            "linked::Reward",
            "compensation/LinkedThing",
            EdgeType::ReferencesSchema,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        engine
            .composition_table_conflict("LinkedThing")
            .await
            .unwrap(),
        None,
        "no collision — the standalone table stays"
    );

    // A "refers wins" containment whose target is an ENTITY keeps the
    // target's own table regardless of naming.
    engine
        .ingest_schema(&make_schema("LegalDocument", "core", true))
        .await
        .unwrap();
    let mut entity_ref_prop = make_property("legalDocuments", true);
    entity_ref_prop.classification_kind =
        Some(codegraph_type_contracts::RefClassificationKind::ValueObject);
    entity_ref_prop.ref_target = Some("LegalDocument".to_string());
    engine
        .ingest_property("Reward", "compensation/Reward", &entity_ref_prop)
        .await
        .unwrap();
    engine
        .ingest_edge(
            "legalDocuments::Reward",
            "core/LegalDocument",
            EdgeType::ItemsOf,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        engine
            .composition_table_conflict("LegalDocument")
            .await
            .unwrap(),
        None,
        "entity-target containments keep their own table"
    );
}
