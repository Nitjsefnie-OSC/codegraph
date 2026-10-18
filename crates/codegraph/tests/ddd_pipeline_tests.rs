//! End-to-end `.ddd` design-driven generation (issue #449 Phase 4): the
//! golden library fixture driven through `driver::run` (mox + ddd) with
//! assertions on the generated tree, run-to-run determinism, and the
//! config-vs-design equivalence gate (a purpose-built minimal model whose
//! config-authored and design-authored runs must produce byte-identical
//! entity files).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ddd")
        .join(name)
}

const LIBRARY_DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.library]
label = "Library"
schema_dir = "library"
postgres_schema = "library"
"#;

/// The config-driven arm of the equivalence gate: the same operations the
/// design's repository built-ins map (save→create+update, findById→read,
/// delete→delete, findAll→list), filtering disabled (the design search
/// declares no filters), and the same FTS surface the design's search
/// declares ([title, synopsis], english).
const EQUIVALENCE_CONFIG_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.equivalence]
label = "Equivalence"
schema_dir = "equivalence"
postgres_schema = "equivalence"

[domains.equivalence.entity_config.WidgetType]
operations = ["create", "update", "read", "delete", "list"]
filter_fields = []

[domains.equivalence.entity_config.WidgetType.search]
fts_columns = ["title", "synopsis"]
fts_language = "english"
"#;

/// The design-driven arm: identical domain entry, NO entity config — the
/// `.ddd` design is the only source for operations, FTS, and filters.
const EQUIVALENCE_DESIGN_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.equivalence]
label = "Equivalence"
schema_dir = "equivalence"
postgres_schema = "equivalence"
"#;

/// One pipeline run: `driver::run` in the no-plan (all generators)
/// backward-compat mode, mox-first, with optional `.ddd` designs.
async fn run_pipeline(config: &Path, output: &Path, mox: &Path, ddd_files: &[PathBuf]) {
    let mox_files = vec![mox.to_path_buf()];
    codegraph::driver::run(codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: config,
        output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        // Nonexistent path ⇒ the no-plan (all generators) backward-compat
        // path, the same mode the mox equivalence/pipeline tests use.
        profiles_config_path: Some(PathBuf::from("definitely-absent-profiles.toml")),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &mox_files,
        rosetta_files: &[],
        ddd_files,
        evt_files: &[],
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
                    .filter(|k| k.contains("library") || k.contains("domain"))
                    .take(80)
                    .collect::<Vec<_>>()
            )
        })
        .clone()
}

// ── library fixture: the design drives the generated surface ────────────

#[tokio::test]
async fn library_design_drives_the_generated_repository_dto_and_search_surface() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("domains.toml"), LIBRARY_DOMAINS_TOML).unwrap();
    let output = root.path().join("generated");
    let ddd = vec![fixture("library.ddd")];
    run_pipeline(
        &root.path().join("domains.toml"),
        &output,
        &fixture("library.mox"),
        &ddd,
    )
    .await;
    let files = collect_files(&output);

    // Movie (designed: findById/findAll/save/delete + findByTitle): full
    // CRUD surface from the builtins AND the declared finder.
    let movie_trait = read_out(&files, "src/domain/library/movie/repository.rs");
    assert!(
        movie_trait.contains("/// Design finder `findByTitle`"),
        "the declared finder's marker: {movie_trait}"
    );
    assert!(movie_trait.contains("async fn find_by_title("));
    assert!(movie_trait.contains("title: String,"));
    assert!(movie_trait.contains("async fn create("));
    assert!(movie_trait.contains("async fn update("));
    assert!(movie_trait.contains("async fn delete("));
    assert!(movie_trait.contains("async fn find_by_id("));
    assert!(movie_trait.contains("async fn list("));
    // Sculptor defaults (rexlang #47): an entity design is auditable unless
    // `!auditable` opts out — Movie's default-on flag drives the auditable
    // repository shape (the include_deleted parameter on find/list).
    assert!(
        movie_trait.contains("include_deleted"),
        "auditable defaults ON for entity designs: {movie_trait}"
    );

    let movie_impl = read_out(&files, "src/domain/library/movie/repository_impl.rs");
    assert!(movie_impl.contains("async fn find_by_title("));
    assert!(
        movie_impl.contains("Column::Title.eq(title.clone())"),
        "equality predicate on the title column: {movie_impl}"
    );
    assert!(movie_impl.contains(".one(db)"), "single finder → .one(db)");
    assert!(
        movie_impl.contains("async fn search_ids("),
        "the MediaSearch design turns the FTS surface on: {movie_impl}"
    );
    assert!(
        movie_impl.contains("websearch_to_tsquery('english'"),
        "the search's default analyzer becomes the FTS language: {movie_impl}"
    );
    let movie_query = read_out(&files, "src/domain/library/movie/query.rs");
    assert!(movie_query.contains("search_ids"));

    // Book (designed WITHOUT a repository): no CRUD ops mapped from a
    // design and no design finders — its schema-derived generation stays.
    let book_trait = read_out(&files, "src/domain/library/book/repository.rs");
    assert!(
        !book_trait.contains("Design finder"),
        "Book has no repository → no design finders: {book_trait}"
    );
    assert!(!book_trait.contains("find_by_title"));
    assert!(
        !book_trait.contains("async fn create("),
        "a design without a repository maps to an EMPTY operation set: {book_trait}"
    );

    // DTO gating: designs with `save` → dto_create/dto_update; designs
    // without (Book, Chapter, Media) → response only.
    for entity in ["library", "movie", "person", "physical_media"] {
        assert!(
            files.contains_key(&format!("src/domain/library/{entity}/dto_create.rs")),
            "{entity} design carries save → dto_create.rs"
        );
        assert!(
            files.contains_key(&format!("src/domain/library/{entity}/dto_update.rs")),
            "{entity} design carries save → dto_update.rs"
        );
    }
    for entity in ["book", "chapter", "media"] {
        assert!(
            !files.contains_key(&format!("src/domain/library/{entity}/dto_create.rs")),
            "{entity} has no save → no dto_create.rs"
        );
        assert!(
            files.contains_key(&format!("src/domain/library/{entity}/dto_response.rs")),
            "{entity} dto_response.rs always emits"
        );
    }

    // Person's MemberDirectory search covers fullName → its FTS surface is
    // on too, and its declared findByName finder lowers into the trait.
    let person_impl = read_out(&files, "src/domain/library/person/repository_impl.rs");
    assert!(person_impl.contains("async fn search_ids("));
    let person_trait = read_out(&files, "src/domain/library/person/repository.rs");
    assert!(person_trait.contains("async fn find_by_name("));
}

// ── determinism ─────────────────────────────────────────────────────────

#[tokio::test]
async fn library_generation_is_deterministic_across_runs() {
    let root_a = tempfile::tempdir().unwrap();
    let root_b = tempfile::tempdir().unwrap();
    for root in [root_a.path(), root_b.path()] {
        fs::write(root.join("domains.toml"), LIBRARY_DOMAINS_TOML).unwrap();
    }
    let out_a = root_a.path().join("generated");
    let out_b = root_b.path().join("generated");
    let ddd = vec![fixture("library.ddd")];
    run_pipeline(
        &root_a.path().join("domains.toml"),
        &out_a,
        &fixture("library.mox"),
        &ddd,
    )
    .await;
    run_pipeline(
        &root_b.path().join("domains.toml"),
        &out_b,
        &fixture("library.mox"),
        &ddd,
    )
    .await;

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

// ── config-vs-design equivalence ────────────────────────────────────────

/// The SAME entity (WidgetType) generated (a) schema/config-driven — mox +
/// `entity_config` carrying the design's operations and search — and (b)
/// design-driven via `equivalence.ddd` (full builtins + a search over the
/// text fields, NO declared finder). The entity's repository trait,
/// repository impl, all three DTO files, and query file must be
/// byte-identical: the design plane expresses nothing the config plane
/// cannot for this model.
#[tokio::test]
async fn design_driven_generation_is_byte_identical_to_config_driven_generation() {
    // (a) config-driven: mox + entity_config.
    let root_a = tempfile::tempdir().unwrap();
    fs::write(root_a.path().join("domains.toml"), EQUIVALENCE_CONFIG_TOML).unwrap();
    let out_a = root_a.path().join("generated");
    run_pipeline(
        &root_a.path().join("domains.toml"),
        &out_a,
        &fixture("equivalence.mox"),
        &[],
    )
    .await;

    // (b) design-driven: the same mox + equivalence.ddd, no entity config.
    let root_b = tempfile::tempdir().unwrap();
    fs::write(root_b.path().join("domains.toml"), EQUIVALENCE_DESIGN_TOML).unwrap();
    let out_b = root_b.path().join("generated");
    let ddd = vec![fixture("equivalence.ddd")];
    run_pipeline(
        &root_b.path().join("domains.toml"),
        &out_b,
        &fixture("equivalence.mox"),
        &ddd,
    )
    .await;

    let files_a = collect_files(&out_a);
    let files_b = collect_files(&out_b);

    let must_match = [
        "src/domain/equivalence/widget/repository.rs",
        "src/domain/equivalence/widget/repository_impl.rs",
        "src/domain/equivalence/widget/dto_create.rs",
        "src/domain/equivalence/widget/dto_update.rs",
        "src/domain/equivalence/widget/dto_response.rs",
        "src/domain/equivalence/widget/query.rs",
    ];
    for rel in must_match {
        let a = read_out(&files_a, rel);
        let b = read_out(&files_b, rel);
        assert_eq!(
            a, b,
            "{rel} must be byte-identical between the config-driven and design-driven runs"
        );
    }
}
