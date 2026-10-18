//! mox-first equivalence harness (issue #233, epic #228).
//!
//! One representative domain model is authored TWICE with identical class
//! titles — once as JSON Schema files (the legacy primary input) and once as
//! a single `.mox` file (the mox-first primary input) — and both are driven
//! through the SAME full pipeline (`driver::run`). The generated outputs are
//! then compared byte-for-byte.
//!
//! Equivalence standard: byte-identical, with NO normalization applied —
//! genuinely-source-agnostic noise turned out to be zero once the bridge
//! mapped faithfully. Reaching that took three P1 mapping-gap fixes in
//! `mox_ingest.rs`, each pinned by tests there and here:
//!
//! 1. enums now bridge to codelist `SchemaNode`s + `ReferencesSchema`/
//!    `ItemsOf` edges (the JSON path's shape — codelist DDL, FK targets
//!    routed to `common`, link generation, and the per-entity artifact set
//!    all key on that node);
//! 2. `extends` now merges ancestor features into the child class
//!    (allOf-canonical: inherited before own, each group name-sorted, first
//!    occurrence wins) — the JSON path flattens at ingest, so the entity,
//!    DTO, and handler field lists need the same merged properties;
//! 3. property/edge ingestion order now matches the JSON path's effective
//!    order — the `CachingQuerier` warm path (`list_all_properties`) has no
//!    `ORDER BY`, so insertion order IS generated field order.
//!
//! With those fixes the equivalence verdict is: full-tree byte-identity
//! (every generator, not just the core four) for this model. The tests
//! below enforce it; `report_residual_content_diffs_*` remains as a triage
//! companion for future models.
//!
//! Model contents: an entity with primitives (String/Long/Double/Boolean and
//! uuid/date/date-time formats), required + optional fields, a contained
//! value-object class (child table, scalar `[0..1]` containment), a `refers`
//! FK to a second entity, an enum codelist, and an `extends` (allOf) pair.
//!
//! Entity/VO decision parity: the JSON path needs explicit declarations
//! (`entities = [...]` in domains.toml — the auto-classifier's structural
//! heuristics cannot know a small class is an entity), while the mox path
//! derives the same decision from the model itself (`refers` targets are
//! entities; containment-only classes are value objects). Both sides
//! therefore encode the same authorial intent through their native mechanism.
//!
//! Namespace parity (issue #268): the mox model declares `package
//! equivalence` (⇒ namespace `equivalence` in the graph); the JSON model
//! declares no namespace. The byte-identity gates below therefore pin the
//! #268 back-compat contract — the flat (default) layout is namespace-
//! INERT, so a namespaced graph generates byte-identically to a
//! namespace-less one. `namespace_graph_parity_between_equivalent_json_
//! and_mox_models` additionally pins that namespace-EQUIVALENT models
//! agree on the namespace graph shape.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use codegraph_core::traits::GraphQuerier;

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.equivalence]
label = "Equivalence"
schema_dir = "equivalence"
postgres_schema = "equivalence"
entities = ["WorkOrderType", "WorkerType", "AssetType", "VehicleType"]
"#;

const CLASSIFIER_TOML: &str = "inline_enum_threshold = 20\n";

// ── JSON Schema authoring ────────────────────────────────────────────────
// Layout rule: the domain is the path segment before `/json/`
// (SchemaLoader::extract_domain_from_path).

const WORK_ORDER_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "WorkOrderType",
  "description": "A work order.",
  "type": "object",
  "properties": {
    "assignee": {
      "description": "Assigned worker.",
      "$ref": "WorkerType.json#"
    },
    "completed_at": {
      "description": "Completion timestamp.",
      "type": "string",
      "format": "date-time"
    },
    "detail": {
      "description": "Order detail.",
      "$ref": "WorkOrderDetailType.json#"
    },
    "due_date": {
      "description": "Due date.",
      "type": "string",
      "format": "date"
    },
    "id": {
      "description": "Primary identifier.",
      "type": "string",
      "format": "uuid"
    },
    "is_active": {
      "description": "Active flag.",
      "type": "boolean"
    },
    "notes": {
      "description": "Optional notes.",
      "type": "string"
    },
    "priority_score": {
      "description": "Priority score.",
      "type": "number"
    },
    "quantity": {
      "description": "Units ordered.",
      "type": "integer"
    },
    "status": {
      "description": "Order status.",
      "$ref": "codelist/WorkOrderStatus.json#"
    },
    "title": {
      "description": "Work order title.",
      "type": "string"
    }
  },
  "required": ["id", "title", "quantity", "is_active", "status"]
}"#;

const WORKER_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "WorkerType",
  "description": "A field worker.",
  "type": "object",
  "properties": {
    "id": {
      "description": "Primary identifier.",
      "type": "string",
      "format": "uuid"
    },
    "full_name": {
      "description": "Full name.",
      "type": "string"
    }
  },
  "required": ["id", "full_name"]
}"#;

const WORK_ORDER_DETAIL_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "WorkOrderDetailType",
  "description": "Work order detail.",
  "type": "object",
  "properties": {
    "instruction": {
      "description": "Instructions.",
      "type": "string"
    },
    "estimated_hours": {
      "description": "Estimated hours.",
      "type": "number"
    }
  },
  "required": ["instruction"]
}"#;

const ASSET_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "AssetType",
  "description": "A tracked asset.",
  "type": "object",
  "properties": {
    "id": {
      "description": "Primary identifier.",
      "type": "string",
      "format": "uuid"
    },
    "label": {
      "description": "Asset label.",
      "type": "string"
    }
  },
  "required": ["id", "label"]
}"#;

const VEHICLE_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "VehicleType",
  "description": "A vehicle.",
  "type": "object",
  "allOf": [
    { "$ref": "AssetType.json#" },
    {
      "type": "object",
      "properties": {
        "plate_no": {
          "description": "License plate.",
          "type": "string"
        }
      },
      "required": ["plate_no"]
    }
  ]
}"#;

const WORK_ORDER_STATUS_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "WorkOrderStatus",
  "description": "Work order lifecycle status.",
  "type": "string",
  "enum": ["Open", "InProgress", "Closed"],
  "enumNames": ["Open", "In progress", "Closed"]
}"#;

// ── .mox authoring ───────────────────────────────────────────────────────
// Same classes, same titles, same descriptions, same multiplicity.

const MODEL_MOX: &str = r#"package equivalence

/// Work order lifecycle status.
enum WorkOrderStatus {
    Open as "Open" = 0
    InProgress as "In progress" = 1
    Closed as "Closed" = 2
}

/// Primary identifier.
type UUID wraps String {
    format "uuid"
}

/// ISO date.
type Date wraps String {
    format "date"
}

/// UTC timestamp.
type Timestamp wraps String {
    format "date-time"
}

/// A tracked asset.
class AssetType {
    /// Primary identifier.
    UUID id
    /// Asset label.
    String label
}

/// A vehicle.
class VehicleType extends AssetType {
    /// License plate.
    String plate_no
}

/// A field worker.
class WorkerType {
    /// Primary identifier.
    UUID id
    /// Full name.
    String full_name
}

/// Work order detail.
class WorkOrderDetailType {
    /// Instructions.
    String instruction
    /// Estimated hours.
    Double [0..1] estimated_hours
}

/// A work order.
class WorkOrderType {
    /// Primary identifier.
    UUID id
    /// Work order title.
    String title
    /// Optional notes.
    String [0..1] notes
    /// Units ordered.
    Long quantity
    /// Priority score.
    Double [0..1] priority_score
    /// Active flag.
    Boolean is_active
    /// Due date.
    Date [0..1] due_date
    /// Completion timestamp.
    Timestamp [0..1] completed_at
    /// Order status.
    WorkOrderStatus status
    /// Assigned worker.
    refers WorkerType [0..1] assignee
    /// Order detail.
    contains WorkOrderDetailType [0..1] detail
}
"#;

// ── Fixture writing ──────────────────────────────────────────────────────

fn write_json_model(root: &Path) -> PathBuf {
    let json_dir = root.join("schemas/equivalence/json");
    let codelist_dir = json_dir.join("codelist");
    fs::create_dir_all(&codelist_dir).unwrap();
    fs::write(root.join("schemas/domains.toml"), DOMAINS_TOML).unwrap();
    fs::write(root.join("schemas/classifier.toml"), CLASSIFIER_TOML).unwrap();
    fs::write(json_dir.join("WorkOrderType.json"), WORK_ORDER_JSON).unwrap();
    fs::write(json_dir.join("WorkerType.json"), WORKER_JSON).unwrap();
    fs::write(
        json_dir.join("WorkOrderDetailType.json"),
        WORK_ORDER_DETAIL_JSON,
    )
    .unwrap();
    fs::write(json_dir.join("AssetType.json"), ASSET_JSON).unwrap();
    fs::write(json_dir.join("VehicleType.json"), VEHICLE_JSON).unwrap();
    fs::write(
        codelist_dir.join("WorkOrderStatus.json"),
        WORK_ORDER_STATUS_JSON,
    )
    .unwrap();
    root.join("schemas")
}

fn write_mox_model(root: &Path) -> PathBuf {
    let model_dir = root.join("model");
    fs::create_dir_all(&model_dir).unwrap();
    fs::write(root.join("model/domains.toml"), DOMAINS_TOML).unwrap();
    let mox = model_dir.join("equivalence.mox");
    fs::write(&mox, MODEL_MOX).unwrap();
    mox
}

// ── Pipeline driving ─────────────────────────────────────────────────────

fn run_args<'a>(
    config_path: &'a Path,
    schemas: Option<&'a Path>,
    classifier: Option<&'a Path>,
    mox_files: &'a [PathBuf],
    output: &'a Path,
) -> codegraph::driver::RunArgs<'a> {
    codegraph::driver::RunArgs {
        schemas,
        classifier,
        config_path,
        output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        // Nonexistent path ⇒ the no-plan (all generators) backward-compat
        // path, the same mode the mox_pipeline_tests use.
        profiles_config_path: Some(PathBuf::from("definitely-absent-profiles.toml")),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files,
        rosetta_files: &[],
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

/// Run the full pipeline from the JSON model, return generated files.
async fn generate_from_json(root: &Path) -> BTreeMap<String, String> {
    let schemas = write_json_model(root);
    let output = root.join("generated");
    let config = root.join("schemas/domains.toml");
    let classifier = root.join("schemas/classifier.toml");
    codegraph::driver::run(run_args(
        &config,
        Some(&schemas),
        Some(&classifier),
        &[],
        &output,
    ))
    .await
    .unwrap();
    collect_files(&output)
}

/// Run the full pipeline from the mox model, return generated files.
async fn generate_from_mox(root: &Path) -> BTreeMap<String, String> {
    let mox = write_mox_model(root);
    let output = root.join("generated");
    let config = root.join("model/domains.toml");
    codegraph::driver::run(run_args(&config, None, None, &[mox], &output))
        .await
        .unwrap();
    collect_files(&output)
}

// ── Comparison helpers ───────────────────────────────────────────────────

/// Render a compact line-level diff for a pair of file contents.
fn first_diffs(label: &str, json: &str, mox: &str, max: usize) -> String {
    let mut out = format!("--- {label}: json vs mox\n");
    let mut shown = 0;
    for (i, (a, b)) in json.lines().zip(mox.lines()).enumerate() {
        if a != b {
            out.push_str(&format!("  json L{}: {a}\n  mox  L{}: {b}\n", i + 1, i + 1));
            shown += 1;
            if shown >= max {
                out.push_str("  ...\n");
                break;
            }
        }
    }
    if json.lines().count() != mox.lines().count() {
        out.push_str(&format!(
            "  line counts differ: json={} mox={}\n",
            json.lines().count(),
            mox.lines().count()
        ));
    }
    out
}

/// Files that must be byte-identical between the two runs: the core four
/// generators (ddl, sea_orm_entity, dto, handler) for every class in the
/// shared model. Migration names carry a deterministic sequence prefix, so
/// entries are matched by unique suffix.
const MUST_MATCH: &[&str] = &[
    "equivalence_work_order.sql",
    "equivalence_worker.sql",
    "equivalence_asset.sql",
    "equivalence_vehicle.sql",
    "equivalence_work_order_detail.sql",
    "equivalence_work_order_status.sql",
    "src/entity/equivalence_work_order.rs",
    "src/entity/equivalence_worker.rs",
    "src/entity/equivalence_asset.rs",
    "src/entity/equivalence_vehicle.rs",
    "src/entity/equivalence_work_order_status.rs",
    "src/domain/equivalence/work_order/dto_create.rs",
    "src/domain/equivalence/work_order/dto_update.rs",
    "src/domain/equivalence/work_order/dto_response.rs",
    "src/api/equivalence/work_order_handler.rs",
];

/// Resolve a MUST_MATCH suffix to a unique output path. Migration names are
/// suffixed by `{schema}_{table}`; the `_rls` guard keeps e.g.
/// `equivalence_work_order.sql` from matching `..._rls.sql` twins.
fn resolve_path<'a>(files: &'a BTreeMap<String, String>, suffix: &str) -> Option<&'a String> {
    let matches: Vec<&String> = files
        .keys()
        .filter(|k| {
            k.ends_with(suffix)
                && !k.ends_with(&format!("_rls{suffix}"))
                && !k.ends_with(&format!("_trigger{suffix}"))
                && !k.ends_with(&format!("_fts{suffix}"))
        })
        .collect();
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

/// The core equivalence gate: the core four generators (DDL, SeaORM entity,
/// DTO, handler) must produce byte-identical output for the shared model.
///
/// With the codelist schema bridging and extends inheritance in place this
/// holds today; if it fails the diff output names the exact file and lines,
/// which is either a P1 mapping gap (fix in mox_ingest.rs) or a residual to
/// be documented here.
#[tokio::test]
async fn core_generators_are_byte_identical_between_json_and_mox_models() {
    let json_root = tempfile::tempdir().unwrap();
    let mox_root = tempfile::tempdir().unwrap();
    let json_files = generate_from_json(json_root.path()).await;
    let mox_files = generate_from_mox(mox_root.path()).await;

    let mut failures = String::new();
    for suffix in MUST_MATCH {
        let json = resolve_path(&json_files, suffix);
        let mox = resolve_path(&mox_files, suffix);
        match (json, mox) {
            (Some(json_path), Some(mox_path)) => {
                let (json, mox) = (&json_files[json_path], &mox_files[mox_path]);
                if json != mox {
                    failures.push_str(&first_diffs(suffix, json, mox, 10));
                }
            }
            (Some(json_path), None) => {
                failures.push_str(&format!(
                    "--- {suffix}: missing from mox output (json at {json_path})\n"
                ));
            }
            (None, Some(mox_path)) => {
                failures.push_str(&format!(
                    "--- {suffix}: missing from json output (mox at {mox_path})\n"
                ));
            }
            (None, None) => {
                failures.push_str(&format!(
                    "--- {suffix}: missing from BOTH outputs (model or path drift)\n"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "byte-identity failures between JSON and mox runs:\n{failures}"
    );
}

/// The remaining generator surface must agree on the FILE SET for the
/// model's entities, and the full trees are additionally compared
/// byte-for-byte (not just the MUST_MATCH subset) — any residual drift
/// fails loudly here so it can be classified (mapping gap vs inherent
/// format difference) instead of silently regressing.
#[tokio::test]
async fn full_output_file_sets_agree_between_json_and_mox_models() {
    let json_root = tempfile::tempdir().unwrap();
    let mox_root = tempfile::tempdir().unwrap();
    let json_files = generate_from_json(json_root.path()).await;
    let mox_files = generate_from_mox(mox_root.path()).await;

    let only_json: Vec<&String> = json_files
        .keys()
        .filter(|k| !mox_files.contains_key(*k))
        .collect();
    let only_mox: Vec<&String> = mox_files
        .keys()
        .filter(|k| !json_files.contains_key(*k))
        .collect();
    assert!(
        only_json.is_empty() && only_mox.is_empty(),
        "file-set drift:\nonly in json output: {only_json:?}\nonly in mox output: {only_mox:?}"
    );
}

/// Full-tree byte-identity: every generated file (all generators, not just
/// the core four) must be identical between the JSON-authored and
/// mox-authored models. This is the strongest form of the equivalence claim
/// and it holds today; a failure here is either a P1 mapping gap in
/// `mox_ingest.rs` (fix there) or a genuinely source-agnostic artifact that
/// must be argued and documented in the harness report.
#[tokio::test]
async fn full_trees_are_byte_identical_between_json_and_mox_models() {
    let json_root = tempfile::tempdir().unwrap();
    let mox_root = tempfile::tempdir().unwrap();
    let json_files = generate_from_json(json_root.path()).await;
    let mox_files = generate_from_mox(mox_root.path()).await;

    let mut failures = String::new();
    for (name, json) in &json_files {
        match mox_files.get(name) {
            Some(mox) if json != mox => failures.push_str(&first_diffs(name, json, mox, 6)),
            Some(_) => {}
            None => failures.push_str(&format!("--- {name}: missing from mox output\n")),
        }
    }
    for name in mox_files.keys() {
        if !json_files.contains_key(name) {
            failures.push_str(&format!("--- {name}: missing from json output\n"));
        }
    }
    assert!(
        failures.is_empty(),
        "full-tree drift between JSON and mox runs ({} vs {} files):\n{failures}",
        json_files.len(),
        mox_files.len()
    );
}

/// Diagnostic companion: print the full content diff report without
/// asserting. Run with `--nocapture` when triaging new residuals.
#[tokio::test]
async fn report_residual_content_diffs_between_json_and_mox_models() {
    let json_root = tempfile::tempdir().unwrap();
    let mox_root = tempfile::tempdir().unwrap();
    let json_files = generate_from_json(json_root.path()).await;
    let mox_files = generate_from_mox(mox_root.path()).await;

    let mut diffs = 0;
    for (name, json) in &json_files {
        if let Some(mox) = mox_files.get(name)
            && json != mox
        {
            diffs += 1;
            eprintln!("{}", first_diffs(name, json, mox, 6));
        }
    }
    eprintln!(
        "residual report: {diffs} differing files (see MUST_MATCH gate for the enforced subset)"
    );
}

// ── Namespace parity (issue #268) ────────────────────────────────────────

/// Namespace parity (issue #268): see the byte-identity gates above.
#[tokio::test]
async fn namespace_graph_parity_between_equivalent_json_and_mox_models() {
    // mox side: the equivalence fixture's package → namespace.
    let mox_engine = codegraph_grafeo::GrafeoEngine::in_memory().unwrap();
    {
        let dir = tempfile::tempdir().unwrap();
        let mox_path = dir.path().join("equivalence.mox");
        fs::write(&mox_path, MODEL_MOX).unwrap();
        let config: codegraph_config::config::DomainConfig =
            codegraph_config::config::parse_domain_config_str(DOMAINS_TOML).unwrap();
        codegraph::ingest::mox_ingest::ingest_mox_files(
            &mox_engine,
            &mox_engine,
            &[mox_path],
            &config,
            &config.defaults.type_suffix,
        )
        .await
        .unwrap();
        dir.close().unwrap();
    }

    // JSON side: the same class set, one schema carrying an `$id` whose
    // path ends in `/equivalence/` — the documented $id derivation must
    // land it in the SAME namespace the mox package declares.
    let json_engine = codegraph_grafeo::GrafeoEngine::in_memory().unwrap();
    {
        let dir = tempfile::tempdir().unwrap();
        let json_dir = dir.path().join("equivalence/json");
        fs::create_dir_all(&json_dir).unwrap();
        let mut worker: serde_json::Value = serde_json::from_str(WORKER_JSON).unwrap();
        worker["$id"] = serde_json::json!("https://app.example/equivalence/WorkerType.json");
        fs::write(json_dir.join("WorkerType.json"), worker.to_string()).unwrap();
        let classifier =
            codegraph_classifier::config::parse_classifier_config_str(CLASSIFIER_TOML).unwrap();
        codegraph::ingest::async_ingest::ingest_schemas(
            &json_engine,
            dir.path(),
            &classifier,
            &Default::default(),
            &Default::default(),
            "Type",
        )
        .await
        .unwrap();
        dir.close().unwrap();
    }

    // Parity: the WorkerType schema lives in namespace `equivalence` on
    // BOTH sides, with identical membership shape.
    async fn assert_ns(engine: &codegraph_grafeo::GrafeoEngine) -> Vec<String> {
        let worker = engine.get_schema("WorkerType").await.unwrap().unwrap();
        assert_eq!(
            worker.namespace.as_deref(),
            Some("equivalence"),
            "WorkerType must carry the equivalence namespace on both sides"
        );
        let members = engine
            .list_schemas_by_namespace("equivalence", false)
            .await
            .unwrap();
        assert!(
            members.iter().any(|s| s.title == "WorkerType"),
            "InNamespace membership: {members:?}"
        );
        engine
            .list_namespaces()
            .await
            .unwrap()
            .iter()
            .map(|n| n.fqn.clone())
            .collect()
    }
    let mox_fqns = assert_ns(&mox_engine).await;
    let json_fqns = assert_ns(&json_engine).await;
    assert_eq!(mox_fqns, json_fqns, "namespace node sets must agree");
}

// ── Broader-suite smoke: cornucopia / IFML / gRPC over the mox model ────

/// Minimal cornucopia-profile plan (monolith topology, cornucopia
/// persistence) used by the smoke test below.
const CORNUCOPIA_PROFILES_TOML: &str = r#"
[profiles.cornucopia-smoke.meta]
name = "cornucopia-smoke"
version = "1.0.0"
description = "Cornucopia persistence over a mox-sourced model"

[profiles.cornucopia-smoke.features]
persistence_provider = "cornucopia"
deployment_topology = "monolith"

[profiles.cornucopia-smoke.api]
generators = [
    "ddl", "cornucopia_queries", "cornucopia_repo", "dto", "repository",
    "command", "query", "event", "handler", "lifecycle_trait",
    "domain_types_dto", "domain_types_query_service",
    "errors", "router", "links",
    "cornucopia_config", "basejump_setup", "pgmq_setup", "platform_schema",
    "workflow_seed", "hook_registry", "domain_types_scaffold", "codelist",
    "openapi",
]
output = "generated/"
"#;

/// The cornucopia profile (persistence_provider = "cornucopia") must
/// generate without errors over the mox-sourced model and produce
/// plausible annotated `.sql` query files plus the cornucopia config.
#[tokio::test]
async fn cornucopia_profile_generates_plausible_sql_from_mox_model() {
    let root = tempfile::tempdir().unwrap();
    let mox = write_mox_model(root.path());
    let profiles = root.path().join("profiles.toml");
    fs::write(&profiles, CORNUCOPIA_PROFILES_TOML).unwrap();
    let output = root.path().join("generated");
    let config = root.path().join("model/domains.toml");

    let mox_files = [mox];
    let mut args = run_args(&config, None, None, &mox_files, &output);
    args.profile_name = "cornucopia-smoke";
    args.profiles_config_path = Some(profiles);
    codegraph::driver::run(args).await.unwrap();

    let queries = root.path().join("generated/queries/equivalence");
    let work_order_sql = fs::read_to_string(queries.join("work_order.sql"))
        .expect("cornucopia query file for work_order");
    for statement in [
        "--! list_work_order",
        "--! count_work_order",
        "--! create_work_order",
        "--! update_work_order",
    ] {
        assert!(
            work_order_sql.contains(statement),
            "expected `{statement}` in cornucopia queries:\n{work_order_sql}"
        );
    }
    assert!(
        work_order_sql.contains("FROM \"equivalence\".\"work_order\""),
        "queries must target the mox-sourced table:\n{work_order_sql}"
    );
    assert!(
        root.path()
            .join("generated/cornucopia-queries/cornucopia.toml")
            .exists(),
        "cornucopia.toml config must be emitted"
    );
    // DDL still flows from the shared mox-sourced graph.
    let ddl = resolve_path(&collect_files(&output), "equivalence_work_order.sql")
        .expect("entity DDL for work_order")
        .clone();
    let ddl_sql = fs::read_to_string(output.join(&ddl)).unwrap();
    assert!(ddl_sql.contains("CREATE TABLE"), "{ddl_sql}");
}

/// IFML `data:` bindings must resolve against mox-sourced schemas: a view
/// bound to `WorkOrder` (the .mox class) generates route pages whose e2e
/// specs carry schema-backed CRUD coverage against the mox entity's API.
#[tokio::test]
async fn ifml_binding_to_mox_sourced_entity_generates_routes() {
    const APP_IFML: &str = r#"
domain "equivalence" {
    schema "equivalence";
}

view "WorkOrderList" {
    label "Work Orders";
    landmark: true;

    component "grid" {
        type: list;
        data: WorkOrder;
        fields: [title, status, quantity];

        on select(row) -> navigate("WorkOrderEdit", {
            workOrderId: row.id
        });
    }
}

view "WorkOrderEdit" {
    params { workOrderId: Uuid };

    component "editor" {
        type: form;
        data: WorkOrder;
        mode: edit;
        fields: [title, notes, quantity];

        on save(values) -> navigate("WorkOrderList");
    }
}
"#;

    let root = tempfile::tempdir().unwrap();
    let mox = write_mox_model(root.path());
    let ifml_path = root.path().join("model/app.ifml");
    fs::write(&ifml_path, APP_IFML).unwrap();
    let output = root.path().join("generated");
    let config = root.path().join("model/domains.toml");

    let ifml_files = [ifml_path];
    let frameworks = ["svelte".to_string()];
    let mox_files = [mox];
    let mut args = run_args(&config, None, None, &mox_files, &output);
    args.ifml_files = &ifml_files;
    args.ifml_framework = &frameworks;
    codegraph::driver::run(args).await.unwrap();

    assert!(
        output
            .join("svelte/src/routes/workorderlist/+page.svelte")
            .exists(),
        "route page for the mox-bound view"
    );
    let spec = fs::read_to_string(
        output
            .join("svelte/tests/ifml")
            .join("work-order-list.spec.ts"),
    )
    .expect("e2e spec for the mox-bound view");
    // Schema-backed enrichment: without a resolvable entity the generator
    // degrades to render-only tests (no request.post / waitForURL).
    assert!(
        spec.contains("request.post") || spec.contains("waitForURL"),
        "mox-sourced entity must enable schema-backed e2e coverage:\n{spec}"
    );
}

/// gRPC generators over the mox model — only runnable when `protoc` is
/// installed (compile validation needs it). Skipped with a note otherwise.
#[tokio::test]
async fn grpc_suite_over_mox_model_when_protoc_available() {
    let protoc = std::process::Command::new("protoc")
        .arg("--version")
        .output();
    let Ok(output) = protoc else {
        eprintln!("SKIP: gRPC smoke — `protoc` is not installed");
        return;
    };
    let _ = output;

    let root = tempfile::tempdir().unwrap();
    let mox = write_mox_model(root.path());
    let profiles = root.path().join("profiles.toml");
    fs::write(
        &profiles,
        r#"
[profiles.grpc-smoke.meta]
name = "grpc-smoke"
version = "1.0.0"
description = "gRPC generators over a mox-sourced model"

[profiles.grpc-smoke.features]
grpc_backend = true

[profiles.grpc-smoke.api]
generators = [
    "ddl", "grpc_proto", "grpc_service", "grpc_router", "grpc_scaffold",
]
output = "generated/"
"#,
    )
    .unwrap();
    let output_dir = root.path().join("generated");
    let config = root.path().join("model/domains.toml");

    let mox_files = [mox];
    let mut args = run_args(&config, None, None, &mox_files, &output_dir);
    args.profile_name = "grpc-smoke";
    args.profiles_config_path = Some(profiles);
    codegraph::driver::run(args).await.unwrap();

    let proto = fs::read_to_string(
        root.path()
            .join("generated/proto/equivalence/work_order.proto"),
    )
    .expect("proto file for the mox-sourced entity");
    assert!(proto.contains("message WorkOrder"), "{proto}");
}
