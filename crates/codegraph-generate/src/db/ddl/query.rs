use std::collections::HashSet;
use std::sync::LazyLock;

use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::{ColumnInfo, CompositionNode};
use codegraph_type_contracts::RefClassificationKind;

use crate::error::Result;
use codegraph_config::{DomainConfig, SearchConfig};

use super::DdlGenerator;
use super::accumulators::DdlAccumulators;
use super::types::{
    CheckConstraint, ChildTableDef, ColumnComment, ColumnDef, DdlContext, EmbeddingContext,
    ForeignKeyDef, FtsColumnWeight, FtsContext, IndexDef, RoleMinimum,
};

/// PostgreSQL reserved words that must be double-quoted when used as column names.
static PG_RESERVED_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "all",
        "analyse",
        "analyze",
        "and",
        "any",
        "array",
        "as",
        "asc",
        "asymmetric",
        "authorization",
        "between",
        "binary",
        "both",
        "case",
        "cast",
        "check",
        "collate",
        "collation",
        "column",
        "concurrently",
        "constraint",
        "create",
        "cross",
        "current_catalog",
        "current_date",
        "current_role",
        "current_schema",
        "current_time",
        "current_timestamp",
        "current_user",
        "default",
        "deferrable",
        "desc",
        "distinct",
        "do",
        "else",
        "end",
        "except",
        "false",
        "fetch",
        "for",
        "foreign",
        "freeze",
        "from",
        "full",
        "grant",
        "group",
        "having",
        "ilike",
        "in",
        "initially",
        "inner",
        "intersect",
        "into",
        "is",
        "isnull",
        "join",
        "lateral",
        "leading",
        "left",
        "like",
        "limit",
        "localtime",
        "localtimestamp",
        "natural",
        "not",
        "notnull",
        "null",
        "offset",
        "on",
        "only",
        "or",
        "order",
        "outer",
        "overlaps",
        "placing",
        "primary",
        "references",
        "returning",
        "right",
        "select",
        "session_user",
        "similar",
        "some",
        "symmetric",
        "table",
        "tablesample",
        "then",
        "to",
        "trailing",
        "true",
        "union",
        "unique",
        "user",
        "using",
        "variadic",
        "verbose",
        "when",
        "where",
        "window",
        "with",
        "abort",
        "absolute",
        "access",
        "action",
        "add",
        "admin",
        "after",
        "aggregate",
        "also",
        "alter",
        "always",
        "assertion",
        "assignment",
        "at",
        "attach",
        "attribute",
        "backward",
        "before",
        "begin",
        "by",
        "cache",
        "call",
        "called",
        "cascade",
        "cascaded",
        "catalog",
        "chain",
        "characteristics",
        "checkpoint",
        "class",
        "close",
        "cluster",
        "comment",
        "comments",
        "commit",
        "committed",
        "configuration",
        "conflict",
        "connection",
        "constraints",
        "content",
        "continue",
        "conversion",
        "copy",
        "cost",
        "csv",
        "cube",
        "current",
        "cursor",
        "cycle",
        "data",
        "database",
        "day",
        "deallocate",
        "declare",
        "defaults",
        "deferred",
        "definer",
        "delete",
        "delimiter",
        "delimiters",
        "depends",
        "detach",
        "dictionary",
        "disable",
        "discard",
        "document",
        "domain",
        "double",
        "drop",
        "each",
        "enable",
        "encoding",
        "encrypted",
        "enum",
        "escape",
        "event",
        "exclude",
        "excluding",
        "exclusive",
        "execute",
        "exists",
        "explain",
        "expression",
        "extension",
        "external",
        "family",
        "filter",
        "first",
        "float",
        "following",
        "force",
        "forward",
        "function",
        "functions",
        "generated",
        "global",
        "granted",
        "grouping",
        "groups",
        "handler",
        "header",
        "hold",
        "hour",
        "identity",
        "if",
        "immediate",
        "immutable",
        "implicit",
        "import",
        "include",
        "including",
        "increment",
        "index",
        "indexes",
        "inherit",
        "inherits",
        "inline",
        "input",
        "insensitive",
        "insert",
        "instead",
        "invoker",
        "isolation",
        "key",
        "label",
        "language",
        "large",
        "last",
        "leakproof",
        "level",
        "listen",
        "load",
        "local",
        "location",
        "lock",
        "locked",
        "logged",
        "mapping",
        "match",
        "materialized",
        "maxvalue",
        "method",
        "minute",
        "minvalue",
        "mode",
        "month",
        "move",
        "name",
        "names",
        "new",
        "next",
        "nfc",
        "nfd",
        "nfkc",
        "nfkd",
        "no",
        "none",
        "normalize",
        "normalized",
        "nothing",
        "notify",
        "nowait",
        "nulls",
        "object",
        "of",
        "off",
        "oids",
        "old",
        "operator",
        "option",
        "options",
        "ordinality",
        "others",
        "over",
        "overriding",
        "owned",
        "owner",
        "parallel",
        "parser",
        "partial",
        "partition",
        "passing",
        "password",
        "plans",
        "policy",
        "position",
        "preceding",
        "prepare",
        "prepared",
        "preserve",
        "prior",
        "privileges",
        "procedural",
        "procedure",
        "procedures",
        "program",
        "publication",
        "quote",
        "range",
        "read",
        "reassign",
        "recheck",
        "recursive",
        "ref",
        "referencing",
        "refresh",
        "reindex",
        "relative",
        "release",
        "rename",
        "repeatable",
        "replace",
        "replica",
        "reset",
        "restart",
        "restrict",
        "return",
        "returns",
        "revoke",
        "role",
        "rollback",
        "rollup",
        "routine",
        "routines",
        "row",
        "rows",
        "rule",
        "savepoint",
        "schema",
        "schemas",
        "scroll",
        "search",
        "second",
        "security",
        "sequence",
        "sequences",
        "serializable",
        "server",
        "session",
        "set",
        "sets",
        "share",
        "show",
        "simple",
        "skip",
        "snapshot",
        "sql",
        "stable",
        "standalone",
        "start",
        "statement",
        "statistics",
        "stdin",
        "stdout",
        "storage",
        "stored",
        "strict",
        "strip",
        "subscription",
        "support",
        "sysid",
        "system",
        "tables",
        "temp",
        "template",
        "temporary",
        "text",
        "ties",
        "transaction",
        "transform",
        "trigger",
        "truncate",
        "trusted",
        "type",
        "types",
        "uescape",
        "unbounded",
        "uncommitted",
        "unencrypted",
        "unknown",
        "unlisten",
        "unlogged",
        "until",
        "update",
        "vacuum",
        "valid",
        "validate",
        "validator",
        "value",
        "values",
        "varying",
        "version",
        "view",
        "views",
        "volatile",
        "whitespace",
        "without",
        "work",
        "wrapper",
        "write",
        "xml",
        "year",
        "yes",
        "zone",
    ]
    .into_iter()
    .collect()
});

/// Wrap a column name in double quotes if it's a PostgreSQL reserved word.
fn quote_if_reserved(name: &str) -> String {
    if PG_RESERVED_WORDS.contains(name.to_lowercase().as_str()) {
        format!("\"{}\"", name)
    } else {
        name.to_string()
    }
}

/// Wrap a string value in PostgreSQL dollar-quoting.
/// Uses `$$` by default; if the value contains `$$`, falls back to `$q$...$q$`.
fn dollar_quote(val: &str) -> String {
    if val.contains("$$") {
        format!("$q${}$q$", val)
    } else {
        format!("$${}$$", val)
    }
}

/// Strip the Rust `r#` keyword escaping prefix from a SQL identifier.
/// This prevents `#` characters from leaking into constraint names
/// (e.g. fk_certification_r#type → fk_certification_type).
fn strip_rsharp(name: &str) -> String {
    name.strip_prefix("r#").unwrap_or(name).to_string()
}

/// DDL artifacts produced from a single column classification.
type DdlArtifacts = (
    Vec<ColumnDef>,
    Vec<ForeignKeyDef>,
    Vec<CheckConstraint>,
    Vec<ColumnComment>,
);

/// Convert a `ColumnInfo` from the composition tree into DDL columns, FKs, and check constraints.
///
/// Returns `None` for ValueObject-classified columns — those are represented as child
/// `CompositionNode`s in the tree, not as columns.
pub(crate) fn column_info_to_ddl(col: &ColumnInfo, table_name: &str) -> Option<DdlArtifacts> {
    let raw_name = &col.name;
    let prop_name = quote_if_reserved(raw_name);
    let description = col.description.as_deref().unwrap_or("");

    let mut columns = Vec::new();
    let mut foreign_keys = Vec::new();
    let mut check_constraints = Vec::new();
    let mut comments = Vec::new();

    match col.classification.as_ref() {
        Some(RefClassificationKind::PrimitiveWrapper)
        | Some(RefClassificationKind::StructuredWrapper)
        | Some(RefClassificationKind::RangeWrapper) => {
            if !description.is_empty() {
                comments.push(ColumnComment {
                    column: prop_name.clone(),
                    comment: description.to_string(),
                });
            }
            let pg_type = if col.postgres_type.is_empty() {
                "TEXT".to_string()
            } else {
                col.postgres_type.clone()
            };
            columns.push(ColumnDef {
                name: prop_name,
                pg_type,
                nullable: col.is_optional,
                default: None,
                is_primary_key: false,
                is_array: false,
            });
        }
        Some(RefClassificationKind::ArrayWrapper) => {
            if !description.is_empty() {
                comments.push(ColumnComment {
                    column: prop_name.clone(),
                    comment: description.to_string(),
                });
            }
            let raw_base = col
                .postgres_type
                .strip_suffix("[]")
                .unwrap_or(&col.postgres_type);
            let pg_type = if raw_base.is_empty() {
                "TEXT".to_string()
            } else {
                raw_base.to_string()
            };
            columns.push(ColumnDef {
                name: prop_name,
                pg_type,
                nullable: col.is_optional,
                default: None,
                is_primary_key: false,
                is_array: true,
            });
        }
        Some(RefClassificationKind::CodelistReference) => {
            // Array codelists are represented as child CompositionNodes, not columns.
            if col.is_array {
                return None;
            }
            let col_name = prop_name;
            if !description.is_empty() {
                comments.push(ColumnComment {
                    column: col_name.clone(),
                    comment: description.to_string(),
                });
            }
            columns.push(ColumnDef {
                name: col_name.clone(),
                pg_type: "TEXT".to_string(),
                nullable: col.is_optional,
                default: None,
                is_primary_key: false,
                is_array: false,
            });
            if let Some(ref fk) = col.fk_target {
                foreign_keys.push(ForeignKeyDef {
                    column_name: strip_rsharp(raw_name),
                    column: col_name,
                    references_schema: fk.schema.clone(),
                    references_table: fk.table.clone(),
                    references_column: fk.column.clone(),
                    on_delete: fk.on_delete.clone(),
                    is_codelist: true,
                });
            }
        }
        Some(RefClassificationKind::EntityReference) => {
            // Array entity refs are represented as junction child tables
            // (CompositionNodes), never as columns on the parent table.
            if col.is_array {
                return None;
            }
            // Only append `_id` when the schema-side column name doesn't
            // already carry the suffix (e.g. `tenantId` -> `tenant_id`).
            let col_name = if raw_name.ends_with("_id") {
                raw_name.to_string()
            } else {
                format!("{}_id", raw_name)
            };
            if !description.is_empty() {
                comments.push(ColumnComment {
                    column: col_name.clone(),
                    comment: description.to_string(),
                });
            }
            columns.push(ColumnDef {
                name: col_name.clone(),
                pg_type: "UUID".to_string(),
                nullable: col.is_optional,
                default: None,
                is_primary_key: false,
                is_array: false,
            });
            if let Some(ref fk) = col.fk_target {
                foreign_keys.push(ForeignKeyDef {
                    column_name: strip_rsharp(&col_name),
                    column: col_name,
                    references_schema: fk.schema.clone(),
                    references_table: fk.table.clone(),
                    references_column: fk.column.clone(),
                    on_delete: fk.on_delete.clone(),
                    is_codelist: false,
                });
            }
        }
        Some(RefClassificationKind::CodelistCheck) | Some(RefClassificationKind::InlineEnum) => {
            // Array CodelistCheck properties are child CompositionNodes, not columns.
            if col.is_array && col.classification == Some(RefClassificationKind::CodelistCheck) {
                return None;
            }
            if !description.is_empty() {
                comments.push(ColumnComment {
                    column: prop_name.clone(),
                    comment: description.to_string(),
                });
            }
            columns.push(ColumnDef {
                name: prop_name.clone(),
                pg_type: "TEXT".to_string(),
                nullable: col.is_optional,
                default: None,
                is_primary_key: false,
                is_array: col.is_array,
            });
            if !col.is_array && !col.check_values.is_empty() {
                let expr = col
                    .check_values
                    .iter()
                    .map(|v| dollar_quote(v))
                    .collect::<Vec<_>>()
                    .join(", ");
                check_constraints.push(CheckConstraint {
                    name: codegraph_naming::truncate_pg_identifier(&format!(
                        "chk_{}_{}",
                        table_name, raw_name
                    )),
                    column: prop_name.clone(),
                    expression: format!("{} IN ({})", prop_name, expr),
                });
            }
        }
        Some(RefClassificationKind::CompositeWrapper)
        | Some(RefClassificationKind::MediaWrapper) => {
            let is_media = col.classification == Some(RefClassificationKind::MediaWrapper);
            let mut col_names_for_check = Vec::new();
            for comp_col in &col.composite_columns {
                let composite_col_name =
                    quote_if_reserved(&format!("{}{}", raw_name, comp_col.suffix));
                if !description.is_empty() {
                    comments.push(ColumnComment {
                        column: composite_col_name.clone(),
                        comment: description.to_string(),
                    });
                }
                if is_media {
                    col_names_for_check.push(composite_col_name.clone());
                }
                columns.push(ColumnDef {
                    name: composite_col_name,
                    pg_type: comp_col.pg_type.clone(),
                    nullable: col.is_optional,
                    default: None,
                    is_primary_key: false,
                    is_array: false,
                });
            }
            if is_media && col_names_for_check.len() == 2 {
                let nulls = col_names_for_check
                    .iter()
                    .map(|c| format!("{c} IS NULL"))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                let not_nulls = col_names_for_check
                    .iter()
                    .map(|c| format!("{c} IS NOT NULL"))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                check_constraints.push(CheckConstraint {
                    name: format!("chk_{}_complete", raw_name),
                    column: raw_name.clone(),
                    expression: format!("({nulls}) OR ({not_nulls})"),
                });
            }
        }
        Some(RefClassificationKind::ValueObject) => {
            // ValueObjects are represented as child CompositionNodes, not columns.
            return None;
        }
        None => {
            if !description.is_empty() {
                comments.push(ColumnComment {
                    column: prop_name.clone(),
                    comment: description.to_string(),
                });
            }
            let pg_type = if col.postgres_type.is_empty() {
                "TEXT".to_string()
            } else {
                col.postgres_type.clone()
            };
            columns.push(ColumnDef {
                name: prop_name,
                pg_type,
                nullable: col.is_optional,
                default: None,
                is_primary_key: false,
                is_array: false,
            });
        }
    }

    Some((columns, foreign_keys, check_constraints, comments))
}

/// FK column a child table/entity uses to reference its parent. Suffix-aware
/// so parents whose table name already ends in `_id` don't double it
/// (`evidence_extracted_field_id` → `evidence_extracted_field_id`, not
/// `evidence_extracted_field_id_id`). Delegates to the codegraph-naming
/// single source of truth (issue #460) shared with entity/repository
/// generation.
pub(crate) fn child_parent_fk_column(parent_table_name: &str) -> String {
    codegraph_naming::child_parent_fk_column(parent_table_name)
}

/// Recursively ensure every child table (and nested child) carries the
/// tenant column, so org-isolation RLS policies can filter on it.
fn inject_child_tenant_columns(children: &mut [ChildTableDef], tenant_col: &ColumnDef) {
    for child in children {
        if !child.columns.iter().any(|c| c.name == tenant_col.name) {
            child.columns.insert(0, tenant_col.clone());
        }
        inject_child_tenant_columns(&mut child.child_tables, tenant_col);
    }
}

/// Convert a child `CompositionNode` into a `ChildTableDef`, recursively processing
/// nested children.
pub(super) fn composition_node_to_child_table(
    node: &CompositionNode,
    parent_table_name: &str,
    parent_schema_name: &str,
    parent_display_name: &str,
    generated_tables: &HashSet<(String, String)>,
) -> ChildTableDef {
    let child_table_name = codegraph_naming::child_table_name(parent_table_name, &node.field_name);
    let child_display_name = format!("{} {}", parent_display_name, node.field_name);

    let mut columns = Vec::new();
    let mut foreign_keys = Vec::new();
    let mut check_constraints = Vec::new();
    let mut comments = Vec::new();

    // Emit composite range column if present
    if let Some(ref range) = node.composite_range {
        columns.push(ColumnDef {
            name: range.pg_column_name.clone(),
            pg_type: range.pg_type.clone(),
            nullable: true,
            default: None,
            is_primary_key: false,
            is_array: false,
        });
    }

    // Convert ColumnInfo → DDL artifacts
    for col in &node.columns {
        if col.name == "id" {
            continue;
        }
        if let Some((cols, fks, checks, cmts)) = column_info_to_ddl(col, &child_table_name) {
            columns.extend(cols);
            foreign_keys.extend(fks);
            check_constraints.extend(checks);
            comments.extend(cmts);
        }
    }

    // Deduplicate check constraints by name
    {
        let mut seen = HashSet::new();
        check_constraints.retain(|chk| seen.insert(chk.name.clone()));
    }

    // Deduplicate columns
    {
        let mut seen = HashSet::new();
        columns.retain(|col| seen.insert(col.name.clone()));
    }

    // Remove any column that collides with the parent FK column
    let parent_fk_col = child_parent_fk_column(parent_table_name);
    columns.retain(|col| col.name != parent_fk_col);

    // The child-table template appends standard created_at/updated_at audit
    // columns; schema-derived timestamps (e.g. provenance.createdAt) would
    // declare them twice, so schema-derived standards are dropped here.
    columns.retain(|col| {
        let normalized = codegraph_naming::to_snake_case(&col.name).to_lowercase();
        normalized != "created_at" && normalized != "updated_at"
    });

    // Filter FK constraints: only keep FKs whose target table belongs to a
    // generated entity (same logic as the parent table FK filter).
    foreign_keys.retain(|fk| {
        fk.is_codelist
            || generated_tables
                .contains(&(fk.references_schema.clone(), fk.references_table.clone()))
    });

    // Recursively convert nested children
    let nested_children: Vec<ChildTableDef> = node
        .children
        .iter()
        .map(|child| {
            composition_node_to_child_table(
                child,
                &child_table_name,
                parent_schema_name,
                &child_display_name,
                generated_tables,
            )
        })
        .collect();

    ChildTableDef {
        schema_name: parent_schema_name.to_string(),
        table_name: child_table_name,
        parent_fk_column: parent_fk_col,
        parent_schema: parent_schema_name.to_string(),
        parent_table: parent_table_name.to_string(),
        columns,
        display_name: child_display_name,
        comments,
        foreign_keys,
        check_constraints,
        child_tables: nested_children,
    }
}

impl DdlGenerator {
    /// Build this entity's DDL context.
    ///
    /// `pub(crate)` because it IS the shared trigger enumeration: the
    /// semantic event model (`crate::events::enumerate_event_trigger_tables`)
    /// calls it per generation-order entry so its per-domain trigger
    /// publications reproduce the per-entity event-trigger set byte for
    /// byte, by construction.
    pub(crate) async fn query_ddl_context(
        &self,
        db: &dyn GraphQuerier,
        schema_title: &str,
        domain: &str,
        config: &DomainConfig,
    ) -> Result<DdlContext> {
        let schema = db
            .get_schema_in_domain(schema_title, domain)
            .await?
            .ok_or_else(|| crate::error::Error::SchemaNotFound(schema_title.into()))?;

        let schema_name = domain.to_string();
        let table_name = schema.pg_table_name.clone();
        let display_name = schema.rust_type_name.clone();

        // Get the pre-built composition tree — it already contains resolved columns,
        // FK targets, check constraint values, composite ranges, and child nodes
        // for ValueObject properties.
        let tree = db.get_composition_tree(schema_title).await?;
        let root = &tree.root;

        let mut artifacts = DdlAccumulators::new(root);

        // Inject FK column for parent-child relationships detected from the schema graph.
        // Honor the schema's `required` when the FK corresponds to a real property;
        // synthetic ArrayItems FKs (no child-side property) stay nullable.
        let entity_cfg = config
            .domains
            .get(domain)
            .and_then(|d| d.get_entity_config(&schema.rust_type_name));
        add_parent_fk(
            db,
            config,
            domain,
            schema_title,
            entity_cfg,
            &self.parent_candidates,
            &mut artifacts,
        )
        .await;

        // Inject self-referential FK column for hierarchy entities
        add_hierarchy_artifacts(entity_cfg, &mut artifacts, &schema_name, &table_name);

        // Convert tree columns to DDL artifacts — no graph queries needed
        add_tree_columns(root, &table_name, &mut artifacts);

        // Filter FK constraints: only keep FKs whose target table belongs to an
        // entity that is actually configured in some domain (will have a migration).
        // This prevents phantom FK constraints to:
        // - VO types classified as entities by the auto-classifier but not in any domain's entities list
        // - Excluded types that exist in a different domain
        // - Cross-domain refs pointing to non-existent schemas (e.g. "jdx")
        // Build (schema, table) pairs — a FK is valid only if its target
        // (references_schema, references_table) matches a generated entity.
        let generated_tables = generated_table_set(db, config, domain).await;
        retain_generated_fks(&mut artifacts.foreign_keys, &generated_tables);

        // Query graph properties for entity-reference columns that the composition
        // tree may have missed (e.g. cross-domain references where the target schema
        // can't be resolved). The entity model generator includes these columns,
        // so the DDL must match to avoid "column does not exist" at runtime.
        //
        // ValueObject properties are handled by the composition tree as child
        // tables (not FK columns on the parent), so we only emit FK columns for
        // EntityReference properties whose target schema actually has a table
        // (i.e. is an entity configured in some domain). This prevents phantom
        // FK constraints to VO types that were force-classified but never
        // generated as entities, or to entities excluded in their domain.
        add_property_ref_columns(
            db,
            schema_title,
            &schema_name,
            &generated_tables,
            &mut artifacts,
        )
        .await;

        // Convert child CompositionNodes → ChildTableDefs
        let mut child_tables: Vec<ChildTableDef> = root
            .children
            .iter()
            .map(|child| {
                composition_node_to_child_table(
                    child,
                    &table_name,
                    &schema_name,
                    &display_name,
                    &generated_tables,
                )
            })
            .collect();

        // Append-only snapshot semantics (issue #284): explicit config flag
        // OR the same operations inference the scaffold grants use
        // (effective operations exclude update AND delete). The explicit
        // flag with update/delete ops is a parse-time config error
        // (validate_append_only_config), so the two sources agree.
        let entity_cfg_early = config
            .domains
            .get(domain)
            .and_then(|d| d.get_entity_config(schema_title));
        let append_only = entity_cfg_early.is_some_and(|ec| ec.is_append_only()) || {
            let ops =
                crate::api::api_model::resolve_entity_operations(db, config, domain, schema_title)
                    .await;
            !ops.iter().any(|op| op == "update" || op == "delete")
        };

        // Add standard timestamp columns and determine tenancy
        let (has_updated_at, is_tenant_scoped) = add_timestamp_and_tenant_columns(
            &mut artifacts.columns,
            &mut child_tables,
            config,
            &table_name,
            append_only,
        );

        // Deduplicate columns by name — CompositeWrapper expansion from
        // allOf-inherited properties can produce duplicate expanded columns.
        // Deduplicate foreign keys by constraint name — cross-domain
        // schema merging via allOf produces duplicate FK definitions.
        // Deduplicate comments by column name — same root cause.
        dedup_ddl_artifacts(&mut artifacts);

        // Query required extensions
        let mut extensions: Vec<String> = db
            .get_required_extensions(schema_title)
            .await
            .unwrap_or_default()
            .iter()
            .map(|ext| ext.name.clone())
            .collect();

        let domain = schema_name.clone();

        // Check if this entity has a workflow config
        let has_workflow =
            apply_workflow_defaults(config, &domain, schema_title, &mut artifacts.columns);

        let resource_name = table_name.replace('_', "-");

        // Flatten the recursive child table tree into a depth-first ordered list.
        // The template iterates this flat list; each entry carries its own parent info.
        let flat_child_tables = flatten_child_tables(child_tables);

        // Build search infrastructure (FTS + embeddings) from config + graph metadata
        let search_config = config
            .domains
            .get(&domain)
            .and_then(|d| d.get_entity_config(schema_title))
            .map(|ec| &ec.search);

        let fts = build_fts_context(search_config, &artifacts.columns, &table_name);

        let embeddings = build_embedding_contexts(search_config, &table_name);

        // Add pgvector extension if embeddings are configured
        if !embeddings.is_empty() && !extensions.contains(&"vector".to_string()) {
            extensions.push("vector".to_string());
        }

        // Detect extensions required by column types (safety net for transitive refs
        // that the ingestion pass may miss, e.g. PersonType → AddressType → GeoType).
        for ext in detect_extensions_from_columns(&artifacts.columns, &flat_child_tables) {
            if !extensions.contains(&ext) {
                extensions.push(ext);
            }
        }

        let is_auditable = config
            .domains
            .get(&domain)
            .and_then(|d| d.auditable)
            .unwrap_or(true)
            && !append_only;

        // Role enforcement (#169): same gate the router used for its
        // permission layers — entities with `permissions.scope` configured —
        // plus any entity carrying explicit `min_roles`. The per-op minima
        // default to the built-in matrix when not configured.
        let entity_cfg = entity_cfg_early;
        let min_roles_cfg = entity_cfg.and_then(|ec| ec.permissions.min_roles.clone());
        let role_enforced = entity_cfg
            .map(|ec| {
                ec.permissions
                    .scope
                    .as_ref()
                    .map(|s| !s.is_empty())
                    .unwrap_or(false)
                    || min_roles_cfg
                        .as_ref()
                        .map(|m| !m.is_empty())
                        .unwrap_or(false)
            })
            .unwrap_or(false);

        let roles_hierarchy = config
            .rbac
            .as_ref()
            .map(|r| r.hierarchy_or_default())
            .unwrap_or_else(codegraph_config::config::default_roles_hierarchy);

        // Canonical op order; explicit config wins over the synthesized
        // defaults.
        let mut role_minima: Vec<RoleMinimum> = Vec::new();
        for (op, default_min) in codegraph_config::config::default_min_roles() {
            let min_role = min_roles_cfg
                .as_ref()
                .and_then(|m| m.get(op))
                .cloned()
                .unwrap_or_else(|| default_min.to_string());
            role_minima.push(RoleMinimum {
                operation: op.to_string(),
                min_role,
            });
        }

        let user_scope_column = entity_cfg.and_then(|ec| ec.permissions.user_scope_column.clone());

        // Detect whether this entity has a _codelist.sql migration (codelist seed data).
        // The codelist generator only creates these for codelist entities in the
        // 'common' domain. Entities outside 'common' are always created by the entity
        // DDL generator with id UUID PRIMARY KEY, even if classified as codelists.
        let _has_codelist_seed = schema.is_codelist
            && domain == "common"
            && !db
                .get_enum_values(schema_title)
                .await
                .unwrap_or_default()
                .is_empty();

        let DdlAccumulators {
            columns,
            foreign_keys,
            check_constraints,
            comments,
            indexes,
        } = artifacts;

        // Debug-time invariant (#446 AC): the DDL context must resolve at
        // least the property-derived columns the dto context resolves. A
        // divergence is the audit-only-skeleton signature — generation is
        // silent while the emitted repository/command code reads columns
        // the table and model lack (E0560/E0609/E0599). Loud advisory, not
        // fatal: the dto plane is the reference resolution.
        #[cfg(debug_assertions)]
        warn_on_ddl_dto_gap(db, schema_title, &domain, config, &columns).await;

        Ok(DdlContext {
            schema_name,
            table_name,
            display_name,
            domain,
            columns,
            primary_key: "id".to_string(),
            foreign_keys,
            check_constraints,
            indexes,
            has_updated_at,
            is_tenant_scoped,
            tenant_table: "common.tenant".to_string(),
            extensions,
            child_tables: flat_child_tables,
            comments,
            has_workflow,
            resource_name,
            fts,
            embeddings,
            is_auditable,
            role_enforced,
            role_minima,
            roles_hierarchy,
            user_scope_column,
            has_demo_flag: is_auditable,
            is_codelist: schema.is_codelist,
            append_only,
        })
    }
}

async fn add_parent_fk(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    domain: &str,
    schema_title: &str,
    entity_cfg: Option<&codegraph_config::EntityConfig>,
    parent_candidates: &[codegraph_core::types::ParentCandidate],
    artifacts: &mut DdlAccumulators,
) {
    let ddl_props = db.get_properties(schema_title).await.unwrap_or_default();
    if let Some(fk_col) = crate::resolve_parent_fk_column(
        schema_title,
        parent_candidates,
        entity_cfg,
        &config.defaults.type_suffix,
    ) {
        let is_required = ddl_props.iter().any(|p| {
            codegraph_core::types::resolve_field(p).rust_field_name == fk_col
                || p.pg_column_name == fk_col
        }) && ddl_props
            .iter()
            .find_map(|p| {
                let fd = codegraph_core::types::resolve_field(p);
                if fd.rust_field_name == fk_col || p.pg_column_name == fk_col {
                    Some(p.is_required)
                } else {
                    None
                }
            })
            .unwrap_or(false);
        artifacts.columns.push(ColumnDef {
            name: fk_col.clone(),
            pg_type: "UUID".to_string(),
            nullable: !is_required,
            default: None,
            is_primary_key: false,
            is_array: false,
        });
        // Resolve the parent's schema and table for the FK constraint.
        // Manual config takes priority over graph detection.
        let mut fk_resolved = false;
        if let Some(ec) = entity_cfg
            && ec.role.as_deref() == Some("child")
            && let Some(ref parent_title) = ec.parent
            && let Ok(Some(parent_schema)) = db.get_schema_in_domain(parent_title, domain).await
        {
            let parent_domain = if config
                .domains
                .get(domain)
                .map(|d| d.entities.contains(parent_title))
                .unwrap_or(false)
            {
                domain
            } else {
                parent_schema.domain.as_deref().unwrap_or(domain)
            };
            artifacts.foreign_keys.push(ForeignKeyDef {
                column_name: strip_rsharp(&fk_col),
                column: fk_col.clone(),
                references_schema: parent_domain.to_string(),
                references_table: parent_schema.pg_table_name.clone(),
                references_column: "id".to_string(),
                on_delete: "CASCADE".to_string(),
                is_codelist: false,
            });
            fk_resolved = true;
        }
        if !fk_resolved {
            let stripped =
                crate::api::router::strip_suffix(schema_title, &config.defaults.type_suffix);
            if let Some(pc) = parent_candidates.iter().find(|pc| {
                crate::api::router::strip_suffix(&pc.child_title, &config.defaults.type_suffix)
                    == stripped
            }) && let Ok(Some(parent_schema)) =
                db.get_schema_in_domain(&pc.parent_title, domain).await
            {
                let parent_domain = if config
                    .domains
                    .get(domain)
                    .map(|d| d.entities.contains(&pc.parent_title))
                    .unwrap_or(false)
                {
                    domain
                } else {
                    parent_schema.domain.as_deref().unwrap_or(domain)
                };
                artifacts.foreign_keys.push(ForeignKeyDef {
                    column_name: strip_rsharp(&fk_col),
                    column: fk_col,
                    references_schema: parent_domain.to_string(),
                    references_table: parent_schema.pg_table_name.clone(),
                    references_column: "id".to_string(),
                    on_delete: "CASCADE".to_string(),
                    is_codelist: false,
                });
            }
        }
    }
}

fn add_hierarchy_artifacts(
    entity_cfg: Option<&codegraph_config::EntityConfig>,
    artifacts: &mut DdlAccumulators,
    schema_name: &str,
    table_name: &str,
) {
    if let Some(ec) = entity_cfg
        && let Some(ref hierarchy_field) = ec.hierarchy_field
    {
        artifacts.columns.push(ColumnDef {
            name: hierarchy_field.clone(),
            pg_type: "UUID".to_string(),
            nullable: true,
            default: None,
            is_primary_key: false,
            is_array: false,
        });
        artifacts.foreign_keys.push(ForeignKeyDef {
            column: hierarchy_field.clone(),
            column_name: strip_rsharp(hierarchy_field),
            references_schema: schema_name.to_string(),
            references_table: table_name.to_string(),
            references_column: "id".to_string(),
            is_codelist: false,
            on_delete: "SET NULL".to_string(),
        });
        artifacts.indexes.push(IndexDef {
            name: format!("idx_{}_{}", table_name, hierarchy_field),
            columns: vec![hierarchy_field.clone()],
            unique: false,
        });
    }
}

fn add_tree_columns(
    root: &codegraph_core::types::CompositionNode,
    table_name: &str,
    artifacts: &mut DdlAccumulators,
) {
    for col in &root.columns {
        if col.name == "id" {
            continue;
        }
        if let Some((cols, fks, checks, cmts)) = column_info_to_ddl(col, table_name) {
            artifacts.columns.extend(cols);
            artifacts.foreign_keys.extend(fks);
            artifacts.check_constraints.extend(checks);
            artifacts.comments.extend(cmts);
        }
    }

    // Deduplicate check constraints by name — duplicate ColumnInfo entries
    // from cross-domain schema merging produce duplicate constraints.
    {
        let mut seen = HashSet::new();
        artifacts
            .check_constraints
            .retain(|chk| seen.insert(chk.name.clone()));
    }
}

/// The set of (schema, table) pairs that will actually have migrations:
/// the config `entities` lists UNION the graph's own entity tables (mox is
/// author-declarative — its domains.toml carries no `entities` key, so the
/// graph's `is_entity` schemas are the authority for FK-target validity,
/// issue #460). Config-listed entities keep priority for legacy flows;
/// graph entities are restricted to domains the config knows about so
/// excluded domains still suppress their FKs.
async fn generated_table_set(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    default_domain: &str,
) -> HashSet<(String, String)> {
    let mut tables: HashSet<(String, String)> = config
        .domains
        .values()
        .flat_map(|d| {
            let schema = &d.postgres_schema;
            d.entities.iter().map(move |title| {
                let table =
                    codegraph_naming::to_snake_case(&codegraph_naming::strip_suffix(title, "Type"));
                (schema.clone(), table)
            })
        })
        .collect();
    if let Ok(schemas) = db.list_schemas(None).await {
        for schema in schemas {
            if !schema.is_entity || schema.pg_table_name.is_empty() {
                continue;
            }
            let domain = match schema.domain.as_deref() {
                Some(d) if config.domains.contains_key(d) => d.to_string(),
                // Graph-assigned domains unknown to the config are excluded
                // surfaces — their tables never generate.
                Some(_) => continue,
                None => default_domain.to_string(),
            };
            tables.insert((domain, schema.pg_table_name));
        }
    }
    tables
}

fn retain_generated_fks(
    foreign_keys: &mut Vec<ForeignKeyDef>,
    generated_tables: &HashSet<(String, String)>,
) {
    foreign_keys.retain(|fk| {
        fk.is_codelist
            || generated_tables
                .contains(&(fk.references_schema.clone(), fk.references_table.clone()))
    });
}

async fn add_property_ref_columns(
    db: &dyn GraphQuerier,
    schema_title: &str,
    schema_name: &str,
    generated_tables: &HashSet<(String, String)>,
    artifacts: &mut DdlAccumulators,
) {
    if let Ok(props) = db.get_properties(schema_title).await {
        let existing_names: std::collections::HashSet<String> =
            artifacts.columns.iter().map(|c| c.name.clone()).collect();

        for prop in &props {
            let kind = prop.effective_kind();
            if kind != Some(RefClassificationKind::EntityReference) {
                continue;
            }
            // Array entity refs are junction/FK-on-child relationships
            // materialized elsewhere (child tables, child-side FKs) — never
            // columns on this table.
            if prop.is_array {
                continue;
            }
            let base = prop
                .rust_field_name
                .strip_prefix("r#")
                .unwrap_or(&prop.rust_field_name);
            let col_name = if base.ends_with("_id") {
                base.to_string()
            } else {
                format!("{}_id", base)
            };
            if !existing_names.contains(col_name.as_str()) {
                // Try to resolve the FK target for the constraint.
                // Only emit the FK if the target schema is an entity
                // configured in some domain (has its own table/migration).
                // VOs, excluded types, and entity types not in any domain's
                // entities list don't have tables, so FK constraints to them
                // would fail with "undefined_table".
                if let Ok(Some(target)) = db.get_property_ref_target(&prop.name, schema_title).await
                    && !target.pg_table_name.is_empty()
                    && target.is_entity
                {
                    artifacts.columns.push(ColumnDef {
                        name: col_name.clone(),
                        pg_type: "UUID".to_string(),
                        nullable: true,
                        default: None,
                        is_primary_key: false,
                        is_array: false,
                    });
                    let fk_schema = target.domain.as_deref().unwrap_or(schema_name);
                    artifacts.foreign_keys.push(ForeignKeyDef {
                        column_name: strip_rsharp(&prop.rust_field_name),
                        column: col_name,
                        references_schema: fk_schema.to_string(),
                        references_table: target.pg_table_name.clone(),
                        is_codelist: false,
                        references_column: "id".to_string(),
                        on_delete: "SET NULL".to_string(),
                    });
                }
            }
        }

        // Re-filter after adding cross-domain FKs (same logic as above)
        retain_generated_fks(&mut artifacts.foreign_keys, generated_tables);
    }
}

fn add_timestamp_and_tenant_columns(
    columns: &mut Vec<ColumnDef>,
    child_tables: &mut [ChildTableDef],
    config: &DomainConfig,
    table_name: &str,
    append_only: bool,
) -> (bool, bool) {
    // Add standard timestamp columns
    columns.push(ColumnDef {
        name: "created_at".to_string(),
        pg_type: "TIMESTAMPTZ".to_string(),
        nullable: false,
        default: Some("now()".to_string()),
        is_primary_key: false,
        is_array: false,
    });
    // Append-only tables keep created_at (the row's birth) but never gain
    // an updated_at column — states are never mutated in place (#284).
    if !append_only {
        columns.push(ColumnDef {
            name: "updated_at".to_string(),
            pg_type: "TIMESTAMPTZ".to_string(),
            nullable: false,
            default: Some("now()".to_string()),
            is_primary_key: false,
            is_array: false,
        });
    }
    let has_updated_at = !append_only;

    // Determine tenancy
    let is_tenant_scoped = !is_global_entity(table_name, config);

    // Add platform_organization_id for tenant-scoped entities
    if is_tenant_scoped {
        columns.insert(
            1,
            ColumnDef {
                name: "platform_organization_id".to_string(),
                pg_type: "UUID".to_string(),
                nullable: false,
                default: Some("'00000000-0000-0000-0000-000000000000'::UUID".to_string()),
                is_primary_key: false,
                is_array: false,
            },
        );

        // Child tables (junction + VO) inherit the parent's tenancy: they
        // carry the same tenant column so org-isolation RLS policies can
        // be applied to them too. Nested children recurse.
        let tenant_col = ColumnDef {
            name: "platform_organization_id".to_string(),
            pg_type: "UUID".to_string(),
            nullable: false,
            default: Some("'00000000-0000-0000-0000-000000000000'::UUID".to_string()),
            is_primary_key: false,
            is_array: false,
        };
        inject_child_tenant_columns(child_tables, &tenant_col);
    }

    (has_updated_at, is_tenant_scoped)
}

fn dedup_ddl_artifacts(artifacts: &mut DdlAccumulators) {
    // Deduplicate columns by name — CompositeWrapper expansion from
    // allOf-inherited properties can produce duplicate expanded columns.
    {
        let mut seen = std::collections::HashSet::new();
        artifacts
            .columns
            .retain(|col| seen.insert(col.name.clone()));
    }

    // Deduplicate foreign keys by constraint name — cross-domain
    // schema merging via allOf produces duplicate FK definitions.
    {
        let mut seen = std::collections::HashSet::new();
        artifacts
            .foreign_keys
            .retain(|fk| seen.insert(fk.column_name.clone()));
    }

    // Deduplicate comments by column name — same root cause.
    {
        let mut seen = std::collections::HashSet::new();
        artifacts.comments.retain(|c| seen.insert(c.column.clone()));
    }
}

pub(super) fn apply_workflow_defaults(
    config: &DomainConfig,
    domain: &str,
    schema_title: &str,
    columns: &mut [ColumnDef],
) -> bool {
    // Check if this entity has a workflow config
    let workflow_cfg = config
        .domains
        .get(domain)
        .and_then(|d| d.get_entity_config(schema_title))
        .and_then(|ec| ec.workflow.as_ref());

    let has_workflow = workflow_cfg
        .map(|wf| wf.generate_action_endpoints)
        .unwrap_or(false);

    // Issue #311: the workflow plane owns the status field — the create DTO
    // excludes it, so every API INSERT omits the column and the row used to
    // land with a NULL status (list badges/chips rendered nothing until the
    // first transition). Emit the initial_state as the DB DEFAULT regardless
    // of column nullability, so creates materialize the workflow at its
    // initial state and both pipelines' list badges work on created rows
    // (matching the details/form view, which reads the {id}/workflow
    // endpoint). Single source: WorkflowConfig.initial_state. Explicit
    // schema/config defaults win. Fresh generates only — pre-existing
    // databases get no backfill; their NULL-status rows stay as they are.
    if let Some(wf) = workflow_cfg {
        for col in columns.iter_mut() {
            if col.name == wf.status_field && col.default.is_none() {
                col.default = Some(pg_string_default(&wf.initial_state));
            }
        }
    }

    has_workflow
}

/// A single-quoted SQL string literal for a column DEFAULT, with embedded
/// single quotes doubled. Workflow state names are identifiers in practice;
/// the escaping keeps a hostile config from breaking out of the literal.
fn pg_string_default(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Mapping from Postgres extension name to the column type patterns that require it.
const EXTENSION_TYPE_PATTERNS: &[(&str, &[&str])] = &[
    ("postgis", &["GEOMETRY", "GEOGRAPHY"]),
    ("vector", &["VECTOR"]),
];

/// Scan column pg_type values and return any Postgres extensions they require.
/// Generator-side safety net for transitive references the ingestion pass may miss.
pub(super) fn detect_extensions_from_columns(
    columns: &[ColumnDef],
    child_tables: &[ChildTableDef],
) -> Vec<String> {
    let mut found = Vec::new();
    let mut check = |pg_type: &str| {
        let upper = pg_type.to_uppercase();
        for &(ext, patterns) in EXTENSION_TYPE_PATTERNS {
            if patterns.iter().any(|p| upper.contains(p))
                && !found.iter().any(|s: &String| s == ext)
            {
                found.push(ext.to_string());
            }
        }
    };
    for col in columns {
        check(&col.pg_type);
    }
    for child in child_tables {
        for col in &child.columns {
            check(&col.pg_type);
        }
    }
    found
}

/// System-managed columns that should never be included in full-text search.
const FTS_EXCLUDED_COLUMNS: &[&str] =
    &["id", "platform_organization_id", "created_at", "updated_at"];

/// Build FTS context from search config and auto-discovered TEXT columns.
///
/// When `fts_columns` is `None` in config, auto-discovers all TEXT data columns.
/// When `fts_columns` is `Some([])` (explicit empty), FTS is disabled.
/// When `fts_columns` is `Some([...])`, uses the explicit list.
fn build_fts_context(
    search_config: Option<&SearchConfig>,
    columns: &[ColumnDef],
    table_name: &str,
) -> Option<FtsContext> {
    let defaults = SearchConfig::default();
    let cfg = search_config.unwrap_or(&defaults);

    let column_names: HashSet<&str> = columns.iter().map(|c| c.name.as_str()).collect();

    let fts_columns: Vec<String> = match &cfg.fts_columns {
        // Explicit empty = FTS disabled
        Some(cols) if cols.is_empty() => return None,
        // Explicit list — filter to columns that actually exist in this table
        Some(cols) => cols
            .iter()
            .filter(|c| column_names.contains(c.as_str()))
            .cloned()
            .collect(),
        // When fts_weights are specified but fts_columns is None, use the weight
        // keys as the FTS columns — but only those that exist in this table
        None if !cfg.fts_weights.is_empty() => cfg
            .fts_weights
            .keys()
            .filter(|c| column_names.contains(c.as_str()))
            .cloned()
            .collect(),
        // Auto-discover: all TEXT columns that aren't system-managed or FKs
        None => columns
            .iter()
            .filter(|c| {
                c.pg_type == "TEXT"
                    && !c.is_primary_key
                    && !c.is_array
                    && !FTS_EXCLUDED_COLUMNS.contains(&c.name.as_str())
                    && !c.name.ends_with("_id")
                    && !c.name.ends_with("_code")
            })
            .map(|c| c.name.clone())
            .collect(),
    };

    if fts_columns.is_empty() {
        return None;
    }

    let weighted_columns: Vec<FtsColumnWeight> = fts_columns
        .iter()
        .map(|col| {
            let weight = cfg
                .fts_weights
                .get(col)
                .cloned()
                .unwrap_or_else(|| "D".to_string());
            FtsColumnWeight {
                column: col.clone(),
                weight,
            }
        })
        .collect();

    Some(FtsContext {
        tsvector_column: "search_tsv".to_string(),
        language: cfg.fts_language.clone(),
        weighted_columns,
        index_name: format!("idx_{}_search_tsv", table_name),
    })
}

/// Build embedding contexts from explicit search config.
/// Embedding columns are never auto-discovered — must be explicitly configured.
fn build_embedding_contexts(
    search_config: Option<&SearchConfig>,
    table_name: &str,
) -> Vec<EmbeddingContext> {
    let cfg = match search_config {
        Some(c) if !c.embedding_columns.is_empty() => c,
        _ => return Vec::new(),
    };

    cfg.embedding_columns
        .iter()
        .map(|col| EmbeddingContext {
            source_column: col.clone(),
            vector_column: format!("{}_embedding", col),
            dimensions: cfg.embedding_dimensions,
            index_name: format!("idx_{}_{}_embedding", table_name, col),
        })
        .collect()
}

/// Flatten a recursive child table tree into a depth-first ordered Vec.
/// Each child table already carries its own parent_schema/parent_table for FK references.
fn flatten_child_tables(children: Vec<ChildTableDef>) -> Vec<ChildTableDef> {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    flatten_child_tables_inner(children, &mut result, &mut seen);
    result
}

fn flatten_child_tables_inner(
    children: Vec<ChildTableDef>,
    result: &mut Vec<ChildTableDef>,
    seen: &mut std::collections::HashSet<String>,
) {
    for mut child in children {
        let nested = std::mem::take(&mut child.child_tables);
        if seen.insert(child.table_name.clone()) {
            result.push(child);
        } else {
            eprintln!(
                "DDL: skipping duplicate child table '{}' (same VO type referenced by multiple properties)",
                child.table_name
            );
        }
        flatten_child_tables_inner(nested, result, seen);
    }
}

fn is_global_entity(_table_name: &str, _config: &DomainConfig) -> bool {
    // TODO: check tenancy config for global tables
    false
}

/// Property-derived dto fields with no backing column in the DDL context —
/// the audit-only-skeleton signature (#446: DDL/entity resolve fewer
/// properties than dto/repository/command/query, and the emitted
/// repository reads columns the table and model lack).
///
/// Exclusions mirror planes that legitimately produce no column on this
/// table: `id` (hardcoded primary key), hierarchy fields (synthetic
/// self-referential FK, added by `add_hierarchy_artifacts`), and array
/// entity references (junction child tables). Codelist fields strip a
/// trailing `_code` on the Rust side (`status_code` column ↔ `status`
/// field), so the suffixed column name is accepted too.
pub(crate) fn ddl_dto_gap_fields(
    ddl_columns: &[ColumnDef],
    dto_fields: &[crate::ddd::dto::DtoField],
) -> Vec<String> {
    // Reserved-word columns are double-quoted in the DDL context
    // (`"name"`) — compare on the bare identifier.
    let column_names: HashSet<&str> = ddl_columns
        .iter()
        .map(|c| c.name.trim_matches('"'))
        .collect();
    let mut gaps = Vec::new();
    for field in dto_fields {
        if field.name == "id" || field.is_hierarchy_field {
            continue;
        }
        if field.is_array && field.is_entity_ref {
            continue;
        }
        if column_names.contains(field.name.as_str()) {
            continue;
        }
        let code_suffixed = format!("{}_code", field.name);
        if column_names.contains(code_suffixed.as_str()) {
            continue;
        }
        gaps.push(field.name.clone());
    }
    gaps
}

/// Loud advisory wiring for [`ddl_dto_gap_fields`] (debug builds only —
/// release runs skip the extra dto-context resolution).
#[cfg(debug_assertions)]
async fn warn_on_ddl_dto_gap(
    db: &dyn GraphQuerier,
    schema_title: &str,
    domain: &str,
    config: &DomainConfig,
    columns: &[ColumnDef],
) {
    let Ok(dto_ctx) = crate::ddd::dto::build_dto_context(db, schema_title, domain, config).await
    else {
        return;
    };
    let gaps = ddl_dto_gap_fields(columns, &dto_ctx.fields);
    if gaps.is_empty() {
        return;
    }
    let message = format!(
        "ddl/dto column divergence for {domain}.{schema_title}: the dto context resolves {} \
         field(s) the DDL context has no column for: {} (audit-only skeleton signature, \
         issue #446)",
        gaps.len(),
        gaps.join(", ")
    );
    eprintln!("warning: ddl-dto-gap: {message}");
    tracing::warn!("{message}");
}

/// The `(parent_title, projected_table)` pair when `schema_title`'s parent
/// projects a `{parent}_{feature}` child table equal to the schema's own
/// `pg_table_name` (the standalone emission renders the parent's child
/// shape and the parent skips its duplicate block).
pub(super) async fn composition_table_conflict(
    db: &dyn GraphQuerier,
    schema_title: &str,
) -> Result<Option<(String, String)>> {
    Ok(db.composition_table_conflict(schema_title).await?)
}
