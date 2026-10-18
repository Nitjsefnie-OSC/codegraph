use std::collections::HashMap;

use codegraph_core::error::GraphError;
use codegraph_core::types::{EdgeProperties, EdgeType, strip_ifml_prefix};

use super::gql::CountingSession;
use super::gql::{
    build_edge_props_string, decode_regulatory_edge_ids, edge_kind_ref, escape_gql,
    regulatory_reference_gql, split_compound_id, strip_api_prefix,
};
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
        let label = match &edge_type {
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
        };

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
            // AT Protocol edges — nodes are matched by their natural keys
            // (Lexicon/Collection by nsid, AtprotoNamespace by authority,
            // Repository by did). `InNamespace` is SHARED with the
            // namespace plane (issue #267): the AT-Protocol projection
            // links Lexicon (nsid) → AtprotoNamespace (authority), while
            // the namespace plane links Schema (schema_id) → Namespace
            // (fqn). The WHERE form covers both callers without guessing.
            EdgeType::InNamespace => {
                format!(
                    "MATCH (a), (b) \
                     WHERE (a.nsid = '{from}' OR a.schema_id = '{from}') \
                     AND (b.authority = '{to}' OR b.fqn = '{to}')",
                    from = escape_gql(from_id),
                    to = escape_gql(to_id),
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
            // These edge types are handled by the early-return above but must
            // be listed to satisfy the exhaustive match. They are unreachable.
            EdgeType::HasProperty
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
}
