//! rexlang `.deploy` deployment ingest (rexlang #49): resolve each
//! deployment file's `import`ed `.ddd` designs and `.ifml` flows, compile
//! with `rex_driver::compile_deploy_str`, convert the resulting
//! `rex_ir::deploy::DeployModel` into the graph-native
//! [`DeployModelGraph`], and land it through
//! [`GraphIngestor::ingest_deploy_model`].
//!
//! Design imports compile through the SAME helper the `.ddd` ingest uses
//! ([`crate::ingest::ddd_ingest::read_and_compile_ddd`]), so
//! union-dependent designs compile against their full transitive `.mox`
//! set; flow imports parse the way the driver's IFML pass does. A
//! deployment file is self-contained for names — resolution of `connects`
//! endpoints, `deployment ... for`, and `use` references happens inside
//! the rex driver, and the graph keeps everything verbatim.
//!
//! Compilation failures are HARD errors ([`Error::DeployModel`], the
//! [`Error::EvtModel`] precedent) — a broken deployment never half-ingests.

use std::path::{Path, PathBuf};

use codegraph_core::traits::GraphIngestor;
use codegraph_core::types::{
    DeployApplicationNode, DeployBindingNode, DeployComponentNode, DeployConnectionNode,
    DeployDeploymentNode, DeployMappingNode, DeployModelGraph, DeployPolicyNode, DeployProfileNode,
    DeploySettingNode, DeployValueNode,
};

use crate::error::{Error, Result};

// ── Counters ────────────────────────────────────────────────────────────────

/// Counters for one [`ingest_deploy_files`] run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeployIngestStats {
    /// The number of `.deploy` files handed to the run.
    pub files: usize,
    /// Applications ingested across all deployment models.
    pub applications: usize,
    /// Profiles ingested across all deployment models.
    pub profiles: usize,
    /// Deployments ingested across all deployment models.
    pub deployments: usize,
}

impl std::fmt::Display for DeployIngestStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} files, {} applications, {} profiles, {} deployments",
            self.files, self.applications, self.profiles, self.deployments
        )
    }
}

// ── Conversion ──────────────────────────────────────────────────────────────

/// Convert a compiled rexlang deployment artifact into its graph-native
/// representation. Every name, path, expression, and resource crosses
/// verbatim; component kinds and targets lower through their `.keyword()`
/// (the authored spelling) because codegraph-core carries no rex types.
/// Ordinals are declaration order within their parent (`ordinal` on
/// components; every other family keeps its Vec order).
pub fn deploy_model_to_graph(
    source_path: &str,
    model: &rex_ir::deploy::DeployModel,
) -> DeployModelGraph {
    let applications = model
        .applications
        .iter()
        .map(|application| DeployApplicationNode {
            source_path: source_path.to_string(),
            name: application.name.clone(),
            components: application
                .components
                .iter()
                .enumerate()
                .map(|(ordinal, component)| DeployComponentNode {
                    application: application.name.clone(),
                    name: component.name.clone(),
                    kind: component.kind.keyword().to_string(),
                    requires: component.requires.clone(),
                    settings: component
                        .settings
                        .iter()
                        .map(|setting| DeploySettingNode {
                            path: setting.path.clone(),
                            value: setting_value(&setting.value),
                        })
                        .collect(),
                    designs: component
                        .designs
                        .iter()
                        .map(|binding| DeployBindingNode {
                            source: binding.source.clone(),
                            module: binding.module.clone(),
                        })
                        .collect(),
                    flows: component
                        .flows
                        .iter()
                        .map(|binding| DeployBindingNode {
                            source: binding.source.clone(),
                            module: binding.module.clone(),
                        })
                        .collect(),
                    ordinal,
                })
                .collect(),
            connections: application
                .connections
                .iter()
                .map(|connection| DeployConnectionNode {
                    application: application.name.clone(),
                    from: connection.from.clone(),
                    to: connection.to.clone(),
                })
                .collect(),
        })
        .collect();
    let profiles = model
        .profiles
        .iter()
        .map(|profile| DeployProfileNode {
            source_path: source_path.to_string(),
            name: profile.name.clone(),
            target: profile.target.keyword().to_string(),
            defaults: profile
                .defaults
                .iter()
                .map(|setting| DeploySettingNode {
                    path: setting.path.clone(),
                    value: setting_value(&setting.value),
                })
                .collect(),
            policies: profile
                .policies
                .iter()
                .map(|policy| DeployPolicyNode {
                    profile: profile.name.clone(),
                    prohibit: policy.prohibit,
                    expression: policy.expression.clone(),
                })
                .collect(),
            mappings: profile
                .mappings
                .iter()
                .map(|mapping| DeployMappingNode {
                    profile: profile.name.clone(),
                    kind: mapping.kind.keyword().to_string(),
                    resource: mapping.resource.clone(),
                })
                .collect(),
        })
        .collect();
    let deployments = model
        .deployments
        .iter()
        .map(|deployment| DeployDeploymentNode {
            source_path: source_path.to_string(),
            name: deployment.name.clone(),
            application: deployment.application.clone(),
            profile: deployment.profile.clone(),
            configure: deployment
                .configure
                .iter()
                .map(|setting| DeploySettingNode {
                    path: setting.path.clone(),
                    value: setting_value(&setting.value),
                })
                .collect(),
        })
        .collect();

    DeployModelGraph {
        source_path: source_path.to_string(),
        applications,
        profiles,
        deployments,
    }
}

/// Lower a rex-ir setting value to its graph-native tagged enum (variant
/// for variant; the wire shapes match under the codegraph-core mirror).
fn setting_value(value: &rex_ir::deploy::DeployValue) -> DeployValueNode {
    match value {
        rex_ir::deploy::DeployValue::Int(value) => DeployValueNode::Int(*value),
        rex_ir::deploy::DeployValue::Text(value) => DeployValueNode::Text(value.clone()),
        rex_ir::deploy::DeployValue::Word(value) => DeployValueNode::Word(value.clone()),
    }
}

// ── Compile ─────────────────────────────────────────────────────────────────

/// One `.deploy` file read, its imports resolved and pre-compiled, and the
/// deployment compiled — the shared result both [`ingest_deploy_files`]
/// and the doctor `--deploy-files` check consume so their import
/// resolution cannot drift.
pub struct DeployCompiled {
    /// The deployment file's driver path (as given on the command line).
    pub path: String,
    /// The deployment file's source text.
    pub source: String,
    /// The imported design sources as `(resolved path, source text)` —
    /// keys are the paths the rex driver's import matcher resolves
    /// against (import-as-written joined onto the deployment file's
    /// directory).
    pub design_sources: Vec<(String, String)>,
    /// The imported flow sources, keyed like [`Self::design_sources`].
    pub flow_sources: Vec<(String, String)>,
    /// The compile's diagnostics, verbatim from the rex driver (the raw
    /// material [`render_deploy_diagnostics`] groups and renders).
    pub diagnostics: Vec<(String, rex_driver::Diagnostic)>,
    /// The converted graph, present exactly when the compile produced a
    /// model (error-severity diagnostics drop it).
    pub graph: Option<DeployModelGraph>,
}

impl DeployCompiled {
    /// The source text of the file with the given driver path, if it is
    /// the deployment file or an imported design/flow (the rex-cli
    /// `source_of` shape).
    fn source_of(&self, path: &str) -> Option<&str> {
        if path == self.path {
            return Some(&self.source);
        }
        self.design_sources
            .iter()
            .chain(self.flow_sources.iter())
            .find(|(name, _)| name == path)
            .map(|(_, source)| source.as_str())
    }
}

/// Wrap [`rex_driver::compile_deploy_str`] and convert its model: the pure
/// compile step [`read_and_resolve_deploy`] lands on after resolving the
/// file's imports. Error-severity diagnostics drop the model (the driver
/// already yields `model: None` for them — this mirrors that in the
/// returned diagnostics so callers can branch on one shape).
///
/// Sync (the [`read_and_compile_ddd`]/`read_and_compile_evt` precedent):
/// `compile_deploy_str` is a pure sync driver call and the conversion does
/// no I/O, so there is nothing to await.
pub fn read_and_compile_deploy(
    path: &str,
    source: &str,
    designs: &[(String, rex_ir::ddd::DddModel)],
    flows: &[(String, rex_ir::ifml::IfmlModel)],
) -> (
    Option<DeployModelGraph>,
    Vec<(String, rex_driver::Diagnostic)>,
) {
    let compilation = rex_driver::compile_deploy_str(path, source, designs, flows);
    let graph = compilation
        .model
        .as_ref()
        .map(|model| deploy_model_to_graph(path, model));
    (graph, compilation.diagnostics)
}

/// Read a `.deploy` file, resolve its `import`ed designs/flows, compile
/// the deployment against them, and convert. An unreadable deployment
/// file, a design import that fails to compile, or a flow import that
/// fails to parse is a hard [`Error::DeployModel`] (the deployment cannot
/// compile without its imports); an import with an unknown extension is
/// skipped — the compile reports it as an unsatisfied import, which the
/// caller turns into the hard error with the rendered diagnostic.
pub fn read_and_resolve_deploy(path: &Path) -> Result<DeployCompiled> {
    let source = std::fs::read_to_string(path).map_err(|e| Error::DeployModel {
        file: path.display().to_string(),
        reason: format!("could not be read: {e}"),
    })?;
    let deploy_path = path.display().to_string();
    let dir = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    // `import "<path>"` lines (the shared rexlang scan — `.deploy` imports
    // carry no trailing-semicolon complication the scan doesn't already
    // tolerate).
    let mut designs: Vec<(String, rex_ir::ddd::DddModel)> = Vec::new();
    let mut flows: Vec<(String, rex_ir::ifml::IfmlModel)> = Vec::new();
    let mut design_sources: Vec<(String, String)> = Vec::new();
    let mut flow_sources: Vec<(String, String)> = Vec::new();
    for import in crate::ingest::rex_imports::scan_imports(&source) {
        // The key the rex driver's import matcher resolves against: the
        // import as written joined onto the deployment file's directory
        // (its lexical normalization is the matcher's other branch).
        let resolved = dir.join(&import);
        let key = resolved.display().to_string();
        match resolved.extension().and_then(|e| e.to_str()) {
            Some("ddd") => {
                let compiled = crate::ingest::ddd_ingest::read_and_compile_ddd(&resolved)?;
                let Some(model) = compiled.compilation.model else {
                    return Err(Error::DeployModel {
                        file: deploy_path.clone(),
                        reason: format!(
                            "imported design '{}' does not compile:\n{}",
                            import,
                            crate::ingest::ddd_ingest::render_ddd_diagnostics(&compiled),
                        ),
                    });
                };
                design_sources.push((key.clone(), compiled.source));
                designs.push((key, model));
            }
            Some("ifml") => match rex_ifml::parse_ifml_file(&resolved) {
                Ok(model) => {
                    let source_text = std::fs::read_to_string(&resolved).unwrap_or_default();
                    flow_sources.push((key.clone(), source_text));
                    flows.push((key, model));
                }
                Err(e) => {
                    return Err(Error::DeployModel {
                        file: deploy_path.clone(),
                        reason: format!("imported flow '{import}' does not parse: {e}"),
                    });
                }
            },
            _ => {
                // Unknown extension: skip — the compile reports the
                // unsatisfied (or unsupported) import as a diagnostic.
            }
        }
    }

    let (graph, diagnostics) = read_and_compile_deploy(&deploy_path, &source, &designs, &flows);
    Ok(DeployCompiled {
        path: deploy_path,
        source,
        design_sources,
        flow_sources,
        diagnostics,
        graph,
    })
}

// ── Diagnostics rendering ───────────────────────────────────────────────────

/// Render a deployment compilation's diagnostics per file with the rex
/// renderer: diagnostics are grouped by their file path (first-appearance
/// order) and each group is rendered against its own source text (the
/// deployment file or an imported design/flow). Paths with no retained
/// source fall back to plain `path: severity: message` lines.
pub fn render_deploy_diagnostics(compiled: &DeployCompiled) -> String {
    let mut groups: Vec<(String, Vec<&rex_driver::Diagnostic>)> = Vec::new();
    for (path, diagnostic) in &compiled.diagnostics {
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

// ── Ingest ──────────────────────────────────────────────────────────────────

/// Ingest rexlang `.deploy` deployment files into the graph's deployment
/// plane.
///
/// Per file: read + resolve imports + compile through
/// [`read_and_resolve_deploy`] (a design import that does not compile, a
/// flow import that does not parse, or any error-severity diagnostic is a
/// hard [`Error::DeployModel`] with the rendered output), then ingest one
/// [`DeployModelGraph`] per file.
pub async fn ingest_deploy_files(
    ingestor: &dyn GraphIngestor,
    deploy_paths: &[PathBuf],
) -> Result<DeployIngestStats> {
    let mut stats = DeployIngestStats {
        files: deploy_paths.len(),
        ..Default::default()
    };
    for path in deploy_paths {
        let compiled = read_and_resolve_deploy(path)?;
        let has_errors = compiled.diagnostics.iter().any(|(_, d)| d.is_error());
        if has_errors || compiled.graph.is_none() {
            let reason = render_deploy_diagnostics(&compiled);
            return Err(Error::DeployModel {
                file: compiled.path.clone(),
                reason: if reason.is_empty() {
                    "compilation produced no deployment artifact".to_string()
                } else {
                    reason
                },
            });
        }
        let graph = compiled.graph.expect("checked above");
        stats.applications += graph.applications.len();
        stats.profiles += graph.profiles.len();
        stats.deployments += graph.deployments.len();
        ingestor
            .ingest_deploy_model(&graph)
            .await
            .map_err(Error::Graph)?;
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full conversion shape: components (kind keyword, requires,
    /// settings, ordinals), connections, profiles (target, defaults,
    /// policies, mappings), deployments (configure) — all verbatim,
    /// declaration order everywhere. Design/flow binding conversion rides
    /// the end-to-end fixture test (bindings validate against provided
    /// imports, which the pure unit test has none of).
    #[test]
    fn conversion_maps_kinds_targets_settings_and_declaration_order() {
        let source = concat!(
            "application Shop {\n",
            "    component gateway: api {\n",
            "        replicas: 2\n",
            "        requires edgeExecution\n",
            "    }\n",
            "    component db: database { engine: d1 }\n",
            "    connects gateway -> db\n",
            "}\n",
            "\n",
            "profile edge {\n",
            "    target: cloudflareWorkers\n",
            "    defaults { db.engine: d1 }\n",
            "    prohibit (db.engine == \"postgres\")\n",
            "    mappings { database -> cloudflare.d1 }\n",
            "}\n",
            "\n",
            "deployment prod for Shop {\n",
            "    use edge\n",
            "    configure { gateway.replicas: 4 }\n",
            "}\n",
        );
        let compilation = rex_driver::compile_deploy_str("shop.deploy", source, &[], &[]);
        assert!(
            compilation.diagnostics.is_empty(),
            "{:?}",
            compilation.diagnostics
        );
        let model = compilation.model.expect("clean deploy compiles");

        let graph = deploy_model_to_graph("shop.deploy", &model);
        assert_eq!(graph.source_path, "shop.deploy");

        let app = &graph.applications[0];
        assert_eq!(app.name, "Shop");
        assert_eq!(app.source_path, "shop.deploy");
        assert_eq!(
            app.components
                .iter()
                .map(|c| (c.name.as_str(), c.kind.as_str(), c.ordinal))
                .collect::<Vec<_>>(),
            vec![("gateway", "api", 0), ("db", "database", 1)],
        );
        let gateway = &app.components[0];
        assert_eq!(gateway.requires, vec!["edgeExecution"]);
        assert_eq!(
            gateway.settings,
            vec![DeploySettingNode {
                path: "replicas".to_string(),
                value: DeployValueNode::Int(2),
            }],
        );
        assert!(gateway.designs.is_empty());
        assert!(gateway.flows.is_empty());
        assert_eq!(
            app.connections,
            vec![DeployConnectionNode {
                application: "Shop".to_string(),
                from: "gateway".to_string(),
                to: "db".to_string(),
            }],
        );

        let profile = &graph.profiles[0];
        assert_eq!(profile.name, "edge");
        assert_eq!(profile.target, "cloudflareWorkers");
        assert_eq!(
            profile.defaults,
            vec![DeploySettingNode {
                path: "db.engine".to_string(),
                value: DeployValueNode::Word("d1".to_string()),
            }],
        );
        assert_eq!(
            profile.policies,
            vec![DeployPolicyNode {
                profile: "edge".to_string(),
                prohibit: true,
                expression: r#"db.engine == "postgres""#.to_string(),
            }],
        );
        assert_eq!(
            profile.mappings,
            vec![DeployMappingNode {
                profile: "edge".to_string(),
                kind: "database".to_string(),
                resource: "cloudflare.d1".to_string(),
            }],
        );

        let deployment = &graph.deployments[0];
        assert_eq!(deployment.name, "prod");
        assert_eq!(deployment.application, "Shop");
        assert_eq!(deployment.profile, "edge");
        assert_eq!(
            deployment.configure,
            vec![DeploySettingNode {
                path: "gateway.replicas".to_string(),
                value: DeployValueNode::Int(4),
            }],
        );
    }

    /// Error-severity diagnostics drop the model, and the diagnostics
    /// survive the wrapper (the ingest hard-error's raw material).
    #[test]
    fn compile_diagnostics_and_dropped_model_flow_through() {
        let (graph, diagnostics) =
            read_and_compile_deploy("bad.deploy", "application Broken {", &[], &[]);
        assert!(graph.is_none(), "a parse error drops the model");
        assert!(
            diagnostics.iter().any(|(_, d)| d.is_error()),
            "the parse error is reported: {diagnostics:?}"
        );
    }
}
