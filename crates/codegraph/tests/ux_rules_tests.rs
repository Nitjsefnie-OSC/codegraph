//! Driver-level content pins for the ux-rules plane (issues #297–#306,
//! consolidated under #304).
//!
//! # Where ux-rules coverage lives (the split, per issue #304)
//!
//! - **THIS file + `ux_rules_byte_identity_tests.rs`** — the PR-CI,
//!   node-free safety net: driver-level content pins over real pipeline
//!   runs (flag ON renders the contract, flag OFF renders none of it),
//!   the diagnostics report() pins, the generated-spec content pins, and
//!   the byte-identity canaries for BOTH pipelines.
//! - **`ui_e2e_test_tests.rs` / `ifml_e2e_tests.rs` +
//!   `codegraph-generate` unit tests** — generator-level pins (mock
//!   engines, rendered-spec content); the fast inner loop for emitter
//!   changes. Flag-OFF negatives in `ifml_template_tests.rs` stay there —
//!   they guard that file's committed byte-identical fixtures.
//! - **`ifml_codegen_gate.rs`** (nightly, `--ignored`) — the real-API
//!   proof: the generated Playwright specs actually run against a live
//!   backend. Never in PR CI (node).
//!
//! Consolidation rule (#304): nothing here deletes or weakens the other
//! files' pins; this file adds the umbrella coverage those files cannot
//! see (full-driver runs) and cross-references the rest.
//!
//! `ux_rules = true` is ON in the default profile, so the mox-first
//! pipeline renders ux markup into every generated list page. These tests
//! pin the #297 contract over a rich fixture model (money, quantity,
//! codelist status, timestamps, identifier):
//!
//! - chip Badge with tone variant (`-chip` testid)
//! - `text-right tabular-nums` on numeric/money header + cell
//! - `Intl.NumberFormat` currency formatting
//! - copy-chip button (`-copy` testid)
//! - row-actions dropdown (`-actions` / `-actions-menu` testids)
//! - Delete behind an AlertDialog confirm
//! - Tooltip wrapper for truncated cells
//! - zebra row shading + the `data-inactive` branch
//! - readable-first column order
//!
//! Flag OFF (the byte-identity canary's shape) renders none of these.
//! Fixture pattern: the `init_tests` mox-first lifecycle (cmd_init →
//! `driver::run` with ONLY `--mox-files` + config), no node, no DB.

use std::fs;
use std::path::{Path, PathBuf};

use codegraph::init::commands::{InitArgs, cmd_init};
use tempfile::TempDir;

/// Absolute repo root (`<repo>/crates/codegraph` → two parents up).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("codegraph crate lives at <repo>/crates/codegraph")
        .to_path_buf()
}

fn init_args(dir: &Path) -> InitArgs {
    InitArgs {
        name: Some("demo-app".to_string()),
        output_dir: dir.to_path_buf(),
        domains: vec!["common".to_string()],
        database_target: "postgres".to_string(),
        persistence_provider: "sea_orm".to_string(),
        deployment_topology: "monolith".to_string(),
        grpc: false,
        ifml: false,
        ops: true,
        rosetta: false,
        rev: Some("abc123".to_string()),
        codegraph_path: Some(repo_root()),
        force: false,
        template_dirs: vec![],
    }
}

/// A rich entity exercising every dimension the list template branches on.
const TASK_MODEL: &str = r#"package common

/// Task lifecycle status.
enum TaskStatus {
    Draft as "Draft" = 0
    Active as "Active" = 1
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

/// A trackable task.
class TaskType {
    /// Primary identifier.
    UUID id
    /// Task name.
    String name
    /// Optional longer description.
    String [0..1] description
    /// Units of work.
    Long quantity
    /// Billed amount.
    Double [0..1] total_amount
    /// When the task is due.
    Date [0..1] due_date
    /// Creation stamp.
    Timestamp [0..1] created_at
    /// Lifecycle status.
    TaskStatus status
    /// Child subtasks managed from the task detail page.
    refers SubTaskType[] sub_tasks
}

/// A subtask under a task (child section on the task detail page).
class SubTaskType {
    /// Primary identifier.
    UUID id
    /// Subtask title.
    String title
    /// Creation stamp.
    Timestamp [0..1] created_at
}
"#;

/// Scaffold the fixture and replace the starter model with the rich task
/// model. Returns (project dir, mox file path).
fn fixture(dir: &TempDir) -> (PathBuf, PathBuf) {
    cmd_init(&init_args(dir.path())).unwrap();
    let project = dir.path().join("demo-app");
    let mox = project.join("model/common.mox");
    fs::write(&mox, TASK_MODEL).unwrap();
    (project, mox)
}

/// Fixture paths bound to locals so `RunArgs` can borrow them across the
/// driver await.
struct FixtureRun {
    mox_files: Vec<PathBuf>,
    config: PathBuf,
    output: PathBuf,
    profiles: PathBuf,
}

impl FixtureRun {
    fn args<'a>(&'a self, ux_rules: Option<&'a Path>) -> codegraph::driver::RunArgs<'a> {
        codegraph::driver::RunArgs {
            schemas: None,
            classifier: None,
            config_path: &self.config,
            output: &self.output,
            extension_points_path: None,
            profile_name: "default",
            variant: None,
            profiles_config_path: Some(self.profiles.clone()),
            no_post_gen: true,
            template_dir: &[],
            ifml_files: &[],
            openapi_files: &[],
            mox_files: &self.mox_files,
            rosetta_files: &[],
            ddd_files: &[],
            evt_files: &[],
            deploy_files: &[],
            ifml_framework: &[],
            ifml_components: None,
            ifml_design_system: None,
            ux_rules,
            codegraph_rev: None,
            check: false,
        }
    }

    fn run(&self, ux_rules: Option<&Path>) {
        tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async { codegraph::driver::run(self.args(ux_rules)).await.unwrap() });
    }
}

/// Every generated list page under `generated/ui`.
fn list_pages(project: &Path) -> Vec<PathBuf> {
    let ui = project.join("generated/ui/src/routes");
    walkdir::WalkDir::new(&ui)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some("+page.svelte"))
        .collect()
}

/// The default profile (flag ON) renders the full ux contract into the
/// task list page.
#[test]
fn ux_rules_default_profile_renders_list_page_pins() {
    let dir = TempDir::new().unwrap();
    let (project, mox) = fixture(&dir);
    let run = FixtureRun {
        config: project.join("domains.toml"),
        output: project.join("generated"),
        profiles: project.join("profiles.toml"),
        mox_files: vec![mox],
    };
    run.run(None);

    let pages = list_pages(&project);
    assert!(
        !pages.is_empty(),
        "flag-on generation must produce list pages"
    );
    let task_page = pages
        .iter()
        .find(|p| p.to_string_lossy().ends_with("/common/task/+page.svelte"))
        .unwrap_or_else(|| panic!("the task list page must be generated, pages: {pages:?}"));
    let page = fs::read_to_string(task_page).unwrap();

    // 1. Chips: Badge with the tone-mapped variant + stable testid.
    assert!(page.contains(r#"data-testid="task-chip""#), "{page}");
    assert!(page.contains("variant={toneFor(col, value)}"), "{page}");

    // 2. Numeric/money alignment on header AND cells.
    assert_eq!(
        page.matches("text-right tabular-nums").count(),
        2,
        "one header + one cell occurrence:\n{page}"
    );

    // 3. Locale/currency formatting via Intl.
    assert!(page.contains("Intl.NumberFormat"), "{page}");
    assert!(page.contains("currency: 'NZD'"), "{page}");
    assert!(
        page.contains("Intl.DateTimeFormat('en-NZ', { dateStyle: 'medium', timeStyle: 'short' })"),
        "{page}"
    );

    // 4. Copy chips.
    assert!(page.contains(r#"data-testid="task-copy""#), "{page}");

    // 5. Row actions dropdown.
    assert!(page.contains(r#"data-testid="task-actions""#), "{page}");
    assert!(
        page.contains(r#"data-testid="task-actions-menu""#),
        "{page}"
    );

    // 6. Delete behind the AlertDialog confirm.
    assert!(page.contains("AlertDialog"), "{page}");
    assert!(
        page.contains(r#"data-testid="task-delete-dialog""#),
        "{page}"
    );
    assert!(page.contains("requestDelete"), "{page}");
    assert!(page.contains("apiDelete"), "{page}");
    assert!(page.contains("invalidateAll"), "{page}");

    // 7. Tooltip wrapper on truncated cells.
    assert!(page.contains("Tooltip.Root"), "{page}");
    assert!(page.contains("truncate(value, 8)"), "{page}");

    // 8. Zebra rows + the inactive-row branch (auditable entities carry
    //    the deleted_at marker by default).
    assert!(page.contains("bg-muted/50"), "{page}");
    assert!(page.contains("data-inactive="), "{page}");
    assert!(
        page.contains("if (row['deleted_at'] != null) return true;"),
        "{page}"
    );

    // 9. Readable-first column order: `name` leads, `id` trails,
    //    `created_at` is last.
    let name_pos = page.find("key: 'name'").expect("name column");
    let id_pos = page.find("key: 'id'").expect("id column");
    let created_pos = page.find("key: 'created_at'").expect("created_at column");
    assert!(name_pos < id_pos, "name must lead:\n{page}");
    assert!(id_pos < created_pos, "audit stamp must trail:\n{page}");

    // Pre-existing behaviors stay intact.
    assert!(page.contains(r#"data-testid="task-table""#), "{page}");
    assert!(page.contains("paraglide/messages.js"), "{page}");
    assert!(page.contains("m.common_"), "{page}");
    assert!(page.contains("handleRowClick"), "{page}");
    assert!(page.contains("onkeydown"), "{page}");
    assert!(page.contains("focus-visible"), "{page}");
    assert!(page.contains(r#"data-testid="task-create-btn""#), "{page}");
    assert!(page.contains(r#"data-testid="task-search""#), "{page}");
    assert!(page.contains(r#"data-testid="task-empty""#), "{page}");
    assert!(page.contains(r#"data-testid="task-pagination""#), "{page}");
}

/// Flag OFF: none of the ux markup renders (the content-level counterpart
/// of the byte-identity canary).
#[test]
fn ux_rules_flag_off_renders_no_ux_markup() {
    let dir = TempDir::new().unwrap();
    let (project, mox) = fixture(&dir);

    let profiles_path = project.join("profiles.toml");
    let profiles = fs::read_to_string(&profiles_path).unwrap();
    let flag_off = profiles.replace("ux_rules = true", "ux_rules = false");
    assert_ne!(profiles, flag_off, "scaffold must ship the ux_rules flag");
    fs::write(&profiles_path, flag_off).unwrap();

    let run = FixtureRun {
        config: project.join("domains.toml"),
        output: project.join("generated"),
        profiles: profiles_path,
        mox_files: vec![mox],
    };
    run.run(None);

    let pages = list_pages(&project);
    assert!(!pages.is_empty(), "flag-off generation still emits pages");
    // Only ENTITY LIST pages are ux-rendered; scaffold-authored pages and
    // detail pages legitimately carry AlertDialog/dropdowns already.
    let list_pages_only: Vec<&PathBuf> = pages
        .iter()
        .filter(|p| !p.to_string_lossy().contains("/["))
        .collect();
    for page_path in list_pages_only {
        let page = fs::read_to_string(page_path).unwrap();
        for needle in [
            "-chip",
            "-actions",
            "-copy",
            "Intl.NumberFormat",
            "data-inactive",
            "Tooltip",
            "DropdownMenu",
            "AlertDialog",
            "invalidateAll",
            "tabular-nums",
            "rowClass",
            "-timeline",
        ] {
            assert!(
                !page.contains(needle),
                "flag-off page {} must not contain {needle:?}:\n{page}",
                page_path.display()
            );
        }
    }
}

/// `--ux-rules` merges project rules ahead of the pack: a project rule
/// pinning the money column's display is honored in the rendered page.
#[test]
fn ux_rules_project_file_overrides_pack_display() {
    let dir = TempDir::new().unwrap();
    let (project, mox) = fixture(&dir);

    let ux_rules_path = project.join("ux-rules.toml");
    fs::write(
        &ux_rules_path,
        "[[column]]\nname_pattern = \"total_amount\"\ndisplay = \"chip\"\n\n[actions]\ninline_max = 2\n",
    )
    .unwrap();

    let run = FixtureRun {
        config: project.join("domains.toml"),
        output: project.join("generated"),
        profiles: project.join("profiles.toml"),
        mox_files: vec![mox],
    };
    run.run(Some(ux_rules_path.as_path()));

    let pages = list_pages(&project);
    let task_page = pages
        .iter()
        .find(|p| p.to_string_lossy().ends_with("/common/task/+page.svelte"))
        .expect("task list page");
    let page = fs::read_to_string(task_page).unwrap();

    // The money column renders as a chip (project rule beats the pack's
    // copy-chip/raw tiering)...
    let amount_pos = page
        .find("key: 'total_amount'")
        .expect("total_amount column");
    let amount_slice = &page[amount_pos..];
    let line_end = amount_slice.find('\n').unwrap_or(amount_slice.len());
    assert!(
        amount_slice[..line_end].contains("display: 'chip'"),
        "project rule pins the money column to a chip:\n{}",
        &amount_slice[..line_end]
    );
    // ...and the pack's delete-confirmation default survives the merge
    // (the project file replaced only inline_max). Note the action-tiering
    // itself is runtime state (Svelte branches), so only the confirm
    // dialog — generation-time markup — is content-pinned here.
    assert!(
        page.contains(r#"data-testid="task-action-delete""#),
        "{page}"
    );
    assert!(
        page.contains(r#"data-testid="task-delete-dialog""#),
        "{page}"
    );
}

/// An explicit `[[collection]] display = "timeline"` rule renders the
/// timeline layout (issue #298): vertical rail entries with the order
/// field formatted through the shared Intl helper, DESC sort, title link,
/// preview fields through the SHARED cell formatter (chip + money), the
/// row-actions dropdown, the workflow badge, and NO table markup. The
/// shared load logic (search, create, empty state, pagination) survives.
#[test]
fn ux_rules_timeline_rule_renders_timeline_layout() {
    let dir = TempDir::new().unwrap();
    let (project, mox) = fixture(&dir);

    // Workflow on Task so the timeline badge branch renders.
    let domains_path = project.join("domains.toml");
    let mut domains = fs::read_to_string(&domains_path).unwrap();
    domains.push_str(
        "\n[domains.common.entity_config.TaskType.workflow]\n\
         status_field = \"status\"\n\
         initial_state = \"draft\"\n\
         states = [\"draft\", \"active\", \"archived\"]\n\
         terminal_states = [\"archived\"]\n\
         generate_action_endpoints = true\n",
    );
    fs::write(&domains_path, domains).unwrap();

    let ux_rules_path = project.join("ux-rules.toml");
    fs::write(
        &ux_rules_path,
        "[[collection]]\n\
         entity_pattern = \"Task*\"\n\
         display = \"timeline\"\n\
         order_by = \"created_at\"\n\
         title_field = \"name\"\n\
         preview = [\"status\", \"total_amount\"]\n",
    )
    .unwrap();

    let run = FixtureRun {
        config: domains_path,
        output: project.join("generated"),
        profiles: project.join("profiles.toml"),
        mox_files: vec![mox],
    };
    run.run(Some(ux_rules_path.as_path()));

    let pages = list_pages(&project);
    let task_page = pages
        .iter()
        .find(|p| p.to_string_lossy().ends_with("/common/task/+page.svelte"))
        .unwrap_or_else(|| panic!("task list page must be generated, pages: {pages:?}"));
    let page = fs::read_to_string(task_page).unwrap();

    // 1. Timeline root + per-entry testids (ids::TIMELINE / TIMELINE_ITEM).
    assert!(page.contains(r#"data-testid="task-timeline""#), "{page}");
    assert!(
        page.contains(r#"data-testid="task-timeline-item""#),
        "{page}"
    );

    // 2. The order field renders through the shared Intl helper.
    assert!(page.contains("function formatDate("), "{page}");
    assert!(
        page.contains("Intl.DateTimeFormat('en-NZ', { dateStyle: 'medium' })"),
        "{page}"
    );
    assert!(page.contains("const orderKey = 'created_at';"), "{page}");
    assert!(
        page.contains("{formatDate((row as Record<string, unknown>)[orderKey])}"),
        "{page}"
    );

    // 3. DESC sort (newest first) over the shared rows.
    assert!(page.contains("[...displayRows].sort"), "{page}");
    assert!(
        page.contains("const av = String((a as Record<string, unknown>)['created_at'] ?? '');"),
        "{page}"
    );
    assert!(
        page.contains("return av < bv ? 1 : av > bv ? -1 : 0;"),
        "{page}"
    );

    // 4. Title link over the entity base path.
    assert!(page.contains("const titleKey = 'name';"), "{page}");
    assert!(page.contains("href={`${basePath}/${row['id']}`}"), "{page}");

    // 5. Preview fields render through the SHARED cell formatter — the
    //    same chip/money branches the table uses, no divergent code.
    assert!(
        page.contains("const previewKeys = ['status', 'total_amount'];"),
        "{page}"
    );
    assert!(page.contains("timeline-meta"), "{page}");
    assert!(page.contains(r#"data-testid="task-chip""#), "{page}");
    assert!(page.contains("variant={toneFor(col, value)}"), "{page}");
    assert!(page.contains("formatMoney(value)"), "{page}");

    // 6. Row-actions dropdown + AlertDialog delete confirm (same pattern
    //    as the table rows).
    assert!(page.contains(r#"data-testid="task-actions""#), "{page}");
    assert!(
        page.contains(r#"data-testid="task-actions-menu""#),
        "{page}"
    );
    assert!(
        page.contains(r#"data-testid="task-action-delete""#),
        "{page}"
    );
    assert!(
        page.contains(r#"data-testid="task-delete-dialog""#),
        "{page}"
    );

    // 7. Workflow badge branch compiles in (WorkflowPanel variant ladder).
    assert!(page.contains("function workflowVariant("), "{page}");
    assert!(
        page.contains(r#"data-testid="task-workflow-state""#),
        "{page}"
    );

    // 8. Table markup absent in timeline mode.
    assert!(!page.contains(r#"data-testid="task-table""#), "{page}");
    assert!(!page.contains("<Table.Root"), "{page}");
    assert!(!page.contains("Table.Head"), "{page}");

    // 9. Shared load logic intact: search, create, empty state, no-results
    //    and pagination all keep their testids.
    for testid in [
        "task-search",
        "task-create-btn",
        "task-empty",
        "task-no-results",
        "task-pagination",
    ] {
        assert!(
            page.contains(&format!(r#"data-testid="{testid}""#)),
            "missing {testid}:\n{page}"
        );
    }
    assert!(page.contains("const displayRows = $derived("), "{page}");
    assert!(page.contains("handlePageChange"), "{page}");
}

/// An explicit `display = "table"` collection rule is a deliberate opt-OUT:
/// the timeline never renders and the table stays byte-shaped.
#[test]
fn ux_rules_explicit_table_rule_never_renders_timeline() {
    let dir = TempDir::new().unwrap();
    let (project, mox) = fixture(&dir);

    let ux_rules_path = project.join("ux-rules.toml");
    fs::write(
        &ux_rules_path,
        "[[collection]]\nentity_pattern = \"Task*\"\ndisplay = \"table\"\n",
    )
    .unwrap();

    let run = FixtureRun {
        config: project.join("domains.toml"),
        output: project.join("generated"),
        profiles: project.join("profiles.toml"),
        mox_files: vec![mox],
    };
    run.run(Some(ux_rules_path.as_path()));

    let pages = list_pages(&project);
    let task_page = pages
        .iter()
        .find(|p| p.to_string_lossy().ends_with("/common/task/+page.svelte"))
        .expect("task list page");
    let page = fs::read_to_string(task_page).unwrap();

    assert!(page.contains(r#"data-testid="task-table""#), "{page}");
    for needle in [
        "task-timeline",
        "timeline-item",
        "timeline-meta",
        "formatDate(",
        "orderKey",
        "titleKey",
        "previewKeys",
        "displayRowsSorted",
        "timeline-rail",
    ] {
        assert!(
            !page.contains(needle),
            "table rule must not leak {needle:?}:\n{page}"
        );
    }
}

/// An unresolvable `order_by` is a generation error naming the candidate
/// time-point fields: the ui-page generator fails for the entity and the
/// timeline layout never reaches the output tree.
#[test]
fn ux_rules_timeline_unresolvable_order_by_skips_entity_page() {
    let dir = TempDir::new().unwrap();
    let (project, mox) = fixture(&dir);

    let ux_rules_path = project.join("ux-rules.toml");
    fs::write(
        &ux_rules_path,
        "[[collection]]\n\
         entity_pattern = \"Task*\"\n\
         display = \"timeline\"\n\
         order_by = \"quantity\"\n",
    )
    .unwrap();

    let run = FixtureRun {
        config: project.join("domains.toml"),
        output: project.join("generated"),
        profiles: project.join("profiles.toml"),
        mox_files: vec![mox],
    };
    // The driver records the ui-page failure in its report and completes;
    // the plan error (with the candidate fields) is printed to stderr.
    run.run(Some(ux_rules_path.as_path()));

    let task_page = project.join("generated/ui/src/routes/(app)/common/task/+page.svelte");
    assert!(
        !task_page.exists(),
        "unresolvable order_by must fail the ui-page generation: {}",
        task_page.display()
    );
}

/// Declare `SubTaskType` as a config child of `TaskType` in the fixture's
/// `domains.toml` (the `role = "child"` + `parent` contract
/// `collect_child_sections` scans for).
fn add_child_config(project: &Path) {
    let domains_path = project.join("domains.toml");
    let mut domains = fs::read_to_string(&domains_path).unwrap();
    domains.push_str(
        "\n[domains.common.entity_config.SubTaskType]\n\
         role = \"child\"\n\
         parent = \"TaskType\"\n",
    );
    fs::write(&domains_path, domains).unwrap();
}

/// Child sections on the detail page tier their actions behind the
/// parent's ux plan (issue #299; prop-driven since the #468 W4
/// extraction): the page's `<ChildSection …>` tag carries the action
/// partition props (`menuActions`, `confirmDelete`) and the testid
/// vocabulary, while the shared `ChildSection.svelte` component renders
/// the per-item dropdown (`{child}-actions` / `{child}-actions-menu`),
/// the AlertDialog confirm, and the inline Manage → affordance. The
/// detail header keeps its primary Edit/Delete buttons, and the scaffold
/// ships the primitive install surface (`ui/PRIMITIVES.md`).
#[test]
fn ux_rules_child_sections_tier_actions_behind_menu() {
    let dir = TempDir::new().unwrap();
    let (project, mox) = fixture(&dir);
    add_child_config(&project);
    let run = FixtureRun {
        config: project.join("domains.toml"),
        output: project.join("generated"),
        profiles: project.join("profiles.toml"),
        mox_files: vec![mox],
    };
    run.run(None);

    let detail_path =
        project.join("generated/ui/src/routes/(app)/common/task/[task_id]/+page.svelte");
    let page = fs::read_to_string(&detail_path).expect("task detail page must be generated");
    let component =
        fs::read_to_string(project.join("generated/ui/src/lib/components/ChildSection.svelte"))
            .expect("the shared ChildSection component must be generated");

    // 1. The tag carries the per-item menu testids (child module name =
    //    `sub_task`); the component renders them from props.
    assert!(page.contains(r#"actionsTid="sub_task-actions""#), "{page}");
    assert!(
        page.contains(r#"actionsMenuTid="sub_task-actions-menu""#),
        "{page}"
    );
    assert!(
        component.contains("data-testid={actionsTid}"),
        "{component}"
    );
    assert!(
        component.contains("data-testid={actionsMenuTid}"),
        "{component}"
    );

    // 2. Edit/Delete live in the menu (component markup, tag-carried
    //    testids); the accordion + add affordances stay put. The Manage →
    //    link renders unconditionally from the component (the W4
    //    extraction dropped the generation-time has-children gate).
    assert!(
        page.contains(r#"actionEditTid="sub_task-action-edit""#),
        "{page}"
    );
    assert!(
        page.contains(r#"actionDeleteTid="sub_task-action-delete""#),
        "{page}"
    );
    assert!(
        component.contains("data-testid={actionEditTid}"),
        "{component}"
    );
    assert!(
        component.contains("data-testid={actionDeleteTid}"),
        "{component}"
    );
    assert!(
        page.contains(r#"addBtnTid="child-section-sub_task-add-btn""#),
        "{page}"
    );
    // The legacy flat-delete wiring must not leak onto the page.
    assert!(!page.contains("deleteChild("), "{page}");

    // 3. Delete behind the confirm: the tag partitions the actions and
    //    flips the confirm flag; the component wires the dialog.
    assert!(
        page.contains(
            "menuActions={[{ action: 'edit', label: 'Edit' }, { action: 'delete', label: 'Delete' }]}"
        ),
        "{page}"
    );
    assert!(page.contains("confirmDelete={ true }"), "{page}");
    assert!(component.contains("<DropdownMenu.Root>"), "{component}");
    assert!(
        component.contains("open={deleteId !== null}"),
        "{component}"
    );
    assert!(
        component.contains("data-testid={deleteDialogTid}"),
        "{component}"
    );
    assert!(
        component.contains("data-testid={deleteDialogConfirmTid}"),
        "{component}"
    );
    assert!(
        component.contains("onclick={confirmDeleteItem}"),
        "{component}"
    );

    // 4. The detail header keeps its primary Edit/Delete buttons.
    assert!(page.contains(r#"data-testid="task-edit-btn""#), "{page}");
    assert!(page.contains(r#"data-testid="task-delete-btn""#), "{page}");
    assert!(
        page.contains(r#"onclick={() => deleteDialogOpen = true}"#),
        "{page}"
    );

    // 5. The scaffold ships the primitive install surface listing the
    //    ux-rules primitives.
    let primitives = fs::read_to_string(project.join("generated/ui/PRIMITIVES.md"))
        .expect("PRIMITIVES.md must be emitted under the ux plane");
    assert!(primitives.contains("npx shadcn-svelte"), "{primitives}");
    assert!(primitives.contains("dropdown-menu"), "{primitives}");
    assert!(primitives.contains("tooltip"), "{primitives}");
}

/// Flag OFF: child sections keep the pre-#299 flat buttons, no menu/confirm
/// markup leaks, and the scaffold's primitive surface is absent.
#[test]
fn ux_rules_flag_off_child_sections_stay_flat() {
    let dir = TempDir::new().unwrap();
    let (project, mox) = fixture(&dir);
    add_child_config(&project);

    let profiles_path = project.join("profiles.toml");
    let profiles = fs::read_to_string(&profiles_path).unwrap();
    let flag_off = profiles.replace("ux_rules = true", "ux_rules = false");
    assert_ne!(profiles, flag_off, "scaffold must ship the ux_rules flag");
    fs::write(&profiles_path, flag_off).unwrap();

    let run = FixtureRun {
        config: project.join("domains.toml"),
        output: project.join("generated"),
        profiles: profiles_path,
        mox_files: vec![mox],
    };
    run.run(None);

    let detail_path =
        project.join("generated/ui/src/routes/(app)/common/task/[task_id]/+page.svelte");
    let page = fs::read_to_string(&detail_path).expect("task detail page must be generated");
    let component =
        fs::read_to_string(project.join("generated/ui/src/lib/components/ChildSection.svelte"))
            .expect("the shared ChildSection component must be generated");
    for needle in [
        "DropdownMenu",
        "DeleteId",
        "editChild(",
        "deleteChild(",
        "startAddChild",
    ] {
        assert!(
            !page.contains(needle),
            "flag-off detail page must not contain {needle:?}:\n{page}"
        );
    }
    // The flag-off partition is empty and unconfirmed: the component's
    // runtime tier dispatch keeps the flat legacy Edit/Delete buttons (in
    // the component file, not the page) and deletes directly.
    assert!(page.contains("menuActions={[]}"), "{page}");
    assert!(page.contains("confirmDelete={false}"), "{page}");
    assert!(
        page.contains(r#"childSectionTid="child-section-sub_task""#),
        "{page}"
    );
    assert!(
        component.contains("{#if menuActions.length > 0}"),
        "{component}"
    );
    assert!(component.contains("{:else}"), "{component}");
    assert!(component.contains("performDelete(child.id)"), "{component}");

    // No scaffold primitive surface under the flag.
    assert!(
        !project.join("generated/ui/PRIMITIVES.md").exists(),
        "PRIMITIVES.md must not be emitted with ux_rules off"
    );
}

/// The ux sort plane on generated list endpoints (issue #306): the
/// handler parses/validates `?sort=`/`?order=` against the plan's
/// allow-list (400 naming the valid fields), the query/repository layers
/// thread the spec into a quoted ORDER BY with a deterministic `id`
/// tiebreaker, and the list page renders `aria-sort` header buttons whose
/// state threads through pagination and FTS search.
#[test]
fn ux_rules_sort_plane_wires_handler_query_and_list_page() {
    let dir = TempDir::new().unwrap();
    let (project, mox) = fixture(&dir);
    let run = FixtureRun {
        config: project.join("domains.toml"),
        output: project.join("generated"),
        profiles: project.join("profiles.toml"),
        mox_files: vec![mox],
    };
    run.run(None);

    // ── API handler: param parsing + allow-list validation ──
    let handler = fs::read_to_string(project.join("generated/src/api/common/task_handler.rs"))
        .expect("task handler generated");
    assert!(
        handler.contains("const ALLOWED_SORT_FIELDS: &[&str] = &["),
        "{handler}"
    );
    // Scope to the allow-list block: the handler body legitimately
    // mentions other field names elsewhere (filters, params).
    let sort_block = handler
        .split("const ALLOWED_SORT_FIELDS")
        .nth(1)
        .and_then(|rest| rest.split("];").next())
        .unwrap_or_default();
    for field in [
        "name",
        "description",
        "quantity",
        "total_amount",
        "due_date",
        "created_at",
    ] {
        assert!(sort_block.contains(&format!("\"{field}\",")), "{handler}");
    }
    // Identifier/status/reference columns stay out of the allow-list.
    for field in ["id", "status", "sub_tasks"] {
        assert!(
            !sort_block.contains(&format!("\"{field}\",")),
            "allow-list must not contain {field:?}:\n{handler}"
        );
    }
    // serde defaults: absent params keep the default ordering.
    assert!(handler.contains("pub sort: Option<String>,"), "{handler}");
    assert!(handler.contains("pub order: Option<String>,"), "{handler}");
    // 400 texts name the valid sortable fields (include-path style).
    assert!(
        handler.contains("Unknown sort field: {sort}. Valid sortable fields: {}"),
        "{handler}"
    );
    assert!(
        handler.contains("order requires sort; valid sortable fields: {}"),
        "{handler}"
    );
    assert!(
        handler.contains("Invalid order: {other}. Valid values: asc, desc"),
        "{handler}"
    );
    // The validated spec rides the existing list-query path (RLS/context
    // bundle untouched). The fixture entity is auditable by default, so
    // the `include_deleted` slot (`false`) sits before the sort spec.
    assert!(
        handler.contains("list_filtered(params.page, params.page_size, &filters, false, ux_sort,"),
        "{handler}"
    );

    // ── Query + repository trait: signature threading ──
    let query = fs::read_to_string(project.join("generated/src/domain/common/task/query.rs"))
        .expect("task query generated");
    assert!(query.contains("sort: Option<(String, bool)>"), "{query}");
    assert!(
        query.contains("self.repo.list(&tx, page, page_size, filters, include_deleted, sort)"),
        "{query}"
    );
    let repo_trait =
        fs::read_to_string(project.join("generated/src/domain/common/task/repository.rs"))
            .expect("task repository trait generated");
    assert!(
        repo_trait.contains("sort: Option<(String, bool)>,"),
        "{repo_trait}"
    );

    // ── Repository impl: ORDER BY with quoted identifiers + tiebreaker ──
    let repo_impl =
        fs::read_to_string(project.join("generated/src/domain/common/task/repository_impl.rs"))
            .expect("task repository impl generated");
    assert!(
        repo_impl.contains("if let Some((sort_field, sort_desc)) = sort"),
        "{repo_impl}"
    );
    // Dialect-safe quoting: fully qualified sea_query aliases.
    assert!(
        repo_impl
            .contains("Alias::new(\"common\"), sea_orm::sea_query::Alias::new(\"task\"), sea_orm::sea_query::Alias::new(\"name\")"),
        "{repo_impl}"
    );
    // Deterministic `, id ASC` tiebreaker for stable pagination.
    assert!(
        repo_impl
            .contains("ordered.order_by_asc(sea_orm::sea_query::Expr::col((sea_orm::sea_query::Alias::new(\"common\"), sea_orm::sea_query::Alias::new(\"task\"), sea_orm::sea_query::Alias::new(\"id\"))))"),
        "{repo_impl}"
    );
    // Default (no params) keeps today's created_at DESC ordering.
    assert!(
        repo_impl.contains("query.order_by_desc(crate::entity::common_task::Column::CreatedAt)"),
        "{repo_impl}"
    );

    // ── List page: sortable header buttons + aria-sort + threading ──
    let page =
        fs::read_to_string(project.join("generated/ui/src/routes/(app)/common/task/+page.svelte"))
            .unwrap();
    assert!(
        page.contains("aria-sort={col.sortable ? sortStateFor(col.key) : undefined}"),
        "{page}"
    );
    assert!(
        page.contains(r#"data-testid="task-sort-{col.key}""#),
        "{page}"
    );
    assert!(
        page.contains("onclick={() => toggleSort(col.key)}"),
        "{page}"
    );
    assert!(page.contains("function sortIndicator("), "{page}");
    assert!(page.contains("'▲'"), "{page}");
    assert!(page.contains("'▼'"), "{page}");
    // Sort survives FTS search and pagination; sort changes reset the page.
    assert!(page.contains("function withSort("), "{page}");
    assert!(
        page.contains("if (searchQuery) params.set('q', searchQuery);"),
        "{page}"
    );
    // The load function forwards the params and returns the current state.
    let load = fs::read_to_string(
        project.join("generated/ui/src/routes/(app)/common/task/+page.server.ts"),
    )
    .unwrap();
    assert!(
        load.contains("apiUrl.searchParams.set('sort', sort);"),
        "{load}"
    );
    assert!(
        load.contains("apiUrl.searchParams.set('order', order === 'desc' ? 'desc' : 'asc');"),
        "{load}"
    );
    assert!(load.contains("sort: sort ?? null,"), "{load}");
}

/// Timeline collections keep their fixed DESC date order (issue #306 v1
/// scope): no sort buttons, no aria-sort, no sort plumbing in the load.
#[test]
fn ux_rules_sort_plane_stays_out_of_timeline_layout() {
    let dir = TempDir::new().unwrap();
    let (project, mox) = fixture(&dir);

    let ux_rules_path = project.join("ux-rules.toml");
    fs::write(
        &ux_rules_path,
        "[[collection]]\n\
         entity_pattern = \"Task*\"\n\
         display = \"timeline\"\n\
         order_by = \"created_at\"\n",
    )
    .unwrap();

    let run = FixtureRun {
        config: project.join("domains.toml"),
        output: project.join("generated"),
        profiles: project.join("profiles.toml"),
        mox_files: vec![mox],
    };
    run.run(Some(ux_rules_path.as_path()));

    let page =
        fs::read_to_string(project.join("generated/ui/src/routes/(app)/common/task/+page.svelte"))
            .expect("task timeline page");
    for needle in ["aria-sort", "toggleSort", "withSort", "-sort-"] {
        assert!(
            !page.contains(needle),
            "timeline page must not contain {needle:?}:\n{page}"
        );
    }
    let load = fs::read_to_string(
        project.join("generated/ui/src/routes/(app)/common/task/+page.server.ts"),
    )
    .unwrap();
    assert!(
        !load.contains("searchParams.set('sort'"),
        "timeline load must not forward sort:\n{load}"
    );
}

/// Flag OFF: no sort param handling anywhere — the handler/query/
/// repository surface stays byte-identical to pre-#306 output.
#[test]
fn ux_rules_flag_off_emits_no_sort_plane() {
    let dir = TempDir::new().unwrap();
    let (project, mox) = fixture(&dir);

    let profiles_path = project.join("profiles.toml");
    let profiles = fs::read_to_string(&profiles_path).unwrap();
    let flag_off = profiles.replace("ux_rules = true", "ux_rules = false");
    fs::write(&profiles_path, flag_off).unwrap();

    let run = FixtureRun {
        config: project.join("domains.toml"),
        output: project.join("generated"),
        profiles: profiles_path,
        mox_files: vec![mox],
    };
    run.run(None);

    let handler = fs::read_to_string(project.join("generated/src/api/common/task_handler.rs"))
        .expect("task handler generated");
    for needle in [
        "ALLOWED_SORT_FIELDS",
        "pub sort:",
        "pub order:",
        "Unknown sort field",
    ] {
        assert!(
            !handler.contains(needle),
            "flag-off handler must not contain {needle:?}:\n{handler}"
        );
    }
    assert!(
        handler
            .contains("list_filtered(params.page, params.page_size, &filters, false, api_key_info"),
        "pre-#306 call shape must stay:\n{handler}"
    );

    let repo_impl =
        fs::read_to_string(project.join("generated/src/domain/common/task/repository_impl.rs"))
            .expect("task repository impl generated");
    assert!(
        !repo_impl.contains("if let Some((sort_field, sort_desc)) = sort"),
        "{repo_impl}"
    );

    let page =
        fs::read_to_string(project.join("generated/ui/src/routes/(app)/common/task/+page.svelte"))
            .unwrap();
    for needle in ["aria-sort", "toggleSort", "withSort", "-sort-"] {
        assert!(
            !page.contains(needle),
            "flag-off page must not contain {needle:?}:\n{page}"
        );
    }
}

// ── Diagnostics pins (issue #304) ─────────────────────────────────────
//
// Limitation, named on purpose: in-process stderr capture is impractical
// (the drivers print through `eprintln!` deep inside generator code), so
// these tests pin at the `report()` boundary instead — the exact strings
// both generators emit — plus the shared stderr prefix as a constant.
// The actual print sites are
// `codegraph-generate/src/ui/page.rs` (`report_ux_diagnostics`, deduped
// per run) and `codegraph-generate/src/ifml/route_generator.rs`
// (`resolve_generation_ux` lines, deduped per generation); both frame the
// payload as `{UX_STDERR_PREFIX}{line}`.

/// The stderr framing both generators print ux diagnostic lines with.
const UX_STDERR_PREFIX: &str = "warning: ux-rules: ";

/// A minimal [`codegraph_generate::ui::page::UiField`] for the
/// fixture-shaped plan inputs below (no graph properties — the heuristics
/// under test run on the field alone).
fn ux_field(
    name: &str,
    pg_type: &str,
    ts_type: &str,
    input_type: &str,
) -> codegraph_generate::ui::page::UiField {
    codegraph_generate::ui::page::UiField {
        name: name.to_string(),
        label: String::new(),
        ts_type: ts_type.to_string(),
        input_type: input_type.to_string(),
        is_required: false,
        is_array: false,
        is_entity_ref: false,
        is_immutable: false,
        is_codelist: false,
        is_range: false,
        codelist_values: vec![],
        description: String::new(),
        pg_type: pg_type.to_string(),
        open_end: false,
        ref_api_path: None,
        structured_sub_fields: vec![],
        nested_type_name: None,
    }
}

/// A fixture-shaped [`codegraph_generate::ux::UxPlanInput`] over the
/// file's fixture entity (no workflow, no soft delete, unpinned order).
fn ux_plan_input<'a>(
    entity_title: &'a str,
    fields: &'a [codegraph_generate::ui::page::UiField],
) -> codegraph_generate::ux::UxPlanInput<'a> {
    codegraph_generate::ux::UxPlanInput {
        entity_title,
        fields,
        prop_by_name: std::collections::BTreeMap::new(),
        workflow_status_field: None,
        workflow_terminal_states: &[],
        has_soft_delete: false,
        user_pinned_list_order: false,
    }
}

/// Money-keyword inference emits the exact hint line (and nothing else):
/// the keyword that matched, and the `[[column]]` rule that pins intent.
#[test]
fn ux_diagnostics_money_hint_pins_exact_report_line() {
    use codegraph_generate::ux::{build_ux_plan, collect_diagnostics, report};

    let fields = vec![ux_field(
        "total_amount",
        "NUMERIC(10,2)",
        "string",
        "number",
    )];
    // inline_max = 3 keeps every default action inline, so the overflow
    // accounting stays out of the report — this fixture isolates the hint.
    let rules = codegraph_config::parse_ux_rules_str("[actions]\ninline_max = 3\n")
        .unwrap()
        .rules;
    let input = ux_plan_input("Task", &fields);
    let plan = build_ux_plan(Some(&rules), &input).unwrap().unwrap();

    let lines = report(&collect_diagnostics(&rules, &input, &plan));
    assert_eq!(lines.len(), 1, "exactly the money hint: {lines:?}");
    assert_eq!(
        lines[0],
        "column `total_amount` inferred Money from its name (\"amount\"); \
         pin intent with a [[column]] rule: dimension = \"money\" (or \"quantity\")"
    );
    // The full stderr line (see the section comment for the print sites).
    assert_eq!(
        format!("{UX_STDERR_PREFIX}{}", lines[0]),
        "warning: ux-rules: column `total_amount` inferred Money from its name \
         (\"amount\"); pin intent with a [[column]] rule: dimension = \"money\" \
         (or \"quantity\")"
    );
}

/// A default table over time-ordered data emits the exact opt-in
/// suggestion: entity + candidate field, and the literal rule to add.
#[test]
fn ux_diagnostics_timeline_suggestion_pins_exact_report_line() {
    use codegraph_generate::ux::{build_ux_plan, collect_diagnostics, report};

    let fields = vec![
        ux_field("name", "TEXT", "string", "text"),
        ux_field("created_at", "TIMESTAMPTZ", "string", "datetime-local"),
    ];
    let rules = codegraph_config::parse_ux_rules_str("[actions]\ninline_max = 3\n")
        .unwrap()
        .rules;
    let input = ux_plan_input("Task", &fields);
    let plan = build_ux_plan(Some(&rules), &input).unwrap().unwrap();

    let lines = report(&collect_diagnostics(&rules, &input, &plan));
    assert_eq!(lines.len(), 1, "exactly the suggestion: {lines:?}");
    assert_eq!(
        lines[0],
        "entity \"Task\" renders as a table but has time-ordered data \
         (field \"created_at\"); to opt into timeline rendering add:\n\
         [[collection]]\n\
         entity_pattern = \"Task*\"\n\
         display = \"timeline\"\n\
         order_by = \"created_at\"\n\
         (tables never auto-switch)"
    );
    assert_eq!(
        format!("{UX_STDERR_PREFIX}{}", lines[0]),
        format!("{UX_STDERR_PREFIX}{}", lines[0]),
        "prefix framing is mechanical; the payload above is the pin"
    );
}

/// The action-budget accounting emits the exact overflow line (both
/// default actions beyond the pack's inline budget).
#[test]
fn ux_diagnostics_moved_to_menu_pins_exact_report_line() {
    use codegraph_generate::ux::{build_ux_plan, collect_diagnostics, report};

    let pack = codegraph_config::builtin_ux_rules().unwrap().rules;
    let input = ux_plan_input("Task", &[]);
    let plan = build_ux_plan(Some(&pack), &input).unwrap().unwrap();

    let lines = report(&collect_diagnostics(&pack, &input, &plan));
    assert_eq!(
        lines,
        vec!["2 row action(s) collapsed into the overflow menu ([actions] inline_max)"]
    );
}

/// Integration-level: the default fixture TRIGGERS diagnostics (numeric
/// money-named column + `created_at` over a default table) yet generation
/// SUCCEEDS and the plan still applies — diagnostics are advisory
/// warnings, never failures. (The warning text itself is pinned at the
/// report() boundary above; stderr capture is impractical in-process.)
#[test]
fn ux_rules_diagnostics_are_non_fatal_and_plan_still_applies() {
    let dir = TempDir::new().unwrap();
    let (project, mox) = fixture(&dir);
    let run = FixtureRun {
        config: project.join("domains.toml"),
        output: project.join("generated"),
        profiles: project.join("profiles.toml"),
        mox_files: vec![mox],
    };
    run.run(None);

    let page =
        fs::read_to_string(project.join("generated/ui/src/routes/(app)/common/task/+page.svelte"))
            .expect("task list page must be generated despite active diagnostics");
    // The plan applied: money formatting + chips are in the markup.
    assert!(page.contains("Intl.NumberFormat"), "{page}");
    assert!(page.contains(r#"data-testid="task-chip""#), "{page}");
}

// ── Generated-spec content pins (issue #304, design item 3) ──────────
//
// Umbrella coverage over FULL driver runs: the `.ux.test.ts` /
// `.ux.spec.ts` emitters are pinned per-block at the generator level
// (`ui_e2e_test_tests.rs`, `codegraph-generate` unit tests); here we pin
// that the real pipelines emit the files and the expected assertion
// strings per gated block — spec CONTENT only, no Playwright (that is the
// nightly gate's job). The ux-ON IFML page-content pins live in
// `ifml_template_tests.rs` (#300 chips/alignment/formatting + #301
// timeline/menu); the flag-OFF negatives there guard that file's
// committed byte-identical fixtures and intentionally stay put.

/// The entity pipeline (flag ON) emits `{seg}.ux.test.ts` mirroring the
/// task list page's plan: header contract, chip, Intl formatting,
/// copy-chip, overflow actions with confirm-cancel, sorting and zebra
/// blocks — every gated block the fixture qualifies for.
#[test]
fn ux_rules_entity_pipeline_emits_the_ux_spec_with_gated_blocks() {
    let dir = TempDir::new().unwrap();
    let (project, mox) = fixture(&dir);
    let run = FixtureRun {
        config: project.join("domains.toml"),
        output: project.join("generated"),
        profiles: project.join("profiles.toml"),
        mox_files: vec![mox],
    };
    run.run(None);

    let spec_path = project.join("generated/ui/tests/generated/common/task.ux.test.ts");
    let spec = fs::read_to_string(&spec_path)
        .expect("task.ux.test.ts must be emitted beside the other generated specs");

    // Fixture plumbing: persona fixtures + the real list path; the POM
    // page class drives every list-page interaction (#316).
    assert!(
        spec.contains("// UX list-rendering E2E tests for Task (ux-rules epic, #302)."),
        "{spec}"
    );
    assert!(
        spec.contains("import { test, expect } from '../../e2e/fixtures/personas';"),
        "{spec}"
    );
    assert!(
        spec.contains("import { TaskPage } from './task.page';"),
        "{spec}"
    );
    assert!(spec.contains("new TaskPage(page)"), "{spec}");
    assert!(spec.contains("const BASE_PATH = '/common/task';"), "{spec}");

    // The Intl baseline (locale/currency) moved into the POM: the page
    // class builds UxTable from the plan's [format] and the kernel runs
    // the formatters.
    let page = fs::read_to_string(project.join("generated/ui/tests/generated/common/task.page.ts"))
        .expect("task.page.ts must be emitted beside the specs");
    assert!(page.contains("locale: 'en-NZ'"), "{page}");
    assert!(page.contains("currency: 'NZD'"), "{page}");

    // Table contract: header count mirrors column_order, readable lead.
    assert!(
        spec.contains("test('list renders the ux table contract'"),
        "{spec}"
    );
    assert!(spec.contains("expect(headerCount).toBe(9);"), "{spec}");
    assert!(spec.contains(".toHaveText('Test Name')"), "{spec}");

    // Chips: the codelist's first value is the known fixture label.
    assert!(
        spec.contains("test('status chips render with the expected labels'"),
        "{spec}"
    );
    assert!(spec.contains("ui.chipFor('Draft')"), "{spec}");

    // Alignment + Intl formatting through the kernel's mirror.
    assert!(
        spec.contains("test('numeric columns right-align and format through Intl'"),
        "{spec}"
    );
    assert!(spec.contains("ui.expectRightAligned("), "{spec}");
    assert!(spec.contains("ui.expectFormatted("), "{spec}");

    // Copy chip (clipboard polling lives in the kernel copyCell).
    assert!(
        spec.contains("test('copy-chip copies the identifier and surfaces a tooltip'"),
        "{spec}"
    );
    assert!(spec.contains("ui.copyCell(row, createdId)"), "{spec}");
    assert!(spec.contains("ui.copyTrigger(row).hover()"), "{spec}");
    assert!(spec.contains("ui.tooltipFor(createdId)"), "{spec}");

    // Overflow actions + confirm-CANCEL.
    assert!(
        spec.contains("test('row actions open the overflow menu'"),
        "{spec}"
    );
    assert!(spec.contains("await ui.openActions();"), "{spec}");
    assert!(spec.contains("ui.menuDelete()"), "{spec}");
    assert!(spec.contains("ui.deleteConfirm()"), "{spec}");
    assert!(
        spec.contains("getByRole('button', { name: /cancel/i })"),
        "{spec}"
    );

    // Sorting: aria-sort toggling + the API allow-list.
    assert!(
        spec.contains("test('sortable headers toggle aria-sort and validate ?sort'"),
        "{spec}"
    );
    assert!(spec.contains("ui.sortHeader('name')"), "{spec}");
    assert!(
        spec.contains("toHaveAttribute('aria-sort', 'ascending')"),
        "{spec}"
    );
    assert!(spec.contains("'Unknown sort field'"), "{spec}");

    // Zebra shading.
    assert!(
        spec.contains("test('rows keep zebra shading and hover/focus feedback'"),
        "{spec}"
    );
    assert!(spec.contains("await ui.expectZebra();"), "{spec}");
}

/// The IFML pipeline (ux rules active) emits
/// `tests/ifml/{view-kebab}.ux.spec.ts` for the view's fallback list:
/// chip tone, numeric/date Intl formatting, and the overflow menu —
/// computed from the SAME resolution the route generator renders from.
#[tokio::test]
async fn ux_rules_ifml_pipeline_emits_the_view_ux_spec_with_gated_blocks() {
    let dir = tempfile::tempdir().unwrap();
    // Schema-backed entity under `schemas/{domain}/json/` (the layout the
    // schema loader derives domains from).
    let schemas_dir = dir.path().join("schemas").join("sales").join("json");
    std::fs::create_dir_all(&schemas_dir).unwrap();
    std::fs::write(
        schemas_dir.join("CustomerType.json"),
        r#"{
  "$id": "CustomerType.json",
  "title": "CustomerType",
  "description": "A customer",
  "type": "object",
  "properties": {
    "id": { "type": "string", "format": "uuid", "description": "Unique identifier" },
    "name": { "type": "string", "description": "Customer name" },
    "status": { "type": "string", "enum": ["draft", "active"], "description": "Status" },
    "total_amount": { "type": "number", "description": "Total billed" },
    "quantity": { "type": "integer", "description": "Units ordered" },
    "created_at": { "type": "string", "format": "date-time", "description": "Created" }
  }
}"#,
    )
    .unwrap();
    let classifier_path = dir.path().join("classifier.toml");
    std::fs::write(&classifier_path, "# minimal classifier config\n").unwrap();
    let ifml_path = dir.path().join("app.ifml");
    std::fs::write(
        &ifml_path,
        r#"
domain "sales" {
    schema "sales";
}

view "CustomerList" {
    label "Customers";

    component "grid" {
        type: list;
        data: Customer;
        fields: [name, status, total_amount, quantity, created_at, id];

        on click(row) -> navigate("CustomerDetail", {
            customerId: row.id
        });
        on delete(row) -> navigate("CustomerTrash");
    }
}

view "CustomerDetail" {
    params { customerId: Uuid };

    component "info" {
        type: details;
        data: Customer;
        fields: [name];
    }
}

view "CustomerTrash" {
    component "trash" {
        type: list;
        data: Customer;
        fields: [name];
    }
}
"#,
    )
    .unwrap();
    let domains = dir.path().join("domains.toml");
    std::fs::write(
        &domains,
        r#"
[defaults]
api_version = "v1"

[domains.sales]
label = "Sales"
schema_dir = "sales"
postgres_schema = "sales"
entities = ["CustomerType"]
"#,
    )
    .unwrap();
    let ux_rules_path = dir.path().join("ux-rules.toml");
    std::fs::write(
        &ux_rules_path,
        "[format]\nlocale = \"en-NZ\"\ncurrency = \"NZD\"\n",
    )
    .unwrap();
    let schemas = dir.path().join("schemas");
    let output = dir.path().join("out");
    let ifml_files = vec![ifml_path.clone()];

    codegraph::driver::ifml_generate(codegraph::driver::IfmlGenerateArgs {
        config_path: &domains,
        output: &output,
        ifml_files: &ifml_files,
        schemas: Some(&schemas),
        classifier: Some(&classifier_path),
        frameworks: &["svelte".to_string()],
        profiles_config_path: None,
        template_dir: &[],
        ifml_components: None,
        ifml_design_system: None,
        ux_rules: Some(&ux_rules_path),
    })
    .await
    .unwrap();

    let spec_path = output.join("svelte/tests/ifml/customer-list.ux.spec.ts");
    let spec = fs::read_to_string(&spec_path)
        .expect("customer-list.ux.spec.ts must be emitted beside the view specs");

    // Header + in-spec Intl baseline (same locale/currency the page runs).
    assert!(
        spec.contains(
            "// IFML Playwright E2E ux-rules tests for view customerlist component grid (#303)."
        ),
        "{spec}"
    );
    assert!(spec.contains("test.describe('grid ux'"), "{spec}");
    assert!(
        spec.contains("const UX_BASE = '/api/v1/sales/customer';"),
        "{spec}"
    );
    assert!(spec.contains("const UX_LOCALE = 'en-NZ';"), "{spec}");
    assert!(
        spec.contains("const UX_MONEY_OPTS = { style: 'currency', currency: 'NZD' };"),
        "{spec}"
    );

    // Chip tone block: seeded value + ToneMap-resolved variant, driven
    // through the page class's UxTable component object (#317).
    assert!(
        spec.contains("test('ux chips render with tone variants'"),
        "{spec}"
    );
    assert!(spec.contains("ui.grid.chipFor('Test status')"), "{spec}");
    assert!(
        spec.contains("toHaveAttribute('data-chip-variant', 'outline')"),
        "{spec}"
    );
    let page = fs::read_to_string(output.join("svelte/tests/pages/customer-list-page.ts"))
        .expect("the view page class is emitted");
    assert!(page.contains("module: 'grid'"), "{page}");
    assert!(page.contains("locale: 'en-NZ'"), "{page}");
    assert!(page.contains("currency: 'NZD'"), "{page}");

    // Numeric/date formatting block: right-aligned cells through the SAME
    // Intl formatters the page runs.
    assert!(
        spec.contains("test('ux numeric columns align and format through Intl'"),
        "{spec}"
    );
    assert!(spec.contains("toHaveClass(/text-right/)"), "{spec}");
    assert!(spec.contains("toHaveClass(/tabular-nums/)"), "{spec}");
    assert!(
        spec.contains("new Intl.NumberFormat(UX_LOCALE, UX_MONEY_OPTS)"),
        "{spec}"
    );
    assert!(
        spec.contains("new Intl.DateTimeFormat(UX_LOCALE, { dateStyle: 'medium' })"),
        "{spec}"
    );

    // Overflow menu block: trigger → menu → first item navigates to the
    // secondary event's target — openActions is the kernel contract, so
    // the trigger/menu testids stay in the kernel/page class.
    assert!(
        spec.contains("test('ux row actions open the overflow menu'"),
        "{spec}"
    );
    assert!(spec.contains("await ui.grid.openActions();"), "{spec}");
    assert!(
        spec.contains("await menu.getByRole('button').first().click();"),
        "{spec}"
    );
    assert!(
        spec.contains("waitForURL(new RegExp('/customertrash$'))"),
        "{spec}"
    );
    assert!(
        !spec.contains("getByTestId("),
        "zero raw testid construction in spec bodies: {spec}"
    );
}

/// Flag OFF: NEITHER pipeline emits a ux spec file — the umbrella
/// negative complementing the flag-ON pins above and the byte-identity
/// canaries (`ux_rules_byte_identity_tests.rs`).
#[tokio::test]
async fn ux_rules_flag_off_emits_no_ux_specs() {
    // Entity pipeline: flag off via profiles.toml. (Direct driver await —
    // `FixtureRun::run` builds its own runtime, which would nest here.)
    let dir = TempDir::new().unwrap();
    let (project, mox) = fixture(&dir);
    let profiles_path = project.join("profiles.toml");
    let profiles = fs::read_to_string(&profiles_path).unwrap();
    let flag_off = profiles.replace("ux_rules = true", "ux_rules = false");
    assert_ne!(profiles, flag_off, "scaffold must ship the ux_rules flag");
    fs::write(&profiles_path, flag_off).unwrap();
    let mox_files = vec![mox];
    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &project.join("domains.toml"),
        output: &project.join("generated"),
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(profiles_path),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &mox_files,
        rosetta_files: &[],
        ddd_files: &[],
        evt_files: &[],
        deploy_files: &[],
        ifml_framework: &[],
        ifml_components: None,
        ifml_design_system: None,
        ux_rules: None,
        codegraph_rev: None,
        check: false,
    })
    .await
    .unwrap();
    assert!(
        !project
            .join("generated/ui/tests/generated/common/task.ux.test.ts")
            .exists(),
        "flag-off entity pipeline must not emit a ux spec"
    );

    // IFML pipeline: flag off = no ux rules file.
    let dir = tempfile::tempdir().unwrap();
    let ifml_path = dir.path().join("app.ifml");
    std::fs::write(
        &ifml_path,
        r#"
domain "sales" {
    schema "sales";
}

view "CustomerList" {
    component "grid" {
        type: list;
        data: Customer;
        fields: [name];
    }
}
"#,
    )
    .unwrap();
    let domains = dir.path().join("domains.toml");
    std::fs::write(
        &domains,
        r#"
[defaults]
api_version = "v1"

[domains.sales]
label = "Sales"
schema_dir = "sales"
postgres_schema = "sales"
entities = ["CustomerType"]
"#,
    )
    .unwrap();
    let output = dir.path().join("out");
    codegraph::driver::ifml_generate(codegraph::driver::IfmlGenerateArgs {
        config_path: &domains,
        output: &output,
        ifml_files: &[ifml_path],
        schemas: None,
        classifier: None,
        frameworks: &["svelte".to_string()],
        profiles_config_path: None,
        template_dir: &[],
        ifml_components: None,
        ifml_design_system: None,
        ux_rules: None,
    })
    .await
    .unwrap();
    let specs: Vec<PathBuf> = walkdir::WalkDir::new(output.join("svelte/tests"))
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".ux.spec.ts") || n.ends_with(".ux.test.ts"))
        })
        .collect();
    assert!(
        specs.is_empty(),
        "flag-off IFML pipeline must not emit ux specs: {specs:?}"
    );
}
