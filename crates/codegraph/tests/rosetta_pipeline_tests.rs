//! Rosetta pipeline threading tests (issue #257).
//!
//! `--rosetta-files` through `driver::run` / `driver::classify`: rosetta as
//! the sole model source, pass ordering before the JSON schema pass, the
//! model-source guard, and stderr notices.

use std::path::PathBuf;

const MODEL: &str = r#"
namespace pipeline.store
version "1.0.0"

type Product: <"A product">
	productId string (1..1)
	label string (0..1)
	price number (1..1)
	currency Currency (1..1)

enum Currency:
	EUR
	USD

type Holder:
	holderId string (1..1)

choice ProductType:
	Product
	Holder
"#;

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.store]
label = "Store"
schema_dir = "store"
postgres_schema = "store"
"#;

fn write_fixture(dir: &std::path::Path) -> (PathBuf, PathBuf) {
    let model = dir.join("store.rosetta");
    std::fs::write(&model, MODEL).unwrap();
    let domains = dir.join("domains.toml");
    std::fs::write(&domains, DOMAINS_TOML).unwrap();
    (domains, model)
}

fn run_args<'a>(
    config: &'a PathBuf,
    rosetta: &'a [PathBuf],
    output: &'a std::path::Path,
) -> codegraph::driver::RunArgs<'a> {
    codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: config,
        output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(PathBuf::from("definitely-absent-profiles.toml")),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &[],
        rosetta_files: rosetta,
        ddd_files: &[],
        evt_files: &[],
        deploy_files: &[],
        ifml_framework: &[],
        ifml_components: None,
        ifml_design_system: None,
        codegraph_rev: None,
        check: false,
        ux_rules: None,
    }
}

#[tokio::test]
async fn rosetta_only_run_generates_the_data_plane() {
    let dir = tempfile::tempdir().unwrap();
    let (config, model) = write_fixture(dir.path());
    let output = dir.path().join("generated");
    let rosetta = vec![model];

    codegraph::driver::run(run_args(&config, &rosetta, &output))
        .await
        .unwrap();

    let mut files: Vec<String> = Vec::new();
    for entry in walkdir::WalkDir::new(&output) {
        let entry = entry.unwrap();
        if entry.file_type().is_file() {
            files.push(
                entry
                    .path()
                    .strip_prefix(&output)
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/"),
            );
        }
    }
    let has = |suffix: &str| files.iter().any(|f| f.ends_with(suffix));

    // The type bridge drove the generators end-to-end.
    let product_ddl = files
        .iter()
        .find(|f| {
            f.starts_with("migrations/")
                && f.ends_with("_store_product.sql")
                && !f.ends_with("_rls.sql")
                && !f.ends_with("_trigger.sql")
                && !f.ends_with("_fts.sql")
        })
        .unwrap_or_else(|| panic!("product DDL missing; files: {files:?}"));
    assert!(!product_ddl.is_empty());
    assert!(has("src/entity/store_product.rs"), "entity missing");
    assert!(
        files
            .iter()
            .any(|f| f.starts_with("src/domain/store/product/")),
        "domain module missing"
    );

    // The enum codelist and the referenced type materialized.
    assert!(has("src/entity/store_currency.rs"));
    assert!(has("src/entity/store_holder.rs"));

    // Choices KEEP their unstripped names (gap-doc defect #9 fix): the
    // choice ProductType gets its own table/artifacts instead of colliding
    // with type Product at pg_table_name `product`.
    assert!(
        has("src/entity/store_product_type.rs"),
        "choice ProductType must be separately addressable after the #9 fix"
    );
}

#[tokio::test]
async fn run_without_any_model_source_is_a_config_error() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _model) = write_fixture(dir.path());
    let output = dir.path().join("generated");
    let rosetta: Vec<PathBuf> = Vec::new();

    let err = codegraph::driver::run(run_args(&config, &rosetta, &output))
        .await
        .expect_err("no sources must be rejected");
    assert!(err.to_string().contains("no model source"), "{err}");
}

#[tokio::test]
async fn rosetta_notice_mentions_the_primary_source() {
    let notice = codegraph::driver::rosetta_primary_notice();
    assert!(notice.contains(".rosetta"));
    assert!(notice.contains("--schemas fills gaps"));
}

#[tokio::test]
async fn classify_with_rosetta_only_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let (config, model) = write_fixture(dir.path());
    let rosetta = vec![model];

    // Rosetta-derived schemas are auto-scored (origin=rosetta never trips
    // the mox bypass) and appear in the report.
    codegraph::driver::classify(
        None,
        None,
        &config,
        Some("store"),
        codegraph::driver::ClassifyFormat::Json,
        &[],
        &rosetta,
    )
    .await
    .unwrap();
}

// ── #446 (A6): entity-reference columns must survive DDL + entity ───────

/// Minimal model reproducing #446: `Invitee` carries a required scalar
/// reference to `Parent` (plus a scalar builtin and an optional reference),
/// so the entity's model columns are ALL property-derived. With the bug,
/// the composition tree demotes the reference properties to jsonb (target
/// `is_entity` is false in the unclassified graph) and DDL/entity emit
/// audit-only skeletons while the dto resolves the fields.
const INVITEE_MODEL: &str = r#"
namespace repro.repro
: <"Reduction fixture for the rosetta entity-reference seam.">

version "1.0.0"

	typeAlias Uuid: <"Postgres uuid."> string(pattern: "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")

	type ParentType: <"Referenced entity.">
		[rootType]
		id Uuid (1..1) <"Primary key."> [metadata id]
		name string (1..1) <"Display name.">

	type InviteeType: <"Entity whose every model column is a property-derived reference or scalar.">
		[rootType]
		id Uuid (1..1) <"Primary key."> [metadata id]
		note string (0..1) <"Optional scalar.">
		parent ParentType (1..1) <"FK parent_id -> parent.id."> [metadata reference]
"#;

const INVITEE_DOMAINS_TOML: &str = r#"
[defaults]
app_name = "repro"
operations = ["create", "read", "update", "delete", "list"]

[domains.repro]
label = "Repro"
schema_dir = "repro"
postgres_schema = "repro"
"#;

/// Walk the generated tree into sorted relative paths.
fn generated_files_under(output: &std::path::Path) -> Vec<String> {
    let mut files: Vec<String> = walkdir::WalkDir::new(output)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            e.path()
                .strip_prefix(output)
                .unwrap()
                .display()
                .to_string()
                .replace('\\', "/")
        })
        .collect();
    files.sort();
    files
}

fn read_generated(output: &std::path::Path, suffix: &str) -> String {
    let files = generated_files_under(output);
    let rel = files
        .iter()
        .find(|f| f.ends_with(suffix))
        .unwrap_or_else(|| panic!("expected a file ending in {suffix}; files: {files:?}"));
    std::fs::read_to_string(output.join(rel)).unwrap()
}

/// #446 (A6): bridge-only rosetta graphs (no classification pass — the
/// flywheel compile gate's exact shape) must emit the entity-reference
/// columns in the DDL migration AND the SeaORM model, matching what the
/// dto/repository/command/query families already resolve.
#[tokio::test]
async fn rosetta_entity_reference_columns_survive_ddl_and_entity() {
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("repro.rosetta");
    std::fs::write(&model, INVITEE_MODEL).unwrap();
    let domains = dir.path().join("domains.toml");
    std::fs::write(&domains, INVITEE_DOMAINS_TOML).unwrap();
    let output = dir.path().join("generated");
    let config = codegraph_config::config::parse_domain_config_str(INVITEE_DOMAINS_TOML).unwrap();

    // Bridge-only ingestion: NO classification pass, exactly like the
    // flywheel compile gate — every schema node keeps the bridge's
    // VO-shaped default while its properties classify as entity references.
    let backend = codegraph_backend::create_backend(&codegraph_backend::BackendConfig::default())
        .await
        .unwrap();
    let outcome = codegraph::ingest::rosetta_ingest::ingest_rosetta_files(
        backend.ingestor(),
        backend.querier(),
        &[model],
        &config,
        &config.defaults.type_suffix,
    )
    .await
    .unwrap();
    assert_eq!(outcome.stats.types, 2, "both types bridged");

    let tera =
        codegraph::generate::template_engine::create_tera(std::path::Path::new("unused")).unwrap();
    let domain_types_dir = dir.path().join("domain-types");
    let hooks_tmp = tempfile::tempdir().unwrap();
    let report =
        codegraph::generate::run_generators_with_opts(codegraph::generate::GeneratorOpts {
            db: backend.querier(),
            config: &config,
            output_dir: &output,
            tera: &tera,
            ui_overrides: &codegraph_config::UiOverrideConfig::default(),
            ui_domains: &codegraph_config::UiDomainConfig::default(),
            schema_base_dir: std::path::Path::new(""),
            seed_config: None,
            domain_types_base: Some(&domain_types_dir),
            hooks_base: Some(hooks_tmp.path()),
            ext_points: None,
            build_plan: None,
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
        !report.has_errors(),
        "generation must be silent: {:?}",
        report
            .errors
            .iter()
            .map(|e| format!("{}/{}: {}", e.entity, e.generator, e.source))
            .collect::<Vec<_>>()
    );

    // The migration carries the reference FK column. (The FK *constraint*
    // plane is governed separately by the config-entity filter
    // (`retain_generated_fks`) and stays out of scope here: rosetta
    // domains.toml declares no `entities` lists, so cross-entity
    // constraints are filtered for every rosetta run, classified or not.)
    let ddl = read_generated(&output, "invitee.sql");
    assert!(
        ddl.contains("parent_id UUID"),
        "invitee DDL must carry the parent_id reference column:\n{ddl}"
    );

    // The SeaORM model carries the matching field (required → non-Option).
    let entity = read_generated(&output, "repro_invitee.rs");
    assert!(
        entity.contains("pub parent_id: Uuid"),
        "invitee entity model must carry pub parent_id: Uuid:\n{entity}"
    );
    assert!(
        entity.contains("pub note: Option<String>"),
        "invitee entity model must carry the optional scalar:\n{entity}"
    );

    // The dto plane already resolved these — pin it so the two families
    // cannot silently diverge again. The app-level dto re-exports from the
    // domain-types crate, which generates into its own base dir.
    let dto = read_generated(&output, "src/domain/repro/invitee/dto_response.rs");
    assert!(
        dto.contains("InviteeResponse"),
        "invitee response dto re-export must exist:\n{dto}"
    );
    let dt_dto =
        std::fs::read_to_string(domain_types_dir.join("src/repro/invitee/dto_response.rs"))
            .unwrap();
    assert!(
        dt_dto.contains("pub parent_id: uuid::Uuid"),
        "invitee response DTO must carry parent_id:\n{dt_dto}"
    );

    // The repository maps the create command onto the ActiveModel column.
    let repo = read_generated(&output, "src/domain/repro/invitee/repository_impl.rs");
    assert!(
        repo.contains("parent_id"),
        "invitee repository_impl must map parent_id:\n{repo}"
    );
}

// ── #333 (A2): single generator-ordering contract ────────────────────────

/// #333 (A2): Entity `Alpha` (domain `aaa`) carries a cross-entity include
/// of `Zeta` (domain `zzz`), and `aaa` generates BEFORE `zzz` — so
/// `repository(alpha)` renders before `dto(zeta)` has registered
/// `ZetaResponse` in the type registry. The generated `repository_impl.rs`
/// must import Zeta's Response from the module the dto generator registers
/// it under, and — debug builds — the pipeline must guarantee the registry
/// is warm before the first repository renders (the #333 debug invariant;
/// a cold registry silently drops the import and the app fails E0425).
///
/// Domain-level generation order overrides the intra-domain entity topo
/// sort, so no entity ordering can save this shape: only a warm registry
/// (or the c0cab515 single-segment fallback, kept as defense in depth)
/// produces the import.
///
/// Unlike [`rosetta_entity_reference_columns_survive_ddl_and_entity`] this
/// runs the full classified driver (`driver::run`) so the include path
/// resolves: includes gate on `is_entity`, which only the classification
/// pass sets. `ZetaType` is pinned as an entity via `force_entities` in its
/// own domain only — keeping it out of every `entities` list so the
/// cross-domain FK validation does not demand an undeclarable dependency.
#[tokio::test]
async fn rosetta_cross_entity_include_repository_import_is_resolvable() {
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("repro.rosetta");
    std::fs::write(
        &model,
        r#"
namespace repro.aaa
: <"Cross-domain include ordering fixture (#333). The zzz namespace holds the include target.">

version "1.0.0"

import repro.zzz.*

	typeAlias Uuid: <"Postgres uuid."> string(pattern: "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")

	type AlphaType: <"Includer entity; domain aaa generates before domain zzz.">
		[rootType]
		id Uuid (1..1) <"Primary key."> [metadata id]
		name string (1..1) <"Display name.">
		zeta ZetaType (0..1) <"FK zeta_id -> zeta.id."> [metadata reference]
"#,
    )
    .unwrap();
    let zeta_model = dir.path().join("zeta.rosetta");
    std::fs::write(
        &zeta_model,
        r#"
namespace repro.zzz
: <"Include target namespace; generates after aaa.">

version "1.0.0"

	typeAlias Uuid: <"Postgres uuid."> string(pattern: "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")

	type ZetaType: <"Included entity.">
		[rootType]
		id Uuid (1..1) <"Primary key."> [metadata id]
		label string (1..1) <"Label.">
"#,
    )
    .unwrap();
    let domains = dir.path().join("domains.toml");
    std::fs::write(
        &domains,
        r#"
[defaults]
app_name = "repro"
operations = ["create", "read", "update", "delete", "list"]

[domains.aaa]
label = "Aaa"
schema_dir = "aaa"
postgres_schema = "aaa"
depends_on = ["zzz"]
entities = ["AlphaType"]

[domains.aaa.entity_config.AlphaType]
role = "root"
allow_include = ["zeta"]

[domains.zzz]
label = "Zzz"
schema_dir = "zzz"
postgres_schema = "zzz"
force_entities = ["ZetaType"]
"#,
    )
    .unwrap();
    let output = dir.path().join("generated");
    let rosetta = vec![model, zeta_model];

    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &domains,
        output: &output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: None,
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &[],
        rosetta_files: &rosetta,
        ddd_files: &[],
        evt_files: &[],
        deploy_files: &[],
        ifml_framework: &[],
        ifml_components: None,
        ifml_design_system: None,
        codegraph_rev: None,
        check: false,
        ux_rules: None,
    })
    .await
    .unwrap();

    // The repository's include import names the dto-registered module path.
    let repo = read_generated(&output, "src/domain/aaa/alpha/repository_impl.rs");
    assert!(
        repo.contains("use crate::domain::zzz::zeta::dto_response::ZetaResponse;"),
        "alpha repository_impl must import ZetaResponse from its dto_response module:\n{repo}"
    );
    // And the fetch method actually uses the imported type.
    assert!(
        repo.contains("fetch_zeta_for_alpha"),
        "alpha repository_impl must carry the include fetch method:\n{repo}"
    );
}
