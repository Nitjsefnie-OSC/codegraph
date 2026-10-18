use std::collections::HashMap;

use crate::error::{Error, Result};

use super::resolve::ResolvedProfile;

/// Generator kind — determines how the build planner invokes the generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratorKind {
    Entity,
    Domain,
    Global,
}

/// Which target (project output section) a generator belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GeneratorTarget {
    Api,
    Ui,
    Cli,
    Mobile,
    Common,
}

impl GeneratorTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            GeneratorTarget::Api => "api",
            GeneratorTarget::Ui => "ui",
            GeneratorTarget::Cli => "cli",
            GeneratorTarget::Mobile => "mobile",
            GeneratorTarget::Common => "common",
        }
    }
}

/// A capability descriptor for a single generator.
///
/// Each generator's `name()` return value is the canonical ID used in
/// `[profile.X.generators]` lists. The registry maps those names to
/// their kind, target, and feature requirements for validation.
#[derive(Debug, Clone)]
pub struct GeneratorCapability {
    pub name: String,
    pub kind: GeneratorKind,
    pub target: GeneratorTarget,
    pub features_required: Vec<String>,
    pub features_optional: Vec<String>,
}

/// The full registry of generator capabilities.
///
/// Indexed by generator name (matching the `name()` trait method).
/// Use [`capabilities`] to get the singleton registry.
pub struct CapabilityRegistry {
    pub generators: HashMap<String, GeneratorCapability>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self {
            generators: capabilities(),
        }
    }

    /// True when this generator is gated behind a backend dependency flag
    /// (`grpc_backend`, `atproto_backend`, `fern_sdk`) — i.e. its output
    /// requires dependencies the scaffolded Cargo.toml only enables when the
    /// corresponding profile feature is on. Such generators must not run
    /// without an explicit [`BuildPlan`]: a plan-less run cannot know whether
    /// the generated crate can compile their output (e.g. tonic references
    /// without the `grpc` feature).
    pub fn requires_build_plan(&self, name: &str) -> bool {
        self.generators.get(name).is_some_and(|c| {
            c.features_required.iter().any(|f| {
                matches!(
                    f.as_str(),
                    "grpc_backend"
                        | "atproto_backend"
                        | "fern_sdk"
                        | "emdash_plugins"
                        | "rls_from_policy"
                        | "public_operations_rls"
                        | "rosetta_backend"
                )
            })
        })
    }

    pub fn get(&self, name: &str) -> Option<&GeneratorCapability> {
        self.generators.get(name)
    }

    pub fn validate_profile(&self, profile: &ResolvedProfile) -> Result<()> {
        for (section_name, section) in &profile.sections {
            for gen_name in &section.generators {
                let cap = self.get(gen_name).ok_or_else(|| {
                    let known: Vec<_> = self.generators.keys().cloned().collect();
                    Error::Config(format!(
                        "generator \"{gen_name}\" in [{}] section not found in capability registry. \
                         known generators: {known:?}",
                        section_name
                    ))
                })?;

                // Validate that the generator's target matches the section.
                // Common generators can appear in any section.
                let section_target = match section_name.as_str() {
                    "api" => GeneratorTarget::Api,
                    "ui" => GeneratorTarget::Ui,
                    "emdash" => GeneratorTarget::Ui,
                    "cli" => GeneratorTarget::Cli,
                    "mobile" => GeneratorTarget::Mobile,
                    _ => GeneratorTarget::Common,
                };

                if cap.target != GeneratorTarget::Common && cap.target != section_target {
                    return Err(Error::Config(format!(
                        "generator \"{gen_name}\" (target={}) cannot be used in [{}] section",
                        cap.target.as_str(),
                        section_name
                    )));
                }

                // Validate feature requirements.
                for req in &cap.features_required {
                    let val = profile.features.get(req);
                    let enabled = val.and_then(|v| v.as_bool()).unwrap_or(false);
                    if !enabled {
                        return Err(Error::Config(format!(
                            "generator \"{gen_name}\" requires feature \"{req}\" but it is \
                             not enabled in the profile features"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

impl Default for CapabilityRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the registry of all known generator capabilities.
///
/// This is the single source of truth for which generators exist and what
/// they require. When adding a new generator, add its entry here.
fn capabilities() -> HashMap<String, GeneratorCapability> {
    let mut map = base_capabilities();
    // Merge IFML generator capabilities
    for cap in crate::ifml::profiles::ifml_capabilities() {
        map.insert(cap.name.clone(), cap);
    }
    map
}

/// Base capabilities without IFML generators.
fn base_capabilities() -> HashMap<String, GeneratorCapability> {
    use GeneratorKind::{Domain, Entity, Global};
    use GeneratorTarget::*;

    #[rustfmt::skip]
    let entries = vec![
        // ── Entity generators ──────────────────────────────────────────
        cap("ddl",                  Entity, Api,  &[], &[]),
        cap("sea_orm_entity",       Entity, Api,  &[], &[]),
        cap("cornucopia_queries",   Entity, Api,  &[], &[]),
        cap("cornucopia_config",    Global, Common, &[], &[]),
        cap("cornucopia_repo",      Entity, Api,  &[], &[]),
        cap("codelist",             Entity, Api,  &[], &[]),
        cap("dto",                  Entity, Api,  &[], &[]),
        cap("repository",           Entity, Api,  &[], &[]),
        cap("command",              Entity, Api,  &[], &[]),
        cap("query",                Entity, Api,  &[], &[]),
        cap("event",                Entity, Api,  &[], &[]),
        cap("handler",              Entity, Api,  &[], &[]),
        cap("workflow_action",      Entity, Api,  &[], &[]),
        cap("media_route",          Entity, Api,  &[], &[]),
        cap("test",                 Entity, Api,  &[], &[]),
        cap("lifecycle_trait",      Entity, Common,  &[], &[]),
        cap("domain_types_dto",     Entity, Api,  &[], &[]),
        cap("domain_types_query_service", Entity, Api, &[], &[]),

        cap("ui_page",              Entity, Ui,   &[], &[]),
        cap("ui_form",              Entity, Ui,   &[], &[]),
        cap("cosmos_entity_form",   Entity, Ui,   &[], &[]),
        cap("ui_store",             Entity, Ui,   &[], &[]),
        cap("ui_e2e_test",          Entity, Ui,   &[], &[]),
        cap("playwright-entity",    Entity, Ui,   &[], &[]),
        cap("ui_descriptor",        Entity, Ui,   &[], &[]),
        cap("ui-shell",             Entity, Ui,   &[], &[]),

        cap("cli_command",          Entity, Cli,  &[], &[]),

        // ── Domain generators ──────────────────────────────────────────
        cap("errors",               Domain, Api,  &[], &[]),
        cap("router",               Domain, Api,  &[], &[]),
        cap("api_contract",         Domain, Api,  &[], &[]),
        cap("links",                Domain, Api,  &[], &[]),
        // hurl contract tests (issue #463): per-entity hurl API contracts +
        // the generated authn/authz suite the ops api suite runs.
        cap("hurl_contract",        Domain, Api,  &[], &[]),
        cap("ui-domain-layout",     Domain, Ui,   &[], &[]),
        cap("cli_domain",           Domain, Cli,  &[], &[]),

        // ── Global generators ──────────────────────────────────────────
        cap("basejump_setup",       Global, Common, &[], &[]),
        cap("pgmq_setup",           Global, Common, &[], &[]),
        cap("label_setup",          Global, Common, &["has_labels"], &[]),
        cap("service_tables",        Global, Common, &["atproto_backend"], &[]),
        cap("platform_schema",      Global, Common, &[], &[]),
        cap("platform_grants",      Global, Common, &[], &[]),
        cap("workflow_seed",        Global, Common, &[], &[]),
        cap("openapi",              Global, Common, &[], &[]),
        cap("api_contract_index",   Global, Common, &[], &[]),
        cap("scaffold",             Global, Common, &[], &[]),
        cap("worker_scaffold",      Global, Common, &[], &[]),
        cap("ui_scaffold",          Global, Ui,    &[], &[]),
        cap("ui_types",             Global, Ui,    &[], &[]),
        cap("ui_codelist",          Global, Ui,    &[], &[]),
        cap("ui_orgchart",          Global, Ui,    &[], &[]),
        cap("ui_testids",           Global, Ui,    &[], &[]),
        cap("hook_registry",        Global, Common, &[], &[]),
        cap("domain_types_scaffold", Global, Common, &[], &[]),
        cap("report_views",         Global, Common, &[], &[]),
        cap("cli_scaffold",         Global, Cli,   &[], &[]),
        cap("playwright-global",    Global, Ui,    &[], &[]),
        cap("integration_tables",   Global, Common, &[], &[]),
        cap("integration_config",   Global, Common, &[], &[]),
        cap("integration_dispatch", Global, Common, &[], &[]),
        cap("integration_catalog",  Global, Common, &[], &[]),
        cap("webhook_dispatch",     Global, Common, &[], &[]),
        cap("webhook_endpoint_api", Global, Common, &[], &[]),
        cap("evt_events",           Global, Common, &[], &[]),
        cap("seed_provision",       Global, Common, &[], &[]),

        // ── ops harness generators ──────────────────────────────────────
        cap("ops",                  Global, Common, &["ops_backend"], &[]),

        // ── gRPC generators ────────────────────────────────────────────
        cap("grpc_proto",           Entity,  Api, &["grpc_backend"], &[]),
        cap("grpc_service",         Entity,  Api, &["grpc_backend"], &[]),
        cap("grpc_router",          Domain,  Api, &["grpc_backend"], &[]),
        cap("grpc_scaffold",        Global,  Api, &["grpc_backend"], &[]),

        // ── policy-driven RLS (issue #219) ─────────────────────────────
        cap("policy_rls",           Global,  Common, &["rls_from_policy"], &[]),

        // ── public-operations RLS + route gating (issue #279) ──────────
        cap("public_operations_rls", Global, Common, &["public_operations_rls"], &[]),

        // ── constraint-plane validations (issue #261) ───────────────────
        cap("condition_validations", Domain, Api,  &["rosetta_backend"], &[]),

        // ── regulatory report scaffolding (issue #265) ──────────────────
        cap("regulatory_reports",  Domain, Api,  &["rosetta_backend"], &[]),

        // ── rosetta function codegen (issue #263) ───────────────────────
        cap("functions",           Domain, Api,  &["rosetta_backend"], &[]),

        // ── rosetta rule codegen (issue #264) ───────────────────────────
        cap("rules",               Domain, Api,  &["rosetta_backend"], &[]),

        // ── Fern SDK generators ─────────────────────────────────────────
        cap("fern_config",          Global, Api,   &["fern_sdk"], &[]),

        // ── EmDash plugin generators ────────────────────────────────────
        cap("emdash_plugin",          Domain, Ui,  &["emdash_plugins"], &[]),
        cap("emdash_plugin_scaffold", Global, Ui,  &["emdash_plugins"], &[]),

        // ── AT Protocol generators ─────────────────────────────────────
        cap("lexicon",              Entity, Common, &["atproto_backend"], &[]),
        cap("lexicon_scaffold",     Global, Common, &["atproto_backend"], &[]),
        cap("atproto_types",        Entity, Common, &["atproto_backend"], &[]),
        cap("atproto_client",       Entity, Common, &["atproto_backend"], &[]),
        cap("atproto_client_scaffold", Global, Common, &["atproto_backend"], &[]),
        cap("atproto_appview",      Domain, Common, &["atproto_backend"], &[]),
        cap("atproto_xrpc",         Entity, Common, &["atproto_backend"], &[]),
        cap("atproto_xrpc_router",  Domain, Common, &["atproto_backend"], &[]),
        cap("atproto_xrpc_merge",   Global, Common, &["atproto_backend"], &[]),
        cap("atproto_identity",     Global, Common, &["atproto_backend"], &[]),
    ];

    entries.into_iter().map(|c| (c.name.clone(), c)).collect()
}

fn cap(
    name: &str,
    kind: GeneratorKind,
    target: GeneratorTarget,
    required: &[&str],
    optional: &[&str],
) -> GeneratorCapability {
    GeneratorCapability {
        name: name.to_string(),
        kind,
        target,
        features_required: required.iter().map(|s| s.to_string()).collect(),
        features_optional: optional.iter().map(|s| s.to_string()).collect(),
    }
}
