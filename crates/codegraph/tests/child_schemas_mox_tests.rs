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

/// Fallback fixture: a child whose only field is a required integer, so no
/// readable display field exists and rows must fall back to `child.id`.
const COUNTER_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "CounterType",
  "description": "A named counter.",
  "type": "object",
  "properties": {
    "name": { "description": "Counter name.", "type": "string" },
    "entries": {
      "description": "The entries on this counter.",
      "type": "array",
      "items": { "$ref": "TallyEntryType.json#" }
    }
  },
  "required": ["name"]
}"#;

const TALLY_ENTRY_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "TallyEntryType",
  "description": "An entry with no readable label field.",
  "type": "object",
  "properties": {
    "quantity": { "description": "How much.", "type": "integer" }
  },
  "required": ["quantity"]
}"#;

const DOMAINS_TOML_COUNTER: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.tally]
label = "Tally"
schema_dir = "tally"
postgres_schema = "tally"
entities = ["CounterType", "TallyEntryType"]

[domains.tally.entity_config.TallyEntryType]
role = "child"
parent = "CounterType"
parent_ref = "counter_id"
path_segment = "entries"
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

    // The shared ChildSection component ships once for the run (issue
    // #468 W4) — the detail page renders prop-driven tags, the component
    // carries the testid-bearing markup.
    let component = files
        .get("ui/src/lib/components/ChildSection.svelte")
        .expect("the run must emit the shared ChildSection component");
    assert!(
        component.contains("data-testid={childSectionTid}")
            && component.contains("data-testid={addBtnTid}"),
        "the component must render the section testids from props:\n{component}"
    );

    // Exactly one child section for the derived child, on the parent's
    // detail page (module name `todo_item` → `child-section-todo_item`).
    assert_eq!(
        count_page_occurrences(&files, r#"childSectionTid="child-section-todo_item""#),
        1,
        "the mox refers-array child must render exactly one detail-page section"
    );
    assert_eq!(
        count_page_occurrences(&files, r#"addBtnTid="child-section-todo_item-add-btn""#),
        1,
        "the section's add button must render with it"
    );

    // Non-children must not gain sections: TodoItemType is not a parent.
    assert_eq!(
        count_page_occurrences(&files, r#"childSectionTid="child-section-todo_list""#),
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
        count_page_occurrences(&files, r#"childSectionTid="child-section-todo_item""#),
        1,
        "config-declared and graph-derived child must dedupe to ONE section"
    );

    // Config authority is visible in the output: the section navigates with
    // the config-declared path_segment, not a graph-derived duplicate's.
    assert_eq!(
        count_page_occurrences(&files, r#"pathSegment="config-items""#),
        1,
        "the surviving section must carry the config-declared path_segment"
    );
}

// ── 4. Typed child loads + display-field row labels (issue #468 W2) ─────

/// The todo_list detail page loads its child rows as `TodoItemResponse[]`
/// (never `any[]`), and the child rows render the child's resolved display
/// field (`title`) — the tag passes `displayField` to the shared
/// ChildSection component, whose `labelFor` resolves it at runtime. The
/// descriptor carries the same resolution for both the child entry and the
/// items entity-ref.
#[tokio::test]
async fn detail_page_child_loads_are_typed_and_rows_render_display_field() {
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

    let detail_page = files
        .iter()
        .find(|(path, content)| {
            path.ends_with("+page.svelte") && content.contains("child-section-todo_item")
        })
        .map(|(_, content)| content)
        .expect("the todo_list detail page must render the todo_item child section");

    let import_line =
        "import type { TodoListResponse, TodoItemResponse } from '#lib/api/types.ts';";
    assert!(
        detail_page.contains(import_line),
        "child DTO types must ride the existing type-only import:\n{detail_page}"
    );
    assert!(
        detail_page.contains("let todo_itemItems = $state<TodoItemResponse[]>([]);"),
        "child section state must be typed:\n{detail_page}"
    );
    assert!(
        detail_page.contains("apiGet<TodoItemResponse[]>('/todo/todo-item'"),
        "child section fetch must be typed:\n{detail_page}"
    );
    assert!(
        !detail_page.contains("$state<any[]>") && !detail_page.contains("apiGet<any[]>"),
        "the child-load region must not fall back to any[]:\n{detail_page}"
    );

    let label_const = r#"{@const label = (child as Record<string, unknown>)['title'] ?? child.id}"#;
    assert!(
        !detail_page.contains(label_const),
        "row-label resolution moved into the component (issue #468 W4):\n{detail_page}"
    );
    assert!(
        detail_page.contains(r#"displayField="title""#),
        "the resolved display field must ride the component tag:\n{detail_page}"
    );
    assert!(
        !detail_page.contains("{child.id}"),
        "no raw-UUID fallback may render on the page when a display field exists:\n{detail_page}"
    );

    // The component resolves the row label at runtime: display field when
    // passed, raw id otherwise — wired through the same prop the tag sets.
    let component = files
        .get("ui/src/lib/components/ChildSection.svelte")
        .expect("the run must emit the shared ChildSection component");
    let label_for = "const labelFor = (item: T): string =>";
    assert!(
        component.contains(label_for),
        "the component must own row-label resolution:\n{component}"
    );
    assert!(
        component.contains(
            "((item as Record<string, unknown>)[displayField] as string | undefined) ?? item.id"
        ),
        "the display-field read must be the single-cast form:\n{component}"
    );
    assert!(
        component.contains(": item.id;"),
        "the raw-id fallback must stay:\n{component}"
    );

    let descriptor = files
        .iter()
        .find(|(path, content)| {
            path.ends_with("descriptor.ts") && content.contains("TodoListDescriptor")
        })
        .map(|(_, content)| content)
        .expect("todo_list must generate a descriptor");

    let child_block = "\
      name: 'todo_item',\n      label: 'TodoItem',\n      entity: 'TodoItem',\n      relationship: 'one-to-many',\n      inline: false,\n      displayField: 'title',";
    assert!(
        descriptor.contains(child_block),
        "the child entry must carry the resolved displayField:\n{descriptor}"
    );
    assert!(
        descriptor
            .contains("ref: { entity: 'TodoItemType', domain: 'todo', displayField: 'title' }"),
        "the items entity-ref must resolve displayField over the target's fields:\n{descriptor}"
    );
}

/// A child with no readable field (required integer only) passes
/// `displayField={null}` on the tag and keeps the raw `item.id` label in
/// the component's `labelFor` fallback — no `@const` branch, and the
/// descriptor falls back to `displayField: 'id'`.
#[tokio::test]
async fn child_without_readable_field_falls_back_to_id() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("domains.toml"), DOMAINS_TOML_COUNTER).unwrap();
    fs::create_dir_all(root.path().join("schemas/tally")).unwrap();
    fs::write(
        root.path().join("schemas/tally/CounterType.json"),
        COUNTER_JSON,
    )
    .unwrap();
    fs::write(
        root.path().join("schemas/tally/TallyEntryType.json"),
        TALLY_ENTRY_JSON,
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

    let detail_page = files
        .iter()
        .find(|(path, content)| {
            path.ends_with("+page.svelte") && content.contains("child-section-tally_entry")
        })
        .map(|(_, content)| content)
        .expect("the counter detail page must render the tally_entry child section");

    assert!(
        detail_page.contains("let tally_entryItems = $state<TallyEntryResponse[]>([]);"),
        "typed loads apply even without a display field:\n{detail_page}"
    );
    assert!(
        !detail_page.contains("@const label"),
        "no display-field const may render when none resolves:\n{detail_page}"
    );
    assert!(
        detail_page.contains("displayField={null}"),
        "the tag must pass the null display field through:\n{detail_page}"
    );

    // The raw-id fallback lives in the component's labelFor (issue #468
    // W4) — the same static file every run emits.
    let component = files
        .get("ui/src/lib/components/ChildSection.svelte")
        .expect("the run must emit the shared ChildSection component");
    assert!(
        component.contains(": item.id;"),
        "rows must fall back to the raw id in the component:\n{component}"
    );

    let descriptor = files
        .iter()
        .find(|(path, content)| {
            path.ends_with("descriptor.ts") && content.contains("CounterDescriptor")
        })
        .map(|(_, content)| content)
        .expect("counter must generate a descriptor");

    assert!(
        descriptor.contains("displayField: 'id'"),
        "the child entry must fall back to displayField: 'id':\n{descriptor}"
    );
    assert!(
        !descriptor.contains("displayField: 'quantity'"),
        "non-string scalars must never become the displayField:\n{descriptor}"
    );
}
