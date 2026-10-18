use std::path::PathBuf;

use async_trait::async_trait;
use codegraph_core::traits::GraphQuerier;

use crate::error::Result;
use codegraph_config::DomainConfig;

use super::db::dialect::DatabaseTarget;
use super::{GenerationEntry, ProjectConfig};

/// A target file to render from a template.
#[derive(Debug)]
pub struct RenderTarget {
    /// Template name (e.g. "db/table.tera")
    pub template: String,
    /// Output file path
    pub output: PathBuf,
    /// Whether to actually render this target (enables conditional generation)
    pub condition: bool,
}

/// Result of a single generator run — files to write.
#[derive(Debug)]
pub struct GeneratedFile {
    pub path: PathBuf,
    pub content: String,
}

/// Typed identity of an [`EntityGenerator`] implementation.
///
/// Each variant's [`EntityGeneratorKind::name`] returns the generator's
/// exact historical identity string (the one `profiles.toml` generator
/// lists and `BuildPlan` matching have always used), byte-for-byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityGeneratorKind {
    Ddl,
    SeaOrmEntity,
    CornucopiaQueries,
    CornucopiaRepo,
    Repository,
    Command,
    Query,
    Event,
    Dto,
    Codelist,
    Handler,
    WorkflowAction,
    MediaRoute,
    Test,
    UiPage,
    UiForm,
    CosmosEntityForm,
    UiStore,
    UiE2eTest,
    PlaywrightEntity,
    UiDescriptor,
    UiShell,
    LifecycleTrait,
    DomainTypesDto,
    DomainTypesQueryService,
    CliCommand,
    GrpcProto,
    GrpcService,
    Lexicon,
    AtprotoTypes,
    AtprotoClient,
    AtprotoXrpc,
}

impl EntityGeneratorKind {
    /// The generator's exact historical identity string.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ddl => "ddl",
            Self::SeaOrmEntity => "sea_orm_entity",
            Self::CornucopiaQueries => "cornucopia_queries",
            Self::CornucopiaRepo => "cornucopia_repo",
            Self::Repository => "repository",
            Self::Command => "command",
            Self::Query => "query",
            Self::Event => "event",
            Self::Dto => "dto",
            Self::Codelist => "codelist",
            Self::Handler => "handler",
            Self::WorkflowAction => "workflow_action",
            Self::MediaRoute => "media_route",
            Self::Test => "test",
            Self::UiPage => "ui-page",
            Self::UiForm => "ui-form",
            Self::CosmosEntityForm => "cosmos_entity_form",
            Self::UiStore => "ui-store",
            Self::UiE2eTest => "ui-e2e-test",
            Self::PlaywrightEntity => "playwright-entity",
            Self::UiDescriptor => "ui-descriptor",
            Self::UiShell => "ui-shell",
            Self::LifecycleTrait => "lifecycle_trait",
            Self::DomainTypesDto => "domain_types_dto",
            Self::DomainTypesQueryService => "domain_types_query_service",
            Self::CliCommand => "cli_command",
            Self::GrpcProto => "grpc_proto",
            Self::GrpcService => "grpc_service",
            Self::Lexicon => "lexicon",
            Self::AtprotoTypes => "atproto_types",
            Self::AtprotoClient => "atproto_client",
            Self::AtprotoXrpc => "atproto_xrpc",
        }
    }

    /// Maps an identity string back to its kind (exact historical spelling).
    pub fn from_name(n: &str) -> Option<Self> {
        Some(match n {
            "ddl" => Self::Ddl,
            "sea_orm_entity" => Self::SeaOrmEntity,
            "cornucopia_queries" => Self::CornucopiaQueries,
            "cornucopia_repo" => Self::CornucopiaRepo,
            "repository" => Self::Repository,
            "command" => Self::Command,
            "query" => Self::Query,
            "event" => Self::Event,
            "dto" => Self::Dto,
            "codelist" => Self::Codelist,
            "handler" => Self::Handler,
            "workflow_action" => Self::WorkflowAction,
            "media_route" => Self::MediaRoute,
            "test" => Self::Test,
            "ui-page" => Self::UiPage,
            "ui-form" => Self::UiForm,
            "cosmos_entity_form" => Self::CosmosEntityForm,
            "ui-store" => Self::UiStore,
            "ui-e2e-test" => Self::UiE2eTest,
            "playwright-entity" => Self::PlaywrightEntity,
            "ui-descriptor" => Self::UiDescriptor,
            "ui-shell" => Self::UiShell,
            "lifecycle_trait" => Self::LifecycleTrait,
            "domain_types_dto" => Self::DomainTypesDto,
            "domain_types_query_service" => Self::DomainTypesQueryService,
            "cli_command" => Self::CliCommand,
            "grpc_proto" => Self::GrpcProto,
            "grpc_service" => Self::GrpcService,
            "lexicon" => Self::Lexicon,
            "atproto_types" => Self::AtprotoTypes,
            "atproto_client" => Self::AtprotoClient,
            "atproto_xrpc" => Self::AtprotoXrpc,
            _ => return None,
        })
    }

    /// True for generators the API layer routes on (handler, workflow, media,
    /// test, UI, CLI, gRPC, playwright). Mirrors the former
    /// `is_api_entity_generator` string list exactly.
    pub fn is_api(self) -> bool {
        matches!(
            self,
            Self::Handler
                | Self::WorkflowAction
                | Self::MediaRoute
                | Self::Test
                | Self::UiPage
                | Self::UiForm
                | Self::UiStore
                | Self::UiE2eTest
                | Self::PlaywrightEntity
                | Self::UiDescriptor
                | Self::UiShell
                | Self::CliCommand
                | Self::GrpcProto
                | Self::GrpcService
        )
    }

    /// True for entity generators whose output is backend Rust source scoped
    /// to a single domain. Mirrors the former
    /// `is_worker_routed_entity_generator` string list exactly.
    pub fn is_worker_routed(self) -> bool {
        matches!(
            self,
            Self::SeaOrmEntity
                | Self::CornucopiaRepo
                | Self::Repository
                | Self::Command
                | Self::Query
                | Self::Event
                | Self::Dto
                | Self::Handler
                | Self::WorkflowAction
                | Self::MediaRoute
        )
    }
}

/// Typed identity of a [`DomainGenerator`] implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DomainGeneratorKind {
    Errors,
    Router,
    Links,
    ApiContract,
    UiDomainLayout,
    CliDomain,
    GrpcRouter,
    AtprotoAppview,
    AtprotoXrpcRouter,
    EmdashPlugin,
    ConditionValidations,
    RegulatoryReports,
    Functions,
    Rules,
    HurlContract,
}

impl DomainGeneratorKind {
    /// The generator's exact historical identity string.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Errors => "errors",
            Self::Router => "router",
            Self::Links => "links",
            Self::ApiContract => "api_contract",
            Self::UiDomainLayout => "ui-domain-layout",
            Self::CliDomain => "cli_domain",
            Self::GrpcRouter => "grpc_router",
            Self::AtprotoAppview => "atproto_appview",
            Self::AtprotoXrpcRouter => "atproto_xrpc_router",
            Self::EmdashPlugin => "emdash_plugin",
            Self::ConditionValidations => "condition_validations",
            Self::RegulatoryReports => "regulatory_reports",
            Self::Functions => "functions",
            Self::Rules => "rules",
            Self::HurlContract => "hurl_contract",
        }
    }

    /// Maps an identity string back to its kind (exact historical spelling).
    pub fn from_name(n: &str) -> Option<Self> {
        Some(match n {
            "errors" => Self::Errors,
            "router" => Self::Router,
            "links" => Self::Links,
            "api_contract" => Self::ApiContract,
            "ui-domain-layout" => Self::UiDomainLayout,
            "cli_domain" => Self::CliDomain,
            "grpc_router" => Self::GrpcRouter,
            "atproto_appview" => Self::AtprotoAppview,
            "atproto_xrpc_router" => Self::AtprotoXrpcRouter,
            "emdash_plugin" => Self::EmdashPlugin,
            "condition_validations" => Self::ConditionValidations,
            "regulatory_reports" => Self::RegulatoryReports,
            "functions" => Self::Functions,
            "rules" => Self::Rules,
            "hurl_contract" => Self::HurlContract,
            _ => return None,
        })
    }

    /// True for domain generators whose output belongs to a single domain's
    /// backend crate. Mirrors the former
    /// `is_worker_routed_domain_generator` string list exactly.
    pub fn is_worker_routed(self) -> bool {
        matches!(self, Self::Errors | Self::Router | Self::Links)
    }
}

/// Typed identity of a [`GlobalGenerator`] implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GlobalGeneratorKind {
    BasejumpSetup,
    PgmqSetup,
    LabelSetup,
    ServiceTables,
    PlatformSchema,
    PlatformGrants,
    WorkflowSeed,
    CornucopiaConfig,
    OpenApi,
    ApiContractIndex,
    WorkerScaffold,
    Scaffold,
    UiScaffold,
    UiTypes,
    UiCodelist,
    UiOrgchart,
    UiTestids,
    HookRegistry,
    DomainTypesScaffold,
    CliScaffold,
    ReportViews,
    SeedData,
    PlaywrightGlobal,
    WebhookDispatch,
    WebhookEndpointApi,
    EvtEvents,
    SeedProvision,
    GrpcScaffold,
    PolicyRls,
    LexiconScaffold,
    AtprotoClientScaffold,
    AtprotoIdentity,
    AtprotoGeneratedTypes,
    AtprotoXrpcMerge,
    FernConfig,
    EmdashPluginScaffold,
    Ops,
    IfmlSkeleton,
    IfmlRoute,
    IfmlNavigation,
    IfmlE2eTest,
    IntegrationTables,
    IntegrationConfig,
    IntegrationDispatch,
    IntegrationCatalog,
    PublicOperationsRls,
}

impl GlobalGeneratorKind {
    /// The generator's exact historical identity string.
    pub const fn name(self) -> &'static str {
        match self {
            Self::BasejumpSetup => "basejump_setup",
            Self::PgmqSetup => "pgmq_setup",
            Self::LabelSetup => "label_setup",
            Self::ServiceTables => "service_tables",
            Self::PlatformSchema => "platform_schema",
            Self::PlatformGrants => "platform_grants",
            Self::WorkflowSeed => "workflow_seed",
            Self::CornucopiaConfig => "cornucopia_config",
            Self::OpenApi => "openapi",
            Self::ApiContractIndex => "api_contract_index",
            Self::WorkerScaffold => "worker_scaffold",
            Self::Scaffold => "scaffold",
            Self::UiScaffold => "ui-scaffold",
            Self::UiTypes => "ui-types",
            Self::UiCodelist => "ui-codelist",
            Self::UiOrgchart => "ui-orgchart",
            Self::UiTestids => "ui-testids",
            Self::HookRegistry => "hook_registry",
            Self::DomainTypesScaffold => "domain_types_scaffold",
            Self::CliScaffold => "cli_scaffold",
            Self::ReportViews => "report_views",
            Self::SeedData => "seed_data",
            Self::PlaywrightGlobal => "playwright-global",
            Self::WebhookDispatch => "webhook_dispatch",
            Self::WebhookEndpointApi => "webhook_endpoint_api",
            Self::EvtEvents => "evt_events",
            Self::SeedProvision => "seed_provision",
            Self::GrpcScaffold => "grpc_scaffold",
            Self::PolicyRls => "policy_rls",
            Self::LexiconScaffold => "lexicon_scaffold",
            Self::AtprotoClientScaffold => "atproto_client_scaffold",
            Self::AtprotoIdentity => "atproto_identity",
            Self::AtprotoGeneratedTypes => "atproto_generated_types",
            Self::AtprotoXrpcMerge => "atproto_xrpc_merge",
            Self::FernConfig => "fern_config",
            Self::EmdashPluginScaffold => "emdash_plugin_scaffold",
            Self::Ops => "ops",
            Self::IfmlSkeleton => "ifml-skeleton",
            Self::IfmlRoute => "ifml-route",
            Self::IfmlNavigation => "ifml-navigation",
            Self::IfmlE2eTest => "ifml-e2e-test",
            Self::IntegrationTables => "integration_tables",
            Self::IntegrationConfig => "integration_config",
            Self::IntegrationDispatch => "integration_dispatch",
            Self::IntegrationCatalog => "integration_catalog",
            Self::PublicOperationsRls => "public_operations_rls",
        }
    }

    /// Maps an identity string back to its kind (exact historical spelling).
    pub fn from_name(n: &str) -> Option<Self> {
        Some(match n {
            "basejump_setup" => Self::BasejumpSetup,
            "pgmq_setup" => Self::PgmqSetup,
            "label_setup" => Self::LabelSetup,
            "service_tables" => Self::ServiceTables,
            "platform_schema" => Self::PlatformSchema,
            "platform_grants" => Self::PlatformGrants,
            "workflow_seed" => Self::WorkflowSeed,
            "cornucopia_config" => Self::CornucopiaConfig,
            "openapi" => Self::OpenApi,
            "api_contract_index" => Self::ApiContractIndex,
            "worker_scaffold" => Self::WorkerScaffold,
            "scaffold" => Self::Scaffold,
            "ui-scaffold" => Self::UiScaffold,
            "ui-types" => Self::UiTypes,
            "ui-testids" => Self::UiTestids,
            "ui-codelist" => Self::UiCodelist,
            "ui-orgchart" => Self::UiOrgchart,
            "hook_registry" => Self::HookRegistry,
            "domain_types_scaffold" => Self::DomainTypesScaffold,
            "cli_scaffold" => Self::CliScaffold,
            "report_views" => Self::ReportViews,
            "seed_data" => Self::SeedData,
            "playwright-global" => Self::PlaywrightGlobal,
            "webhook_dispatch" => Self::WebhookDispatch,
            "webhook_endpoint_api" => Self::WebhookEndpointApi,
            "evt_events" => Self::EvtEvents,
            "seed_provision" => Self::SeedProvision,
            "grpc_scaffold" => Self::GrpcScaffold,
            "policy_rls" => Self::PolicyRls,
            "lexicon_scaffold" => Self::LexiconScaffold,
            "atproto_client_scaffold" => Self::AtprotoClientScaffold,
            "atproto_identity" => Self::AtprotoIdentity,
            "atproto_generated_types" => Self::AtprotoGeneratedTypes,
            "atproto_xrpc_merge" => Self::AtprotoXrpcMerge,
            "fern_config" => Self::FernConfig,
            "emdash_plugin_scaffold" => Self::EmdashPluginScaffold,
            "ops" => Self::Ops,
            "ifml-skeleton" => Self::IfmlSkeleton,
            "ifml-route" => Self::IfmlRoute,
            "ifml-navigation" => Self::IfmlNavigation,
            "ifml-e2e-test" => Self::IfmlE2eTest,
            "integration_tables" => Self::IntegrationTables,
            "integration_config" => Self::IntegrationConfig,
            "integration_dispatch" => Self::IntegrationDispatch,
            "integration_catalog" => Self::IntegrationCatalog,
            "public_operations_rls" => Self::PublicOperationsRls,
            _ => return None,
        })
    }
}

/// Per-entity generator: runs once for each entity in generation order.
#[async_trait]
pub trait EntityGenerator: Send + Sync {
    /// The typed identity of this generator.
    fn kind(&self) -> EntityGeneratorKind;

    /// The generator's identity string as used in profile generator lists.
    /// Provided: derives from [`Self::kind`].
    fn name(&self) -> &str {
        self.kind().name()
    }

    /// Returns the database targets this generator supports.
    /// Returns `None` to indicate all targets are supported.
    fn supported_targets(&self) -> Option<Vec<DatabaseTarget>> {
        None
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        schema_title: &str,
        domain: &str,
        config: &DomainConfig,
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>>;
}

/// Per-domain generator: runs once for each domain.
#[async_trait]
pub trait DomainGenerator: Send + Sync {
    /// The typed identity of this generator.
    fn kind(&self) -> DomainGeneratorKind;

    /// The generator's identity string as used in profile generator lists.
    /// Provided: derives from [`Self::kind`].
    fn name(&self) -> &str {
        self.kind().name()
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        domain: &str,
        entity_titles: &[String],
        config: &DomainConfig,
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>>;
}

/// Global generator: runs once for the entire project.
///
/// Receives `generation_order` — the graph-derived list of entities that were
/// actually ingested and will have per-entity files generated. Implementations
/// should use this instead of `config.domains[*].entities` to stay consistent
/// with the entity generators.
#[async_trait]
pub trait GlobalGenerator: Send + Sync {
    /// The typed identity of this generator.
    fn kind(&self) -> GlobalGeneratorKind;

    /// The generator's identity string as used in profile generator lists.
    /// Provided: derives from [`Self::kind`].
    fn name(&self) -> &str {
        self.kind().name()
    }

    /// Returns the database targets this generator supports.
    /// Returns `None` to indicate all targets are supported.
    fn supported_targets(&self) -> Option<Vec<DatabaseTarget>> {
        None
    }

    /// Whether this generator must run (and have its files written) before
    /// the parallel global phase. Scaffolding generators whose output other
    /// generators consult via if-absent checks return true — the IFML
    /// skeleton's package.json supersedes the e2e generator's minimal stub.
    fn sequential_first(&self) -> bool {
        false
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        config: &DomainConfig,
        generation_order: &[GenerationEntry],
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>>;
}
