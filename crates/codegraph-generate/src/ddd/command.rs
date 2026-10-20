use crate::ProjectConfig;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::ParentCandidate;
use serde::Serialize;

use crate::api::api_model::resolve_entity_operations;
use crate::error::Result;
use crate::render_template_with_project;
use crate::traits::{EntityGenerator, EntityGeneratorKind, GeneratedFile};
use codegraph_config::DomainConfig;

#[derive(Debug, Serialize)]
pub struct CommandContext {
    pub entity_name: String,
    pub module_name: String,
    pub domain: String,
    pub operations: Vec<String>,
    pub has_create: bool,
    pub has_update: bool,
    pub has_delete: bool,
    pub parent_ref: Option<String>,
    /// A push-direction extension point covers this entity (issue #503):
    /// the command layer carries dispatcher.notify call sites.
    pub has_extension_points: bool,
    /// The full `domain.table` ref the dispatcher addresses and the
    /// installations SQL matches against `ep.allowed_entities`.
    pub entity_ref: String,
}

pub struct CommandGenerator {
    output_dir: PathBuf,
    parent_candidates: Vec<ParentCandidate>,
    /// Push-covered entity refs ("domain.table") from the parsed
    /// extension-points config (issue #503).
    extension_covered: std::collections::HashSet<String>,
}

impl CommandGenerator {
    pub fn new(output_dir: &Path) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
            parent_candidates: Vec::new(),
            extension_covered: std::collections::HashSet::new(),
        }
    }

    pub fn with_parent_candidates(mut self, candidates: Vec<ParentCandidate>) -> Self {
        self.parent_candidates = candidates;
        self
    }

    pub fn with_extension_points(mut self, covered: std::collections::HashSet<String>) -> Self {
        self.extension_covered = covered;
        self
    }
}

#[async_trait]
impl EntityGenerator for CommandGenerator {
    fn kind(&self) -> EntityGeneratorKind {
        EntityGeneratorKind::Command
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        schema_title: &str,
        domain: &str,
        config: &DomainConfig,
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        let schema = db
            .get_schema_in_domain(schema_title, domain)
            .await?
            .ok_or_else(|| crate::error::Error::SchemaNotFound(schema_title.into()))?;

        let entity_name = schema.rust_type_name.clone();
        let module_name = schema.pg_table_name.clone();
        let domain = domain.to_string();

        if module_name.is_empty() {
            return Ok(Vec::new());
        }

        let entity_cfg = config
            .domains
            .get(&domain)
            .and_then(|d| d.get_entity_config(&entity_name));

        // DDD design surface (issue #449): a design covering this title
        // replaces the operations with its repository built-in mapping.
        let design_surface = crate::ddd::design::DddDesignSurface::from_graph(db).await?;
        let mut operations = resolve_entity_operations(db, config, &domain, &entity_name).await;
        if let Some(design_ops) = design_surface.operations_for(schema_title) {
            operations = design_ops;
        }

        // Resolve parent_ref for child entities
        let parent_ref = crate::resolve_parent_fk_column_same_domain(
            schema_title,
            &self.parent_candidates,
            entity_cfg,
            &domain,
            config,
            db,
        )
        .await;

        // Hook-only entities (no create/update/delete) get no command handler —
        // emit nothing rather than a dead skeleton (unused imports, dead struct).
        let has_command_ops = operations
            .iter()
            .any(|op| op == "create" || op == "update" || op == "delete");
        if !has_command_ops {
            return Ok(Vec::new());
        }

        let ctx = CommandContext {
            has_create: operations.contains(&"create".to_string()),
            has_update: operations.contains(&"update".to_string()),
            has_delete: operations.contains(&"delete".to_string()),
            parent_ref,
            entity_name,
            has_extension_points: self
                .extension_covered
                .contains(&format!("{domain}.{}", module_name)),
            entity_ref: format!("{domain}.{}", module_name),
            module_name: module_name.clone(),
            domain: domain.clone(),
            operations,
        };

        let content = render_template_with_project(tera, "ddd/command.tera", &ctx, project)?;
        Ok(vec![GeneratedFile {
            path: self
                .output_dir
                .join("src")
                .join("domain")
                .join(&domain)
                .join(&module_name)
                .join("command.rs"),
            content,
        }])
    }
}
