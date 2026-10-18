//! Playwright UI runner (port of hr-platform/test-ui.sh, genericised).
//! Prefers `vite preview` over the dev server for speed, per the e2e bash
//! script.

use std::path::Path;
use std::process::Command;

use crate::config::OpsConfig;
use crate::error::{OpsError, OpsResult};
use crate::output;
use crate::proc::{ManagedProcess, Supervisor, run_streaming, run_streaming_quiet};
use crate::wait::wait_for_url;

/// Where the UI suite persists the provisioned API key (shared with the cli
/// and e2e suites).
pub const API_KEY_FILE: &str = "/tmp/codegraph-ops-api-key";
/// Log for the SvelteKit preview server.
const SVELTEKIT_LOG: &str = "/tmp/codegraph-ops-sveltekit.log";

/// Arguments for the Playwright UI runner.
pub struct UiArgs {
    pub keep: bool,
    pub headed: bool,
    /// Reuse a registry-known preview server already running on the UI port
    /// instead of taking it over (documented caveat: possibly stale bundle).
    pub reuse: bool,
    pub playwright_args: Vec<String>,
}

/// Start the SvelteKit preview server on `{ui_port}` serving `config.ui_dir`
/// and run Playwright. Requires API running (caller checks). If
/// capabilities.has_ui is false: warn + Ok.
pub async fn run_ui(config: &OpsConfig, args: &UiArgs) -> OpsResult<()> {
    output::section("=== UI E2E (Playwright) ===");

    if !config.manifest.capabilities.has_ui {
        output::warn("UI capability not generated — skipping UI suite");
        return Ok(());
    }

    // 1. API reachable.
    let api_url = config.api_url();
    if !http_ok(&format!("{api_url}/health")).await {
        return Err(OpsError::TestFailure(format!(
            "API server not running at {api_url}/health — run 'api --keep' first"
        )));
    }
    output::ok("API server reachable");

    // 2. Generated UI exists.
    if !config.ui_dir.is_dir() {
        return Err(OpsError::TestFailure(format!(
            "generated UI not found at {} — run generation first",
            config.ui_dir.display()
        )));
    }

    // Port preflight: the preview server binds the UI port below (the API
    // port is intentionally occupied — the running API is this suite's
    // precondition). A stale process holding the port must fail fast;
    // a registry-known prior preview is taken over by default or reused
    // with `--reuse`.
    let port_state = crate::preflight::ensure_port_available(
        &config.root_dir,
        config.manifest.servers.ui_port,
        args.reuse,
    )?;

    // 3. Install dependencies if needed (best-effort).
    if !config.ui_dir.join("node_modules").is_dir() {
        output::info("Installing UI dependencies (pnpm install)...");
        if let Err(e) = run_quiet("pnpm", &["install"], &config.ui_dir) {
            output::warn(format!("pnpm install failed (continuing): {e}"));
        }
    }

    // 4. Build production bundle when missing (vite preview serves `dist`).
    if !config.ui_dir.join("dist").is_dir() {
        output::info("No dist/ found — building production bundle (best-effort)...");
        if let Err(e) = run_quiet("pnpm", &["run", "build"], &config.ui_dir) {
            output::warn(format!("pnpm run build failed (continuing): {e}"));
        }
    }

    // 5. Read or provision the API key.
    let api_key = read_or_provision_api_key(config).await?;
    let api_key = match api_key {
        Some(k) => {
            output::ok("API key available");
            k
        }
        None => {
            output::warn("no API key available — auth-dependent tests will fail");
            String::new()
        }
    };

    // 6. Start SvelteKit preview server.
    let mut supervisor = Supervisor::new(args.keep);
    let ui_url = config.ui_url();
    if port_state.reused() {
        output::warn(
            "--reuse: not booting vite preview — the running registry service keeps serving \
             (it may serve a stale bundle)",
        );
    } else {
        let mut cmd = Command::new("pnpm");
        cmd.arg("exec")
            .arg("vite")
            .arg("preview")
            .arg("--port")
            .arg(config.manifest.servers.ui_port.to_string());
        cmd.current_dir(&config.ui_dir);
        cmd.env("PUBLIC_API_URL", &api_url);
        cmd.env("PUBLIC_API_KEY", &api_key);
        // The generated (app) layout creates a Supabase SSR client on every
        // navigation — without these it throws "Your project's URL and Key
        // are required" and every page renders the 500 boundary (issue
        // #468: 1,199 measured failures traced to exactly this).
        cmd.env("PUBLIC_SUPABASE_URL", supabase_base_url(config));
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
            "ui",
            None,
        ));
        if let Err(e) = wait_for_url(&ui_url, 45, "SvelteKit").await {
            print_log_tail(SVELTEKIT_LOG);
            return Err(e);
        }
        // Health OK → record; removed again on shutdown, left behind on --keep.
        proc.record_service();
        supervisor.add(proc);
    }
    output::ok(format!("SvelteKit preview ready at {ui_url}"));

    // 7. Install the Playwright browser (best-effort).
    if let Err(e) = run_quiet(
        "npx",
        &["playwright", "install", "chromium"],
        &config.ui_dir,
    ) {
        output::warn(format!(
            "playwright install chromium failed (continuing): {e}"
        ));
    }
    // Stale transpile-cache hygiene (whole-dir clear — see pwcache for the
    // scoping decision): Playwright once executed OLD transpiled specs after
    // a regeneration.
    crate::pwcache::clear_and_report();

    // 8. Run the Playwright suite.
    output::section("Playwright tests");
    let mut cmd = Command::new("npx");
    cmd.arg("playwright").arg("test");
    if args.headed {
        cmd.arg("--headed");
    }
    cmd.args(&args.playwright_args);
    for (key, value) in playwright_env(config, Some(&api_key)) {
        cmd.env(key, value);
    }
    cmd.current_dir(&config.ui_dir);
    // Streamed with uniform tails (previously raw `.status()` passthrough).
    let out = run_streaming(&mut cmd, "playwright")
        .map_err(|e| OpsError::Command(format!("failed to spawn playwright: {e}")))?;
    let passed = out.status.success();

    // 9. Summary + shutdown.
    output::section("=== UI E2E Summary ===");
    if passed {
        output::ok("All Playwright tests passed");
    } else {
        output::fail("Some Playwright tests failed");
    }
    supervisor.shutdown_all().await;
    if passed {
        Ok(())
    } else {
        Err(OpsError::TestFailure(
            "Playwright UI suite failed".to_string(),
        ))
    }
}

/// Assemble the Playwright test environment: PUBLIC_API_URL/PUBLIC_API_KEY,
/// SUPABASE_* (from manifest or local defaults), DATABASE_URL (e2e target),
/// PUBLIC_SVELTEKIT_URL.
pub fn playwright_env(config: &OpsConfig, api_key: Option<&str>) -> Vec<(String, String)> {
    let mut env = vec![
        ("PUBLIC_API_URL".to_string(), config.api_url()),
        (
            "PUBLIC_API_KEY".to_string(),
            api_key.unwrap_or("").to_string(),
        ),
        ("SUPABASE_URL".to_string(), supabase_base_url(config)),
        ("SUPABASE_ANON_KEY".to_string(), config.anon_key.clone()),
        (
            "SUPABASE_SERVICE_ROLE_KEY".to_string(),
            config.service_key.clone(),
        ),
    ];
    if let Some(db) = &config.e2e_db {
        env.push(("DATABASE_URL".to_string(), db.url()));
    }
    env.push(("PUBLIC_SVELTEKIT_URL".to_string(), config.ui_url()));
    env
}

/// Base Supabase URL for the local stack: the manifest health URL with any
/// trailing `/auth/v1/health` path stripped, or the local default.
pub fn supabase_base_url(config: &OpsConfig) -> String {
    match config
        .manifest
        .supabase
        .as_ref()
        .and_then(|s| s.health_url.clone())
    {
        Some(url) => url
            .trim_end_matches("/auth/v1/health")
            .trim_end_matches('/')
            .to_string(),
        None => "http://localhost:54321".to_string(),
    }
}

/// Read the API key from the shared key file, or provision a fresh one via
/// `public.create_api_key()` against the e2e (preferred) or api database
/// target, persisting it to the shared file for reuse. `Ok(None)` when
/// neither the file nor a usable key function is available.
pub async fn read_or_provision_api_key(config: &OpsConfig) -> OpsResult<Option<String>> {
    let target = config.e2e_db.as_ref().unwrap_or(&config.api_db);
    if let Ok(contents) = std::fs::read_to_string(API_KEY_FILE) {
        let key = contents.trim().to_string();
        if !key.is_empty() {
            // A db reset wipes api_keys, so a persisted key can be stale.
            // Verify it against the live database before trusting it; if
            // invalid (or unverifiable because the function is absent),
            // fall through to provisioning a fresh one.
            let verify = crate::db::psql_query(
                target,
                &format!(
                    "SELECT coalesce(public.verify_api_key('{key}') ->> 'valid', 'false') AS valid;"
                ),
            )
            .await;
            match verify {
                Ok(raw) if raw.contains("true") => {
                    output::ok(format!("Read API key from {API_KEY_FILE} (verified)"));
                    return Ok(Some(key));
                }
                Ok(_) => {
                    output::warn("persisted API key is stale (db was reset) — re-provisioning");
                }
                Err(_) => {
                    // verify_api_key() absent: keep the historical behavior.
                    output::ok(format!("Read API key from {API_KEY_FILE}"));
                    return Ok(Some(key));
                }
            }
        }
    }
    let present = match crate::db::psql_query(
        target,
        "SELECT count(*) FROM pg_proc WHERE proname = 'create_api_key';",
    )
    .await
    {
        Ok(count) => count.trim().parse::<u64>().unwrap_or(0) > 0,
        Err(e) => {
            output::warn(format!(
                "could not check for create_api_key() ({e}) — leaving API key unset"
            ));
            return Ok(None);
        }
    };
    if !present {
        output::warn("create_api_key() not found — leaving API key unset");
        return Ok(None);
    }
    const CREATE_KEY_SQL: &str = "SELECT public.create_api_key(\
        '00000000-0000-0000-0000-000000000001'::uuid, 'ops-e2e-key', \
        '[{\"entity_type\":\"*\",\"entity_id\":\"*\",\"action\":\"*\"}]'::jsonb);";
    match crate::db::psql_query(target, CREATE_KEY_SQL).await {
        Ok(raw) => match extract_key_from_jsonb(&raw) {
            Some(key) => {
                if let Err(e) = std::fs::write(API_KEY_FILE, &key) {
                    output::warn(format!("could not persist API key: {e}"));
                }
                output::ok("Provisioned API key via create_api_key()");
                Ok(Some(key))
            }
            None => {
                output::warn("create_api_key() returned no key field — API key unset");
                Ok(None)
            }
        },
        Err(e) => {
            output::warn(format!("create_api_key() failed ({e}) — API key unset"));
            Ok(None)
        }
    }
}

/// Extract an API key from a `create_api_key()` JSON result: tries `key`
/// then `api_key` fields, then falls back to a bare JWT-looking string.
fn extract_key_from_jsonb(raw: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw.trim()) {
        if let Some(key) = value.get("key").and_then(|k| k.as_str()) {
            return Some(key.to_string());
        }
        if let Some(key) = value.get("api_key").and_then(|k| k.as_str()) {
            return Some(key.to_string());
        }
    }
    let trimmed = raw.trim();
    if trimmed.starts_with("eyJ") && !trimmed.contains(' ') {
        return Some(trimmed.to_string());
    }
    None
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

/// Run a command, streaming its output (quiet: capture only unless
/// `--verbose`) and returning Err(Command) with a tail on failure (used for
/// best-effort steps where callers decide).
fn run_quiet(bin: &str, args: &[&str], cwd: &Path) -> OpsResult<()> {
    let mut cmd = Command::new(bin);
    cmd.args(args).current_dir(cwd);
    let out = run_streaming_quiet(&mut cmd, bin)?;
    if out.status.success() {
        return Ok(());
    }
    Err(OpsError::Command(format!(
        "{bin} {args:?} failed: {}",
        tail(&out.captured, 400)
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
    use codegraph_config::{OpsDatabase, OpsDbTarget, OpsManifest, OpsSupabase};

    fn manifest_with(ui_port: u16, supabase: Option<OpsSupabase>, e2e: bool) -> OpsManifest {
        OpsManifest {
            app_name: "demo-app".into(),
            graph_binary: None,
            schemas_dir: None,
            mox_files: Vec::new(),
            rosetta_files: Vec::new(),
            classifier: None,
            domain_config: None,
            profile: None,
            output_dir: "generated-app".into(),
            ui_dir: None,
            smoke: None,
            api_version: "v1".to_string(),
            servers: codegraph_config::OpsServers {
                api_port: 3000,
                ui_port,
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
                e2e: e2e.then(|| OpsDbTarget {
                    host: "localhost".into(),
                    port: 54322,
                    user: "postgres".into(),
                    password: "postgres".into(),
                    database: "postgres".into(),
                    reset_sql: None,
                    seed_sql: None,
                    grant_role: None,
                    grant_strict: None,
                }),
                e2e_app: None,
            },
            supabase,
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
    fn playwright_env_includes_expected_vars() {
        let supabase = OpsSupabase {
            dir: "supabase".into(),
            health_url: Some("http://localhost:54321/auth/v1/health".into()),
            anon_key: Some("anon".into()),
            service_key: Some("service".into()),
            jwt_secret: Some("secret".into()),
        };
        let manifest = manifest_with(5173, Some(supabase), true);
        let cfg = config_for(manifest);
        let env = playwright_env(&cfg, Some("key-123"));
        let map: std::collections::HashMap<String, String> = env.into_iter().collect();
        assert_eq!(map.get("PUBLIC_API_URL").unwrap(), "http://localhost:3000");
        assert_eq!(map.get("PUBLIC_API_KEY").unwrap(), "key-123");
        assert_eq!(map.get("SUPABASE_URL").unwrap(), "http://localhost:54321");
        assert_eq!(map.get("SUPABASE_ANON_KEY").unwrap(), "anon");
        assert_eq!(map.get("SUPABASE_SERVICE_ROLE_KEY").unwrap(), "service");
        assert_eq!(
            map.get("DATABASE_URL").unwrap(),
            "postgres://postgres:postgres@localhost:54322/postgres"
        );
        assert_eq!(
            map.get("PUBLIC_SVELTEKIT_URL").unwrap(),
            "http://localhost:5173"
        );
    }

    #[test]
    fn playwright_env_without_key_or_e2e_db() {
        let cfg = config_for(manifest_with(5174, None, false));
        let env = playwright_env(&cfg, None);
        let map: std::collections::HashMap<String, String> = env.into_iter().collect();
        assert_eq!(map.get("PUBLIC_API_KEY").unwrap(), "");
        assert!(!map.contains_key("DATABASE_URL"));
        assert_eq!(map.get("SUPABASE_URL").unwrap(), "http://localhost:54321");
    }

    #[test]
    fn supabase_base_url_strips_health_path() {
        let supabase = OpsSupabase {
            dir: "supabase".into(),
            health_url: Some("http://localhost:54321/auth/v1/health".into()),
            anon_key: None,
            service_key: None,
            jwt_secret: None,
        };
        let cfg = config_for(manifest_with(5173, Some(supabase), false));
        assert_eq!(supabase_base_url(&cfg), "http://localhost:54321");
    }

    #[test]
    fn supabase_base_url_defaults_when_unset() {
        let cfg = config_for(manifest_with(5173, None, false));
        assert_eq!(supabase_base_url(&cfg), "http://localhost:54321");
    }

    #[test]
    fn extract_key_from_jsonb_variants() {
        assert_eq!(
            extract_key_from_jsonb(r#"{"key": "k1", "api_key": "k2"}"#),
            Some("k1".to_string())
        );
        assert_eq!(
            extract_key_from_jsonb(r#"{"api_key": "k2"}"#),
            Some("k2".to_string())
        );
        assert_eq!(
            extract_key_from_jsonb("eyJhbGciOiJIUzI1NiJ9.abc.def"),
            Some("eyJhbGciOiJIUzI1NiJ9.abc.def".to_string())
        );
        assert_eq!(extract_key_from_jsonb("garbage"), None);
        assert_eq!(extract_key_from_jsonb(r#"{"nope": 1}"#), None);
    }
}
