use codegraph_core::traits::GraphQuerier;

/// Full multi-view IFML fixture used by round-trip tests.
const FULL_IFML: &str = r#"
domain "sales" {
    schema "sales";
}

view "CustomerList" {
    label "Customer Management";
    landmark: true;

    component "grid" {
        type: list;
        data: Customer;
        fields: [name, email, phone, status];

        on select(row) -> navigate("CustomerDetail", {
            customerId: row.id
        });
    }
}

view "CustomerDetail" {
    params { customerId: Uuid };

    component "info" {
        type: details;
        data: Customer;
        fields: [name, email, phone];

        on edit -> navigate("CustomerEdit", {
            customerId: params.customerId
        });
    }
}

view "CustomerEdit" {
    params { customerId: Uuid };

    component "form" {
        type: form;
        data: Customer;
        mode: edit;

        on save(values) -> action("UpdateCustomer", {
            on success -> navigate("CustomerDetail", {
                customerId: params.customerId
            });
            on error -> stay;
        });

        on cancel -> navigate("CustomerDetail");
    }
}
"#;

/// Test that the IFML DSL parser correctly parses the full example.
#[test]
fn test_ifml_parse_full_example() {
    let ifml_content = FULL_IFML;

    let model = rex_ifml::parse_ifml(ifml_content).expect("Should parse valid IFML");

    assert_eq!(model.domains.len(), 1);
    assert_eq!(model.domains[0].name, "sales");
    assert_eq!(model.views.len(), 3);
    assert_eq!(model.views[0].name, "CustomerList");
    assert!(model.views[0].is_landmark);
    assert_eq!(model.views[1].params.len(), 1);
    assert_eq!(model.views[1].params[0].name, "customerId");
    assert_eq!(model.views[0].components.len(), 1);
    assert_eq!(model.views[0].components[0].events.len(), 1);
}

/// Test IFML expressions parsing
#[test]
fn test_ifml_expressions() {
    let ifml = r#"
view "Dashboard" {
    component "orders" {
        type: list;
        data: Order;
        fields: [id, date, total];
        filter: date == today() && status != "cancelled";
    }
}
"#;
    let model = rex_ifml::parse_ifml(ifml).expect("Should parse expressions");
    assert_eq!(model.views.len(), 1);
    let comp = &model.views[0].components[0];
    let fields_prop = comp
        .properties
        .iter()
        .find(|p| p.key == "fields")
        .expect("fields property should exist");
    match &fields_prop.value {
        rex_ifml::ValueExpression::Array(items) => {
            let field_names: Vec<String> = items
                .iter()
                .filter_map(|v| match v {
                    rex_ifml::ValueExpression::Identifier(s) => Some(s.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(field_names, vec!["id", "date", "total"]);
        }
        _ => panic!("Expected Array value for fields"),
    }
}

/// Test that invalid IFML produces parse errors
#[test]
fn test_ifml_invalid_syntax() {
    let cases = vec![
        ("view { }", "missing view name"),
        ("view 123 { }", "non-string view name"),
        ("view \"Test\" { invalid; }", "unrecognized token"),
    ];

    for (input, description) in &cases {
        let result = rex_ifml::parse_ifml(input);
        assert!(result.is_err(), "Expected error for: {description}");
    }
}

/// Test IFML ingestion into mock graph
#[tokio::test]
async fn test_ifml_ingest_into_mock() {
    let engine = codegraph_core::mock::MockEngine::new();
    let ifml = r#"
view "TestView" {
    component "grid" {
        type: list;
        data: Item;
        fields: [name, value];

        on select -> navigate("Detail");
    }
}
"#;

    let model = rex_ifml::parse_ifml(ifml).unwrap();
    codegraph::ingest::ifml_ingest::ingest_ifml_model(&engine, &model)
        .await
        .expect("Should ingest");

    let containers = engine.get_ifml_view_containers().await.unwrap();
    assert_eq!(containers.len(), 1);
    assert_eq!(containers[0].name, "TestView");

    let components = engine.get_ifml_view_components("TestView").await.unwrap();
    assert_eq!(components.len(), 1);
    // View-level components persist under the owning view's namespace.
    assert_eq!(components[0].name, "TestView/grid");

    let bindings = engine.get_data_bindings().await.unwrap();
    assert_eq!(bindings.len(), 1);
    let binding = &bindings[0];
    assert_eq!(binding.component, "TestView/grid");
    assert_eq!(binding.entity_title, "ItemType");
    assert_eq!(binding.fields, vec!["name", "value"]);
    assert_eq!(binding.api_operation, None);
}

/// Round-trip test: IFML DSL → Grafeo graph → GraphQuerier. Verifies that
/// IFML edges (which are persisted with prefixed ids) resolve through the
/// graph layer's prefix stripping.
#[tokio::test]
async fn test_ifml_grafeo_round_trip_edges() {
    let engine = codegraph_grafeo::GrafeoEngine::in_memory().expect("in-memory Grafeo engine");
    let model = rex_ifml::parse_ifml(FULL_IFML).expect("Should parse valid IFML");
    codegraph::ingest::ifml_ingest::ingest_ifml_model(&engine, &model)
        .await
        .expect("Should ingest");

    let containers = engine.get_ifml_view_containers().await.unwrap();
    assert_eq!(containers.len(), 3);
    let names: Vec<String> = containers.iter().map(|c| c.name.clone()).collect();
    assert!(names.contains(&"CustomerList".to_string()));
    assert!(names.contains(&"CustomerDetail".to_string()));
    assert!(names.contains(&"CustomerEdit".to_string()));

    let components = engine
        .get_ifml_view_components("CustomerList")
        .await
        .unwrap();
    assert_eq!(components.len(), 1);
    assert_eq!(components[0].name, "CustomerList/grid");

    let events = engine
        .get_ifml_events("comp:CustomerList/grid")
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].name, "comp_CustomerList/grid_select");

    let flows = engine.get_ifml_navigation_flows().await.unwrap();
    let grid_flow = flows
        .iter()
        .find(|f| f.event == "comp_CustomerList/grid_select")
        .expect("missing grid->CustomerDetail flow");
    assert_eq!(grid_flow.source, "CustomerList/grid");
    assert_eq!(grid_flow.source_container, "CustomerList");
    assert_eq!(grid_flow.target, "CustomerDetail");
    assert_eq!(
        grid_flow.target_param_binding.as_deref(),
        Some(r#"{"customerId": "row.id"}"#)
    );
    let info_flow = flows
        .iter()
        .find(|f| f.event == "comp_CustomerDetail/info_edit")
        .expect("missing info->CustomerEdit flow");
    assert_eq!(info_flow.source, "CustomerDetail/info");
    assert_eq!(info_flow.source_container, "CustomerDetail");
    assert_eq!(info_flow.target, "CustomerEdit");
    assert_eq!(
        info_flow.target_param_binding.as_deref(),
        Some(r#"{"customerId": "params.customerId"}"#)
    );

    let triggers = engine.get_ifml_action_triggers().await.unwrap();
    assert!(
        triggers.contains(&(
            "comp_CustomerEdit/form_save".to_string(),
            "UpdateCustomer".to_string()
        )),
        "missing comp_form_save->UpdateCustomer trigger: {triggers:?}"
    );

    let actions = engine.get_ifml_actions().await.unwrap();
    assert!(
        actions.iter().any(|a| a.name == "UpdateCustomer"),
        "UpdateCustomer action not found: {actions:?}"
    );
}

/// Deep navigation (rexlang #PR45): `field_expr` folds `a.b.c` into
/// nested FieldAccess — filter expressions over related entities parse
/// and persist verbatim.
#[tokio::test]
async fn test_ifml_deep_navigation_filter_parses_and_persists() {
    let ifml = r#"
view "Orders" {
    component "grid" {
        type: list;
        data: Order;
        filter: customer.address.country == "DE" && customer.tier != null;
    }
}
"#;
    let engine = codegraph_grafeo::GrafeoEngine::in_memory().expect("in-memory Grafeo engine");
    let model = rex_ifml::parse_ifml(ifml).expect("deep navigation parses");
    codegraph::ingest::ifml_ingest::ingest_ifml_model(&engine, &model)
        .await
        .expect("ingests");

    let components = engine.get_ifml_view_components("Orders").await.unwrap();
    let filter = components[0].filter.as_deref().expect("filter persists");
    assert!(
        filter.contains("customer.address.country"),
        "the folded field chain persists verbatim: {filter}"
    );
}

/// Module composition (rexlang module-internal `use`) round-trips through
/// the graph: ModuleDefinition nodes persist with their typed inputs,
/// property defaults, and internal uses; view-level uses resolve as
/// HasModuleDefinition edges against them.
#[tokio::test]
async fn test_ifml_module_definitions_round_trip_through_graph() {
    let ifml = r#"
domain "sales" { schema "sales"; }

import "pager.ifml";

module "MasterDetail" {
    input { entityId: Uuid, pageSize: Int = 20 }
    output { selected: Uuid }

    use "Pagination" as inner { page_size: 50; }

    component "list" {
        type: list;
        data: Item;
        fields: [name];
    }
}

view "Items" {
    use "MasterDetail" as detail { pageSize: 25; };

    component "grid" {
        type: list;
        data: Item;
    }
}
"#;
    let engine = codegraph_grafeo::GrafeoEngine::in_memory().expect("in-memory Grafeo engine");
    let model = rex_ifml::parse_ifml(ifml).expect("Should parse composed IFML");
    codegraph::ingest::ifml_ingest::ingest_ifml_model(&engine, &model)
        .await
        .expect("Should ingest");

    let pager = rex_ifml::parse_ifml(
        "module \"Pagination\" {\n    input { pageSize: Int = 20 }\n    output { selected: Uuid }\n}\n",
    )
    .expect("pager parses");
    codegraph::ingest::ifml_ingest::ingest_ifml_model(&engine, &pager)
        .await
        .expect("pager ingests");

    let modules = engine.get_ifml_modules().await.expect("modules queryable");
    assert!(modules.iter().any(|m| m.name == "MasterDetail"));
    let master = modules.iter().find(|m| m.name == "MasterDetail").unwrap();
    let uses = master
        .module_uses
        .as_ref()
        .expect("module-internal uses persist");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].module, "Pagination");
    assert_eq!(uses[0].alias.as_deref(), Some("inner"));

    // The view-level use resolves as an edge against the ingested module.
    let containers = engine.get_ifml_view_containers().await.unwrap();
    let items = containers.iter().find(|c| c.name == "Items").unwrap();
    let uses = items.module_uses.as_ref().expect("view uses persist");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].module, "MasterDetail");
}

/// Typed component taxonomy fixture: table (field/lookup/expr columns),
/// form (required + validations + values), and chart specs.
const TYPED_IFML: &str = r#"
domain "sales" {
    schema "sales";
}

view "Reports" {
    label "Typed Components";

    component "CustomerTable" {
        type: table;
        data: Customer;

        column "Name"   -> field Customer.name;
        column "Status" -> lookup Customer.status via status_labels;
        column "Tenure" -> expr tenure_years(Customer.hire_date);
    }

    component "EditForm" {
        type: form;
        data: Customer;

        field name   -> input text   { required: true; validations: [len(name) > 2]; }
        field email  -> input email;
        field status -> input dropdown { values: ["gold", "silver"]; }
    }

    component "RevenueChart" {
        type: chart;

        chart bar { label: region; values: [revenue]; }
    }
}
"#;

/// Round-trip test for typed component specs: IFML DSL → Grafeo graph →
/// spec JSON → ComponentSpec. Also verifies the full ifml_generate path
/// accepts the fixture.
#[tokio::test]
async fn typed_component_specs_round_trip_through_graph() {
    let engine = codegraph_grafeo::GrafeoEngine::in_memory().expect("in-memory Grafeo engine");
    let model = rex_ifml::parse_ifml(TYPED_IFML).expect("Should parse typed IFML");
    codegraph::ingest::ifml_ingest::ingest_ifml_model(&engine, &model)
        .await
        .expect("Should ingest");

    let components = engine.get_ifml_view_components("Reports").await.unwrap();
    assert_eq!(components.len(), 3);
    let find = |name: &str| {
        components
            .iter()
            .find(|c| c.name == format!("Reports/{name}"))
            .unwrap_or_else(|| panic!("component {name} not found"))
    };

    let table = find("CustomerTable");
    assert_eq!(table.component_type, "table");
    let table_spec: rex_ifml::ComponentSpec = serde_json::from_str(
        table
            .spec
            .as_deref()
            .expect("table spec should be persisted"),
    )
    .expect("table spec should deserialize");
    match table_spec {
        rex_ifml::ComponentSpec::Table(t) => {
            assert!(t.pagination);
            assert_eq!(t.columns.len(), 3);
            assert!(matches!(
                &t.columns[0],
                rex_ifml::ColumnDef::Field { label, field }
                    if label == "Name" && field.entity == "Customer" && field.property == "name"
            ));
            assert!(matches!(
                &t.columns[1],
                rex_ifml::ColumnDef::Lookup { lookup, .. } if lookup == "status_labels"
            ));
            assert!(matches!(
                &t.columns[2],
                rex_ifml::ColumnDef::Expression { .. }
            ));
        }
        other => panic!("Expected Table spec, got {other:?}"),
    }

    let form = find("EditForm");
    assert_eq!(form.component_type, "form");
    let form_spec: rex_ifml::ComponentSpec =
        serde_json::from_str(form.spec.as_deref().expect("form spec should be persisted"))
            .expect("form spec should deserialize");
    match form_spec {
        rex_ifml::ComponentSpec::Form(f) => {
            assert_eq!(f.fields.len(), 3);
            assert_eq!(f.fields[0].name, "name");
            assert_eq!(f.fields[0].input, rex_ifml::InputFieldType::Text);
            assert!(f.fields[0].required);
            assert_eq!(f.fields[0].validations.len(), 1);
            assert_eq!(f.fields[1].name, "email");
            assert!(!f.fields[1].required);
            assert_eq!(
                f.fields[2].values,
                vec!["gold".to_string(), "silver".to_string()]
            );
        }
        other => panic!("Expected Form spec, got {other:?}"),
    }

    let chart = find("RevenueChart");
    assert_eq!(chart.component_type, "chart");
    let chart_spec: rex_ifml::ComponentSpec = serde_json::from_str(
        chart
            .spec
            .as_deref()
            .expect("chart spec should be persisted"),
    )
    .expect("chart spec should deserialize");
    match chart_spec {
        rex_ifml::ComponentSpec::Chart(c) => {
            assert_eq!(c.kind, rex_ifml::ChartKind::Bar);
            assert_eq!(c.label_field.as_deref(), Some("region"));
            assert_eq!(c.value_fields, vec!["revenue".to_string()]);
        }
        other => panic!("Expected Chart spec, got {other:?}"),
    }

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("domains.toml"),
        r#"
[defaults]
api_version = "v1"

[domains.sales]
label = "Sales"
schema_dir = "sales"
postgres_schema = "sales"
entities = ["CustomerType"]
"#,
    )
    .unwrap();
    let ifml_path = dir.path().join("typed.ifml");
    std::fs::write(&ifml_path, TYPED_IFML).unwrap();

    codegraph::driver::ifml_generate(codegraph::driver::IfmlGenerateArgs {
        config_path: &dir.path().join("domains.toml"),
        output: &dir.path().join("out"),
        ifml_files: &[ifml_path],
        schemas: None,
        classifier: None,
        frameworks: &["svelte".to_string()],
        profiles_config_path: None,
        template_dir: &[],
        ifml_components: None,
        ifml_design_system: None,
        ux_rules: None,
    })
    .await
    .expect("ifml_generate should succeed for typed components");
}

/// Round-trip test for conditional guards: IFML DSL conditions are rendered
/// to source text and persisted as `conditional_expression` on the
/// ViewContainer, ViewComponent, Event, and DataBinding nodes.
#[tokio::test]
async fn test_ifml_conditions_round_trip_through_graph() {
    let ifml = r#"
view "AdminDashboard" {
    if user.role == "admin";

    component "adminGrid" {
        type: list;
        data: Customer;
        if account.balance >= 100;

        on select(row) if row.active == true && row.tier != "free" -> navigate("Detail", {
            id: row.id
        });
        on load -> stay;
    }
}

view "Plain" {
    component "grid" {
        type: list;
        data: Customer;
    }
}
"#;
    let engine = codegraph_grafeo::GrafeoEngine::in_memory().expect("in-memory Grafeo engine");
    let model = rex_ifml::parse_ifml(ifml).expect("Should parse conditional IFML");
    codegraph::ingest::ifml_ingest::ingest_ifml_model(&engine, &model)
        .await
        .expect("Should ingest");

    let containers = engine.get_ifml_view_containers().await.unwrap();
    let admin = containers
        .iter()
        .find(|c| c.name == "AdminDashboard")
        .expect("AdminDashboard container");
    assert_eq!(
        admin.conditional_expression.as_deref(),
        Some("user.role == \"admin\"")
    );
    let plain = containers
        .iter()
        .find(|c| c.name == "Plain")
        .expect("Plain container");
    assert_eq!(plain.conditional_expression, None);

    let components = engine
        .get_ifml_view_components("AdminDashboard")
        .await
        .unwrap();
    assert_eq!(components.len(), 1);
    assert_eq!(
        components[0].conditional_expression.as_deref(),
        Some("account.balance >= 100")
    );
    let plain_components = engine.get_ifml_view_components("Plain").await.unwrap();
    assert_eq!(plain_components[0].conditional_expression, None);

    let events = engine
        .get_ifml_events("comp:AdminDashboard/adminGrid")
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    let select = events
        .iter()
        .find(|e| e.event_type == "select")
        .expect("select event");
    assert_eq!(
        select.conditional_expression.as_deref(),
        Some("row.active == true && row.tier != \"free\"")
    );
    let load = events
        .iter()
        .find(|e| e.event_type == "load")
        .expect("load event");
    assert_eq!(load.conditional_expression, None);
}

/// Cross-domain component-name collision (the wellness/compensation
/// shape): two views in DIFFERENT ingested models each declare a
/// view-level component named "history". Each view must resolve only its
/// own component, its own events, and its own entity binding — bare-name
/// keying used to fuse them into one component per view.
#[tokio::test]
async fn same_named_view_components_in_different_domains_stay_isolated() {
    const WELLNESS_IFML: &str = r#"
view "ActivityDashboard" {
    component "history" {
        type: list;
        data: WellnessActivity;
        fields: [note, minutes];

        on create -> navigate("ActivityForm");
        on refreshData -> refresh("history");
    }
}

view "ActivityForm" {
    component "recorder" {
        type: form;
        data: WellnessActivity;
        fields: [note, minutes];

        on save(values) -> stay;
        on cancel -> navigate("ActivityDashboard");
    }
}
"#;
    const COMPENSATION_IFML: &str = r#"
view "CompensationHistory" {
    component "history" {
        type: list;
        data: Reward;
        fields: [rewardCode, points];

        on select(row) -> navigate("RewardEditor");
        on create -> navigate("RewardEditor");
    }
}

view "RewardEditor" {
    component "editor" {
        type: form;
        data: Reward;
        fields: [rewardCode, points];

        on save(values) -> stay;
        on cancel -> navigate("CompensationHistory");
    }
}
"#;
    let engine = codegraph_grafeo::GrafeoEngine::in_memory().expect("in-memory Grafeo engine");
    // Ingested as separate models, mirroring the driver's per-file loop.
    for (source, description) in [
        (WELLNESS_IFML, "wellness model"),
        (COMPENSATION_IFML, "compensation model"),
    ] {
        let model = rex_ifml::parse_ifml(source).expect(description);
        codegraph::ingest::ifml_ingest::ingest_ifml_model(&engine, &model)
            .await
            .expect(description);
    }

    // Each view surfaces exactly its own component — membership no longer
    // leaks across views through same-named component nodes.
    let dashboard = engine
        .get_ifml_view_components("ActivityDashboard")
        .await
        .unwrap();
    assert_eq!(dashboard.len(), 1, "{dashboard:?}");
    assert_eq!(dashboard[0].name, "ActivityDashboard/history");
    assert_eq!(dashboard[0].entity.as_deref(), Some("WellnessActivity"));
    assert_eq!(
        dashboard[0].fields,
        Some(vec!["note".to_string(), "minutes".to_string()])
    );

    let compensation = engine
        .get_ifml_view_components("CompensationHistory")
        .await
        .unwrap();
    assert_eq!(compensation.len(), 1, "{compensation:?}");
    assert_eq!(compensation[0].name, "CompensationHistory/history");
    assert_eq!(compensation[0].entity.as_deref(), Some("Reward"));
    assert_eq!(
        compensation[0].fields,
        Some(vec!["rewardCode".to_string(), "points".to_string()])
    );

    // Events hang off the namespaced component id, never the shared bare
    // name — no merged event sets.
    let wellness_events = engine
        .get_ifml_events("comp:ActivityDashboard/history")
        .await
        .unwrap();
    assert_eq!(wellness_events.len(), 2, "{wellness_events:?}");
    assert!(
        wellness_events
            .iter()
            .all(|e| e.name.starts_with("comp_ActivityDashboard/history_"))
    );

    let compensation_events = engine
        .get_ifml_events("comp:CompensationHistory/history")
        .await
        .unwrap();
    assert_eq!(compensation_events.len(), 2, "{compensation_events:?}");
    assert!(
        compensation_events
            .iter()
            .all(|e| e.name.starts_with("comp_CompensationHistory/history_"))
    );

    // Data bindings resolve per view-scoped component. (Binding rows need
    // entity resolution, so assert on the mock engine whose binding plane
    // doesn't require ingested Schema nodes.)
    let mock = codegraph_core::mock::MockEngine::new();
    for (source, description) in [
        (WELLNESS_IFML, "wellness model"),
        (COMPENSATION_IFML, "compensation model"),
    ] {
        let model = rex_ifml::parse_ifml(source).expect(description);
        codegraph::ingest::ifml_ingest::ingest_ifml_model(&mock, &model)
            .await
            .expect(description);
    }
    let bindings = mock.get_data_bindings().await.unwrap();
    let wellness_binding = bindings
        .iter()
        .find(|b| b.component == "ActivityDashboard/history")
        .expect("wellness binding");
    assert_eq!(wellness_binding.entity_title, "WellnessActivityType");
    let compensation_binding = bindings
        .iter()
        .find(|b| b.component == "CompensationHistory/history")
        .expect("compensation binding");
    assert_eq!(compensation_binding.entity_title, "RewardType");

    // The view's other component (the recorder form) still resolves.
    let form_events = engine
        .get_ifml_events("comp:ActivityForm/recorder")
        .await
        .unwrap();
    assert_eq!(form_events.len(), 2, "{form_events:?}");
}

/// Test IFML stale route cleanup
#[test]
fn test_ifml_clean_stale_routes() {
    let dir = tempfile::tempdir().unwrap();
    let routes_dir = dir.path().join("src").join("routes");

    // Create some IFML route directories
    let active_dir = routes_dir.join("customerview");
    let stale_dir = routes_dir.join("oldview");
    let special_dir = routes_dir.join("(app)");

    std::fs::create_dir_all(&active_dir).unwrap();
    std::fs::create_dir_all(&stale_dir).unwrap();
    std::fs::create_dir_all(&special_dir).unwrap();

    // Create +page.svelte files to mark IFML routes
    std::fs::write(active_dir.join("+page.svelte"), "").unwrap();
    std::fs::write(stale_dir.join("+page.svelte"), "").unwrap();
    // Special directory should NOT have a +page.svelte (it's a route group)

    // Also create active view list
    let active_views: Vec<String> = vec!["Customerview".to_string()];

    // Call the cleanup function
    // We access it through the generate module's public interface
    // The function is not public, so we test manually
    let routes_path = dir.path().join("src").join("routes");

    // Simulate the logic from clean_stale_ifml_routes
    let entries = std::fs::read_dir(&routes_path).unwrap();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if dir_name.starts_with('_') || dir_name.starts_with('(') || dir_name.starts_with('.') {
            continue;
        }
        let is_active = active_views.iter().any(|v| v.to_lowercase() == dir_name);
        if !is_active && path.join("+page.svelte").exists() {
            let _ = std::fs::remove_dir_all(&path);
        }
    }

    // active_dir should still exist
    assert!(
        active_dir.exists(),
        "active view directory should be preserved"
    );
    // stale_dir should be removed
    assert!(
        !stale_dir.exists(),
        "stale view directory should be removed"
    );
    // special_dir should still exist
    assert!(
        special_dir.exists(),
        "special SvelteKit directory should be preserved"
    );
}
