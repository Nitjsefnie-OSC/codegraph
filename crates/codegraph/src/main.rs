use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use clap::Parser;

mod cli;

use codegraph_backend::{BackendConfig, create_backend};

#[tokio::main]
async fn main() -> codegraph::error::Result<()> {
    if let Ok(filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .try_init();
    }

    let cli = cli::Cli::parse();

    match cli.command {
        cli::Commands::Generate {
            config,
            output,
            extension_points,
            template_dir,
            ifml_framework,
            ifml_components,
            ifml_design_system,
            ux_rules,
            check,
        } => {
            codegraph::driver::generate(
                &config,
                &output,
                extension_points.as_deref(),
                &template_dir,
                &ifml_framework,
                ifml_components.as_deref(),
                ifml_design_system.as_deref(),
                ux_rules.as_deref(),
                check,
            )
            .await
        }
        cli::Commands::Migrate(args) => {
            codegraph::migrate::migrate(codegraph::migrate::MigrateArgs {
                schemas: &args.schemas,
                output: &args.output,
                dry_run: args.dry_run,
                force: args.force,
            })
            .map(|report| {
                println!(
                    "Migration summary: {} converted, {} needs review, {} unsupported",
                    report.converted,
                    report.needs_review.len(),
                    report.unsupported.len()
                );
            })
        }
        cli::Commands::Classify {
            schemas,
            classifier,
            config,
            domain,
            format,
            mox_files,
            rosetta_files,
        } => {
            let format = match format {
                cli::ClassifyFormat::Table => codegraph::driver::ClassifyFormat::Table,
                cli::ClassifyFormat::Json => codegraph::driver::ClassifyFormat::Json,
            };
            codegraph::driver::classify(
                schemas.as_deref(),
                classifier.as_deref(),
                &config,
                domain.as_deref(),
                format,
                &mox_files,
                &rosetta_files,
            )
            .await
        }
        cli::Commands::Run {
            schemas,
            classifier,
            config,
            output,
            extension_points,
            profile,
            variant,
            profiles_config,
            no_post_gen,
            template_dir,
            ifml_files,
            openapi_files,
            mox_files,
            rosetta_files,
            ddd_files,
            evt_files,
            deploy_files,
            ifml_framework,
            ifml_components,
            ifml_design_system,
            ux_rules,
            graph_cache,
            check,
        } => codegraph::driver::run_with_graph_cache(
            codegraph::driver::RunArgs {
                schemas: schemas.as_deref(),
                classifier: classifier.as_deref(),
                config_path: &config,
                output: &output,
                extension_points_path: extension_points.as_deref(),
                profile_name: &profile,
                variant: variant.as_deref(),
                profiles_config_path: profiles_config,
                no_post_gen,
                template_dir: &template_dir,
                ifml_files: &ifml_files,
                openapi_files: &openapi_files,
                mox_files: &mox_files,
                rosetta_files: &rosetta_files,
                ddd_files: &ddd_files,
                evt_files: &evt_files,
                deploy_files: &deploy_files,
                ifml_framework: &ifml_framework,
                ifml_components: ifml_components.as_deref(),
                ifml_design_system: ifml_design_system.as_deref(),
                ux_rules: ux_rules.as_deref(),
                codegraph_rev: None,
                check,
            },
            graph_cache.as_deref(),
        )
        .await
        .map(|_| ()),
        cli::Commands::IfmlGenerate {
            config,
            output,
            ifml_files,
            schemas,
            classifier,
            framework,
            profiles_config,
            template_dir,
            ifml_components,
            ifml_design_system,
            ux_rules,
        } => {
            codegraph::driver::ifml_generate(codegraph::driver::IfmlGenerateArgs {
                config_path: &config,
                output: &output,
                ifml_files: &ifml_files,
                schemas: schemas.as_deref(),
                classifier: classifier.as_deref(),
                frameworks: &framework,
                profiles_config_path: profiles_config,
                template_dir: &template_dir,
                ifml_components: ifml_components.as_deref(),
                ifml_design_system: ifml_design_system.as_deref(),
                ux_rules: ux_rules.as_deref(),
            })
            .await
        }
        cli::Commands::IfmlScaffold {
            schemas,
            classifier,
            config,
            output,
            force,
            domain,
        } => {
            codegraph::ifml_scaffold::ifml_scaffold(codegraph::ifml_scaffold::IfmlScaffoldArgs {
                schemas: &schemas,
                classifier: &classifier,
                config_path: &config,
                output: &output,
                force,
                domains: &domain,
            })
            .await
        }
        cli::Commands::IfmlDerive {
            from_svelte,
            output,
            name,
            force,
        } => codegraph::ifml_derive::ifml_derive(codegraph::ifml_derive::IfmlDeriveArgs {
            from_svelte: &from_svelte,
            output: &output,
            name: name.as_deref(),
            force,
        }),
        cli::Commands::Lsp {
            schemas,
            classifier,
            config,
            mox_files,
        } => {
            cmd_lsp(
                &schemas,
                classifier.as_deref(),
                config.as_deref(),
                &mox_files,
            )
            .await
        }
        cli::Commands::Init {
            name,
            output,
            domains,
            database_target,
            persistence_provider,
            deployment_topology,
            grpc,
            ifml,
            rosetta,
            no_ops,
            rev,
            codegraph_path,
            force,
            template_dir,
        } => {
            let args = codegraph::init::commands::InitArgs {
                name,
                output_dir: output.unwrap_or_else(|| PathBuf::from(".")),
                domains: domains.unwrap_or_else(|| vec!["common".to_string()]),
                database_target,
                persistence_provider,
                deployment_topology,
                grpc,
                ifml,
                ops: !no_ops,
                rosetta,
                rev,
                codegraph_path,
                force,
                template_dirs: template_dir,
            };
            codegraph::init::commands::cmd_init(&args)
        }
        cli::Commands::Doctor {
            config,
            schemas,
            classifier,
            profiles_config,
            mox_files,
            rosetta_files,
            ddd_files,
            evt_files,
            deploy_files,
            ifml_files,
        } => {
            let args = codegraph::init::commands::DoctorArgs {
                config,
                schemas,
                classifier,
                profiles_config,
                mox_files,
                rosetta_files,
                ddd_files,
                evt_files,
                deploy_files,
                ifml_files,
            };
            codegraph::init::commands::cmd_doctor(&args).map(|summary| {
                println!(
                    "doctor: {} hard failure(s), {} warning(s) ({} model warning(s))",
                    summary.hard_failures, summary.soft_warnings, summary.model_warnings
                );
            })
        }
        cli::Commands::Add { target } => match target {
            cli::AddTarget::Domain { name, rosetta } => codegraph::init::commands::cmd_add_domain(
                &PathBuf::from("domains.toml"),
                &name,
                rosetta,
            ),
        },
    }
}

async fn cmd_lsp(
    schema_dirs: &[PathBuf],
    classifier: Option<&Path>,
    config: Option<&Path>,
    mox_files: &[PathBuf],
) -> codegraph::error::Result<()> {
    use codegraph::lsp::{GrafeoState, SchemaInfo, init_mox, run_lsp_server};

    let backend_config = BackendConfig::default();
    let be = create_backend(&backend_config)
        .await
        .map_err(|e| codegraph::error::Error::Config(e.to_string()))?;

    // Load JSON Schema files into the graph
    for dir in schema_dirs {
        if dir.exists() {
            let empty_entities = std::collections::HashSet::new();
            let default_ui = codegraph_config::UiOverrideConfig::default();
            let default_suffix = "Type".to_string();
            let classifier_config = if let Some(classifier_path) = classifier {
                codegraph_classifier::config::parse_classifier_config(classifier_path)
                    .map_err(|e| codegraph::error::Error::Config(e.to_string()))?
            } else {
                codegraph_classifier::config::parse_classifier_config_str("")
                    .map_err(|e| codegraph::error::Error::Config(e.to_string()))?
            };

            codegraph::ingest::async_ingest::ingest_schemas(
                be.ingestor(),
                dir,
                &classifier_config,
                &empty_entities,
                &default_ui,
                &default_suffix,
            )
            .await?;
        }
    }

    // Run entity classification using AutoClassifier if domain config is provided.
    // This replaces the naive suffix-stripping with structural scoring + naming rules.
    let classifier_config = if let Some(classifier_path) = classifier {
        codegraph_classifier::config::parse_classifier_config(classifier_path)
            .map_err(|e| codegraph::error::Error::Config(e.to_string()))?
    } else {
        codegraph_classifier::config::parse_classifier_config_str("")
            .map_err(|e| codegraph::error::Error::Config(e.to_string()))?
    };

    let all_data = be
        .querier()
        .get_classification_data()
        .await
        .map_err(codegraph::error::Error::Graph)?;

    let classifier_types: HashSet<String> = classifier_config
        .primitive_wrappers
        .keys()
        .cloned()
        .chain(classifier_config.array_wrappers.keys().cloned())
        .chain(classifier_config.range_wrappers.keys().cloned())
        .chain(
            classifier_config
                .composite_wrappers
                .iter()
                .map(|cw| cw.schema.clone()),
        )
        .collect();

    let naming_rules = classifier_config.naming_rules.clone();
    let auto_classifier = codegraph::classify::AutoClassifier::new(classifier_types, naming_rules);

    // Build entity names by stripping the "Type" suffix from raw schema titles.
    // IFML references entities without the suffix (e.g. "Customer" not "CustomerType").
    // We ALSO try the AutoClassifier for more precise entity/VO classification,
    // but always include suffix-stripped names as a reliable fallback.
    let default_suffix = "Type";
    let mut entity_names_set: HashSet<String> = HashSet::new();

    // Always include suffix-stripped names for every loaded schema (reliable fallback)
    let schemas = be.querier().list_schemas(None).await?;
    for schema in &schemas {
        entity_names_set.insert(
            schema
                .title
                .strip_suffix(default_suffix)
                .unwrap_or(&schema.title)
                .to_string(),
        );
    }

    // Also try AutoClassifier if domain config is provided (more precise)
    if let Some(config_path) = config
        && let Ok(domain_config) = codegraph_config::config::parse_domain_config(config_path)
    {
        for (domain_name, domain_entry) in &domain_config.domains {
            let domain_schemas: Vec<_> = all_data
                .iter()
                .filter(|d| d.domain.as_deref() == Some(domain_name.as_str()))
                .cloned()
                .collect();
            let result =
                auto_classifier.classify_domain(domain_name, domain_entry, &domain_schemas);
            for score in &result.entities {
                let name = score
                    .title
                    .strip_suffix(default_suffix)
                    .unwrap_or(&score.title)
                    .to_string();
                entity_names_set.insert(name);
            }
            // Also include legacy explicit entities from domains.toml
            for entity in &domain_entry.entities {
                entity_names_set.insert(entity.clone());
            }
        }
    }

    let entity_names: Vec<String> = entity_names_set.into_iter().collect();

    // Build schema_infos keyed by entity name for synchronous LSP access
    let schemas = be.querier().list_schemas(None).await?;
    let mut schema_infos = HashMap::new();
    for schema in &schemas {
        let entity_name = schema
            .title
            .strip_suffix(default_suffix)
            .unwrap_or(&schema.title)
            .to_string();
        if let Ok(props) = be.querier().get_properties(&schema.title).await {
            schema_infos.insert(
                entity_name,
                SchemaInfo {
                    title: schema.title.clone(),
                    description: schema.description.clone(),
                    properties: props.iter().map(|p| p.name.clone()).collect(),
                    rel_path: schema.rel_path.clone(),
                },
            );
        }
    }

    let grafeo_state = GrafeoState {
        entity_names,
        schema_infos,
        schema_dirs: schema_dirs.to_vec(),
    };

    // Mox model for .mox diagnostics/completions. Schema titles feed the
    // `import schema` alias resolution (the wire_alias_refs replay: exact
    // title, then title + type suffix).
    if mox_files.is_empty() {
        init_mox(None);
    } else {
        let schema_titles: HashSet<String> = grafeo_state
            .schema_infos
            .values()
            .map(|s| s.title.clone())
            .collect();
        let mox_state = codegraph::lsp::mox::build_mox_state(mox_files, &schema_titles, "Type");
        init_mox(Some(mox_state));
    }

    eprintln!("codegraph LSP server starting (IFML language)...");
    let (connection, _io_threads) = auto_lsp::lsp_server::Connection::stdio();
    run_lsp_server(connection, grafeo_state)
        .map_err(|e| codegraph::error::Error::Config(e.to_string()))?;

    Ok(())
}
