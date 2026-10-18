use std::collections::HashMap;

use codegraph_core::error::GraphError;
use codegraph_core::types::{
    CodeList, ColumnInfo, CompositeColumn, CompositeRange, CompositionNode, CompositionTree,
    Extension, FkDirection, FkTarget, PropertyNode, SchemaNode, StructuredSubField,
};

use super::query::{query_gql, query_gql_params, query_many_params, query_one_params};
use super::{PROPERTY_RETURN_COLS, SCHEMA_RETURN_COLS};
use crate::conversions::{
    RowReader, row_to_codelist, row_to_composite_column, row_to_composite_range, row_to_extension,
    row_to_property_node, row_to_schema_node, row_to_structured_sub_field,
};
use crate::engine::GrafeoEngine;

impl GrafeoEngine {
    pub(super) async fn query_composite_columns(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Vec<CompositeColumn>, GraphError> {
        let params = HashMap::from([
            (
                "pname".to_string(),
                grafeo::Value::String(property_name.into()),
            ),
            (
                "stitle".to_string(),
                grafeo::Value::String(schema_title.into()),
            ),
        ]);
        query_many_params(
            self,
            "MATCH (:Property {name: $pname, _schema_title: $stitle})-[:ExpandsTo]->(cc:CompositeColumn) \
             RETURN cc.suffix, cc.pg_type, cc.rust_type, cc.sea_orm_type, cc.fk_target, cc.dto_rust_type, cc.wrapper_schema",
            params,
            row_to_composite_column,
        )
        .await
    }

    pub(super) async fn query_structured_sub_fields(
        &self,
        schema_title: &str,
    ) -> Result<Vec<StructuredSubField>, GraphError> {
        let params = HashMap::from([(
            "title".to_string(),
            grafeo::Value::String(schema_title.into()),
        )]);
        query_many_params(
            self,
            "MATCH (:Schema {title: $title})-[:HasProperty]->(p:Property) \
             RETURN p.name, p.description, p.is_required \
             ORDER BY p.is_required DESC, p.name ASC",
            params,
            row_to_structured_sub_field,
        )
        .await
    }

    pub(super) async fn query_composite_range(
        &self,
        schema_title: &str,
    ) -> Result<Option<CompositeRange>, GraphError> {
        let params = HashMap::from([(
            "title".to_string(),
            grafeo::Value::String(schema_title.into()),
        )]);
        // First try direct CollapsesTo edge on this schema
        let result = query_gql_params(
            self,
            "MATCH (:Schema {title: $title})-[:CollapsesTo]->(r:CompositeRange) \
             RETURN r.pg_column_name, r.pg_type, r.rust_type, r.start_field, r.end_field, r.open_end",
            params.clone(),
        )?;
        if !result.rows.is_empty() {
            let reader = RowReader::from_columns(&result.columns);
            return Ok(Some(row_to_composite_range(&reader, &result.rows[0])?));
        }

        // Follow allOf/ExtendsSchema inheritance to find composite range on parent schemas
        let parent_result = query_gql_params(
            self,
            "MATCH (:Schema {title: $title})-[:ExtendsSchema]->(:Schema)-[:CollapsesTo]->(r:CompositeRange) \
             RETURN r.pg_column_name, r.pg_type, r.rust_type, r.start_field, r.end_field, r.open_end",
            params,
        )?;
        if !parent_result.rows.is_empty() {
            let reader = RowReader::from_columns(&parent_result.columns);
            return Ok(Some(row_to_composite_range(
                &reader,
                &parent_result.rows[0],
            )?));
        }

        Ok(None)
    }

    pub(super) async fn query_consumed_fields(
        &self,
        schema_title: &str,
    ) -> Result<Vec<(PropertyNode, String)>, GraphError> {
        let params = HashMap::from([(
            "title".to_string(),
            grafeo::Value::String(schema_title.into()),
        )]);
        // First try direct CollapsesTo edge
        let result = query_gql_params(
            self,
            &format!(
                "MATCH (:Schema {{title: $title}})-[:CollapsesTo]->(r:CompositeRange)-[cf:ConsumesField]->(p:Property) \
                 RETURN {PROPERTY_RETURN_COLS}, cf.role"
            ),
            params.clone(),
        )?;
        if !result.rows.is_empty() {
            let reader = RowReader::from_columns(&result.columns);
            let mut pairs = Vec::new();
            for row in &result.rows {
                let prop = row_to_property_node(&reader, row)?;
                let role = reader.get_string(row, "cf.role")?;
                pairs.push((prop, role));
            }
            return Ok(pairs);
        }

        // Follow allOf/ExtendsSchema inheritance
        let parent_result = query_gql_params(
            self,
            &format!(
                "MATCH (:Schema {{title: $title}})-[:ExtendsSchema]->(:Schema)-[:CollapsesTo]->(r:CompositeRange)-[cf:ConsumesField]->(p:Property) \
                 RETURN {PROPERTY_RETURN_COLS}, cf.role"
            ),
            params,
        )?;
        let reader = RowReader::from_columns(&parent_result.columns);
        let mut pairs = Vec::new();
        for row in &parent_result.rows {
            let prop = row_to_property_node(&reader, row)?;
            let role = reader.get_string(row, "cf.role")?;
            pairs.push((prop, role));
        }
        Ok(pairs)
    }

    pub(super) async fn query_codelist_for_property(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Option<(CodeList, String)>, GraphError> {
        let params = HashMap::from([
            (
                "pname".to_string(),
                grafeo::Value::String(property_name.into()),
            ),
            (
                "stitle".to_string(),
                grafeo::Value::String(schema_title.into()),
            ),
        ]);
        query_one_params(
            self,
            "MATCH (:Property {name: $pname, _schema_title: $stitle})-[u:UsesCodeList]->(c:CodeList) \
             RETURN c.name, c.description, c.pg_table_name, c.render_as, c.check_expression, u.render_as",
            params,
            |reader, row| {
                Ok((
                    row_to_codelist(reader, row)?,
                    reader.get_string(row, "u.render_as")?,
                ))
            },
        )
        .await
    }

    pub(super) async fn query_required_extensions(
        &self,
        schema_title: &str,
    ) -> Result<Vec<Extension>, GraphError> {
        let params = HashMap::from([(
            "title".to_string(),
            grafeo::Value::String(schema_title.into()),
        )]);
        query_many_params(
            self,
            "MATCH (:Schema {title: $title})-[:RequiresExtension]->(e:Extension) RETURN e.name",
            params,
            row_to_extension,
        )
        .await
    }

    pub(super) async fn query_composition_tree(
        &self,
        schema_title: &str,
    ) -> Result<CompositionTree, GraphError> {
        let mut visited = std::collections::HashSet::new();
        let mut loadout = CompositionLoadout::default();
        let mut root = self
            .build_composition_node(
                schema_title,
                schema_title,
                None,
                false,
                &mut visited,
                0,
                &mut loadout,
            )
            .await?;
        root.dedup_fields();
        Ok(CompositionTree { root })
    }

    pub(super) async fn query_allof_targets(
        &self,
        schema_title: &str,
    ) -> Result<Vec<String>, GraphError> {
        let params = HashMap::from([(
            "title".to_string(),
            grafeo::Value::String(schema_title.into()),
        )]);
        query_many_params(
            self,
            "MATCH (:Schema {title: $title})-[:ExtendsSchema {composition_type: 'allOf'}]->(t:Schema) \
             RETURN t.title",
            params,
            |reader, row| reader.get_string(row, "t.title"),
        )
        .await
    }

    pub(super) async fn query_schemas_that_extend(
        &self,
        parent_title: &str,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        let params = HashMap::from([(
            "title".to_string(),
            grafeo::Value::String(parent_title.into()),
        )]);
        query_many_params(
            self,
            &format!(
                "MATCH (s:Schema)-[:ExtendsSchema]->(:Schema {{title: $title}}) RETURN {}",
                SCHEMA_RETURN_COLS
            ),
            params,
            row_to_schema_node,
        )
        .await
    }

    pub(super) async fn query_referencing_schemas(
        &self,
        schema_title: &str,
    ) -> Result<Vec<String>, GraphError> {
        let params = HashMap::from([(
            "title".to_string(),
            grafeo::Value::String(schema_title.into()),
        )]);
        query_many_params(
            self,
            "MATCH (p:Property)-[:ReferencesSchema]->(:Schema {title: $title}) \
             RETURN DISTINCT p._schema_title",
            params,
            |reader, row| reader.get_string(row, "p._schema_title"),
        )
        .await
    }

    pub(super) async fn query_referenced_schemas(
        &self,
        schema_title: &str,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        let params = HashMap::from([(
            "title".to_string(),
            grafeo::Value::String(schema_title.into()),
        )]);
        query_many_params(
            self,
            &format!(
                "MATCH (:Schema {{title: $title}})-[:HasProperty]->(p:Property)-[:ReferencesSchema]->(s:Schema) \
                 RETURN DISTINCT {SCHEMA_RETURN_COLS}"
            ),
            params,
            row_to_schema_node,
        )
        .await
    }

    pub(super) async fn query_property_ref_target(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        let params = HashMap::from([
            (
                "pname".to_string(),
                grafeo::Value::String(property_name.into()),
            ),
            (
                "stitle".to_string(),
                grafeo::Value::String(schema_title.into()),
            ),
        ]);
        query_one_params(
            self,
            &format!(
                "MATCH (:Property {{name: $pname, _schema_title: $stitle}})-[:ReferencesSchema]->(s:Schema) \
                 RETURN {SCHEMA_RETURN_COLS}"
            ),
            params,
            row_to_schema_node,
        )
        .await
    }

    /// Bulk form of `query_property_ref_target` (issue #350): every
    /// property's ref-target resolution in ONE query, keyed by (property
    /// name, owning schema title). `OPTIONAL MATCH` also covers the
    /// negative space — properties with no `ReferencesSchema` edge seed a
    /// `None` entry, so post-seed point lookups only happen for keys the
    /// graph never heard of. First row wins per key — the same first-row
    /// semantics as the point lookup.
    pub(super) async fn query_all_property_ref_targets(
        &self,
    ) -> Result<HashMap<(String, String), Option<SchemaNode>>, GraphError> {
        let result = query_gql(
            self,
            &format!(
                "MATCH (p:Property) OPTIONAL MATCH (p)-[:ReferencesSchema]->(s:Schema) \
                 RETURN p.name, p._schema_title, {SCHEMA_RETURN_COLS}"
            ),
        )?;
        let reader = RowReader::from_columns(&result.columns);
        let mut map: HashMap<(String, String), Option<SchemaNode>> = HashMap::new();
        for row in &result.rows {
            let (Ok(name), Ok(stitle)) = (
                reader.get_string(row, "p.name"),
                reader.get_string(row, "p._schema_title"),
            ) else {
                continue;
            };
            let target = if reader.get_opt_string(row, "s.title")?.is_some() {
                Some(row_to_schema_node(&reader, row)?)
            } else {
                None
            };
            map.entry((name, stitle)).or_insert(target);
        }
        Ok(map)
    }

    pub(super) async fn query_property_ref_target_by_id(
        &self,
        property_name: &str,
        schema_id: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        let params = HashMap::from([
            (
                "pname".to_string(),
                grafeo::Value::String(property_name.into()),
            ),
            ("sid".to_string(), grafeo::Value::String(schema_id.into())),
        ]);
        query_one_params(
            self,
            &format!(
                "MATCH (:Property {{name: $pname, _schema_id: $sid}})-[:ReferencesSchema]->(s:Schema) \
                 RETURN {SCHEMA_RETURN_COLS}"
            ),
            params,
            row_to_schema_node,
        )
        .await
    }

    pub(super) async fn query_properties_by_schema_id(
        &self,
        schema_id: &str,
    ) -> Result<Vec<PropertyNode>, GraphError> {
        let params = HashMap::from([("sid".to_string(), grafeo::Value::String(schema_id.into()))]);
        query_many_params(
            self,
            &format!("MATCH (p:Property {{_schema_id: $sid}}) RETURN {PROPERTY_RETURN_COLS}"),
            params,
            row_to_property_node,
        )
        .await
    }

    pub(super) async fn query_array_item_schema(
        &self,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        let params = HashMap::from([
            (
                "pname".to_string(),
                grafeo::Value::String(property_name.into()),
            ),
            (
                "stitle".to_string(),
                grafeo::Value::String(schema_title.into()),
            ),
        ]);
        query_one_params(
            self,
            &format!(
                "MATCH (:Property {{name: $pname, _schema_title: $stitle}})-[:ItemsOf]->(s:Schema) \
                 RETURN {SCHEMA_RETURN_COLS}"
            ),
            params,
            row_to_schema_node,
        )
        .await
    }
}

/// Maximum nesting depth for recursive composition tree building.
const MAX_COMPOSITION_DEPTH: usize = 10;

/// Per-`get_composition_tree` memo of graph reads (issue #389).
///
/// Each distinct lookup executes its underlying GQL query at most once per
/// tree build; repeats (sibling VOs of the same schema type, allOf revisits,
/// multi-path recursion) replay the cached result. The graph is immutable for
/// the duration of a build and every wrapped query is a pure read, so replay
/// is semantically identical to re-querying — the `CompositionTree` output is
/// byte-identical, only the query volume drops (58 → 48 on the pin-test
/// fixture; see the `query_count_tests` module).
///
/// Error semantics are preserved: resolvers whose call sites propagate errors
/// (`?`) propagate on a cache miss and never cache `Err`; resolvers whose call
/// sites swallow errors (`.ok().flatten()`) cache the already-flattened value,
/// which is observationally equivalent at every call site (an `Err` and an
/// `Ok(None)` take the same fall-through branch in all consumers).
#[derive(Default)]
struct CompositionLoadout {
    schemas: HashMap<String, Option<SchemaNode>>,
    properties: HashMap<String, Vec<PropertyNode>>,
    codelist_for_property: HashMap<(String, String), Option<(CodeList, String)>>,
    composite_columns: HashMap<(String, String), Vec<CompositeColumn>>,
    composite_range: HashMap<String, Option<CompositeRange>>,
    consumed_fields: HashMap<String, Vec<(PropertyNode, String)>>,
    property_ref_target: HashMap<(String, String), Option<SchemaNode>>,
    array_item_schema: HashMap<(String, String), Option<SchemaNode>>,
    enum_values: HashMap<String, Vec<String>>,
    schema_in_domain: HashMap<(String, String), Option<SchemaNode>>,
    allof_edges: HashMap<String, Vec<(String, Option<String>)>>,
    vo_entity_extendee: HashMap<String, Option<SchemaNode>>,
}

impl CompositionLoadout {
    async fn schema(
        &mut self,
        engine: &GrafeoEngine,
        title: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        if let Some(cached) = self.schemas.get(title) {
            return Ok(cached.clone());
        }
        let result = engine.query_schema(title).await?;
        self.schemas.insert(title.to_string(), result.clone());
        Ok(result)
    }

    async fn properties(
        &mut self,
        engine: &GrafeoEngine,
        schema_title: &str,
    ) -> Result<Vec<PropertyNode>, GraphError> {
        if let Some(cached) = self.properties.get(schema_title) {
            return Ok(cached.clone());
        }
        let result = engine.query_properties(schema_title).await?;
        self.properties
            .insert(schema_title.to_string(), result.clone());
        Ok(result)
    }

    async fn codelist_for_property(
        &mut self,
        engine: &GrafeoEngine,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Option<(CodeList, String)>, GraphError> {
        let key = (property_name.to_string(), schema_title.to_string());
        if let Some(cached) = self.codelist_for_property.get(&key) {
            return Ok(cached.clone());
        }
        let result = engine
            .query_codelist_for_property(property_name, schema_title)
            .await?;
        self.codelist_for_property.insert(key, result.clone());
        Ok(result)
    }

    async fn composite_columns(
        &mut self,
        engine: &GrafeoEngine,
        property_name: &str,
        schema_title: &str,
    ) -> Result<Vec<CompositeColumn>, GraphError> {
        let key = (property_name.to_string(), schema_title.to_string());
        if let Some(cached) = self.composite_columns.get(&key) {
            return Ok(cached.clone());
        }
        let result = engine
            .query_composite_columns(property_name, schema_title)
            .await?;
        self.composite_columns.insert(key, result.clone());
        Ok(result)
    }

    async fn composite_range(
        &mut self,
        engine: &GrafeoEngine,
        schema_title: &str,
    ) -> Option<CompositeRange> {
        if let Some(cached) = self.composite_range.get(schema_title) {
            return cached.clone();
        }
        let result = engine
            .query_composite_range(schema_title)
            .await
            .ok()
            .flatten();
        self.composite_range
            .insert(schema_title.to_string(), result.clone());
        result
    }

    async fn consumed_fields(
        &mut self,
        engine: &GrafeoEngine,
        schema_title: &str,
    ) -> Vec<(PropertyNode, String)> {
        if let Some(cached) = self.consumed_fields.get(schema_title) {
            return cached.clone();
        }
        let result = engine
            .query_consumed_fields(schema_title)
            .await
            .unwrap_or_default();
        self.consumed_fields
            .insert(schema_title.to_string(), result.clone());
        result
    }

    async fn property_ref_target(
        &mut self,
        engine: &GrafeoEngine,
        property_name: &str,
        schema_title: &str,
    ) -> Option<SchemaNode> {
        let key = (property_name.to_string(), schema_title.to_string());
        if let Some(cached) = self.property_ref_target.get(&key) {
            return cached.clone();
        }
        let result = engine
            .query_property_ref_target(property_name, schema_title)
            .await
            .ok()
            .flatten();
        self.property_ref_target.insert(key, result.clone());
        result
    }

    async fn array_item_schema(
        &mut self,
        engine: &GrafeoEngine,
        property_name: &str,
        schema_title: &str,
    ) -> Option<SchemaNode> {
        let key = (property_name.to_string(), schema_title.to_string());
        if let Some(cached) = self.array_item_schema.get(&key) {
            return cached.clone();
        }
        let result = engine
            .query_array_item_schema(property_name, schema_title)
            .await
            .ok()
            .flatten();
        self.array_item_schema.insert(key, result.clone());
        result
    }

    async fn check_values(&mut self, engine: &GrafeoEngine, codelist_name: &str) -> Vec<String> {
        if let Some(cached) = self.enum_values.get(codelist_name) {
            return cached.clone();
        }
        let result: Vec<String> = engine
            .query_enum_values(codelist_name)
            .await
            .ok()
            .map(|vals| vals.into_iter().map(|v| v.value).collect())
            .unwrap_or_default();
        self.enum_values
            .insert(codelist_name.to_string(), result.clone());
        result
    }

    async fn schema_in_domain(
        &mut self,
        engine: &GrafeoEngine,
        title: &str,
        domain: &str,
    ) -> Option<SchemaNode> {
        let key = (title.to_string(), domain.to_string());
        if let Some(cached) = self.schema_in_domain.get(&key) {
            return cached.clone();
        }
        let result = engine
            .query_schema_in_domain(title, domain)
            .await
            .ok()
            .flatten();
        self.schema_in_domain.insert(key, result.clone());
        result
    }

    async fn allof_edges(
        &mut self,
        engine: &GrafeoEngine,
        schema_title: &str,
    ) -> Result<Vec<(String, Option<String>)>, GraphError> {
        if let Some(cached) = self.allof_edges.get(schema_title) {
            return Ok(cached.clone());
        }
        let params = HashMap::from([(
            "title".to_string(),
            grafeo::Value::String(schema_title.into()),
        )]);
        let result = query_gql_params(
            engine,
            "MATCH (:Schema {title: $title})-[e:ExtendsSchema]->(child:Schema) \
             RETURN child.title, e.composition_type",
            params,
        )?;
        let reader = RowReader::from_columns(&result.columns);
        let mut edges = Vec::with_capacity(result.rows.len());
        for row in &result.rows {
            edges.push((
                reader.get_string(row, "child.title")?,
                reader.get_opt_string(row, "e.composition_type")?,
            ));
        }
        self.allof_edges
            .insert(schema_title.to_string(), edges.clone());
        Ok(edges)
    }

    async fn vo_entity_extendee(
        &mut self,
        engine: &GrafeoEngine,
        vo_title: &str,
    ) -> Option<SchemaNode> {
        if let Some(cached) = self.vo_entity_extendee.get(vo_title) {
            return cached.clone();
        }
        let result = codegraph_core::traits::find_entity_extended_by_vo(engine, vo_title)
            .await
            .ok()
            .flatten();
        self.vo_entity_extendee
            .insert(vo_title.to_string(), result.clone());
        result
    }
}

/// Build the synthetic codelist-array child node (single "code" column) for a
/// codelist array property and push it onto `children`.
fn push_codelist_array_child(
    prop: &PropertyNode,
    col: &ColumnInfo,
    schema: &SchemaNode,
    default_schema: &str,
    children: &mut Vec<CompositionNode>,
) {
    let child_table =
        codegraph_naming::child_table_name(&schema.pg_table_name, &prop.pg_column_name);
    let child_fk_col = codegraph_naming::child_parent_fk_column(&schema.pg_table_name);

    let codelist_title = prop
        .ref_target
        .as_deref()
        .map(|r| {
            r.rsplit('/')
                .next()
                .unwrap_or(r)
                .trim_end_matches(".json#")
                .trim_end_matches(".json")
                .to_string()
        })
        .unwrap_or_else(|| prop.name.clone());

    let code_col = ColumnInfo {
        name: "code".to_string(),
        description: col.description.clone(),
        rust_type: "String".to_string(),
        postgres_type: "TEXT".to_string(),
        is_optional: false,
        is_codelist_fk: true,
        composite_columns: vec![],
        is_array: false,
        classification: col.classification.clone(),
        fk_target: col.fk_target.clone(),
        check_values: col.check_values.clone(),
    };

    children.push(CompositionNode {
        field_name: prop.pg_column_name.clone(),
        schema_title: codelist_title,
        table_schema: default_schema.to_string(),
        table_name: child_table,
        fk: Some(FkDirection::OnChild {
            column: child_fk_col,
        }),
        is_collection: true,
        columns: vec![code_col],
        jsonb_columns: vec![],
        children: vec![],
        composite_range: None,
        consumed_fields: vec![],
    });
}

impl GrafeoEngine {
    #[allow(clippy::too_many_arguments)]
    async fn build_composition_node(
        &self,
        schema_title: &str,
        field_name: &str,
        fk: Option<FkDirection>,
        is_collection: bool,
        visited: &mut std::collections::HashSet<String>,
        depth: usize,
        loadout: &mut CompositionLoadout,
    ) -> Result<CompositionNode, GraphError> {
        let schema = loadout
            .schema(self, schema_title)
            .await?
            .ok_or_else(|| GraphError::NotFound(format!("Schema '{schema_title}'")))?;

        let default_schema = schema
            .domain
            .clone()
            .unwrap_or_else(|| "public".to_string());

        let properties = loadout.properties(self, schema_title).await?;
        let mut columns = Vec::new();
        let mut jsonb_columns = Vec::new();
        let mut children = Vec::new();

        // Resolve composite range and consumed fields for this node
        let composite_range = loadout.composite_range(self, schema_title).await;
        let consumed_fields_raw = loadout.consumed_fields(self, schema_title).await;
        let consumed_field_names: Vec<String> = consumed_fields_raw
            .iter()
            .map(|(p, _)| p.name.clone())
            .collect();
        let consumed_set: std::collections::HashSet<&str> =
            consumed_field_names.iter().map(|s| s.as_str()).collect();

        for prop in &properties {
            // Skip fields consumed by composite ranges
            if consumed_set.contains(prop.name.as_str()) {
                continue;
            }

            let is_codelist_fk = loadout
                .codelist_for_property(self, &prop.name, schema_title)
                .await?
                .is_some();
            let composite_columns = loadout
                .composite_columns(self, &prop.name, schema_title)
                .await?;
            let classification = prop.effective_kind();

            // Resolve FK target for reference columns
            let fk_target = self
                .resolve_property_fk_target(
                    prop,
                    schema_title,
                    &default_schema,
                    &classification,
                    loadout,
                )
                .await;

            // Resolve enum values for check-constraint columns
            let check_values = self
                .resolve_property_check_values(prop, &classification, loadout)
                .await;

            let col = ColumnInfo {
                name: prop.pg_column_name.clone(),
                description: prop.description.clone(),
                rust_type: prop.rust_field_type.clone(),
                postgres_type: prop.pg_column_type.clone(),
                is_optional: !prop.is_required,
                is_codelist_fk,
                composite_columns,
                is_array: prop.is_array,
                classification: classification.clone(),
                fk_target,
                check_values,
            };

            // ValueObject properties → recurse into child nodes instead of
            // flattening into jsonb_columns. This matches the DDL child-table
            // hierarchy: each ValueObject becomes a separate SQL table.
            if classification == Some(codegraph_type_contracts::RefClassificationKind::ValueObject)
            {
                self.push_value_object_child(
                    prop,
                    col,
                    schema_title,
                    &schema,
                    &default_schema,
                    visited,
                    depth,
                    &mut columns,
                    &mut children,
                    loadout,
                )
                .await?;
                continue;
            }

            // Codelist array properties → synthetic child node with a single
            // "code" column.  Codelist schemas are plain enums (no object
            // properties to recurse into), so we build the CompositionNode
            // directly instead of recursing via build_composition_node.
            if prop.is_array
                && matches!(
                    classification,
                    Some(codegraph_type_contracts::RefClassificationKind::CodelistReference)
                        | Some(codegraph_type_contracts::RefClassificationKind::CodelistCheck)
                )
            {
                push_codelist_array_child(prop, &col, &schema, &default_schema, &mut children);
                continue;
            }

            // Array of entity refs: the relationship cannot be represented by a
            // column on the parent table. Two shapes are supported:
            //   1. FK-on-child: the target schema carries a back-reference
            //      column to this schema (e.g. party.case_id for case.party_ids).
            //      The child-side FK is injected by the DDL generator's parent
            //      candidate handling, so the parent side is skipped entirely.
            //   2. Junction table: no back-reference exists. Emit a
            //      `<parent>_<field>` junction child table holding parent_id +
            //      child_id FKs (many-to-many).
            if prop.is_array
                && classification
                    == Some(codegraph_type_contracts::RefClassificationKind::EntityReference)
            {
                self.push_junction_array_child(
                    prop,
                    schema_title,
                    &schema,
                    &default_schema,
                    &mut children,
                    loadout,
                )
                .await?;
                continue;
            }

            // Scalar entity references are FK columns by their own
            // property-level classification (#277) — the one canonical
            // classification plane. The target schema's `is_entity` flag is
            // set by the classifier/config plane and can lag the property
            // classification (the rosetta bridge records a VO-shaped default
            // for every schema and never declares entities, while its
            // properties still classify model-typed attributes as
            // EntityReference). Demoting such a property to the jsonb plane
            // here makes the DDL/entity families resolve zero reference
            // columns while the dto family — which trusts the property
            // classification — resolves them all (#446): audit-only skeleton
            // tables whose repositories read columns the model lacks. The
            // generation-order plane treats every schema with a pg_table_name
            // as generated, so the FK column's target table exists whenever
            // the property classification says entity reference.
            let is_scalar_entity_reference = classification
                == Some(codegraph_type_contracts::RefClassificationKind::EntityReference)
                && !prop.is_array;
            if !is_scalar_entity_reference
                && let Some(ref_target) = &prop.ref_target
                && let Some(target_schema) = loadout.schema(self, ref_target).await?
                && !target_schema.is_entity
                && !target_schema.is_codelist
                && target_schema.schema_type == "object"
            {
                jsonb_columns.push(col);
                continue;
            }
            columns.push(col);
        }

        // Query ExtendsSchema edges for children (allOf composition)
        self.push_allof_children(schema_title, visited, depth, &mut children, loadout)
            .await?;

        Ok(CompositionNode {
            field_name: field_name.to_string(),
            schema_title: schema_title.to_string(),
            table_schema: default_schema,
            table_name: schema.pg_table_name.clone(),
            fk,
            is_collection,
            columns,
            jsonb_columns,
            children,
            composite_range,
            consumed_fields: consumed_field_names,
        })
    }

    async fn resolve_property_fk_target(
        &self,
        prop: &PropertyNode,
        schema_title: &str,
        default_schema: &str,
        classification: &Option<codegraph_type_contracts::RefClassificationKind>,
        loadout: &mut CompositionLoadout,
    ) -> Option<FkTarget> {
        match classification {
            Some(codegraph_type_contracts::RefClassificationKind::CodelistReference) => {
                // Resolve FK for both scalar and array codelists —
                // array codelists need the FK target for their child table's code column.
                self.resolve_fk_target(
                    &prop.name,
                    schema_title,
                    default_schema,
                    prop.ref_target.as_deref(),
                    "code",
                    "RESTRICT",
                    loadout,
                )
                .await
            }
            Some(codegraph_type_contracts::RefClassificationKind::EntityReference) => {
                if !prop.is_array {
                    self.resolve_fk_target(
                        &prop.name,
                        schema_title,
                        default_schema,
                        prop.ref_target.as_deref(),
                        "id",
                        "SET NULL",
                        loadout,
                    )
                    .await
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    async fn resolve_property_check_values(
        &self,
        prop: &PropertyNode,
        classification: &Option<codegraph_type_contracts::RefClassificationKind>,
        loadout: &mut CompositionLoadout,
    ) -> Vec<String> {
        match classification {
            Some(codegraph_type_contracts::RefClassificationKind::CodelistCheck)
            | Some(codegraph_type_contracts::RefClassificationKind::InlineEnum)
                if !prop.is_array =>
            {
                if let Some(ref codelist_name) = prop.ref_target {
                    loadout.check_values(self, codelist_name).await
                } else {
                    vec![]
                }
            }
            _ => vec![],
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn push_value_object_child(
        &self,
        prop: &PropertyNode,
        col: ColumnInfo,
        schema_title: &str,
        schema: &SchemaNode,
        default_schema: &str,
        visited: &mut std::collections::HashSet<String>,
        depth: usize,
        columns: &mut Vec<ColumnInfo>,
        children: &mut Vec<CompositionNode>,
        loadout: &mut CompositionLoadout,
    ) -> Result<(), GraphError> {
        if depth < MAX_COMPOSITION_DEPTH {
            // Resolve target schema
            let target = if prop.is_array {
                loadout
                    .array_item_schema(self, &prop.name, schema_title)
                    .await
            } else {
                loadout
                    .property_ref_target(self, &prop.name, schema_title)
                    .await
            };

            if let Some(target_schema) = target {
                // Non-array entity targets get a FK column on this node.
                // Array entity targets are SKIPPED — a one-to-many relationship
                // cannot be represented by a single UUID FK on the parent. The FK
                // lives on the child entity's table instead (configured via
                // parent_ref in domains.toml).
                let vo_entity = if !target_schema.is_entity {
                    loadout.vo_entity_extendee(self, &target_schema.title).await
                } else {
                    None
                };

                if (target_schema.is_entity || vo_entity.is_some()) && !prop.is_array {
                    let mut entity_col = col;
                    entity_col.classification =
                        Some(codegraph_type_contracts::RefClassificationKind::EntityReference);
                    // VO→entity synthetic FK columns are always nullable: the
                    // DTO and repository generators model the VO as a nested
                    // child table, so no create command ever supplies a value
                    // for this column. Genuine EntityReference columns honor
                    // the schema's `required` (is_optional was already derived
                    // from !prop.is_required above).
                    if vo_entity.is_some() {
                        entity_col.is_optional = true;
                    }
                    if let Some(entity) = &vo_entity {
                        entity_col.fk_target = Some(FkTarget {
                            schema: entity
                                .domain
                                .clone()
                                .unwrap_or_else(|| default_schema.to_string()),
                            table: entity.pg_table_name.clone(),
                            column: "id".to_string(),
                            on_delete: "SET NULL".to_string(),
                        });
                    } else {
                        entity_col.fk_target = self
                            .resolve_fk_target(
                                &prop.name,
                                schema_title,
                                default_schema,
                                prop.ref_target.as_deref(),
                                "id",
                                "SET NULL",
                                loadout,
                            )
                            .await;
                    }
                    columns.push(entity_col);
                }
                if !target_schema.is_entity && !visited.contains(&target_schema.title) {
                    // Recurse into ValueObject as a child node.
                    // Use a fresh visited set (seeded with the current
                    // path) so sibling VO properties referencing the same
                    // schema type each get their own child table — matching
                    // the entity generator's per-property visited approach.
                    let mut child_visited = visited.clone();
                    child_visited.insert(target_schema.title.clone());
                    let child_fk = Some(FkDirection::OnChild {
                        column: format!(
                            "{}_id",
                            codegraph_naming::truncate_pg_identifier(&schema.pg_table_name)
                        ),
                    });
                    let child_node = Box::pin(self.build_composition_node(
                        &target_schema.title,
                        &prop.pg_column_name,
                        child_fk,
                        prop.is_array,
                        &mut child_visited,
                        depth + 1,
                        loadout,
                    ))
                    .await?;
                    children.push(child_node);
                }
            }
        }
        Ok(())
    }

    async fn push_junction_array_child(
        &self,
        prop: &PropertyNode,
        schema_title: &str,
        schema: &SchemaNode,
        default_schema: &str,
        children: &mut Vec<CompositionNode>,
        loadout: &mut CompositionLoadout,
    ) -> Result<(), GraphError> {
        let target_title = loadout
            .array_item_schema(self, &prop.name, schema_title)
            .await;
        let has_back_ref = match &target_title {
            Some(target_schema) => {
                let back_ref = format!(
                    "{}_id",
                    codegraph_naming::truncate_pg_identifier(&schema.pg_table_name)
                );
                loadout
                    .properties(self, &target_schema.title)
                    .await
                    .map(|ps| {
                        ps.iter().any(|p| {
                            p.pg_column_name == back_ref
                                || p.pg_column_name
                                    == format!(
                                        "{}_id",
                                        codegraph_naming::to_snake_case(&schema.title)
                                    )
                        })
                    })
                    .unwrap_or(false)
            }
            None => false,
        };

        if !has_back_ref && let Some(target_schema) = target_title {
            let child_table =
                codegraph_naming::child_table_name(&schema.pg_table_name, &prop.pg_column_name);
            let child_fk_col = codegraph_naming::child_parent_fk_column(&schema.pg_table_name);
            let child_id_col =
                codegraph_naming::child_parent_fk_column(&target_schema.pg_table_name);

            let child_col = ColumnInfo {
                name: child_id_col.clone(),
                description: Some(format!(
                    "FK to {}.{}",
                    target_schema
                        .domain
                        .clone()
                        .unwrap_or_else(|| default_schema.to_string()),
                    target_schema.pg_table_name
                )),
                rust_type: "uuid::Uuid".to_string(),
                postgres_type: "UUID".to_string(),
                is_optional: false,
                is_codelist_fk: false,
                composite_columns: vec![],
                is_array: false,
                classification: Some(
                    codegraph_type_contracts::RefClassificationKind::EntityReference,
                ),
                fk_target: Some(FkTarget {
                    schema: target_schema
                        .domain
                        .clone()
                        .unwrap_or_else(|| default_schema.to_string()),
                    table: target_schema.pg_table_name.clone(),
                    column: "id".to_string(),
                    on_delete: "CASCADE".to_string(),
                }),
                check_values: vec![],
            };

            children.push(CompositionNode {
                field_name: prop.pg_column_name.clone(),
                schema_title: target_schema.title.clone(),
                table_schema: default_schema.to_string(),
                table_name: child_table,
                fk: Some(FkDirection::OnChild {
                    column: child_fk_col,
                }),
                is_collection: true,
                columns: vec![child_col],
                jsonb_columns: vec![],
                children: vec![],
                composite_range: None,
                consumed_fields: vec![],
            });
        }
        Ok(())
    }

    async fn push_allof_children(
        &self,
        schema_title: &str,
        visited: &mut std::collections::HashSet<String>,
        depth: usize,
        children: &mut Vec<CompositionNode>,
        loadout: &mut CompositionLoadout,
    ) -> Result<(), GraphError> {
        let child_edges = loadout.allof_edges(self, schema_title).await?;

        if !child_edges.is_empty() {
            for (child_title, comp_type) in &child_edges {
                if visited.contains(child_title) {
                    continue;
                }
                visited.insert(child_title.clone());

                let child_fk = Some(FkDirection::OnChild {
                    column: format!("{}_id", schema_title.to_lowercase()),
                });
                let child_is_collection = comp_type.as_deref() == Some("collection");
                let child_field_name = child_title.to_lowercase();

                let child_node = Box::pin(self.build_composition_node(
                    child_title,
                    &child_field_name,
                    child_fk,
                    child_is_collection,
                    visited,
                    depth + 1,
                    loadout,
                ))
                .await?;
                children.push(child_node);
            }
        }
        Ok(())
    }

    /// Resolve a property's FK target to (schema, table, column, on_delete) using graph edges.
    #[allow(clippy::too_many_arguments)]
    async fn resolve_fk_target(
        &self,
        property_name: &str,
        schema_title: &str,
        default_schema: &str,
        ref_target: Option<&str>,
        target_column: &str,
        on_delete: &str,
        loadout: &mut CompositionLoadout,
    ) -> Option<FkTarget> {
        // Try ReferencesSchema edge, then ItemsOf edge (for array properties)
        let mut target_schema = loadout
            .property_ref_target(self, property_name, schema_title)
            .await;
        if target_schema.is_none() {
            target_schema = loadout
                .array_item_schema(self, property_name, schema_title)
                .await;
        }

        if let Some(ts) = target_schema {
            // Only create FK targets for entities (types with their own tables).
            // VOs are embedded as child tables, not referenced via FK.
            // Codelists always live in the "common" schema regardless of source domain.
            let schema_name = if ts.is_codelist {
                "common".to_string()
            } else if !ts.is_entity {
                return None;
            } else {
                ts.domain.unwrap_or_else(|| default_schema.to_string())
            };
            if !ts.pg_table_name.is_empty() {
                return Some(FkTarget {
                    schema: schema_name,
                    table: ts.pg_table_name,
                    column: target_column.to_string(),
                    on_delete: on_delete.to_string(),
                });
            }
        }

        // Fallback: parse the ref_target string path.
        // This is a last-resort heuristic when graph edges are missing.
        let ref_str = ref_target.unwrap_or("");
        if ref_str.is_empty() {
            return None;
        }
        let is_codelist_ref = ref_str.contains("/codelist/") || ref_str.starts_with("codelist/");
        let schema_name = if is_codelist_ref {
            "common".to_string()
        } else {
            let domain = extract_ref_domain(ref_str).unwrap_or(default_schema);
            // If the "domain" looks like a JSON schema filename (contains `.json`),
            // the ref_target is a bare filename without path (no domain prefix).
            // Use the default schema instead of the filename.
            if domain.contains(".json") || domain.contains(".json#") {
                default_schema.to_string()
            } else {
                domain.to_string()
            }
        };
        let table = extract_ref_table(ref_str)?;

        // Verify the target exists as an entity in the graph before emitting FK.
        // Try to find the target by table name in the resolved schema domain.
        if let Some(target_check) = loadout.schema_in_domain(self, &table, &schema_name).await
            && !target_check.is_entity
        {
            return None;
        }
        // If the schema doesn't exist in the resolved domain, try the default domain
        // as a fallback (cross-domain allOf references).
        if schema_name != default_schema
            && let Some(target_check) = loadout.schema_in_domain(self, &table, default_schema).await
            && target_check.is_entity
        {
            return Some(FkTarget {
                schema: default_schema.to_string(),
                table,
                column: target_column.to_string(),
                on_delete: on_delete.to_string(),
            });
        }

        Some(FkTarget {
            schema: schema_name,
            table,
            column: target_column.to_string(),
            on_delete: on_delete.to_string(),
        })
    }
}

/// Extract the domain name from a JSON Schema $ref path.
///
/// Examples:
///   "common/json/GenderCodeList.json"       → Some("common")
///   "../../../common/json/codelist/X.json"   → Some("common")
///   "codelist/CandidateRelationshipCodeList.json" → None
fn extract_ref_domain(ref_target: &str) -> Option<&str> {
    let segments: Vec<&str> = ref_target
        .split('/')
        .filter(|s| !s.is_empty() && *s != "..")
        .collect();
    for (i, seg) in segments.iter().enumerate() {
        if *seg == "json" && i > 0 {
            return Some(segments[i - 1]);
        }
    }
    segments.first().copied().filter(|s| *s != "codelist")
}

/// Extract the table name from a JSON Schema $ref path by converting the filename.
fn extract_ref_table(ref_target: &str) -> Option<String> {
    let filename = ref_target.rsplit('/').next()?;
    let stem = filename
        .strip_suffix(".json#")
        .or_else(|| filename.strip_suffix(".json"))
        .unwrap_or(filename);
    Some(codegraph_naming::to_snake_case(
        &codegraph_naming::strip_suffix(stem, "Type"),
    ))
}

#[cfg(test)]
mod query_count_tests {
    //! Issue #389: pins the GQL query volume of one `get_composition_tree`
    //! call over a representative model (multi-property entity with sibling
    //! VO children of the same type, a codelist property, a scalar entity
    //! ref, a junction array, and an allOf extender). If you change the
    //! recursion, re-measure and update the pinned number — the count must
    //! only ever go DOWN.

    use super::super::query::{COUNTER_LOCK, query_count, reset_query_count};
    use super::*;
    use codegraph_core::traits::{GraphIngestor, GraphQuerier};
    use codegraph_core::types::{EdgeProperties, EdgeType};
    use codegraph_type_contracts::RefClassificationKind;

    fn schema(title: &str, domain: &str, is_entity: bool) -> SchemaNode {
        SchemaNode {
            namespace: None,
            schema_id: format!("{domain}/{title}"),
            title: title.to_string(),
            description: None,
            schema_type: "object".to_string(),
            classification: if is_entity { "entity" } else { "value_object" }.to_string(),
            domain: Some(domain.to_string()),
            rel_path: format!("{domain}/{title}.json"),
            pg_type: "TABLE".to_string(),
            rust_type: title.to_string(),
            sea_orm_type: "Entity".to_string(),
            rust_type_name: title.to_string(),
            pg_table_name: title.to_string(),
            api_path_segment: title.to_string(),
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

    fn codelist_schema(title: &str) -> SchemaNode {
        let mut s = schema(title, "common", false);
        s.is_codelist = true;
        s
    }

    fn property(name: &str, is_required: bool) -> PropertyNode {
        PropertyNode {
            name: name.to_string(),
            prop_type: "string".to_string(),
            description: None,
            format: None,
            is_required,
            is_nullable: false,
            is_id: false,
            is_array: false,
            min_items: None,
            max_items: None,
            pattern: None,
            min_length: None,
            max_length: None,
            minimum: None,
            maximum: None,
            pg_column_name: name.to_string(),
            pg_column_type: "TEXT".to_string(),
            rust_field_name: name.to_string(),
            rust_field_type: "String".to_string(),
            sea_orm_type: "String".to_string(),
            render_strategy: "scalar".to_string(),
            ref_target: None,
            classification: None,
            projection: None,
            classification_kind: None,
            ui_override_detail: None,
            ui_override_list_cell: None,
            ui_override_form: None,
            ui_override_inline: None,
            type_expr: None,
        }
    }

    fn classified(
        name: &str,
        required: bool,
        kind: RefClassificationKind,
        ref_target: &str,
        is_array: bool,
    ) -> PropertyNode {
        let mut p = property(name, required);
        p.is_array = is_array;
        p.ref_target = Some(ref_target.to_string());
        p.classification_kind = Some(kind);
        p
    }

    /// Representative model:
    ///
    /// ```text
    /// CaseType (entity, allOf → CaseBaseType)
    ///   homeAddress    → AddressType (VO, sibling #1)
    ///   postalAddress  → AddressType (VO, sibling #2 — same type re-visited)
    ///   owner          → OrgType (scalar entity ref)
    ///   status         → StatusCode (codelist w/ UsesCodeList edge)
    ///   title          → plain required column
    ///   watchers       → TagType[] (junction array)
    /// AddressType (VO)
    ///   city           → plain column
    /// CaseBaseType (entity)
    ///   baseField      → plain column
    /// ```
    async fn representative_engine() -> GrafeoEngine {
        let engine = GrafeoEngine::in_memory().unwrap();
        for (title, entity) in [
            ("CaseType", true),
            ("CaseBaseType", true),
            ("AddressType", false),
            ("OrgType", true),
            ("TagType", true),
        ] {
            engine
                .ingest_schema(&schema(title, "common", entity))
                .await
                .unwrap();
        }
        engine
            .ingest_schema(&codelist_schema("StatusCode"))
            .await
            .unwrap();

        for prop in [
            classified(
                "homeAddress",
                false,
                RefClassificationKind::ValueObject,
                "AddressType",
                false,
            ),
            classified(
                "postalAddress",
                false,
                RefClassificationKind::ValueObject,
                "AddressType",
                false,
            ),
            classified(
                "owner",
                false,
                RefClassificationKind::EntityReference,
                "OrgType",
                false,
            ),
            classified(
                "status",
                true,
                RefClassificationKind::CodelistReference,
                "StatusCode",
                false,
            ),
            property("title", true),
            classified(
                "watchers",
                false,
                RefClassificationKind::EntityReference,
                "TagType",
                true,
            ),
        ] {
            engine
                .ingest_property("CaseType", "common/CaseType", &prop)
                .await
                .unwrap();
        }
        engine
            .ingest_property("AddressType", "common/AddressType", &property("city", true))
            .await
            .unwrap();
        engine
            .ingest_property(
                "CaseBaseType",
                "common/CaseBaseType",
                &property("baseField", true),
            )
            .await
            .unwrap();

        // ReferencesSchema edges (prop::Title → domain/Title).
        for (prop, target) in [
            ("homeAddress", "AddressType"),
            ("postalAddress", "AddressType"),
            ("owner", "OrgType"),
            ("status", "StatusCode"),
            ("watchers", "TagType"),
        ] {
            engine
                .ingest_edge(
                    &format!("{prop}::CaseType"),
                    &format!("common/{target}"),
                    EdgeType::ReferencesSchema,
                    None,
                )
                .await
                .unwrap();
        }

        // UsesCodeList edge (status → StatusCodeList).
        engine
            .ingest_codelist(&CodeList {
                name: "StatusCodeList".to_string(),
                description: None,
                pg_table_name: "status_code_list".to_string(),
                render_as: "dropdown".to_string(),
                check_expression: None,
            })
            .await
            .unwrap();
        engine
            .ingest_edge(
                "status::CaseType",
                "StatusCodeList",
                EdgeType::UsesCodeList,
                Some(&EdgeProperties {
                    render_as: Some("dropdown".to_string()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();

        // allOf extender: CaseType → CaseBaseType.
        engine
            .ingest_edge(
                "CaseType",
                "CaseBaseType",
                EdgeType::ExtendsSchema,
                Some(&EdgeProperties {
                    composition_type: Some("allOf".to_string()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();

        // Junction array lowering: watchers → ItemsOf → TagType (the target
        // resolution path used by push_junction_array_child).
        engine
            .ingest_edge(
                "watchers::CaseType",
                "common/TagType",
                EdgeType::ItemsOf,
                None,
            )
            .await
            .unwrap();

        engine
    }

    #[tokio::test]
    async fn composition_tree_query_count_pin() {
        // QUERY_COUNT is process-global: hold the counter lock so no
        // concurrent in-crate read test lands inside the measurement.
        let _counter = COUNTER_LOCK.lock().await;
        let engine = representative_engine().await;

        reset_query_count();
        let tree = engine.get_composition_tree("CaseType").await.unwrap();

        // Structural sanity: the fixture exercises every branch.
        assert_eq!(tree.root.schema_title, "CaseType");
        // 4 scalar/VO columns (homeAddress + postalAddress fold into children;
        // watchers folds into a junction child; owner/status/title remain).
        assert_eq!(tree.root.columns.len(), 3, "owner/status/title columns");
        assert_eq!(tree.root.jsonb_columns.len(), 0);
        // 2 VO children + 1 junction child + 1 allOf child.
        assert_eq!(tree.root.children.len(), 4);
        let vo_children = tree
            .root
            .children
            .iter()
            .filter(|c| c.schema_title == "AddressType")
            .count();
        assert_eq!(vo_children, 2, "sibling VOs each get their own child");
        let case_base = tree
            .root
            .children
            .iter()
            .find(|c| c.schema_title == "CaseBaseType")
            .expect("allOf extender child");
        assert_eq!(case_base.columns.len(), 1);

        // PIN: measured GQL executions for this tree build. See the module
        // docs before touching this number. Baseline before the memoized
        // loadout (issue #389): 58. #446 (scalar entity references are
        // never demoted to the jsonb plane) skips the target-schema lookup
        // for the fixture's one scalar entity ref: 47 → 46.
        assert_eq!(query_count(), 46);
    }

    /// Issue #350: the batch ref-target resolution must return identical
    /// data to the N single-key `get_property_ref_target` calls it
    /// replaces (same keys, same targets, including `None` for the
    /// negative space). Executes GQL, so it holds the counter lock like
    /// every other in-crate read test.
    #[tokio::test]
    async fn list_all_property_ref_targets_matches_point_lookups() {
        let _counter = COUNTER_LOCK.lock().await;
        let engine = representative_engine().await;

        let batch = engine.list_all_property_ref_targets().await.unwrap();
        for (pname, stitle) in [
            ("homeAddress", "CaseType"),
            ("postalAddress", "CaseType"),
            ("owner", "CaseType"),
            ("status", "CaseType"),
            ("watchers", "CaseType"),
        ] {
            let point = engine.get_property_ref_target(pname, stitle).await.unwrap();
            assert_eq!(
                batch
                    .get(&(pname.to_string(), stitle.to_string()))
                    .cloned()
                    .flatten(),
                point,
                "{pname}/{stitle}: batch must match the point lookup"
            );
        }
        // Negative space: `title` has no reference edge — the batch carries
        // None so the point query never runs for it again.
        let plain = engine
            .get_property_ref_target("title", "CaseType")
            .await
            .unwrap();
        assert_eq!(plain, None);
        assert_eq!(
            batch
                .get(&("title".to_string(), "CaseType".to_string()))
                .cloned()
                .flatten(),
            None
        );
        // The property universe: 6 CaseType props + 1 AddressType + 1
        // CaseBaseType.
        assert_eq!(batch.len(), 8);
    }
}
