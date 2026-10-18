use std::collections::HashMap;

use codegraph_core::error::GraphError;
use codegraph_core::types::{EdgeProperties, EdgeType, strip_ifml_prefix};

use super::gql::CountingSession;
use super::gql::{
    build_edge_props_string, decode_regulatory_edge_ids, edge_kind_ref, escape_gql,
    regulatory_reference_gql, split_compound_id, strip_api_prefix,
};
use super::registry::keys;
use crate::engine::GrafeoEngine;

impl GrafeoEngine {
    pub(super) async fn insert_edge(
        &self,
        from_id: &str,
        to_id: &str,
        edge_type: EdgeType,
        props: Option<&EdgeProperties>,
    ) -> Result<(), GraphError> {
        crate::perf::bump_ingest_edge();
        let session = CountingSession::new(self.db().session());

        // Regulatory reference plane (issue #265) — encoded natural keys,
        // same GQL the dedicated `ingest_regulatory_reference` emits.
        if matches!(
            edge_type,
            EdgeType::RegulatoryReference
                | EdgeType::HasRuleSource
                | EdgeType::CorpusInBody
                | EdgeType::DerivesFrom
        ) {
            let Some((owner, target, target_kind)) = decode_regulatory_edge_ids(from_id, to_id)
            else {
                return Err(GraphError::Ingest(format!(
                    "ingest_edge: regulatory edge {edge_type:?} needs encoded ids \
                     (regowner:... -> reg:...), got '{from_id}' -> '{to_id}'"
                )));
            };
            let ref_path = props.and_then(|p| p.ref_path.as_deref());
            let gql = regulatory_reference_gql(
                &owner,
                &target,
                target_kind,
                edge_kind_ref(&edge_type),
                ref_path,
            );
            session
                .execute(&gql)
                .map_err(|e| GraphError::Ingest(format!("ingest_edge regulatory failed: {e}")))?;
            return Ok(());
        }

        // `InNamespace` is shared by two planes with different endpoint
        // keys — handled (registry + labeled fallback) in its own method.
        if matches!(edge_type, EdgeType::InNamespace) {
            return self
                .insert_in_namespace_edge(&session, from_id, to_id)
                .await;
        }

        // ── Registry fast path (issue #482) ──
        // When both endpoints were ingested THIS run, their NodeIds are
        // known and the edge is written via the direct session API: zero
        // GQL, no parse/plan/MATCH. Any miss (pre-existing node, unknown
        // key form) falls through to the original GQL statements below
        // verbatim — never an error, never a mis-link.
        if let Some((src_key, dst_key)) = registry_keys(&edge_type, from_id, to_id) {
            if let (Some(src), Some(dst)) = (self.lookup_node(&src_key), self.lookup_node(&dst_key))
            {
                crate::perf::bump_registry_edge_hit();
                self.create_edge_direct(
                    src,
                    dst,
                    edge_label(&edge_type),
                    &edge_props_pairs(props),
                )?;
                return Ok(());
            }
            crate::perf::bump_registry_edge_miss();
        }

        // ── Hot-path edge types: parameterized queries for plan caching ──
        match &edge_type {
            EdgeType::HasProperty => {
                let (prop_name, schema_title) = split_compound_id(to_id, "HasProperty")?;
                let gql = "MATCH (a:Schema {title: $from_title}), (b:Property {name: $prop_name, _schema_title: $prop_schema_title}) \
                     INSERT (a)-[:HasProperty]->(b)";
                let params = HashMap::from([
                    ("from_title".into(), grafeo::Value::String(from_id.into())),
                    ("prop_name".into(), grafeo::Value::String(prop_name.into())),
                    (
                        "prop_schema_title".into(),
                        grafeo::Value::String(schema_title.into()),
                    ),
                ]);
                session.execute_with_params(gql, params).map_err(|e| {
                    GraphError::Ingest(format!("ingest_edge HasProperty failed: {e}"))
                })?;
                return Ok(());
            }
            EdgeType::ReferencesSchema => {
                let (prop_name, schema_title) = split_compound_id(from_id, "ReferencesSchema")?;
                let gql = "MATCH (a:Property {name: $prop_name, _schema_title: $prop_schema_title}), (b:Schema {schema_id: $schema_id}) \
                     INSERT (a)-[:ReferencesSchema]->(b)";
                let params = HashMap::from([
                    ("prop_name".into(), grafeo::Value::String(prop_name.into())),
                    (
                        "prop_schema_title".into(),
                        grafeo::Value::String(schema_title.into()),
                    ),
                    ("schema_id".into(), grafeo::Value::String(to_id.into())),
                ]);
                session.execute_with_params(gql, params).map_err(|e| {
                    GraphError::Ingest(format!("ingest_edge ReferencesSchema failed: {e}"))
                })?;
                return Ok(());
            }
            EdgeType::ItemsOf => {
                let (prop_name, schema_title) = split_compound_id(from_id, "ItemsOf")?;
                let gql = "MATCH (a:Property {name: $prop_name, _schema_title: $prop_schema_title}), (b:Schema {schema_id: $schema_id}) \
                     INSERT (a)-[:ItemsOf]->(b)";
                let params = HashMap::from([
                    ("prop_name".into(), grafeo::Value::String(prop_name.into())),
                    (
                        "prop_schema_title".into(),
                        grafeo::Value::String(schema_title.into()),
                    ),
                    ("schema_id".into(), grafeo::Value::String(to_id.into())),
                ]);
                session
                    .execute_with_params(gql, params)
                    .map_err(|e| GraphError::Ingest(format!("ingest_edge ItemsOf failed: {e}")))?;
                return Ok(());
            }
            EdgeType::ExtendsSchema | EdgeType::DependsOn => {
                let label_str = match &edge_type {
                    EdgeType::ExtendsSchema => "ExtendsSchema",
                    _ => "DependsOn",
                };
                let props_str = build_edge_props_string(props);
                let gql = format!(
                    "MATCH (a:Schema {{title: $from_title}}), (b:Schema {{title: $to_title}}) \
                     INSERT (a)-[:{}{}]->(b)",
                    label_str, props_str,
                );
                let params = HashMap::from([
                    ("from_title".into(), grafeo::Value::String(from_id.into())),
                    ("to_title".into(), grafeo::Value::String(to_id.into())),
                ]);
                session.execute_with_params(&gql, params).map_err(|e| {
                    GraphError::Ingest(format!("ingest_edge {label_str} failed: {e}"))
                })?;
                return Ok(());
            }
            _ => {}
        }

        // ── Remaining edge types: format!()-based (lower frequency) ──
        let label = edge_label(&edge_type);

        let match_clause = match &edge_type {
            EdgeType::HasEnumValue => {
                let (value, codelist) = split_compound_id(to_id, "HasEnumValue")?;
                format!(
                    "MATCH (a:CodeList {{name: '{}'}}), (b:EnumValue {{value: '{}', _codelist_name: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(value),
                    escape_gql(codelist),
                )
            }
            EdgeType::HasCondition => {
                format!(
                    "MATCH (a:Schema {{title: '{}'}}), (b:Condition {{name: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }
            EdgeType::UsesCodeList => {
                let (prop_name, schema_title) = split_compound_id(from_id, "UsesCodeList")?;
                format!(
                    "MATCH (a:Property {{name: '{}', _schema_title: '{}'}}), (b:CodeList {{name: '{}'}})",
                    escape_gql(prop_name),
                    escape_gql(schema_title),
                    escape_gql(to_id),
                )
            }
            EdgeType::ExpandsTo => {
                let (prop_name, schema_title) = split_compound_id(from_id, "ExpandsTo")?;
                let (suffix, wrapper_schema) = split_compound_id(to_id, "ExpandsTo(target)")?;
                format!(
                    "MATCH (a:Property {{name: '{}', _schema_title: '{}'}}), \
                     (b:CompositeColumn {{suffix: '{}', wrapper_schema: '{}'}})",
                    escape_gql(prop_name),
                    escape_gql(schema_title),
                    escape_gql(suffix),
                    escape_gql(wrapper_schema),
                )
            }
            EdgeType::CollapsesTo => {
                format!(
                    "MATCH (a:Schema {{title: '{}'}}), (b:CompositeRange {{pg_column_name: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }
            EdgeType::ConsumesField => {
                let (prop_name, schema_title) = split_compound_id(to_id, "ConsumesField")?;
                format!(
                    "MATCH (a:CompositeRange {{pg_column_name: '{}'}}), (b:Property {{name: '{}', _schema_title: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(prop_name),
                    escape_gql(schema_title),
                )
            }
            EdgeType::ContainsDef => {
                format!(
                    "MATCH (a:Schema {{title: '{}'}}), (b:Schema {{title: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }
            EdgeType::RequiresExtension => {
                format!(
                    "MATCH (a:Schema {{title: '{}'}}), (b:Extension {{name: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }
            EdgeType::InDomain => {
                format!(
                    "MATCH (a:Schema {{title: '{}'}}), (b:Domain {{name: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }
            EdgeType::DomainDepends => {
                format!(
                    "MATCH (a:Domain {{name: '{}'}}), (b:Domain {{name: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }

            // IFML edge types — simple MATCH on node label + name
            EdgeType::ContainsViewContainer => {
                format!(
                    "MATCH (a:ViewContainer {{name: '{}'}}), (b:ViewContainer {{name: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(strip_ifml_prefix(to_id)),
                )
            }
            EdgeType::ContainsViewComponent => {
                format!(
                    "MATCH (a:ViewContainer {{name: '{}'}}), (b:ViewComponent {{name: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(strip_ifml_prefix(to_id)),
                )
            }
            EdgeType::HasEvent => {
                format!(
                    "MATCH (a {{name: '{}'}}), (b:Event {{name: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(strip_ifml_prefix(to_id)),
                )
            }
            EdgeType::NavigationFlow => {
                format!(
                    "MATCH (a:Event {{name: '{}'}}), (b:ViewContainer {{name: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(strip_ifml_prefix(to_id)),
                )
            }
            EdgeType::DataFlow => {
                format!(
                    "MATCH (a {{name: '{}'}}), (b {{name: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(strip_ifml_prefix(to_id)),
                )
            }
            EdgeType::HasParameter => {
                format!(
                    "MATCH (a {{name: '{}'}}), (b:ParameterDefinition {{name: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(strip_ifml_prefix(to_id)),
                )
            }
            EdgeType::ParameterBindingGroup => {
                format!(
                    "MATCH (a {{name: '{}'}}), (b {{name: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(strip_ifml_prefix(to_id)),
                )
            }
            EdgeType::ParameterBinding => {
                format!(
                    "MATCH (a {{name: '{}'}}), (b {{name: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(strip_ifml_prefix(to_id)),
                )
            }
            EdgeType::HasDataBinding => {
                format!(
                    "MATCH (a:ViewComponent {{name: '{}'}}), (b:DataBinding {{name: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(strip_ifml_prefix(to_id)),
                )
            }
            EdgeType::BindsToEntity => {
                format!(
                    "MATCH (a:DataBinding {{name: '{}'}}), (b:Schema {{title: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(to_id),
                )
            }
            EdgeType::BindsToProperty => {
                let (prop_name, schema_title) = split_compound_id(to_id, "BindsToProperty")?;
                format!(
                    "MATCH (a:ViewComponent {{name: '{}'}}), \
                     (b:Property {{name: '{}', _schema_title: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(prop_name),
                    escape_gql(schema_title),
                )
            }
            EdgeType::BindsToOperation => {
                format!(
                    "MATCH (a:ViewComponent {{name: '{}'}}), (b:ApiOperation {{name: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(strip_ifml_prefix(to_id)),
                )
            }
            EdgeType::TriggersAction => {
                format!(
                    "MATCH (a:Event {{name: '{}'}}), (b:ActionNode {{name: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(strip_ifml_prefix(to_id)),
                )
            }
            EdgeType::ActionEvent => {
                format!(
                    "MATCH (a:ActionNode {{name: '{}'}}), (b:Event {{name: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(strip_ifml_prefix(to_id)),
                )
            }
            EdgeType::HasModuleDefinition => {
                format!(
                    "MATCH (a:ViewContainer {{name: '{}'}}), (b:ModuleDefinition {{name: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(strip_ifml_prefix(to_id)),
                )
            }
            EdgeType::HasViewComponentPart => {
                format!(
                    "MATCH (a:ViewComponent {{name: '{}'}}), (b:ViewComponent {{name: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(strip_ifml_prefix(to_id)),
                )
            }
            EdgeType::HasConditionalExpr => {
                format!(
                    "MATCH (a {{name: '{}'}}), (b {{name: '{}'}})",
                    escape_gql(strip_ifml_prefix(from_id)),
                    escape_gql(strip_ifml_prefix(to_id)),
                )
            }
            // API metamodel edges. Name-bearing nodes (ApiResource,
            // ApiOperation, Pipeline, Permission, ErrorDefinition) store plain
            // names; Interaction/HttpEndpoint store the full prefixed id as
            // `name`; Schema targets match by title.
            EdgeType::ExposesResource => {
                format!(
                    "MATCH (a:ApiResource {{name: '{}'}}), (b:ApiResource {{name: '{}'}})",
                    escape_gql(strip_api_prefix(from_id)),
                    escape_gql(strip_api_prefix(to_id)),
                )
            }
            EdgeType::BindsToSchema => {
                format!(
                    "MATCH (a:ApiResource {{name: '{}'}}), (b:Schema {{title: '{}'}})",
                    escape_gql(strip_api_prefix(from_id)),
                    escape_gql(to_id),
                )
            }
            EdgeType::HasOperation => {
                format!(
                    "MATCH (a:ApiResource {{name: '{}'}}), (b:ApiOperation {{name: '{}'}})",
                    escape_gql(strip_api_prefix(from_id)),
                    escape_gql(strip_api_prefix(to_id)),
                )
            }
            EdgeType::InputBoundTo | EdgeType::OutputBoundTo => {
                format!(
                    "MATCH (a:ApiOperation {{name: '{}'}}), (b:Schema {{title: '{}'}})",
                    escape_gql(strip_api_prefix(from_id)),
                    escape_gql(to_id),
                )
            }
            EdgeType::CanReturnError => {
                format!(
                    "MATCH (a:ApiOperation {{name: '{}'}}), (b:ErrorDefinition {{code: '{}'}})",
                    escape_gql(strip_api_prefix(from_id)),
                    escape_gql(strip_api_prefix(to_id)),
                )
            }
            EdgeType::RequiresPermission => {
                format!(
                    "MATCH (a:ApiOperation {{name: '{}'}}), (b:Permission {{name: '{}'}})",
                    escape_gql(strip_api_prefix(from_id)),
                    escape_gql(strip_api_prefix(to_id)),
                )
            }
            EdgeType::HasInteraction => {
                format!(
                    "MATCH (a:ApiOperation {{name: '{}'}}), (b:Interaction {{name: '{}'}})",
                    escape_gql(strip_api_prefix(from_id)),
                    escape_gql(to_id),
                )
            }
            EdgeType::BindsHttpEndpoint => {
                format!(
                    "MATCH (a:Interaction {{name: '{}'}}), (b:HttpEndpoint {{name: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }
            EdgeType::UsesPipeline => {
                format!(
                    "MATCH (a:HttpEndpoint {{name: '{}'}}), (b:Pipeline {{name: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(strip_api_prefix(to_id)),
                )
            }
            // Namespace plane (issue #267): all three edges connect
            // Namespace nodes matched by fqn.
            EdgeType::NamespaceParent | EdgeType::NamespaceImports | EdgeType::NamespaceDepends => {
                format!(
                    "MATCH (a:Namespace {{fqn: '{}'}}), (b:Namespace {{fqn: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }
            // DDD design plane (issue #449). Secondary path — `ingest_ddd_model`
            // wires these edges directly. Ids are compound natural keys:
            // `app::module`, `app::class`, `app::repository`.
            EdgeType::DddHasModule => {
                let (app, module) = split_compound_id(to_id, "DddHasModule")?;
                format!(
                    "MATCH (a:DddApplication {{name: '{}'}}), \
                     (b:DddModule {{application: '{}', name: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(app),
                    escape_gql(module),
                )
            }
            EdgeType::DddHasDesign | EdgeType::DddHasService | EdgeType::DddHasSearch => {
                let (app, module) = split_compound_id(from_id, "DddHas{Design,Service,Search}")?;
                format!(
                    "MATCH (a:DddModule {{application: '{}', name: '{}'}}), \
                     (b:DddDesign {{application: '{}', module: '{}', class: '{}'}})",
                    escape_gql(app),
                    escape_gql(module),
                    escape_gql(app),
                    escape_gql(module),
                    escape_gql(to_id),
                )
            }
            EdgeType::DddHasRepository => {
                let (app, class) = split_compound_id(from_id, "DddHasRepository")?;
                format!(
                    "MATCH (a:DddDesign {{application: '{}', class: '{}'}}), \
                     (b:DddRepository {{application: '{}', name: '{}'}})",
                    escape_gql(app),
                    escape_gql(class),
                    escape_gql(app),
                    escape_gql(to_id),
                )
            }
            EdgeType::DddBindsClass => {
                let (app, class) = split_compound_id(from_id, "DddBindsClass")?;
                format!(
                    "MATCH (a:DddDesign {{application: '{}', class: '{}'}}), \
                     (b:Schema {{title: '{}'}})",
                    escape_gql(app),
                    escape_gql(class),
                    escape_gql(to_id),
                )
            }
            EdgeType::DddHasOperation => {
                let (app, repository) = split_compound_id(from_id, "DddHasOperation")?;
                format!(
                    "MATCH (a:DddRepository {{application: '{}', name: '{}'}}), \
                     (b:DddRepositoryOperation {{application: '{}', repository_name: '{}', name: '{}'}})",
                    escape_gql(app),
                    escape_gql(repository),
                    escape_gql(app),
                    escape_gql(repository),
                    escape_gql(to_id),
                )
            }
            EdgeType::ProjectsToLexicon => {
                format!(
                    "MATCH (a:Schema {{title: '{}'}}), (b:Lexicon {{nsid: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }
            EdgeType::DefinesCollection => {
                format!(
                    "MATCH (a:Lexicon {{nsid: '{}'}}), (b:Collection {{nsid: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }
            EdgeType::LexiconReferences => {
                format!(
                    "MATCH (a:Lexicon {{nsid: '{}'}}), (b:Lexicon {{nsid: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }
            EdgeType::StoredInRepository => {
                format!(
                    "MATCH (a:Collection {{nsid: '{}'}}), (b:Repository {{did: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }
            EdgeType::HasPolicy
            | EdgeType::PolicyAppliesTo
            | EdgeType::HasRelationship
            | EdgeType::RelationshipSource
            | EdgeType::RelationshipTarget
            | EdgeType::PolicyOnRelationship
            | EdgeType::TenantOwns
            | EdgeType::HasMembership
            | EdgeType::MembershipInTenant
            | EdgeType::HasRole => {
                format!(
                    "MATCH (a {{name: '{}'}}), (b {{name: '{}'}})",
                    escape_gql(strip_api_prefix(from_id)),
                    escape_gql(strip_api_prefix(to_id)),
                )
            }
            EdgeType::Grant => {
                format!(
                    "MATCH (a:Actor {{name: '{}'}}), (b:Capability {{name: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }
            // Computation plane (issue #263): functions match by name.
            EdgeType::FunctionExtends => {
                format!(
                    "MATCH (a:Function {{name: '{}'}}), (b:Function {{name: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }
            // Rule plane (issue #264): rules match by name; the input
            // schema and the rule-source class data match by title.
            EdgeType::RuleAppliesTo => {
                format!(
                    "MATCH (a:Rule {{name: '{}'}}), (b:Schema {{title: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }
            EdgeType::RuleReference => {
                format!(
                    "MATCH (a:Schema {{title: '{}'}}), (b:Rule {{name: '{}'}})",
                    escape_gql(from_id),
                    escape_gql(to_id),
                )
            }
            // These edge types are handled by the early-returns above but
            // must be listed to satisfy the exhaustive match. They are
            // unreachable.
            EdgeType::InNamespace
            | EdgeType::HasProperty
            | EdgeType::ReferencesSchema
            | EdgeType::ItemsOf
            | EdgeType::ExtendsSchema
            | EdgeType::DependsOn
            | EdgeType::RegulatoryReference
            | EdgeType::HasRuleSource
            | EdgeType::CorpusInBody
            | EdgeType::DerivesFrom => unreachable!(),
        };

        let props_str = build_edge_props_string(props);
        let write = if matches!(edge_type, EdgeType::HasParameter) {
            "MERGE"
        } else {
            "INSERT"
        };
        let gql = format!("{match_clause} {write} (a)-[:{label}{props_str}]->(b)");
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_edge {label} failed: {e}")))?;
        Ok(())
    }

    /// `InNamespace` (issue #482) — ONE edge type shared by TWO planes:
    /// the namespace plane links Schema (by indexed `schema_id`) →
    /// Namespace (by indexed `fqn`), the AT-Protocol plane links Lexicon
    /// (by indexed `nsid`) → AtprotoNamespace (by indexed `authority`).
    ///
    /// Registry pairs first (zero GQL when both endpoints were ingested
    /// this run); otherwise per-plane LABELED equality statements selected
    /// by one indexed existence check. This replaces the previous single
    /// UNLABELED, OR-shaped statement (`WHERE (a.nsid = $x OR a.schema_id
    /// = $x) AND (b.authority = $y OR b.fqn = $y)`): grafeo's planner
    /// never absorbs OR (`collect_equality_conditions` handles And/Eq
    /// only) and unlabeled matches scan `node_ids()` = O(V) — the whole
    /// -store scan behind the 30s ingest timeout (git 160c687d).
    async fn insert_in_namespace_edge(
        &self,
        session: &CountingSession,
        from_id: &str,
        to_id: &str,
    ) -> Result<(), GraphError> {
        // Namespace plane: Schema(schema_id) → Namespace(fqn).
        if let (Some(src), Some(dst)) = (
            self.lookup_node(&keys::schema_id(from_id)),
            self.lookup_node(&keys::namespace(to_id)),
        ) {
            crate::perf::bump_registry_edge_hit();
            return self.create_edge_direct(src, dst, "InNamespace", &[]);
        }
        // AT-Protocol plane: Lexicon(nsid) → AtprotoNamespace(authority).
        if let (Some(src), Some(dst)) = (
            self.lookup_node(&keys::lexicon(from_id)),
            self.lookup_node(&keys::atproto_namespace(to_id)),
        ) {
            crate::perf::bump_registry_edge_hit();
            return self.create_edge_direct(src, dst, "InNamespace", &[]);
        }
        crate::perf::bump_registry_edge_miss();

        // Fallback: detect the plane with one indexed labeled seek, then
        // issue the matching labeled statement. A MATCH with no rows
        // inserts nothing — the advisory semantics are unchanged.
        let detect = session
            .execute_with_params(
                "MATCH (l:Lexicon {nsid: $from}) RETURN 1 AS hit LIMIT 1",
                HashMap::from([("from".to_string(), grafeo::Value::String(from_id.into()))]),
            )
            .map_err(|e| {
                GraphError::Ingest(format!("ingest_edge InNamespace plane detect failed: {e}"))
            })?;
        if !detect.rows().is_empty() {
            let gql = "MATCH (a:Lexicon {nsid: $from}), (b:AtprotoNamespace {authority: $to}) \
                    INSERT (a)-[:InNamespace]->(b)";
            session
                .execute_with_params(
                    gql,
                    HashMap::from([
                        ("from".to_string(), grafeo::Value::String(from_id.into())),
                        ("to".to_string(), grafeo::Value::String(to_id.into())),
                    ]),
                )
                .map_err(|e| {
                    GraphError::Ingest(format!("ingest_edge InNamespace (atproto) failed: {e}"))
                })?;
        } else {
            let gql = "MATCH (a:Schema {schema_id: $from}), (b:Namespace {fqn: $to}) \
                    INSERT (a)-[:InNamespace]->(b)";
            session
                .execute_with_params(
                    gql,
                    HashMap::from([
                        ("from".to_string(), grafeo::Value::String(from_id.into())),
                        ("to".to_string(), grafeo::Value::String(to_id.into())),
                    ]),
                )
                .map_err(|e| {
                    GraphError::Ingest(format!("ingest_edge InNamespace (namespace) failed: {e}"))
                })?;
        }
        Ok(())
    }
}

/// The GQL edge label for an `EdgeType` (verbatim from the former inline
/// match in `insert_edge`).
fn edge_label(edge_type: &EdgeType) -> &'static str {
    match edge_type {
        EdgeType::HasProperty => "HasProperty",
        EdgeType::ReferencesSchema => "ReferencesSchema",
        EdgeType::ItemsOf => "ItemsOf",
        EdgeType::ExtendsSchema => "ExtendsSchema",
        EdgeType::DependsOn => "DependsOn",
        EdgeType::HasEnumValue => "HasEnumValue",
        EdgeType::UsesCodeList => "UsesCodeList",
        EdgeType::ExpandsTo => "ExpandsTo",
        EdgeType::CollapsesTo => "CollapsesTo",
        EdgeType::ConsumesField => "ConsumesField",
        EdgeType::ContainsDef => "ContainsDef",
        EdgeType::RequiresExtension => "RequiresExtension",
        EdgeType::InDomain => "InDomain",
        EdgeType::DomainDepends => "DomainDepends",
        EdgeType::ContainsViewContainer => "ContainsViewContainer",
        EdgeType::ContainsViewComponent => "ContainsViewComponent",
        EdgeType::HasEvent => "HasEvent",
        EdgeType::NavigationFlow => "NavigationFlow",
        EdgeType::DataFlow => "DataFlow",
        EdgeType::HasParameter => "HasParameter",
        EdgeType::ParameterBindingGroup => "ParameterBindingGroup",
        EdgeType::ParameterBinding => "ParameterBinding",
        EdgeType::HasDataBinding => "HasDataBinding",
        EdgeType::BindsToEntity => "BindsToEntity",
        EdgeType::BindsToProperty => "BindsToProperty",
        EdgeType::BindsToOperation => "BindsToOperation",
        EdgeType::TriggersAction => "TriggersAction",
        EdgeType::ActionEvent => "ActionEvent",
        EdgeType::HasModuleDefinition => "HasModuleDefinition",
        EdgeType::HasViewComponentPart => "HasViewComponentPart",
        EdgeType::HasConditionalExpr => "HasConditionalExpr",
        EdgeType::HasCondition => "HasCondition",
        EdgeType::InNamespace => "InNamespace",
        EdgeType::ProjectsToLexicon => "ProjectsToLexicon",
        EdgeType::DefinesCollection => "DefinesCollection",
        EdgeType::LexiconReferences => "LexiconReferences",
        EdgeType::StoredInRepository => "StoredInRepository",
        EdgeType::ExposesResource => "ExposesResource",
        EdgeType::BindsToSchema => "BindsToSchema",
        EdgeType::HasOperation => "HasOperation",
        EdgeType::InputBoundTo => "InputBoundTo",
        EdgeType::OutputBoundTo => "OutputBoundTo",
        EdgeType::CanReturnError => "CanReturnError",
        EdgeType::RequiresPermission => "RequiresPermission",
        EdgeType::HasInteraction => "HasInteraction",
        EdgeType::BindsHttpEndpoint => "BindsHttpEndpoint",
        EdgeType::UsesPipeline => "UsesPipeline",
        EdgeType::HasPolicy => "HasPolicy",
        EdgeType::PolicyAppliesTo => "PolicyAppliesTo",
        EdgeType::HasRelationship => "HasRelationship",
        EdgeType::RelationshipSource => "RelationshipSource",
        EdgeType::RelationshipTarget => "RelationshipTarget",
        EdgeType::PolicyOnRelationship => "PolicyOnRelationship",
        EdgeType::TenantOwns => "TenantOwns",
        EdgeType::HasMembership => "HasMembership",
        EdgeType::MembershipInTenant => "MembershipInTenant",
        EdgeType::HasRole => "HasRole",
        EdgeType::Grant => "Grant",
        EdgeType::RegulatoryReference => "RegulatoryReference",
        EdgeType::HasRuleSource => "HasRuleSource",
        EdgeType::CorpusInBody => "CorpusInBody",
        EdgeType::DerivesFrom => "DerivesFrom",
        EdgeType::FunctionExtends => "FunctionExtends",
        EdgeType::RuleAppliesTo => "RuleAppliesTo",
        EdgeType::RuleReference => "RuleReference",
        EdgeType::NamespaceParent => "NamespaceParent",
        EdgeType::NamespaceImports => "NamespaceImports",
        EdgeType::NamespaceDepends => "NamespaceDepends",
        EdgeType::DddHasModule => "DddHasModule",
        EdgeType::DddHasDesign => "DddHasDesign",
        EdgeType::DddHasRepository => "DddHasRepository",
        EdgeType::DddHasService => "DddHasService",
        EdgeType::DddHasSearch => "DddHasSearch",
        EdgeType::DddHasOperation => "DddHasOperation",
        EdgeType::DddBindsClass => "DddBindsClass",
    }
}

/// Registry keys for an edge's endpoints, mirroring the endpoint MATCH
/// forms above EXACTLY (issue #482). `None` = the edge type's MATCH form
/// is not registry-tracked — UNLABELED matches (`(a {name})`, which the
/// registry cannot disambiguate), nodes written by idempotent MERGE
/// (ViewContainer/CompositeColumn/Extension — no NodeId captured), the
/// Domain plane (no Domain ingest exists), and the DDD plane (its edges
/// are wired inside `insert_ddd_model`). A `None` — or a key that simply
/// misses — takes the GQL fallback, so a mismatch can never mis-link.
fn registry_keys(edge_type: &EdgeType, from_id: &str, to_id: &str) -> Option<(String, String)> {
    let keys = match edge_type {
        // ── Schema/property plane (hot path) ──
        // Compound ids are `"{prop_name}::{schema_title}"` /
        // `"{value}::{codelist}"` / `"{suffix}::{wrapper}"` verbatim — the
        // node side registers under the same compound string.
        EdgeType::HasProperty => (keys::schema_title(from_id), keys::property(to_id)),
        EdgeType::ReferencesSchema | EdgeType::ItemsOf => {
            (keys::property(from_id), keys::schema_id(to_id))
        }
        EdgeType::ExtendsSchema | EdgeType::DependsOn => {
            (keys::schema_title(from_id), keys::schema_title(to_id))
        }
        EdgeType::HasEnumValue => (keys::codelist(from_id), keys::enum_value(to_id)),
        EdgeType::UsesCodeList => (keys::property(from_id), keys::codelist(to_id)),
        EdgeType::CollapsesTo => (keys::schema_title(from_id), keys::composite_range(to_id)),
        EdgeType::ConsumesField => (keys::composite_range(from_id), keys::property(to_id)),
        EdgeType::ContainsDef => (keys::schema_title(from_id), keys::schema_title(to_id)),

        // ── Composite/range plane ──
        // (ExpandsTo targets a MERGE-written CompositeColumn: fallback.)

        // ── IFML plane (labeled arms only) ──
        EdgeType::BindsToEntity => (
            keys::data_binding(strip_ifml_prefix(from_id)),
            keys::schema_title(to_id),
        ),
        EdgeType::TriggersAction => (
            keys::event(strip_ifml_prefix(from_id)),
            keys::action(strip_ifml_prefix(to_id)),
        ),
        EdgeType::ActionEvent => (
            keys::action(strip_ifml_prefix(from_id)),
            keys::event(strip_ifml_prefix(to_id)),
        ),
        EdgeType::HasViewComponentPart => (
            keys::view_component(strip_ifml_prefix(from_id)),
            keys::view_component(strip_ifml_prefix(to_id)),
        ),

        // ── API metamodel (Interaction/HttpEndpoint ids are NOT stripped;
        // everything else strips its `xx:` prefix, mirroring the MATCH) ──
        EdgeType::ExposesResource => (
            keys::api_resource(strip_api_prefix(from_id)),
            keys::api_resource(strip_api_prefix(to_id)),
        ),
        EdgeType::BindsToSchema => (
            keys::api_resource(strip_api_prefix(from_id)),
            keys::schema_title(to_id),
        ),
        EdgeType::HasOperation => (
            keys::api_resource(strip_api_prefix(from_id)),
            keys::api_operation(strip_api_prefix(to_id)),
        ),
        EdgeType::InputBoundTo | EdgeType::OutputBoundTo => (
            keys::api_operation(strip_api_prefix(from_id)),
            keys::schema_title(to_id),
        ),
        EdgeType::CanReturnError => (
            keys::api_operation(strip_api_prefix(from_id)),
            keys::error_definition(strip_api_prefix(to_id)),
        ),
        EdgeType::RequiresPermission => (
            keys::api_operation(strip_api_prefix(from_id)),
            keys::permission(strip_api_prefix(to_id)),
        ),
        EdgeType::HasInteraction => (
            keys::api_operation(strip_api_prefix(from_id)),
            keys::interaction(to_id),
        ),
        EdgeType::BindsHttpEndpoint => (keys::interaction(from_id), keys::http_endpoint(to_id)),
        EdgeType::UsesPipeline => (
            keys::http_endpoint(from_id),
            keys::pipeline(strip_api_prefix(to_id)),
        ),

        // ── Namespace plane (issue #267; InNamespace is dual-plane and
        // handled in `insert_in_namespace_edge`) ──
        EdgeType::NamespaceParent | EdgeType::NamespaceImports | EdgeType::NamespaceDepends => {
            (keys::namespace(from_id), keys::namespace(to_id))
        }

        // ── AT Protocol ──
        EdgeType::ProjectsToLexicon => (keys::schema_title(from_id), keys::lexicon(to_id)),
        EdgeType::DefinesCollection => (keys::lexicon(from_id), keys::collection(to_id)),
        EdgeType::LexiconReferences => (keys::lexicon(from_id), keys::lexicon(to_id)),
        EdgeType::StoredInRepository => (keys::collection(from_id), keys::repository(to_id)),

        // ── Authorization / computation / rule planes ──
        EdgeType::Grant => (keys::actor(from_id), keys::capability(to_id)),
        EdgeType::FunctionExtends => (keys::function(from_id), keys::function(to_id)),
        EdgeType::RuleAppliesTo => (keys::rule(from_id), keys::schema_title(to_id)),
        EdgeType::RuleReference => (keys::schema_title(from_id), keys::rule(to_id)),

        // Everything else: untracked (unlabeled MATCHes, MERGE-written
        // targets, Domain/DDD planes, regulatory encoding) → GQL fallback.
        _ => return None,
    };
    Some(keys)
}

/// Edge properties as `(name, Value)` pairs for the direct edge path —
/// the EXACT property set (same fields, same names, same order of checks)
/// `build_edge_props_string` emits on the GQL path.
fn edge_props_pairs(props: Option<&EdgeProperties>) -> Vec<(&'static str, grafeo::Value)> {
    let Some(p) = props else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(v) = p.sort_order {
        out.push(("sort_order", grafeo::Value::Int64(i64::from(v))));
    }
    if let Some(v) = &p.ref_path {
        out.push(("ref_path", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = &p.resolved_classification {
        out.push((
            "resolved_classification",
            grafeo::Value::String(v.as_str().into()),
        ));
    }
    if let Some(v) = &p.composition_type {
        out.push(("composition_type", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = &p.dependency_type {
        out.push(("dependency_type", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = &p.render_as {
        out.push(("render_as", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = &p.role {
        out.push(("role", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = &p.def_name {
        out.push(("def_name", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = &p.target_param_binding {
        out.push((
            "target_param_binding",
            grafeo::Value::String(v.as_str().into()),
        ));
    }
    if let Some(v) = &p.source_param {
        out.push(("source_param", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = &p.event_type {
        out.push(("event_type", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = &p.outcome {
        out.push(("outcome", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = &p.component_type {
        out.push(("component_type", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = &p.direction {
        out.push(("direction", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = &p.expression {
        out.push(("expression", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = &p.effect {
        out.push(("effect", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = &p.when_expr {
        out.push(("when_expr", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = &p.obligations {
        out.push(("obligations", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = &p.rule_source {
        out.push(("rule_source", grafeo::Value::String(v.as_str().into())));
    }
    if let Some(v) = p.import_wildcard {
        out.push(("wildcard", grafeo::Value::Bool(v)));
    }
    if let Some(v) = &p.import_alias {
        out.push(("alias", grafeo::Value::String(v.as_str().into())));
    }
    out
}
