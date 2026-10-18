/// The unified error type for ingest and generation pipelines.
///
/// Lives in `codegraph-core` so both the `codegraph` binary crate and the
/// `codegraph-generate` crate can share one `Result` alias; each crate
/// re-exports it as `crate::error`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Graph error: {0}")]
    Graph(#[from] GraphError),
    #[error("Schema not found: {0}")]
    SchemaNotFound(String),
    #[error("Ref resolution failed: {0}")]
    RefResolution(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Config error: {0}")]
    Config(String),
    #[error("mox schema import '{import_path}' imported by '{mox_path}' {reason}")]
    MoxSchemaImport {
        /// The .mox file declaring the import (as compiled).
        mox_path: String,
        /// The import path exactly as written in the declaration.
        import_path: String,
        /// Failure cause ("could not be read: …" / "is not valid JSON: …").
        reason: String,
    },
    #[error("rosetta model error in '{file}': {reason}")]
    RosettaModel {
        /// The .rosetta file the error came from.
        file: String,
        /// Failure cause, including sigil diagnostic codes/spans when the
        /// error comes from parsing or resolution.
        reason: String,
    },
    #[error("ddd model error in '{file}': {reason}")]
    DddModel {
        /// The .ddd file the error came from.
        file: String,
        /// Failure cause, including the rendered rex diagnostics when the
        /// error comes from compilation (grouped per file).
        reason: String,
    },
    #[error("evt model error in '{file}': {reason}")]
    EvtModel {
        /// The .evt file the error came from.
        file: String,
        /// Failure cause, including the rendered rex diagnostics when the
        /// error comes from compilation (grouped per file).
        reason: String,
    },
    #[error("deploy model error in '{file}': {reason}")]
    DeployModel {
        /// The .deploy file the error came from.
        file: String,
        /// Failure cause, including the rendered rex diagnostics when the
        /// error comes from compilation (grouped per file).
        reason: String,
    },
    #[error("Template error: {0}")]
    Template(String),
    #[error("Validation error: {0}")]
    Validation(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("Schema not found: {0}")]
    NotFound(String),
    #[error("Connection failed: {0}")]
    Connection(String),
    #[error("Query failed: {0}")]
    Query(String),
    #[error("Ingest failed: {0}")]
    Ingest(String),
    #[error("Internal error: {0}")]
    Internal(#[from] Box<dyn std::error::Error + Send + Sync>),
    #[error("Not implemented")]
    NotImplemented,
}
