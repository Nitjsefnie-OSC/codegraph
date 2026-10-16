//! Workers-topology suite (port of hr-platform `dual-test.sh` Test B):
//! regenerate the `workers-cornucopia` profile (one Cloudflare Worker crate
//! per domain + a gateway) → reset/migrate/seed the Supabase database → build
//! the worker workspace → boot per-domain workers + gateway → gateway smoke +
//! hurl contract tests through the gateway → teardown.
//!
//! Deviations from the bash original: missing worker binaries fail the run
//! instead of being skipped silently, the gateway port and the worker port
//! range are preflight-checked, and every hurl skip carries its reason.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::OpsConfig;
use crate::db::{
    psql_exec, psql_exec_file, psql_exec_file_ok, psql_exec_file_with_vars, psql_query,
};
use crate::error::{OpsError, OpsResult};
use crate::ext::{HookPolicy, run_hooks};
use crate::output;
use crate::proc::{ManagedProcess, Supervisor};
use crate::suites::api::{
    TestCounters, http_get_body, http_post_body, http_status, hurl_error_excerpt, hurl_log_path,
    hurl_suite_passed, parse_requests, pluralize_entity_route, provision_api_key, write_hurl_log,
};
use crate::wait::wait_for_url;

/// Regeneration profile for the workers topology. Suite constant — the
/// manifest `profile` stays on the monolith suites' choice.
const PROFILE: &str = "workers-cornucopia";

/// Generated workers output directory (relative to `root_dir`).
const OUTPUT_DIR: &str = "generated-workers";

/// Cargo workspace inside the generated output producing the binaries.
const WORKERS_SUBDIR: &str = "workers";

/// Gateway listen port (mirrors dual-test.sh). Also swept by `clean` — the
/// workers-suite generalization is deferred, so `clean` shares these
/// constants rather than manifest plumbing.
pub const GATEWAY_PORT: u16 = 8787;

/// First per-domain worker port; worker *i* binds `3001 + i`.
pub const WORKER_BASE_PORT: u16 = 3001;

/// Generated binary prefix: `hr-app-{domain}` per worker, `hr-app-gateway`.
const BINARY_PREFIX: &str = "hr-app";

/// Domain worker crates in generation (port) order.
pub const WORKER_DOMAINS: &[&str] = &[
    "assessments",
    "benefits",
    "common",
    "compensation",
    "compliance",
    "interviewing",
    "payroll",
    "recruiting",
    "screening",
    "timecard",
    "wellness",
];

/// Infrastructure schemas created before migrations on plain Postgres (the
/// Supabase stack normally provides `api_keys_private` via basejump setup).
const INFRA_SCHEMAS: &[&str] = &["platform", "api_keys_private"];

/// hurl files skipped by default on the workers topology, with the reason
/// logged for each. RLS isolation and webhooks are covered by the monolith
/// suites; compliance routes are hr-extensions monolith-only (verified
/// failing through the workers gateway).
const DEFAULT_HURL_SKIPS: &[(&str, &str)] = &[
    (
        "08_rls_isolation.hurl",
        "RLS isolation runs via the monolith api suite",
    ),
    (
        "12_webhook_extensions.hurl",
        "webhooks run via the monolith suites",
    ),
    (
        "10_compliance_rules.hurl",
        "compliance routes are hr-extensions monolith-only",
    ),
    (
        "11_compliance_check.hurl",
        "compliance routes are hr-extensions monolith-only",
    ),
    (
        "12b_ird_sync_review.hurl",
        "ird suite runs via the monolith api suite (ird mock sidecar + api-key harness vars)",
    ),
    (
        "13_ird_rls_isolation.hurl",
        "ird suite runs via the monolith api suite (ird mock sidecar + api-key harness vars)",
    ),
    (
        "14_ird_connection_lifecycle.hurl",
        "ird suite runs via the monolith api suite (ird mock sidecar + api-key harness vars)",
    ),
    (
        "15_ird_employment_sync.hurl",
        "ird suite runs via the monolith api suite (ird mock sidecar + api-key harness vars)",
    ),
    (
        "16_ird_ei_filing.hurl",
        "ird suite runs via the monolith api suite (ird mock sidecar + api-key harness vars)",
    ),
    (
        "17_ird_events_inbox.hurl",
        "ird suite runs via the monolith api suite (ird mock sidecar + api-key harness vars)",
    ),
];

/// Generated output entries wiped before regeneration (mirrors dual-test.sh
/// B1). The manifest's `clean-generated` pre_generate hook only targets the
/// monolith `generated-candidate/` paths, so the workers output is wiped here.
const REGEN_WIPE: &[&str] = &[
    "src",
    "workers",
    "migrations",
    "queries",
    "cornucopia-queries",
    "cli",
    "ui",
    "Cargo.toml",
];

/// Gateway readiness wait (seconds).
const GATEWAY_WAIT_SECS: u64 = 30;

/// Default test orgs (mirror the manifest hurl org ids / api-seed.sql).
const DEFAULT_ORG_A: &str = "00000000-0000-0000-0000-000000000001";
const DEFAULT_ORG_B: &str = "00000000-0000-0000-0000-000000000002";

/// Arguments for the workers suite.
#[derive(Debug, Clone)]
pub struct WorkersArgs {
    pub keep: bool,
    pub skip_generate: bool,
    pub release: bool,
    /// Write a machine-readable `--results` JSON report to this path.
    pub results_file: Option<String>,
    /// Warn instead of failing when the generator rev in the workers output's
    /// `.codegraph-manifest.json` differs from this testkit's pinned rev.
    pub allow_gen_rev_mismatch: bool,
    /// Reuse registry-known services already running on needed ports instead
    /// of taking them over.
    pub reuse: bool,
}

/// Port for the worker at 0-based index `i`: `3001 + i`.
pub fn worker_port(i: u16) -> u16 {
    WORKER_BASE_PORT + i
}

/// Gateway upstream env var name for a domain, using the same `-` → `_` +
/// uppercase mapping the generated gateway applies to route bindings.
fn gateway_env_name(domain: &str) -> String {
    format!(
        "GATEWAY_UPSTREAM_{}",
        domain.replace('-', "_").to_ascii_uppercase()
    )
}

/// Skip reason for a hurl file on the workers topology: `Some(reason)` when
/// the file must not run. Default skips carry their specific explanation;
/// other manifest `hurl.skip` entries fall back to the generic reason.
fn skip_reason(name: &str, manifest_skip: &[String]) -> Option<String> {
    if let Some((_, reason)) = DEFAULT_HURL_SKIPS.iter().find(|(f, _)| *f == name) {
        return Some((*reason).to_string());
    }
    if manifest_skip.iter().any(|s| s == name) {
        return Some("manifest hurl.skip".to_string());
    }
    None
}

/// Run the workers-topology suite. Returns Err(TestFailure) if any check
/// failed.
pub async fn run_workers(config: &OpsConfig, args: &WorkersArgs) -> OpsResult<()> {
    run_workers_inner(config, args).await
}

async fn run_workers_inner(config: &OpsConfig, args: &WorkersArgs) -> OpsResult<()> {
    let mut counters = TestCounters::new();
    let workers_out = config.root_dir.join(OUTPUT_DIR);

    // ---- 0. Supabase stack ----
    // The workers topology targets the same Supabase-provisioned database as
    // the api and e2e suites ([database.api]); all migrations, including the
    // basejump/pgmq platform files, apply against it.
    output::section("0. Supabase stack");
    config.metrics.begin("Supabase");
    match config.supabase_dir.as_ref() {
        Some(dir) => crate::suites::e2e::supabase_ensure_up(config, dir).await?,
        None => {
            return Err(OpsError::TestFailure(
                "[supabase] dir missing in manifest — the workers suite needs the stack".into(),
            ));
        }
    }
    config.metrics.end();
    run_hooks(config, "pre_db", HookPolicy::PerHook).await?;

    // ---- 1. Preflight ----
    output::section("1. Preflight");
    config.metrics.begin("Preflight");

    match psql_query(&config.api_db, "SELECT 1").await {
        Ok(_) => counters.pass("Postgres running"),
        Err(_) => {
            counters.fail_test("Postgres not reachable");
            return Err(OpsError::TestFailure(
                "preflight failed: Postgres not reachable".into(),
            ));
        }
    }

    let has_hurl = Command::new("hurl")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if has_hurl {
        counters.pass("hurl installed");
    } else {
        counters.fail_test("hurl not found");
        return Err(OpsError::MissingTool(
            "hurl",
            "install hurl or set manifest.hurl = none",
        ));
    }

    // Registry-aware preflight: a leaked `--keep` worker/gateway is taken
    // over by default (killed via its registry pid) or reused with --reuse.
    let mut reused_ports: HashSet<u16> = HashSet::new();
    for port in std::iter::once(GATEWAY_PORT).chain(worker_ports()) {
        match crate::preflight::ensure_port_available(&config.root_dir, port, args.reuse) {
            Ok(outcome) => {
                if outcome.reused() {
                    reused_ports.insert(port);
                }
            }
            Err(e) => {
                counters.fail_test(e.to_string());
                return Err(e);
            }
        }
    }
    if reused_ports.is_empty() {
        counters.pass(format!(
            "Ports free (gateway {GATEWAY_PORT}, workers {WORKER_BASE_PORT}..{})",
            worker_port(WORKER_DOMAINS.len() as u16 - 1)
        ));
    } else {
        counters.pass(format!(
            "Ports resolved ({} reused via --reuse)",
            reused_ports.len()
        ));
    }
    config.metrics.end();

    // ---- 2. Regenerate (workers profile) ----
    output::section("2. Regenerate (workers profile)");
    config.metrics.begin("Regenerate (workers)");
    if args.skip_generate {
        output::info("Generation skipped (--skip-generate)");
        config.metrics.skip("--skip-generate");
    } else {
        let Some(graph) = config.manifest.graph_binary.as_deref() else {
            return Err(OpsError::Config(
                "workers suite requires manifest.graph_binary (or run with --skip-generate)"
                    .to_string(),
            ));
        };
        run_hooks(config, "pre_generate", HookPolicy::PerHook).await?;
        // The manifest's clean-generated hook targets generated-candidate
        // paths, so wipe the workers output here (dual-test.sh B1).
        for rel in REGEN_WIPE {
            let path = workers_out.join(rel);
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else if path.is_file() {
                std::fs::remove_file(&path)?;
            }
        }
        output::info(format!(
            "Wiped stale workers output under {}",
            workers_out.display()
        ));
        let (ok, output_text) = regenerate(config, graph, &workers_out);
        if !ok {
            counters.fail_test("Regeneration failed");
            for line in output_text.lines().filter(|l| l.contains("error")).take(10) {
                println!("    {line}");
            }
            return Err(OpsError::TestFailure(
                "workers regeneration failed (see errors above)".into(),
            ));
        }
        counters.pass("Workers output regenerated");
        // #357: the freshly written manifest names the generator rev — a
        // stale graph binary is a hard error before the build/DB stages.
        if let Err(e) =
            crate::freshness::check_generator_rev(&workers_out, args.allow_gen_rev_mismatch)
        {
            counters.fail_test(e.to_string());
            return Err(e);
        }
        // post_generate hooks may be monolith-specific (the hr-specs repin
        // hook only touches generated-candidate) — warn, never abort
        // (WarnOnly: `fatal = true` cannot escalate past this).
        if let Err(e) = run_hooks(config, "post_generate", HookPolicy::WarnOnly).await {
            output::warn(format!("post_generate hook failed (continuing): {e}"));
        }
        config.metrics.end();
    }

    // ---- 3. Database (plain Postgres) ----
    output::section("3. Database");
    config.metrics.begin("DB reset + migrate + seed");

    if let Some(reset) = &config.manifest.database.api.reset_sql {
        let reset_path = config.root_dir.join(reset);
        let _ = psql_exec_file_ok(&config.api_db, &reset_path).await;
        counters.pass(format!("Reset applied ({})", reset.display()));
    }

    let schemas_sql = WORKER_DOMAINS
        .iter()
        .chain(INFRA_SCHEMAS.iter())
        .map(|s| format!("CREATE SCHEMA IF NOT EXISTS {s};"))
        .collect::<Vec<_>>()
        .join(" ");
    match psql_exec(&config.api_db, &schemas_sql).await {
        Ok(()) => counters.pass(format!(
            "{} schemas ensured ({} domains + {} infra)",
            WORKER_DOMAINS.len() + INFRA_SCHEMAS.len(),
            WORKER_DOMAINS.len(),
            INFRA_SCHEMAS.len()
        )),
        Err(e) => output::warn(format!("schema creation failed (continuing): {e}")),
    }

    let (applied, failed) = apply_migrations(&workers_out.join("migrations"), &config.api_db).await;
    if failed > 0 {
        output::warn(format!("{failed} migration file(s) failed (tolerated)"));
    }
    counters.pass(format!("Migrations: {applied} applied, {failed} failed"));

    let org_a = hurl_org_id(config, "org_id_a", DEFAULT_ORG_A);
    let org_b = hurl_org_id(config, "org_id_b", DEFAULT_ORG_B);
    if let Some(seed) = &config.manifest.database.api.seed_sql {
        let seed_path = config.root_dir.join(seed);
        match psql_exec_file_with_vars(
            &config.api_db,
            &seed_path,
            &[("org_a_id", org_a.as_str()), ("org_b_id", org_b.as_str())],
        )
        .await
        {
            Ok(()) => counters.pass("Seed applied"),
            Err(e) => output::warn(format!("seed failed (continuing): {e}")),
        }
    }
    config.metrics.end();

    // ---- 4. Build workers workspace ----
    output::section("4. Build workers workspace");
    config.metrics.begin("Cargo build (workers)");
    let build_dir = workers_out.join(WORKERS_SUBDIR);
    if !build_dir.is_dir() {
        return Err(OpsError::PathNotFound(build_dir));
    }
    let build_log = "/tmp/codegraph-ops-workers-build.log";
    let (ok, output_text) = cargo_build(&build_dir, args.release, &config.api_db.url());
    let _ = std::fs::write(build_log, &output_text);
    if !ok {
        counters.fail_test("cargo build (workers) failed");
        for line in output_text.lines().filter(|l| l.contains("error")).take(10) {
            println!("    {line}");
        }
        return Err(OpsError::TestFailure(format!(
            "workers cargo build failed — full output: {build_log}"
        )));
    }
    counters.pass("cargo build (workers workspace)");
    let bin_dir = build_dir
        .join("target")
        .join(if args.release { "release" } else { "debug" });
    let gateway_bin = bin_dir.join(format!("{BINARY_PREFIX}-gateway"));
    if !gateway_bin.is_file() {
        counters.fail_test("gateway binary missing");
        return Err(OpsError::TestFailure(format!(
            "no gateway binary at {} — build produced no hr-app-gateway",
            gateway_bin.display()
        )));
    }
    counters.pass(format!("Gateway binary ({})", gateway_bin.display()));
    config.metrics.end();

    // ---- 5-7. Boot + smoke + hurl (teardown runs on every path) ----
    let mut supervisor = Supervisor::new(args.keep);
    let stages = run_boot_and_tests(
        config,
        &mut counters,
        &bin_dir,
        &reused_ports,
        &mut supervisor,
    )
    .await;
    supervisor.shutdown_all().await;
    stages?;

    // ---- Summary ----
    let ok = summarize(&counters);
    if let Some(results_file) = &args.results_file {
        let mut report = crate::results::ResultsReport::new(
            "workers",
            &config.manifest_path,
            config.manifest.profile.as_deref(),
            &config.metrics,
        );
        report.passed = counters.passes;
        report.failed = counters.failures;
        report.failures = counters.failure_log.clone();
        report.exit = i32::from(!ok);
        let _ = report.write(std::path::Path::new(results_file));
    }
    if ok {
        Ok(())
    } else {
        Err(OpsError::TestFailure(format!(
            "{} of {} workers tests failed",
            counters.failures,
            counters.passes + counters.failures
        )))
    }
}

/// Boot the per-domain workers + gateway, then run the gateway smoke checks
/// and the hurl contract tests. The caller owns the supervisor and tears
/// everything down afterwards (both success and failure paths). `reused_ports`
/// (`--reuse`) lists ports served by registry-known prior processes — those
/// are NOT spawned, but the gateway still routes to them.
async fn run_boot_and_tests(
    config: &OpsConfig,
    counters: &mut TestCounters,
    bin_dir: &Path,
    reused_ports: &HashSet<u16>,
    supervisor: &mut Supervisor,
) -> OpsResult<()> {
    // ---- 5. Boot workers + gateway ----
    output::section("5. Boot workers + gateway");
    config.metrics.begin("Boot workers + gateway");

    let mut upstreams: Vec<(String, u16)> = Vec::with_capacity(WORKER_DOMAINS.len());
    for (i, domain) in WORKER_DOMAINS.iter().enumerate() {
        let port = worker_port(i as u16);
        if reused_ports.contains(&port) {
            output::warn(format!(
                "--reuse: worker {domain} already running on port {port} — not booting a new one"
            ));
            upstreams.push(((*domain).to_string(), port));
            continue;
        }
        let binary = bin_dir.join(format!("{BINARY_PREFIX}-{domain}"));
        if !binary.is_file() {
            // Unlike dual-test.sh, which skipped missing binaries silently
            // (and left the gateway routing to a dead upstream), fail fast.
            return Err(OpsError::TestFailure(format!(
                "missing worker binary {} — all {} domain workers are required",
                binary.display(),
                WORKER_DOMAINS.len()
            )));
        }
        let mut cmd = Command::new(&binary);
        cmd.env("DATABASE_URL", config.api_db.url())
            .env("SUPABASE_JWT_SECRET", config.jwt_secret.clone())
            .env("BIND_ADDR", format!("127.0.0.1:{port}"));
        cmd.current_dir(&config.root_dir);
        let log_path = PathBuf::from(format!("/tmp/codegraph-ops-worker-{domain}.log"));
        let proc = ManagedProcess::spawn(cmd, &format!("worker {domain}"), &log_path)?;
        supervisor.add(proc);
        upstreams.push(((*domain).to_string(), port));
        output::info(format!(
            "worker {BINARY_PREFIX}-{domain} -> 127.0.0.1:{port}"
        ));
    }

    if reused_ports.contains(&GATEWAY_PORT) {
        output::warn(format!(
            "--reuse: gateway already running on port {GATEWAY_PORT} — not booting a new one"
        ));
    } else {
        let gateway_bin = bin_dir.join(format!("{BINARY_PREFIX}-gateway"));
        let mut gw_cmd = Command::new(&gateway_bin);
        gw_cmd.env("BIND_ADDR", format!("127.0.0.1:{GATEWAY_PORT}"));
        for (domain, port) in &upstreams {
            gw_cmd.env(gateway_env_name(domain), format!("http://127.0.0.1:{port}"));
        }
        // The UI is cross-origin (vite preview :5173 -> gateway :8787);
        // without the allowlist every browser write fails "Failed to
        // fetch" (issue #468; mirrors the e2e suite's Axum path).
        gw_cmd.env("CORS_ALLOWED_ORIGINS", config.ui_url());
        gw_cmd.current_dir(&config.root_dir);
        let gateway = ManagedProcess::spawn(
            gw_cmd,
            "gateway",
            Path::new("/tmp/codegraph-ops-gateway.log"),
        )?;
        supervisor.add(gateway);
    }

    if let Err(e) = wait_for_url(
        &format!("http://127.0.0.1:{GATEWAY_PORT}/health"),
        GATEWAY_WAIT_SECS,
        "gateway",
    )
    .await
    {
        print_log_tail(Path::new("/tmp/codegraph-ops-gateway.log"), 20);
        return Err(e);
    }
    counters.pass(format!(
        "Gateway healthy ({} workers upstream)",
        upstreams.len()
    ));
    match http_get_body(&format!("http://127.0.0.1:{GATEWAY_PORT}/health/all"), &[]).await {
        Ok((status, body)) if status == "200" => {
            counters.pass("/health/all -> 200");
            output::info(format!("/health/all: {body}"));
        }
        Ok((status, _)) => counters.fail_test(format!("/health/all -> {status}")),
        Err(e) => counters.fail_test(format!("/health/all: {e}")),
    }
    config.metrics.end();

    // ---- 6. Gateway smoke ----
    output::section("6. Gateway smoke");
    config.metrics.begin("Gateway smoke");

    let org_a = hurl_org_id(config, "org_id_a", DEFAULT_ORG_A);
    let api_key = match provision_api_key(config, &org_a, "ops-workers-key").await {
        Ok(key) => {
            counters.pass(format!(
                "API key provisioned (prefix: {})",
                &key[..key.len().min(7)]
            ));
            key
        }
        Err(_) => {
            counters.fail_test("could not provision API key");
            return Err(OpsError::TestFailure(
                "smoke failed: could not provision API key".into(),
            ));
        }
    };

    match http_status(&format!("http://127.0.0.1:{GATEWAY_PORT}/version"), &[]).await {
        Ok(200) => counters.pass("gateway /version -> 200"),
        Ok(status) => counters.fail_test(format!("gateway /version -> {status} (want 200)")),
        Err(e) => counters.fail_test(format!("gateway /version: {e}")),
    }

    if let Some(smoke) = &config.manifest.smoke {
        let route = smoke
            .route
            .clone()
            .unwrap_or_else(|| pluralize_entity_route(&smoke.entity));
        let base = format!(
            "http://127.0.0.1:{GATEWAY_PORT}/api/{}",
            config.manifest.api_version
        );
        // Unauthenticated list must be rejected.
        match http_status(&format!("{base}/{route}"), &[]).await {
            Ok(401) => counters.pass("unauth list -> 401"),
            Ok(status) => counters.fail_test(format!("unauth list -> {status} (want 401)")),
            Err(e) => counters.fail_test(format!("unauth list: {e}")),
        }
        // An unknown domain is a gateway-level 404 (no upstream configured).
        match http_status(&format!("{base}/nonexistent/thing"), &[]).await {
            Ok(404) => counters.pass("unknown domain -> 404"),
            Ok(status) => counters.fail_test(format!("unknown domain -> {status} (want 404)")),
            Err(e) => counters.fail_test(format!("unknown domain: {e}")),
        }
        // Authenticated create through the gateway.
        let bearer = format!("Bearer {api_key}");
        let headers = vec![
            ("Authorization", bearer.as_str()),
            ("Content-Type", "application/json"),
        ];
        match http_post_body(&format!("{base}/{route}"), &smoke.create_body, &headers).await {
            Ok((status, _)) if status == "201" => {
                counters.pass(format!("POST /{route} -> 201"));
            }
            Ok((status, body)) => counters.fail_test(format!(
                "POST /{route} -> {status} (want 201): {}",
                body.lines().next().unwrap_or("")
            )),
            Err(e) => counters.fail_test(format!("POST /{route}: {e}")),
        }
    } else {
        output::warn("no smoke entity configured — skipping curl smoke");
    }
    config.metrics.end();

    // ---- 7. Hurl contract tests (via gateway) ----
    output::section("7. Hurl contract tests (via gateway)");
    config.metrics.begin("Hurl (gateway)");

    if let Some(hurl) = &config.manifest.hurl {
        let hurl_dir = config.root_dir.join(&hurl.dir);
        if hurl_dir.is_dir() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&hurl_dir)
                .map_err(OpsError::Io)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "hurl"))
                .map(|e| e.path())
                .collect();
            files.sort();
            let mut passed = 0usize;
            let mut failed = 0usize;
            for f in files {
                let name = file_name(&f);
                if let Some(reason) = skip_reason(&name, &hurl.skip) {
                    output::info(format!("skip {name} ({reason})"));
                    continue;
                }
                let mut cmd = Command::new("hurl");
                cmd.arg("--test")
                    .arg("--variable")
                    .arg(format!("base_url=http://127.0.0.1:{GATEWAY_PORT}"))
                    .arg("--variable")
                    .arg(format!("api_key={api_key}"))
                    .arg(&f);
                let (ok, requests, output_text) = match cmd.output() {
                    Ok(out) => {
                        let text = format!(
                            "{}{}",
                            String::from_utf8_lossy(&out.stdout),
                            String::from_utf8_lossy(&out.stderr)
                        );
                        let log_path = hurl_log_path(config, &name);
                        if let Err(e) = write_hurl_log(&log_path, &text) {
                            output::warn(format!(
                                "could not write hurl log {}: {e}",
                                log_path.display()
                            ));
                        }
                        if hurl_suite_passed(&text) {
                            (true, parse_requests(&text), String::new())
                        } else {
                            (false, 0, text)
                        }
                    }
                    Err(e) => (false, 0, e.to_string()),
                };
                if ok {
                    passed += 1;
                    counters.pass(format!("{name} ({requests} request(s))"));
                } else {
                    failed += 1;
                    counters.fail_test(format!(
                        "{name} — full output: {}",
                        hurl_log_path(config, &name).display()
                    ));
                    for line in hurl_error_excerpt(&output_text) {
                        println!("    {line}");
                    }
                }
            }
            output::info(format!(
                "hurl via gateway: {passed} passed, {failed} failed"
            ));
        } else {
            output::warn(format!(
                "hurl dir {} missing — skipping",
                hurl_dir.display()
            ));
        }
    } else {
        output::info("no hurl config — skipping hurl tests");
    }
    config.metrics.end();

    Ok(())
}

/// Worker ports in domain order (3001 .. 3001+N-1). Also used by `clean`'s
/// port sweep.
pub fn worker_ports() -> impl Iterator<Item = u16> {
    (0..WORKER_DOMAINS.len() as u16).map(worker_port)
}

/// Regenerate the workers output via `cargo run --release -p {graph} -- run`,
/// forcing the workers profile + output dir regardless of the manifest's
/// monolith settings. Returns (success, combined output).
///
/// Cargo runs from `workspace_root` (the consumer repo root, like
/// dual-test.sh's `(cd "$ROOT" ...)`): the worker scaffold resolves relative
/// crate bases (e.g. profiles.toml `domain_types_base = "crates/..."`)
/// against the process CWD, so generating from a deeper directory emits
/// one-level-short path deps. Manifest-relative inputs are absolutized
/// against `root_dir` to compensate.
fn regenerate(config: &OpsConfig, graph: &str, workers_out: &Path) -> (bool, String) {
    if let Some(hint) = missing_model_source_hint(config) {
        output::warn(hint);
    }
    let cargo_args = regenerate_args(config, graph, workers_out);
    let mut cmd = Command::new("cargo");
    cmd.args(&cargo_args).current_dir(&config.workspace_root);
    match crate::proc::run_streaming(&mut cmd, "generate") {
        Ok(out) => (out.status.success(), out.captured),
        Err(e) => (false, e.to_string()),
    }
}

/// Warning returned when the manifest provides neither `mox_files` nor
/// `schemas_dir` — regeneration then runs the codegen binary without a
/// model source and will fail.
fn missing_model_source_hint(config: &OpsConfig) -> Option<&'static str> {
    (config.manifest.mox_files.is_empty() && config.manifest.schemas_dir.is_none()).then_some(
        "mox_files not set and schemas_dir not set — regenerating without a model source",
    )
}

/// Build the `cargo run --release -p {graph} -- run ...` argument vector.
///
/// mox mode (non-empty `mox_files`): one `--mox-files` flag per file in
/// manifest order (absolutized against `root_dir`), then optional
/// `--config`, then the forced `--profile`/`--output`. No `--schemas`/
/// `--classifier` is passed — the mox pipeline needs no classifier, and a
/// manifest that also sets `schemas_dir` regenerates from mox.
///
/// Legacy schemas mode (`mox_files` empty) is unchanged: `--schemas` +
/// optional `--classifier` only when `schemas_dir` is set. `--profile` is
/// always forced to the workers profile and `--output` to `workers_out`.
fn regenerate_args(config: &OpsConfig, graph: &str, workers_out: &Path) -> Vec<String> {
    let mut cargo_args = vec![
        "run".to_string(),
        "--release".to_string(),
        "-p".to_string(),
        graph.to_string(),
        "--".to_string(),
        "run".to_string(),
    ];
    if !config.manifest.mox_files.is_empty() {
        for file in &config.manifest.mox_files {
            cargo_args.push("--mox-files".to_string());
            cargo_args.push(absolutize(&config.root_dir, Path::new(file)));
        }
    } else if let Some(schemas) = &config.manifest.schemas_dir {
        cargo_args.push("--schemas".to_string());
        cargo_args.push(absolutize(&config.root_dir, schemas));
        if let Some(classifier) = &config.manifest.classifier {
            cargo_args.push("--classifier".to_string());
            cargo_args.push(absolutize(&config.root_dir, classifier));
        }
    }
    if let Some(domain_config) = &config.manifest.domain_config {
        cargo_args.push("--config".to_string());
        cargo_args.push(absolutize(&config.root_dir, domain_config));
    }
    cargo_args.push("--profile".to_string());
    cargo_args.push(PROFILE.to_string());
    cargo_args.push("--output".to_string());
    cargo_args.push(workers_out.to_string_lossy().into_owned());
    cargo_args
}

/// Resolve a manifest path as an absolute string: relative paths are rooted
/// at the manifest directory (`root_dir`).
fn absolutize(root_dir: &Path, path: &Path) -> String {
    if path.is_absolute() {
        path.to_string_lossy().into_owned()
    } else {
        root_dir.join(path).to_string_lossy().into_owned()
    }
}

/// `cargo build` inside the generated workers workspace. The cornucopia
/// `build.rs` connects to Postgres at BUILD time via `CORNUCOPIA_DATABASE_URL`.
/// Returns (success, combined output).
fn cargo_build(build_dir: &Path, release: bool, cornucopia_db_url: &str) -> (bool, String) {
    let mut cmd = Command::new("cargo");
    cmd.arg("build");
    if release {
        cmd.arg("--release");
    }
    cmd.env("CORNUCOPIA_DATABASE_URL", cornucopia_db_url);
    cmd.current_dir(build_dir);
    match crate::proc::run_streaming(&mut cmd, "build") {
        Ok(out) => (out.status.success(), out.captured),
        Err(e) => (false, e.to_string()),
    }
}

/// Apply generated migrations to the database target, one psql invocation
/// per file (mirrors dual-test.sh B2); per-file failures are tolerated but
/// counted. Returns (applied, failed).
async fn apply_migrations(dir: &Path, target: &crate::pg::PgTarget) -> (usize, usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        output::warn(format!("no migrations dir at {} — skipping", dir.display()));
        return (0, 0);
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    files.sort();
    let mut applied = 0usize;
    let mut failed = 0usize;
    for f in files {
        let name = file_name(&f);
        match psql_exec_file(target, &f).await {
            Ok(()) => applied += 1,
            Err(e) => {
                failed += 1;
                output::warn(format!("migration {name} failed: {e}"));
            }
        }
    }
    (applied, failed)
}

/// Org id from `manifest.hurl` with a fallback default.
fn hurl_org_id(config: &OpsConfig, field: &str, default: &str) -> String {
    let Some(hurl) = config.manifest.hurl.as_ref() else {
        return default.to_string();
    };
    match field {
        "org_id_a" => hurl.org_id_a.clone().unwrap_or_else(|| default.to_string()),
        _ => hurl.org_id_b.clone().unwrap_or_else(|| default.to_string()),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Suite summary line; returns true if no failures.
fn summarize(counters: &TestCounters) -> bool {
    let total = counters.passes + counters.failures;
    if counters.failures == 0 {
        println!(
            "\n{}{}ALL {total} WORKERS TESTS PASSED{}",
            output::bold(""),
            output::GREEN_DEF,
            output::NC_DEF
        );
        true
    } else {
        println!(
            "\n{}{}{} of {} WORKERS TESTS FAILED{}",
            output::bold(""),
            output::RED_DEF,
            counters.failures,
            total,
            output::NC_DEF
        );
        false
    }
}

fn print_log_tail(log_path: &Path, n: usize) {
    if let Ok(content) = std::fs::read_to_string(log_path) {
        let lines: Vec<&str> = content.lines().rev().take(n).collect();
        output::warn(format!("--- {} (last {n} lines) ---", log_path.display()));
        for line in lines.iter().rev() {
            println!("    {line}");
        }
        output::warn("--- end ---");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codegraph_config::{OpsDatabase, OpsDbTarget, OpsManifest};

    #[test]
    fn worker_ports_are_sequential_from_base() {
        assert_eq!(worker_port(0), 3001);
        assert_eq!(worker_port(1), 3002);
        assert_eq!(worker_port(10), 3011);
    }

    #[test]
    fn worker_ports_never_collide_with_gateway() {
        for port in worker_ports() {
            assert_ne!(port, GATEWAY_PORT);
        }
        assert_eq!(worker_ports().count(), WORKER_DOMAINS.len());
    }

    #[test]
    fn gateway_env_names_match_generated_gateway_binding() {
        // The generated gateway uppercases the binding and maps '-' → '_'.
        assert_eq!(gateway_env_name("payroll"), "GATEWAY_UPSTREAM_PAYROLL");
        assert_eq!(gateway_env_name("timecard"), "GATEWAY_UPSTREAM_TIMECARD");
        assert_eq!(gateway_env_name("time-card"), "GATEWAY_UPSTREAM_TIME_CARD");
    }

    #[test]
    fn default_hurl_skips_carry_reasons() {
        let none: Vec<String> = vec![];
        for (file, reason) in DEFAULT_HURL_SKIPS {
            assert_eq!(
                skip_reason(file, &none).as_deref(),
                Some(*reason),
                "{file} must carry its default reason"
            );
        }
        // Compliance + RLS + webhooks never run through the gateway.
        assert!(
            skip_reason("10_compliance_rules.hurl", &none)
                .unwrap()
                .contains("monolith")
        );
        assert!(
            skip_reason("11_compliance_check.hurl", &none)
                .unwrap()
                .contains("monolith")
        );
    }

    #[test]
    fn manifest_skips_are_honored() {
        let skip = vec!["04_db_inspection.hurl".to_string()];
        assert_eq!(
            skip_reason("04_db_inspection.hurl", &skip).as_deref(),
            Some("manifest hurl.skip")
        );
        assert!(skip_reason("01_candidate_crud.hurl", &skip).is_none());
    }

    #[test]
    fn default_skip_reason_wins_over_manifest_entry() {
        // 08_rls_isolation is in BOTH the default set and the manifest skip
        // list — the specific default reason must be reported.
        let skip = vec!["08_rls_isolation.hurl".to_string()];
        let reason = skip_reason("08_rls_isolation.hurl", &skip).unwrap();
        assert!(!reason.contains("manifest"), "got: {reason}");
    }

    #[test]
    fn all_eleven_domains_listed() {
        assert_eq!(WORKER_DOMAINS.len(), 11);
        assert!(!WORKER_DOMAINS.contains(&"gateway"));
        for domain in WORKER_DOMAINS {
            assert!(!domain.contains('-'), "bindings assume no dashes: {domain}");
        }
    }

    #[test]
    fn absolutize_roots_relative_paths_at_the_manifest_dir() {
        let root = Path::new("/repo/hr-platform");
        // Plain join (no lexical normalization) — `..` segments stay but the
        // result resolves identically for every consumer.
        assert_eq!(
            absolutize(root, Path::new("../schemas")),
            "/repo/hr-platform/../schemas"
        );
        // Absolute paths pass through untouched.
        assert_eq!(
            absolutize(root, Path::new("/elsewhere/schemas")),
            "/elsewhere/schemas"
        );
    }

    fn workers_manifest() -> OpsManifest {
        OpsManifest {
            app_name: "demo-app".into(),
            graph_binary: Some("hr-graph".into()),
            schemas_dir: Some("schemas".into()),
            mox_files: Vec::new(),
            rosetta_files: Vec::new(),
            classifier: Some("classifier.toml".into()),
            domain_config: Some("domains.toml".into()),
            profile: Some("default".into()),
            output_dir: "generated-app".into(),
            ui_dir: None,
            smoke: None,
            api_version: "v1".to_string(),
            servers: Default::default(),
            database: OpsDatabase {
                api: OpsDbTarget {
                    host: "localhost".into(),
                    port: 5432,
                    user: "u".into(),
                    password: "p".into(),
                    database: "postgres".into(),
                    reset_sql: None,
                    seed_sql: None,
                    grant_role: None,
                    grant_strict: None,
                },
                e2e: None,
                e2e_app: None,
            },
            supabase: None,
            capabilities: Default::default(),
            hurl: None,
            hooks: vec![],
            extensions: vec![],
            doctor: Default::default(),
            bundle: Default::default(),
        }
    }

    fn workers_config(manifest: OpsManifest) -> OpsConfig {
        OpsConfig::from_manifest(manifest, PathBuf::from("/tmp/repo")).unwrap()
    }

    #[test]
    fn regenerate_args_mox_mode_replaces_schemas_and_classifier() {
        let mut manifest = workers_manifest();
        manifest.mox_files = vec!["model/common.mox".into(), "model/billing.mox".into()];
        let cfg = workers_config(manifest);
        let args = regenerate_args(&cfg, "hr-graph", Path::new("/tmp/repo/generated-workers"));
        assert_eq!(
            args,
            vec![
                "run",
                "--release",
                "-p",
                "hr-graph",
                "--",
                "run",
                "--mox-files",
                "/tmp/repo/model/common.mox",
                "--mox-files",
                "/tmp/repo/model/billing.mox",
                "--config",
                "/tmp/repo/domains.toml",
                "--profile",
                "workers-cornucopia",
                "--output",
                "/tmp/repo/generated-workers",
            ]
        );
    }

    #[test]
    fn regenerate_args_legacy_schemas_mode_unchanged() {
        let manifest = workers_manifest();
        let cfg = workers_config(manifest);
        let args = regenerate_args(&cfg, "hr-graph", Path::new("/tmp/repo/generated-workers"));
        assert_eq!(
            args,
            vec![
                "run",
                "--release",
                "-p",
                "hr-graph",
                "--",
                "run",
                "--schemas",
                "/tmp/repo/schemas",
                "--classifier",
                "/tmp/repo/classifier.toml",
                "--config",
                "/tmp/repo/domains.toml",
                "--profile",
                "workers-cornucopia",
                "--output",
                "/tmp/repo/generated-workers",
            ]
        );
    }

    #[test]
    fn missing_model_source_hint_only_when_neither_set() {
        // Neither source → hint naming both knobs.
        let mut manifest = workers_manifest();
        manifest.schemas_dir = None;
        let cfg = workers_config(manifest);
        let hint = missing_model_source_hint(&cfg).expect("neither source must hint");
        assert!(hint.contains("mox_files not set"), "{hint}");
        assert!(hint.contains("schemas_dir not set"), "{hint}");
        // mox-only → no hint.
        let mut manifest = workers_manifest();
        manifest.schemas_dir = None;
        manifest.mox_files = vec!["model/app.mox".into()];
        let cfg = workers_config(manifest);
        assert!(missing_model_source_hint(&cfg).is_none());
        // schemas-only → no hint (legacy behavior).
        let cfg = workers_config(workers_manifest());
        assert!(missing_model_source_hint(&cfg).is_none());
    }
}
