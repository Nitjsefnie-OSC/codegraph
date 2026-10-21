use crate::ProjectConfig;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;
use serde::Serialize;

use crate::GenerationEntry;
use crate::api::api_model::resolve_entity_operations;
use crate::error::Result;
use crate::render_template_with_project;
use crate::traits::{GeneratedFile, GlobalGenerator, GlobalGeneratorKind};
use codegraph_config::DomainConfig;

#[derive(Debug, Serialize)]
pub struct ScaffoldContext {
    pub app_name: String,
    pub domains: Vec<ScaffoldDomain>,
    /// Schemas the generated API role (`app_user`) needs DML access to:
    /// every configured domain's postgres schema plus `common` (codelists)
    /// and the `platform` infra schema. Rendered into the grant migrations.
    pub grant_schemas: Vec<String>,
    pub codegraph_workflow_path: String,
    pub type_contracts_path: String,
    pub domain_types_path: String,
    pub hooks_api_path: String,
    pub extensions_path: String,
    pub app_config_path: String,
    pub decision_engine_path: String,
    pub has_webhooks: bool,
    pub has_reports: bool,
    pub has_grpc: bool,
    pub has_atproto: bool,
    pub has_fern: bool,
    pub has_cli: bool,
    pub has_test_gen: bool,
    pub has_auth_rate_limit: bool,
    pub has_admin_cli: bool,
    /// Whether the labels substrate (atproto.label table + SubscribeLabels
    /// consumer + selfLabels bridge) is enabled (issue #33).
    pub has_labels: bool,
    /// Whether the `seed_provision` capability is active — gates the
    /// `hr-seed` dependency and the `seed` `[[bin]]` entry in Cargo.toml
    /// plus `pub mod seed;` in lib.rs.
    pub has_seed: bool,
    /// Whether the graph carries a semantic event architecture (.evt,
    /// issues #454/#455) — gates `pub mod events;` in lib.rs and the
    /// `anyhow` dependency the events consumers use. Skipped from the
    /// serialized context when false, keeping every template rendering
    /// byte-identical on flag-off runs.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub has_events: bool,
    /// Resolved path (relative to the output dir when possible) of the
    /// `hr-seed` crate, discovered by walking up from the output dir and
    /// CWD looking for a sibling `hr-seed` directory.
    pub seed_crate_path: String,
    pub migration_strategy: String,
}

#[derive(Debug, Serialize)]
pub struct ScaffoldEntity {
    pub name: String,
    pub module_name: String,
    pub domain: String,
    /// Physical table name in Postgres (from the graph, e.g. "case").
    pub table_name: String,
    /// Whether any command operations (create/update/delete) are enabled for
    /// this entity — hook-only entities get no command handler usage, so the
    /// AppState field would otherwise be dead code.
    pub has_commands: bool,
    /// True when the entity's operations exclude update AND delete (append-only
    /// event streams): app_user must not be granted UPDATE/DELETE on the table.
    pub append_only: bool,
    /// Whether the generated query handler takes a hooks argument (mirrors the
    /// `uses_find_by_id` condition in ddd/query.tera).
    pub has_query_hooks: bool,
}

#[derive(Debug, Serialize)]
pub struct ScaffoldDomain {
    pub name: String,
    pub label: String,
    pub postgres_schema: String,
    pub entities: Vec<ScaffoldEntity>,
    /// True for entity-less domains flagged `custom_routes` — the domain gets
    /// a router scaffold + (workers topology) a worker crate/gateway binding
    /// even though it has zero entities. The server template nests such
    /// domains at `/api/{version}/{name}` even with an empty entity list.
    pub has_custom_routes: bool,
    /// The `community.os.*` (or authority-scoped) NSIDs of the domain's
    /// portable-record entities, in generation order. Used by server.tera to
    /// build Jetstream `?wantedCollections=` filters per domain consumer.
    pub atproto_nsids: Vec<String>,
}

pub struct ScaffoldGenerator {
    output_dir: PathBuf,
    has_webhooks: bool,
    has_reports: bool,
    has_grpc: bool,
    has_atproto: bool,
    has_fern: bool,
    has_cli: bool,
    has_test_gen: bool,
    has_auth_rate_limit: bool,
    has_admin_cli: bool,
    has_labels: bool,
    has_seed: bool,
    migration_strategy: String,
}

impl ScaffoldGenerator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        output_dir: &Path,
        has_webhooks: bool,
        has_reports: bool,
        has_grpc: bool,
        has_atproto: bool,
        has_cli: bool,
        has_test_gen: bool,
        has_fern: bool,
        has_auth_rate_limit: bool,
        has_admin_cli: bool,
        has_labels: bool,
        migration_strategy: &str,
    ) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
            has_webhooks,
            has_reports,
            has_grpc,
            has_atproto,
            has_fern,
            has_cli,
            has_test_gen,
            has_auth_rate_limit,
            has_admin_cli,
            has_labels,
            has_seed: false,
            migration_strategy: migration_strategy.to_string(),
        }
    }

    /// Enable seed-provisioning output (hr-seed dependency, `seed` `[[bin]]`,
    /// `pub mod seed;`). Opt-in via the `seed_provision` capability.
    pub fn with_seed(mut self, has_seed: bool) -> Self {
        self.has_seed = has_seed;
        self
    }
}

/// Resolve a base path (relative to CWD or absolute) to a path relative
/// to the output directory. Returns empty string if `base` is empty.
pub(crate) fn resolve_path(base: &str, abs_output: &Path) -> String {
    if base.is_empty() {
        return String::new();
    }
    let abs_base = abs_output.join(base);
    pathdiff::diff_paths(&abs_base, abs_output)
        .unwrap_or_else(|| PathBuf::from(base))
        .to_string_lossy()
        .into_owned()
}

/// Locate the `hr-seed` crate for the `seed_provision` capability: walk up
/// from the output directory (then from CWD) looking for a sibling `hr-seed`
/// directory with a Cargo.toml, and express it relative to the output when
/// possible. Falls back to the hr-specs workspace layout convention
/// (`../../hr-seed`) when nothing is found.
pub(crate) fn resolve_seed_crate_path(abs_output: &Path) -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    for start in [abs_output.to_path_buf(), cwd] {
        let mut dir: Option<&Path> = Some(start.as_path());
        while let Some(d) = dir {
            let candidate = d.join("hr-seed");
            if candidate.join("Cargo.toml").exists() {
                if let Some(rel) = pathdiff::diff_paths(&candidate, abs_output) {
                    let rel_str = rel.to_string_lossy().into_owned();
                    // Deep `../` climbs resolve but produce unreadable
                    // manifests (e.g. /tmp outputs) — prefer absolute there.
                    let ups = rel
                        .components()
                        .take_while(|c| c.as_os_str() == "..")
                        .count();
                    if ups <= 2 {
                        return rel_str;
                    }
                }
                return candidate.to_string_lossy().into_owned();
            }
            dir = d.parent();
        }
    }
    "../../hr-seed".to_string()
}

/// Group the generation order into per-domain scaffold domains (entities with
/// resolved operation flags), exactly as the monolith `ScaffoldGenerator` does.
///
/// Shared with the workers-topology `WorkerScaffoldGenerator` so both
/// topologies produce the same domain groupings. Only domains that actually
/// have entities in the generation order are returned.
pub async fn build_scaffold_domains(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    generation_order: &[GenerationEntry],
) -> Vec<ScaffoldDomain> {
    let mut domain_entity_map: std::collections::HashMap<String, Vec<ScaffoldEntity>> =
        std::collections::HashMap::new();
    let mut domain_nsids: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut seen_scaffold_entities = std::collections::HashSet::new();
    for entry in generation_order {
        let stripped = config.defaults.strip_suffix(&entry.schema_title);
        // Titles may contain spaces (e.g. "Review Decision") — sanitize to
        // PascalCase so generated Rust identifiers compile. Prefer the graph's
        // canonical rust_type_name so emitted identifiers match hr-domain-types
        // exactly (acronym titles like "...LER-RSType" -> "...LERRS...").
        let type_name = db
            .get_schema_in_domain(&entry.schema_title, &entry.domain)
            .await
            .ok()
            .flatten()
            .map(|s| s.rust_type_name)
            .unwrap_or_else(|| codegraph_naming::to_pascal_case(&stripped));
        let entity_name = codegraph_naming::to_pascal_case(&stripped);
        let module_name = codegraph_naming::to_snake_case(&stripped);
        // Dedup by (domain, module_name) to prevent cross-domain name collisions
        if !seen_scaffold_entities.insert((entry.domain.clone(), module_name.clone())) {
            continue;
        }
        if let Ok(Some(lexicon)) = db.get_lexicon_by_schema(&entry.schema_title).await {
            domain_nsids
                .entry(entry.domain.clone())
                .or_default()
                .push(lexicon.nsid.clone());
        }
        let operations = resolve_entity_operations(db, config, &entry.domain, &entity_name).await;
        let has_commands = operations
            .iter()
            .any(|op| op == "create" || op == "update" || op == "delete");
        let has_create = operations.iter().any(|op| op == "create");
        let has_read = operations.iter().any(|op| op == "read");
        let has_update = operations.iter().any(|op| op == "update");
        let has_delete = operations.iter().any(|op| op == "delete");
        let table_name = db
            .get_schema_in_domain(&stripped, &entry.domain)
            .await
            .ok()
            .flatten()
            .map(|s| s.pg_table_name)
            .unwrap_or_else(|| module_name.clone());
        let has_config_parent = config
            .domains
            .get(&entry.domain)
            .and_then(|d| d.get_entity_config(&entity_name))
            .and_then(|ec| ec.parent_ref.as_ref())
            .is_some();
        let has_query_hooks = has_create || (has_read && !has_config_parent);
        domain_entity_map
            .entry(entry.domain.clone())
            .or_default()
            .push(ScaffoldEntity {
                module_name: module_name.clone(),
                name: type_name,
                domain: entry.domain.clone(),
                table_name,
                has_commands,
                append_only: !has_update && !has_delete,
                has_query_hooks,
            });
    }

    let mut domains: Vec<ScaffoldDomain> = config
        .domains
        .iter()
        .filter_map(|(name, entry)| {
            // Domains with entities in the generation order get their scaffold
            // entities; entity-less custom-routes domains are included with an
            // empty entity list so the monolith nests their router and the
            // workers scaffold emits a worker crate + gateway binding.
            let entities = match domain_entity_map.remove(name.as_str()) {
                Some(entities) => entities,
                None if entry.custom_routes => Vec::new(),
                None => return None,
            };
            Some(ScaffoldDomain {
                name: name.clone(),
                label: entry.label.clone(),
                postgres_schema: entry.postgres_schema.clone(),
                entities,
                has_custom_routes: entry.custom_routes,
                atproto_nsids: domain_nsids.remove(name.as_str()).unwrap_or_default(),
            })
        })
        .collect();
    domains.sort_by(|a, b| a.name.cmp(&b.name));
    domains
}

#[async_trait]
impl GlobalGenerator for ScaffoldGenerator {
    fn kind(&self) -> GlobalGeneratorKind {
        GlobalGeneratorKind::Scaffold
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        config: &DomainConfig,
        generation_order: &[GenerationEntry],
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        let domains = build_scaffold_domains(db, config, generation_order).await;

        // Compute absolute output dir (shared by all path calculations)
        let abs_output = if self.output_dir.is_absolute() {
            self.output_dir.clone()
        } else {
            std::env::current_dir()
                .expect("current_dir should be accessible")
                .join(&self.output_dir)
        };

        let codegraph_workflow_path =
            resolve_path(&project.paths.codegraph_workflow_base, &abs_output);
        let type_contracts_path = resolve_path(&project.paths.type_contracts_base, &abs_output);
        let domain_types_path = resolve_path(&project.paths.domain_types_base, &abs_output);
        let hooks_api_path = resolve_path(&project.paths.hooks_api_base, &abs_output);
        let extensions_path = resolve_path(&project.paths.extensions_base, &abs_output);
        let app_config_path = resolve_path(&project.paths.app_config_base, &abs_output);
        let decision_engine_path = resolve_path(&project.paths.decision_engine_base, &abs_output);
        let seed_crate_path = resolve_seed_crate_path(&abs_output);

        // Physical Postgres schemas that need app_user grants: every domain
        // schema plus "common" (codelists) and the platform infra schema.
        // Sorted + deduped for stable SQL.
        let mut grant_schemas: Vec<String> = domains
            .iter()
            .map(|d| d.postgres_schema.clone())
            .chain(["common".to_string(), "platform".to_string()])
            .collect();
        grant_schemas.sort();
        grant_schemas.dedup();

        // Semantic event architecture (.evt, issues #454/#455): the flag
        // is GRAPH-driven, not build-plan-driven — the `evt_events`
        // generator registers unconditionally and emits the `src/events/`
        // module only when the architecture is non-empty, so lib.rs must
        // declare it under exactly the same condition.
        let has_events = !crate::evt_events::architecture_for(db, project, config)
            .await?
            .is_empty();

        let ctx = ScaffoldContext {
            app_name: project.identity.app_name.clone(),
            domains,
            grant_schemas,
            codegraph_workflow_path,
            type_contracts_path,
            domain_types_path,
            hooks_api_path,
            extensions_path,
            app_config_path,
            decision_engine_path,
            has_webhooks: self.has_webhooks,
            has_reports: self.has_reports,
            has_grpc: self.has_grpc,
            has_atproto: self.has_atproto,
            has_fern: self.has_fern,
            has_cli: self.has_cli,
            has_test_gen: self.has_test_gen,
            has_auth_rate_limit: self.has_auth_rate_limit,
            has_admin_cli: self.has_admin_cli,
            has_labels: self.has_labels,
            has_seed: self.has_seed,
            has_events,
            seed_crate_path,
            migration_strategy: self.migration_strategy.clone(),
        };

        let mut files = Vec::new();

        let main_rs = render_template_with_project(tera, "scaffold/main.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: self.output_dir.join("src").join("main.rs"),
            content: main_rs,
        });

        let server_rs = render_template_with_project(tera, "scaffold/server.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: self.output_dir.join("src").join("server.rs"),
            content: server_rs,
        });

        if self.has_admin_cli {
            let config_rs =
                render_template_with_project(tera, "scaffold/config.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: self.output_dir.join("src").join("config.rs"),
                content: config_rs,
            });

            let doctor_rs =
                render_template_with_project(tera, "scaffold/doctor.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: self.output_dir.join("src").join("doctor.rs"),
                content: doctor_rs,
            });

            let migration_rs =
                render_template_with_project(tera, "scaffold/migration.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: self.output_dir.join("src").join("migration.rs"),
                content: migration_rs,
            });
        }

        let app_state =
            render_template_with_project(tera, "scaffold/app_state.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: self.output_dir.join("src").join("app_state.rs"),
            content: app_state,
        });

        let cargo_toml =
            render_template_with_project(tera, "scaffold/cargo_toml.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: self.output_dir.join("Cargo.toml"),
            content: cargo_toml,
        });

        let build_rs = render_template_with_project(tera, "scaffold/build_rs.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: self.output_dir.join("build.rs"),
            content: build_rs,
        });

        let lib_rs = render_template_with_project(tera, "scaffold/lib.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: self.output_dir.join("src").join("lib.rs"),
            content: lib_rs,
        });

        // Cornucopia client plumbing for the monolith: the per-domain worker
        // scaffold emits its own `db_client.rs` (see worker.rs); the monolith
        // needs the same ClientSource/DbTx abstraction at the crate root when
        // the cornucopia repositories are selected. `db_client.tera` only
        // interpolates `worker_name` / `name` in comments, so a synthetic
        // context is sufficient.
        if project.is_cornucopia() {
            let db_client = render_template_with_project(
                tera,
                "scaffold/db_client.tera",
                &serde_json::json!({
                    "worker_name": project.identity.app_name,
                    "name": "monolith",
                }),
                project,
            )?;
            files.push(GeneratedFile {
                path: self.output_dir.join("src").join("db_client.rs"),
                content: db_client,
            });
        }

        // Lifecycle-hook handle adapters (issue #509): adapt the provider's
        // native transaction/connection handles to the hooks crate's
        // persistence-agnostic SPI. Emitted whenever the hooks surface is
        // configured, both providers.
        if !project.identity.hooks_api_crate.is_empty() {
            let hook_adapters =
                render_template_with_project(tera, "scaffold/hook_adapters.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: self.output_dir.join("src").join("hook_adapters.rs"),
                content: hook_adapters,
            });
        }

        let error_rs = render_template_with_project(tera, "scaffold/error.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: self.output_dir.join("src").join("error.rs"),
            content: error_rs,
        });

        let middleware_rs =
            render_template_with_project(tera, "scaffold/middleware.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: self
                .output_dir
                .join("src")
                .join("middleware")
                .join("mod.rs"),
            content: middleware_rs,
        });

        let permission_rs = render_template_with_project(
            tera,
            "scaffold/permission_middleware.tera",
            &ctx,
            project,
        )?;
        files.push(GeneratedFile {
            path: self
                .output_dir
                .join("src")
                .join("middleware")
                .join("permission.rs"),
            content: permission_rs,
        });

        let metrics_middleware_rs =
            render_template_with_project(tera, "scaffold/metrics_middleware.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: self.output_dir.join("src").join("metrics_middleware.rs"),
            content: metrics_middleware_rs,
        });

        let qs_query_rs =
            render_template_with_project(tera, "scaffold/qs_query.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: self.output_dir.join("src").join("qs_query.rs"),
            content: qs_query_rs,
        });

        let meta_content = render_template_with_project(
            tera,
            "scaffold/meta.tera",
            &serde_json::json!({}),
            project,
        )?;
        files.push(GeneratedFile {
            path: self.output_dir.join("src").join("api").join("meta.rs"),
            content: meta_content,
        });

        // API-key scope enforcement is DB-level (#169): the RESTRICTIVE
        // scope_enforced_* RLS policies raise P0403 INSUFFICIENT_SCOPE and
        // the generated error mapper turns that into HTTP 403. The former
        // per-route scope guard (api/scope.tera) is retired.

        let integrations_rs = render_template_with_project(
            tera,
            "scaffold/integrations_handler.tera",
            &ctx,
            project,
        )?;
        files.push(GeneratedFile {
            path: self.output_dir.join("src").join("integrations.rs"),
            content: integrations_rs,
        });

        let api_key_migration =
            render_template_with_project(tera, "db/api_key_migration.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: crate::db::migrations_root(&self.output_dir).join("0002_api_key_management.sql"),
            content: api_key_migration,
        });

        // SeaORM migration crate: applies the raw SQL files in migrations/ via
        // the Migrator (run with `sea-orm-cli migrate up` or `cargo run
        // --manifest-path migration/Cargo.toml -- up`), and on app boot.
        let migration_cargo_toml = render_template_with_project(
            tera,
            "scaffold/migration_cargo_toml.tera",
            &ctx,
            project,
        )?;
        files.push(GeneratedFile {
            path: self.output_dir.join("migration").join("Cargo.toml"),
            content: migration_cargo_toml,
        });

        let migration_lib =
            render_template_with_project(tera, "scaffold/migration_lib.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: self.output_dir.join("migration").join("src").join("lib.rs"),
            content: migration_lib,
        });

        let migration_main =
            render_template_with_project(tera, "scaffold/migration_main.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: self
                .output_dir
                .join("migration")
                .join("src")
                .join("main.rs"),
            content: migration_main,
        });

        let migration_migrator =
            render_template_with_project(tera, "scaffold/migration_migrator.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: self
                .output_dir
                .join("migration")
                .join("src")
                .join("migrator.rs"),
            content: migration_migrator,
        });

        if self.has_fern {
            let dump_openapi =
                render_template_with_project(tera, "scaffold/dump_openapi.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: self
                    .output_dir
                    .join("src")
                    .join("bin")
                    .join("dump_openapi.rs"),
                content: dump_openapi,
            });
        }

        // Late-binding app_user grants: 0002 runs before entity DDL, so its
        // IF EXISTS-guarded grants never fire on a fresh database. This
        // migration sorts after every codelist/entity band and derives its
        // schema list + append-only revokes from the project config.
        let app_user_grants =
            render_template_with_project(tera, "db/app_user_grants.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: self
                .output_dir
                .join("migrations")
                .join("9000_app_user_grants.sql"),
            content: app_user_grants,
        });

        // Late-binding app_user grants: 0002 runs before entity DDL, so its
        // IF EXISTS-guarded grants never fire on a fresh database. This
        // migration sorts after every codelist/entity band and derives its
        // schema list + append-only revokes from the project config.
        let app_user_grants =
            render_template_with_project(tera, "db/app_user_grants.tera", &ctx, project)?;
        files.push(GeneratedFile {
            path: self
                .output_dir
                .join("migrations")
                .join("9000_app_user_grants.sql"),
            content: app_user_grants,
        });

        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> ScaffoldContext {
        ScaffoldContext {
            app_name: "onboarding-app".to_string(),
            grant_schemas: vec![
                "common".to_string(),
                "compliance".to_string(),
                "core".to_string(),
            ],
            domains: vec![ScaffoldDomain {
                name: "core".to_string(),
                label: "Core".to_string(),
                postgres_schema: "core".to_string(),
                entities: vec![ScaffoldEntity {
                    name: "CaseStatusChanged".to_string(),
                    module_name: "case_status_changed".to_string(),
                    domain: "core".to_string(),
                    table_name: "case_status_changed".to_string(),
                    has_commands: true,
                    append_only: true,
                    has_query_hooks: true,
                }],
                has_custom_routes: false,
                atproto_nsids: vec![],
            }],
            codegraph_workflow_path: String::new(),
            type_contracts_path: String::new(),
            domain_types_path: String::new(),
            hooks_api_path: String::new(),
            extensions_path: String::new(),
            app_config_path: String::new(),
            decision_engine_path: String::new(),
            has_webhooks: false,
            has_reports: false,
            has_grpc: false,
            has_atproto: false,
            has_fern: false,
            has_cli: false,
            has_test_gen: false,
            has_auth_rate_limit: false,
            has_admin_cli: false,
            has_labels: false,
            has_seed: false,
            has_events: false,
            seed_crate_path: String::new(),
            migration_strategy: "sea-orm".to_string(),
        }
    }

    fn tera() -> tera::Tera {
        crate::template_engine::create_tera(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates"),
        )
        .unwrap()
    }

    #[test]
    fn api_key_migration_schema_list_is_dynamic() {
        let sql = render_template_with_project(
            &tera(),
            "db/api_key_migration.tera",
            &test_ctx(),
            &crate::ProjectConfig::default(),
        )
        .unwrap();
        assert!(sql.contains("'compliance'"), "dynamic schema list");
        assert!(sql.contains("'core'"));
        assert!(
            !sql.contains("'recruiting'"),
            "hardcoded HR schema list must be gone"
        );
    }

    #[test]
    fn app_user_grants_cover_schemas_and_pgmq() {
        let sql = render_template_with_project(
            &tera(),
            "db/app_user_grants.tera",
            &test_ctx(),
            &crate::ProjectConfig::default(),
        )
        .unwrap();
        for schema in ["common", "compliance", "core"] {
            assert!(
                sql.contains(&format!("'{}'", schema)),
                "missing schema {schema} in grant_schemas array"
            );
        }
        assert!(sql.contains("GRANT USAGE ON SCHEMA %I TO app_user"));
        assert!(sql.contains("GRANT USAGE ON SCHEMA pgmq TO app_user"));
        assert!(sql.contains("pgmq.q_events_core"));
    }

    #[test]
    fn app_user_grants_revoke_mutation_for_append_only() {
        let sql = render_template_with_project(
            &tera(),
            "db/app_user_grants.tera",
            &test_ctx(),
            &crate::ProjectConfig::default(),
        )
        .unwrap();
        assert!(sql.contains("REVOKE UPDATE, DELETE ON core.case_status_changed FROM app_user"));
    }
}
