use serde::Serialize;

/// One per-operation minimum role for the role_enforced_* policies (#169).
#[derive(Debug, Clone, Serialize)]
pub struct RoleMinimum {
    pub operation: String,
    pub min_role: String,
}

/// Context for DDL table generation.
///
/// Identifier fields (`ColumnDef.name`, `ForeignKeyDef.column`,
/// `ColumnComment.column`, `CheckConstraint.column`, FTS weighted
/// columns) carry BARE names (#451); quoting for reserved words happens
/// only on the render clone (`quote_ddl_identifiers`).
#[derive(Debug, Clone, Serialize)]
pub struct DdlContext {
    pub schema_name: String,
    pub table_name: String,
    pub display_name: String,
    pub domain: String,
    pub columns: Vec<ColumnDef>,
    pub primary_key: String,
    pub foreign_keys: Vec<ForeignKeyDef>,
    pub check_constraints: Vec<CheckConstraint>,
    pub indexes: Vec<IndexDef>,
    pub has_updated_at: bool,
    pub is_tenant_scoped: bool,
    pub tenant_table: String,
    pub extensions: Vec<String>,
    pub child_tables: Vec<ChildTableDef>,
    pub comments: Vec<ColumnComment>,
    /// Whether this entity has a workflow config (generates process history view).
    pub has_workflow: bool,
    /// Kebab-case resource name for RLS scope checks (e.g. "candidate").
    pub resource_name: String,
    /// Full-text search configuration (tsvector column, GIN index, trigger).
    pub fts: Option<FtsContext>,
    /// Embedding columns for semantic search (pgvector).
    pub embeddings: Vec<EmbeddingContext>,
    /// Whether this entity tracks soft deletes and audit columns.
    pub is_auditable: bool,
    /// Whether the entity has `permissions.scope` configured (#169): gates
    /// the RESTRICTIVE role_enforced_* RLS policies that replace the request
    /// path's permission middleware.
    pub role_enforced: bool,
    /// Per-operation minimum roles for the role_enforced_* policies (#169),
    /// in canonical op order (create, read, update, delete, list). Defaults
    /// are synthesized from the built-in matrix when `permissions.min_roles`
    /// is not configured.
    pub role_minima: Vec<RoleMinimum>,
    /// The configured role hierarchy (#169) — drives `role_rank()` and the
    /// custom-role `ALTER TYPE` values in the RBAC migration.
    pub roles_hierarchy: Vec<String>,
    /// Optional per-row user scoping (#169): when configured, the RLS file
    /// adds the column + auto-set trigger and a RESTRICTIVE
    /// `user_scope_enforced_select` policy.
    pub user_scope_column: Option<String>,
    pub is_codelist: bool,
    /// Whether this entity supports demo data flagging.
    pub has_demo_flag: bool,
    /// Append-only snapshot semantics (issue #284, CDM TradeState pattern):
    /// explicit `entity_config.append_only` OR inferred from the effective
    /// operations excluding update+delete (the same inference the scaffold
    /// grants use). Gates the updated_at column, audit band, BEFORE UPDATE
    /// trigger, and SELECT/INSERT-only grants; child tables inherit.
    pub append_only: bool,
}

/// Full-text search context for DDL generation.
#[derive(Debug, Clone, Serialize)]
pub struct FtsContext {
    /// Name of the generated tsvector column (e.g. "search_tsv").
    pub tsvector_column: String,
    /// Postgres text search configuration name (e.g. "english").
    pub language: String,
    /// Columns with their FTS weights.
    pub weighted_columns: Vec<FtsColumnWeight>,
    /// GIN index name.
    pub index_name: String,
}

/// A column participating in full-text search with its weight.
#[derive(Debug, Clone, Serialize)]
pub struct FtsColumnWeight {
    pub column: String,
    /// Postgres tsvector weight: A (highest), B, C, or D (lowest).
    pub weight: String,
}

/// Embedding column context for semantic search DDL generation.
#[derive(Debug, Clone, Serialize)]
pub struct EmbeddingContext {
    /// Source text column name.
    pub source_column: String,
    /// Generated vector column name (e.g. "executive_summary_embedding").
    pub vector_column: String,
    /// Vector dimensions (e.g. 1536).
    pub dimensions: u32,
    /// HNSW index name.
    pub index_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColumnDef {
    pub name: String,
    pub pg_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub is_primary_key: bool,
    pub is_array: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForeignKeyDef {
    pub column: String,
    /// Column name without PG quotes — safe for use in constraint/index names.
    pub column_name: String,
    pub references_schema: String,
    pub references_table: String,
    pub references_column: String,
    pub on_delete: String,
    /// True when the FK targets a codelist table. Codelists are generated in
    /// a separate band and are not entity tables, so the entity-table FK
    /// filter must not drop them.
    pub is_codelist: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckConstraint {
    pub name: String,
    pub column: String,
    pub expression: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexDef {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChildTableDef {
    pub schema_name: String,
    pub table_name: String,
    pub parent_fk_column: String,
    /// Schema of the parent table (for FK REFERENCES clause in nested children)
    pub parent_schema: String,
    /// Table name of the parent (for FK REFERENCES clause in nested children)
    pub parent_table: String,
    pub columns: Vec<ColumnDef>,
    pub display_name: String,
    pub comments: Vec<ColumnComment>,
    pub foreign_keys: Vec<ForeignKeyDef>,
    pub check_constraints: Vec<CheckConstraint>,
    pub child_tables: Vec<ChildTableDef>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColumnComment {
    pub column: String,
    pub comment: String,
}
