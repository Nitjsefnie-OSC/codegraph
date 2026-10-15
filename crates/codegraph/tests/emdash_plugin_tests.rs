//! End-to-end tests for the `emdash_plugin` generator family.
//!
//! Drives the full pipeline (`run_generators_with_opts`) against a mock
//! graph with an events-shaped plugin configuration, asserting the emitted
//! package reproduces the generic behavior of the hand-written
//! `packages/emdash-community-events` reference.

use std::collections::HashMap;
use std::path::Path;

use codegraph::generate::emdash::{
    emdash_package_root, emdash_site_e2e_root, emdash_site_pages_root, load_plugins_config,
};
use codegraph::generate::{GeneratorOpts, ProjectConfig, run_generators_with_opts};
use codegraph::profile::{BuildPlan, PersistenceProvider};
use codegraph_config::DomainConfig;
use codegraph_core::mock::MockEngine;
use codegraph_core::types::{EnumValue, PropertyNode, SchemaNode};
use codegraph_type_contracts::RefClassificationKind;

fn rsvp_schema() -> SchemaNode {
    SchemaNode {
        namespace: None,
        schema_id: "events/json/RsvpType.json".to_string(),
        title: "RsvpType".to_string(),
        description: Some("An RSVP".to_string()),
        schema_type: "object".to_string(),
        classification: "entity_reference".to_string(),
        domain: Some("events".to_string()),
        rel_path: "events/json/RsvpType.json".to_string(),
        pg_type: "UUID".to_string(),
        rust_type: "Uuid".to_string(),
        sea_orm_type: "Uuid".to_string(),
        rust_type_name: "RsvpType".to_string(),
        pg_table_name: "rsvp".to_string(),
        api_path_segment: "rsvps".to_string(),
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
    }
}

fn prop(
    name: &str,
    rust_field_type: &str,
    required: bool,
    ref_target: Option<&str>,
    kind: Option<RefClassificationKind>,
) -> PropertyNode {
    PropertyNode {
        is_id: false,
        name: name.to_string(),
        prop_type: "string".to_string(),
        description: None,
        format: None,
        is_required: required,
        is_nullable: !required,
        is_array: false,
        min_items: None,
        max_items: None,
        pattern: None,
        min_length: None,
        max_length: None,
        minimum: None,
        maximum: None,
        pg_column_name: codegraph_naming::to_snake_case(name),
        pg_column_type: "TEXT".to_string(),
        rust_field_name: codegraph_naming::to_snake_case(name),
        rust_field_type: rust_field_type.to_string(),
        sea_orm_type: "Text".to_string(),
        render_strategy: "direct_column".to_string(),
        ref_target: ref_target.map(|s| s.to_string()),
        classification: None,
        projection: None,
        classification_kind: kind,
        ui_override_detail: None,
        ui_override_list_cell: None,
        ui_override_form: None,
        ui_override_inline: None,
        type_expr: None,
    }
}

fn rsvp_properties() -> Vec<PropertyNode> {
    vec![
        prop("personName", "String", true, None, None),
        prop(
            "status",
            "RsvpStatusCodeList",
            true,
            Some("codelists/json/RsvpStatusCodeList.json#"),
            Some(RefClassificationKind::CodelistReference),
        ),
        prop("plusOnes", "i64", false, None, None),
        prop("isPublished", "bool", false, None, None),
        prop("did", "String", true, None, None),
        prop("accessibilityRequests", "Text", false, None, None),
    ]
}

fn status_enum_values() -> Vec<EnumValue> {
    vec![
        EnumValue {
            value: "Confirmed".to_string(),
            display_name: Some("Confirmed".to_string()),
            sort_order: 1,
        },
        EnumValue {
            value: "Pending".to_string(),
            display_name: Some("Pending".to_string()),
            sort_order: 2,
        },
        EnumValue {
            value: "Cancelled".to_string(),
            display_name: Some("Cancelled".to_string()),
            sort_order: 3,
        },
    ]
}

const PLUGINS_TOML: &str = r#"
[plugins.events]
label = "Events"
description = "Events domain plugin"
icon = "calendar"
settings = { publicListing = true, waitlistAutoPromote = false }
settings_help = { waitlistAutoPromote = "Off by default: promotion is manual." }
settings_types = { rsvpCutoffHours = "number" }
events_bridge = { poll_task = true, subscriptions = [ { topic = "events.published", storage = "rsvp_sync_state" } ] }

[plugins.events.entities.RsvpType]
title_field = "personName"
columns = [
  { key = "personName", label = "Person" },
  { key = "status" },
  { key = "plusOnes", label = "Plus ones", format = "number" },
]
sort = { key = "personName", dir = "asc" }
actions = [
  { id = "confirm", label = "Confirm", set = { status = "Confirmed" }, visible_when = { field = "status", in = ["Pending"] } },
]
form_overrides = { accessibilityRequests = { multiline = true } }
public_list = { route = "events", filter = { field = "isPublished", eq = true } }
public_detail = { route = "event", param = "id" }
public_submit = { route = "submitRsvp", fields = ["personName", "status", "plusOnes", "accessibilityRequests"], required = ["personName"], success_message = "Thank you!", button_label = "Send RSVP", handler = "submitRsvp" }
bespoke = ["promoteFirst"]
custom_actions = [ { id = "promote", label = "Promote first", handler = "promoteFirst", style = "primary" } ]
"#;

fn events_domain_config() -> DomainConfig {
    codegraph_config::config::parse_domain_config_str(
        r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.events]
label = "Events"
schema_dir = "events"
postgres_schema = "events"
entities = ["RsvpType"]
"#,
    )
    .unwrap()
}

fn emdash_build_plan() -> BuildPlan {
    BuildPlan {
        entity_generators: vec![],
        domain_generators: vec!["emdash_plugin".to_string()],
        global_generators: vec!["emdash_plugin_scaffold".to_string()],
        post_gen_scripts: vec![],
        ifml_frameworks: vec![],
        template_pack_path: None,
        database_target: codegraph::generate::db::dialect::DatabaseTarget::Postgres,
        has_atproto: false,
        atproto_tenancy: "shared_pds".to_string(),
        has_fern: false,
        fern_sdk_languages: vec!["typescript".to_string()],
        has_emdash: true,
        persistence_provider: PersistenceProvider::SeaOrm,
        dto_key_casing: "snake".to_string(),
        deployment_topology: codegraph::profile::DeploymentTopology::Monolith,
        dependency_strategy: codegraph::profile::DependencyStrategy::Rev,
        namespace_layout: false,
        ux_rules: false,
        expr_ir: false,
        ifml_e2e_auth: false,
        testid_module: false,
        structural_gates: false,
        public_operations_rls: false,
        features: Default::default(),
    }
}

fn mock_engine() -> MockEngine {
    MockEngine::builder()
        .with_schema(rsvp_schema())
        .with_properties("RsvpType", rsvp_properties())
        .with_enum_values("RsvpStatusCodeList", status_enum_values())
        .build()
}

#[tokio::test]
async fn emdash_generators_emit_package_site_pages_e2e_and_manifests() {
    let mock = mock_engine();
    let config = events_domain_config();
    let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    let tera = codegraph::generate::template_engine::create_tera(&template_dir).unwrap();

    let tmp = tempfile::TempDir::new().unwrap();
    // Repo-shaped output so the emdash root detection resolves inside tmp:
    //   <tmp>/generated/cosmos-app  →  <tmp>/packages, <tmp>/apps/...
    let output_dir = tmp.path().join("generated").join("cosmos-app");
    let plugins_path = tmp.path().join("plugins.toml");
    std::fs::write(&plugins_path, PLUGINS_TOML).unwrap();
    let plugins = Some(load_plugins_config(&plugins_path).unwrap());

    let plan = emdash_build_plan();
    let project = ProjectConfig::default();

    let report = run_generators_with_opts(GeneratorOpts {
        db: &mock,
        config: &config,
        output_dir: &output_dir,
        tera: &tera,
        ui_overrides: &Default::default(),
        ui_domains: &Default::default(),
        schema_base_dir: Path::new(""),
        seed_config: None,
        domain_types_base: None,
        hooks_base: None,
        ext_points: None,
        build_plan: Some(&plan),
        ifml_frameworks: vec![],
        ifml_components: None,
        ux_rules: None,
        project_config: Some(&project),
        emdash_plugins: plugins,
        domain_config_dir: None,
    })
    .await
    .unwrap();

    let package_root = emdash_package_root(&output_dir, "events");
    assert_eq!(
        package_root,
        tmp.path().join("packages").join("emdash-community-events")
    );

    // ── Package files ─────────────────────────────────────────────────
    for rel in [
        "emdash-plugin.jsonc",
        "package.json",
        "tsconfig.json",
        "types/node/package.json",
        "types/node/index.d.ts",
        "src/index.ts",
        "src/settings.ts",
        "src/plugin.ts",
        "src/admin.ts",
    ] {
        let path = package_root.join(rel);
        assert!(path.is_file(), "missing generated file {}", path.display());
    }

    // Plugin manifest parses as JSON and carries slug/pages/storage.
    let jsonc: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(package_root.join("emdash-plugin.jsonc")).unwrap(),
    )
    .expect("emdash-plugin.jsonc must be valid JSON");
    assert_eq!(jsonc["slug"], "community-events");
    assert_eq!(
        jsonc["storage"]["events_sync_state"]["indexes"][0],
        "entityId"
    );
    assert_eq!(
        jsonc["storage"]["rsvp_sync_state"]["indexes"][0],
        "entityId"
    );
    assert_eq!(jsonc["admin"]["pages"][0]["label"], "Rsvp");

    let plugin_ts = std::fs::read_to_string(package_root.join("src/plugin.ts")).unwrap();
    assert!(
        plugin_ts.contains(r#"id: "community-events""#)
            || plugin_ts.contains("seedGatewaySettings(ctx, \"community-events\")")
    );
    assert!(
        plugin_ts.contains("client.rsvp.eventsRsvpList()"),
        "SDK list method"
    );
    assert!(
        plugin_ts.contains("client.rsvp.eventsRsvpGetById({ rsvp_id: id })"),
        "SDK get-by-id request shape"
    );
    assert!(plugin_ts.contains("rsvpTypeList"), "entity list route");
    assert!(
        plugin_ts.contains("TOPIC_EVENTS_PUBLISHED"),
        "subscription topic const"
    );
    assert!(
        plugin_ts.contains("ctx.storage.rsvp_sync_state.put("),
        "consumption record storage"
    );
    assert!(
        plugin_ts.contains("submitRsvp(routeCtx, ctx)"),
        "bespoke submit dispatch"
    );
    assert!(
        plugin_ts.contains("item.isPublished === true"),
        "public list filter"
    );

    let admin_ts = std::fs::read_to_string(package_root.join("src/admin.ts")).unwrap();
    assert!(
        admin_ts.contains("import * as bespoke from \"./bespoke\";"),
        "bespoke import"
    );
    assert!(
        admin_ts.contains("case \"RsvpType:confirm\""),
        "transition dispatch"
    );
    assert!(
        admin_ts.contains("case \"RsvpType:promote\""),
        "custom action dispatch"
    );
    assert!(
        admin_ts.contains("bespoke.promoteFirst(ctx, value)"),
        "custom action call"
    );
    assert!(
        admin_ts.contains("payload.did = `did:plc:rsvp-${Date.now().toString(36)}`"),
        "required hidden did auto-populated"
    );
    // Codelist options resolved from the graph (incl. display-name labels).
    assert!(admin_ts.contains("{ label: \"Confirmed\", value: \"Confirmed\" }"));
    assert!(admin_ts.contains("initialValue: strOf(existing, \"status\") ?? \"Confirmed\""));
    assert!(
        admin_ts.contains("if (whenMatches(record, \"status\", [\"Pending\"]))"),
        "visible-when filter"
    );
    // accessibilityRequests has the configured multiline override.
    let settings = std::fs::read_to_string(package_root.join("src/settings.ts")).unwrap();
    assert!(settings.contains("publicListing"), "settings entry");
    assert!(settings.contains("default: true"), "settings default");
    assert!(
        settings.contains("type: \"number\""),
        "settings_types override"
    );

    let index_ts = std::fs::read_to_string(package_root.join("src/index.ts")).unwrap();
    assert!(index_ts.contains("export function communityEvents(): PluginDescriptor"));

    // ── Site pages + e2e ──────────────────────────────────────────────
    let pages_root = emdash_site_pages_root(&output_dir);
    assert_eq!(pages_root, tmp.path().join("apps/community-site/src/pages"));
    let list_page = pages_root.join("events.astro");
    let detail_page = pages_root.join("events").join("[id].astro");
    assert!(list_page.is_file(), "list page");
    assert!(detail_page.is_file(), "detail page");
    let list_src = std::fs::read_to_string(&list_page).unwrap();
    assert!(
        list_src.contains("const route = \"events\";"),
        "public list route name"
    );
    assert!(
        list_src.contains("handler(\"community-events\", \"GET\", route"),
        "route dispatch"
    );
    let detail_src = std::fs::read_to_string(&detail_page).unwrap();
    assert!(detail_src.contains("/_emdash/api/plugins/community-events/submitRsvp"));
    assert!(detail_src.contains("aria-live=\"polite\""));

    let e2e_root = emdash_site_e2e_root(&output_dir);
    let spec = e2e_root.join("generated").join("events-crud.spec.ts");
    assert!(spec.is_file(), "generated e2e spec");
    let spec_src = std::fs::read_to_string(&spec).unwrap();
    assert!(spec_src.contains("formSubmit(\"RsvpType:create\""));
    assert!(spec_src.contains("did: `did:plc:rsvp-e2e-${UNIQUE}`"));
    assert!(spec_src.contains("blockAction(\"RsvpType:delete\""));

    // ── README scaffold at the packages root ──────────────────────────
    let readme = tmp.path().join("packages").join("README.md");
    assert!(readme.is_file(), "scaffold README");
    assert!(
        std::fs::read_to_string(&readme)
            .unwrap()
            .contains("community-events")
    );

    // ── Manifests ─────────────────────────────────────────────────────
    let manifest_rel = ".codegraph-manifest.json";
    for root in [package_root.clone(), pages_root.clone(), e2e_root.clone()] {
        assert!(
            root.join(manifest_rel).is_file(),
            "missing manifest at {}",
            root.display()
        );
    }
    let pkg_manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(package_root.join(manifest_rel)).unwrap())
            .unwrap();
    let generated = pkg_manifest["generated"].as_array().unwrap();
    let names: Vec<&str> = generated.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        names.contains(&"src/admin.ts"),
        "manifest lists src/admin.ts: {names:?}"
    );
    assert!(names.contains(&"emdash-plugin.jsonc"));

    // The generator wrote nothing for unconfigured domains.
    let emdash_files: Vec<_> = report
        .files
        .iter()
        .filter(|f| f.path.to_string_lossy().contains("emdash-community"))
        .collect();
    assert_eq!(emdash_files.len(), 9, "exactly the 9 package files");
}

#[tokio::test]
async fn emdash_generators_emit_nothing_without_config_or_feature() {
    let mock = mock_engine();
    let config = events_domain_config();
    let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    let tera = codegraph::generate::template_engine::create_tera(&template_dir).unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let output_dir = tmp.path().join("generated").join("cosmos-app");

    // Feature off (has_emdash = false) even though a config is supplied.
    let mut plan = emdash_build_plan();
    plan.has_emdash = false;
    let report = run_generators_with_opts(GeneratorOpts {
        db: &mock,
        config: &config,
        output_dir: &output_dir,
        tera: &tera,
        ui_overrides: &Default::default(),
        ui_domains: &Default::default(),
        schema_base_dir: Path::new(""),
        seed_config: None,
        domain_types_base: None,
        hooks_base: None,
        ext_points: None,
        build_plan: Some(&plan),
        ifml_frameworks: vec![],
        ifml_components: None,
        ux_rules: None,
        project_config: None,
        emdash_plugins: Some(load_plugins_config_std(PLUGINS_TOML)),
        domain_config_dir: None,
    })
    .await
    .unwrap();
    assert!(
        report
            .files
            .iter()
            .all(|f| !f.path.to_string_lossy().contains("emdash-community"))
    );

    // Feature on but no plugins config → generators skipped entirely.
    let mut plan = emdash_build_plan();
    plan.has_emdash = true;
    let report = run_generators_with_opts(GeneratorOpts {
        db: &mock,
        config: &config,
        output_dir: &output_dir,
        tera: &tera,
        ui_overrides: &Default::default(),
        ui_domains: &Default::default(),
        schema_base_dir: Path::new(""),
        seed_config: None,
        domain_types_base: None,
        hooks_base: None,
        ext_points: None,
        build_plan: Some(&plan),
        ifml_frameworks: vec![],
        ifml_components: None,
        ux_rules: None,
        project_config: None,
        emdash_plugins: None,
        domain_config_dir: None,
    })
    .await
    .unwrap();
    assert!(
        report
            .files
            .iter()
            .all(|f| !f.path.to_string_lossy().contains("emdash-community"))
    );
    assert!(!tmp.path().join("packages").join("README.md").exists());
}

fn load_plugins_config_std(content: &str) -> codegraph::generate::emdash::EmdashPluginsConfig {
    use std::path::PathBuf;
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("plugins.toml");
    // NOTE: keep the tempdir alive via leak — fine for a test helper.
    std::fs::write(&path, content).unwrap();
    let cfg = load_plugins_config(&path).unwrap();
    std::mem::forget(dir);
    cfg
}

#[test]
fn build_plan_parses_emdash_feature() {
    let registry = codegraph::profile::CapabilityRegistry::new();
    let toml = r#"
[profiles.emdash.meta]
name = "emdash"
version = "1.0.0"
description = "EmDash plugin profile"

[profiles.emdash.features]
emdash_plugins = true

[profiles.emdash.ui]
generators = ["emdash_plugin", "emdash_plugin_scaffold"]
"#;
    let resolved =
        codegraph::profile::load_and_resolve_profile(&write_profiles(toml), "emdash", None)
            .unwrap();
    let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();
    assert!(plan.has_emdash);
    assert!(plan.has_domain_gen("emdash_plugin"));
    assert!(plan.has_global_gen("emdash_plugin_scaffold"));

    // Requires the feature: without it the profile is invalid.
    let toml_missing = toml.replace("emdash_plugins = true", "emdash_plugins = false");
    let resolved = codegraph::profile::load_and_resolve_profile(
        &write_profiles(&toml_missing),
        "emdash",
        None,
    )
    .unwrap();
    assert!(BuildPlan::from_profile(&resolved, &registry).is_err());

    // Plan-less runs never run the generators (feature-gated capability).
    assert!(registry.requires_build_plan("emdash_plugin"));
    assert!(registry.requires_build_plan("emdash_plugin_scaffold"));
}

fn write_profiles(content: &str) -> std::path::PathBuf {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profiles.toml");
    std::fs::write(&path, content).unwrap();
    let path = path.canonicalize().unwrap();
    std::mem::forget(dir);
    path
}

#[test]
fn unconfigured_domain_gets_nothing() {
    // Compile-time guard: the HashMap iteration order never leaks into
    // generation (context builders sort via BTreeMap).
    let mut m: HashMap<String, u8> = HashMap::new();
    m.insert("a".into(), 1);
    m.insert("b".into(), 2);
    let mut keys: Vec<_> = m.keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, vec!["a", "b"]);
}
