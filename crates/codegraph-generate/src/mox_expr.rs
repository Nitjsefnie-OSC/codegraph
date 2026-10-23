//! Lowering of rexlang neutral `expr` bodies to Rust (issues #501/#508).
//!
//! Mox derived features and `expr`-bodied operations store the body as
//! verbatim source text. This module parses that text with `rex-expr`
//! (parse only — no model, no typechecking) and lowers a **normative
//! closed subset** to a Rust expression, targeting parity with what
//! `rex-backend-rust` lowers (the Tier-2 `expr_lower.rs` contract).
//!
//! # The normative subset
//!
//! | rexlang                              | Rust                                        |
//! |--------------------------------------|---------------------------------------------|
//! | `field` (own-class feature)          | `self.<field>` (`.clone()` on non-Copy)     |
//! | `123` / `-123`                       | width-suffixed int (`i32`, `i64` in long context) |
//! | `1.5`                                | float literal (`f64`/`f32` by context)      |
//! | `"text"`                             | `"text".to_string()` (`&str` inside `==`/`!=`) |
//! | `true` / `false`                     | `true` / `false`                            |
//! | `date("YYYY-MM-DD")`                 | `chrono::NaiveDate::from_ymd_opt(..).expect(..)` |
//! | `null` in `==`/`!=`                  | `is_none()` / constant (spec R2)            |
//! | `a ?: b`                             | `a.unwrap_or(b)` (eager default, spec R3)   |
//! | `==` `=` / `!=` / `<` `<=` `>` `>=`  | same (fully parenthesized)                  |
//! | `&&`, `\|\|`, `!` / `+`, `-`, `*`, `/` | same (fully parenthesized)                |
//! | `if c { t } else { e }`              | Rust if-expression (spec U1)                |
//! | `let x = init; body`                 | `{ let x = init; body }` (locals clone on use) |
//! | `[a, b]`                             | `vec![a, b]` (empty list refused)           |
//! | `xs.first(p)` / `first()` / `filter` / `map` / `any` / `size` / `sum` | A1–A6 collection algebra |
//! | `x.field` / `x?.field` (class id x)  | `res.get_<class>(x).map(\|o\| ..)` arena navigation |
//! | `x.op(args)` (class id x)            | `res.get_<class>(x).expect(..).<op>(res, args)` |
//! | `d.plus_days(n)` / `plus_months` / `diff_days` | chrono date algebra (clamp/overflow per R5/R8) |
//!
//! `res` is the generated `Resource` arena (issue #508 R4): lookups are
//! `res.get_<class>(id) -> Option<&<Mirror>>` over whatever the runner
//! loaded; a lookup that misses panics on plain navigation
//! (`expect("dangling .. id")`) and yields `None` under `?.` — the same
//! contract `rex-backend-rust` emits.
//!
//! # Refusals are generation errors (issue #508 R3)
//!
//! ANYTHING ELSE — standalone `null`, bare lambdas, arithmetic on
//! unrepresentable types, unknown names, empty list literals, method
//! calls that are neither date algebra nor declared operations — comes
//! back as [`Unsupported`]. The caller raises a **generation error**
//! naming the operation/feature (the `expr_sql`/`expr_ts` precedent):
//! never a `todo!()` stub, never a silent passthrough.

use std::collections::HashMap;

use codegraph_naming::{escape_rust_keyword, to_snake_case};
use rex_expr::{BinOp, Expr, ExprKind, UnOp};

/// One in-scope symbol the lowering may emit.
#[derive(Debug, Clone)]
pub struct FieldBinding {
    /// The receiver expression for a bare read (e.g. `self.candidate_id`).
    pub recv: String,
    /// Whether a bare read clones (non-Copy Rust types).
    pub clone: bool,
    /// The Rust field type (`String`, `i32`, `Uuid`, `Option<Uuid>`,
    /// `Vec<Uuid>`, `chrono::NaiveDate`, …) — drives width suffixes,
    /// Option shapes, and date algebra.
    pub rust_type: String,
    /// Set when the field carries class identity (to-one `Option<Uuid>` /
    /// `Uuid`, to-many `Vec<Uuid>`): enables arena navigation.
    pub class: Option<String>,
}

impl FieldBinding {
    pub fn new(recv: String, clone: bool, rust_type: String) -> Self {
        Self {
            recv,
            clone,
            rust_type,
            class: None,
        }
    }

    pub fn with_class(mut self, class: impl Into<String>) -> Self {
        self.class = Some(class.into());
        self
    }

    fn is_option(&self) -> bool {
        self.rust_type.starts_with("Option<")
    }

    fn is_many(&self) -> bool {
        self.rust_type.starts_with("Vec<")
    }

    fn element_type(&self) -> &str {
        self.rust_type
            .strip_prefix("Vec<")
            .or_else(|| self.rust_type.strip_prefix("Option<"))
            .and_then(|inner| inner.strip_suffix('>'))
            .unwrap_or(&self.rust_type)
    }
}

/// A class reachable through the `res` arena.
#[derive(Debug, Clone)]
pub struct ClassBinding {
    /// Mirror struct name in the mox module (`Book`).
    pub mirror: String,
    /// Arena getter name (`get_book`).
    pub getter: String,
    /// Rust type of the class id (`Uuid`).
    pub id_type: String,
    /// Features of the class (keys as written + snake_case).
    pub fields: HashMap<String, FieldBinding>,
    /// Operations of the class keyed by written + snake name.
    pub ops: HashMap<String, OpBinding>,
}

/// A declared operation callable through the arena.
#[derive(Debug, Clone)]
pub struct OpBinding {
    /// Rust method name (`pen_name`).
    pub snake: String,
    /// Rust parameter types (for literal width adaptation).
    pub params: Vec<String>,
}

/// The class universe bodies may navigate. Own-class fields stay in the
/// per-call `fields` map; `arena` answers `x.field` / `x.op(..)`.
pub type ArenaIndex = HashMap<String, ClassBinding>;

/// A construct outside the lowered subset (the caller raises a
/// generation error naming the op/feature).
#[derive(Debug, Clone)]
pub struct Unsupported {
    pub kind: String,
    pub detail: String,
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.detail)
    }
}

/// Lower rexlang expression `source` to a Rust expression.
pub fn lower_expr_to_rust(
    source: &str,
    fields: &HashMap<String, FieldBinding>,
    arena: &ArenaIndex,
) -> Result<String, Unsupported> {
    let parsed = rex_expr::parse(source);
    match parsed.ast {
        Some(ast) if parsed.errors.is_empty() => {
            lower(&ast, fields, arena, TyHint::Unknown, &mut Vec::new())
        }
        _ => Err(Unsupported {
            kind: "syntax".to_string(),
            detail: format!("not valid rexlang expression syntax: {source:?}"),
        }),
    }
}

/// Contextual type hint for literal width adaptation (L1-lite): an
/// integer literal in a long context is `i64`, otherwise `i32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TyHint {
    Unknown,
    Long,
    I32,
    F64,
    F32,
    /// Inside `==`/`!=` where string literals bind as `&str`.
    EqCtx,
    /// Receiver of a date method (chrono).
    Date,
    /// String-typed — string literals strip to `&str` inside `==`/`!=`.
    Str,
}

fn hint_of(rust_type: &str) -> TyHint {
    // Look through `Option<..>`/`Vec<..>` wrappers (issue #517: DTO-shaped
    // mirror field types carry them) at the element type.
    let mut rust_type = rust_type;
    loop {
        let stripped = rust_type
            .strip_prefix("Option<")
            .or_else(|| rust_type.strip_prefix("Vec<"))
            .and_then(|s| s.strip_suffix('>'));
        match stripped {
            Some(inner) => rust_type = inner,
            None => break,
        }
    }
    if rust_type == "i64" || rust_type == "u64" {
        TyHint::Long
    } else if rust_type == "i32" {
        TyHint::I32
    } else if rust_type == "f64" {
        TyHint::F64
    } else if rust_type == "f32" {
        TyHint::F32
    } else if rust_type.starts_with("chrono::NaiveDate") {
        TyHint::Date
    } else if rust_type.contains("String") {
        TyHint::Str
    } else {
        TyHint::Unknown
    }
}

#[derive(Debug, Clone)]
struct LocalBinding {
    name: String,
    emitted: String,
    read: String,
    binding: Option<FieldBinding>,
}

type Locals = Vec<LocalBinding>;

fn fresh_local_name(base: &str, locals: &Locals, used: &str) -> String {
    let mut name = base.to_string();
    while name == "res" || locals.iter().any(|local| local.emitted == name) || used.contains(&name)
    {
        name.push('_');
    }
    name
}

fn lower(
    expr: &Expr,
    fields: &HashMap<String, FieldBinding>,
    arena: &ArenaIndex,
    hint: TyHint,
    locals: &mut Locals,
) -> Result<String, Unsupported> {
    let unsupported = |kind: &str, detail: String| {
        Err(Unsupported {
            kind: kind.to_string(),
            detail,
        })
    };
    match &expr.kind {
        ExprKind::Int(value) => Ok(match hint {
            TyHint::Long => format!("{value}i64"),
            _ => format!("{value}i32"),
        }),
        ExprKind::Float(value) => Ok(match hint {
            TyHint::F32 => format!("{value}f32"),
            _ => format!("{value}f64"),
        }),
        ExprKind::String(text) => Ok(match hint {
            TyHint::EqCtx => format!("{text:?}"),
            _ => format!("{text:?}.to_string()"),
        }),
        ExprKind::Bool(b) => Ok(b.to_string()),
        ExprKind::Null => unsupported(
            "null",
            "`null` is only valid in equality and `?:` contexts (spec R2/R3)".to_string(),
        ),
        ExprKind::Date { text, .. } => lower_date_literal(text),
        ExprKind::Name(name) => symbol_for(name, fields, locals),
        ExprKind::Unary { op, expr } => {
            let inner = lower(expr, fields, arena, hint, locals)?;
            match op {
                UnOp::Not => Ok(format!("(!{inner})")),
                UnOp::Neg => Ok(format!("(-{inner})")),
            }
        }
        ExprKind::Binary { op, lhs, rhs } => match op {
            BinOp::Eq | BinOp::Ne => equality(*op == BinOp::Ne, lhs, rhs, fields, arena, locals),
            _ => {
                let rust_op = match op {
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
                    BinOp::Eq | BinOp::Ne => unreachable!(),
                };
                // L1-lite: an int literal alongside a long operand is long.
                let operand_hint = operand_hint(lhs, rhs, fields, locals);
                let l = lower(lhs, fields, arena, operand_hint, locals)?;
                let r = lower(rhs, fields, arena, operand_hint, locals)?;
                Ok(format!("({l} {rust_op} {r})"))
            }
        },
        ExprKind::FeatureAccess {
            receiver,
            name,
            optional_safe,
        } => feature_access(receiver, &name.value, *optional_safe, fields, arena, locals),
        ExprKind::Coalesce { value, default } => {
            let v = lower(value, fields, arena, hint, locals)?;
            let d = lower(default, fields, arena, hint, locals)?;
            if v == "null" {
                return Ok(d);
            }
            Ok(format!("{v}.unwrap_or({d})"))
        }
        ExprKind::Call {
            receiver,
            name,
            args,
            optional_safe,
        } => call(
            receiver,
            &name.value,
            args,
            *optional_safe,
            fields,
            arena,
            locals,
        ),
        ExprKind::Algebra {
            kind,
            receiver,
            lambda,
            optional_safe,
        } => algebra(
            kind,
            receiver,
            lambda.as_ref().map(|(p, b)| (p.as_str(), b.as_ref())),
            *optional_safe,
            fields,
            arena,
            locals,
        ),
        ExprKind::If { cond, then, else_ } => {
            let c = lower(cond, fields, arena, TyHint::Unknown, locals)?;
            let t = lower(then, fields, arena, hint, locals)?;
            let e = lower(else_, fields, arena, hint, locals)?;
            Ok(format!("if {c} {{ {t} }} else {{ {e} }}"))
        }
        ExprKind::Let { name, init, body } => {
            let init_code = lower(init, fields, arena, TyHint::Unknown, locals)?;
            let let_name = escape_rust_keyword(&to_snake_case(&name.value));
            // Locals bind by value; uses clone (parity with rex-backend-rust).
            locals.push(LocalBinding {
                name: name.value.clone(),
                emitted: let_name.clone(),
                read: format!("{let_name}.clone()"),
                binding: None,
            });
            let body_code = lower(body, fields, arena, hint, locals);
            locals.pop();
            let body_code = body_code?;
            Ok(format!("{{ let {let_name} = {init_code}; {body_code} }}"))
        }
        ExprKind::Lambda { .. } => unsupported(
            "a lambda",
            "a lambda is only valid inside a collection algebra call".to_string(),
        ),
        ExprKind::ListLiteral(items) => {
            if items.is_empty() {
                return unsupported(
                    "list literal",
                    "cannot infer the element type of an empty list literal".to_string(),
                );
            }
            let mut elems = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let h = if i == 0 { TyHint::Unknown } else { hint };
                elems.push(lower(item, fields, arena, h, locals)?);
            }
            Ok(format!("vec![{}]", elems.join(", ")))
        }
    }
}

/// Spec R2: equality with `null`, string-literal `&str` strips, and
/// plain same-type comparisons (never moving — Rust borrows both sides).
fn equality(
    negated: bool,
    lhs: &Expr,
    rhs: &Expr,
    fields: &HashMap<String, FieldBinding>,
    arena: &ArenaIndex,
    locals: &mut Locals,
) -> Result<String, Unsupported> {
    let is_null = |e: &Expr| matches!(e.kind, ExprKind::Null);
    match (is_null(lhs), is_null(rhs)) {
        (true, true) => return Ok((!negated).to_string()),
        (true, false) | (false, true) => {
            let (value, _) = if is_null(lhs) { (rhs, lhs) } else { (lhs, rhs) };
            let binding = binding_of(value, fields, locals);
            let code = lower(value, fields, arena, TyHint::Unknown, locals)?;
            let opt = binding.as_ref().map(|b| b.is_option()).unwrap_or(false)
                || matches!(value.kind, ExprKind::Coalesce { .. })
                || matches!(
                    &value.kind,
                    ExprKind::FeatureAccess {
                        optional_safe: true,
                        ..
                    }
                );
            if opt {
                let check = format!("{code}.is_none()");
                return Ok(if negated {
                    format!("(!{check})")
                } else {
                    check
                });
            }
            // A non-optional value can never be absent: the answer is constant.
            return Ok((negated).to_string());
        }
        (false, false) => {}
    }
    let hint = match (
        binding_of(lhs, fields, locals),
        binding_of(rhs, fields, locals),
    ) {
        (Some(l), _) => hint_of(&l.rust_type),
        (_, Some(r)) => hint_of(&r.rust_type),
        _ => TyHint::Unknown,
    };
    // String literals inside ==/!= bind as &str (no allocation).
    let hint = if matches!(hint, TyHint::Str) {
        TyHint::EqCtx
    } else {
        hint
    };
    let l = lower(lhs, fields, arena, hint, locals)?;
    let r = lower(rhs, fields, arena, hint, locals)?;
    let op = if negated { "!=" } else { "==" };
    Ok(format!("({l} {op} {r})"))
}

/// L1-lite: when one operand carries a known width, the other operand's
/// literal follows it.
fn operand_hint(
    lhs: &Expr,
    rhs: &Expr,
    fields: &HashMap<String, FieldBinding>,
    locals: &Locals,
) -> TyHint {
    for e in [rhs, lhs] {
        if let Some(b) = binding_of(e, fields, locals) {
            let h = hint_of(&b.rust_type);
            if matches!(h, TyHint::Long | TyHint::I32 | TyHint::F64 | TyHint::F32) {
                return h;
            }
        }
    }
    TyHint::Unknown
}

/// Resolve the binding behind a symbol expression, if any.
fn binding_of(
    expr: &Expr,
    fields: &HashMap<String, FieldBinding>,
    locals: &Locals,
) -> Option<FieldBinding> {
    match &expr.kind {
        ExprKind::Name(name) => {
            if let Some(local) = locals.iter().rev().find(|local| local.name == *name) {
                return local.binding.clone();
            }
            fields
                .get(name)
                .or_else(|| fields.get(&to_snake_case(name)))
                .cloned()
        }
        _ => None,
    }
}

/// `date("YYYY-MM-DD")` — format-validated at parse time by the rex
/// checker upstream; we still refuse malformed text rather than guess.
fn lower_date_literal(text: &str) -> Result<String, Unsupported> {
    let parts: Vec<&str> = text.split('-').collect();
    let valid = parts.len() == 3
        && parts[0].len() == 4
        && parts[1].len() == 2
        && parts[2].len() == 2
        && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()));
    if !valid {
        return Err(Unsupported {
            kind: "date literal".to_string(),
            detail: format!("`date({text:?})` is not a YYYY-MM-DD date"),
        });
    }
    let (y, m, d) = (parts[0], parts[1], parts[2]);
    Ok(format!(
        "chrono::NaiveDate::from_ymd_opt({y}, {m}, {d}).expect(\"R6-checked date literal\")"
    ))
}

fn symbol_for(
    name: &str,
    fields: &HashMap<String, FieldBinding>,
    locals: &Locals,
) -> Result<String, Unsupported> {
    // Locals first (shadowing), clone-on-use — parity with rex-backend-rust.
    if let Some(local) = locals.iter().rev().find(|local| local.name == name) {
        return Ok(local.read.clone());
    }
    let binding = fields
        .get(name)
        .or_else(|| fields.get(&to_snake_case(name)))
        .ok_or_else(|| Unsupported {
            kind: "unknown symbol".to_string(),
            detail: format!("`{name}` is not a feature of the class"),
        })?;
    Ok(bind_read(binding))
}

/// A bare read of one field binding.
fn bind_read(binding: &FieldBinding) -> String {
    let recv = escape_rust_keyword(&binding.recv);
    if binding.clone {
        format!("{recv}.clone()")
    } else {
        recv
    }
}

/// Field access on a resolved object `o` (the arena `map`/`and_then` shape).
fn field_shape(binding: &FieldBinding, obj: &str) -> String {
    let field = escape_rust_keyword(&to_snake_case(
        binding.recv.rsplit('.').next().unwrap_or(&binding.recv),
    ));
    if binding.clone {
        format!("{obj}.{field}.clone()")
    } else {
        format!("{obj}.{field}")
    }
}

/// `x.field` / `x?.field` — arena navigation when the receiver carries a
/// class id; the two idioms from the rex contract (plain `map().expect()`,
/// safe `and_then()` chains).
fn feature_access(
    receiver: &Expr,
    name: &str,
    optional_safe: bool,
    fields: &HashMap<String, FieldBinding>,
    arena: &ArenaIndex,
    locals: &mut Locals,
) -> Result<String, Unsupported> {
    let recv_binding = binding_of(receiver, fields, locals);
    let class_name = recv_binding
        .as_ref()
        .and_then(|b| b.class.clone())
        .ok_or_else(|| Unsupported {
            kind: if optional_safe {
                "safe navigation (`?.`)".to_string()
            } else {
                "field access".to_string()
            },
            detail: format!(
                "`{}.{}` — only class-id receivers navigate (arena lookups)",
                write_path(receiver),
                name
            ),
        })?;
    let class = class_of(arena, &class_name)?;
    let target = class
        .fields
        .get(name)
        .or_else(|| class.fields.get(&to_snake_case(name)))
        .ok_or_else(|| Unsupported {
            kind: "unknown feature".to_string(),
            detail: format!("`{name}` is not a feature of `{}`", class.mirror),
        })?;

    let recv_code = lower(receiver, fields, arena, TyHint::Unknown, locals)?;
    let recv_is_option = recv_binding
        .as_ref()
        .map(|b| b.is_option())
        .unwrap_or(false);
    let id_expr = recv_code.clone();

    let obj = fresh_local_name("o", locals, "");
    let plain = format!(
        "res.{getter}({id_expr}).map(|{obj}| {shape}).expect(\"dangling `{mirror}` id\")",
        getter = class.getter,
        shape = field_shape(target, &obj),
        mirror = class.mirror,
    );
    if !optional_safe {
        return Ok(format!("({plain})"));
    }
    let target_is_option = target.is_option();
    match (recv_is_option, target_is_option) {
        (true, _) => Ok(format!(
            "({id_expr}.and_then(|id| res.{getter}(id)).map(|{obj}| {shape}))",
            getter = class.getter,
            shape = field_shape(target, &obj),
        )),
        (false, true) => Ok(format!("({plain})")),
        (false, false) => Ok(format!("(Some({plain}))")),
    }
}

/// Operation call: date algebra on date receivers, declared ops through
/// the arena on class-id receivers.
fn call(
    receiver: &Expr,
    method: &str,
    args: &[Expr],
    optional_safe: bool,
    fields: &HashMap<String, FieldBinding>,
    arena: &ArenaIndex,
    locals: &mut Locals,
) -> Result<String, Unsupported> {
    let recv_binding = binding_of(receiver, fields, locals);
    let recv_hint = recv_binding
        .as_ref()
        .map(|b| hint_of(&b.rust_type))
        .unwrap_or(TyHint::Unknown);
    let recv_date_elem = recv_binding
        .as_ref()
        .map(|b| b.is_option() && hint_of(b.element_type()) == TyHint::Date)
        .unwrap_or(false);

    if recv_hint == TyHint::Date || recv_date_elem {
        return date_method(receiver, method, args, fields, arena, locals, &recv_binding);
    }

    let class_name = recv_binding
        .as_ref()
        .and_then(|b| b.class.clone())
        .ok_or_else(|| Unsupported {
            kind: "an operation call".to_string(),
            detail: format!(
                "`{}.{method}(..)` — receivers must carry a class id",
                write_path(receiver)
            ),
        })?;
    let class = class_of(arena, &class_name)?;
    let op = class
        .ops
        .get(method)
        .or_else(|| class.ops.get(&to_snake_case(method)))
        .ok_or_else(|| Unsupported {
            kind: "an operation call".to_string(),
            detail: format!(
                "`{method}` is not a declared operation of `{}`",
                class.mirror
            ),
        })?;
    if op.params.len() != args.len() {
        return Err(Unsupported {
            kind: "an operation call".to_string(),
            detail: format!(
                "`{method}` expects {} argument(s), got {}",
                op.params.len(),
                args.len()
            ),
        });
    }
    let mut lowered_args = Vec::with_capacity(args.len());
    for (arg, ty) in args.iter().zip(&op.params) {
        lowered_args.push(lower(arg, fields, arena, hint_of(ty), locals)?);
    }
    let arg_list = if lowered_args.is_empty() {
        "res".to_string()
    } else {
        format!("res, {}", lowered_args.join(", "))
    };

    let recv_code = lower(receiver, fields, arena, TyHint::Unknown, locals)?;
    let recv_is_option = recv_binding
        .as_ref()
        .map(|b| b.is_option())
        .unwrap_or(false);
    let id_expr = recv_code.clone();
    let invoke = format!("{}({arg_list})", op.snake);

    if recv_is_option {
        let obj = fresh_local_name("o", locals, &arg_list);
        Ok(format!(
            "({id_expr}.and_then(|id| res.{getter}(id)).map(|{obj}| {obj}.{invoke}))",
            getter = class.getter
        ))
    } else {
        let plain = format!(
            "res.{getter}({id_expr}).expect(\"dangling `{mirror}` id\").{invoke}",
            getter = class.getter,
            mirror = class.mirror
        );
        if optional_safe {
            Ok(format!("(Some({plain}))"))
        } else {
            Ok(format!("({plain})"))
        }
    }
}

/// Date calendar algebra (R5/R8): exactly the three methods the rex
/// checker admits.
fn date_method(
    receiver: &Expr,
    method: &str,
    args: &[Expr],
    fields: &HashMap<String, FieldBinding>,
    arena: &ArenaIndex,
    locals: &mut Locals,
    recv_binding: &Option<FieldBinding>,
) -> Result<String, Unsupported> {
    let one_arg = |m: &str| -> Result<&Expr, Unsupported> {
        if args.len() == 1 {
            Ok(&args[0])
        } else {
            Err(Unsupported {
                kind: "date algebra".to_string(),
                detail: format!("`{m}` takes exactly one argument"),
            })
        }
    };
    let recv_code = lower(receiver, fields, arena, TyHint::Date, locals)?;
    let recv_is_option = recv_binding
        .as_ref()
        .map(|b| b.is_option())
        .unwrap_or(false);
    match method {
        "plusDays" | "plus_days" => {
            let n = lower(one_arg(method)?, fields, arena, TyHint::I32, locals)?;
            let body = format!("({recv_code} + chrono::Duration::days({n} as i64))");
            Ok(wrap_optional_date(recv_is_option, &recv_code, &body))
        }
        "plusMonths" | "plus_months" => {
            let n = lower(one_arg(method)?, fields, arena, TyHint::I32, locals)?;
            let body = format!("({recv_code} + chrono::Months::new({n} as u32))");
            Ok(wrap_optional_date(recv_is_option, &recv_code, &body))
        }
        "diffDays" | "diff_days" => {
            let other = lower(one_arg(method)?, fields, arena, TyHint::Date, locals)?;
            Ok(format!("(({recv_code} - {other}).num_days() as i32)"))
        }
        _ => Err(Unsupported {
            kind: "date algebra".to_string(),
            detail: format!("`{method}` is not date algebra (only plusDays/plusMonths/diffDays)"),
        }),
    }
}

/// `?.` on an optional date receiver maps over the Option (the
/// `__rex_date` idiom from the rex contract).
fn wrap_optional_date(recv_is_option: bool, recv_code: &str, body: &str) -> String {
    if recv_is_option {
        // `body` was built from `recv_code`; rebuild as a map over the inner value.
        let inner = body.replace(recv_code, "__rex_date");
        format!("({recv_code}.map(|__rex_date| {inner}))")
    } else {
        body.to_string()
    }
}

/// A1–A6 collection algebra over to-many features.
fn algebra(
    kind: &rex_expr::AlgebraKind,
    receiver: &Expr,
    lambda: Option<(&str, &Expr)>,
    optional_safe: bool,
    fields: &HashMap<String, FieldBinding>,
    arena: &ArenaIndex,
    locals: &mut Locals,
) -> Result<String, Unsupported> {
    use rex_expr::AlgebraKind;
    let recv_binding = binding_of(receiver, fields, locals).ok_or_else(|| Unsupported {
        kind: "collection algebra".to_string(),
        detail: format!("`{kind:?}` receiver must be a feature"),
    })?;
    if !recv_binding.is_many() {
        return Err(Unsupported {
            kind: "collection algebra".to_string(),
            detail: format!("`{kind:?}` requires a to-many feature"),
        });
    }
    let recv_code = bind_read(&recv_binding);
    let elem_copy = is_copy_rust_type(recv_binding.element_type());
    let adapter = if elem_copy { ".copied()" } else { ".cloned()" };

    // Closure params (the rex refs convention: map/any lend 1 ref,
    // find/filter lend 2).
    let push_local = |locals: &mut Locals, param: &str, refs: usize| {
        let var = fresh_local_name(&escape_rust_keyword(&to_snake_case(param)), locals, "");
        let deref = "*".repeat(refs);
        let binding = FieldBinding {
            recv: format!("({deref}{var})"),
            clone: !elem_copy,
            rust_type: recv_binding.element_type().to_string(),
            class: recv_binding.class.clone(),
        };
        locals.push(LocalBinding {
            name: param.to_string(),
            emitted: var.clone(),
            read: bind_read(&binding),
            binding: Some(binding),
        });
        var
    };

    // `?.` wraps non-`first` results in Some (spec A1).
    let wrap = |code: String| -> String {
        if optional_safe {
            match kind {
                AlgebraKind::First => code,
                _ => format!("Some({code})"),
            }
        } else {
            code
        }
    };

    match kind {
        AlgebraKind::First => match lambda {
            Some((param, body)) => {
                let var = push_local(locals, param, 2);
                let pred = lower(body, fields, arena, TyHint::Unknown, locals);
                locals.pop();
                let pred = pred?;
                Ok(wrap(format!(
                    "({recv_code}.iter().find(|{var}| {pred}){adapter})"
                )))
            }
            None => Ok(wrap(format!("({recv_code}.first(){adapter})"))),
        },
        AlgebraKind::Filter => {
            let Some((param, body)) = lambda else {
                return Err(Unsupported {
                    kind: "collection algebra".to_string(),
                    detail: "`filter` requires a lambda".to_string(),
                });
            };
            let var = push_local(locals, param, 2);
            let pred = lower(body, fields, arena, TyHint::Unknown, locals);
            locals.pop();
            let pred = pred?;
            Ok(wrap(format!(
                "({recv_code}.iter().filter(|{var}| {pred}){adapter}.collect::<Vec<_>>())"
            )))
        }
        AlgebraKind::Map => {
            let Some((param, body)) = lambda else {
                return Err(Unsupported {
                    kind: "collection algebra".to_string(),
                    detail: "`map` requires a lambda".to_string(),
                });
            };
            let var = push_local(locals, param, 1);
            let mapped = lower(body, fields, arena, TyHint::Unknown, locals);
            locals.pop();
            let mapped = mapped?;
            Ok(wrap(format!(
                "({recv_code}.iter().map(|{var}| {mapped}).collect::<Vec<_>>())"
            )))
        }
        AlgebraKind::Any => {
            let Some((param, body)) = lambda else {
                return Err(Unsupported {
                    kind: "collection algebra".to_string(),
                    detail: "`any` requires a lambda".to_string(),
                });
            };
            let var = push_local(locals, param, 1);
            let pred = lower(body, fields, arena, TyHint::Unknown, locals);
            locals.pop();
            let pred = pred?;
            Ok(wrap(format!("({recv_code}.iter().any(|{var}| {pred}))")))
        }
        AlgebraKind::Size => Ok(wrap(format!("({recv_code}.len() as i32)"))),
        AlgebraKind::Sum => {
            let elem = recv_binding.element_type().to_string();
            if !is_copy_rust_type(&elem) && !elem.starts_with("chrono::") {
                return Err(Unsupported {
                    kind: "collection algebra".to_string(),
                    detail: format!("`sum` over non-numeric element type `{elem}`"),
                });
            }
            Ok(wrap(format!("({recv_code}.iter().sum::<{elem}>())")))
        }
    }
}

/// Resolve a class binding by written, snake, or mirror name.
fn class_of<'a>(arena: &'a ArenaIndex, name: &str) -> Result<&'a ClassBinding, Unsupported> {
    arena
        .get(name)
        .or_else(|| arena.get(&to_snake_case(name)))
        .ok_or_else(|| Unsupported {
            kind: "unknown class".to_string(),
            detail: format!("`{name}` is not a class of the model"),
        })
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
            | "uuid::Uuid"
            | "Uuid"
    )
}

/// The Rust type for a mox primitive `type_ref` (the rex-backend-rust
/// signature mapping). `None` for anything without a primitive Rust
/// mapping — the caller raises a generation error instead of a guessed
/// type.
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

    fn fields(entries: &[(&str, bool, &str)]) -> HashMap<String, FieldBinding> {
        entries
            .iter()
            .map(|(name, clone, ty)| {
                (
                    name.to_string(),
                    FieldBinding::new(format!("self.{name}"), *clone, (*ty).to_string()),
                )
            })
            .collect()
    }

    fn empty_arena() -> ArenaIndex {
        ArenaIndex::new()
    }

    #[test]
    fn symbol_refs_bind_to_self_with_copy_aware_clone() {
        let f = fields(&[("candidate_id", true, "String"), ("count", false, "i32")]);
        assert_eq!(
            lower_expr_to_rust("candidateId", &f, &empty_arena()).unwrap(),
            "self.candidate_id.clone()"
        );
        assert_eq!(
            lower_expr_to_rust("count", &f, &empty_arena()).unwrap(),
            "self.count"
        );
    }

    #[test]
    fn comparisons_and_booleans_lower_with_literal_widths() {
        let f = fields(&[("candidate_id", true, "String"), ("count", false, "i32")]);
        assert_eq!(
            lower_expr_to_rust("candidateId == \"x\"", &f, &empty_arena()).unwrap(),
            "(self.candidate_id.clone() == \"x\")"
        );
        assert_eq!(
            lower_expr_to_rust("count > 2", &f, &empty_arena()).unwrap(),
            "(self.count > 2i32)"
        );
        assert_eq!(
            lower_expr_to_rust("!false", &f, &empty_arena()).unwrap(),
            "(!false)"
        );
    }

    #[test]
    fn long_context_suffixes_integer_literals() {
        let f = fields(&[("downloads", false, "i64")]);
        assert_eq!(
            lower_expr_to_rust("downloads + 1", &f, &empty_arena()).unwrap(),
            "(self.downloads + 1i64)"
        );
    }

    #[test]
    fn null_equality_follows_spec_r2() {
        let f = fields(&[("name", true, "String"), ("opt", true, "Option<String>")]);
        // Non-optional value can never be absent: constant answers.
        assert_eq!(
            lower_expr_to_rust("name == null", &f, &empty_arena()).unwrap(),
            "false"
        );
        assert_eq!(
            lower_expr_to_rust("name != null", &f, &empty_arena()).unwrap(),
            "true"
        );
        // Optional value: is_none() checks.
        assert_eq!(
            lower_expr_to_rust("opt == null", &f, &empty_arena()).unwrap(),
            "self.opt.clone().is_none()"
        );
    }

    #[test]
    fn coalesce_if_let_and_lists_lower() {
        let f = fields(&[("count", false, "i32"), ("name", true, "String")]);
        assert_eq!(
            lower_expr_to_rust("name ?: \"anon\"", &f, &empty_arena()).unwrap(),
            "self.name.clone().unwrap_or(\"anon\".to_string())"
        );
        assert_eq!(
            lower_expr_to_rust(
                "if count > 3 { name } else { \"short\" }",
                &f,
                &empty_arena()
            )
            .unwrap(),
            "if (self.count > 3i32) { self.name.clone() } else { \"short\".to_string() }"
        );
        assert!(
            lower_expr_to_rust("let x = count; x", &f, &empty_arena())
                .unwrap()
                .contains("let x = self.count;")
        );
        assert_eq!(
            lower_expr_to_rust("[1, 2]", &f, &empty_arena()).unwrap(),
            "vec![1i32, 2i32]"
        );
    }

    #[test]
    fn date_literals_and_algebra_lower_to_chrono() {
        let f = fields(&[("effective", false, "chrono::NaiveDate")]);
        assert_eq!(
            lower_expr_to_rust("effective.plusDays(1)", &f, &empty_arena()).unwrap(),
            "(self.effective + chrono::Duration::days(1i32 as i64))"
        );
        assert_eq!(
            lower_expr_to_rust("effective.plusMonths(6)", &f, &empty_arena()).unwrap(),
            "(self.effective + chrono::Months::new(6i32 as u32))"
        );
        let lit =
            lower_expr_to_rust("effective < date(\"2027-01-01\")", &f, &empty_arena()).unwrap();
        assert!(
            lit.contains("chrono::NaiveDate::from_ymd_opt(2027, 01, 01)"),
            "{lit}"
        );
    }

    #[test]
    fn collection_algebra_lowers_the_a1_a6_shapes() {
        let mut f = fields(&[("count", false, "i32")]);
        f.insert(
            "pages_list".to_string(),
            FieldBinding::new("self.pages_list".into(), true, "Vec<i32>".into()),
        );
        assert_eq!(
            lower_expr_to_rust("pagesList.size()", &f, &empty_arena()).unwrap(),
            "(self.pages_list.clone().len() as i32)"
        );
        assert_eq!(
            lower_expr_to_rust("pagesList.sum()", &f, &empty_arena()).unwrap(),
            "(self.pages_list.clone().iter().sum::<i32>())"
        );
        assert_eq!(
            lower_expr_to_rust("pagesList.map(p => p + 1)", &f, &empty_arena()).unwrap(),
            "(self.pages_list.clone().iter().map(|p| ((*p) + 1i32)).collect::<Vec<_>>())"
        );
    }

    #[test]
    fn arena_navigation_lowers_like_rex_backend_rust() {
        let mut f = fields(&[("name", true, "String")]);
        f.insert(
            "book".to_string(),
            FieldBinding::new("self.book".into(), false, "Uuid".into()).with_class("Book"),
        );
        let mut arena = ArenaIndex::new();
        let mut book_fields = HashMap::new();
        book_fields.insert(
            "title".to_string(),
            FieldBinding::new("self.title".into(), true, "String".into()),
        );
        arena.insert(
            "Book".to_string(),
            ClassBinding {
                mirror: "Book".into(),
                getter: "get_book".into(),
                id_type: "Uuid".into(),
                fields: book_fields,
                ops: HashMap::new(),
            },
        );
        let plain = lower_expr_to_rust("book.title", &f, &arena).unwrap();
        assert!(
            plain.contains("res.get_book(self.book)")
                && plain.contains("o.title.clone()")
                && plain.contains("expect(\"dangling `Book` id\")"),
            "{plain}"
        );
    }

    fn books_arena() -> (HashMap<String, FieldBinding>, ArenaIndex) {
        let mut f = fields(&[]);
        f.insert(
            "books".to_string(),
            FieldBinding::new("self.books".into(), true, "Vec<Uuid>".into()).with_class("Book"),
        );
        let mut arena = empty_arena();
        let mut book_fields = fields(&[("title", true, "String"), ("pages", false, "i32")]);
        book_fields.insert(
            "related".to_string(),
            FieldBinding::new("self.related".into(), true, "Option<Uuid>".into())
                .with_class("Book"),
        );
        arena.insert(
            "Book".to_string(),
            ClassBinding {
                mirror: "Book".into(),
                getter: "get_book".into(),
                id_type: "Uuid".into(),
                fields: book_fields,
                ops: HashMap::from([(
                    "active".to_string(),
                    OpBinding {
                        snake: "active".into(),
                        params: Vec::new(),
                    },
                )]),
            },
        );
        (f, arena)
    }

    #[test]
    fn map_lambda_navigates_element_fields_with_one_deref() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.map(b => b.title)", &f, &arena).map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|b| (res.get_book((*b)).map(|o| o.title.clone()).expect(\"dangling `Book` id\"))).collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn map_lambda_calls_element_operations_with_one_deref() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.map(b => b.active())", &f, &arena).map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|b| (res.get_book((*b)).expect(\"dangling `Book` id\").active(res))).collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn any_lambda_navigates_element_fields_with_one_deref() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.any(b => b.pages > 10)", &f, &arena).map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().any(|b| ((res.get_book((*b)).map(|o| o.pages).expect(\"dangling `Book` id\")) > 10i32)))".into())
        );
    }

    #[test]
    fn any_lambda_calls_element_operations_with_one_deref() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.any(b => b.active())", &f, &arena).map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().any(|b| (res.get_book((*b)).expect(\"dangling `Book` id\").active(res))))".into())
        );
    }

    #[test]
    fn first_lambda_navigates_element_fields_with_two_derefs() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.first(b => b.pages > 10)", &f, &arena).map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().find(|b| ((res.get_book((**b)).map(|o| o.pages).expect(\"dangling `Book` id\")) > 10i32)).copied())".into())
        );
    }

    #[test]
    fn first_lambda_calls_element_operations_with_two_derefs() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.first(b => b.active())", &f, &arena).map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().find(|b| (res.get_book((**b)).expect(\"dangling `Book` id\").active(res))).copied())".into())
        );
    }

    #[test]
    fn filter_lambda_navigates_element_fields_with_two_derefs() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.filter(b => b.pages > 10)", &f, &arena).map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().filter(|b| ((res.get_book((**b)).map(|o| o.pages).expect(\"dangling `Book` id\")) > 10i32)).copied().collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn filter_lambda_calls_element_operations_with_two_derefs() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.filter(b => b.active())", &f, &arena).map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().filter(|b| (res.get_book((**b)).expect(\"dangling `Book` id\").active(res))).copied().collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn lambda_param_metadata_shadows_same_named_self_feature() {
        let (mut f, arena) = books_arena();
        f.insert(
            "b".to_string(),
            FieldBinding::new("self.b".into(), false, "Uuid".into()).with_class("Other"),
        );
        assert_eq!(
            lower_expr_to_rust("books.map(b => b.title)", &f, &arena).map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|b| (res.get_book((*b)).map(|o| o.title.clone()).expect(\"dangling `Book` id\"))).collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn lambda_safe_field_navigation_wraps_non_optional_elements() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.map(b => b?.title)", &f, &arena).map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|b| (Some(res.get_book((*b)).map(|o| o.title.clone()).expect(\"dangling `Book` id\")))).collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn lambda_safe_operation_call_wraps_non_optional_elements() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.map(b => b?.active())", &f, &arena).map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|b| (Some(res.get_book((*b)).expect(\"dangling `Book` id\").active(res)))).collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn lambda_safe_field_navigation_maps_optional_elements() {
        let (mut f, arena) = books_arena();
        f.get_mut("books").unwrap().rust_type = "Vec<Option<Uuid>>".into();
        assert_eq!(
            lower_expr_to_rust("books.map(b => b?.title)", &f, &arena).map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|b| ((*b).clone().and_then(|id| res.get_book(id)).map(|o| o.title.clone()))).collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn lambda_safe_operation_call_maps_optional_elements() {
        let (mut f, arena) = books_arena();
        f.get_mut("books").unwrap().rust_type = "Vec<Option<Uuid>>".into();
        assert_eq!(
            lower_expr_to_rust("books.map(b => b?.active())", &f, &arena).map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|b| ((*b).clone().and_then(|id| res.get_book(id)).map(|o| o.active(res)))).collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn nested_lambda_parameters_read_the_innermost_binding() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.map(b => books.first(b => b.active()))", &f, &arena)
                .map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|b| (self.books.clone().iter().find(|b_| (res.get_book((**b_)).expect(\"dangling `Book` id\").active(res))).copied())).collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn untyped_let_shadows_lambda_parameter_metadata() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.map(b => let b = 1; b.title)", &f, &arena)
                .map_err(|e| (e.kind, e.detail)),
            Err((
                "field access".into(),
                "`b.title` — only class-id receivers navigate (arena lookups)".into()
            ))
        );
    }

    #[test]
    fn camel_case_lambda_parameter_uses_its_emitted_identifier() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.map(aBook => aBook.title)", &f, &arena)
                .map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|a_book| (res.get_book((*a_book)).map(|o| o.title.clone()).expect(\"dangling `Book` id\"))).collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn lambda_field_navigation_keeps_the_arena_identifier() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.map(res => res.title)", &f, &arena)
                .map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|res_| (res.get_book((*res_)).map(|o| o.title.clone()).expect(\"dangling `Book` id\"))).collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn lambda_operation_call_keeps_the_arena_identifier() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.any(res => res.active())", &f, &arena)
                .map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().any(|res_| (res.get_book((*res_)).expect(\"dangling `Book` id\").active(res))))".into())
        );
    }

    #[test]
    fn normalized_lambda_parameters_avoid_active_emitted_locals() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.map(aBook => books.filter(a_book => a_book.pages > 10))", &f, &arena)
                .map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|a_book| (self.books.clone().iter().filter(|a_book_| ((res.get_book((**a_book_)).map(|o| o.pages).expect(\"dangling `Book` id\")) > 10i32)).copied().collect::<Vec<_>>())).collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn renamed_arena_parameters_avoid_active_emitted_locals() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.map(res => books.map(res => res.title))", &f, &arena)
                .map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|res_| (self.books.clone().iter().map(|res__| (res.get_book((*res__)).map(|o| o.title.clone()).expect(\"dangling `Book` id\"))).collect::<Vec<_>>())).collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn generated_object_names_avoid_nested_lambda_parameters() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.map(o => books.map(o => o.title))", &f, &arena)
                .map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|o| (self.books.clone().iter().map(|o_| (res.get_book((*o_)).map(|o__| o__.title.clone()).expect(\"dangling `Book` id\"))).collect::<Vec<_>>())).collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn optional_lambda_operation_arguments_keep_the_parameter_binding() {
        let (mut f, mut arena) = books_arena();
        f.get_mut("books").unwrap().rust_type = "Vec<Option<Uuid>>".into();
        arena
            .get_mut("Book")
            .unwrap()
            .ops
            .get_mut("active")
            .unwrap()
            .params = vec!["bool".into()];
        assert_eq!(
            lower_expr_to_rust("books.map(o => o?.active(o == null))", &f, &arena)
                .map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|o| ((*o).clone().and_then(|id| res.get_book(id)).map(|o_| o_.active(res, (*o).clone().is_none())))).collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn nested_collection_sum_refusal_preserves_complete_element_type() {
        let f = fields(&[("books", true, "Vec<Vec<Uuid>>")]);
        let err = lower_expr_to_rust("books.sum()", &f, &empty_arena()).unwrap_err();
        assert_eq!(
            (err.kind.as_str(), err.detail.as_str()),
            (
                "collection algebra",
                "`sum` over non-numeric element type `Vec<Uuid>`"
            )
        );
    }

    #[test]
    fn lambda_safe_field_navigation_keeps_optional_target() {
        let (f, arena) = books_arena();
        assert_eq!(
            lower_expr_to_rust("books.map(b => b?.related)", &f, &arena).map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|b| (res.get_book((*b)).map(|o| o.related.clone()).expect(\"dangling `Book` id\"))).collect::<Vec<_>>())".into())
        );
    }

    #[test]
    fn lambda_vec_elements_keep_nested_collection_navigation() {
        let (mut f, arena) = books_arena();
        f.get_mut("books").unwrap().rust_type = "Vec<Vec<Uuid>>".into();
        assert_eq!(
            lower_expr_to_rust("books.map(bs => bs.map(b => b.title))", &f, &arena).map_err(|e| e.to_string()),
            Ok("(self.books.clone().iter().map(|bs| ((*bs).clone().iter().map(|b| (res.get_book((*b)).map(|o| o.title.clone()).expect(\"dangling `Book` id\"))).collect::<Vec<_>>())).collect::<Vec<_>>())".into())
        );

        f.get_mut("books").unwrap().rust_type = "Vec<Vec<Vec<Uuid>>>".into();
        let err = lower_expr_to_rust("books.map(bs => bs.sum())", &f, &arena).unwrap_err();
        assert_eq!(
            (err.kind.as_str(), err.detail.as_str()),
            (
                "collection algebra",
                "`sum` over non-numeric element type `Vec<Uuid>`"
            )
        );
    }

    #[test]
    fn non_copy_lambda_elements_keep_cloned_result_adapters() {
        let (mut f, mut arena) = books_arena();
        f.get_mut("books").unwrap().rust_type = "Vec<String>".into();
        arena.get_mut("Book").unwrap().id_type = "String".into();
        for (source, expected) in [
            (
                "books.first(b => b.active())",
                "(self.books.clone().iter().find(|b| (res.get_book((**b).clone()).expect(\"dangling `Book` id\").active(res))).cloned())",
            ),
            (
                "books.filter(b => b.active())",
                "(self.books.clone().iter().filter(|b| (res.get_book((**b).clone()).expect(\"dangling `Book` id\").active(res))).cloned().collect::<Vec<_>>())",
            ),
        ] {
            assert_eq!(
                lower_expr_to_rust(source, &f, &arena).map_err(|e| e.to_string()),
                Ok(expected.into()),
                "{source}"
            );
        }
    }

    #[test]
    fn lambda_elements_without_class_metadata_keep_named_refusals() {
        let (mut f, arena) = books_arena();
        f.get_mut("books").unwrap().class = None;
        for (source, kind, detail) in [
            (
                "books.map(b => b.title)",
                "field access",
                "`b.title` — only class-id receivers navigate (arena lookups)",
            ),
            (
                "books.map(b => b?.title)",
                "safe navigation (`?.`)",
                "`b.title` — only class-id receivers navigate (arena lookups)",
            ),
            (
                "books.any(b => b.active())",
                "an operation call",
                "`b.active(..)` — receivers must carry a class id",
            ),
        ] {
            let err = lower_expr_to_rust(source, &f, &arena).unwrap_err();
            assert_eq!((err.kind.as_str(), err.detail.as_str()), (kind, detail));
        }
    }

    #[test]
    fn lambda_safe_navigation_chains_keep_named_refusal() {
        let (f, arena) = books_arena();
        let err = lower_expr_to_rust("books.map(b => b?.related?.title)", &f, &arena).unwrap_err();
        assert_eq!(
            (err.kind.as_str(), err.detail.as_str()),
            (
                "safe navigation (`?.`)",
                "`b?.related.title` — only class-id receivers navigate (arena lookups)"
            )
        );
    }

    #[test]
    fn unknown_names_and_failing_constructs_are_refused() {
        let f = fields(&[("candidate_id", true, "String")]);
        let err = lower_expr_to_rust("ghost", &f, &empty_arena()).unwrap_err();
        assert_eq!(err.kind, "unknown symbol");
        for source in [
            "null",
            "candidateId.upper",
            "a ?: b",
            "[]",
            "candidateId.map(x => x)",
        ] {
            assert!(
                lower_expr_to_rust(source, &f, &empty_arena()).is_err(),
                "{source}"
            );
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
