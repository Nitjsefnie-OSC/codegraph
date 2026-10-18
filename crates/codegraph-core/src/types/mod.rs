mod annotation;
mod api;
mod atproto;
mod authorization;
mod codelist;
mod composite;
mod composition;
mod condition;
mod ddd;
mod deploy;
mod discovery;
mod edge;
mod events;
mod field_def;
mod function;
mod ifml;
mod incompleteness;
mod mox;
mod namespace;
mod persistence;
mod policy;
mod property;
mod regulatory;
mod relationship;
mod rule;
mod schema;
mod security;
mod stats;

pub use annotation::{Annotation, AnnotationArg};
pub use api::{
    ApiOperationNode, ApiResourceNode, ErrorDefinitionNode, HttpEndpointNode, InteractionNode,
    PermissionNode, PipelineNode,
};
// Naming-collision resolution (issue #267): the AT-Protocol repo namespace
// node is renamed `AtprotoNamespaceNode` (grafeo label `AtprotoNamespace`,
// trait methods `ingest_atproto_namespace`/`get_atproto_namespaces`) so the
// graph-wide namespace concept (#267) can own the canonical `NamespaceNode`
// name, `:Namespace` label, and `ingest_namespace`/`list_namespaces` trait
// methods. `EdgeType::InNamespace` stays shared by both features.
pub use atproto::{AtprotoNamespaceNode, CollectionNode, LexiconNode, RepositoryNode};
pub use authorization::{
    ActorNode, ActorPolicyModel, ActorPolicyNode, CapabilityNode, DelegationRecord, GrantEdge,
    NeverBothGroup, Permit, resolve_effective_permits,
};
pub use codelist::{CodeList, EnumValue};
pub use composite::{CompositeColumn, CompositeRange, StructuredSubField};
pub use composition::{ColumnInfo, CompositionNode, CompositionTree, FkDirection, FkTarget};
pub use condition::{ConditionKind, ConditionNode};
pub use ddd::{
    DddApplicationNode, DddDesignFlags, DddDesignNode, DddDocumentField, DddModelGraph,
    DddModuleNode, DddPagination, DddParam, DddRepositoryNode, DddRepositoryOperation,
    DddSearchField, DddSearchNode, DddServiceNode, DddServiceOperation,
};
pub use deploy::{
    DeployApplicationNode, DeployBindingNode, DeployComponentNode, DeployConnectionNode,
    DeployDeploymentNode, DeployMappingNode, DeployModelGraph, DeployPolicyNode, DeployProfileNode,
    DeploySettingNode, DeployValueNode,
};
pub use discovery::{DetectionSource, Extension, ParentCandidate};
pub use edge::{EdgeProperties, EdgeType};
pub use events::{EvtChannelNode, EvtEventField, EvtEventNode, EvtModelGraph, EvtSubscriptionNode};
pub use field_def::{
    FieldDefinition, codelist_enum_name_from_ref, ensure_id_suffix, resolve_field,
    resolve_fk_column_name,
};
pub use function::{
    FunctionAlias, FunctionDispatch, FunctionInput, FunctionNode, FunctionOperation,
    FunctionPostCondition, FunctionTransform, FunctionTransformKind,
};
pub use ifml::{
    ActionNode, DataBindingNode, DataBindingResolution, DataFlowData, EventNode,
    ModuleDefinitionNode, ModuleUseRecord, NavigationFlowData, NavigationFlowRecord,
    ParameterDefinitionNode, ViewComponentNode, ViewContainerNode, strip_ifml_prefix,
};
pub use incompleteness::{Incompleteness, IncompletenessReason};
pub use mox::{
    MoxDerivedFeatureNode, MoxDomainModel, MoxEntry, MoxFacet, MoxOperationNode, MoxPackageNode,
    MoxParam, MoxVocabularyNode,
};
pub use namespace::{
    NamespaceImport, NamespaceNode, derive_namespace_depends, descendants, disambiguate_schema_ids,
    namespace_module_path, namespace_module_rust, qualified_schema_id, topological_namespace_order,
    topological_order,
};
pub use persistence::{
    AuditEffect, AuditTimestampKind, AuditUserKind, PersistenceChildTable, PersistenceColumn,
    PersistenceColumnRole, PersistenceEntity, PersistenceEntityRelation, PersistencePolicies,
    RetentionEffect, RowSecurityEffect, SoftDeleteEffect, TenantIsolationEffect,
};
pub use policy::{
    AuditPolicy, DeletionPropagation, PolicyKind, PolicyNode, RetentionPolicy, RowOperation,
    RowSecurityPolicy, SoftDeleteMarker, SoftDeletePolicy, SoftDeleteVisibility,
    TenantIsolationPolicy, TenantPropagation, TenantStrategy,
};
pub use property::{PropertyNode, inject_codelist_properties, ref_target_candidate_title};
pub use regulatory::{
    RegulatoryEdgeKind, RegulatoryKind, RegulatoryNode, RegulatoryOwner, RegulatoryRefRecord,
};
pub use relationship::{
    Cardinality, ForeignKeySpec, Ownership, PropagationRule, PropagationTrigger, RelationshipNode,
};
pub use rule::{RuleKind, RuleNode, RuleRefRecord};
pub use schema::{Access, MOX_SOURCE, SchemaClassificationData, SchemaNode};
pub use security::{
    MembershipNode, MembershipStatus, Scope, ScopeKind, SecurityIdentityNode, TenantNode,
};
pub use stats::IngestStats;
