//! Driver-level pins for derived child schemas from mox `refers` arrays
//! (issue #312).
//!
//! `GraphQuerier::get_child_schemas` is the graph route behind the detail
//! page's child sections (`collect_child_sections`). It historically only
//! matched the inline `#/$defs` `parent_schema` back-pointer, so mox
//! `refers S[] items` graphs (parent-side array-of-entity-ref, ItemsOf
//! edge, FK-on-child lowering) yielded no children — child sections then
//! required explicit `entity_config` (`role = "child"`) authorship.
//!
//! These pins cover the three contract surfaces:
//!
//! 1. **Querier parity** — the same mox fixture bridged into the Grafeo
//!    engine and into `MockEngine` resolves identical children (Grafeo via
//!    the ItemsOf chain, Mock via the property `ref_target`), with
//!    non-children (the reverse direction, codelist arrays) excluded.
//! 2. **mox-only model, no entity_config** — the child section renders on
//!    the parent's detail page and the DDL carries the derived child-side
//!    FK.
//! 3. **Config stays authoritative** — when `entity_config` ALSO declares
//!    the child, exactly one section renders and the config values
//!    (`path_segment`) win over graph-derived defaults.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use codegraph_core::traits::GraphQuerier;

const TODO_MOX: &str = r#"package todo

/// A named collection of todo items.
class TodoListType {
    /// Display name of the list.
    String name

    /// The items on this list — array `refers` is the parent-side child
    /// declaration (FK lands on the child table).
    refers TodoItemType[] items
}

/// A single todo item belonging to a list.
class TodoItemType {
    /// Short title of the item.
    String title
}
"#;

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.todo]
label = "Todo"
schema_dir = "todo"
postgres_schema = "todo"
"#;

/// The same relationship declared the config way: JSON schemas plus an
/// `entity_config` child declaration for TodoItemType.
const TODO_LIST_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "TodoListType",
  "description": "A named collection of todo items.",
  "type": "object",
  "properties": {
    "name": { "description": "Display name.", "type": "string" },
    "items": {
      "description": "The items on this list.",
      "type": "array",
      "items": { "$ref": "TodoItemType.json#" }
    }
  },
  "required": ["name"]
}"#;

const TODO_ITEM_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "TodoItemType",
  "description": "A single todo item belonging to a list.",
  "type": "object",
  "properties": {
    "title": { "description": "Short title.", "type": "string" }
  },
  "required": ["title"]
}"#;

/// Config-declared twin of the mox fixture: the child declaration is
/// explicit (`role = "child"` + `parent` + `parent_ref`) AND the graph can
/// derive the same child from the array property — the dedupe case.
const DOMAINS_TOML_WITH_CHILD_CONFIG: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.todo]
label = "Todo"
schema_dir = "todo"
postgres_schema = "todo"
entities = ["TodoListType", "TodoItemType"]

[domains.todo.entity_config.TodoItemType]
role = "child"
parent = "TodoListType"
parent_ref = "todo_list_id"
path_segment = "config-items"
"#;

struct MoxFixture {
    root: tempfile::TempDir,
    config: PathBuf,
    mox: PathBuf,
}

fn write_mox_fixture() -> MoxFixture {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("domains.toml"), DOMAINS_TOML).unwrap();
    fs::create_dir_all(root.path().join("model")).unwrap();
    fs::write(root.path().join("model/todo.mox"), TODO_MOX).unwrap();
    MoxFixture {
        config: root.path().join("domains.toml"),
        mox: root.path().join("model/todo.mox"),
        root,
    }
}

fn run_args<'a>(
    config_path: &'a Path,
    schemas: Option<&'a Path>,
    classifier: Option<&'a Path>,
    mox_files: &'a [PathBuf],
    output: &'a Path,
    root: &'a Path,
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
        profiles_config_path: Some(root.join("definitely-absent-profiles.toml")),
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

/// Total occurrence count of `needle` across the collected Svelte pages —
/// the detail-page surface the child-section contract lives on. (Generated
/// e2e specs reference the same testids and are deliberately excluded.)
fn count_page_occurrences(files: &BTreeMap<String, String>, needle: &str) -> usize {
    files
        .iter()
        .filter(|(path, _)| path.ends_with("+page.svelte"))
        .map(|(_, content)| content.matches(needle).count())
        .sum()
}

// ── 1. Querier parity: Grafeo and Mock resolve the same mox children ────

#[tokio::test]
async fn mox_refers_children_resolve_alike_on_grafeo_and_mock() {
    let fixture = write_mox_fixture();
    let config = codegraph_config::config::parse_domain_config(&fixture.config).unwrap();
    let mox_files = vec![fixture.mox.clone()];

    let grafeo = codegraph_grafeo::GrafeoEngine::in_memory().unwrap();
    codegraph::ingest::mox_ingest::ingest_mox_files(
        &grafeo,
        &grafeo,
        &mox_files,
        &config,
        &config.defaults.type_suffix,
    )
    .await
    .unwrap();

    let mock = codegraph_core::mock::MockEngine::new();
    codegraph::ingest::mox_ingest::ingest_mox_files(
        &mock,
        &mock,
        &mox_files,
        &config,
        &config.defaults.type_suffix,
    )
    .await
    .unwrap();

    for engine in [&grafeo as &dyn GraphQuerier, &mock] {
        let children = engine.get_child_schemas("TodoListType").await.unwrap();
        let titles: Vec<_> = children.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["TodoItemType"],
            "engine {} must derive the refers-array child",
            if std::ptr::eq(engine, &grafeo) {
                "grafeo"
            } else {
                "mock"
            }
        );
        assert!(children[0].is_entity, "derived child must be the entity");

        // Non-children stay non-children: the reverse direction resolves
        // nothing (the parent is not a child of its items).
        let reverse = engine.get_child_schemas("TodoItemType").await.unwrap();
        assert!(
            reverse.is_empty(),
            "referenced entity must not gain phantom children"
        );
    }
}

// ── 2. mox-only model: child sections + derived child-side FK ───────────

#[tokio::test]
async fn mox_only_model_renders_detail_page_child_sections_without_config() {
    let fixture = write_mox_fixture();
    let output = fixture.root.path().join("generated");
    let mox_files = vec![fixture.mox.clone()];

    codegraph::driver::run(run_args(
        &fixture.config,
        None,
        None,
        &mox_files,
        &output,
        fixture.root.path(),
    ))
    .await
    .unwrap();

    let files = collect_files(&output);

    // Exactly one child section for the derived child, on the parent's
    // detail page (module name `todo_item` → `child-section-todo_item`).
    assert_eq!(
        count_page_occurrences(&files, r#"data-testid="child-section-todo_item""#),
        1,
        "the mox refers-array child must render exactly one detail-page section"
    );
    assert_eq!(
        count_page_occurrences(&files, r#"data-testid="child-section-todo_item-add-btn""#),
        1,
        "the section's add button must render with it"
    );

    // Non-children must not gain sections: TodoItemType is not a parent.
    assert_eq!(
        count_page_occurrences(&files, r#"data-testid="child-section-todo_list""#),
        0,
        "the referenced entity must not render a phantom child section"
    );

    // The FK-on-child lowering is unchanged: todo_item carries the derived
    // todo_list_id FK (guard — the section rides the same relationship).
    let ddl: String = files
        .iter()
        .filter(|(path, _)| path.starts_with("migrations/") && path.ends_with(".sql"))
        .map(|(_, content)| content.as_str())
        .collect();
    assert!(
        ddl.contains("todo_item") && ddl.contains("todo_list_id"),
        "DDL must keep the derived child-side FK:\n{ddl}"
    );
}

// ── 3. Config stays authoritative; both routes dedupe to one section ────

#[tokio::test]
async fn config_declared_child_wins_and_dedupes_with_derived_child() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("domains.toml"),
        DOMAINS_TOML_WITH_CHILD_CONFIG,
    )
    .unwrap();
    fs::create_dir_all(root.path().join("schemas/todo")).unwrap();
    fs::write(
        root.path().join("schemas/todo/TodoListType.json"),
        TODO_LIST_JSON,
    )
    .unwrap();
    fs::write(
        root.path().join("schemas/todo/TodoItemType.json"),
        TODO_ITEM_JSON,
    )
    .unwrap();

    let config = root.path().join("domains.toml");
    let schemas = root.path().join("schemas");
    let output = root.path().join("generated");

    codegraph::driver::run(run_args(
        &config,
        Some(&schemas),
        Some(Path::new("tests/fixtures/classifier.toml")),
        &[],
        &output,
        root.path(),
    ))
    .await
    .unwrap();

    let files = collect_files(&output);

    // Both routes resolve TodoItemType — config first, graph derived second.
    // The section must render exactly once.
    assert_eq!(
        count_page_occurrences(&files, r#"data-testid="child-section-todo_item""#),
        1,
        "config-declared and graph-derived child must dedupe to ONE section"
    );

    // Config authority is visible in the output: the section navigates with
    // the config-declared path_segment, not a graph-derived duplicate's.
    assert_eq!(
        count_page_occurrences(&files, "startAddChild('todo', 'config-items')"),
        1,
        "the surviving section must carry the config-declared path_segment"
    );
}
