use crate::ProjectConfig;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;

use crate::GenerationEntry;
use crate::error::Result;
use crate::render_template_with_project;
use crate::traits::{GeneratedFile, GlobalGenerator, GlobalGeneratorKind};
use codegraph_config::DomainConfig;

pub struct IntegrationDispatchGenerator {
    output_dir: PathBuf,
    /// Domains receiving a per-worker copy of the dispatcher module under
    /// the workers topology (the covered domains — a worker crate whose
    /// command layer carries notify call sites needs the module locally).
    worker_domains: Vec<String>,
}

impl IntegrationDispatchGenerator {
    pub fn new(output_dir: &Path) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
            worker_domains: Vec::new(),
        }
    }

    /// Route per-domain worker copies under the workers topology (issue
    /// #503): one `workers/{domain}/src/integration_dispatch.rs` per
    /// covered domain.
    pub fn with_worker_domains(mut self, domains: Vec<String>) -> Self {
        self.worker_domains = domains;
        self
    }

    fn emit_files(&self, content: String) -> Vec<GeneratedFile> {
        let mut files = vec![GeneratedFile {
            path: self.output_dir.join("src").join("integration_dispatch.rs"),
            content: content.clone(),
        }];
        for domain in &self.worker_domains {
            files.push(GeneratedFile {
                path: self
                    .output_dir
                    .join("workers")
                    .join(domain)
                    .join("src")
                    .join("integration_dispatch.rs"),
                content: content.clone(),
            });
        }
        files
    }
}

#[async_trait]
impl GlobalGenerator for IntegrationDispatchGenerator {
    fn kind(&self) -> GlobalGeneratorKind {
        GlobalGeneratorKind::IntegrationDispatch
    }

    async fn generate(
        &self,
        _db: &dyn GraphQuerier,
        _config: &DomainConfig,
        _generation_order: &[GenerationEntry],
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        let ctx: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let content =
            render_template_with_project(tera, "integration/dispatcher.tera", &ctx, project)?;

        Ok(self.emit_files(content))
    }
}
