use crate::ProjectConfig;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;
use serde::Serialize;

use crate::GenerationEntry;
use crate::error::Result;
use crate::render_template_with_project;
use crate::traits::{GeneratedFile, GlobalGenerator, GlobalGeneratorKind};
use codegraph_config::DomainConfig;

#[derive(Debug, Serialize)]
struct RegistryContext {
    domains: Vec<RegistryDomain>,
}

#[derive(Debug, Serialize)]
struct RegistryDomain {
    name: String,
    entities: Vec<RegistryEntity>,
}

#[derive(Debug, Serialize)]
struct RegistryEntity {
    name: String,
    module_name: String,
}

#[derive(Debug, Serialize)]
struct DomainModContext {
    domain: String,
    entities: Vec<DomainModEntity>,
}

#[derive(Debug, Serialize)]
struct DomainModEntity {
    module_name: String,
}

/// Parse existing `pub mod <name>;` declarations from a file.
fn parse_existing_pub_mods(path: &Path) -> std::collections::HashSet<String> {
    let mut mods = std::collections::HashSet::new();
    if let Ok(content) = std::fs::read_to_string(path) {
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("pub mod ")
                && let Some(name) = rest.strip_suffix(';')
            {
                mods.insert(name.trim().to_string());
            }
        }
    }
    mods
}

pub struct HookRegistryGenerator {
    /// Base directory for generated hooks output.
    generated_dir: PathBuf,
}

impl HookRegistryGenerator {
    pub fn new_with_base(base_dir: PathBuf) -> Self {
        Self {
            // Hooks-api output lives under `src/generated/` (mirrors the
            // domain-types convention of writing into `{base}/src/`). The
            // base points at the crate root so the generated app's
            // `cosmos-hooks-api = { path = "../crates/cosmos-hooks-api" }`
            // dependency resolves to the crate's Cargo.toml.
            generated_dir: base_dir.join("src").join("generated"),
        }
    }
}

#[async_trait]
impl GlobalGenerator for HookRegistryGenerator {
    fn kind(&self) -> GlobalGeneratorKind {
        GlobalGeneratorKind::HookRegistry
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        config: &DomainConfig,
        generation_order: &[GenerationEntry],
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        // Group generation_order entries by domain (same pattern as ScaffoldGenerator)
        let mut domain_entity_map: std::collections::HashMap<String, Vec<RegistryEntity>> =
            std::collections::HashMap::new();
        let mut seen = std::collections::HashSet::new();
        let mut domain_order = Vec::new();
        let mut seen_domains = std::collections::HashSet::new();

        for entry in generation_order {
            let stripped = config.defaults.strip_suffix(&entry.schema_title);
            // Prefer the graph's canonical rust_type_name so registry imports
            // match hr-domain-types emissions exactly — naive pascal-casing
            // mangles acronym titles ("...LER-RSType" -> "...Lerrs...").
            let entity_name = db
                .get_schema_in_domain(&entry.schema_title, &entry.domain)
                .await
                .ok()
                .flatten()
                .map(|s| s.rust_type_name)
                .unwrap_or_else(|| codegraph_naming::to_pascal_case(&stripped));
            let module_name = codegraph_naming::to_snake_case(&stripped);

            if seen_domains.insert(entry.domain.clone()) {
                domain_order.push(entry.domain.clone());
            }

            // Dedup by (domain, module_name)
            if !seen.insert((entry.domain.clone(), module_name.clone())) {
                continue;
            }

            domain_entity_map
                .entry(entry.domain.clone())
                .or_default()
                .push(RegistryEntity {
                    name: entity_name,
                    module_name,
                });
        }

        let domains: Vec<RegistryDomain> = domain_order
            .iter()
            .filter_map(|name| {
                let entities = domain_entity_map.remove(name.as_str())?;
                Some(RegistryDomain {
                    name: name.clone(),
                    entities,
                })
            })
            .collect();

        let registry_ctx = RegistryContext {
            domains: domains
                .iter()
                .map(|d| RegistryDomain {
                    name: d.name.clone(),
                    entities: d
                        .entities
                        .iter()
                        .map(|e| RegistryEntity {
                            name: e.name.clone(),
                            module_name: e.module_name.clone(),
                        })
                        .collect(),
                })
                .collect(),
        };

        let generated_dir = &self.generated_dir;
        let mut files = Vec::new();

        // 1. Render registry.rs
        let registry_content =
            render_template_with_project(tera, "hooks/registry.tera", &registry_ctx, project)?;
        files.push(GeneratedFile {
            path: generated_dir.join("registry.rs"),
            content: registry_content,
        });

        // 1b. Render types.rs — the persistence-agnostic hook context
        //     contract (issue #509): TxContext/PostCommitContext plus the
        //     HookTx/HookConnSource handle SPIs. Emitted only when the
        //     hooks surface is configured — an empty file in the tree
        //     would still perturb byte-identity canaries.
        if !project.identity.hooks_api_crate.is_empty() {
            let types_content =
                render_template_with_project(tera, "hooks/types.tera", &registry_ctx, project)?;
            files.push(GeneratedFile {
                path: generated_dir.join("types.rs"),
                content: types_content,
            });
        }

        // 2. Render top-level generated/mod.rs, preserving any existing domain
        //    modules not in the current generation order. This prevents stripping
        //    modules for domains that aren't in the graph but were previously generated.
        let mod_path = generated_dir.join("mod.rs");
        let existing_modules = if mod_path.exists() {
            parse_existing_pub_mods(&mod_path)
        } else {
            std::collections::HashSet::new()
        };
        // Merge: keep existing modules and add new ones from this generation
        // (`registry`/`types` are always rendered by the template, never
        // treated as domains).
        let mut all_domain_names: Vec<String> = domains.iter().map(|d| d.name.clone()).collect();
        for existing in &existing_modules {
            if existing != "registry" && existing != "types" && !all_domain_names.contains(existing)
            {
                all_domain_names.push(existing.clone());
            }
        }
        all_domain_names.sort();
        let merged_ctx = RegistryContext {
            domains: all_domain_names
                .iter()
                .map(|name| RegistryDomain {
                    name: name.clone(),
                    entities: domains
                        .iter()
                        .find(|d| &d.name == name)
                        .map(|d| {
                            d.entities
                                .iter()
                                .map(|e| RegistryEntity {
                                    name: e.name.clone(),
                                    module_name: e.module_name.clone(),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect(),
        };
        let generated_mod_content =
            render_template_with_project(tera, "hooks/generated_mod.tera", &merged_ctx, project)?;
        files.push(GeneratedFile {
            path: mod_path,
            content: generated_mod_content,
        });

        // 3. Render per-domain mod.rs
        for domain in &domains {
            let domain_mod_ctx = DomainModContext {
                domain: domain.name.clone(),
                entities: domain
                    .entities
                    .iter()
                    .map(|e| DomainModEntity {
                        module_name: e.module_name.clone(),
                    })
                    .collect(),
            };
            let domain_mod_content = render_template_with_project(
                tera,
                "hooks/domain_mod.tera",
                &domain_mod_ctx,
                project,
            )?;
            files.push(GeneratedFile {
                path: generated_dir.join(&domain.name).join("mod.rs"),
                content: domain_mod_content,
            });
        }

        Ok(files)
    }
}
