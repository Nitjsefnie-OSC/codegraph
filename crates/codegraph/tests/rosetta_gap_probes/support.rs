//! Shared harness for the Rosetta gap-analysis probe tests (issue #254).

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const CLASSIFIER_TOML: &str = "inline_enum_threshold = 20\n";

/// A minimal single-domain JSON-schema project on disk.
pub struct SchemaProject {
    pub root: PathBuf,
    pub config: PathBuf,
    pub schemas: PathBuf,
    pub classifier: PathBuf,
    pub output: PathBuf,
}

/// `domains.toml` for one domain. Explicit `entities` mirror the JSON
/// path's author-declarative entity decision (see the mox equivalence
/// harness header).
pub fn domains_toml(domain: &str, entities: &[&str]) -> String {
    let mut toml = String::from(
        "[defaults]\noperations = [\"create\", \"read\", \"update\", \"delete\", \"list\"]\n\n",
    );
    toml.push_str(&domain_toml_entry(domain, entities));
    toml
}

/// Just the `[domains.<name>]` entry (no `[defaults]` header) — for
/// appending further domains to an existing `domains.toml`.
pub fn domain_toml_entry(domain: &str, entities: &[&str]) -> String {
    let mut toml = format!("[domains.{domain}]\n");
    toml.push_str(&format!("label = \"{domain}\"\n"));
    toml.push_str(&format!("schema_dir = \"{domain}\"\n"));
    toml.push_str(&format!("postgres_schema = \"{domain}\"\n"));
    if !entities.is_empty() {
        let list = entities
            .iter()
            .map(|e| format!("\"{e}\""))
            .collect::<Vec<_>>()
            .join(", ");
        toml.push_str(&format!("entities = [{list}]\n"));
    }
    toml
}

/// Write domains.toml + classifier.toml + schema files under
/// `<root>/schemas/<domain>/json/`. `files` entries are
/// `(rel path under <domain>/json/, content)`.
pub fn write_schema_project(
    root: &Path,
    domain: &str,
    entities: &[&str],
    files: &[(&str, &str)],
) -> SchemaProject {
    let schemas = root.join("schemas");
    let json_dir = schemas.join(domain).join("json");
    fs::create_dir_all(&json_dir).unwrap();
    fs::write(schemas.join("domains.toml"), domains_toml(domain, entities)).unwrap();
    fs::write(schemas.join("classifier.toml"), CLASSIFIER_TOML).unwrap();
    for (rel, content) in files {
        let path = json_dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }
    SchemaProject {
        root: root.to_path_buf(),
        config: schemas.join("domains.toml"),
        schemas: schemas.clone(),
        classifier: schemas.join("classifier.toml"),
        output: root.join("generated"),
    }
}

/// Same as [`write_schema_project`], but writes schema files into an
/// explicitly given second domain directory (for cross-domain probes).
pub fn write_extra_domain(root: &Path, domain: &str, files: &[(&str, &str)]) {
    let json_dir = root.join("schemas").join(domain).join("json");
    fs::create_dir_all(&json_dir).unwrap();
    for (rel, content) in files {
        let path = json_dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }
}

/// Append a second `[domains.<name>]` entry to the project's domains.toml.
pub fn add_domain_entry(p: &SchemaProject, domain: &str, entities: &[&str]) {
    let mut toml = fs::read_to_string(&p.config).unwrap();
    toml.push('\n');
    toml.push_str(&domain_toml_entry(domain, entities));
    fs::write(&p.config, toml).unwrap();
}

/// `driver::run` args for a schema project, matching the shape used by the
/// mox equivalence harness (no profiles plan, no post-gen).
pub fn driver_args(p: &SchemaProject) -> codegraph::driver::RunArgs<'_> {
    codegraph::driver::RunArgs {
        schemas: Some(&p.schemas),
        classifier: Some(&p.classifier),
        config_path: &p.config,
        output: &p.output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        profiles_config_path: Some(PathBuf::from("definitely-absent-profiles.toml")),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: &[],
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

/// Run the full pipeline (ingest + classify + generate) over a schema
/// project and return `rel path → content` for the generated tree.
pub async fn run_pipeline(
    root: &Path,
    domain: &str,
    entities: &[&str],
    files: &[(&str, &str)],
) -> BTreeMap<String, String> {
    let p = write_schema_project(root, domain, entities, files);
    codegraph::driver::run(driver_args(&p)).await.unwrap();
    collect_files(&p.output)
}

/// Ingest only (no generators): ingest the schema project into an
/// in-memory graph backend and return the backend for graph queries.
pub async fn ingest_into_graph(
    root: &Path,
    domain: &str,
    entities: &[&str],
    files: &[(&str, &str)],
) -> codegraph_backend::Backend {
    let p = write_schema_project(root, domain, entities, files);
    let be = codegraph_backend::create_backend(&codegraph_backend::BackendConfig::default())
        .await
        .unwrap();
    let domain_config = codegraph_config::config::parse_domain_config(&p.config).unwrap();
    let classifier = codegraph_classifier::config::parse_classifier_config(&p.classifier).unwrap();
    codegraph::ingest::async_ingest::ingest_schemas(
        be.ingestor(),
        &p.schemas,
        &classifier,
        &HashSet::from_iter(entities.iter().map(|s| s.to_string())),
        &codegraph_config::config::UiOverrideConfig::default(),
        &domain_config.defaults.type_suffix,
    )
    .await
    .unwrap();
    be
}

/// Recursively collect `dir` into a `relative path → content` map.
pub fn collect_files(root: &Path) -> BTreeMap<String, String> {
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

/// Find the unique generated file whose path ends with `suffix` (ignoring
/// `_rls`/`_trigger`/`_fts` twins, same rule as the equivalence harness).
pub fn file_by_suffix<'a>(files: &'a BTreeMap<String, String>, suffix: &str) -> Option<&'a String> {
    files
        .iter()
        .filter(|(k, _)| {
            k.ends_with(suffix)
                && !k.ends_with(&format!("_rls{suffix}"))
                && !k.ends_with(&format!("_trigger{suffix}"))
                && !k.ends_with(&format!("_fts{suffix}"))
        })
        .map(|(_, v)| v)
        .next()
}
