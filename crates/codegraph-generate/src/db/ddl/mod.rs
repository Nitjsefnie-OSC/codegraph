use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;

use crate::ProjectConfig;
use crate::db::dialect::{DatabaseTarget, SqlDialect, db_template_for, dialect_for_target};
use crate::error::{Error, Result};
use crate::render_template_with_project;
use crate::traits::{EntityGenerator, EntityGeneratorKind, GeneratedFile};
use codegraph_config::DomainConfig;

mod accumulators;
mod query;
mod types;

pub use types::{
    CheckConstraint, ChildTableDef, ColumnComment, ColumnDef, DdlContext, EmbeddingContext,
    ForeignKeyDef, FtsColumnWeight, FtsContext, IndexDef, RoleMinimum,
};

pub(crate) use query::{child_parent_fk_column, column_info_to_ddl};

use accumulators::quote_ddl_identifiers;

#[cfg(test)]
use codegraph_core::types::{ColumnInfo, CompositionNode};
#[cfg(test)]
use query::{
    apply_workflow_defaults, composition_node_to_child_table, detect_extensions_from_columns,
};
#[cfg(test)]
use std::collections::HashSet;

#[cfg(test)]
mod tests;

pub struct DdlGenerator {
    output_dir: PathBuf,
    parent_candidates: Vec<codegraph_core::types::ParentCandidate>,
    dialect: Box<dyn SqlDialect>,
}

impl DdlGenerator {
    pub fn new(output_dir: &Path) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
            parent_candidates: Vec::new(),
            dialect: dialect_for_target(DatabaseTarget::Postgres),
        }
    }

    pub fn with_dialect(mut self, dialect: Box<dyn SqlDialect>) -> Self {
        self.dialect = dialect;
        self
    }

    pub fn with_parent_candidates(
        mut self,
        candidates: Vec<codegraph_core::types::ParentCandidate>,
    ) -> Self {
        self.parent_candidates = candidates;
        self
    }
}

/// Build a minimal [`DdlContext`] targeting a child table so the RLS template
/// emits the parent's org-isolation policies for it. Only org-isolation is
/// emitted (`is_auditable = false`): child tables are not api-key-scoped
/// resources.
pub(crate) fn child_table_rls_context(parent: &DdlContext, child: &ChildTableDef) -> DdlContext {
    DdlContext {
        schema_name: child.schema_name.clone(),
        table_name: child.table_name.clone(),
        display_name: child.display_name.clone(),
        domain: parent.domain.clone(),
        columns: child.columns.clone(),
        primary_key: "id".to_string(),
        foreign_keys: child.foreign_keys.clone(),
        check_constraints: child.check_constraints.clone(),
        indexes: Vec::new(),
        has_updated_at: false,
        is_tenant_scoped: true,
        tenant_table: parent.tenant_table.clone(),
        extensions: Vec::new(),
        child_tables: Vec::new(),
        comments: child.comments.clone(),
        has_workflow: false,
        resource_name: child.table_name.replace('_', "-"),
        fts: None,
        embeddings: Vec::new(),
        is_auditable: false,
        role_enforced: false,
        role_minima: Vec::new(),
        roles_hierarchy: Vec::new(),
        user_scope_column: None,
        is_codelist: false,
        has_demo_flag: false,
        append_only: parent.append_only,
    }
}

/// Post-process column types and defaults through the dialect.
/// Converts PG types to dialect-appropriate types (e.g. UUID → TEXT for SQLite)
/// and wraps default expressions (e.g. strips ::type casts, removes gen_random_uuid()).
///
/// Also validates every column type via
/// [`SqlDialect::validate_column_type`](crate::db::dialect::SqlDialect) so an
/// unrepresentable type is a generation error instead of invalid DDL.
fn apply_dialect_type_mapping(dialect: &dyn SqlDialect, ctx: &mut DdlContext) -> Result<()> {
    let mut errors = Vec::new();
    for col in &mut ctx.columns {
        if let Err(type_err) = dialect.validate_column_type(&col.pg_type) {
            errors.push(format!(
                "table `{}`: column `{}`: {type_err}",
                ctx.table_name, col.name
            ));
        }
        let original_type = col.pg_type.clone();
        if let Some(mapped) = dialect.map_pg_type(&original_type) {
            col.pg_type = mapped;
        }
        if let Some(default) = col.default.take() {
            let wrapped = dialect.wrap_default(&default, &original_type);
            if !wrapped.is_empty() {
                col.default = Some(wrapped);
            }
        }
    }
    for child in &mut ctx.child_tables {
        for col in &mut child.columns {
            if let Err(type_err) = dialect.validate_column_type(&col.pg_type) {
                errors.push(format!(
                    "table `{}`: column `{}`: {type_err}",
                    child.table_name, col.name
                ));
            }
            let original_type = col.pg_type.clone();
            if let Some(mapped) = dialect.map_pg_type(&original_type) {
                col.pg_type = mapped;
            }
            if let Some(default) = col.default.take() {
                let wrapped = dialect.wrap_default(&default, &original_type);
                if !wrapped.is_empty() {
                    col.default = Some(wrapped);
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::Validation(format!(
            "DDL contains column types the `{}` dialect cannot represent:\n  - {}",
            dialect.name(),
            errors.join("\n  - ")
        )))
    }
}

#[async_trait]
impl EntityGenerator for DdlGenerator {
    fn kind(&self) -> EntityGeneratorKind {
        EntityGeneratorKind::Ddl
    }

    fn supported_targets(&self) -> Option<Vec<DatabaseTarget>> {
        Some(vec![DatabaseTarget::Postgres, DatabaseTarget::Sqlite])
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        schema_title: &str,
        domain: &str,
        config: &DomainConfig,
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        let mut ctx = self
            .query_ddl_context(db, schema_title, domain, config)
            .await?;

        // Skip non-entity schemas (codelists handled separately). The
        // trigger-bearing-table view is the ONE enumeration shared with
        // crate::events (the semantic event model's implicit per-domain
        // channels publish exactly this set) — an empty table_name means
        // no event trigger and no publication.
        if crate::events::trigger_publication_from_context(&ctx)
            .table_name
            .is_empty()
        {
            return Ok(Vec::new());
        }

        // Apply dialect-specific type mapping, default wrapping, and
        // per-dialect column type validation
        apply_dialect_type_mapping(&*self.dialect, &mut ctx)?;

        // A composition child whose parent's `{parent}_{feature}` child
        // table COLLIDES with this schema's own pg_table_name renders the
        // parent's child-table shape as its standalone migration (cornucopia
        // 's build-time SQL validation type-checks every query against the
        // LIVE database; a bare standalone shape would shadow the projection
        // under `IF NOT EXISTS`). Augment the child's own context with what
        // the projection adds — the physical parent-FK column and
        // `updated_at` — but NOT the constraints: the parent's table does
        // not exist in this band, and the parent's own migration adds the
        // FK/index to the now-existing table. Both files exist (the
        // #460/equivalence contracts); both agree on shape. Non-colliding
        // contains-only VOs keep BOTH surfaces untouched.
        if let Some((parent_title, projected)) =
            query::composition_table_conflict(db, schema_title).await?
        {
            let parent_ctx = self
                .query_ddl_context(db, &parent_title, domain, config)
                .await?;
            let child_def = parent_ctx
                .child_tables
                .iter()
                .find(|c| c.table_name == projected)
                .ok_or_else(|| {
                    crate::error::Error::Config(format!(
                        "composition conflict: parent {parent_title} projects child table \
                         {projected} but carries no such child def"
                    ))
                })?;
            let fk = child_def.parent_fk_column.clone();
            if !ctx.columns.iter().any(|c| c.name == fk) {
                ctx.columns.push(ColumnDef {
                    name: fk,
                    pg_type: "UUID".to_string(),
                    nullable: false,
                    default: None,
                    is_primary_key: false,
                    is_array: false,
                });
            }
            if !ctx.columns.iter().any(|c| c.name == "updated_at") {
                ctx.columns.push(ColumnDef {
                    name: "updated_at".to_string(),
                    pg_type: "TIMESTAMPTZ".to_string(),
                    nullable: false,
                    default: Some("now()".to_string()),
                    is_primary_key: false,
                    is_array: false,
                });
            }
            ctx.has_updated_at = true;
        }

        // Quote PostgreSQL reserved words in column names (PG-specific, harmless pass-through for other dialects)
        quote_ddl_identifiers(&mut ctx);

        let mut files = Vec::new();

        // Main table DDL — dialect-aware template path
        let table_sql = render_template_with_project(
            tera,
            &db_template_for(&*self.dialect, "table"),
            &ctx,
            project,
        )?;
        files.push(GeneratedFile {
            path: crate::db::migrations_root(&self.output_dir)
                .join(format!("{}_{}.sql", ctx.schema_name, ctx.table_name)),
            content: table_sql,
        });

        // RLS policy — only on tenant-scoped tables with RLS support
        if ctx.is_tenant_scoped && self.dialect.has_rls() {
            let rls_sql = render_template_with_project(
                tera,
                &db_template_for(&*self.dialect, "rls"),
                &ctx,
                project,
            )?;
            files.push(GeneratedFile {
                path: crate::db::migrations_root(&self.output_dir)
                    .join(format!("{}_{}_rls.sql", ctx.schema_name, ctx.table_name)),
                content: rls_sql,
            });

            // Child tables (junction + VO) get the same org-isolation
            // policies — without them, junction rows are readable
            // cross-tenant via direct DB access. Children are not
            // api-key-scoped resources, so only the org-isolation block
            // is emitted (is_auditable = false). ctx.child_tables is the
            // depth-flattened tree, so nested children are covered.
            for child in &ctx.child_tables {
                let child_ctx = child_table_rls_context(&ctx, child);
                let child_rls_sql = render_template_with_project(
                    tera,
                    &db_template_for(&*self.dialect, "rls"),
                    &child_ctx,
                    project,
                )?;
                files.push(GeneratedFile {
                    path: crate::db::migrations_root(&self.output_dir).join(format!(
                        "{}_{}_rls.sql",
                        child_ctx.schema_name, child_ctx.table_name
                    )),
                    content: child_rls_sql,
                });
            }
        }

        // Timestamp + org-assignment triggers — dialect-aware style. The
        // set_org_id BEFORE INSERT trigger is required for EVERY tenant-scoped
        // entity (it assigns platform_organization_id from the session GUC);
        // append-only entities skip only the updated_at trigger (#284).
        if (ctx.has_updated_at || ctx.is_tenant_scoped) && self.dialect.has_plpgsql() {
            let trigger_sql = render_template_with_project(
                tera,
                &db_template_for(&*self.dialect, "trigger"),
                &ctx,
                project,
            )?;
            files.push(GeneratedFile {
                path: crate::db::migrations_root(&self.output_dir).join(format!(
                    "{}_{}_trigger.sql",
                    ctx.schema_name, ctx.table_name
                )),
                content: trigger_sql,
            });
        }

        // Domain event trigger — dialect-aware style
        let event_trigger_sql = render_template_with_project(
            tera,
            &db_template_for(&*self.dialect, "domain_event_trigger"),
            &ctx,
            project,
        )?;
        files.push(GeneratedFile {
            path: crate::db::migrations_root(&self.output_dir).join(format!(
                "{}_{}_event_trigger.sql",
                ctx.schema_name, ctx.table_name
            )),
            content: event_trigger_sql,
        });

        // ProcessHistoryType-compatible view for workflow entities
        if ctx.has_workflow {
            let view_sql = render_template_with_project(
                tera,
                &db_template_for(&*self.dialect, "process_history_view"),
                &ctx,
                project,
            )?;
            files.push(GeneratedFile {
                path: crate::db::migrations_root(&self.output_dir).join(format!(
                    "{}_{}_process_history_view.sql",
                    ctx.schema_name, ctx.table_name
                )),
                content: view_sql,
            });
        }

        // Full-text search — only on dialects that support it
        if ctx.fts.is_some() && self.dialect.has_fulltext_search() {
            let fts_sql = render_template_with_project(
                tera,
                &db_template_for(&*self.dialect, "fts"),
                &ctx,
                project,
            )?;
            files.push(GeneratedFile {
                path: crate::db::migrations_root(&self.output_dir)
                    .join(format!("{}_{}_fts.sql", ctx.schema_name, ctx.table_name)),
                content: fts_sql,
            });
        }

        // Semantic search (embeddings) — only on dialects that support it
        if !ctx.embeddings.is_empty() && self.dialect.has_embeddings() {
            let embedding_sql = render_template_with_project(
                tera,
                &db_template_for(&*self.dialect, "embedding"),
                &ctx,
                project,
            )?;
            files.push(GeneratedFile {
                path: crate::db::migrations_root(&self.output_dir).join(format!(
                    "{}_{}_embedding.sql",
                    ctx.schema_name, ctx.table_name
                )),
                content: embedding_sql,
            });
        }

        // Parse-validate generated SQLite DDL before emitting it: STRICT
        // tables reject invalid statements at apply time, so the failure
        // must surface at generation time. Postgres output is not gated —
        // it contains PL/pgSQL the generic parser cannot accept.
        if self.dialect.name() == "sqlite" {
            super::sqlite_gate::validate_sqlite_files(&files)?;
        }

        Ok(files)
    }
}
