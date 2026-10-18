//! Deterministic graph artifact + persisted-graph cache (issue #275).
//!
//! - **L1 graph cache**: a plain persistent Grafeo database
//!   (`Config::persistent` → `GrafeoDB::with_config`; reopens read-only)
//!   plus an inputs hash (all model/config input file contents + the graph
//!   format version) that decides whether a run may reuse the persisted
//!   graph instead of re-ingesting everything. This layer stands alone —
//!   it touches no document/artifact machinery.
//! - **L2 artifact** (the `doc` layer, implemented in `codegraph-grafeo`):
//!   `export_ir`/`import_ir` produce a canonical, hashable JSON document
//!   of the full graph state whose round trip is byte-identical. A
//!   separate deliverable for diffing/publishing — never used by the
//!   persistence path.
//! - **L3 conformance kit**: executable cases pinning the document format
//!   (the committed case file lives at
//!   `tests/fixtures/artifact_kit/graph_document_v1.md`).

pub mod kit;

use std::path::Path;

pub use codegraph_grafeo::artifact::{
    ArtifactError, EdgeRecord, ExportedArtifact, GraphDocument, NodeRecord, PropValue,
    canonical_bytes, export_ir, import_ir, normalize, parse_document, sha256_hex,
};
pub use codegraph_grafeo::schema_ddl::GRAPH_FORMAT_VERSION as FORMAT_VERSION;

/// Layout of the persisted-graph cache directory.
pub const CACHE_GRAPH_FILE: &str = "graph.grafeo";
pub const CACHE_INPUTS_MARKER: &str = "inputs.sha256";

/// An input file contributing to the graph: its path (as given on the
/// command line or discovered under a schema dir) and its full contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// SHA-256 over the graph format version followed by every input file's
/// path and contents, in sorted path order. Identical inputs (and an
/// unchanged graph format) therefore always produce the same hash.
pub fn inputs_hash(files: &[InputFile]) -> String {
    use sha2::Digest;

    let mut sorted: Vec<&InputFile> = files.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));

    let mut hasher = sha2::Sha256::new();
    hasher.update(FORMAT_VERSION.to_le_bytes());
    for file in sorted {
        hasher.update(file.path.as_bytes());
        hasher.update([0u8]);
        hasher.update((file.bytes.len() as u64).to_le_bytes());
        hasher.update(&file.bytes);
    }
    hex_digest(hasher.finalize())
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    let mut out = String::new();
    for byte in digest.as_ref() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Read one input file.
pub fn read_input_file(path: &Path) -> Result<InputFile, ArtifactError> {
    Ok(InputFile {
        path: path.to_string_lossy().to_string(),
        bytes: std::fs::read(path)?,
    })
}

/// Recursively collect the JSON schema files under `dir`, keyed by their
/// path relative to `dir` so the hash does not depend on the absolute
/// location of the schemas tree.
pub fn collect_schema_dir(dir: &Path) -> Result<Vec<InputFile>, ArtifactError> {
    let mut files = Vec::new();
    collect_schema_dir_into(dir, dir, &mut files)?;
    Ok(files)
}

fn collect_schema_dir_into(
    root: &Path,
    dir: &Path,
    files: &mut Vec<InputFile>,
) -> Result<(), ArtifactError> {
    let entries = std::fs::read_dir(dir)?;
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_schema_dir_into(root, &path, files)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            files.push(InputFile {
                path: format!("<schemas>/{rel}"),
                bytes: std::fs::read(&path)?,
            });
        }
    }
    Ok(())
}

/// Every model/config input that shapes the graph: domain config,
/// classifier config, `ui-overrides.toml` (ingested onto property nodes),
/// JSON schemas, mox/rosetta/ddd/evt/deploy/IFML/OpenAPI sources.
/// Transitive files pulled in by model sources (mox `import schema`
/// targets, IFML `.actor` imports, `.ddd`/`.evt` imported `.mox` domains,
/// `.deploy` imported designs/flows) are NOT tracked yet — editing one of
/// those invalidates the cache only when its importing file changes too.
#[allow(clippy::too_many_arguments)]
pub fn collect_run_inputs(
    schemas: Option<&Path>,
    classifier: Option<&Path>,
    config_path: &Path,
    mox_files: &[std::path::PathBuf],
    rosetta_files: &[std::path::PathBuf],
    ddd_files: &[std::path::PathBuf],
    evt_files: &[std::path::PathBuf],
    deploy_files: &[std::path::PathBuf],
    ifml_files: &[std::path::PathBuf],
    openapi_files: &[std::path::PathBuf],
) -> Result<Vec<InputFile>, ArtifactError> {
    let mut files = Vec::new();
    files.push(read_input_file(config_path)?);
    if let Some(classifier) = classifier {
        files.push(read_input_file(classifier)?);
    }
    if let Some(parent) = config_path.parent() {
        let ui_overrides = parent.join("ui-overrides.toml");
        if ui_overrides.exists() {
            files.push(read_input_file(&ui_overrides)?);
        }
    }
    if let Some(schemas_dir) = schemas {
        files.extend(collect_schema_dir(schemas_dir)?);
    }
    for group in [
        mox_files,
        rosetta_files,
        ddd_files,
        evt_files,
        deploy_files,
        ifml_files,
        openapi_files,
    ] {
        for path in group {
            files.push(read_input_file(path)?);
        }
    }
    Ok(files)
}

/// Open the persisted graph when the stored inputs hash matches. The
/// database is opened READ-ONLY (checkpoint snapshot, shared file lock,
/// mutations rejected): the reuse path must never mutate the cached graph.
/// The inputs marker lives outside the DB, so guarding the DB file does
/// not affect the hash bookkeeping. Returns `Ok(None)` when the cache is
/// absent, unreadable, or was written for different inputs (or an older
/// graph format).
pub fn open_reusable_engine(
    cache_dir: &Path,
    inputs_hash: &str,
) -> Result<Option<codegraph_grafeo::GrafeoEngine>, ArtifactError> {
    let marker = cache_dir.join(CACHE_INPUTS_MARKER);
    let graph_file = cache_dir.join(CACHE_GRAPH_FILE);
    if !marker.exists() || !graph_file.exists() {
        return Ok(None);
    }
    let stored = std::fs::read_to_string(&marker)?;
    if stored.trim() != inputs_hash {
        return Ok(None);
    }
    codegraph_grafeo::GrafeoEngine::open_read_only(&graph_file)
        .map(Some)
        .map_err(ArtifactError::from)
}

/// Record the inputs hash after a fresh ingestion so the next run can
/// reuse the persisted graph.
pub fn persist_cache_marker(cache_dir: &Path, inputs_hash: &str) -> Result<(), ArtifactError> {
    std::fs::create_dir_all(cache_dir)?;
    std::fs::write(
        cache_dir.join(CACHE_INPUTS_MARKER),
        format!("{inputs_hash}\n"),
    )?;
    Ok(())
}
