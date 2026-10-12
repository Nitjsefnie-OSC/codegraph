use std::collections::HashMap;

use codegraph_core::error::GraphError;
use codegraph_core::types::{
    CodeList, DetectionSource, EnumValue, ParentCandidate, PropertyNode, SchemaClassificationData,
    SchemaNode,
};

use super::query::{query_gql, query_gql_params, query_many, query_many_params, query_one_params};
use super::{PROPERTY_RETURN_COLS, SCHEMA_RETURN_COLS};
use crate::conversions::{
    RowReader, row_to_codelist, row_to_enum_value, row_to_property_node, row_to_schema_node,
};
use crate::engine::GrafeoEngine;

impl GrafeoEngine {
    pub(super) async fn query_schema(&self, title: &str) -> Result<Option<SchemaNode>, GraphError> {
        let params = HashMap::from([("title".to_string(), grafeo::Value::String(title.into()))]);
        let result = query_gql_params(
            self,
            &format!("MATCH (s:Schema {{title: $title}}) RETURN {SCHEMA_RETURN_COLS}"),
            params,
        )?;
        if result.rows.is_empty() {
            return Ok(None);
        }
        let reader = RowReader::from_columns(&result.columns);
        if result.rows.len() == 1 {
            return Ok(Some(row_to_schema_node(&reader, &result.rows[0])?));
        }
        // Multiple nodes for same title — pick deterministically by domain (alphabetic first)
        let mut schemas: Vec<SchemaNode> = result
            .rows
            .iter()
            .map(|row| row_to_schema_node(&reader, row))
            .collect::<Result<_, _>>()?;
        schemas.sort_by(|a, b| a.domain.cmp(&b.domain));
        Ok(schemas.into_iter().next())
    }

    pub(super) async fn query_schema_by_id(
        &self,
        schema_id: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        let params = HashMap::from([("sid".to_string(), grafeo::Value::String(schema_id.into()))]);
        query_one_params(
            self,
            &format!("MATCH (s:Schema {{schema_id: $sid}}) RETURN {SCHEMA_RETURN_COLS}"),
            params,
            row_to_schema_node,
        )
        .await
    }

    pub(super) async fn query_schema_in_domain(
        &self,
        title: &str,
        domain: &str,
    ) -> Result<Option<SchemaNode>, GraphError> {
        let params = HashMap::from([
            ("title".to_string(), grafeo::Value::String(title.into())),
            ("domain".to_string(), grafeo::Value::String(domain.into())),
        ]);
        query_one_params(
            self,
            &format!(
                "MATCH (s:Schema {{title: $title, domain: $domain}}) RETURN {SCHEMA_RETURN_COLS}"
            ),
            params,
            row_to_schema_node,
        )
        .await
    }

    pub(super) async fn query_schemas(
        &self,
        domain: Option<&str>,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        let result = match domain {
            Some(d) => {
                let params =
                    HashMap::from([("domain".to_string(), grafeo::Value::String(d.into()))]);
                query_gql_params(
                    self,
                    &format!("MATCH (s:Schema {{domain: $domain}}) RETURN {SCHEMA_RETURN_COLS}"),
                    params,
                )?
            }
            None => query_gql(
                self,
                &format!("MATCH (s:Schema) RETURN {SCHEMA_RETURN_COLS}"),
            )?,
        };
        let reader = RowReader::from_columns(&result.columns);
        result
            .rows
            .iter()
            .map(|row| row_to_schema_node(&reader, row))
            .collect()
    }

    pub(super) async fn query_properties(
        &self,
        schema_title: &str,
    ) -> Result<Vec<PropertyNode>, GraphError> {
        let params = HashMap::from([(
            "title".to_string(),
            grafeo::Value::String(schema_title.into()),
        )]);
        query_many_params(
            self,
            &format!(
                "MATCH (:Schema {{title: $title}})-[:HasProperty]->(p:Property) \
                 RETURN {PROPERTY_RETURN_COLS} ORDER BY p.name"
            ),
            params,
            row_to_property_node,
        )
        .await
    }

    pub(super) async fn query_properties_in_domain(
        &self,
        schema_title: &str,
        domain: &str,
    ) -> Result<Vec<PropertyNode>, GraphError> {
        let params = HashMap::from([
            (
                "title".to_string(),
                grafeo::Value::String(schema_title.into()),
            ),
            ("domain".to_string(), grafeo::Value::String(domain.into())),
        ]);
        let result = query_gql_params(
            self,
            &format!(
                "MATCH (:Schema {{title: $title, domain: $domain}})-[:HasProperty]->(p:Property) \
             RETURN {PROPERTY_RETURN_COLS} ORDER BY p.name"
            ),
            params,
        )?;
        if result.rows.is_empty() {
            // Fallback: schema may not exist in this domain, use title-only query
            return self.query_properties(schema_title).await;
        }
        let reader = RowReader::from_columns(&result.columns);
        result
            .rows
            .iter()
            .map(|row| row_to_property_node(&reader, row))
            .collect()
    }

    pub(super) async fn query_child_schemas(
        &self,
        schema_title: &str,
    ) -> Result<Vec<SchemaNode>, GraphError> {
        let params =
            HashMap::from([("ps".to_string(), grafeo::Value::String(schema_title.into()))]);
        // Route 1: inline #/$defs children (parent_schema back-pointer).
        let inline = query_gql_params(
            self,
            &format!("MATCH (s:Schema {{parent_schema: $ps}}) RETURN {SCHEMA_RETURN_COLS}"),
            params.clone(),
        )?;
        // Route 2 (issue #312): derived refers children — array-of-entity-ref
        // properties whose ItemsOf target is an entity (the FK-on-child
        // lowering, child table carries `{parent}_id`). `is_entity` keeps
        // value-object (`contains`) and codelist arrays on their own planes;
        // the title comparison excludes self-references.
        let derived = query_gql_params(
            self,
            &format!(
                "MATCH (:Schema {{title: $ps}})-[:HasProperty]->(:Property {{is_array: true}}) \
                 -[:ItemsOf]->(s:Schema {{is_entity: true}}) RETURN DISTINCT {SCHEMA_RETURN_COLS}"
            ),
            params,
        )?;
        let reader = RowReader::from_columns(&inline.columns);
        let mut children: Vec<SchemaNode> = inline
            .rows
            .iter()
            .map(|row| row_to_schema_node(&reader, row))
            .collect::<Result<_, _>>()?;
        let derived_reader = RowReader::from_columns(&derived.columns);
        let mut seen: std::collections::HashSet<String> =
            children.iter().map(|c| c.title.clone()).collect();
        for row in &derived.rows {
            let node = row_to_schema_node(&derived_reader, row)?;
            // A self-referential ItemsOf target is not a child of itself.
            if node.title == schema_title {
                continue;
            }
            if seen.insert(node.title.clone()) {
                children.push(node);
            }
        }
        children.sort_by(|a, b| a.title.cmp(&b.title));
        Ok(children)
    }

    pub(super) async fn query_classification_data(
        &self,
    ) -> Result<Vec<SchemaClassificationData>, GraphError> {
        let schemas = self.query_schemas(None).await?;

        // Bulk query: all properties with their schema title and required flag.
        // This replaces N individual get_properties() calls.
        let prop_result = query_gql(
            self,
            "MATCH (s:Schema)-[:HasProperty]->(p:Property) RETURN s.title, p.is_required",
        )?;
        let prop_reader = RowReader::from_columns(&prop_result.columns);
        let mut field_counts: HashMap<String, usize> = HashMap::new();
        let mut required_counts: HashMap<String, usize> = HashMap::new();
        for row in &prop_result.rows {
            let title = prop_reader.get_string(row, "s.title")?;
            *field_counts.entry(title.clone()).or_default() += 1;
            let is_req = prop_reader.get_bool(row, "p.is_required").unwrap_or(false);
            if is_req {
                *required_counts.entry(title).or_default() += 1;
            }
        }

        // Bulk query: ref counts (properties that reference another schema).
        let ref_result = query_gql(
            self,
            "MATCH (s:Schema)-[:HasProperty]->(p:Property)-[:ReferencesSchema]->() RETURN s.title",
        )?;
        let ref_reader = RowReader::from_columns(&ref_result.columns);
        let mut ref_counts: HashMap<String, usize> = HashMap::new();
        for row in &ref_result.rows {
            let title = ref_reader.get_string(row, "s.title")?;
            *ref_counts.entry(title).or_default() += 1;
        }

        // Bulk query: in-degree (schemas referenced by other properties).
        let in_result = query_gql(
            self,
            "MATCH ()-[:ReferencesSchema]->(s:Schema) RETURN s.title",
        )?;
        let in_reader = RowReader::from_columns(&in_result.columns);
        let mut in_degrees: HashMap<String, usize> = HashMap::new();
        for row in &in_result.rows {
            let title = in_reader.get_string(row, "s.title")?;
            *in_degrees.entry(title).or_default() += 1;
        }

        // Bulk query: schemas with ExtendsSchema edges (composition check).
        let ext_result = query_gql(self, "MATCH (s:Schema)-[:ExtendsSchema]->() RETURN s.title")?;
        let ext_reader = RowReader::from_columns(&ext_result.columns);
        let mut extends_set: std::collections::HashSet<String> = std::collections::HashSet::new();
        for row in &ext_result.rows {
            let title = ext_reader.get_string(row, "s.title")?;
            extends_set.insert(title);
        }

        // Assemble results using the bulk data
        let mut results = Vec::with_capacity(schemas.len());
        for schema in &schemas {
            let title = &schema.title;
            let field_count = field_counts.get(title).copied().unwrap_or(0);
            let required_field_count = required_counts.get(title).copied().unwrap_or(0);
            let ref_count = ref_counts.get(title).copied().unwrap_or(0);
            let in_degree = in_degrees.get(title).copied().unwrap_or(0);
            let composes_noun_type = extends_set.contains(title);

            results.push(SchemaClassificationData {
                namespace: schema.namespace.clone(),
                title: title.clone(),
                domain: schema.domain.clone(),
                rel_path: schema.rel_path.clone(),
                schema_type: schema.schema_type.clone(),
                is_codelist: schema.is_codelist,
                is_primitive_wrapper: schema.is_primitive_wrapper,
                has_all_of: schema.has_all_of,
                composes_noun_type,
                field_count,
                required_field_count,
                ref_count,
                in_degree,
                is_enum: schema.has_one_of && field_count == 0,
                is_string_type: schema.schema_type == "string",
                is_entity: schema.is_entity,
                source: schema
                    .custom_annotations
                    .get("source")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            });
        }

        Ok(results)
    }

    pub(super) async fn query_entity_names(&self) -> Result<Vec<String>, GraphError> {
        let mut names: Vec<String> = query_many(
            self,
            "MATCH (s:Schema {is_entity: true}) RETURN s.title",
            |reader, row| reader.get_string(row, "s.title"),
        )
        .await?;
        names.sort();
        names.dedup();
        Ok(names)
    }

    pub(super) async fn query_entity_schema_map(
        &self,
    ) -> Result<HashMap<String, String>, GraphError> {
        let result = query_gql(
            self,
            "MATCH (s:Schema {is_entity: true}) RETURN s.title, s.rel_path",
        )?;
        let reader = RowReader::from_columns(&result.columns);
        let mut map = HashMap::new();
        for row in &result.rows {
            let title = reader.get_string(row, "s.title")?;
            let rel_path = reader.get_string(row, "s.rel_path")?;
            map.insert(title, rel_path);
        }
        Ok(map)
    }

    pub(super) async fn query_value_object_schemas(&self) -> Result<Vec<SchemaNode>, GraphError> {
        query_many(
            self,
            &format!(
                "MATCH (s:Schema) WHERE s.is_entity = false AND s.is_codelist = false AND s.schema_type = 'object' \
                 RETURN {SCHEMA_RETURN_COLS}"
            ),
            row_to_schema_node,
        )
        .await
    }

    pub(super) async fn query_parent_candidates(&self) -> Result<Vec<ParentCandidate>, GraphError> {
        let gql = "MATCH (child:Schema)-[:HasProperty]->(p:Property {is_array: false})-[:ReferencesSchema]->(parent:Schema {is_entity: true}) \
                   RETURN DISTINCT child.title, parent.title, p.name";
        let result = query_gql(self, gql)?;
        let reader = RowReader::from_columns(&result.columns);
        let mut candidates = Vec::new();
        for row in &result.rows {
            candidates.push(ParentCandidate {
                child_title: reader.get_string(row, "child.title")?,
                parent_title: reader.get_string(row, "parent.title")?,
                field_name: reader.get_string(row, "p.name")?,
                source: DetectionSource::ScalarRef,
            });
        }

        // Detect one-to-many relationships: parent entity has an array property
        // whose items reference a child entity (ItemsOf edge).
        let array_gql = "MATCH (parent:Schema {is_entity: true})-[:HasProperty]->(p:Property {is_array: true})-[:ItemsOf]->(child:Schema {is_entity: true}) \
                          RETURN DISTINCT child.title, parent.title, p.name";
        let array_result = query_gql(self, array_gql)?;
        let array_reader = RowReader::from_columns(&array_result.columns);
        let scalar_keys: std::collections::HashSet<(String, String)> = candidates
            .iter()
            .map(|c| (c.child_title.clone(), c.parent_title.clone()))
            .collect();
        for row in &array_result.rows {
            let child_title = array_reader.get_string(row, "child.title")?;
            let parent_title = array_reader.get_string(row, "parent.title")?;
            let field_name = array_reader.get_string(row, "p.name")?;
            if scalar_keys.contains(&(child_title.clone(), parent_title.clone())) {
                continue;
            }
            candidates.push(ParentCandidate {
                child_title,
                parent_title,
                field_name,
                source: DetectionSource::ArrayItems,
            });
        }

        Ok(candidates)
    }

    pub(super) async fn query_codelist(&self, name: &str) -> Result<Option<CodeList>, GraphError> {
        let params = HashMap::from([("name".to_string(), grafeo::Value::String(name.into()))]);
        query_one_params(
            self,
            "MATCH (c:CodeList {name: $name}) RETURN c.name, c.description, \
             c.pg_table_name, c.render_as, c.check_expression",
            params,
            row_to_codelist,
        )
        .await
    }

    /// Composition table-conflict detection (see
    /// `GraphQuerier::composition_table_conflict`): a ValueObject-classified
    /// property on some OTHER schema references this schema (value targets
    /// only — refers-wins entity targets keep their own table), and the
    /// parent's `{parent}_{feature}` child-table projection equals this
    /// schema's `pg_table_name`. The three property fields mirror
    /// `PropertyNode::effective_kind`'s resolution order; the edge is the
    /// property→target link the mox lowering writes — `ItemsOf` for the
    /// child-table (contains) semantics, `ReferencesSchema` for the JSON
    /// path.
    pub(super) async fn query_composition_table_conflict(
        &self,
        schema_title: &str,
    ) -> Result<Option<(String, String)>, GraphError> {
        let params = HashMap::from([(
            "title".to_string(),
            grafeo::Value::String(schema_title.into()),
        )]);
        let result = query_gql_params(
            self,
            "MATCH (p:Property)-[:ItemsOf|ReferencesSchema]->(me:Schema), (parent:Schema) \
             WHERE (me.title = $title OR me.pg_table_name = $title) \
               AND parent.title = p._schema_title \
               AND (p.classification_kind = 'value_object' \
                OR p.classification = 'value_object' \
                OR p.render_strategy = 'value_object') \
               AND me.is_entity = false \
             RETURN parent.title, parent.pg_table_name, p.pg_column_name, me.pg_table_name",
            params,
        )?;
        let reader = RowReader::from_columns(&result.columns);
        for row in &result.rows {
            let parent_table = reader.get_string(row, "parent.pg_table_name")?;
            let feature = reader.get_string(row, "p.pg_column_name")?;
            let own_table = reader.get_string(row, "me.pg_table_name")?;
            let projected = codegraph_naming::child_table_name(&parent_table, &feature);
            if projected == own_table {
                let parent_title = reader.get_string(row, "parent.title")?;
                return Ok(Some((parent_title, projected)));
            }
        }
        Ok(None)
    }

    pub(super) async fn query_codelists(&self) -> Result<Vec<CodeList>, GraphError> {
        query_many(
            self,
            "MATCH (c:CodeList) RETURN c.name, c.description, c.pg_table_name, c.render_as, c.check_expression",
            row_to_codelist,
        )
        .await
    }

    pub(super) async fn query_enum_values(
        &self,
        codelist_name: &str,
    ) -> Result<Vec<EnumValue>, GraphError> {
        let params = HashMap::from([(
            "name".to_string(),
            grafeo::Value::String(codelist_name.into()),
        )]);
        query_many_params(
            self,
            "MATCH (:CodeList {name: $name})-[:HasEnumValue]->(v:EnumValue) \
             RETURN v.value, v.display_name, v.sort_order",
            params,
            row_to_enum_value,
        )
        .await
    }
}
