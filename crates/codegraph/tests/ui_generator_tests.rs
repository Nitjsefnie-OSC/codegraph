//! Tests for UI generators (C-2: zero test coverage for UI generators).
//!
//! Covers: UiPageGenerator, UiFormGenerator, UiStoreGenerator,
//! UiScaffoldGenerator, UiTypeGenerator, UiDomainLayoutGenerator.
//! Also covers unit tests for form helper functions.

use codegraph::generate;
use codegraph::generate::GenerationEntry;
use codegraph::generate::template_engine;
use codegraph::generate::traits::{DomainGenerator, EntityGenerator, GlobalGenerator};
use codegraph::generate::ui::form;
use codegraph_core::mock::MockEngine;
use codegraph_core::types::{PropertyNode, SchemaNode};
use std::path::Path;

fn test_domain_config() -> codegraph_config::DomainConfig {
    codegraph_config::config::parse_domain_config(Path::new("tests/fixtures/domains.toml")).unwrap()
}

fn test_tera() -> tera::Tera {
    let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    template_engine::create_tera(&template_dir).unwrap()
}

fn test_project_config() -> codegraph::generate::ProjectConfig {
    codegraph::generate::ProjectConfig::default()
}

fn candidate_schema() -> SchemaNode {
    SchemaNode {
        namespace: None,
        schema_id: "recruiting/json/CandidateType.json".to_string(),
        title: "CandidateType".to_string(),
        description: Some("A person requesting consideration for a position".to_string()),
        schema_type: "object".to_string(),
        classification: "entity_reference".to_string(),
        domain: Some("recruiting".to_string()),
        rel_path: "recruiting/json/CandidateType.json".to_string(),
        pg_type: "UUID".to_string(),
        rust_type: "Uuid".to_string(),
        sea_orm_type: "Uuid".to_string(),
        rust_type_name: "Candidate".to_string(),
        pg_table_name: "candidate".to_string(),
        api_path_segment: "candidates".to_string(),
        parent_schema: None,
        is_entity: true,
        is_codelist: false,
        is_primitive_wrapper: false,
        has_all_of: true,
        has_one_of: false,
        has_any_of: false,
        has_definitions: true,
        custom_annotations: Default::default(),
        access: None,
        annotations: None,
    }
}

fn candidate_properties() -> Vec<PropertyNode> {
    vec![
        PropertyNode {
            is_id: false,
            name: "givenName".to_string(),
            prop_type: "string".to_string(),
            description: Some("The person's given name".to_string()),
            format: None,
            is_required: true,
            is_nullable: false,
            is_array: false,
            min_items: None,
            max_items: None,
            pattern: None,
            min_length: None,
            max_length: None,
            minimum: None,
            maximum: None,
            pg_column_name: "given_name".to_string(),
            pg_column_type: "TEXT".to_string(),
            rust_field_name: "given_name".to_string(),
            rust_field_type: "String".to_string(),
            sea_orm_type: "Text".to_string(),
            render_strategy: "direct_column".to_string(),
            ref_target: None,
            classification: Some("primitive_wrapper".to_string()),
            projection: None,
            classification_kind: None,
            ui_override_detail: None,
            ui_override_list_cell: None,
            ui_override_form: None,
            ui_override_inline: None,
            type_expr: None,
        },
        PropertyNode {
            is_id: false,
            name: "familyName".to_string(),
            prop_type: "string".to_string(),
            description: Some("The person's family name".to_string()),
            format: None,
            is_required: false,
            is_nullable: true,
            is_array: false,
            min_items: None,
            max_items: None,
            pattern: None,
            min_length: None,
            max_length: None,
            minimum: None,
            maximum: None,
            pg_column_name: "family_name".to_string(),
            pg_column_type: "TEXT".to_string(),
            rust_field_name: "family_name".to_string(),
            rust_field_type: "String".to_string(),
            sea_orm_type: "Text".to_string(),
            render_strategy: "direct_column".to_string(),
            ref_target: None,
            classification: Some("primitive_wrapper".to_string()),
            projection: None,
            classification_kind: None,
            ui_override_detail: None,
            ui_override_list_cell: None,
            ui_override_form: None,
            ui_override_inline: None,
            type_expr: None,
        },
    ]
}

fn setup_mock() -> MockEngine {
    MockEngine::builder()
        .with_schema(candidate_schema())
        .with_properties("CandidateType", candidate_properties())
        .build()
}

fn test_generation_order() -> Vec<GenerationEntry> {
    vec![GenerationEntry {
        schema_title: "CandidateType".to_string(),
        domain: "recruiting".to_string(),
        pg_schema: "recruiting".to_string(),
        is_cyclic: false,
    }]
}

// === UiPageGenerator Tests ===

#[tokio::test]
async fn ui_page_generator_produces_list_and_detail_pages() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-page");

    let generator = generate::ui::page::UiPageGenerator::new(&output_dir);
    let files = generator
        .generate(
            &mock,
            "CandidateType",
            "recruiting",
            &config,
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    assert!(!files.is_empty(), "UI page generator should produce files");

    // Should have list page
    let list_page = files
        .iter()
        .find(|f| {
            f.path.to_string_lossy().ends_with("+page.svelte")
                && !f.path.to_string_lossy().contains("[id]")
        })
        .expect("Should have a list page");
    assert!(
        !list_page.content.is_empty(),
        "List page should not be empty"
    );
    assert!(
        !list_page.content.contains("{variant}="),
        "Should not contain invalid Svelte syntax"
    );

    // Should have detail page
    let detail_page = files.iter().find(|f| {
        f.path.to_string_lossy().contains("[candidate_id]")
            && f.path.to_string_lossy().ends_with("+page.svelte")
            && !f.path.to_string_lossy().contains("edit")
    });
    assert!(detail_page.is_some(), "Should have a detail page");
}

#[tokio::test]
async fn ui_page_generator_produces_create_page() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-page-create");

    let generator = generate::ui::page::UiPageGenerator::new(&output_dir);
    let files = generator
        .generate(
            &mock,
            "CandidateType",
            "recruiting",
            &config,
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    let create_page = files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("new"));
    assert!(
        create_page.is_some(),
        "Should have a create page for entities with create operation"
    );
}

#[tokio::test]
async fn ui_page_generator_skips_delete_page_when_no_delete_op() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-page-ops");

    // CandidateType has operations = ["create", "read", "update", "list"] — no delete
    let generator = generate::ui::page::UiPageGenerator::new(&output_dir);
    let files = generator
        .generate(
            &mock,
            "CandidateType",
            "recruiting",
            &config,
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    // All generated page file names should not have "delete" as a separate page
    // (delete is an action on detail page, not a separate route)
    for f in &files {
        let file_name = f.path.file_name().unwrap_or_default().to_string_lossy();
        assert!(
            !file_name.contains("delete"),
            "Should not have a delete page route file"
        );
    }
}

// === UiFormGenerator Tests ===

#[tokio::test]
async fn ui_form_generator_produces_form_component() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-form");

    let generator = generate::ui::form::UiFormGenerator::new(&output_dir);
    let files = generator
        .generate(
            &mock,
            "CandidateType",
            "recruiting",
            &config,
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    assert_eq!(files.len(), 1, "Should produce one form component");
    assert!(
        files[0]
            .path
            .to_string_lossy()
            .contains("CandidateForm.svelte"),
        "Should have CandidateForm.svelte"
    );
    assert!(!files[0].content.is_empty(), "Form should not be empty");
    // Verify it contains Svelte script tag
    assert!(
        files[0].content.contains("<script"),
        "Should contain a script tag"
    );
    // W2 type-safe seams (#468): initialData is the typed entity Response,
    // read via optional chaining — never string indexing.
    assert!(
        files[0].content.contains(
            "import type { CreateCandidateRequest, UpdateCandidateRequest, CandidateResponse } from '#lib/api/types.ts';"
        ),
        "Form should import the typed Response from '#lib/api/types.ts'"
    );
    assert!(
        files[0]
            .content
            .contains("initialData?: CandidateResponse;"),
        "initialData prop should be typed as the entity Response"
    );
    assert!(
        !files[0].content.contains("initialData['"),
        "Form must not string-index initialData"
    );
    assert!(
        files[0].content.contains("String(initialData?."),
        "Form should still coerce via typed optional-chained access"
    );
}

#[tokio::test]
async fn ui_cosmos_form_generator_types_initial_data() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-cosmos-form");

    let generator = generate::ui::cosmos_entity_form::CosmosEntityFormGenerator::new(&output_dir);
    let files = generator
        .generate(
            &mock,
            "CandidateType",
            "recruiting",
            &config,
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    assert_eq!(files.len(), 1, "Should produce one cosmos form component");
    assert!(
        files[0]
            .content
            .contains("import type { CandidateResponse } from '#lib/api/types.ts';"),
        "Cosmos form should import the typed Response from '#lib/api/types.ts'"
    );
    assert!(
        files[0]
            .content
            .contains("initialData?: CandidateResponse;"),
        "Cosmos form initialData prop should be typed as the entity Response"
    );
    assert!(
        !files[0].content.contains("initialData['"),
        "Cosmos form must not string-index initialData"
    );
    assert!(
        files[0].content.contains("String(initialData?."),
        "Cosmos form should still coerce via typed optional-chained access"
    );
}

// === UiStoreGenerator Tests ===

#[tokio::test]
async fn ui_store_generator_produces_store_file() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-store");

    let generator = generate::ui::store::UiStoreGenerator::new(&output_dir);
    let files = generator
        .generate(
            &mock,
            "CandidateType",
            "recruiting",
            &config,
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    assert_eq!(files.len(), 1, "Should produce one store file");
    assert!(
        files[0].path.to_string_lossy().contains("candidate.ts"),
        "Should have candidate.ts store"
    );
    assert!(!files[0].content.is_empty(), "Store should not be empty");
}

// === UiScaffoldGenerator Tests ===

#[tokio::test]
async fn ui_scaffold_generator_produces_scaffold_files() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-scaffold");

    let generator = generate::ui::scaffold::UiScaffoldGenerator::new(&output_dir, false, false);
    let files = generator
        .generate(
            &mock,
            &config,
            &test_generation_order(),
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    assert!(
        files.len() >= 10,
        "Scaffold should produce many files, got {}",
        files.len()
    );

    // Check package.json exists
    let pkg = files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("package.json"))
        .expect("Should have package.json");
    assert!(
        pkg.content.contains("codegraph-app"),
        "package.json should contain app name"
    );

    // ConfirmDialog was replaced by shadcn AlertDialog (from @crewbase/ui)
}

#[tokio::test]
async fn ui_scaffold_nav_respects_configured_path_segment() {
    // #162 phase 4 route convention: the dashboard nav must link to the same
    // route the page generator emits — including configured path_segment
    // overrides (e.g. plural "candidates-custom").
    let mock = setup_mock();
    let toml = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.recruiting]
label = "Recruiting"
schema_dir = "recruiting"
postgres_schema = "recruiting"
entities = ["CandidateType"]

[domains.recruiting.entity_config.CandidateType]
role = "root"
path_segment = "candidates-custom"
"#;
    let config = codegraph_config::config::parse_domain_config_str(toml).unwrap();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-scaffold-nav");

    let generator = generate::ui::scaffold::UiScaffoldGenerator::new(&output_dir, false, false);
    let files = generator
        .generate(
            &mock,
            &config,
            &test_generation_order(),
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    let dashboard = files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("dashboard"))
        .expect("Should have a dashboard page");
    assert!(
        dashboard.content.contains("/recruiting/candidates-custom"),
        "dashboard nav must use the configured path_segment override"
    );
    assert!(
        !dashboard.content.contains("/recruiting/candidate\'")
            && !dashboard.content.contains("path: '/recruiting/candidate'"),
        "dashboard nav must not emit the singular auto-derived segment"
    );
}

// === Org Signup Scaffold Tests ===

#[tokio::test]
async fn ui_scaffold_generates_supabase_client() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-scaffold-supabase");

    let generator = generate::ui::scaffold::UiScaffoldGenerator::new(&output_dir, false, false);
    let files = generator
        .generate(
            &mock,
            &config,
            &test_generation_order(),
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    let supabase = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("supabase.ts"))
        .expect("Should have supabase.ts");
    assert!(
        supabase.content.contains("createBrowserClient"),
        "Supabase client should use createBrowserClient"
    );
    assert!(
        supabase.content.contains("PUBLIC_SUPABASE_URL"),
        "Supabase client should reference PUBLIC_SUPABASE_URL"
    );
}

#[tokio::test]
async fn ui_scaffold_generates_auth_callback() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-scaffold-auth-cb");

    let generator = generate::ui::scaffold::UiScaffoldGenerator::new(&output_dir, false, false);
    let files = generator
        .generate(
            &mock,
            &config,
            &test_generation_order(),
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    let callback = files
        .iter()
        .find(|f| {
            f.path.to_string_lossy().contains("callback")
                && f.path.to_string_lossy().ends_with("+server.ts")
        })
        .expect("Should have auth callback +server.ts");
    assert!(
        callback.content.contains("exchangeCodeForSession"),
        "Auth callback should exchange code for session"
    );
}

#[tokio::test]
async fn ui_scaffold_generates_login_and_signup_pages() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-scaffold-auth-pages");

    let generator = generate::ui::scaffold::UiScaffoldGenerator::new(&output_dir, false, false);
    let files = generator
        .generate(
            &mock,
            &config,
            &test_generation_order(),
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    let login = files
        .iter()
        .find(|f| {
            f.path.to_string_lossy().contains("login")
                && f.path.to_string_lossy().ends_with("+page.svelte")
        })
        .expect("Should have login +page.svelte");
    assert!(
        login.content.contains("LoginPage"),
        "Login page should import LoginPage"
    );
    assert!(
        login.content.contains("@crewbase/ui"),
        "Login page should import from @crewbase/ui"
    );

    let signup = files
        .iter()
        .find(|f| {
            f.path.to_string_lossy().contains("signup")
                && f.path.to_string_lossy().ends_with("+page.svelte")
        })
        .expect("Should have signup +page.svelte");
    assert!(
        signup.content.contains("SignupPage"),
        "Signup page should import SignupPage"
    );
    assert!(
        signup.content.contains("@crewbase/ui"),
        "Signup page should import from @crewbase/ui"
    );
}

#[tokio::test]
async fn ui_scaffold_generates_dashboard_page() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-scaffold-dash");

    let generator = generate::ui::scaffold::UiScaffoldGenerator::new(&output_dir, false, false);
    let files = generator
        .generate(
            &mock,
            &config,
            &test_generation_order(),
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    let dashboard = files
        .iter()
        .find(|f| {
            let p = f.path.to_string_lossy();
            p.contains("/dashboard/") && p.ends_with("+page.svelte")
        })
        .expect("Should have dashboard +page.svelte");
    assert!(
        dashboard.content.contains("Dashboard"),
        "Dashboard page should import Dashboard"
    );
    assert!(
        dashboard.content.contains("@crewbase/ui"),
        "Dashboard page should import from @crewbase/ui"
    );
    assert!(
        dashboard.content.contains("orgName"),
        "Dashboard page should pass orgName prop"
    );
}

#[tokio::test]
async fn ui_scaffold_generates_settings_pages() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-scaffold-settings");

    let generator = generate::ui::scaffold::UiScaffoldGenerator::new(&output_dir, false, false);
    let files = generator
        .generate(
            &mock,
            &config,
            &test_generation_order(),
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    let team = files
        .iter()
        .find(|f| {
            f.path.to_string_lossy().contains("team")
                && f.path.to_string_lossy().ends_with("+page.svelte")
        })
        .expect("Should have team settings +page.svelte");
    assert!(
        team.content.contains("TeamSettings"),
        "Team settings should import TeamSettings"
    );

    let api_keys = files
        .iter()
        .find(|f| {
            f.path.to_string_lossy().contains("api-keys")
                && f.path.to_string_lossy().ends_with("+page.svelte")
        })
        .expect("Should have api-keys settings +page.svelte");
    assert!(
        api_keys.content.contains("ApiKeySettings"),
        "API keys settings should import ApiKeySettings"
    );
}

#[tokio::test]
async fn ui_scaffold_api_client_uses_supabase_jwt() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-scaffold-api-jwt");

    let generator = generate::ui::scaffold::UiScaffoldGenerator::new(&output_dir, false, false);
    let files = generator
        .generate(
            &mock,
            &config,
            &test_generation_order(),
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    let client = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("client.ts"))
        .expect("Should have api client.ts");
    assert!(
        client.content.contains("authHeaders"),
        "API client should have authHeaders function"
    );
    assert!(
        client.content.contains("supabase.auth.getSession"),
        "API client should use Supabase session"
    );
    assert!(
        client.content.contains("PUBLIC_API_KEY"),
        "API client should still have API key fallback"
    );
}

#[tokio::test]
async fn ui_scaffold_package_json_has_supabase_deps() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-scaffold-pkg-deps");

    let generator = generate::ui::scaffold::UiScaffoldGenerator::new(&output_dir, false, false);
    let files = generator
        .generate(
            &mock,
            &config,
            &test_generation_order(),
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    let pkg = files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("package.json"))
        .expect("Should have package.json");
    assert!(
        pkg.content.contains("@crewbase/ui"),
        "package.json should have @crewbase/ui dependency"
    );
    assert!(
        pkg.content.contains("@supabase/supabase-js"),
        "package.json should have @supabase/supabase-js dependency"
    );
    assert!(
        pkg.content.contains("@supabase/ssr"),
        "package.json should have @supabase/ssr dependency"
    );
}

#[tokio::test]
async fn ui_scaffold_app_layout_has_settings_nav() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-scaffold-layout-nav");

    let generator = generate::ui::scaffold::UiScaffoldGenerator::new(&output_dir, false, false);
    let files = generator
        .generate(
            &mock,
            &config,
            &test_generation_order(),
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    let layout = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("+layout.svelte"))
        .expect("Should have +layout.svelte");
    assert!(
        layout.content.contains("/settings/team"),
        "App layout should have team settings link"
    );
    assert!(
        layout.content.contains("/settings/api-keys"),
        "App layout should have API keys settings link"
    );
    assert!(
        layout.content.contains("createSupabaseClient"),
        "App layout should import createSupabaseClient"
    );
}

// === UiTypeGenerator Tests ===

#[tokio::test]
async fn ui_type_generator_produces_types_file() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-types");

    let generator = generate::ui::types::UiTypeGenerator::new(&output_dir);
    let files = generator
        .generate(
            &mock,
            &config,
            &test_generation_order(),
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    assert_eq!(
        files.len(),
        2,
        "Should produce types.ts + format.ts (issue #468 W2)"
    );
    assert!(
        files[0].path.to_string_lossy().contains("types.ts"),
        "Should have types.ts"
    );
    assert!(
        !files[0].content.is_empty(),
        "Types file should not be empty"
    );
    assert!(
        files[1].path.to_string_lossy().ends_with("lib/format.ts"),
        "Should have lib/format.ts"
    );
    assert!(
        files[1].content.contains("export function formatRange"),
        "format.ts carries the shared formatters"
    );
}

// === UiDomainLayoutGenerator Tests ===

#[tokio::test]
async fn ui_domain_layout_generator_produces_layout() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-domain-layout");

    let generator = generate::ui::domain_layout::UiDomainLayoutGenerator::new(&output_dir);
    let entities = vec!["CandidateType".to_string()];
    let files = generator
        .generate(
            &mock,
            "recruiting",
            &entities,
            &config,
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    assert_eq!(files.len(), 1, "Should produce one layout file");
    assert!(
        files[0].path.to_string_lossy().contains("+layout.svelte"),
        "Should have +layout.svelte"
    );
    assert!(!files[0].content.is_empty(), "Layout should not be empty");
}

// === Unit tests for form helper functions ===

#[test]
fn rust_type_to_ts_maps_string() {
    assert_eq!(form::rust_type_to_ts("String", false), "string");
}

#[test]
fn rust_type_to_ts_maps_bool() {
    assert_eq!(form::rust_type_to_ts("bool", false), "boolean");
}

#[test]
fn rust_type_to_ts_maps_numeric_types() {
    assert_eq!(form::rust_type_to_ts("i32", false), "number");
    assert_eq!(form::rust_type_to_ts("f64", false), "number");
    assert_eq!(form::rust_type_to_ts("u64", false), "number");
}

#[test]
fn rust_type_to_ts_maps_entity_ref_to_string() {
    // Entity references are UUID strings in TypeScript
    assert_eq!(form::rust_type_to_ts("Uuid", true), "string");
}

#[test]
fn rust_type_to_ts_maps_datetime() {
    assert_eq!(form::rust_type_to_ts("DateTime<Utc>", false), "string");
}

#[test]
fn rust_type_to_ts_maps_decimal() {
    assert_eq!(form::rust_type_to_ts("Decimal", false), "string");
}

#[test]
fn rust_type_to_ts_maps_vec() {
    assert_eq!(form::rust_type_to_ts("Vec<String>", false), "Array<string>");
}

#[test]
fn classify_input_type_codelist_returns_select() {
    let prop = make_test_property("status", "String");
    assert_eq!(form::classify_input_type(&prop, false, true), "select");
}

#[test]
fn classify_input_type_entity_ref_returns_text() {
    let prop = make_test_property("person_id", "Uuid");
    assert_eq!(form::classify_input_type(&prop, true, false), "text");
}

#[test]
fn classify_input_type_bool_returns_checkbox() {
    let prop = make_test_property("is_active", "bool");
    assert_eq!(form::classify_input_type(&prop, false, false), "checkbox");
}

#[test]
fn classify_input_type_date_returns_date() {
    let prop = make_test_property("start_date", "NaiveDate");
    assert_eq!(form::classify_input_type(&prop, false, false), "date");
}

#[test]
fn classify_input_type_number_returns_number() {
    let prop = make_test_property("count", "i32");
    assert_eq!(form::classify_input_type(&prop, false, false), "number");
}

#[test]
fn field_name_to_label_converts_snake_case() {
    assert_eq!(form::field_name_to_label("given_name"), "Given Name");
    assert_eq!(form::field_name_to_label("family_name"), "Family Name");
    assert_eq!(form::field_name_to_label("id"), "Id");
}

#[test]
fn field_name_to_label_handles_single_word() {
    assert_eq!(form::field_name_to_label("status"), "Status");
}

// === Conditional generation test ===

#[tokio::test]
async fn ui_form_generator_skips_when_no_create_or_update() {
    // Create a mock entity with no create/update operations.
    // ApplicationType in the fixture has default operations, so let's use a
    // schema that isn't in any domain config (will use defaults which include create/update).
    // Instead, we test with an entity-specific config that excludes create+update.
    //
    // The simplest approach: create a minimal DomainConfig that has an entity with
    // operations = ["read", "list"] only.
    let schema = SchemaNode {
        namespace: None,
        schema_id: "common/json/ReadOnlyType.json".to_string(),
        title: "ReadOnlyType".to_string(),
        description: None,
        schema_type: "object".to_string(),
        classification: "entity_reference".to_string(),
        domain: Some("common".to_string()),
        rel_path: "common/json/ReadOnlyType.json".to_string(),
        pg_type: "UUID".to_string(),
        rust_type: "Uuid".to_string(),
        sea_orm_type: "Uuid".to_string(),
        rust_type_name: "ReadOnly".to_string(),
        pg_table_name: "read_only".to_string(),
        api_path_segment: "read-only".to_string(),
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
    };

    let mock = MockEngine::builder()
        .with_schema(schema)
        .with_properties("ReadOnlyType", candidate_properties())
        .build();

    // Use a config where the entity has only read + list
    let toml_str = r#"
[defaults]
operations = ["read", "list"]

[domains.common]
label = "Common"
schema_dir = "common"
postgres_schema = "common"
entities = ["ReadOnlyType"]
"#;
    let config: codegraph_config::DomainConfig = toml::from_str(toml_str).unwrap();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-form-skip");

    let generator = generate::ui::form::UiFormGenerator::new(&output_dir);
    let files = generator
        .generate(
            &mock,
            "ReadOnlyType",
            "common",
            &config,
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    assert!(
        files.is_empty(),
        "Form generator should produce no files when entity has no create/update operations"
    );
}

// === Version Information Tests ===

#[tokio::test]
async fn ui_scaffold_vite_config_has_version_defines() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-vite-version");

    let generator = generate::ui::scaffold::UiScaffoldGenerator::new(&output_dir, false, false);
    let files = generator
        .generate(
            &mock,
            &config,
            &test_generation_order(),
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    let vite_config = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("vite.config.ts"))
        .expect("Should have vite.config.ts");

    assert!(
        vite_config.content.contains("__APP_VERSION__"),
        "vite.config.ts should define __APP_VERSION__"
    );
    assert!(
        vite_config.content.contains("__BUILD_TIME__"),
        "vite.config.ts should define __BUILD_TIME__"
    );
    assert!(
        vite_config.content.contains("define:"),
        "vite.config.ts should have a define block"
    );
}

#[tokio::test]
async fn ui_scaffold_generates_version_server_route() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-version-server");

    let generator = generate::ui::scaffold::UiScaffoldGenerator::new(&output_dir, false, false);
    let files = generator
        .generate(
            &mock,
            &config,
            &test_generation_order(),
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    let server_route = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("version/+server.ts"))
        .expect("Should generate version +server.ts");

    // Version route must be outside the (app) auth group
    assert!(
        !server_route.path.to_string_lossy().contains("(app)"),
        "version server route must not be behind (app) auth group"
    );

    // Exports a GET handler
    assert!(
        server_route.content.contains("export const GET"),
        "version server route should export GET handler"
    );

    // Returns aggregated UI + API version info
    assert!(
        server_route.content.contains("__APP_VERSION__"),
        "version server route should use __APP_VERSION__"
    );
    assert!(
        server_route.content.contains("__BUILD_TIME__"),
        "version server route should use __BUILD_TIME__"
    );

    // Fetches backend /version endpoint
    assert!(
        server_route.content.contains("/version"),
        "version server route should fetch backend /version"
    );

    // Returns JSON with ui and api sections
    assert!(
        server_route.content.contains("json("),
        "version server route should return json response"
    );

    // Handles API errors gracefully
    assert!(
        server_route.content.contains("error"),
        "version server route should handle API errors"
    );
}

#[tokio::test]
async fn ui_scaffold_generates_version_page() {
    let mock = setup_mock();
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-ui-version-page");

    let generator = generate::ui::scaffold::UiScaffoldGenerator::new(&output_dir, false, false);
    let files = generator
        .generate(
            &mock,
            &config,
            &test_generation_order(),
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    let version_page = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("version/+page.svelte"))
        .expect("Should generate version +page.svelte");

    // Version page must be outside the (app) auth group
    assert!(
        !version_page.path.to_string_lossy().contains("(app)"),
        "version page must not be behind (app) auth group"
    );

    // Fetches version data from server route
    assert!(
        version_page.content.contains("fetch('/version')"),
        "version page should fetch from /version server route"
    );

    // Displays UI version section
    assert!(
        version_page.content.contains("data.ui.version"),
        "version page should display UI version"
    );

    // Displays API version section
    assert!(
        version_page.content.contains("data.api"),
        "version page should display API version section"
    );

    // Shows git commit
    assert!(
        version_page.content.contains("git_commit"),
        "version page should show git commit"
    );

    // Handles API unavailable state
    assert!(
        version_page.content.contains("data.error"),
        "version page should handle error state"
    );
}

// === Helper ===

fn make_test_property(name: &str, rust_type: &str) -> PropertyNode {
    PropertyNode {
        is_id: false,
        name: name.to_string(),
        prop_type: "string".to_string(),
        description: None,
        format: None,
        is_required: false,
        is_nullable: true,
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
        sea_orm_type: "Text".to_string(),
        render_strategy: "direct_column".to_string(),
        ref_target: None,
        classification: Some("primitive_wrapper".to_string()),
        projection: None,
        classification_kind: None,
        ui_override_detail: None,
        ui_override_list_cell: None,
        ui_override_form: None,
        ui_override_inline: None,
        type_expr: None,
    }
}
