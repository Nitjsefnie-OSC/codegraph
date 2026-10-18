//! Cross-project domain dependencies (issue #276).
//!
//! A consumer pins a publisher's domain face — a #275 artifact document —
//! via `[[domains.X.dependencies]]` in domains.toml. Loading replays the
//! document's schema plane (Schema, Property, CodeList, EnumValue nodes and
//! the schema reference edges) into the live graph through the typed
//! `GraphIngestor` trait. Everything ingested this way is READ-ONLY model
//! data: provenance is stamped (`custom_annotations["source"] =
//! "dependency:<domain>"`), the auto-classifier never sees the foreign
//! schemas (they load after classification), and a foreign title that
//! already exists locally is skipped — the local copy wins dedup, the
//! skipped titles are reported for `doctor`/stderr.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use codegraph_config::config::{DomainConfig, DomainDependency, DomainEntry};
use codegraph_core::error::GraphError;
use codegraph_core::traits::{GraphIngestor, GraphQuerier};
use codegraph_core::types::{
    CodeList, EdgeProperties, EdgeType, EnumValue, PropertyNode, SchemaNode,
};
use codegraph_type_contracts::RefClassificationKind;
use serde::Deserialize;

use crate::artifact::{NodeRecord, PropValue, parse_document};
use crate::error::{Error, Result};

/// `SchemaNode.custom_annotations["source"]` prefix for dependency-ingested
/// schemas; the suffix is the foreign domain name (mox provenance precedent).
pub const DEPENDENCY_SOURCE_PREFIX: &str = "dependency:";

/// `SchemaNode.custom_annotations` key carrying the pinned face version.
pub const DEPENDENCY_VERSION_KEY: &str = "dependency_version";

/// Optional publishing metadata a producer may embed alongside the #275
/// document body (top-level `"meta"` object; the artifact parser ignores
/// unknown keys, so producers can add this without a format change).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct DependencyArtifactMeta {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
}

/// What a dependency load did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyLoadStats {
    pub artifacts: usize,
    pub schemas: usize,
    pub properties: usize,
    pub codelists: usize,
    pub enum_values: usize,
    pub edges: usize,
    /// Foreign titles skipped because a local schema already owns the title
    /// (local wins dedup). Sorted, deduplicated.
    pub shadowed_titles: Vec<String>,
}

impl std::fmt::Display for DependencyLoadStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} artifacts, {} schemas, {} properties, {} codelists, {} enum values, {} edges",
            self.artifacts,
            self.schemas,
            self.properties,
            self.codelists,
            self.enum_values,
            self.edges
        )?;
        if !self.shadowed_titles.is_empty() {
            write!(
                f,
                ", {} local-shadowed titles: {}",
                self.shadowed_titles.len(),
                self.shadowed_titles.join(", ")
            )?;
        }
        Ok(())
    }
}

/// Resolve a dependency `source` path: absolute paths pass through;
/// relative paths resolve against the directory holding domains.toml
/// (the project root).
pub fn resolve_dependency_path(config_dir: Option<&Path>, source: &str) -> PathBuf {
    let path = Path::new(source);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        config_dir.unwrap_or(Path::new(".")).join(path)
    }
}

/// Read the optional `meta` block from an artifact document without
/// consuming it: IO/JSON failures are load errors; a missing `meta` block
/// is an unversioned face (all fields `None`).
pub fn read_dependency_meta(path: &Path) -> Result<DependencyArtifactMeta> {
    let bytes = std::fs::read(path)?;
    dependency_meta_from_bytes(&bytes)
}

/// [`read_dependency_meta`] over already-read document bytes.
pub fn dependency_meta_from_bytes(bytes: &[u8]) -> Result<DependencyArtifactMeta> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let meta = value.get("meta").cloned();
    Ok(match meta {
        Some(meta) => Some(serde_json::from_value::<DependencyArtifactMeta>(meta)?),
        None => None,
    }
    .unwrap_or_default())
}

/// Load every dependency face declared in `config` into the graph behind
/// `ingestor`. Consumer domains are visited in sorted order and each face
/// exactly once (a face pinned by two consumer domains loads once).
/// Returns the aggregate load stats.
#[cfg_attr(feature = "hotpath", hotpath::measure)]
pub async fn load_dependency_artifacts(
    ingestor: &dyn GraphIngestor,
    querier: &dyn GraphQuerier,
    config: &DomainConfig,
    config_dir: Option<&Path>,
) -> Result<DependencyLoadStats> {
    let mut stats = DependencyLoadStats::default();
    let local_titles: HashSet<String> = querier
        .list_schemas(None)
        .await
        .map_err(Error::Graph)?
        .into_iter()
        .map(|s| s.title)
        .collect();

    // One face per foreign domain, loaded once even when several consumer
    // domains pin it. Pin selection is deterministic: consumer domains in
    // sorted order, then declaration order within a domain.
    let mut pins: Vec<(String, String, &DomainDependency)> = Vec::new();
    let mut consumer_domains: Vec<&String> = config.domains.keys().collect();
    consumer_domains.sort();
    for consumer in consumer_domains {
        for dep in &config.domains[consumer.as_str()].dependencies {
            pins.push((dep.domain.clone(), consumer.clone(), dep));
        }
    }
    pins.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    pins.dedup_by(|a, b| a.0 == b.0);

    for (domain, _, dep) in pins {
        let path = resolve_dependency_path(config_dir, &dep.source);
        load_one(ingestor, &local_titles, &domain, dep, &path, &mut stats).await?;
    }
    Ok(stats)
}

/// Register synthetic `DomainEntry`s for every dependency domain so
/// generation and validation treat the foreign faces as bounded contexts.
/// The entries carry no model config of their own (no entities, no schema
/// dir): generation reads the face from the graph.
#[cfg_attr(feature = "hotpath", hotpath::measure)]
pub fn inject_dependency_domains(config: &mut DomainConfig) -> usize {
    let mut names: BTreeSet<String> = BTreeSet::new();
    for entry in config.domains.values() {
        for dep in &entry.dependencies {
            if !config.domains.contains_key(&dep.domain) {
                names.insert(dep.domain.clone());
            }
        }
    }
    let count = names.len();
    for name in names {
        config
            .domains
            .insert(name.clone(), DomainEntry::dependency_placeholder(&name));
    }
    count
}

/// Collect the dependency artifact files as graph-cache inputs (issue #275
/// hash plane): editing a pinned face must invalidate the persisted graph.
pub fn collect_dependency_inputs(
    config: &DomainConfig,
    config_dir: Option<&Path>,
) -> Result<Vec<crate::artifact::InputFile>> {
    let mut files = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for entry in config.domains.values() {
        for dep in &entry.dependencies {
            if !seen.insert(dep.domain.clone()) {
                continue;
            }
            let path = resolve_dependency_path(config_dir, &dep.source);
            let mut input = crate::artifact::read_input_file(&path).map_err(|e| {
                Error::Config(format!("dependency face {} is unreadable: {e}", dep.source))
            })?;
            input.path = format!("<dependency:{}>{}", dep.domain, dep.source);
            files.push(input);
        }
    }
    Ok(files)
}

async fn load_one(
    ingestor: &dyn GraphIngestor,
    local_titles: &HashSet<String>,
    domain: &str,
    dep: &DomainDependency,
    path: &Path,
    stats: &mut DependencyLoadStats,
) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let meta = dependency_meta_from_bytes(&bytes)?;
    if let Some(face_domain) = meta.domain.as_deref()
        && face_domain != domain
    {
        return Err(Error::Config(format!(
            "dependency pinned as {domain:?} but {} carries meta.domain {face_domain:?}",
            dep.source
        )));
    }
    let doc = parse_document(&bytes).map_err(|e| {
        Error::Config(format!(
            "dependency artifact {} is invalid: {e}",
            dep.source
        ))
    })?;
    stats.artifacts += 1;

    // Partition the foreign schemas: local titles shadow dependency titles
    // (local wins dedup, made explicit — issue #276). Only loadable schemas
    // reach the id maps, so their properties and edges are skipped with
    // them.
    let mut shadowed: HashSet<String> = HashSet::new();
    let mut schemas: Vec<(SchemaNode, &NodeRecord)> = Vec::new();
    for node in &doc.nodes {
        if node.label != "Schema" {
            continue;
        }
        let title = prop_str(node, "title").unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        if local_titles.contains(&title) {
            if shadowed.insert(title.clone()) {
                stats.shadowed_titles.push(title);
            }
            continue;
        }
        let node_schema = schema_node_from_record(node, domain, dep)?;
        schemas.push((node_schema, node));
    }
    stats.shadowed_titles.sort();
    stats.shadowed_titles.dedup();

    // Natural-id maps for edge replay: artifact content keys → the ids the
    // typed ingest methods match on.
    let mut schema_id_by_key: HashMap<&str, String> = HashMap::new();
    let mut schema_title_by_key: HashMap<&str, String> = HashMap::new();
    for (node_schema, node) in &schemas {
        schema_id_by_key.insert(node.key.as_str(), node_schema.schema_id.clone());
        schema_title_by_key.insert(node.key.as_str(), node_schema.title.clone());
    }
    let mut property_id_by_key: HashMap<&str, String> = HashMap::new();
    for node in &doc.nodes {
        if node.label != "Property" {
            continue;
        }
        let (Some(name), Some(schema_title)) =
            (prop_str(node, "name"), prop_str(node, "_schema_title"))
        else {
            continue;
        };
        if shadowed.contains(&schema_title) {
            continue;
        }
        property_id_by_key.insert(node.key.as_str(), format!("{name}::{schema_title}"));
    }

    for (node_schema, _) in &schemas {
        ingestor
            .ingest_schema(node_schema)
            .await
            .map_err(Error::Graph)?;
        stats.schemas += 1;
    }

    for node in &doc.nodes {
        if node.label != "Property" {
            continue;
        }
        let Some(schema_title) = prop_str(node, "_schema_title") else {
            continue;
        };
        if shadowed.contains(&schema_title) {
            continue;
        }
        let Some(schema_id) = prop_str(node, "_schema_id") else {
            continue;
        };
        let prop = property_node_from_record(node)?;
        ingestor
            .ingest_property(&schema_title, &schema_id, &prop)
            .await
            .map_err(Error::Graph)?;
        stats.properties += 1;
    }

    for node in &doc.nodes {
        if node.label != "CodeList" {
            continue;
        }
        let Some(name) = prop_str(node, "name") else {
            continue;
        };
        if shadowed.contains(&name) {
            continue;
        }
        let codelist = CodeList {
            name,
            description: prop_str(node, "description"),
            pg_table_name: prop_str(node, "pg_table_name").unwrap_or_default(),
            render_as: prop_str(node, "render_as").unwrap_or_default(),
            check_expression: prop_str(node, "check_expression"),
        };
        ingestor
            .ingest_codelist(&codelist)
            .await
            .map_err(Error::Graph)?;
        stats.codelists += 1;
    }

    for node in &doc.nodes {
        if node.label != "EnumValue" {
            continue;
        }
        let Some(codelist_name) = prop_str(node, "_codelist_name") else {
            continue;
        };
        if shadowed.contains(&codelist_name) {
            continue;
        }
        let Some(value) = prop_str(node, "value") else {
            continue;
        };
        let enum_value = EnumValue {
            value,
            display_name: prop_str(node, "display_name"),
            sort_order: prop_int(node, "sort_order").unwrap_or(0) as i32,
        };
        ingestor
            .ingest_enum_value(&codelist_name, &enum_value)
            .await
            .map_err(Error::Graph)?;
        stats.enum_values += 1;
    }

    // Edge replay: the HasProperty and HasEnumValue edges are recreated by
    // the typed ingest methods themselves; the schema-plane reference edges
    // are replayed from the document with natural ids.
    for edge in &doc.edges {
        let edge_type = match edge.edge_type.as_str() {
            "ReferencesSchema" => EdgeType::ReferencesSchema,
            "ItemsOf" => EdgeType::ItemsOf,
            "ExtendsSchema" => EdgeType::ExtendsSchema,
            _ => continue,
        };
        let (from_id, to_id) = match edge_type {
            EdgeType::ExtendsSchema => (
                schema_title_by_key.get(edge.from.as_str()).cloned(),
                schema_title_by_key.get(edge.to.as_str()).cloned(),
            ),
            _ => (
                property_id_by_key.get(edge.from.as_str()).cloned(),
                schema_id_by_key.get(edge.to.as_str()).cloned(),
            ),
        };
        let (Some(from_id), Some(to_id)) = (from_id, to_id) else {
            continue;
        };
        let props = edge_properties_from_record(edge);
        ingestor
            .ingest_edge(&from_id, &to_id, edge_type, props.as_ref())
            .await
            .map_err(Error::Graph)?;
        stats.edges += 1;
    }
    Ok(())
}

fn prop_str(node: &NodeRecord, key: &str) -> Option<String> {
    match node.properties.get(key)? {
        PropValue::Str(s) => Some(s.clone()),
        _ => None,
    }
}

fn prop_bool(node: &NodeRecord, key: &str) -> Option<bool> {
    match node.properties.get(key)? {
        PropValue::Bool(b) => Some(*b),
        _ => None,
    }
}

fn prop_int(node: &NodeRecord, key: &str) -> Option<i64> {
    match node.properties.get(key)? {
        PropValue::Int(i) => Some(*i),
        PropValue::Str(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

fn prop_str_bounds(node: &NodeRecord, key: &str) -> Option<String> {
    match node.properties.get(key)? {
        PropValue::Str(s) => Some(s.clone()),
        PropValue::Int(i) => Some(i.to_string()),
        _ => None,
    }
}

/// Rehydrate a `SchemaNode` from the persisted property encoding (the same
/// fields `row_to_schema_node` reads back), stamping the dependency
/// provenance on top of the face's own annotations.
fn schema_node_from_record(
    node: &NodeRecord,
    domain: &str,
    dep: &DomainDependency,
) -> Result<SchemaNode> {
    let custom_annotations = prop_str(node, "custom_annotations")
        .and_then(|json| serde_json::from_str::<HashMap<String, serde_json::Value>>(&json).ok())
        .unwrap_or_default();
    let mut custom_annotations = custom_annotations;
    custom_annotations.insert(
        "source".to_string(),
        serde_json::Value::String(format!("{DEPENDENCY_SOURCE_PREFIX}{domain}")),
    );
    custom_annotations.insert(
        DEPENDENCY_VERSION_KEY.to_string(),
        serde_json::Value::String(dep.version.clone()),
    );
    let missing = |key: &str| {
        Error::Graph(GraphError::Ingest(format!(
            "dependency schema record {} is missing the {key:?} property",
            node.key
        )))
    };
    Ok(SchemaNode {
        schema_id: prop_str(node, "schema_id").ok_or_else(|| missing("schema_id"))?,
        title: prop_str(node, "title").ok_or_else(|| missing("title"))?,
        description: prop_str(node, "description"),
        schema_type: prop_str(node, "schema_type").unwrap_or_default(),
        classification: prop_str(node, "classification").unwrap_or_default(),
        domain: prop_str(node, "domain"),
        namespace: prop_str(node, "namespace"),
        rel_path: prop_str(node, "rel_path").unwrap_or_default(),
        pg_type: prop_str(node, "pg_type").unwrap_or_default(),
        rust_type: prop_str(node, "rust_type").unwrap_or_default(),
        sea_orm_type: prop_str(node, "sea_orm_type").unwrap_or_default(),
        rust_type_name: prop_str(node, "rust_type_name").unwrap_or_default(),
        pg_table_name: prop_str(node, "pg_table_name").unwrap_or_default(),
        api_path_segment: prop_str(node, "api_path_segment").unwrap_or_default(),
        parent_schema: prop_str(node, "parent_schema"),
        is_entity: prop_bool(node, "is_entity").unwrap_or(false),
        is_codelist: prop_bool(node, "is_codelist").unwrap_or(false),
        is_primitive_wrapper: prop_bool(node, "is_primitive_wrapper").unwrap_or(false),
        has_all_of: prop_bool(node, "has_all_of").unwrap_or(false),
        has_one_of: prop_bool(node, "has_one_of").unwrap_or(false),
        has_any_of: prop_bool(node, "has_any_of").unwrap_or(false),
        has_definitions: prop_bool(node, "has_definitions").unwrap_or(false),
        access: prop_str(node, "access").map(|a| match a.as_str() {
            "public" => codegraph_core::types::Access::Public,
            _ => codegraph_core::types::Access::Private,
        }),
        annotations: prop_str(node, "annotations").and_then(|a| serde_json::from_str(&a).ok()),
        custom_annotations,
    })
}

/// Rehydrate a `PropertyNode` from the persisted property encoding (the
/// same fields `row_to_property_node` reads back; bounds ride as strings).
fn property_node_from_record(node: &NodeRecord) -> Result<PropertyNode> {
    let missing = |key: &str| {
        Error::Graph(GraphError::Ingest(format!(
            "dependency property record {} is missing the {key:?} property",
            node.key
        )))
    };
    let parse_u64 = |key: &str| -> Option<u64> { prop_str_bounds(node, key)?.parse().ok() };
    let parse_u32 = |key: &str| -> Option<u32> { prop_str_bounds(node, key)?.parse().ok() };
    let parse_decimal =
        |key: &str| -> Option<rust_decimal::Decimal> { prop_str_bounds(node, key)?.parse().ok() };
    Ok(PropertyNode {
        name: prop_str(node, "name").ok_or_else(|| missing("name"))?,
        prop_type: prop_str(node, "prop_type").unwrap_or_default(),
        description: prop_str(node, "description"),
        format: prop_str(node, "format"),
        is_required: prop_bool(node, "is_required").unwrap_or(false),
        is_nullable: prop_bool(node, "is_nullable").unwrap_or(false),
        is_id: false,
        is_array: prop_bool(node, "is_array").unwrap_or(false),
        pattern: prop_str(node, "pattern"),
        min_length: parse_u64("min_length"),
        max_length: parse_u64("max_length"),
        min_items: parse_u32("min_items"),
        max_items: parse_u32("max_items"),
        minimum: parse_decimal("minimum"),
        maximum: parse_decimal("maximum"),
        pg_column_name: prop_str(node, "pg_column_name").unwrap_or_default(),
        pg_column_type: prop_str(node, "pg_column_type").unwrap_or_default(),
        rust_field_name: prop_str(node, "rust_field_name").unwrap_or_default(),
        rust_field_type: prop_str(node, "rust_field_type").unwrap_or_default(),
        sea_orm_type: prop_str(node, "sea_orm_type").unwrap_or_default(),
        render_strategy: prop_str(node, "render_strategy").unwrap_or_default(),
        ref_target: prop_str(node, "ref_target"),
        classification: prop_str(node, "classification"),
        projection: None,
        classification_kind: prop_str(node, "classification_kind")
            .as_deref()
            .and_then(parse_classification_kind),
        type_expr: None,
        ui_override_detail: None,
        ui_override_list_cell: None,
        ui_override_form: None,
        ui_override_inline: None,
    })
}

fn edge_properties_from_record(edge: &crate::artifact::EdgeRecord) -> Option<EdgeProperties> {
    let mut props = EdgeProperties::default();
    let mut has_any = false;
    if let Some(PropValue::Str(ref_path)) = edge.properties.get("ref_path") {
        props.ref_path = Some(ref_path.clone());
        has_any = true;
    }
    if let Some(PropValue::Str(composition_type)) = edge.properties.get("composition_type") {
        props.composition_type = Some(composition_type.clone());
        has_any = true;
    }
    has_any.then_some(props)
}

/// Full round trip of the classification-kind strings the ingestor writes
/// (mirrors `classification_kind_to_str` in the grafeo ingestor).
fn parse_classification_kind(s: &str) -> Option<RefClassificationKind> {
    use RefClassificationKind as K;
    match s {
        "primitive_wrapper" => Some(K::PrimitiveWrapper),
        "array_wrapper" => Some(K::ArrayWrapper),
        "range_wrapper" => Some(K::RangeWrapper),
        "codelist" => Some(K::CodelistReference),
        "codelist_check" => Some(K::CodelistCheck),
        "inline_enum" => Some(K::InlineEnum),
        "entity_reference" => Some(K::EntityReference),
        "value_object" => Some(K::ValueObject),
        "composite_wrapper" => Some(K::CompositeWrapper),
        "structured_wrapper" => Some(K::StructuredWrapper),
        "media_wrapper" => Some(K::MediaWrapper),
        _ => None,
    }
}
