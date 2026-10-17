use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use codegraph_config::DomainConfig;
use codegraph_core::traits::GraphQuerier;

use crate::ProjectConfig;
use crate::api::api_model::{
    resolve_entity_operations, resolve_path_segment, resolve_path_segment_with_config,
};
use crate::error::Result;
use crate::render_template_with_project;
use crate::traits::{EntityGenerator, EntityGeneratorKind, GeneratedFile};

use super::common::{collect_child_sections, collect_ui_fields};
use super::context::UiE2eTestContext;
use super::deps::build_required_dependencies;
use super::fixtures::build_test_data_json;
use super::include::resolve_e2e_include_config;
use super::page::UiField;
use super::pom_ctx::PomCtx;
use super::refs::{apply_convention_refs, is_convention_ref_gap};
use super::store::UiParentInfo;
use super::ux_spec::build_ux_e2e_spec;

pub struct UiE2eTestGenerator {
    output_dir: PathBuf,
    parent_candidates: Vec<codegraph_core::types::ParentCandidate>,
    /// The shared POM kernel (`ui/tests/generated/_support/pom.ts`) is
    /// entity-independent content emitted ONCE per generation run: the
    /// first entity whose generation emits any spec file renders it (the
    /// e2e generator is Entity-kind, so this is the once-per-run hook —
    /// content is stable and the file is regenerated every run, #316).
    kernel_emitted: AtomicBool,
}

impl UiE2eTestGenerator {
    pub fn new(output_dir: &Path) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
            parent_candidates: Vec::new(),
            kernel_emitted: AtomicBool::new(false),
        }
    }

    pub fn with_parent_candidates(
        mut self,
        candidates: Vec<codegraph_core::types::ParentCandidate>,
    ) -> Self {
        self.parent_candidates = candidates;
        self
    }

    /// Resolve grandparent info for a given parent entity (depth-2 nesting).
    /// Checks manual config first, then graph parent_candidates.
    async fn resolve_grandparent(
        &self,
        db: &dyn GraphQuerier,
        config: &DomainConfig,
        parent_title: &str,
        parent_domain: &str,
    ) -> Option<Box<super::store::UiGrandparentInfo>> {
        let parent_stripped =
            crate::api::router::strip_suffix(parent_title, &config.defaults.type_suffix);

        // 1. Check manual config for parent's parent
        if let Some(parent_ec) = config
            .domains
            .get(parent_domain)
            .and_then(|d| d.get_entity_config(parent_title))
        {
            if parent_ec.role.as_deref() == Some("child") {
                if let Some(ref gp_title) = parent_ec.parent
                    && let Ok(Some(gp_schema)) =
                        db.get_schema_in_domain(gp_title, parent_domain).await
                {
                    let gp_domain = if config
                        .domains
                        .get(parent_domain)
                        .map(|d| d.entities.contains(gp_title))
                        .unwrap_or(false)
                    {
                        parent_domain.to_string()
                    } else {
                        gp_schema
                            .domain
                            .clone()
                            .unwrap_or_else(|| parent_domain.to_string())
                    };
                    return Some(Box::new(super::store::UiGrandparentInfo {
                        param_name: crate::api::router::param_name_from_path_segment(
                            &resolve_path_segment_with_config(None, &gp_schema, config),
                        ),
                        domain: gp_domain,
                        path_segment: resolve_path_segment_with_config(None, &gp_schema, config),
                        entity_name: gp_schema.rust_type_name.clone(),
                    }));
                }
            } else if parent_ec.role.as_deref() == Some("root") {
                return None; // Explicitly root — no grandparent
            }
        }

        // 2. Check graph parent_candidates
        for gpc in &self.parent_candidates {
            let gpc_child =
                crate::api::router::strip_suffix(&gpc.child_title, &config.defaults.type_suffix);
            if gpc_child == parent_stripped {
                if let Ok(Some(gp_schema)) = db
                    .get_schema_in_domain(&gpc.parent_title, parent_domain)
                    .await
                {
                    let gp_domain = if config
                        .domains
                        .get(parent_domain)
                        .map(|d| d.entities.contains(&gpc.parent_title))
                        .unwrap_or(false)
                    {
                        parent_domain.to_string()
                    } else {
                        gp_schema
                            .domain
                            .clone()
                            .unwrap_or_else(|| parent_domain.to_string())
                    };
                    return Some(Box::new(super::store::UiGrandparentInfo {
                        param_name: crate::api::router::param_name_from_path_segment(
                            &resolve_path_segment_with_config(None, &gp_schema, config),
                        ),
                        domain: gp_domain,
                        path_segment: resolve_path_segment_with_config(None, &gp_schema, config),
                        entity_name: gp_schema.rust_type_name.clone(),
                    }));
                }
                break;
            }
        }

        None
    }
}

#[async_trait]
impl EntityGenerator for UiE2eTestGenerator {
    fn kind(&self) -> EntityGeneratorKind {
        EntityGeneratorKind::UiE2eTest
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
        let entity_label =
            codegraph_naming::to_display_name(&config.defaults.strip_suffix(&schema.title));
        let module_name = schema.pg_table_name.clone();
        let domain = domain.to_string();

        if module_name.is_empty() {
            return Ok(Vec::new());
        }

        let entity_cfg = config
            .domains
            .get(&domain)
            .and_then(|d| d.get_entity_config(&entity_name));

        let path_segment = resolve_path_segment(entity_cfg, &schema);

        let operations = resolve_entity_operations(db, config, &domain, &entity_name).await;

        let dto_config = entity_cfg.map(|ec| &ec.dto);
        let immutable_fields: Vec<String> = dto_config
            .map(|d| d.immutable_fields.clone())
            .unwrap_or_default();

        let workflow = entity_cfg.and_then(|ec| ec.workflow.as_ref());
        let has_workflow = workflow
            .map(|wf| wf.generate_action_endpoints)
            .unwrap_or(false);
        let workflow_states = workflow.map(|wf| wf.states.clone()).unwrap_or_default();
        let initial_state = workflow
            .map(|wf| wf.initial_state.clone())
            .unwrap_or_default();
        let terminal_states = workflow
            .map(|wf| wf.terminal_states.clone())
            .unwrap_or_default();

        // Workflow-excluded fields for create/update
        let mut all_excluded: Vec<String> = immutable_fields.clone();
        if let Some(wf) = workflow {
            all_excluded.push(wf.status_field.clone());
            if let Some(ref approval_field) = wf.approval_status_field {
                all_excluded.push(approval_field.clone());
            }
        }

        let mut fields =
            collect_ui_fields(db, schema_title, &immutable_fields, Some(&domain), config).await?;

        let all_props = match Some(domain.as_str()) {
            Some(d) => db.get_properties_in_domain(schema_title, d).await?,
            None => db.get_properties(schema_title).await?,
        };
        // Required plain-uuid FK columns without a `$ref`/graph edge (e.g.
        // `party.case_id`) resolve to their entity by naming convention and
        // must be marked before create/update fields are derived.
        let convention_refs =
            apply_convention_refs(db, config, &domain, &all_props, &mut fields).await;

        // A convention ref is only dep-created when its target entity has a
        // create API (the schema-ref path). Otherwise the field is unmarked —
        // testData keeps the text sentinel — and the gap is named in the
        // emitted payload.
        let mut creatable_convention_refs: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for res in &convention_refs {
            let target_domain = res.target.domain.clone().unwrap_or_else(|| domain.clone());
            let target_operations =
                resolve_entity_operations(db, config, &target_domain, &res.target.title).await;
            if target_operations.iter().any(|op| op == "create") {
                creatable_convention_refs.insert(res.field_name.clone());
            } else if let Some(f) = fields.iter_mut().find(|f| f.name == res.field_name) {
                f.is_entity_ref = false;
                f.ref_api_path = None;
            }
        }

        let mut create_fields: Vec<UiField> = fields
            .iter()
            .filter(|f| !all_excluded.contains(&f.name))
            .cloned()
            .collect();
        // For codelist entities with no UI fields (enum-only schemas), inject a
        // synthetic code field so testData() produces a valid create payload.
        if create_fields.is_empty()
            && let Ok(Some(schema)) = db.get_schema_in_domain(schema_title, &domain).await
            && schema.is_codelist
            && domain == "common"
        {
            create_fields.push(UiField {
                name: "code".to_string(),
                label: "Code".to_string(),
                ts_type: "string".to_string(),
                input_type: "code".to_string(),
                is_required: true,
                is_array: false,
                is_entity_ref: false,
                is_immutable: false,
                is_codelist: false,
                is_range: false,
                codelist_values: vec![],
                description: String::new(),
                pg_type: "TEXT".to_string(),
                open_end: false,
                ref_api_path: None,
                structured_sub_fields: vec![],
                nested_type_name: None,
            });
        }

        let required_create_fields: Vec<UiField> = create_fields
            .iter()
            .filter(|f| f.is_required)
            .cloned()
            .collect();

        let update_fields: Vec<UiField> = fields
            .iter()
            .filter(|f| !f.is_immutable && !all_excluded.contains(&f.name))
            .cloned()
            .collect();

        // Convention-ref gaps, over the create payload only (the comment is
        // emitted inside the testData loop): required `_id` columns the name
        // convention claims that did not resolve to a creatable entity.
        let convention_ref_gaps: Vec<String> = create_fields
            .iter()
            .filter(|f| {
                !creatable_convention_refs.contains(&f.name) && is_convention_ref_gap(f, &all_props)
            })
            .map(|f| f.name.clone())
            .collect();

        let first_list_column = fields.first().map(|f| f.name.clone());

        // Determine FTS availability and search field
        let has_fts = entity_cfg
            .map(|ec| {
                !ec.search.fts_weights.is_empty()
                    || ec
                        .search
                        .fts_columns
                        .as_ref()
                        .map(|c| !c.is_empty())
                        .unwrap_or(false)
            })
            .unwrap_or(false);

        let fts_search_field = if has_fts {
            // Pick the field with the highest FTS weight (A > B > C > D)
            let weights = &entity_cfg.unwrap().search.fts_weights;
            let weight_order = ["A", "B", "C", "D"];
            weight_order
                .iter()
                .find_map(|w| {
                    weights
                        .iter()
                        .find(|(_, v)| v.as_str() == *w)
                        .map(|(k, _)| k.clone())
                })
                .unwrap_or_default()
        } else {
            String::new()
        };

        let has_create = operations.contains(&"create".to_string());
        let has_read = operations.contains(&"read".to_string());
        let has_update = operations.contains(&"update".to_string());
        let has_delete = operations.contains(&"delete".to_string());
        let has_list = operations.contains(&"list".to_string());

        // ux-rules spec contract (issue #302): emitted only when the
        // entity's plan is active AND the list fixture path exists (the
        // blocks assert API-created fixtures against the list page).
        let list_include = dto_config
            .map(|d| d.list_include.clone())
            .unwrap_or_default();
        let list_exclude = dto_config
            .map(|d| d.list_exclude.clone())
            .unwrap_or_default();
        let ux_inputs = (has_list && has_create).then(|| {
            (
                list_include.clone(),
                list_exclude.clone(),
                fields.clone(),
                create_fields.clone(),
                operations.clone(),
                initial_state.clone(),
            )
        });
        let ux_spec = match &ux_inputs {
            Some((
                list_include,
                list_exclude,
                fields,
                create_fields,
                operations,
                initial_state,
            )) => {
                build_ux_e2e_spec(
                    db,
                    config,
                    project,
                    schema_title,
                    &domain,
                    fields,
                    create_fields,
                    list_include,
                    list_exclude,
                    operations,
                    initial_state,
                )
                .await?
            }
            None => None,
        };

        // Build the required entity-ref dependency closure (leaf-first).
        // Required FKs (e.g. case.tenant_id) are created in beforeAll; optional
        // refs are omitted from payloads and never created.
        let (entity_ref_deps, dependency_steps, dependency_errors) = build_required_dependencies(
            db,
            config,
            schema_title,
            &domain,
            &all_props,
            &create_fields,
        )
        .await;
        let has_entity_ref_deps = !entity_ref_deps.is_empty() || !dependency_errors.is_empty();

        // Resolve include test config
        let e2e_include = if let Some(ec) = entity_cfg {
            if ec.allow_include.as_ref().is_some_and(|v| !v.is_empty()) {
                let resolved = crate::api::include_path::resolve_include_paths(
                    db,
                    config,
                    &domain,
                    schema_title,
                    ec.allow_include.as_ref(),
                )
                .await?;
                resolve_e2e_include_config(
                    db,
                    config,
                    &domain,
                    schema_title,
                    &resolved,
                    has_list,
                    project,
                )
                .await?
            } else {
                None
            }
        } else {
            None
        };

        // Collect child sections for detail page testing
        let child_sections = collect_child_sections(db, schema_title, config, &domain).await?;
        let has_child_sections = !child_sections.is_empty();

        // Resolve parent info for child entities.
        // Manual config (role = "child", parent = "...") takes priority over graph detection.
        let (parent, parent_title_for_data) = {
            let stripped =
                crate::api::router::strip_suffix(schema_title, &config.defaults.type_suffix);
            let mut result = None;
            let mut parent_title_str = String::new();

            // 1. Check manual config first
            if let Some(ec) = config
                .domains
                .get(&domain)
                .and_then(|d| d.get_entity_config(schema_title))
                && ec.role.as_deref() == Some("child")
                && let Some(ref parent_title) = ec.parent
                && let Ok(Some(parent_schema)) =
                    db.get_schema_in_domain(parent_title, &domain).await
            {
                let parent_domain = if config
                    .domains
                    .get(&domain)
                    .map(|d| d.entities.contains(parent_title))
                    .unwrap_or(false)
                {
                    domain.clone()
                } else {
                    parent_schema
                        .domain
                        .clone()
                        .unwrap_or_else(|| domain.clone())
                };

                let grandparent = self
                    .resolve_grandparent(db, config, parent_title, &parent_domain)
                    .await;

                parent_title_str = parent_title.clone();
                result = Some(UiParentInfo {
                    param_name: crate::api::router::param_name_from_path_segment(
                        &resolve_path_segment_with_config(None, &parent_schema, config),
                    ),
                    domain: parent_domain,
                    path_segment: resolve_path_segment_with_config(None, &parent_schema, config),
                    module_name: parent_schema.pg_table_name.clone(),
                    entity_name: parent_schema.rust_type_name.clone(),
                    grandparent,
                });
            }

            // 2. Fall back to graph parent_candidates (only if parent is in same domain,
            //    and the entity is not explicitly/implicitly configured as root).
            let effective_role = entity_cfg
                .and_then(|ec| ec.role.as_deref())
                .unwrap_or("root");
            if result.is_none() && effective_role != "root" {
                for pc in &self.parent_candidates {
                    let child_name = crate::api::router::strip_suffix(
                        &pc.child_title,
                        &config.defaults.type_suffix,
                    );
                    if child_name == stripped {
                        let in_explicit = config
                            .domains
                            .get(&domain)
                            .map(|d| d.entities.contains(&pc.parent_title))
                            .unwrap_or(false);
                        let parent_in_domain = in_explicit
                            || db
                                .get_schema_in_domain(&pc.parent_title, &domain)
                                .await
                                .ok()
                                .flatten()
                                .and_then(|s| s.domain.as_ref().map(|d| *d == domain))
                                .unwrap_or(false);
                        if !parent_in_domain {
                            break;
                        }
                        if let Ok(Some(parent_schema)) =
                            db.get_schema_in_domain(&pc.parent_title, &domain).await
                        {
                            let parent_domain = domain.clone();

                            let grandparent = self
                                .resolve_grandparent(db, config, &pc.parent_title, &parent_domain)
                                .await;

                            parent_title_str = pc.parent_title.clone();
                            result = Some(UiParentInfo {
                                param_name: crate::api::router::param_name_from_path_segment(
                                    &resolve_path_segment_with_config(None, &parent_schema, config),
                                ),
                                domain: parent_domain,
                                path_segment: resolve_path_segment_with_config(
                                    None,
                                    &parent_schema,
                                    config,
                                ),
                                module_name: parent_schema.pg_table_name.clone(),
                                entity_name: parent_schema.rust_type_name.clone(),
                                grandparent,
                            });
                        }
                        break;
                    }
                }
            }
            (result, parent_title_str)
        };

        // Build parent entity test data for creating parent in beforeAll
        let parent_test_data_json = if parent.is_some() && !parent_title_for_data.is_empty() {
            let parent_domain = parent.as_ref().map(|p| p.domain.as_str());
            build_test_data_json(db, &parent_title_for_data, parent_domain, config).await
        } else {
            String::new()
        };

        // Build grandparent entity test data for depth-2 nesting
        let grandparent_test_data_json = if let Some(ref p) = parent {
            if let Some(ref gp) = p.grandparent {
                // Find grandparent's schema title from parent_candidates
                let parent_stripped = crate::api::router::strip_suffix(
                    &parent_title_for_data,
                    &config.defaults.type_suffix,
                );
                let mut gp_title = String::new();
                for gpc in &self.parent_candidates {
                    let gpc_child = crate::api::router::strip_suffix(
                        &gpc.child_title,
                        &config.defaults.type_suffix,
                    );
                    if gpc_child == parent_stripped {
                        gp_title = gpc.parent_title.clone();
                        break;
                    }
                }
                if !gp_title.is_empty() {
                    build_test_data_json(db, &gp_title, Some(&gp.domain), config).await
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        // POM inputs (issue #316): spec-infra gated — populated whenever
        // ANY spec file is emitted (any-op entities), NOT ux-flag gated.
        // The ux plan mirror is the SAME struct both sides render from:
        // the `.ux.test.ts` spec reads `ux_spec`, the page class reads
        // `pom.ux` — the deterministic builder produces both instances,
        // so they can never drift.
        let any_op = has_list || has_create || has_read || has_update || has_delete;
        let mut pom = None;
        if any_op {
            let pom_ux = match &ux_inputs {
                Some((
                    list_include,
                    list_exclude,
                    fields,
                    create_fields,
                    operations,
                    initial_state,
                )) => {
                    build_ux_e2e_spec(
                        db,
                        config,
                        project,
                        schema_title,
                        &domain,
                        fields,
                        create_fields,
                        list_include,
                        list_exclude,
                        operations,
                        initial_state,
                    )
                    .await?
                }
                None => None,
            };
            pom = Some(PomCtx::build(
                &module_name,
                &domain,
                &path_segment,
                &crate::api::router::param_name_from_path_segment(&path_segment),
                parent.as_ref(),
                &create_fields,
                has_workflow,
                &workflow_states,
                &initial_state,
                &terminal_states,
                pom_ux,
            ));
        }

        let ctx = UiE2eTestContext {
            entity_name,
            entity_label,
            module_name: module_name.clone(),
            domain: domain.clone(),
            path_segment: path_segment.clone(),
            has_create,
            has_read,
            has_update,
            has_delete,
            has_list,
            has_workflow,
            workflow_states,
            initial_state,
            terminal_states,
            fields,
            create_fields,
            required_create_fields,
            update_fields,
            first_list_column,
            has_fts,
            fts_search_field,
            entity_ref_deps,
            dependency_steps,
            dependency_errors,
            has_entity_ref_deps,
            convention_ref_gaps,
            child_sections,
            has_child_sections,
            param_name: crate::api::router::param_name_from_path_segment(&path_segment),
            parent,
            parent_test_data_json,
            grandparent_test_data_json,
            e2e_include,
            ux_spec,
            pom,
        };

        let tests_dir = self
            .output_dir
            .join("ui")
            .join("tests")
            .join("generated")
            .join(&domain);

        let mut files = Vec::new();

        // Shared POM kernel — entity-independent content, emitted ONCE per
        // generation run (first spec-emitting entity wins the flag). Always
        // rewritten, stable bytes, imported from every `{domain}/` depth.
        if ctx.pom.is_some()
            && self
                .kernel_emitted
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            let content =
                render_template_with_project(tera, "ui/test/_pom_kernel.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: self
                    .output_dir
                    .join("ui")
                    .join("tests")
                    .join("generated")
                    .join("_support")
                    .join("pom.ts"),
                content,
            });
        }

        // Per-entity page class (`{seg}.page.ts`) — every spec family
        // drives UI interaction through it.
        if ctx.pom.is_some() {
            let content = render_template_with_project(tera, "ui/test/page.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: tests_dir.join(format!("{}.page.ts", path_segment)),
                content,
            });
        }

        // CRUD test
        if has_list || has_create || has_read || has_update || has_delete {
            let content =
                render_template_with_project(tera, "ui/test/crud.test.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: tests_dir.join(format!("{}.api.crud.test.ts", path_segment)),
                content,
            });
        }

        // Validation test
        if has_create {
            let content =
                render_template_with_project(tera, "ui/test/validation.test.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: tests_dir.join(format!("{}.validation.test.ts", path_segment)),
                content,
            });
        }

        // Workflow test
        if has_workflow {
            let content =
                render_template_with_project(tera, "ui/test/workflow.test.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: tests_dir.join(format!("{}.workflow.test.ts", path_segment)),
                content,
            });
        }

        // Persona-based tests
        if has_list || has_create || has_read || has_update || has_delete {
            let content =
                render_template_with_project(tera, "ui/test/owner_crud.test.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: tests_dir.join(format!("{}.owner.crud.test.ts", path_segment)),
                content,
            });

            let content = render_template_with_project(
                tera,
                "ui/test/employee_view.test.tera",
                &ctx,
                project,
            )?;
            files.push(GeneratedFile {
                path: tests_dir.join(format!("{}.employee.view.test.ts", path_segment)),
                content,
            });

            let content = render_template_with_project(
                tera,
                "ui/test/manager_team.test.tera",
                &ctx,
                project,
            )?;
            files.push(GeneratedFile {
                path: tests_dir.join(format!("{}.manager.team.test.ts", path_segment)),
                content,
            });

            let content =
                render_template_with_project(tera, "ui/test/isolation.test.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: tests_dir.join(format!("{}.isolation.test.ts", path_segment)),
                content,
            });
        }

        // Search tests (only for FTS-enabled entities)
        if has_fts && has_list {
            let content =
                render_template_with_project(tera, "ui/test/search.test.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: tests_dir.join(format!("{}.search.test.ts", path_segment)),
                content,
            });

            let content = render_template_with_project(
                tera,
                "ui/test/search_isolation.test.tera",
                &ctx,
                project,
            )?;
            files.push(GeneratedFile {
                path: tests_dir.join(format!("{}.search.isolation.test.ts", path_segment)),
                content,
            });
        }

        // Include test
        if ctx.e2e_include.is_some() && has_read {
            let content =
                render_template_with_project(tera, "ui/test/include.test.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: tests_dir.join(format!("{}.include.test.ts", path_segment)),
                content,
            });
        }

        // UX list-rendering spec (issue #302) — first-class alongside the
        // crud/validation/workflow specs, gated per feature.
        if ctx.ux_spec.is_some() {
            let content =
                render_template_with_project(tera, "ui/test/ux.test.tera", &ctx, project)?;
            files.push(GeneratedFile {
                path: tests_dir.join(format!("{}.ux.test.ts", path_segment)),
                content,
            });
        }

        Ok(files)
    }
}
