use std::path::Path;

use codegraph_config::{DomainConfig, UiDomainConfig, UiOverrideConfig};
use codegraph_core::traits::GraphQuerier;
use tera::Tera;

use crate::db::dialect::DatabaseTarget;
use crate::emdash;
use crate::profile::{DependencyStrategy, DeploymentTopology, PersistenceProvider};

// =============================================================================
// Project-level configuration for template rendering.
// Threaded explicitly: `run_generators_with_opts` receives a
// `ProjectConfig` (from `GeneratorOpts.project_config` or the default)
// and passes it to every generator and helper that needs it.
//
// The config is composed of cohesive sub-configs. Every sub-config field is
// `#[serde(flatten)]`-ed into the parent so the serialized `project` map the
// Tera templates see stays key-for-key FLAT (`{{ project.app_name }}`, …) —
// templates must never be edited for this structure.
// =============================================================================

/// DTO serde key casing for domain-types DTOs.
///
/// `Snake` (default) keeps keys at the Rust field names; `Camel` emits
/// `#[serde(rename_all = "camelCase")]` on create/update/response DTOs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DtoKeyCasing {
    #[default]
    Snake,
    Camel,
}

impl DtoKeyCasing {
    /// Parse from the normalized `BuildPlan.dto_key_casing` string.
    /// Unknown values fall back to `Snake` — worst case the wire keys stay
    /// at the Rust field names (the historical contract).
    pub fn from_config(s: &str) -> Self {
        match s {
            "camel" => Self::Camel,
            _ => Self::Snake,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Snake => "snake",
            Self::Camel => "camel",
        }
    }
}

/// What the generated app is called: crate names, API title, generator tag.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IdentityConfig {
    pub app_name: String,
    /// Rust crate name of the generated library (the `[lib] name` in the
    /// generated Cargo.toml). Test templates reference the crate by this name.
    #[serde(default)]
    pub lib_name: String,
    pub domain_types_crate: String,
    pub hooks_api_crate: String,
    pub api_title: String,
    pub generator_name: String,
    /// API version prefix used in URL path construction (e.g. "v1" → `/api/v1/...`).
    #[serde(default)]
    pub api_version: String,
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            app_name: "app".into(),
            lib_name: "cosmos".into(),
            domain_types_crate: "domain_types".into(),
            hooks_api_crate: String::new(),
            api_title: "HR Open API".into(),
            generator_name: "codegraph".into(),
            api_version: "v1".into(),
        }
    }
}

/// Repo-relative target directories for the generated companion crates.
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct PathsConfig {
    /// Path to the domain-types crate root (e.g. "crates/placekit-domain-types").
    /// Used by the scaffold generator to add a path dependency in Cargo.toml.
    /// Empty string means "no separate domain-types crate" (types live in the app).
    pub domain_types_base: String,
    pub hooks_api_base: String,
    pub extensions_base: String,
    pub app_config_base: String,
    pub decision_engine_base: String,
    pub codegraph_workflow_base: String,
    pub type_contracts_base: String,
}

/// Database generation targets: dialect and persistence provider.
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct DatabaseConfig {
    /// Database target dialect for SQL generation. Used by DB templates to
    /// branch on dialect-specific syntax (serializes as "postgres"/"sqlite").
    pub database_target: DatabaseTarget,
    /// Persistence provider for entity/repository code generation. Used by
    /// templates to select provider-specific rendering paths (serializes as
    /// "sea_orm"/"cornucopia").
    pub persistence_provider: PersistenceProvider,
}

/// Deployment shape of the generated application.
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct DeploymentConfig {
    /// Deployment topology for the generated application. Used by templates
    /// to select topology-specific rendering paths (serializes as
    /// "monolith"/"workers").
    pub deployment_topology: DeploymentTopology,
}

/// Code-emission toggles that are not tied to one generator.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodegenConfig {
    /// DTO serde key casing for domain-types DTOs (serializes as
    /// "snake"/"camel").
    pub dto_key_casing: DtoKeyCasing,
    /// Namespace-aware module layout (issue #268): schemas carrying a
    /// namespace emit under namespace-derived module paths
    /// (`cdm.base.datetime` → `cdm/base/datetime/...`). Default false =
    /// flat domain layout, byte-identical output.
    #[serde(default)]
    pub namespace_layout: bool,
    /// Canonical expression IR (issue #278): IFML guards render from the
    /// persisted `expr_json` AST via the TypeScript lowering. Default false
    /// = byte-identical output.
    #[serde(default)]
    pub expr_ir: bool,
    /// IFML test-side auth surface (issue #463): when true the IFML e2e
    /// generator emits `tests/e2e/auth.setup.ts` (per-persona API-key
    /// provisioning), the persona fixtures, `{view}.auth.spec.ts`, journey
    /// specs, and the extended POM surface. Default false = byte-identical.
    #[serde(default)]
    pub ifml_e2e_auth: bool,
    /// One testid vocabulary (issue #468 W3): when true the `ui_testids`
    /// global generator emits `ui/src/lib/testids.ts` and the generated
    /// POM/spec templates import it, binding the test surface to the same
    /// `ux::plan::ids` table the page markup renders from. Default false =
    /// byte-identical (POM/spec templates keep their literals).
    #[serde(default)]
    pub testid_module: bool,
    /// Post-generation structural gates (issue #468): when true the run
    /// scans the emitted tree and hard-fails on retired vocabulary
    /// (`-confirm-confirm`, raw POM compositions under `testid_module`).
    /// Default false = zero behavior.
    #[serde(default)]
    pub structural_gates: bool,
    /// Import prefix for structured wrapper types in generated re-exports.
    /// Default: "codegraph_type_contracts".
    /// Domain crates should set this to their own crate or module path (e.g. "crate").
    pub types_import_prefix: String,
}

impl Default for CodegenConfig {
    fn default() -> Self {
        Self {
            dto_key_casing: DtoKeyCasing::default(),
            namespace_layout: false,
            expr_ir: false,
            ifml_e2e_auth: false,
            testid_module: false,
            structural_gates: false,
            types_import_prefix: "codegraph_type_contracts".into(),
        }
    }
}

/// Cargo manifest emission: rev pins, patch section, extra dependencies.
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct CargoConfig {
    /// Git revision SHA used for fallback path dependencies in generated Cargo.toml.
    /// When domain_types_base is empty, the domain types Cargo.toml uses this rev
    /// to reference codegraph-type-contracts as a git dependency.
    #[serde(default)]
    pub codegraph_rev: String,
    /// How the generated app's Cargo.toml references codegraph crates
    /// (issue #347; default `Rev` = the historical git+rev pins). `Path`
    /// switches the emitted `codegraph-*` deps to absolute path deps.
    #[serde(default)]
    pub dependency_strategy: DependencyStrategy,
    /// Absolute path of the codegraph checkout that ran generation (parent of
    /// the codegraph crate's compile-time `CARGO_MANIFEST_DIR`). Only read by
    /// templates when `dependency_strategy` is `Path` — the emitted path deps
    /// then carry BUILD-MACHINE paths by design (local development loops).
    #[serde(default)]
    pub codegraph_path_root: String,
    /// Raw `[patch.'https://github.com/magick93/codegraph.git']` entries emitted
    /// into the generated Cargo.toml (dev environments pin the local codegraph
    /// checkout via path overrides). Empty string = no patch section.
    #[serde(default)]
    pub cargo_patch: String,
    /// Raw extra lines appended to the generated Cargo.toml [dependencies]
    /// section (e.g. `url = "2"` or local path deps).
    #[serde(default)]
    pub extra_dependencies: String,
    /// Whether the generated Cargo.toml declares a `[workspace]` with a `cli/`
    /// member (hand-maintained CLI crates that are not emitted by the
    /// `cli_scaffold` generator). Defaults to false.
    #[serde(default)]
    pub cargo_workspace: bool,
}

/// Integration feature flags and their per-integration settings.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IntegrationFlags {
    /// Whether atproto generators are enabled via profile feature flag.
    pub has_atproto: bool,
    /// Whether Fern SDK generation is enabled via profile feature flag.
    #[serde(default)]
    pub has_fern: bool,
    /// Fern SDK languages to generate (e.g. ["typescript", "rust"]).
    #[serde(default)]
    pub fern_sdk_languages: Vec<String>,
    /// Whether EmDash plugin generation is enabled via profile feature flag.
    #[serde(default)]
    pub has_emdash: bool,
    /// Public-operations consumer (issue #279): schemas carrying
    /// `access = Public` whose entity config declares `public_operations`
    /// emit `TO PUBLIC` RLS policies and mount routes without permission
    /// layers. Default false = byte-identical output.
    #[serde(default)]
    pub public_operations_rls: bool,
    /// Whether function post-conditions emit `debug_assert!` checks
    /// (issue #263). Gated by the `function_postconditions` profile
    /// feature; default OFF = the emitted functions module carries none.
    #[serde(default)]
    pub has_function_postconditions: bool,
    /// Repo-relative base path for the community site's public pages
    /// (emdash plugin generator). Default: "apps/community-site/src/pages".
    #[serde(default = "default_emdash_site_pages_base")]
    pub emdash_site_pages_base: String,
    /// Repo-relative base path for the community site's e2e suite
    /// (emdash plugin generator). Default: "apps/community-site/e2e".
    #[serde(default = "default_emdash_site_e2e_base")]
    pub emdash_site_e2e_base: String,
}

impl Default for IntegrationFlags {
    fn default() -> Self {
        Self {
            has_atproto: false,
            has_fern: false,
            fern_sdk_languages: vec!["typescript".into()],
            has_emdash: false,
            public_operations_rls: false,
            has_function_postconditions: false,
            emdash_site_pages_base: default_emdash_site_pages_base(),
            emdash_site_e2e_base: default_emdash_site_e2e_base(),
        }
    }
}

/// AT Protocol integration settings.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AtprotoConfig {
    /// AT Protocol namespace authority (e.g. "nz.gravy").
    /// Read from domain config or hard-coded default. Empty string = atproto disabled.
    pub atproto_authority: String,
    /// AT Protocol tenancy mode: "shared_pds" or "per_org_pds" (raw passthrough
    /// from profile features — not a closed set at construction).
    pub atproto_tenancy: String,
    /// AT Protocol float policy: what to do with JSON Schema "number" types.
    /// "reject", "string", "integer_scaled", or "unknown" (raw passthrough
    /// from profile features — not a closed set at construction).
    pub atproto_float_policy: String,
}

impl Default for AtprotoConfig {
    fn default() -> Self {
        Self {
            atproto_authority: String::new(),
            atproto_tenancy: "shared_pds".to_string(),
            atproto_float_policy: "integer_scaled".to_string(),
        }
    }
}

/// UX rules integration (issue #293).
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct UxConfig {
    /// Resolved ux-rules: the built-in `ux-default` pack, optionally merged
    /// with a project `--ux-rules` file. `None` = flag off / plan-less run
    /// without a CLI file — templates see `project.ux` only when rules
    /// resolved, so unset keeps output byte-identical (the key is absent
    /// from the serialized map when `None`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ux: Option<codegraph_config::UxRules>,
}

/// Project-level configuration injected into every template context as
/// `project`. Sub-configs are flattened on serialize so the template-visible
/// map keeps the historical flat keys.
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct ProjectConfig {
    #[serde(flatten)]
    pub identity: IdentityConfig,
    #[serde(flatten)]
    pub paths: PathsConfig,
    #[serde(flatten)]
    pub database: DatabaseConfig,
    #[serde(flatten)]
    pub deployment: DeploymentConfig,
    #[serde(flatten)]
    pub codegen: CodegenConfig,
    #[serde(flatten)]
    pub cargo: CargoConfig,
    #[serde(flatten)]
    pub integration: IntegrationFlags,
    #[serde(flatten)]
    pub atproto: AtprotoConfig,
    #[serde(flatten)]
    pub ux: UxConfig,
}

fn default_emdash_site_pages_base() -> String {
    emdash::DEFAULT_SITE_PAGES_BASE.to_string()
}

fn default_emdash_site_e2e_base() -> String {
    emdash::DEFAULT_SITE_E2E_BASE.to_string()
}

impl ProjectConfig {
    /// True when entity/repository code generation targets the Cornucopia backend.
    pub fn is_cornucopia(&self) -> bool {
        matches!(
            self.database.persistence_provider,
            PersistenceProvider::Cornucopia
        )
    }

    /// True when the generated app is split into per-domain Cloudflare Workers.
    pub fn is_workers_topology(&self) -> bool {
        matches!(
            self.deployment.deployment_topology,
            DeploymentTopology::Workers
        )
    }
}

/// Namespace-derived module directory for a schema under the
/// `namespace_layout` gate (issue #268).
///
/// Returns `Some(("cdm/base/datetime", "cdm::base::datetime"))` when the
/// gate is ON and the schema carries a namespace, `None` otherwise (gate
/// OFF or namespace-less schema) — callers fall back to the flat
/// domain-derived paths, keeping default output byte-identical.
pub fn namespace_module_dir(
    schema: &codegraph_core::types::SchemaNode,
    project: &ProjectConfig,
) -> Option<(String, String)> {
    if !project.codegen.namespace_layout {
        return None;
    }
    let ns = schema.namespace.as_deref()?.trim();
    if ns.is_empty() {
        return None;
    }
    Some((
        codegraph_core::types::namespace_module_path(ns),
        codegraph_core::types::namespace_module_rust(ns),
    ))
}

/// The namespace-derived directory segment for a raw namespace FQN under
/// the `namespace_layout` gate (`Some("cdm/base/datetime")`), `None` when
/// flat (gate off / namespace-less). Path-form counterpart of
/// [`namespace_rust_prefix`].
pub fn namespace_dir_prefix(ns: Option<&str>, project: &ProjectConfig) -> Option<String> {
    if !project.codegen.namespace_layout {
        return None;
    }
    let ns = ns?.trim();
    (!ns.is_empty()).then(|| codegraph_core::types::namespace_module_path(ns))
}

/// The namespace-derived Rust module path prefix for a raw namespace FQN
/// under the `namespace_layout` gate (`Some("cdm::base::datetime")`),
/// `None` when flat.
pub fn namespace_rust_prefix(ns: Option<&str>, project: &ProjectConfig) -> Option<String> {
    if !project.codegen.namespace_layout {
        return None;
    }
    let ns = ns?.trim();
    (!ns.is_empty()).then(|| codegraph_core::types::namespace_module_rust(ns))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The serialized shape of `ProjectConfig` is FROZEN: templates read the
    /// `project` map FLAT (`{{ project.database_target }}`, …), so the
    /// `#[serde(flatten)]` sub-configs must keep producing exactly the
    /// historical key set with the historical values. `ux` is absent when
    /// `None` (`skip_serializing_if`), matching the pre-composition shape.
    #[test]
    fn serialized_shape_stays_flat_and_unchanged() {
        let json = serde_json::to_value(ProjectConfig::default()).unwrap();
        let obj = json.as_object().unwrap();
        let expected: serde_json::Map<String, serde_json::Value> = [
            ("app_name", serde_json::json!("app")),
            ("lib_name", serde_json::json!("cosmos")),
            ("domain_types_crate", serde_json::json!("domain_types")),
            ("hooks_api_crate", serde_json::json!("")),
            ("api_title", serde_json::json!("HR Open API")),
            ("generator_name", serde_json::json!("codegraph")),
            ("api_version", serde_json::json!("v1")),
            ("domain_types_base", serde_json::json!("")),
            ("hooks_api_base", serde_json::json!("")),
            ("extensions_base", serde_json::json!("")),
            ("app_config_base", serde_json::json!("")),
            ("decision_engine_base", serde_json::json!("")),
            ("codegraph_workflow_base", serde_json::json!("")),
            ("type_contracts_base", serde_json::json!("")),
            ("database_target", serde_json::json!("postgres")),
            ("persistence_provider", serde_json::json!("sea_orm")),
            ("dto_key_casing", serde_json::json!("snake")),
            ("deployment_topology", serde_json::json!("monolith")),
            ("namespace_layout", serde_json::json!(false)),
            ("expr_ir", serde_json::json!(false)),
            ("ifml_e2e_auth", serde_json::json!(false)),
            ("testid_module", serde_json::json!(false)),
            ("structural_gates", serde_json::json!(false)),
            ("public_operations_rls", serde_json::json!(false)),
            (
                "types_import_prefix",
                serde_json::json!("codegraph_type_contracts"),
            ),
            ("codegraph_rev", serde_json::json!("")),
            ("dependency_strategy", serde_json::json!("rev")),
            ("codegraph_path_root", serde_json::json!("")),
            ("has_atproto", serde_json::json!(false)),
            ("has_fern", serde_json::json!(false)),
            ("fern_sdk_languages", serde_json::json!(["typescript"])),
            ("has_emdash", serde_json::json!(false)),
            ("has_function_postconditions", serde_json::json!(false)),
            (
                "emdash_site_pages_base",
                serde_json::json!("apps/community-site/src/pages"),
            ),
            (
                "emdash_site_e2e_base",
                serde_json::json!("apps/community-site/e2e"),
            ),
            ("atproto_authority", serde_json::json!("")),
            ("atproto_tenancy", serde_json::json!("shared_pds")),
            ("atproto_float_policy", serde_json::json!("integer_scaled")),
            ("cargo_patch", serde_json::json!("")),
            ("extra_dependencies", serde_json::json!("")),
            ("cargo_workspace", serde_json::json!(false)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        assert_eq!(*obj, expected);
        assert!(
            !obj.contains_key("ux"),
            "`ux` must stay absent from the default serialized map (skip_serializing_if)"
        );
    }
}

/// An entity in the generation order with its graph schema_id and domain.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GenerationEntry {
    pub schema_title: String,
    pub domain: String,
    pub pg_schema: String,
    pub is_cyclic: bool,
}

/// Configuration for the code generation pipeline.
///
/// Groups all the various config inputs so generator entry points
/// don't need 10+ positional arguments.
pub struct GeneratorOpts<'a> {
    pub db: &'a dyn GraphQuerier,
    pub config: &'a DomainConfig,
    pub output_dir: &'a Path,
    pub tera: &'a Tera,
    pub ui_overrides: &'a UiOverrideConfig,
    pub ui_domains: &'a UiDomainConfig,
    /// Root of the HR Open Standards schema tree.
    /// Pass an empty path for tests that don't need real codelist files.
    pub schema_base_dir: &'a Path,
    /// Path to the optional seed.toml config file for demo seed data.
    /// If `None` or the file doesn't exist, the generator falls back to
    /// hardcoded HR-specific demo data.
    pub seed_config: Option<&'a Path>,
    /// Override target dir for domain-types crate generators.
    /// `None` defaults to the main output directory.
    pub domain_types_base: Option<&'a Path>,
    /// Override target dir for hooks generators.
    pub hooks_base: Option<&'a Path>,
    /// Extension points config for integration infrastructure generators.
    pub ext_points: Option<&'a codegraph_ext_points::ExtensionPointsConfig>,
    /// Build profile plan controlling which generators to run.
    pub build_plan: Option<&'a crate::profile::BuildPlan>,
    /// IFML framework targets (e.g. "svelte", "react").
    /// If empty, defaults to `["svelte"]` at dispatch.
    pub ifml_frameworks: Vec<String>,
    /// Optional IFML component mappings (`ifml-components.toml`). `None` or
    /// empty renders all components with the built-in templates.
    pub ifml_components: Option<&'a codegraph_config::IfmlComponentMappings>,
    /// Resolved ux-rules (issue #293): the built-in `ux-default` pack,
    /// optionally merged with a project `--ux-rules` file. `None` = flag
    /// off — generators keep their pre-#293 rendering byte-identical.
    pub ux_rules: Option<codegraph_config::UxRules>,
    /// Project-level config injected into all template contexts.
    pub project_config: Option<&'a ProjectConfig>,
    /// EmDash plugin packages config (plugins.toml), loaded by the CLI
    /// wrapper when the profile enables the `emdash_plugins` feature.
    /// `None` (or an empty map) disables the emdash generators.
    pub emdash_plugins: Option<crate::emdash::EmdashPluginsConfig>,
    /// Directory of the `domains.toml` config used for this run. Optional
    /// sibling configs (`reports.toml`) are discovered relative to this
    /// directory instead of the process current directory, so server
    /// composition no longer depends on the invoking shell's cwd.
    /// `None` falls back to the current directory (legacy behavior).
    pub domain_config_dir: Option<&'a Path>,
}
