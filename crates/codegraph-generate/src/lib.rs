pub mod code_writer;
pub mod codelist;
pub mod consistency;
pub mod domain_model;
pub mod error;
pub mod filter_fields;
pub mod ifml;
pub mod manifest;
pub mod persistence;
pub mod profile;
pub mod quality_gates;
pub mod report;
pub mod rosetta_expr;
pub mod template_engine;
pub mod traits;
pub mod type_registry;
pub mod ux;

pub mod api;
pub mod atproto;
pub mod cli;
pub mod db;
pub mod ddd;
pub mod domain_types;
pub mod emdash;
pub mod events;
pub mod evt_events;
pub mod fern;
pub mod grpc;
pub mod hooks;
pub mod integration;
pub mod ops;
pub mod playwright;
pub mod scaffold;
pub mod seed;
pub mod test;
pub mod ui;
pub mod webhook;

mod capabilities;
mod context;
mod fk;
mod ordering;
mod output;
mod pipeline;
mod project_config;
mod registry;

#[cfg(test)]
mod tests;

pub use fk::{
    fk_column_for_candidate, is_geometry_cast, pg_cast_for_type, resolve_parent_fk_column,
    resolve_parent_fk_column_same_domain,
};
pub use ordering::{all_domains_for_generation, compute_generation_order};
pub use output::{
    is_api_entity_generator, is_worker_routed_domain_generator, is_worker_routed_entity_generator,
    render_template, render_template_with_project, render_template_with_project_and_dialect,
};
pub use pipeline::{
    run_generators, run_generators_with_domain_types_base, run_generators_with_opts,
    run_ifml_generators,
};
pub use project_config::{
    AtprotoConfig, CargoConfig, CodegenConfig, DatabaseConfig, DeploymentConfig, DtoKeyCasing,
    GenerationEntry, GeneratorOpts, IdentityConfig, IntegrationFlags, PathsConfig, ProjectConfig,
    UxConfig, namespace_dir_prefix, namespace_module_dir, namespace_rust_prefix,
};
