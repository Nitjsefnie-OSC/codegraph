//! rexlang `.evt` event-contract ingest (issue #454 Phase 2a): compile each
//! contract file against its imported `.mox` domains with the rex driver,
//! convert the resulting `rex_ir::events::EventModel` into the graph-native
//! [`EvtModelGraph`], resolve payload field types against the ingested
//! schema titles, and land the model through
//! [`GraphIngestor::ingest_evt_model`].
//!
//! Compilation failures are HARD errors ([`Error::EvtModel`], the
//! [`Error::DddModel`] precedent) — a broken contract never half-ingests.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use codegraph_config::config::DomainConfig;
use codegraph_core::traits::{GraphIngestor, GraphQuerier};
use codegraph_core::types::{
    EvtChannelNode, EvtEventField, EvtEventNode, EvtModelGraph, EvtSubscriptionNode,
};
use rex_driver::{DomainImports, SchemaImports, SigilImports, compile_evt_str};

use crate::error::{Error, Result};
use crate::ingest::ddd_ingest::{resolve_with_suffix, short_name};
use crate::ingest::rex_imports::{collect_domains, discover_rosetta_files, scan_sigil_imports};

/// Counters for one [`ingest_evt_files`] run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EvtIngestStats {
    /// The number of `.evt` files handed to the run.
    pub files: usize,
    /// Events ingested across all contracts.
    pub events: usize,
    /// Channels ingested across all contracts.
    pub channels: usize,
    /// Subscriptions ingested across all contracts.
    pub subscriptions: usize,
    /// Payload field types that matched no ingested schema title (exact or
    /// +`type_suffix`) and were not primitives; the field keeps `None` and
    /// the miss warns on stderr.
    pub unresolved_types: usize,
}

impl std::fmt::Display for EvtIngestStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} files, {} events, {} channels, {} subscriptions, {} unresolved types",
            self.files, self.events, self.channels, self.subscriptions, self.unresolved_types
        )
    }
}

/// Ingest rexlang `.evt` event-contract files into the graph's events plane.
///
/// Per file: read the source (a missing file is a hard error), collect the
/// transitively imported `.mox` domains and their `import schema`/`import
/// sigil` content, compile with `rex_driver::compile_evt_str`, hard-error
/// on any error-severity diagnostic (rendered per file into the reason),
/// resolve payload field types against the graph's schema titles
/// (exact → title + `type_suffix` → warn + count; primitives resolve to
/// `None` silently), and ingest one [`EvtModelGraph`] per file.
#[cfg_attr(feature = "hotpath", hotpath::measure)]
pub async fn ingest_evt_files(
    ingestor: &dyn GraphIngestor,
    querier: &dyn GraphQuerier,
    evt_paths: &[PathBuf],
    domain_config: &DomainConfig,
) -> Result<EvtIngestStats> {
    let mut stats = EvtIngestStats {
        files: evt_paths.len(),
        ..Default::default()
    };
    let type_suffix = &domain_config.defaults.type_suffix;

    // Schema titles available for field-type resolution (the wire_alias_refs
    // chain: exact title first, then title + the configured type suffix).
    let schemas = querier.list_schemas(None).await.map_err(Error::Graph)?;
    let titles: HashSet<String> = schemas.iter().map(|s| s.title.clone()).collect();

    for path in evt_paths {
        let compiled = read_and_compile_evt(path)?;
        let has_errors = compiled
            .compilation
            .diagnostics
            .iter()
            .any(|(_, d)| d.is_error());
        if has_errors || compiled.compilation.model.is_none() {
            let reason = render_evt_diagnostics(&compiled);
            return Err(Error::EvtModel {
                file: compiled.path.clone(),
                reason: if reason.is_empty() {
                    "compilation produced no event artifact".to_string()
                } else {
                    reason
                },
            });
        }
        let Some(model) = compiled.compilation.model.as_ref() else {
            return Err(Error::EvtModel {
                file: compiled.path.clone(),
                reason: "compilation produced no event artifact".to_string(),
            });
        };

        // Field-type resolution warns and counts on miss; the field keeps
        // `resolved_title: None` for the generate plane to degrade on.
        // Primitives are normal (they resolve to None silently — no
        // warning, no count).
        let file_path = compiled.path.clone();
        let unresolved = std::cell::Cell::new(0usize);
        let graph = evt_model_graph_from_rex(model, &file_path, &|ty: &rex_ir::TypeRef| {
            match resolve_type_title(ty, type_suffix, &titles) {
                Some(title) => Some(title),
                None if matches!(ty, rex_ir::TypeRef::Primitive(_)) => None,
                None => {
                    let name = ty.qualified_name().unwrap_or_else(|| "?".to_string());
                    let short = short_name(&name);
                    let suffixed = format!("{short}{type_suffix}");
                    eprintln!(
                        "Warning: evt contract '{file_path}' field type '{name}' matches no \
                         ingested schema title — tried '{short}' and '{suffixed}'; the field \
                         keeps no resolved title"
                    );
                    unresolved.set(unresolved.get() + 1);
                    None
                }
            }
        });
        stats.unresolved_types += unresolved.get();
        stats.events += graph.events.len();
        stats.channels += graph.channels.len();
        stats.subscriptions += graph.subscriptions.len();
        ingestor
            .ingest_evt_model(&graph)
            .await
            .map_err(Error::Graph)?;
    }
    Ok(stats)
}

/// One `.evt` file read, its imports collected, and compiled — the shared
/// helper both [`ingest_evt_files`] and the doctor `--evt-files` check
/// call so their import resolution cannot drift.
pub struct EvtCompiled {
    /// The contract file's driver path (as given on the command line).
    pub path: String,
    /// The contract file's source text.
    pub source: String,
    /// The imported `.mox` domain sources as `(driver path, source text)`.
    pub mox_sources: Vec<(String, String)>,
    /// Every provided rosetta source as `(tag path, source text)` — the
    /// named `import sigil` files (tagged with the import path as written)
    /// plus the discovered sibling candidates (tagged with their resolved
    /// paths).
    pub sigil_sources: Vec<(String, String)>,
    /// The compile result.
    pub compilation: rex_driver::EventCompilation,
}

impl EvtCompiled {
    /// The source text of the file with the given driver path, if it is
    /// the contract file, an imported domain, or a rosetta source (the
    /// rex-cli `source_of` shape).
    fn source_of(&self, path: &str) -> Option<&str> {
        if path == self.path {
            return Some(&self.source);
        }
        self.mox_sources
            .iter()
            .find(|(name, _)| name == path)
            .map(|(_, source)| source.as_str())
            .or_else(|| {
                self.sigil_sources
                    .iter()
                    .find(|(name, _)| name == path)
                    .map(|(_, source)| source.as_str())
            })
    }
}

/// Read a `.evt` file, collect its transitively imported `.mox` domains
/// and their `import schema`/`import sigil` content (sigil named imports
/// plus the sibling candidate pool), and compile. An unreadable contract
/// file is a hard [`Error::EvtModel`]; unreadable import files warn and
/// the compile reports the unsatisfied import as a diagnostic (which the
/// callers turn into the hard error).
pub fn read_and_compile_evt(path: &Path) -> Result<EvtCompiled> {
    let source = std::fs::read_to_string(path).map_err(|e| Error::EvtModel {
        file: path.display().to_string(),
        reason: format!("could not be read: {e}"),
    })?;
    let evt_path = path.display().to_string();
    let dir = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    // Transitive `import "x.mox"` collection (the shared .actor plumbing;
    // `.evt` imports carry no trailing semicolon — the scan tolerates both).
    let domains = collect_domains(dir, &source);
    let domain_pairs: Vec<(String, String)> = domains
        .iter()
        .map(|d| (d.path.clone(), d.source.clone()))
        .collect();

    let mut schema_imports = SchemaImports::new();
    let mut sigil = SigilImports::new();
    let mut sigil_sources: Vec<(String, String)> = Vec::new();
    for domain in &domains {
        // `import schema "<path>" as <Alias>`: JSON content keyed
        // (mox path, import path as written) — the compile contract.
        for decl in crate::ingest::mox_ingest::scan_schema_imports(&domain.source) {
            let abs_path = domain.dir.join(&decl.path);
            match std::fs::read_to_string(&abs_path) {
                Ok(json) => {
                    if let Err(e) = serde_json::from_str::<serde_json::Value>(&json) {
                        eprintln!(
                            "Warning: evt contract '{evt_path}' domain '{}' import schema '{}' \
                             is not valid JSON ({e}) — the compile reports the unsatisfied import",
                            domain.path, decl.path
                        );
                        continue;
                    }
                    schema_imports.insert(&domain.path, &decl.path, json);
                }
                Err(e) => eprintln!(
                    "Warning: evt contract '{evt_path}' domain '{}' import schema '{}' \
                     could not be read: {e}",
                    domain.path, decl.path
                ),
            }
        }
        // `import sigil "<path>"`: the named rosetta file keyed
        // (mox path, import path as written), plus the candidate pool of
        // every `*.rosetta` next to it (rosetta-internal namespace imports
        // resolve transitively; the driver lowers only the needed closure).
        for import_path in scan_sigil_imports(&domain.source) {
            let resolved = domain.dir.join(&import_path);
            let text = match std::fs::read_to_string(&resolved) {
                Ok(text) => text,
                Err(e) => {
                    eprintln!(
                        "Warning: evt contract '{evt_path}' domain '{}' import sigil '{}' \
                         could not be read: {e}",
                        domain.path, import_path
                    );
                    continue;
                }
            };
            sigil.insert(&domain.path, &import_path, text.clone());
            if !sigil_sources.iter().any(|(tag, _)| tag == &import_path) {
                sigil_sources.push((import_path.clone(), text));
            }
            for candidate in discover_rosetta_files(&resolved) {
                let candidate_path = candidate.display().to_string();
                if sigil_sources.iter().any(|(tag, _)| tag == &candidate_path) {
                    continue;
                }
                match std::fs::read_to_string(&candidate) {
                    Ok(text) => {
                        sigil.insert(&domain.path, &candidate_path, text.clone());
                        sigil_sources.push((candidate_path, text));
                    }
                    Err(e) => eprintln!(
                        "Warning: evt contract '{evt_path}' discovered sigil '{}' \
                         could not be read: {e}",
                        candidate.display()
                    ),
                }
            }
        }
    }

    let imports = DomainImports {
        schemas: schema_imports,
        sigil,
    };
    let compilation = compile_evt_str(&evt_path, &source, &domain_pairs, &imports);
    Ok(EvtCompiled {
        path: evt_path,
        source,
        mox_sources: domain_pairs,
        sigil_sources,
        compilation,
    })
}

/// Render a compilation's diagnostics per file with the rex renderer:
/// diagnostics are grouped by their file path (first-appearance order) and
/// each group is rendered against its own source text (contract file, mox
/// domain, or rosetta source). Paths with no retained source fall back to
/// plain `path: severity: message` lines.
pub fn render_evt_diagnostics(compiled: &EvtCompiled) -> String {
    let mut groups: Vec<(String, Vec<&rex_driver::Diagnostic>)> = Vec::new();
    for (path, diagnostic) in &compiled.compilation.diagnostics {
        match groups.iter_mut().find(|(name, _)| name == path) {
            Some((_, group)) => group.push(diagnostic),
            None => groups.push((path.clone(), vec![diagnostic])),
        }
    }
    let mut blocks: Vec<String> = Vec::new();
    for (path, group) in groups {
        match compiled.source_of(&path) {
            Some(source) => {
                let owned: Vec<rex_driver::Diagnostic> =
                    group.iter().map(|d| (*d).clone()).collect();
                blocks.push(rex_driver::render(&path, source, &owned));
            }
            None => {
                for diagnostic in group {
                    let severity = if diagnostic.is_error() {
                        "error"
                    } else {
                        "warning"
                    };
                    blocks.push(format!("{path}: {severity}: {}", diagnostic.message));
                }
            }
        }
    }
    blocks.join("\n")
}

/// Resolve an event payload field type against the graph's schema titles:
/// primitives resolve to `None` SILENTLY (they are normal — a primitive
/// payload never carries a schema title); every other reference resolves
/// by its short name (last segment after `::` then `.`) — exact title
/// first, then title + `type_suffix` (the `Customer` → `CustomerType`
/// convention, the `wire_alias_refs` chain). `None` for a non-primitive
/// means the type is not (yet) in the schema graph — the caller warns and
/// counts.
pub fn resolve_type_title(
    ty: &rex_ir::TypeRef,
    type_suffix: &str,
    titles: &HashSet<String>,
) -> Option<String> {
    match ty {
        rex_ir::TypeRef::Primitive(_) => None,
        _ => {
            let qualified = ty.qualified_name()?;
            resolve_with_suffix(short_name(&qualified), type_suffix, titles)
        }
    }
}

/// Convert a compiled rexlang event-contract artifact into its
/// graph-native representation.
///
/// Ordering contract (the [`EvtModelGraph`] flattening the querier's
/// `get_evt_models` restores): events, channels, and subscriptions keep
/// declaration order; every node's `ordinal` is its declaration index
/// within its kind, so the authored order reconstructs per kind.
/// `title_resolver` receives each payload field's rex-ir `TypeRef` and
/// returns the schema title, or `None` for a primitive or a type not in
/// the schema graph. Field `type_json` carries the rex-ir `TypeRef` wire
/// JSON (adjacent tagging under the `"type"` key); `version` is pinned
/// verbatim.
pub fn evt_model_graph_from_rex(
    model: &rex_ir::events::EventModel,
    source_path: &str,
    title_resolver: &dyn Fn(&rex_ir::TypeRef) -> Option<String>,
) -> EvtModelGraph {
    let events = model
        .events
        .iter()
        .enumerate()
        .map(|(ordinal, event)| EvtEventNode {
            source_path: source_path.to_string(),
            name: event.name.clone(),
            version: event.version.clone(),
            fields: event
                .fields
                .iter()
                .map(|field| EvtEventField {
                    name: field.name.clone(),
                    type_json: to_json(&field.ty),
                    resolved_title: title_resolver(&field.ty),
                })
                .collect(),
            ordinal,
        })
        .collect();
    let channels = model
        .channels
        .iter()
        .enumerate()
        .map(|(ordinal, channel)| EvtChannelNode {
            source_path: source_path.to_string(),
            name: channel.name.clone(),
            publishes: channel
                .publishes
                .iter()
                .map(|publication| publication.event.clone())
                .collect(),
            ordinal,
        })
        .collect();
    let subscriptions = model
        .subscriptions
        .iter()
        .enumerate()
        .map(|(ordinal, subscription)| EvtSubscriptionNode {
            source_path: source_path.to_string(),
            name: subscription.name.clone(),
            events: subscription.events.clone(),
            consumer: subscription.consumer.clone(),
            ordinal,
        })
        .collect();

    EvtModelGraph {
        source_path: source_path.to_string(),
        events,
        channels,
        subscriptions,
    }
}

/// Serialize a rex-ir wire payload (`TypeRef`) to its JSON value; a
/// serialization failure degrades to `Value::Null`, never a panic.
fn to_json<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_type_title_is_silent_on_primitives_and_hits_exact_then_suffixed() {
        let titles: HashSet<String> = ["OrderStatus", "WidgetType"]
            .into_iter()
            .map(str::to_string)
            .collect();

        // Primitives: None, no distinction from a miss at this layer (the
        // CALLER separates them via the variant match — never a warning).
        assert_eq!(
            resolve_type_title(
                &rex_ir::TypeRef::Primitive(rex_ir::PrimitiveType::String),
                "Type",
                &titles
            ),
            None
        );
        // Exact hit through both qualifier styles.
        assert_eq!(
            resolve_type_title(
                &rex_ir::TypeRef::Enum {
                    package: "nz.example.orders".to_string(),
                    name: "OrderStatus".to_string(),
                },
                "Type",
                &titles
            ),
            Some("OrderStatus".to_string())
        );
        assert_eq!(
            resolve_type_title(
                &rex_ir::TypeRef::Class {
                    package: "shop".to_string(),
                    name: "Widget".to_string(),
                },
                "Type",
                &titles
            ),
            Some("WidgetType".to_string())
        );
        // Miss.
        assert_eq!(
            resolve_type_title(
                &rex_ir::TypeRef::Class {
                    package: "shop".to_string(),
                    name: "Ghost".to_string(),
                },
                "Type",
                &titles
            ),
            None
        );
    }
}
