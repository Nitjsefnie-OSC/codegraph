//! Full E2E suite (port of hr-platform/test.sh `cmd_e2e`, genericised):
//! supabase → generate → migrate → build → services → playwright.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::OpsConfig;
use crate::error::{OpsError, OpsResult};
use crate::ext::HookPolicy;
use crate::metrics::Metrics;
use crate::output;
use crate::proc::{ManagedProcess, Supervisor, run_streaming, run_streaming_quiet};
use crate::results::{ResultsReport, SuiteFailure};
use crate::wait::wait_for_url;

/// Logs for the e2e services.
const APP_LOG: &str = "/tmp/codegraph-ops-e2e-app.log";
const SVELTEKIT_LOG: &str = "/tmp/codegraph-ops-sveltekit-e2e.log";

/// Arguments for the full E2E suite.
pub struct E2eArgs {
    pub keep: bool,
    pub skip_build: bool,
    pub skip_generate: bool,
    pub release: bool,
    pub headed: bool,
    /// Skip the SvelteKit production build. DANGEROUS: `vite preview` will
    /// then serve whatever stale bundle happens to be in `dist/`.
    pub skip_ui_build: bool,
    /// When the main Playwright run fails, immediately rerun only the failed
    /// tests (`--last-failed`) in the same session; a green retry counts as
    /// transient and the suite passes.
    pub retry_failed: bool,
    /// Write a machine-readable `--results` JSON report to this path.
    pub results_file: Option<String>,
    /// Warn instead of failing when the generator rev in the app's
    /// `.codegraph-manifest.json` differs from this testkit's pinned rev.
    pub allow_gen_rev_mismatch: bool,
    /// Reuse registry-known servers on the api/ui ports instead of taking
    /// them over (documented caveat: they may serve a stale build).
    pub reuse: bool,
    pub playwright_args: Vec<String>,
}

/// Full E2E: supabase → generate → migrate → build → services → playwright.
/// Requires manifest.supabase and manifest.database.e2e to be set (else
/// Err(Config) explaining what's missing).
///
/// `post_e2e` hooks fire on EVERY path (success and failure) with
/// [`HookPolicy::WarnOnly`] — cleanup hooks must always run and never mask
/// the real failure, and `fatal = true` cannot escalate past this
/// (documented contract). All other hook points follow each hook's manifest
/// `fatal` flag (missing = fatal).
pub async fn run_e2e(config: &OpsConfig, args: &E2eArgs) -> OpsResult<()> {
    let result = run_e2e_inner(config, args).await;
    if let Err(e) = crate::ext::run_hooks(config, "post_e2e", HookPolicy::WarnOnly).await {
        output::warn(format!("post_e2e hook failed: {e}"));
    }
    result
}

/// Time a fallible e2e stage: `metrics.begin(name)` → run → `metrics.end()`.
/// The stage is recorded on BOTH paths, so the results-JSON `stages` array
/// stays complete even when a stage fails.
async fn timed<F, T>(metrics: &Metrics, name: &str, body: F) -> OpsResult<T>
where
    F: std::future::Future<Output = OpsResult<T>>,
{
    metrics.begin(name);
    let res = body.await;
    metrics.end();
    res
}

async fn run_e2e_inner(config: &OpsConfig, args: &E2eArgs) -> OpsResult<()> {
    output::section("=== E2E Suite ===");

    if config.manifest.supabase.is_none() {
        return Err(OpsError::Config(
            "e2e requires a manifest [[supabase]] section (dir + standard local keys)".to_string(),
        ));
    }
    if config.manifest.database.e2e.is_none() {
        return Err(OpsError::Config(
            "e2e requires manifest database.e2e (the supabase postgres target, usually :54322)"
                .to_string(),
        ));
    }
    let Some(supabase_dir) = config.supabase_dir.as_ref() else {
        return Err(OpsError::Config(
            "supabase.dir did not resolve to a path".to_string(),
        ));
    };

    // Fast doctor (#358): npx/pnpm/supabase/chromium/ports/disk in seconds —
    // a missing npx used to surface only at the supabase stage as
    // `failed to spawn npx`, after nothing useful had happened yet.
    crate::doctor::run_fast_doctor(config, crate::doctor::FastDoctorSuite::E2e).await?;

    // Port preflight: the suite binds both ports itself, and a leftover dev
    // server must fail the run in seconds instead of after supabase/build.
    // Registry-known prior servers (`--keep` leaks) are taken over by
    // default or reused with `--reuse`.
    let api_port_state = crate::preflight::ensure_port_available(
        &config.root_dir,
        config.manifest.servers.api_port,
        args.reuse,
    )?;
    let ui_port_state = crate::preflight::ensure_port_available(
        &config.root_dir,
        config.manifest.servers.ui_port,
        args.reuse,
    )?;

    let mut supervisor = Supervisor::new(args.keep);

    // 0. Freshness pre-check (--skip-build only): with the build skipped the
    // binary cannot become fresher later, so an early stale verdict is final
    // and costs seconds — instead of surfacing after supabase up, generation,
    // `db reset` + seed and API-key provisioning (~10 minutes of DB work).
    // When build is ENABLED the check stays post-build (`e2e_build`), since
    // building may fix staleness.
    if freshness_precheck_required(args.skip_build)
        && let Some(binary) = pick_binary(&config.app_dir, &config.app_binary_name(), args.release)
    {
        output::section("E2E 0. freshness (pre-check, --skip-build)");
        config.metrics.begin("Freshness pre-check");
        let verdict = crate::preflight::ensure_binary_fresh(&config.app_dir, &binary);
        config.metrics.end();
        verdict?;
    }

    // 1. Supabase.
    e2e_supabase_up(config, supabase_dir).await?;

    // 2. Generate.
    e2e_generate(config, args).await?;

    // 3. Migrate.
    e2e_migrate(config, supabase_dir).await?;

    // 4. Provision the API key (shared file, also used by the ui/cli suites).
    let api_key = e2e_provision_api_key(config).await?;

    // 5. Build.
    let binary = e2e_build(config, args).await?;

    // 6. Services.
    e2e_start_services(
        config,
        args,
        &binary,
        api_key.as_deref(),
        api_port_state.reused(),
        ui_port_state.reused(),
        &mut supervisor,
    )
    .await?;

    // 7. Playwright.
    let outcome = e2e_playwright(config, args, api_key.as_deref()).await?;

    // 8. Summary.
    e2e_write_summary(config, args, &outcome);
    supervisor.shutdown_all().await;
    if outcome.passed {
        Ok(())
    } else {
        Err(OpsError::TestFailure(
            "Playwright E2E suite failed".to_string(),
        ))
    }
}

/// Whether the e2e suite must run its binary-freshness check at stage 0
/// (before Supabase) instead of after the build stage: with `--skip-build`
/// the binary cannot become fresher later, so an early stale verdict is
/// final and cheap; with a build enabled the post-build check in
/// [`e2e_build`] stays authoritative.
pub fn freshness_precheck_required(skip_build: bool) -> bool {
    skip_build
}

/// Ensure the Supabase CLI stack is running: health check first, then
/// `npx supabase start` when down. Every suite (e2e, api, workers) targets
/// the same Supabase-provisioned database, so they all run this before any
/// DB-touching stage.
pub(crate) async fn supabase_ensure_up(config: &OpsConfig, supabase_dir: &Path) -> OpsResult<()> {
    let health_url = supabase_health_url(config);
    if http_ok(&health_url).await {
        output::ok(format!("Supabase already running ({health_url})"));
    } else {
        output::info("Starting Supabase (npx supabase start)...");
        run_blocking("supabase", "npx", &["supabase", "start"], supabase_dir)?;
        output::ok("Supabase started");
    }
    Ok(())
}

async fn e2e_supabase_up(config: &OpsConfig, supabase_dir: &Path) -> OpsResult<()> {
    output::section("E2E 1. Supabase");
    timed(&config.metrics, "Supabase", async {
        supabase_ensure_up(config, supabase_dir).await?;
        // pre_e2e hooks (e.g. the pgmq patch) need the supabase container
        // running and must complete BEFORE the migration symlink + `supabase
        // db reset`.
        crate::ext::run_hooks(config, "pre_e2e", HookPolicy::PerHook)
            .await
            .map(|_| ())
    })
    .await
}

async fn e2e_generate(config: &OpsConfig, args: &E2eArgs) -> OpsResult<()> {
    output::section("E2E 2. Generate");
    timed(&config.metrics, "Generate", async {
        if args.skip_generate {
            output::info("Generation skipped (--skip-generate)");
            config.metrics.end_with("skipped");
            return Ok(());
        }
        let Some(binary) = generation_binary(config) else {
            if config.manifest.graph_binary.is_none() {
                output::warn("no graph_binary configured — skipping generation");
            } else {
                output::warn("schemas_dir not configured — skipping generation");
            }
            config.metrics.end_with("no model source");
            return Ok(());
        };
        crate::ext::run_hooks(config, "pre_generate", HookPolicy::PerHook).await?;
        output::info(format!("Building {binary} (release)..."));
        run_blocking(
            "build",
            "cargo",
            &["build", "-p", &binary, "--release"],
            &config.workspace_root,
        )
        .map_err(|e| OpsError::TestFailure(format!("graph binary build failed: {e}")))?;
        let gen_bin = config
            .workspace_root
            .join("target")
            .join("release")
            .join(&binary);
        if !gen_bin.is_file() {
            return Err(OpsError::TestFailure(format!(
                "{binary} build produced no binary at {}",
                gen_bin.display()
            )));
        }
        let gen_args = generate_args(config);
        output::info("Generating app...");
        let gen_bin_str = gen_bin.to_string_lossy().into_owned();
        let gen_arg_refs: Vec<&str> = gen_args.iter().map(String::as_str).collect();
        run_blocking("generate", &gen_bin_str, &gen_arg_refs, &config.root_dir)?;
        if !generation_outputs(config) {
            return Err(OpsError::TestFailure(
                "code generation produced no output (src/ui/migrations empty)".to_string(),
            ));
        }
        // #357: the freshly written manifest names the generator rev — a
        // stale graph binary is a hard error here, before DB work starts.
        crate::freshness::check_generator_rev(&config.app_dir, args.allow_gen_rev_mismatch)?;
        output::ok("App generated");
        crate::ext::run_hooks(config, "post_generate", HookPolicy::PerHook)
            .await
            .map(|_| ())
    })
    .await
}

async fn e2e_migrate(config: &OpsConfig, supabase_dir: &Path) -> OpsResult<()> {
    output::section("E2E 3. Database");
    timed(&config.metrics, "DB reset + seed", async {
        let app_migrations = config.app_dir.join("migrations");
        let supabase_migrations = supabase_dir.join("supabase").join("migrations");
        if app_migrations.is_dir() {
            crate::migrate::link_migrations_to_supabase(&app_migrations, &supabase_migrations)?;
        } else {
            output::warn(format!(
                "no migrations dir at {} — skipping symlink",
                app_migrations.display()
            ));
        }
        output::info("Resetting database (npx supabase db reset)...");
        run_blocking(
            "supabase",
            "npx",
            &["supabase", "db", "reset"],
            supabase_dir,
        )
        .map_err(|e| OpsError::TestFailure(format!("supabase db reset failed: {e}")))?;
        output::ok("Database reset with migrations");

        let seed = supabase_dir.join("supabase").join("seed.sql");
        if seed.is_file()
            && let Some(e2e) = &config.e2e_db
        {
            crate::db::psql_exec_file_ok(e2e, &seed).await?;
            output::ok("seed.sql applied");
        }

        crate::ext::run_hooks(config, "post_migrate", HookPolicy::PerHook)
            .await
            .map(|_| ())
    })
    .await
}

async fn e2e_provision_api_key(config: &OpsConfig) -> OpsResult<Option<String>> {
    timed(&config.metrics, "API key", async {
        let api_key = super::ui::read_or_provision_api_key(config).await?;
        match &api_key {
            Some(_) => output::ok("API key provisioned"),
            None => output::warn("API key not provisioned — auth-dependent tests will fail"),
        }
        Ok(api_key)
    })
    .await
}

async fn e2e_build(config: &OpsConfig, args: &E2eArgs) -> OpsResult<PathBuf> {
    output::section("E2E 4. Build");
    timed(&config.metrics, "App build", async {
        if args.skip_build {
            config.metrics.end_with("skipped (--skip-build)");
        } else if let Err(e) = cargo_build_app(config, args.release) {
            return Err(OpsError::TestFailure(format!("app build failed: {e}")));
        }
        let binary = pick_binary(&config.app_dir, &config.app_binary_name(), args.release)
            .ok_or_else(|| {
                OpsError::TestFailure(format!(
                    "no app binary under {} — run without --skip-build",
                    config.app_dir.join("target").display()
                ))
            })?;
        crate::preflight::ensure_binary_fresh(&config.app_dir, &binary)?;
        output::ok(format!("Using binary {}", binary.display()));
        Ok(binary)
    })
    .await
}

async fn e2e_start_services(
    config: &OpsConfig,
    args: &E2eArgs,
    binary: &Path,
    api_key: Option<&str>,
    api_reused: bool,
    ui_reused: bool,
    supervisor: &mut Supervisor,
) -> OpsResult<()> {
    output::section("E2E 5. Start Services");
    let api_url = config.api_url();
    if api_reused {
        output::warn(
            "--reuse: not booting the app server — the running registry service keeps serving \
             (it may be a stale build)",
        );
    } else {
        let db_url = config
            .e2e_app_db
            .as_ref()
            .map(|t| t.url())
            .unwrap_or_else(|| config.api_db.url());
        let mut cmd = Command::new(binary);
        cmd.arg("start")
            .arg("--bind-addr")
            .arg(bind_addr_with_port(config))
            .arg("--database-url")
            .arg(&db_url);
        cmd.env("CORS_ALLOWED_ORIGINS", config.ui_url());
        cmd.env("SUPABASE_JWT_SECRET", &config.jwt_secret);
        cmd.env("SUPABASE_URL", super::ui::supabase_base_url(config));
        // Same contract as the api suite: the app resolves `config/` relative
        // to its cwd — pin it to the manifest root instead of inheriting the
        // caller's cwd.
        cmd.current_dir(&config.root_dir);
        let mut proc = match ManagedProcess::spawn(cmd, "Axum app", Path::new(APP_LOG)) {
            Ok(proc) => proc,
            Err(e) => {
                return Err(OpsError::Command(format!(
                    "failed to spawn app server: {e}"
                )));
            }
        };
        proc.set_registration(crate::proc::ServiceRegistration::new(
            config.root_dir.clone(),
            "api",
            config.manifest.servers.api_port,
            Some("/health"),
            "e2e",
            args.release.then(|| "release".to_string()),
        ));
        if let Err(e) = wait_for_url(&format!("{api_url}/health"), 30, "Axum").await {
            print_log_tail(APP_LOG);
            return Err(e);
        }
        // Health OK → record; removed again on shutdown, left behind on --keep.
        proc.record_service();
        supervisor.add(proc);
        output::ok("Axum API running");
    }

    // pre_playwright hooks (e.g. UI-sync rsync steps) must land BEFORE the
    // SvelteKit production build so synced sources get compiled.
    crate::ext::run_hooks(config, "pre_playwright", HookPolicy::PerHook).await?;

    // Web build stage covers dependency install + the production bundle.
    config.metrics.begin("Web build");
    if !config.ui_dir.join("node_modules").is_dir()
        && let Err(e) = run_blocking_quiet("web", "pnpm", &["install"], &config.ui_dir)
    {
        output::warn(format!("pnpm install failed (continuing): {e}"));
    }
    if args.skip_ui_build {
        output::warn(
            "--skip-ui-build: skipping SvelteKit build — preview may serve a STALE bundle",
        );
        config.metrics.end_with("skipped (--skip-ui-build)");
    } else {
        output::info("Building SvelteKit production bundle...");
        // A failed build is fatal: a stale UI bundle produces baffling test
        // failures far removed from the real cause.
        let built = run_blocking("web", "pnpm", &["run", "build"], &config.ui_dir).map_err(|e| {
            OpsError::TestFailure(format!(
                "SvelteKit build failed (use --skip-ui-build to override): {e}"
            ))
        });
        config.metrics.end();
        built?;
    }
    let ui_url = config.ui_url();
    if ui_reused {
        output::warn(
            "--reuse: not booting vite preview — the running registry service keeps serving \
             (it may serve a stale bundle even though dist/ was just rebuilt)",
        );
        return Ok(());
    }
    {
        let mut cmd = Command::new("pnpm");
        cmd.arg("exec")
            .arg("vite")
            .arg("preview")
            .arg("--port")
            .arg(config.manifest.servers.ui_port.to_string());
        cmd.current_dir(&config.ui_dir);
        cmd.env("PUBLIC_API_URL", &api_url);
        if let Some(key) = api_key {
            cmd.env("PUBLIC_API_KEY", key);
        }
        // Same Supabase SSR requirement as the ui suite (issue #468).
        cmd.env(
            "PUBLIC_SUPABASE_URL",
            crate::suites::ui::supabase_base_url(config),
        );
        cmd.env("PUBLIC_SUPABASE_ANON_KEY", &config.anon_key);
        let mut proc =
            match ManagedProcess::spawn(cmd, "SvelteKit preview", Path::new(SVELTEKIT_LOG)) {
                Ok(proc) => proc,
                Err(e) => {
                    return Err(OpsError::Command(format!(
                        "failed to spawn vite preview: {e}"
                    )));
                }
            };
        proc.set_registration(crate::proc::ServiceRegistration::new(
            config.root_dir.clone(),
            "ui",
            config.manifest.servers.ui_port,
            None,
            "e2e",
            None,
        ));
        if let Err(e) = wait_for_url(&ui_url, 45, "SvelteKit").await {
            print_log_tail(SVELTEKIT_LOG);
            return Err(e);
        }
        proc.record_service();
        supervisor.add(proc);
    }
    output::ok(format!("SvelteKit preview running at {ui_url}"));
    Ok(())
}

/// Outcome of the Playwright run (including the in-session `--last-failed`
/// retry), consumed by the summary/report stage.
struct PlaywrightOutcome {
    passed: bool,
    tallies: Vec<ProjectTally>,
    failed_titles: Vec<String>,
    transient_resolved: usize,
}

async fn e2e_playwright(
    config: &OpsConfig,
    args: &E2eArgs,
    api_key: Option<&str>,
) -> OpsResult<PlaywrightOutcome> {
    output::section("E2E 6. Playwright Tests");
    if let Err(e) = run_blocking_quiet(
        "web",
        "npx",
        &["playwright", "install", "chromium"],
        &config.ui_dir,
    ) {
        output::warn(format!(
            "playwright install chromium failed (continuing): {e}"
        ));
    }
    // Stale transpile-cache hygiene: the cache once served OLD transpiled
    // specs after regeneration (Playwright executing code matching no file
    // on disk). Whole-dir clear — see pwcache for the scoping decision.
    crate::pwcache::clear_and_report();
    let mut cmd = Command::new("npx");
    cmd.arg("playwright").arg("test");
    if args.headed {
        cmd.arg("--headed");
    }
    cmd.args(&args.playwright_args);
    for (key, value) in super::ui::playwright_env(config, api_key) {
        cmd.env(key, value);
    }
    if let Some(chromium) = crate::env::find_chromium() {
        cmd.env("PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH", chromium);
    }
    cmd.current_dir(&config.ui_dir);
    // Streams list-format result lines LIVE (tests visibly finish) while
    // capturing for the per-project tally and failed-title parsing.
    config.metrics.begin("Playwright");
    let out = match run_streaming(&mut cmd, "playwright") {
        Ok(out) => out,
        Err(e) => {
            config.metrics.end();
            return Err(OpsError::Command(format!(
                "failed to spawn playwright: {e}"
            )));
        }
    };
    let mut passed = out.status.success();
    let tallies = tally_playwright_projects(&out.captured);
    let mut failed_titles = failed_test_titles(&out.captured);
    let mut transient_resolved = 0usize;
    config.metrics.end();

    // Retry just the failures, in-session: `--last-failed` reuses
    // Playwright's own record of what failed, so the caller doesn't have to
    // reconstruct grep patterns. Known-transient flakes (a server hiccup
    // closing a socket mid-run) then don't fail an otherwise-green suite.
    if !passed && args.retry_failed && !failed_titles.is_empty() {
        output::section("E2E 7. Retry failed tests");
        let raw_failure_count = failed_titles.len();
        output::info(format!(
            "rerunning {raw_failure_count} failed test(s) via --last-failed..."
        ));
        let mut retry_cmd = Command::new("npx");
        retry_cmd.arg("playwright").arg("test").arg("--last-failed");
        if args.headed {
            retry_cmd.arg("--headed");
        }
        retry_cmd.args(&args.playwright_args);
        for (key, value) in super::ui::playwright_env(config, api_key) {
            retry_cmd.env(key, value);
        }
        if let Some(chromium) = crate::env::find_chromium() {
            retry_cmd.env("PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH", chromium);
        }
        retry_cmd.current_dir(&config.ui_dir);
        config.metrics.begin("Retry failed");
        match run_streaming(&mut retry_cmd, "playwright") {
            Ok(retry_out) => {
                failed_titles = failed_test_titles(&retry_out.captured);
                if retry_out.status.success() {
                    transient_resolved = raw_failure_count;
                    passed = true;
                    output::ok(format!(
                        "{transient_resolved} failed test(s) passed on retry — transient"
                    ));
                }
            }
            Err(e) => output::warn(format!("retry run could not start: {e}")),
        }
        config.metrics.end();
    }

    Ok(PlaywrightOutcome {
        passed,
        tallies,
        failed_titles,
        transient_resolved,
    })
}

fn e2e_write_summary(config: &OpsConfig, args: &E2eArgs, outcome: &PlaywrightOutcome) {
    output::section("=== E2E Summary ===");
    let tallies = &outcome.tallies;
    if !tallies.is_empty() {
        output::info("Per-project Playwright results:");
        for t in tallies {
            println!("  {}: {} passed, {} failed", t.project, t.passed, t.failed);
        }
    }
    if outcome.passed {
        if outcome.transient_resolved > 0 {
            output::ok(format!(
                "ALL E2E TESTS PASSED ({} transient, passed on retry)",
                outcome.transient_resolved
            ));
        } else {
            output::ok("ALL E2E TESTS PASSED");
        }
    } else {
        output::fail("SOME E2E TESTS FAILED");
        let failed_titles = &outcome.failed_titles;
        if !failed_titles.is_empty() {
            output::info("Failed tests:");
            for title in failed_titles.iter().take(20) {
                println!("  - {title}");
            }
            if failed_titles.len() > 20 {
                println!("  … and {} more", failed_titles.len() - 20);
            }
            output::info("Retry just the failures (same session state):");
            let manifest = config.manifest_path.display();
            println!("  testkit --config {manifest} e2e -- --last-failed");
            let greps: Vec<String> = failed_titles
                .iter()
                .take(8)
                .map(|t| format!("--grep \"{t}\""))
                .collect();
            if !greps.is_empty() {
                println!("  testkit --config {manifest} e2e -- {}", greps.join(" "));
            }
        }
    }
    if let Some(results_file) = &args.results_file {
        let mut report = ResultsReport::new(
            "e2e",
            &config.manifest_path,
            config.manifest.profile.as_deref(),
            &config.metrics,
        );
        report.passed = tallies.iter().map(|t| t.passed).sum();
        report.failed = tallies.iter().map(|t| t.failed).sum();
        report.failures = outcome
            .failed_titles
            .iter()
            .map(|title| SuiteFailure {
                context: String::new(),
                title: title.clone(),
            })
            .collect();
        report.transient_resolved = outcome.transient_resolved;
        report.exit = i32::from(!outcome.passed);
        let _ = report.write(Path::new(results_file));
    }
}

/// Per-project Playwright pass/fail tally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTally {
    pub project: String,
    pub passed: usize,
    pub failed: usize,
}

/// Tally Playwright's list-format result lines, `✓/✗ <n> [<project>] › ...`,
/// into per-project pass/fail counts (first-seen project order). Lines that
/// don't match the format (summaries, retry banners, failure re-prints) are
/// ignored. The run index `<n>` is optional (varies by Playwright version).
pub fn tally_playwright_projects(output: &str) -> Vec<ProjectTally> {
    let mut tallies: Vec<ProjectTally> = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim_start();
        let (failed, rest) = if let Some(rest) = trimmed.strip_prefix('✓') {
            (false, rest)
        } else if let Some(rest) = trimmed
            .strip_prefix('✗')
            .or_else(|| trimmed.strip_prefix('✘'))
        {
            (true, rest)
        } else {
            continue;
        };
        let rest = rest.trim_start();
        // Optional run index: `✓ 12 [chromium] › ...`.
        let rest = match rest.split_once(' ') {
            Some((index, remainder)) if index.parse::<u32>().is_ok() => remainder.trim_start(),
            _ => rest,
        };
        let Some(rest) = rest.strip_prefix('[') else {
            continue;
        };
        let Some((project, rest)) = rest.split_once(']') else {
            continue;
        };
        if project.is_empty() || !rest.trim_start().starts_with('›') {
            continue;
        }
        match tallies.iter_mut().find(|t| t.project == project) {
            Some(t) => {
                if failed {
                    t.failed += 1;
                } else {
                    t.passed += 1;
                }
            }
            None => tallies.push(ProjectTally {
                project: project.to_string(),
                passed: usize::from(!failed),
                failed: usize::from(failed),
            }),
        }
    }
    tallies
}

/// Extract the grep-able title path from Playwright failure result lines:
///
/// ```text
/// ✘  3 [crud] › tests/x.owner.crud.test.ts:95:3 › Owner CRUD › owner can edit X (453ms)
/// ```
///
/// → `Owner CRUD › owner can edit X`.
///
/// The `file:line:col` prefix and the trailing `(duration)` are stripped —
/// `--grep` matches against the title path only, which is exactly why
/// grepping hyphenated *file names* never matches anything.
pub fn failed_test_titles(output: &str) -> Vec<String> {
    let mut titles: Vec<String> = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed
            .strip_prefix('✗')
            .or_else(|| trimmed.strip_prefix('✘'))
        else {
            continue;
        };
        let rest = rest.trim_start();
        // Optional run index: `✘ 12 [project] › ...`.
        let rest = match rest.split_once(' ') {
            Some((index, remainder)) if index.parse::<u32>().is_ok() => remainder.trim_start(),
            _ => rest,
        };
        let Some(rest) = rest.strip_prefix('[') else {
            continue;
        };
        let Some((_, rest)) = rest.split_once(']') else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('›').map(str::trim_start) else {
            continue;
        };
        // `file:line:col › Title path (duration)` — drop the first segment
        // when a second one exists, then drop a trailing `(duration)`.
        let mut segments = rest.split(" › ").peekable();
        let _file = segments.next();
        let title_parts: Vec<&str> = segments.collect();
        if title_parts.is_empty() {
            continue;
        }
        let mut title = title_parts.join(" › ");
        if let Some(open) = title.rfind(" (")
            && title.ends_with(')')
        {
            title.truncate(open);
        }
        let title = title.trim().to_string();
        if !title.is_empty() && !titles.contains(&title) {
            titles.push(title);
        }
    }
    titles
}

/// Graph binary to build for generation, or `None` when the caller should
/// skip with a warning. Generation proceeds only when `graph_binary` is set
/// AND a model source exists: `mox_files` (preferred) or `schemas_dir` —
/// a manifest setting both regenerates from mox.
fn generation_binary(config: &OpsConfig) -> Option<String> {
    let binary = config.manifest.graph_binary.as_ref()?;
    if !config.manifest.mox_files.is_empty() || config.manifest.schemas_dir.is_some() {
        Some(binary.clone())
    } else {
        None
    }
}

/// Build the graph-binary `run ...` argument vector.
///
/// mox mode (non-empty `mox_files`): `run --mox-files <file>...` in manifest
/// order, then the optional `--config`/`--profile`, then `--output`. No
/// `--schemas`/`--classifier` is passed — the mox pipeline needs no
/// classifier, and a manifest that also sets `schemas_dir` regenerates from
/// mox (schemas_dir/classifier are ignored for generation).
///
/// Legacy schemas mode (`mox_files` empty) is unchanged: only manifest flags
/// whose values are `Some` are passed.
fn generate_args(config: &OpsConfig) -> Vec<String> {
    if !config.manifest.mox_files.is_empty() {
        let mut args = vec!["run".to_string()];
        for file in &config.manifest.mox_files {
            args.push("--mox-files".to_string());
            args.push(file.clone());
        }
        if let Some(domain_config) = &config.manifest.domain_config {
            args.push("--config".to_string());
            args.push(domain_config.to_string_lossy().into_owned());
        }
        if let Some(profile) = &config.manifest.profile {
            args.push("--profile".to_string());
            args.push(profile.clone());
        }
        args.push("--output".to_string());
        args.push(config.app_dir.to_string_lossy().into_owned());
        return args;
    }
    let schemas = config
        .manifest
        .schemas_dir
        .as_ref()
        .expect("caller checks schemas_dir")
        .to_string_lossy()
        .into_owned();
    let mut args = vec!["run".to_string(), "--schemas".to_string(), schemas];
    if let Some(classifier) = &config.manifest.classifier {
        args.push("--classifier".to_string());
        args.push(classifier.to_string_lossy().into_owned());
    }
    if let Some(domain_config) = &config.manifest.domain_config {
        args.push("--config".to_string());
        args.push(domain_config.to_string_lossy().into_owned());
    }
    if let Some(profile) = &config.manifest.profile {
        args.push("--profile".to_string());
        args.push(profile.clone());
    }
    args.push("--output".to_string());
    args.push(config.app_dir.to_string_lossy().into_owned());
    args
}

/// True when generation produced anything (src/ui/migrations with content).
fn generation_outputs(config: &OpsConfig) -> bool {
    dir_has_content(&config.app_dir.join("src"))
        || dir_has_content(&config.app_dir.join("migrations"))
        || dir_has_content(&config.app_dir.join("ui"))
}

fn dir_has_content(dir: &Path) -> bool {
    dir.is_dir()
        && std::fs::read_dir(dir)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false)
}

/// Locate the app binary: preferred profile first, falling back to the other.
pub fn pick_binary(app_dir: &Path, name: &str, release: bool) -> Option<PathBuf> {
    let (preferred, other) = if release {
        ("release", "debug")
    } else {
        ("debug", "release")
    };
    for profile in [preferred, other] {
        let candidate = app_dir.join("target").join(profile).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// `{bind_addr}:{api_port}` — unless bind_addr already carries a port.
fn bind_addr_with_port(config: &OpsConfig) -> String {
    let bind = &config.manifest.servers.bind_addr;
    if bind.contains(':') {
        bind.clone()
    } else {
        format!("{bind}:{}", config.manifest.servers.api_port)
    }
}

/// Health URL to probe for a running supabase stack.
fn supabase_health_url(config: &OpsConfig) -> String {
    config
        .manifest
        .supabase
        .as_ref()
        .and_then(|s| s.health_url.clone())
        .unwrap_or_else(|| "http://localhost:54321/auth/v1/health".to_string())
}

/// True when `curl -sf` succeeds against `url`.
async fn http_ok(url: &str) -> bool {
    Command::new("curl")
        .args(["-sf", "--max-time", "5"])
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `cargo build` inside the generated app, exporting `CORNUCOPIA_DATABASE_URL`
/// when the manifest selects the cornucopia persistence provider — its
/// `build.rs` connects to Postgres at build time to compile the SQL-first
/// repositories (the api and workers suites export the same env).
fn cargo_build_app(config: &OpsConfig, release: bool) -> Result<(), String> {
    // Delegate to the shared builder: it streams under [build], pins the
    // app dir as cwd, and post-build touches the app binary so mtime-based
    // freshness (ensure_binary_fresh) passes even when the pre_generate
    // clean hook wiped src and cargo skipped the relink on byte-identical
    // regeneration.
    super::api::cargo_build_in(config, release)
}

/// Run a blocking command to completion, streaming its output under
/// `[label]` (never a silent stage), and returning Err(Command) with an
/// output tail on non-zero exit.
fn run_blocking(label: &str, bin: &str, args: &[&str], cwd: &Path) -> OpsResult<()> {
    let mut cmd = Command::new(bin);
    cmd.args(args).current_dir(cwd);
    let out = run_streaming(&mut cmd, label).map_err(|e| {
        OpsError::Command(format!("failed to spawn {bin} in {}: {e}", cwd.display()))
    })?;
    if out.status.success() {
        return Ok(());
    }
    Err(OpsError::Command(format!(
        "{bin} {args:?} failed in {} (exit {:?}):\n{}",
        cwd.display(),
        out.status.code(),
        tail(&out.captured, 800)
    )))
}

/// [`run_blocking`] for noise-heavy stages (dependency installs, browser
/// downloads): captured but not echoed per-line unless `--verbose`.
fn run_blocking_quiet(label: &str, bin: &str, args: &[&str], cwd: &Path) -> OpsResult<()> {
    let mut cmd = Command::new(bin);
    cmd.args(args).current_dir(cwd);
    let out = run_streaming_quiet(&mut cmd, label).map_err(|e| {
        OpsError::Command(format!("failed to spawn {bin} in {}: {e}", cwd.display()))
    })?;
    if out.status.success() {
        return Ok(());
    }
    Err(OpsError::Command(format!(
        "{bin} {args:?} failed in {} (exit {:?}):\n{}",
        cwd.display(),
        out.status.code(),
        tail(&out.captured, 800)
    )))
}

fn print_log_tail(path: &str) {
    if let Ok(contents) = std::fs::read_to_string(path) {
        let t = tail(&contents, 800);
        if !t.is_empty() {
            output::fail(format!("--- {path} (tail) ---\n{t}"));
        }
    }
}

/// Last `max` chars of `s`, prefixed with a truncation marker (UTF-8 safe).
fn tail(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let skipped = chars.len() - max;
    let rest: String = chars[skipped..].iter().collect();
    format!("…[truncated {skipped} chars]\n{rest}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use codegraph_config::{OpsDatabase, OpsDbTarget, OpsManifest};

    fn manifest_with(graph_binary: Option<&str>) -> OpsManifest {
        OpsManifest {
            app_name: "demo-app".into(),
            graph_binary: graph_binary.map(String::from),
            schemas_dir: Some("schemas".into()),
            mox_files: Vec::new(),
            rosetta_files: Vec::new(),
            classifier: Some("classifier.toml".into()),
            domain_config: None,
            profile: Some("default".into()),
            output_dir: "generated-app".into(),
            ui_dir: None,
            smoke: None,
            api_version: "v1".to_string(),
            servers: codegraph_config::OpsServers {
                api_port: 3000,
                ui_port: 5173,
                bind_addr: "0.0.0.0".into(),
            },
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

    fn config_for(manifest: OpsManifest) -> OpsConfig {
        OpsConfig::from_manifest(manifest, std::path::PathBuf::from("/tmp/repo")).unwrap()
    }

    #[test]
    fn pick_binary_prefers_requested_profile() {
        let dir = tempfile::tempdir().unwrap();
        let rel = dir.path().join("target/release");
        let dbg = dir.path().join("target/debug");
        std::fs::create_dir_all(&rel).unwrap();
        std::fs::create_dir_all(&dbg).unwrap();
        std::fs::write(rel.join("demo-app"), "x").unwrap();
        std::fs::write(dbg.join("demo-app"), "x").unwrap();
        assert_eq!(
            pick_binary(dir.path(), "demo-app", true),
            Some(rel.join("demo-app"))
        );
        assert_eq!(
            pick_binary(dir.path(), "demo-app", false),
            Some(dbg.join("demo-app"))
        );
    }

    #[test]
    fn freshness_precheck_runs_only_with_skip_build() {
        // --skip-build: the binary cannot become fresher later, so the
        // freshness check must fire at stage 0 (before Supabase).
        assert!(freshness_precheck_required(true));
        // Build enabled: the post-build check in e2e_build stays
        // authoritative (building may fix staleness).
        assert!(!freshness_precheck_required(false));
    }

    #[test]
    fn pick_binary_falls_back_to_other_profile() {
        let dir = tempfile::tempdir().unwrap();
        let rel = dir.path().join("target/release");
        std::fs::create_dir_all(&rel).unwrap();
        std::fs::write(rel.join("demo-app"), "x").unwrap();
        assert_eq!(
            pick_binary(dir.path(), "demo-app", false),
            Some(rel.join("demo-app"))
        );
    }

    #[test]
    fn pick_binary_returns_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(pick_binary(dir.path(), "demo-app", true), None);
    }

    #[test]
    fn bind_addr_with_port_appends_or_keeps() {
        let mut manifest = manifest_with(None);
        manifest.servers.bind_addr = "0.0.0.0".into();
        let cfg = config_for(manifest.clone());
        assert_eq!(bind_addr_with_port(&cfg), "0.0.0.0:3000");
        manifest.servers.bind_addr = "127.0.0.1:4444".into();
        let cfg = config_for(manifest);
        assert_eq!(bind_addr_with_port(&cfg), "127.0.0.1:4444");
    }

    #[test]
    fn supabase_health_url_uses_manifest_or_default() {
        let cfg = config_for(manifest_with(None));
        assert_eq!(
            supabase_health_url(&cfg),
            "http://localhost:54321/auth/v1/health"
        );
        let mut manifest = manifest_with(None);
        manifest.supabase = Some(codegraph_config::OpsSupabase {
            dir: "supabase".into(),
            health_url: Some("http://localhost:54321/auth/v1/health".into()),
            anon_key: None,
            service_key: None,
            jwt_secret: None,
        });
        let cfg = config_for(manifest);
        assert_eq!(
            supabase_health_url(&cfg),
            "http://localhost:54321/auth/v1/health"
        );
    }

    #[test]
    fn generate_args_include_only_some_flags() {
        let cfg = config_for(manifest_with(Some("hr-graph")));
        let args = generate_args(&cfg);
        assert_eq!(args[0], "run");
        assert!(args.contains(&"--schemas".to_string()));
        assert!(args.contains(&"--classifier".to_string()));
        assert!(args.contains(&"--profile".to_string()));
        assert!(!args.contains(&"--config".to_string()));
        assert!(args.contains(&"--output".to_string()));
        assert!(args.contains(&"/tmp/repo/generated-app".to_string()));
    }

    #[test]
    fn generate_args_mox_files_only_exact_vector() {
        let mut manifest = manifest_with(Some("hr-graph"));
        manifest.schemas_dir = None;
        // classifier must be ignored in mox mode — the mox pipeline needs none.
        manifest.classifier = Some("classifier.toml".into());
        manifest.domain_config = Some("domains.toml".into());
        manifest.profile = Some("default".into());
        manifest.mox_files = vec!["model/common.mox".into(), "model/billing.mox".into()];
        let cfg = config_for(manifest);
        assert_eq!(
            generate_args(&cfg),
            vec![
                "run",
                "--mox-files",
                "model/common.mox",
                "--mox-files",
                "model/billing.mox",
                "--config",
                "domains.toml",
                "--profile",
                "default",
                "--output",
                "/tmp/repo/generated-app",
            ]
        );
    }

    #[test]
    fn generate_args_mox_files_win_over_schemas_dir() {
        let mut manifest = manifest_with(Some("hr-graph"));
        manifest.mox_files = vec!["model/app.mox".into()];
        let cfg = config_for(manifest);
        let args = generate_args(&cfg);
        assert!(args.contains(&"--mox-files".to_string()));
        assert!(
            !args.contains(&"--schemas".to_string()),
            "mox_files must win: {args:?}"
        );
        assert!(
            !args.contains(&"--classifier".to_string()),
            "mox mode passes no --classifier: {args:?}"
        );
    }

    #[test]
    fn generation_gate_proceeds_on_schemas_only() {
        let cfg = config_for(manifest_with(Some("hr-graph")));
        assert_eq!(generation_binary(&cfg).as_deref(), Some("hr-graph"));
    }

    #[test]
    fn generation_gate_proceeds_on_mox_files_without_schemas_dir() {
        let mut manifest = manifest_with(Some("hr-graph"));
        manifest.schemas_dir = None;
        manifest.mox_files = vec!["model/app.mox".into()];
        let cfg = config_for(manifest);
        assert_eq!(generation_binary(&cfg).as_deref(), Some("hr-graph"));
    }

    #[test]
    fn generation_gate_skips_without_any_model_source() {
        let mut manifest = manifest_with(Some("hr-graph"));
        manifest.schemas_dir = None;
        let cfg = config_for(manifest);
        assert!(generation_binary(&cfg).is_none());
        // And without a graph binary there is nothing to run either.
        let cfg = config_for(manifest_with(None));
        assert!(generation_binary(&cfg).is_none());
    }

    #[test]
    fn generation_outputs_detects_non_empty_dirs() {
        let cfg = config_for(manifest_with(None));
        assert!(!generation_outputs(&cfg));
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("generated-app/src");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("main.rs"), "fn main() {}").unwrap();
        let cfg = OpsConfig::from_manifest(manifest_with(None), dir.path().to_path_buf()).unwrap();
        assert!(generation_outputs(&cfg));
    }

    #[test]
    fn tallies_parse_playwright_list_output() {
        let sample = "\
Running 5 tests using 2 workers

  ✓  1 [chromium] › tests/generated/candidate.spec.ts:14:5 › create (1.2s)
  ✓  2 [chromium] › tests/generated/candidate.spec.ts:30:5 › list (0.9s)
  ✘  3 [firefox] › tests/generated/candidate.spec.ts:14:5 › create (2.0s)
  ✓  4 [firefox] › tests/generated/candidate.spec.ts:30:5 › list (0.8s)
  -    5 skipped (not counted)

  ✘  6 [webkit] › tests/generated/other.spec.ts:5:3 › broken (0.3s)

1 failed
    1) [chromium] › tests/generated/candidate.spec.ts:14:5 › create › error text
";
        let tallies = tally_playwright_projects(sample);
        assert_eq!(
            tallies,
            vec![
                ProjectTally {
                    project: "chromium".into(),
                    passed: 2,
                    failed: 0
                },
                ProjectTally {
                    project: "firefox".into(),
                    passed: 1,
                    failed: 1
                },
                ProjectTally {
                    project: "webkit".into(),
                    passed: 0,
                    failed: 1
                },
            ]
        );
    }

    #[test]
    fn tallies_tolerate_missing_run_index_and_alt_fail_marker() {
        // Some Playwright versions print `✓ [project] › ...` without the
        // index, and `✘` as the failure marker.
        let sample = "\
  ✓ [chromium] › a.spec.ts:1:1 › ok
  ✘ [chromium] › b.spec.ts:2:2 › bad
";
        let tallies = tally_playwright_projects(sample);
        assert_eq!(tallies.len(), 1);
        assert_eq!(tallies[0].project, "chromium");
        assert_eq!(tallies[0].passed, 1);
        assert_eq!(tallies[0].failed, 1);
    }

    #[test]
    fn tallies_ignore_non_result_lines() {
        assert!(tally_playwright_projects("").is_empty());
        assert!(tally_playwright_projects("2 passed (5.1s)\n1 failed").is_empty());
        assert!(tally_playwright_projects("  ➤ SRV log line").is_empty());
        // `✓` without the `[project] ›` shape is not a result line.
        assert!(tally_playwright_projects("✓ all good").is_empty());
    }

    #[test]
    fn failed_titles_strip_file_location_and_duration() {
        let sample = "\
  ✓  1 [crud] › tests/generated/a.spec.ts:14:5 › Owner CRUD › create (1.2s)
  ✘  2 [crud] › tests/generated/x.owner.crud.test.ts:95:3 › Owner CRUD › owner can edit X (453ms)
  ✘  3 [api] › tests/generated/y.api.crud.test.ts:184:3 › detail page shows all fields (16.4s)
";
        assert_eq!(
            failed_test_titles(sample),
            vec![
                "Owner CRUD › owner can edit X",
                "detail page shows all fields",
            ]
        );
    }

    #[test]
    fn failed_titles_dedupe_and_ignore_non_failures() {
        let sample = "\
  ✘ [crud] › tests/a.spec.ts:1:1 › duplicated title (1s)
  ✘ [crud] › tests/a.spec.ts:1:1 › duplicated title (1s)
2 passed (5.1s)
1 failed
  ✘ no project bracket here
";
        assert_eq!(failed_test_titles(sample), vec!["duplicated title"]);
    }
}
