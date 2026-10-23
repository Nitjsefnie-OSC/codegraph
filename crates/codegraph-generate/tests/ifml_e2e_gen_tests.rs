//! Integration tests for the IFML E2E test generator (`ifml-e2e-test`):
//! render tests, API-gated click-through / form tests, and the schema-less
//! gating rule (render tests only).

use std::collections::HashMap;
use std::path::Path;

use codegraph_config::{DomainConfig, IfmlComponentMappings};
use codegraph_core::mock::MockEngine;
use codegraph_core::traits::GraphIngestor;
use codegraph_core::types::{
    ActorNode, ActorPolicyModel, ActorPolicyNode, CapabilityNode, EdgeProperties, EdgeType,
    EventNode, GrantEdge, ParameterDefinitionNode, PropertyNode, SchemaNode, ViewComponentNode,
    ViewContainerNode,
};
use codegraph_generate::ProjectConfig;
use codegraph_generate::ifml::e2e_test::IfmlE2eTestGenerator;
use codegraph_generate::traits::GlobalGenerator;

const EDITOR_FORM_SPEC: &str = r#"{"type":"form","value":{"fields":[{"name":"name","input":{"type":"text"},"required":true,"validations":[],"values":[]}]}}"#;

fn test_config() -> DomainConfig {
    toml::from_str(
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
    .unwrap()
}

fn workflow_config() -> DomainConfig {
    toml::from_str(
        r#"
[defaults]
api_version = "v1"

[domains.sales]
label = "Sales"
schema_dir = "sales"
postgres_schema = "sales"
entities = ["CustomerType"]

[domains.sales.entity_config.CustomerType.workflow]
status_field = "status"
initial_state = "received"
states = ["received", "review", "done"]
terminal_states = ["done"]
"#,
    )
    .unwrap()
}

fn customer_schema() -> SchemaNode {
    SchemaNode {
        schema_id: "sales/customer_type.json".into(),
        title: "CustomerType".into(),
        description: None,
        schema_type: "object".into(),
        classification: "entity".into(),
        domain: Some("sales".into()),
        namespace: None,
        rel_path: "sales/customer_type.json".into(),
        pg_type: "JSONB".into(),
        rust_type: "customer::Model".into(),
        sea_orm_type: "customer".into(),
        rust_type_name: "CustomerType".into(),
        pg_table_name: "customer".into(),
        api_path_segment: "customer".into(),
        parent_schema: None,
        is_entity: true,
        is_codelist: false,
        is_primitive_wrapper: false,
        has_all_of: false,
        has_one_of: false,
        has_any_of: false,
        has_definitions: false,
        custom_annotations: HashMap::new(),
        access: None,
        annotations: None,
    }
}

fn property(name: &str, rust_type: &str) -> PropertyNode {
    PropertyNode {
        name: name.to_string(),
        prop_type: "string".to_string(),
        description: None,
        format: None,
        is_required: true,
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
        rust_field_type: rust_type.to_string(),
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

async fn ingest_customer_schema(db: &MockEngine) {
    db.ingest_schema(&customer_schema()).await.unwrap();
    db.ingest_property(
        "CustomerType",
        "sales/customer_type.json",
        &property("name", "String"),
    )
    .await
    .unwrap();
    db.ingest_property(
        "CustomerType",
        "sales/customer_type.json",
        &property("age", "i32"),
    )
    .await
    .unwrap();
}

async fn ingest_view(db: &MockEngine, name: &str, label: Option<&str>) {
    ingest_view_with_flags(db, name, label, false).await;
}

async fn ingest_modal_view(db: &MockEngine, name: &str) {
    ingest_view_with_flags(db, name, None, true).await;
}

async fn ingest_view_with_flags(db: &MockEngine, name: &str, label: Option<&str>, is_modal: bool) {
    ingest_guarded_view(db, name, label, is_modal, &[], &[]).await;
}

async fn ingest_guarded_view(
    db: &MockEngine,
    name: &str,
    label: Option<&str>,
    is_modal: bool,
    roles: &[&str],
    requires: &[&str],
) {
    let roles = (!roles.is_empty()).then(|| roles.iter().map(|s| s.to_string()).collect());
    let requires = (!requires.is_empty()).then(|| requires.iter().map(|s| s.to_string()).collect());
    db.ingest_view_container(&ViewContainerNode {
        name: name.to_string(),
        label: label.map(str::to_string),
        is_xor: false,
        is_default: false,
        is_landmark: label.is_some(),
        is_modal,
        conditional_expression: None,
        expr_json: None,
        domain: None,
        module_uses: None,
        roles,
        requires,
    })
    .await
    .unwrap();
}

/// Admin (human) permits `manage_refunds`; Intern (human) holds nothing;
/// Helper (agent, extends Admin) inherits the permit but is excluded from
/// e2e personas.
async fn ingest_policy(db: &MockEngine) {
    let actor = |name: &str, kind: &str, extends: Option<&str>| ActorNode {
        name: name.to_string(),
        kind: Some(kind.to_string()),
        extends: extends.map(str::to_string),
        block: Some("core".to_string()),
    };
    db.ingest_actor_policy(&ActorPolicyModel {
        actors: vec![
            actor("Admin", "human", None),
            actor("Intern", "human", None),
            actor("Helper", "agent", Some("Admin")),
        ],
        capabilities: vec![CapabilityNode {
            name: "manage_refunds".to_string(),
            class: "Refund".to_string(),
            block: None,
        }],
        grants: vec![GrantEdge {
            actor: "Admin".to_string(),
            capability: "manage_refunds".to_string(),
            effect: "permit".to_string(),
            when: None,
            expr_json: None,
            obligations: vec![],
        }],
        policy: ActorPolicyNode {
            blocks: vec!["core".to_string()],
            never_both: vec![],
            purposes: vec![],
            delegations: vec![],
        },
    })
    .await
    .unwrap();
}

async fn ingest_component(
    db: &MockEngine,
    parent: &str,
    name: &str,
    component_type: &str,
    fields: &[&str],
    spec: Option<&str>,
) {
    db.ingest_view_component(&ViewComponentNode {
        name: name.to_string(),
        component_type: component_type.to_string(),
        mode: None,
        entity: Some("Customer".to_string()),
        fields: Some(fields.iter().map(|f| f.to_string()).collect()),
        filter: None,
        api_operation: None,
        spec: spec.map(str::to_string),
        conditional_expression: None,
        expr_json: None,
        domain: None,
    })
    .await
    .unwrap();
    db.ingest_edge(
        &format!("vc:{parent}"),
        &format!("comp:{name}"),
        EdgeType::ContainsViewComponent,
        None,
    )
    .await
    .unwrap();
}

async fn ingest_event(db: &MockEngine, parent: &str, name: &str, event_type: &str) {
    db.ingest_event(&EventNode {
        name: name.to_string(),
        event_type: event_type.to_string(),
        params: None,
        conditional_expression: None,
        expr_json: None,
        requires: Vec::new(),
        domain: None,
    })
    .await
    .unwrap();
    db.ingest_edge(
        &format!("comp:{parent}"),
        &format!("evt:{name}"),
        EdgeType::HasEvent,
        None,
    )
    .await
    .unwrap();
}

async fn ingest_navigation_flow(db: &MockEngine, event: &str, target: &str, binding: Option<&str>) {
    db.ingest_edge(
        &format!("evt:{event}"),
        &format!("vc:{target}"),
        EdgeType::NavigationFlow,
        binding
            .map(|b| EdgeProperties {
                target_param_binding: Some(b.to_string()),
                ..Default::default()
            })
            .as_ref(),
    )
    .await
    .unwrap();
}

async fn ingest_id_param(db: &MockEngine, view: &str, param: &str) {
    db.ingest_parameter_definition(&ParameterDefinitionNode {
        name: param.to_string(),
        direction: "in".to_string(),
        type_ref: "Uuid".to_string(),
        domain: None,
    })
    .await
    .unwrap();
    db.ingest_edge(
        &format!("vc:{view}"),
        &format!("param:{param}"),
        EdgeType::HasParameter,
        None,
    )
    .await
    .unwrap();
}

/// CustomerList --select--> CustomerDetail, plus a CustomerEdit form view
/// whose save event returns to the list.
async fn ingest_ifml_model(db: &MockEngine) {
    ingest_view(db, "CustomerList", Some("Customer Management")).await;
    ingest_view(db, "CustomerDetail", None).await;
    ingest_view(db, "CustomerEdit", None).await;

    ingest_component(db, "CustomerList", "grid", "list", &["name", "age"], None).await;
    ingest_component(db, "CustomerDetail", "info", "details", &["name"], None).await;
    ingest_component(
        db,
        "CustomerEdit",
        "editor",
        "form",
        &["name"],
        Some(EDITOR_FORM_SPEC),
    )
    .await;

    ingest_event(db, "grid", "comp_grid_select", "select").await;
    ingest_navigation_flow(
        db,
        "comp_grid_select",
        "CustomerDetail",
        Some(r#"{"customerId": "row.id"}"#),
    )
    .await;

    ingest_event(db, "editor", "comp_editor_save", "save").await;
    ingest_navigation_flow(db, "comp_editor_save", "CustomerList", None).await;

    ingest_id_param(db, "CustomerDetail", "customerId").await;
    ingest_id_param(db, "CustomerEdit", "customerId").await;
}

async fn generate(
    db: &MockEngine,
    output: &Path,
    mappings: Option<IfmlComponentMappings>,
) -> Vec<codegraph_generate::traits::GeneratedFile> {
    generate_with_config(db, output, mappings, &test_config()).await
}

async fn generate_with_config(
    db: &MockEngine,
    output: &Path,
    mappings: Option<IfmlComponentMappings>,
    config: &DomainConfig,
) -> Vec<codegraph_generate::traits::GeneratedFile> {
    let generator = IfmlE2eTestGenerator::new(output, "svelte").with_mappings(mappings);
    generator
        .generate(
            db,
            config,
            &[],
            &tera::Tera::default(),
            &ProjectConfig::default(),
        )
        .await
        .unwrap()
}

fn content_of(files: &[codegraph_generate::traits::GeneratedFile], suffix: &str) -> String {
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .unwrap_or_else(|| {
            panic!(
                "no file ending in {suffix}; got {:?}",
                files
                    .iter()
                    .map(|f| f.path.display().to_string())
                    .collect::<Vec<_>>()
            )
        })
        .content
        .clone()
}

#[tokio::test]
async fn emits_render_click_through_and_form_tests_with_schemas() {
    let engine = MockEngine::new();
    ingest_customer_schema(&engine).await;
    ingest_ifml_model(&engine).await;
    let dir = tempfile::tempdir().unwrap();

    let files = generate(&engine, dir.path(), None).await;

    // The POM (issue #317): page classes carry every testid; specs drive
    // them through the page object.
    let list_page = content_of(&files, "tests/pages/customer-list-page.ts");
    assert!(
        list_page.contains("static readonly route = '/customerlist';"),
        "{list_page}"
    );
    assert!(list_page.contains("primaryRoot(): Locator"), "{list_page}");
    assert!(
        list_page.contains("getByTestId('grid-table')"),
        "{list_page}"
    );
    assert!(
        list_page.contains("getByRole('heading', { name: 'Customer Management' })"),
        "{list_page}"
    );
    assert!(
        list_page.contains("waitForURL(new RegExp('/customerdetail\\\\?customerId=[^&]+'))"),
        "{list_page}"
    );
    assert!(
        content_of(&files, "tests/pages/support/base-page.ts").contains("export class BasePage"),
        "kernel base-page.ts must be emitted"
    );
    assert!(
        content_of(&files, "tests/pages/support/ux-table.ts").contains("export class UxTable"),
        "kernel ux-table.ts must be emitted"
    );
    assert!(
        content_of(&files, "tests/pages/customer-detail-page.ts").contains("infoName(): Locator"),
        "every view container gets a page class"
    );

    let list_spec = content_of(&files, "tests/ifml/customer-list.spec.ts");
    assert!(
        list_spec.contains("test('renders Customer Management'"),
        "{list_spec}"
    );
    assert!(
        list_spec.contains("const ui = new CustomerListPage(page);"),
        "{list_spec}"
    );
    assert!(list_spec.contains("await ui.open();"), "{list_spec}");
    assert!(
        list_spec.contains("await expect(ui.heading()).toBeVisible();"),
        "{list_spec}"
    );
    assert!(
        list_spec.contains("await ui.navigateToCustomerDetail();"),
        "{list_spec}"
    );
    assert!(
        !list_spec.contains("getByTestId("),
        "zero raw testid construction in spec bodies: {list_spec}"
    );

    let edit_spec = content_of(&files, "tests/ifml/customer-edit.spec.ts");
    assert!(
        edit_spec.contains("request.post('/api/v1/sales/customer'"),
        "{edit_spec}"
    );
    // Validation negatives run on the create path (empty form): no fixture
    // navigation, plain route.
    assert!(
        edit_spec.contains("test('form validation blocks empty submit', async ({ page }) => {"),
        "{edit_spec}"
    );
    assert!(edit_spec.contains("await ui.open();"), "{edit_spec}");
    assert!(
        edit_spec.contains("await ui.submitEditor();"),
        "{edit_spec}"
    );
    assert!(
        edit_spec.contains(
            "await expect(ui.editorInput('name')).toHaveJSProperty('validity.valid', false);"
        ),
        "invalid-state assertions use the Playwright JS-property matcher: {edit_spec}"
    );
    assert!(!edit_spec.contains("toBeInvalid"), "{edit_spec}");
    assert!(
        edit_spec.contains("await ui.fillEditorName('Updated name');"),
        "{edit_spec}"
    );
    assert!(edit_spec.contains("await ui.saveEditor();"), "{edit_spec}");
    let edit_page = content_of(&files, "tests/pages/customer-edit-page.ts");
    assert!(
        edit_page.contains("waitForURL(new RegExp('/customerlist$'))"),
        "the save navigation pattern lives in the page class: {edit_page}"
    );
    assert!(
        edit_spec.contains("expect((persisted.data ?? persisted).name).toBe('Updated name')"),
        "persistence reads through the API envelope: {edit_spec}"
    );

    assert!(content_of(&files, "playwright.config.ts").contains("baseURL"));
    assert!(content_of(&files, "package.json").contains("@playwright/test"));
}

#[tokio::test]
async fn without_schemas_emits_render_tests_only() {
    let engine = MockEngine::new();
    ingest_ifml_model(&engine).await;
    let dir = tempfile::tempdir().unwrap();

    let files = generate(&engine, dir.path(), None).await;

    let list_spec = content_of(&files, "tests/ifml/customer-list.spec.ts");
    assert!(
        list_spec.contains("test('renders Customer Management'"),
        "{list_spec}"
    );
    let list_page = content_of(&files, "tests/pages/customer-list-page.ts");
    assert!(
        list_page.contains("getByTestId('grid-table')"),
        "the render assertion target lives in the page class: {list_page}"
    );
    assert!(
        !files.iter().any(|f| f.content.contains("request.post")),
        "no API-dependent tests may be emitted without schemas"
    );
    assert!(
        !files.iter().any(|f| f.content.contains("waitForURL")),
        "no navigation-flow tests may be emitted without schemas"
    );
    assert!(
        files
            .iter()
            .any(|f| f.path.to_string_lossy().ends_with("playwright.config.ts")),
        "render-only runs still scaffold the Playwright harness"
    );
}

#[tokio::test]
async fn mapped_components_use_mapping_testids() {
    let engine = MockEngine::new();
    ingest_customer_schema(&engine).await;
    ingest_ifml_model(&engine).await;
    let dir = tempfile::tempdir().unwrap();

    let mappings: IfmlComponentMappings = toml::from_str(
        r#"
[[component]]
type = "list"
path = "$lib/components/DataTable.svelte"
export = "DataTable"
testids = { root = "data-table", row = "data-row" }
"#,
    )
    .unwrap();

    let files = generate(&engine, dir.path(), Some(mappings)).await;
    let list_page = content_of(&files, "tests/pages/customer-list-page.ts");
    assert!(
        list_page.contains("getByTestId('data-table')"),
        "the mapped root testid lands in the page class: {list_page}"
    );
    assert!(
        list_page.contains("getByTestId('data-row').first()"),
        "the mapped row testid lands in the page class: {list_page}"
    );
    let list_spec = content_of(&files, "tests/ifml/customer-list.spec.ts");
    assert!(
        list_spec.contains("await ui.navigateToCustomerDetail();"),
        "{list_spec}"
    );
    assert!(
        !list_spec.contains("getByTestId("),
        "zero raw testid construction in spec bodies: {list_spec}"
    );
}

#[tokio::test]
async fn modal_click_through_asserts_dialog_and_close() {
    let engine = MockEngine::new();
    ingest_customer_schema(&engine).await;

    ingest_view(&engine, "CustomerList", Some("Customer Management")).await;
    ingest_modal_view(&engine, "CustomerDialog").await;
    ingest_component(
        &engine,
        "CustomerList",
        "grid",
        "list",
        &["name", "age"],
        None,
    )
    .await;
    ingest_component(
        &engine,
        "CustomerDialog",
        "editor",
        "form",
        &["name"],
        Some(EDITOR_FORM_SPEC),
    )
    .await;
    ingest_event(&engine, "grid", "comp_grid_select", "select").await;
    ingest_navigation_flow(
        &engine,
        "comp_grid_select",
        "CustomerDialog",
        Some(r#"{"customerId": "row.id"}"#),
    )
    .await;
    ingest_id_param(&engine, "CustomerDialog", "customerId").await;

    let mappings: IfmlComponentMappings = toml::from_str(
        r#"
[[component]]
role = "modal-view"
path = "$lib/components/Dialog.svelte"
export = "Dialog"
testids = { root = "customer-modal" }
"#,
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();

    let files = generate(&engine, dir.path(), Some(mappings)).await;
    let list_page = content_of(&files, "tests/pages/customer-list-page.ts");
    assert!(
        list_page.contains("async navigateToCustomerDialog()"),
        "{list_page}"
    );
    assert!(
        list_page
            .contains("waitForURL(new RegExp('/customerdialog\\\\?customerId=[^&]+&dialog=open'))"),
        "{list_page}"
    );
    assert!(
        list_page.contains("async closeCustomerDialogModal()"),
        "{list_page}"
    );
    assert!(
        list_page.contains("await expect(this.page.getByTestId('customer-modal')).toBeVisible();"),
        "{list_page}"
    );
    assert!(
        list_page.contains("await this.page.getByTestId('customerdialog-modal-close').click();"),
        "{list_page}"
    );
    assert!(
        list_page.contains("waitForURL(new RegExp('/customerlist$'))"),
        "{list_page}"
    );
    let list_spec = content_of(&files, "tests/ifml/customer-list.spec.ts");
    assert!(
        list_spec.contains("await ui.navigateToCustomerDialog();")
            && list_spec.contains("await ui.closeCustomerDialogModal();"),
        "{list_spec}"
    );
}

#[tokio::test]
async fn non_modal_click_through_keeps_plain_target_pattern() {
    let engine = MockEngine::new();
    ingest_customer_schema(&engine).await;
    ingest_ifml_model(&engine).await;

    let mappings: IfmlComponentMappings = toml::from_str(
        r#"
[[component]]
role = "action-control"
path = "$lib/components/Button.svelte"
"#,
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();

    let files = generate(&engine, dir.path(), Some(mappings)).await;
    let list_page = content_of(&files, "tests/pages/customer-list-page.ts");
    assert!(
        list_page.contains("waitForURL(new RegExp('/customerdetail\\\\?customerId=[^&]+'))"),
        "{list_page}"
    );
    assert!(
        !list_page.contains("dialog=open"),
        "non-modal targets must not gain the dialog param: {list_page}"
    );
    let list_spec = content_of(&files, "tests/ifml/customer-list.spec.ts");
    assert!(
        list_spec.contains("await ui.navigateToCustomerDetail();"),
        "{list_spec}"
    );
}

#[tokio::test]
async fn form_tests_use_mapped_button_testid() {
    let engine = MockEngine::new();
    ingest_customer_schema(&engine).await;
    ingest_ifml_model(&engine).await;

    let mappings: IfmlComponentMappings = toml::from_str(
        r#"
[[component]]
role = "action-control"
path = "$lib/components/Button.svelte"
export = "Button"
testids = { root = "ui-save-btn" }
"#,
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();

    let files = generate(&engine, dir.path(), Some(mappings)).await;
    let edit_page = content_of(&files, "tests/pages/customer-edit-page.ts");
    assert!(
        edit_page.contains("getByTestId('ui-save-btn')"),
        "the mapped button testid lands in the page class: {edit_page}"
    );
    assert!(
        !edit_page.contains("getByTestId('editor-submit')"),
        "mapped button testid must replace the fallback: {edit_page}"
    );
    let edit_spec = content_of(&files, "tests/ifml/customer-edit.spec.ts");
    assert!(
        edit_spec.contains("await ui.submitEditor();"),
        "{edit_spec}"
    );
    assert!(
        !edit_spec.contains("editor-submit"),
        "specs never carry testids: {edit_spec}"
    );
}

#[tokio::test]
async fn scaffold_files_are_never_overwritten() {
    let engine = MockEngine::new();
    ingest_customer_schema(&engine).await;
    ingest_ifml_model(&engine).await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("playwright.config.ts"), "// sentinel").unwrap();
    std::fs::write(dir.path().join("package.json"), "{}").unwrap();

    let files = generate(&engine, dir.path(), None).await;

    assert!(
        !files
            .iter()
            .any(|f| f.path.to_string_lossy().ends_with("playwright.config.ts")),
        "existing playwright.config.ts must not be regenerated"
    );
    assert!(
        !files
            .iter()
            .any(|f| f.path.to_string_lossy().ends_with("package.json")),
        "existing package.json must not be regenerated"
    );
    assert!(
        files
            .iter()
            .any(|f| f.path.to_string_lossy().ends_with(".spec.ts")),
        "specs are still emitted"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("playwright.config.ts")).unwrap(),
        "// sentinel"
    );
}

#[tokio::test]
async fn non_svelte_frameworks_emit_nothing() {
    let engine = MockEngine::new();
    ingest_customer_schema(&engine).await;
    ingest_ifml_model(&engine).await;
    let dir = tempfile::tempdir().unwrap();

    let generator = IfmlE2eTestGenerator::new(dir.path(), "react");
    let files = generator
        .generate(
            &engine,
            &test_config(),
            &[],
            &tera::Tera::default(),
            &ProjectConfig::default(),
        )
        .await
        .unwrap();
    assert!(files.is_empty());
}

#[tokio::test]
async fn render_tests_assert_shell_nav_when_mapped() {
    let engine = MockEngine::new();
    ingest_ifml_model(&engine).await;
    let dir = tempfile::tempdir().unwrap();

    let mappings: IfmlComponentMappings = toml::from_str(
        r#"
[[component]]
role = "shell"
path = "$lib/components/Nav.svelte"
export = "Nav"
testids = { root = "side-nav" }
"#,
    )
    .unwrap();
    let files = generate(&engine, dir.path(), Some(mappings)).await;
    let list_spec = content_of(&files, "tests/ifml/customer-list.spec.ts");
    assert!(
        list_spec.contains("await expect(ui.navRoot()).toBeVisible();"),
        "{list_spec}"
    );
    let list_page = content_of(&files, "tests/pages/customer-list-page.ts");
    assert!(
        list_page.contains("getByTestId('side-nav')"),
        "the shell nav testid lives in the page class: {list_page}"
    );
}

#[tokio::test]
async fn render_tests_omit_nav_assertion_without_shell_mapping() {
    let engine = MockEngine::new();
    ingest_ifml_model(&engine).await;
    let dir = tempfile::tempdir().unwrap();

    let files = generate(&engine, dir.path(), None).await;
    let list_spec = content_of(&files, "tests/ifml/customer-list.spec.ts");
    assert!(
        !list_spec.contains("side-nav"),
        "unmapped shell must not add nav assertions: {list_spec}"
    );
}

#[tokio::test]
async fn render_tests_assert_mapped_container_wrapper() {
    let engine = MockEngine::new();

    engine
        .ingest_view_container(&ViewContainerNode {
            name: "Checkout".to_string(),
            label: Some("Checkout".to_string()),
            is_xor: true,
            is_default: false,
            is_landmark: false,
            is_modal: false,
            conditional_expression: None,
            expr_json: None,
            domain: None,
            module_uses: None,
            roles: None,
            requires: None,
        })
        .await
        .unwrap();
    ingest_component(&engine, "Checkout", "editor", "form", &["name"], None).await;

    let dir = tempfile::tempdir().unwrap();
    let mappings: IfmlComponentMappings = toml::from_str(
        r#"
[[component]]
role = "presentation-container"
path = "$lib/components/Card.svelte"
export = "Card"
testids = { root = "card" }
"#,
    )
    .unwrap();
    let files = generate(&engine, dir.path(), Some(mappings)).await;
    let spec = content_of(&files, "tests/ifml/checkout.spec.ts");
    assert!(
        spec.contains("await expect(ui.containerRoot()).toBeVisible();"),
        "{spec}"
    );
    let page = content_of(&files, "tests/pages/checkout-page.ts");
    assert!(
        page.contains("getByTestId('card')"),
        "the wrapper testid lives in the page class: {page}"
    );
}

#[tokio::test]
async fn form_fixtures_derive_typed_values_from_spec_fields() {
    let engine = MockEngine::new();
    engine.ingest_schema(&customer_schema()).await.unwrap();
    engine
        .ingest_property(
            "CustomerType",
            "sales/customer_type.json",
            &property("amount", "i64"),
        )
        .await
        .unwrap();
    engine
        .ingest_property(
            "CustomerType",
            "sales/customer_type.json",
            &property("urgent", "bool"),
        )
        .await
        .unwrap();
    engine
        .ingest_property(
            "CustomerType",
            "sales/customer_type.json",
            &property("submittedAt", "Option< DateTime < Utc > >"),
        )
        .await
        .unwrap();
    engine
        .ingest_property(
            "CustomerType",
            "sales/customer_type.json",
            &property("reason", "RefundReasonCodeList"),
        )
        .await
        .unwrap();

    ingest_view(&engine, "CustomerEdit", None).await;
    // Typed form specs declare fields in the spec, not in `fields`.
    let typed_spec = r#"{"type":"form","value":{"fields":[
        {"name":"name","input":{"type":"text"},"required":true,"validations":[],"values":[]},
        {"name":"amount","input":{"type":"number"},"required":false,"validations":[],"values":[]},
        {"name":"urgent","input":{"type":"checkbox"},"required":false,"validations":[],"values":[]},
        {"name":"submittedAt","input":{"type":"dateTime"},"required":false,"validations":[],"values":[]},
        {"name":"reason","input":{"type":"dropdown"},"required":false,"validations":[],"values":["Damaged","WrongItem"]}
    ]}}"#;
    engine
        .ingest_view_component(&ViewComponentNode {
            name: "editor".to_string(),
            component_type: "form".to_string(),
            mode: Some("edit".to_string()),
            entity: Some("Customer".to_string()),
            fields: None,
            filter: None,
            api_operation: None,
            spec: Some(typed_spec.to_string()),
            conditional_expression: None,
            expr_json: None,
            domain: None,
        })
        .await
        .unwrap();
    engine
        .ingest_edge(
            "vc:CustomerEdit",
            "comp:editor",
            EdgeType::ContainsViewComponent,
            None,
        )
        .await
        .unwrap();
    ingest_event(&engine, "editor", "comp_editor_save", "save").await;
    ingest_navigation_flow(&engine, "comp_editor_save", "CustomerList", None).await;
    ingest_id_param(&engine, "CustomerEdit", "customerId").await;
    ingest_view(&engine, "CustomerList", Some("Customer Management")).await;

    let dir = tempfile::tempdir().unwrap();
    let files = generate(&engine, dir.path(), None).await;
    let edit_spec = content_of(&files, "tests/ifml/customer-edit.spec.ts");

    assert!(edit_spec.contains("'name': 'Test name'"), "{edit_spec}");
    assert!(
        edit_spec.contains("'amount': 42"),
        "numeric fields must post numbers: {edit_spec}"
    );
    assert!(
        edit_spec.contains("'urgent': true"),
        "boolean fields must post booleans: {edit_spec}"
    );
    assert!(
        edit_spec.contains("'submittedAt': '2024-01-15T10:30:00Z'"),
        "datetime fields must post ISO strings: {edit_spec}"
    );
    assert!(
        edit_spec.contains("'reason': 'Damaged'"),
        "codelist fields must post the first declared value: {edit_spec}"
    );
}

#[tokio::test]
async fn stale_specs_are_removed_on_regeneration() {
    let engine = MockEngine::new();
    ingest_customer_schema(&engine).await;
    ingest_ifml_model(&engine).await;
    let dir = tempfile::tempdir().unwrap();
    let specs_dir = dir.path().join("tests").join("ifml");
    let pages_dir = dir.path().join("tests").join("pages");
    let support_dir = pages_dir.join("support");
    std::fs::create_dir_all(&specs_dir).unwrap();
    std::fs::create_dir_all(&support_dir).unwrap();
    std::fs::write(specs_dir.join("removed-view.spec.ts"), "// stale").unwrap();
    std::fs::write(specs_dir.join("removed-view.workflow.spec.ts"), "// stale").unwrap();
    std::fs::write(specs_dir.join("customer-list.spec.ts"), "// existing").unwrap();
    // Stale POM page classes (#317): removed views lose their page class;
    // the kernel support files and non-page files stay.
    std::fs::write(pages_dir.join("removed-view-page.ts"), "// stale").unwrap();
    std::fs::write(pages_dir.join("customer-list-page.ts"), "// existing").unwrap();
    std::fs::write(support_dir.join("base-page.ts"), "// kernel").unwrap();
    std::fs::write(dir.path().join("unrelated.txt"), "keep").unwrap();

    generate(&engine, dir.path(), None).await;

    assert!(
        !specs_dir.join("removed-view.spec.ts").exists(),
        "stale view spec must be removed"
    );
    assert!(
        !specs_dir.join("removed-view.workflow.spec.ts").exists(),
        "stale workflow spec must be removed"
    );
    assert!(
        specs_dir.join("customer-list.spec.ts").exists(),
        "active view specs are regenerated"
    );
    assert!(
        !pages_dir.join("removed-view-page.ts").exists(),
        "stale page classes must be removed"
    );
    assert!(
        pages_dir.join("customer-list-page.ts").exists(),
        "active page classes are not stale-removed"
    );
    assert!(
        support_dir.join("base-page.ts").exists(),
        "the kernel support files are never stale-cleaned"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("unrelated.txt")).unwrap(),
        "keep",
        "non-spec files are never touched"
    );
}

#[tokio::test]
async fn playwright_config_gates_bearer_auth_behind_env() {
    let engine = MockEngine::new();
    ingest_ifml_model(&engine).await;
    let dir = tempfile::tempdir().unwrap();

    let files = generate(&engine, dir.path(), None).await;
    let config = content_of(&files, "playwright.config.ts");

    assert!(config.contains("baseURL"), "{config}");
    assert!(
        config.contains("extraHTTPHeaders"),
        "config must carry the extraHTTPHeaders hook: {config}"
    );
    assert!(
        config.contains("process.env.IFML_API_KEY"),
        "auth must be env-gated: {config}"
    );
    assert!(
        config.contains("Bearer ${process.env.IFML_API_KEY}"),
        "the key rides the authorization header: {config}"
    );
}

#[tokio::test]
async fn workflow_spec_emitted_for_schema_backed_workflow_entity() {
    let engine = MockEngine::new();
    ingest_customer_schema(&engine).await;
    ingest_ifml_model(&engine).await;
    let dir = tempfile::tempdir().unwrap();

    let files = generate_with_config(&engine, dir.path(), None, &workflow_config()).await;

    let list_spec = content_of(&files, "tests/ifml/customer-list.workflow.spec.ts");
    assert!(
        list_spec.contains("test.describe('Customer Management workflow'"),
        "{list_spec}"
    );
    assert!(
        list_spec.contains("shows the initial workflow state for grid"),
        "{list_spec}"
    );
    assert!(
        list_spec.contains("request.post('/api/v1/sales/customer'"),
        "{list_spec}"
    );
    assert!(list_spec.contains("'name': 'Test name'"), "{list_spec}");
    assert!(list_spec.contains("'age': 42"), "{list_spec}");
    assert!(list_spec.contains("await ui.open();"), "{list_spec}");
    assert!(
        list_spec.contains("await ui.expectGridState('received');"),
        "{list_spec}"
    );

    let detail_spec = content_of(&files, "tests/ifml/customer-detail.workflow.spec.ts");
    assert!(
        detail_spec.contains("await ui.open({ customerId: created.data?.id ?? created.id });"),
        "workflow specs read the fixture id through the API envelope: {detail_spec}"
    );
    assert!(
        detail_spec.contains("await ui.expectInfoState('received');"),
        "{detail_spec}"
    );

    let edit_spec = content_of(&files, "tests/ifml/customer-edit.workflow.spec.ts");
    assert!(
        edit_spec.contains("await ui.open({ customerId: created.data?.id ?? created.id });"),
        "{edit_spec}"
    );

    // The state-badge testids live in the page classes (#317).
    let list_page = content_of(&files, "tests/pages/customer-list-page.ts");
    assert!(
        list_page.contains("getByTestId('grid-state')"),
        "{list_page}"
    );
    let detail_page = content_of(&files, "tests/pages/customer-detail-page.ts");
    assert!(
        detail_page.contains("getByTestId('info-state')"),
        "{detail_page}"
    );
}

#[tokio::test]
async fn workflow_spec_not_emitted_without_workflow_config() {
    let engine = MockEngine::new();
    ingest_customer_schema(&engine).await;
    ingest_ifml_model(&engine).await;
    let dir = tempfile::tempdir().unwrap();

    let files = generate(&engine, dir.path(), None).await;

    assert!(
        !files
            .iter()
            .any(|f| f.path.to_string_lossy().ends_with(".workflow.spec.ts")),
        "no workflow config must mean no workflow specs"
    );
    assert!(
        files
            .iter()
            .any(|f| f.path.to_string_lossy().ends_with("customer-list.spec.ts")),
        "regular specs are still emitted"
    );
}

#[tokio::test]
async fn workflow_spec_requires_schema_backing() {
    let engine = MockEngine::new();
    ingest_ifml_model(&engine).await;
    let dir = tempfile::tempdir().unwrap();

    let files = generate_with_config(&engine, dir.path(), None, &workflow_config()).await;

    assert!(
        !files
            .iter()
            .any(|f| f.path.to_string_lossy().ends_with(".workflow.spec.ts")),
        "schema-less runs must not emit workflow specs"
    );
}

/// CustomerList (unguarded denial target) plus a roles-guarded AdminConsole.
async fn ingest_guarded_model(db: &MockEngine, roles: &[&str], requires: &[&str]) {
    ingest_view(db, "CustomerList", Some("Customer Management")).await;
    ingest_component(db, "CustomerList", "grid", "list", &["name"], None).await;
    ingest_guarded_view(
        db,
        "AdminConsole",
        Some("Admin Console"),
        false,
        roles,
        requires,
    )
    .await;
    ingest_component(db, "AdminConsole", "grid", "list", &["name"], None).await;
}

#[tokio::test]
async fn persona_tests_emitted_per_human_actor_with_policy() {
    let engine = MockEngine::new();
    ingest_guarded_model(&engine, &["Admin"], &[]).await;
    ingest_policy(&engine).await;
    let dir = tempfile::tempdir().unwrap();

    let files = generate(&engine, dir.path(), None).await;
    let spec = content_of(&files, "tests/ifml/admin-console.spec.ts");

    assert!(
        spec.contains("test('actor Admin views Admin Console', async ({ page }) => {"),
        "{spec}"
    );
    assert!(
        spec.contains("(globalThis as any).__USER_ROLES__ = ['Admin', 'admin'];"),
        "{spec}"
    );
    assert!(spec.contains("await ui.open();"), "{spec}");
    assert!(
        spec.contains("await expect(ui.heading()).toBeVisible();"),
        "{spec}"
    );

    assert!(
        spec.contains(
            "test('actor Intern is redirected from Admin Console', async ({ page }) => {"
        ),
        "{spec}"
    );
    assert!(
        spec.contains("(globalThis as any).__USER_ROLES__ = ['Intern', 'intern'];"),
        "{spec}"
    );
    assert!(spec.contains("await ui.expectDenied();"), "{spec}");
    let page = content_of(&files, "tests/pages/admin-console-page.ts");
    assert!(
        page.contains("waitForURL('/customerlist', { timeout: 5000 })"),
        "the denial target lives in the page class: {page}"
    );

    assert!(
        !spec.contains("Helper"),
        "agent-kind actors must not become e2e personas: {spec}"
    );
}

#[tokio::test]
async fn capability_only_view_personas_seed_user_capabilities() {
    let engine = MockEngine::new();
    ingest_guarded_model(&engine, &[], &["manage_refunds"]).await;
    ingest_policy(&engine).await;
    let dir = tempfile::tempdir().unwrap();

    let files = generate(&engine, dir.path(), None).await;
    let spec = content_of(&files, "tests/ifml/admin-console.spec.ts");

    assert!(
        spec.contains("(globalThis as any).__USER_CAPABILITIES__ = ['manage_refunds'];"),
        "{spec}"
    );
    assert!(
        spec.contains("test('actor Admin views Admin Console'"),
        "{spec}"
    );
    assert!(
        spec.contains("test('actor Intern is redirected from Admin Console'"),
        "{spec}"
    );
}

#[tokio::test]
async fn no_policy_emits_no_persona_tests() {
    let engine = MockEngine::new();
    ingest_guarded_model(&engine, &["Admin"], &[]).await;
    let dir = tempfile::tempdir().unwrap();

    let files = generate(&engine, dir.path(), None).await;

    // Guarded views get no plain render test (they redirect unauthenticated
    // visitors) and no personas without a policy: nothing spec-worthy covers
    // the view, so no spec file is emitted at all.
    assert!(
        !files
            .iter()
            .any(|f| f.path.to_string_lossy().ends_with("admin-console.spec.ts")),
        "guarded view without policy must not emit a spec: {files:?}"
    );
    assert!(
        !files
            .iter()
            .any(|f| f.content.contains("__USER_CAPABILITIES__")),
        "no policy must mean no capability seeding anywhere"
    );
    // Unguarded views keep their render specs.
    assert!(
        files
            .iter()
            .any(|f| f.path.to_string_lossy().ends_with("customer-list.spec.ts")),
        "unguarded views still render-spec"
    );
}

/// CustomerList (unguarded denial target) plus a guarded RefundEdit form view.
async fn ingest_guarded_form_model(db: &MockEngine, roles: &[&str], requires: &[&str]) {
    ingest_view(db, "CustomerList", Some("Customer Management")).await;
    ingest_guarded_view(
        db,
        "RefundEdit",
        Some("Refund Edit"),
        false,
        roles,
        requires,
    )
    .await;
    ingest_component(
        db,
        "RefundEdit",
        "editor",
        "form",
        &["name"],
        Some(EDITOR_FORM_SPEC),
    )
    .await;
}

#[tokio::test]
async fn persona_test_asserts_gated_submit_for_permitted_actor_only() {
    let engine = MockEngine::new();
    ingest_guarded_form_model(&engine, &[], &["manage_refunds"]).await;
    ingest_policy(&engine).await;
    let dir = tempfile::tempdir().unwrap();

    let files = generate(&engine, dir.path(), None).await;
    let spec = content_of(&files, "tests/ifml/refund-edit.spec.ts");

    let admin_start = spec.find("actor Admin views").expect("admin persona");
    let intern_start = spec
        .find("actor Intern is redirected")
        .expect("intern persona");
    let intern_end = spec[intern_start..]
        .find("\n\ttest(")
        .map(|i| intern_start + i)
        .unwrap_or(spec.len());
    let admin_block = &spec[admin_start..intern_start];
    assert!(
        admin_block.contains("await expect(ui.editorSubmit()).toBeVisible();"),
        "permitted persona must assert the gated control is visible: {spec}"
    );
    let intern_block = &spec[intern_start..intern_end];
    assert!(
        !intern_block.contains("editorSubmit"),
        "denied persona redirects before any control assertion: {intern_block}"
    );
    let page = content_of(&files, "tests/pages/refund-edit-page.ts");
    assert!(
        page.contains("getByTestId('editor-submit')"),
        "the control testid lives in the page class: {page}"
    );
}

#[tokio::test]
async fn persona_test_skips_control_assertion_for_mapped_forms() {
    let engine = MockEngine::new();
    ingest_guarded_form_model(&engine, &[], &["manage_refunds"]).await;
    ingest_policy(&engine).await;
    let dir = tempfile::tempdir().unwrap();

    let mappings: IfmlComponentMappings = toml::from_str(
        r#"
[[component]]
type = "form"
path = "$lib/components/RefundForm.svelte"
export = "RefundForm"
testids = { root = "refund-form" }
"#,
    )
    .unwrap();

    let files = generate(&engine, dir.path(), Some(mappings)).await;
    let spec = content_of(&files, "tests/ifml/refund-edit.spec.ts");

    assert!(
        spec.contains("await expect(ui.primaryRoot()).toBeVisible();"),
        "mapped form root is still asserted through the page class: {spec}"
    );
    assert!(
        !spec.contains("editorSubmit"),
        "mapped forms own their internals — no fallback control assertion: {spec}"
    );
    let page = content_of(&files, "tests/pages/refund-edit-page.ts");
    assert!(
        page.contains("getByTestId('refund-form')"),
        "the mapped root lands in the page class: {page}"
    );
}
