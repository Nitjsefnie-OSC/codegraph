//! Lowering of rexlang neutral `expr` bodies to Rust (issue #501).
//!
//! Mox derived features and `expr`-bodied operations store the body as
//! verbatim source text (issue #218 non-goal: codegraph never parses or
//! executes them at ingest). This module parses that text with `rex-expr`
//! (parse only — no model, no typechecking) and lowers a **documented
//! closed subset** to a Rust expression, per the rex-backend-rust contract
//! where class features are in scope as `self.<field>`:
//!
//! | rexlang                            | Rust                              |
//! |------------------------------------|-----------------------------------|
//! | `field` (own-class feature ref)    | `self.<field>` (`.clone()` on non-Copy types) |
//! | `123` / `-123` / `1.5`             | numeric literal                   |
//! | `"text"`                           | `"text"` (escaped)                |
//! | `true` / `false`                   | `true` / `false`                  |
//! | `==` `=` / `!=`                    | `==` / `!=`                       |
//! | `<`, `<=`, `>`, `>=`               | `<`, `<=`, `>`, `>=`              |
//! | `&&`, `\|\|`, `!`                  | `&&`, `||`, `!`                   |
//! | `+`, `-`, `*`, `/`                 | `+`, `-`, `*`, `/`                |
//! | parentheses                        | preserved (fully parenthesized)   |
//!
//! ANYTHING ELSE is refused: `null`, date literals, `?.` safe navigation,
//! dotted relationship navigation, `?:`, operation calls, collection
//! algebra, `if`, `let`, lambdas, list literals, and references to names
//! that are not features of the class. Per the marker convention of the
//! func/rule codegen (#263/#264) and the `expr_sql`/`expr_ts` precedents,
//! what cannot be mapped faithfully comes back as an
//! [`Unsupported`] error so the caller emits a `TODO(#501)` marker — the
//! module is the record, never a guess.

use std::collections::HashMap;

use codegraph_naming::{escape_rust_keyword, to_snake_case};
use rex_expr::{BinOp, Expr, ExprKind, UnOp};

/// One in-scope symbol the lowering may emit: the receiver expression
/// (e.g. `self.candidate_id`) and whether it needs `.clone()` (non-Copy
/// Rust types). Keys are the snake_case field names; resolution also tries
/// the name as written (mirrors `expr_sql`).
#[derive(Debug, Clone)]
pub struct FieldBinding {
    pub recv: String,
    pub clone: bool,
}

/// A construct outside the lowered subset (the caller emits a marker).
#[derive(Debug, Clone)]
pub struct Unsupported {
    pub kind: String,
    pub detail: String,
}

/// Lower rexlang expression `source` to a Rust expression.
pub fn lower_expr_to_rust(
    source: &str,
    fields: &HashMap<String, FieldBinding>,
) -> Result<String, Unsupported> {
    let parsed = rex_expr::parse(source);
    match parsed.ast {
        Some(ast) if parsed.errors.is_empty() => lower(&ast, fields),
        _ => Err(Unsupported {
            kind: "syntax".to_string(),
            detail: format!("not valid rexlang expression syntax: {source:?}"),
        }),
    }
}

fn lower(expr: &Expr, fields: &HashMap<String, FieldBinding>) -> Result<String, Unsupported> {
    let unsupported = |kind: &str, detail: String| {
        Err(Unsupported {
            kind: kind.to_string(),
            detail,
        })
    };
    match &expr.kind {
        ExprKind::Int(value) => Ok(value.to_string()),
        ExprKind::Float(value) => Ok(format!("{value}")),
        ExprKind::String(text) => Ok(format!("{:?}", text)),
        ExprKind::Bool(b) => Ok(b.to_string()),
        ExprKind::Null => unsupported("null", "`null` has no untyped Rust equivalent".to_string()),
        ExprKind::Date { text, .. } => unsupported(
            "date literal",
            format!("date({text:?}) lowers to no primitive Rust type"),
        ),
        ExprKind::Name(name) => symbol_for(name, fields),
        ExprKind::Unary { op, expr } => {
            let inner = lower(expr, fields)?;
            match op {
                UnOp::Not => Ok(format!("(!{inner})")),
                UnOp::Neg => Ok(format!("(-{inner})")),
            }
        }
        ExprKind::Binary { op, lhs, rhs } => {
            let rust_op = match op {
                BinOp::Eq => "==",
                BinOp::Ne => "!=",
                BinOp::Lt => "<",
                BinOp::Le => "<=",
                BinOp::Gt => ">",
                BinOp::Ge => ">=",
                BinOp::And => "&&",
                BinOp::Or => "||",
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => "/",
            };
            Ok(format!(
                "({} {rust_op} {})",
                lower(lhs, fields)?,
                lower(rhs, fields)?
            ))
        }
        ExprKind::FeatureAccess {
            receiver,
            name,
            optional_safe,
        } => {
            let kind = if *optional_safe {
                "safe navigation (`?.`)"
            } else {
                "field access"
            };
            unsupported(
                kind,
                format!(
                    "`{}.{}` — only bare own-class feature refs are supported",
                    write_path(receiver),
                    name.value
                ),
            )
        }
        ExprKind::Coalesce { .. } => unsupported("`?:`", "no Rust elvis operator".to_string()),
        ExprKind::Call { .. } => unsupported(
            "an operation call",
            "operation calls are not lowered".to_string(),
        ),
        ExprKind::Algebra { kind, .. } => {
            unsupported("collection algebra", format!("`{kind}` is not lowered"))
        }
        ExprKind::If { .. } => unsupported("`if`", "if expressions are not lowered".to_string()),
        ExprKind::Let { .. } => unsupported("`let`", "let bindings are not lowered".to_string()),
        ExprKind::Lambda { .. } => unsupported("a lambda", "lambdas are not lowered".to_string()),
        ExprKind::ListLiteral(_) => {
            unsupported("list literals", "list literals are not lowered".to_string())
        }
    }
}

/// Resolve a bare symbol to its receiver expression.
fn symbol_for(name: &str, fields: &HashMap<String, FieldBinding>) -> Result<String, Unsupported> {
    let binding = fields
        .get(name)
        .or_else(|| fields.get(&to_snake_case(name)))
        .ok_or_else(|| Unsupported {
            kind: "unknown symbol".to_string(),
            detail: format!("`{name}` is not a feature of the class"),
        })?;
    let recv = escape_rust_keyword(&binding.recv);
    if binding.clone {
        Ok(format!("{recv}.clone()"))
    } else {
        Ok(recv)
    }
}

/// Render an expression path back to source-like text for diagnostics.
fn write_path(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Name(name) => name.clone(),
        ExprKind::FeatureAccess {
            receiver,
            name,
            optional_safe,
        } => format!(
            "{}{}{}",
            write_path(receiver),
            if *optional_safe { "?." } else { "." },
            name.value
        ),
        _ => "<expression>".to_string(),
    }
}

/// Rust field types that bind bare (Copy) — everything else clones. Mirrors
/// the `eq_arg` convention of the repository emitter: the decision keys off
/// Copy scalars, not off non-Copy spellings.
pub fn is_copy_rust_type(rust_type: &str) -> bool {
    matches!(
        rust_type,
        "i8" | "i16"
            | "i32"
            | "i64"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "usize"
            | "isize"
            | "f32"
            | "f64"
            | "bool"
            | "char"
    )
}

/// The Rust type for a mox primitive `type_ref` (the rex-backend-rust
/// signature mapping). `None` for anything without a primitive Rust
/// mapping — the caller emits a marker instead of a guessed type.
pub fn mox_primitive_rust_type(type_ref: &str) -> Option<&'static str> {
    match type_ref {
        "string" => Some("String"),
        "int" => Some("i32"),
        "long" => Some("i64"),
        "number" => Some("f64"),
        "boolean" => Some("bool"),
        // Date/time builtins carry ISO strings (the TEXT-mapping precedent).
        "date" | "dateTime" | "zonedDateTime" | "time" | "timestamp" => Some("String"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(entries: &[(&str, bool)]) -> HashMap<String, FieldBinding> {
        entries
            .iter()
            .map(|(name, clone)| {
                (
                    name.to_string(),
                    FieldBinding {
                        recv: format!("self.{name}"),
                        clone: *clone,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn symbol_refs_bind_to_self_with_copy_aware_clone() {
        let f = fields(&[("candidate_id", true), ("count", false)]);
        assert_eq!(
            lower_expr_to_rust("candidateId", &f).unwrap(),
            "self.candidate_id.clone()"
        );
        // snake_case lookup of the camelCase written form
        assert_eq!(lower_expr_to_rust("count", &f).unwrap(), "self.count");
    }

    #[test]
    fn comparisons_and_booleans_lower() {
        let f = fields(&[("candidate_id", true)]);
        assert_eq!(
            lower_expr_to_rust("candidateId == \"x\"", &f).unwrap(),
            "(self.candidate_id.clone() == \"x\")"
        );
        assert_eq!(
            lower_expr_to_rust("candidateId == \"a\" && 1 < 2", &f).unwrap(),
            "((self.candidate_id.clone() == \"a\") && (1 < 2))"
        );
        assert_eq!(lower_expr_to_rust("!false", &f).unwrap(), "(!false)");
    }

    #[test]
    fn arithmetic_lowers() {
        let f = fields(&[("count", false)]);
        assert_eq!(
            lower_expr_to_rust("count + 1", &f).unwrap(),
            "(self.count + 1)"
        );
    }

    #[test]
    fn unknown_symbols_and_exotic_constructs_are_refused() {
        let f = fields(&[("candidate_id", true)]);
        let err = lower_expr_to_rust("ghost", &f).unwrap_err();
        assert_eq!(err.kind, "unknown symbol");
        for source in [
            "candidateId.upper",
            "null",
            "a ?: b",
            "if true { 1 } else { 2 }",
        ] {
            assert!(lower_expr_to_rust(source, &f).is_err(), "{source}");
        }
    }

    #[test]
    fn primitive_type_mapping_follows_the_rex_contract() {
        assert_eq!(mox_primitive_rust_type("int"), Some("i32"));
        assert_eq!(mox_primitive_rust_type("string"), Some("String"));
        assert_eq!(mox_primitive_rust_type("boolean"), Some("bool"));
        assert_eq!(mox_primitive_rust_type("CandidateType"), None);
    }
}
