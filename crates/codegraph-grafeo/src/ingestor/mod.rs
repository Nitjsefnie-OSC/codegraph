//! Grafeo ingestor split (issue #366), mirroring the `querier/` layout:
//! family files hold inherent `insert_*` methods (bodies moved verbatim from
//! the former single-file `ingestor.rs`), and this module holds the ONE
//! `#[async_trait] impl GraphIngestor for GrafeoEngine` adapter that
//! delegates to them (Rust coherence forbids splitting one trait impl
//! across modules).
//!
//! - `gql.rs` — shared GQL/escape/param helpers (`escape_gql` stays
//!   `pub(crate)`; the rest are `pub(super)`)
//! - `edges.rs` — generic `insert_edge` (all `EdgeType` match arms verbatim)
//! - `nodes_core` (this file) — schema/property/codelist/namespace ingest +
//!   finalize/update methods (largest block; kept here under the ~900-line
//!   rule so no separate `nodes_core.rs` was needed)
//! - `nodes_ifml.rs` — the 6 IFML ingest methods
//! - `nodes_authz.rs` — policy + actor/capability/grant ingest
//! - `nodes_misc.rs` — regulatory, function/rule, atproto, API metamodel,
//!   persistence/security, mox remainder

mod edges;
mod gql;
mod nodes_authz;
mod nodes_ddd;
mod nodes_deploy;
mod nodes_evt;
mod nodes_ifml;
mod nodes_misc;

use std::collections::HashMap;

use async_trait::async_trait;
use codegraph_core::error::GraphError;
use codegraph_core::traits::GraphIngestor;
use codegraph_core::types::{
    ActionNode, ActorPolicyModel, ApiOperationNode, ApiResourceNode, AtprotoNamespaceNode,
    CodeList, CollectionNode, CompositeColumn, CompositeRange, ConditionKind, ConditionNode,
    DataBindingNode, DddModelGraph, DeployModelGraph, EdgeProperties, EdgeType, EnumValue,
    ErrorDefinitionNode, EventNode, EvtModelGraph, FunctionNode, HttpEndpointNode, IngestStats,
    InteractionNode, LexiconNode, MembershipNode, ModuleDefinitionNode, MoxDomainModel,
    NamespaceImport, NamespaceNode, ParameterDefinitionNode, PermissionNode, PipelineNode,
    PolicyNode, PropertyNode, RegulatoryEdgeKind, RegulatoryKind, RegulatoryNode, RegulatoryOwner,
    RelationshipNode, RepositoryNode, RuleNode, SchemaNode, SecurityIdentityNode, TenantNode,
    ViewComponentNode, ViewContainerNode,
};

use self::gql::{
    bool_to_grafeo_value, classification_kind_to_str, count_from_gql, escape_gql, opt_str,
    opt_to_grafeo_value,
};
use crate::engine::GrafeoEngine;

impl GrafeoEngine {
    pub(super) async fn insert_schema(&self, node: &SchemaNode) -> Result<String, GraphError> {
        let session = self.db().session();
        let gql = "INSERT (:Schema {\
            schema_id: $schema_id, title: $title, description: $description, \
            schema_type: $schema_type, classification: $classification, \
            pg_type: $pg_type, rust_type: $rust_type, sea_orm_type: $sea_orm_type, \
            domain: $domain, namespace: $namespace, rel_path: $rel_path, \
            rust_type_name: $rust_type_name, pg_table_name: $pg_table_name, \
            api_path_segment: $api_path_segment, \
            parent_schema: $parent_schema, \
            is_entity: $is_entity, is_codelist: $is_codelist, \
            is_primitive_wrapper: $is_primitive_wrapper, \
            has_all_of: $has_all_of, has_one_of: $has_one_of, \
            has_any_of: $has_any_of, has_definitions: $has_definitions, \
            custom_annotations: $custom_annotations, \
            access: $access, annotations: $annotations\
        })";
        let custom_annotations_str =
            serde_json::to_string(&node.custom_annotations).unwrap_or_else(|_| "{}".to_string());
        let annotations_str = node
            .annotations
            .as_ref()
            .map(|a| serde_json::to_string(a).unwrap_or_default());
        let access_str = node.access.as_ref().map(|a| match a {
            codegraph_core::types::Access::Public => "public".to_string(),
            codegraph_core::types::Access::Private => "private".to_string(),
        });
        let params = HashMap::from([
            (
                "schema_id".into(),
                grafeo::Value::String(node.schema_id.clone().into()),
            ),
            (
                "title".into(),
                grafeo::Value::String(node.title.clone().into()),
            ),
            ("description".into(), opt_to_grafeo_value(&node.description)),
            (
                "schema_type".into(),
                grafeo::Value::String(node.schema_type.clone().into()),
            ),
            (
                "classification".into(),
                grafeo::Value::String(node.classification.clone().into()),
            ),
            (
                "pg_type".into(),
                grafeo::Value::String(node.pg_type.clone().into()),
            ),
            (
                "rust_type".into(),
                grafeo::Value::String(node.rust_type.clone().into()),
            ),
            (
                "sea_orm_type".into(),
                grafeo::Value::String(node.sea_orm_type.clone().into()),
            ),
            ("domain".into(), opt_to_grafeo_value(&node.domain)),
            ("namespace".into(), opt_to_grafeo_value(&node.namespace)),
            (
                "rel_path".into(),
                grafeo::Value::String(node.rel_path.clone().into()),
            ),
            (
                "rust_type_name".into(),
                grafeo::Value::String(node.rust_type_name.clone().into()),
            ),
            (
                "pg_table_name".into(),
                grafeo::Value::String(node.pg_table_name.clone().into()),
            ),
            (
                "api_path_segment".into(),
                grafeo::Value::String(node.api_path_segment.clone().into()),
            ),
            (
                "parent_schema".into(),
                opt_to_grafeo_value(&node.parent_schema),
            ),
            ("is_entity".into(), bool_to_grafeo_value(node.is_entity)),
            ("is_codelist".into(), bool_to_grafeo_value(node.is_codelist)),
            (
                "is_primitive_wrapper".into(),
                bool_to_grafeo_value(node.is_primitive_wrapper),
            ),
            ("has_all_of".into(), bool_to_grafeo_value(node.has_all_of)),
            ("has_one_of".into(), bool_to_grafeo_value(node.has_one_of)),
            ("has_any_of".into(), bool_to_grafeo_value(node.has_any_of)),
            (
                "has_definitions".into(),
                bool_to_grafeo_value(node.has_definitions),
            ),
            (
                "custom_annotations".into(),
                grafeo::Value::String(custom_annotations_str.into()),
            ),
            ("access".into(), opt_to_grafeo_value(&access_str)),
            ("annotations".into(), opt_to_grafeo_value(&annotations_str)),
        ]);
        session
            .execute_with_params(gql, params)
            .map_err(|e| GraphError::Ingest(format!("ingest_schema failed: {e}")))?;
        Ok(node.schema_id.clone())
    }

    pub(super) async fn insert_property(
        &self,
        schema_title: &str,
        schema_id: &str,
        prop: &PropertyNode,
    ) -> Result<(), GraphError> {
        let session = self.db().session();
        let gql = "INSERT (:Property {\
            name: $name, prop_type: $prop_type, description: $description, \
            format: $format, \
            is_required: $is_required, is_nullable: $is_nullable, \
            is_id: $is_id, is_array: $is_array, pattern: $pattern, \
            min_length: $min_length, max_length: $max_length, \
            min_items: $min_items, max_items: $max_items, \
            minimum: $minimum, maximum: $maximum, \
            pg_column_name: $pg_column_name, pg_column_type: $pg_column_type, \
            rust_field_name: $rust_field_name, rust_field_type: $rust_field_type, \
            sea_orm_type: $sea_orm_type, render_strategy: $render_strategy, \
            ref_target: $ref_target, classification: $classification, \
            classification_kind: $classification_kind, \
            _schema_title: $schema_title, _schema_id: $schema_id\
        })";
        let classification_kind_str = prop
            .classification_kind
            .as_ref()
            .map(classification_kind_to_str);
        // Bounds persist as STRING: Decimal has no native grafeo Value, and
        // conversions.rs parses all of them back from strings.
        let min_length_str = prop.min_length.map(|v| v.to_string());
        let max_length_str = prop.max_length.map(|v| v.to_string());
        let min_items_str = prop.min_items.map(|v| v.to_string());
        let max_items_str = prop.max_items.map(|v| v.to_string());
        let minimum_str = prop.minimum.map(|v| v.to_string());
        let maximum_str = prop.maximum.map(|v| v.to_string());
        let params = HashMap::from([
            (
                "name".into(),
                grafeo::Value::String(prop.name.clone().into()),
            ),
            (
                "prop_type".into(),
                grafeo::Value::String(prop.prop_type.clone().into()),
            ),
            ("description".into(), opt_to_grafeo_value(&prop.description)),
            ("format".into(), opt_to_grafeo_value(&prop.format)),
            ("is_required".into(), bool_to_grafeo_value(prop.is_required)),
            ("is_nullable".into(), bool_to_grafeo_value(prop.is_nullable)),
            ("is_id".into(), bool_to_grafeo_value(prop.is_id)),
            ("is_array".into(), bool_to_grafeo_value(prop.is_array)),
            ("pattern".into(), opt_to_grafeo_value(&prop.pattern)),
            ("min_length".into(), opt_to_grafeo_value(&min_length_str)),
            ("max_length".into(), opt_to_grafeo_value(&max_length_str)),
            ("min_items".into(), opt_to_grafeo_value(&min_items_str)),
            ("max_items".into(), opt_to_grafeo_value(&max_items_str)),
            ("minimum".into(), opt_to_grafeo_value(&minimum_str)),
            ("maximum".into(), opt_to_grafeo_value(&maximum_str)),
            (
                "pg_column_name".into(),
                grafeo::Value::String(prop.pg_column_name.clone().into()),
            ),
            (
                "pg_column_type".into(),
                grafeo::Value::String(prop.pg_column_type.clone().into()),
            ),
            (
                "rust_field_name".into(),
                grafeo::Value::String(prop.rust_field_name.clone().into()),
            ),
            (
                "rust_field_type".into(),
                grafeo::Value::String(prop.rust_field_type.clone().into()),
            ),
            (
                "sea_orm_type".into(),
                grafeo::Value::String(prop.sea_orm_type.clone().into()),
            ),
            (
                "render_strategy".into(),
                grafeo::Value::String(prop.render_strategy.clone().into()),
            ),
            ("ref_target".into(), opt_to_grafeo_value(&prop.ref_target)),
            (
                "classification".into(),
                opt_to_grafeo_value(&prop.classification),
            ),
            (
                "classification_kind".into(),
                opt_to_grafeo_value(&classification_kind_str),
            ),
            (
                "schema_title".into(),
                grafeo::Value::String(schema_title.into()),
            ),
            ("schema_id".into(), grafeo::Value::String(schema_id.into())),
        ]);
        session
            .execute_with_params(gql, params)
            .map_err(|e| GraphError::Ingest(format!("ingest_property INSERT failed: {e}")))?;

        let edge_gql = "MATCH (s:Schema {title: $st}), (p:Property {name: $pn, _schema_title: $st2}) \
             INSERT (s)-[:HasProperty]->(p)";
        let edge_params = HashMap::from([
            ("st".into(), grafeo::Value::String(schema_title.into())),
            ("st2".into(), grafeo::Value::String(schema_title.into())),
            ("pn".into(), grafeo::Value::String(prop.name.clone().into())),
        ]);
        session
            .execute_with_params(edge_gql, edge_params)
            .map_err(|e| {
                GraphError::Ingest(format!("ingest_property HasProperty edge failed: {e}"))
            })?;
        Ok(())
    }

    pub(super) async fn insert_condition(&self, node: &ConditionNode) -> Result<(), GraphError> {
        let session = self.db().session();
        let options_json =
            serde_json::to_string(&node.options).unwrap_or_else(|_| "[]".to_string());
        let options_str = format!("'{}'", escape_gql(&options_json));
        let gql = format!(
            "INSERT (:Condition {{name: '{name}', owner_title: '{owner}', \
             kind: '{kind}', expr_json: {expr}, options: {options}, \
             definition: {definition}, domain: {domain}}})",
            name = escape_gql(&node.name),
            owner = escape_gql(&node.owner_title),
            kind = match node.kind {
                ConditionKind::Condition => "condition",
                ConditionKind::OneOf => "one_of",
            },
            expr = opt_str(&node.expr_json),
            options = options_str,
            definition = opt_str(&node.definition),
            domain = opt_str(&node.domain),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_condition INSERT failed: {e}")))?;
        // Link the condition to its owning schema (Schema → Condition).
        let edge = format!(
            "MATCH (a:Schema), (b:Condition) \
             WHERE a.title = '{owner}' AND b.name = '{name}' \
             INSERT (a)-[:HasCondition]->(b)",
            owner = escape_gql(&node.owner_title),
            name = escape_gql(&node.name),
        );
        session
            .execute(&edge)
            .map_err(|e| GraphError::Ingest(format!("ingest_condition edge failed: {e}")))?;
        Ok(())
    }

    pub(super) async fn insert_codelist(&self, codelist: &CodeList) -> Result<(), GraphError> {
        let session = self.db().session();
        let gql = format!(
            "INSERT (:CodeList {{name: '{name}', description: {description}, \
             pg_table_name: '{pg_table_name}', render_as: '{render_as}', \
             check_expression: {check_expression}}})",
            name = escape_gql(&codelist.name),
            description = opt_str(&codelist.description),
            pg_table_name = escape_gql(&codelist.pg_table_name),
            render_as = escape_gql(&codelist.render_as),
            check_expression = opt_str(&codelist.check_expression),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(e.to_string()))?;
        Ok(())
    }

    pub(super) async fn insert_enum_value(
        &self,
        codelist_name: &str,
        value: &EnumValue,
    ) -> Result<(), GraphError> {
        let session = self.db().session();
        let gql = format!(
            "INSERT (:EnumValue {{value: '{val}', display_name: {dn}, sort_order: {so}, \
             _codelist_name: '{cn}'}})",
            val = escape_gql(&value.value),
            dn = opt_str(&value.display_name),
            so = value.sort_order,
            cn = escape_gql(codelist_name),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(e.to_string()))?;

        let edge_gql = format!(
            "MATCH (c:CodeList {{name: '{cn}'}}), \
             (v:EnumValue {{value: '{val}', _codelist_name: '{cn}'}}) \
             INSERT (c)-[:HasEnumValue]->(v)",
            cn = escape_gql(codelist_name),
            val = escape_gql(&value.value),
        );
        session
            .execute(&edge_gql)
            .map_err(|e| GraphError::Ingest(e.to_string()))?;
        Ok(())
    }

    pub(super) async fn insert_composite_column(
        &self,
        col: &CompositeColumn,
    ) -> Result<(), GraphError> {
        let session = self.db().session();
        let gql = format!(
            "MERGE (:CompositeColumn {{suffix: '{suffix}', wrapper_schema: '{wrapper_schema}', \
             pg_type: '{pg_type}', rust_type: '{rust_type}', sea_orm_type: '{sea_orm_type}', \
             fk_target: {fk_target}, dto_rust_type: {dto_rust_type}}})",
            suffix = escape_gql(&col.suffix),
            wrapper_schema = escape_gql(&col.wrapper_schema),
            pg_type = escape_gql(&col.pg_type),
            rust_type = escape_gql(&col.rust_type),
            sea_orm_type = escape_gql(&col.sea_orm_type),
            fk_target = opt_str(&col.fk_target),
            dto_rust_type = opt_str(&col.dto_rust_type),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(e.to_string()))?;
        Ok(())
    }

    pub(super) async fn insert_extension(&self, name: &str) -> Result<(), GraphError> {
        let session = self.db().session();
        let gql = format!("MERGE (:Extension {{name: '{}'}})", escape_gql(name),);
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(e.to_string()))?;
        Ok(())
    }

    pub(super) async fn insert_composite_range(
        &self,
        range: &CompositeRange,
    ) -> Result<(), GraphError> {
        let session = self.db().session();
        let gql = format!(
            "INSERT (:CompositeRange {{pg_column_name: '{pg_col}', pg_type: '{pg_type}', \
             rust_type: '{rust_type}', start_field: '{start}', end_field: '{end}', \
             open_end: {open_end}}})",
            pg_col = escape_gql(&range.pg_column_name),
            pg_type = escape_gql(&range.pg_type),
            rust_type = escape_gql(&range.rust_type),
            start = escape_gql(&range.start_field),
            end = escape_gql(&range.end_field),
            open_end = range.open_end,
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(e.to_string()))?;
        Ok(())
    }

    pub(super) async fn insert_namespace(
        &self,
        node: &NamespaceNode,
    ) -> Result<String, GraphError> {
        let session = self.db().session();
        let gql = format!(
            "INSERT (:Namespace {{ fqn: '{}', parent: {}, source: {} }})",
            escape_gql(&node.fqn),
            opt_str(&node.parent),
            opt_str(&node.source),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_namespace failed: {e}")))?;
        // Persist the hierarchy as an edge too (child → parent) so graph
        // traversals see it; the flat `parent` property keeps read-back to
        // a single query.
        if let Some(parent) = &node.parent {
            let edge = format!(
                "MATCH (a:Namespace {{fqn: '{}'}}), (b:Namespace {{fqn: '{}'}}) \
                 INSERT (a)-[:NamespaceParent]->(b)",
                escape_gql(&node.fqn),
                escape_gql(parent),
            );
            session.execute(&edge).map_err(|e| {
                GraphError::Ingest(format!("ingest_namespace parent edge failed: {e}"))
            })?;
        }
        Ok(node.fqn.clone())
    }

    pub(super) async fn insert_namespace_import(
        &self,
        import: &NamespaceImport,
    ) -> Result<(), GraphError> {
        let session = self.db().session();
        let gql = format!(
            "MATCH (a:Namespace {{fqn: '{}'}}), (b:Namespace {{fqn: '{}'}}) \
             INSERT (a)-[:NamespaceImports {{wildcard: {}, alias: {}}}]->(b)",
            escape_gql(&import.from_ns),
            escape_gql(&import.to_ns),
            import.wildcard,
            opt_str(&import.alias),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_namespace_import failed: {e}")))?;
        Ok(())
    }

    pub(super) async fn insert_finalize(&self) -> Result<IngestStats, GraphError> {
        Ok(IngestStats {
            schema_count: count_from_gql(self, "MATCH (s:Schema) RETURN count(s) AS cnt")?,
            property_count: count_from_gql(self, "MATCH (p:Property) RETURN count(p) AS cnt")?,
            reference_edge_count: count_from_gql(
                self,
                "MATCH ()-[r:ReferencesSchema]->() RETURN count(r) AS cnt",
            )?,
            composition_edge_count: count_from_gql(
                self,
                "MATCH ()-[r:ExtendsSchema]->() RETURN count(r) AS cnt",
            )?,
            codelist_count: count_from_gql(self, "MATCH (c:CodeList) RETURN count(c) AS cnt")?,
            enum_value_count: count_from_gql(self, "MATCH (v:EnumValue) RETURN count(v) AS cnt")?,
            composite_column_count: count_from_gql(
                self,
                "MATCH (c:CompositeColumn) RETURN count(c) AS cnt",
            )?,
            composite_range_count: count_from_gql(
                self,
                "MATCH (r:CompositeRange) RETURN count(r) AS cnt",
            )?,
            domain_count: count_from_gql(self, "MATCH (d:Domain) RETURN count(d) AS cnt")?,
            ifml_node_count: count_from_gql(
                self,
                "MATCH (n) WHERE n:ViewContainer OR n:ViewComponent OR n:Event OR n:ActionNode OR n:ParameterDefinition OR n:DataBinding RETURN count(n) AS cnt",
            )?,
            lexicons_ingested: count_from_gql(self, "MATCH (l:Lexicon) RETURN count(l) AS cnt")?,
            collections_ingested: count_from_gql(
                self,
                "MATCH (c:Collection) RETURN count(c) AS cnt",
            )?,
            namespaces_ingested: count_from_gql(
                self,
                "MATCH (n:Namespace) RETURN count(n) AS cnt",
            )?,
            repositories_ingested: count_from_gql(
                self,
                "MATCH (r:Repository) RETURN count(r) AS cnt",
            )?,
            api_resource_count: count_from_gql(
                self,
                "MATCH (r:ApiResource) RETURN count(r) AS cnt",
            )?,
            policy_count: count_from_gql(self, "MATCH (p:Policy) RETURN count(p) AS cnt")?,
            relationship_count: count_from_gql(
                self,
                "MATCH (r:Relationship) RETURN count(r) AS cnt",
            )?,
            security_node_count: count_from_gql(
                self,
                "MATCH (n) WHERE n:SecurityIdentity OR n:Membership OR n:Tenant RETURN count(n) AS cnt",
            )?,
            duration: self.start_time().elapsed(),
        })
    }

    pub(super) async fn insert_update_entity_flag(
        &self,
        title: &str,
        is_entity: bool,
    ) -> Result<(), GraphError> {
        let session = self.db().session();
        let query = format!(
            "MATCH (s:Schema {{title: '{}'}}) SET s.is_entity = {}",
            title.replace('\'', "\\'"),
            is_entity
        );
        session
            .execute(&query)
            .map_err(|e| GraphError::Query(e.to_string()))?;
        Ok(())
    }

    pub(super) async fn insert_update_property_classification(
        &self,
        schema_title: &str,
        property_name: &str,
        kind: &str,
    ) -> Result<(), GraphError> {
        let session = self.db().session();
        let query = format!(
            "MATCH (s:Schema {{title: '{}'}})-[:HasProperty]->(p:Property {{name: '{}'}}) SET p.classification_kind = '{}'",
            schema_title.replace('\'', "\\'"),
            property_name.replace('\'', "\\'"),
            kind.replace('\'', "\\'"),
        );
        session
            .execute(&query)
            .map_err(|e| GraphError::Query(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl GraphIngestor for GrafeoEngine {
    async fn ingest_schema(&self, node: &SchemaNode) -> Result<String, GraphError> {
        self.insert_schema(node).await
    }

    async fn ingest_property(
        &self,
        schema_title: &str,
        schema_id: &str,
        prop: &PropertyNode,
    ) -> Result<(), GraphError> {
        self.insert_property(schema_title, schema_id, prop).await
    }

    async fn ingest_condition(&self, node: &ConditionNode) -> Result<(), GraphError> {
        self.insert_condition(node).await
    }

    async fn ingest_regulatory(&self, node: &RegulatoryNode) -> Result<(), GraphError> {
        self.insert_regulatory(node).await
    }

    async fn ingest_regulatory_reference(
        &self,
        owner: &RegulatoryOwner,
        target: &str,
        target_kind: RegulatoryKind,
        edge_kind: RegulatoryEdgeKind,
        ref_path: Option<&str>,
    ) -> Result<(), GraphError> {
        self.insert_regulatory_reference(owner, target, target_kind, edge_kind, ref_path)
            .await
    }

    async fn ingest_function(&self, node: &FunctionNode) -> Result<(), GraphError> {
        self.insert_function(node).await
    }

    async fn ingest_rule(&self, node: &RuleNode) -> Result<(), GraphError> {
        self.insert_rule(node).await
    }

    async fn ingest_codelist(&self, codelist: &CodeList) -> Result<(), GraphError> {
        self.insert_codelist(codelist).await
    }

    async fn ingest_enum_value(
        &self,
        codelist_name: &str,
        value: &EnumValue,
    ) -> Result<(), GraphError> {
        self.insert_enum_value(codelist_name, value).await
    }

    async fn ingest_composite_column(&self, col: &CompositeColumn) -> Result<(), GraphError> {
        self.insert_composite_column(col).await
    }

    async fn ingest_extension(&self, name: &str) -> Result<(), GraphError> {
        self.insert_extension(name).await
    }

    async fn ingest_composite_range(&self, range: &CompositeRange) -> Result<(), GraphError> {
        self.insert_composite_range(range).await
    }

    async fn ingest_edge(
        &self,
        from_id: &str,
        to_id: &str,
        edge_type: EdgeType,
        props: Option<&EdgeProperties>,
    ) -> Result<(), GraphError> {
        self.insert_edge(from_id, to_id, edge_type, props).await
    }

    async fn ingest_view_container(&self, node: &ViewContainerNode) -> Result<String, GraphError> {
        self.insert_view_container(node).await
    }

    async fn ingest_module_definition(
        &self,
        node: &ModuleDefinitionNode,
    ) -> Result<String, GraphError> {
        self.insert_module_definition(node).await
    }

    async fn ingest_view_component(&self, node: &ViewComponentNode) -> Result<String, GraphError> {
        self.insert_view_component(node).await
    }

    async fn ingest_event(&self, node: &EventNode) -> Result<String, GraphError> {
        self.insert_event(node).await
    }

    async fn ingest_action_node(&self, node: &ActionNode) -> Result<String, GraphError> {
        self.insert_action_node(node).await
    }

    async fn ingest_parameter_definition(
        &self,
        node: &ParameterDefinitionNode,
    ) -> Result<String, GraphError> {
        self.insert_parameter_definition(node).await
    }

    async fn ingest_data_binding(&self, node: &DataBindingNode) -> Result<String, GraphError> {
        self.insert_data_binding(node).await
    }

    async fn ingest_atproto_namespace(
        &self,
        node: &AtprotoNamespaceNode,
    ) -> Result<String, GraphError> {
        self.insert_atproto_namespace(node).await
    }

    async fn ingest_namespace(&self, node: &NamespaceNode) -> Result<String, GraphError> {
        self.insert_namespace(node).await
    }

    async fn ingest_namespace_import(&self, import: &NamespaceImport) -> Result<(), GraphError> {
        self.insert_namespace_import(import).await
    }

    async fn ingest_lexicon(&self, node: &LexiconNode) -> Result<String, GraphError> {
        self.insert_lexicon(node).await
    }

    async fn ingest_collection(&self, node: &CollectionNode) -> Result<String, GraphError> {
        self.insert_collection(node).await
    }

    async fn ingest_repository(&self, node: &RepositoryNode) -> Result<String, GraphError> {
        self.insert_repository(node).await
    }

    async fn finalize(&self) -> Result<IngestStats, GraphError> {
        self.insert_finalize().await
    }

    async fn update_entity_flag(&self, title: &str, is_entity: bool) -> Result<(), GraphError> {
        self.insert_update_entity_flag(title, is_entity).await
    }

    async fn update_property_classification(
        &self,
        schema_title: &str,
        property_name: &str,
        kind: &str,
    ) -> Result<(), GraphError> {
        self.insert_update_property_classification(schema_title, property_name, kind)
            .await
    }

    async fn ingest_api_resource(&self, node: &ApiResourceNode) -> Result<String, GraphError> {
        self.insert_api_resource(node).await
    }

    async fn ingest_api_operation(&self, node: &ApiOperationNode) -> Result<String, GraphError> {
        self.insert_api_operation(node).await
    }

    async fn ingest_interaction(&self, node: &InteractionNode) -> Result<String, GraphError> {
        self.insert_interaction(node).await
    }

    async fn ingest_http_endpoint(&self, node: &HttpEndpointNode) -> Result<String, GraphError> {
        self.insert_http_endpoint(node).await
    }

    async fn ingest_pipeline(&self, node: &PipelineNode) -> Result<String, GraphError> {
        self.insert_pipeline(node).await
    }

    async fn ingest_error_definition(
        &self,
        node: &ErrorDefinitionNode,
    ) -> Result<String, GraphError> {
        self.insert_error_definition(node).await
    }

    async fn ingest_permission(&self, node: &PermissionNode) -> Result<String, GraphError> {
        self.insert_permission(node).await
    }

    async fn ingest_policy(&self, policy: &PolicyNode) -> Result<(), GraphError> {
        self.insert_policy(policy).await
    }

    async fn ingest_relationship(&self, relationship: &RelationshipNode) -> Result<(), GraphError> {
        self.insert_relationship(relationship).await
    }

    async fn ingest_security_identity(
        &self,
        identity: &SecurityIdentityNode,
    ) -> Result<(), GraphError> {
        self.insert_security_identity(identity).await
    }

    async fn ingest_membership(&self, membership: &MembershipNode) -> Result<(), GraphError> {
        self.insert_membership(membership).await
    }

    async fn ingest_tenant(&self, tenant: &TenantNode) -> Result<(), GraphError> {
        self.insert_tenant(tenant).await
    }

    async fn ingest_actor_policy(&self, model: &ActorPolicyModel) -> Result<(), GraphError> {
        self.insert_actor_policy(model).await
    }

    async fn ingest_mox_domain(&self, model: &MoxDomainModel) -> Result<(), GraphError> {
        self.insert_mox_domain(model).await
    }

    async fn ingest_ddd_model(&self, model: &DddModelGraph) -> Result<(), GraphError> {
        self.insert_ddd_model(model).await
    }

    async fn ingest_evt_model(&self, model: &EvtModelGraph) -> Result<(), GraphError> {
        self.insert_evt_model(model).await
    }

    async fn ingest_deploy_model(&self, model: &DeployModelGraph) -> Result<(), GraphError> {
        self.insert_deploy_model(model).await
    }
}
