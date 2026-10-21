//! mox op/derived consuming-generator tests (issue #501, B5 of #218).
//!
//! The #218 ingest surface persists `MoxOperationNode` / `MoxDerivedFeatureNode`
//! but — before this suite — nothing downstream consumed the behavior
//! surface: derived features not matching a stored field were silently
//! dropped, ops generated nothing, and body-less ops were invisible.
//!
//! Slices (independently shippable, per the issue):
//! 1. derived `expr` features → read-only response fields + accessors
//! 2. `rust`/`expr`-bodied ops → impl methods with the rexlang
//!    rex-backend-rust fixed signature (`pub fn name(&self, res: &Resource,
//!    params) -> Ret`; `rust` bodies verbatim, `expr` bodies lowered through
//!    the supported subset or `TODO(#501)` markers)
//! 3. body-less ops → a generated, machine-checkable seam (signature-complete
//!    `todo!` stub) plus a doctor INFO section naming every unimplemented seam
//!
//! Presence-gating: models without ops/derived features produce
//! byte-identical output (the standing contract) — pinned by
//! `without_mox_ops_output_is_unchanged`.

use std::fs;
use std::path::{Path, PathBuf};

use codegraph_grafeo::GrafeoEngine;

/// Derived feature that is NOT a stored field (`upperId` names nothing in
/// CandidateType.json) — the shape slice 1 keys on. The stored-field case
/// (annotate + omit from create/update) is pinned by `mox_dto_tests`.
const DERIVED_NONSTORED_MOX: &str = r#"
package recruiting

class CandidateType {
    String candidateId
    String status

    derived String upperId {
        expr { candidateId }
    }
}
"#;

/// A `rust`-bodied op and an `expr`-bodied op — the shape slice 2 keys on.
/// The `rust` body must be embedded verbatim (byte-identical to the source
/// text between the braces); the `expr` body lowers through the supported
/// subset (feature refs → `self.<field>`).
const RUST_AND_EXPR_OPS_MOX: &str = r#"
package recruiting

class CandidateType {
    String candidateId
    String status

    op int idLength(String prefix) {
        rust { prefix.len() as i32 }
    }

    op boolean hasShortId() {
        expr { candidateId == "" }
    }
}
"#;

/// A body-less op — the rexlang "abstract hook" gap class. Slice 3 makes the
/// seam visible: a signature-complete stub in the generated module plus a
/// doctor INFO entry.
const BODYLESS_OP_MOX: &str = r#"
package recruiting

class CandidateType {
    String candidateId
    String status

    op int auditCount()
}
"#;

/// Writes a `.mox` file (no vocabulary ⇒ no snapshot needed).
fn write_mox_fixture(dir: &Path, name: &str, source: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, source).unwrap();
    path
}

/// Ingest the fixture JSON schemas (schema entities must exist for class
/// name matching).
async fn ingest_fixture_schemas(
    engine: &GrafeoEngine,
    config: &codegraph_config::config::DomainConfig,
) {
    let classifier = codegraph_classifier::config::parse_classifier_config(Path::new(
        "tests/fixtures/classifier.toml",
    ))
    .unwrap();
    let entity_names: std::collections::HashSet<String> = config
        .domains
        .values()
        .flat_map(|d| d.entities.iter().cloned())
        .collect();
    codegraph::ingest::async_ingest::ingest_schemas(
        engine,
        Path::new("tests/fixtures/schemas"),
        &classifier,
        &entity_names,
        &codegraph_config::UiOverrideConfig::default(),
        &config.defaults.type_suffix,
    )
    .await
    .unwrap();
}

/// Ingest the fixture's mox file (the class must match the schema entity).
async fn ingest_mox(engine: &GrafeoEngine, mox: &Path) {
    let config =
        codegraph_config::config::parse_domain_config(Path::new("tests/fixtures/domains.toml"))
            .unwrap();
    let stats = codegraph::ingest::mox_ingest::ingest_mox_files(
        engine,
        engine,
        &[mox.to_path_buf()],
        &config,
        &config.defaults.type_suffix,
    )
    .await
    .unwrap()
    .stats;
    assert_eq!(
        stats.skipped, 0,
        "fixture class must match the schema entity"
    );
}

fn profiles_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .join("profiles.toml")
}

/// Run the full generator suite (minus gRPC) over the fixture graph.
async fn run_generation(
    engine: &GrafeoEngine,
    mox: Option<PathBuf>,
    domain_types_dir: &Path,
    output_dir: &Path,
    hooks_tmp: &Path,
) {
    let tera = codegraph::generate::template_engine::create_tera(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("templates"),
    )
    .unwrap();

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let type_contracts_path = workspace_root
        .join("crates")
        .join("codegraph-type-contracts");
    let workflow_path = workspace_root.join("crates").join("codegraph-workflow");
    let profiles = profiles_path();

    let registry = codegraph::profile::CapabilityRegistry::new();
    let mut resolved =
        codegraph::profile::load_and_resolve_profile(&profiles, "default", None).unwrap();
    for section in resolved.sections.values_mut() {
        section.generators.retain(|g| !g.starts_with("grpc_"));
    }
    let plan = codegraph::profile::BuildPlan::from_profile(&resolved, &registry).unwrap();

    let project_config = codegraph::generate::ProjectConfig {
        identity: codegraph::generate::IdentityConfig {
            app_name: "test-app".into(),
            domain_types_crate: "domain_types".into(),
            generator_name: "codegraph-test".into(),
            ..Default::default()
        },
        paths: codegraph::generate::PathsConfig {
            type_contracts_base: type_contracts_path.to_string_lossy().to_string(),
            codegraph_workflow_base: workflow_path.to_string_lossy().to_string(),
            domain_types_base: "domain-types".into(),
            ..Default::default()
        },
        codegen: codegraph::generate::CodegenConfig {
            types_import_prefix: "crate::types".into(),
            ..Default::default()
        },
        ..Default::default()
    };

    let config =
        codegraph_config::config::parse_domain_config(Path::new("tests/fixtures/domains.toml"))
            .unwrap();

    if let Some(mox) = &mox {
        ingest_mox(engine, mox).await;
    }

    let report =
        codegraph::generate::run_generators_with_opts(codegraph::generate::GeneratorOpts {
            db: engine,
            config: &config,
            output_dir,
            tera: &tera,
            ui_overrides: &Default::default(),
            ui_domains: &Default::default(),
            schema_base_dir: Path::new(""),
            seed_config: None,
            domain_types_base: Some(domain_types_dir),
            hooks_base: Some(hooks_tmp),
            ext_points: None,
            build_plan: Some(&plan),
            ifml_frameworks: vec![],
            ifml_components: None,
            ux_rules: None,
            project_config: Some(&project_config),
            emdash_plugins: None,
            domain_config_dir: None,
        })
        .await
        .unwrap();
    assert!(!report.has_errors(), "generation reported errors");
}

/// Candidate domain-types DTO files for the fixture entity.
fn read_dtos(domain_types_dir: &Path) -> (String, String, String) {
    let dto_dir = domain_types_dir
        .join("src")
        .join("recruiting")
        .join("candidate");
    let response = fs::read_to_string(dto_dir.join("dto_response.rs"))
        .expect("dto_response.rs must be generated");
    let create =
        fs::read_to_string(dto_dir.join("dto_create.rs")).expect("dto_create.rs must be generated");
    let update =
        fs::read_to_string(dto_dir.join("dto_update.rs")).expect("dto_update.rs must be generated");
    (response, create, update)
}

/// The slice-2/3 artifact: per-domain mox operation module in the app crate.
fn read_mox_ops(output_dir: &Path) -> String {
    fs::read_to_string(
        output_dir
            .join("src")
            .join("domain")
            .join("recruiting")
            .join("mox_ops.rs"),
    )
    .expect("src/domain/recruiting/mox_ops.rs must be generated for a model with ops")
}

// ── Slice 1: derived `expr` features → read-only response fields ──

#[tokio::test]
async fn mox_derived_feature_exposed_in_response() {
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("out");
    let domain_types_dir = dir.path().join("domain-types");
    fs::create_dir_all(&output_dir).unwrap();

    let engine = GrafeoEngine::in_memory().unwrap();
    let config =
        codegraph_config::config::parse_domain_config(Path::new("tests/fixtures/domains.toml"))
            .unwrap();
    ingest_fixture_schemas(&engine, &config).await;

    let mox_dir = tempfile::tempdir().unwrap();
    let mox = write_mox_fixture(mox_dir.path(), "model.mox", DERIVED_NONSTORED_MOX);

    let hooks_tmp = tempfile::tempdir().unwrap();
    run_generation(
        &engine,
        Some(mox),
        &domain_types_dir,
        &output_dir,
        hooks_tmp.path(),
    )
    .await;

    let (response, create, update) = read_dtos(&domain_types_dir);

    // Response struct: the non-stored derived feature is exposed as a
    // read-only field (serde-default so responses deserialize before the
    // SELECT-time population plane lands).
    assert!(
        response.contains("/// Computed by the mox domain model (derived) — read-only."),
        "response must mark the derived field:\n{response}"
    );
    assert!(
        response.contains("pub upper_id: String,"),
        "response must expose the non-stored derived field:\n{response}"
    );

    // Accessor with the rexlang fixed signature, lowered from the expr
    // body (feature ref → self.<field>); the arena is the domain's REAL
    // Resource (issue #508 R4) — no placeholder.
    assert!(
        response.contains(
            "pub fn upper_id(&self, res: &crate::recruiting::mox_types::Resource) -> String"
        ),
        "response module must carry the computed accessor:\n{response}"
    );
    assert!(
        response.contains("self.candidate_id.clone()"),
        "accessor body must lower the expr (feature ref → self.<field>):\n{response}"
    );
    assert!(
        !response.contains("pub struct Resource;"),
        "the module-local Resource placeholder is retired (issue #508 R4):\n{response}"
    );

    // The real Resource contract lives in the domain's mox_types module:
    // mirrors + the synchronous arena.
    let mox_types = fs::read_to_string(
        domain_types_dir
            .join("src")
            .join("recruiting")
            .join("mox_types.rs"),
    )
    .expect("src/recruiting/mox_types.rs must be generated for a model with derived features");
    for fragment in [
        "pub struct Candidate",
        "pub struct Resource",
        "pub fn insert_candidate(",
        "pub fn get_candidate(&self, id: Uuid) -> Option<&Candidate>",
        "pub organization_id: Uuid,",
    ] {
        assert!(
            mox_types.contains(fragment),
            "mox_types must carry the real Resource contract (`{fragment}`):\n{mox_types}"
        );
    }

    // Create/update: derived features are never writable inputs.
    assert!(
        !create.contains("upper_id"),
        "create DTO must not expose derived upper_id:\n{create}"
    );
    assert!(
        !update.contains("upper_id"),
        "update DTO must not expose derived upper_id:\n{update}"
    );

    // Control: stored fields are untouched in all three DTOs.
    for (name, content) in [
        ("response", &response),
        ("create", &create),
        ("update", &update),
    ] {
        assert!(
            content.contains("pub candidate_id"),
            "{name} DTO must keep the stored candidate_id field"
        );
        assert!(
            content.contains("pub status"),
            "{name} DTO must keep the stored status field"
        );
    }
}

// ── Slice 2: `rust`/`expr`-bodied ops → impl methods ──

#[tokio::test]
async fn mox_op_body_emitted_with_fixed_signature() {
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("out");
    let domain_types_dir = dir.path().join("domain-types");
    fs::create_dir_all(&output_dir).unwrap();

    let engine = GrafeoEngine::in_memory().unwrap();
    let config =
        codegraph_config::config::parse_domain_config(Path::new("tests/fixtures/domains.toml"))
            .unwrap();
    ingest_fixture_schemas(&engine, &config).await;

    let mox_dir = tempfile::tempdir().unwrap();
    let mox = write_mox_fixture(mox_dir.path(), "model.mox", RUST_AND_EXPR_OPS_MOX);

    let hooks_tmp = tempfile::tempdir().unwrap();
    run_generation(
        &engine,
        Some(mox),
        &domain_types_dir,
        &output_dir,
        hooks_tmp.path(),
    )
    .await;

    let ops_rs = read_mox_ops(&output_dir);

    // The rexlang fixed-signature seam (issue #508 R1): impl on the
    // persistence-agnostic row mirror from the domain's mox_types module —
    // NOT the SeaORM Model, so the module compiles under workers-cornucopia
    // on both targets.
    assert!(
        !ops_rs.contains("pub struct Resource;"),
        "the module-local Resource placeholder is retired (issue #508 R4):\n{ops_rs}"
    );
    assert!(
        !ops_rs.contains("crate::entity::"),
        "ops must not depend on the SeaORM entity module:\n{ops_rs}"
    );
    assert!(
        ops_rs.contains("use domain_types::recruiting::mox_types::{Candidate, Resource};"),
        "ops must impl on the mox_types row mirror:\n{ops_rs}"
    );
    assert!(
        ops_rs.contains("impl Candidate {"),
        "ops must be emitted as an inherent impl on the mirror:\n{ops_rs}"
    );

    // mox_types carries the mirror + the real Resource arena.
    let mox_types = fs::read_to_string(
        domain_types_dir
            .join("src")
            .join("recruiting")
            .join("mox_types.rs"),
    )
    .expect("src/recruiting/mox_types.rs must be generated for a model with ops");
    assert!(
        mox_types.contains("pub struct Resource"),
        "mox_types must carry the real Resource contract:\n{mox_types}"
    );

    // `rust` body: fixed signature with rex type mapping (int → i32, String
    // param stays owned String) and the body embedded verbatim — byte-equal
    // to the source text between the braces, on its own line.
    assert!(
        ops_rs.contains("pub fn id_length(&self, res: &Resource, prefix: String) -> i32 {"),
        "rust-bodied op must carry the fixed signature:\n{ops_rs}"
    );
    assert!(
        ops_rs.contains("-> i32 {\n prefix.len() as i32 \n    }"),
        "rust body must be embedded verbatim (own line, byte-identical):\n{ops_rs}"
    );

    // `expr` body: same fixed signature, lowered through the supported
    // subset (feature ref → self.<field>, comparison, boolean return).
    assert!(
        ops_rs.contains("pub fn has_short_id(&self, res: &Resource) -> bool {"),
        "expr-bodied op must carry the fixed signature:\n{ops_rs}"
    );
    assert!(
        ops_rs.contains("self.candidate_id"),
        "expr body must lower feature refs to self.<field>:\n{ops_rs}"
    );
}

// ── Slice 2b: the tested invocation contract (issue #508 R2) ──

/// Qualifying ops (bodied, primitive signature) route as
/// `POST {id}/op/{snake}` through the handler/router planes — the
/// generated-and-never-called era is over.
#[tokio::test]
async fn mox_op_routes_wired_through_handler_and_router() {
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("out");
    let domain_types_dir = dir.path().join("domain-types");
    fs::create_dir_all(&output_dir).unwrap();

    let engine = GrafeoEngine::in_memory().unwrap();
    let config =
        codegraph_config::config::parse_domain_config(Path::new("tests/fixtures/domains.toml"))
            .unwrap();
    ingest_fixture_schemas(&engine, &config).await;

    let mox_dir = tempfile::tempdir().unwrap();
    let mox = write_mox_fixture(mox_dir.path(), "model.mox", RUST_AND_EXPR_OPS_MOX);

    let hooks_tmp = tempfile::tempdir().unwrap();
    run_generation(
        &engine,
        Some(mox),
        &domain_types_dir,
        &output_dir,
        hooks_tmp.path(),
    )
    .await;

    let handler = fs::read_to_string(
        output_dir
            .join("src")
            .join("api")
            .join("recruiting")
            .join("candidate_handler.rs"),
    )
    .expect("candidate handler module must be generated");
    let router = fs::read_to_string(
        output_dir
            .join("src")
            .join("api")
            .join("recruiting")
            .join("router.rs"),
    )
    .expect("recruiting router must be generated");

    // Handler: one action fn per qualifying op — loads the row, copies it
    // into the mox_types mirror, builds the real Resource arena, and
    // INVOKES the op with the body params.
    for fragment in [
        "pub async fn mox_op_id_length(",
        "pub async fn mox_op_has_short_id(",
        "res.insert_candidate(mirror);",
        "target.id_length(&res, prefix)",
        "target.has_short_id(&res)",
        "\"data\": out",
    ] {
        assert!(
            handler.contains(fragment),
            "handler must wire the mox op invocation (`{fragment}`):\n{handler}"
        );
    }

    // Router: the action routes, guarded like every other action route.
    for fragment in ["/op/id_length", "/op/has_short_id", "mox_op_id_length"] {
        assert!(
            router.contains(fragment),
            "router must mount the mox op route (`{fragment}`):\n{router}"
        );
    }
}

// ── Slice 3: body-less ops → a visible seam ──

#[tokio::test]
async fn mox_bodyless_op_seam_is_visible() {
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("out");
    let domain_types_dir = dir.path().join("domain-types");
    fs::create_dir_all(&output_dir).unwrap();

    let engine = GrafeoEngine::in_memory().unwrap();
    let config =
        codegraph_config::config::parse_domain_config(Path::new("tests/fixtures/domains.toml"))
            .unwrap();
    ingest_fixture_schemas(&engine, &config).await;

    let mox_dir = tempfile::tempdir().unwrap();
    let mox = write_mox_fixture(mox_dir.path(), "model.mox", BODYLESS_OP_MOX);

    let hooks_tmp = tempfile::tempdir().unwrap();
    run_generation(
        &engine,
        Some(mox),
        &domain_types_dir,
        &output_dir,
        hooks_tmp.path(),
    )
    .await;

    let ops_rs = read_mox_ops(&output_dir);

    // The seam: signature-complete stub naming the gap class — the module is
    // the record of what a hand implementation must provide.
    assert!(
        ops_rs.contains("pub fn audit_count(&self, res: &Resource) -> i32 {"),
        "body-less op must still emit its fixed signature:\n{ops_rs}"
    );
    assert!(
        ops_rs.contains("body-less mox op 'auditCount'"),
        "stub must name the body-less op:\n{ops_rs}"
    );
    assert!(
        ops_rs.contains("todo!("),
        "stub body must be a todo! (compiles, never guessed):\n{ops_rs}"
    );

    // Body-less ops never route (issue #508 R2) — the todo! seam stays
    // unreachable from the API plane.
    let router = fs::read_to_string(
        output_dir
            .join("src")
            .join("api")
            .join("recruiting")
            .join("router.rs"),
    )
    .expect("recruiting router must be generated");
    assert!(
        !router.contains("/op/audit_count"),
        "body-less op must not expose a route:\n{router}"
    );
}

/// The doctor half of slice 3: `codegraph doctor --mox-files` names every
/// body-less op (package, class, op) as an unimplemented seam, via the
/// seam-list helper the doctor prints. Pure INFO — never a warning (a
/// body-less op is a legitimate abstract hook, not a misconfiguration).
#[tokio::test]
async fn mox_bodyless_op_doctor_names_the_seam() {
    let sources = vec![("model.mox".to_string(), BODYLESS_OP_MOX.to_string())];
    let compilation = rex_driver::compile_files(
        &sources,
        &rex_driver::DomainImports {
            schemas: rex_driver::SchemaImports::new(),
            sigil: rex_driver::SigilImports::new(),
        },
    );
    let model = compilation.model.expect("fixture mox must compile");
    let seams = codegraph::init::commands::bodyless_op_seams(&model);
    assert_eq!(
        seams,
        vec!["recruiting::CandidateType::auditCount".to_string()],
        "doctor seam list must name the body-less op"
    );

    // A model without body-less ops contributes no seams (the doctor mox
    // matrix stays zero-warning).
    let sources = vec![("model.mox".to_string(), RUST_AND_EXPR_OPS_MOX.to_string())];
    let compilation = rex_driver::compile_files(
        &sources,
        &rex_driver::DomainImports {
            schemas: rex_driver::SchemaImports::new(),
            sigil: rex_driver::SigilImports::new(),
        },
    );
    let model = compilation.model.expect("fixture mox must compile");
    assert!(codegraph::init::commands::bodyless_op_seams(&model).is_empty());
}

// ── Presence gating (the byte-identity contract) ──

/// With NO mox ingested the pipeline is unchanged: no mox_ops.rs anywhere,
/// no Resource placeholder, no derived markers in the DTOs.
#[tokio::test]
async fn without_mox_ops_output_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("out");
    let domain_types_dir = dir.path().join("domain-types");
    fs::create_dir_all(&output_dir).unwrap();

    let engine = GrafeoEngine::in_memory().unwrap();
    let config =
        codegraph_config::config::parse_domain_config(Path::new("tests/fixtures/domains.toml"))
            .unwrap();
    ingest_fixture_schemas(&engine, &config).await;

    let hooks_tmp = tempfile::tempdir().unwrap();
    run_generation(
        &engine,
        None,
        &domain_types_dir,
        &output_dir,
        hooks_tmp.path(),
    )
    .await;

    assert!(
        !output_dir
            .join("src")
            .join("domain")
            .join("recruiting")
            .join("mox_ops.rs")
            .exists(),
        "no mox_ops.rs may be generated without ingested operations"
    );

    let (response, create, update) = read_dtos(&domain_types_dir);
    for (name, content) in [
        ("dto_response.rs", &response),
        ("dto_create.rs", &create),
        ("dto_update.rs", &update),
    ] {
        assert!(
            !content.contains("Computed by the mox domain model"),
            "{name} must not carry computed markers when no mox is ingested"
        );
        assert!(
            !content.contains("pub struct Resource;"),
            "{name} must not carry the Resource placeholder when no mox is ingested"
        );
        assert!(
            content.contains("pub candidate_id"),
            "{name} must keep candidate_id when no mox is ingested"
        );
    }
}
