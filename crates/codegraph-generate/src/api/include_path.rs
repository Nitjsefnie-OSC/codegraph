use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::SchemaNode;
use codegraph_core::types::resolve_field;
use codegraph_naming::strip_suffix;
use codegraph_type_contracts::RefClassificationKind;
use serde::Serialize;

use crate::error::Result;

/// A fully resolved include path ready for DTO/repository code generation.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedIncludePath {
    /// Full dot-notation alias, e.g. "person" or "deployment.position"
    pub alias: String,
    /// Dot-delimited segments, resolved against the graph
    pub segments: Vec<IncludeSegment>,
    /// Generated DTO type name for this path's response, e.g. "DeploymentCombinedResponse".
    /// For single-segment paths this matches the target entity's Response type.
    /// For multi-segment paths this is the combined enriched type name.
    pub response_rust_type: String,
    /// The method name generated in the repository, e.g. "fetch_person_for_worker"
    pub fetch_method: String,
    /// The method name generated for list batch-fetch, e.g. "fetch_person_batch_for_worker"
    pub batch_fetch_method: String,
}

/// A single segment in an include path chain.
#[derive(Debug, Clone, Serialize)]
pub struct IncludeSegment {
    /// The target entity display name, e.g. "Worker" (from rust_type_name).
    pub entity_name: String,
    /// The canonical schema title used as the graph node key, e.g. "WorkerType".
    pub schema_title: String,
    /// Rust-safe module name, e.g. "person"
    pub module_name: String,
    /// Domain name, e.g. "common"
    pub domain: String,
    /// Schema-qualified table name, e.g. "\"common\".\"person\""
    pub table: String,
    /// FK column on the source entity's table that references this target,
    /// e.g. "person_id" on the worker table
    pub fk_column: String,
    /// FK column on this target's table that references back to the source,
    /// e.g. "worker_id" on the person table
    pub reverse_fk_column: String,
    /// Whether the FK column on the source table is a REQUIRED genuine
    /// EntityReference (JSON schema `required`). When true, the repository
    /// fetch code accesses the field directly (Uuid) instead of via
    /// Option patterns (Option<Uuid>). VO→entity synthetic FKs are always
    /// nullable and therefore never required.
    pub fk_is_required: bool,
    /// Whether the reverse FK column on the target table is a REQUIRED
    /// genuine EntityReference. Same semantics as `fk_is_required` but for
    /// the array/child side of the relationship.
    pub reverse_fk_is_required: bool,
    /// Whether this is a one-to-many relationship (Vec) vs one-to-one (Option)
    pub is_array: bool,
    /// When the target entity is reached via a VO→entity allOf chain (Tier 1.5),
    /// the data is stored in a VO child table (not the entity table). This override
    /// tells the fetch method to query the child table directly by parent FK.
    pub child_table_override: Option<ChildTableOverride>,
}

/// Metadata for querying a VO child table instead of the entity table
/// when the include path resolves through a VO→entity allOf chain.
#[derive(Debug, Clone, Serialize)]
pub struct ChildTableOverride {
    /// The VO's schema title (e.g. "PersonLegalType") — used for property lookup
    pub vo_title: String,
    /// Child table name (e.g. "worker_person")
    pub child_table_name: String,
    /// SeaORM entity module (e.g. "common_worker_person")
    pub child_module: String,
    /// Schema name (e.g. "common")
    pub child_schema: String,
    /// Parent FK column name in snake_case (e.g. "worker_id").
    /// Use for Model field access; wrap in to_pascal_case() for Column filters.
    pub parent_fk_column: String,
    /// Response type for the include path e.g. "WorkerPersonLegalResponse"
    pub response_type: String,
}

/// Resolve include paths from configuration against graph data.
/// Returns resolved paths with FK columns, table names, and generated type names.
pub async fn resolve_include_paths(
    db: &dyn GraphQuerier,
    config: &codegraph_config::DomainConfig,
    domain: &str,
    schema_title: &str,
    allow_include: Option<&Vec<String>>,
) -> Result<Vec<ResolvedIncludePath>> {
    // Monolith behaviour: cross-domain include paths stay (same-DB joins).
    resolve_include_paths_for_topology(db, config, domain, schema_title, allow_include, false).await
}

/// [`resolve_include_paths`] with an explicit workers-topology flag.
///
/// In workers topology, include paths that cross a domain boundary are
/// dropped: the generated fetch code would reference the target domain's
/// `crate::domain::{domain}/...` and `crate::entity/{module}/...` modules,
/// which live in a *different* worker crate.
///
/// TODO(worker-remote-includes): fetch cross-domain includes over the target
/// worker's API (service binding) instead of same-DB SQL; until then the
/// workers compile gate keeps only same-domain includes.
pub async fn resolve_include_paths_for_topology(
    db: &dyn GraphQuerier,
    config: &codegraph_config::DomainConfig,
    domain: &str,
    schema_title: &str,
    allow_include: Option<&Vec<String>>,
    workers_topology: bool,
) -> Result<Vec<ResolvedIncludePath>> {
    let source_schema = db
        .get_schema_in_domain(schema_title, domain)
        .await?
        .ok_or_else(|| crate::error::Error::SchemaNotFound(schema_title.into()))?;

    let source_entity_name = &source_schema.rust_type_name;
    let source_module = &source_schema.pg_table_name;

    match allow_include {
        Some(paths) if paths.is_empty() => Ok(Vec::new()),
        Some(paths) => {
            let resolved = resolve_explicit_paths(
                db,
                config,
                domain,
                schema_title,
                &source_schema.schema_id,
                source_entity_name,
                source_module,
                paths,
            )
            .await?;
            Ok(filter_cross_domain_paths(
                domain,
                resolved,
                workers_topology,
            ))
        }
        None => {
            let resolved = resolve_auto_paths(
                db,
                config,
                domain,
                schema_title,
                source_entity_name,
                source_module,
            )
            .await?;
            Ok(filter_cross_domain_paths(
                domain,
                resolved,
                workers_topology,
            ))
        }
    }
}

/// [`resolve_include_paths_for_topology`] with the standard entity gate
/// applied: non-root entities (`role != "root"`) are skipped unless they
/// declare an explicit non-empty `allow_include`.
///
/// This is the gate the handler, DTO, and repository generators all apply
/// before resolving paths. Exposing it here lets emitters that are invoked
/// without pre-resolved paths (e.g. [`crate::ddd::
/// repository_emitter::RepositoryImplEmitter::emit`] called outside the
/// pipeline) derive the exact same include surface the handler hydrates —
/// keeping the handler↔repository fetch-method contract intact regardless
/// of caller.
pub async fn resolve_include_paths_gated(
    db: &dyn GraphQuerier,
    config: &codegraph_config::DomainConfig,
    domain: &str,
    schema_title: &str,
    workers_topology: bool,
) -> Result<Vec<ResolvedIncludePath>> {
    let entity_cfg = config
        .domains
        .get(domain)
        .and_then(|d| d.get_entity_config(schema_title));
    let has_explicit_include = entity_cfg
        .and_then(|ec| ec.allow_include.as_ref())
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    let is_root = entity_cfg
        .and_then(|ec| ec.role.as_deref())
        .map(|r| r == "root")
        .unwrap_or(true);
    if !(has_explicit_include || is_root) {
        return Ok(Vec::new());
    }
    resolve_include_paths_for_topology(
        db,
        config,
        domain,
        schema_title,
        entity_cfg.and_then(|ec| ec.allow_include.as_ref()),
        workers_topology,
    )
    .await
}

/// Drop include paths with cross-domain segments when `workers_topology` is
/// set; monolith keeps them.
fn filter_cross_domain_paths(
    source_domain: &str,
    paths: Vec<ResolvedIncludePath>,
    workers_topology: bool,
) -> Vec<ResolvedIncludePath> {
    if !workers_topology {
        return paths;
    }
    paths
        .into_iter()
        .filter(|path| {
            let cross_domain = path.segments.iter().any(|seg| seg.domain != source_domain);
            if cross_domain {
                tracing::warn!(
                    include_path = %path.alias,
                    source_domain = %source_domain,
                    "dropping cross-domain include path in workers topology \
                     (TODO(worker-remote-includes))"
                );
            }
            !cross_domain
        })
        .collect()
}

// ── Explicit path resolution ──────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn resolve_explicit_paths(
    db: &dyn GraphQuerier,
    config: &codegraph_config::DomainConfig,
    domain: &str,
    schema_title: &str,
    source_schema_id: &str,
    source_entity_name: &str,
    source_module: &str,
    paths: &[String],
) -> Result<Vec<ResolvedIncludePath>> {
    let mut resolved = Vec::new();

    for path in paths {
        let segment_strs: Vec<&str> = path.split('.').collect();
        if segment_strs.len() > 3 {
            tracing::warn!("include path '{path}' exceeds max depth of 3 — skipping");
            continue;
        }

        let mut segments = Vec::new();
        // `current_source_title` always holds the canonical schema title so that
        // graph queries (get_referenced_schemas, get_properties) work correctly
        // at every depth level.
        let mut current_source_title: &str = schema_title;
        let mut current_source_schema_id: String = source_schema_id.to_string();

        for &seg in &segment_strs {
            // Resolve the target schema via graph identity (schema_id).
            let target_schema = resolve_schema_target(
                db,
                &current_source_schema_id,
                current_source_title,
                seg,
                domain,
            )
            .await?;
            let target_title = target_schema.title.clone();

            // Skip force_value_objects — they don't have standalone entity or DTO
            // generation, so fetch methods referencing {Entity}Response would fail.
            let is_force_vo = config
                .domains
                .get(domain)
                .map(|d| d.force_value_objects.contains(&target_title))
                .unwrap_or(false);
            if is_force_vo {
                tracing::warn!(
                    "include path '{path}' targets force_value_object '{target_title}' — skipping"
                );
                break;
            }

            // Skip codelists — they have standalone enum generation but no entity
            // module or response DTO, so fetch methods referencing {Entity}Response
            // would fail.
            if target_schema.is_codelist {
                tracing::warn!(
                    "include path '{path}' targets codelist '{target_title}' — skipping (no DTO/entity)"
                );
                break;
            }

            // NOTE: is_entity is a domain-model concept here — it determines whether a
            // schema is an entity vs a value object for FK resolution and include path
            // computation. API exposure is handled by ApiResourceNode in the API model layer.
            //
            // Skip non-entity types — they don't have standalone entity generation
            // or response DTOs, so include paths referencing them would fail.
            if !target_schema.is_entity {
                tracing::warn!(
                    "include path '{path}' targets non-entity '{target_title}' — skipping"
                );
                break;
            }

            let target_entity_name = target_schema.rust_type_name.clone();
            let target_schema_title = target_schema.title.clone();
            let target_module = target_schema.pg_table_name.clone();
            let target_domain = target_schema
                .domain
                .clone()
                .unwrap_or_else(|| domain.to_string());
            let target_table = format!("\"{}\".\"{}\"", target_domain, target_module);

            // Resolve FK column and array flag via graph query — uses
            // db.get_properties() which runs GQL internally.
            let (fk_column, is_array, _) =
                resolve_fk_via_graph(db, current_source_title, &target_title, seg, false).await?;

            // Reverse FK: check config parent_ref first, then graph. A None
            // reverse (the target owns no verified FK property referencing
            // this source) is only fatal for array segments — the reverse
            // column is what an array fetch scans.
            let reverse_fk_column =
                resolve_child_fk_column(config, domain, &target_title, current_source_title, db)
                    .await?;

            // Detect VO→entity: when the segment resolves to an entity via a VO
            // allOf chain, the data lives in a child table, not the entity table.
            let mut child_table_override = None;
            let seg_lower_detect = seg.to_lowercase();
            if let Ok(props) = db
                .get_properties_by_schema_id(&current_source_schema_id)
                .await
            {
                for prop in &props {
                    let p_stem = prop.name.to_lowercase();
                    let r_stem = prop
                        .rust_field_name
                        .strip_suffix("_id")
                        .unwrap_or(&prop.rust_field_name)
                        .to_lowercase();
                    if p_stem != seg_lower_detect && r_stem != seg_lower_detect {
                        continue;
                    }
                    if let Ok(Some(ref_target)) = db
                        .get_property_ref_target_by_id(&prop.name, &current_source_schema_id)
                        .await
                        && (!ref_target.is_entity || ref_target.pg_table_name.is_empty())
                    {
                        // It's a VO — does it extend the resolved entity?
                        if let Ok(Some(entity)) =
                            find_entity_through_vo(db, &ref_target.title).await
                            && entity.schema_id == target_schema.schema_id
                        {
                            let ct_name = codegraph_naming::truncate_pg_identifier(&format!(
                                "{}_{}",
                                source_module, prop.rust_field_name
                            ));
                            let ct_module = format!("{}_{}", domain, ct_name);
                            let p_fk = format!("{}_id", source_module);
                            let child_struct = format!(
                                "{}{}",
                                strip_suffix(source_entity_name, &config.defaults.type_suffix),
                                strip_suffix(
                                    &ref_target.rust_type_name,
                                    &config.defaults.type_suffix
                                ),
                            );
                            child_table_override = Some(ChildTableOverride {
                                vo_title: ref_target.title.clone(),
                                child_table_name: ct_name,
                                child_module: ct_module,
                                child_schema: domain.to_string(),
                                parent_fk_column: p_fk,
                                response_type: format!("{}Response", child_struct),
                            });
                            break;
                        }
                    }
                }
            }

            // Gate: the column the fetch will use must actually exist.
            // Junction arrays (array-of-entity-ref) materialize as junction
            // tables — neither the source-side FK nor the target-side
            // back-ref column exists, so paths through them are unfetchable.
            // VO→entity overrides are exempt: the fetch queries the VO child
            // table by its own parent FK (child_table_override), not these
            // columns. Config-declared child entities are also exempt when
            // the fetch column IS the configured parent_ref: the DDL emits
            // that column from the same config (`parent = <source>` +
            // `parent_ref`), so it exists in the database even though the
            // schema JSON carries no matching property — the property-based
            // gate below cannot see it and would over-skip valid reverse
            // includes (e.g. `worker?include=deployment.position` with
            // DeploymentType.parent_ref = "worker_type_id").
            let src_title: &str = current_source_title;
            let config_child_fk: Option<String> = config
                .domains
                .get(domain)
                .and_then(|d| d.get_entity_config(&target_title))
                .filter(|ec| ec.parent.as_deref().is_some_and(|p| p == src_title))
                .and_then(|ec| ec.parent_ref.clone());
            if child_table_override.is_none() {
                let (fetch_column, fetch_on_title): (&String, String) = if is_array {
                    match &reverse_fk_column {
                        Some(col) => (col, target_title.clone()),
                        None => {
                            tracing::warn!(
                                "include path '{path}' segment '{seg}' is unfetchable — no back-FK \
                                 column on '{target_title}' (junction array?) — skipping"
                            );
                            break;
                        }
                    }
                } else {
                    (&fk_column, current_source_title.to_string())
                };
                let config_authoritative =
                    config_child_fk.as_deref() == Some(fetch_column.as_str());
                if !config_authoritative
                    && !schema_has_fk_column(db, &fetch_on_title, fetch_column).await?
                {
                    tracing::warn!(
                        "include path '{path}' segment '{seg}' is unfetchable — column \
                         '{fetch_column}' not found on '{fetch_on_title}' (junction array?) — skipping"
                    );
                    break;
                }
            }

            // Nullability: JSON schema `required` is the source of truth for
            // genuine EntityReference FKs. VO→entity (child_table_override)
            // FKs are always nullable.
            let effective_reverse_fk = reverse_fk_column
                .clone()
                .unwrap_or_else(|| fk_column.clone());
            let fk_is_required = child_table_override.is_none()
                && fk_column_is_required(db, current_source_title, &fk_column).await?;
            let reverse_fk_is_required =
                fk_column_is_required(db, &target_title, &effective_reverse_fk).await?;

            segments.push(IncludeSegment {
                entity_name: target_entity_name,
                schema_title: target_schema_title,
                module_name: target_module,
                domain: target_domain,
                table: target_table,
                fk_column,
                reverse_fk_column: effective_reverse_fk,
                fk_is_required,
                reverse_fk_is_required,
                is_array,
                child_table_override,
            });

            // Use the canonical schema title for the next iteration so graph
            // queries at depth ≥ 2 resolve correctly.
            current_source_title = &segments.last().unwrap().schema_title;
            // Advance the schema_id to the target's identity for the next
            // segment's identity-native property lookup.
            current_source_schema_id = target_schema.schema_id.clone();
        }

        // Every segment was skipped (junction array, force VO, codelist,
        // non-entity) — the path is not resolvable, don't emit a fetch
        // method for a zero-segment path.
        if segments.is_empty() {
            continue;
        }

        let alias_snake = path.replace('.', "_");
        // When the segment has a child table override (VO→entity), use its
        // response type (child DTO) instead of the entity's response DTO.
        let response_rust_type = if let Some(over) = segments
            .first()
            .and_then(|s| s.child_table_override.as_ref())
        {
            over.response_type.clone()
        } else {
            derive_response_type(&segments)
        };
        let fetch_method = format!("fetch_{alias_snake}_for_{source_module}");
        let batch_fetch_method = format!("fetch_{alias_snake}_batch_for_{source_module}");

        resolved.push(ResolvedIncludePath {
            alias: path.clone(),
            segments,
            response_rust_type,
            fetch_method,
            batch_fetch_method,
        });
    }

    Ok(resolved)
}

// ── Auto-discover path resolution ─────────────────────────────────────

async fn resolve_auto_paths(
    db: &dyn GraphQuerier,
    config: &codegraph_config::DomainConfig,
    domain: &str,
    schema_title: &str,
    source_entity_name: &str,
    source_module: &str,
) -> Result<Vec<ResolvedIncludePath>> {
    let mut paths: Vec<ResolvedIncludePath> = Vec::new();

    // Source 1: Children from parent_candidates.
    let parent_candidates = db.get_parent_candidates().await?;
    for pc in &parent_candidates {
        if pc.parent_title != schema_title {
            continue;
        }
        let target_title = &pc.child_title;
        let Some(target_schema) = db.get_schema_in_domain(target_title, domain).await? else {
            continue;
        };
        // Skip child entities and inline definitions — they don't have standalone
        // entity .rs files, so repository code referencing crate::entity::<module>::
        // would fail with E0583.
        if target_schema.parent_schema.is_some() {
            continue;
        }
        // Skip force_value_objects — they won't have standalone entity generation.
        let is_force_vo = config
            .domains
            .get(domain)
            .map(|d| d.force_value_objects.contains(target_title))
            .unwrap_or(false);
        if is_force_vo {
            continue;
        }
        // Skip codelists — no standalone entity generation.
        if target_schema.is_codelist {
            continue;
        }
        // Skip non-entity types — no standalone entity generation.
        if !target_schema.is_entity {
            continue;
        }
        let target_module = target_schema.pg_table_name.clone();
        let target_schema_title = target_schema.title.clone();
        let target_entity_name = target_schema.rust_type_name.clone();
        let target_domain = target_schema
            .domain
            .clone()
            .unwrap_or_else(|| domain.to_string());
        let target_table = format!("\"{}\".\"{}\"", target_domain, target_module);

        // Resolve FK column from the child entity's domain config (parent_ref)
        // or from graph properties. Both fk_column and reverse_fk_column
        // resolve to the same FK on the child entity that references the
        // parent. A candidate whose child owns no verified FK property is a
        // phantom edge (derived from the wrong direction of a relationship)
        // — emitting it would reference a column the child entity does not
        // have (E0599), so it is skipped with a warning.
        let Some(fk_column) =
            resolve_child_fk_column(config, domain, target_title, schema_title, db).await?
        else {
            tracing::warn!(
                child = %target_title,
                parent = %schema_title,
                "parent candidate skipped: child owns no FK property referencing the parent (phantom edge)"
            );
            continue;
        };
        let reverse_fk_column = fk_column.clone();

        // Junction guard: array-of-entity-ref properties (e.g.
        // trust.settlorIds → party) materialize as junction tables — the
        // child entity carries no parent FK, so child-style fetch helpers
        // would reference a nonexistent SeaORM `Column`. ScalarRef children
        // and VO children DO get the FK injected into their entity model by
        // the entity generator, so they remain include-able.
        if parent_holds_child_as_junction(db, schema_title, target_title).await? {
            tracing::debug!(
                parent = %schema_title,
                child = %target_title,
                "auto-discovery skipping junction relationship"
            );
            continue;
        }

        let alias_seg = codegraph_naming::to_snake_case(super::router::strip_suffix(
            target_title,
            &config.defaults.type_suffix,
        ));

        // Nullability: JSON schema `required` is the source of truth for
        // genuine EntityReference FKs. Both point at the same child-side FK.
        let fk_is_required = fk_column_is_required(db, schema_title, &fk_column).await?;
        let reverse_fk_is_required =
            fk_column_is_required(db, target_title, &reverse_fk_column).await?;

        // Parent-candidate includes are reverse relationships: the FK column is
        // owned by the child (target) entity, and the fetch scans the child table
        // by that FK (`deployment.worker_type_id == job.id`) — array semantics in
        // both discovery modes (`DetectionSource::ScalarRef`: the child holds a
        // scalar ref to the parent; `DetectionSource::ArrayItems`: the parent
        // holds an ItemsOf array). This is intrinsic to the discovery source, so
        // the include always resolves as the array/reverse variant.
        //
        // Pre-99da338 this was expressed as `... || true`. The property-list
        // check it was OR'd onto inspected the PARENT's own properties, which is
        // unrelated to this candidate relationship; without the `|| true` the
        // include flips to forward-single semantics, and the generated fetch
        // reads `source.<fk_column>` off the parent model — a column the parent
        // doesn't own (it lives on the child) — producing uncompilable
        // `repository_impl.rs`.
        let is_array = true;

        paths.push(ResolvedIncludePath {
            alias: alias_seg.clone(),
            segments: vec![IncludeSegment {
                entity_name: target_entity_name,
                schema_title: target_schema_title,
                module_name: target_module,
                domain: target_domain,
                table: target_table,
                fk_column,
                reverse_fk_column,
                fk_is_required,
                reverse_fk_is_required,
                is_array,
                child_table_override: None,
            }],
            response_rust_type: format!("{}Response", target_schema.rust_type_name),
            fetch_method: format!("fetch_{alias_seg}_for_{source_module}"),
            batch_fetch_method: format!("fetch_{alias_seg}_batch_for_{source_module}"),
        });
    }

    // Source 2: Entity references (cross-refs from referenced_schemas).
    let schema_title_with_type = format!("{source_entity_name}Type");
    let referenced = db
        .get_referenced_schemas(&schema_title_with_type)
        .await
        .unwrap_or_default();

    // Collect already-discovered entity names to avoid duplicates.
    let existing_entity_names: std::collections::HashSet<String> = paths
        .iter()
        .flat_map(|p| p.segments.iter().map(|s| s.schema_title.clone()))
        .collect();

    for ref_schema in &referenced {
        let ref_title = &ref_schema.title;
        if ref_title == schema_title {
            continue;
        }
        if existing_entity_names.contains(ref_title) {
            continue;
        }
        let Some(target_schema) = db.get_schema_in_domain(ref_title, domain).await? else {
            continue;
        };
        if target_schema.pg_table_name.is_empty() {
            continue;
        }
        // Skip child entities and inline definitions — they don't have standalone
        // entity .rs files, so repository code referencing crate::entity::<module>::
        // would fail with E0583.
        if target_schema.parent_schema.is_some() {
            continue;
        }
        // Skip force_value_objects — they won't have standalone entity generation.
        let is_force_vo = config
            .domains
            .get(domain)
            .map(|d| d.force_value_objects.contains(ref_title))
            .unwrap_or(false);
        if is_force_vo {
            continue;
        }
        // Skip codelists — no standalone entity generation.
        if target_schema.is_codelist {
            continue;
        }
        // Skip non-entity types — no standalone entity generation.
        if !target_schema.is_entity {
            continue;
        }
        let target_entity_name = target_schema.rust_type_name.clone();
        let target_schema_title = target_schema.title.clone();
        let target_module = target_schema.pg_table_name.clone();
        let target_domain = target_schema
            .domain
            .clone()
            .unwrap_or_else(|| domain.to_string());
        let target_table = format!("\"{}\".\"{}\"", target_domain, target_module);

        // Resolve FK property via graph query.
        let ref_entity_name = super::router::strip_suffix(ref_title, &config.defaults.type_suffix);
        let (fk_column, is_array, fk_verified) = resolve_fk_via_graph(
            db,
            schema_title,
            ref_title,
            &codegraph_naming::to_snake_case(ref_entity_name),
            false,
        )
        .await?;

        let source_entity_name =
            super::router::strip_suffix(schema_title, &config.defaults.type_suffix);
        // Strict: the reverse check decides which column an array fetch
        // scans on the target — a decoy name match here used to emit
        // `{target}::Column::{Child}` (issue #458).
        let (reverse_resolved, _, reverse_verified) =
            resolve_fk_via_graph(db, ref_title, schema_title, source_entity_name, true).await?;

        // A single forward reference whose FK column could not be located on
        // the source (convention fallback) would emit a fetch reading a
        // column the source entity does not have — skip it loudly.
        if !fk_verified && !is_array {
            tracing::warn!(
                source = %schema_title,
                target = %ref_title,
                "entity reference skipped: no FK property on the source references the target"
            );
            continue;
        }

        // Junction guard: array-of-entity-ref cross-refs live in junction
        // tables and cannot be fetched by either FK column.
        if parent_holds_child_as_junction(db, schema_title, ref_title).await? {
            tracing::debug!(
                source = %schema_title,
                target = %ref_title,
                "auto-discovery skipping junction cross-reference"
            );
            continue;
        }

        // An array cross-reference needs a back-FK on the target to hydrate
        // the reverse collection; without one (junction or unmanaged
        // relationship) there is nothing to fetch by column.
        if is_array && !reverse_verified {
            tracing::warn!(
                source = %schema_title,
                target = %ref_title,
                "array entity reference skipped: no back-FK property on the target (junction or unmanaged relationship)"
            );
            continue;
        }

        let alias_seg = codegraph_naming::to_snake_case(ref_entity_name);

        // Nullability: JSON schema `required` is the source of truth for
        // genuine EntityReference FKs. fk lives on the source, reverse FK on
        // the referenced (target) schema. When no real back-FK exists the
        // reverse column is never read by the emitted fetch/batch (both
        // drive off the source-side FK), so the verified forward column is
        // reused instead of a convention-fabricated name.
        let reverse_fk_column = if reverse_verified {
            reverse_resolved
        } else {
            fk_column.clone()
        };

        // Source-1-style gate (issue #458): the column an array fetch scans
        // must exist as a real property of the target before the path is
        // emitted — otherwise the generated `Column::{Reverse}` references a
        // column the target entity does not have (E0599).
        if is_array && !schema_has_fk_column(db, ref_title, &reverse_fk_column).await? {
            tracing::warn!(
                source = %schema_title,
                target = %ref_title,
                column = %reverse_fk_column,
                "array entity reference skipped: back-FK column not found on the target \
                 (junction or unmanaged relationship)"
            );
            continue;
        }

        let fk_is_required = fk_column_is_required(db, schema_title, &fk_column).await?;
        let reverse_fk_is_required =
            fk_column_is_required(db, ref_title, &reverse_fk_column).await?;

        paths.push(ResolvedIncludePath {
            alias: alias_seg.clone(),
            segments: vec![IncludeSegment {
                entity_name: target_entity_name,
                schema_title: target_schema_title,
                module_name: target_module,
                domain: target_domain,
                table: target_table,
                fk_column,
                reverse_fk_column,
                fk_is_required,
                reverse_fk_is_required,
                is_array,
                child_table_override: None,
            }],
            response_rust_type: format!("{}Response", target_schema.rust_type_name),
            fetch_method: format!("fetch_{alias_seg}_for_{source_module}"),
            batch_fetch_method: format!("fetch_{alias_seg}_batch_for_{source_module}"),
        });
    }

    Ok(paths)
}

// ── Helpers ───────────────────────────────────────────────────────────

/// Resolve a segment string to a target schema node using graph identity
/// (schema_id) for cross-domain collision safety.
///
/// Primary path: query the graph for schemas referenced by the current source
/// via `HasProperty → ReferencesSchema` edges. This is authoritative because
/// the graph stores the actual `$ref` relationships from ingestion.
///
/// Tiers:
///   1 — Property-name match via graph (authoritative, follows $ref edges)
///   1.5 — VO→entity resolution via allOf composition chain
///   2 — ItemsOf / parent_candidates (one-to-many direction)
///   3 — PascalCase naming convention (only with graph evidence)
async fn resolve_schema_target(
    db: &dyn GraphQuerier,
    current_source_schema_id: &str,
    current_source_title: &str,
    seg: &str,
    domain: &str,
) -> Result<SchemaNode> {
    let seg_lower = seg.to_lowercase();

    tracing::debug!(target: "resolve_schema", seg=%seg, source=%current_source_title, domain=%domain, "resolving include segment");

    // 1. Property-name match via the graph.
    //    Queries properties of the source schema by schema_id, matches by name
    //    or rust_field_name (with _id suffix stripped), follows ReferencesSchema
    //    edges to find the target. VOs are skipped — Tier 1.5 follows the allOf
    //    chain to find the entity behind a VO.
    let mut vo_titles: Vec<String> = Vec::new();
    if let Ok(props) = db
        .get_properties_by_schema_id(current_source_schema_id)
        .await
    {
        for prop in &props {
            let prop_stem = prop.name.to_lowercase();
            let rust_stem = prop
                .rust_field_name
                .strip_suffix("_id")
                .unwrap_or(&prop.rust_field_name)
                .to_lowercase();
            if prop_stem != seg_lower && rust_stem != seg_lower {
                continue;
            }
            // Property matches — follow ReferencesSchema edge
            if let Ok(Some(target)) = db
                .get_property_ref_target_by_id(&prop.name, current_source_schema_id)
                .await
            {
                if !target.is_entity || target.pg_table_name.is_empty() {
                    tracing::debug!(target: "resolve_schema", tier=1, prop=%prop.name, target=%target.title, "found VO — queuing for Tier 1.5");
                    vo_titles.push(target.title.clone());
                    continue;
                }
                tracing::debug!(target: "resolve_schema", tier=1, target=%target.title, "resolved via property-name match");
                if let Some(auth) = db.get_schema_by_id(&target.schema_id).await? {
                    return Ok(auth);
                }
                return Ok(target);
            }
        }
    }

    // 1.5 VO → entity via allOf composition chain.
    //    A VO like PersonLegalType allOf-composes shared definitions (PersonBaseType,
    //    PersonLegalInclusion) that an entity (PersonType) also composes. Follow the
    //    chain: VO → allOf target → schemas extending that target → filter to entity.
    if !vo_titles.is_empty() {
        for vo_title in &vo_titles {
            if let Some(entity) = find_entity_through_vo(db, vo_title).await? {
                tracing::debug!(target: "resolve_schema", tier=1.5, vo=%vo_title, entity=%entity.title, "resolved VO → entity via allOf chain");
                return Ok(entity);
            }
        }
    }

    tracing::debug!(target: "resolve_schema", source=%current_source_title, "Tier 1/1.5: no match, falling to Tier 2");

    // 2. ItemsOf references (array items the source holds).
    //    Discovered via parent_candidates query (one-to-many direction).
    if let Ok(candidates) = db.get_parent_candidates().await {
        for pc in &candidates {
            if pc.parent_title == current_source_title {
                let child_stripped = pc
                    .child_title
                    .strip_suffix("Type")
                    .unwrap_or(&pc.child_title)
                    .to_lowercase();
                if child_stripped == seg_lower
                    && let Some(node) = db.get_schema_in_domain(&pc.child_title, domain).await?
                {
                    tracing::debug!(target: "resolve_schema", tier=2, child=%node.title, "resolved via parent_candidates");
                    if let Some(auth_node) = db.get_schema_by_id(&node.schema_id).await? {
                        return Ok(auth_node);
                    }
                    return Ok(node);
                }
            }
        }
    }

    // 3. PascalCase naming convention — only with graph evidence.
    //    Checks that the source has a property whose $ref target matches the
    //    PascalCase candidate, or there is an ItemsOf edge to it. This prevents
    //    false positives where a naming-convention match has no actual graph
    //    relationship (e.g., WorkerType.person → PersonLegalType but Tier 3
    //    would incorrectly match PersonType by name).
    let pascal = codegraph_naming::to_pascal_case(seg);
    let candidates = [format!("{pascal}Type"), pascal.clone()];
    for title in &candidates {
        if let Ok(Some(node)) = db.get_schema_in_domain(title, domain).await {
            if !node.is_entity || node.pg_table_name.is_empty() {
                continue;
            }
            // Verify graph evidence: the source must have a property referencing
            // this schema, or an ItemsOf edge to it.
            if !has_graph_evidence(db, current_source_schema_id, current_source_title, &node).await
            {
                tracing::debug!(target: "resolve_schema", tier=3, candidate=%title, "no graph evidence — skipping");
                continue;
            }
            tracing::debug!(target: "resolve_schema", tier=3, title=%title, "resolved via PascalCase with graph evidence");
            if let Some(auth_node) = db.get_schema_by_id(&node.schema_id).await? {
                return Ok(auth_node);
            }
            return Ok(node);
        }
    }

    tracing::debug!(target: "resolve_schema", seg=%seg, source=%current_source_title, "include segment not resolvable via any tier");

    Err(crate::error::Error::RefResolution(format!(
        "cannot resolve include segment '{seg}' from '{current_source_title}'"
    )))
}

/// Follow the allOf composition chain from a VO to find an entity that shares
/// the same parent definitions. Delegates to the shared utility in codegraph-core.
async fn find_entity_through_vo(
    db: &dyn GraphQuerier,
    vo_title: &str,
) -> Result<Option<SchemaNode>> {
    Ok(codegraph_core::traits::find_entity_extended_by_vo(db, vo_title).await?)
}

/// Check that the source schema has a graph relationship (property $ref or
/// ItemsOf edge) with the candidate entity. Prevents PascalCase from matching
/// entities that share a naming convention but have no actual relationship.
async fn has_graph_evidence(
    db: &dyn GraphQuerier,
    source_schema_id: &str,
    source_title: &str,
    candidate: &SchemaNode,
) -> bool {
    // Check property $ref: does the source have a property referencing this candidate?
    if let Ok(props) = db.get_properties_by_schema_id(source_schema_id).await {
        for prop in &props {
            if let Ok(Some(target)) = db
                .get_property_ref_target_by_id(&prop.name, source_schema_id)
                .await
                && (target.schema_id == candidate.schema_id || target.title == candidate.title)
            {
                return true;
            }
        }
    }
    // Check ItemsOf: does the source have an ItemsOf edge to this candidate?
    if let Ok(candidates_list) = db.get_parent_candidates().await {
        for pc in &candidates_list {
            if pc.parent_title == source_title && pc.child_title == candidate.title {
                return true;
            }
        }
    }
    false
}

/// Whether the property backing `fk_column` on `schema_title` is a REQUIRED
/// genuine EntityReference. The JSON schema's `required` is the source of
/// truth for FK nullability (see entity.rs): a required genuine entity ref
/// emits `Uuid` in the entity model / DTOs, so include-path fetch code must
/// access the field directly instead of via Option patterns.
///
/// VO→entity synthetic FK columns are excluded: they are always nullable
/// (the DTO/repository model the VO as a nested child table), regardless of
/// the schema's `required`.
async fn fk_column_is_required(
    db: &dyn GraphQuerier,
    schema_title: &str,
    fk_column: &str,
) -> Result<bool> {
    let props = db.get_properties(schema_title).await?;
    Ok(props.iter().any(|p| {
        (resolve_field(p).column_name == fk_column || p.pg_column_name == fk_column)
            && p.is_required
            && p.effective_kind() == Some(RefClassificationKind::EntityReference)
    }))
}

/// Whether `parent_title` holds `child_title` through an array-of-entity-ref
/// property — i.e. the relationship is materialized as a junction table.
async fn parent_holds_child_as_junction(
    db: &dyn GraphQuerier,
    parent_title: &str,
    child_title: &str,
) -> Result<bool> {
    let props = db.get_properties(parent_title).await?;
    for p in props.iter().filter(|p| {
        p.is_array && p.effective_kind() == Some(RefClassificationKind::EntityReference)
    }) {
        if let Ok(Some(target)) = db.get_property_ref_target(&p.name, parent_title).await
            && target.title == child_title
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether the schema genuinely carries a column named `column` (a property
/// whose resolved or raw pg column matches). Guards include resolution
/// against emitting fetch helpers that reference nonexistent columns —
/// the dominant case being junction targets (array-of-entity-ref), which
/// carry no parent FK column at all.
async fn schema_has_fk_column(
    db: &dyn GraphQuerier,
    child_title: &str,
    fk_column: &str,
) -> Result<bool> {
    let props = db.get_properties(child_title).await?;
    Ok(props.iter().any(|p| {
        resolve_field(p).column_name == fk_column
            || p.pg_column_name == fk_column
            // VO→entity chains keep the schema-side column unsuffixed
            // (`person`) while the entity model carries `person_id`.
            || codegraph_core::types::ensure_id_suffix(&p.pg_column_name) == fk_column
    }))
}

/// Resolve the FK column and array flag for a source→target relationship
/// by querying the source entity's properties from the graph.
///
/// Uses `db.get_properties()` which runs GQL internally (`HasProperty` edges),
/// then matches properties by `ref_target` or field name.  This is the same
/// pattern used by `build_composition_node()` in the Grafeo querier.
///
/// `strict` mode (reverse-side checks only): Priorities 2–4 additionally
/// require the matched property to carry a genuine reference edge to the
/// target schema (`ReferencesSchema`), not merely a matching name or column
/// name — a target-side property that merely happens to be named like the
/// source (`deployment`, `deployment_id`) must never verify a back-FK.
///
async fn resolve_fk_via_graph(
    db: &dyn GraphQuerier,
    source_title: &str,
    target_title: &str,
    seg: &str,
    strict: bool,
) -> Result<(String, bool, bool)> {
    // The third field is `verified`: false only for the terminal
    // convention fallback, where the returned column name is fabricated
    // ("{seg}_id") rather than read off a real property of the source
    // schema. Callers must not emit fetches against unverified columns —
    // the generated entity model may not have them (E0599).
    let seg_snake = codegraph_naming::to_snake_case(seg);
    let source_props = db.get_properties(source_title).await.unwrap_or_default();
    // Strict (reverse-side) mode resolves a STORED back-FK column — array
    // properties are junction/child-table relationships with no column on
    // either side, so they never verify one (issue #458: a junction array
    // property must not be matched by the ref-target priority either).
    let source_props: Vec<codegraph_core::types::PropertyNode> = if strict {
        source_props.into_iter().filter(|p| !p.is_array).collect()
    } else {
        source_props
    };

    // Priority 1: property whose ref_target matches target_title (exact).
    // Schema titles often contain spaces while ref targets use filenames without
    // spaces (e.g. title "Campaign Type" vs ref "CampaignType.json").
    // Compare both the raw title and a space-stripped version.
    let target_clean: String = target_title.replace(' ', "");
    let target_stripped: String = codegraph_naming::strip_suffix(&target_clean, "Type");
    for prop in &source_props {
        let matches = prop
            .ref_target
            .as_deref()
            .map(|rt| {
                // Handle both plain title refs ("PersonType") and path refs
                // ("common/json/person/PersonType.json").
                let rt_clean = rt
                    .rsplit('/')
                    .next()
                    .unwrap_or(rt)
                    .strip_suffix(".json#")
                    .or_else(|| rt.strip_suffix(".json"))
                    .unwrap_or(rt);
                rt_clean == target_title || rt_clean == target_clean || rt_clean == target_stripped
            })
            .unwrap_or(false);
        if matches {
            let fd = resolve_field(prop);
            return Ok((fd.column_name, prop.is_array, true));
        }
    }

    // Priority 2: property whose name or rust_field_name matches the segment.
    for prop in &source_props {
        if prop.name.to_lowercase() == seg_snake || prop.rust_field_name.to_lowercase() == seg_snake
        {
            if strict && !property_references(db, prop, source_title, target_title).await {
                continue;
            }
            let fd = resolve_field(prop);
            let mut col_name = fd.column_name;
            // If the property's $ref target is an entity (directly or via allOf),
            // the FK column on the entity model uses _id suffix. This mirrors the
            // entity generator's resolve_fk_column_name logic but uses graph data
            // (is_entity flag) instead of domain config (entity_titles).
            if !col_name.ends_with("_id")
                && let Some(ref_title) = prop.ref_target.as_deref().map(|rt| {
                    rt.rsplit('/')
                        .next()
                        .unwrap_or(rt)
                        .strip_suffix(".json#")
                        .or_else(|| rt.strip_suffix(".json"))
                        .unwrap_or(rt)
                })
                && let Ok(Some(target)) = db.get_property_ref_target(&prop.name, source_title).await
            {
                if target.is_entity && !target.pg_table_name.is_empty() {
                    col_name = codegraph_core::types::ensure_id_suffix(&col_name);
                } else if let Ok(Some(_entity)) =
                    codegraph_core::traits::find_entity_extended_by_vo(db, ref_title).await
                {
                    col_name = codegraph_core::types::ensure_id_suffix(&col_name);
                }
            }
            return Ok((col_name, prop.is_array, true));
        }
    }

    // Priority 3: property whose pg_column_name is "{seg}_id". Identity
    // and audit columns are never FKs — a naming match against one is a
    // false positive (e.g. a `deployment_id` identity column matching a
    // Deployment include).
    let seg_id = format!("{seg_snake}_id");
    for prop in &source_props {
        if prop.is_id || is_audit_column(&prop.pg_column_name) {
            continue;
        }
        if prop.pg_column_name.to_lowercase() == seg_id {
            if strict && !property_references(db, prop, source_title, target_title).await {
                continue;
            }
            let fd = resolve_field(prop);
            return Ok((fd.column_name, prop.is_array, true));
        }
    }

    // Priority 4: property whose pg_column_name is "{parent_ref_stem}_id"
    // where parent_ref_stem is the target_title with spaces stripped and Type suffix removed.
    // This handles array relationships where the child has a generated FK
    // column named after the parent entity (e.g., events_app_id) instead of
    // the schema property name (e.g., public_events_id).
    let parent_ref_stem = codegraph_naming::to_snake_case(&codegraph_naming::strip_suffix(
        &target_title.replace(' ', ""),
        "Type",
    ));
    if parent_ref_stem != seg_snake {
        let parent_seg_id = format!("{parent_ref_stem}_id");
        for prop in &source_props {
            if prop.is_id || is_audit_column(&prop.pg_column_name) {
                continue;
            }
            if prop.pg_column_name.to_lowercase() == parent_seg_id {
                if strict && !property_references(db, prop, source_title, target_title).await {
                    continue;
                }
                let fd = resolve_field(prop);
                return Ok((fd.column_name, prop.is_array, true));
            }
        }
    }

    // Fallback: convention-based default using seg — UNVERIFIED.
    Ok((seg_id, false, false))
}

/// Audit columns (timestamps + soft-delete) are scaffold-owned and never
/// reference FKs — exclude them from column-name-convention FK matching.
fn is_audit_column(pg_column_name: &str) -> bool {
    matches!(
        pg_column_name,
        "created_at" | "updated_at" | "deleted_at" | "deleted_by" | "updated_by"
    )
}

/// Whether the property carries a genuine reference edge resolving to
/// `target_title` — the Priority-1 evidence, required of the looser
/// name/column priorities when resolving in strict (reverse-side) mode.
async fn property_references(
    db: &dyn GraphQuerier,
    prop: &codegraph_core::types::PropertyNode,
    source_title: &str,
    target_title: &str,
) -> bool {
    matches!(
        db.get_property_ref_target(&prop.name, source_title).await,
        Ok(Some(target)) if target.title == target_title
    )
}

/// Derive the response Rust type name for a resolved include path.
///
/// - Single segment: `{TargetEntity}Response`
/// - Multi segment:  `{FirstEntity}CombinedResponse`
fn derive_response_type(segments: &[IncludeSegment]) -> String {
    if segments.len() == 1 {
        format!("{}Response", segments[0].entity_name)
    } else {
        format!("{}CombinedResponse", segments[0].entity_name)
    }
}

/// Resolve the FK column on a child entity that references its parent.
/// Priority: 1) domain config `parent_ref`, 2) graph properties, 3) convention.
async fn resolve_child_fk_column(
    config: &codegraph_config::DomainConfig,
    domain: &str,
    child_title: &str,
    parent_title: &str,
    db: &dyn GraphQuerier,
) -> Result<Option<String>> {
    // Priority 1: parent_ref from the child entity's domain config. Only
    // honored when the config entry's declared `parent` matches the actual
    // parent schema title — otherwise a parent_ref written for a different
    // relationship (e.g. DeploymentType.parent_ref = "worker_type_id" for the
    // DeploymentType → WorkerType link) would leak into FK resolution for an
    // unrelated parent (e.g. a JobType include of deployment would resolve to
    // the worker FK instead of the job FK).
    if let Some(ec) = config
        .domains
        .get(domain)
        .and_then(|d| d.get_entity_config(child_title))
        && let Some(fk) = ec.parent_ref.clone()
        && ec.parent.as_deref().is_some_and(|p| p == parent_title)
    {
        return Ok(Some(fk));
    }

    // Priority 2: graph properties — find the property on the child that
    // references the parent. Only a VERIFIED match (a real property/column
    // on the child) is accepted: the resolver's convention fallback returns
    // "{child_seg}_id" without graph evidence, and emitting a fetch against
    // such a phantom column breaks the generated entity model (E0599).
    let child_seg = codegraph_naming::to_snake_case(super::router::strip_suffix(
        child_title,
        &config.defaults.type_suffix,
    ));
    // Strict (issue #458): the child-side FK must be a genuine reference to
    // the parent — name/column matches alone are decoy-prone.
    let (fk, _, verified) =
        resolve_fk_via_graph(db, child_title, parent_title, &child_seg, true).await?;
    if !verified {
        return Ok(None);
    }
    Ok(Some(fk))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codegraph_config::config::parse_domain_config_str;
    use codegraph_core::mock::MockEngine;
    use codegraph_core::types::PropertyNode;
    use codegraph_core::types::SchemaNode;
    use codegraph_core::types::{DetectionSource, ParentCandidate};

    fn schema_node(title: &str, domain: &str, table: &str, is_entity: bool) -> SchemaNode {
        SchemaNode {
            namespace: None,
            schema_id: format!("{domain}/json/{title}.json"),
            title: title.to_string(),
            description: None,
            schema_type: "object".to_string(),
            classification: if is_entity {
                "entity_reference".to_string()
            } else {
                "value_object".to_string()
            },
            domain: Some(domain.to_string()),
            rel_path: format!("{domain}/json/{title}.json"),
            pg_type: "UUID".to_string(),
            rust_type: "Uuid".to_string(),
            sea_orm_type: "Uuid".to_string(),
            rust_type_name: title.to_string(),
            pg_table_name: table.to_string(),
            api_path_segment: table.to_string(),
            parent_schema: None,
            is_entity,
            is_codelist: false,
            is_primitive_wrapper: false,
            has_all_of: false,
            has_one_of: false,
            has_any_of: false,
            has_definitions: false,
            custom_annotations: Default::default(),
            access: None,
            annotations: None,
        }
    }

    fn ref_property(name: &str, pg_column: &str, ref_target: &str) -> PropertyNode {
        PropertyNode {
            name: name.to_string(),
            prop_type: "string".to_string(),
            description: None,
            format: None,
            is_required: false,
            is_nullable: true,
            is_id: false,
            is_array: false,
            min_items: None,
            max_items: None,
            pattern: None,
            min_length: None,
            max_length: None,
            minimum: None,
            maximum: None,
            pg_column_name: pg_column.to_string(),
            pg_column_type: "UUID".to_string(),
            rust_field_name: pg_column.to_string(),
            rust_field_type: "Uuid".to_string(),
            sea_orm_type: "Uuid".to_string(),
            render_strategy: "entity_reference".to_string(),
            ref_target: Some(ref_target.to_string()),
            classification: Some("entity_reference".to_string()),
            classification_kind: None,
            projection: None,
            ui_override_detail: None,
            ui_override_list_cell: None,
            ui_override_form: None,
            ui_override_inline: None,
            type_expr: None,
        }
    }

    fn string_property(name: &str, pg_column: &str) -> PropertyNode {
        PropertyNode {
            name: name.to_string(),
            prop_type: "string".to_string(),
            description: None,
            format: None,
            is_required: false,
            is_nullable: true,
            is_id: false,
            is_array: false,
            min_items: None,
            max_items: None,
            pattern: None,
            min_length: None,
            max_length: None,
            minimum: None,
            maximum: None,
            pg_column_name: pg_column.to_string(),
            pg_column_type: "TEXT".to_string(),
            rust_field_name: pg_column.to_string(),
            rust_field_type: "String".to_string(),
            sea_orm_type: "Text".to_string(),
            render_strategy: "primitive_wrapper".to_string(),
            ref_target: None,
            classification: None,
            classification_kind: None,
            projection: None,
            ui_override_detail: None,
            ui_override_list_cell: None,
            ui_override_form: None,
            ui_override_inline: None,
            type_expr: None,
        }
    }

    fn array_ref_property(name: &str, pg_column: &str, ref_target: &str) -> PropertyNode {
        PropertyNode {
            is_array: true,
            ..ref_property(name, pg_column, ref_target)
        }
    }

    fn array_string_property(name: &str, pg_column: &str) -> PropertyNode {
        let mut prop = string_property(name, pg_column);
        prop.is_array = true;
        prop
    }

    fn hr_config(entities: &[&str], extra: &str) -> codegraph_config::DomainConfig {
        let entities_list = entities
            .iter()
            .map(|e| format!("\"{e}\""))
            .collect::<Vec<_>>()
            .join(", ");
        parse_domain_config_str(&format!(
            r#"
[defaults]
[domains.hr]
label = "HR"
schema_dir = "hr"
postgres_schema = "hr"
entities = [{entities_list}]
{extra}
"#
        ))
        .unwrap()
    }

    fn deployment_parent_candidate(parent: &str, field: &str) -> ParentCandidate {
        ParentCandidate {
            child_title: "DeploymentType".to_string(),
            parent_title: parent.to_string(),
            field_name: field.to_string(),
            source: DetectionSource::ScalarRef,
        }
    }

    /// Auto-discovered parent-candidate includes must resolve as array/reverse
    /// semantics: the FK column is read from the CHILD (target) model, never
    /// from the source — `deployment.worker_type_id == job.id`, not
    /// `job.worker_type_id`. Regression test for 99da338, which dropped the
    /// array default and flipped these includes to forward-single semantics,
    /// producing uncompilable `repository_impl.rs`.
    #[tokio::test]
    async fn auto_reverse_include_resolves_as_array_with_child_fk() {
        let engine = MockEngine::builder()
            .with_schema(schema_node("JobType", "hr", "job", true))
            .with_schema(schema_node("DeploymentType", "hr", "deployment", true))
            .with_properties(
                "DeploymentType",
                vec![ref_property("worker_type_id", "worker_type_id", "JobType")],
            )
            .with_ref_target(
                "worker_type_id",
                "DeploymentType",
                schema_node("JobType", "hr", "job", true),
            )
            .with_parent_candidate(deployment_parent_candidate("JobType", "worker_type_id"))
            .build();

        let config = hr_config(&["JobType", "DeploymentType"], "");
        let paths = resolve_include_paths(&engine, &config, "hr", "JobType", None)
            .await
            .unwrap();

        let deployment = paths
            .iter()
            .find(|p| p.alias == "deployment")
            .expect("auto-discovery should include a 'deployment' path");
        let seg = &deployment.segments[0];
        assert!(
            seg.is_array,
            "reverse include must resolve with array semantics (fetch scans the child table)"
        );
        assert_eq!(
            seg.fk_column, "worker_type_id",
            "FK must be read from the CHILD model (deployment.worker_type_id), not the source"
        );
        assert_eq!(seg.reverse_fk_column, "worker_type_id");
    }

    /// End-to-end: the repository code generated for an auto-discovered reverse
    /// include must filter the child table by the child-side FK column and must
    /// NOT read the FK off the source (parent) model.
    #[tokio::test]
    async fn auto_reverse_include_emits_child_side_fk_fetch() {
        let engine = MockEngine::builder()
            .with_schema(schema_node("JobType", "hr", "job", true))
            .with_schema(schema_node("DeploymentType", "hr", "deployment", true))
            .with_properties(
                "DeploymentType",
                vec![ref_property("worker_type_id", "worker_type_id", "JobType")],
            )
            .with_ref_target(
                "worker_type_id",
                "DeploymentType",
                schema_node("JobType", "hr", "job", true),
            )
            .with_parent_candidate(deployment_parent_candidate("JobType", "worker_type_id"))
            .build();

        let config = hr_config(&["JobType", "DeploymentType"], "");
        let paths = resolve_include_paths(&engine, &config, "hr", "JobType", None)
            .await
            .unwrap();

        let code = crate::ddd::repository_emitter::RepositoryImplEmitter
            .emit(
                &engine,
                "JobType",
                "hr",
                &config,
                None,
                &paths,
                &crate::ProjectConfig::default(),
            )
            .await
            .unwrap();

        assert!(
            code.contains("Column::WorkerTypeId.eq(source_id)"),
            "fetch must filter the child table by the child-side FK. Got:\n{code}"
        );
        assert!(
            !code.contains("source.worker_type_id"),
            "must not read the FK off the SOURCE (job) model — it lives on the child. Got:\n{code}"
        );
    }

    /// `resolve_child_fk_column` must honor a config `parent_ref` only when the
    /// config's `parent` matches the actual parent schema title. Here the
    /// DeploymentType entry declares parent = WorkerType (parent_ref =
    /// worker_type_id); resolving the DeploymentType → JobType link must NOT
    /// apply it and must fall through to the graph (job_type_id).
    #[tokio::test]
    async fn child_fk_column_ignores_parent_ref_for_other_parent() {
        let engine = MockEngine::builder()
            .with_schema(schema_node("JobType", "hr", "job", true))
            .with_schema(schema_node("WorkerType", "hr", "worker", true))
            .with_schema(schema_node("DeploymentType", "hr", "deployment", true))
            .with_properties(
                "DeploymentType",
                vec![
                    ref_property("worker_type_id", "worker_type_id", "WorkerType"),
                    ref_property("job_type_id", "job_type_id", "JobType"),
                ],
            )
            .with_ref_target(
                "worker_type_id",
                "DeploymentType",
                schema_node("WorkerType", "hr", "worker", true),
            )
            .with_ref_target(
                "job_type_id",
                "DeploymentType",
                schema_node("JobType", "hr", "job", true),
            )
            .build();

        let config = hr_config(
            &["JobType", "WorkerType", "DeploymentType"],
            r#"
[domains.hr.entity_config.DeploymentType]
role = "child"
parent = "WorkerType"
parent_ref = "worker_type_id"
"#,
        );

        let fk = resolve_child_fk_column(&config, "hr", "DeploymentType", "JobType", &engine)
            .await
            .unwrap();
        assert_eq!(
            fk.as_deref(),
            Some("job_type_id"),
            "parent_ref for the DeploymentType → WorkerType link must not leak into the JobType relationship"
        );
    }

    /// When the config's `parent` matches the actual parent, `parent_ref` still
    /// applies directly.
    #[tokio::test]
    async fn child_fk_column_applies_parent_ref_when_parent_matches() {
        let engine = MockEngine::builder()
            .with_schema(schema_node("WorkerType", "hr", "worker", true))
            .with_schema(schema_node("DeploymentType", "hr", "deployment", true))
            .with_properties(
                "DeploymentType",
                vec![ref_property(
                    "worker_type_id",
                    "worker_type_id",
                    "WorkerType",
                )],
            )
            .with_ref_target(
                "worker_type_id",
                "DeploymentType",
                schema_node("WorkerType", "hr", "worker", true),
            )
            .build();

        let config = hr_config(
            &["WorkerType", "DeploymentType"],
            r#"
[domains.hr.entity_config.DeploymentType]
role = "child"
parent = "WorkerType"
parent_ref = "worker_type_id"
"#,
        );

        let fk = resolve_child_fk_column(&config, "hr", "DeploymentType", "WorkerType", &engine)
            .await
            .unwrap();
        assert_eq!(
            fk.as_deref(),
            Some("worker_type_id"),
            "parent_ref applies when the config parent matches the actual parent"
        );
    }

    /// A candidate whose child owns no property referencing the parent is a
    /// phantom edge (derived from the wrong direction of a relationship):
    /// resolve_child_fk_column must report None instead of a
    /// convention-fabricated column, so the caller skips it (the fabricated
    /// column does not exist on the child entity — E0599 in generated code).
    #[tokio::test]
    async fn child_fk_column_returns_none_for_phantom_edge() {
        let engine = MockEngine::builder()
            .with_schema(schema_node("Job", "hr", "job", true))
            .with_schema(schema_node("Deployment", "hr", "deployment", true))
            // Deployment references Job — but Job has no property
            // referencing Deployment, so a (child: Job, parent:
            // Deployment) candidate is a phantom.
            .with_properties("Deployment", vec![ref_property("job_id", "job_id", "Job")])
            .with_ref_target(
                "job_id",
                "Deployment",
                schema_node("Job", "hr", "job", true),
            )
            .build();

        let config = hr_config(&["Job", "Deployment"], "");

        let fk = resolve_child_fk_column(&config, "hr", "Job", "Deployment", &engine)
            .await
            .unwrap();
        assert_eq!(
            fk, None,
            "phantom edge must resolve to None, not a fabricated {{parent}}_id column"
        );
    }

    /// Issue #458: a target-side property merely NAMED like the source
    /// ("deployment" / "deployment_id") must NOT verify a reverse FK. Here
    /// DeploymentType array-references JobType (`assignments`, the ItemsOf
    /// shape — no ReferencesSchema edge) and scalar-references it (`job`, the
    /// edge that makes JobType discoverable). JobType carries two decoys and
    /// NO genuine FK to DeploymentType: an array property named `deployment`
    /// and a plain TEXT column `deployment_id`. The name/column match must
    /// not count as a verified back-FK — the array include must be skipped,
    /// never resolved with `Column::Deployment`/`DeploymentId` on core_job.
    #[tokio::test]
    async fn array_entity_reference_requires_genuine_reverse_reference() {
        let mut deployment = schema_node("DeploymentType", "hr", "deployment", true);
        deployment.rust_type_name = "Deployment".to_string();
        let mut job = schema_node("JobType", "hr", "job", true);
        job.rust_type_name = "Job".to_string();
        let engine = MockEngine::builder()
            .with_schema(deployment.clone())
            .with_schema(job.clone())
            .with_properties(
                "DeploymentType",
                vec![
                    array_ref_property("assignments", "assignments", "JobType.json"),
                    ref_property("job", "job_id", "JobType.json"),
                ],
            )
            .with_ref_target("job", "DeploymentType", job.clone())
            .with_properties(
                "JobType",
                vec![
                    array_string_property("deployment", "deployment"),
                    string_property("deployment_id", "deployment_id"),
                ],
            )
            .build();

        let config = hr_config(&["DeploymentType", "JobType"], "");
        let paths = resolve_include_paths(&engine, &config, "hr", "DeploymentType", None)
            .await
            .unwrap();

        let wrong_side: Vec<&str> = paths
            .iter()
            .flat_map(|p| p.segments.iter())
            .filter(|s| s.schema_title == "JobType")
            .map(|s| s.reverse_fk_column.as_str())
            .collect();
        assert!(
            wrong_side.is_empty(),
            "include targeting JobType must not resolve with a decoy reverse FK \
             (no genuine JobType→DeploymentType reference exists); got {wrong_side:?} \
             — the emitter would filter core_job::Column::Deployment(Id) (E0599)"
        );
        assert!(
            !paths
                .iter()
                .any(|p| p.segments[0].schema_title == "JobType"),
            "array cross-reference without a genuine back-FK must be skipped entirely"
        );
    }
}
