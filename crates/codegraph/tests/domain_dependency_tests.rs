//! Cross-project domain dependencies (issue #276): a consumer project pins a
//! publisher's domain face (a #275 artifact document) via
//! `[[domains.X.dependencies]]`. The foreign schemas ingest read-only with
//! `dependency:` provenance, local titles shadow dependency titles, doctor
//! reports resolution/version/conflict outcomes, and the driver generates
//! against the pinned face end to end.

use std::fs;
use std::path::Path;

use codegraph::artifact;
use codegraph::doctor_dependencies::{DependencyCheckStatus, check_domain_dependencies};
use codegraph::ingest::dependencies::load_dependency_artifacts;
use codegraph_config::config::parse_domain_config_str;
use codegraph_core::traits::{GraphIngestor, GraphQuerier};
use codegraph_core::types::{
    CodeList, EdgeProperties, EdgeType, EnumValue, PropertyNode, SchemaNode,
};
use codegraph_grafeo::GrafeoEngine;
use serde_json::Value as Json;
use std::path::PathBuf;

// ── fixture helpers ──────────────────────────────────────────────────────

fn schema(title: &str, classification: &str, domain: &str) -> SchemaNode {
    SchemaNode {
        namespace: None,
        schema_id: format!("{domain}/{title}"),
        title: title.to_string(),
        description: Some(format!("The {title} type.")),
        schema_type: "object".to_string(),
        classification: classification.to_string(),
        domain: Some(domain.to_string()),
        rel_path: format!("{domain}/{title}.json"),
        pg_type: "TABLE".to_string(),
        rust_type: title.to_string(),
        sea_orm_type: if classification == "entity" {
            "Entity"
        } else {
            "Model"
        }
        .to_string(),
        rust_type_name: title.to_string(),
        pg_table_name: codegraph_naming::to_snake_case(title),
        api_path_segment: title.to_string(),
        parent_schema: None,
        is_entity: classification == "entity",
        is_codelist: classification == "codelist",
        is_primitive_wrapper: false,
        has_all_of: false,
        has_one_of: false,
        has_any_of: false,
        has_definitions: false,
        access: None,
        annotations: None,
        custom_annotations: Default::default(),
    }
}

fn property(name: &str, required: bool) -> PropertyNode {
    PropertyNode {
        is_id: false,
        name: name.to_string(),
        prop_type: "string".to_string(),
        description: Some(format!("The {name}.")),
        format: None,
        is_required: required,
        is_nullable: !required,
        is_array: false,
        pattern: None,
        min_length: None,
        max_length: None,
        minimum: None,
        maximum: None,
        min_items: None,
        max_items: None,
        pg_column_name: name.to_string(),
        pg_column_type: "TEXT".to_string(),
        rust_field_name: name.to_string(),
        rust_field_type: "String".to_string(),
        sea_orm_type: "Text".to_string(),
        render_strategy: "scalar".to_string(),
        ref_target: None,
        classification: None,
        projection: None,
        classification_kind: None,
        type_expr: None,
        ui_override_detail: None,
        ui_override_list_cell: None,
        ui_override_form: None,
        ui_override_inline: None,
    }
}

/// The publisher's `party` face: an entity with properties, a status
/// codelist with two enum values, and a refers edge to the codelist.
async fn party_face_graph() -> GrafeoEngine {
    let engine = GrafeoEngine::in_memory().unwrap();
    engine
        .ingest_schema(&schema("PartyContact", "entity", "party"))
        .await
        .unwrap();
    engine
        .ingest_property(
            "PartyContact",
            "party/PartyContact",
            &property("email", true),
        )
        .await
        .unwrap();
    engine
        .ingest_property(
            "PartyContact",
            "party/PartyContact",
            &property("phone", false),
        )
        .await
        .unwrap();
    let mut status = property("status", false);
    status.ref_target = Some("PartyKind".to_string());
    engine
        .ingest_property("PartyContact", "party/PartyContact", &status)
        .await
        .unwrap();
    engine
        .ingest_schema(&schema("PartyKind", "codelist", "party"))
        .await
        .unwrap();
    engine
        .ingest_codelist(&CodeList {
            name: "PartyKind".to_string(),
            description: Some("Party kinds.".to_string()),
            pg_table_name: "party_kind".to_string(),
            render_as: "check".to_string(),
            check_expression: None,
        })
        .await
        .unwrap();
    for (value, sort) in [("person", 0), ("organisation", 1)] {
        engine
            .ingest_enum_value(
                "PartyKind",
                &EnumValue {
                    value: value.to_string(),
                    display_name: Some(value.to_string()),
                    sort_order: sort,
                },
            )
            .await
            .unwrap();
    }
    engine
        .ingest_edge(
            "status::PartyContact",
            "party/PartyKind",
            EdgeType::ReferencesSchema,
            Some(&EdgeProperties {
                ref_path: Some("#/PartyKind".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    engine
}

/// Export `engine` as a dependency artifact and inject the publishing meta
/// block (`version`, `domain`) alongside the #275 document body.
fn write_face_artifact(engine: &GrafeoEngine, path: &Path, version: &str, domain: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    artifact::export_ir(engine, path).unwrap();
    let mut doc: Json = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let mut meta = serde_json::Map::new();
    meta.insert("version".into(), Json::String(version.to_string()));
    meta.insert("domain".into(), Json::String(domain.to_string()));
    doc["meta"] = Json::Object(meta);
    fs::write(path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
}

fn consumer_config(source: &str, version: &str) -> codegraph_config::DomainConfig {
    let toml = format!(
        "[domains.common]\nlabel = \"Common\"\nschema_dir = \"common\"\npostgres_schema = \"common\"\n\n[[domains.common.dependencies]]\ndomain = \"party\"\nsource = \"{source}\"\nversion = \"{version}\"\n"
    );
    parse_domain_config_str(&toml).unwrap()
}

// ── read-only resolution ─────────────────────────────────────────────────

#[tokio::test]
async fn foreign_titles_resolve_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let artifact_path = dir.path().join("party/artifact.json");
    write_face_artifact(&party_face_graph().await, &artifact_path, "1.2.0", "party");

    let consumer = GrafeoEngine::in_memory().unwrap();
    let mut local = schema("LocalThing", "entity", "common");
    local
        .custom_annotations
        .insert("source".into(), Json::String("mox".into()));
    consumer.ingest_schema(&local).await.unwrap();

    let config = consumer_config(artifact_path.to_str().unwrap(), "1.2.0");
    let stats = load_dependency_artifacts(&consumer, &consumer, &config, Some(dir.path()))
        .await
        .unwrap();

    assert_eq!(stats.schemas, 2, "both party schemas ingest: {stats:?}");
    assert_eq!(stats.properties, 3);
    assert_eq!(stats.codelists, 1);
    assert_eq!(stats.enum_values, 2);
    assert_eq!(stats.edges, 1);
    assert!(stats.shadowed_titles.is_empty());

    let schemas = consumer.list_schemas(None).await.unwrap();
    let contact = schemas
        .iter()
        .find(|s| s.title == "PartyContact")
        .expect("foreign entity must be in the graph");
    assert_eq!(contact.domain.as_deref(), Some("party"));
    assert!(
        contact.is_entity,
        "foreign classification must ride the artifact, not be re-derived"
    );
    assert_eq!(
        contact.custom_annotations.get("source"),
        Some(&Json::String("dependency:party".into())),
        "foreign schemas must carry dependency provenance"
    );
    assert_eq!(
        contact.custom_annotations.get("dependency_version"),
        Some(&Json::String("1.2.0".into()))
    );

    let props = consumer.get_properties("PartyContact").await.unwrap();
    let names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names.len(), 3, "all face properties ingest: {names:?}");
    assert!(names.contains(&"email") && names.contains(&"phone"));
    let status = props.iter().find(|p| p.name == "status").unwrap();
    assert_eq!(
        status.ref_target.as_deref(),
        Some("PartyKind"),
        "the refers target must ride the face"
    );

    let codelist = consumer.get_codelist("PartyKind").await.unwrap();
    assert!(codelist.is_some(), "codelists ride the face");
    let values = consumer.get_enum_values("PartyKind").await.unwrap();
    assert_eq!(values.len(), 2, "enum values ride the face");

    // The local schema is untouched by the dependency load.
    let local_after = schemas.iter().find(|s| s.title == "LocalThing").unwrap();
    assert_eq!(
        local_after.custom_annotations.get("source"),
        Some(&Json::String("mox".into()))
    );
}

// ── dedup precedence: local wins ─────────────────────────────────────────

#[tokio::test]
async fn local_title_wins_dedup_over_dependency() {
    let dir = tempfile::tempdir().unwrap();
    let artifact_path = dir.path().join("party/artifact.json");
    write_face_artifact(&party_face_graph().await, &artifact_path, "1.2.0", "party");

    let consumer = GrafeoEngine::in_memory().unwrap();
    let mut local = schema("PartyContact", "entity", "common");
    local.pg_table_name = "local_party_contact".to_string();
    consumer.ingest_schema(&local).await.unwrap();

    let config = consumer_config(artifact_path.to_str().unwrap(), "1.2.0");
    let stats = load_dependency_artifacts(&consumer, &consumer, &config, Some(dir.path()))
        .await
        .unwrap();

    assert_eq!(
        stats.shadowed_titles,
        vec!["PartyContact".to_string()],
        "the shadowed foreign title must be reported"
    );
    assert_eq!(
        stats.schemas, 1,
        "only the non-shadowed foreign schema ingests"
    );

    let matches = consumer
        .list_schemas(None)
        .await
        .unwrap()
        .into_iter()
        .filter(|s| s.title == "PartyContact")
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "the title must exist exactly once");
    assert_eq!(matches[0].domain.as_deref(), Some("common"));
    assert_eq!(matches[0].pg_table_name, "local_party_contact");
    assert!(
        !matches[0]
            .custom_annotations
            .get("source")
            .is_some_and(|v| v.as_str().unwrap_or("").starts_with("dependency:")),
        "the surviving copy is the local one"
    );

    let props = consumer.get_properties("PartyContact").await.unwrap();
    assert!(
        props.is_empty(),
        "foreign properties of a shadowed title must not leak: {props:?}"
    );
}

// ── doctor outcomes ──────────────────────────────────────────────────────

#[tokio::test]
async fn doctor_reports_dependency_graph() {
    let dir = tempfile::tempdir().unwrap();
    let ok_path = dir.path().join("party/artifact.json");
    write_face_artifact(&party_face_graph().await, &ok_path, "1.2.0", "party");
    let bare_path = dir.path().join("bare.artifact.json");
    artifact::export_ir(&party_face_graph().await, &bare_path).unwrap();

    // Resolved with a matching version.
    let config = consumer_config(ok_path.to_str().unwrap(), "1.2.0");
    let checks = check_domain_dependencies(&config, Some(dir.path()));
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].dependency_domain, "party");
    match &checks[0].status {
        DependencyCheckStatus::Resolved {
            artifact_version,
            schema_count,
        } => {
            assert_eq!(artifact_version.as_deref(), Some("1.2.0"));
            assert_eq!(*schema_count, 2);
        }
        other => panic!("expected Resolved, got {other:?}"),
    }

    // Version conflict: the face moved on since the pin.
    let config = consumer_config(ok_path.to_str().unwrap(), "2.0.0");
    let checks = check_domain_dependencies(&config, Some(dir.path()));
    match &checks[0].status {
        DependencyCheckStatus::VersionMismatch { declared, found } => {
            assert_eq!(declared, "2.0.0");
            assert_eq!(found, "1.2.0");
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }

    // Missing artifact file.
    let config = consumer_config(
        dir.path().join("gone/artifact.json").to_str().unwrap(),
        "1.0.0",
    );
    let checks = check_domain_dependencies(&config, Some(dir.path()));
    assert!(matches!(
        checks[0].status,
        DependencyCheckStatus::MissingArtifact { .. }
    ));

    // An unversioned face resolves, but cannot be verified.
    let config = consumer_config(bare_path.to_str().unwrap(), "1.2.0");
    let checks = check_domain_dependencies(&config, Some(dir.path()));
    match &checks[0].status {
        DependencyCheckStatus::Resolved {
            artifact_version, ..
        } => assert_eq!(artifact_version.as_deref(), None),
        other => panic!("expected unversioned Resolved, got {other:?}"),
    }
}

#[tokio::test]
async fn doctor_reports_cross_face_title_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    write_face_artifact(
        &party_face_graph().await,
        &dir.path().join("party/artifact.json"),
        "1.2.0",
        "party",
    );
    // A second face that also exports a `PartyContact` title.
    let geo = GrafeoEngine::in_memory().unwrap();
    geo.ingest_schema(&schema("RegionCode", "entity", "geo"))
        .await
        .unwrap();
    geo.ingest_schema(&schema("PartyContact", "entity", "geo"))
        .await
        .unwrap();
    write_face_artifact(&geo, &dir.path().join("geo/artifact.json"), "0.3.1", "geo");

    let toml = format!(
        "[domains.common]\nlabel = \"Common\"\nschema_dir = \"common\"\npostgres_schema = \"common\"\n\n[[domains.common.dependencies]]\ndomain = \"party\"\nsource = \"{}/party/artifact.json\"\nversion = \"1.2.0\"\n\n[[domains.common.dependencies]]\ndomain = \"geo\"\nsource = \"{}/geo/artifact.json\"\nversion = \"0.3.1\"\n",
        dir.path().display(),
        dir.path().display()
    );
    let config = parse_domain_config_str(&toml).unwrap();
    let checks = check_domain_dependencies(&config, Some(dir.path()));
    assert_eq!(checks.len(), 2);
    for check in &checks {
        match &check.status {
            DependencyCheckStatus::TitleConflict { titles } => {
                assert!(titles.contains(&"PartyContact".to_string()));
            }
            other => panic!(
                "expected TitleConflict for {}, got {other:?}",
                check.dependency_domain
            ),
        }
    }
}

// ── consumer e2e ─────────────────────────────────────────────────────────

const LOCAL_MOX: &str = r#"
package common

class LocalThingType {
    String [1] name
}
"#;

#[tokio::test]
async fn consumer_generates_against_pinned_face() {
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path();

    // Publisher: export the party face as a #275 artifact document.
    let face_path = root_path.join("publisher/out/party.artifact.json");
    write_face_artifact(&party_face_graph().await, &face_path, "1.2.0", "party");

    // Consumer: a local mox model + a pinned dependency on the party face.
    let consumer_dir = root_path.join("consumer");
    fs::create_dir_all(consumer_dir.join("model")).unwrap();
    fs::write(consumer_dir.join("model/common.mox"), LOCAL_MOX).unwrap();
    let config_path = consumer_dir.join("domains.toml");
    fs::write(
        &config_path,
        "[defaults]\noperations = [\"create\", \"read\", \"update\", \"delete\", \"list\"]\n\n[domains.common]\nlabel = \"Common\"\nschema_dir = \"common\"\npostgres_schema = \"common\"\n\n[[domains.common.dependencies]]\ndomain = \"party\"\nsource = \"../publisher/out/party.artifact.json\"\nversion = \"1.2.0\"\n",
    )
    .unwrap();

    let output = consumer_dir.join("generated");
    let mox_files = vec![consumer_dir.join("model/common.mox")];
    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &config_path,
        output: &output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(consumer_dir.join("profiles.toml")),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &mox_files,
        rosetta_files: &[],
        ddd_files: &[],
        evt_files: &[],
        deploy_files: &[],
        ifml_framework: &[],
        ifml_components: None,
        ifml_design_system: None,
        ux_rules: None,
        codegraph_rev: None,
        check: false,
    })
    .await
    .unwrap();

    let generated: Vec<PathBuf> = walkdir::WalkDir::new(&output)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .collect();
    assert!(!generated.is_empty(), "generation must produce files");

    let mut ddl = String::new();
    for path in &generated {
        if path.extension().and_then(|e| e.to_str()) == Some("sql") {
            ddl.push_str(&fs::read_to_string(path).unwrap());
            ddl.push('\n');
        }
    }
    assert!(
        ddl.contains("local_thing"),
        "the local entity must generate:\n{ddl}"
    );
    assert!(
        ddl.contains("party_contact"),
        "the pinned foreign entity must generate DDL:\n{ddl}"
    );
    assert!(
        ddl.contains("party_kind"),
        "the pinned foreign codelist must generate DDL:\n{ddl}"
    );
    let foreign_rust = generated
        .iter()
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("rs")
                && fs::read_to_string(p)
                    .map(|c| c.contains("PartyContact"))
                    .unwrap_or(false)
        })
        .count();
    assert!(
        foreign_rust > 0,
        "the foreign entity must appear in generated Rust output"
    );
}
// ── doctor command wiring ────────────────────────────────────────────────

#[test]
fn doctor_command_reports_dependency_outcomes() {
    use codegraph::init::commands::{DoctorArgs, cmd_add_domain, cmd_doctor};

    let root = tempfile::tempdir().unwrap();
    write_face_artifact(
        &block_on(party_face_graph()),
        &root.path().join("publisher/out/party.artifact.json"),
        "1.2.0",
        "party",
    );

    let project = root.path().join("app");
    std::fs::create_dir_all(project.join("model")).unwrap();
    std::fs::write(project.join("model/common.mox"), LOCAL_MOX).unwrap();
    std::fs::write(
        project.join("domains.toml"),
        "[domains.common]\nlabel = \"Common\"\nschema_dir = \"common\"\npostgres_schema = \"common\"\n",
    )
    .unwrap();
    cmd_add_domain(&project.join("domains.toml"), "billing", false).unwrap();

    // Pin the party face; the artifact lives outside the project.
    {
        let toml = std::fs::read_to_string(project.join("domains.toml")).unwrap();
        std::fs::write(
            project.join("domains.toml"),
            format!(
                "{toml}\n[[domains.common.dependencies]]\ndomain = \"party\"\nsource = \"../publisher/out/party.artifact.json\"\nversion = \"1.2.0\"\n"
            ),
        )
        .unwrap();
    }
    let args = |config: PathBuf| DoctorArgs {
        config,
        schemas: None,
        classifier: None,
        profiles_config: None,
        mox_files: vec![project.join("model/common.mox")],
        rosetta_files: vec![],
        ddd_files: vec![],
        evt_files: vec![],
        deploy_files: vec![],
        ifml_files: vec![],
    };
    let summary = cmd_doctor(&args(project.join("domains.toml"))).unwrap();
    assert_eq!(
        summary.hard_failures, 0,
        "resolved face must not fail doctor"
    );

    // A stale pin (face moved to 2.0.0) is a hard failure.
    let stale = root.path().join("stale");
    std::fs::create_dir_all(stale.join("model")).unwrap();
    std::fs::write(stale.join("model/common.mox"), LOCAL_MOX).unwrap();
    std::fs::write(
        stale.join("domains.toml"),
        format!(
            "[domains.common]\nlabel = \"Common\"\nschema_dir = \"common\"\npostgres_schema = \"common\"\n\n[[domains.common.dependencies]]\ndomain = \"party\"\nsource = \"{}\"\nversion = \"9.9.9\"\n",
            root.path().join("publisher/out/party.artifact.json").display()
        ),
    )
    .unwrap();
    assert!(
        cmd_doctor(&args(stale.join("domains.toml"))).is_err(),
        "version mismatch must fail doctor"
    );
}

fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Runtime::new().unwrap().block_on(fut)
}
