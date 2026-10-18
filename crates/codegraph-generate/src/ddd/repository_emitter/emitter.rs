use codegraph_config::DomainConfig;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::{
    AuditPolicy, DeletionPropagation, PolicyKind, SoftDeleteMarker, SoftDeleteVisibility,
};
use codegraph_type_contracts::RefClassificationKind;

use crate::ProjectConfig;
use crate::api::api_model::resolve_entity_operations;
use crate::api::include_path::{ResolvedIncludePath, resolve_include_paths_gated};
use crate::code_writer::{CodeWriter, wln};
use crate::error::Result;
use crate::filter_fields::{resolve_filter_fields, resolve_nested_filter_fields};
use crate::type_registry;

use super::child::flatten_child_tables;
use super::context::{
    ClassificationContext, build_columns_and_children, resolve_worker_detail_joins,
};
use super::{ChildTableInfo, EntityTree, TreeColumn, TreeIncludeResolved, UxSortColumn};

/// Emits repository implementation Rust code by walking the entity's graph subtree.
pub struct RepositoryImplEmitter;

/// Map the ux sort plan's fields onto the entity's direct columns
/// (issue #306). Flag off ⇒ no plan, empty mapping. Returns whether the
/// plan is non-empty at all alongside the mapped columns: the trait, the
/// query handler and the cornucopia adapter all carry the `sort` param
/// whenever the plan is non-empty, so the SeaORM impl must too — even
/// when no plan field maps onto a direct column (the param is then
/// accepted and ignored, matching the cornucopia adapter's contract).
/// Mapped columns feed the emitted match arms and stay compile-clean.
async fn sort_columns_for_tree(
    tree: &EntityTree,
    project: &ProjectConfig,
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    schema_title: &str,
    domain: &str,
) -> Result<(bool, Vec<UxSortColumn>)> {
    let plan =
        crate::ux::sort::resolve_ux_sort_plan(db, config, project, schema_title, domain).await?;
    let has_sort_plan = !plan.is_empty();
    let sort_columns: Vec<UxSortColumn> = plan
        .fields
        .iter()
        .filter_map(|field| {
            tree.direct_columns.iter().find_map(|col| {
                let bare = col.field_name.strip_prefix("r#").unwrap_or(&col.field_name);
                let matches = col
                    .dto_field_name
                    .as_deref()
                    .unwrap_or(bare)
                    .eq_ignore_ascii_case(field)
                    || bare.eq_ignore_ascii_case(field);
                matches.then(|| UxSortColumn {
                    key: field.clone(),
                    column: col.pg_column_name.clone(),
                })
            })
        })
        .collect();
    Ok((has_sort_plan, sort_columns))
}

impl RepositoryImplEmitter {
    /// Resolve whether `find_tree` returns JOINed `serde_json::Value` rows
    /// (tree_include configured AND resolvable) versus typed `{Entity}Response`
    /// rows. The repository trait generator calls this so the trait's
    /// `find_tree` return type stays in sync with the emitted implementation,
    /// which derives the same boolean from `!tree.tree_include.is_empty()`.
    pub async fn resolve_tree_include(
        &self,
        db: &dyn GraphQuerier,
        schema_title: &str,
        domain: &str,
        config: &DomainConfig,
        parent_ref: Option<&str>,
    ) -> Result<bool> {
        let tree = self
            .query_entity_tree(db, schema_title, domain, config, parent_ref)
            .await?;
        Ok(!tree.tree_include.is_empty())
    }

    // Issue #167 Phase 1 will restructure this API (resolver + renderer
    // split), at which point the argument list collapses into the model.
    #[allow(clippy::too_many_arguments)]
    pub async fn emit(
        &self,
        db: &dyn GraphQuerier,
        schema_title: &str,
        domain: &str,
        config: &DomainConfig,
        parent_ref: Option<&str>,
        include_paths: &[ResolvedIncludePath],
        project: &ProjectConfig,
    ) -> Result<String> {
        let tree = self
            .query_entity_tree(db, schema_title, domain, config, parent_ref)
            .await?;
        // Issue #268 (namespace_layout): namespaced entities reference
        // their entity module through the namespace path —
        // `crate::entity::cdm::base::datetime::{module}`. Flat (gate off /
        // namespace-less) keeps `{schema}_{table}` byte-identical; every
        // emitted `crate::entity::{}::…` site formats this one field.
        let tree =
            if let Some(ns) = crate::namespace_rust_prefix(tree.namespace.as_deref(), project) {
                let mut tree = tree;
                tree.entity_module = format!("{ns}::{}", tree.entity_module);
                tree
            } else {
                tree
            };
        let mut code = CodeWriter::new();

        // Cross-generator contract: the handler's hydration block calls
        // `repo.fetch_{alias}_for_{module}(...)` for every include path it
        // resolves, so the emitted impl must provide exactly those methods.
        // Pipeline callers pass their pre-resolved paths; when a caller
        // supplies none, resolve them here with the same entity gate the
        // handler applies. Trusting an empty caller-supplied list verbatim
        // silently stripped the hydration methods from the emitted impl and
        // broke the generated app's compilation (E0599 on
        // `fetch_{alias}_for_{module}`) whenever `emit` was invoked outside
        // the pipeline without paths.
        let include_paths: Vec<ResolvedIncludePath> = if include_paths.is_empty() {
            resolve_include_paths_gated(
                db,
                config,
                domain,
                schema_title,
                project.is_workers_topology(),
            )
            .await?
        } else {
            include_paths.to_vec()
        };

        // Deduplicate include paths by alias to prevent duplicate struct field
        // emissions (auto-discover can produce the same child entity through
        // multiple FK relationships).
        let include_paths = {
            let mut seen = std::collections::HashSet::new();
            include_paths
                .iter()
                .filter(|path| seen.insert(path.alias.clone()))
                .cloned()
                .collect::<Vec<_>>()
        };

        self.emit_header(&tree, &include_paths, project, &mut code);
        if tree.has_create {
            self.emit_create_fn(&tree, &mut code);
        }
        // find_by_id is only referenced when the query handler emits it
        // (create bulk path, or read without a parent) or when FTS/embedding
        // search hydrates results through it.
        let needs_find_by_id = tree.has_create
            || (tree.has_read && tree.parent_ref.is_none())
            || tree.has_fts
            || tree.has_embeddings;
        if needs_find_by_id {
            self.emit_find_by_id_fn(&tree, &mut code);
        }
        if tree.parent_ref.is_some() {
            self.emit_find_by_id_scoped_fn(&tree, &mut code);
        }
        if tree.has_update {
            self.emit_update_fn(&tree, &mut code);
        }
        if tree.has_delete {
            self.emit_delete_fn(&tree, &mut code);
        }
        // Issue #306: map the plan's sortable fields onto the entity's
        // direct columns; fields without a direct column (synthetic or
        // expanded slots) are dropped so the emitted match arms stay
        // compile-clean.
        let (has_sort_plan, sort_columns) =
            sort_columns_for_tree(&tree, project, db, config, schema_title, domain).await?;
        if tree.has_list {
            self.emit_list_fn(&tree, has_sort_plan, &sort_columns, &mut code);
        }
        if tree.has_fts {
            self.emit_search_fn(&tree, &mut code);
        }
        if tree.has_embeddings {
            self.emit_semantic_search_fn(&tree, &mut code);
        }
        if tree.hierarchy_field.is_some() {
            self.emit_find_tree_fn(&tree, &mut code);
        }
        // Declared `.ddd` design finders (issue #449) — nothing renders
        // when the entity's design carries none.
        self.emit_design_finders(&tree, &mut code);
        self.emit_footer(&mut code);

        // Resolve scalar fields for each include path segment using resolve_field()
        // so that both the DTO side and entity Model side use rust_field_name.
        // EntityReference fields get _id appended; CodelistReference fields get _code stripped.
        let all_props = db.list_all_properties().await?;
        let mut include_segment_dto_fields: Vec<Vec<Vec<String>>> = Vec::new();
        let mut include_segment_col_fields: Vec<Vec<Vec<String>>> = Vec::new();
        let mut include_segment_is_structured: Vec<Vec<Vec<bool>>> = Vec::new();
        let mut include_segment_is_codelist: Vec<Vec<Vec<bool>>> = Vec::new();
        let mut include_segment_dto_rust_types: Vec<Vec<Vec<Option<String>>>> = Vec::new();
        let mut include_segment_is_nullable: Vec<Vec<Vec<bool>>> = Vec::new();
        let mut include_segment_col_rust_types: Vec<Vec<Vec<String>>> = Vec::new();
        for path in &include_paths {
            let mut per_seg_dto: Vec<Vec<String>> = Vec::new();
            let mut per_seg_col: Vec<Vec<String>> = Vec::new();
            let mut per_seg_is_structured: Vec<Vec<bool>> = Vec::new();
            let mut per_seg_is_codelist: Vec<Vec<bool>> = Vec::new();
            let mut per_seg_dto_rust_types: Vec<Vec<Option<String>>> = Vec::new();
            let mut per_seg_is_nullable: Vec<Vec<bool>> = Vec::new();
            let mut per_seg_col_rust_types: Vec<Vec<String>> = Vec::new();
            for (seg_idx, seg) in path.segments.iter().enumerate() {
                // Use schema_title directly — include_path.rs already resolves
                // it to the canonical title for each segment. The fallback graph
                // query could return the wrong properties from a shared parent
                // when schema inheritance is involved.
                // Query consumed fields once per segment — fields consumed by
                // composite range columns (e.g. start/end) that don't exist as
                // direct columns on the entity Model.
                let consumed_fields: std::collections::HashSet<String> = db
                    .get_consumed_fields(&seg.schema_title)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(p, _)| p.name)
                    .collect();
                let mut dto_fields: Vec<String> = Vec::new();
                let mut col_fields: Vec<String> = Vec::new();
                let mut is_structured: Vec<bool> = Vec::new();
                let mut is_codelist: Vec<bool> = Vec::new();
                let mut dto_rust_types: Vec<Option<String>> = Vec::new();
                let mut is_nullable: Vec<bool> = Vec::new();
                let mut col_rust_types: Vec<String> = Vec::new();
                // When the segment has a child table override (VO→entity), use
                // the VO's properties instead of the entity's properties.
                let props_key = seg
                    .child_table_override
                    .as_ref()
                    .map_or(&seg.schema_title, |over| &over.vo_title);
                if let Some(props) = all_props.get(props_key) {
                    let mut seen = std::collections::HashSet::new();
                    for prop in props {
                        // Skip entity reference properties that match the next
                        // segment's module_name — the nested field (e.g.,
                        // {position: leaf_dto}) is added separately in the
                        // dot-fetch method.  Matches DTO builder's skip at
                        // build_include_dtos (dto.rs:1127-1131).
                        if seg_idx < path.segments.len() - 1 {
                            let next_module = &path.segments[seg_idx + 1].module_name;
                            // Skip entity reference FK columns that match a leaf
                            // segment — the combined DTO excludes these since the
                            // enriched type has a nested field for the leaf entity
                            // instead. Match both raw rust_field_name and with _id
                            // suffix, since entity generators may differ in naming.
                            if matches!(
                                prop.effective_kind(),
                                Some(RefClassificationKind::EntityReference)
                            ) && (prop.rust_field_name == *next_module
                                || prop.rust_field_name == format!("{}_id", next_module))
                            {
                                continue;
                            }
                        }
                        // Skip properties that don't map to database columns —
                        // the entity Model won't have them as Rust fields.
                        if prop.pg_column_name.is_empty() {
                            continue;
                        }
                        // Skip array properties — the entity generator expands
                        // these into separate child tables, not direct Model fields.
                        if prop.is_array {
                            continue;
                        }
                        if consumed_fields.contains(&prop.name) {
                            continue;
                        }
                        // Skip composite/media wrappers — expanded into sub-columns.
                        if matches!(
                            prop.effective_kind(),
                            Some(
                                RefClassificationKind::CompositeWrapper
                                    | RefClassificationKind::MediaWrapper
                            )
                        ) {
                            continue;
                        }
                        if prop.rust_field_name == "id"
                            || prop.rust_field_name == "created_at"
                            || prop.rust_field_name == "updated_at"
                        {
                            continue;
                        }
                        // Only include properties that produce a single direct
                        // column on the entity Model — matching the entity
                        // generator's match arm logic.
                        match prop.effective_kind() {
                            Some(
                                RefClassificationKind::PrimitiveWrapper
                                | RefClassificationKind::StructuredWrapper
                                | RefClassificationKind::CodelistReference
                                | RefClassificationKind::CodelistCheck
                                | RefClassificationKind::EntityReference
                                | RefClassificationKind::RangeWrapper
                                | RefClassificationKind::InlineEnum,
                            ) => {}
                            _ => continue,
                        }
                        // Skip properties whose direct $ref target is a
                        // force_value_object — the entity generator skips
                        // composed/inherited properties from allOf chains,
                        // so the Model doesn't have these columns.
                        if prop.effective_kind().is_some()
                            && let Ok(Some(target)) = db
                                .get_property_ref_target(&prop.name, &seg.schema_title)
                                .await
                            && (!target.is_entity || target.pg_table_name.is_empty())
                        {
                            continue;
                        }
                        let fd = codegraph_core::types::resolve_field(prop);
                        // Deduplicate by rust_field_name — list_all_properties()
                        // can return duplicate entries from interface inheritance.
                        if seen.insert(fd.rust_field_name.clone()) {
                            // DTO side uses resolve_field().rust_field_name (with _id for
                            // entity refs) — consistent with both base entity DTO
                            // (build_dto_context) and include-path compound DTO
                            // (build_include_dtos).
                            dto_fields.push(fd.rust_field_name.clone());
                            // Entity Model side uses column_name so it matches
                            // config-specified FK column names (e.g. person_type_id).
                            col_fields.push(fd.column_name.clone());
                            // Type conversion flags for emit_field_assignments_typed.
                            is_structured.push(matches!(
                                prop.effective_kind(),
                                Some(RefClassificationKind::StructuredWrapper)
                            ));
                            is_codelist.push(matches!(
                                prop.effective_kind(),
                                Some(RefClassificationKind::CodelistReference)
                                    | Some(RefClassificationKind::CodelistCheck)
                            ));
                            // Entity model column rust type — the same rule the
                            // entity generator (entity.rs) and the tree context
                            // (context.rs) apply: FK columns are Uuid, codelists
                            // String, structured wrappers JSONB; everything else
                            // keeps the graph-declared type.
                            let col_rust_type = match prop.effective_kind() {
                                Some(RefClassificationKind::EntityReference) => "Uuid".to_string(),
                                Some(
                                    RefClassificationKind::CodelistReference
                                    | RefClassificationKind::CodelistCheck,
                                ) => "String".to_string(),
                                Some(RefClassificationKind::StructuredWrapper) => {
                                    "serde_json::Value".to_string()
                                }
                                _ => prop.rust_field_type.clone(),
                            };
                            // The DTO field type the Response template renders:
                            // entity refs are uuid::Uuid, codelists the enum name,
                            // everything else the graph-declared type (bare —
                            // nullability rides is_nullable). Recorded only when it
                            // differs from the column type so the hydrator adapts
                            // (issue #459) without touching matching-type output.
                            let dto_rust_type = match prop.effective_kind() {
                                Some(
                                    RefClassificationKind::CodelistReference
                                    | RefClassificationKind::CodelistCheck,
                                ) => crate::ddd::dto::codelist_enum_name_from_ref(&prop.ref_target),
                                Some(RefClassificationKind::EntityReference) => {
                                    Some("Uuid".to_string())
                                }
                                _ => {
                                    let raw = prop.rust_field_type.as_str();
                                    let bare = raw
                                        .strip_prefix("Option<")
                                        .and_then(|r| r.strip_suffix('>'))
                                        .unwrap_or(raw);
                                    (bare != col_rust_type).then(|| bare.to_string())
                                }
                            };
                            dto_rust_types.push(dto_rust_type);
                            is_nullable.push(prop.is_nullable);
                            col_rust_types.push(col_rust_type);
                        }
                    }
                }
                per_seg_dto.push(dto_fields);
                per_seg_col.push(col_fields);
                per_seg_is_structured.push(is_structured);
                per_seg_is_codelist.push(is_codelist);
                per_seg_dto_rust_types.push(dto_rust_types);
                per_seg_is_nullable.push(is_nullable);
                per_seg_col_rust_types.push(col_rust_types);
            }
            include_segment_dto_fields.push(per_seg_dto);
            include_segment_col_fields.push(per_seg_col);
            include_segment_is_structured.push(per_seg_is_structured);
            include_segment_is_codelist.push(per_seg_is_codelist);
            include_segment_dto_rust_types.push(per_seg_dto_rust_types);
            include_segment_is_nullable.push(per_seg_is_nullable);
            include_segment_col_rust_types.push(per_seg_col_rust_types);
        }

        if !include_paths.is_empty() {
            // Add import statements for cross-entity types referenced by include paths.
            // These types (e.g. PersonResponse) live in other entity modules and need
            // use crate::domain::{domain}::{module}::dto_response::TypeName imports.
            // Issue #268: under namespace_layout the CALLER's own module
            // path runs through the namespace segments (self-import
            // detection must agree with dto.rs's registered paths).
            let caller_ns = crate::namespace_rust_prefix(tree.namespace.as_deref(), project);
            let caller_base: Vec<String> = match &caller_ns {
                Some(ns) => format!("crate::domain::{ns}::{}", tree.module_name)
                    .split("::")
                    .map(str::to_string)
                    .collect(),
                None => vec![
                    "crate".into(),
                    "domain".into(),
                    domain.into(),
                    tree.module_name.clone(),
                ],
            };
            let caller_base = {
                let mut base = caller_base;
                base.push("repository_impl".into());
                base
            };

            // Build the target entity trees so include-fetch responses hydrate
            // the target's child tables (e.g. person.name) instead of emitting
            // all-None sub-objects. Targets whose tree cannot be built simply
            // skip child hydration. The trees also contribute their (nested)
            // child response struct names to the import resolution below — the
            // include-fetch child reads reference them.
            let mut include_target_trees: Vec<Option<EntityTree>> = Vec::new();
            for path in &include_paths {
                let last = path.segments.last().unwrap();
                let ttree = self
                    .query_entity_tree(db, &last.schema_title, &last.domain, config, None)
                    .await
                    .ok();
                include_target_trees.push(ttree);
            }

            let mut include_type_names: Vec<String> = Vec::new();
            for path in &include_paths {
                include_type_names.push(path.response_rust_type.clone());
                if path.segments.len() > 1
                    && let Some(last_seg) = path.segments.last()
                {
                    include_type_names.push(format!("{}Response", last_seg.entity_name));
                }
            }
            // Deduplicate while preserving order.
            let mut seen = std::collections::HashSet::new();
            include_type_names.retain(|n| seen.insert(n.clone()));
            // Pre-register each include target's `{Entity}Response` with the
            // exact module path the target's own DTO generator uses. Junction
            // linked entities impose no generation-order constraint, so this
            // emitter can run before the target's DTO generator registered
            // the type — and the import below was nondeterministically
            // dropped depending on emission order. Registration is
            // idempotent for identical paths (register_type ignores
            // conflicts), so a warm registry wins and this only fills gaps.
            for (idx, path) in include_paths.iter().enumerate() {
                let Some(last_seg) = path.segments.last() else {
                    continue;
                };
                let ns = crate::namespace_rust_prefix(
                    include_target_trees
                        .get(idx)
                        .and_then(|t| t.as_ref())
                        .and_then(|t| t.namespace.as_deref()),
                    project,
                );
                let mut module: Vec<String> = match ns {
                    Some(ns) => format!("crate::domain::{ns}")
                        .split("::")
                        .map(str::to_string)
                        .collect(),
                    None => vec!["crate".into(), "domain".into(), last_seg.domain.clone()],
                };
                module.push(last_seg.module_name.clone());
                module.push("dto_response".into());
                type_registry::register_type(&format!("{}Response", last_seg.entity_name), module);
            }
            let imports = type_registry::resolve_imports(&include_type_names, &caller_base);
            let mut resolved_names: std::collections::HashSet<String> = imports
                .iter()
                .map(|u| {
                    u.rsplit("::")
                        .next()
                        .unwrap_or(u)
                        .trim_end_matches(';')
                        .rsplit(' ')
                        .next()
                        .unwrap_or(u)
                        .to_string()
                })
                .collect();
            for import in &imports {
                wln!(code, "{}", import);
            }
            // Entity-native include targets (e.g. `CompanyServiceResponse`) may
            // not be registered yet when repositories emit ahead of the DTO
            // generator's type registration; the registry silently resolves
            // nothing and the repository fails with E0425. The include path
            // itself knows the target module — emit the direct import for any
            // single-segment entity response the registry could not resolve.
            // Names already imported via the caller/target child walks
            // (`use super::dto_response::…` and the target-tree block below)
            // are skipped to keep E0252 away.
            let mut already_imported_children: std::collections::HashSet<String> =
                flatten_child_tables(&tree.child_tables)
                    .into_iter()
                    .map(|c| format!("{}Response", c.struct_name))
                    .collect();
            for ttree in include_target_trees.iter().flatten() {
                for child in flatten_child_tables(&ttree.child_tables) {
                    already_imported_children.insert(format!("{}Response", child.struct_name));
                }
            }
            for path in &include_paths {
                if path.segments.len() != 1 {
                    continue;
                }
                let resp = &path.response_rust_type;
                if already_imported_children.contains(resp) {
                    continue;
                }
                // Same-module targets (a VO child of this very entity) are
                // already imported via `super::dto_response`; compare against
                // the caller's real module path — tree.module_name can be the
                // plural table name while the segment uses the schema module.
                let target_mod = format!(
                    "crate::domain::{}::{}",
                    path.segments[0].domain, path.segments[0].module_name
                );
                let caller_mod = caller_base
                    .strip_suffix(&["repository_impl".to_string()][..])
                    .map(|b| b.join("::"))
                    .unwrap_or_default();
                let is_self_target = path.segments[0].domain == domain
                    // Compare module names (pg_table_name), not rust type
                    // names: the two can diverge (rust_type_name vs schema
                    // title) while naming the same module, and the header
                    // already imports self-module dto types wholesale via
                    // `use super::dto_response::…`.
                    && path.segments[0].module_name == tree.module_name;
                if resolved_names.insert(resp.clone())
                    && resp.ends_with("Response")
                    && !is_self_target
                    && target_mod != caller_mod
                {
                    wln!(code, "use {target_mod}::dto_response::{resp};");
                }
            }
            // Also add direct imports for enriched types from dto_included module.
            // These types (e.g. DeploymentCombinedResponse) are generated in the
            // current entity's dto_included.rs but may not yet be registered in the
            // type registry when the repository emitter runs (DTO generator runs later).
            // Skip types that resolve_imports already imported above — emitting both
            // `use crate::domain::..::dto_included::Type;` and
            // `use super::dto_included::Type;` for the same module is an E0252
            // "defined multiple times" compile error.
            let mut seen_enriched = std::collections::HashSet::new();
            for path in &include_paths {
                if path.segments.len() > 1 && seen_enriched.insert(path.response_rust_type.clone())
                {
                    let already_imported = !type_registry::resolve_imports(
                        std::slice::from_ref(&path.response_rust_type),
                        &caller_base,
                    )
                    .is_empty();
                    if already_imported {
                        continue;
                    }
                    wln!(
                        code,
                        "use super::dto_included::{};",
                        path.response_rust_type
                    );
                }
            }

            // Child-table response structs are all re-exported through the
            // TARGET entity's dto_response module (the domain-types crate
            // emits every nested level there). The include-fetch child
            // hydration references them; without these imports the repository
            // fails to compile wherever a target tree nests deeply.
            {
                let mut seen_child_imports = std::collections::HashSet::new();
                let mut child_import_lines: Vec<String> = Vec::new();
                fn walk_child_imports(
                    child: &ChildTableInfo,
                    base: &str,
                    seen: &mut std::collections::HashSet<String>,
                    out: &mut Vec<String>,
                ) {
                    if seen.insert(child.struct_name.clone()) {
                        // base already ends at the target entity's module —
                        // every nested level is re-exported by its dto_response.
                        // The hydration reads emit {Struct}Response types.
                        out.push(format!(
                            "use {}dto_response::{}Response;",
                            base, child.struct_name
                        ));
                    }
                    for nested in &child.child_tables {
                        walk_child_imports(nested, base, seen, out);
                    }
                }
                for (idx, path) in include_paths.iter().enumerate() {
                    let Some(Some(ttree)) = include_target_trees.get(idx) else {
                        continue;
                    };
                    if ttree.child_tables.is_empty() {
                        continue;
                    }
                    let last = path.segments.last().unwrap();
                    // A self-targeting include path (the entity's own tree,
                    // e.g. a self-referencing FK) already had every child
                    // imported via `use super::dto_response::…` in the
                    // header — emitting the full-path form again is an E0252
                    // duplicate. Compare entity identity by domain and
                    // module name: formatted paths can diverge under
                    // namespace_layout while naming the same module.
                    if last.domain == domain && last.module_name == tree.module_name {
                        continue;
                    }
                    // Issue #268: namespace-derived target module path
                    // under namespace_layout (must match dto.rs's
                    // registered module paths for the target entity).
                    let base = match crate::namespace_rust_prefix(
                        include_target_trees
                            .get(idx)
                            .and_then(|t| t.as_ref())
                            .and_then(|t| t.namespace.as_deref()),
                        project,
                    ) {
                        Some(ns) => format!("crate::domain::{ns}::{}::", last.module_name),
                        None => {
                            format!("crate::domain::{}::{}::", last.domain, last.module_name)
                        }
                    };
                    // A self-targeting path whose segment entity_name diverged
                    // (rust_type_name vs schema title) still resolves to this
                    // very module — the header's `use super::dto_response::…`
                    // already imports everything; emitting the full-path form
                    // again is an E0252 duplicate.
                    let own_base =
                        match crate::namespace_rust_prefix(tree.namespace.as_deref(), project) {
                            Some(ns) => format!("crate::domain::{ns}::{}::", tree.module_name),
                            None => {
                                format!("crate::domain::{}::{}::", domain, tree.module_name)
                            }
                        };
                    if base == own_base {
                        continue;
                    }
                    for child in &ttree.child_tables {
                        walk_child_imports(
                            child,
                            &base,
                            &mut seen_child_imports,
                            &mut child_import_lines,
                        );
                    }
                }
                for line in child_import_lines {
                    wln!(code, "{}", line);
                }
            }

            wln!(code);
            wln!(code, "impl {}RepositoryImpl {{", tree.entity_name);
            self.emit_include_fetch_methods(
                &tree,
                &mut code,
                &include_paths,
                &include_target_trees,
                &include_segment_dto_fields,
                &include_segment_col_fields,
                &include_segment_is_structured,
                &include_segment_is_codelist,
                &include_segment_dto_rust_types,
                &include_segment_is_nullable,
                &include_segment_col_rust_types,
            );
            wln!(code, "}}");
        }

        Ok(code.into_string())
    }

    pub async fn query_entity_tree(
        &self,
        db: &dyn GraphQuerier,
        schema_title: &str,
        domain: &str,
        config: &DomainConfig,
        parent_ref: Option<&str>,
    ) -> Result<EntityTree> {
        let schema = db
            .get_schema_in_domain(schema_title, domain)
            .await?
            .ok_or_else(|| crate::error::Error::SchemaNotFound(schema_title.into()))?;

        let entity_name = schema.rust_type_name.clone();
        let module_name = schema.pg_table_name.clone();
        let schema_name = domain.to_string();

        // DDD design surface (issue #449): a design covering this schema
        // title overrides the operations source, the FTS surface, the
        // filter fields, and the audit fallback for the emitted impl too —
        // it must agree with the repository trait byte-for-byte. Empty
        // surface → schema-derived values, byte-identical.
        let design_surface = crate::ddd::design::DddDesignSurface::from_graph(db).await?;
        design_surface.warn_stereotype_mismatch(schema_title, schema.is_entity);
        let design = design_surface.design_for(schema_title);
        let design_search = design_surface.search_for(schema_title);

        // Determine enabled operations
        let mut operations = resolve_entity_operations(db, config, domain, &entity_name).await;
        if let Some(design_ops) = design_surface.operations_for(schema_title) {
            operations = design_ops;
        }
        let has_create = operations.contains(&"create".to_string());
        let has_read = operations.contains(&"read".to_string());
        let has_update = operations.contains(&"update".to_string());
        let has_delete = operations.contains(&"delete".to_string());
        let has_list = operations.contains(&"list".to_string());
        let entity_cfg = config
            .domains
            .get(domain)
            .and_then(|d| d.get_entity_config(schema_title));
        let has_workflow = entity_cfg
            .and_then(|ec| ec.workflow.as_ref())
            .map(|wf| wf.generate_action_endpoints)
            .unwrap_or(false);

        let policies = db.get_policies_for_schema(schema_title).await?;
        let has_audit_policy = policies
            .iter()
            .any(|p| matches!(p.kind, PolicyKind::Audit(_)));
        let soft_delete_policy = policies.iter().find_map(|p| {
            if let PolicyKind::SoftDelete(ref sd) = p.kind {
                Some(sd.clone())
            } else {
                None
            }
        });
        let audit_policy: Option<AuditPolicy> = policies.iter().find_map(|p| {
            if let PolicyKind::Audit(ref a) = p.kind {
                Some(a.clone())
            } else {
                None
            }
        });

        // Append-only snapshot semantics (issue #284) — same inference as
        // db/ddl.rs: explicit entity-config flag, or effective operations
        // exclude update AND delete. Append-only tables carry no updated_at
        // and no audit columns, so repositories must not reference them.
        // A design override feeds its own mapped operations into the
        // inference (has_update/has_delete above are already overridden).
        let append_only = entity_cfg.as_ref().is_some_and(|ec| ec.is_append_only())
            || !(has_update || has_delete);

        // Issue #449: when a design exists for this title, `flags.auditable`
        // WINS over the policy/domains.toml fallback ordering.
        let is_auditable = (if let Some(design) = design {
            design.flags.auditable
        } else if has_audit_policy {
            audit_policy
                .as_ref()
                .map(|a| a.track_deleted)
                .unwrap_or(false)
        } else {
            config
                .domains
                .get(domain)
                .and_then(|d| d.auditable)
                .unwrap_or(true)
        }) && !append_only;

        let soft_delete_visibility = soft_delete_policy
            .as_ref()
            .map(|sd| match sd.visibility {
                SoftDeleteVisibility::ExcludeByDefault => "exclude_by_default".to_string(),
                SoftDeleteVisibility::IncludeByDefault => "include_by_default".to_string(),
                SoftDeleteVisibility::ExplicitOnly => "explicit_only".to_string(),
            })
            .unwrap_or_else(|| "exclude_by_default".to_string());

        let soft_delete_column = soft_delete_policy.as_ref().map(|sd| match &sd.marker {
            SoftDeleteMarker::Timestamp(name)
            | SoftDeleteMarker::Boolean(name)
            | SoftDeleteMarker::Status(name) => name.clone(),
        });

        let soft_delete_cascade = soft_delete_policy
            .as_ref()
            .map(|sd| match sd.cascade {
                DeletionPropagation::Restrict => "restrict".to_string(),
                DeletionPropagation::Cascade => "cascade".to_string(),
                DeletionPropagation::SoftCascade => "soft_cascade".to_string(),
                DeletionPropagation::Ignore => "ignore".to_string(),
            })
            .unwrap_or_else(|| "restrict".to_string());

        let track_updated_user = audit_policy
            .as_ref()
            .map(|a| a.track_updated)
            .unwrap_or(false);
        let track_deleted_user = audit_policy
            .as_ref()
            .map(|a| a.track_deleted)
            .unwrap_or(false);

        // Workflow-managed fields are excluded from create/update DTOs but
        // included in response DTOs. Mark them so the repository can include
        // them in reads but skip them in create/update writes.
        let mut workflow_managed = std::collections::HashSet::new();
        if let Some(wf) = entity_cfg.and_then(|ec| ec.workflow.as_ref()) {
            workflow_managed.insert(codegraph_naming::to_snake_case(&wf.status_field));
            if let Some(ref af) = wf.approval_status_field {
                workflow_managed.insert(codegraph_naming::to_snake_case(af));
            }
        }

        let all_props = db.get_properties(schema_title).await?;
        let mut props = {
            let mut seen = std::collections::HashSet::new();
            all_props
                .into_iter()
                .filter(|p| seen.insert(p.rust_field_name.clone()))
                .collect::<Vec<_>>()
        };
        // For codelist entities with no graph properties (enum-only JSON schema),
        // inject the three columns created by the codelist DDL template.
        codegraph_core::types::inject_codelist_properties(&mut props, schema.is_codelist, domain);

        // Consumed fields from composite range collapsing — skip these in all operations
        let consumed_fields: std::collections::HashSet<String> = db
            .get_consumed_fields(schema_title)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(prop, _role)| prop.name)
            .collect();

        // Composite range: collapsed start/end → single range column
        let composite_range = db.get_composite_range(schema_title).await.ok().flatten();

        // Collect all raw field names to detect collisions when stripping _code suffix.
        let all_field_names: std::collections::HashSet<String> =
            props.iter().map(|p| p.rust_field_name.clone()).collect();

        // Query the graph for all entity titles so we can detect when a
        // ValueObject property actually targets an entity (FK column, not child table).
        let entity_titles: std::collections::HashSet<String> =
            db.get_entity_names().await?.into_iter().collect();

        let cls_ctx = ClassificationContext {
            schema_title,
            module_name: &module_name,
            schema_name: &schema_name,
            entity_name: &entity_name,
            composite_range: &composite_range,
            consumed_fields: &consumed_fields,
            all_field_names: &all_field_names,
            entity_titles: &entity_titles,
            workflow_managed: &workflow_managed,
            suffix: &config.defaults.type_suffix,
        };
        let (mut direct_columns, child_tables, junction_tables) =
            build_columns_and_children(db, &props, &cls_ctx).await?;

        // Add synthetic hierarchy column (self-referential FK) when configured.
        if let Some(ref hf) = entity_cfg.and_then(|ec| ec.hierarchy_field.as_ref())
            && !direct_columns.iter().any(|c| c.field_name == **hf)
        {
            direct_columns.push(TreeColumn {
                field_name: hf.to_string(),
                pg_column_name: hf.to_string(),
                dto_field_name: None,
                rust_type: "Uuid".to_string(),
                is_nullable: true,
                is_entity_ref: false,
                dto_rust_type: None,
                is_workflow_managed: false,
                is_array: false,
                pg_cast: None,
                is_composite_range: false,
                is_structured_wrapper: false,
                is_media: false,
            });
        }

        let entity_module = format!("{}_{}", schema_name, module_name);

        let search = entity_cfg.map(|ec| &ec.search);
        // Issue #449: a design search wins for has_fts and supplies the FTS
        // language from its default analyzer (per-field overrides warn on
        // the surface and fall back to the default analyzer).
        let has_fts = match design_search {
            Some(design) => !design.text.is_empty(),
            None => search
                .and_then(|s| s.fts_columns.as_ref())
                .map(|cols| !cols.is_empty())
                .unwrap_or(false),
        };
        let has_embeddings = search
            .map(|s| !s.embedding_columns.is_empty())
            .unwrap_or(false);
        let fts_language = match design_search.and_then(|s| s.analyzer.clone()) {
            Some(analyzer) => analyzer,
            None => search
                .map(|s| s.fts_language.clone())
                .unwrap_or_else(|| "english".to_string()),
        };

        let filter_fields = match design_search {
            Some(design) => {
                crate::ddd::design::design_filter_fields(db, schema_title, design).await?
            }
            None => {
                resolve_filter_fields(
                    db,
                    schema_title,
                    entity_cfg
                        .and_then(|ec| ec.filter_fields.as_ref())
                        .map(|v| v.as_slice()),
                )
                .await?
            }
        };

        let nested_filter_fields =
            resolve_nested_filter_fields(db, schema_title, &module_name, &schema_name, config)
                .await?;

        let hierarchy_field = entity_cfg
            .and_then(|ec| ec.hierarchy_field.as_ref())
            .cloned();

        // Resolve tree_include entries: find FK columns and parent refs
        let tree_include = {
            let mut resolved = Vec::new();
            if let Some(entries) = entity_cfg.and_then(|ec| ec.tree_include.as_ref()) {
                for entry in entries {
                    // Find the via entity's domain entry (search all domains)
                    let via_domain_entry = config
                        .domains
                        .values()
                        .find(|d| d.entity_config.contains_key(&entry.via_entity));
                    let via_entity_cfg =
                        via_domain_entry.and_then(|d| d.entity_config.get(&entry.via_entity));

                    // Get via entity's parent_ref column name
                    let parent_ref_col = via_entity_cfg
                        .and_then(|ec| ec.parent_ref.as_ref())
                        .cloned();

                    // Get via entity's parent entity name from role/parent config
                    let parent_entity_name =
                        via_entity_cfg.and_then(|ec| ec.parent.as_ref()).cloned();

                    // Find the FK column on via_entity that references the current entity.
                    // Use the naming convention: snake_case(prop.name) + "_id" matches the DDL.
                    let via_props = db.get_properties(&entry.via_entity).await?;
                    let mut via_fk = None;
                    for prop in &via_props {
                        if let Ok(Some(target)) = db
                            .get_property_ref_target(&prop.name, &entry.via_entity)
                            .await
                            && target.title == schema_title
                        {
                            let col = codegraph_core::types::resolve_field(prop).column_name;
                            via_fk = Some(col);
                            break;
                        }
                    }

                    // Get via entity's schema for table name
                    let via_schema = db
                        .get_schema_in_domain(&entry.via_entity, domain)
                        .await?
                        .ok_or_else(|| {
                            crate::error::Error::SchemaNotFound(entry.via_entity.clone())
                        })?;

                    // Get parent entity's schema for table name
                    let parent_schema = if let Some(ref name) = parent_entity_name {
                        db.get_schema_in_domain(name, domain).await.ok().flatten()
                    } else {
                        None
                    };

                    // Resolve worker detail JOINs from the parent entity's composition tree.
                    // Walks child tables to find person name columns (given, family) and avatar_url.
                    let worker_detail_joins = if let Some(ref parent_name) = parent_entity_name {
                        resolve_worker_detail_joins(db, parent_name, domain).await
                    } else {
                        Vec::new()
                    };

                    if let (Some(p_ref), Some(fk), Some(parent_schema)) =
                        (parent_ref_col, via_fk, parent_schema)
                    {
                        resolved.push(TreeIncludeResolved {
                            alias: entry.alias.clone(),
                            via_table: format!(
                                "{}.{}",
                                via_schema.domain.as_deref().unwrap_or("public"),
                                via_schema.pg_table_name
                            ),
                            via_fk_column: fk,
                            parent_table: format!(
                                "{}.{}",
                                parent_schema.domain.as_deref().unwrap_or("public"),
                                parent_schema.pg_table_name
                            ),
                            parent_ref_column: p_ref,
                            worker_detail_joins,
                        });
                    }
                }
            }
            resolved
        };

        // Declared `.ddd` finders, validated against the entity's direct
        // columns: a finder whose parameter does not name an existing
        // compatible column is dropped with a one-time warning so the trait
        // and BOTH impls stay in sync (trait methods are only emitted for
        // finders that made it onto the tree).
        // The built-in finders (`findByKeys`/`findByExample`) lower against
        // the entity's graph features — fetched once alongside the tree.
        let entity_features =
            crate::ddd::design::EntityFeatures::from_graph(db, schema_title).await?;
        let design_finders = design_surface
            .finders_for(schema_title, &entity_name, &entity_features)
            .into_iter()
            .filter(|finder| match super::finders::finder_columns(finder, &direct_columns) {
                Some(_) => true,
                None => {
                    crate::ddd::design::warn_once(
                        &format!("finder:column:{}::{}", entity_name, finder.name),
                        &format!(
                            "finder `{}.{}` skipped: parameters do not resolve to columns of `{}`",
                            entity_name, finder.name, entity_name
                        ),
                    );
                    false
                }
            })
            .collect();

        Ok(EntityTree {
            entity_name,
            module_name: module_name.clone(),
            schema_name,
            table_name: module_name,
            entity_module,
            namespace: schema.namespace.clone(),
            direct_columns,
            child_tables,
            junction_tables,
            has_create,
            has_read,
            has_update,
            has_delete,
            has_list,
            append_only,
            has_workflow,
            has_fts,
            has_embeddings,
            fts_language,
            filter_fields,
            nested_filter_fields,
            parent_ref: parent_ref.map(|s| s.to_string()),
            hierarchy_field,
            tree_include,
            design_finders,
            is_auditable,
            soft_delete_visibility,
            soft_delete_column,
            soft_delete_cascade,
            track_updated_user,
            track_deleted_user,
        })
    }

    fn emit_header(
        &self,
        tree: &EntityTree,
        include_paths: &[ResolvedIncludePath],
        project: &ProjectConfig,
        code: &mut CodeWriter,
    ) {
        wln!(
            code,
            "//! Generated repository implementation for {}.",
            tree.entity_name
        );
        wln!(
            code,
            "//! DO NOT EDIT — generated by {}.",
            project.identity.generator_name
        );
        wln!(code);
        wln!(code, "use async_trait::async_trait;");
        wln!(code, "use sea_orm::{{");
        let has_range_cols = tree
            .direct_columns
            .iter()
            .any(|c| c.pg_cast.is_some() && !c.is_composite_range);
        // Only count child tables that actually produce raw SQL inserts
        // (empty children are skipped by emit_child_inserts).
        let has_meaningful_children = tree
            .child_tables
            .iter()
            .any(|c| !c.columns.is_empty() || !c.child_tables.is_empty());
        let needs_raw_sql = has_meaningful_children
            || !tree.junction_tables.is_empty()
            || tree.has_fts
            || tree.has_embeddings
            || has_range_cols
            || tree.hierarchy_field.is_some()
            // Scoped-include hydration emits raw-SQL child fetches
            // (Statement/DatabaseBackend/db.query_all) even when the CRUD
            // child tree is empty — the imports must match.
            || !include_paths.is_empty();
        let needs_active_model =
            tree.has_create || tree.has_update || (tree.is_auditable && tree.has_delete);

        let mut sea_orm_items: Vec<&str> = Vec::new();
        if needs_active_model {
            sea_orm_items.push("ActiveModelTrait");
        }
        sea_orm_items.push("ColumnTrait");
        if needs_raw_sql {
            sea_orm_items.push("ConnectionTrait");
        }
        sea_orm_items.push("DatabaseTransaction");
        sea_orm_items.push("EntityTrait");
        sea_orm_items.push("PaginatorTrait");
        sea_orm_items.push("QueryFilter");
        sea_orm_items.push("QueryOrder");
        if needs_active_model {
            sea_orm_items.push("Set");
        }
        if needs_raw_sql {
            sea_orm_items.push("DatabaseBackend");
            sea_orm_items.push("Statement");
        }
        wln!(code, "    {},", sea_orm_items.join(", "));
        wln!(code, "}};");
        wln!(code, "use uuid::Uuid;");
        wln!(code);
        wln!(
            code,
            "use super::repository::{}Repository;",
            tree.entity_name
        );
        if tree.has_create {
            wln!(
                code,
                "use super::dto_create::Create{}Request;",
                tree.entity_name
            );
        }
        if tree.has_update {
            wln!(
                code,
                "use super::dto_update::Update{}Request;",
                tree.entity_name
            );
        }
        wln!(
            code,
            "use super::dto_response::{}Response;",
            tree.entity_name
        );
        // Import child DTO response types (including nested children)
        let all_children = flatten_child_tables(&tree.child_tables);
        let mut imported = std::collections::HashSet::new();
        for child in &all_children {
            if imported.insert(child.struct_name.clone()) {
                wln!(
                    code,
                    "use super::dto_response::{}Response;",
                    child.struct_name
                );
            }
        }
        wln!(code);
        wln!(code, "pub struct {}RepositoryImpl;", tree.entity_name);
        wln!(code);
        wln!(code, "#[async_trait]");
        wln!(
            code,
            "impl {}Repository<sea_orm::DatabaseTransaction> for {}RepositoryImpl {{",
            tree.entity_name,
            tree.entity_name
        );
    }

    fn emit_footer(&self, code: &mut CodeWriter) {
        wln!(code, "}}");
    }
}
