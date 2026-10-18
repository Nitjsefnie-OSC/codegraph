//! The generated `ui/src/lib/testids.ts` module (issue #468 W3, feature
//! `testid_module`): the TypeScript projection of [`crate::ux::plan::ids`]
//! that the generated POMs and specs import. Page markup carries the same
//! vocabulary via the `tid` render contexts ([`super::testids::for_module`]),
//! so all three surfaces — markup, POM, specs — read one Rust table.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;
use serde::Serialize;

use super::common::collect_ui_fields;
use crate::GenerationEntry;
use crate::ProjectConfig;
use crate::error::Result;
use crate::render_template_with_project;
use crate::traits::{GeneratedFile, GlobalGenerator, GlobalGeneratorKind};
use codegraph_config::DomainConfig;

#[derive(Debug, Serialize)]
pub struct UiTestidsEntity {
    pub name: String,
    pub module_name: String,
    pub domain: String,
    /// The entity's own field names — the typed union behind `field()`.
    pub fields: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct UiTestidsContext {
    pub entities: Vec<UiTestidsEntity>,
    pub shared: super::testids::SharedTids,
}

pub struct UiTestidsGenerator {
    output_dir: PathBuf,
}

impl UiTestidsGenerator {
    pub fn new(output_dir: &Path) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
        }
    }
}

#[async_trait]
impl GlobalGenerator for UiTestidsGenerator {
    fn kind(&self) -> GlobalGeneratorKind {
        GlobalGeneratorKind::UiTestids
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        config: &DomainConfig,
        generation_order: &[GenerationEntry],
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        if !project.codegen.testid_module {
            // Listed in the plan but the feature is off: the silent-nothing
            // antipattern (issue #468 phase 1) — say so.
            eprintln!(
                "warning: ui_testids: generator is in the plan but the \
                 `testid_module` feature is off; no testids module emitted"
            );
            return Ok(vec![]);
        }

        let mut entities = Vec::new();
        for entry in generation_order {
            let schema = match db
                .get_schema_in_domain(&entry.schema_title, &entry.domain)
                .await?
            {
                Some(s) => s,
                None => continue,
            };
            let module_name = schema.pg_table_name.clone();
            if module_name.is_empty() {
                continue;
            }
            let entity_name = schema.rust_type_name.clone();
            let domain = entry.domain.clone();
            let entity_cfg = config
                .domains
                .get(&domain)
                .and_then(|d| d.get_entity_config(&entity_name));
            let immutable_fields: Vec<String> = entity_cfg
                .map(|ec| ec.dto.immutable_fields.clone())
                .unwrap_or_default();
            let fields = collect_ui_fields(
                db,
                &entry.schema_title,
                &immutable_fields,
                Some(&domain),
                config,
            )
            .await?
            .into_iter()
            .map(|f| f.name)
            .collect();
            entities.push(UiTestidsEntity {
                name: entity_name,
                module_name,
                domain,
                fields,
            });
        }
        entities.sort_by(|a, b| a.name.cmp(&b.name));

        let ctx = UiTestidsContext {
            entities,
            shared: super::testids::shared(),
        };
        let content = render_template_with_project(tera, "ui/testids.ts.tera", &ctx, project)?;
        Ok(vec![GeneratedFile {
            path: self
                .output_dir
                .join("ui")
                .join("src")
                .join("lib")
                .join("testids.ts"),
            content,
        }])
    }
}
