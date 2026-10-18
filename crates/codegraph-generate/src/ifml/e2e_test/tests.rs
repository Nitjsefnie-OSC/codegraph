//! Generator unit tests (carried from the pre-split `e2e_test.rs`, #317).
//! Behavior pins (url-pattern helpers, fixture values, suffix cleanup
//! ordering, workflow-pin semantics) are unchanged; spec-content assertions
//! churn with the POM rewiring (specs now drive the emitted page classes
//! and carry zero raw testid construction).

use codegraph_config::DomainConfig;
use codegraph_config::ux::Display;
use codegraph_core::mock::MockEngine;
use codegraph_core::types::{EnumValue, PropertyNode, SchemaNode};
use codegraph_type_contracts::RefClassificationKind;

use super::super::route_generator::RenderWorkflow;
use super::fixtures::{
    binding_value_pattern, entry_inner, escape_regex, js_literal_inner, js_value_for_type,
    url_pattern, url_pattern_with_dialog,
};
use super::render::{render_spec, render_ux_spec, ux_column_checks, ux_menu_event};
use super::spec_payload::{
    ClickThroughTest, Fixture, ModalCloseAssertions, RenderTest, UxCellFormat, UxChipCheck,
    UxColumnCheck, UxMenuCheck, UxTimelineCheck, UxViewTest, ViewTestSpec,
};
use crate::ux::plan::{UxPlanInput, build_ux_plan};
use std::collections::{BTreeMap, HashMap};

#[test]
fn url_pattern_sorts_bindings_and_substitutes_expressions() {
    let mut binding = HashMap::new();
    binding.insert("region".to_string(), "\"eu\"".to_string());
    binding.insert("customerId".to_string(), "row.id".to_string());
    assert_eq!(
        url_pattern("/customerdetail", &binding),
        "/customerdetail\\?customerId=[^&]+&region=eu"
    );
    assert_eq!(
        url_pattern("/customerlist", &HashMap::new()),
        "/customerlist$"
    );
}

#[test]
fn url_pattern_with_dialog_appends_marker_last() {
    let mut binding = HashMap::new();
    binding.insert("customerId".to_string(), "row.id".to_string());
    assert_eq!(
        url_pattern_with_dialog("/customerdialog", &binding),
        "/customerdialog\\?customerId=[^&]+&dialog=open"
    );
    assert_eq!(
        url_pattern_with_dialog("/customerdialog", &HashMap::new()),
        "/customerdialog\\?dialog=open"
    );
}

#[test]
fn modal_click_through_renders_wrapper_and_close_assertions() {
    let spec = ViewTestSpec {
        view_name: "CustomerList".to_string(),
        label: "Customer Management".to_string(),
        route: "/customerlist".to_string(),
        render: None,
        click_throughs: vec![ClickThroughTest {
            title: "comp_grid_select navigates to CustomerDialog".to_string(),
            source_route: "/customerlist".to_string(),
            source_component: "grid".to_string(),
            target_view: "CustomerDialog".to_string(),
            row_testid: "grid-row".to_string(),
            fixture: Fixture {
                base_path: "/api/v1/sales/customer".to_string(),
                entries: vec![],
            },
            target_pattern: "/customerdialog\\?dialog=open".to_string(),
            modal: Some(ModalCloseAssertions {
                wrapper_testid: "customer-modal".to_string(),
                close_testid: "customerdialog-modal-close".to_string(),
                back_pattern: "/customerlist$".to_string(),
            }),
        }],
        validations: Vec::new(),
        round_trips: Vec::new(),
        personas: Vec::new(),
    };

    let rendered = render_spec(&spec, true);
    // The flow (and its modal close) is a page-class method; the spec body
    // carries no raw testid construction (#317).
    assert!(
        rendered.contains("await ui.navigateToCustomerDialog();"),
        "{rendered}"
    );
    assert!(
        rendered.contains("await ui.closeCustomerDialogModal();"),
        "{rendered}"
    );
    assert!(!rendered.contains("getByTestId("), "{rendered}");
    assert!(!rendered.contains("waitForURL"), "{rendered}");
}

#[test]
fn binding_value_pattern_handles_literals_and_expressions() {
    assert_eq!(binding_value_pattern("\"eu\""), "eu");
    assert_eq!(binding_value_pattern("row.id"), "[^&]+");
    assert_eq!(binding_value_pattern("a.b(c)"), "[^&]+");
}

#[test]
fn escape_regex_escapes_metacharacters() {
    assert_eq!(escape_regex("/customer-detail"), "/customer-detail");
    assert_eq!(escape_regex("a.b*c"), "a\\.b\\*c");
}

#[test]
fn js_value_follows_rust_types() {
    assert_eq!(js_value_for_type("active", "bool"), "true");
    assert_eq!(js_value_for_type("age", "Option< i32 >"), "42");
    assert_eq!(js_value_for_type("age", "i32"), "42");
    assert_eq!(js_value_for_type("name", "String"), "'Test name'");
    assert_eq!(
        js_value_for_type("submittedAt", "Option< DateTime < Utc > >"),
        "'2024-01-15T10:30:00Z'"
    );
}

#[test]
fn spec_render_includes_all_test_kinds() {
    let spec = ViewTestSpec {
        view_name: "CustomerList".to_string(),
        label: "Customer Management".to_string(),
        route: "/customerlist".to_string(),
        render: Some(RenderTest {
            label: "Customer Management".to_string(),
            route: "/customerlist".to_string(),
            primary_testid: "grid-table".to_string(),
            assert_heading: true,
            nav_testid: Some("navigation-menu".to_string()),
            container_testid: Some("card".to_string()),
        }),
        click_throughs: vec![ClickThroughTest {
            title: "comp_grid_select navigates to CustomerDetail".to_string(),
            source_route: "/customerlist".to_string(),
            source_component: "grid".to_string(),
            target_view: "CustomerDetail".to_string(),
            row_testid: "grid-row".to_string(),
            fixture: Fixture {
                base_path: "/api/v1/sales/customer".to_string(),
                entries: vec![("name".to_string(), "'Test name'".to_string())],
            },
            target_pattern: "/customerdetail\\?customerId=[^&]+".to_string(),
            modal: None,
        }],
        validations: Vec::new(),
        round_trips: Vec::new(),
        personas: Vec::new(),
    };

    let rendered = render_spec(&spec, true);
    assert!(rendered.contains("import { test, expect } from '@playwright/test';"));
    assert!(
        rendered.contains("import { CustomerListPage } from '../pages/customer-list-page';"),
        "{rendered}"
    );
    assert!(rendered.contains("test.describe('Customer Management'"));
    assert!(
        rendered.contains("const ui = new CustomerListPage(page);"),
        "{rendered}"
    );
    assert!(rendered.contains("await ui.open();"), "{rendered}");
    assert!(
        rendered.contains("await expect(ui.heading()).toBeVisible();"),
        "{rendered}"
    );
    assert!(
        rendered.contains("await expect(ui.primaryRoot()).toBeVisible();"),
        "{rendered}"
    );
    assert!(
        rendered.contains("await expect(ui.navRoot()).toBeVisible();"),
        "{rendered}"
    );
    assert!(
        rendered.contains("await expect(ui.containerRoot()).toBeVisible();"),
        "{rendered}"
    );
    assert!(rendered.contains("request.post('/api/v1/sales/customer'"));
    assert!(
        rendered.contains("await ui.navigateToCustomerDetail();"),
        "{rendered}"
    );
    assert!(!rendered.contains("getByTestId("), "{rendered}");
    assert!(rendered.ends_with("});\n"));
}

fn ux_test_fixture() -> UxViewTest {
    UxViewTest {
        component_name: "grid".to_string(),
        route: "/refundrequestlist".to_string(),
        fixture: Fixture {
            base_path: "/api/v1/refunds/refund-request".to_string(),
            entries: vec![
                ("title".to_string(), "'Test title'".to_string()),
                ("status".to_string(), "'draft'".to_string()),
                ("amount".to_string(), "42".to_string()),
                ("urgent".to_string(), "true".to_string()),
                (
                    "submittedAt".to_string(),
                    "'2024-01-15T10:30:00Z'".to_string(),
                ),
            ],
        },
        table_testid: "grid-table".to_string(),
        row_testid: "grid-row".to_string(),
        chip_checks: vec![
            UxChipCheck {
                field: "status".to_string(),
                override_literal: Some("'draft'".to_string()),
                text: "draft".to_string(),
                variant: "secondary".to_string(),
            },
            UxChipCheck {
                field: "status".to_string(),
                override_literal: Some("'approved'".to_string()),
                text: "approved".to_string(),
                variant: "default".to_string(),
            },
        ],
        column_checks: vec![
            UxColumnCheck {
                header: Some("amount".to_string()),
                cell_index: 2,
                format: UxCellFormat::Money {
                    literal: "42".to_string(),
                },
                assert_right: true,
            },
            UxColumnCheck {
                header: None,
                cell_index: 4,
                format: UxCellFormat::Date {
                    literal: "2024-01-15T10:30:00Z".to_string(),
                },
                assert_right: false,
            },
        ],
        copy_check: None,
        menu: Some(UxMenuCheck {
            trigger_testid: "grid-actions".to_string(),
            menu_testid: "grid-actions-menu".to_string(),
            target_pattern: "/refundrequestdetail\\?id=[^&]+".to_string(),
        }),
        timeline: None,
        locale: "en-NZ".to_string(),
        money_options: "{ style: 'currency', currency: 'NZD' }".to_string(),
    }
}

#[test]
fn ux_spec_renders_chip_tone_and_intl_assertions() {
    let rendered = render_ux_spec("RefundRequestList", &ux_test_fixture());
    assert!(rendered.ends_with("});\n"), "{rendered}");
    // Pinned title prefixes — the gate's `ux` category matches these.
    assert!(
        rendered.contains("test('ux chips render with tone variants'"),
        "{rendered}"
    );
    assert!(
        rendered.contains("test('ux numeric columns align and format through Intl'"),
        "{rendered}"
    );
    assert!(
        rendered.contains("test('ux row actions open the overflow menu'"),
        "{rendered}"
    );
    // Fixture + in-spec Intl computation.
    assert!(
        rendered.contains("const UX_BASE = '/api/v1/refunds/refund-request';"),
        "{rendered}"
    );
    assert!(
        rendered.contains("const UX_LOCALE = 'en-NZ';"),
        "{rendered}"
    );
    assert!(
        rendered.contains("const UX_MONEY_OPTS = { style: 'currency', currency: 'NZD' };"),
        "{rendered}"
    );
    assert!(
        rendered.contains("new Intl.NumberFormat(UX_LOCALE, UX_MONEY_OPTS)"),
        "{rendered}"
    );
    assert!(
        rendered.contains("new Intl.DateTimeFormat(UX_LOCALE, { dateStyle: 'medium' })"),
        "{rendered}"
    );
    // Balanced parens on the Intl text assertions (regression pin).
    assert!(
        rendered.contains("await expect(td).toHaveText(fmt.format(42));"),
        "{rendered}"
    );
    // Chip variant + visibility through the POM component object (#317).
    assert!(rendered.contains("ui.grid.chipFor('draft')"), "{rendered}");
    assert!(
        rendered.contains("toHaveAttribute('data-chip-variant', 'secondary')"),
        "{rendered}"
    );
    assert!(
        rendered.contains("toHaveAttribute('data-chip-variant', 'default')"),
        "{rendered}"
    );
    // Numeric alignment classes + nth cell addressing within the POM row.
    assert!(rendered.contains("toHaveClass(/text-right/)"), "{rendered}");
    assert!(
        rendered.contains("toHaveClass(/tabular-nums/)"),
        "{rendered}"
    );
    assert!(rendered.contains("row.locator('td').nth(2)"), "{rendered}");
    assert!(rendered.contains("row.locator('td').nth(4)"), "{rendered}");
    // Overflow menu open + first item navigation, through the kernel.
    assert!(
        rendered.contains("await ui.grid.openActions();"),
        "{rendered}"
    );
    assert!(
        rendered.contains("waitForURL(new RegExp('/refundrequestdetail\\\\?id=[^&]+'))"),
        "{rendered}"
    );
    // Zero raw testid construction in the spec body.
    assert!(!rendered.contains("getByTestId("), "{rendered}");
    assert!(!rendered.contains("data-testid="), "{rendered}");
}

#[test]
fn ux_spec_omits_absent_features_and_renders_timeline() {
    let mut test = ux_test_fixture();
    test.chip_checks.clear();
    test.column_checks.clear();
    test.menu = None;
    test.timeline = Some(UxTimelineCheck {
        root_testid: "grid-timeline".to_string(),
        item_testid: "grid-timeline-item".to_string(),
        order_literal: Some("2024-01-15T10:30:00Z".to_string()),
    });
    let rendered = render_ux_spec("RefundRequestList", &test);
    assert!(!rendered.contains("ux chips render"), "{rendered}");
    assert!(!rendered.contains("ux numeric columns"), "{rendered}");
    assert!(!rendered.contains("overflow menu"), "{rendered}");
    assert!(!rendered.contains("UX_MONEY_OPTS)"), "{rendered}");
    assert!(
        rendered.contains("test('ux timeline renders entries newest-first'"),
        "{rendered}"
    );
    assert!(rendered.contains("ui.grid.timelineItems()"), "{rendered}");
    assert!(
        rendered.contains("expect(times).toContain(fmt.format(new Date('2024-01-15T10:30:00Z')))"),
        "{rendered}"
    );
    assert!(
        rendered.contains("expect(parsed[i]).toBeLessThanOrEqual(parsed[i - 1]);"),
        "{rendered}"
    );
}

#[test]
fn ux_menu_event_requires_a_second_navigate() {
    let navigate = |target: &str| super::super::context::IfmlEvent {
        name: String::new(),
        event_type: "select".to_string(),
        params: Vec::new(),
        requires: Vec::new(),
        action: super::super::context::IfmlAction::Navigate {
            target: target.to_string(),
            binding: HashMap::new(),
        },
    };
    let single = vec![navigate("Detail")];
    assert!(ux_menu_event(&single).is_none());
    let mut second = navigate("Review");
    second.event_type = "review".to_string();
    let pair = vec![navigate("Detail"), second];
    let menu = ux_menu_event(&pair).expect("second navigate discloses into the menu");
    match &menu.action {
        super::super::context::IfmlAction::Navigate { target, .. } => assert_eq!(target, "Review"),
        _ => panic!("expected a navigate action"),
    }
}

#[test]
fn ux_chip_checks_pin_the_workflow_status_to_the_initial_state() {
    fn schema(title: &str, is_codelist: bool) -> SchemaNode {
        SchemaNode {
            namespace: None,
            schema_id: format!("id:{title}"),
            title: title.to_string(),
            description: None,
            schema_type: "object".to_string(),
            classification: "entity".to_string(),
            access: None,
            annotations: None,
            domain: Some("refunds".to_string()),
            rel_path: format!("{title}.json"),
            pg_type: "UUID".to_string(),
            rust_type: "Uuid".to_string(),
            sea_orm_type: "Uuid".to_string(),
            rust_type_name: title.to_string(),
            pg_table_name: codegraph_naming::to_snake_case(title),
            api_path_segment: codegraph_naming::to_kebab_case(title),
            parent_schema: None,
            is_entity: !is_codelist,
            is_codelist,
            is_primitive_wrapper: false,
            has_all_of: false,
            has_one_of: false,
            has_any_of: false,
            has_definitions: false,
            custom_annotations: Default::default(),
        }
    }

    let rules = codegraph_config::builtin_ux_rules().unwrap().rules;
    let mut status_prop = ux_test_prop("status");
    status_prop.classification_kind = Some(RefClassificationKind::CodelistReference);
    status_prop.ref_target = Some("codelist/RefundStatusCodeList.json".to_string());
    let field = super::ux_plans::ux_ui_field(&status_prop);
    let input = UxPlanInput {
        entity_title: "RefundRequest",
        fields: std::slice::from_ref(&field),
        prop_by_name: BTreeMap::from([("status", &status_prop)]),
        workflow_status_field: None,
        workflow_terminal_states: &[],
        has_soft_delete: false,
        user_pinned_list_order: false,
    };
    let plan = build_ux_plan(Some(&rules), &input)
        .unwrap()
        .expect("codelist column planned");
    assert!(
        matches!(plan.columns.get("status"), Some(col) if col.display == Display::Chip),
        "precondition: the codelist column resolves to a chip"
    );

    let c = super::super::context::IfmlComponent {
        name: "grid".to_string(),
        component_type: "list".to_string(),
        mode: None,
        entity: Some("RefundRequest".to_string()),
        fields: vec!["status".to_string()],
        fields_with_types: Vec::new(),
        filter: None,
        properties: HashMap::new(),
        events: Vec::new(),
        parts: Vec::new(),
        spec: None,
    };
    let entries = vec![("status".to_string(), "'draft'".to_string())];
    let db = MockEngine::builder()
        .with_schema(schema("RefundRequestType", false))
        .with_properties("RefundRequestType", vec![status_prop])
        .with_schema(schema("RefundStatusCodeList", true))
        .with_enum_values(
            "RefundStatusCodeList",
            vec![
                EnumValue {
                    value: "draft".to_string(),
                    display_name: None,
                    sort_order: 0,
                },
                EnumValue {
                    value: "approved".to_string(),
                    display_name: None,
                    sort_order: 1,
                },
            ],
        )
        .build();

    // Control: without a workflow the codelist chip is exercised with
    // distinct expected variants and per-check overrides.
    let control = futures::executor::block_on(super::render::ux_chip_checks(
        &db, &c, &plan, &entries, None,
    ));
    assert!(
        control.len() >= 2 && control.iter().all(|check| check.override_literal.is_some()),
        "control: the codelist chip column is exercised with overrides without a workflow"
    );
    // With a workflow the status column pins ONE check to the initial
    // state, WITHOUT an override (issue #311): the create payload cannot
    // set the status field, and the DDL default materializes it, so the
    // created row's chip shows the initial state and its tone variant.
    let workflow = RenderWorkflow {
        status_field: "status".to_string(),
        states: vec!["draft".to_string(), "approved".to_string()],
        terminal_states: vec!["approved".to_string()],
        initial_state: "draft".to_string(),
        transition_map: Default::default(),
        generate_action_endpoints: true,
        transitions: Vec::new(),
        badge_html: String::new(),
        each: false,
    };
    let pinned = futures::executor::block_on(super::render::ux_chip_checks(
        &db,
        &c,
        &plan,
        &entries,
        Some(&workflow),
    ));
    assert_eq!(pinned.len(), 1, "one initial-state chip check: {pinned:?}");
    let check = &pinned[0];
    assert_eq!(check.field, "status");
    assert_eq!(check.text, "draft");
    assert_eq!(check.override_literal, None);
    assert_eq!(
        check.variant,
        control
            .iter()
            .find(|chk| chk.text == "draft")
            .map(|chk| chk.variant.as_str())
            .unwrap_or_default(),
        "the pinned variant is the same ToneMap lookup the override path uses"
    );
}

fn ux_test_prop(name: &str) -> PropertyNode {
    PropertyNode {
        name: name.to_string(),
        prop_type: "string".into(),
        description: None,
        format: None,
        is_required: false,
        is_nullable: false,
        is_id: false,
        type_expr: None,
        is_array: false,
        min_items: None,
        max_items: None,
        pattern: None,
        min_length: None,
        max_length: None,
        minimum: None,
        maximum: None,
        pg_column_name: name.into(),
        pg_column_type: "TEXT".into(),
        rust_field_name: name.into(),
        rust_field_type: "String".into(),
        sea_orm_type: "String".into(),
        render_strategy: "scalar".into(),
        ref_target: None,
        classification: None,
        projection: None,
        classification_kind: None,
        ui_override_detail: None,
        ui_override_list_cell: None,
        ui_override_form: None,
        ui_override_inline: None,
    }
}

#[test]
fn ux_column_checks_pin_dimensions_and_alignment() {
    let rules = codegraph_config::parse_ux_rules_str(
        "[[column]]\nname_pattern = \"amount\"\ndimension = \"money\"\n",
    )
    .unwrap()
    .rules;
    let c = super::super::context::IfmlComponent {
        name: "grid".to_string(),
        component_type: "list".to_string(),
        mode: None,
        entity: Some("RefundRequest".to_string()),
        fields: vec!["title".to_string(), "amount".to_string()],
        fields_with_types: Vec::new(),
        filter: None,
        properties: HashMap::new(),
        events: Vec::new(),
        parts: Vec::new(),
        spec: None,
    };
    let field = crate::ui::page::UiField {
        name: "amount".to_string(),
        label: String::new(),
        ts_type: "number".to_string(),
        input_type: String::new(),
        is_required: false,
        is_array: false,
        is_entity_ref: false,
        is_immutable: false,
        is_codelist: false,
        is_range: false,
        codelist_values: Vec::new(),
        description: String::new(),
        pg_type: "BIGINT".to_string(),
        open_end: false,
        ref_api_path: None,
        structured_sub_fields: Vec::new(),
        nested_type_name: None,
    };
    let input = UxPlanInput {
        entity_title: "RefundRequest",
        fields: std::slice::from_ref(&field),
        prop_by_name: BTreeMap::new(),
        workflow_status_field: None,
        workflow_terminal_states: &[],
        has_soft_delete: false,
        user_pinned_list_order: false,
    };
    let plan = build_ux_plan(Some(&rules), &input)
        .unwrap()
        .expect("rules on → plan");
    let checks = ux_column_checks(&c, &plan, &[("amount".to_string(), "42".to_string())]);
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].cell_index, 1);
    assert_eq!(checks[0].header.as_deref(), Some("amount"));
    assert!(checks[0].assert_right);
    assert!(matches!(checks[0].format, UxCellFormat::Money { .. }));
}

#[test]
fn stale_cleanup_removes_ux_specs_by_suffix() {
    // Mirrors the suffix ordering in generate(): ux specs must be
    // matched BEFORE the generic `.spec.ts` fallback or active ux files
    // would be deleted as stale plain specs.
    let name = "refund-request-list.ux.spec.ts";
    assert!(name.strip_suffix(".workflow.spec.ts").is_none());
    assert!(name.strip_suffix(".ux.spec.ts").is_some());
    let plain = "refund-request-list.spec.ts";
    assert!(plain.strip_suffix(".ux.spec.ts").is_none());
    assert!(plain.strip_suffix(".workflow.spec.ts").is_none());
}

#[test]
fn pom_page_imports_resolve_to_emitted_kernel_files() {
    // The gate caught this class of bug (#318): a page class importing
    // `./support/pom` while the kernel emits `base-page.ts`/`ux-table.ts`
    // typechecks NOWHERE (svelte-check fails, Playwright cannot transpile).
    // Pin the contract structurally: every relative `./support/…` import
    // specifier in an emitted page class must resolve, as
    // `tests/pages/support/<name>.ts`, to a file in the SAME emission set.
    use super::super::context::{IfmlComponent, IfmlViewContainer};
    use super::pom::pom_file_set;
    use codegraph_config::ux::UxRules;

    let view = || IfmlViewContainer {
        name: "CustomerList".to_string(),
        label: Some("Customer Management".to_string()),
        is_xor: false,
        is_default: false,
        is_landmark: false,
        is_modal: false,
        conditional_expr_json: None,
        conditional_expression: None,
        roles: Vec::new(),
        requires: Vec::new(),
        params: Vec::new(),
        components: vec![IfmlComponent {
            name: "grid".to_string(),
            component_type: "list".to_string(),
            mode: None,
            entity: Some("Customer".to_string()),
            fields: vec!["name".to_string()],
            fields_with_types: Vec::new(),
            filter: None,
            properties: HashMap::new(),
            events: Vec::new(),
            parts: Vec::new(),
            spec: None,
        }],
        events: Vec::new(),
        containers: Vec::new(),
    };
    let config: DomainConfig = toml::from_str(
        r#"
[defaults]
api_version = "v1"

[domains.sales]
label = "Sales"
schema_dir = "sales"
postgres_schema = "sales"
entities = []
"#,
    )
    .unwrap();

    for (label, rules) in [
        ("no-ux-rules", None),
        ("ux-rules-on", Some(UxRules::default())),
    ] {
        let pom = super::pom::build_view_pom(
            &config,
            None,
            &view(),
            None,
            rules.as_ref(),
            &HashMap::new(),
            false,
            &[],
        );
        let files = pom_file_set(&[("customer-list", &pom)]);
        let paths: Vec<String> = files
            .iter()
            .map(|(p, _)| p.to_string_lossy().into_owned())
            .collect();
        for (path, content) in &files {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with("-page.ts") {
                continue;
            }
            for (at, _) in content.match_indices("from './support/") {
                let rest = &content[at + "from './support/".len()..];
                let module = rest.split('\'').next().unwrap_or("");
                assert!(!module.is_empty(), "{label}: empty support import");
                let resolved = format!("tests/pages/support/{module}.ts");
                assert!(
                    paths.iter().any(|p| p == &resolved),
                    "{label}: {name} imports './support/{module}' but the emission set \
                     carries no {resolved}: {paths:?}"
                );
            }
        }
        // The kernel files themselves are always in the set.
        assert!(
            paths.contains(&"tests/pages/support/base-page.ts".to_string()),
            "{label}: kernel base-page.ts missing: {paths:?}"
        );
        assert!(
            paths.contains(&"tests/pages/support/ux-table.ts".to_string()),
            "{label}: kernel ux-table.ts missing: {paths:?}"
        );
    }
}

#[test]
fn js_literal_inner_strips_quotes() {
    assert_eq!(js_literal_inner("'draft'"), "draft");
    assert_eq!(js_literal_inner("42"), "42");
    assert_eq!(js_literal_inner("true"), "true");
    assert_eq!(
        entry_inner(&[("a".into(), "'x y'".into())], "a"),
        Some("x y".into())
    );
    assert_eq!(entry_inner(&[("a".into(), "''".into())], "a"), None);
}

#[test]
fn view_journey_spec_walks_hops_and_arrivals() {
    use super::fixtures::view_route;
    use super::spec_payload::{JourneyHop, JourneyHopKind, JourneyPath, JourneyStep, ViewJourney};

    let hop = |target: &str| JourneyHop {
        source_component: "grid".to_string(),
        kind: JourneyHopKind::Row,
        target_view: target.to_string(),
        target_pattern: format!("{}$", escape_regex(&view_route(target))),
        seed: Some(Fixture {
            base_path: "/api/v1/sales/customer".to_string(),
            entries: vec![("name".to_string(), "'Test name'".to_string())],
        }),
        fills: Vec::new(),
    };
    let journey = ViewJourney {
        view_name: "CustomerList".to_string(),
        label: "Customer Management".to_string(),
        persona: Some("admin".to_string()),
        open_param: None,
        open_seed: None,
        seeds: vec![Fixture {
            base_path: "/api/v1/sales/customer".to_string(),
            entries: vec![("name".to_string(), "'Test name'".to_string())],
        }],
        paths: vec![JourneyPath {
            views: vec![
                "CustomerList".to_string(),
                "CustomerDetail".to_string(),
                "CustomerEdit".to_string(),
            ],
            steps: vec![
                JourneyStep {
                    method: "journeyToCustomerDetail".to_string(),
                    hop: hop("CustomerDetail"),
                    arrival: true,
                    arrival_heading: false,
                    target_steps: Vec::new(),
                },
                JourneyStep {
                    method: "journeyToCustomerEdit".to_string(),
                    hop: hop("CustomerEdit"),
                    arrival: false,
                    arrival_heading: false,
                    target_steps: vec!["Plan".to_string()],
                },
            ],
        }],
    };

    let rendered = super::auth::render_view_journey(&journey, "../e2e/personas");
    assert!(rendered.contains("import { asPersona, admin } from '../e2e/personas';"));
    assert!(rendered.contains("asPersona(admin);"));
    assert!(rendered.contains("test('journey: CustomerList → CustomerDetail → CustomerEdit'"));
    // Seeds + open through the source page class.
    assert!(rendered.contains("const rowSeed0 = await request.post('/api/v1/sales/customer'"));
    assert!(rendered.contains("let ui = new CustomerListPage(page);"));
    assert!(rendered.contains("await ui.open();"));
    // Hop 1: drive + assert URL + arrival surface.
    assert!(rendered.contains("await ui.journeyToCustomerDetail();"));
    assert!(rendered.contains("await expect(page).toHaveURL(new RegExp('/customerdetail$'));"));
    assert!(rendered.contains("const customer_detail = new CustomerDetailPage(page);"));
    assert!(rendered.contains("await expect(customer_detail.primaryRoot()).toBeVisible();"));
    // Hop 2 departs from the CURRENT view — ui rebinds, and the xor target
    // asserts its wizard steps.
    assert!(rendered.contains("ui = new CustomerDetailPage(page);"));
    assert!(rendered.contains("await ui.journeyToCustomerEdit();"));
    assert!(rendered.contains("await expect(customer_edit.step('Plan')).toBeVisible();"));
    // Zero raw testids in the spec body.
    assert!(!rendered.contains("getByTestId("), "{rendered}");
}

#[test]
fn auth_spec_renders_gating_family_and_skip_comment() {
    use super::spec_payload::PersonaTest;

    let personas = |permitted: bool| PersonaTest {
        actor: if permitted { "Admin" } else { "Agent" }.to_string(),
        permitted,
        route: "/reviewqueue".to_string(),
        label: "Review Queue".to_string(),
        assert_heading: true,
        primary_testid: None,
        denial_target: "/requestlist".to_string(),
        capabilities: None,
        control_testid: None,
        control_component: None,
    };
    let spec = |personas: Vec<PersonaTest>, gate: bool| super::auth::AuthTestSpec {
        view_name: "ReviewQueue".to_string(),
        label: "Review Queue".to_string(),
        denial_presentation: true,
        required_capabilities: vec!["ApproveRequest".to_string()],
        gate: gate.then_some(super::auth::GatePlan {
            denied: personas
                .iter()
                .filter(|p| !p.permitted)
                .map(|p| (super::auth::persona_id(&p.actor), p.actor.clone()))
                .collect(),
        }),
        personas,
    };

    // Differing access: the gating describe drives the denied persona.
    let rendered = super::auth::render_auth_spec(
        &spec(vec![personas(true), personas(false)], true),
        "../e2e/personas",
    );
    assert!(
        rendered.contains("test.describe('Review Queue gated from agent'"),
        "{rendered}"
    );
    assert!(rendered.contains("asPersona(agent);"));
    assert!(rendered.contains("await ui.expectDenied();"));
    assert!(rendered.contains(
        "await ui.expectDeniedPresentation('actor Agent is denied — ApproveRequest gating');"
    ));

    // No differing access: the skip comment, never a gating describe.
    let skipped =
        super::auth::render_auth_spec(&spec(vec![personas(true)], false), "../e2e/personas");
    assert!(
        skipped.contains("Gating coverage (issue #468 W5) skipped"),
        "{skipped}"
    );
    assert!(!skipped.contains("gated from"));
}
