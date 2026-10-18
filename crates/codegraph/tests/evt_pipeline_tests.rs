//! End-to-end `.evt` event-contract generation (issues #454/#455 Phase 5):
//! the committed orders fixture (rexlang conformance, verbatim) driven
//! through `driver::run` (mox + evt) with assertions on the generated
//! typed events plane, run-to-run determinism, and the flag-off contract
//! (no `.evt` input ⇒ no events plane and every common file byte-identical
//! modulo the explicitly gated files).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Minimal view of `.codegraph-manifest.json` (codegraph-generate's
/// `Manifest`) for the flag-off delta assertion.
#[derive(serde::Deserialize)]
struct CodegraphManifest {
    generated: Vec<String>,
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/evt")
        .join(name)
}

/// One domain for the orders mox: the package `nz.example.orders` resolves
/// to the domain key by its last dot-segment (the mox `resolve_domain`
/// chain).
const ORDERS_DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.orders]
label = "Orders"
schema_dir = "orders"
postgres_schema = "orders"
"#;

/// One pipeline run: `driver::run` in the no-plan (all generators)
/// backward-compat mode, mox-first, with optional `.evt` contracts. The
/// `.evt` imports `orders.mox` from beside itself, so both ride the run
/// (the fixture directory is read-only input; output goes to `output`).
async fn run_pipeline(config: &Path, output: &Path, evt_files: &[PathBuf]) {
    let mox_files = vec![fixture("orders.mox")];
    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: config,
        output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        // Nonexistent path ⇒ the no-plan (all generators) backward-compat
        // path, the same mode the mox/ddd pipeline tests use.
        profiles_config_path: Some(PathBuf::from("definitely-absent-profiles.toml")),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &mox_files,
        rosetta_files: &[],
        ddd_files: &[],
        evt_files,
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
}

/// Recursively collect `dir` into a `relative path → content` map.
fn collect_files(root: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    collect_into(root, root, &mut files);
    files
}

fn collect_into(root: &Path, dir: &Path, files: &mut BTreeMap<String, String>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            collect_into(root, &path, files);
        } else if let (Ok(rel), Ok(content)) = (
            path.strip_prefix(root)
                .map(|p| p.to_string_lossy().to_string()),
            fs::read_to_string(&path),
        ) {
            files.insert(rel.replace('\\', "/"), content);
        }
    }
}

fn read_out(files: &BTreeMap<String, String>, rel: &str) -> String {
    files
        .get(rel)
        .unwrap_or_else(|| {
            panic!(
                "{rel} missing from output (have: {:?})",
                files
                    .keys()
                    .filter(|k| {
                        k.contains("orders") || k.contains("events") || k.contains("webhook")
                    })
                    .take(80)
                    .collect::<Vec<_>>()
            )
        })
        .clone()
}

/// The one file whose path ends with `suffix` (migration files carry
/// sequence-number prefixes).
fn read_out_ending<'a>(files: &'a BTreeMap<String, String>, suffix: &str) -> &'a str {
    let matches: Vec<&String> = files.keys().filter(|k| k.ends_with(suffix)).collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one file ending with {suffix}, got {matches:?}"
    );
    files[matches[0]].as_str()
}

/// A standard fixture root: tempdir with domains.toml; returns
/// (root, config path).
fn fixture_root() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("domains.toml"), ORDERS_DOMAINS_TOML).unwrap();
    let config = root.path().join("domains.toml");
    (root, config)
}

// ── the typed events plane ──────────────────────────────────────────────

#[tokio::test]
async fn evt_input_generates_the_typed_events_plane() {
    let (root, config) = fixture_root();
    let output = root.path().join("generated");
    let evt = vec![fixture("orders.evt")];
    run_pipeline(&config, &output, &evt).await;
    let files = collect_files(&output);

    // mod.rs — the module declarations over the four-file plane.
    let events_mod = read_out(&files, "src/events/mod.rs");
    assert!(events_mod.contains("pub mod consumers;"), "{events_mod}");
    assert!(events_mod.contains("pub mod contracts;"));
    assert!(events_mod.contains("pub mod emit;"));

    // contracts.rs — one payload struct per declared event with the
    // mapped field types (rex primitives, the enum-resolved codelist
    // type) and the pinned version consts.
    let contracts = read_out(&files, "src/events/contracts.rs");
    assert!(
        contracts.contains("pub struct OrderPlacedPayload"),
        "{contracts}"
    );
    assert!(contracts.contains("pub order_id: String,"));
    assert!(
        contracts.contains("pub total: i64,"),
        "rex `int` maps to i64"
    );
    assert!(
        contracts.contains("pub status: OrderStatus,"),
        "the enum-resolved field keeps the codelist's rust type"
    );
    assert!(contracts.contains("pub const ORDER_PLACED_VERSION: &str = \"1.0.0\";"));
    assert!(contracts.contains("pub struct OrderCancelledPayload"));
    assert!(contracts.contains("pub reason: String,"));
    assert!(contracts.contains("pub const ORDER_CANCELLED_VERSION: &str = \"1.1.0\";"));
    assert!(
        contracts.contains("use crate::codelist::OrderStatus;"),
        "enum-resolved fields import from the codelist module"
    );

    // emit.rs — one event enum + publish fn per bound channel, the pgmq
    // send onto the channel queue, and the full superset envelope
    // (`event_type` mirrors the declared event name so webhook
    // subscription matching is unchanged).
    let emit = read_out(&files, "src/events/emit.rs");
    assert!(emit.contains("pub enum OrdersEvent"), "{emit}");
    assert!(emit.contains("OrderPlaced(OrderPlacedPayload),"));
    assert!(emit.contains("OrderCancelled(OrderCancelledPayload),"));
    assert!(emit.contains("pub async fn publish_orders("));
    assert!(
        emit.contains("pgmq.send('events_orders'"),
        "the publish statement targets the channel queue: {emit}"
    );
    for key in [
        "\"event\": event.event_name(),",
        "\"version\": event.event_version(),",
        "\"channel\": \"orders\",",
        "\"payload\": event.payload(),",
        "\"event_type\": event.event_name(),",
        "\"domain\": event.domain(),",
        "\"entity_table\"",
        "\"entity_id\"",
        "\"tenant_id\"",
        "\"occurred_at\"",
        "\"correlation_id\"",
    ] {
        assert!(emit.contains(key), "envelope key {key} missing: {emit}");
    }

    // consumers.rs — one handler trait + dispatcher per subscription and
    // the process-wide registry the webhook drain dispatches through.
    let consumers = read_out(&files, "src/events/consumers.rs");
    assert!(
        consumers.contains("pub struct BillingServiceContext<'a>"),
        "{consumers}"
    );
    assert!(consumers.contains("pub trait BillingServiceHandler"));
    assert!(consumers.contains("async fn on_order_placed("));
    assert!(consumers.contains("async fn on_order_cancelled("));
    assert!(consumers.contains("pub struct AnalyticsServiceContext<'a>"));
    assert!(consumers.contains("pub trait AnalyticsServiceHandler"));
    assert!(consumers.contains("pub fn register_billing_service("));
    assert!(
        consumers.contains("pub(crate) async fn dispatch_registered(\n    db: DatabaseConnection,"),
        "the drain's entry point"
    );

    // Scaffold wiring (`has_events` → `pub mod events;` in lib.rs and the
    // gated `anyhow` in Cargo.toml) is pinned at TEMPLATE level by the
    // in-crate golden (`scaffold_templates_render_events_module_when_has_
    // events`); at pipeline level the plan-less run leaves `domain_types_
    // base` unset, so the domain-types scaffold overwrites both output-root
    // paths with its own lib.rs/Cargo.toml — pre-existing no-plan layout
    // behavior for the whole app-scaffold surface, not an events-plane
    // property. The pipeline-level module declaration is the post-generation
    // mod index instead.
    let mod_index = read_out(&files, "src/mod.rs");
    assert!(mod_index.contains("pub mod events;"), "{mod_index}");

    // pgmq setup migration: the `events_orders` queue with the
    // app_user/api_key grants. The channel `orders` and the domain
    // `orders` collide on one queue name — the collapse contract means
    // the DOMAIN migration band owns its creation, exactly once.
    let pgmq = read_out(&files, "migrations/0003_pgmq_setup.sql");
    assert!(pgmq.contains("pgmq.create('events_orders')"), "{pgmq}");
    assert_eq!(
        pgmq.matches("pgmq.create").count(),
        1,
        "channel/domain collision collapses to one creation: {pgmq}"
    );
    assert!(pgmq.contains("GRANT ALL ON TABLE pgmq.q_events_orders TO app_user, api_key;"));

    // The Database(Trigger) plane is untouched: the order table's DDL and
    // its per-table domain-event trigger still emit (sequence-prefixed
    // migration names).
    let table = read_out_ending(&files, "_orders_order.sql");
    assert!(table.contains("CREATE TABLE"), "{table}");
    let trigger = read_out_ending(&files, "_orders_order_event_trigger.sql");
    assert!(trigger.contains("trg_order_domain_event"), "{trigger}");
    assert!(trigger.contains("EXECUTE FUNCTION emit_domain_event('orders', 'order')"));

    // Webhook dispatch drain: the typed-envelope branch plus the
    // dispatch_registered delete condition.
    let dispatch = read_out(&files, "src/webhook_dispatch.rs");
    assert!(dispatch.contains("is_typed_envelope"), "{dispatch}");
    assert!(
        dispatch.contains("dispatch_registered(\n                            self.db.clone(),"),
        "the delete condition extends to consumer dispatches"
    );

    // Webhook subscription API: vocabulary validation over the declared
    // events ∪ the CRUD verbs (declared names sorted).
    let api = read_out(&files, "src/webhook_api.rs");
    assert!(
        api.contains("DECLARED.contains(&event_type.as_str())"),
        "{api}"
    );
    assert!(
        api.contains("\"OrderCancelled\", \"OrderPlaced\", \"created\", \"updated\", \"deleted\""),
        "the vocabulary lists the declared events and the CRUD verbs"
    );
    assert!(api.contains("unknown event_type"));
}

// ── determinism ─────────────────────────────────────────────────────────

#[tokio::test]
async fn evt_generation_is_deterministic() {
    let (root_a, config_a) = fixture_root();
    let (root_b, config_b) = fixture_root();
    let out_a = root_a.path().join("generated");
    let out_b = root_b.path().join("generated");
    let evt = vec![fixture("orders.evt")];
    run_pipeline(&config_a, &out_a, &evt).await;
    run_pipeline(&config_b, &out_b, &evt).await;

    let files_a = collect_files(&out_a);
    let files_b = collect_files(&out_b);
    assert_eq!(
        files_a.keys().collect::<Vec<_>>(),
        files_b.keys().collect::<Vec<_>>(),
        "file sets must agree across runs"
    );
    for (name, content_a) in &files_a {
        assert_eq!(
            content_a, &files_b[name],
            "{name} differs between two runs of identical inputs"
        );
    }
}

// ── flag-off contract ───────────────────────────────────────────────────

/// The same fixture WITHOUT `.evt` input: no events plane anywhere, and
/// the two trees agree byte-for-byte on every common file EXCEPT the two
/// explicitly gated webhook files (`src/webhook_dispatch.rs` +
/// `src/webhook_api.rs` via their typed branches; the scaffold's
/// lib.rs/Cargo.toml gating is template-pinned — the plan-less run's
/// domain-types scaffold overwrites both output-root paths identically in
/// either run). Notably `migrations/0003_pgmq_setup.sql` is byte-identical:
/// the channel/domain queue-name collision means the domain band owns
/// `events_orders` in both runs.
#[tokio::test]
async fn no_evt_input_generates_no_events_plane() {
    let (root_evt, config_evt) = fixture_root();
    let (root_off, config_off) = fixture_root();
    let out_evt = root_evt.path().join("generated");
    let out_off = root_off.path().join("generated");
    let evt = vec![fixture("orders.evt")];
    run_pipeline(&config_evt, &out_evt, &evt).await;
    run_pipeline(&config_off, &out_off, &[]).await;

    let files_evt = collect_files(&out_evt);
    let files_off = collect_files(&out_off);

    // Absence set: the flag-off tree has no events plane at all — neither
    // the generated module nor the scaffold's lib declaration (identical
    // domain-types lib.rs in either run; see the typed-plane test).
    assert!(
        !files_off.keys().any(|k| k.starts_with("src/events/")),
        "no src/events/ without .evt input: {:?}",
        files_off
            .keys()
            .filter(|k| k.contains("events"))
            .collect::<Vec<_>>()
    );
    let lib_off = read_out(&files_off, "src/lib.rs");
    let lib_evt = read_out(&files_evt, "src/lib.rs");
    assert_eq!(
        lib_off, lib_evt,
        "the overwritten output-root lib.rs is identical in both runs"
    );

    // The domain trigger queue survives flag-off (the per-domain band is
    // the trigger plane's infrastructure, not an .evt feature) — and the
    // channel band adds nothing (empty here by the collision collapse).
    let pgmq_off = read_out(&files_off, "migrations/0003_pgmq_setup.sql");
    assert!(
        pgmq_off.contains("pgmq.create('events_orders')"),
        "{pgmq_off}"
    );
    assert_eq!(pgmq_off.matches("pgmq.create").count(), 1);
    assert_eq!(
        pgmq_off, files_evt["migrations/0003_pgmq_setup.sql"],
        "0003 must be byte-identical between the runs"
    );

    // Webhook templates render their pre-#455 branches.
    let dispatch_off = read_out(&files_off, "src/webhook_dispatch.rs");
    assert!(
        !dispatch_off.contains("is_typed_envelope"),
        "{dispatch_off}"
    );
    assert!(!dispatch_off.contains("dispatch_registered"));
    let api_off = read_out(&files_off, "src/webhook_api.rs");
    assert!(!api_off.contains("DECLARED"), "{api_off}");
    assert!(!api_off.contains("unknown event_type"));

    // The gated files exist in BOTH trees and differ exactly by their
    // events-plane content (the evt side's markers are pinned by the
    // typed-events-plane test above; here only the delta shape).
    const GATED: [&str; 4] = [
        "src/webhook_dispatch.rs",
        "src/webhook_api.rs",
        // The post-generation module index: gains `pub mod events;` with
        // the events plane (the pipeline-level declaration in this
        // plan-less mode — see the typed-plane test's lib.rs note).
        "src/mod.rs",
        // The binary entry point declares `mod events;` alongside its
        // webhook modules when the events plane is present — the gated
        // `webhook_dispatch.rs` references `crate::events::…`.
        "src/main.rs",
    ];
    for rel in GATED {
        assert!(
            files_evt.contains_key(rel) && files_off.contains_key(rel),
            "{rel} must exist in both trees"
        );
        assert_ne!(
            files_evt[rel], files_off[rel],
            "{rel} is events-gated and must differ between the runs"
        );
    }
    let mod_index_evt = read_out(&files_evt, "src/mod.rs");
    let mod_index_off = read_out(&files_off, "src/mod.rs");
    assert!(mod_index_evt.contains("pub mod events;"), "{mod_index_evt}");
    assert!(
        !mod_index_off.contains("pub mod events;"),
        "{mod_index_off}"
    );

    // The evt-only files are EXACTLY the events plane.
    let evt_only: Vec<&String> = files_evt
        .keys()
        .filter(|k| !files_off.contains_key(*k))
        .collect();
    assert_eq!(
        evt_only,
        [
            "src/events/consumers.rs",
            "src/events/contracts.rs",
            "src/events/emit.rs",
            "src/events/mod.rs",
        ]
        .iter()
        .collect::<Vec<_>>(),
        "the .evt run adds exactly the four src/events files"
    );

    // The ownership manifest differs by exactly the same four entries.
    let manifest_evt: CodegraphManifest =
        serde_json::from_str(read_out(&files_evt, ".codegraph-manifest.json").as_str()).unwrap();
    let manifest_off: CodegraphManifest =
        serde_json::from_str(read_out(&files_off, ".codegraph-manifest.json").as_str()).unwrap();
    let added: Vec<&String> = manifest_evt
        .generated
        .iter()
        .filter(|f| !manifest_off.generated.contains(f))
        .collect();
    assert_eq!(
        added,
        [
            "src/events/consumers.rs",
            "src/events/contracts.rs",
            "src/events/emit.rs",
            "src/events/mod.rs",
        ]
        .iter()
        .collect::<Vec<_>>(),
        "the manifest records exactly the new events files"
    );

    // Strongest practical assertion: every OTHER common file is
    // byte-identical — the flag-off contract at pipeline level.
    let mut differing = Vec::new();
    for (name, content_evt) in &files_evt {
        if GATED.contains(&name.as_str()) || name == ".codegraph-manifest.json" {
            continue;
        }
        match files_off.get(name) {
            Some(content_off) if content_off == content_evt => {}
            // Evt-only files are covered by the exact evt-only assertion.
            None if name.starts_with("src/events/") => {}
            _ => differing.push(name.clone()),
        }
    }
    assert!(
        differing.is_empty(),
        "common files must be byte-identical with .evt off; these differ: {differing:?}"
    );
}
