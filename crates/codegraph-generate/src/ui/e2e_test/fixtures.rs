use codegraph_config::DomainConfig;
use codegraph_core::traits::GraphQuerier;

use super::common::collect_ui_fields;
use super::page::UiField;

/// Build a JS object-literal body (without outer braces) for creating an entity
/// via the API.  Used for parent and grandparent entity creation in tests.
pub(super) async fn build_test_data_json(
    db: &dyn GraphQuerier,
    schema_title: &str,
    domain: Option<&str>,
    config: &DomainConfig,
) -> String {
    let fields = match collect_ui_fields(db, schema_title, &[], domain, config).await {
        Ok(f) => f,
        Err(_) => {
            // Fallback: if collect_ui_fields failed (e.g. codelist entities with
            // no UI fields in the graph), generate a minimal payload with a
            // code placeholder to satisfy NOT NULL constraints.
            // Only common-domain codelists have code columns.
            if !schema_title.is_empty() && (domain == Some("common")) {
                return "code: `TestCode-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`"
                    .to_string();
            }
            return String::new();
        }
    };
    // Also handle empty-success: collect_ui_fields may return Ok(vec![]) when
    // the schema has no properties (e.g. enum-only code-list schemas).
    // Only common-domain codelists have code columns.
    if fields.is_empty() && !schema_title.is_empty() && (domain == Some("common")) {
        return "code: `TestCode-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`"
            .to_string();
    }
    let mut entries = Vec::new();
    for f in &fields {
        if f.is_entity_ref || f.name == "id" {
            continue;
        }
        if f.name.ends_with("_id") && !f.is_codelist {
            continue;
        }
        // ValueObject fields: omit non-array nested types entirely.
        // All are Option<T> or Vec<T> with #[serde(default)] — omitting
        // the key lets serde use None / empty vec.  Only emit [] for arrays.
        if f.nested_type_name.is_some() {
            if f.is_array {
                entries.push(format!("'{}': []", f.name));
            } else {
                // Omitted — serde uses #[serde(default)] → None
            }
            continue;
        }
        let value = test_value_for_field(f);
        if !value.is_empty() {
            entries.push(format!("'{}': {}", f.name, value));
        }
    }
    entries.join(", ")
}

/// A runtime-valid v4 UUID as a TypeScript template literal (fixed version
/// nibble `4` and variant nibble `8`, random 12-hex tail).
fn uuid_literal() -> String {
    "`00000000-0000-4000-8000-${Math.random().toString(16).slice(2, 14).padStart(12, '0')}`"
        .to_string()
}

/// Sentinel literal for one typed array element (`pg_type` ending in `[]`).
/// `None` = no typed signal (the caller's text-array branch applies).
fn typed_array_value(inner_pg: &str) -> Option<String> {
    Some(match inner_pg {
        "SMALLINT" | "INTEGER" | "BIGINT" => "[42]".to_string(),
        "REAL" | "DOUBLE PRECISION" => "[42.5]".to_string(),
        "BOOLEAN" => "[true]".to_string(),
        "UUID" => format!("[{}]", uuid_literal()),
        _ => return None,
    })
}

/// Type-driven sentinel derived from the field's Postgres column type.
///
/// The generated API body is an untagged enum (`Bulk(Vec<CreateXRequest>)` |
/// `Single(CreateXRequest)`) — a single mistyped field makes EVERY variant
/// reject with `data did not match any variant`.  So each sentinel must
/// type-check against the request DTO's field type, which follows the pg
/// column type (`PgType::canonical_rust_type`): integers → number, NUMERIC →
/// Decimal (serde accepts number or string), BOOLEAN → bool, DATE/TIMESTAMPTZ
/// → ISO strings, ranges → their text literal `'[start,end)'`.
///
/// Returns `None` when the pg type carries no numeric/temporal/uuid signal —
/// the caller falls back to the input-kind heuristics and the readable text
/// sentinel.  `Some("")` = deliberately omit (geometry).
fn typed_pg_value(field: &UiField) -> Option<String> {
    let pg = field.pg_type.to_uppercase();
    if pg.contains("GEOMETRY") {
        // Geometry fields omitted — plain WKT strings are not accepted
        // without ST_GeomFromText
        return Some(String::new());
    }
    if field.is_entity_ref {
        // Entity refs are filled from created dependencies, never a literal.
        return None;
    }
    if let Some(inner) = pg.strip_suffix("[]") {
        return typed_array_value(inner);
    }
    if pg.contains("RANGE") {
        // Range columns carry their text literal through to the SQL cast;
        // half-open `)` matches the detail page's formatRange parser.
        return Some(if pg.starts_with("DATERANGE") {
            "'[2026-01-01,2026-12-31)'".to_string()
        } else if pg.starts_with("INT4RANGE") || pg.starts_with("INT8RANGE") {
            "'[1,42)'".to_string()
        } else {
            "'[2026-01-01T00:00:00Z,2026-12-31T23:59:59Z)'".to_string()
        });
    }
    match pg.as_str() {
        "UUID" => Some(uuid_literal()),
        "BOOLEAN" => Some("true".to_string()),
        "DATE" => Some("'2026-01-15'".to_string()),
        "TIMESTAMPTZ" => Some("'2026-01-15T08:00:00Z'".to_string()),
        // Non-integer numerics: rust_decimal's serde visitor accepts JSON
        // numbers (visit_f64/visit_i64) and strings alike.
        "REAL" | "DOUBLE PRECISION" => Some("42.5".to_string()),
        "SMALLINT" | "INTEGER" | "BIGINT" => Some("42".to_string()),
        _ if pg.starts_with("NUMERIC") => Some("42.5".to_string()),
        _ => None,
    }
}

/// Generate a JS literal value for a UiField, matching the same logic used in
/// test templates' testData() function.
pub(super) fn test_value_for_field(field: &UiField) -> String {
    // StructuredWrapper fields emit JSONB objects
    if !field.structured_sub_fields.is_empty() {
        if field.is_array {
            return format!("[{{ value: 'Test {}' }}]", field.label);
        }
        return format!("{{ value: 'Test {}' }}", field.label);
    }
    if field.is_codelist && !field.codelist_values.is_empty() {
        if field.is_array {
            return format!("[{{ code: '{}' }}]", field.codelist_values[0]);
        }
        return format!("'{}'", field.codelist_values[0]);
    }
    // 1. Type-driven: derive from the pg column type so every value
    //    type-checks against the request DTO (untagged-enum safe).
    if let Some(typed) = typed_pg_value(field) {
        return typed;
    }
    // 2. Input-kind fallbacks: synthetic fields (e.g. the injected codelist
    //    `sort_order`) carry a generic pg type but a meaningful input kind.
    match field.input_type.as_str() {
        "number" => "42".to_string(),
        "checkbox" => "true".to_string(),
        "date" => "'2026-01-15'".to_string(),
        "datetime-local" => "'2026-01-15T08:00:00Z'".to_string(),
        "code" => {
            // code column in codelist entities: must be globally unique at
            // runtime across parallel test invocations. Use a TS template
            // literal with Date.now() + random suffix.
            "`TestCode-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`".to_string()
        }
        _ => {
            if field.is_array {
                format!("['Test {}']", field.label)
            } else if field.name == "code" && !field.is_codelist {
                // code column in codelist entities: must be globally unique at
                // runtime across parallel test invocations. Use a TS template
                // literal with Date.now() + random suffix.
                "`TestCode-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`".to_string()
            } else {
                // Readable text sentinel — the ONLY name-heuristic branch,
                // reached exclusively by untyped TEXT/JSONB columns.
                format!("'Test {}'", field.label)
            }
        }
    }
}

#[cfg(test)]
mod sentinel_tests {
    use super::*;

    fn field(name: &str, pg_type: &str, input_type: &str, ts_type: &str) -> UiField {
        UiField {
            name: name.to_string(),
            label: "Test Label".to_string(),
            ts_type: ts_type.to_string(),
            input_type: input_type.to_string(),
            is_required: true,
            is_array: pg_type.ends_with("[]"),
            is_entity_ref: false,
            is_immutable: false,
            is_codelist: false,
            is_range: pg_type.contains("RANGE"),
            codelist_values: vec![],
            description: String::new(),
            pg_type: pg_type.to_string(),
            open_end: false,
            ref_api_path: None,
            structured_sub_fields: vec![],
            nested_type_name: None,
        }
    }

    #[test]
    fn numeric_pg_types_emit_numbers_not_text() {
        // The 422 regression: a NUMERIC (FTE) field must never get a text
        // sentinel — one mistyped field rejects every untagged-enum variant.
        assert_eq!(
            test_value_for_field(&field("fte", "NUMERIC(5,2)", "number", "Decimal")),
            "42.5"
        );
        assert_eq!(
            test_value_for_field(&field("rate", "NUMERIC(19,4)", "number", "Decimal")),
            "42.5"
        );
        assert_eq!(
            test_value_for_field(&field("count", "INTEGER", "number", "i32")),
            "42"
        );
        assert_eq!(
            test_value_for_field(&field("big", "BIGINT", "number", "i64")),
            "42"
        );
        assert_eq!(
            test_value_for_field(&field("small", "SMALLINT", "number", "i16")),
            "42"
        );
        assert_eq!(
            test_value_for_field(&field("ratio", "REAL", "number", "f32")),
            "42.5"
        );
        assert_eq!(
            test_value_for_field(&field("score", "DOUBLE PRECISION", "number", "f64")),
            "42.5"
        );
    }

    #[test]
    fn temporal_pg_types_emit_iso_strings() {
        assert_eq!(
            test_value_for_field(&field("d", "DATE", "date", "NaiveDate")),
            "'2026-01-15'"
        );
        assert_eq!(
            test_value_for_field(&field(
                "ts",
                "TIMESTAMPTZ",
                "datetime-local",
                "DateTime<Utc>"
            )),
            "'2026-01-15T08:00:00Z'"
        );
    }

    #[test]
    fn range_pg_types_emit_half_open_range_literals() {
        assert_eq!(
            test_value_for_field(&field("p", "DATERANGE", "date-range", "String")),
            "'[2026-01-01,2026-12-31)'"
        );
        assert_eq!(
            test_value_for_field(&field("p", "TSTZRANGE", "date-range", "String")),
            "'[2026-01-01T00:00:00Z,2026-12-31T23:59:59Z)'"
        );
        assert_eq!(
            test_value_for_field(&field("p", "INT4RANGE", "date-range", "String")),
            "'[1,42)'"
        );
    }

    #[test]
    fn boolean_and_uuid_pg_types() {
        assert_eq!(
            test_value_for_field(&field("flag", "BOOLEAN", "checkbox", "bool")),
            "true"
        );
        let uuid_value = test_value_for_field(&field("case_id", "UUID", "text", "Uuid"));
        assert!(
            uuid_value.starts_with("`00000000-0000-4000-8000-"),
            "{uuid_value}"
        );
        assert!(uuid_value.ends_with('`'), "{uuid_value}");
    }

    #[test]
    fn typed_arrays_and_text_fallback() {
        assert_eq!(
            test_value_for_field(&field("tags", "TEXT[]", "array", "Vec<String>")),
            "['Test Test Label']"
        );
        assert_eq!(
            test_value_for_field(&field("nums", "INTEGER[]", "array", "Vec<i32>")),
            "[42]"
        );
        assert_eq!(
            test_value_for_field(&field("nums", "BIGINT[]", "array", "Vec<i64>")),
            "[42]"
        );
        assert_eq!(
            test_value_for_field(&field("ratios", "DOUBLE PRECISION[]", "array", "Vec<f64>")),
            "[42.5]"
        );
        // Plain text still gets the readable label sentinel.
        assert_eq!(
            test_value_for_field(&field("note", "TEXT", "text", "String")),
            "'Test Test Label'"
        );
        assert_eq!(
            test_value_for_field(&field("extra", "JSONB", "text", "Json")),
            "'Test Test Label'"
        );
    }

    #[test]
    fn geometry_is_omitted_and_entity_refs_untyped() {
        assert_eq!(
            test_value_for_field(&field("geom", "GEOMETRY", "geometry", "String")),
            ""
        );
        // Entity refs never emit a literal — they are filled from depIds.
        let mut r = field("owner_id", "UUID", "text", "Uuid");
        r.is_entity_ref = true;
        assert_eq!(test_value_for_field(&r), "'Test Test Label'");
    }

    #[test]
    fn synthetic_fields_fall_back_to_input_kind() {
        // Injected codelist columns carry generic pg types but meaningful
        // input kinds — the fallback must keep working.
        assert_eq!(
            test_value_for_field(&field("sort_order", "TEXT", "number", "i32")),
            "42"
        );
        assert_eq!(
            test_value_for_field(&field("flag", "TEXT", "checkbox", "bool")),
            "true"
        );
        assert_eq!(
            test_value_for_field(&field("d", "TEXT", "date", "NaiveDate")),
            "'2026-01-15'"
        );
        let code = test_value_for_field(&field("code", "TEXT", "code", "String"));
        assert!(code.starts_with("`TestCode-${"), "{code}");
    }

    #[test]
    fn codelist_values_win_over_pg_type() {
        let mut cl = field("status", "TEXT", "select", "RefundStatusKind");
        cl.is_codelist = true;
        cl.codelist_values = vec!["draft".to_string(), "approved".to_string()];
        assert_eq!(test_value_for_field(&cl), "'draft'");
        cl.is_array = true;
        assert_eq!(test_value_for_field(&cl), "[{ code: 'draft' }]");
    }
}
