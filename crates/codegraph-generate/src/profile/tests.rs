#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_profile() {
        let toml = r#"
[profiles.default.meta]
name = "default"
version = "1.0.0"
description = "Default profile"

[profiles.default.api]
generators = ["api_server", "db_migrations"]
scripts.post_gen = ["cargo check"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.profiles.len(), 1);

        let def = &config.profiles["default"];
        assert_eq!(def.meta.as_ref().unwrap().name, "default");
        assert_eq!(def.sections.len(), 1);
        assert!(def.sections.contains_key("api"));

        let api = &def.sections["api"];
        assert_eq!(api.generators, vec!["api_server", "db_migrations"]);
        assert_eq!(api.scripts.as_ref().unwrap().post_gen, vec!["cargo check"]);
    }

    #[test]
    fn parse_profile_with_ui_and_cli_sections() {
        let toml = r#"
[profiles.fullstack.meta]
name = "fullstack"
version = "1.0.0"
description = "Full stack profile"
tags = ["web"]

[profiles.fullstack.features]
auth = true
pagination = true
validation_level = "strict"

[profiles.fullstack.api]
generators = ["api_server", "db_migrations"]
output = "/tmp/api-out/"

[profiles.fullstack.ui]
generators = ["ui_routes", "ui_forms"]
scripts.post_gen = ["pnpm run check", "pnpm run build"]

[profiles.fullstack.cli]
generators = ["cli_commands"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["fullstack"];

        assert_eq!(def.sections.len(), 3);
        assert_eq!(def.sections["api"].output, Some("/tmp/api-out/".into()));

        let ui = &def.sections["ui"];
        assert_eq!(ui.generators, vec!["ui_routes", "ui_forms"]);
        assert_eq!(
            ui.scripts.as_ref().unwrap().post_gen,
            vec!["pnpm run check", "pnpm run build"]
        );

        let cli = &def.sections["cli"];
        assert!(cli.scripts.is_none());
        assert!(cli.output.is_none());
    }

    #[test]
    fn parse_profile_with_variants() {
        let toml = r#"
[profiles.api.meta]
name = "api"
version = "1.0.0"
description = "API profile"

[profiles.api.features]
auth = true
validation_level = "strict"

[profiles.api.api]
generators = ["api_server", "db_migrations"]
scripts.post_gen = ["cargo check"]

[profiles.api.variants.lite]
[profiles.api.variants.lite.api]
generators = ["api_server"]
[profiles.api.variants.lite.features]
auth = false
validation_level = "balanced"
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["api"];
        assert!(def.variants.is_some());
        assert!(def.variants.as_ref().unwrap().contains_key("lite"));
    }

    #[test]
    fn resolve_profile_without_variant() {
        let toml = r#"
[profiles.test.meta]
name = "test"
version = "1.0.0"
description = "Test profile"

[profiles.test.features]
auth = true

[profiles.test.api]
generators = ["api_server", "db_migrations"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["test"];

        let resolved = resolve_profile(def, None).unwrap();
        assert_eq!(resolved.meta.name, "test");
        assert_eq!(resolved.features.get("auth").unwrap().as_bool(), Some(true));
        assert_eq!(resolved.sections.len(), 1);
        assert_eq!(
            resolved.sections["api"].generators,
            vec!["api_server", "db_migrations"]
        );
        assert!(resolved.sections["api"].scripts.is_empty());
    }

    #[test]
    fn resolve_profile_with_variant_merges_features() {
        let toml = r#"
[profiles.api.meta]
name = "api"
version = "1.0.0"
description = "API profile"

[profiles.api.features]
auth = true
validation_level = "strict"

[profiles.api.api]
generators = ["api_server", "db_migrations"]
scripts.post_gen = ["cargo check"]

[profiles.api.variants.lite]
[profiles.api.variants.lite.api]
generators = ["api_server"]
[profiles.api.variants.lite.features]
auth = false
validation_level = "balanced"
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["api"];

        let resolved = resolve_profile(def, Some("lite")).unwrap();
        assert_eq!(resolved.sections.len(), 1);
        // Variant overrides the generators
        assert_eq!(resolved.sections["api"].generators, vec!["api_server"]);
        // Variant overrides features
        assert_eq!(
            resolved.features.get("auth").unwrap().as_bool(),
            Some(false)
        );
        assert_eq!(
            resolved.features.get("validation_level").unwrap().as_str(),
            Some("balanced")
        );
        // Variant didn't provide scripts → inherit from base if present... actually
        // in the current implementation, variant sections fully replace base sections.
        // post_gen scripts from the base are NOT inherited when a variant overrides
        // the section. This is by design: the variant explicitly lists what it wants.
        // Since the variant section doesn't have scripts, this section has none.
        assert!(resolved.sections["api"].scripts.is_empty());
    }

    #[test]
    fn resolve_unknown_profile_yields_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.toml");

        std::fs::write(
            &path,
            r#"
[profiles.known.meta]
name = "known"
version = "1.0.0"
description = "Known"

[profiles.known.api]
generators = ["foo"]
"#,
        )
        .unwrap();

        let result = load_and_resolve_profile(&path, "unknown", None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found"),
            "expected profile-not-found error, got: {err}"
        );
    }

    #[test]
    fn resolve_unknown_variant_yields_error() {
        let toml = r#"
[profiles.p.meta]
name = "p"
version = "1.0.0"
description = ".."

[profiles.p.api]
generators = ["x"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["p"];

        let result = resolve_profile(def, Some("nonexistent"));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("variant \"nonexistent\" not found"));
    }

    #[test]
    fn parse_profile_no_meta_no_features() {
        let toml = r#"
[profiles.minimal.api]
generators = ["gen1", "gen2"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["minimal"];
        assert!(def.meta.is_none());
        assert!(def.features.is_none());
        assert_eq!(def.sections.len(), 1);

        let resolved = resolve_profile(def, None).unwrap();
        assert_eq!(resolved.meta.name, ""); // default
        assert!(resolved.features.is_empty());
    }

    #[test]
    fn round_trip_full_default_profile() {
        let toml = r#"
[profiles.default.meta]
name = "default"
version = "1.0.0"
description = "Current all-artifacts behavior"
authors = ["hr-graph team"]
since = "2026-05-08"

[profiles.default.features]
auth = true
pagination = true
validation_level = "strict"
offline_mode = false

[profiles.default.api]
generators = ["api_server", "api_openapi", "db_migrations"]
output = "review/generated-candidate/"
scripts.post_gen = ["cargo check --workspace 2>&1"]

[profiles.default.ui]
generators = ["ui_routes", "ui_forms", "ui_api_client", "ui_stores"]
output = "review/generated-candidate/"
scripts.post_gen = ["pnpm run check", "pnpm run lint"]

[profiles.default.cli]
generators = ["cli_commands"]
output = "review/generated-candidate/"
scripts.post_gen = ["cargo check -p crewbase-cli 2>&1"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["default"];
        assert_eq!(def.sections.len(), 3);
        assert_eq!(
            def.sections["api"].generators,
            vec!["api_server", "api_openapi", "db_migrations"]
        );
        assert_eq!(
            def.sections["ui"].generators,
            vec!["ui_routes", "ui_forms", "ui_api_client", "ui_stores"]
        );
        assert_eq!(def.sections["cli"].generators, vec!["cli_commands"]);
    }

    #[test]
    fn parse_profiles_file_from_disk_smoke_test() {
        // This test uses a tempfile to verify the full file→parse→resolve path.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.toml");

        std::fs::write(
            &path,
            r#"
[profiles.smoke.meta]
name = "smoke"
version = "1.0.0"
description = "Smoke test profile"

[profiles.smoke.api]
generators = ["api_server"]
scripts.post_gen = ["cargo check"]
"#,
        )
        .unwrap();

        let resolved = load_and_resolve_profile(&path, "smoke", None).unwrap();
        assert_eq!(resolved.meta.name, "smoke");
        assert_eq!(resolved.sections["api"].generators, vec!["api_server"]);
    }

    #[test]
    fn dto_key_casing_defaults_to_snake() {
        let registry = CapabilityRegistry::new();
        let toml = r#"
[profiles.default.meta]
name = "default"
version = "1.0.0"
description = "Default"

[profiles.default.api]
generators = ["ddl", "dto", "handler"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let resolved = resolve_profile(&config.profiles["default"], None).unwrap();
        let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();
        assert_eq!(plan.dto_key_casing, "snake");
    }

    #[test]
    fn dto_key_casing_camel_feature() {
        let registry = CapabilityRegistry::new();
        let toml = r#"
[profiles.default.meta]
name = "default"
version = "1.0.0"
description = "Default"

[profiles.default.features]
dto_key_casing = "camel"

[profiles.default.api]
generators = ["ddl", "dto", "handler"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let resolved = resolve_profile(&config.profiles["default"], None).unwrap();
        let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();
        assert_eq!(plan.dto_key_casing, "camel");
    }

    #[test]
    fn dto_key_casing_unknown_value_falls_back_to_snake() {
        let registry = CapabilityRegistry::new();
        let toml = r#"
[profiles.default.meta]
name = "default"
version = "1.0.0"
description = "Default"

[profiles.default.features]
dto_key_casing = "pascal"

[profiles.default.api]
generators = ["ddl", "dto", "handler"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let resolved = resolve_profile(&config.profiles["default"], None).unwrap();
        let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();
        assert_eq!(plan.dto_key_casing, "snake");
    }

    #[test]
    fn parse_profiles_file_not_found_is_error() {
        let result = load_and_resolve_profile(Path::new("/nonexistent/path.toml"), "x", None);
        assert!(result.is_err());
    }

    // ── Capability Registry Tests ─────────────────────────────────────

    #[test]
    fn registry_has_all_known_generators() {
        let registry = CapabilityRegistry::new();
        // Spot-check some well-known generators
        assert!(registry.get("ddl").is_some());
        assert!(registry.get("dto").is_some());
        assert!(registry.get("handler").is_some());
        assert!(registry.get("ui_page").is_some());
        assert!(registry.get("openapi").is_some());
        assert!(registry.get("scaffold").is_some());
    }

    #[test]
    fn registry_unknown_generator_is_none() {
        let registry = CapabilityRegistry::new();
        assert!(registry.get("nonexistent_generator").is_none());
    }

    #[test]
    fn registry_validates_known_generators() {
        let registry = CapabilityRegistry::new();

        let toml = r#"
[profiles.valid.meta]
name = "valid"
version = "1.0.0"
description = "Valid"

[profiles.valid.api]
generators = ["ddl", "dto", "handler"]

[profiles.valid.ui]
generators = ["ui_page", "ui_form"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["valid"];
        let resolved = resolve_profile(def, None).unwrap();

        assert!(registry.validate_profile(&resolved).is_ok());
    }

    #[test]
    fn registry_rejects_unknown_generator() {
        let registry = CapabilityRegistry::new();

        let toml = r#"
[profiles.bad.meta]
name = "bad"
version = "1.0.0"
description = "Bad"

[profiles.bad.api]
generators = ["nonexistent_xyz"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["bad"];
        let resolved = resolve_profile(def, None).unwrap();

        let result = registry.validate_profile(&resolved);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("nonexistent_xyz"));
    }

    #[test]
    fn registry_rejects_wrong_target() {
        let registry = CapabilityRegistry::new();

        let toml = r#"
[profiles.wrong.meta]
name = "wrong"
version = "1.0.0"
description = "Wrong"

[profiles.wrong.ui]
generators = ["ddl"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["wrong"];
        let resolved = resolve_profile(def, None).unwrap();

        let result = registry.validate_profile(&resolved);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("[ui]"),
            "expected mention of [ui] section: {msg}"
        );
    }

    #[test]
    fn registry_allows_common_generators_in_any_section() {
        let registry = CapabilityRegistry::new();

        let toml = r#"
[profiles.c.meta]
name = "c"
version = "1.0.0"
description = ""

[profiles.c.api]
generators = ["openapi"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["c"];
        let resolved = resolve_profile(def, None).unwrap();

        // openapi has target=Common, so it should be valid in [api] section
        assert!(registry.validate_profile(&resolved).is_ok());
    }

    // ── Build Plan Tests ─────────────────────────────────────────────

    #[test]
    fn build_plan_from_default_profile() {
        let registry = CapabilityRegistry::new();

        let toml = r#"
[profiles.default.meta]
name = "default"
version = "1.0.0"
description = ""

[profiles.default.api]
generators = ["ddl", "dto", "handler", "openapi", "scaffold"]

[profiles.default.ui]
generators = ["ui_page", "ui_form", "ui_scaffold"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["default"];
        let resolved = resolve_profile(def, None).unwrap();

        let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();

        assert!(plan.has_entity_gen("ddl"));
        assert!(plan.has_entity_gen("dto"));
        assert!(plan.has_entity_gen("handler"));
        assert!(plan.has_entity_gen("ui_page"));
        assert!(plan.has_entity_gen("ui_form"));
        assert!(plan.has_global_gen("openapi"));
        assert!(plan.has_global_gen("scaffold"));
        assert!(plan.has_global_gen("ui_scaffold"));

        // Domain generators not listed → absent
        assert!(!plan.has_domain_gen("router"));
    }

    #[test]
    fn build_plan_includes_post_gen_scripts() {
        let registry = CapabilityRegistry::new();

        let toml = r#"
[profiles.ci.meta]
name = "ci"
version = "1.0.0"
description = ""

[profiles.ci.api]
generators = ["ddl"]
scripts.post_gen = ["cargo check"]

[profiles.ci.ui]
generators = ["ui_page"]
scripts.post_gen = ["pnpm run check", "pnpm run build"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["ci"];
        let resolved = resolve_profile(def, None).unwrap();

        let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();

        assert_eq!(plan.post_gen_scripts.len(), 2);
        // Iteration is deterministic (sorted by section name), so verify both sections.
        let api_scripts: Vec<String> = plan
            .post_gen_scripts
            .iter()
            .filter(|(s, _)| s == "api")
            .flat_map(|(_, scripts)| scripts.clone())
            .collect();
        assert_eq!(api_scripts, vec!["cargo check".to_string()]);

        let ui_scripts: Vec<String> = plan
            .post_gen_scripts
            .iter()
            .filter(|(s, _)| s == "ui")
            .flat_map(|(_, scripts)| scripts.clone())
            .collect();
        assert_eq!(
            ui_scripts,
            vec!["pnpm run check".to_string(), "pnpm run build".to_string()]
        );
    }

    #[test]
    fn build_plan_empty_sections_are_skipped() {
        let registry = CapabilityRegistry::new();

        let toml = r#"
[profiles.only_api.meta]
name = "only_api"
version = "1.0.0"
description = ""

# No [ui] section — it's not declared
[profiles.only_api.api]
generators = ["ddl"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["only_api"];
        let resolved = resolve_profile(def, None).unwrap();

        let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();

        assert!(plan.has_entity_gen("ddl"));
        assert!(!plan.has_global_gen("ui_scaffold"));
        assert_eq!(plan.post_gen_scripts.len(), 0);
    }

    // ── Deployment Topology Tests ─────────────────────────────────────

    #[test]
    fn deployment_topology_from_config_parses_known_values() {
        assert_eq!(
            DeploymentTopology::from_config("monolith").unwrap(),
            DeploymentTopology::Monolith
        );
        assert_eq!(
            DeploymentTopology::from_config("workers").unwrap(),
            DeploymentTopology::Workers
        );
        assert_eq!(DeploymentTopology::default(), DeploymentTopology::Monolith);
    }

    #[test]
    fn deployment_topology_from_config_unknown_value_errors() {
        let result = DeploymentTopology::from_config("distributed");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown deployment_topology \"distributed\""),
            "expected clear error message, got: {err}"
        );
    }

    #[test]
    fn build_plan_deployment_topology_defaults_to_monolith() {
        let registry = CapabilityRegistry::new();

        let toml = r#"
[profiles.default.meta]
name = "default"
version = "1.0.0"
description = ""

[profiles.default.features]
auth = true

[profiles.default.api]
generators = ["ddl"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["default"];
        let resolved = resolve_profile(def, None).unwrap();

        let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();
        assert_eq!(plan.deployment_topology(), DeploymentTopology::Monolith);
    }

    #[test]
    fn build_plan_deployment_topology_workers() {
        let registry = CapabilityRegistry::new();

        let toml = r#"
[profiles.workers.meta]
name = "workers"
version = "1.0.0"
description = ""

[profiles.workers.features]
deployment_topology = "workers"
persistence_provider = "cornucopia"

[profiles.workers.api]
generators = ["ddl"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["workers"];
        let resolved = resolve_profile(def, None).unwrap();

        let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();
        assert_eq!(plan.deployment_topology(), DeploymentTopology::Workers);
    }

    #[test]
    fn build_plan_workers_topology_requires_cornucopia() {
        let registry = CapabilityRegistry::new();

        let toml = r#"
[profiles.workers-sea-orm.meta]
name = "workers-sea-orm"
version = "1.0.0"
description = ""

[profiles.workers-sea-orm.features]
deployment_topology = "workers"
persistence_provider = "sea_orm"

[profiles.workers-sea-orm.api]
generators = ["ddl"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["workers-sea-orm"];
        let resolved = resolve_profile(def, None).unwrap();

        let err = BuildPlan::from_profile(&resolved, &registry).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("workers topology requires the cornucopia persistence provider"),
            "expected topology×provider error, got: {msg}"
        );
    }

    /// Issue #509 (the #502 refusal is RETIRED): `hooks_api_crate` under
    /// `deployment_topology = "workers"` is a supported combination — the
    /// lifecycle-hooks surface is persistence-agnostic since the context
    /// types moved to the generated `types` module (erased HookTx /
    /// HookConnSource handles with cornucopia adapters). Transactional
    /// hooks run on the open cornucopia transaction and post-commit hooks
    /// fire from the wasm32 worker slice.
    #[test]
    fn build_plan_hooks_api_crate_under_workers_topology_is_supported() {
        let registry = CapabilityRegistry::new();

        let toml = r#"
[profiles.workers-hooks.meta]
name = "workers-hooks"
version = "1.0.0"
description = ""
hooks_api_crate = "my_hooks"

[profiles.workers-hooks.features]
deployment_topology = "workers"
persistence_provider = "cornucopia"

[profiles.workers-hooks.api]
generators = ["ddl"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["workers-hooks"];
        let resolved = resolve_profile(def, None).unwrap();

        let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();
        assert_eq!(plan.deployment_topology(), DeploymentTopology::Workers);
    }

    /// Control: workers topology WITHOUT hooks stays supported (the
    /// crewbase shape — hooks_api_crate unset).
    #[test]
    fn build_plan_workers_without_hooks_api_crate_is_supported() {
        let registry = CapabilityRegistry::new();

        let toml = r#"
[profiles.workers-clean.meta]
name = "workers-clean"
version = "1.0.0"
description = ""

[profiles.workers-clean.features]
deployment_topology = "workers"
persistence_provider = "cornucopia"

[profiles.workers-clean.api]
generators = ["ddl"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["workers-clean"];
        let resolved = resolve_profile(def, None).unwrap();

        let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();
        assert_eq!(plan.deployment_topology(), DeploymentTopology::Workers);
    }

    /// Control: the SeaORM monolith keeps its hooks surface — the refusal
    /// is scoped to the workers topology only.
    #[test]
    fn build_plan_monolith_sea_orm_with_hooks_api_crate_is_supported() {
        let registry = CapabilityRegistry::new();

        let toml = r#"
[profiles.monolith-hooks.meta]
name = "monolith-hooks"
version = "1.0.0"
description = ""
hooks_api_crate = "my_hooks"

[profiles.monolith-hooks.features]
persistence_provider = "sea_orm"

[profiles.monolith-hooks.api]
generators = ["ddl"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["monolith-hooks"];
        let resolved = resolve_profile(def, None).unwrap();

        let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();
        assert_eq!(plan.deployment_topology(), DeploymentTopology::Monolith);
    }

    #[test]
    fn build_plan_deployment_topology_invalid_value_errors() {
        let registry = CapabilityRegistry::new();

        let toml = r#"
[profiles.bad.meta]
name = "bad"
version = "1.0.0"
description = ""

[profiles.bad.features]
deployment_topology = "distributed"

[profiles.bad.api]
generators = ["ddl"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["bad"];
        let resolved = resolve_profile(def, None).unwrap();

        let result = BuildPlan::from_profile(&resolved, &registry);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown deployment_topology"),
            "expected clear error message, got: {err}"
        );
    }

    #[test]
    fn build_plan_deployment_topology_non_string_errors() {
        let registry = CapabilityRegistry::new();

        let toml = r#"
[profiles.bad.meta]
name = "bad"
version = "1.0.0"
description = ""

[profiles.bad.features]
deployment_topology = true

[profiles.bad.api]
generators = ["ddl"]
"#;
        let config: ProfilesConfig = toml::from_str(toml).unwrap();
        let def = &config.profiles["bad"];
        let resolved = resolve_profile(def, None).unwrap();

        let result = BuildPlan::from_profile(&resolved, &registry);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("must be a string"),
            "expected clear error message, got: {err}"
        );
    }

    // ── ux_rules feature (issue #293) ──

    fn plan_with_features(features_toml: &str) -> Result<BuildPlan> {
        let registry = CapabilityRegistry::new();
        let toml = format!(
            r#"
[profiles.f.meta]
name = "f"
version = "1.0.0"
description = ""

[profiles.f.features]
{features_toml}

[profiles.f.api]
generators = ["ddl"]
"#
        );
        let config: ProfilesConfig = toml::from_str(&toml).unwrap();
        let def = &config.profiles["f"];
        let resolved = resolve_profile(def, None).unwrap();
        BuildPlan::from_profile(&resolved, &registry)
    }

    #[test]
    fn build_plan_ux_rules_defaults_to_false() {
        let plan = plan_with_features("auth = true").unwrap();
        assert!(!plan.ux_rules);
    }

    #[test]
    fn build_plan_ux_rules_parses_bool_values() {
        let plan = plan_with_features("ux_rules = true").unwrap();
        assert!(plan.ux_rules);
        let plan = plan_with_features("ux_rules = false").unwrap();
        assert!(!plan.ux_rules);
    }

    #[test]
    fn build_plan_ux_rules_non_bool_errors_naming_the_key() {
        let err = plan_with_features("ux_rules = \"yes\"").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("ux_rules") && msg.contains("boolean"),
            "expected ux_rules type error, got: {msg}"
        );
    }

    // ── dependency_strategy feature (issue #347) ──

    #[test]
    fn dependency_strategy_from_config_parses_known_values() {
        assert_eq!(
            DependencyStrategy::from_config("rev").unwrap(),
            DependencyStrategy::Rev
        );
        assert_eq!(
            DependencyStrategy::from_config("path").unwrap(),
            DependencyStrategy::Path
        );
        assert_eq!(DependencyStrategy::default(), DependencyStrategy::Rev);
    }

    #[test]
    fn dependency_strategy_from_config_unknown_value_errors() {
        let result = DependencyStrategy::from_config("branch");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown dependency_strategy \"branch\""),
            "expected clear error message, got: {err}"
        );
    }

    #[test]
    fn build_plan_dependency_strategy_defaults_to_rev() {
        let plan = plan_with_features("auth = true").unwrap();
        assert_eq!(plan.dependency_strategy, DependencyStrategy::Rev);
    }

    #[test]
    fn build_plan_dependency_strategy_parses_both_values() {
        let plan = plan_with_features("dependency_strategy = \"rev\"").unwrap();
        assert_eq!(plan.dependency_strategy, DependencyStrategy::Rev);
        let plan = plan_with_features("dependency_strategy = \"path\"").unwrap();
        assert_eq!(plan.dependency_strategy, DependencyStrategy::Path);
    }

    #[test]
    fn build_plan_dependency_strategy_invalid_value_errors() {
        let err = plan_with_features("dependency_strategy = \"branch\"").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unknown dependency_strategy"),
            "expected clear error message, got: {msg}"
        );
    }

    #[test]
    fn build_plan_dependency_strategy_non_string_errors() {
        let err = plan_with_features("dependency_strategy = true").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("dependency_strategy") && msg.contains("must be a string"),
            "expected dependency_strategy type error, got: {msg}"
        );
    }
}
