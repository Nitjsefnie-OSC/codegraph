use codegraph::profile::{self, BuildPlan, CapabilityRegistry};

use crate::harness::profiles_path;

#[test]
fn profiles_file_exists_and_parses() {
    let path = profiles_path();
    assert!(
        path.exists(),
        "profiles.toml not found at {}",
        path.display()
    );

    let content = std::fs::read_to_string(&path).unwrap();
    let config: profile::ProfilesConfig = toml::from_str(&content).unwrap();
    assert!(!config.profiles.is_empty());
}

#[test]
fn default_profile_parses_and_builds_plan() {
    let registry = CapabilityRegistry::new();
    let resolved = profile::load_and_resolve_profile(&profiles_path(), "default", None).unwrap();
    let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();

    assert_eq!(resolved.meta.name, "default");
    assert!(
        !plan.entity_generators.is_empty(),
        "default plan needs entity generators"
    );
    assert!(
        !plan.domain_generators.is_empty(),
        "default plan needs domain generators"
    );
    assert!(
        !plan.global_generators.is_empty(),
        "default plan needs global generators"
    );
    assert!(
        !plan.post_gen_scripts.is_empty(),
        "default plan needs post-gen scripts"
    );
}

#[test]
fn api_profile_parses_and_builds_plan() {
    let registry = CapabilityRegistry::new();
    let resolved = profile::load_and_resolve_profile(&profiles_path(), "api", None).unwrap();
    let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();

    assert_eq!(resolved.meta.name, "api");
    // API profile should have entity generators but no UI entity generators
    assert!(plan.has_entity_gen("ddl"));
    assert!(plan.has_entity_gen("handler"));
    assert!(
        !plan.has_entity_gen("ui_page"),
        "api profile should not include UI generators"
    );
    assert!(
        !plan.has_entity_gen("cli_command"),
        "api profile should not include CLI generators"
    );

    // Should have API domain generators
    assert!(plan.has_domain_gen("router"));
    assert!(plan.has_domain_gen("links"));
    assert!(!plan.has_domain_gen("ui-domain-layout"));
    assert!(!plan.has_domain_gen("cli_domain"));

    // Should have API global generators
    assert!(plan.has_global_gen("openapi"));
    assert!(plan.has_global_gen("scaffold"));
    assert!(
        !plan.has_global_gen("ui_scaffold"),
        "api profile should not include UI global generators"
    );
    assert!(
        !plan.has_global_gen("cli_scaffold"),
        "api profile should not include CLI global generators"
    );

    assert!(!plan.post_gen_scripts.is_empty());
}

#[test]
fn ui_profile_parses_and_builds_plan() {
    let registry = CapabilityRegistry::new();
    let resolved = profile::load_and_resolve_profile(&profiles_path(), "ui", None).unwrap();
    let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();

    assert_eq!(resolved.meta.name, "ui");
    // UI profile should have UI entity generators only
    assert!(
        !plan.has_entity_gen("ddl"),
        "ui profile should not include API generators"
    );
    assert!(!plan.has_entity_gen("handler"));
    assert!(plan.has_entity_gen("ui_page"));
    assert!(plan.has_entity_gen("ui_form"));
    assert!(plan.has_entity_gen("ui_store"));

    // Should have UI domain generators only
    assert!(!plan.has_domain_gen("router"));
    assert!(plan.has_domain_gen("ui-domain-layout"));

    // Should have UI global generators
    assert!(plan.has_global_gen("ui_scaffold"));
    assert!(plan.has_global_gen("ui_types"));
    assert!(plan.has_global_gen("ui_codelist"));
    assert!(
        !plan.has_global_gen("openapi"),
        "ui profile should not include API global generators"
    );

    assert_eq!(resolved.meta.tags, vec!["frontend", "sveltekit"]);
}

#[test]
fn cli_profile_parses_and_builds_plan() {
    let registry = CapabilityRegistry::new();
    let resolved = profile::load_and_resolve_profile(&profiles_path(), "cli", None).unwrap();
    let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();

    assert_eq!(resolved.meta.name, "cli");
    assert!(plan.has_entity_gen("cli_command"));
    assert!(!plan.has_entity_gen("ddl"));
    assert!(!plan.has_entity_gen("ui_page"));

    assert!(plan.has_domain_gen("cli_domain"));
    assert!(!plan.has_domain_gen("router"));

    assert!(plan.has_global_gen("cli_scaffold"));
    assert!(!plan.has_global_gen("ui_scaffold"));
    assert!(!plan.has_global_gen("openapi"));
}

#[test]
fn fullstack_profile_parses_and_builds_plan() {
    let registry = CapabilityRegistry::new();
    let resolved = profile::load_and_resolve_profile(&profiles_path(), "fullstack", None).unwrap();
    let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();

    assert_eq!(resolved.meta.name, "fullstack");
    // Should have both API and UI generators
    assert!(plan.has_entity_gen("ddl"));
    assert!(plan.has_entity_gen("ui_page"));
    assert!(plan.has_domain_gen("router"));
    assert!(plan.has_domain_gen("ui-domain-layout"));
    assert!(plan.has_global_gen("openapi"));
    assert!(plan.has_global_gen("ui_scaffold"));
    // But not CLI
    assert!(!plan.has_entity_gen("cli_command"));
    assert!(!plan.has_global_gen("cli_scaffold"));
}

#[test]
fn ci_profile_parses_and_builds_plan() {
    let registry = CapabilityRegistry::new();
    let resolved = profile::load_and_resolve_profile(&profiles_path(), "ci", None).unwrap();
    let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();

    assert_eq!(resolved.meta.name, "ci");
    // CI should have all generators (compile-gate)
    assert!(plan.has_entity_gen("ddl"));
    assert!(plan.has_entity_gen("ui_page"));
    assert!(plan.has_entity_gen("cli_command"));
    assert!(plan.has_global_gen("openapi"));
    assert!(plan.has_global_gen("ui_scaffold"));
    assert!(plan.has_global_gen("cli_scaffold"));
}

#[test]
fn api_profile_variant_lite_reduces_generators() {
    let registry = CapabilityRegistry::new();
    let resolved =
        profile::load_and_resolve_profile(&profiles_path(), "api", Some("lite")).unwrap();
    let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();

    // Lite variant should have fewer generators than full API
    assert!(plan.has_entity_gen("ddl"));
    assert!(plan.has_entity_gen("handler"));
    assert!(
        !plan.has_entity_gen("test"),
        "lite should exclude test generator"
    );
    assert!(
        !plan.has_entity_gen("workflow_action"),
        "lite should exclude workflow_action"
    );
    assert!(
        !plan.has_entity_gen("media_route"),
        "lite should exclude media_route"
    );
    assert!(
        !plan.has_entity_gen("lifecycle_trait"),
        "lite should exclude lifecycle_trait"
    );
    assert!(
        !plan.has_entity_gen("domain_types_dto"),
        "lite should exclude domain_types_dto"
    );

    // Features should be overridden
    assert_eq!(
        resolved.features.get("auth").unwrap().as_bool(),
        Some(false)
    );
    assert_eq!(
        resolved.features.get("validation_level").unwrap().as_str(),
        Some("balanced")
    );
}

#[test]
fn fullstack_variant_enterprise_adds_features() {
    let registry = CapabilityRegistry::new();
    let resolved =
        profile::load_and_resolve_profile(&profiles_path(), "fullstack", Some("enterprise"))
            .unwrap();
    let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();

    assert!(plan.has_entity_gen("ddl"));
    assert!(plan.has_entity_gen("ui_page"));
    // Features merged from variant
    assert_eq!(
        resolved.features.get("audit_trail").unwrap().as_bool(),
        Some(true)
    );
    assert_eq!(
        resolved.features.get("multitenancy").unwrap().as_bool(),
        Some(true)
    );
    assert_eq!(
        resolved
            .features
            .get("row_level_security")
            .unwrap()
            .as_bool(),
        Some(true)
    );
}

#[test]
fn enterprise_variant_generators_match_base_profile() {
    // Regression test: the enterprise variant is additive (keeps all generators
    // from the base, adds features). If a generator is added to the base
    // fullstack profile but not to the enterprise variant, it would silently
    // stop generating. This test catches that drift.
    let base = profile::load_and_resolve_profile(&profiles_path(), "fullstack", None).unwrap();
    let enterprise =
        profile::load_and_resolve_profile(&profiles_path(), "fullstack", Some("enterprise"))
            .unwrap();

    for (section_name, base_section) in &base.sections {
        let ent_section = &enterprise.sections[section_name.as_str()];
        for generator in &base_section.generators {
            assert!(
                ent_section.generators.contains(generator),
                "enterprise variant is missing generator \"{generator}\" from \
                 base profile's [{section_name}] section. \
                 If the generator was intentionally omitted, update the variant.",
            );
        }
    }
}

#[test]
fn fullstack_variant_lite_strips_to_minimal() {
    let registry = CapabilityRegistry::new();
    let resolved =
        profile::load_and_resolve_profile(&profiles_path(), "fullstack", Some("lite")).unwrap();
    let plan = BuildPlan::from_profile(&resolved, &registry).unwrap();

    // Lite has only minimal generators
    let mut entity_gens = plan.entity_generators.clone();
    entity_gens.sort();
    assert_eq!(
        entity_gens,
        vec!["ddl", "dto", "handler", "ui_form", "ui_page"]
    );

    // Lite keeps the mox behavior surface (issue #508: no band carries a
    // "we support mox" asterisk).
    assert_eq!(plan.domain_generators, vec!["router", "mox_ops"]);

    let mut global_gens = plan.global_generators;
    global_gens.sort();
    assert_eq!(global_gens, vec!["openapi", "scaffold", "ui_scaffold"]);

    assert!(!resolved.features.get("auth").unwrap().as_bool().unwrap());
}

#[test]
fn unknown_profile_is_error() {
    let result = profile::load_and_resolve_profile(&profiles_path(), "nonexistent", None);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("nonexistent"),
        "expected mention of name: {err}"
    );
}

#[test]
fn unknown_variant_is_error() {
    let result = profile::load_and_resolve_profile(&profiles_path(), "api", Some("nonexistent"));
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("nonexistent"),
        "expected mention of variant: {err}"
    );
}

#[test]
fn all_profiles_have_valid_meta() {
    let _registry = CapabilityRegistry::new();
    let content = std::fs::read_to_string(profiles_path()).unwrap();
    let config: profile::ProfilesConfig = toml::from_str(&content).unwrap();

    for (name, def) in &config.profiles {
        let meta = def.meta.as_ref().unwrap();
        assert!(!meta.name.is_empty(), "profile {name} has no meta.name");
        assert!(
            !meta.version.is_empty(),
            "profile {name} has no meta.version"
        );
        assert!(
            !meta.description.is_empty(),
            "profile {name} has no meta.description"
        );
    }
}
