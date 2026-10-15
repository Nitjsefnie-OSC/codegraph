use std::path::Path;

use codegraph::generate::template_engine;
use codegraph::profile::BuildPlan;
use codegraph_core::types::{PropertyNode, SchemaNode};

/// Path to the project root's profiles.toml
pub(crate) fn profiles_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .join("profiles.toml")
}

/// Set up a mock engine with a minimal schema, same pattern as template_harness.
pub(crate) fn mock_test_setup() -> (
    codegraph_core::mock::MockEngine,
    codegraph_config::DomainConfig,
    tera::Tera,
    tempfile::TempDir,
) {
    let schema = SchemaNode {
        namespace: None,
        schema_id: "recruiting/json/CandidateType.json".to_string(),
        title: "CandidateType".to_string(),
        description: Some("A candidate for a position".to_string()),
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
    };

    let props = vec![PropertyNode {
        is_id: false,
        name: "givenName".to_string(),
        prop_type: "string".to_string(),
        description: Some("First name".to_string()),
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
    }];

    let engine = codegraph_core::mock::MockEngine::builder()
        .with_schema(schema)
        .with_properties("CandidateType", props)
        .build();

    let config = codegraph_config::config::parse_domain_config_str(
        r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.recruiting]
label = "Recruiting"
schema_dir = "recruiting"
postgres_schema = "recruiting"
entities = ["CandidateType"]

[domains.recruiting.entity_config.CandidateType]
operations = ["create", "read", "update", "delete", "list"]
"#,
    )
    .unwrap();

    let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    let tera = template_engine::create_tera(&template_dir).unwrap();

    let output_dir = tempfile::TempDir::new().unwrap();

    (engine, config, tera, output_dir)
}

/// Run the routing-sensitive generator subset with a directly-constructed
/// build plan for the given topology, using the provided fixture pieces.
pub(crate) async fn run_routing_generators_with_parts(
    mock: codegraph_core::mock::MockEngine,
    config: codegraph_config::DomainConfig,
    tera: tera::Tera,
    output_dir: tempfile::TempDir,
    topology: codegraph::profile::DeploymentTopology,
    project: codegraph::generate::ProjectConfig,
) -> (
    codegraph::generate::report::GenerationReport,
    tempfile::TempDir,
) {
    let domain_types_tmp = tempfile::TempDir::new().unwrap();
    let hooks_tmp = tempfile::TempDir::new().unwrap();

    let plan = BuildPlan {
        entity_generators: vec![
            "ddl".to_string(),
            "sea_orm_entity".to_string(),
            "handler".to_string(),
            "dto".to_string(),
            "repository".to_string(),
            "command".to_string(),
            "query".to_string(),
            "event".to_string(),
        ],
        domain_generators: vec![
            "errors".to_string(),
            "router".to_string(),
            "links".to_string(),
        ],
        global_generators: vec!["worker_scaffold".to_string(), "scaffold".to_string()],
        post_gen_scripts: vec![],
        ifml_frameworks: vec![],
        template_pack_path: None,
        database_target: codegraph::generate::db::dialect::DatabaseTarget::Postgres,
        has_atproto: false,
        atproto_tenancy: "shared_pds".to_string(),
        has_fern: false,
        has_emdash: false,
        fern_sdk_languages: vec!["typescript".to_string()],
        persistence_provider: codegraph::profile::PersistenceProvider::SeaOrm,
        dto_key_casing: "snake".to_string(),
        deployment_topology: topology,
        dependency_strategy: codegraph::profile::DependencyStrategy::Rev,
        namespace_layout: false,
        ux_rules: false,
        expr_ir: false,
        ifml_e2e_auth: false,
        testid_module: false,
        structural_gates: false,
        public_operations_rls: false,
        features: toml::Table::new(),
    };

    let report =
        codegraph::generate::run_generators_with_opts(codegraph::generate::GeneratorOpts {
            db: &mock,
            config: &config,
            output_dir: output_dir.path(),
            tera: &tera,
            ui_overrides: &Default::default(),
            ui_domains: &Default::default(),
            schema_base_dir: Path::new(""),
            domain_types_base: Some(domain_types_tmp.path()),
            hooks_base: Some(hooks_tmp.path()),
            ext_points: None,
            seed_config: None,
            build_plan: Some(&plan),
            ifml_frameworks: vec![],
            ifml_components: None,
            ux_rules: None,
            project_config: Some(&project),
            emdash_plugins: None,
            domain_config_dir: None,
        })
        .await
        .expect("routing generator run should succeed");

    assert!(
        !report.has_errors(),
        "routing run should have no errors: {}",
        report.summary()
    );
    (report, output_dir)
}
