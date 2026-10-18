//! Flywheel corpus tests (issue #352, deliverable E1).
//!
//! `tests/fixtures/flywheel/` is the real Flywheel consumer model — 11
//! `.rosetta` files (~1444 lines, ~156 top-level constructs across the
//! `flywheel.<domain>` namespaces) recreated from the Flywheel Supabase
//! schema and promoted into the committed fixture corpus as the standard
//! heavy rosetta fixture. It exercises the construct shapes that caught
//! five real generator bugs in one consumer session (commit c0cab515):
//! cross-namespace wildcard imports, self-referencing FKs
//! (`Comment.parentComment`), choices (`ProposedBudget`,
//! `BusinessServiceProvider`), named conditions, ~30 enums, custom
//! `typeAlias`es, and number aliases with `digits`/`fractionalDigits`.
//!
//! Three tests:
//!
//! - [`flywheel_smoke_generates_common_domain`] — always on. Single-file,
//!   plan-less run over `model/common.rosetta` (the base namespace: it has
//!   no imports of other flywheel namespaces). Catches gross
//!   bridge/generator breakage on every PR. Measured ~95 s in debug — the
//!   rosetta bridge's per-GQL-statement grafeo overhead dominates (the
//!   #350 D5 perf target); in release it is seconds.
//! - [`flywheel_corpus_generates_full_artifact_set`] — `#[ignore]`d heavy
//!   gate: full `driver::run` over all 11 files + the fixture
//!   `domains.toml` + the consumer's `profiles.toml`
//!   (`rosetta_backend = true`). Heavy because debug-mode classification
//!   scores every one of the ~156 constructs; run explicitly:
//!   `cargo test -p codegraph --test flywheel_corpus_tests -- --ignored`.
//!   Wired into the consumer-e2e CI job by #353.
//! - [`flywheel_corpus_generated_crate_checks_clean`] — `#[ignore]`d
//!   compile gate modeled on `grafeo_e2e_tests/compile_gate.rs`: generate
//!   into a scratch dir under `target/` (same mount as the workspace's
//!   cargo cache) with path deps into this checkout, then
//!   `cargo check --manifest-path`. Pins the corpus compiling clean —
//!   failures (the A6 skeleton class #446, the A2 ordering/registry class
//!   #334/#333) are reported rather than papered over.

use std::path::{Path, PathBuf};
use std::time::Instant;

use codegraph::driver;
use codegraph_backend::{BackendConfig, create_backend};
use codegraph_config::config::{DomainConfig, parse_domain_config};

const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/flywheel");

/// Model files in the consumer's dependency order (base namespace first),
/// matching the flywheel justfile's `--rosetta-files` sequence.
const MODEL_NAMES: &[&str] = &[
    "common",
    "reference",
    "profiles",
    "accounts",
    "company",
    "taxonomy",
    "qanda",
    "jobs",
    "marketplace",
    "inbox",
    "pricing",
];

fn fixture_path(rel: &str) -> PathBuf {
    Path::new(FIXTURE_DIR).join(rel)
}

fn model_file(name: &str) -> PathBuf {
    fixture_path("model").join(format!("{name}.rosetta"))
}

fn all_model_files() -> Vec<PathBuf> {
    MODEL_NAMES.iter().map(|n| model_file(n)).collect()
}

fn fixture_domains() -> DomainConfig {
    parse_domain_config(&fixture_path("domains.toml")).unwrap()
}

/// Relative (slash-normalized) paths of every generated file under `output`.
fn generated_files(output: &Path) -> Vec<String> {
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

fn has_file(files: &[String], suffix: &str) -> bool {
    files.iter().any(|f| f.ends_with(suffix))
}

fn read_output_file(output: &Path, rel: &str) -> String {
    std::fs::read_to_string(output.join(rel))
        .unwrap_or_else(|e| panic!("expected generated file {rel} to be readable: {e}"))
}

fn run_args<'a>(
    config_path: &'a Path,
    rosetta_files: &'a [PathBuf],
    output: &'a Path,
    profiles_config_path: Option<PathBuf>,
) -> driver::RunArgs<'a> {
    driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path,
        output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path,
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &[],
        rosetta_files,
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

/// The common namespace is the smoke test's whole input: it must not import
/// any other flywheel namespace, otherwise a single-file run would resolve
/// dangling references.
#[test]
fn common_rosetta_is_import_free_for_the_smoke_run() {
    let source = std::fs::read_to_string(model_file("common")).unwrap();
    assert!(
        !source
            .lines()
            .any(|l| l.trim_start().starts_with("import ")),
        "common.rosetta gained an import — the single-file smoke test \
         (flywheel_smoke_generates_common_domain) needs a revisit"
    );
}

/// Always-on smoke: the base namespace alone must bridge and drive the core
/// (plan-less) generator set. `common.rosetta` carries the three construct
/// families the corpus leans on — typeAliases (Money/Uuid/...), range VOs
/// with named conditions (IntRange.LowerBeforeUpper) and all ~30 enums — so
/// a bridge or data-plane generator regression trips here on every PR.
#[tokio::test]
async fn flywheel_smoke_generates_common_domain() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("generated");
    let rosetta = vec![model_file("common")];

    let started = Instant::now();
    driver::run(run_args(
        &fixture_path("domains.toml"),
        &rosetta,
        &output,
        // Absent → plan-less: the core generator set without profile-gated
        // families (condition_validations & co. need `rosetta_backend`).
        Some(PathBuf::from("definitely-absent-profiles.toml")),
    ))
    .await
    .unwrap();
    eprintln!(
        "flywheel smoke (common only) generated in {:?}",
        started.elapsed()
    );

    let files = generated_files(&output);

    let migrations: Vec<&String> = files
        .iter()
        .filter(|f| f.starts_with("migrations/") && f.ends_with(".sql"))
        .collect();
    assert!(
        !migrations.is_empty(),
        "smoke run must produce migrations; files: {files:?}"
    );

    // Representative artifact shapes from the common namespace. Migration
    // files carry sequence-number prefixes — match on suffixes.
    for entity in [
        "src/entity/common_rgb_color.rs",   // VO type (PostGIS point style)
        "src/entity/common_int_range.rs",   // VO with a named condition
        "src/entity/common_money_range.rs", // VO over the Money typeAlias
        "src/entity/common_work_type_enum.rs", // enum → codelist entity
    ] {
        assert!(
            has_file(&files, entity),
            "expected {entity}; files: {files:?}"
        );
    }

    let ddl_path = files
        .iter()
        .find(|f| f.starts_with("migrations/") && f.ends_with("_common_rgb_color.sql"))
        .unwrap_or_else(|| panic!("rgb_color DDL migration missing; files: {files:?}"));
    let ddl = read_output_file(&output, ddl_path);
    assert!(
        ddl.contains("CREATE TABLE IF NOT EXISTS common.rgb_color"),
        "rgb_color DDL missing:\n{ddl}"
    );

    let seed_path = files
        .iter()
        .find(|f| f.ends_with("_common_work_type_enum_codelist.sql"))
        .unwrap_or_else(|| panic!("work_type_enum codelist seed missing; files: {files:?}"));
    let seed = read_output_file(&output, seed_path);
    assert!(
        seed.contains("INSERT INTO common.work_type_enum"),
        "codelist seed for WorkTypeEnum missing:\n{seed}"
    );
    assert!(
        seed.contains("full_time"),
        "codelist seed must carry displayName-normalized codes:\n{seed}"
    );

    // Only the ingested namespace may produce artifacts.
    assert!(
        !has_file(&files, "src/entity/jobs_job_post.rs"),
        "single-file smoke run leaked jobs-domain artifacts: {files:?}"
    );
}

/// Heavy gate: the full corpus through the real driver path with the
/// consumer's profile (`rosetta_backend = true`, so condition validations /
/// functions / rules generators run).
///
/// Ignored because debug-mode classification scores all ~156 constructs
/// (~minutes); run explicitly:
///
/// ```text
/// cargo test -p codegraph --test flywheel_corpus_tests -- --ignored
/// ```
///
/// Wired into the consumer-e2e CI job by #353, which is where the wall-clock
/// number for #350 (D5 perf) is tracked. Measured baseline (debug, 2026-10-04):
/// ~30 min total (1788 s warm, 1883 s under parallel build load) — the
/// rosetta bridge dominates (~90 s for `common.rosetta` alone;
/// per-GQL-statement grafeo overhead in debug builds), generation of the
/// 6238-file artifact set is minutes on top.
///
/// Note: `driver::run` returns `Ok` even when individual generators error
/// (they are reported on stdout and skip their artifacts), so the structural
/// artifact assertions below carry the "0 errors" weight.
#[tokio::test]
#[ignore = "heavy: debug-mode classify over ~156 constructs; run via `cargo test -p codegraph --test flywheel_corpus_tests -- --ignored` (consumer-e2e CI per #353)"]
async fn flywheel_corpus_generates_full_artifact_set() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("generated");
    let rosetta = all_model_files();

    let started = Instant::now();
    driver::run(run_args(
        &fixture_path("domains.toml"),
        &rosetta,
        &output,
        Some(fixture_path("profiles.toml")),
    ))
    .await
    .unwrap();
    let elapsed = started.elapsed();
    eprintln!(
        "flywheel corpus (11 files) full generation took {elapsed:?} in debug mode \
         (#350 D5 perf baseline)"
    );

    let files = generated_files(&output);

    let migrations: Vec<&String> = files
        .iter()
        .filter(|f| f.starts_with("migrations/") && f.ends_with(".sql"))
        .collect();
    assert!(
        !migrations.is_empty(),
        "corpus run must produce migrations; got {} files total",
        files.len()
    );

    // Entity files for representative titles across the domains (config
    // force list pins these 1:1 with the original postgres tables).
    for entity in [
        "src/entity/common_rgb_color.rs",
        "src/entity/reference_iso_country.rs",
        "src/entity/accounts_account.rs",
        "src/entity/company_company.rs",
        "src/entity/qanda_form.rs",
        "src/entity/jobs_job_post.rs",
        "src/entity/marketplace_auction.rs",
        "src/entity/pricing_invoice.rs",
        "src/entity/inbox_message.rs",
    ] {
        assert!(has_file(&files, entity), "expected {entity}");
    }

    // Domain module dirs for the same entities.
    for module in [
        "src/domain/company/company/",
        "src/domain/jobs/job_post/",
        "src/domain/marketplace/auction/",
    ] {
        assert!(
            files.iter().any(|f| f.starts_with(module)),
            "expected domain module {module}"
        );
    }

    // Choices keep their unstripped names and are separately addressable
    // (gap-doc defect #9 semantics, pinned for the corpus union shapes).
    for choice in [
        "src/entity/jobs_proposed_budget.rs",
        "src/entity/jobs_business_service_provider.rs",
    ] {
        assert!(
            has_file(&files, choice),
            "expected choice artifact {choice}"
        );
    }

    // Self-referencing FK: comments.thread parent → comments (the construct
    // that caught the duplicate/self-FK bug class in c0cab515).
    let comment_ddl_path = files
        .iter()
        .find(|f| f.starts_with("migrations/") && f.ends_with("_jobs_comment.sql"))
        .unwrap_or_else(|| panic!("comment DDL migration missing; files: {files:?}"));
    let comment_ddl = read_output_file(&output, comment_ddl_path);
    assert!(
        comment_ddl.contains("parent_comment_id") && comment_ddl.contains("REFERENCES"),
        "self-referencing FK parent_comment_id missing from comment DDL:\n{comment_ddl}"
    );

    // Named conditions → constraint-plane validations.rs under the domains
    // that model them. jobs.ApplicantIdentified (if/then/else over `exists`)
    // is inside the #262 slice-1 transpiler subset and emits a real function;
    // company.ExactlyOneAuthor (`required choice`) is a documented stub
    // (choice-variant representation deferred per #262) — pin BOTH shapes.
    let jobs_validations = read_output_file(&output, "src/domain/jobs/validations.rs");
    assert!(
        jobs_validations.contains("applicant_identified"),
        "ApplicantIdentified condition missing from jobs validations.rs:\n{jobs_validations}"
    );
    let company_validations = read_output_file(&output, "src/domain/company/validations.rs");
    assert!(
        company_validations.contains("ExactlyOneAuthor"),
        "ExactlyOneAuthor condition missing from company validations.rs:\n{company_validations}"
    );
    assert!(
        company_validations.contains("TODO(#262)"),
        "choice-kind conditions must carry the documented #262 stub marker:\n{company_validations}"
    );

    // Codelist seeds route through common (all enums live there); migration
    // files carry sequence-number prefixes.
    let codelist_seeds: Vec<&String> = files
        .iter()
        .filter(|f| {
            f.starts_with("migrations/") && f.contains("_common_") && f.ends_with("_codelist.sql")
        })
        .collect();
    assert!(
        codelist_seeds.len() >= 25,
        "expected the ~30 common enums as codelist seeds, got {}: {codelist_seeds:?}",
        codelist_seeds.len()
    );
    let seed_path = files
        .iter()
        .find(|f| f.ends_with("_common_pricing_model_type_enum_codelist.sql"))
        .unwrap_or_else(|| panic!("pricing_model_type_enum codelist seed missing"));
    let seed = read_output_file(&output, seed_path);
    assert!(
        seed.contains("INSERT INTO common.pricing_model_type_enum"),
        "codelist seed for the 20-value PricingModelTypeEnum missing:\n{seed}"
    );
}

/// Compile gate (modeled on `grafeo_e2e_tests/compile_gate.rs`): generate
/// the full flywheel app into a scratch dir under `target/` and
/// `cargo check` it with path deps into this checkout — no network, cargo
/// artifacts stay on the same mount.
///
/// This gate was RED while the A6 skeleton bug (#446) was open: generation
/// was silent (0 errors / 0 warnings) but 53 of 136 emitted entities were
/// audit-only skeletons — the DDL and `sea_orm_entity` families resolved
/// ZERO model properties for them while the dto/repository/command/query
/// families resolved all of them, and cargo check failed with 805 errors in
/// one missing-column triad (E0560×161 / E0609×483 / E0599×161; first
/// errors `accounts_account.primary_owner_user_id`,
/// `accounts_account_invitee.account_id`/`professional_profile_id`).
///
/// Root cause (fixed): the composition tree demoted every scalar property
/// classified `EntityReference` to the jsonb plane whenever the TARGET
/// schema's `is_entity` flag was false — and the rosetta bridge records a
/// VO-shaped default (`is_entity = false`) for every schema while its
/// properties still classify model-typed attributes as entity references,
/// so the bridge-only gate graph (no classification pass) demoted them all.
/// DDL/entity resolve through the tree; dto trusts the property
/// classification — hence the divergence. The seam now keeps scalar
/// entity-reference columns in the tree (see
/// `codegraph-grafeo/src/querier/composition.rs`), and a debug-only
/// ddl/dto gap advisory (`ddl_dto_gap_fields`) warns if the planes ever
/// diverge again. The gate is GREEN as of #446: `cargo check` exits clean.
///
/// Ignored because a cold `cargo check` of the generated app compiles the
/// whole dependency tree (sea-orm, axum, utoipa, …); run explicitly:
///
/// ```text
/// cargo test -p codegraph --test flywheel_corpus_tests -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "slow: cold cargo check of the full generated app; run via `cargo test -p codegraph --test flywheel_corpus_tests -- --ignored --nocapture`"]
async fn flywheel_corpus_generated_crate_checks_clean() {
    use std::time::Duration;

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let target_root = workspace_root.join("target");
    let out = target_root.join(format!(
        "flywheel-corpus-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&out).unwrap();

    // Bridge the corpus into a fresh in-memory graph.
    let backend = create_backend(&BackendConfig::default()).await.unwrap();
    let domain_config = fixture_domains();
    let outcome = codegraph::ingest::rosetta_ingest::ingest_rosetta_files(
        backend.ingestor(),
        backend.querier(),
        &all_model_files(),
        &domain_config,
        &domain_config.defaults.type_suffix,
    )
    .await
    .unwrap();
    assert_eq!(outcome.stats.files, MODEL_NAMES.len(), "bridged file count");

    // Build the plan from the fixture profiles, minus the gRPC generators
    // (they need protoc and have their own compile coverage).
    let registry = codegraph::profile::CapabilityRegistry::new();
    let mut resolved = codegraph::profile::load_and_resolve_profile(
        &fixture_path("profiles.toml"),
        "default",
        None,
    )
    .unwrap();
    for section in resolved.sections.values_mut() {
        section.generators.retain(|g| !g.starts_with("grpc_"));
    }
    let plan = codegraph::profile::BuildPlan::from_profile(&resolved, &registry).unwrap();

    // Path deps into this checkout so `cargo check` never touches the
    // network; the domain-types crate lives inside the app output (the
    // consumer shape) so its manifest path dep resolves.
    let type_contracts_path = workspace_root.join("crates/codegraph-type-contracts");
    let workflow_path = workspace_root.join("crates/codegraph-workflow");
    let mut project_config = codegraph::generate::ProjectConfig {
        identity: codegraph::generate::IdentityConfig {
            app_name: "flywheel".into(),
            domain_types_crate: "app_domain_types".into(),
            generator_name: "flywheel-corpus-gate".into(),
            ..Default::default()
        },
        paths: codegraph::generate::PathsConfig {
            type_contracts_base: type_contracts_path.to_string_lossy().to_string(),
            codegraph_workflow_base: workflow_path.to_string_lossy().to_string(),
            domain_types_base: "crates/domain-types".into(),
            ..Default::default()
        },
        codegen: codegraph::generate::CodegenConfig {
            types_import_prefix: domain_config.defaults.types_import_prefix.clone(),
            ..Default::default()
        },
        cargo: codegraph::generate::CargoConfig {
            extra_dependencies: format!(
                "codegraph-workflow = {{ path = \"{}\" }}\n\
                 codegraph-type-contracts = {{ path = \"{}\" }}",
                workflow_path.display(),
                type_contracts_path.display(),
            ),
            ..Default::default()
        },
        ..Default::default()
    };

    let tera = codegraph::generate::template_engine::create_tera(Path::new("unused")).unwrap();
    // Resolve the ux plane exactly like the driver: the fixture profile
    // enables `ux_rules`, so the built-in pack feeds GeneratorOpts and
    // ProjectConfig.ux.
    let ux_resolved = if plan.ux_rules {
        Some(codegraph_config::builtin_ux_rules().unwrap().rules)
    } else {
        None
    };
    project_config.ux.ux = ux_resolved.clone();
    let domain_types_dir = out.join("crates/domain-types");
    let hooks_tmp = tempfile::tempdir().unwrap();
    let report =
        codegraph::generate::run_generators_with_opts(codegraph::generate::GeneratorOpts {
            db: backend.querier(),
            config: &domain_config,
            output_dir: &out,
            tera: &tera,
            ui_overrides: &codegraph_config::UiOverrideConfig::default(),
            ui_domains: &codegraph_config::UiDomainConfig::default(),
            schema_base_dir: Path::new(""),
            seed_config: None,
            domain_types_base: Some(&domain_types_dir),
            hooks_base: Some(hooks_tmp.path()),
            ext_points: None,
            build_plan: Some(&plan),
            ifml_frameworks: vec![],
            ifml_components: None,
            ux_rules: ux_resolved,
            project_config: Some(&project_config),
            emdash_plugins: None,
            domain_config_dir: None,
        })
        .await
        .unwrap();

    if report.has_errors() {
        let detail = report
            .errors
            .iter()
            .map(|e| format!("{}/{}: {}", e.entity, e.generator, e.source))
            .take(10)
            .collect::<Vec<_>>()
            .join("\n");
        panic!(
            "corpus generation reported errors (kept at {}):\n{detail}",
            out.display()
        );
    }

    let cargo_toml = out.join("Cargo.toml");
    assert!(cargo_toml.exists(), "scaffold must emit the app manifest");

    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(900),
        tokio::process::Command::new("cargo")
            .args(["check", "--manifest-path"])
            .arg(&cargo_toml)
            .output(),
    )
    .await;

    let check = match result {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => panic!(
            "failed to spawn cargo check: {e} (output kept at {})",
            out.display()
        ),
        Err(_) => panic!(
            "cargo check timed out after 900s (output kept at {})",
            out.display()
        ),
    };
    eprintln!(
        "flywheel corpus cargo check took {:?} (kept at {})",
        started.elapsed(),
        out.display()
    );

    if !check.status.success() {
        let stderr = String::from_utf8_lossy(&check.stderr);
        panic!(
            "generated flywheel corpus app failed cargo check (kept at {}):\n{}",
            out.display(),
            summarize_rustc_errors(&stderr)
        );
    }

    // Success: sweep the scratch dir (best effort — it may hold a warmed
    // cargo cache the next run reuses).
    let _ = std::fs::remove_dir_all(&out);
}

/// Group rustc errors by error code for a readable failure digest: distinct
/// codes with occurrence counts plus the first few error headlines.
fn summarize_rustc_errors(stderr: &str) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut headlines: Vec<String> = Vec::new();
    for line in stderr.lines() {
        let trimmed = line.trim_start();
        let is_error = trimmed.starts_with("error");
        if let Some(code) = trimmed
            .strip_prefix("error")
            .and_then(|rest| rest.trim_start().strip_prefix('['))
            .and_then(|rest| rest.split(']').next())
            .map(str::to_string)
        {
            match counts.iter_mut().find(|(c, _)| *c == code) {
                Some((_, n)) => *n += 1,
                None => counts.push((code, 1)),
            }
        }
        if is_error && headlines.len() < 10 {
            headlines.push(trimmed.to_string());
        }
    }
    if counts.is_empty() {
        // Not rustc-code shaped (resolution failure, manifest error, …):
        // return a trimmed tail for context.
        let tail: Vec<&str> = stderr.lines().rev().take(40).collect();
        return tail.into_iter().rev().collect::<Vec<_>>().join("\n");
    }
    let grouped = counts
        .iter()
        .map(|(code, n)| format!("  {code}: {n} occurrence(s)"))
        .collect::<Vec<_>>()
        .join("\n");
    let first = headlines.join("\n");
    format!("distinct error codes:\n{grouped}\n\nfirst error headlines:\n{first}")
}
