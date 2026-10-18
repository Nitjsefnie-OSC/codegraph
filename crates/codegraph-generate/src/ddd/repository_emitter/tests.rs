use crate::api::include_path::{IncludeSegment, ResolvedIncludePath};
use crate::code_writer::CodeWriter;

use super::query_search::emit_sort_ordering;
use super::*;

fn make_column(
    field: &str,
    rust_type: &str,
    nullable: bool,
    dto_rust_type: Option<&str>,
    is_array: bool,
    is_structured_wrapper: bool,
) -> TreeColumn {
    TreeColumn {
        field_name: field.to_string(),
        pg_column_name: field.to_string(),
        dto_field_name: None,
        rust_type: rust_type.to_string(),
        is_nullable: nullable,
        is_entity_ref: false,
        dto_rust_type: dto_rust_type.map(|s| s.to_string()),
        is_workflow_managed: false,
        is_array,
        pg_cast: None,
        is_composite_range: false,
        is_structured_wrapper,
        is_media: false,
    }
}

// --- emit_entity_to_dto_field tests ---

#[test]
fn entity_to_dto_plain_required() {
    let col = make_column("given_name", "String", false, None, false, false);
    let mut code = CodeWriter::new();
    emit_entity_to_dto_field(&mut code, &col, "row", "    ");
    assert_eq!(code.as_str(), "    given_name: row.given_name,\n");
}

#[test]
fn entity_to_dto_codelist_required() {
    let col = make_column(
        "gender_code",
        "String",
        false,
        Some("GenderCodeList"),
        false,
        false,
    );
    let mut code = CodeWriter::new();
    emit_entity_to_dto_field(&mut code, &col, "row", "    ");
    assert!(code.as_str().contains(".parse().unwrap_or_default()"));
}

#[test]
fn entity_to_dto_codelist_nullable() {
    let col = make_column(
        "currency_code",
        "String",
        true,
        Some("CurrencyCodeList"),
        false,
        false,
    );
    let mut code = CodeWriter::new();
    emit_entity_to_dto_field(&mut code, &col, "row", "    ");
    assert!(code.as_str().contains(".and_then(|v| v.parse().ok())"));
}

#[test]
fn entity_to_dto_codelist_array_required() {
    let col = make_column(
        "codes",
        "Vec<String>",
        false,
        Some("StatusEnum"),
        true,
        false,
    );
    let mut code = CodeWriter::new();
    emit_entity_to_dto_field(&mut code, &col, "row", "    ");
    assert!(code.as_str().contains("filter_map"));
    assert!(!code.as_str().contains(".map(|v|"));
}

#[test]
fn entity_to_dto_codelist_array_nullable() {
    let col = make_column(
        "codes",
        "Vec<String>",
        true,
        Some("StatusEnum"),
        true,
        false,
    );
    let mut code = CodeWriter::new();
    emit_entity_to_dto_field(&mut code, &col, "row", "    ");
    assert!(code.as_str().contains(".map(|v| v.into_iter()"));
}

#[test]
fn entity_to_dto_jsonb_required() {
    let col = make_column("address", "serde_json::Value", false, None, false, true);
    let mut code = CodeWriter::new();
    emit_entity_to_dto_field(&mut code, &col, "row", "    ");
    assert!(
        code.as_str()
            .contains("serde_json::from_value(row.address).unwrap_or_default()")
    );
}

#[test]
fn entity_to_dto_jsonb_nullable() {
    let col = make_column("metadata", "serde_json::Value", true, None, false, true);
    let mut code = CodeWriter::new();
    emit_entity_to_dto_field(&mut code, &col, "row", "    ");
    assert!(
        code.as_str()
            .contains(".and_then(|v| serde_json::from_value(v).ok())")
    );
}

#[test]
fn entity_to_dto_jsonb_array_required() {
    let col = make_column("tags", "serde_json::Value", false, None, true, true);
    let mut code = CodeWriter::new();
    emit_entity_to_dto_field(&mut code, &col, "row", "    ");
    assert!(
        code.as_str()
            .contains("serde_json::from_value(row.tags).unwrap_or_default()")
    );
}

#[test]
fn entity_to_dto_jsonb_array_nullable() {
    let col = make_column("prefs", "serde_json::Value", true, None, true, true);
    let mut code = CodeWriter::new();
    emit_entity_to_dto_field(&mut code, &col, "row", "    ");
    assert!(
        code.as_str()
            .contains(".and_then(|v| serde_json::from_value(v).ok())")
    );
}

// --- emit_child_field_population tests ---

#[test]
fn child_population_array() {
    let children = vec![ChildTableInfo {
        field_name: "addresses".to_string(),
        struct_name: "CandidateAddress".to_string(),
        sql_table_name: "candidate_address".to_string(),
        sql_schema_name: "recruiting".to_string(),
        parent_fk_column: "candidate_id".to_string(),
        is_array: true,
        columns: vec![],
        child_tables: vec![],
        is_back_ref: false,
    }];
    let mut code = CodeWriter::new();
    emit_child_field_population(&mut code, &children, "    ");
    assert_eq!(code.as_str(), "    addresses: addresses_rows,\n");
}

#[test]
fn child_population_single() {
    let children = vec![ChildTableInfo {
        field_name: "profile".to_string(),
        struct_name: "CandidateProfile".to_string(),
        sql_table_name: "candidate_profile".to_string(),
        sql_schema_name: "recruiting".to_string(),
        parent_fk_column: "candidate_id".to_string(),
        is_array: false,
        columns: vec![],
        child_tables: vec![],
        is_back_ref: false,
    }];
    let mut code = CodeWriter::new();
    emit_child_field_population(&mut code, &children, "    ");
    assert_eq!(
        code.as_str(),
        "    profile: profile_rows.into_iter().next(),\n"
    );
}

#[test]
fn child_population_empty() {
    let mut code = CodeWriter::new();
    emit_child_field_population(&mut code, &[], "    ");
    assert_eq!(code.as_str(), "");
}

// --- emit_sort_ordering tests (issue #306) ---

fn sort_test_tree(is_auditable: bool) -> EntityTree {
    EntityTree {
        entity_name: "Task".into(),
        module_name: "task".into(),
        schema_name: "common".into(),
        table_name: "task".into(),
        entity_module: "common_task".into(),
        namespace: None,
        direct_columns: vec![],
        child_tables: vec![],
        junction_tables: vec![],
        has_create: false,
        has_read: true,
        has_update: false,
        has_delete: false,
        has_list: true,
        has_workflow: false,
        has_fts: false,
        has_embeddings: false,
        fts_language: "english".into(),
        is_auditable,
        soft_delete_visibility: "exclude_by_default".into(),
        soft_delete_column: None,
        soft_delete_cascade: "restrict".into(),
        track_updated_user: false,
        track_deleted_user: false,
        append_only: false,
        filter_fields: vec![],
        nested_filter_fields: vec![],
        parent_ref: None,
        hierarchy_field: None,
        tree_include: vec![],
        design_finders: vec![],
    }
}

/// The sort-aware ordering names the validated column (fully quoted
/// through sea_query aliases) with the requested direction and the `id`
/// tiebreaker; the else arm keeps `created_at DESC`.
#[test]
fn sort_ordering_quotes_columns_and_appends_id_tiebreaker() {
    let tree = sort_test_tree(false);
    let sort_columns = vec![
        UxSortColumn {
            key: "name".into(),
            column: "name".into(),
        },
        UxSortColumn {
            key: "total_amount".into(),
            column: "total_amount".into(),
        },
    ];
    let mut code = CodeWriter::new();
    emit_sort_ordering(&tree, &sort_columns, &mut code);
    let out = code.as_str();

    assert!(
        out.contains("let query = if let Some((sort_field, sort_desc)) = sort {"),
        "{out}"
    );
    assert!(
        out.contains("let dir = if sort_desc {\n                sea_orm::sea_query::Order::Desc\n            } else {\n                sea_orm::sea_query::Order::Asc\n            };"),
        "{out}"
    );
    // Schema/table/column fully qualified and quoted by sea_query Alias.
    assert!(
        out.contains("Alias::new(\"common\"), sea_orm::sea_query::Alias::new(\"task\"), sea_orm::sea_query::Alias::new(\"name\")"),
        "{out}"
    );
    assert!(out.contains("Alias::new(\"total_amount\")"), "{out}");
    // Deterministic `, id ASC` tiebreaker for stable pagination.
    assert!(
        out.contains("ordered.order_by_asc(sea_orm::sea_query::Expr::col((sea_orm::sea_query::Alias::new(\"common\"), sea_orm::sea_query::Alias::new(\"task\"), sea_orm::sea_query::Alias::new(\"id\"))))"),
        "{out}"
    );
    // Default arm keeps today's ordering.
    assert!(
        out.contains("query.order_by_desc(crate::entity::common_task::Column::CreatedAt)"),
        "{out}"
    );
}

/// The `id` tiebreaker applies in both directions (the match arm runs
/// before the shared tiebreaker line).
#[test]
fn sort_ordering_tiebreaker_is_direction_independent() {
    let tree = sort_test_tree(true);
    let sort_columns = vec![UxSortColumn {
        key: "due_date".into(),
        column: "due_date".into(),
    }];
    let mut code = CodeWriter::new();
    emit_sort_ordering(&tree, &sort_columns, &mut code);
    let out = code.as_str();
    assert_eq!(out.matches("order_by_asc").count(), 1, "{out}");
    assert!(out.contains("\"due_date\""), "{out}");
}

// --- include-fetch hydrator type adaptation (issue #459) ---

fn single_include_path() -> ResolvedIncludePath {
    ResolvedIncludePath {
        alias: "job".into(),
        segments: vec![IncludeSegment {
            entity_name: "Job".into(),
            schema_title: "JobType".into(),
            module_name: "job".into(),
            domain: "hr".into(),
            table: "\"hr\".\"job\"".into(),
            fk_column: "job_id".into(),
            reverse_fk_column: "job_id".into(),
            fk_is_required: false,
            reverse_fk_is_required: false,
            is_array: false,
            child_table_override: None,
        }],
        response_rust_type: "JobResponse".into(),
        fetch_method: "fetch_job_for_task".into(),
        batch_fetch_method: "fetch_job_batch_for_task".into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_single_include(dto_type: Option<&str>, col_type: &str, nullable: bool) -> String {
    let tree = sort_test_tree(false);
    let path = single_include_path();
    let mut code = CodeWriter::new();
    RepositoryImplEmitter.emit_include_fetch_methods(
        &tree,
        &mut code,
        std::slice::from_ref(&path),
        &[None],
        &[vec![vec!["organization_id".to_string()]]],
        &[vec![vec!["organization_id".to_string()]]],
        &[vec![vec![false]]],
        &[vec![vec![false]]],
        &[vec![vec![dto_type.map(str::to_string)]]],
        &[vec![vec![nullable]]],
        &[vec![vec![col_type.to_string()]]],
    );
    code.into_string()
}

/// A Uuid entity column hydrating a String DTO field (the mox unformatted
/// datatype shape: the refers-FK column is Uuid while the TEXT-mapped DTO
/// field is String) must adapt with `.to_string()` — a plain assignment
/// fails E0308 in the generated app (issue #459).
#[test]
fn include_fetch_converts_uuid_column_to_string_dto() {
    let out = emit_single_include(Some("String"), "Uuid", false);
    assert!(
        out.contains("organization_id: target.organization_id.to_string(),"),
        "uuid column → String DTO must emit .to_string(). Got:\n{out}"
    );
}

#[test]
fn include_fetch_converts_nullable_uuid_column_to_string_dto() {
    let out = emit_single_include(Some("String"), "Uuid", true);
    assert!(
        out.contains("organization_id: target.organization_id.map(|v| v.to_string()),"),
        "nullable uuid column → String DTO must emit .map(|v| v.to_string()). Got:\n{out}"
    );
}

/// A String entity column hydrating a non-String DTO field mirrors the main
/// hydrator's `.parse()` conversion.
#[test]
fn include_fetch_parses_string_column_to_non_string_dto() {
    let out = emit_single_include(Some("CurrencyCodeList"), "String", false);
    assert!(
        out.contains("organization_id: target.organization_id.parse().unwrap_or_default(),"),
        "String column → non-String DTO must emit .parse().unwrap_or_default(). Got:\n{out}"
    );
}

#[test]
fn include_fetch_parses_nullable_string_column_to_non_string_dto() {
    let out = emit_single_include(Some("CurrencyCodeList"), "String", true);
    assert!(
        out.contains("organization_id: target.organization_id.and_then(|v| v.parse().ok()),"),
        "nullable String column → non-String DTO must emit .and_then(parse). Got:\n{out}"
    );
}

/// Matching types keep the byte-identical plain assignment.
#[test]
fn include_fetch_keeps_plain_assignment_for_matching_types() {
    let out = emit_single_include(Some("Uuid"), "Uuid", false);
    assert!(
        out.contains("organization_id: target.organization_id,"),
        "matching types must keep the plain assignment. Got:\n{out}"
    );
    assert!(!out.contains(".to_string()"), "{out}");
}
