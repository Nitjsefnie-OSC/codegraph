use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::db::dialect::DatabaseTarget;
use crate::error::{Error, Result};

use super::capabilities::{CapabilityRegistry, GeneratorKind};
use super::resolve::{
    IfmlFrameworkTarget, ResolvedProfile, ResolvedSection, default_framework_target,
};
use super::types::{DependencyStrategy, DeploymentTopology, PersistenceProvider};

// ── Build Plan ───────────────────────────────────────────────────────────────

/// Resolved execution plan for a profile.
///
/// Groups generator names by their [`GeneratorKind`] so the generator dispatch
/// in `run_generators_with_opts` can filter which generators to instantiate.
#[derive(Debug, Clone)]
pub struct BuildPlan {
    /// Entity generator names to run (per-entity, across all sections).
    pub entity_generators: Vec<String>,
    /// Domain generator names to run (per-domain, across all sections).
    pub domain_generators: Vec<String>,
    /// Global generator names to run (once, across all sections).
    pub global_generators: Vec<String>,
    /// Post-gen scripts collected from all sections, ordered alphabetically by section name.
    pub post_gen_scripts: Vec<(String, Vec<String>)>,
    /// IFML framework targets configured for this build.
    pub ifml_frameworks: Vec<IfmlFrameworkTarget>,
    /// Optional template pack directory override from the selected variant.
    pub template_pack_path: Option<PathBuf>,
    /// Database target dialect for SQL generation (default: Postgres).
    pub database_target: DatabaseTarget,
    /// Whether atproto generators are enabled (from `atproto_backend` feature).
    pub has_atproto: bool,
    /// AT Protocol tenancy mode: "shared_pds" or "per_org_pds".
    pub atproto_tenancy: String,
    /// Whether Fern SDK generation is enabled (from `fern_sdk` feature).
    pub has_fern: bool,
    /// Fern SDK languages to generate (from `fern_sdk_languages` feature, defaults to ["typescript"]).
    pub fern_sdk_languages: Vec<String>,
    /// Whether EmDash plugin generation is enabled (from `emdash_plugins` feature).
    pub has_emdash: bool,
    /// Persistence provider for entity/repository code generation (default: SeaOrm).
    pub persistence_provider: PersistenceProvider,
    /// DTO serde key casing for domain-types DTOs (default: "snake").
    /// "camel" emits `#[serde(rename_all = "camelCase")]` on create/update/response
    /// DTOs (community-os wire contract); "snake" leaves keys at the Rust field
    /// names (hr-specs wire contract).
    pub dto_key_casing: String,
    /// Deployment topology for the generated application (default: Monolith).
    pub deployment_topology: DeploymentTopology,
    /// How the generated app's Cargo.toml references codegraph crates
    /// (issue #347; default: `Rev` = the historical git+rev pins).
    /// `Path` emits ABSOLUTE `path` dependencies into the checkout that ran
    /// generation (`project.codegraph_path_root`), so the generated output
    /// carries build-machine paths and is only buildable on the generating
    /// machine (or wherever that checkout lives) — that is the documented
    /// cost of the flag, which exists for local development loops.
    pub dependency_strategy: DependencyStrategy,
    /// Namespace-aware module layout (issue #268): when true, schemas that
    /// carry a namespace emit under namespace-derived module paths
    /// (`cdm.base.datetime` → `cdm/base/datetime/...`) instead of the flat
    /// domain layout. Default OFF = byte-identical flat output.
    pub namespace_layout: bool,
    /// UX rules plane (issue #293): when true, the built-in `ux-default`
    /// pack resolves into `ProjectConfig.ux` for every template and
    /// generator. Default OFF = `ProjectConfig.ux` stays `None`,
    /// byte-identical output.
    pub ux_rules: bool,
    /// IFML test-side auth surface (issue #463): `ifml_e2e_auth` feature.
    pub ifml_e2e_auth: bool,
    /// One testid vocabulary (issue #468 W3): `testid_module` feature —
    /// emits `ui/src/lib/testids.ts` and switches the POM/spec templates
    /// to import it. Default false = byte-identical.
    pub testid_module: bool,
    /// Post-generation structural gates (issue #468): `structural_gates`
    /// feature — the run hard-fails on retired vocabulary in the emitted
    /// tree. Default false = zero behavior.
    pub structural_gates: bool,
    /// Canonical expression IR (issue #278): when true, IFML guards render
    /// from the persisted `expr_json` AST via the TypeScript lowering
    /// instead of raw source interpolation. Default OFF = byte-identical.
    pub expr_ir: bool,
    /// Public-operations consumer (issue #279): when true, schemas carrying
    /// `access = Public` whose entity config declares `public_operations`
    /// emit permissive `TO PUBLIC` RLS policies (`public_operations_rls`
    /// global generator) and mount their routes without the permission
    /// layers. Default OFF = byte-identical.
    pub public_operations_rls: bool,
    /// Feature flags from the profile (e.g., has_admin_cli, auth, etc.).
    pub features: toml::Table,
}

impl BuildPlan {
    /// Construct a build plan from a resolved profile and the capability registry.
    ///
    /// Returns an error if any generator name is unknown or if feature requirements
    /// are not met.
    pub fn from_profile(profile: &ResolvedProfile, registry: &CapabilityRegistry) -> Result<Self> {
        let expanded_sections =
            Self::expand_ifml_sections(&profile.sections, &profile.ifml_frameworks);

        let validation_profile = ResolvedProfile {
            sections: expanded_sections.clone(),
            ..profile.clone()
        };
        registry.validate_profile(&validation_profile)?;

        let mut entity_gens = Vec::new();
        let mut domain_gens = Vec::new();
        let mut global_gens = Vec::new();
        let mut post_gen_scripts = Vec::new();

        let mut section_names: Vec<&String> = expanded_sections.keys().collect();
        section_names.sort();
        for section_name in section_names {
            let section = &expanded_sections[section_name];
            if section.generators.is_empty() {
                continue;
            }
            for gen_name in &section.generators {
                let cap = registry.get(gen_name).expect("already validated");
                match cap.kind {
                    GeneratorKind::Entity => {
                        if !entity_gens.contains(gen_name) {
                            entity_gens.push(gen_name.clone());
                        }
                    }
                    GeneratorKind::Domain => {
                        if !domain_gens.contains(gen_name) {
                            domain_gens.push(gen_name.clone());
                        }
                    }
                    GeneratorKind::Global => {
                        if !global_gens.contains(gen_name) {
                            global_gens.push(gen_name.clone());
                        }
                    }
                }
            }
            if !section.scripts.is_empty() {
                post_gen_scripts.push((section_name.clone(), section.scripts.clone()));
            }
        }

        // Parse database_target from features (default: Postgres)
        let database_target = profile
            .features
            .get("database_target")
            .and_then(|v| v.as_str())
            .map(DatabaseTarget::from_config)
            .unwrap_or_default();

        // Parse atproto features
        let has_atproto = profile
            .features
            .get("atproto_backend")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let atproto_tenancy = profile
            .features
            .get("atproto_tenancy")
            .and_then(|v| v.as_str())
            .unwrap_or("shared_pds")
            .to_string();

        // Parse fern sdk features
        let has_fern = profile
            .features
            .get("fern_sdk")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let fern_sdk_languages: Vec<String> = profile
            .features
            .get("fern_sdk_languages")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_else(|| vec!["typescript".to_string()]);

        // Parse emdash plugin feature
        let has_emdash = profile
            .features
            .get("emdash_plugins")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Parse persistence_provider from features (default: SeaOrm)
        let persistence_provider = profile
            .features
            .get("persistence_provider")
            .and_then(|v| v.as_str())
            .map(PersistenceProvider::from_config)
            .unwrap_or_default();

        // Parse deployment_topology from features (default: Monolith).
        // Unlike persistence_provider, unknown values are a hard error.
        let deployment_topology = match profile.features.get("deployment_topology") {
            Some(v) => {
                let s = v.as_str().ok_or_else(|| {
                    Error::Config("feature \"deployment_topology\" must be a string".to_string())
                })?;
                DeploymentTopology::from_config(s)?
            }
            None => DeploymentTopology::default(),
        };

        // Parse dependency_strategy from features (issue #347; default: Rev
        // = byte-identical). Same strictness as deployment_topology: unknown
        // values are a hard error (a typo must not silently flip the emitted
        // dependency shape) and a non-string value is a parse error naming
        // the key.
        let dependency_strategy = match profile.features.get("dependency_strategy") {
            Some(v) => {
                let s = v.as_str().ok_or_else(|| {
                    Error::Config("feature \"dependency_strategy\" must be a string".to_string())
                })?;
                DependencyStrategy::from_config(s)?
            }
            None => DependencyStrategy::default(),
        };

        // Topology × provider rule: the workers scaffold emits per-domain
        // Cloudflare Worker crates whose wasm32 slice cannot link SeaORM
        // (sqlx/mio do not compile to wasm32-unknown-unknown). Cornucopia is
        // the only supported persistence provider for workers topology.
        if deployment_topology == DeploymentTopology::Workers
            && persistence_provider != PersistenceProvider::Cornucopia
        {
            return Err(Error::Config(format!(
                "workers topology requires the cornucopia persistence provider \
                 (deployment_topology = \"workers\" with persistence_provider = \
                 \"{}\" is not supported)",
                persistence_provider.as_str()
            )));
        }

        // Parse dto_key_casing from features (default: "snake").
        // Unknown values fall back to "snake" — worst case the wire keys
        // stay at the Rust field names (the historical contract).
        let dto_key_casing = match profile.features.get("dto_key_casing") {
            Some(v) => match v.as_str() {
                Some("camel") => "camel".to_string(),
                _ => "snake".to_string(),
            },
            None => "snake".to_string(),
        };

        // Parse namespace_layout from features (default: false = flat).
        let namespace_layout = profile
            .features
            .get("namespace_layout")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Parse ux_rules from features (default: false = byte-identical).
        // Unlike namespace_layout's lenient swallow, a non-bool value is a
        // parse error naming the key (the flag gates an output plane, so a
        // typo'd `ux_rules = "yes"` must not silently disable it).
        let ux_rules = match profile.features.get("ux_rules") {
            Some(v) => v.as_bool().ok_or_else(|| {
                Error::Config("feature \"ux_rules\" must be a boolean".to_string())
            })?,
            None => false,
        };

        // Parse expr_ir from features (issue #278; default: false).
        let expr_ir = profile
            .features
            .get("expr_ir")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Parse ifml_e2e_auth from features (issue #463; default: false).
        let ifml_e2e_auth = profile
            .features
            .get("ifml_e2e_auth")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Parse testid_module from features (issue #468 W3; default: false).
        let testid_module = profile
            .features
            .get("testid_module")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Parse structural_gates from features (issue #468; default: false).
        let structural_gates = profile
            .features
            .get("structural_gates")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Parse public_operations_rls from features (issue #279; default: false).
        let public_operations_rls = profile
            .features
            .get("public_operations_rls")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok(BuildPlan {
            entity_generators: entity_gens,
            domain_generators: domain_gens,
            global_generators: global_gens,
            post_gen_scripts,
            ifml_frameworks: profile.ifml_frameworks.clone(),
            template_pack_path: profile.template_pack_path.clone(),
            database_target,
            has_atproto,
            atproto_tenancy,
            has_fern,
            fern_sdk_languages,
            has_emdash,
            persistence_provider,
            dto_key_casing,
            deployment_topology,
            dependency_strategy,
            namespace_layout,
            ux_rules,
            expr_ir,
            ifml_e2e_auth,
            testid_module,
            structural_gates,
            public_operations_rls,
            features: profile.features.clone(),
        })
    }

    /// Construct a minimal IFML-only build plan for the given frameworks.
    ///
    /// Used by the `ifml_generate` driver when no profiles.toml is provided
    /// (or when the provided profile does not declare per-framework IFML
    /// generators). Each framework gets `ifml_skeleton_{fw}`, `ifml_route_{fw}`,
    /// `ifml_navigation_{fw}`, and `ifml_e2e_test_{fw}` in the global
    /// generator list, the `ifml_backend` + `framework_{fw}` features, and a
    /// default `IfmlFrameworkTarget`. Unknown framework names are rejected
    /// against the capability registry.
    pub fn ifml_only(frameworks: &[String]) -> Result<Self> {
        let registry = CapabilityRegistry::new();
        let mut features = toml::Table::new();
        features.insert("ifml_backend".to_string(), toml::Value::Boolean(true));
        let mut global_generators = Vec::new();
        let mut ifml_frameworks = Vec::new();
        for fw in frameworks {
            let route_name = format!("ifml_route_{fw}");
            if registry.get(&route_name).is_none() {
                return Err(Error::Config(format!(
                    "unknown IFML framework \"{fw}\"; known frameworks: \
                     svelte, react, vue, flutter, swiftui"
                )));
            }
            features.insert(format!("framework_{fw}"), toml::Value::Boolean(true));
            global_generators.push(format!("ifml_skeleton_{fw}"));
            global_generators.push(route_name);
            global_generators.push(format!("ifml_navigation_{fw}"));
            global_generators.push(format!("ifml_e2e_test_{fw}"));
            ifml_frameworks.push(IfmlFrameworkTarget {
                name: fw.clone(),
                output: None,
                target: default_framework_target(),
            });
        }
        Ok(BuildPlan {
            entity_generators: Vec::new(),
            domain_generators: Vec::new(),
            global_generators,
            post_gen_scripts: Vec::new(),
            ifml_frameworks,
            template_pack_path: None,
            database_target: DatabaseTarget::default(),
            has_atproto: false,
            atproto_tenancy: "shared_pds".to_string(),
            has_fern: false,
            fern_sdk_languages: vec!["typescript".to_string()],
            has_emdash: false,
            persistence_provider: PersistenceProvider::default(),
            dto_key_casing: "snake".to_string(),
            deployment_topology: DeploymentTopology::default(),
            dependency_strategy: DependencyStrategy::default(),
            namespace_layout: false,
            ux_rules: false,
            expr_ir: false,
            ifml_e2e_auth: false,
            testid_module: false,
            structural_gates: false,
            public_operations_rls: false,
            features,
        })
    }

    /// Expand IFML generators in sections based on configured frameworks.
    ///
    /// When `ifml_frameworks` is non-empty, each `ifml_route` generator is
    /// replaced with `ifml_route_{framework}` for every configured framework,
    /// and similarly for `ifml_skeleton`, `ifml_navigation`, and
    /// `ifml_e2e_test`. If no frameworks are configured, sections are
    /// returned unchanged (backward compatible).
    fn expand_ifml_sections(
        sections: &HashMap<String, ResolvedSection>,
        ifml_frameworks: &[IfmlFrameworkTarget],
    ) -> HashMap<String, ResolvedSection> {
        if ifml_frameworks.is_empty() {
            return sections.clone();
        }

        sections
            .iter()
            .map(|(name, section)| {
                let generators: Vec<String> = section
                    .generators
                    .iter()
                    .flat_map(|generator| match generator.as_str() {
                        "ifml_skeleton" | "ifml_route" | "ifml_navigation" | "ifml_e2e_test" => {
                            ifml_frameworks
                                .iter()
                                .map(|fw| format!("{}_{}", generator, fw.name))
                                .collect::<Vec<_>>()
                        }
                        _ => vec![generator.clone()],
                    })
                    .collect();

                (
                    name.clone(),
                    ResolvedSection {
                        generators,
                        ..section.clone()
                    },
                )
            })
            .collect()
    }

    /// Returns true if the named entity generator is in the plan.
    pub fn has_entity_gen(&self, name: &str) -> bool {
        self.entity_generators.iter().any(|g| g == name)
    }

    /// Returns true if the named domain generator is in the plan.
    pub fn has_domain_gen(&self, name: &str) -> bool {
        self.domain_generators.iter().any(|g| g == name)
    }

    /// Returns true if the named global generator is in the plan.
    pub fn has_global_gen(&self, name: &str) -> bool {
        self.global_generators.iter().any(|g| g == name)
    }

    /// Returns the database target dialect for this build plan.
    pub fn database_target(&self) -> DatabaseTarget {
        self.database_target
    }

    /// Returns the persistence provider for this build plan.
    pub fn persistence_provider(&self) -> PersistenceProvider {
        self.persistence_provider
    }

    /// Returns the deployment topology for this build plan.
    pub fn deployment_topology(&self) -> DeploymentTopology {
        self.deployment_topology
    }

    /// Returns the IFML framework targets configured for this build plan.
    pub fn ifml_framework_targets(&self) -> Vec<&IfmlFrameworkTarget> {
        self.ifml_frameworks.iter().collect()
    }

    /// Returns the template override directories for template resolution.
    ///
    /// When the profile variant specifies a `template_pack`, the resolved
    /// directory is returned so generators can look there first for overrides.
    pub fn template_override_dirs(&self) -> Vec<&Path> {
        self.template_pack_path
            .iter()
            .map(|p| p.as_path())
            .collect()
    }
}
