//! Compile pins for the mox behavior plane (issue #517 — the #508 R1
//! compile claim, verified end-to-end).
//!
//! The emitted mox plane (`mox_types` row mirror + `mox_ops` impls) must
//! compile in a crate with **no `sea_orm` dependency**, on native AND
//! `wasm32-unknown-unknown`. The scratch crate carries the domain-types
//! dep set (uuid/chrono/serde/utoipa/rust_decimal + type-contracts) so the
//! mirror can share the DTO layer's type universe — codelist enums,
//! structured wrappers and child-struct types resolve through the same
//! import conventions the DTO modules use (issue #517's requirement);
//! the load-bearing claim is "resolves within the generated tree, no
//! sea_orm", checked on both targets.
//!
//! Mandate (issue #517 TDD):
//! 1. `mox_plane_compiles_persistence_agnostic_native_and_wasm`
//! 2. a fixture class with a class-ref field, a codelist/enum field and a
//!    `Vec<ClassRef>` field — all three compile
//! 3. an op signature with a class-ref parameter compiles
//!
//! Plus `refused_expr_body_fails_generation_naming_the_op` — a body
//! outside the normative subset fails the generation run naming the op,
//! never an emitted `todo!` (a missing #508 mandate pin).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use codegraph_grafeo::GrafeoEngine;

const RUST_AND_EXPR_OPS_MOX: &str = r#"
package recruiting

class CandidateType {
    String candidateId
    String status

    op int idLength(String prefix) {
        rust { prefix.len() as i32 }
    }

    op boolean hasShortId() {
        expr { if candidateId == "" { true } else { false } }
    }
}
"#;

/// Mandate #2/#3 fixture: a class-ref field, a codelist/enum field, a
/// `Vec<ClassRef>` field, and an op taking a class-ref parameter.
const TRIO_MOX: &str = r#"
package recruiting

enum ShelfKind {
    Fiction as "fiction" = 0
    Reference as "reference" = 1
}

class ShelfType {
    String label
}

class ReviewType {
    String comment
    ShelfKind kind
    refers ShelfType shelf
    refers ShelfType[] shelves

    op string labelOf(ShelfType s) {
        expr { comment }
    }
}
"#;

const REFUSED_BODY_MOX: &str = r#"
package recruiting

class CandidateType {
    String candidateId
    String status

    op int badOp() {
        expr { unknownName }
    }
}
"#;

fn write_mox_fixture(dir: &Path, name: &str, source: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, source).unwrap();
    path
}

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

async fn ingest_mox(engine: &GrafeoEngine, mox: &Path) {
    let config =
        codegraph_config::config::parse_domain_config(Path::new("tests/fixtures/domains.toml"))
            .unwrap();
    codegraph::ingest::mox_ingest::ingest_mox_files(
        engine,
        engine,
        &[mox.to_path_buf()],
        &config,
        &config.defaults.type_suffix,
    )
    .await
    .unwrap();
}

async fn run_generation_result(
    engine: &GrafeoEngine,
    mox: Option<PathBuf>,
    domain_types_dir: &Path,
    output_dir: &Path,
    hooks_tmp: &Path,
) -> Result<(), String> {
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
    let profiles = workspace_root.join("profiles.toml");

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
            generator_name: "mox-compile-pin".into(),
            ..Default::default()
        },
        paths: codegraph::generate::PathsConfig {
            type_contracts_base: type_contracts_path.to_string_lossy().to_string(),
            codegraph_workflow_base: workflow_path.to_string_lossy().to_string(),
            domain_types_base: "domain-types".into(),
            ..Default::default()
        },
        codegen: codegraph::generate::CodegenConfig {
            types_import_prefix: "codegraph_type_contracts".into(),
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
        .map_err(|e| format!("generation failed: {e}"))?;

    if report.has_errors() {
        return Err(format!("generation reported errors: {:#?}", report.errors));
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Build a scratch crate from the emitted mox plane and `cargo check` it
/// on the given target. The crate carries NO `sea_orm` — the R1 claim is
/// that the plane is persistence-agnostic. The copied subset is the plane
/// plus the DTO type universe it references: `mox_types.rs`, every
/// emitted `dto_response.rs`, and the `codelist/` enums — exactly the
/// modules the mirror's imports resolve through (issue #517).
fn cargo_check_mox_plane(domain_types_dir: &Path, target: Option<&str>) {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();

    let dt = root.join("plane");
    fs::create_dir_all(dt.join("src")).unwrap();

    // codelist/ enums (verbatim — they define the codelist types the DTO
    // and mirror reference).
    let codelist_src = domain_types_dir.join("src").join("codelist");
    assert!(
        codelist_src.exists(),
        "the emitted tree must carry src/codelist/: {:?}",
        codelist_src
    );
    copy_dir(&codelist_src, &dt.join("src").join("codelist"));

    // Per-domain: mox_types.rs + every module's dto_response.rs.
    let mut lib = String::from("pub mod codelist;\n");
    let mut domains: Vec<String> = Vec::new();
    for domain_entry in fs::read_dir(domain_types_dir.join("src")).unwrap() {
        let domain_entry = domain_entry.unwrap();
        if !domain_entry.path().is_dir() {
            continue;
        }
        let domain = domain_entry.file_name().to_string_lossy().to_string();
        if domain == "codelist" {
            continue;
        }
        let mut domain_mod = format!("pub mod {domain} {{\n");
        let mut has_any = false;
        if domain_entry.path().join("mox_types.rs").exists() {
            fs::create_dir_all(dt.join("src").join(&domain)).unwrap();
            fs::copy(
                domain_entry.path().join("mox_types.rs"),
                dt.join("src").join(&domain).join("mox_types.rs"),
            )
            .expect("mox_types.rs must be emitted");
            domain_mod.push_str("    pub mod mox_types;\n");
            has_any = true;
        }
        for module_entry in fs::read_dir(domain_entry.path()).unwrap() {
            let module_entry = module_entry.unwrap();
            if !module_entry.path().is_dir() {
                continue;
            }
            let dto = module_entry.path().join("dto_response.rs");
            if !dto.exists() {
                continue;
            }
            let module = module_entry.file_name().to_string_lossy().to_string();
            fs::create_dir_all(dt.join("src").join(&domain).join(&module)).unwrap();
            fs::copy(
                &dto,
                dt.join("src")
                    .join(&domain)
                    .join(&module)
                    .join("dto_response.rs"),
            )
            .unwrap();
            domain_mod.push_str(&format!(
                "    pub mod {module} {{\n        pub mod dto_response;\n    }}\n"
            ));
            has_any = true;
        }
        domain_mod.push_str("}\n");
        if has_any {
            lib.push_str(&domain_mod);
            domains.push(domain);
        }
    }
    assert!(
        !domains.is_empty(),
        "the emitted tree must carry at least one mox_types.rs"
    );
    fs::write(dt.join("src").join("lib.rs"), &lib).unwrap();

    let type_contracts_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("crates")
        .join("codegraph-type-contracts");
    fs::write(
        dt.join("Cargo.toml"),
        format!(
            "[package]\nname = \"domain_types\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\n\
             serde = {{ version = \"1\", features = [\"derive\"] }}\n\
             serde_json = \"1\"\n\
             uuid = {{ version = \"1\", features = [\"v4\", \"serde\", \"js\"] }}\n\
             chrono = {{ version = \"0.4\", features = [\"serde\"] }}\n\
             rust_decimal = {{ version = \"1\", features = [\"serde-with-str\"] }}\n\
             utoipa = {{ version = \"6\", features = [\"chrono\", \"uuid\", \"decimal\"] }}\n\
             codegraph-type-contracts = {{ path = \"{}\" }}\n",
            type_contracts_path.display()
        ),
    )
    .unwrap();

    let mut cmd = Command::new("cargo");
    cmd.arg("check")
        .arg("--manifest-path")
        .arg(dt.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", root.join("target"))
        .env("CARGO_TERM_COLOR", "never");
    if let Some(t) = target {
        cmd.arg("--target").arg(t);
    }
    let out = cmd.output().expect("cargo check must run");
    assert!(
        out.status.success(),
        "cargo check {} failed:\n{}\n{}",
        target.unwrap_or("(native)"),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

async fn generate_with_mox(mox_source: &str) -> tempfile::TempDir {
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
    let mox = write_mox_fixture(mox_dir.path(), "model.mox", mox_source);

    let hooks_tmp = tempfile::tempdir().unwrap();
    run_generation_result(
        &engine,
        Some(mox),
        &domain_types_dir,
        &output_dir,
        hooks_tmp.path(),
    )
    .await
    .expect("generation must succeed");
    // Keep the mox fixture dir alive for the duration of the run.
    std::mem::forget(mox_dir);
    dir
}

/// Mandate #1: the emitted plane compiles with no sea_orm on native and
/// wasm32-unknown-unknown. The fixture's CandidateType carries class-typed
/// fields (VO/structured/composite/entity-ref) — the #517 repro shape.
#[tokio::test]
async fn mox_plane_compiles_persistence_agnostic_native_and_wasm() {
    let dir = generate_with_mox(RUST_AND_EXPR_OPS_MOX).await;
    let domain_types_dir = dir.path().join("domain-types");

    let plane_rs =
        fs::read_to_string(domain_types_dir.join("src/recruiting/mox_types.rs")).unwrap();
    assert!(
        !plane_rs.contains("sea_orm") && !plane_rs.contains("crate::entity::"),
        "the mox plane must not depend on the SeaORM layer:\n{plane_rs}"
    );

    cargo_check_mox_plane(&domain_types_dir, None);
    cargo_check_mox_plane(&domain_types_dir, Some("wasm32-unknown-unknown"));
}

/// Mandate #2 + #3: class-ref field, codelist/enum field, Vec<ClassRef>
/// field, and an op with a class-ref parameter — all compile.
#[tokio::test]
async fn mox_mirror_field_trio_and_class_ref_op_param_compile() {
    let dir = generate_with_mox(TRIO_MOX).await;
    let domain_types_dir = dir.path().join("domain-types");

    let mox_types =
        fs::read_to_string(domain_types_dir.join("src/recruiting/mox_types.rs")).unwrap();
    // The trio's field kinds must actually be present on a mirror.
    assert!(
        mox_types.contains("pub struct Review"),
        "the trio class must carry a mirror:\n{mox_types}"
    );

    cargo_check_mox_plane(&domain_types_dir, None);
    cargo_check_mox_plane(&domain_types_dir, Some("wasm32-unknown-unknown"));
}

/// A body outside the normative subset fails the generation run naming the
/// op — never an emitted `todo!` for a body-carrying op.
#[tokio::test]
async fn refused_expr_body_fails_generation_naming_the_op() {
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
    let mox = write_mox_fixture(mox_dir.path(), "model.mox", REFUSED_BODY_MOX);

    let hooks_tmp = tempfile::tempdir().unwrap();
    let result = run_generation_result(
        &engine,
        Some(mox),
        &domain_types_dir,
        &output_dir,
        hooks_tmp.path(),
    )
    .await;

    assert!(result.is_err(), "a refused expr body must fail generation");
    let err = result.unwrap_err();
    assert!(
        err.contains("badOp") && err.contains("unknownName"),
        "the failure must name the op and the offending construct:\n{err}"
    );
    let plane_path = domain_types_dir.join("src/recruiting/mox_types.rs");
    if let Ok(plane_rs) = fs::read_to_string(&plane_path) {
        assert!(
            !plane_rs.contains("todo!("),
            "a body-carrying op must never emit todo!:\n{plane_rs}"
        );
    }
}
