//! `mox_ops` — mox operation codegen (issue #501, quality bar #508,
//! compile contract #517).
//!
//! Per domain, emits ONE file —
//! `{domain_types_base}/src/{domain}/mox_types.rs` — carrying:
//!
//! 1. **Row mirrors** (issue #508 R1) — one per mox class with operations
//!    or derived features. Field names/types/wrappers are **exactly** the
//!    entity's `{Entity}Response` DTO surface (issue #517: the DTO layer's
//!    kind-driven resolution — entity refs → `uuid::Uuid` with `_id`
//!    names, codelists → generated enums, structured wrappers →
//!    `types_import_prefix`, VO children → same-file child-struct types),
//!    with the same import conventions the DTO modules use. Never bare
//!    names — every emitted type resolves within the generated tree.
//! 2. **`from_response`** — the Response → mirror copy the runner wiring
//!    (issue #508 R2) uses; the handler/query copy lists collapsed into
//!    this one constructor.
//! 3. **The `Resource` arena** (issue #508 R4) — request context plus a
//!    synchronous arena (`res.get_<class>(id)` / `insert_<class>`), the
//!    rex-backend-rust navigation contract (plain navigation
//!    `expect("dangling …")`, `?.` yields `None`).
//! 4. **Operation impls** on the mirrors, with the rex-backend-rust FIXED
//!    signature `pub fn <snake>(&self, res: &Resource, params) -> Ret`.
//!    The impls live WITH the type (same file — the mirrors and ops are
//!    one model surface, mirroring rex-backend-rust's layout); the
//!    app-side `mox_ops.rs` is retired, so the plane is reachable from
//!    every band through the domain-types dependency alone.
//!
//! Signature types map per the rex contract (issue #517 — every slot
//! RESOLVES): `int` → `i32`, `string` → `String`, `boolean` → `bool`,
//! `number` → `f64`; known class refs → `uuid::Uuid` (the `{Class}Id`
//! slot); codelist/enum refs → `{domain_types}::codelist::{Enum}`;
//! **anything else is a generation error naming the op** — never a bare
//! name. `rust` bodies embed VERBATIM (byte-identical — the Tier 1
//! contract); `expr` bodies lower through [`crate::mox_expr`]'s normative
//! subset (issue #508 R3 — refused bodies are generation errors too);
//! body-less ops emit the signature-complete `todo!` seam (and never
//! route).
//!
//! Presence-gated: with no mox operations or derived features the
//! generator emits nothing and output stays byte-identical.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::{MoxDerivedFeatureNode, MoxOperationNode, PropertyNode};
use codegraph_naming::{escape_rust_keyword, strip_suffix, to_pascal_case, to_snake_case};

use crate::ProjectConfig;
use crate::code_writer::{CodeWriter, wln};
use crate::ddd::dto::{ChildDtoContext, DtoField, build_dto_context};
use crate::error::{Error, Result};
use crate::mox_expr::{
    ArenaIndex, ClassBinding, FieldBinding, OpBinding, is_copy_rust_type, lower_expr_to_rust,
    mox_primitive_rust_type,
};
use crate::traits::{DomainGenerator, DomainGeneratorKind, GeneratedFile};
use codegraph_config::DomainConfig;

/// The exact type spelling a DTO field carries on the response struct
/// (issue #517 — the template's shape rules, in one place).
fn wrap_shape(inner: &str, is_array: bool, is_required: bool) -> String {
    let base = if is_array {
        format!("Vec<{inner}>")
    } else {
        inner.to_string()
    };
    if is_required {
        base
    } else {
        format!("Option<{base}>")
    }
}

fn mirror_field_type(f: &DtoField) -> String {
    if f.is_entity_ref {
        wrap_shape("uuid::Uuid", f.is_array, f.is_required)
    } else {
        wrap_shape(&f.rust_type, f.is_array, f.is_required)
    }
}

/// Child-struct fields on the response: arrays are bare `Vec<..>`,
/// singletons are `Option<..>` (the dto_response.tera shape).
fn child_field_type(child: &ChildDtoContext) -> String {
    if child.is_array {
        format!("Vec<{}Response>", child.struct_name)
    } else {
        format!("Option<{}Response>", child.struct_name)
    }
}

/// Resolve one signature slot through wrappers (`Vec<..>`, `Option<..>`).
fn resolve_slot(
    type_ref: &str,
    resolve: &dyn Fn(&str) -> std::result::Result<String, String>,
) -> std::result::Result<String, String> {
    for open in ["Vec<", "Option<"] {
        if let Some(inner) = type_ref
            .strip_prefix(open)
            .and_then(|s| s.strip_suffix('>'))
        {
            let resolved = resolve_slot(inner, resolve)?;
            return Ok(format!("{open}{resolved}>"));
        }
    }
    resolve(type_ref)
}

/// The Rust type of an operation signature slot (param or return), per the
/// rex contract — issue #517: every slot RESOLVES. mox primitives map
/// canonically; known class refs map to their id slot (`uuid::Uuid`); known
/// codelists map to the generated codelist enum; anything else is refused
/// (the caller raises a generation error naming the op — never a bare
/// name).
fn sig_type(
    type_ref: &str,
    known_classes: &HashSet<String>,
    codelists: &HashSet<String>,
    domain_types_crate: &str,
    suffix: &str,
) -> std::result::Result<String, String> {
    resolve_slot(type_ref, &|inner| {
        if let Some(t) = mox_primitive_rust_type(inner) {
            return Ok(t.to_string());
        }
        // Class/enum slots may be written package-qualified
        // (`pkg::ShelfType`) or carry the type suffix — normalize to the
        // short name before matching.
        let short = inner.rsplit("::").next().unwrap_or(inner);
        let short = strip_suffix(short, suffix);
        if known_classes.contains(short.as_str()) || known_classes.contains(&to_pascal_case(&short))
        {
            return Ok("uuid::Uuid".to_string());
        }
        if codelists.contains(short.as_str()) || codelists.contains(&to_pascal_case(&short)) {
            return Ok(format!("{domain_types_crate}::codelist::{short}"));
        }
        Err(type_ref.to_string())
    })
}

/// Signature-resolution context (issue #517): the sets a signature slot
/// resolves against.
struct SigCtx<'a> {
    known_classes: &'a HashSet<String>,
    codelists: &'a HashSet<String>,
    suffix: &'a str,
    project: &'a ProjectConfig,
}

/// One mirror field: name + the exact response-shaped type spelling.
struct MirrorField {
    name: String,
    rust_type: String,
}

/// One mirror class: schema title, Rust names, DTO-shaped fields, ops.
struct ClassSurface {
    /// The schema title the mox class resolved to.
    title: String,
    /// The mirror struct name (`Candidate`).
    entity_name: String,
    /// The mirror module key (`candidate`) — arena getter names.
    module: String,
    /// The DTO module (`pg_table_name`) whose `dto_response` defines the
    /// Response and child structs the mirror references.
    dto_module: String,
    /// Operations in name order.
    ops: Vec<MoxOperationNode>,
    /// Mirror fields (id first, then the DTO field list, then children).
    fields: Vec<MirrorField>,
    /// Child-struct names (`{Struct}Response`) this surface references.
    child_structs: Vec<String>,
    /// Codelist enum names used by this surface's fields.
    codelist_names: Vec<String>,
    /// Structured-wrapper import lines (`use {types_import_prefix}::{T};`).
    structured_imports: Vec<String>,
    /// Field bindings for `expr` lowering (keyed by property name).
    bindings: HashMap<String, FieldBinding>,
}

pub struct MoxOpsGenerator {
    #[allow(dead_code)]
    output_dir: PathBuf,
    /// Resolved domain-types crate root — `mox_types.rs` (mirrors +
    /// `Resource` + op impls) lives there so the DTO response accessors
    /// and every generated band reach the one model surface.
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
        // arena index keys) and known codelists (signature enum slots).
        let mut known_classes: HashSet<String> = HashSet::new();
        let mut title_to_mirror: HashMap<String, String> = HashMap::new();
        let mut entity_props: HashMap<String, Vec<PropertyNode>> = HashMap::new();
        for title in entity_titles {
            let props = db.get_properties(title).await.unwrap_or_default();
            entity_props.insert(title.clone(), props);
            if let Ok(Some(schema)) = db.get_schema_in_domain(title, domain).await {
                let mirror = escape_rust_keyword(&schema.rust_type_name);
                title_to_mirror.insert(title.clone(), mirror.clone());
                known_classes.insert(mirror);
            }
        }
        let codelists: HashSet<String> = db
            .list_codelists()
            .await?
            .iter()
            .map(|cl| cl.name.clone())
            .collect();

        // Arena index (all domain entities — bodies may navigate any of
        // them). Mirror names are the graph's canonical `rust_type_name`.
        let mut arena: ArenaIndex = HashMap::new();
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

            // The DTO context is the single source of truth for field
            // names/types/shape (issue #517).
            let dto = build_dto_context(db, title, domain, config).await?;

            let mut fields: Vec<MirrorField> = vec![MirrorField {
                name: "id".to_string(),
                rust_type: "uuid::Uuid".to_string(),
            }];
            let mut child_structs: Vec<String> = Vec::new();
            for f in &dto.fields {
                if f.name == "id" {
                    continue;
                }
                fields.push(MirrorField {
                    name: f.name.clone(),
                    rust_type: mirror_field_type(f),
                });
            }
            for child in &dto.child_dtos {
                fields.push(MirrorField {
                    name: child.field_name.clone(),
                    rust_type: child_field_type(child),
                });
                child_structs.push(format!("{}Response", child.struct_name));
            }

            // Bindings for `expr` lowering: keyed by PROPERTY name (bodies
            // reference features as written), recv targets the DTO/mirror
            // field name so renamed refs (`_id`) resolve.
            let mut bindings: HashMap<String, FieldBinding> = HashMap::new();
            for p in &props {
                let def = codegraph_core::types::resolve_field(p);
                let ty = fields
                    .iter()
                    .find(|f| f.name == def.rust_field_name)
                    .map(|f| f.rust_type.clone())
                    .unwrap_or_else(|| p.rust_field_type.clone());
                let mut binding = FieldBinding::new(
                    format!("self.{}", def.rust_field_name),
                    !is_copy_rust_type(&ty),
                    ty,
                );
                if let Some(target) = &p.ref_target {
                    for (t, m) in &title_to_mirror {
                        if t == target || strip_suffix(t, suffix) == *target {
                            binding.class = Some(m.clone());
                            break;
                        }
                    }
                }
                bindings.insert(p.rust_field_name.clone(), binding.clone());
                if def.rust_field_name != p.rust_field_name {
                    bindings.insert(def.rust_field_name.clone(), binding);
                }
            }
            for child in &dto.child_dtos {
                let ty = child_field_type(child);
                bindings.insert(
                    child.field_name.clone(),
                    FieldBinding::new(format!("self.{}", child.field_name), true, ty),
                );
            }

            surfaces.push(ClassSurface {
                entity_name,
                module: escape_rust_keyword(&to_snake_case(&strip_suffix(title, suffix))),
                dto_module: dto.module_name.clone(),
                fields,
                child_structs,
                codelist_names: dto.codelist_imports.clone(),
                structured_imports: dto.structured_imports.clone(),
                title: title.clone(),
                ops: class_ops,
                bindings,
            });
        }
        if surfaces.is_empty() {
            return Ok(Vec::new());
        }

        let content = emit_mox_types(
            domain,
            &surfaces,
            &arena,
            &known_classes,
            &codelists,
            suffix,
            project,
        )?;
        Ok(vec![GeneratedFile {
            path: self
                .domain_types_dir
                .join("src")
                .join(domain)
                .join("mox_types.rs"),
            content,
        }])
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

/// Emit the complete `mox_types.rs` for one domain (deterministic:
/// surfaces title-ordered, ops name-ordered). `Err` on any unresolvable
/// signature slot or refused `expr` body — issue #508 R3 / #517.
fn emit_mox_types(
    domain: &str,
    surfaces: &[ClassSurface],
    arena: &ArenaIndex,
    known_classes: &HashSet<String>,
    codelists: &HashSet<String>,
    suffix: &str,
    project: &ProjectConfig,
) -> Result<String> {
    let mut code = CodeWriter::new();
    wln!(
        code,
        "//! Mox type surface for the {domain} domain (issues #508 R1/R4, #517).",
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
        "//! mirrors (DTO-shaped — every type resolves within the generated"
    );
    wln!(
        code,
        "//! tree), the `Resource` arena of the fixed mox operation signature,"
    );
    wln!(
        code,
        "//! and the operation impls themselves (one model surface, mirroring"
    );
    wln!(code, "//! the rex-backend-rust layout).");
    wln!(code, "#![allow(dead_code)]");
    wln!(code);
    wln!(code, "use std::collections::HashMap;");
    wln!(code, "use uuid::Uuid;");

    // Import block — the DTO layer's conventions (issue #517).
    let mut structured: Vec<&String> = Vec::new();
    let mut seen_structured = HashSet::new();
    let mut codelist_used: Vec<&String> = Vec::new();
    let mut seen_codelist = HashSet::new();
    for s in surfaces {
        for line in &s.structured_imports {
            if seen_structured.insert(line.clone()) {
                structured.push(line);
            }
        }
        for name in &s.codelist_names {
            if seen_codelist.insert(name.clone()) {
                codelist_used.push(name);
            }
        }
    }
    structured.sort();
    codelist_used.sort();
    if !codelist_used.is_empty() {
        let names = codelist_used
            .iter()
            .map(|n| n.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        wln!(code, "use crate::codelist::{{{names}}};", names = names);
    }
    for line in structured {
        wln!(code, "{line}", line = line.trim());
    }
    for s in surfaces {
        let mut names = vec![format!("{}Response", s.entity_name)];
        names.extend(s.child_structs.iter().cloned());
        names.sort();
        names.dedup();
        wln!(
            code,
            "use super::{module}::dto_response::{{{names}}};",
            module = s.dto_module,
            names = names.join(", "),
        );
    }

    let mut failures: Vec<String> = Vec::new();
    let sig_ctx = SigCtx {
        known_classes,
        codelists,
        suffix,
        project,
    };
    for surface in surfaces {
        wln!(code);
        wln!(
            code,
            "/// Row mirror for mox class `{title}` — the persistence-agnostic",
            title = surface.title
        );
        wln!(
            code,
            "/// impl target of `{name}` operations (issue #508 R1). Field",
            name = surface.entity_name
        );
        wln!(
            code,
            "/// names/types mirror `{name}Response` exactly (issue #517).",
            name = surface.entity_name
        );
        wln!(code, "#[derive(Debug, Clone)]");
        wln!(code, "pub struct {name} {{", name = surface.entity_name);
        for f in &surface.fields {
            wln!(
                code,
                "    pub {name}: {ty},",
                name = f.name,
                ty = f.rust_type
            );
        }
        wln!(code, "}}");
        wln!(code);
        wln!(code, "impl {name} {{", name = surface.entity_name);
        wln!(
            code,
            "    /// Copy the loaded response into the row mirror (issue #508 R2)."
        );
        wln!(
            code,
            "    pub fn from_response(r: &{name}Response) -> Self {{",
            name = surface.entity_name
        );
        wln!(code, "        Self {{");
        for f in &surface.fields {
            if f.name == "id" {
                wln!(code, "            id: r.id,");
            } else {
                wln!(code, "            {name}: r.{name}.clone(),", name = f.name);
            }
        }
        wln!(code, "        }}");
        wln!(code, "    }}");
        for op in &surface.ops {
            emit_operation(&mut code, surface, op, arena, &sig_ctx, &mut failures);
        }
        wln!(code, "}}");
    }

    emit_resource(&mut code, surfaces);

    if !failures.is_empty() {
        return Err(Error::Validation(format!(
            "mox behavior surface outside the emitted subset (issues #508 R3, #517):\n  - {}",
            failures.join("\n  - ")
        )));
    }
    Ok(code.into_string())
}

/// Emit the `Resource` arena (issue #508 R4).
fn emit_resource(code: &mut CodeWriter, surfaces: &[ClassSurface]) {
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
}

/// The fixed signature: `pub fn <snake>(&self, res: &Resource<, params>) -> <Ret>`.
fn signature(op: &MoxOperationNode, ctx: &SigCtx) -> std::result::Result<String, String> {
    let tt = &ctx.project.identity.domain_types_crate;
    let name = escape_rust_keyword(&to_snake_case(&op.name));
    let mut params = Vec::with_capacity(op.params.len());
    for p in &op.params {
        let ty = sig_type(&p.type_, ctx.known_classes, ctx.codelists, tt, ctx.suffix)?;
        params.push(format!(
            "{}: {}",
            escape_rust_keyword(&to_snake_case(&p.name)),
            ty
        ));
    }
    let ret = sig_type(
        &op.return_type,
        ctx.known_classes,
        ctx.codelists,
        tt,
        ctx.suffix,
    )?;
    Ok(format!(
        "pub fn {name}(&self, res: &Resource{sep}{params}) -> {ret}",
        sep = if params.is_empty() { "" } else { ", " },
        params = params.join(", "),
    ))
}

/// Emit one operation method (verbatim rust body, lowered expr body, or —
/// for a body-less op — the signature-complete seam).
fn emit_operation(
    code: &mut CodeWriter,
    surface: &ClassSurface,
    op: &MoxOperationNode,
    arena: &ArenaIndex,
    ctx: &SigCtx,
    failures: &mut Vec<String>,
) {
    let sig = match signature(op, ctx) {
        Ok(sig) => sig,
        Err(type_ref) => {
            failures.push(format!(
                "op `{}` on `{}`: signature type `{type_ref}` does not resolve",
                op.name, surface.title
            ));
            return;
        }
    };
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
    wln!(code, "    {sig} {{", sig = sig);

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

    fn empty_codelists() -> HashSet<String> {
        HashSet::new()
    }

    fn test_project() -> ProjectConfig {
        let mut project = ProjectConfig::default();
        project.identity.domain_types_crate = "domain_types".into();
        project
    }

    fn sig_ctx<'a>(
        classes: &'a HashSet<String>,
        codelists: &'a HashSet<String>,
        project: &'a ProjectConfig,
    ) -> SigCtx<'a> {
        SigCtx {
            known_classes: classes,
            codelists,
            suffix: "",
            project,
        }
    }

    fn op_with(param_type: &str, return_type: &str) -> MoxOperationNode {
        MoxOperationNode {
            name: "getBook".to_string(),
            class: "Library".to_string(),
            package: "lib".to_string(),
            description: None,
            return_type: return_type.to_string(),
            params: vec![codegraph_core::types::MoxParam {
                name: "title".to_string(),
                type_: param_type.to_string(),
            }],
            bodies: BTreeMap::new(),
        }
    }

    #[test]
    fn signature_maps_primitives() {
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
                    name: "year".to_string(),
                    type_: "long".to_string(),
                },
            ],
            bodies: BTreeMap::new(),
        };
        assert_eq!(
            signature(
                &op,
                &sig_ctx(&empty_classes(), &empty_codelists(), &test_project())
            )
            .unwrap(),
            "pub fn get_book(&self, res: &Resource, title: String, year: i64) -> i32"
        );
    }

    #[test]
    fn signature_maps_known_class_refs_to_the_id_slot() {
        let mut classes = HashSet::new();
        classes.insert("Shelf".to_string());
        let op = op_with("Shelf", "Shelf");
        assert_eq!(
            signature(&op, &sig_ctx(&classes, &empty_codelists(), &test_project())).unwrap(),
            "pub fn get_book(&self, res: &Resource, title: uuid::Uuid) -> uuid::Uuid"
        );
    }

    #[test]
    fn signature_maps_codelist_refs_to_the_generated_enum() {
        let mut codelists = HashSet::new();
        codelists.insert("BookCategory".to_string());
        let op = op_with("BookCategory", "string");
        assert_eq!(
            signature(&op, &sig_ctx(&empty_classes(), &codelists, &test_project())).unwrap(),
            "pub fn get_book(&self, res: &Resource, title: domain_types::codelist::BookCategory) -> String"
        );
    }

    #[test]
    fn signature_refuses_unresolvable_types() {
        // Issue #517: never a bare name — an unknown non-primitive slot is
        // a generation error.
        let op = op_with("Ghost", "int");
        assert_eq!(
            signature(
                &op,
                &sig_ctx(&empty_classes(), &empty_codelists(), &test_project())
            )
            .unwrap_err(),
            "Ghost"
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
            signature(
                &op,
                &sig_ctx(&empty_classes(), &empty_codelists(), &test_project())
            )
            .unwrap(),
            "pub fn has_short_id(&self, res: &Resource) -> bool"
        );
    }

    #[test]
    fn mirror_field_shapes_follow_the_response_template() {
        let required = DtoField {
            name: "title".into(),
            rust_type: "String".into(),
            is_required: true,
            is_array: false,
            description: String::new(),
            render_strategy: String::new(),
            is_entity_ref: false,
            is_hierarchy_field: false,
            min_length: None,
            max_length: None,
            minimum: None,
            maximum: None,
            pattern: None,
            format: None,
        };
        assert_eq!(mirror_field_type(&required), "String");
        let optional = DtoField {
            is_required: false,
            ..required.clone()
        };
        assert_eq!(mirror_field_type(&optional), "Option<String>");
        let id_ref = DtoField {
            is_entity_ref: true,
            rust_type: "ShelfType".into(),
            ..required.clone()
        };
        assert_eq!(mirror_field_type(&id_ref), "uuid::Uuid");
        let id_ref_array = DtoField {
            is_entity_ref: true,
            is_array: true,
            rust_type: "ShelfType".into(),
            ..required.clone()
        };
        assert_eq!(mirror_field_type(&id_ref_array), "Vec<uuid::Uuid>");
        let id_ref_array_opt = DtoField {
            is_required: false,
            ..id_ref_array.clone()
        };
        assert_eq!(
            mirror_field_type(&id_ref_array_opt),
            "Option<Vec<uuid::Uuid>>"
        );
    }
}
