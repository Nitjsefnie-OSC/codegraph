use crate::harness::{
    candidate_schema, gender_codelist_schema, setup_mock, test_domain_config, test_project_config,
    test_tera,
};
use codegraph::generate;
use codegraph::generate::db::codelist::CodelistGenerator;
use codegraph::generate::db::dialect::{DatabaseTarget, dialect_for_target};
#[allow(unused_imports)]
use codegraph::generate::traits::{DomainGenerator, EntityGenerator, GlobalGenerator};
use codegraph_core::mock::MockEngine;
use codegraph_core::types::{EnumValue, PropertyNode};

// === DTO Template Tests ===

#[tokio::test]
async fn candidate_dto_create() {
    let mock = setup_mock().await;
    let config = test_domain_config();
    let tera = test_tera();

    // App DTOs are now re-exports; verify the re-export references the correct type
    let app_output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-harness-dto");
    let app_gen = generate::ddd::dto::DtoGenerator::new(&app_output_dir);
    let app_files = app_gen
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
    let app_create = app_files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("dto_create"))
        .expect("Should have a create DTO file");
    assert!(
        app_create
            .content
            .contains("pub use domain_types::recruiting::candidate::CreateCandidateRequest"),
        "App DTO should re-export from domain_types. Got:\n{}",
        app_create.content
    );

    // Verify struct content in domain_types output
    let tmp = std::env::temp_dir().join("hr-graph-test-harness-dto-domain");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let dt_gen = generate::domain_types::dto::DomainTypesDtoGenerator::new_with_base(tmp.clone());
    let dt_files = dt_gen
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
    let dt_create = dt_files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("dto_create"))
        .expect("Should have a domain_types create DTO file");
    assert!(
        dt_create.content.contains("CreateCandidateRequest"),
        "Domain types DTO should contain CreateCandidateRequest struct"
    );
    assert!(
        dt_create.content.contains("garde::Validate"),
        "Domain types DTO should derive garde::Validate. Got:\n{}",
        dt_create.content
    );
    assert!(
        dt_create.content.contains("given_name"),
        "Should contain given_name field"
    );
}

#[tokio::test]
async fn dto_create_template_omits_garde_when_disabled() {
    let tera = test_tera();
    let project = codegraph::generate::ProjectConfig::default();

    let ctx = codegraph::generate::ddd::dto::DtoContext {
        module_name: "test_entity".to_string(),
        entity_name: "TestEntity".to_string(),
        domain: "test".to_string(),
        fields: vec![
            codegraph::generate::ddd::dto::DtoField {
                name: "name".to_string(),
                rust_type: "String".to_string(),
                is_required: true,
                is_array: false,
                description: String::new(),
                render_strategy: "direct_column".to_string(),
                is_entity_ref: false,
                is_hierarchy_field: false,
                min_length: Some(2),
                max_length: Some(100),
                minimum: None,
                maximum: None,
                pattern: None,
                format: None,
            },
            codegraph::generate::ddd::dto::DtoField {
                name: "email".to_string(),
                rust_type: "String".to_string(),
                is_required: false,
                is_array: false,
                description: String::new(),
                render_strategy: "direct_column".to_string(),
                is_entity_ref: false,
                is_hierarchy_field: false,
                min_length: None,
                max_length: None,
                minimum: None,
                maximum: None,
                pattern: None,
                format: Some("email".to_string()),
            },
            codegraph::generate::ddd::dto::DtoField {
                name: "parent".to_string(),
                rust_type: "Uuid".to_string(),
                is_required: false,
                is_array: false,
                description: String::new(),
                render_strategy: "entity_ref".to_string(),
                is_entity_ref: true,
                is_hierarchy_field: false,
                min_length: None,
                max_length: None,
                minimum: None,
                maximum: None,
                pattern: None,
                format: None,
            },
        ],
        immutable_fields: vec![],
        workflow_excluded_fields: vec![],
        list_exclude: vec![],
        list_include: vec![],
        has_list_fields: false,
        operations: vec!["create".to_string(), "update".to_string()],
        child_dtos: vec![],
        all_child_dtos: vec![],
        codelist_imports: vec![],
        codelist_imports_update: vec![],
        has_workflow: false,
        has_approval_status: false,
        structured_imports: vec![],
        has_validate: false,
        derived_fields: vec![],
        mox_derived_extras: vec![],
    };

    let create = codegraph::generate::render_template_with_project(
        &tera,
        "domain_types/dto_create.tera",
        &ctx,
        &project,
    )
    .unwrap();
    assert!(
        !create.contains("garde::Validate"),
        "Should NOT derive garde::Validate when disabled"
    );
    assert!(
        !create.contains("#[garde("),
        "Should NOT have garde attributes when disabled"
    );

    let update = codegraph::generate::render_template_with_project(
        &tera,
        "domain_types/dto_update.tera",
        &ctx,
        &project,
    )
    .unwrap();
    assert!(
        !update.contains("garde::Validate"),
        "Update DTO should NOT derive garde::Validate when disabled"
    );
    assert!(
        !update.contains("#[garde("),
        "Update DTO should NOT have garde attributes when disabled"
    );
}

#[tokio::test]
async fn candidate_dto_response() {
    let mock = setup_mock().await;
    let config = test_domain_config();
    let tera = test_tera();

    // App DTOs are re-exports; verify the re-export
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-harness-dto-resp");
    let generator = generate::ddd::dto::DtoGenerator::new(&output_dir);
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
    let response_file = files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("dto_response"))
        .expect("Should have a response DTO file");
    assert!(
        response_file.content.contains("CandidateResponse"),
        "Should contain CandidateResponse re-export"
    );

    // Verify struct content in domain_types output
    let tmp = std::env::temp_dir().join("hr-graph-test-harness-dto-resp-domain");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let dt_gen = generate::domain_types::dto::DomainTypesDtoGenerator::new_with_base(tmp.clone());
    let dt_files = dt_gen
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
    let dt_response = dt_files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("dto_response"))
        .expect("Should have a domain_types response DTO file");
    assert!(
        dt_response.content.contains("pub struct CandidateResponse"),
        "Domain types should contain CandidateResponse struct"
    );
}

// === Test Generator Template Tests ===

#[tokio::test]
async fn candidate_test_gen() {
    let mock = setup_mock().await;
    let config = test_domain_config();
    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-harness-testgen");

    let generator = generate::test::test_gen::TestGenerator::new(&output_dir);
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

    assert_eq!(files.len(), 2, "Should have entity_test and dto_test");
}

// === Entity Reference DTO Tests ===

#[tokio::test]
async fn candidate_create_dto_renders_entity_ref_as_id_field() {
    let engine = MockEngine::builder()
        .with_schema(candidate_schema())
        .with_properties(
            "CandidateType",
            vec![
                PropertyNode {
                    is_id: false,
                    name: "givenName".into(),
                    prop_type: "string".into(),
                    description: None,
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
                    pg_column_name: "given_name".into(),
                    pg_column_type: "TEXT".into(),
                    rust_field_name: "given_name".into(),
                    rust_field_type: "String".into(),
                    sea_orm_type: "Text".into(),
                    render_strategy: "primitive_wrapper".into(),
                    ref_target: None,
                    classification: None,
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
                    name: "referredByApplication".into(),
                    prop_type: "object".into(),
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
                    pg_column_name: "referred_by_application".into(),
                    pg_column_type: "UUID".into(),
                    rust_field_name: "referred_by_application".into(),
                    rust_field_type: "Uuid".into(),
                    sea_orm_type: "Uuid".into(),
                    render_strategy: "entity_reference".into(),
                    ref_target: Some("ApplicationType".into()),
                    classification: None,
                    projection: None,
                    classification_kind: None,
                    ui_override_detail: None,
                    ui_override_list_cell: None,
                    ui_override_form: None,
                    ui_override_inline: None,
                    type_expr: None,
                },
            ],
        )
        .build();

    let config = test_domain_config();
    let tera = test_tera();

    // App DTOs are re-exports; check struct content in domain_types output
    let tmp = std::env::temp_dir().join("hr-graph-test-entity-ref-dto");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let generator =
        generate::domain_types::dto::DomainTypesDtoGenerator::new_with_base(tmp.clone());
    let files = generator
        .generate(
            &engine,
            "CandidateType",
            "recruiting",
            &config,
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    let create_file = files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("dto_create"))
        .expect("should produce dto_create file");

    // Entity reference should render as _id field, not full object
    assert!(
        create_file.content.contains("referred_by_application_id"),
        "entity ref should render as _id field. Got:\n{}",
        create_file.content,
    );
    assert!(
        create_file.content.contains("Option<uuid::Uuid>"),
        "entity ref should be Option<uuid::Uuid>. Got:\n{}",
        create_file.content,
    );
    // Should NOT contain the raw field name without _id suffix
    assert!(
        !create_file.content.contains("pub referred_by_application:"),
        "should not contain full entity object field. Got:\n{}",
        create_file.content,
    );
}

// === Codelist Rust Enum Template Tests ===

#[tokio::test]
async fn codelist_enum_template_renders_correctly() {
    let tera = test_tera();
    let ctx = codegraph::generate::codelist::rust_enum::RustEnumContext {
        enum_name: "GenderCodeList".to_string(),
        description: "Gender code list.".to_string(),
        variants: vec![
            codegraph::generate::codelist::rust_enum::RustEnumVariant {
                name: "Male".to_string(),
                code: "Male".to_string(),
                serde_rename: None,
            },
            codegraph::generate::codelist::rust_enum::RustEnumVariant {
                name: "Female".to_string(),
                code: "Female".to_string(),
                serde_rename: None,
            },
            codegraph::generate::codelist::rust_enum::RustEnumVariant {
                name: "X".to_string(),
                code: "X".to_string(),
                serde_rename: None,
            },
        ],
    };

    let content = generate::render_template_with_project(
        &tera,
        "codelist/enum.tera",
        &ctx,
        &test_project_config(),
    )
    .unwrap();

    assert!(
        content.contains("pub enum GenderCodeList"),
        "Should contain enum declaration. Got:\n{}",
        content
    );
    assert!(
        content.contains("Serialize, Deserialize, ToSchema"),
        "Should have serde + utoipa derives"
    );
    assert!(
        content.contains("impl std::fmt::Display for GenderCodeList"),
        "Should implement Display"
    );
    assert!(
        content.contains("Self::Male => write!(f, \"Male\")"),
        "Display should write original code"
    );
    // No serde rename needed for PascalCase values
    assert!(
        !content.contains("#[serde(rename"),
        "PascalCase values should NOT have serde rename"
    );
    // Default derive (first variant marked with #[default]) — issue #9
    assert!(
        content.contains("Default,"),
        "Codelist enum should derive Default (issue #9)"
    );
    assert!(
        content.contains("#[default]"),
        "First variant should be #[default]"
    );
    assert!(
        content.contains("Male"),
        "Default should be first variant (Male)"
    );
    // FromStr impl
    assert!(
        content.contains("impl std::str::FromStr for GenderCodeList"),
        "Should implement FromStr"
    );
}

#[tokio::test]
async fn codelist_enum_template_renders_serde_rename() {
    let tera = test_tera();
    let ctx = codegraph::generate::codelist::rust_enum::RustEnumContext {
        enum_name: "CurrencyCodeList".to_string(),
        description: "ISO 4217 currency codes.".to_string(),
        variants: vec![
            codegraph::generate::codelist::rust_enum::RustEnumVariant {
                name: "Usd".to_string(),
                code: "USD".to_string(),
                serde_rename: Some("USD".to_string()),
            },
            codegraph::generate::codelist::rust_enum::RustEnumVariant {
                name: "Eur".to_string(),
                code: "EUR".to_string(),
                serde_rename: Some("EUR".to_string()),
            },
        ],
    };

    let content = generate::render_template_with_project(
        &tera,
        "codelist/enum.tera",
        &ctx,
        &test_project_config(),
    )
    .unwrap();

    assert!(
        content.contains("#[serde(rename = \"USD\")]"),
        "Should have serde rename for USD"
    );
    assert!(content.contains("    Usd,"), "Should have Usd variant");
    assert!(
        content.contains("Self::Usd => write!(f, \"USD\")"),
        "Display should write USD for Usd variant"
    );
}

// === Sanitize Variant Name Tests ===

#[test]
fn sanitize_variant_name_rules() {
    use codegraph::generate::codelist::rust_enum::sanitize_variant_name;

    // PascalCase pass-through
    assert_eq!(sanitize_variant_name("Male"), "Male");
    assert_eq!(sanitize_variant_name("FullTime"), "FullTime");

    // ALL-CAPS → PascalCase
    assert_eq!(sanitize_variant_name("USD"), "Usd");
    assert_eq!(sanitize_variant_name("EUR"), "Eur");

    // Leading digit → prefix with _
    assert_eq!(sanitize_variant_name("3rdParty"), "_3rdParty");

    // Rust keyword → prefix with R (only when PascalCase result is still a keyword)
    assert_eq!(sanitize_variant_name("type"), "Type"); // "type" → PascalCase "Type" (not a keyword)
    assert_eq!(sanitize_variant_name("Self"), "RSelf"); // "Self" stays "Self" (keyword)

    // Special characters
    assert_eq!(sanitize_variant_name("full-time"), "FullTime");
    assert_eq!(sanitize_variant_name("a/b"), "AB");
    assert_eq!(sanitize_variant_name("a.b"), "AB");
}

/// Verify that CodelistGenerator with SQLite dialect produces INSERT OR IGNORE
/// instead of the PostgreSQL ON CONFLICT DO NOTHING pattern.
#[tokio::test]
async fn codelist_with_sqlite_dialect_uses_insert_or_ignore() {
    let engine = MockEngine::builder()
        .with_schema(gender_codelist_schema())
        .with_enum_values(
            "GenderCodeList",
            vec![
                EnumValue {
                    value: "Male".to_string(),
                    display_name: Some("Male".to_string()),
                    sort_order: 0,
                },
                EnumValue {
                    value: "Female".to_string(),
                    display_name: Some("Female".to_string()),
                    sort_order: 1,
                },
                EnumValue {
                    value: "X".to_string(),
                    display_name: Some("Non-binary".to_string()),
                    sort_order: 2,
                },
            ],
        )
        .build();

    let tera = test_tera();
    let output_dir = std::path::PathBuf::from("/tmp/hr-graph-test-harness-dto-include-stripped");

    let config = codegraph_config::config::parse_domain_config_str(
        r#"
[defaults]
operations = ["create", "read", "update", "list"]

[domains.hr]
label = "HR"
schema_dir = "hr"
postgres_schema = "hr"
entities = ["WorkerType", "DeploymentType", "PositionType"]

[domains.hr.entity_config.WorkerType]
operations = ["create", "read", "update", "list"]
allow_include = ["deployment.position"]
"#,
    )
    .unwrap();

    let generator = CodelistGenerator::new(&output_dir)
        .with_dialect(dialect_for_target(DatabaseTarget::Sqlite));
    let files = generator
        .generate(
            &engine,
            "GenderCodeList",
            "common",
            &config,
            &tera,
            &test_project_config(),
        )
        .await
        .unwrap();

    assert!(
        !files.is_empty(),
        "Codelist generator should produce a file"
    );
    let content = &files[0].content;

    // SQLite codelist template uses INSERT OR IGNORE
    assert!(
        content.contains("INSERT OR IGNORE"),
        "SQLite codelist should use INSERT OR IGNORE. Got:\n{content}"
    );

    // Should contain codelist values
    assert!(
        content.contains("'Male'"),
        "Should contain Male codelist value. Got:\n{content}"
    );
    assert!(
        content.contains("'Female'"),
        "Should contain Female codelist value. Got:\n{content}"
    );

    // Should use STRICT table mode
    assert!(
        content.contains("STRICT"),
        "SQLite codelist table should use STRICT mode. Got:\n{content}"
    );
}
