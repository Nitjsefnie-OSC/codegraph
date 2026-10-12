use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;

use crate::ProjectConfig;
use crate::error::Result;
use crate::persistence::build_persistence_entity;
use crate::traits::{EntityGenerator, EntityGeneratorKind, GeneratedFile};
use codegraph_config::DomainConfig;

use crate::ddd::repository_emitter::{
    ChildTableInfo, EntityTree, RepositoryImplEmitter, TreeColumn,
};

/// Generates annotated Cornucopia SQL files for an entity's CRUD surface.
///
/// One file per entity at `queries/{domain}/{table}.sql`. Each annotated query
/// block (`--! name (params) : (returns)`) becomes a typed Rust function in the
/// generated `cornucopia-queries` crate.
///
/// Tenant isolation is NOT expressed in the SQL: the DDL trigger fills the
/// tenant column from `app.organization_id` on INSERT and Postgres RLS
/// (`SET LOCAL ROLE app_user` in the query/command layers) filters reads —
/// exactly like the SeaORM backend.
pub struct CornucopiaQueryGenerator {
    output_dir: PathBuf,
    parent_candidates: Vec<codegraph_core::types::ParentCandidate>,
}

impl CornucopiaQueryGenerator {
    pub fn new(output_dir: &Path) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
            parent_candidates: Vec::new(),
        }
    }

    pub fn with_parent_candidates(
        mut self,
        candidates: Vec<codegraph_core::types::ParentCandidate>,
    ) -> Self {
        self.parent_candidates = candidates;
        self
    }
}

#[async_trait]
impl EntityGenerator for CornucopiaQueryGenerator {
    fn kind(&self) -> EntityGeneratorKind {
        EntityGeneratorKind::CornucopiaQueries
    }

    fn supported_targets(&self) -> Option<Vec<crate::db::dialect::DatabaseTarget>> {
        None
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        schema_title: &str,
        domain: &str,
        config: &DomainConfig,
        _tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        if !project.is_cornucopia() {
            return Ok(Vec::new());
        }

        let parent_ref = crate::resolve_parent_fk_column_same_domain(
            schema_title,
            &self.parent_candidates,
            config
                .domains
                .get(domain)
                .and_then(|d| d.get_entity_config(schema_title)),
            domain,
            config,
            db,
        )
        .await;

        let tree = RepositoryImplEmitter
            .query_entity_tree(db, schema_title, domain, config, parent_ref.as_deref())
            .await?;

        if tree.table_name.is_empty() {
            return Ok(Vec::new());
        }

        // Column → PostgreSQL type name, from the persistence IR (used to
        // derive SQL casts for columns whose rust_type doesn't reveal the DB
        // type, e.g. geometry/geography columns stored as String).
        let entity_ir =
            build_persistence_entity(db, schema_title, domain, config, &self.parent_candidates)
                .await?;
        let pg_types: std::collections::HashMap<String, String> = entity_ir
            .columns
            .iter()
            .map(|c| (c.column_name.clone(), c.pg_type.clone()))
            .collect();

        let mut sql = render_entity_sql(schema_title, domain, config, &tree, &pg_types);

        // Deployed-worker query for tree_include entities: the resolution
        // needs the repository emitter's graph walk (the persistence tree
        // leaves tree_include empty), so it happens here in the async caller.
        if tree.hierarchy_field.is_some() {
            let resolved = RepositoryImplEmitter
                .query_entity_tree(db, schema_title, domain, config, None)
                .await?;
            if !resolved.tree_include.is_empty() {
                write_tree_workers_query(&mut sql, &resolved);
            }
        }

        let rel_path = format!("queries/{}/{}.sql", domain, tree.table_name);
        Ok(vec![GeneratedFile {
            path: self.output_dir.join(&rel_path),
            content: sql,
        }])
    }
}

/// The Postgres cast target for a column — explicit range casts win; otherwise
/// derive from the DTO rust type. Scalar params bind as text (Option<String>
/// binds NULL cleanly) and cast in SQL.
fn pg_cast_for(col: &TreeColumn, pg_types: &std::collections::HashMap<String, String>) -> String {
    if let Some(ref cast) = col.pg_cast {
        return cast.clone();
    }
    if let Some(pg) = pg_types.get(&col.pg_column_name) {
        let pg_upper = pg.to_uppercase();
        if pg_upper.contains("GEOMETRY") {
            return "geometry".to_string();
        }
        if pg_upper.contains("GEOGRAPHY") {
            return "geography".to_string();
        }
        if pg_upper.contains("VECTOR") {
            return "vector".to_string();
        }
        if pg_upper.contains("TSTZRANGE") {
            return "tstzrange".to_string();
        }
        if pg_upper.contains("DATERANGE") {
            return "daterange".to_string();
        }
    }
    match col.rust_type.as_str() {
        "Uuid" | "uuid::Uuid" => "uuid",
        "i32" => "int4",
        "i64" => "int8",
        "f32" => "float4",
        "f64" => "float8",
        "bool" => "bool",
        "Decimal" | "rust_decimal::Decimal" => "numeric",
        "NaiveDate" | "chrono::NaiveDate" => "date",
        "DateTime<Utc>" | "chrono::DateTime<chrono::Utc>" => "timestamptz",
        "serde_json::Value" | "Vec<serde_json::Value>" => "jsonb",
        _ => "text",
    }
    .to_string()
}

/// Postgres cast target for a child-table column.
fn child_pg_cast_for(col: &crate::ddd::repository_emitter::ChildColumn) -> String {
    if let Some(ref cast) = col.pg_cast {
        return cast.clone();
    }
    match col.rust_type.as_str() {
        "Uuid" | "uuid::Uuid" => "uuid",
        "i32" => "int4",
        "i64" => "int8",
        "f32" => "float4",
        "f64" => "float8",
        "bool" => "bool",
        "Decimal" | "rust_decimal::Decimal" => "numeric",
        "NaiveDate" | "chrono::NaiveDate" => "date",
        "DateTime<Utc>" | "chrono::DateTime<chrono::Utc>" => "timestamptz",
        "serde_json::Value" | "Vec<serde_json::Value>" => "jsonb",
        _ => "text",
    }
    .to_string()
}

/// Whether the column is bound typed (arrays) rather than as text.
fn is_array_col(col: &TreeColumn) -> bool {
    col.is_array || col.rust_type.starts_with("Vec<")
}

/// The snake_case entity name used to prefix query function names.
fn snake_name(tree: &EntityTree) -> String {
    codegraph_naming::to_snake_case(&tree.entity_name)
}

/// Render the full SQL file for one entity.
fn render_entity_sql(
    schema_title: &str,
    domain: &str,
    config: &DomainConfig,
    tree: &EntityTree,
    pg_types: &std::collections::HashMap<String, String>,
) -> String {
    let mut sql = String::with_capacity(16 * 1024);

    let table = format!("\"{}\".\"{}\"", tree.schema_name, tree.table_name);
    let entity_name = snake_name(tree);
    let soft_delete_col = tree.soft_delete_column.as_deref();

    // Direct columns participating in row returns (excludes composite ranges,
    // which do not exist as physical columns).
    let row_cols: Vec<&TreeColumn> = tree
        .direct_columns
        .iter()
        .filter(|c| !c.is_composite_range)
        .collect();

    sql.push_str(&format!(
        "-- Cornucopia queries for {} (domain: {})\n\
         -- Generated by codegraph — persistence_provider = \"cornucopia\"\n\n",
        schema_title, domain,
    ));

    // ── list (+ count) ─────────────────────────────────────────────────
    write_list_queries(
        &mut sql,
        &table,
        &entity_name,
        soft_delete_col,
        &row_cols,
        pg_types,
        tree.append_only,
    );

    // ── get by id ──────────────────────────────────────────────────────
    write_get_queries(
        &mut sql,
        &table,
        &entity_name,
        soft_delete_col,
        &row_cols,
        pg_types,
        tree.append_only,
    );
    if let Some(ref parent_fk) = tree.parent_ref {
        write_get_scoped_queries(
            &mut sql,
            &table,
            &entity_name,
            soft_delete_col,
            &row_cols,
            parent_fk,
            pg_types,
            tree.append_only,
        );
    }

    // ── create ─────────────────────────────────────────────────────────
    write_create_query(&mut sql, &table, &entity_name, tree, pg_types);

    // ── update (COALESCE so missing Option fields keep existing values) ─
    write_update_query(
        &mut sql,
        &table,
        &entity_name,
        tree,
        soft_delete_col,
        pg_types,
    );

    // ── delete ─────────────────────────────────────────────────────────
    write_delete_query(&mut sql, &table, &entity_name, soft_delete_col);

    // ── child tables (VO tables) ───────────────────────────────────────
    for child in &tree.child_tables {
        write_child_queries(&mut sql, &tree.schema_name, &entity_name, child);
    }

    // ── nested filter helpers (child/grandchild EXISTS equivalents) ─────
    write_nested_filter_queries(&mut sql, tree);

    // ── FTS / embeddings / tree ────────────────────────────────────────
    if tree.has_fts {
        write_search_queries(
            &mut sql,
            &table,
            &entity_name,
            soft_delete_col,
            &tree.fts_language,
        );
    }
    if tree.has_embeddings
        && let Some(ec) = config
            .domains
            .get(domain)
            .and_then(|d| d.get_entity_config(schema_title))
        && let Some(first) = ec.search.embedding_columns.first()
    {
        let emb_col = format!("{}_embedding", first);
        write_embedding_queries(&mut sql, &table, &entity_name, soft_delete_col, &emb_col);
    }
    if let Some(ref hf) = tree.hierarchy_field {
        write_tree_query(
            &mut sql,
            &table,
            &entity_name,
            hf,
            &row_cols,
            pg_types,
            tree.append_only,
        );
    }

    // ── declared .ddd design finders (issue #449) ──────────────────────
    write_design_finder_queries(
        &mut sql,
        &table,
        &entity_name,
        tree,
        soft_delete_col,
        &row_cols,
        pg_types,
    );

    sql
}

/// One annotated equality query per declared design finder. Parameter
/// columns resolve exactly like the SeaORM emitter's (the same
/// `finder_columns` contract), so both providers emit the same finders.
///
/// Binding: `String`/`bool`/`i64`/`f64`/`Uuid` params bind their natural
/// rust type (tokio-postgres infers the column type from the prepared
/// comparison, exactly like `get_{entity} (id)`); `Decimal`/`NaiveDate`
/// params bind as text with an explicit `::text` cast on the column to
/// avoid depending on the generated queries crate's numeric/chrono
/// features.
fn write_design_finder_queries(
    sql: &mut String,
    table: &str,
    entity_name: &str,
    tree: &EntityTree,
    soft_delete_col: Option<&str>,
    row_cols: &[&TreeColumn],
    pg_types: &std::collections::HashMap<String, String>,
) {
    if tree.design_finders.is_empty() {
        return;
    }
    let cols = row_col_list(row_cols, pg_types, tree.append_only);
    let hints = row_hints(row_cols, tree.append_only);

    for finder in &tree.design_finders {
        let Some(param_cols) =
            crate::ddd::repository_emitter::finder_columns(finder, &tree.direct_columns)
        else {
            continue;
        };
        // `findByExample`'s Option params use the nullability idiom:
        // `(:p IS NULL OR col = :p)` — a None param filters its column out.
        // The FIRST reference needs an explicit cast: Postgres cannot infer
        // a bare parameter's type from `IS NULL` alone.
        let example = finder.kind == crate::ddd::design::FinderKind::FindByExample;
        let mut clauses = Vec::with_capacity(param_cols.len() + 1);
        for (param, col) in finder.params.iter().zip(param_cols.iter()) {
            let pg_col = format!("\"{}\"", col.pg_column_name);
            if example {
                // `:p IS NULL` stays bare — cornucopia's derive detects the
                // nullability idiom textually and binds Option; the cast
                // rides the EQUALITY so Postgres can type the parameter
                // (a bare param used only in IS NULL has no inferable type).
                // Fully cast on BOTH references: Postgres must type the
                // parameter without relying on cross-reference inference,
                // and the header's `?` (below) is what makes cornucopia
                // bind Option — never the SQL text.
                if crate::ddd::repository_emitter::finder_param_binds_as_text(&param.rust_type) {
                    clauses.push(format!(
                        "(:{p}::text IS NULL OR {pg_col}::text = :{p}::text)",
                        p = param.name
                    ));
                } else {
                    let pg_type = pg_types
                        .get(&col.pg_column_name)
                        .map(String::as_str)
                        .unwrap_or("text");
                    clauses.push(format!(
                        "(:{p}::{t} IS NULL OR {pg_col} = :{p}::{t})",
                        p = param.name,
                        t = pg_type
                    ));
                }
            } else if crate::ddd::repository_emitter::finder_param_binds_as_text(&param.rust_type) {
                clauses.push(format!("{pg_col}::text = :{}", param.name));
            } else {
                clauses.push(format!("{pg_col} = :{}", param.name));
            }
        }
        // The declared signature carries no include_deleted parameter —
        // soft-deleted rows always stay invisible to design finders.
        if let Some(sd) = soft_delete_col {
            clauses.push(format!("\"{sd}\" IS NULL"));
        }
        sql.push_str(&format!(
            "--! {fn_name} ({params}) : ({hints})\n\
             --- Design finder `{author}` (declared in the .ddd design): equality query.\n\
             SELECT {cols}\n\
             FROM {table}\n\
             WHERE {};\n\n",
            clauses.join("\n    AND "),
            fn_name = design_finder_query_name(finder, entity_name),
            params = finder
                .params
                .iter()
                .zip(param_cols.iter())
                .map(|(p, _col)| {
                    // `?` marks the header param nullable — cornucopia binds
                    // Option<T> for it. Every findByExample parameter binds
                    // as Option (the None-filters idiom); other finders bind
                    // their declared type directly.
                    if example || p.rust_type.starts_with("Option<") {
                        format!("{}?", p.name)
                    } else {
                        p.name.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(", "),
            author = finder.name,
        ));
    }
}

/// The cornucopia query function name for a design finder:
/// `{method_name}_{entity_snake}` (e.g. `find_by_title_book`).
fn design_finder_query_name(
    finder: &crate::ddd::design::DesignFinder,
    entity_name: &str,
) -> String {
    format!("{}_{}", finder.method_name, entity_name)
}

fn row_col_list(
    cols: &[&TreeColumn],
    pg_types: &std::collections::HashMap<String, String>,
    append_only: bool,
) -> String {
    let mut names: Vec<String> = vec!["\"id\"".to_string()];
    for c in cols {
        names.push(select_col(
            c.field_name.trim_start_matches("r#"),
            c,
            pg_types,
        ));
    }
    names.push("\"created_at\"".to_string());
    // Append-only entities (#284) carry no updated_at column.
    if !append_only {
        names.push("\"updated_at\"".to_string());
    }
    names.join(", ")
}

/// One SELECT column, aliased to the Rust field name the row hints (and
/// the repository emitter's `row.<field>` accessors) use: physical columns
/// renamed at the DDL (codelist-ref `reward` → `reward_code`) must still
/// surface under their field names, and cornucopia validates the hints
/// against the RETURNED column names.
fn select_col(
    field_name: &str,
    c: &TreeColumn,
    pg_types: &std::collections::HashMap<String, String>,
) -> String {
    let expr = if needs_text_cast(c, pg_types) {
        format!("\"{}\"::text", c.pg_column_name)
    } else {
        format!("\"{}\"", c.pg_column_name)
    };
    if c.pg_column_name == field_name {
        expr
    } else {
        format!("{expr} AS \"{field_name}\"")
    }
}

/// Whether a column's DB type needs an explicit `::text` cast in SELECT lists
/// because its mapped Rust type (String) cannot be read directly by
/// tokio-postgres (ranges, geometry/geography, vector, numeric — `String`'s
/// `FromSql` rejects all of these, even for NULL values, because
/// `Row::try_get` type-checks `accepts()` before checking for NULL).
fn needs_text_cast(col: &TreeColumn, pg_types: &std::collections::HashMap<String, String>) -> bool {
    if col.pg_cast.is_some() {
        return true;
    }
    let Some(pg) = pg_types.get(&col.pg_column_name) else {
        return false;
    };
    let pg_upper = pg.to_uppercase();
    pg_upper.contains("GEOMETRY")
        || pg_upper.contains("GEOGRAPHY")
        || pg_upper.contains("VECTOR")
        || pg_upper.contains("RANGE")
        || pg_upper.contains("NUMERIC")
}

fn row_hints(cols: &[&TreeColumn], append_only: bool) -> String {
    let mut hints: Vec<String> = vec!["id".to_string()];
    for c in cols {
        // Hints name the cornucopia-generated struct fields — the Rust
        // field names the repository emitter reads (`row.reward`). The
        // SELECT list aliases each physical column to its field name (see
        // `select_col`), so hint == returned column == struct field.
        let name = c.field_name.trim_start_matches("r#");
        hints.push(if c.is_nullable {
            format!("{}?", name)
        } else {
            name.to_string()
        });
    }
    hints.push("created_at".to_string());
    // Append-only entities (#284) carry no updated_at column.
    if !append_only {
        hints.push("updated_at".to_string());
    }
    hints.join(", ")
}

fn write_list_queries(
    sql: &mut String,
    table: &str,
    entity_name: &str,
    soft_delete_col: Option<&str>,
    row_cols: &[&TreeColumn],
    pg_types: &std::collections::HashMap<String, String>,
    append_only: bool,
) {
    let cols = row_col_list(row_cols, pg_types, append_only);
    let hints = row_hints(row_cols, append_only);

    for (suffix, include_deleted) in [("", false), ("_including_deleted", true)] {
        let where_clause = if !include_deleted {
            soft_delete_col
                .map(|sd| format!("\n  WHERE \"{sd}\" IS NULL"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        sql.push_str(&format!(
            "--! list_{entity_name}{suffix} (offset, page_size) : ({hints})\n\
             --- List {entity_name} records.\n\
             SELECT {cols}\n\
             FROM {table}{where_clause}\n\
             ORDER BY \"created_at\" DESC\n\
             LIMIT :page_size OFFSET :offset;\n\n",
        ));
        let count_where_clause = if include_deleted {
            String::new()
        } else {
            soft_delete_col
                .map(|sd| format!("\n  WHERE \"{sd}\" IS NULL"))
                .unwrap_or_default()
        };
        sql.push_str(&format!(
            "--! count_{entity_name}{suffix} : (count)\n\
             --- Count of {entity_name} records.\n\
             SELECT COUNT(*) AS \"count\"\n\
             FROM {table}{count_where_clause};\n\n",
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn write_get_scoped_queries(
    sql: &mut String,
    table: &str,
    entity_name: &str,
    soft_delete_col: Option<&str>,
    row_cols: &[&TreeColumn],
    parent_fk: &str,
    pg_types: &std::collections::HashMap<String, String>,
    append_only: bool,
) {
    let cols = row_col_list(row_cols, pg_types, append_only);
    let hints = row_hints(row_cols, append_only);

    for (suffix, include_deleted) in [("", false), ("_including_deleted", true)] {
        let mut clauses = vec![
            "\"id\" = :id".to_string(),
            format!("\"{parent_fk}\" = :parent_id"),
        ];
        if !include_deleted && let Some(sd) = soft_delete_col {
            clauses.push(format!("\"{}\" IS NULL", sd));
        }
        sql.push_str(&format!(
            "--! get_{entity_name}_scoped{suffix} (id, parent_id) : ({hints})\n\
             --- Get a single {entity_name} by ID, scoped to its parent.\n\
             SELECT {cols}\n\
             FROM {table}\n\
             WHERE {};\n\n",
            clauses.join("\n    AND "),
        ));
    }
}

fn write_get_queries(
    sql: &mut String,
    table: &str,
    entity_name: &str,
    soft_delete_col: Option<&str>,
    row_cols: &[&TreeColumn],
    pg_types: &std::collections::HashMap<String, String>,
    append_only: bool,
) {
    let cols = row_col_list(row_cols, pg_types, append_only);
    let hints = row_hints(row_cols, append_only);

    for (suffix, include_deleted) in [("", false), ("_including_deleted", true)] {
        let mut clauses = vec!["\"id\" = :id".to_string()];
        if !include_deleted && let Some(sd) = soft_delete_col {
            clauses.push(format!("\"{}\" IS NULL", sd));
        }
        sql.push_str(&format!(
            "--! get_{entity_name}{suffix} (id) : ({hints})\n\
             --- Get a single {entity_name} by ID.\n\
             SELECT {cols}\n\
             FROM {table}\n\
             WHERE {};\n\n",
            clauses.join("\n    AND "),
        ));
    }
}

fn is_writable_col(col: &TreeColumn) -> bool {
    !col.is_workflow_managed && !col.is_composite_range && !col.is_media
}

fn param_name(col: &TreeColumn) -> String {
    col.field_name.trim_start_matches("r#").to_string()
}

/// Annotation param name with the cornucopia `?` nullable marker.
fn param_sig(col: &TreeColumn) -> String {
    if col.is_nullable {
        format!("{}?", param_name(col))
    } else {
        param_name(col)
    }
}

fn write_create_query(
    sql: &mut String,
    table: &str,
    entity_name: &str,
    tree: &EntityTree,
    pg_types: &std::collections::HashMap<String, String>,
) {
    // The parent FK column (when present in direct_columns) is bound from the
    // path parameter as a trailing :parent_id — keep it out of the DTO-driven
    // column list.
    let writable: Vec<&TreeColumn> = tree
        .direct_columns
        .iter()
        .filter(|c| is_writable_col(c))
        .filter(|c| {
            !tree
                .parent_ref
                .as_ref()
                .is_some_and(|pr| c.pg_column_name.eq_ignore_ascii_case(pr))
        })
        .collect();

    if writable.is_empty() {
        // Entities whose data lives entirely in child tables (or system-managed
        // columns) still need a create query — Postgres fills the defaults.
        sql.push_str(&format!(
            "--! create_{entity_name} : (id)\n\
             --- Create a new {entity_name} (defaults only).\n\
             INSERT INTO {table} DEFAULT VALUES\n\
             RETURNING \"id\";\n\n",
        ));
        return;
    }

    let mut insert_cols: Vec<String> = writable
        .iter()
        .map(|c| format!("\"{}\"", c.pg_column_name))
        .collect();
    let mut insert_params: Vec<String> = writable
        .iter()
        .map(|c| {
            if c.is_structured_wrapper || is_array_col(c) {
                format!(":{}", param_name(c))
            } else {
                format!(":{}::text::{}", param_name(c), pg_cast_for(c, pg_types))
            }
        })
        .collect();
    let mut param_defs: Vec<String> = writable.iter().map(|c| param_sig(c)).collect();
    // Child entities: the parent FK column is not part of the DTO — bind it
    // from the path parameter via a trailing :parent_id param.
    if let Some(ref parent_fk) = tree.parent_ref {
        insert_cols.push(format!("\"{parent_fk}\""));
        insert_params.push(":parent_id".to_string());
        param_defs.push("parent_id".to_string());
    }

    sql.push_str(&format!(
        "--! create_{entity_name} ({}) : (id)\n\
         --- Create a new {entity_name}.\n\
         INSERT INTO {table} ({})\n\
         VALUES ({})\n\
         RETURNING \"id\";\n\n",
        param_defs.join(", "),
        insert_cols.join(", "),
        insert_params.join(", "),
    ));
}

fn write_update_query(
    sql: &mut String,
    table: &str,
    entity_name: &str,
    tree: &EntityTree,
    soft_delete_col: Option<&str>,
    pg_types: &std::collections::HashMap<String, String>,
) {
    let updatable: Vec<&TreeColumn> = tree
        .direct_columns
        .iter()
        .filter(|c| is_writable_col(c))
        .collect();

    // Append-only entities (#284) have no UPDATE path — the repository never
    // calls update for them, so no query is emitted (the table carries no
    // updated_at column to no-op-touch either).
    if tree.append_only {
        return;
    }

    if updatable.is_empty() {
        sql.push_str(&format!(
            "--! update_{entity_name} (id)\n\
             --- No-op update for {entity_name}.\n\
             UPDATE {table}\n\
             SET \"updated_at\" = \"updated_at\"\n\
             WHERE \"id\" = :id;\n\n",
        ));
        return;
    }

    let set_clauses = updatable
        .iter()
        .map(|c| {
            if c.is_structured_wrapper || is_array_col(c) {
                format!(
                    "\"{}\" = COALESCE(:{}, \"{}\")",
                    c.pg_column_name,
                    param_name(c),
                    c.pg_column_name
                )
            } else {
                format!(
                    "\"{}\" = COALESCE(:{}::text::{}, \"{}\")",
                    c.pg_column_name,
                    param_name(c),
                    pg_cast_for(c, pg_types),
                    c.pg_column_name
                )
            }
        })
        .collect::<Vec<_>>()
        .join(",\n    ");

    let mut params = vec!["id".to_string()];
    for c in &updatable {
        // Update DTO fields are all Option<T> — mark every param nullable so
        // Option binds compile and NULL keeps existing values via COALESCE.
        params.push(format!("{}?", param_name(c)));
    }

    let mut where_clauses = vec!["\"id\" = :id".to_string()];
    if let Some(sd) = soft_delete_col {
        where_clauses.push(format!("\"{}\" IS NULL", sd));
    }

    sql.push_str(&format!(
        "--! update_{entity_name} ({})\n\
         --- Update an existing {entity_name}.\n\
         UPDATE {table}\n\
         SET {set_clauses}\n\
         WHERE {};\n\n",
        params.join(", "),
        where_clauses.join("\n  AND "),
    ));
}

fn write_delete_query(
    sql: &mut String,
    table: &str,
    entity_name: &str,
    soft_delete_col: Option<&str>,
) {
    let mut where_clauses = vec!["\"id\" = :id".to_string()];
    if let Some(sd) = soft_delete_col {
        where_clauses.push(format!("\"{}\" IS NULL", sd));
    }
    match soft_delete_col {
        Some(sd) => {
            sql.push_str(&format!(
                "--! delete_{entity_name} (id)\n\
                 --- Soft-delete a {entity_name} by setting the deletion marker.\n\
                 UPDATE {table}\n\
                 SET \"{sd}\" = NOW()\n\
                 WHERE {};\n\n",
                where_clauses.join("\n  AND "),
            ));
        }
        None => {
            sql.push_str(&format!(
                "--! delete_{entity_name} (id)\n\
                 --- Hard-delete a {entity_name}.\n\
                 DELETE FROM {table}\n\
                 WHERE {};\n\n",
                where_clauses.join("\n  AND "),
            ));
        }
    }
}

/// Emit list/insert/delete queries for a child (value-object) table.
fn write_child_queries(
    sql: &mut String,
    schema_name: &str,
    entity_name: &str,
    child: &ChildTableInfo,
) {
    let child_table = format!("\"{}\".\"{}\"", schema_name, child.sql_table_name);
    let fk = &child.parent_fk_column;
    // All child columns participate in the row return (the FK is a real
    // property on the child DTO); only the INSERT excludes it.
    let data_cols: Vec<_> = child.columns.iter().collect();
    // Aliased to the child DTO field names — same contract as `select_col`
    // (hints name struct fields; the returned columns must match them).
    let col_names: Vec<String> = data_cols
        .iter()
        .map(|c| {
            let field = c.field_name.trim_start_matches("r#");
            let expr = if c.pg_cast.is_some()
                || matches!(
                    c.rust_type.as_str(),
                    "String" | "Decimal" | "rust_decimal::Decimal"
                ) {
                // Range/geometry/numeric columns: the DTO holds strings — cast
                // to text so tokio-postgres can deserialize them.
                format!("\"{}\"::text", c.pg_column_name)
            } else {
                format!("\"{}\"", c.pg_column_name)
            };
            if c.pg_column_name == field {
                expr
            } else {
                format!("{expr} AS \"{field}\"")
            }
        })
        .collect();

    if col_names.is_empty() {
        sql.push_str(&format!(
            "--! list_{entity_name}_{child} ({fk}) : (id)\n\
             --- Child rows for {entity_name}.\n\
             SELECT \"id\"\n\
             FROM {table}\n\
             WHERE \"{fk}\" = :{fk};\n\n",
            child = child.sql_table_name,
            table = child_table,
        ));
    } else {
        let hints: Vec<String> = data_cols
            .iter()
            .map(|c| {
                let name = c.field_name.trim_start_matches("r#");
                if c.is_nullable {
                    format!("{}?", name)
                } else {
                    name.to_string()
                }
            })
            .collect();
        sql.push_str(&format!(
            "--! list_{entity_name}_{child} ({fk}) : (id, {hints})\n\
             --- Child rows for {entity_name}.\n\
             SELECT \"id\", {cols}\n\
             FROM {table}\n\
             WHERE \"{fk}\" = :{fk};\n\n",
            child = child.sql_table_name,
            hints = hints.join(", "),
            cols = col_names.join(", "),
            table = child_table,
        ));
    }

    // Insert a child row (id generated by the adapter).
    let mut insert_cols = vec!["\"id\"".to_string(), format!("\"{}\"", fk)];
    let mut insert_vals = vec![":id".to_string(), format!(":{}", fk)];
    let mut param_defs = vec!["id".to_string(), fk.clone()];
    for c in &data_cols {
        if c.pg_column_name == *fk {
            continue;
        }
        insert_cols.push(format!("\"{}\"", c.pg_column_name));
        let pname = c.field_name.trim_start_matches("r#");
        if c.rust_type.starts_with("Vec<") {
            insert_vals.push(format!(":{}", pname));
        } else {
            insert_vals.push(format!(":{}::text::{}", pname, child_pg_cast_for(c)));
        }
        param_defs.push(if c.is_nullable {
            format!("{}?", pname)
        } else {
            pname.to_string()
        });
    }
    sql.push_str(&format!(
        "--! insert_{entity_name}_{child} ({})\n\
         --- Insert a child row for {entity_name}.\n\
         INSERT INTO {table} ({})\n\
         VALUES ({});\n\n",
        param_defs.join(", "),
        insert_cols.join(", "),
        insert_vals.join(", "),
        child = child.sql_table_name,
        table = child_table,
    ));

    // Delete child rows by parent FK (used when replacing children on update).
    sql.push_str(&format!(
        "--! delete_{entity_name}_{child} ({fk})\n\
         --- Delete child rows for {entity_name}.\n\
         DELETE FROM {table}\n\
         WHERE \"{fk}\" = :{fk};\n\n",
        child = child.sql_table_name,
        table = child_table,
    ));

    // Nested grandchildren.
    for grandchild in &child.child_tables {
        write_child_queries(sql, schema_name, entity_name, grandchild);
    }
}

/// Emit one helper query per nested (dot-notation) filter. The adapter uses
/// these to filter rows in memory by parent-id membership — the equivalent of
/// the SeaORM EXISTS subqueries.
/// Collect all (recursive) child-table SQL names for an entity.
fn collect_child_table_names(
    children: &[ChildTableInfo],
    out: &mut std::collections::HashSet<String>,
) {
    for child in children {
        out.insert(child.sql_table_name.clone());
        collect_child_table_names(&child.child_tables, out);
    }
}

fn write_nested_filter_queries(sql: &mut String, tree: &EntityTree) {
    // The nested-filter resolver over-generates VO child tables (the DDL may
    // flatten a VO into a scalar column instead). Only emit helper queries for
    // tables that actually exist:
    //   - VO children the DDL materialized (present in tree.child_tables), or
    //   - child-entity filters (parent FK is not the plain {parent_table}_id).
    let vo_pattern = format!("{}_id", tree.table_name);
    let real_vo_tables: std::collections::HashSet<String> = {
        let mut set = std::collections::HashSet::new();
        collect_child_table_names(&tree.child_tables, &mut set);
        set
    };
    for nf in &tree.nested_filter_fields {
        let is_vo_style = nf.parent_fk_column == vo_pattern;
        let is_child_entity = nf.intermediate_join.is_none() && !is_vo_style;
        let is_real_table = real_vo_tables.contains(&nf.sql_table_name);
        if !is_real_table && !is_child_entity {
            continue;
        }
        let qname = nf.filter_key.replace('.', "_");
        let leaf_cast = nested_filter_cast(&nf.rust_type);
        let val_param = if leaf_cast.is_empty() {
            ":val".to_string()
        } else {
            format!(":val::text::{leaf_cast}")
        };
        if let Some(ref ij) = nf.intermediate_join {
            sql.push_str(&format!(
                "--! filter_{qname} (val) : ({fk})\n\
                 --- Nested filter helper: parent ids with a matching grandchild.\n\
                 SELECT _i.\"{fk}\"\n\
                 FROM \"{ij_schema}\".\"{ij_table}\" _i\n\
                 WHERE EXISTS (SELECT 1 FROM \"{schema}\".\"{table}\" _gc\n\
                 \x20       WHERE _gc.\"{gc_fk}\" = _i.\"id\" AND _gc.\"{col}\" = {val_param});\n\n",
                fk = ij.parent_fk_column,
                ij_schema = ij.sql_schema,
                ij_table = ij.sql_table_name,
                schema = nf.sql_schema,
                table = nf.sql_table_name,
                gc_fk = nf.parent_fk_column,
                col = nf.pg_column_name,
            ));
        } else {
            sql.push_str(&format!(
                "--! filter_{qname} (val) : ({fk})\n\
                 --- Nested filter helper: parent ids with a matching child.\n\
                 SELECT \"{fk}\"\n\
                 FROM \"{schema}\".\"{table}\"\n\
                 WHERE \"{col}\" = {val_param};\n\n",
                fk = nf.parent_fk_column,
                schema = nf.sql_schema,
                table = nf.sql_table_name,
                col = nf.pg_column_name,
            ));
        }
    }
}

/// Postgres cast target for a nested-filter value ("" = plain text compare).
fn nested_filter_cast(rust_type: &str) -> &'static str {
    let base = rust_type
        .strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(rust_type);
    match base {
        "Uuid" | "uuid::Uuid" => "uuid",
        // Entity reference types (e.g. "OrganizationType") are always UUID FK columns.
        ty if ty.ends_with("Type") && ty.chars().next().is_some_and(|c| c.is_uppercase()) => "uuid",
        "i32" => "int4",
        "i64" => "int8",
        "f32" => "float4",
        "f64" => "float8",
        "bool" => "bool",
        "Decimal" | "rust_decimal::Decimal" => "numeric",
        "NaiveDate" | "chrono::NaiveDate" => "date",
        "DateTime<Utc>" | "chrono::DateTime<chrono::Utc>" => "timestamptz",
        _ => "",
    }
}

fn write_search_queries(
    sql: &mut String,
    table: &str,
    entity_name: &str,
    soft_delete_col: Option<&str>,
    fts_language: &str,
) {
    for (suffix, include_deleted) in [("", false), ("_including_deleted", true)] {
        let extra = if !include_deleted {
            soft_delete_col
                .map(|sd| format!(" AND \"{sd}\" IS NULL"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        sql.push_str(&format!(
            "--! search_{entity_name}{suffix} (q, offset, page_size) : (id)\n\
             --- Full-text search over {entity_name}.\n\
             SELECT \"id\"\n\
             FROM {table}\n\
             WHERE search_tsv @@ websearch_to_tsquery('{fts_language}', :q){extra}\n\
             ORDER BY ts_rank(search_tsv, websearch_to_tsquery('{fts_language}', :q)) DESC\n\
             LIMIT :page_size OFFSET :offset;\n\n",
        ));
        sql.push_str(&format!(
            "--! search_count_{entity_name}{suffix} (q) : (count)\n\
             --- Count of full-text search matches for {entity_name}.\n\
             SELECT COUNT(*) AS \"count\"\n\
             FROM {table}\n\
             WHERE search_tsv @@ websearch_to_tsquery('{fts_language}', :q){extra};\n\n",
        ));
    }
}

fn write_embedding_queries(
    sql: &mut String,
    table: &str,
    entity_name: &str,
    soft_delete_col: Option<&str>,
    emb_col: &str,
) {
    for (suffix, include_deleted) in [("", false), ("_including_deleted", true)] {
        let extra = if !include_deleted {
            soft_delete_col
                .map(|sd| format!(" AND \"{sd}\" IS NULL"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        sql.push_str(&format!(
            "--! semantic_{entity_name}{suffix} (embedding, limit) : (id)\n\
             --- Semantic similarity search over {entity_name}.\n\
             SELECT \"id\"\n\
             FROM {table}\n\
             WHERE \"{emb_col}\" IS NOT NULL{extra}\n\
             ORDER BY \"{emb_col}\" <=> :embedding::text::vector\n\
             LIMIT :limit;\n\n",
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn write_tree_query(
    sql: &mut String,
    table: &str,
    entity_name: &str,
    hierarchy_field: &str,
    row_cols: &[&TreeColumn],
    pg_types: &std::collections::HashMap<String, String>,
    append_only: bool,
) {
    let cols = row_col_list(row_cols, pg_types, append_only);
    let prefixed: Vec<String> = std::iter::once("c.\"id\"".to_string())
        .chain(row_cols.iter().map(|c| {
            if needs_text_cast(c, pg_types) {
                format!("c.\"{}\"::text", c.pg_column_name)
            } else {
                format!("c.\"{}\"", c.pg_column_name)
            }
        }))
        .chain(std::iter::once("c.\"created_at\"".to_string()))
        .chain(std::iter::once("c.\"updated_at\"".to_string()).filter(|_| !append_only))
        .collect();
    let hints = row_hints(row_cols, append_only);
    sql.push_str(&format!(
        "--! tree_{entity_name} (root_id, max_depth?) : ({hints})\n\
         --- Recursive subtree rooted at a {entity_name}.\n\
         WITH RECURSIVE tree AS (\n\
         \x20 SELECT {cols}, 0 AS _tree_depth FROM {table} WHERE \"id\" = :root_id\n\
         \x20 UNION ALL\n\
         \x20 SELECT {prefixed}, t._tree_depth + 1 AS _tree_depth FROM {table} c JOIN tree t ON c.\"{hierarchy_field}\" = t.\"id\"\n\
         \x20 WHERE (:max_depth::int4 IS NULL OR t._tree_depth < :max_depth::int4)\n\
         )\n\
         SELECT {cols} FROM tree ORDER BY _tree_depth, \"created_at\";\n\n",
        prefixed = prefixed.join(", "),
    ));
}

/// Worker-map query for `tree_include` hydration: deployments ⨝ workers ⨝
/// person ⨝ person-name, keyed by position id — the cornucopia counterpart
/// of the SeaORM find_tree's `deployed_worker` fetch, so both persistence
/// providers return identical tree rows.
fn write_tree_workers_query(sql: &mut String, tree: &EntityTree) {
    let Some(inc) = tree.tree_include.first() else {
        return;
    };
    let mut from_clause = format!(
        "{} d JOIN {} w ON w.\"id\" = d.\"{}\" AND w.deleted_at IS NULL",
        inc.via_table, inc.parent_table, inc.parent_ref_column
    );
    for (i, (table, fk_col, parent_alias)) in inc.worker_detail_joins.iter().enumerate() {
        let alias = if i == 0 { "wp" } else { "wpn" };
        from_clause.push_str(&format!(
            " JOIN {table} {alias} ON {alias}.\"{fk_col}\" = {parent_alias}.\"id\""
        ));
    }
    let worker_json = if inc.worker_detail_joins.is_empty() {
        "jsonb_build_object('id', w.\"id\")".to_string()
    } else {
        "jsonb_build_object('id', w.\"id\", 'given_name', wpn.given, 'family_name', wpn.family, 'avatar_url', wp.avatar_url)".to_string()
    };
    sql.push_str(&format!(
        "--! tree_{snake}_workers (position_ids) : (position_id, deployed_worker)\n\
         --- Deployed-worker map for tree rows, keyed by position id.\n\
         SELECT d.\"{fk}\" AS \"position_id\", {worker_json} AS \"deployed_worker\"\n\
         FROM {from_clause}\n\
         WHERE d.\"{fk}\" = ANY(:position_ids::uuid[]) AND d.deleted_at IS NULL;\n\n",
        snake = tree.table_name,
        fk = inc.via_fk_column,
    ));
}
