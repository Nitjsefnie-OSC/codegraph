//! `mox_ops` — mox operation codegen (issue #501, quality bar #508).
//!
//! Per domain, emits two files:
//!
//! 1. `{domain_types_base}/src/{domain}/mox_types.rs` — the persistence-
//!    agnostic surface (issue #508 R1/R4): one row-mirror struct per mox
//!    class carrying operations or derived features, plus the `Resource`
//!    contract of the fixed signature — request context (org /
//!    correlation / api key) and a **synchronous arena** of loaded
//!    mirrors (`res.get_<class>(id)`), which is what lowered `x.field`
//!    navigation resolves through. Not the SeaORM `Model`: the same
//!    module compiles under sea_orm AND workers-cornucopia, on native
//!    and wasm32 targets.
//! 2. `src/domain/<domain>/mox_ops.rs` — one inherent impl per class on
//!    its mirror, with the rexlang rex-backend-rust FIXED signature:
//!
//! ```text
//! pub fn <snake_name>(&self, res: &Resource, <param>: <type>, ...) -> <Ret>
//! ```
//!
//! | mox body  | Rust emission |
//! |-----------|---------------|
//! | `rust { … }` | body text embedded VERBATIM (byte-identical — the rex-backend-rust Tier 1 contract) |
//! | `expr { … }` | lowered through [`crate::mox_expr`] (the normative subset; class navigation via the `res` arena) |
//! | (no body) | signature-complete `todo!` seam naming the abstract hook (the documented rexlang gap class — body-less ops get no route) |
//!
//! **Refused `expr` bodies are generation errors** (issue #508 R3 — the
//! `expr_sql`/`expr_ts` precedent): the run reports the op/feature and
//! fails instead of emitting a `todo!` that panics at runtime.
//!
//! Signature types map per the rex contract: `int` → `i32`, `string` →
//! `String`, `boolean` → `bool`, `number` → `f64`; class refs map to
//! `Uuid` (the `{Class}Id` slot); anything else stays verbatim — never a
//! guessed path.
//!
//! Presence-gated: with no mox operations or derived features in the
//! graph the generator emits nothing and output stays byte-identical.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::{MoxDerivedFeatureNode, MoxOperationNode, PropertyNode};
use codegraph_naming::{escape_rust_keyword, strip_suffix, to_pascal_case, to_snake_case};

use crate::ProjectConfig;
use crate::code_writer::{CodeWriter, wln};
use crate::error::{Error, Result};
use crate::mox_expr::{
    ArenaIndex, ClassBinding, FieldBinding, OpBinding, is_copy_rust_type, lower_expr_to_rust,
    mox_primitive_rust_type,
};
use crate::traits::{DomainGenerator, DomainGeneratorKind, GeneratedFile};
use codegraph_config::DomainConfig;

/// The Rust type of an operation signature slot (param or return), per the
/// rex contract. Class refs (known to the domain) map to their id type
/// (`Uuid` — the `{Class}Id` slot); everything else stays verbatim.
fn sig_type(type_ref: &str, known_classes: &HashSet<String>) -> String {
    if known_classes.contains(type_ref) || known_classes.contains(&to_pascal_case(type_ref)) {
        return "Uuid".to_string();
    }
    mox_primitive_rust_type(type_ref)
        .map(str::to_string)
        .unwrap_or_else(|| type_ref.to_string())
}

/// One mirror class: schema title, Rust names, stored fields, ops, and
/// whether it carries derived features (which need the mirror too).
struct ClassSurface {
    /// The schema title the mox class resolved to.
    title: String,
    /// The mirror struct name (`Candidate`).
    entity_name: String,
    /// The mirror module key (`candidate`).
    module: String,
    /// Operations in name order.
    ops: Vec<MoxOperationNode>,
    /// Mirror fields: (rust field name, rust type).
    fields: Vec<(String, String)>,
    /// Field bindings for `expr` lowering.
    bindings: HashMap<String, FieldBinding>,
}

pub struct MoxOpsGenerator {
    output_dir: PathBuf,
    /// Resolved domain-types crate root — `mox_types.rs` (mirrors + the
    /// real `Resource`) lives there so the domain-types response accessors
    /// can name the same arena type.
    domain_types_dir: PathBuf,
}

impl MoxOpsGenerator {
    pub fn new(output_dir: &Path, domain_types_dir: PathBuf) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
            domain_types_dir,
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
        let all_ops = db.get_mox_operations().await?;
        let all_derived = db.get_mox_derived_features().await?;
        if all_ops.is_empty() && all_derived.is_empty() {
            return Ok(Vec::new());
        }
        let suffix = &config.defaults.type_suffix;

        // Group ops/derived by the entity title their class resolves to
        // (exact title, or suffix-stripped entity name — the dto.rs
        // matching convention).
        let mut per_class_ops: HashMap<String, Vec<MoxOperationNode>> = HashMap::new();
        for op in all_ops.iter().cloned() {
            let resolved = entity_titles
                .iter()
                .find(|t| t.as_str() == op.class || strip_suffix(t, suffix) == op.class);
            if let Some(title) = resolved {
                per_class_ops.entry(title.clone()).or_default().push(op);
            }
        }
        let mut per_class_derived: HashMap<String, Vec<MoxDerivedFeatureNode>> = HashMap::new();
        for feat in all_derived {
            let resolved = entity_titles
                .iter()
                .find(|t| t.as_str() == feat.class || strip_suffix(t, suffix) == feat.class);
            if let Some(title) = resolved {
                per_class_derived
                    .entry(title.clone())
                    .or_default()
                    .push(feat);
            }
        }

        // Every class with ops OR derived features gets a mirror.
        let mut titles: Vec<String> = per_class_ops
            .keys()
            .chain(per_class_derived.keys())
            .cloned()
            .collect();
        titles.sort();
        titles.dedup();
        if titles.is_empty() {
            return Ok(Vec::new());
        }

        // Known classes of the domain (signature class-ref mapping and the
        // arena index keys).
        let known_classes: HashSet<String> = {
            let mut set = HashSet::new();
            for title in entity_titles {
                if let Ok(Some(schema)) = db.get_schema_in_domain(title, domain).await {
                    set.insert(escape_rust_keyword(&schema.rust_type_name));
                }
            }
            set
        };

        // Build the arena index (all domain entities — bodies may
        // navigate any of them) and the class surfaces. Mirror names are
        // the graph's canonical `rust_type_name` (acronym-safe).
        let mut arena: ArenaIndex = HashMap::new();
        let mut entity_props: HashMap<String, Vec<PropertyNode>> = HashMap::new();
        let mut title_to_mirror: HashMap<String, String> = HashMap::new();
        for title in entity_titles {
            let props = db.get_properties(title).await.unwrap_or_default();
            entity_props.insert(title.clone(), props);
            if let Ok(Some(schema)) = db.get_schema_in_domain(title, domain).await {
                title_to_mirror.insert(title.clone(), escape_rust_keyword(&schema.rust_type_name));
            }
        }
        for title in entity_titles {
            let Some(props) = entity_props.get(title) else {
                continue;
            };
            let Some(mirror) = title_to_mirror.get(title).cloned() else {
                continue;
            };
            let getter = format!("get_{}", to_snake_case(&strip_suffix(title, suffix)));
            let fields = props
                .iter()
                .map(|p| {
                    let binding = binding_for(p, &title_to_mirror, suffix);
                    (p.rust_field_name.clone(), binding)
                })
                .collect::<HashMap<String, FieldBinding>>();
            let ops = ops_bindings_for(title, suffix, &all_ops);
            arena.insert(
                mirror.clone(),
                ClassBinding {
                    mirror,
                    getter,
                    id_type: "Uuid".to_string(),
                    fields,
                    ops,
                },
            );
        }

        let mut surfaces = Vec::with_capacity(titles.len());
        for title in &titles {
            let Some(entity_name) = title_to_mirror.get(title).cloned() else {
                continue;
            };
            let props = entity_props.get(title).cloned().unwrap_or_default();
            let mut class_ops = per_class_ops.remove(title).unwrap_or_default();
            class_ops.sort_by(|a, b| a.name.cmp(&b.name));
            per_class_derived.remove(title);

            let mut fields: Vec<(String, String)> = Vec::new();
            let mut bindings = HashMap::new();
            let mut has_id = false;
            for p in &props {
                if p.rust_field_name == "id" {
                    has_id = true;
                }
                fields.push((p.rust_field_name.clone(), p.rust_field_type.clone()));
                bindings.insert(
                    p.rust_field_name.clone(),
                    binding_for(p, &title_to_mirror, suffix),
                );
            }
            if !has_id {
                fields.insert(0, ("id".to_string(), "Uuid".to_string()));
            }
            surfaces.push(ClassSurface {
                entity_name,
                module: escape_rust_keyword(&to_snake_case(&strip_suffix(title, suffix))),
                fields,
                bindings,
                title: title.clone(),
                ops: class_ops,
            });
        }
        if surfaces.is_empty() {
            return Ok(Vec::new());
        }

        // Class-id aware signature types (known_classes).
        let mox_types = emit_mox_types(domain, &surfaces, project);
        let mox_ops = emit_domain_mox_ops(domain, &surfaces, &arena, &known_classes, project)?;

        let mut files = vec![GeneratedFile {
            path: self
                .domain_types_dir
                .join("src")
                .join(domain)
                .join("mox_types.rs"),
            content: mox_types,
        }];
        files.push(GeneratedFile {
            path: self
                .output_dir
                .join("src")
                .join("domain")
                .join(domain)
                .join("mox_ops.rs"),
            content: mox_ops,
        });
        Ok(files)
    }
}

/// Build one field binding (class metadata for arena navigation).
fn binding_for(
    p: &PropertyNode,
    title_to_mirror: &HashMap<String, String>,
    suffix: &str,
) -> FieldBinding {
    let mut binding = FieldBinding::new(
        format!("self.{}", p.rust_field_name),
        !is_copy_rust_type(&p.rust_field_type),
        p.rust_field_type.clone(),
    );
    if let Some(target) = &p.ref_target {
        // Resolve the target title to a domain mirror name.
        for (title, mirror) in title_to_mirror {
            if title == target || strip_suffix(title, suffix) == *target {
                binding.class = Some(mirror.clone());
                break;
            }
        }
    }
    binding
}

/// Arena op bindings for one class (written + snake keys).
fn ops_bindings_for(
    title: &str,
    suffix: &str,
    all: &[MoxOperationNode],
) -> HashMap<String, OpBinding> {
    let mut out = HashMap::new();
    for op in all
        .iter()
        .filter(|o| o.class == title || strip_suffix(title, suffix) == o.class)
    {
        let binding = OpBinding {
            snake: to_snake_case(&op.name),
            params: op
                .params
                .iter()
                .map(|p| {
                    mox_primitive_rust_type(&p.type_)
                        .map(str::to_string)
                        .unwrap_or_else(|| p.type_.clone())
                })
                .collect(),
        };
        out.insert(op.name.clone(), binding.clone());
        out.insert(binding.snake.clone(), binding);
    }
    out
}

/// Emit `{domain}/mox_types.rs`: mirrors + the real `Resource` contract.
fn emit_mox_types(domain: &str, surfaces: &[ClassSurface], project: &ProjectConfig) -> String {
    let mut code = CodeWriter::new();
    wln!(
        code,
        "//! Mox type surface for the {domain} domain (issue #508 R1/R4).",
        domain = domain
    );
    wln!(code, "//!");
    wln!(
        code,
        "//! Generated by {} — DO NOT EDIT. Persistence-agnostic row",
        project.identity.generator_name
    );
    wln!(
        code,
        "//! mirrors plus the `Resource` contract of the fixed mox operation"
    );
    wln!(
        code,
        "//! signature: request context and a synchronous arena of loaded"
    );
    wln!(
        code,
        "//! mirrors — `res.get_<class>(id)` is what lowered navigation"
    );
    wln!(
        code,
        "//! resolves through. Compiles on every provider and target."
    );
    wln!(code);
    wln!(code, "use std::collections::HashMap;");
    wln!(code, "use uuid::Uuid;");
    wln!(code);

    for surface in surfaces {
        wln!(code);
        wln!(
            code,
            "/// Row mirror for mox class `{title}` — the persistence-agnostic",
            title = surface.title
        );
        wln!(
            code,
            "/// impl target of `{name}` operations (issue #508 R1).",
            name = surface.entity_name
        );
        wln!(code, "#[derive(Debug, Clone)]");
        wln!(code, "pub struct {name} {{", name = surface.entity_name);
        for (fname, ftype) in &surface.fields {
            wln!(code, "    pub {fname}: {ftype},");
        }
        wln!(code, "}}");
    }

    wln!(code);
    wln!(
        code,
        "/// The `res: &Resource` slot of the fixed operation signature"
    );
    wln!(
        code,
        "/// (issue #508 R4): a REAL contract — request context plus a"
    );
    wln!(
        code,
        "/// synchronous arena. The runner inserts the target row before"
    );
    wln!(
        code,
        "/// invoking an operation; lookups of unloaded rows return `None`"
    );
    wln!(
        code,
        "/// (plain navigation then panics with `dangling`, `?.` yields"
    );
    wln!(code, "/// `None`) — the rex-backend-rust contract.");
    wln!(code, "pub struct Resource {{");
    wln!(code, "    pub organization_id: Uuid,");
    wln!(code, "    pub correlation_id: Uuid,");
    wln!(code, "    pub api_key_id: Option<Uuid>,");
    for surface in surfaces {
        wln!(
            code,
            "    {module}: HashMap<Uuid, {name}>,",
            module = surface.module,
            name = surface.entity_name
        );
    }
    wln!(code, "}}");
    wln!(code);
    wln!(code, "impl Resource {{");
    wln!(
        code,
        "    pub fn new(organization_id: Uuid, correlation_id: Uuid, api_key_id: Option<Uuid>) -> Self {{"
    );
    wln!(code, "        Self {{");
    wln!(code, "            organization_id,");
    wln!(code, "            correlation_id,");
    wln!(code, "            api_key_id,");
    for surface in surfaces {
        wln!(
            code,
            "            {module}: HashMap::new(),",
            module = surface.module
        );
    }
    wln!(code, "        }}");
    wln!(code, "    }}");
    for surface in surfaces {
        wln!(code);
        wln!(
            code,
            "    /// Insert a loaded `{name}` row into the arena.",
            name = surface.entity_name
        );
        wln!(
            code,
            "    pub fn insert_{module}(&mut self, value: {name}) {{",
            module = surface.module,
            name = surface.entity_name
        );
        wln!(
            code,
            "        self.{module}.insert(value.id, value);",
            module = surface.module
        );
        wln!(code, "    }}");
        wln!(code);
        wln!(
            code,
            "    /// Arena lookup (lowered `x.field` navigation resolves here)."
        );
        wln!(
            code,
            "    pub fn get_{module}(&self, id: Uuid) -> Option<&{name}> {{",
            module = surface.module,
            name = surface.entity_name
        );
        wln!(
            code,
            "        self.{module}.get(&id)",
            module = surface.module
        );
        wln!(code, "    }}");
    }
    wln!(code, "}}");
    code.into_string()
}

/// Emit the complete `mox_ops.rs` for one domain (deterministic: surfaces
/// title-ordered, ops name-ordered). `Err` on any refused `expr` body —
/// issue #508 R3 (generation error, never an emitted `todo!`).
fn emit_domain_mox_ops(
    domain: &str,
    surfaces: &[ClassSurface],
    arena: &ArenaIndex,
    known_classes: &HashSet<String>,
    project: &ProjectConfig,
) -> Result<String> {
    let mut code = CodeWriter::new();
    wln!(
        code,
        "//! Mox operations for the {domain} domain (issue #508).",
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
        "//! on its persistence-agnostic row mirror, with the rexlang"
    );
    wln!(
        code,
        "//! rex-backend-rust fixed signature (`&self, res: &Resource,`)."
    );
    wln!(
        code,
        "//! `rust` bodies are embedded verbatim; `expr` bodies lower through"
    );
    wln!(
        code,
        "//! the normative subset (a refused body is a generation error, never"
    );
    wln!(code, "//! an emitted `todo!`).");
    wln!(code, "#![allow(dead_code)]");
    wln!(code);
    wln!(
        code,
        "use {tt}::{domain}::mox_types::{{{names}, Resource}};",
        tt = project.identity.domain_types_crate,
        domain = domain,
        names = surfaces
            .iter()
            .map(|s| s.entity_name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    );

    let mut failures: Vec<String> = Vec::new();
    for surface in surfaces {
        wln!(code);
        wln!(code, "impl {entity} {{", entity = surface.entity_name);
        for op in &surface.ops {
            emit_operation(&mut code, surface, op, arena, known_classes, &mut failures);
        }
        wln!(code, "}}");
    }

    if !failures.is_empty() {
        return Err(Error::Validation(format!(
            "mox expr bodies outside the lowered subset (issue #508 R3):\n  - {}",
            failures.join("\n  - ")
        )));
    }
    Ok(code.into_string())
}

/// The fixed signature: `pub fn <snake>(&self, res: &Resource<, params>) -> <Ret>`.
fn signature(op: &MoxOperationNode, known_classes: &HashSet<String>) -> String {
    let name = escape_rust_keyword(&to_snake_case(&op.name));
    let params = op
        .params
        .iter()
        .map(|p| {
            format!(
                "{}: {}",
                escape_rust_keyword(&to_snake_case(&p.name)),
                sig_type(&p.type_, known_classes)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "pub fn {name}(&self, res: &Resource{sep}{params}) -> {ret}",
        sep = if op.params.is_empty() { "" } else { ", " },
        ret = sig_type(&op.return_type, known_classes),
    )
}

/// Emit one operation method (verbatim rust body, lowered expr body, or —
/// for a body-less op — the signature-complete seam).
fn emit_operation(
    code: &mut CodeWriter,
    surface: &ClassSurface,
    op: &MoxOperationNode,
    arena: &ArenaIndex,
    known_classes: &HashSet<String>,
    failures: &mut Vec<String>,
) {
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
    wln!(code, "    {} {{", signature(op, known_classes));

    if let Some(body) = op.bodies.get("rust") {
        // Verbatim embedding (the rex-backend-rust Tier 1 contract): the
        // body text byte-identical, on its own line between the braces.
        code.push_line(body);
    } else if let Some(expr) = op.bodies.get("expr") {
        match lower_expr_to_rust(expr, &surface.bindings, arena) {
            Ok(lowered) => wln!(code, "        {lowered}"),
            Err(e) => failures.push(format!(
                "op `{}` on `{}`: {} ({})",
                op.name, surface.title, e.detail, e.kind
            )),
        }
    } else {
        // Body-less op: the rexlang "abstract hook" gap class. Emit the
        // seam — signature-complete, compiling, never a guessed body.
        // Body-less ops get no route (issue #508 R2).
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

    fn empty_classes() -> HashSet<String> {
        HashSet::new()
    }

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
            signature(&op, &empty_classes()),
            "pub fn get_book(&self, res: &Resource, title: String, shelf: Shelf) -> i32"
        );
    }

    #[test]
    fn signature_maps_known_class_refs_to_the_id_slot() {
        let mut classes = HashSet::new();
        classes.insert("Shelf".to_string());
        let op = MoxOperationNode {
            name: "getBook".to_string(),
            class: "Library".to_string(),
            package: "lib".to_string(),
            description: None,
            return_type: "Shelf".to_string(),
            params: vec![codegraph_core::types::MoxParam {
                name: "shelf".to_string(),
                type_: "Shelf".to_string(),
            }],
            bodies: BTreeMap::new(),
        };
        assert_eq!(
            signature(&op, &classes),
            "pub fn get_book(&self, res: &Resource, shelf: Uuid) -> Uuid"
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
            signature(&op, &empty_classes()),
            "pub fn has_short_id(&self, res: &Resource) -> bool"
        );
    }
}
