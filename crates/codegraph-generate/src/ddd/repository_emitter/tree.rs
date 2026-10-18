use crate::api::include_path::ResolvedIncludePath;
use crate::code_writer::{CodeWriter, wln};

use super::child::{emit_child_reads, include_hydration_children};
use super::dto::{emit_child_field_population, emit_entity_to_dto_field};
use super::helpers::q;
use super::junction::{emit_junction_field_population, emit_junction_reads};
use super::{EntityTree, RepositoryImplEmitter};

impl RepositoryImplEmitter {
    /// Emit `find_tree` — recursive CTE fetching the subtree rooted at `root_id`.
    pub(crate) fn emit_find_tree_fn(&self, tree: &EntityTree, code: &mut CodeWriter) {
        let hf = tree.hierarchy_field.as_deref().unwrap();
        let has_tree_include = !tree.tree_include.is_empty();
        wln!(code);
        wln!(
            code,
            "    #[tracing::instrument(skip(self, db), fields(db.operation = \"find_tree\", db.table = \"{}.{}\"))]",
            tree.schema_name,
            tree.table_name
        );
        wln!(code, "    async fn find_tree(");
        wln!(code, "        &self,");
        wln!(code, "        db: &DatabaseTransaction,");
        wln!(code, "        root_id: Uuid,");
        wln!(code, "        max_depth: Option<i32>,");
        if has_tree_include {
            wln!(
                code,
                "    ) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {{"
            );
        } else {
            wln!(
                code,
                "    ) -> Result<Vec<{}Response>, Box<dyn std::error::Error>> {{",
                tree.entity_name
            );
        }
        wln!(code, "        let sql = if let Some(_depth) = max_depth {{");
        wln!(
            code,
            "            format!(\"WITH RECURSIVE tree AS (SELECT *, 0 AS _tree_depth FROM {schema}.{table} WHERE id = $1 UNION ALL SELECT c.*, t._tree_depth + 1 AS _tree_depth FROM {schema}.{table} c JOIN tree t ON c.{hf} = t.id WHERE t._tree_depth < $2) SELECT * FROM tree ORDER BY _tree_depth, created_at\",)",
            schema = tree.schema_name,
            table = q(&tree.table_name),
            hf = hf
        );
        wln!(code, "        }} else {{");
        wln!(
            code,
            "            format!(\"WITH RECURSIVE tree AS (SELECT *, 0 AS _tree_depth FROM {schema}.{table} WHERE id = $1 UNION ALL SELECT c.*, t._tree_depth + 1 AS _tree_depth FROM {schema}.{table} c JOIN tree t ON c.{hf} = t.id) SELECT * FROM tree ORDER BY _tree_depth, created_at\",)",
            schema = tree.schema_name,
            table = q(&tree.table_name),
            hf = hf
        );
        wln!(code, "        }};");
        wln!(
            code,
            "        let values = if let Some(depth) = max_depth {{"
        );
        wln!(code, "            vec![root_id.into(), depth.into()]");
        wln!(code, "        }} else {{");
        wln!(code, "            vec![root_id.into()]");
        wln!(code, "        }};");
        wln!(
            code,
            "        let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values);"
        );
        wln!(
            code,
            "        let rows = crate::entity::{}::Entity::find()",
            tree.entity_module
        );
        wln!(code, "            .from_raw_sql(stmt)");
        wln!(code, "            .all(db)");
        wln!(code, "            .await?;");
        wln!(code);

        if has_tree_include {
            // Emit worker map query
            self.emit_tree_include_worker_fetch(tree, code);
        }

        // Emit result construction (Vec<Response> or Vec<Value>)
        wln!(
            code,
            "        let mut results = Vec::with_capacity(rows.len());"
        );
        wln!(code, "        for row in rows {{");
        emit_child_reads(code, &tree.child_tables, "row.id", 3);
        emit_junction_reads(code, &tree.junction_tables, "row.id", 3);
        if has_tree_include {
            wln!(
                code,
                "            let mut val = serde_json::to_value({}Response {{",
                tree.entity_name
            );
        } else {
            wln!(
                code,
                "            results.push({}Response {{",
                tree.entity_name
            );
        }
        wln!(code, "                id: row.id,");
        for col in &tree.direct_columns {
            if col.is_composite_range {
                continue;
            }
            if col.field_name == "created_at" || col.field_name == "updated_at" {
                continue;
            }
            emit_entity_to_dto_field(code, col, "row", "                ");
        }
        emit_child_field_population(code, &tree.child_tables, "                ");
        emit_junction_field_population(code, &tree.junction_tables, "                ");
        if tree.has_workflow {
            wln!(code, "                workflow_state: None,");
        }
        wln!(code, "                created_at: row.created_at,");
        if !tree.append_only {
            wln!(code, "                updated_at: row.updated_at,");
        }
        wln!(code, "                ..Default::default()");
        if has_tree_include {
            wln!(
                code,
                "            }}).map_err(|e| -> Box<dyn std::error::Error> {{ format!(\"Serialization error: {{e}}\").into() }})?;"
            );
            // Emit worker merge block
            for inc in &tree.tree_include {
                wln!(
                    code,
                    "            if let Some(worker) = worker_map.get(&row.id) {{"
                );
                wln!(
                    code,
                    "                val.as_object_mut().unwrap().insert(\"{}\".to_string(), worker.clone());",
                    inc.alias
                );
                wln!(code, "            }}");
            }
            wln!(code, "            results.push(val);");
        } else {
            wln!(code, "            }});");
        }
        wln!(code, "        }}");
        wln!(code);
        wln!(code, "        Ok(results)");
        wln!(code, "    }}");
    }

    /// Emit code that fetches tree_include worker data into a position_id→Value map.
    fn emit_tree_include_worker_fetch(&self, tree: &EntityTree, code: &mut CodeWriter) {
        wln!(
            code,
            "        let mut worker_map: std::collections::HashMap<Uuid, serde_json::Value> = std::collections::HashMap::new();"
        );
        wln!(
            code,
            "        let pos_ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();"
        );
        wln!(code);
        for inc in &tree.tree_include {
            // Build JOIN chain from parent table through worker detail tables.
            // The composition tree tells us: WorkerType → WorkerPersonType → WorkerPersonNameType.
            // We assign fixed aliases: w (worker), wp (worker_person), wpn (worker_person_name).
            let mut from_clause = format!(
                "{} d JOIN {} w ON w.id = d.\"{}\" AND w.deleted_at IS NULL",
                inc.via_table, inc.parent_table, inc.parent_ref_column
            );
            let mut has_person = false;
            // worker_detail_joins is [person→worker, name→person] after reverse.
            // Index 0 (person): alias "wp", parent alias "w" (worker)
            // Index 1 (name): alias "wpn", parent alias "wp" (person)
            for (i, (table, fk_col, parent_alias)) in inc.worker_detail_joins.iter().enumerate() {
                let alias = if i == 0 { "wp" } else { "wpn" };
                from_clause.push_str(&format!(
                    " JOIN {} {} ON {}.{} = {}.id",
                    table, alias, alias, fk_col, parent_alias
                ));
                has_person = true;
            }
            if has_person {
                // Escape double-quotes in from_clause for embedding in Rust string literals
                let escaped_from = from_clause.replace('"', "\\\"");
                wln!(
                    code,
                    "        let worker_sql = format!(\"SELECT d.\\\"{fk}\\\" AS position_id, jsonb_build_object('id', w.id, 'given_name', wpn.given, 'family_name', wpn.family, 'avatar_url', wp.avatar_url) AS deployed_worker FROM {from} WHERE d.\\\"{fk}\\\" = ANY($1) AND d.deleted_at IS NULL\");",
                    fk = inc.via_fk_column,
                    from = escaped_from,
                );
            } else {
                // Fallback: no person chain, just return worker ID
                wln!(
                    code,
                    "        let worker_sql = format!(\"SELECT d.\\\"{fk}\\\" AS position_id, jsonb_build_object('id', w.id) AS deployed_worker FROM {from} WHERE d.\\\"{fk}\\\" = ANY($1) AND d.deleted_at IS NULL\");",
                    fk = inc.via_fk_column,
                    from = from_clause,
                );
            }
            wln!(
                code,
                "        let worker_stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, worker_sql, vec![pos_ids.clone().into()]);"
            );
            wln!(
                code,
                "        let worker_rows = db.query_all_raw(worker_stmt).await?;"
            );
            wln!(code, "        for wr in &worker_rows {{");
            wln!(
                code,
                "            let pos_id: Uuid = wr.try_get_by_index(0).map_err(|e| -> Box<dyn std::error::Error> {{ format!(\"Missing position_id: {{e}}\").into() }})?;"
            );
            wln!(
                code,
                "            let worker_json: serde_json::Value = wr.try_get_by_index(1).map_err(|e| -> Box<dyn std::error::Error> {{ format!(\"Missing deployed_worker: {{e}}\").into() }})?;"
            );
            wln!(code, "            worker_map.insert(pos_id, worker_json);");
            wln!(code, "        }}");
            wln!(code);
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn emit_include_fetch_methods(
        &self,
        tree: &EntityTree,
        code: &mut CodeWriter,
        include_paths: &[ResolvedIncludePath],
        include_target_trees: &[Option<EntityTree>],
        include_segment_dto_fields: &[Vec<Vec<String>>],
        include_segment_col_fields: &[Vec<Vec<String>>],
        include_segment_is_structured: &[Vec<Vec<bool>>],
        include_segment_is_codelist: &[Vec<Vec<bool>>],
        include_segment_dto_rust_types: &[Vec<Vec<Option<String>>>],
        include_segment_is_nullable: &[Vec<Vec<bool>>],
        include_segment_col_rust_types: &[Vec<Vec<String>>],
    ) {
        for (idx, path) in include_paths.iter().enumerate() {
            let target_tree = include_target_trees.get(idx).and_then(|t| t.as_ref());
            let per_seg_dto = include_segment_dto_fields
                .get(idx)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let per_seg_col = include_segment_col_fields
                .get(idx)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let per_seg_structured = include_segment_is_structured
                .get(idx)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let per_seg_codelist = include_segment_is_codelist
                .get(idx)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let per_seg_dto_types = include_segment_dto_rust_types
                .get(idx)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let per_seg_is_nullable = include_segment_is_nullable
                .get(idx)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let per_seg_col_types = include_segment_col_rust_types
                .get(idx)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            if path.segments.len() == 1 {
                let dto_fields = per_seg_dto.first().map(|v| v.as_slice()).unwrap_or(&[]);
                let col_fields = per_seg_col.first().map(|v| v.as_slice()).unwrap_or(&[]);
                let is_structured = per_seg_structured
                    .first()
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let is_codelist = per_seg_codelist
                    .first()
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let dto_rust_types = per_seg_dto_types
                    .first()
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let is_nullable = per_seg_is_nullable
                    .first()
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let col_rust_types = per_seg_col_types
                    .first()
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                self.emit_single_fetch_method(
                    tree,
                    code,
                    path,
                    target_tree,
                    dto_fields,
                    col_fields,
                    is_structured,
                    is_codelist,
                    dto_rust_types,
                    is_nullable,
                    col_rust_types,
                );
                self.emit_batch_fetch_method(
                    tree,
                    code,
                    path,
                    target_tree,
                    dto_fields,
                    col_fields,
                    is_structured,
                    is_codelist,
                    dto_rust_types,
                    is_nullable,
                    col_rust_types,
                );
            } else {
                let intermediate_dto = per_seg_dto.first().map(|v| v.as_slice()).unwrap_or(&[]);
                let leaf_dto = per_seg_dto.get(1).map(|v| v.as_slice()).unwrap_or(&[]);
                let intermediate_col = per_seg_col.first().map(|v| v.as_slice()).unwrap_or(&[]);
                let leaf_col = per_seg_col.get(1).map(|v| v.as_slice()).unwrap_or(&[]);
                let intermediate_structured = per_seg_structured
                    .first()
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let intermediate_codelist = per_seg_codelist
                    .first()
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let leaf_structured = per_seg_structured
                    .get(1)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let leaf_codelist = per_seg_codelist.get(1).map(|v| v.as_slice()).unwrap_or(&[]);
                let intermediate_dto_types = per_seg_dto_types
                    .first()
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let leaf_dto_types = per_seg_dto_types
                    .get(1)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let intermediate_nullable = per_seg_is_nullable
                    .first()
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let leaf_nullable = per_seg_is_nullable
                    .get(1)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let intermediate_col_types = per_seg_col_types
                    .first()
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let leaf_col_types = per_seg_col_types
                    .get(1)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                self.emit_dot_fetch_method(
                    tree,
                    code,
                    path,
                    intermediate_dto,
                    intermediate_col,
                    intermediate_structured,
                    intermediate_codelist,
                    intermediate_dto_types,
                    intermediate_nullable,
                    intermediate_col_types,
                    leaf_dto,
                    leaf_col,
                    leaf_structured,
                    leaf_codelist,
                    leaf_dto_types,
                    leaf_nullable,
                    leaf_col_types,
                );
                self.emit_dot_batch_fetch_method(tree, code, path);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// Like emit_field_assignments but applies the same type conversions as
    /// emit_entity_to_dto_field — serde_json::from_value() for structured
    /// wrappers, .parse() for codelists, direct assignment otherwise.
    /// When the entity column's rust type differs from the DTO field type
    /// (e.g. a Uuid FK column hydrating a TEXT-mapped String DTO field,
    /// issue #459), the row value is adapted to the DTO type.
    fn emit_field_assignments_typed(
        code: &mut CodeWriter,
        row_var: &str,
        dto_fields: &[String],
        col_fields: &[String],
        is_structured: &[bool],
        is_codelist: &[bool],
        dto_rust_types: &[Option<String>],
        is_nullable: &[bool],
        col_rust_types: &[String],
    ) {
        for i in 0..dto_fields.len() {
            let dto_name = &dto_fields[i];
            let col_name = &col_fields[i];
            if i < is_structured.len() && is_structured[i] {
                // StructuredWrapper: serde_json::from_value() or .and_then() for nullable
                if i < is_nullable.len() && is_nullable[i] {
                    wln!(
                        code,
                        "                {dto_name}: {row_var}.{col_name}.and_then(|v| serde_json::from_value(v).ok()),",
                    );
                } else {
                    wln!(
                        code,
                        "                {dto_name}: serde_json::from_value({row_var}.{col_name}).unwrap_or_default(),",
                    );
                }
            } else if i < is_codelist.len()
                && is_codelist[i]
                && i < dto_rust_types.len()
                && dto_rust_types[i].is_some()
            {
                // Codelist: .parse(). Required columns are non-nullable, so
                // parse directly instead of Option-style and_then.
                if i < is_nullable.len() && is_nullable[i] {
                    wln!(
                        code,
                        "                {dto_name}: {row_var}.{col_name}.and_then(|v| v.parse().ok()),",
                    );
                } else {
                    wln!(
                        code,
                        "                {dto_name}: {row_var}.{col_name}.parse().unwrap_or_default(),",
                    );
                }
            } else {
                // Plain column: adapt when the entity column type differs
                // from the DTO field type (issue #459 — e.g. a Uuid FK
                // column hydrating a TEXT-mapped String DTO field).
                let col_ty = col_rust_types.get(i).map(String::as_str);
                let dto_ty = dto_rust_types.get(i).and_then(Option::as_deref).or(col_ty);
                let nullable = i < is_nullable.len() && is_nullable[i];
                match (col_ty, dto_ty) {
                    (Some("Uuid"), Some("String")) if nullable => wln!(
                        code,
                        "                {dto_name}: {row_var}.{col_name}.map(|v| v.to_string()),",
                    ),
                    (Some("Uuid"), Some("String")) => wln!(
                        code,
                        "                {dto_name}: {row_var}.{col_name}.to_string(),",
                    ),
                    (Some("String"), Some(other)) if other != "String" && nullable => wln!(
                        code,
                        "                {dto_name}: {row_var}.{col_name}.and_then(|v| v.parse().ok()),",
                    ),
                    (Some("String"), Some(other)) if other != "String" => wln!(
                        code,
                        "                {dto_name}: {row_var}.{col_name}.parse().unwrap_or_default(),",
                    ),
                    _ => wln!(code, "                {dto_name}: {row_var}.{col_name},"),
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn emit_single_fetch_method(
        &self,
        tree: &EntityTree,
        code: &mut CodeWriter,
        path: &ResolvedIncludePath,
        target_tree: Option<&EntityTree>,
        dto_fields: &[String],
        col_fields: &[String],
        is_structured: &[bool],
        is_codelist: &[bool],
        dto_rust_types: &[Option<String>],
        is_nullable: &[bool],
        col_rust_types: &[String],
    ) {
        let seg = &path.segments[0];
        let src_module = &tree.entity_module;
        let resp_type = &path.response_rust_type;
        let target_module = format!("{}_{}", seg.domain, seg.module_name);

        wln!(code);
        wln!(code, "    pub(crate) async fn {}(", path.fetch_method);
        wln!(code, "        &self,");
        wln!(code, "        db: &DatabaseTransaction,");
        wln!(code, "        source_id: Uuid,");
        if seg.is_array {
            wln!(
                code,
                "    ) -> Result<Vec<{}>, Box<dyn std::error::Error>> {{",
                resp_type
            );
        } else {
            wln!(
                code,
                "    ) -> Result<Option<{}>, Box<dyn std::error::Error>> {{",
                resp_type
            );
        }

        // Which child tables the response hydrates: the override subtree's
        // nested children (scoped responses, #162 phase 2) or the target
        // tree's own children (entity-native responses, #161).
        let hydration_children = include_hydration_children(tree, path, target_tree);

        if let Some(ref over) = seg.child_table_override {
            // VO→entity: query child table directly by parent FK
            let over_fk_pascal = codegraph_naming::to_pascal_case(&over.parent_fk_column);
            wln!(
                code,
                "        let target = crate::entity::{}::Entity::find()",
                over.child_module,
            );
            wln!(
                code,
                "            .filter(crate::entity::{}::Column::{}.eq(source_id))",
                over.child_module,
                over_fk_pascal,
            );
            wln!(code, "            .one(db)");
            wln!(code, "            .await?;");
            wln!(code, "        let target = match target {{");
            wln!(code, "            Some(t) => t,");
            wln!(code, "            None => return Ok(None),");
            wln!(code, "        }};");
            if !hydration_children.is_empty() {
                emit_child_reads(code, hydration_children, "target.id", 2);
            }
            wln!(code, "        Ok(Some({} {{", resp_type);
            Self::emit_field_assignments_typed(
                code,
                "target",
                dto_fields,
                col_fields,
                is_structured,
                is_codelist,
                dto_rust_types,
                is_nullable,
                col_rust_types,
            );
            if !hydration_children.is_empty() {
                emit_child_field_population(code, hydration_children, "            ");
            }
            wln!(code, "            ..Default::default()");
            wln!(code, "        }}))");
        } else if seg.is_array {
            let reverse_fk_pascal = codegraph_naming::to_pascal_case(&seg.reverse_fk_column);
            wln!(
                code,
                "        let rows = crate::entity::{}::Entity::find()",
                target_module
            );
            wln!(
                code,
                "            .filter(crate::entity::{}::Column::{}.eq(source_id))",
                target_module,
                reverse_fk_pascal
            );
            wln!(code, "            .all(db)");
            wln!(code, "            .await?;");
            wln!(
                code,
                "        let mut results = Vec::with_capacity(rows.len());"
            );
            wln!(code, "        for row in rows {{");
            if !hydration_children.is_empty() {
                emit_child_reads(code, hydration_children, "row.id", 3);
            }
            wln!(code, "            results.push({} {{", resp_type);
            wln!(code, "                id: row.id,");
            Self::emit_field_assignments_typed(
                code,
                "row",
                dto_fields,
                col_fields,
                is_structured,
                is_codelist,
                dto_rust_types,
                is_nullable,
                col_rust_types,
            );
            wln!(code, "                created_at: row.created_at,");
            if !target_tree.is_some_and(|t| t.append_only) {
                wln!(code, "                updated_at: row.updated_at,");
            }
            if !hydration_children.is_empty() {
                emit_child_field_population(code, hydration_children, "                ");
            }
            wln!(code, "                ..Default::default()");
            wln!(code, "            }});");
            wln!(code, "        }}");
            wln!(code, "        Ok(results)");
        } else {
            wln!(
                code,
                "        let source = crate::entity::{}::Entity::find()",
                src_module
            );
            wln!(
                code,
                "            .filter(crate::entity::{}::Column::Id.eq(source_id))",
                src_module
            );
            wln!(code, "            .one(db)");
            wln!(code, "            .await?;");
            wln!(code, "        let source = match source {{");
            wln!(code, "            Some(s) => s,");
            wln!(code, "            None => return Ok(None),");
            wln!(code, "        }};");
            if seg.fk_is_required {
                // Required genuine EntityReference: the FK field is a plain Uuid.
                wln!(code, "        let fk_value = source.{};", seg.fk_column);
            } else {
                wln!(
                    code,
                    "        let fk_value = match source.{} {{",
                    seg.fk_column
                );
                wln!(code, "            Some(v) => v,");
                wln!(code, "            None => return Ok(None),");
                wln!(code, "        }};");
            }
            wln!(
                code,
                "        let target = crate::entity::{}::Entity::find()",
                target_module
            );
            wln!(
                code,
                "            .filter(crate::entity::{}::Column::Id.eq(fk_value))",
                target_module
            );
            wln!(code, "            .one(db)");
            wln!(code, "            .await?;");
            wln!(code, "        let target = match target {{");
            wln!(code, "            Some(t) => t,");
            wln!(code, "            None => return Ok(None),");
            wln!(code, "        }};");
            if !hydration_children.is_empty() {
                emit_child_reads(code, hydration_children, "target.id", 2);
            }
            wln!(code, "        Ok(Some({} {{", resp_type);
            wln!(code, "            id: target.id,");
            Self::emit_field_assignments_typed(
                code,
                "target",
                dto_fields,
                col_fields,
                is_structured,
                is_codelist,
                dto_rust_types,
                is_nullable,
                col_rust_types,
            );
            wln!(code, "            created_at: target.created_at,");
            wln!(code, "            updated_at: target.updated_at,");
            if !hydration_children.is_empty() {
                emit_child_field_population(code, hydration_children, "            ");
            }
            wln!(code, "            ..Default::default()");
            wln!(code, "        }}))");
        }

        wln!(code, "    }}");
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn emit_batch_fetch_method(
        &self,
        tree: &EntityTree,
        code: &mut CodeWriter,
        path: &ResolvedIncludePath,
        target_tree: Option<&EntityTree>,
        dto_fields: &[String],
        col_fields: &[String],
        is_structured: &[bool],
        is_codelist: &[bool],
        dto_rust_types: &[Option<String>],
        is_nullable: &[bool],
        col_rust_types: &[String],
    ) {
        let seg = &path.segments[0];
        let src_module = &tree.entity_module;
        let resp_type = &path.response_rust_type;
        let target_module = format!("{}_{}", seg.domain, seg.module_name);
        // Which child tables the response hydrates (see single fetch, #162).
        let hydration_children = include_hydration_children(tree, path, target_tree);

        wln!(code);
        wln!(code, "    pub(crate) async fn {}(", path.batch_fetch_method);
        wln!(code, "        &self,");
        wln!(code, "        db: &DatabaseTransaction,");
        wln!(code, "        source_ids: &[Uuid],");
        if seg.is_array {
            wln!(
                code,
                "    ) -> Result<std::collections::HashMap<Uuid, Vec<{}>>, Box<dyn std::error::Error>> {{",
                resp_type
            );
        } else {
            wln!(
                code,
                "    ) -> Result<std::collections::HashMap<Uuid, Option<{}>>, Box<dyn std::error::Error>> {{",
                resp_type
            );
        }

        if let Some(ref over) = seg.child_table_override {
            let over_fk_pascal = codegraph_naming::to_pascal_case(&over.parent_fk_column);
            wln!(
                code,
                "        let rows = crate::entity::{}::Entity::find()",
                over.child_module,
            );
            wln!(
                code,
                "            .filter(crate::entity::{}::Column::{}.is_in(source_ids.to_vec()))",
                over.child_module,
                over_fk_pascal,
            );
            wln!(code, "            .all(db)");
            wln!(code, "            .await?;");
            wln!(
                code,
                "        let mut result: std::collections::HashMap<Uuid, Option<{}>> = std::collections::HashMap::new();",
                resp_type
            );
            wln!(code, "        for id in source_ids {{");
            wln!(code, "            result.entry(*id).or_insert(None);");
            wln!(code, "        }}");
            wln!(code, "        for row in rows {{");
            if !hydration_children.is_empty() {
                emit_child_reads(code, hydration_children, "row.id", 3);
            }
            wln!(
                code,
                "            result.insert(row.{}, Some({} {{",
                over.parent_fk_column,
                resp_type,
            );
            Self::emit_field_assignments_typed(
                code,
                "row",
                dto_fields,
                col_fields,
                is_structured,
                is_codelist,
                dto_rust_types,
                is_nullable,
                col_rust_types,
            );
            if !hydration_children.is_empty() {
                emit_child_field_population(code, hydration_children, "                ");
            }
            wln!(code, "                ..Default::default()");
            wln!(code, "            }}));");
            wln!(code, "        }}");
        } else if seg.is_array {
            let reverse_fk_pascal = codegraph_naming::to_pascal_case(&seg.reverse_fk_column);
            wln!(
                code,
                "        let rows = crate::entity::{}::Entity::find()",
                target_module
            );
            wln!(
                code,
                "            .filter(crate::entity::{}::Column::{}.is_in(source_ids.to_vec()))",
                target_module,
                reverse_fk_pascal
            );
            wln!(code, "            .all(db)");
            wln!(code, "            .await?;");
            wln!(
                code,
                "        let mut result: std::collections::HashMap<Uuid, Vec<{}>> = std::collections::HashMap::new();",
                resp_type
            );
            wln!(code, "        for id in source_ids {{");
            wln!(
                code,
                "            result.entry(*id).or_insert_with(Vec::new);"
            );
            wln!(code, "        }}");
            wln!(code, "        for row in rows {{");
            if seg.reverse_fk_is_required {
                // Required reverse FK (genuine EntityReference): plain Uuid.
                wln!(code, "            let key = row.{};", seg.reverse_fk_column);
            } else {
                wln!(
                    code,
                    "            let key = match row.{} {{",
                    seg.reverse_fk_column
                );
                wln!(code, "                Some(v) => v,");
                wln!(code, "                None => continue,");
                wln!(code, "            }};");
            }
            if !hydration_children.is_empty() {
                emit_child_reads(code, hydration_children, "row.id", 3);
            }
            wln!(
                code,
                "            result.entry(key).or_default().push({} {{",
                resp_type
            );
            wln!(code, "                id: row.id,");
            Self::emit_field_assignments_typed(
                code,
                "row",
                dto_fields,
                col_fields,
                is_structured,
                is_codelist,
                dto_rust_types,
                is_nullable,
                col_rust_types,
            );
            wln!(code, "                created_at: row.created_at,");
            if !target_tree.is_some_and(|t| t.append_only) {
                wln!(code, "                updated_at: row.updated_at,");
            }
            if !hydration_children.is_empty() {
                emit_child_field_population(code, hydration_children, "                ");
            }
            wln!(code, "                ..Default::default()");
            wln!(code, "            }});");
            wln!(code, "        }}");
        } else {
            wln!(
                code,
                "        let sources = crate::entity::{}::Entity::find()",
                src_module
            );
            wln!(
                code,
                "            .filter(crate::entity::{}::Column::Id.is_in(source_ids.to_vec()))",
                src_module
            );
            wln!(code, "            .all(db)");
            wln!(code, "            .await?;");
            wln!(code, "        let mut fk_values: Vec<Uuid> = Vec::new();");
            wln!(code, "        for source in &sources {{");
            if seg.fk_is_required {
                // Required genuine EntityReference: FK field is a plain Uuid.
                wln!(
                    code,
                    "            fk_values.push(source.{});",
                    seg.fk_column
                );
            } else {
                wln!(
                    code,
                    "            if let Some(fk) = source.{} {{",
                    seg.fk_column
                );
                wln!(code, "                fk_values.push(fk);");
                wln!(code, "            }}");
            }
            wln!(code, "        }}");
            wln!(
                code,
                "        let targets = crate::entity::{}::Entity::find()",
                target_module
            );
            wln!(
                code,
                "            .filter(crate::entity::{}::Column::Id.is_in(fk_values))",
                target_module
            );
            wln!(code, "            .all(db)");
            wln!(code, "            .await?;");
            // A plain for-loop (not .map) so target child-table hydration can
            // await inside the body.
            wln!(
                code,
                "        let mut target_by_id: std::collections::HashMap<Uuid, {}> = std::collections::HashMap::new();",
                resp_type
            );
            wln!(code, "        for t in targets {{");
            if !hydration_children.is_empty() {
                emit_child_reads(code, hydration_children, "t.id", 3);
            }
            wln!(
                code,
                "            target_by_id.insert(t.id, {} {{",
                resp_type
            );
            wln!(code, "                id: t.id,");
            Self::emit_field_assignments_typed(
                code,
                "t",
                dto_fields,
                col_fields,
                is_structured,
                is_codelist,
                dto_rust_types,
                is_nullable,
                col_rust_types,
            );
            wln!(code, "                created_at: t.created_at,");
            if !target_tree.is_some_and(|t2| t2.append_only) {
                wln!(code, "                updated_at: t.updated_at,");
            }
            if !hydration_children.is_empty() {
                emit_child_field_population(code, hydration_children, "                ");
            }
            wln!(code, "                ..Default::default()");
            wln!(code, "            }});");
            wln!(code, "        }}");
            wln!(
                code,
                "        let mut result: std::collections::HashMap<Uuid, Option<{}>> = std::collections::HashMap::new();",
                resp_type
            );
            wln!(code, "        for id in source_ids {{");
            if seg.fk_is_required {
                // Required genuine EntityReference: FK field is a plain Uuid,
                // so look up the target directly (the outer find is still Option).
                wln!(
                    code,
                    "            let found = sources.iter().find(|s| s.id == *id).and_then(|s| target_by_id.get(&s.{}).cloned());",
                    seg.fk_column
                );
            } else {
                wln!(
                    code,
                    "            let found = sources.iter().find(|s| s.id == *id).and_then(|s| s.{}.and_then(|fk| target_by_id.get(&fk).cloned()));",
                    seg.fk_column
                );
            }
            wln!(code, "            result.insert(*id, found);");
            wln!(code, "        }}");
        }

        wln!(code, "        Ok(result)");
        wln!(code, "    }}");
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_dot_fetch_method(
        &self,
        _tree: &EntityTree,
        code: &mut CodeWriter,
        path: &ResolvedIncludePath,
        intermediate_dto: &[String],
        intermediate_col: &[String],
        intermediate_structured: &[bool],
        intermediate_codelist: &[bool],
        intermediate_dto_types: &[Option<String>],
        intermediate_nullable: &[bool],
        intermediate_col_types: &[String],
        leaf_dto: &[String],
        leaf_col: &[String],
        leaf_structured: &[bool],
        leaf_codelist: &[bool],
        leaf_dto_types: &[Option<String>],
        leaf_nullable: &[bool],
        leaf_col_types: &[String],
    ) {
        let seg0 = &path.segments[0];
        let seg1 = &path.segments[1];
        let resp_type = &path.response_rust_type;
        let leaf_resp_type = path
            .segments
            .last()
            .map(|s| format!("{}Response", s.entity_name))
            .unwrap_or_default();
        let intermediate_module = format!("{}_{}", seg0.domain, seg0.module_name);
        let leaf_module = format!("{}_{}", seg1.domain, seg1.module_name);

        wln!(code);
        wln!(code, "    pub(crate) async fn {}(", path.fetch_method);
        wln!(code, "        &self,");
        wln!(code, "        db: &DatabaseTransaction,");
        wln!(code, "        source_id: Uuid,");
        wln!(
            code,
            "    ) -> Result<Option<{}>, Box<dyn std::error::Error>> {{",
            resp_type
        );

        wln!(
            code,
            "        let intermediate = crate::entity::{}::Entity::find()",
            intermediate_module
        );
        if seg0.is_array {
            let rev_fk_pascal = codegraph_naming::to_pascal_case(&seg0.reverse_fk_column);
            wln!(
                code,
                "            .filter(crate::entity::{}::Column::{}.eq(source_id))",
                intermediate_module,
                rev_fk_pascal
            );
            wln!(code, "            .one(db)");
        } else {
            wln!(
                code,
                "            .filter(crate::entity::{}::Column::Id.eq(source_id))",
                intermediate_module
            );
            wln!(code, "            .one(db)");
        }
        wln!(code, "            .await?;");
        wln!(code, "        let intermediate = match intermediate {{");
        wln!(code, "            Some(s) => s,");
        wln!(code, "            None => return Ok(None),");
        wln!(code, "        }};");
        if seg1.fk_is_required {
            // Required genuine EntityReference: FK field is a plain Uuid.
            wln!(
                code,
                "        let fk_value = intermediate.{};",
                seg1.fk_column
            );
        } else {
            wln!(
                code,
                "        let fk_value = match intermediate.{} {{",
                seg1.fk_column
            );
            wln!(code, "            Some(v) => v,");
            wln!(code, "            None => return Ok(None),");
            wln!(code, "        }};");
        }
        wln!(
            code,
            "        let leaf = crate::entity::{}::Entity::find()",
            leaf_module
        );
        wln!(
            code,
            "            .filter(crate::entity::{}::Column::Id.eq(fk_value))",
            leaf_module
        );
        wln!(code, "            .one(db)");
        wln!(code, "            .await?;");

        // Build enriched response: base fields from intermediate, nested leaf from leaf
        wln!(
            code,
            "        let leaf_dto = leaf.map(|l| {} {{",
            leaf_resp_type
        );
        wln!(code, "            id: l.id,");
        Self::emit_field_assignments_typed(
            code,
            "l",
            leaf_dto,
            leaf_col,
            leaf_structured,
            leaf_codelist,
            leaf_dto_types,
            leaf_nullable,
            leaf_col_types,
        );
        wln!(code, "            created_at: l.created_at,");
        wln!(code, "            updated_at: l.updated_at,");
        wln!(code, "            ..Default::default()");
        wln!(code, "        }});");

        wln!(code, "        Ok(Some({} {{", resp_type);
        wln!(code, "            id: intermediate.id,");
        // Intermediate entity fields go into the enriched struct base
        Self::emit_field_assignments_typed(
            code,
            "intermediate",
            intermediate_dto,
            intermediate_col,
            intermediate_structured,
            intermediate_codelist,
            intermediate_dto_types,
            intermediate_nullable,
            intermediate_col_types,
        );
        wln!(code, "            created_at: intermediate.created_at,");
        wln!(code, "            updated_at: intermediate.updated_at,");
        wln!(code, "            {}: leaf_dto,", seg1.module_name);
        wln!(code, "            ..Default::default()");
        wln!(code, "        }}))");

        wln!(code, "    }}");
    }

    /// Batch variant of [`Self::emit_dot_fetch_method`] for list endpoints:
    /// delegates to the single-source dot fetch per id. Dot-path chains are
    /// point queries (indexed fk hops), so the loop is acceptable at page
    /// sizes; a set-based chain with HashMap grouping would be the
    /// optimization if it ever shows up in profiles.
    fn emit_dot_batch_fetch_method(
        &self,
        _tree: &EntityTree,
        code: &mut CodeWriter,
        path: &ResolvedIncludePath,
    ) {
        let resp_type = &path.response_rust_type;
        wln!(code);
        wln!(code, "    pub(crate) async fn {}(", path.batch_fetch_method);
        wln!(code, "        &self,");
        wln!(code, "        db: &DatabaseTransaction,");
        wln!(code, "        source_ids: &[Uuid],");
        wln!(
            code,
            "    ) -> Result<std::collections::HashMap<Uuid, {resp_type}>, Box<dyn std::error::Error>> {{"
        );
        wln!(
            code,
            "        let mut result = std::collections::HashMap::new();"
        );
        wln!(code, "        for source_id in source_ids {{");
        wln!(
            code,
            "            if let Some(combined) = self.{}(db, *source_id).await? {{",
            path.fetch_method
        );
        wln!(code, "                result.insert(*source_id, combined);");
        wln!(code, "            }}");
        wln!(code, "        }}");
        wln!(code, "        Ok(result)");
        wln!(code, "    }}");
    }
}
