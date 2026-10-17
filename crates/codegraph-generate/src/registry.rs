use std::path::Path;

use crate::capabilities::Capability;
use crate::context::GeneratorContext;
use crate::output::{
    generator_base, is_worker_routed_domain_generator, is_worker_routed_entity_generator,
};
use crate::traits::{DomainGenerator, EntityGenerator, GlobalGenerator};
use crate::{
    api, atproto, cli, db, ddd, domain_types, emdash, evt_events, fern, grpc, hooks, ifml,
    integration, ops, playwright, scaffold, seed, test, ui, webhook,
};

/// Build the per-entity generator set.
pub(crate) fn build_entity_generators(
    ctx: &GeneratorContext<'_>,
    parent_candidates: &[codegraph_core::types::ParentCandidate],
    worker_base: Option<&Path>,
) -> Vec<Box<dyn EntityGenerator>> {
    let output_dir = ctx.output_dir;
    // Routed generators (backend Rust source for one domain) get the
    // per-domain worker crate directory; all others stay at the root.
    let base = |name: &str| {
        generator_base(
            output_dir,
            worker_base,
            is_worker_routed_entity_generator(name),
        )
    };
    vec![
        Box::new(
            db::ddl::DdlGenerator::new(base("ddl"))
                .with_dialect(ctx.make_dialect())
                .with_parent_candidates(parent_candidates.to_vec()),
        ) as Box<dyn EntityGenerator>,
        Box::new(
            db::entity::SeaOrmEntityGenerator::new(base("sea_orm_entity"))
                .with_dialect(ctx.make_dialect())
                .with_parent_candidates(parent_candidates.to_vec()),
        ) as Box<dyn EntityGenerator>,
        Box::new(
            db::cornucopia_queries::CornucopiaQueryGenerator::new(base("cornucopia_queries"))
                .with_parent_candidates(parent_candidates.to_vec()),
        ) as Box<dyn EntityGenerator>,
        Box::new(
            ddd::cornucopia_repo::CornucopiaRepoGenerator::new(base("cornucopia_repo"))
                .with_parent_candidates(parent_candidates.to_vec()),
        ) as Box<dyn EntityGenerator>,
        Box::new(
            ddd::repository::RepositoryTraitGenerator::new(base("repository"))
                .with_parent_candidates(parent_candidates.to_vec()),
        ) as Box<dyn EntityGenerator>,
        Box::new(
            ddd::command::CommandGenerator::new(base("command"))
                .with_parent_candidates(parent_candidates.to_vec()),
        ) as Box<dyn EntityGenerator>,
        Box::new(
            ddd::query::QueryGenerator::new(base("query"))
                .with_parent_candidates(parent_candidates.to_vec()),
        ) as Box<dyn EntityGenerator>,
        Box::new(ddd::event::EventGenerator::new(base("event"))) as Box<dyn EntityGenerator>,
        Box::new(ddd::dto::DtoGenerator::new(base("dto"))) as Box<dyn EntityGenerator>,
        Box::new(
            api::handler::HandlerGenerator::new(base("handler"))
                .with_parent_candidates(parent_candidates.to_vec()),
        ) as Box<dyn EntityGenerator>,
        Box::new(
            api::workflow_action::WorkflowActionGenerator::new(base("workflow_action"))
                .with_parent_candidates(parent_candidates.to_vec()),
        ) as Box<dyn EntityGenerator>,
        Box::new(api::media::MediaRouteGenerator::new(base("media_route")))
            as Box<dyn EntityGenerator>,
        Box::new(test::test_gen::TestGenerator::new(base("test"))) as Box<dyn EntityGenerator>,
        Box::new(
            ui::page::UiPageGenerator::new(base("ui-page"))
                .with_parent_candidates(parent_candidates.to_vec()),
        ) as Box<dyn EntityGenerator>,
        Box::new(ui::form::UiFormGenerator::new(base("ui-form"))) as Box<dyn EntityGenerator>,
        Box::new(ui::cosmos_entity_form::CosmosEntityFormGenerator::new(
            base("ui-form"),
        )) as Box<dyn EntityGenerator>,
        Box::new(
            ui::store::UiStoreGenerator::new(base("ui-store"))
                .with_parent_candidates(parent_candidates.to_vec()),
        ) as Box<dyn EntityGenerator>,
        Box::new(
            ui::e2e_test::UiE2eTestGenerator::new(base("ui-e2e-test"))
                .with_parent_candidates(parent_candidates.to_vec()),
        ) as Box<dyn EntityGenerator>,
        Box::new(playwright::entity_gen::PlaywrightEntityGenerator::new(
            base("playwright-entity"),
        )) as Box<dyn EntityGenerator>,
        Box::new(ui::descriptor::UiDescriptorGenerator::new(
            base("ui-descriptor"),
            ctx.ui_overrides.clone(),
            ctx.ui_domains.clone(),
        )) as Box<dyn EntityGenerator>,
        Box::new(ui::shell::UiShellGenerator::new(base("ui-shell"))) as Box<dyn EntityGenerator>,
        Box::new(
            hooks::lifecycle_trait::LifecycleTraitGenerator::new_with_base(
                ctx.hooks_base
                    .map(|b| b.to_path_buf())
                    .unwrap_or_else(|| output_dir.to_path_buf()),
            ),
        ) as Box<dyn EntityGenerator>,
        // domain_types generators: use the provided base override, defaulting to output_dir.
        Box::new(domain_types::dto::DomainTypesDtoGenerator::new_with_base(
            ctx.domain_types_base
                .map(|b| b.to_path_buf())
                .unwrap_or_else(|| output_dir.to_path_buf()),
        )) as Box<dyn EntityGenerator>,
        Box::new(
            domain_types::query_service::QueryServiceGenerator::new_with_base(
                ctx.domain_types_base
                    .map(|b| b.to_path_buf())
                    .unwrap_or_else(|| output_dir.to_path_buf()),
            ),
        ) as Box<dyn EntityGenerator>,
        Box::new(cli::command::CliCommandGenerator::new(base("cli_command")))
            as Box<dyn EntityGenerator>,
        // gRPC entity generators
        Box::new(grpc::proto::GrpcProtoGenerator::new(base("grpc_proto")))
            as Box<dyn EntityGenerator>,
        Box::new(grpc::service::GrpcServiceGenerator::new(base(
            "grpc_service",
        ))) as Box<dyn EntityGenerator>,
        // atproto entity generators
        Box::new(atproto::lexicon_gen::LexiconEmitter::new(base("lexicon")))
            as Box<dyn EntityGenerator>,
        Box::new(atproto::types_gen::AtprotoTypesEmitter::new(base(
            "atproto_types",
        ))) as Box<dyn EntityGenerator>,
        Box::new(atproto::client_gen::AtprotoClientEmitter::new(base(
            "atproto_client",
        ))) as Box<dyn EntityGenerator>,
        Box::new(atproto::xrpc_gen::AtprotoXrpcEmitter::new(base(
            "atproto_xrpc",
        ))) as Box<dyn EntityGenerator>,
    ]
    .into_iter()
    .filter(|generator| ctx.plan_has_entity(generator.name()))
    .filter(|generator| {
        generator
            .supported_targets()
            .map(|targets| targets.contains(&ctx.current_target))
            .unwrap_or(true)
    })
    .collect::<Vec<_>>()
}

/// Build the per-domain generator set.
pub(crate) fn build_domain_generators(
    ctx: &GeneratorContext<'_>,
    parent_candidates: &[codegraph_core::types::ParentCandidate],
    worker_base: Option<&Path>,
) -> Vec<Box<dyn DomainGenerator>> {
    let output_dir = ctx.output_dir;
    // Routed domain generators (backend source for one domain) get the
    // per-domain worker crate directory; all others stay at the root.
    let base = |name: &str| {
        generator_base(
            output_dir,
            worker_base,
            is_worker_routed_domain_generator(name),
        )
    };
    let mut gens: Vec<Box<dyn DomainGenerator>> = vec![
        Box::new(ddd::errors::ErrorGenerator::new(base("errors"))) as Box<dyn DomainGenerator>,
        // Constraint-plane validations (issue #261) — gated by the
        // `rosetta_backend` capability.
        Box::new(ddd::validations::ConditionValidationsGenerator::new(base(
            "condition_validations",
        ))) as Box<dyn DomainGenerator>,
        // Regulatory report scaffolding (issue #265) — gated by the
        // `rosetta_backend` capability.
        Box::new(ddd::regulatory_report::RegulatoryReportGenerator::new(
            base("regulatory_reports"),
        )) as Box<dyn DomainGenerator>,
        // Rosetta function codegen (issue #263) — gated by the
        // `rosetta_backend` capability.
        Box::new(ddd::functions::FunctionsGenerator::new(base("functions")))
            as Box<dyn DomainGenerator>,
        // Rosetta rule codegen (issue #264) — gated by the
        // `rosetta_backend` capability.
        Box::new(ddd::rules::RulesGenerator::new(base("rules"))) as Box<dyn DomainGenerator>,
        Box::new(
            api::router::RouterGenerator::new(base("router"))
                .with_parent_candidates(parent_candidates.to_vec()),
        ) as Box<dyn DomainGenerator>,
        Box::new(
            api::contract::ApiContractGenerator::new(base("api_contract"))
                .with_parent_candidates(parent_candidates.to_vec()),
        ) as Box<dyn DomainGenerator>,
        Box::new(api::links::LinksGenerator::new(base("links"))) as Box<dyn DomainGenerator>,
        // hurl API contract tests (issue #463) — per-entity contracts plus
        // the once-per-run authn/authz suite.
        Box::new(api::hurl_contract::HurlContractGenerator::new(base(
            "hurl_contract",
        ))) as Box<dyn DomainGenerator>,
        Box::new(ui::domain_layout::UiDomainLayoutGenerator::new(base(
            "ui-domain-layout",
        ))) as Box<dyn DomainGenerator>,
        Box::new(cli::domain::CliDomainGenerator::new(base("cli_domain")))
            as Box<dyn DomainGenerator>,
        // gRPC domain generator
        Box::new(grpc::router::GrpcRouterGenerator::new(base("grpc_router")))
            as Box<dyn DomainGenerator>,
        // atproto domain generators
        Box::new(atproto::appview_gen::AtprotoAppviewEmitter::new(base(
            "atproto_appview",
        ))) as Box<dyn DomainGenerator>,
        Box::new(atproto::xrpc_gen::AtprotoXrpcEmitter::new(base(
            "atproto_xrpc_router",
        ))) as Box<dyn DomainGenerator>,
    ];
    // EmDash plugin packages — only when the profile enables the feature
    // AND the plugins.toml config was loaded by the CLI wrapper.
    if ctx.capabilities.has(Capability::EmDash)
        && let Some(ref plugins) = ctx.emdash_plugins
    {
        gens.push(Box::new(emdash::plugin_gen::EmdashPluginGenerator::new(
            output_dir.to_path_buf(),
            plugins.clone(),
        )) as Box<dyn DomainGenerator>);
    }
    gens.into_iter()
        .filter(|generator| ctx.plan_has_domain(generator.name()))
        .collect::<Vec<_>>()
}

/// Build the global generator set: fixed core generators, the topology-
/// dependent scaffold, feature-gated extras, IFML generators per framework,
/// and integration generators when extension points are configured.
pub(crate) fn build_global_generators(ctx: &GeneratorContext<'_>) -> Vec<Box<dyn GlobalGenerator>> {
    let output_dir = ctx.output_dir;
    let mut global_gens: Vec<Box<dyn GlobalGenerator>> = vec![
        Box::new(
            db::basejump_setup::BasejumpSetupGenerator::new(output_dir)
                .with_dialect(ctx.make_dialect()),
        ) as Box<dyn GlobalGenerator>,
        Box::new(
            db::event_trigger::PgmqSetupGenerator::new(output_dir).with_dialect(ctx.make_dialect()),
        ) as Box<dyn GlobalGenerator>,
        Box::new(
            db::label_setup::LabelSetupGenerator::new(output_dir).with_dialect(ctx.make_dialect()),
        ) as Box<dyn GlobalGenerator>,
        Box::new(
            db::service_tables::ServiceTablesGenerator::new(output_dir)
                .with_dialect(ctx.make_dialect()),
        ) as Box<dyn GlobalGenerator>,
        Box::new(
            db::platform_schema::PlatformSchemaGenerator::new(output_dir)
                .with_dialect(ctx.make_dialect()),
        ) as Box<dyn GlobalGenerator>,
        Box::new(
            db::platform_grants::PlatformGrantsGenerator::new(output_dir)
                .with_dialect(ctx.make_dialect()),
        ) as Box<dyn GlobalGenerator>,
        Box::new(
            db::workflow_seed::WorkflowSeedGenerator::new(output_dir)
                .with_dialect(ctx.make_dialect()),
        ) as Box<dyn GlobalGenerator>,
        Box::new(db::cornucopia_config::CornucopiaConfigGenerator::new(
            output_dir,
        )) as Box<dyn GlobalGenerator>,
        Box::new(api::openapi::OpenApiGenerator::new(output_dir)) as Box<dyn GlobalGenerator>,
        Box::new(api::contract::ApiContractIndexGenerator::new(output_dir))
            as Box<dyn GlobalGenerator>,
    ];

    // The monolith scaffold (src/main.rs, src/server.rs, root Cargo.toml,
    // etc.) is gated off in workers topology: the workers scaffold generators
    // (step 3) own the per-domain worker crate and gateway scaffolds plus the
    // workspace Cargo.toml, and a root-level monolith Cargo.toml / src/ would
    // collide with them.  In monolith topology it keeps its original position
    // in the generator list so monolith output stays byte-identical.
    if ctx.workers_topology {
        global_gens.push(
            Box::new(scaffold::worker::WorkerScaffoldGenerator::new(output_dir))
                as Box<dyn GlobalGenerator>,
        );
    } else {
        global_gens.push(Box::new(
            scaffold::generator::ScaffoldGenerator::new(
                output_dir,
                ctx.capabilities.has(Capability::Webhooks),
                ctx.capabilities.has(Capability::Reports),
                ctx.capabilities.has(Capability::Grpc),
                ctx.capabilities.has(Capability::Atproto),
                ctx.capabilities.has(Capability::Cli),
                ctx.capabilities.has(Capability::TestGen),
                ctx.capabilities.has(Capability::Fern),
                ctx.capabilities.has(Capability::AuthRateLimit),
                ctx.capabilities.has(Capability::AdminCli),
                ctx.capabilities.has(Capability::Labels),
                &ctx.migration_strategy,
            )
            .with_seed(ctx.capabilities.has(Capability::Seed)),
        ) as Box<dyn GlobalGenerator>);
    }

    global_gens.push(Box::new(ui::scaffold::UiScaffoldGenerator::new(
        output_dir,
        ctx.ext_points.is_some(),
        ctx.capabilities.has(Capability::Webhooks),
    )) as Box<dyn GlobalGenerator>);
    global_gens
        .push(Box::new(ui::types::UiTypeGenerator::new(output_dir)) as Box<dyn GlobalGenerator>);
    // One testid vocabulary (issue #468 W3): emits `ui/src/lib/testids.ts`
    // when the `testid_module` feature is on (the generator itself warns
    // and emits nothing otherwise). Plan-filtered like the other globals.
    global_gens.push(
        Box::new(ui::testids_gen::UiTestidsGenerator::new(output_dir)) as Box<dyn GlobalGenerator>,
    );
    global_gens.push(Box::new(ui::codelist::UiCodelistGenerator::new(
        output_dir,
        ctx.schema_base_dir,
    )) as Box<dyn GlobalGenerator>);
    global_gens.push(
        Box::new(hooks::registry::HookRegistryGenerator::new_with_base(
            ctx.hooks_base
                .map(|b| b.to_path_buf())
                .unwrap_or_else(|| output_dir.to_path_buf()),
        )) as Box<dyn GlobalGenerator>,
    );
    global_gens.push(Box::new(
        domain_types::scaffold::DomainTypesScaffoldGenerator::new_with_base(
            ctx.domain_types_base
                .map(|b| b.to_path_buf())
                .unwrap_or_else(|| output_dir.to_path_buf()),
        ),
    ) as Box<dyn GlobalGenerator>);
    global_gens.push(
        Box::new(cli::scaffold::CliScaffoldGenerator::new(output_dir)) as Box<dyn GlobalGenerator>,
    );
    global_gens.push(Box::new(
        db::report_view::ReportViewGenerator::new(output_dir)
            .with_reports_dir(ctx.domain_config_dir)
            .with_dialect(ctx.make_dialect()),
    ) as Box<dyn GlobalGenerator>);
    global_gens.push(Box::new(
        db::seed::SeedDataGenerator::new(output_dir, ctx.seed_config.map(|p| p.to_path_buf()))
            .with_dialect(ctx.make_dialect()),
    ) as Box<dyn GlobalGenerator>);
    global_gens.push(
        Box::new(playwright::global_gen::PlaywrightGlobalGenerator::new(
            output_dir,
        )) as Box<dyn GlobalGenerator>,
    );
    global_gens.push(
        Box::new(webhook::dispatch::WebhookDispatchGenerator::new(output_dir))
            as Box<dyn GlobalGenerator>,
    );
    global_gens.push(
        Box::new(webhook::endpoint_api::WebhookEndpointApiGenerator::new(
            output_dir,
        )) as Box<dyn GlobalGenerator>,
    );
    // Typed event emission + subscriptions (issues #454/#455): the
    // generator itself gates on the architecture — no `.evt` input emits
    // zero files — so it registers unconditionally.
    global_gens.push(
        Box::new(evt_events::EvtEventsGenerator::new(output_dir)) as Box<dyn GlobalGenerator>,
    );
    // Demo-data seed module + CLI (opt-in via the `seed_provision` capability).
    if ctx.capabilities.has(Capability::Seed) {
        global_gens.push(
            Box::new(seed::provision::SeedProvisionGenerator::new(output_dir))
                as Box<dyn GlobalGenerator>,
        );
    }
    // gRPC global generator
    global_gens.push(
        Box::new(grpc::scaffold::GrpcScaffoldGenerator::new(output_dir))
            as Box<dyn GlobalGenerator>,
    );
    // Policy-driven RLS from the actor policy graph (issue #219) — gated by
    // the `rls_from_policy` capability (Postgres only; sqlite is a no-op).
    global_gens.push(Box::new(
        db::policy_rls::PolicyRlsGenerator::new(output_dir).with_dialect(ctx.make_dialect()),
    ) as Box<dyn GlobalGenerator>);
    // Public-operations RLS + route gating (issue #279) — gated by the
    // `public_operations_rls` capability (Postgres only; sqlite no-op).
    global_gens.push(Box::new(
        db::public_operations_rls::PublicOperationsRlsGenerator::new(output_dir)
            .with_dialect(ctx.make_dialect()),
    ) as Box<dyn GlobalGenerator>);
    // atproto global generators
    global_gens.push(Box::new(atproto::scaffold_gen::LexiconScaffoldEmitter::new(
        output_dir,
    )) as Box<dyn GlobalGenerator>);
    global_gens.push(
        Box::new(atproto::client_gen::AtprotoClientScaffoldEmitter::new(
            output_dir,
        )) as Box<dyn GlobalGenerator>,
    );
    global_gens.push(Box::new(atproto::identity_gen::AtprotoIdentityEmitter::new(
        output_dir,
    )) as Box<dyn GlobalGenerator>);
    global_gens.push(
        Box::new(atproto::types_gen::GeneratedTypesEmitter::new(output_dir))
            as Box<dyn GlobalGenerator>,
    );
    global_gens.push(
        Box::new(atproto::xrpc_merge_gen::AtprotoXrpcMergeEmitter::new(
            output_dir,
        )) as Box<dyn GlobalGenerator>,
    );
    // Fern SDK config generator
    global_gens
        .push(Box::new(fern::config::FernConfigGenerator::new(output_dir))
            as Box<dyn GlobalGenerator>);
    // EmDash plugin family scaffold — only when the profile enables the
    // feature AND the plugins.toml config was loaded by the CLI wrapper.
    if ctx.capabilities.has(Capability::EmDash)
        && let Some(ref plugins) = ctx.emdash_plugins
    {
        global_gens.push(
            Box::new(emdash::scaffold_gen::EmdashPluginScaffoldGenerator::new(
                output_dir.to_path_buf(),
                plugins.clone(),
            )) as Box<dyn GlobalGenerator>,
        );
    }
    // ops harness manifest + testkit crate
    global_gens.push(Box::new(
        ops::OpsManifestGenerator::new(
            output_dir,
            ctx.capabilities.has(Capability::Cli),
            ctx.capabilities.has(Capability::Ui),
            ctx.capabilities.has(Capability::AdminCli),
            ctx.capabilities.has(Capability::Grpc),
        )
        // The [hurl] manifest section rides the hurl_contract generator
        // (issue #463): emit it exactly when that generator is in the plan.
        .with_hurl_contracts(ctx.plan_has_domain("hurl_contract")),
    ) as Box<dyn GlobalGenerator>);

    // .evt inputs imply the events module: the graph-driven scaffold gate
    // (`has_events`) declares `pub mod events;` whenever the architecture is
    // non-empty, so a profile that predates `evt_events` (init templates
    // before #456) must not silently drop the generator — the generated lib
    // would declare a module with no files (E0583). Implied beats omitted.
    let evt_events_implied = match ctx.build_plan {
        Some(bp) => !bp.has_global_gen("evt_events"),
        None => false,
    };

    let mut global_gens: Vec<Box<dyn GlobalGenerator>> = global_gens
        .into_iter()
        .filter(|generator| {
            // The workers topology swap (above) replaces the monolith
            // scaffold with the per-worker scaffold + gateway — the plan may
            // gate it under either spelling: the profile-facing "scaffold"
            // (profiles.toml lists the monolith name) or the explicit
            // "worker_scaffold".
            if ctx.workers_topology && generator.name() == "worker_scaffold" {
                ctx.plan_has_global("worker_scaffold") || ctx.plan_has_global("scaffold")
            } else {
                generator.name() == "evt_events" && evt_events_implied || {
                    ctx.plan_has_global(generator.name())
                }
            }
        })
        .filter(|generator| {
            generator
                .supported_targets()
                .map(|targets| targets.contains(&ctx.current_target))
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();

    // Add IFML generators per framework
    let ifml_frameworks = if ctx.ifml_frameworks.is_empty() {
        vec!["svelte".to_string()]
    } else {
        ctx.ifml_frameworks.clone()
    };
    for fw in &ifml_frameworks {
        // Issue #468 W1b: integrated mode — when the SAME plan builds the
        // entity ui app (`ui_scaffold`/`ui_page`) AND the IFML route
        // generator for svelte, the IFML artifacts land INSIDE the ui tree
        // (`ui/`) so one app and one playwright run cover entity CRUD and
        // the modeled views. Standalone plans (ifml-only profiles, plan-less
        // runs, non-svelte frameworks) keep the disjoint `{fw}/` tree.
        let integrated = ifml_integrated(ctx, fw);
        let fw_output = if integrated {
            output_dir.join("ui")
        } else {
            output_dir.join(fw)
        };
        if ctx.build_plan.is_none() || ctx.plan_has_global(&format!("ifml_skeleton_{}", fw)) {
            // If-absent emission: inside an existing ui tree every skeleton
            // file is already scaffolded, so this is a no-op; in an ifml-only
            // plan it still builds the standalone SvelteKit skeleton.
            global_gens.push(
                Box::new(ifml::skeleton::IfmlSkeletonGenerator::new(&fw_output, fw))
                    as Box<dyn GlobalGenerator>,
            );
        }
        if ctx.build_plan.is_none() || ctx.plan_has_global(&format!("ifml_route_{}", fw)) {
            let mut route_gen = ifml::route_generator::IfmlRouteGenerator::new(&fw_output, fw)
                .with_mappings(ctx.ifml_components.cloned());
            if integrated {
                // Route group: sibling of the entity `(app)`/`(auth)` groups,
                // so the entity auth-guard layout does not wrap views — views
                // gate themselves via their +page.ts guards.
                route_gen = route_gen.with_route_root(ifml::route_generator::ROUTE_GROUP);
            }
            global_gens.push(Box::new(route_gen) as Box<dyn GlobalGenerator>);
        }
        if ctx.build_plan.is_none() || ctx.plan_has_global(&format!("ifml_navigation_{}", fw)) {
            global_gens.push(
                Box::new(ifml::navigation_generator::IfmlNavigationGenerator::new(
                    &fw_output, fw,
                )) as Box<dyn GlobalGenerator>,
            );
        }
        if ctx.build_plan.is_none() || ctx.plan_has_global(&format!("ifml_e2e_test_{}", fw)) {
            let mut e2e_gen = ifml::e2e_test::IfmlE2eTestGenerator::new(&fw_output, fw)
                .with_mappings(ctx.ifml_components.cloned());
            if integrated {
                // Specs/journeys/bootstrap move under ui/tests/ifml (support
                // under ui/tests/ifml/support) so they never collide with the
                // entity suite's tests/e2e tree.
                e2e_gen = e2e_gen.with_integration();
            }
            global_gens.push(Box::new(e2e_gen) as Box<dyn GlobalGenerator>);
        }
    }

    if let Some(ext) = ctx.ext_points {
        let integration_gens: [Box<dyn GlobalGenerator>; 4] = [
            Box::new(integration::tables::IntegrationTablesGenerator::new(
                output_dir,
                ext.clone(),
            )),
            Box::new(integration::config::IntegrationConfigGenerator::new(
                output_dir,
                ext.clone(),
            )),
            Box::new(integration::dispatch::IntegrationDispatchGenerator::new(
                output_dir,
            )),
            Box::new(integration::catalog::IntegrationCatalogGenerator::new(
                output_dir,
            )),
        ];
        for generator in integration_gens {
            if ctx.plan_has_global(generator.name()) {
                global_gens.push(generator);
            }
        }
    }

    global_gens
}

/// Integrated IFML mode (issue #468 W1b): the build plan carries BOTH the
/// entity ui app generators (`ui_scaffold` global or `ui_page` entity) AND
/// the `ifml_route_{fw}` generator for the framework. Svelte only — the
/// route-group layout is a SvelteKit device. Plan-less runs never integrate
/// (there is no plan promising the ui app), and the plan-less `ifml_generate`
/// path never reaches here anyway.
fn ifml_integrated(ctx: &GeneratorContext<'_>, fw: &str) -> bool {
    if fw != "svelte" {
        return false;
    }
    match ctx.build_plan {
        Some(bp) => {
            (bp.has_global_gen("ui_scaffold") || bp.has_entity_gen("ui_page"))
                && bp.has_global_gen(&format!("ifml_route_{fw}"))
        }
        None => false,
    }
}
