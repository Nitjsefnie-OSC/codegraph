use super::*;
use crate::db::ddl::query::ddl_dto_gap_fields;
use crate::db::dialect::{PostgresDialect, SqliteDialect};

fn col(pg_type: &str) -> ColumnDef {
    ColumnDef {
        name: "col".to_string(),
        pg_type: pg_type.to_string(),
        nullable: true,
        default: None,
        is_primary_key: false,
        is_array: false,
    }
}

fn child(columns: Vec<ColumnDef>) -> ChildTableDef {
    ChildTableDef {
        schema_name: "s".to_string(),
        table_name: "t".to_string(),
        parent_fk_column: "fk".to_string(),
        parent_schema: "s".to_string(),
        parent_table: "p".to_string(),
        columns,
        display_name: "T".to_string(),
        comments: vec![],
        foreign_keys: vec![],
        check_constraints: vec![],
        child_tables: vec![],
    }
}

#[test]
fn empty_columns_returns_empty() {
    assert!(detect_extensions_from_columns(&[], &[]).is_empty());
}

#[test]
fn geometry_column_returns_postgis() {
    let cols = vec![col("GEOMETRY(Point, 4326)")];
    assert_eq!(detect_extensions_from_columns(&cols, &[]), vec!["postgis"]);
}

#[test]
fn geometry_lowercase_returns_postgis() {
    let cols = vec![col("geometry")];
    assert_eq!(detect_extensions_from_columns(&cols, &[]), vec!["postgis"]);
}

#[test]
fn geography_column_returns_postgis() {
    let cols = vec![col("GEOGRAPHY(Point, 4326)")];
    assert_eq!(detect_extensions_from_columns(&cols, &[]), vec!["postgis"]);
}

#[test]
fn vector_column_returns_vector() {
    let cols = vec![col("VECTOR(1536)")];
    assert_eq!(detect_extensions_from_columns(&cols, &[]), vec!["vector"]);
}

#[test]
fn postgis_in_child_table_detected() {
    let children = vec![child(vec![col("GEOMETRY(Point, 4326)")])];
    assert_eq!(
        detect_extensions_from_columns(&[], &children),
        vec!["postgis"]
    );
}

#[test]
fn multiple_geometry_columns_produce_one_entry() {
    let cols = vec![col("GEOMETRY(Point, 4326)"), col("GEOGRAPHY(Point, 4326)")];
    assert_eq!(detect_extensions_from_columns(&cols, &[]), vec!["postgis"]);
}

#[test]
fn unrelated_types_return_empty() {
    let cols = vec![col("TEXT"), col("UUID"), col("TIMESTAMPTZ"), col("JSONB")];
    assert!(detect_extensions_from_columns(&cols, &[]).is_empty());
}

#[test]
fn mixed_extensions_detected() {
    let cols = vec![col("GEOMETRY(Point, 4326)"), col("VECTOR(1536)")];
    let exts = detect_extensions_from_columns(&cols, &[]);
    assert_eq!(exts, vec!["postgis", "vector"]);
}

fn info_col(name: &str, pg_type: &str) -> ColumnInfo {
    ColumnInfo {
        name: name.to_string(),
        description: None,
        rust_type: "String".to_string(),
        postgres_type: pg_type.to_string(),
        is_optional: false,
        is_codelist_fk: false,
        composite_columns: vec![],
        is_array: false,
        classification: None,
        fk_target: None,
        check_values: vec![],
    }
}

fn node_with_columns(columns: Vec<ColumnInfo>) -> CompositionNode {
    CompositionNode {
        field_name: "provenance".to_string(),
        schema_title: "Provenance".to_string(),
        table_schema: "core".to_string(),
        table_name: "trust_provenance".to_string(),
        fk: None,
        is_collection: false,
        columns,
        jsonb_columns: vec![],
        children: vec![],
        composite_range: None,
        consumed_fields: vec![],
    }
}

#[test]
fn child_parent_fk_column_avoids_double_id_suffix() {
    assert_eq!(child_parent_fk_column("trust"), "trust_id");
    assert_eq!(
        child_parent_fk_column("evidence_extracted_field_id"),
        "evidence_extracted_field_id"
    );
}

#[test]
fn child_table_dedupes_schema_derived_timestamps() {
    let node = node_with_columns(vec![
        info_col("createdAt", "TIMESTAMPTZ"),
        info_col("note", "TEXT"),
    ]);
    let def = composition_node_to_child_table(&node, "trust", "core", "Trust", &HashSet::new());
    let names: Vec<&str> = def.columns.iter().map(|c| c.name.as_str()).collect();
    assert!(!names.contains(&"created_at"), "got {:?}", names);
    assert!(!names.contains(&"updated_at"), "got {:?}", names);
    assert!(names.contains(&"note"));
}

#[test]
fn child_table_timestamp_dedupe_is_case_insensitive() {
    let node = node_with_columns(vec![info_col("CreatedAt", "TIMESTAMPTZ")]);
    let def = composition_node_to_child_table(&node, "trust", "core", "Trust", &HashSet::new());
    assert!(def.columns.iter().all(|c| c.name != "CreatedAt"));
}

#[test]
fn child_table_parent_fk_single_suffix_for_id_named_parent() {
    let node = node_with_columns(vec![info_col("value", "TEXT")]);
    let def = composition_node_to_child_table(
        &node,
        "evidence_extracted_field_id",
        "compliance",
        "Evidence Extracted Field Id",
        &HashSet::new(),
    );
    assert_eq!(def.parent_fk_column, "evidence_extracted_field_id");
    assert!(
        def.columns
            .iter()
            .all(|c| c.name != "evidence_extracted_field_id_id")
    );
}

#[test]
fn rls_template_renders_org_isolation_for_child_table() {
    let template_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    let tera = crate::template_engine::create_tera(&template_dir).unwrap();
    let project = crate::ProjectConfig::default();

    let parent = DdlContext {
        schema_name: "core".to_string(),
        table_name: "trust".to_string(),
        display_name: "Trust".to_string(),
        domain: "core".to_string(),
        columns: vec![],
        primary_key: "id".to_string(),
        foreign_keys: vec![],
        check_constraints: vec![],
        indexes: vec![],
        has_updated_at: true,
        is_tenant_scoped: true,
        tenant_table: "platform.organization".to_string(),
        extensions: vec![],
        child_tables: vec![],
        comments: vec![],
        has_workflow: false,
        resource_name: "trust".to_string(),
        fts: None,
        embeddings: vec![],
        is_auditable: true,
        role_enforced: true,
        role_minima: Vec::new(),
        roles_hierarchy: Vec::new(),
        user_scope_column: None,
        is_codelist: false,
        has_demo_flag: false,
        append_only: false,
    };

    let junction = ChildTableDef {
        schema_name: "core".to_string(),
        table_name: "trust_settlor_ids".to_string(),
        parent_fk_column: "trust_id".to_string(),
        parent_schema: "core".to_string(),
        parent_table: "trust".to_string(),
        columns: vec![
            ColumnDef {
                name: "platform_organization_id".to_string(),
                pg_type: "UUID".to_string(),
                nullable: false,
                default: Some("'00000000-0000-0000-0000-000000000000'::UUID".to_string()),
                is_primary_key: false,
                is_array: false,
            },
            ColumnDef {
                name: "party_id".to_string(),
                pg_type: "UUID".to_string(),
                nullable: false,
                default: None,
                is_primary_key: false,
                is_array: false,
            },
        ],
        display_name: "Trust Settlor Ids".to_string(),
        comments: vec![],
        foreign_keys: vec![],
        check_constraints: vec![],
        child_tables: vec![],
    };

    let child_ctx = child_table_rls_context(&parent, &junction);
    let sql = render_template_with_project(&tera, "db/rls.tera", &child_ctx, &project).unwrap();

    assert!(sql.contains("ALTER TABLE core.trust_settlor_ids ENABLE ROW LEVEL SECURITY"));
    assert!(sql.contains("CREATE POLICY \"org_isolation_select\""));
    assert!(sql.contains("CREATE POLICY \"org_isolation_delete\""));
    // Children are not api-key-scoped resources.
    assert!(!sql.contains("api_key_scoped_"));
}

/// #169: role_enforced_* policies rank the caller's role against the
/// configured per-op minima via role_at_least(); defaults come from the
/// built-in matrix, explicit min_roles win.
#[test]
fn rls_role_enforced_policies_use_configured_minima() {
    let template_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    let tera = crate::template_engine::create_tera(&template_dir).unwrap();
    let project = crate::ProjectConfig::default();

    let mut ctx = DdlContext {
        schema_name: "compensation".to_string(),
        table_name: "pay_run".to_string(),
        display_name: "Pay Run".to_string(),
        domain: "compensation".to_string(),
        columns: vec![],
        primary_key: "id".to_string(),
        foreign_keys: vec![],
        check_constraints: vec![],
        indexes: vec![],
        has_updated_at: true,
        is_tenant_scoped: true,
        tenant_table: "platform.organization".to_string(),
        extensions: vec![],
        child_tables: vec![],
        comments: vec![],
        has_workflow: false,
        resource_name: "pay-run".to_string(),
        fts: None,
        embeddings: vec![],
        is_auditable: true,
        role_enforced: true,
        role_minima: vec![
            RoleMinimum {
                operation: "create".into(),
                min_role: "payroll_admin".into(),
            },
            RoleMinimum {
                operation: "read".into(),
                min_role: "employee".into(),
            },
            RoleMinimum {
                operation: "update".into(),
                min_role: "payroll_admin".into(),
            },
            RoleMinimum {
                operation: "delete".into(),
                min_role: "admin".into(),
            },
            RoleMinimum {
                operation: "list".into(),
                min_role: "employee".into(),
            },
        ],
        roles_hierarchy: vec![
            "owner".into(),
            "admin".into(),
            "hr_admin".into(),
            "payroll_admin".into(),
            "manager".into(),
            "member".into(),
            "employee".into(),
        ],
        user_scope_column: None,
        is_codelist: false,
        has_demo_flag: false,
        append_only: false,
    };
    let sql = render_template_with_project(&tera, "db/rls.tera", &ctx, &project).unwrap();

    assert!(sql.contains("public.role_at_least('payroll_admin')"));
    assert!(sql.contains("public.role_at_least('admin')"));
    assert!(sql.contains("CREATE POLICY \"role_enforced_select\""));
    assert!(sql.contains("CREATE POLICY \"role_enforced_delete\""));
    assert!(
        !sql.contains("enforce_role_action"),
        "the fixed matrix fn is retired"
    );
    assert!(!sql.contains("user_scope_enforced_select"));

    // User-scope column config: column + auto-set trigger + RESTRICTIVE policy.
    ctx.user_scope_column = Some("requested_by_user_id".to_string());
    let sql = render_template_with_project(&tera, "db/rls.tera", &ctx, &project).unwrap();
    assert!(sql.contains("ADD COLUMN IF NOT EXISTS requested_by_user_id UUID"));
    assert!(sql.contains("CREATE TRIGGER trg_pay_run_set_req_user_id"));
    assert!(sql.contains("CREATE POLICY \"user_scope_enforced_select\""));
    assert!(sql.contains(
        "role_rank(coalesce(current_setting('app.role', true), '')) < public.role_rank('member')"
    ));
}

/// #169: the RBAC migration generalizes the role enum over the configured
/// hierarchy and emits role_rank/role_at_least.
#[test]
fn rbac_roles_renders_hierarchy_rank_helpers() {
    let template_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    let tera = crate::template_engine::create_tera(&template_dir).unwrap();
    let project = crate::ProjectConfig::default();

    let ctx = serde_json::json!({
        "roles_hierarchy": ["owner", "admin", "hr_admin", "payroll_admin", "manager", "member", "employee"],
    });
    let sql = render_template_with_project(&tera, "db/rbac_roles.tera", &ctx, &project).unwrap();

    // Custom roles get enum-extension guards.
    assert!(sql.contains("AND enumlabel = 'hr_admin'"));
    assert!(sql.contains("AND enumlabel = 'payroll_admin'"));
    assert!(sql.contains("AND enumlabel = 'manager'"));
    // rank fn covers every role in order; 'member' normalizes to 'employee'.
    assert!(sql.contains("CREATE OR REPLACE FUNCTION public.role_rank(p_role text)"));
    assert!(sql.contains("IF r = 'hr_admin' THEN"));
    assert!(sql.contains("IF r = 'member' THEN"));
    assert!(sql.contains("CREATE OR REPLACE FUNCTION public.role_at_least(p_min_role text)"));
    assert!(sql.contains("'ROLE_FORBIDDEN'"));
    // The retired fixed-matrix function is gone.
    assert!(!sql.contains("enforce_role_action"));
}

fn test_context() -> DdlContext {
    DdlContext {
        schema_name: "hr".to_string(),
        table_name: "candidate".to_string(),
        display_name: "Candidate".to_string(),
        domain: "hr".to_string(),
        columns: vec![],
        primary_key: "id".to_string(),
        foreign_keys: vec![],
        check_constraints: vec![],
        indexes: vec![],
        has_updated_at: false,
        is_tenant_scoped: false,
        tenant_table: String::new(),
        extensions: vec![],
        child_tables: vec![],
        comments: vec![],
        has_workflow: false,
        resource_name: "candidate".to_string(),
        fts: None,
        embeddings: vec![],
        is_auditable: false,
        role_enforced: false,
        role_minima: vec![],
        roles_hierarchy: vec![],
        user_scope_column: None,
        is_codelist: false,
        has_demo_flag: false,
        append_only: false,
    }
}

#[test]
fn sqlite_dialect_refuses_unrepresentable_column_types() {
    let mut ctx = test_context();
    ctx.columns.push(col("DATERANGE"));
    ctx.child_tables.push(child(vec![col("TEXT[]")]));
    let err = match apply_dialect_type_mapping(&SqliteDialect::new(), &mut ctx) {
        Err(err) => err,
        Ok(()) => panic!("sqlite must refuse range and array types"),
    };
    let msg = err.to_string();
    assert!(msg.contains("sqlite"), "must name the dialect: {msg}");
    assert!(msg.contains("candidate"), "must name the table: {msg}");
    assert!(msg.contains("DATERANGE"), "must name the type: {msg}");
    assert!(msg.contains("TEXT[]"), "must name the child type: {msg}");
}

#[test]
fn sqlite_dialect_maps_date_numeric_and_bytea() {
    let mut ctx = test_context();
    ctx.columns = vec![col("DATE"), col("NUMERIC(10,2)"), col("BYTEA")];
    apply_dialect_type_mapping(&SqliteDialect::new(), &mut ctx)
        .expect("date/numeric/bytea must map for sqlite");
    assert_eq!(ctx.columns[0].pg_type, "TEXT");
    assert_eq!(ctx.columns[1].pg_type, "REAL");
    assert_eq!(ctx.columns[2].pg_type, "BLOB");
}

#[test]
fn postgres_dialect_accepts_all_pg_types() {
    let mut ctx = test_context();
    ctx.columns.push(col("DATERANGE"));
    ctx.columns.push(col("TEXT[]"));
    apply_dialect_type_mapping(&PostgresDialect::new(), &mut ctx)
        .expect("postgres accepts every PostgreSQL type");
    // Unmapped types pass through unchanged.
    assert_eq!(ctx.columns[0].pg_type, "DATERANGE");
    assert_eq!(ctx.columns[1].pg_type, "TEXT[]");
}

fn workflow_config_toml() -> codegraph_config::config::DomainConfig {
    codegraph_config::config::parse_domain_config_str(
        r#"
[domains.refunds]
label = "Refunds"
schema_dir = "refunds"
postgres_schema = "refunds"
entities = ["RefundRequestType"]

[domains.refunds.entity_config.RefundRequestType.workflow]
status_field = "status"
states = ["draft", "submitted", "approved", "rejected"]
initial_state = "draft"
terminal_states = ["approved", "rejected"]
generate_action_endpoints = true
"#,
    )
    .expect("workflow domains.toml must parse")
}

fn status_col(nullable: bool, default: Option<&str>) -> ColumnDef {
    ColumnDef {
        name: "status".to_string(),
        pg_type: "TEXT".to_string(),
        nullable,
        default: default.map(str::to_string),
        is_primary_key: false,
        is_array: false,
    }
}

/// Issue #311: the workflow status column gains a DEFAULT of the
/// configured initial_state, so API creates (which omit the column —
/// the create DTO excludes workflow-managed fields) materialize the
/// initial state. Applies to NULLABLE columns too: the gate fixture's
/// status column is a non-required codelist ref, and the pre-fix
/// `!col.nullable` guard silently skipped it.
#[test]
fn workflow_status_column_defaults_to_initial_state() {
    let config = workflow_config_toml();
    let mut columns = vec![status_col(true, None), col("TEXT")];
    let has_workflow =
        apply_workflow_defaults(&config, "refunds", "RefundRequestType", &mut columns);
    assert!(has_workflow);
    assert_eq!(
        columns[0].default.as_deref(),
        Some("'draft'"),
        "nullable status column must carry the initial_state default"
    );
    // Nullability is untouched — the default alone fixes creates.
    assert!(columns[0].nullable);
}

#[test]
fn workflow_status_default_survives_when_column_not_null() {
    let config = workflow_config_toml();
    let mut columns = vec![status_col(false, None)];
    apply_workflow_defaults(&config, "refunds", "RefundRequestType", &mut columns);
    assert_eq!(columns[0].default.as_deref(), Some("'draft'"));
}

#[test]
fn workflow_default_preserves_explicit_column_default() {
    let config = workflow_config_toml();
    let mut columns = vec![status_col(true, Some("'new'"))];
    apply_workflow_defaults(&config, "refunds", "RefundRequestType", &mut columns);
    assert_eq!(
        columns[0].default.as_deref(),
        Some("'new'"),
        "an explicit schema/config default must win over the workflow default"
    );
}

#[test]
fn workflow_default_escapes_quoted_initial_state() {
    let mut quoted = workflow_config_toml();
    if let Some(ec) = quoted
        .domains
        .get_mut("refunds")
        .and_then(|d| d.entity_config.get_mut("RefundRequestType"))
        && let Some(wf) = ec.workflow.as_mut()
    {
        wf.initial_state = "o'clock".to_string();
    }
    let mut columns = vec![status_col(true, None)];
    apply_workflow_defaults(&quoted, "refunds", "RefundRequestType", &mut columns);
    assert_eq!(columns[0].default.as_deref(), Some("'o''clock'"));
}

#[test]
fn non_status_columns_and_workflowless_entities_are_untouched() {
    let config = workflow_config_toml();
    let mut columns = vec![col("TEXT"), status_col(true, None)];
    apply_workflow_defaults(&config, "refunds", "OtherEntityType", &mut columns);
    assert!(
        columns.iter().all(|c| c.default.is_none()),
        "no workflow config on the entity → no defaults injected"
    );
}

// ── ddl_dto_gap_fields (#446 debug invariant) ────────────────────────────

fn dto_field(name: &str, is_array: bool, is_entity_ref: bool) -> crate::ddd::dto::DtoField {
    crate::ddd::dto::DtoField {
        name: name.to_string(),
        rust_type: "String".to_string(),
        is_required: true,
        is_array,
        description: String::new(),
        render_strategy: String::new(),
        is_entity_ref,
        is_hierarchy_field: false,
        min_length: None,
        max_length: None,
        minimum: None,
        maximum: None,
        pattern: None,
        format: None,
    }
}

fn ddl_cols(names: &[&str]) -> Vec<ColumnDef> {
    names
        .iter()
        .map(|n| ColumnDef {
            name: (*n).to_string(),
            pg_type: "TEXT".to_string(),
            nullable: true,
            default: None,
            is_primary_key: false,
            is_array: false,
        })
        .collect()
}

#[test]
fn gap_fields_flags_missing_columns() {
    let ddl = ddl_cols(&["id", "note", "parent_id"]);
    let dto = vec![
        dto_field("id", false, false),
        dto_field("note", false, false),
        dto_field("parent_id", false, true),
        dto_field("missing_id", false, true),
    ];
    assert_eq!(ddl_dto_gap_fields(&ddl, &dto), vec!["missing_id"]);
}

#[test]
fn gap_fields_allows_codelist_code_suffix_and_junction_arrays() {
    let ddl = ddl_cols(&["status_code"]);
    let dto = vec![
        // Codelist field: rust-side `_code` strip → status_code column.
        dto_field("status", false, false),
        // Junction array entity ref: child table, never a column.
        dto_field("tags_id", true, true),
    ];
    assert!(ddl_dto_gap_fields(&ddl, &dto).is_empty());
}

#[test]
fn gap_fields_skips_hierarchy_fields() {
    let ddl = ddl_cols(&[]);
    let mut hierarchy = dto_field("parent_organization_id", false, false);
    hierarchy.is_hierarchy_field = true;
    assert!(ddl_dto_gap_fields(&ddl, &[hierarchy]).is_empty());
}

// ── issue #451: ColumnDef.name is the bare identifier; quoting is render-time ──

#[test]
fn reserved_word_column_def_name_is_bare() {
    let (cols, _, _, _) =
        column_info_to_ddl(&info_col("name", "TEXT"), "store_order").expect("plain column");
    assert_eq!(cols[0].name, "name");
}

#[test]
fn reserved_word_fk_and_check_identifiers_are_bare() {
    let mut col = info_col("order", "TEXT");
    col.classification = Some(codegraph_type_contracts::RefClassificationKind::InlineEnum);
    col.check_values = vec!["asc".to_string(), "desc".to_string()];
    let (cols, _, checks, _) = column_info_to_ddl(&col, "store_order").expect("check column");
    assert_eq!(cols[0].name, "order");
    assert_eq!(checks[0].column, "order");
    assert_eq!(checks[0].expression, "\"order\" IN ($$asc$$, $$desc$$)");
}

#[test]
fn rendered_table_ddl_quotes_reserved_column_names() {
    let template_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    let tera = crate::template_engine::create_tera(&template_dir).unwrap();
    let project = crate::ProjectConfig::default();

    let mut ctx = test_context();
    ctx.columns.push(ColumnDef {
        name: "name".to_string(),
        pg_type: "TEXT".to_string(),
        nullable: false,
        default: None,
        is_primary_key: false,
        is_array: false,
    });
    let mut render_ctx = ctx.clone();
    super::accumulators::quote_ddl_identifiers(&mut render_ctx);
    let sql = render_template_with_project(&tera, "db/table.tera", &render_ctx, &project).unwrap();
    assert!(
        sql.contains("\n    \"name\" TEXT NOT NULL"),
        "rendered migration must quote reserved-word columns: {sql}"
    );
}
