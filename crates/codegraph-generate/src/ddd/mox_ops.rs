//! `mox_ops` — mox operation codegen (issue #501 slice 2, B5 of #218).
//!
//! Per domain, emits `src/domain/<domain>/mox_ops.rs`: one inherent impl
//! per mox class carrying operations, on the entity's SeaORM `Model` type,
//! with the rexlang rex-backend-rust FIXED signature convention:
//!
//! ```text
//! pub fn <snake_name>(&self, res: &Resource, <param>: <type>, ...) -> <Ret>
//! ```
//!
//! | mox body                    | Rust emission |
//! |-----------------------------|---------------|
//! | `rust { ... }`              | body text embedded VERBATIM (byte-identical, on its own line between the braces — the rex-backend-rust contract) |
//! | `expr { ... }`              | lowered through [`crate::mox_expr`] (own-class feature refs → `self.<field>`, literals, comparisons, boolean/arithmetic); constructs outside the subset carry `// TODO(#501)` markers + a `todo!` stub — the module is the record, never a guess |
//! | (no body)                   | slice 3: signature-complete `todo!` stub naming the abstract hook |
//!
//! Signature types map per the rex contract: `int` → `i32`, `string` →
//! `String`, `boolean` → `bool`, `number` → `f64`; anything else (class
//! refs, codelists) stays verbatim — never a guessed path.
//!
//! The `res: &Resource` slot is faithful to the rexlang contract; codegraph
//! has no resource bag, so the module carries a documented placeholder
//! struct and bodies referencing `res.` fail to compile until hand-wired —
//! that IS the seam.
//!
//! Presence-gated: with no mox operations in the graph the generator emits
//! nothing and output stays byte-identical (the standing contract).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::MoxOperationNode;
use codegraph_naming::{escape_rust_keyword, strip_suffix, to_snake_case};

use crate::ProjectConfig;
use crate::code_writer::{CodeWriter, wln};
use crate::error::Result;
use crate::mox_expr::{
    FieldBinding, is_copy_rust_type, lower_expr_to_rust, mox_primitive_rust_type,
};
use crate::traits::{DomainGenerator, DomainGeneratorKind, GeneratedFile};
use codegraph_config::DomainConfig;

/// The Rust type of an operation signature slot (param or return), per the
/// rex contract. Non-primitives stay verbatim (documented caveat — class
/// refs resolve in the consumer crate).
fn sig_type(type_ref: &str) -> String {
    mox_primitive_rust_type(type_ref)
        .map(str::to_string)
        .unwrap_or_else(|| type_ref.to_string())
}

/// One resolved class surface: the entity model alias, its module path, and
/// its name-ordered operations.
struct ClassOps {
    /// The schema title the mox class resolved to.
    title: String,
    /// The entity's Rust type name (impl target alias).
    entity_name: String,
    /// The entity module (`src/entity/{module}.rs`).
    module: String,
    /// Operations in name order (the graph read's order).
    ops: Vec<MoxOperationNode>,
    /// Field bindings for `expr` lowering: snake field name → receiver.
    fields: HashMap<String, FieldBinding>,
}

pub struct MoxOpsGenerator {
    output_dir: PathBuf,
}

impl MoxOpsGenerator {
    pub fn new(output_dir: &Path) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
        }
    }
}

#[async_trait]
impl DomainGenerator for MoxOpsGenerator {
    fn kind(&self) -> DomainGeneratorKind {
        DomainGeneratorKind::MoxOps
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        domain: &str,
        entity_titles: &[String],
        config: &DomainConfig,
        _tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        let ops = db.get_mox_operations().await?;
        if ops.is_empty() {
            return Ok(Vec::new());
        }
        let suffix = &config.defaults.type_suffix;

        // Group ops by the entity title their class resolves to (exact
        // title, or suffix-stripped entity name — the dto.rs matching
        // convention). Ops for classes outside this domain's entity set
        // belong to another domain's file (or stay unclaimed).
        let mut per_class: HashMap<String, Vec<MoxOperationNode>> = HashMap::new();
        for op in ops {
            let resolved = entity_titles
                .iter()
                .find(|t| t.as_str() == op.class || strip_suffix(t, suffix) == op.class);
            if let Some(title) = resolved {
                per_class.entry(title.clone()).or_default().push(op);
            }
        }
        if per_class.is_empty() {
            return Ok(Vec::new());
        }

        let mut surfaces = Vec::with_capacity(per_class.len());
        for (title, mut class_ops) in per_class {
            class_ops.sort_by(|a, b| a.name.cmp(&b.name));
            let schema = match db.get_schema_in_domain(&title, domain).await {
                Ok(Some(schema)) => schema,
                _ => continue,
            };
            let module = escape_rust_keyword(&to_snake_case(&strip_suffix(&title, suffix)));
            let fields = db
                .get_properties(&title)
                .await
                .unwrap_or_default()
                .iter()
                .map(|p| {
                    (
                        p.rust_field_name.clone(),
                        FieldBinding {
                            recv: format!("self.{}", p.rust_field_name),
                            clone: !is_copy_rust_type(&p.rust_field_type),
                        },
                    )
                })
                .collect();
            surfaces.push(ClassOps {
                entity_name: escape_rust_keyword(&schema.rust_type_name),
                module,
                fields,
                title,
                ops: class_ops,
            });
        }
        if surfaces.is_empty() {
            return Ok(Vec::new());
        }
        surfaces.sort_by(|a, b| a.title.cmp(&b.title));

        let content = emit_domain_mox_ops(domain, &surfaces, project);
        Ok(vec![GeneratedFile {
            path: self
                .output_dir
                .join("src")
                .join("domain")
                .join(domain)
                .join("mox_ops.rs"),
            content,
        }])
    }
}

/// Emit the complete `mox_ops.rs` for one domain (deterministic: surfaces
/// title-ordered, ops name-ordered).
fn emit_domain_mox_ops(domain: &str, surfaces: &[ClassOps], project: &ProjectConfig) -> String {
    let mut code = CodeWriter::new();
    wln!(
        code,
        "//! Mox operations for the {domain} domain (issue #501).",
        domain = domain,
    );
    wln!(code, "//!");
    wln!(
        code,
        "//! Generated by {} — DO NOT EDIT. One inherent impl per mox class",
        project.identity.generator_name
    );
    wln!(
        code,
        "//! carrying operations, on the entity's SeaORM `Model` type, with the"
    );
    wln!(
        code,
        "//! rexlang rex-backend-rust fixed signature (`&self, res: &Resource,`)."
    );
    wln!(
        code,
        "//! `rust` bodies are embedded verbatim; `expr` bodies lower through"
    );
    wln!(
        code,
        "//! the supported subset or carry TODO(#501) markers — the module is"
    );
    wln!(code, "//! the record, never a guess.");
    wln!(code, "#![allow(dead_code)]");

    for surface in surfaces {
        emit_class_impl(&mut code, surface);
    }

    code.into_string()
}

/// Emit one class: the entity alias, the Resource placeholder (once per
/// module, from the first class), and one method per operation.
fn emit_class_impl(code: &mut CodeWriter, surface: &ClassOps) {
    wln!(code);
    wln!(
        code,
        "use crate::entity::{module}::Model as {entity};",
        module = surface.module,
        entity = surface.entity_name,
    );
    wln!(code);
    wln!(
        code,
        "/// Placeholder for the rexlang `res: &Resource` slot of the fixed",
    );
    wln!(
        code,
        "/// operation signature (issue #501) — codegraph's generated app has",
    );
    wln!(
        code,
        "/// no resource bag; bodies referencing `res.` fail to compile until",
    );
    wln!(code, "/// wired by hand.");
    wln!(code, "pub struct Resource;");
    wln!(code);
    wln!(code, "impl {entity} {{", entity = surface.entity_name);
    for op in &surface.ops {
        emit_operation(code, surface, op);
    }
    wln!(code, "}}");
}

/// The fixed signature: `pub fn <snake>(&self, res: &Resource<, params>) -> <Ret>`.
fn signature(op: &MoxOperationNode) -> String {
    let name = escape_rust_keyword(&to_snake_case(&op.name));
    let params = op
        .params
        .iter()
        .map(|p| {
            format!(
                "{}: {}",
                escape_rust_keyword(&to_snake_case(&p.name)),
                sig_type(&p.type_)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "pub fn {name}(&self, res: &Resource{sep}{params}) -> {ret}",
        sep = if op.params.is_empty() { "" } else { ", " },
        ret = sig_type(&op.return_type),
    )
}

/// Emit one operation method (verbatim rust body, lowered expr body, or —
/// for a body-less op — the slice-3 signature-complete stub).
fn emit_operation(code: &mut CodeWriter, surface: &ClassOps, op: &MoxOperationNode) {
    wln!(code);
    if let Some(description) = &op.description {
        wln!(code, "    /// {description}");
    }
    wln!(
        code,
        "    /// Declared operation `{name}` on mox class `{class}`.",
        name = op.name,
        class = op.class,
    );
    wln!(code, "    {} {{", signature(op));

    if let Some(body) = op.bodies.get("rust") {
        // Verbatim embedding (the rex-backend-rust Tier 1 contract): the
        // body text byte-identical, on its own line between the braces.
        code.push_line(body);
    } else if let Some(expr) = op.bodies.get("expr") {
        match lower_expr_to_rust(expr, &surface.fields) {
            Ok(lowered) => wln!(code, "        {lowered}"),
            Err(e) => {
                wln!(
                    code,
                    "        // TODO(#501): transpile op '{}' (unsupported: {kind}: {detail})",
                    op.name,
                    kind = e.kind,
                    detail = e.detail,
                );
                wln!(
                    code,
                    "        todo!(\"op '{}': expr body did not lower\")",
                    op.name
                );
            }
        }
    } else {
        // Body-less op: the rexlang "abstract hook" gap class. Emit the
        // seam — signature-complete, compiling, never a guessed body.
        wln!(
            code,
            "    todo!(\"body-less mox op '{}' — implement by hand (rexlang gap class)\")",
            op.name
        );
    }

    wln!(code, "    }}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn signature_maps_primitives_and_keeps_order() {
        let op = MoxOperationNode {
            name: "getBook".to_string(),
            class: "Library".to_string(),
            package: "lib".to_string(),
            description: None,
            return_type: "int".to_string(),
            params: vec![
                codegraph_core::types::MoxParam {
                    name: "title".to_string(),
                    type_: "string".to_string(),
                },
                codegraph_core::types::MoxParam {
                    name: "shelf".to_string(),
                    type_: "Shelf".to_string(),
                },
            ],
            bodies: BTreeMap::new(),
        };
        assert_eq!(
            signature(&op),
            "pub fn get_book(&self, res: &Resource, title: String, shelf: Shelf) -> i32"
        );
    }

    #[test]
    fn signature_without_params_has_no_trailing_separator() {
        let op = MoxOperationNode {
            name: "hasShortId".to_string(),
            class: "CandidateType".to_string(),
            package: "recruiting".to_string(),
            description: None,
            return_type: "boolean".to_string(),
            params: vec![],
            bodies: BTreeMap::new(),
        };
        assert_eq!(
            signature(&op),
            "pub fn has_short_id(&self, res: &Resource) -> bool"
        );
    }
}
