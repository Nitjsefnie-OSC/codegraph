use crate::ingestor::registry::NodeRegistry;
use crate::schema_ddl;
use codegraph_core::error::GraphError;
use grafeo::GrafeoDB;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

pub struct GrafeoEngine {
    db: Arc<GrafeoDB>,
    start_time: Instant,
    /// Natural-key → NodeId registry for the direct edge path (issue #482).
    /// Populated only by this engine instance's node ingests; empty after a
    /// persistent reopen, which sends every edge to the GQL fallback.
    pub(crate) node_registry: NodeRegistry,
}

impl GrafeoEngine {
    /// Create a new in-memory Grafeo instance with typed schema DDL.
    pub fn in_memory() -> Result<Self, GraphError> {
        let db = GrafeoDB::new_in_memory();
        let engine = Self {
            db: Arc::new(db),
            start_time: Instant::now(),
            node_registry: NodeRegistry::new(),
        };
        engine.init_schema()?;
        Ok(engine)
    }

    /// Create a persistent Grafeo instance using a config.
    pub fn with_config(config: grafeo::Config) -> Result<Self, GraphError> {
        let db = GrafeoDB::with_config(config)
            .map_err(|e| GraphError::Connection(format!("config open failed: {e}")))?;
        let engine = Self {
            db: Arc::new(db),
            start_time: Instant::now(),
            node_registry: NodeRegistry::new(),
        };
        engine.init_schema()?;
        Ok(engine)
    }

    /// Open (or create) a persistent engine via the documented persistent
    /// configuration (`Config::persistent` → `GrafeoDB::with_config`),
    /// pinned to the single-file storage format. Parent directories are
    /// created as needed. Ingested data survives process exit once
    /// [`GrafeoEngine::checkpoint`] (or close) flushes the WAL.
    ///
    /// Persistence is plain database durability — independent of the
    /// artifact export/import layer, which is a separate, optional
    /// document concern.
    pub fn persistent(path: &Path) -> Result<Self, GraphError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                GraphError::Connection(format!(
                    "failed to create graph dir {}: {e}",
                    parent.display()
                ))
            })?;
        }
        let config = grafeo::Config::persistent(path)
            .with_storage_format(grafeo_engine::config::StorageFormat::SingleFile)
            // Ingest fans out one MATCH-per-edge (no label/property index
            // yet), so individual edge-insert queries on a large model run
            // tens of seconds — the engine's default 30s query timeout
            // aborts them mid-ingest ("Ingest failed: ingest_edge ...
            // timeout"). The in-memory default carries no timeout and the
            // same ingest completes; the persistent path must not time
            // out either.
            .without_query_timeout();
        Self::with_config(config)
    }

    /// Open an existing persistent database in read-only mode (the
    /// read-replica pattern): shared file lock, loads the last checkpoint
    /// snapshot, no WAL replay, and mutations are rejected at the session
    /// level. Schema DDL is NOT re-run — the checkpoint snapshot already
    /// carries the catalog — so call this only on a database that was
    /// checkpointed before.
    ///
    /// Property indexes ARE re-created (issue #485): the grafeo v2 load
    /// path never rebuilds them (grafeo-engine catalog_section deserializes
    /// the index names for reference only), so every MATCH keyed on an
    /// indexed property degrades to a full node scan for the whole warm
    /// generation phase. Restoring the known set costs one O(N) scan per
    /// property at open.
    pub fn open_read_only(path: &Path) -> Result<Self, GraphError> {
        let config = grafeo::Config::read_only(path);
        let db = GrafeoDB::with_config(config)
            .map_err(|e| GraphError::Connection(format!("read-only open failed: {e}")))?;
        let engine = Self {
            db: Arc::new(db),
            start_time: Instant::now(),
            node_registry: NodeRegistry::new(),
        };
        for prop in schema_ddl::indexed_properties() {
            engine.db.create_property_index(prop);
        }
        Ok(engine)
    }

    /// Flush the write-ahead log into the storage file so all committed
    /// mutations are durable on disk.
    pub fn checkpoint(&self) -> Result<(), GraphError> {
        self.db
            .wal_checkpoint()
            .map_err(|e| GraphError::Connection(format!("checkpoint failed: {e}")))
    }

    /// Re-run schema DDL (idempotent due to IF NOT EXISTS).
    pub fn reinit_schema(&self) -> Result<(), GraphError> {
        self.init_schema()
    }

    /// Get a reference to the underlying database.
    pub fn db(&self) -> &GrafeoDB {
        &self.db
    }

    /// Get the start time for duration tracking in finalize().
    pub(crate) fn start_time(&self) -> Instant {
        self.start_time
    }

    fn init_schema(&self) -> Result<(), GraphError> {
        let session = self.db.session();
        for ddl in schema_ddl::ddl_statements() {
            session
                .execute(ddl)
                .map_err(|e| GraphError::Ingest(format!("DDL failed: {e}")))?;
        }
        // Use programmatic API for indexes (GQL CREATE INDEX has parser issues with FOR keyword)
        for prop in schema_ddl::indexed_properties() {
            self.db.create_property_index(prop);
        }
        Ok(())
    }
}
