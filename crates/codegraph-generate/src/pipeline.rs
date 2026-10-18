use std::fs;
use std::path::Path;

use codegraph_config::{DomainConfig, UiDomainConfig, UiOverrideConfig};
use codegraph_core::caching_querier::CachingQuerier;
use codegraph_core::traits::GraphQuerier;
use tera::Tera;

use crate::codelist;
use crate::context::{GeneratorContext, build_generator_context, build_manifest_roots};
use crate::db;
use crate::domain_types;
use crate::error::{Error, Result};
use crate::ifml;
use crate::ifml::IfmlQuerier;
use crate::manifest;
use crate::ordering::{all_domains_for_generation, compute_generation_order};
use crate::output::{
    MigrationSeq, clean_generated_output, clean_stale_ifml_routes, generate_mod_files,
    generate_test_mod_files, is_api_entity_generator, prefix_migration_path, prune_entity_mod,
    write_output,
};
use crate::project_config::{GenerationEntry, GeneratorOpts, ProjectConfig};
use crate::registry::{build_domain_generators, build_entity_generators, build_global_generators};
use crate::report;
use crate::traits::{DomainGenerator, EntityGenerator, GeneratedFile, GlobalGenerator};
use crate::type_registry;

/// Run all generators for all entities in topological order.
pub async fn run_generators(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    output_dir: &Path,
    tera: &Tera,
    ui_overrides: &UiOverrideConfig,
    ui_domains: &UiDomainConfig,
    schema_base_dir: &Path,
) -> Result<report::GenerationReport> {
    run_generators_with_opts(GeneratorOpts {
        db,
        config,
        output_dir,
        tera,
        ui_overrides,
        ui_domains,
        schema_base_dir,
        seed_config: None,
        domain_types_base: None,
        hooks_base: None,
        ext_points: None,
        build_plan: None,
        ifml_frameworks: vec![],
        ifml_components: None,
        ux_rules: None,
        project_config: None,
        emdash_plugins: None,
        domain_config_dir: None,
    })
    .await
}

/// Like [`run_generators`] but redirects `hr-domain-types` and `hr-hooks-api`
/// output to temp directories instead of the compiled-in workspace paths.
#[allow(clippy::too_many_arguments)]
pub async fn run_generators_with_domain_types_base(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    output_dir: &Path,
    tera: &Tera,
    ui_overrides: &UiOverrideConfig,
    ui_domains: &UiDomainConfig,
    schema_base_dir: &Path,
    domain_types_base: &Path,
    hooks_base: &Path,
) -> Result<report::GenerationReport> {
    run_generators_with_opts(GeneratorOpts {
        db,
        config,
        output_dir,
        tera,
        ui_overrides,
        ui_domains,
        schema_base_dir,
        seed_config: None,
        domain_types_base: Some(domain_types_base),
        hooks_base: Some(hooks_base),
        ext_points: None,
        build_plan: None,
        ifml_frameworks: vec![],
        ifml_components: None,
        ux_rules: None,
        project_config: None,
        emdash_plugins: None,
        domain_config_dir: None,
    })
    .await
}

/// Run generators with full configuration via [`GeneratorOpts`].
pub async fn run_generators_with_opts(opts: GeneratorOpts<'_>) -> Result<report::GenerationReport> {
    // Project config is threaded explicitly to every generator/helper that
    // needs it (no global state).
    let default_project = ProjectConfig::default();
    let project = opts.project_config.unwrap_or(&default_project);

    let ctx = build_generator_context(opts, project).await?;
    let manifest_root_paths = build_manifest_roots(&ctx);
    let manifest_roots: Vec<&Path> = manifest_root_paths.iter().map(|p| p.as_path()).collect();

    let order = compute_generation_order(ctx.db(), ctx.config).await?;
    let mut report = report::GenerationReport::new();

    // Clean stale generated files from the output directory before generators
    // run.  Previous pipeline runs may have produced files for entities that
    // are no longer in the generation order (e.g. reclassified as VOs).
    // The filesystem-scanning `generate_mod_files` pass would otherwise pick
    // them up and emit broken `pub mod` declarations.
    clean_generated_output(
        ctx.output_dir,
        &order,
        &ctx.config.defaults.type_suffix,
        ctx.workers_topology,
    );

    // Clean stale IFML route directories from previous runs.  Both the
    // IFML-only path (`run_ifml_generators`) and the full pipeline clean per
    // framework with the active views from the current model, so removed
    // views do not leave stale `src/routes/{view}` dirs behind.
    clean_stale_ifml_output(&ctx).await;

    // Clean generated migration files (seq >= 10) left by previous runs.
    clean_stale_migrations(&ctx);

    // Fetch parent-child relationship candidates from the graph for
    // router and handler generators to populate nested route information.
    let parent_candidates = ctx
        .db()
        .get_parent_candidates()
        .await
        .map_err(|e| Error::Config(e.to_string()))?;

    let mut entity_gens: Vec<Box<dyn EntityGenerator>> = if ctx.workers_topology {
        Vec::new()
    } else {
        build_entity_generators(&ctx, &parent_candidates, None)
    };
    let monolith_domain_gens: Vec<Box<dyn DomainGenerator>> = if ctx.workers_topology {
        Vec::new()
    } else {
        build_domain_generators(&ctx, &parent_candidates, None)
    };
    let global_gens: Vec<Box<dyn GlobalGenerator>> = build_global_generators(&ctx);

    // Per-entity generators — run entities sequentially to ensure TypeRegistry
    // is populated for earlier entities before later entities reference their types.
    let entity_results = run_entity_phase(&ctx, &mut entity_gens, &parent_candidates, &order).await;

    // Issue #460: the cross-plane consistency sweep — every table the
    // repository plane writes or reads must be created by the DDL plane.
    // Divergence is a hard generation error: divergent output is a
    // guaranteed runtime failure (INSERT against a table no migration
    // creates). Skipped when the profile carries no `ddl` generator
    // (nothing to check against); monolith-only (workers rebuild their
    // entity generators per domain inside the phase).
    if entity_gens.iter().any(|g| g.name() == "ddl") {
        let divergences = crate::consistency::check_child_table_consistency(
            ctx.db(),
            ctx.config,
            &order,
            &parent_candidates,
        )
        .await?;
        if !divergences.is_empty() {
            return Err(Error::Config(format!(
                "child-table projection divergence (issue #460): repository SQL targets \
                 tables the DDL never creates:\n  - {}",
                divergences.join("\n  - ")
            )));
        }
    }

    write_entity_results(entity_results, &mut report)?;

    // Codelists are not entities — SQL migrations + Rust enums run separately.
    run_codelist_generators(&ctx, &order, &mut report).await?;

    let domains_with_entities = all_domains_for_generation(ctx.config, &order);
    run_domain_phase(
        &ctx,
        &monolith_domain_gens,
        &parent_candidates,
        &domains_with_entities,
        &mut report,
    )
    .await?;

    run_global_phase(&ctx, &global_gens, &order, &mut report).await?;

    // Codelist Rust enum re-exports (the generated app re-exports from the
    // domain-types crate). Runs AFTER the global phase: the events plane
    // (`src/events/contracts.rs`, emitted by the `evt_events` global
    // generator) also references `crate::codelist::<Name>`, and the scan
    // must see it — running before it left the app-side re-export list
    // empty for evt models (crewbase compile gate). Monolith: the FULL
    // codelist list (see `generate_reexport_mod`). Workers topology: the
    // root crate is a placeholder — header-only re-exports; each worker
    // carries its own referenced-only list (emitted in
    // `run_codelist_generators`).
    if ctx.workers_topology {
        match codelist::rust_enum::RustCodelistGenerator::new(ctx.output_dir)
            .generate_reexport_mod_for(ctx.db(), ctx.output_dir, ctx.project)
            .await
        {
            Ok(files) => {
                for file in &files {
                    write_output(file)?;
                }
                report.files.extend(files);
            }
            Err(e) => {
                report.errors.push(report::GenerationError {
                    entity: "(codelists)".into(),
                    generator: "rust_enum".into(),
                    source: e,
                });
            }
        }
    } else {
        match codelist::rust_enum::RustCodelistGenerator::new(ctx.output_dir)
            .generate_reexport_mod(ctx.db(), ctx.project)
            .await
        {
            Ok(files) => {
                for file in &files {
                    write_output(file)?;
                }
                report.files.extend(files);
            }
            Err(e) => {
                report.errors.push(report::GenerationError {
                    entity: "(codelists)".into(),
                    generator: "rust_enum".into(),
                    source: e,
                });
            }
        }
    }

    // Validate that every entity in the generation order has entity-specific files
    report.validate_consistency(&order, &ctx.config.defaults.type_suffix);

    // Generate mod.rs files for all directories under src/.
    write_mod_files(&ctx, &domains_with_entities, &mut report)?;

    // Emit integration-test glue + `.codegraph-manifest.json` at each output root.
    emit_run_manifests(&ctx, &manifest_roots, &mut report)?;

    // Post-generation structural gates (issue #468): the emitted tree must
    // not carry the vocabulary the pipeline retired. Hard error, file:line.
    crate::quality_gates::run_structural_gates(ctx.output_dir, ctx.project)?;

    Ok(report)
}

/// Clean stale IFML route directories for the full pipeline path: mirrors
/// `run_ifml_generators` — per configured framework, only when the build
/// plan includes that framework's `ifml_route` generator, and only when the
/// IFML model has at least one view (an empty model means nothing is
/// IFML-owned, so nothing may be removed).
async fn clean_stale_ifml_output(ctx: &GeneratorContext<'_>) {
    let frameworks = if ctx.ifml_frameworks.is_empty() {
        vec!["svelte".to_string()]
    } else {
        ctx.ifml_frameworks.clone()
    };
    let route_frameworks: Vec<&String> = frameworks
        .iter()
        .filter(|fw| ctx.plan_has_global(&format!("ifml_route_{}", fw)))
        .collect();
    if route_frameworks.is_empty() {
        return;
    }
    let querier = ifml::IfmlGraphQuerier::new(ctx.db());
    let Ok(model) = querier.get_ifml_model().await else {
        return;
    };
    if model.view_containers.is_empty() {
        return;
    }
    let active_views: Vec<String> = model
        .view_containers
        .iter()
        .map(|vc| vc.name.clone())
        .collect();
    for fw in route_frameworks {
        let fw_output = ctx.output_dir.join(fw);
        clean_stale_ifml_routes(&fw_output, fw, &active_views);
    }
}

/// Clean generated migration files (seq >= 10) from previous runs.  New runs
/// may generate a different set of files (e.g. duplicates removed),
/// and stale numbered SQL files must not linger in the migrations directory.
/// Hand-written bootstrap migrations (0000–0009) are preserved.
///
/// Only runs when the active build plan includes the `ddl` generator (the
/// entity migration emitter). Narrower profiles (e.g. e2e/playwright-only)
/// must NOT delete migrations produced by a previous fullstack run — they
/// would never be regenerated, corrupting the migration set.
fn clean_stale_migrations(ctx: &GeneratorContext<'_>) {
    let plan_emits_migrations = ctx
        .build_plan
        .map(|bp| bp.has_entity_gen("ddl"))
        .unwrap_or(true);
    let migrations_dir = db::migrations_root(ctx.output_dir);
    if plan_emits_migrations && migrations_dir.is_dir() {
        let mut removed = 0usize;
        for entry in fs::read_dir(&migrations_dir)
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("sql") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                // Parse leading numeric prefix.
                let prefix_end = stem
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(stem.len());
                if let Ok(seq) = stem[..prefix_end].parse::<usize>() {
                    // Generated migrations start at the codelist band
                    // (codelist, integration, entity); platform bootstrap
                    // files below it are preserved.
                    if seq >= MigrationSeq::CODELIST_START.get() as usize {
                        let _ = fs::remove_file(&path);
                        removed += 1;
                    }
                }
            }
        }
        if removed > 0 {
            tracing::debug!(
                "Removed {removed} stale generated migration files from {}",
                migrations_dir.display()
            );
        }
    }
}

/// Run every per-entity generator for every entity in the generation order.
///
/// Two passes (#333 single generator-ordering contract):
///
/// **Pass 1 — registration.** The `dto` generator is the entity-level
/// registrar of the `{Entity}Response`/request/child types that the
/// repository emitter, query, and handler generators resolve through the
/// type registry. Rendering a repository for entity A before entity B's dto
/// has registered `BResponse` silently drops the cross-module import
/// (E0425). Pass 1 therefore runs ONLY the dto generator per entity, in
/// generation order, discarding its files (pass 2 renders them); errors
/// re-surface in pass 2 where they are reported. Files, migration
/// numbering, and the report are unchanged — only the registry is warm by
/// construction.
///
/// **Pass 2 — render.** The historical loop: per entity, all generators in
/// registration order. Entities run sequentially so TypeRegistry entries
/// for earlier entities exist before later entities reference them.
///
/// The generation order is already grouped by domain (see
/// `compute_generation_order`), so in workers topology we rebuild the
/// entity generator set each time the domain changes, constructing
/// routed generators with `workers/{domain}/` as their base so their
/// output lands inside the per-domain worker crate.  In monolith topology
/// the generator set is built exactly once with the root output dir.
async fn run_entity_phase(
    ctx: &GeneratorContext<'_>,
    entity_gens: &mut Vec<Box<dyn EntityGenerator>>,
    parent_candidates: &[codegraph_core::types::ParentCandidate],
    order: &[GenerationEntry],
) -> Vec<(Vec<GeneratedFile>, Vec<report::GenerationError>)> {
    let mut entity_results: Vec<(Vec<GeneratedFile>, Vec<report::GenerationError>)> = Vec::new();
    let mut current_worker_domain: Option<String> = None;

    let runs_dto = entity_gens.iter().any(|g| g.name() == "dto");

    // Pass 1 (monolith): register every DTO type up front.
    if runs_dto && !ctx.workers_topology {
        register_dto_types(ctx, entity_gens, order).await;
        #[cfg(debug_assertions)]
        if entity_gens.iter().any(|g| g.name() == "repository") {
            debug_assert_repository_registry_ready(ctx, order).await;
        }
    }

    for entry in order {
        if ctx.workers_topology && current_worker_domain.as_deref() != Some(entry.domain.as_str()) {
            let worker_dir = ctx.output_dir.join("workers").join(&entry.domain);
            *entity_gens = build_entity_generators(ctx, parent_candidates, Some(&worker_dir));
            current_worker_domain = Some(entry.domain.clone());
            // Pass 1 (workers): the routed dto generator for this domain
            // registers its types before the domain's repositories render.
            if entity_gens.iter().any(|g| g.name() == "dto") {
                let domain_entries: Vec<GenerationEntry> = order
                    .iter()
                    .filter(|e| e.domain == entry.domain)
                    .cloned()
                    .collect();
                register_dto_types(ctx, entity_gens, &domain_entries).await;
            }
        }

        let generation_mode = ctx
            .config
            .domains
            .get(&entry.domain)
            .and_then(|d| d.get_entity_config(&entry.schema_title))
            .and_then(|ec| ec.generation_mode.as_deref())
            .unwrap_or(&ctx.config.defaults.generation_mode);

        if generation_mode == "none" {
            continue;
        }

        let mut entity_files = Vec::new();
        let mut errors = Vec::new();
        for generator in entity_gens.iter() {
            if generation_mode == "ddd_only" && is_api_entity_generator(generator.name()) {
                continue;
            }
            match generator
                .generate(
                    ctx.db(),
                    &entry.schema_title,
                    &entry.domain,
                    ctx.config,
                    ctx.tera,
                    ctx.project,
                )
                .await
            {
                Ok(files) => entity_files.extend(files),
                Err(e) => {
                    errors.push(report::GenerationError {
                        entity: entry.schema_title.clone(),
                        generator: generator.name().to_string(),
                        source: e,
                    });
                }
            }
        }
        entity_results.push((entity_files, errors));
    }

    // Debug invariant for the workers rebuilds whose domains were checked
    // above; the monolith check ran right after its registration pass.
    #[cfg(debug_assertions)]
    if ctx.workers_topology && runs_dto && entity_gens.iter().any(|g| g.name() == "repository") {
        debug_assert_repository_registry_ready(ctx, order).await;
    }

    entity_results
}

/// Pass 1 of the entity phase (#333): run the `dto` generator for every
/// entry so the type registry holds its `{Entity}Response`/request/child
/// registrations before any dependent generator renders. Files are
/// discarded — pass 2 renders them — and errors are ignored here: pass 2
/// runs the same generator and reports the failure exactly once.
async fn register_dto_types(
    ctx: &GeneratorContext<'_>,
    entity_gens: &[Box<dyn EntityGenerator>],
    entries: &[GenerationEntry],
) {
    for entry in entries {
        let generation_mode = ctx
            .config
            .domains
            .get(&entry.domain)
            .and_then(|d| d.get_entity_config(&entry.schema_title))
            .and_then(|ec| ec.generation_mode.as_deref())
            .unwrap_or(&ctx.config.defaults.generation_mode);
        if generation_mode == "none" {
            continue;
        }
        for generator in entity_gens.iter().filter(|g| g.name() == "dto") {
            if generation_mode == "ddd_only" && is_api_entity_generator(generator.name()) {
                continue;
            }
            let _ = generator
                .generate(
                    ctx.db(),
                    &entry.schema_title,
                    &entry.domain,
                    ctx.config,
                    ctx.tera,
                    ctx.project,
                )
                .await;
        }
    }
}

/// #333 debug invariant: when repositories begin, the type registry holds a
/// `{Entity}Response` for every generated entity. A miss means a repository
/// would silently drop the cross-module import for that type (E0425) — the
/// failure class the registration pass exists to prevent. Debug builds
/// only; release runs skip the schema lookups.
#[cfg(debug_assertions)]
async fn debug_assert_repository_registry_ready(
    ctx: &GeneratorContext<'_>,
    order: &[GenerationEntry],
) {
    for entry in order {
        let generation_mode = ctx
            .config
            .domains
            .get(&entry.domain)
            .and_then(|d| d.get_entity_config(&entry.schema_title))
            .and_then(|ec| ec.generation_mode.as_deref())
            .unwrap_or(&ctx.config.defaults.generation_mode);
        if generation_mode == "none" {
            continue;
        }
        let Ok(Some(schema)) = ctx
            .db()
            .get_schema_in_domain(&entry.schema_title, &entry.domain)
            .await
        else {
            continue;
        };
        let type_name = format!("{}Response", schema.rust_type_name);
        // Registry probe via the existing resolution API: a name resolves to
        // a `use` statement iff it is registered (and no caller base ever
        // equals a registered module path).
        let registered =
            !type_registry::resolve_imports(std::slice::from_ref(&type_name), &[]).is_empty();
        assert!(
            registered,
            "#333 ordering invariant: `{type_name}` is not registered but the repository \
             generator is about to run for {}.{} — the dto registration pass must render \
             before dependent generators",
            entry.domain, entry.schema_title
        );
    }
}

/// Write per-entity output: migrations get sequential prefixes (starting at
/// 500) and files sharing an unprefixed base name are deduplicated.
fn write_entity_results(
    entity_results: Vec<(Vec<GeneratedFile>, Vec<report::GenerationError>)>,
    report: &mut report::GenerationReport,
) -> Result<()> {
    // Deduplicate migration files by their unprefixed base name: two different schema
    // titles can produce the same pg_table_name (e.g. "AssessmentAccessType" and a
    // cross-domain ref "AssessmentAccess" both → assessments_assessment_access.sql).
    // Keep only the first occurrence; skip subsequent duplicates.
    let mut seen_migration_names: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let mut entity_seq = MigrationSeq::ENTITY_START;
    for (entity_files, errors) in entity_results.into_iter() {
        for file in entity_files {
            // Check for duplicate migration base names before assigning seq number.
            let is_migration = file
                .path
                .parent()
                .and_then(|p| p.file_name())
                .is_some_and(|d| d == "migrations");
            if is_migration && let Some(name) = file.path.file_name().and_then(|n| n.to_str()) {
                let base = name
                    .trim_start_matches(|c: char| c.is_ascii_digit() || c == '_')
                    .to_string();
                if !seen_migration_names.insert(base.clone()) {
                    // Two different schema titles produced the same pg_table_name
                    // (e.g. "AssessmentAccessType" and a cross-domain ref
                    // "AssessmentAccess" both produce assessments_assessment_access.sql).
                    // Keep the first occurrence; skip subsequent ones.
                    tracing::warn!(
                        migration = %name,
                        base = %base,
                        "skipping duplicate migration — same pg_table_name produced by \
                         multiple schema titles; first occurrence wins"
                    );
                    continue;
                }
            }
            let file = prefix_migration_path(file, entity_seq.get() as usize);
            entity_seq = entity_seq.plus(1);
            write_output(&file)?;
            report.files.push(file);
        }
        report.errors.extend(errors);
    }
    Ok(())
}

/// Run the codelist generators: SQL migrations (codelists are not entities,
/// so they run separately), Rust enums into the domain-types crate, the
/// generated-app re-export module, and — in workers topology — per-worker
/// codelist re-export modules.
async fn run_codelist_generators(
    ctx: &GeneratorContext<'_>,
    order: &[GenerationEntry],
    report: &mut report::GenerationReport,
) -> Result<()> {
    // Codelist SQL migration generators (codelists are not entities, run separately)
    {
        let mut codelists = ctx
            .db()
            .list_codelists()
            .await
            .map_err(|e| Error::Config(e.to_string()))?;
        // Migration-number allocation (issue #332): the `CODELIST_START +
        // idx` sequence below is only deterministic if the codelist order
        // is. The graph query returns scan order (unsorted contract), so
        // sort by name at the allocation boundary.
        codelists.sort_by(|a, b| a.name.cmp(&b.name));
        let codelist_sql_gen =
            db::codelist::CodelistGenerator::new(ctx.output_dir).with_dialect(ctx.make_dialect());
        for (idx, cl) in codelists.iter().enumerate() {
            let files = codelist_sql_gen
                .generate(
                    ctx.db(),
                    &cl.name,
                    "common",
                    ctx.config,
                    ctx.tera,
                    ctx.project,
                )
                .await?;
            for file in files {
                let seq = MigrationSeq::CODELIST_START.plus(idx as u32);
                let file = prefix_migration_path(file, seq.get() as usize);
                write_output(&file)?;
                report.files.push(file);
            }
        }
    }

    // Codelist Rust enums into domain-types crate (source-of-truth for DTOs)
    let codelist_gen = domain_types::codelist::DomainTypesCodelistGenerator::new_with_base(
        ctx.domain_types_base
            .map(|b| b.to_path_buf())
            .unwrap_or_else(|| ctx.output_dir.to_path_buf()),
    );
    match codelist_gen
        .generate_all(ctx.db(), ctx.tera, ctx.project)
        .await
    {
        Ok(files) => {
            for file in &files {
                write_output(file)?;
            }
            report.files.extend(files);
        }
        Err(e) => {
            report.errors.push(report::GenerationError {
                entity: "(domain-types-codelists)".into(),
                generator: "domain_types_codelist".into(),
                source: e,
            });
        }
    }

    // In workers topology each worker crate gets its own `src/codelist/mod.rs`
    // re-exporting exactly the codelists its routed DTO code references
    // (`crate::codelist::<Name>`).  The root-anchored scan above finds
    // nothing in workers topology (domain/api source lives under
    // `workers/{domain}/src/`), so the worker `lib.rs`'s `pub mod codelist;`
    // would otherwise not compile.  Monolith topology never enters this
    // branch, keeping the root output byte-identical. The MONOLITH root
    // re-export runs LATER (after the global phase) — the events plane
    // (`src/events/contracts.rs`, emitted by the `evt_events` global
    // generator) also references `crate::codelist::<Name>`, so the scan
    // must see it.
    if ctx.workers_topology {
        let worker_codelist_gen = codelist::rust_enum::RustCodelistGenerator::new(ctx.output_dir);
        for (domain, _entity_titles) in all_domains_for_generation(ctx.config, order) {
            let worker_base = ctx.output_dir.join("workers").join(&domain);
            match worker_codelist_gen
                .generate_reexport_mod_for(ctx.db(), &worker_base, ctx.project)
                .await
            {
                Ok(files) => {
                    for file in &files {
                        write_output(file)?;
                    }
                    report.files.extend(files);
                }
                Err(e) => {
                    report.errors.push(report::GenerationError {
                        entity: format!("(worker-codelists:{domain})"),
                        generator: "rust_enum".into(),
                        source: e,
                    });
                }
            }
        }
    }

    Ok(())
}

/// Run all (domain, generator) pairs, writing files and collecting errors
/// into the report.
///
/// In workers topology the domain generator set is constructed per domain
/// with `workers/{domain}/` as the base for routed generators, so
/// errors.rs / router.rs / links.rs land inside the worker crate.
/// In monolith topology a single generator set is built once with the
/// root output dir, exactly as before. Entity-less custom-routes domains
/// are included with an empty entity list (see
/// [`all_domains_for_generation`]).
async fn run_domain_phase(
    ctx: &GeneratorContext<'_>,
    monolith_domain_gens: &[Box<dyn DomainGenerator>],
    parent_candidates: &[codegraph_core::types::ParentCandidate],
    domains_with_entities: &[(String, Vec<String>)],
    report: &mut report::GenerationReport,
) -> Result<()> {
    let domain_results: Vec<_> = if ctx.workers_topology {
        let mut results = Vec::new();
        for (domain, entity_titles) in domains_with_entities {
            let worker_dir = ctx.output_dir.join("workers").join(domain);
            let domain_gens = build_domain_generators(ctx, parent_candidates, Some(&worker_dir));
            let per_domain: Vec<_> =
                futures::future::join_all(domain_gens.iter().map(|generator| {
                    let domain = domain.clone();
                    async move {
                        let result = generator
                            .generate(
                                ctx.db(),
                                &domain,
                                entity_titles,
                                ctx.config,
                                ctx.tera,
                                ctx.project,
                            )
                            .await;
                        (domain, generator.name().to_string(), result)
                    }
                }))
                .await;
            results.extend(per_domain);
        }
        results
    } else {
        futures::future::join_all(domains_with_entities.iter().flat_map(
            |(domain, entity_titles)| {
                monolith_domain_gens.iter().map(move |generator| {
                    let domain = domain.clone();
                    async move {
                        let result = generator
                            .generate(
                                ctx.db(),
                                &domain,
                                entity_titles,
                                ctx.config,
                                ctx.tera,
                                ctx.project,
                            )
                            .await;
                        (domain, generator.name().to_string(), result)
                    }
                })
            },
        ))
        .await
    };

    for (domain, gen_name, result) in domain_results {
        match result {
            Ok(files) => {
                for file in &files {
                    write_output(file)?;
                }
                report.files.extend(files);
            }
            Err(e) => {
                report.errors.push(report::GenerationError {
                    entity: domain,
                    generator: gen_name,
                    source: e,
                });
            }
        }
    }

    Ok(())
}

/// Run global generators in parallel; any failure is fatal.
///
/// Generators flagged [`GlobalGenerator::sequential_first`] run (and write)
/// before the parallel wave: later generators' if-absent checks consult their
/// output (the IFML skeleton's package.json supersedes the e2e generator's
/// minimal stub).
async fn run_global_phase(
    ctx: &GeneratorContext<'_>,
    global_gens: &[Box<dyn GlobalGenerator>],
    order: &[GenerationEntry],
    report: &mut report::GenerationReport,
) -> Result<()> {
    let (first, parallel): (Vec<_>, Vec<_>) = global_gens
        .iter()
        .partition(|generator| generator.sequential_first());
    for generator in &first {
        let files = generator
            .generate(ctx.db(), ctx.config, order, ctx.tera, ctx.project)
            .await?;
        for file in &files {
            write_output(file)?;
        }
        report.files.extend(files);
    }

    let global_results: Vec<_> =
        futures::future::join_all(parallel.iter().map(|generator| {
            generator.generate(ctx.db(), ctx.config, order, ctx.tera, ctx.project)
        }))
        .await;

    for result in global_results {
        let files = result?;
        for file in &files {
            write_output(file)?;
        }
        report.files.extend(files);
    }

    Ok(())
}

/// Generate mod.rs files for all directories under src/.
/// Collects all .rs files and subdirs, writes `pub mod` declarations.
/// Always overwrites existing mod.rs UNLESS it contains `pub use`
/// (indicating it was written by a specialised generator like codelist).
///
/// In workers topology this runs per worker crate (workers/{domain}/src),
/// plus once on the root src/ tree for any root-anchored src output
/// (e.g. gRPC service code).  Monolith mode keeps the single root pass.
fn write_mod_files(
    ctx: &GeneratorContext<'_>,
    domains_with_entities: &[(String, Vec<String>)],
    report: &mut report::GenerationReport,
) -> Result<()> {
    let mod_file_dirs: Vec<std::path::PathBuf> = if ctx.workers_topology {
        let mut dirs: Vec<std::path::PathBuf> = domains_with_entities
            .iter()
            .map(|(domain, _)| ctx.output_dir.join("workers").join(domain).join("src"))
            .collect();
        dirs.push(ctx.output_dir.join("src"));
        dirs
    } else {
        vec![ctx.output_dir.join("src")]
    };
    for src_dir in &mod_file_dirs {
        let mod_files = generate_mod_files(src_dir)?;
        for file in &mod_files {
            write_output(file)?;
        }
        report.files.extend(mod_files);

        // Prune entity/mod.rs to only declare modules that are actually
        // referenced by repository code, eliminating thousands of dead-code
        // warnings from unused SeaORM entities.
        let entity_mod = prune_entity_mod(src_dir)?;
        if let Some(file) = entity_mod {
            write_output(&file)?;
            report.files.push(file);
        }
    }
    Ok(())
}

/// Emit integration-test glue (tests/<domain>/mod.rs + tests/tests.rs) so
/// cargo actually compiles the generated entity tests under tests/, then a
/// `.codegraph-manifest.json` at each output root listing every file written
/// this run (report.files mirrors every `write_output` call), merged with
/// any manifest already on disk. The pinned generator-source rev
/// (`project.cargo.codegraph_rev`) is recorded so drift/CI can reproduce the exact
/// checkout the committed tree was produced at.
fn emit_run_manifests(
    ctx: &GeneratorContext<'_>,
    manifest_roots: &[&Path],
    report: &mut report::GenerationReport,
) -> Result<()> {
    let test_mod_files = generate_test_mod_files(ctx.output_dir)?;
    for file in &test_mod_files {
        write_output(file)?;
    }
    report.files.extend(test_mod_files);

    manifest::emit_manifests(
        manifest_roots,
        &report.files,
        &ctx.project.cargo.codegraph_rev,
    )?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn run_ifml_generators(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    output_dir: &Path,
    tera: &Tera,
    ifml_frameworks: &[String],
    build_plan: Option<&crate::profile::BuildPlan>,
    project: &ProjectConfig,
    ifml_components: Option<&codegraph_config::IfmlComponentMappings>,
) -> Result<report::GenerationReport> {
    let cached_db = CachingQuerier::new(db);
    let db: &dyn GraphQuerier = &cached_db;

    let querier = ifml::IfmlGraphQuerier::new(db);
    let model = querier.get_ifml_model().await.map_err(Error::Graph)?;
    if model.view_containers.is_empty() {
        return Ok(report::GenerationReport::new());
    }

    // Active views are the complete set of view containers in the model —
    // stale route cleaning below removes anything not in this list.
    let active_views: Vec<String> = model
        .view_containers
        .iter()
        .map(|vc| vc.name.clone())
        .collect();

    let frameworks = if ifml_frameworks.is_empty() {
        vec!["svelte".to_string()]
    } else {
        ifml_frameworks.to_vec()
    };

    let plan_has_global =
        |name: &str| build_plan.is_none_or(|bp| bp.has_global_gen(&name.replace('-', "_")));

    let mut global_gens: Vec<Box<dyn GlobalGenerator>> = Vec::new();
    for fw in &frameworks {
        let fw_output = output_dir.join(fw);
        clean_stale_ifml_routes(&fw_output, fw, &active_views);
        if build_plan.is_none() || plan_has_global(&format!("ifml_skeleton_{fw}")) {
            global_gens.push(
                Box::new(ifml::skeleton::IfmlSkeletonGenerator::new(&fw_output, fw))
                    as Box<dyn GlobalGenerator>,
            );
        }
        if build_plan.is_none() || plan_has_global(&format!("ifml_route_{fw}")) {
            global_gens.push(Box::new(
                ifml::route_generator::IfmlRouteGenerator::new(&fw_output, fw)
                    .with_mappings(ifml_components.cloned()),
            ) as Box<dyn GlobalGenerator>);
        }
        if build_plan.is_none() || plan_has_global(&format!("ifml_navigation_{fw}")) {
            global_gens.push(
                Box::new(ifml::navigation_generator::IfmlNavigationGenerator::new(
                    &fw_output, fw,
                )) as Box<dyn GlobalGenerator>,
            );
        }
        if build_plan.is_none() || plan_has_global(&format!("ifml_e2e_test_{fw}")) {
            global_gens.push(Box::new(
                ifml::e2e_test::IfmlE2eTestGenerator::new(&fw_output, fw)
                    .with_mappings(ifml_components.cloned()),
            ) as Box<dyn GlobalGenerator>);
        }
    }

    // Scaffolding generators (sequential_first) run and write before the
    // parallel wave so later generators' if-absent checks (the e2e
    // generator's package.json stub) resolve in their favor.
    let (first, parallel): (Vec<_>, Vec<_>) = global_gens
        .iter()
        .partition(|generator| generator.sequential_first());
    let mut report = report::GenerationReport::new();
    for generator in &first {
        let files = generator.generate(db, config, &[], tera, project).await?;
        for file in &files {
            write_output(file)?;
        }
        report.files.extend(files);
    }

    let global_results: Vec<_> = futures::future::join_all(
        parallel
            .iter()
            .map(|generator| generator.generate(db, config, &[], tera, project)),
    )
    .await;

    for result in global_results {
        let files = result?;
        for file in &files {
            write_output(file)?;
        }
        report.files.extend(files);
    }

    Ok(report)
}
