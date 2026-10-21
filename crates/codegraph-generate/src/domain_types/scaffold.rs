use crate::ProjectConfig;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;
use codegraph_type_contracts::RefClassificationKind;
use serde::Serialize;

use crate::GenerationEntry;
use crate::api::api_model::resolve_entity_operations;
use crate::error::Result;
use crate::render_template_with_project;
use crate::traits::{GeneratedFile, GlobalGenerator, GlobalGeneratorKind};
use codegraph_config::DomainConfig;

#[derive(Debug, Serialize)]
struct DomainModContext {
    domain: String,
    entities: Vec<DomainModEntity>,
    /// `pub mod mox_types;` — the domain carries mox operations or derived
    /// features (issue #508: the mirrors + `Resource` arena live there).
    has_mox_types: bool,
}

#[derive(Debug, Serialize)]
struct DomainModEntity {
    module_name: String,
}

#[derive(Debug, Serialize)]
struct EntityModContext {
    entity_name: String,
    has_create: bool,
    has_update: bool,
}

/// Generates scaffold files (`lib.rs`, per-domain `mod.rs`, per-entity `mod.rs`)
/// for the domain-types crate.
pub struct DomainTypesScaffoldGenerator {
    /// Base directory for domain-types/src output.
    ///
    /// In production this is `{workspace_root}/crates/domain-types/src`.
    /// In tests this should be a temp directory to avoid corrupting the real
    /// workspace source files.
    src_dir: PathBuf,
}

impl DomainTypesScaffoldGenerator {
    /// Creates a generator that writes output under `base_dir` (crate root), appending `src/` internally.
    /// Pass a `tempfile::tempdir()` path to avoid corrupting the real source when using a mock graph.
    pub fn new_with_base(base_dir: PathBuf) -> Self {
        Self {
            src_dir: base_dir.join("src"),
        }
    }
}

#[async_trait]
impl GlobalGenerator for DomainTypesScaffoldGenerator {
    fn kind(&self) -> GlobalGeneratorKind {
        GlobalGeneratorKind::DomainTypesScaffold
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        config: &DomainConfig,
        generation_order: &[GenerationEntry],
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        // Group generation_order entries by domain, deduplicating by (domain, module_name).
        // The tuple carries the entry's schema_title so the per-entity mod.rs
        // loop can apply the same DDD-design operation override the dto
        // generator applies (below) — otherwise mod.rs declares dto_create/
        // dto_update modules the dto generator intentionally skipped.
        let mut domain_entity_map: std::collections::HashMap<
            String,
            Vec<(String, String, String)>,
        > = std::collections::HashMap::new();
        let mut seen = std::collections::HashSet::new();
        let mut domain_order = Vec::new();
        let mut seen_domains = std::collections::HashSet::new();

        for entry in generation_order {
            let stripped = config.defaults.strip_suffix(&entry.schema_title);
            // Sanitize to a PascalCase Rust identifier matching rust_type_name.
            // Naively pascal-casing the title is NOT enough for titles with
            // acronyms/hyphens (e.g. "...LER-RSType" -> "...Lerrs..." vs the
            // graph's canonical "...LERRS..."), which made this mod.rs re-export
            // disagree with the trait emitted by the query_service generator.
            // Prefer the graph's canonical name and fall back to the sanitized
            // title when the schema is absent (tests).
            let entity_name = db
                .get_schema_in_domain(&entry.schema_title, &entry.domain)
                .await
                .ok()
                .flatten()
                .map(|s| s.rust_type_name)
                .unwrap_or_else(|| codegraph_naming::to_pascal_case(&stripped));
            let module_name = codegraph_naming::to_snake_case(&stripped);

            if seen_domains.insert(entry.domain.clone()) {
                domain_order.push(entry.domain.clone());
            }

            // Dedup by (domain, module_name)
            if !seen.insert((entry.domain.clone(), module_name.clone())) {
                continue;
            }

            domain_entity_map
                .entry(entry.domain.clone())
                .or_default()
                .push((entity_name, module_name, entry.schema_title.clone()));
        }

        let src_dir = &self.src_dir;
        let mut files = Vec::new();

        // DDD design surface (issue #449), loaded once: designs override the
        // per-entity operation set below in lockstep with the dto generator.
        let design_surface = crate::ddd::design::DddDesignSurface::from_graph(db).await?;

        // Clean stale entity directories that no longer appear in the
        // generation order.  Previous pipeline runs may have written
        // directories for entities that are now classified differently
        // (e.g. demoted to value objects).  If left on disk they cause
        // the domain mod.rs to be out of sync with the actual entity set.
        {
            let mut valid_modules_by_domain: HashMap<String, std::collections::HashSet<String>> =
                HashMap::new();
            for entry in generation_order {
                let stripped = config.defaults.strip_suffix(&entry.schema_title);
                let module_name = codegraph_naming::to_snake_case(&stripped);
                valid_modules_by_domain
                    .entry(entry.domain.clone())
                    .or_default()
                    .insert(module_name);
            }

            for domain_name in &domain_order {
                let domain_dir = src_dir.join(domain_name);
                if !domain_dir.is_dir() {
                    continue;
                }
                let valid_for_domain = valid_modules_by_domain.get(domain_name);
                if let Ok(entries) = std::fs::read_dir(&domain_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if !path.is_dir() {
                            continue;
                        }
                        let name = entry.file_name().to_string_lossy().to_string();
                        let is_valid = valid_for_domain.map(|s| s.contains(&name)).unwrap_or(false);
                        if !is_valid {
                            tracing::debug!(
                                domain = %domain_name,
                                module = %name,
                                path = %path.display(),
                                "removing stale entity directory"
                            );
                            if let Err(e) = std::fs::remove_dir_all(&path) {
                                tracing::warn!(
                                    path = %path.display(),
                                    "failed to remove stale entity directory: {e}"
                                );
                            }
                        }
                    }
                }
            }
        }

        // Mox surface per domain (issue #508): classes carrying operations
        // or derived features own a `mox_types` module (mirrors + the real
        // `Resource` arena).
        let mox_ops = db.get_mox_operations().await.unwrap_or_default();
        let mox_derived = db.get_mox_derived_features().await.unwrap_or_default();
        let suffix = &config.defaults.type_suffix;

        // 1. Generate per-domain mod.rs
        for domain_name in &domain_order {
            let entities = match domain_entity_map.get(domain_name) {
                Some(e) => e,
                None => continue,
            };

            let has_mox_types = entities.iter().any(|(_name, _module, title)| {
                let matches = |class: &str| {
                    class == title.as_str()
                        || codegraph_naming::strip_suffix(title, suffix) == class
                };
                mox_ops.iter().any(|o| matches(&o.class))
                    || mox_derived.iter().any(|d| matches(&d.class))
            });

            let domain_mod_ctx = DomainModContext {
                domain: domain_name.clone(),
                entities: entities
                    .iter()
                    .map(|(_name, module, _title)| DomainModEntity {
                        module_name: module.clone(),
                    })
                    .collect(),
                has_mox_types,
            };

            let content = render_template_with_project(
                tera,
                "domain_types/domain_mod.tera",
                &domain_mod_ctx,
                project,
            )?;
            files.push(GeneratedFile {
                path: src_dir.join(domain_name).join("mod.rs"),
                content,
            });

            // 2. Generate per-entity mod.rs
            for (entity_name, module_name, schema_title) in entities {
                // DDD design surface (issue #449): a design covering this
                // title overrides the operation set exactly as
                // `build_dto_context` does — a design without a repository
                // maps to an empty set, so no dto_create/dto_update modules
                // are declared for it. Without this, mod.rs declares modules
                // the dto generator intentionally skipped (E0583).
                let mut operations =
                    resolve_entity_operations(db, config, domain_name, entity_name).await;
                if let Some(design_ops) = design_surface.operations_for(schema_title) {
                    operations = design_ops;
                }

                // `entity_name` is already the graph's canonical PascalCase
                // identifier (rust_type_name, acronym-safe). Re-pascal-casing
                // it here corrupts initialisms (LERRS -> Lerrs, US -> Us,
                // W3C -> W3c), making this mod.rs re-export disagree with the
                // query_service struct names (E0432).
                let entity_name_pascal = entity_name;
                let entity_mod_ctx = EntityModContext {
                    entity_name: entity_name_pascal.to_string(),
                    has_create: operations.contains(&"create".to_string()),
                    has_update: operations.contains(&"update".to_string()),
                };

                let content = render_template_with_project(
                    tera,
                    "domain_types/entity_mod.tera",
                    &entity_mod_ctx,
                    project,
                )?;
                files.push(GeneratedFile {
                    path: src_dir.join(domain_name).join(module_name).join("mod.rs"),
                    content,
                });
            }
        }

        // 3. Generate lib.rs with hand-written modules preserved and domain modules appended.
        // Suppress lints inherent to code-generated domain types:
        // - module_inception: HR Open entity names sometimes match their domain (e.g. screening::screening)
        // - unused_imports: Update DTOs import all Create* types for completeness
        self.emit_support_modules(src_dir);

        // Collect structured wrapper types used by generated entities so that
        // lib.rs provides a local re-export (e.g. `pub use codegraph_type_contracts::IdentifierType;`).
        // This allows generated DTOs to write `use crate::IdentifierType;` when
        // `types_import_prefix = "crate"` in domains.toml, avoiding direct
        // dependency on the codegraph crate name.
        let mut structured_types: HashSet<String> = HashSet::new();
        for entry in generation_order {
            if let Ok(props) = db.get_properties(&entry.schema_title).await {
                for prop in &props {
                    if prop.effective_kind() == Some(RefClassificationKind::StructuredWrapper)
                        || prop.effective_kind() == Some(RefClassificationKind::PrimitiveWrapper)
                    {
                        let mut ty = prop.rust_field_type.as_str();
                        if let Some(s) = ty.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')) {
                            ty = s;
                        }
                        if let Some(s) =
                            ty.strip_prefix("Option<").and_then(|s| s.strip_suffix('>'))
                        {
                            ty = s;
                        }
                        if !ty.is_empty()
                            && ty != "serde_json::Value"
                            && ty != "String"
                            && ty != "bool"
                            && ty != "f64"
                            && ty != "i64"
                            && ty != "i32"
                            && ty != "i16"
                            && ty != "u64"
                            && ty != "u32"
                            && ty != "Decimal"
                            && ty != "NaiveDate"
                            && ty != "DateTime"
                            && !ty.contains('<')
                            && !ty.contains("::")
                        {
                            structured_types.insert(ty.to_string());
                        }
                    }
                }
            }
        }

        let mut sorted_domains = domain_order.clone();
        sorted_domains.sort();
        let domain_mods: String = sorted_domains
            .iter()
            .map(|d| format!("pub mod {};\n", d))
            .collect();

        let mut structured_re_exports = String::new();
        let mut sorted_types: Vec<&String> = structured_types.iter().collect();
        sorted_types.sort();
        let prefix = &project.codegen.types_import_prefix;
        for ty in &sorted_types {
            structured_re_exports.push_str(&format!("pub use {}::{};\n", prefix, ty));
        }
        if !structured_re_exports.is_empty() {
            structured_re_exports = format!(
                "\n// --- STRUCTURED WRAPPER RE-EXPORTS ---\n{}",
                structured_re_exports
            );
        }

        let lib_content = format!(
            "// Generated crate — do not edit.\n\
             #![allow(clippy::module_inception, unused_imports, ambiguous_glob_reexports)]\n\
             \n\
             pub mod codelist;\n\
             pub mod context;\n\
             pub mod query;\n\
             \n\
             pub use context::{{SourceContext, SourceOrigin}};\n\
             pub use query::{{ListParams, PagedResult, QueryError, SortOrder}};\n\
             pub use serde_json;\
             {structured_re_exports}\n\
             \n\
             // --- GENERATED DOMAIN MODULES ---\n\
             {domain_mods}"
        );
        files.push(GeneratedFile {
            path: src_dir.join("lib.rs"),
            content: lib_content,
        });

        // 4. Generate Cargo.toml at the crate root (parent of src/)
        let cargo_toml = render_template_with_project(
            tera,
            "domain_types/cargo_toml.tera",
            &serde_json::json!({}),
            project,
        )?;
        files.push(GeneratedFile {
            path: src_dir.parent().unwrap().join("Cargo.toml"),
            content: cargo_toml,
        });

        Ok(files)
    }
}

impl DomainTypesScaffoldGenerator {
    /// Emit the support modules (`context.rs`, `query.rs`, `codelist/mod.rs`)
    /// declared by the generated lib.rs, but only when the target crate does
    /// not already provide hand-written versions — consumers may hand-write
    /// richer versions (e.g. extra `SourceOrigin` variants).
    fn emit_support_modules(&self, src_dir: &std::path::Path) {
        if !src_dir.join("context.rs").exists() {
            let _ = std::fs::create_dir_all(src_dir);
            let _ = std::fs::write(src_dir.join("context.rs"), GENERIC_CONTEXT_RS);
        }
        if !src_dir.join("query.rs").exists() {
            let _ = std::fs::create_dir_all(src_dir);
            let _ = std::fs::write(src_dir.join("query.rs"), GENERIC_QUERY_RS);
        }
        let codelist_mod = src_dir.join("codelist/mod.rs");
        if !codelist_mod.exists() {
            let _ = std::fs::create_dir_all(src_dir.join("codelist"));
            let _ = std::fs::write(
                codelist_mod,
                "//! Codelist enum re-exports.\n\
                 //! Generated by codegraph. DO NOT EDIT.\n\
                 \n",
            );
        }
    }
}

/// Generic `SourceContext`/`SourceOrigin` support module. Consumers may
/// replace it with hand-written variants (extra origins, etc.).
const GENERIC_CONTEXT_RS: &str = "// Generated by codegraph. DO NOT EDIT.\n\
use serde::{Deserialize, Serialize};\n\
\n\
/// Well-known mutation origins.\n\
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]\n\
pub enum SourceOrigin {\n\
    /// Normal API request.\n\
    Api,\n\
    /// Internal system process (e.g. scheduled job, migration).\n\
    System,\n\
    /// An origin not covered by the well-known variants.\n\
    Custom(String),\n\
}\n\
\n\
impl SourceOrigin {\n\
    pub fn as_str(&self) -> &str {\n\
        match self {\n\
            Self::Api => \"api\",\n\
            Self::System => \"system\",\n\
            Self::Custom(s) => s.as_str(),\n\
        }\n\
    }\n\
}\n\
\n\
impl std::fmt::Display for SourceOrigin {\n\
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n\
        f.write_str(self.as_str())\n\
    }\n\
}\n\
\n\
/// Identifies where a mutation originated — API, external system, or internal process.\n\
#[derive(Debug, Clone, Serialize, Deserialize)]\n\
pub struct SourceContext {\n\
    pub origin: SourceOrigin,\n\
    pub external_id: Option<String>,\n\
    pub external_ref: Option<String>,\n\
    pub idempotency_key: Option<String>,\n\
    pub external_timestamp: Option<chrono::DateTime<chrono::Utc>>,\n\
}\n\
\n\
impl SourceContext {\n\
    pub fn api() -> Self {\n\
        Self {\n\
            origin: SourceOrigin::Api,\n\
            external_id: None,\n\
            external_ref: None,\n\
            idempotency_key: None,\n\
            external_timestamp: None,\n\
        }\n\
    }\n\
\n\
    pub fn external(origin: SourceOrigin) -> Self {\n\
        Self {\n\
            origin,\n\
            external_id: None,\n\
            external_ref: None,\n\
            idempotency_key: None,\n\
            external_timestamp: None,\n\
        }\n\
    }\n\
\n\
    pub fn with_external_id(mut self, id: impl Into<String>) -> Self {\n\
        self.external_id = Some(id.into());\n\
        self\n\
    }\n\
\n\
    pub fn with_external_ref(mut self, r: impl Into<String>) -> Self {\n\
        self.external_ref = Some(r.into());\n\
        self\n\
    }\n\
\n\
    pub fn with_idempotency_key(mut self, key: impl Into<String>) -> Self {\n\
        self.idempotency_key = Some(key.into());\n\
        self\n\
    }\n\
\n\
    pub fn with_external_timestamp(mut self, ts: chrono::DateTime<chrono::Utc>) -> Self {\n\
        self.external_timestamp = Some(ts);\n\
        self\n\
    }\n\
}\n";

/// Generic query support module (ListParams/PagedResult/QueryError/SortOrder).
const GENERIC_QUERY_RS: &str = "// Generated by codegraph. DO NOT EDIT.\n\
use serde::{Deserialize, Serialize};\n\
\n\
#[derive(Debug, Clone, Serialize, Deserialize)]\n\
pub struct ListParams {\n\
    pub page: u64,\n\
    pub per_page: u64,\n\
    pub sort_by: Option<String>,\n\
    pub sort_order: Option<SortOrder>,\n\
    pub filter: Option<serde_json::Value>,\n\
}\n\
\n\
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]\n\
pub enum SortOrder {\n\
    Asc,\n\
    Desc,\n\
}\n\
\n\
#[derive(Debug, Clone, Serialize, Deserialize)]\n\
pub struct PagedResult<T> {\n\
    pub items: Vec<T>,\n\
    pub total: u64,\n\
    pub page: u64,\n\
    pub per_page: u64,\n\
}\n\
\n\
#[derive(Debug, thiserror::Error)]\n\
pub enum QueryError {\n\
    #[error(\"not found: {0}\")]\n\
    NotFound(String),\n\
    #[error(\"database: {0}\")]\n\
    Database(String),\n\
    #[error(\"internal: {0}\")]\n\
    Internal(#[from] Box<dyn std::error::Error + Send + Sync>),\n\
}\n\
\n\
impl Default for ListParams {\n\
    fn default() -> Self {\n\
        Self {\n\
            page: 1,\n\
            per_page: 20,\n\
            sort_by: None,\n\
            sort_order: None,\n\
            filter: None,\n\
        }\n\
    }\n\
}\n";
