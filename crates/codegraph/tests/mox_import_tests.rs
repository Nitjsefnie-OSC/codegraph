//! mox `import schema` tests (issue #230, epic #228 — C1, codegraph half).
//!
//! rexlang owns NAME resolution (the lowered IR carries a nominal
//! feature-less class per import); codegraph's existing JSON Schema
//! pipeline owns the graph nodes for imported files:
//!
//! - `ingest_mox_files` scans `import schema "<path>"` declarations,
//!   resolves them relative to the .mox file's directory, reads + validates
//!   the JSON (hard error on missing/invalid — structural refs depend on
//!   it), and passes the content to `compile_files_with_imports`;
//! - the imported files are ingested through the same schema pipeline as
//!   `--schemas` (classification, codelists, ranges all keep working), and
//!   their titles join the schema pass's skip-set so a file passed both
//!   ways ingests exactly once;
//! - alias-typed features (`refers TodoItem[] items`) get their
//!   ReferencesSchema/ItemsOf edges in a post-schema-pass wiring step:
//!   exact title match → title + `defaults.type_suffix` → warning (never a
//!   hard error — the file was valid; the mismatch is a naming miss).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use codegraph_core::traits::GraphQuerier;
use codegraph_grafeo::GrafeoEngine;

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.todo]
label = "Todo"
schema_dir = "schemas"
postgres_schema = "todo"
"#;

/// Imported JSON schema: title `TodoItemType` (the `Customer` → `CustomerType`
/// suffix convention the alias resolution must bridge), with a codelist-shaped
/// enum property so the imported file exercises the codelist path.
const TODO_ITEM_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "TodoItemType",
  "type": "object",
  "properties": {
    "id": { "description": "Primary identifier.", "type": "string", "format": "uuid" },
    "title": { "description": "What to do.", "type": "string" },
    "priority": {
      "description": "How urgent.",
      "enum": ["low", "high"],
      "enumNames": ["Low", "High"]
    }
  },
  "required": ["id", "title"]
}"#;

const TODO_LIST_MOX: &str = r#"
package todo

import schema "schemas/todo_item.json" as TodoItem

class TodoListType {
    String [1] name
    refers TodoItem[] items
}
"#;

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
}

fn write_file(dir: &Path, rel: &str, contents: &str) -> PathBuf {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, contents).unwrap();
    path
}

/// Standard fixture: domains.toml, the imported schema under `schemas/`, and
/// a .mox at the root importing it relative to its own directory.
fn write_fixture(mox_source: &str, json: &str) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let config = write_file(root.path(), "domains.toml", DOMAINS_TOML);
    write_file(root.path(), "schemas/todo_item.json", json);
    write_file(root.path(), "todo_list.mox", mox_source);
    Fixture { root, config }
}

impl Fixture {
    fn mox(&self) -> PathBuf {
        self.root.path().join("todo_list.mox")
    }

    fn schemas(&self) -> PathBuf {
        self.root.path().join("schemas")
    }

    fn out(&self) -> PathBuf {
        self.root.path().join("generated")
    }
}

fn run_args<'a>(
    fixture: &'a Fixture,
    schemas: Option<&'a Path>,
    mox_files: &'a [PathBuf],
    output: &'a Path,
) -> codegraph::driver::RunArgs<'a> {
    codegraph::driver::RunArgs {
        schemas,
        classifier: schemas.map(|_| Path::new("tests/fixtures/classifier.toml")),
        config_path: &fixture.config,
        output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        // Nonexistent path → the no-plan (all generators) backward-compat
        // path, the same mode the mox_pipeline_tests use.
        profiles_config_path: Some(fixture.root.path().join("profiles.toml")),
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

/// Locate a migration for a table in the todo schema (files carry a sequence
/// prefix, e.g. `000042_todo_todo_item.sql`).
fn migration(table: &str, output: &Path) -> PathBuf {
    let dir = output.join("migrations");
    fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(&format!("todo_{table}.sql")))
        })
        .unwrap_or_else(|| panic!("no migration for todo.{table} under {}", dir.display()))
}

/// Drive the same ingest-layer passes the driver runs for a mox-only run:
/// mox ingest (with import scanning) → imported-file ingestion through the
/// JSON pipeline → post-schema-pass alias wiring. Returns the final stats.
async fn run_ingest_layer(
    engine: &GrafeoEngine,
    fixture: &Fixture,
    with_schema_pass: bool,
) -> codegraph::ingest::mox_ingest::MoxIngestStats {
    let config = codegraph_config::config::parse_domain_config(&fixture.config).unwrap();
    let suffix = &config.defaults.type_suffix;

    let outcome = codegraph::ingest::mox_ingest::ingest_mox_files(
        engine,
        engine,
        &[fixture.mox()],
        &config,
        suffix,
    )
    .await
    .unwrap();

    let classifier = codegraph_classifier::config::parse_classifier_config_str("").unwrap();
    let imported = codegraph::ingest::async_ingest::ingest_imported_schemas(
        engine,
        &outcome.imported_files,
        &classifier,
        &codegraph_config::UiOverrideConfig::default(),
        suffix,
    )
    .await
    .unwrap();

    let mut skip: HashSet<String> = outcome.stats.bridged_titles.iter().cloned().collect();
    skip.extend(imported.titles);

    if with_schema_pass {
        codegraph::ingest::async_ingest::ingest_schemas_with_skips(
            engine,
            &fixture.schemas(),
            &classifier,
            &HashSet::new(),
            &codegraph_config::UiOverrideConfig::default(),
            suffix,
            &skip,
        )
        .await
        .unwrap();
    }

    let mut stats = outcome.stats;
    codegraph::ingest::mox_ingest::wire_alias_refs(
        engine,
        engine,
        &outcome.pending_alias_refs,
        suffix,
        &mut stats,
    )
    .await
    .unwrap();
    stats
}

// ── 1. mox-only run: imported file ingested via the JSON pipeline ──

/// Full-pipeline: a mox-only run (no --schemas) generates DDL for the
/// imported schema's entity (TodoItemType → todo.todo_item) and for the
/// mox-authored class.
#[tokio::test]
async fn mox_only_run_generates_artifacts_for_imported_schema() {
    let fixture = write_fixture(TODO_LIST_MOX, TODO_ITEM_JSON);
    let output = fixture.out();
    let mox_files = vec![fixture.mox()];

    codegraph::driver::run(run_args(&fixture, None, &mox_files, &output))
        .await
        .unwrap();

    let item = fs::read_to_string(migration("todo_item", &output)).unwrap();
    assert!(item.contains("CREATE TABLE"), "{item}");
    assert!(item.contains("title"), "{item}");

    let list = fs::read_to_string(migration("todo_list", &output)).unwrap();
    assert!(list.contains("CREATE TABLE"), "{list}");
}

/// Ingest layer: the imported file produces a TodoItemType SchemaNode with
/// its properties (classified/codelist-shaped per the JSON pipeline), the
/// mox class bridges as usual, and the alias-typed feature resolves through
/// the wiring step to the REAL title (suffix convention:
/// alias `TodoItem` → title `TodoItemType`). The edge is ItemsOf because the
/// feature is many — the same array convention both the JSON path and the
/// mox bridge use for `items.$ref` arrays.
#[tokio::test]
async fn imported_schema_ingests_via_json_pipeline_and_alias_wires_to_real_title() {
    let fixture = write_fixture(TODO_LIST_MOX, TODO_ITEM_JSON);
    let engine = GrafeoEngine::in_memory().unwrap();
    let stats = run_ingest_layer(&engine, &fixture, false).await;

    // Imported file went through the JSON pipeline.
    let item = engine
        .get_schema("TodoItemType")
        .await
        .unwrap()
        .expect("imported schema must be ingested");
    assert_ne!(
        item.custom_annotations
            .get("source")
            .and_then(|v| v.as_str()),
        Some("mox"),
        "imported files are JSON-pipeline-authored, not mox-bridged"
    );
    let props = engine.get_properties("TodoItemType").await.unwrap();
    let names: HashSet<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert!(
        names.contains("id") && names.contains("title") && names.contains("priority"),
        "imported properties missing: {names:?}"
    );
    // The codelist-shaped enum property keeps its synthetic codelist values.
    let priority = props.iter().find(|p| p.name == "priority").unwrap();
    let codelist = priority.ref_target.clone().expect("enum ref_target");
    let values = engine.get_enum_values(&codelist).await.unwrap();
    assert_eq!(
        values.iter().map(|v| v.value.as_str()).collect::<Vec<_>>(),
        vec!["low", "high"]
    );

    // The mox class bridges as usual.
    let list = engine.get_schema("TodoListType").await.unwrap().unwrap();
    assert_eq!(
        list.custom_annotations
            .get("source")
            .and_then(|v| v.as_str()),
        Some("mox")
    );
    // The import's nominal class must NOT be bridged as a schema node.
    assert!(engine.get_schema("TodoItem").await.unwrap().is_none());

    // Alias wiring: `refers TodoItem[] items` → ItemsOf edge to TodoItemType.
    let target = engine
        .get_array_item_schema("items", "TodoListType")
        .await
        .unwrap()
        .expect("wired array edge must resolve");
    assert_eq!(target.title, "TodoItemType");

    assert_eq!(stats.imported_files, 1);
    assert_eq!(stats.resolved_aliases, 1);
    assert_eq!(stats.unresolved_aliases, 0);
}

// ── 2. alias → title resolution order ──

/// An alias that IS the real title resolves via exact match (scalar feature
/// → ReferencesSchema edge).
#[tokio::test]
async fn exact_title_alias_resolves_and_wires_scalar_reference() {
    let mox = r#"
package todo

import schema "schemas/todo_item.json" as TodoItemType

class TodoListType {
    String [1] name
    refers TodoItemType [0..1] primary
}
"#;
    let fixture = write_fixture(mox, TODO_ITEM_JSON);
    let engine = GrafeoEngine::in_memory().unwrap();
    let stats = run_ingest_layer(&engine, &fixture, false).await;

    let target = engine
        .get_property_ref_target("primary", "TodoListType")
        .await
        .unwrap()
        .expect("exact-title alias must wire a ReferencesSchema edge");
    assert_eq!(target.title, "TodoItemType");
    assert_eq!(stats.resolved_aliases, 1);
    assert_eq!(stats.unresolved_aliases, 0);
}

/// A deliberately unresolvable alias warns (naming both names), creates no
/// edge, and the run still succeeds — the imported file itself was valid.
#[tokio::test]
async fn unresolvable_alias_warns_and_skips_the_edge_but_run_succeeds() {
    let json = TODO_ITEM_JSON.replace("TodoItemType", "GizmoType");
    let mox = r#"
package todo

import schema "schemas/todo_item.json" as Widget

class TodoListType {
    String [1] name
    refers Widget[] items
}
"#;
    let fixture = write_fixture(mox, &json);
    let engine = GrafeoEngine::in_memory().unwrap();
    let stats = run_ingest_layer(&engine, &fixture, false).await;

    assert_eq!(stats.unresolved_aliases, 1);
    assert_eq!(stats.resolved_aliases, 0);
    assert!(
        engine
            .get_array_item_schema("items", "TodoListType")
            .await
            .unwrap()
            .is_none(),
        "unresolvable alias must not produce an edge"
    );
    // The imported file still ingested under its real title.
    assert!(engine.get_schema("GizmoType").await.unwrap().is_some());

    // Full run succeeds.
    let output = fixture.out();
    let mox_files = vec![fixture.mox()];
    codegraph::driver::run(run_args(&fixture, None, &mox_files, &output))
        .await
        .unwrap();
}

// ── 3. hard errors ──

#[tokio::test]
async fn missing_import_file_is_a_hard_error_naming_both_paths() {
    let mox = r#"
package todo

import schema "schemas/missing.json" as Ghost

class TodoListType {
    String [1] name
}
"#;
    let fixture = write_fixture(mox, TODO_ITEM_JSON);
    let output = fixture.out();
    let mox_files = vec![fixture.mox()];

    let err = codegraph::driver::run(run_args(&fixture, None, &mox_files, &output))
        .await
        .unwrap_err();
    let message = format!("{err}");
    assert!(message.contains("schemas/missing.json"), "{message}");
    assert!(message.contains("todo_list.mox"), "{message}");
    assert!(message.contains("could not be read"), "{message}");
}

#[tokio::test]
async fn invalid_import_json_is_a_hard_error_naming_both_paths() {
    let fixture = write_fixture(TODO_LIST_MOX, "{ definitely not json");
    let output = fixture.out();
    let mox_files = vec![fixture.mox()];

    let err = codegraph::driver::run(run_args(&fixture, None, &mox_files, &output))
        .await
        .unwrap_err();
    let message = format!("{err}");
    assert!(message.contains("schemas/todo_item.json"), "{message}");
    assert!(message.contains("todo_list.mox"), "{message}");
    assert!(message.contains("not valid JSON"), "{message}");
}

// ── 4. --schemas overlap: exactly one node ──

/// The same directory passed both as import target and `--schemas`: the
/// imported ingestion wins, the schema pass skips the covered title, and the
/// graph keeps exactly one TodoItemType (JSON-authored — imported files are
/// not mox-bridged).
#[tokio::test]
async fn schemas_overlap_ingests_the_imported_file_exactly_once() {
    let fixture = write_fixture(TODO_LIST_MOX, TODO_ITEM_JSON);
    let engine = GrafeoEngine::in_memory().unwrap();
    run_ingest_layer(&engine, &fixture, true).await;

    let schemas = engine.list_schemas(None).await.unwrap();
    let items: Vec<_> = schemas
        .iter()
        .filter(|s| s.title == "TodoItemType")
        .collect();
    assert_eq!(items.len(), 1, "no duplicate TodoItemType: {schemas:?}");
    assert_ne!(
        items[0]
            .custom_annotations
            .get("source")
            .and_then(|v| v.as_str()),
        Some("mox")
    );
}

// ── 5. byte identity for import-free mox ──

/// A .mox with NO imports behaves exactly as on master: no imported files,
/// no pending refs, zero import counters, and the stats Display carries no
/// import suffix.
#[tokio::test]
async fn import_free_mox_has_no_import_side_effects() {
    let mox = r#"
package todo

class TodoListType {
    String [1] name
}
"#;
    let fixture = write_fixture(mox, TODO_ITEM_JSON);
    let engine = GrafeoEngine::in_memory().unwrap();
    let stats = run_ingest_layer(&engine, &fixture, false).await;

    assert_eq!(stats.imported_files, 0);
    assert_eq!(stats.resolved_aliases, 0);
    assert_eq!(stats.unresolved_aliases, 0);
    assert!(
        !stats.to_string().contains("imported"),
        "import-free stats display must stay byte-identical: {}",
        stats
    );
}
