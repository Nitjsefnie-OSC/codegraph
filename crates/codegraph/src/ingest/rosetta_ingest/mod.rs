//! Rosetta (Rune DSL / `.rosetta`) data-plane bridge (issue #256).
//!
//! The [`crate::ingest::mox_ingest`] bridge pattern applied to `.rosetta`:
//! parse → lower → resolve via the sigil crates (ALL user files in ONE
//! resolve call, builtins automatic), then walk the resolved
//! [`sigil_model::SemanticElement`]s into the same graph shapes the JSON
//! Schema path produces — the dispositions pinned by
//! `docs/rosetta-gap-analysis.md` (issue #254):
//!
//! - `type`/`choice` ([`sigil_model::Data`]) → `SchemaNode` (+ properties,
//!   `ExtendsSchema` for `extends` with the JSON path's ingest-time
//!   attribute merge: ancestors first, each group name-sorted, first
//!   occurrence wins; Rosetta `override` attributes REPLACE the inherited
//!   slot in place). Choices keep their options as `(0..1)` attributes and
//!   carry a `rosetta_choice` annotation. Rosetta types are AUTO-SCORED by
//!   the classifier (#258) — the bridge never declares entity/VO (unlike
//!   the mox bridge, where that is author-declarative).
//! - attributes → `PropertyNode` with `ReferencesSchema`/`ItemsOf` edges
//!   (`ref_target` from the referenced titles); builtin
//!   `int/number/string/date/dateTime/time/boolean` → the JSON path's
//!   primitive mappings.
//! - `enum` ([`sigil_model::Enumeration`]) → `CodeList` + `EnumValue`s
//!   (value + display_name) + the codelist `SchemaNode` the DDL/FK/link
//!   machinery keys on; `enum extends` merges the parent's values first.
//! - `[metadata]`/labels/`[ruleReference]`/`[docReference]` →
//!   `custom_annotations` payloads (gap-analysis finding 4): schema-level
//!   annotations land on the SchemaNode, attribute-level ones under a
//!   property-keyed `rosetta_attribute_annotations` map on the owning
//!   SchemaNode (a serde-defaulted `PropertyNode.custom_annotations` field
//!   is the flagged follow-up uplift — 60+ literal construction sites make
//!   it more than a bridge-sized change).
//! - conditions ([`sigil_model::Condition`]) → `ConditionNode`s (issue
//!   #261): each named condition lands as `kind: Condition` carrying the
//!   canonical `Expr::to_json()` payload (the embedding contract pinned by
//!   WP1.6: deterministic, serialization-stable, write-once — `Expr` is
//!   Serialize-only), linked to its schema via a `HasCondition` edge;
//!   `choice` types derive ONE `kind: OneOf` node whose options are the
//!   option attributes' referenced titles. Transpilation is #262. The
//!   `rosetta_conditions` annotation payload on the SchemaNode remains as
//!   provenance (#259 snapshot).
//! - namespaces (issue #268): declaring files get NamespaceNodes
//!   (`source = "rosetta"`, `InNamespace` + dotted `NamespaceParent`);
//!   import-only targets land as `source = "discovered"` so
//!   `NamespaceImports` edges resolve. Namespaces are still NOT domains —
//!   the domain falls back to the mox bridge's `resolve_domain` semantics
//!   over the dotted namespace (#267 config `domain=` wins when declared).
//! - regulatory reference elements (issue #265) → `RegulatoryNode`s:
//!   reports/bodies/corpora/segments/rule sources/rule schemas/meta types
//!   land as ONE parameterized node family (kind-tagged), with the
//!   associations the model actually expresses as edges — `[docReference]`
//!   metadata and report regulatory refs as `RegulatoryReference` edges
//!   (Schema/Condition/Function/Report → body/corpus/segment), `with
//!   source S` as `HasRuleSource`, a corpus' parent body as
//!   `CorpusInBody`, and rule-source `extends` as `DerivesFrom`.
//!   Doc-reference targets no declared element backs are skipped (sigil
//!   itself does not validate them).
//! - functions (issue #263) → `FunctionNode`s: each sigil `Function`
//!   lands as ONE structured node carrying its dispatch head
//!   (`(attr: Enum->VALUE)`), typed inputs/output, aliases (`alias n: e`),
//!   `set`/`add` operations with their `->` paths, and post-conditions —
//!   every expression persisted as the canonical `Expr::to_json()` payload
//!   for the generation-time transpiler. `extends` resolves within the
//!   run (functions sorted by name per domain; an extends CYCLE is a hard
//!   error naming the cycle) and becomes a `FunctionExtends` edge plus the
//!   denormalized `extends` field. `[transform]` annotations land on the
//!   node (typed) AND keep riding onto the named rule-schema node's
//!   properties (`transform_annotations`, the #265 surface). Function
//!   `[docReference ...]` metadata emits `RegulatoryReference` edges owned
//!   by `RegulatoryOwner::Function`.
//! - rules (issue #264) → `RuleNode`s: each sigil `Rule` lands as ONE
//!   structured node (`kind`: reporting/eligibility from the `eligibility`
//!   bool, `input_type` from the `from TypeCall` clause, canonical
//!   `Expr::to_json` body payload). Rules do NOT extend (sigil has no
//!   rule parent ref). A resolvable input type becomes a `RuleAppliesTo`
//!   edge (Rule → Schema, written after the schema bridging passes);
//!   rule `[docReference ...]` metadata emits `RegulatoryReference` edges
//!   owned by `RegulatoryOwner::Rule`; `[ruleReference R]` entries on
//!   `rule source` class attributes promote to `RuleReference` edges
//!   (Schema → Rule, carrying the attribute + source name) when R names a
//!   rule of this run — unresolvable references stay documented skips.
//! - out-of-data-plane elements (annotation decls/basic types/aliases/
//!   library functions) are counted and named as `needs_review` — never
//!   silently dropped, never bridged.
//!
//! Provenance: every bridged node carries `custom_annotations["origin"]
//! = "rosetta"` (issue #255) — deliberately DISTINCT from the mox
//! `source` key, so `is_mox_sourced` keeps applying to mox only and the
//! classifier's priority-0 bypass does not pick rosetta schemas up
//! (rosetta stays auto-scored).
//!
//! Title coordination (issue #257 wires the pipeline): `bridged_titles`
//! feeds the JSON-schema pass's skip-set (`ingest_schemas_with_skips`),
//! mirroring mox-first ordering — `.rosetta` wins title conflicts by
//! passing first.

mod bridge;
mod mapping;
mod naming;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use sigil_model::SemanticElement;
use sigil_resolve::Resolution;

use codegraph_config::config::DomainConfig;
use codegraph_core::traits::{GraphIngestor, GraphQuerier};
use codegraph_core::types::{
    CodeList, ConditionKind, ConditionNode, EdgeProperties, EdgeType, EnumValue, NamespaceImport,
    RegulatoryEdgeKind, RegulatoryKind, RegulatoryNode, RegulatoryOwner,
};
use codegraph_naming::{strip_suffix, to_snake_case};

use crate::error::{Error, Result};
use crate::ingest::async_ingest::sanitize_description;
use crate::ingest::mox_ingest::{
    DISCOVERED_NAMESPACE_SOURCE, emit_schema_namespace_edge, ingest_namespaces_deduped,
    resolve_domain,
};

use bridge::{attribute_property, data_schema_node, enum_schema_node, ordered_bridge_attributes};
pub(crate) use naming::function_node;
use naming::{
    collect_universes, detect_function_extends_cycle, element_dedup_key, element_kind_label,
    merged_enum_values, model_namespace, named_ref_title, referenced_title, referenced_title_str,
    rule_node, schema_id, synthesize_report_name,
};

/// Provenance marker on every rosetta-bridged node (`custom_annotations`
/// key `origin`, value `rosetta`) — distinct from the mox `source` key.
pub const ROSETTA_ORIGIN: &str = "rosetta";

/// `NamespaceNode.source` provenance for rosetta-declared namespaces.
pub(crate) const ROSETTA_NAMESPACE_SOURCE: &str = "rosetta";

/// Counters for one [`ingest_rosetta_files`] run. `bridged_titles` feeds
/// the JSON-schema pass's skip set (issue #257).
#[derive(Debug, Default)]
pub struct RosettaIngestStats {
    pub files: usize,
    pub types: usize,
    pub choices: usize,
    pub properties: usize,
    pub edges: usize,
    pub enums: usize,
    pub enum_values: usize,
    pub enum_schemas: usize,
    pub extends: usize,
    pub conditions_recorded: usize,
    /// Named conditions that landed as ConditionNodes (#261).
    pub conditions_ingested: usize,
    /// Bridge-derived one_of nodes (one per `choice` type).
    pub one_of_ingested: usize,
    /// Regulatory reference nodes ingested (issue #265): reports, bodies,
    /// corpora, segments, rule sources, rule schemas, meta types.
    pub regulatory_nodes: usize,
    /// Regulatory reference edges (doc references, report regulatory refs,
    /// rule sources, corpus parents, metaType/rule-source derivation).
    pub regulatory_edges: usize,
    /// Function nodes ingested (issue #263): sigil `Function` elements.
    pub functions_ingested: usize,
    /// Rule nodes ingested (issue #264): sigil `Rule` elements
    /// (reporting + eligibility).
    pub rules_ingested: usize,
    /// `RuleAppliesTo` edges written for rules whose `from` input type
    /// matches a schema of the graph (pre-existing or bridged this run).
    pub rule_applies_to: usize,
    /// Rule-source `[ruleReference R]` entries promoted to `RuleReference`
    /// edges (issue #264).
    pub rule_reference_edges: usize,
    /// `[ruleReference R]` entries naming no rule of this run — documented
    /// skips, never silent drops.
    pub rule_reference_skips: usize,
    pub namespaces: usize,
    pub namespace_imports: usize,
    /// Out-of-data-plane elements counted for review (never silently
    /// dropped, never bridged — #264 owns rule nodes).
    pub needs_review: usize,
    pub skipped: usize,
    pub bridged_titles: Vec<String>,
    /// Names of the out-of-data-plane elements (for #257's stderr notice).
    pub needs_review_names: Vec<String>,
}

impl std::fmt::Display for RosettaIngestStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} files, {} types ({} choices), {} properties, {} edges, {} enums \
                 ({} values, {} codelist schemas), {} extends, {} conditions recorded \
                 ({} nodes, {} one_of), {} regulatory nodes ({} refs), {} functions, \
                 {} rules ({} applies-to, {} rule-source refs, {} skipped), \
                 {} namespaces ({} imports)",
            self.files,
            self.types,
            self.choices,
            self.properties,
            self.edges,
            self.enums,
            self.enum_values,
            self.enum_schemas,
            self.extends,
            self.conditions_recorded,
            self.conditions_ingested,
            self.one_of_ingested,
            self.regulatory_nodes,
            self.regulatory_edges,
            self.functions_ingested,
            self.rules_ingested,
            self.rule_applies_to,
            self.rule_reference_edges,
            self.rule_reference_skips,
            self.namespaces,
            self.namespace_imports,
        )?;
        if self.needs_review != 0 {
            write!(f, ", {} needs_review", self.needs_review)?;
        }
        Ok(())
    }
}

/// Result of one [`ingest_rosetta_files`] run.
#[derive(Debug, Default)]
pub struct RosettaIngestOutcome {
    pub stats: RosettaIngestStats,
}

/// Ingest `.rosetta` data-plane models into the graph.
///
/// Hard-errors on syntax or resolution failures via
/// [`Error::RosettaModel`] (codes/spans surfaced) — a broken model never
/// half-ingests, mirroring the `MoxSchemaImport` precedent.
#[cfg_attr(feature = "hotpath", hotpath::measure)]
pub async fn ingest_rosetta_files(
    ingestor: &dyn GraphIngestor,
    querier: &dyn GraphQuerier,
    rosetta_paths: &[PathBuf],
    domain_config: &DomainConfig,
    type_suffix: &str,
) -> Result<RosettaIngestOutcome> {
    let mut stats = RosettaIngestStats {
        files: rosetta_paths.len(),
        ..Default::default()
    };

    // Deterministic: parse each file in input order (deduped by canonical
    // path), then resolve ALL user files in ONE call (builtins automatic).
    let mut parsed: Vec<sigil_model::ModelFile> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for path in rosetta_paths {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if !seen.insert(canonical.display().to_string()) {
            stats.files -= 1;
            continue;
        }
        let display = path.display().to_string();
        let text = std::fs::read_to_string(path).map_err(|e| Error::RosettaModel {
            file: display.clone(),
            reason: format!("could not be read: {e}"),
        })?;
        let source = sigil_diag::SourceFile::new(display.clone(), text);
        let (unit, diagnostics) = sigil_syntax::parse(&source);
        let errors: Vec<String> = diagnostics
            .iter()
            .filter(|d| d.severity == sigil_diag::Severity::Error)
            .map(|d| format!("[{}] {:?} {}", d.code, d.span, d.message))
            .collect();
        if !errors.is_empty() {
            return Err(Error::RosettaModel {
                file: display,
                reason: format!("syntax errors: {}", errors.join("; ")),
            });
        }
        let Some(unit) = unit else {
            return Err(Error::RosettaModel {
                file: display,
                reason: "parsing produced no syntax tree".to_string(),
            });
        };
        parsed.push(sigil_syntax::lower(&display, &unit));
    }
    if parsed.is_empty() {
        return Ok(RosettaIngestOutcome { stats });
    }

    let resolution: Resolution = sigil_resolve::resolve(parsed);
    let resolve_errors: Vec<String> = resolution
        .diagnostics
        .iter()
        .filter(|d| d.severity == sigil_diag::Severity::Error)
        .map(|d| format!("[{}] {}", d.code, d.message))
        .collect();
    if !resolve_errors.is_empty() {
        return Err(Error::RosettaModel {
            file: "rosetta workspace".to_string(),
            reason: format!("resolution errors: {}", resolve_errors.join("; ")),
        });
    }

    // `Resolution.files` = builtin files first, then user files in
    // submission order. Builtins contribute to the name universes but are
    // never bridged.
    let builtin_count = sigil_resolve::builtin_files().len();
    let user_files = resolution.files.get(builtin_count..).unwrap_or(&[]);

    // Name universes for reference classification (bare last segments —
    // v1 keeps Rosetta's per-namespace uniqueness; the title-suffix
    // collision rule matches the JSON skip-set contract).
    let mut enum_titles: HashSet<String> = HashSet::new();
    let mut type_titles: HashSet<String> = HashSet::new();
    for model in &resolution.files {
        collect_universes(model, &mut type_titles, &mut enum_titles);
    }

    // User-declared typeAliases (`typeAlias Money: number(fractionalDigits: 2)`)
    // lower onto primitive property types at attribute-bridge time: the alias
    // target's builtin name + arguments decide the PgType (gap doc, Data
    // plane "TypeAlias → bridge-side alias lowering"). Chain-safe up to a
    // small depth; aliases landing on data types fall through to the
    // entity-reference arm instead.
    let mut alias_types: HashMap<String, sigil_model::TypeRef> = HashMap::new();
    for model in &resolution.files {
        for element in &model.elements {
            if let SemanticElement::TypeAlias(alias) = element {
                alias_types.insert(alias.name.clone(), alias.type_ref.clone());
            }
        }
    }

    // Schema entities already in the graph: rosetta types attach by NAME
    // and never override an existing node (mox precedent).
    let schemas = querier.list_schemas(None).await.unwrap_or_default();
    let known_titles: HashSet<String> = schemas.iter().map(|s| s.title.clone()).collect();
    let json_schema_ids: HashMap<String, String> = schemas
        .iter()
        .map(|s| (s.title.clone(), s.schema_id.clone()))
        .collect();

    // ── Element universe (user files only): data types + enums to bridge;
    // regulatory reference elements for the #265 plane; everything else is
    // out-of-data-plane and recorded for review.
    let mut data_index: Vec<(&sigil_model::Data, String, String, &str)> = Vec::new();
    let mut enum_index: Vec<(&sigil_model::Enumeration, String, String)> = Vec::new();
    let mut reg_bodies: Vec<(&sigil_model::Body, String)> = Vec::new();
    let mut reg_corpora: Vec<(&sigil_model::Corpus, String)> = Vec::new();
    let mut reg_segments: Vec<(&sigil_model::Segment, String)> = Vec::new();
    let mut reg_rule_schemas: Vec<(&sigil_model::Schema, String)> = Vec::new();
    let mut reg_meta_types: Vec<(&sigil_model::MetaType, String)> = Vec::new();
    let mut reg_rule_sources: Vec<(&sigil_model::ExternalRuleSource, String)> = Vec::new();
    // (report, domain, synthesized name).
    let mut reg_reports: Vec<(&sigil_model::Report, String, String)> = Vec::new();
    // (function, domain).
    let mut func_index: Vec<(&sigil_model::Function, String)> = Vec::new();
    // (rule, domain).
    let mut rule_index: Vec<(&sigil_model::Rule, String)> = Vec::new();
    // Function `[transform]` annotations awaiting their target rule-schema
    // node: (bare reference, function name, transform kind).
    let mut function_transforms: Vec<(String, String, &'static str)> = Vec::new();
    let mut bridged_schema_ids: HashMap<String, String> = HashMap::new();
    let mut seen_elements: HashSet<String> = HashSet::new();
    // Namespace universe (issue #268): declaring-file namespaces get the
    // "rosetta" source; import-only targets (e.g. the sigil builtins'
    // `com.rosetta.model`) land as "discovered" so import edges resolve
    // and validation sees them.
    let mut namespace_sources: BTreeMap<String, String> = BTreeMap::new();
    let mut namespace_imports: Vec<NamespaceImport> = Vec::new();
    for model in user_files {
        if !model.namespace.is_empty() {
            namespace_sources
                .entry(model.namespace.clone())
                .or_insert_with(|| ROSETTA_NAMESPACE_SOURCE.to_string());
        }
        // Namespaces are NOT domains (#267/#268): domain resolution
        // mirrors the mox package fallback until the namespace
        // assignment config lands.
        let (domain, _) = resolve_domain(domain_config, &model.namespace);
        for element in &model.elements {
            let name = element.name();
            if !seen_elements.insert(element_dedup_key(element, name)) {
                continue;
            }
            match element {
                SemanticElement::Data(data) => {
                    let id = schema_id(&model.namespace, name);
                    data_index.push((data, domain.clone(), id.clone(), &model.namespace));
                }
                SemanticElement::Enumeration(enumeration) => {
                    let id = schema_id(&model.namespace, name);
                    enum_index.push((enumeration, domain.clone(), id));
                }
                SemanticElement::Body(body) => reg_bodies.push((body, domain.clone())),
                SemanticElement::Corpus(corpus) => reg_corpora.push((corpus, domain.clone())),
                SemanticElement::Segment(segment) => reg_segments.push((segment, domain.clone())),
                SemanticElement::Schema(schema) => reg_rule_schemas.push((schema, domain.clone())),
                SemanticElement::MetaType(meta_type) => {
                    reg_meta_types.push((meta_type, domain.clone()))
                }
                SemanticElement::ExternalRuleSource(source) => {
                    reg_rule_sources.push((source, domain.clone()))
                }
                SemanticElement::Report(report) => {
                    reg_reports.push((report, domain.clone(), synthesize_report_name(report)))
                }
                SemanticElement::Function(function) => {
                    func_index.push((function, domain.clone()));
                    // Transform annotations keep riding onto the targeted
                    // rule-schema node's properties (the #265 surface) in
                    // ADDITION to landing on the FunctionNode itself.
                    for transform in &function.transform {
                        if let Some(reference) = &transform.reference {
                            function_transforms.push((
                                referenced_title_str(reference),
                                function.name.clone(),
                                transform.kind.as_str(),
                            ));
                        }
                    }
                }
                SemanticElement::Rule(rule) => {
                    rule_index.push((rule, domain.clone()));
                }
                SemanticElement::Annotation(_)
                | SemanticElement::TypeAlias(_)
                | SemanticElement::BasicType(_)
                | SemanticElement::RecordType(_)
                | SemanticElement::LibraryFunction(_) => {
                    stats.needs_review += 1;
                    stats
                        .needs_review_names
                        .push(format!("{} {name}", element_kind_label(element)));
                }
            }
        }
    }
    // Import scan: `import a.b.*` / `import a.b as x` → NamespaceImports
    // edges (issue #268). The sigil lowered form appends `.*` to the
    // wildcard namespace string — strip it back to the bare FQN. Targets
    // outside the workspace's own namespaces join the node universe as
    // "discovered". Empty importing namespaces record no edges (nothing
    // to import FROM); dedup keeps repeat imports to one edge.
    let mut seen_imports: HashSet<(String, String, bool, Option<String>)> = HashSet::new();
    for model in user_files {
        if model.namespace.is_empty() {
            continue;
        }
        for imp in &model.imports {
            let target = imp
                .imported_namespace
                .trim_end_matches('*')
                .trim_end_matches('.');
            if target.is_empty() {
                continue;
            }
            namespace_sources
                .entry(target.to_string())
                .or_insert_with(|| DISCOVERED_NAMESPACE_SOURCE.to_string());
            let payload = (
                model.namespace.clone(),
                target.to_string(),
                imp.wildcard,
                imp.namespace_alias.clone(),
            );
            if seen_imports.insert(payload.clone()) {
                namespace_imports.push(NamespaceImport {
                    from_ns: payload.0,
                    to_ns: payload.1,
                    wildcard: payload.2,
                    alias: payload.3,
                });
            }
        }
    }

    // ── Namespace bridge (issue #268): namespace nodes with their dotted
    // parent chains, then the import edges (both endpoints exist by now).
    stats.namespaces = ingest_namespaces_deduped(ingestor, &namespace_sources).await?;
    for imp in &namespace_imports {
        ingestor
            .ingest_namespace_import(imp)
            .await
            .map_err(Error::Graph)?;
        stats.namespace_imports += 1;
    }

    // ── Bridge pass 0: regulatory reference nodes (#265). ONE parameterized
    // node family (kind-tagged); `reg_known` drives best-effort edge
    // emission below so stats stay backend-independent.
    let mut reg_known: HashSet<(String, String)> = HashSet::new();
    let transform_annotations: HashMap<&str, Vec<serde_json::Value>> = function_transforms
        .iter()
        .fold(HashMap::new(), |mut acc, (reference, function, kind)| {
            acc.entry(reference.as_str())
                .or_insert_with(Vec::new)
                .push(serde_json::json!({ "function": function, "kind": kind }));
            acc
        });
    for (body, domain) in &reg_bodies {
        let node = RegulatoryNode {
            name: body.name.clone(),
            kind: RegulatoryKind::Body,
            label: None,
            definition: body.definition.clone(),
            domain: Some(domain.clone()),
            properties: serde_json::json!({
                "origin": ROSETTA_ORIGIN,
                "body_type": body.body_type,
            }),
        };
        ingestor
            .ingest_regulatory(&node)
            .await
            .map_err(Error::Graph)?;
        reg_known.insert((RegulatoryKind::Body.as_str().to_string(), body.name.clone()));
        stats.regulatory_nodes += 1;
    }
    for (corpus, domain) in &reg_corpora {
        let node = RegulatoryNode {
            name: corpus.name.clone(),
            kind: RegulatoryKind::Corpus,
            label: corpus.display_name.clone(),
            definition: corpus.definition.clone(),
            domain: Some(domain.clone()),
            properties: serde_json::json!({
                "origin": ROSETTA_ORIGIN,
                "corpus_type": corpus.corpus_type,
                "parent_body": corpus.body.as_deref().map(referenced_title_str),
            }),
        };
        ingestor
            .ingest_regulatory(&node)
            .await
            .map_err(Error::Graph)?;
        reg_known.insert((
            RegulatoryKind::Corpus.as_str().to_string(),
            corpus.name.clone(),
        ));
        stats.regulatory_nodes += 1;
    }
    for (segment, domain) in &reg_segments {
        let node = RegulatoryNode {
            name: segment.name.clone(),
            kind: RegulatoryKind::Segment,
            label: None,
            definition: None,
            domain: Some(domain.clone()),
            properties: serde_json::json!({ "origin": ROSETTA_ORIGIN }),
        };
        ingestor
            .ingest_regulatory(&node)
            .await
            .map_err(Error::Graph)?;
        reg_known.insert((
            RegulatoryKind::Segment.as_str().to_string(),
            segment.name.clone(),
        ));
        stats.regulatory_nodes += 1;
    }
    for (schema, domain) in &reg_rule_schemas {
        let transforms = transform_annotations
            .get(schema.name.as_str())
            .cloned()
            .unwrap_or_default();
        let mut properties = serde_json::json!({
            "origin": ROSETTA_ORIGIN,
            "format": schema.format,
            "transform_annotations": transforms,
        });
        if !schema.annotations.is_empty()
            && let Ok(annotations) = serde_json::to_value(&schema.annotations)
        {
            properties["annotations"] = annotations;
        }
        let node = RegulatoryNode {
            name: schema.name.clone(),
            kind: RegulatoryKind::RuleSchema,
            label: None,
            definition: schema.definition.clone(),
            domain: Some(domain.clone()),
            properties,
        };
        ingestor
            .ingest_regulatory(&node)
            .await
            .map_err(Error::Graph)?;
        reg_known.insert((
            RegulatoryKind::RuleSchema.as_str().to_string(),
            schema.name.clone(),
        ));
        stats.regulatory_nodes += 1;
    }
    for (meta_type, domain) in &reg_meta_types {
        let node = RegulatoryNode {
            name: meta_type.name.clone(),
            kind: RegulatoryKind::MetaType,
            label: None,
            definition: None,
            domain: Some(domain.clone()),
            properties: serde_json::json!({
                "origin": ROSETTA_ORIGIN,
                "type_ref": referenced_title(&meta_type.type_ref),
            }),
        };
        ingestor
            .ingest_regulatory(&node)
            .await
            .map_err(Error::Graph)?;
        reg_known.insert((
            RegulatoryKind::MetaType.as_str().to_string(),
            meta_type.name.clone(),
        ));
        stats.regulatory_nodes += 1;
    }
    for (source, domain) in &reg_rule_sources {
        let classes: Vec<serde_json::Value> = source
            .classes
            .iter()
            .map(|class| {
                serde_json::json!({
                    "data": referenced_title(&class.data),
                    "attributes": class.attributes.iter().map(|attribute| {
                        serde_json::json!({
                            "add": attribute.add,
                            "attribute": attribute.attribute,
                            "rule_references": attribute.rule_references.iter().map(|r| {
                                serde_json::json!({
                                    "rule": r.rule,
                                    "empty": r.empty,
                                })
                            }).collect::<Vec<_>>(),
                        })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();
        let node = RegulatoryNode {
            name: source.name.clone(),
            kind: RegulatoryKind::RuleSource,
            label: None,
            definition: None,
            domain: Some(domain.clone()),
            properties: serde_json::json!({
                "origin": ROSETTA_ORIGIN,
                "super_source": source.super_source.as_ref().map(referenced_title),
                "classes": classes,
            }),
        };
        ingestor
            .ingest_regulatory(&node)
            .await
            .map_err(Error::Graph)?;
        reg_known.insert((
            RegulatoryKind::RuleSource.as_str().to_string(),
            source.name.clone(),
        ));
        stats.regulatory_nodes += 1;
    }
    for (report, domain, name) in &reg_reports {
        let node = RegulatoryNode {
            name: name.clone(),
            kind: RegulatoryKind::Report,
            label: None,
            definition: None,
            domain: Some(domain.clone()),
            properties: serde_json::json!({
                "origin": ROSETTA_ORIGIN,
                "timing": report.timing.as_str(),
                "input_type": referenced_title(&report.input_type),
                "report_type": named_ref_title(&report.report_type),
                "eligibility_rules": report.eligibility_rules.iter().map(named_ref_title)
                    .collect::<Vec<_>>(),
                "rule_source": report.rule_source.as_ref().map(named_ref_title),
                "regulatory": {
                    "body": named_ref_title(&report.regulatory.body),
                    "corpora": report.regulatory.corpora.iter().map(named_ref_title)
                        .collect::<Vec<_>>(),
                    "segments": report.regulatory.segments.iter().map(|segment| {
                        serde_json::json!({
                            "segment": named_ref_title(&segment.segment),
                            "reference": segment.reference,
                        })
                    }).collect::<Vec<_>>(),
                },
            }),
        };
        ingestor
            .ingest_regulatory(&node)
            .await
            .map_err(Error::Graph)?;
        reg_known.insert((RegulatoryKind::Report.as_str().to_string(), name.clone()));
        stats.regulatory_nodes += 1;
    }

    // ── Bridge pass 0b: functions (issue #263). Deterministic order —
    // functions sorted by name within a domain — drives extends resolution:
    // every child carries the parent's name (denormalized `extends` field)
    // and the backend links the `FunctionExtends` edge when the parent node
    // exists in this run. An extends CYCLE is a hard error naming the
    // cycle (a cyclic family could never be code-generated in dependency
    // order, so half-bridging it would be worse than failing).
    func_index.sort_by(|(a, da), (b, db)| (da, &a.name).cmp(&(db, &b.name)));
    if let Some(cycle) = detect_function_extends_cycle(&func_index) {
        return Err(Error::RosettaModel {
            file: "rosetta workspace".to_string(),
            reason: format!("function extends cycle detected: {cycle}"),
        });
    }
    for (function, domain) in &func_index {
        let node = function_node(function, domain);
        ingestor
            .ingest_function(&node)
            .await
            .map_err(Error::Graph)?;
        stats.functions_ingested += 1;
        // Doc-reference edges come after the node exists (backends match
        // the owner by name).
        for doc in &function.doc_references {
            emit_doc_reference_edges(
                ingestor,
                &RegulatoryOwner::Function(node.name.clone()),
                doc,
                &reg_known,
                &mut stats,
            )
            .await?;
        }
    }
    // The FunctionExtends edges come after every function node exists —
    // name-ordered ingestion is not parent-first. Only functions of this
    // run are linked (a parent outside the run, e.g. a builtin, is
    // unresolvable here); the child's `extends` field keeps the name.
    for (function, _) in &func_index {
        let Some(parent) = function.super_function.as_ref().map(referenced_title) else {
            continue;
        };
        if func_index.iter().any(|(f, _)| f.name == parent) {
            ingestor
                .ingest_edge(&function.name, &parent, EdgeType::FunctionExtends, None)
                .await
                .map_err(Error::Graph)?;
        }
    }

    // ── Bridge pass 0c: rules (issue #264). Deterministic order — rules
    // sorted by name within a domain. Rules do NOT extend (sigil has no
    // rule parent ref), so there are no cycle semantics to check.
    rule_index.sort_by(|(a, da), (b, db)| (da, &a.name).cmp(&(db, &b.name)));
    for (rule, domain) in &rule_index {
        let node = rule_node(rule, domain);
        ingestor.ingest_rule(&node).await.map_err(Error::Graph)?;
        stats.rules_ingested += 1;
        // Doc-reference edges come after the node exists (backends match
        // the owner by name).
        for doc in &rule.doc_references {
            emit_doc_reference_edges(
                ingestor,
                &RegulatoryOwner::Rule(node.name.clone()),
                doc,
                &reg_known,
                &mut stats,
            )
            .await?;
        }
    }

    // ── Bridge pass 1: codelist SchemaNodes + CodeLists + EnumValues.
    let mut bridged: HashSet<String> = HashSet::new();
    let mut bridged_enum_schema_ids: HashMap<String, String> = HashMap::new();
    let enum_by_name: HashMap<&str, &sigil_model::Enumeration> = enum_index
        .iter()
        .map(|(e, _, _)| (e.name.as_str(), *e))
        .collect();
    for (enumeration, domain, id) in &enum_index {
        if known_titles.contains(&enumeration.name) || bridged.contains(&enumeration.name) {
            continue;
        }
        let node = enum_schema_node(
            enumeration,
            domain,
            id,
            model_namespace(user_files, &enumeration.name),
            type_suffix,
        );
        ingestor.ingest_schema(&node).await.map_err(Error::Graph)?;
        bridged.insert(enumeration.name.clone());
        bridged_enum_schema_ids.insert(enumeration.name.clone(), id.clone());
        bridged_schema_ids.insert(enumeration.name.clone(), id.clone());
        stats.bridged_titles.push(enumeration.name.clone());
        stats.enum_schemas += 1;
        emit_schema_namespace_edge(
            ingestor,
            id,
            Some(model_namespace(user_files, &enumeration.name)),
        )
        .await?;

        let codelist = CodeList {
            name: enumeration.name.clone(),
            description: enumeration.definition.as_deref().map(sanitize_description),
            pg_table_name: to_snake_case(&strip_suffix(&enumeration.name, type_suffix)),
            render_as: "codelist".to_string(),
            check_expression: None,
        };
        ingestor
            .ingest_codelist(&codelist)
            .await
            .map_err(Error::Graph)?;
        stats.enums += 1;
        for value in merged_enum_values(enumeration, &enum_by_name) {
            let ev = EnumValue {
                value: value.name.clone(),
                display_name: value.display.clone(),
                sort_order: stats.enum_values as i32,
            };
            ingestor
                .ingest_enum_value(&enumeration.name, &ev)
                .await
                .map_err(Error::Graph)?;
            stats.enum_values += 1;
        }
    }

    // ── Bridge pass 2: SchemaNodes for types matching no existing schema.
    for (data, domain, id, namespace) in &data_index {
        if known_titles.contains(&data.name) || bridged.contains(&data.name) {
            continue;
        }
        let node = data_schema_node(data, domain, id, namespace, type_suffix);
        ingestor.ingest_schema(&node).await.map_err(Error::Graph)?;
        bridged.insert(data.name.clone());
        bridged_schema_ids.insert(data.name.clone(), id.clone());
        stats.bridged_titles.push(data.name.clone());
        emit_schema_namespace_edge(ingestor, id, Some(*namespace)).await?;
        if data.is_choice {
            stats.choices += 1;
        }
        stats.conditions_recorded += data.conditions.len();

        // Named conditions land as ConditionNodes (#261). The
        // `rosetta_conditions` annotation payload on the SchemaNode stays
        // untouched — it remains the provenance snapshot pinned by #259.
        for (idx, condition) in data.conditions.iter().enumerate() {
            let name = condition
                .name
                .clone()
                .unwrap_or_else(|| format!("{}_condition_{}", data.name, idx));
            let expr_json =
                serde_json::to_string(&condition.expression.to_json()).map_err(|e| {
                    Error::RosettaModel {
                        file: data.name.clone(),
                        reason: format!("condition '{name}' expression serialization failed: {e}"),
                    }
                })?;
            let condition_node = ConditionNode {
                name,
                owner_title: data.name.clone(),
                kind: ConditionKind::Condition,
                expr_json: Some(expr_json),
                options: Vec::new(),
                definition: condition.definition.clone(),
                domain: Some(domain.clone()),
            };
            ingestor
                .ingest_condition(&condition_node)
                .await
                .map_err(Error::Graph)?;
            stats.conditions_ingested += 1;

            // The condition's own `[docReference ...]` metadata →
            // RegulatoryReference edges (issue #265).
            for doc in &condition.doc_references {
                emit_doc_reference_edges(
                    ingestor,
                    &RegulatoryOwner::Condition(condition_node.name.clone()),
                    doc,
                    &reg_known,
                    &mut stats,
                )
                .await?;
            }
        }

        // Choices derive ONE one_of node whose options are the referenced
        // titles of the `(0..1)` option attributes (declaration order,
        // deduplicated). one_of is not a sigil Expr kind, so `expr_json`
        // stays empty — transpilation is #262.
        if data.is_choice {
            let mut seen: HashSet<String> = HashSet::new();
            let options: Vec<String> = data
                .attributes
                .iter()
                .map(|attribute| referenced_title(&attribute.type_ref))
                .filter(|title| seen.insert(title.clone()))
                .collect();
            let one_of = ConditionNode {
                name: format!("{}_one_of", data.name),
                owner_title: data.name.clone(),
                kind: ConditionKind::OneOf,
                expr_json: None,
                options,
                definition: None,
                domain: Some(domain.clone()),
            };
            ingestor
                .ingest_condition(&one_of)
                .await
                .map_err(Error::Graph)?;
            stats.one_of_ingested += 1;
        }

        stats.types += 1;
    }

    // ── Bridge pass 2b: rule attachment edges (issue #264). Written after
    // the schema nodes exist — `ingest_edge` MATCHes the endpoints at
    // execute time.
    //
    // RuleAppliesTo (Rule → Schema): the `from TypeCall` input, matched by
    // bare title against schemas pre-existing in the graph or bridged this
    // run. An input naming no schema is a documented no-op (the rule node
    // keeps its `input_type` name either way — mirroring FunctionExtends,
    // where only in-run parents link).
    for (rule, _) in &rule_index {
        let Some(input) = rule.input.as_ref().map(referenced_title) else {
            continue;
        };
        if known_titles.contains(&input) || bridged.contains(&input) {
            ingestor
                .ingest_edge(&rule.name, &input, EdgeType::RuleAppliesTo, None)
                .await
                .map_err(Error::Graph)?;
            stats.rule_applies_to += 1;
        }
    }
    // RuleReference (Schema → Rule): `[ruleReference R]` entries on rule
    // source class attributes, promoted to real edges when R names a rule
    // of this run. Unresolvable references stay documented skips (counted,
    // never silent).
    for (source, _) in &reg_rule_sources {
        for class in &source.classes {
            let class_title = referenced_title(&class.data);
            for attribute in &class.attributes {
                for reference in &attribute.rule_references {
                    let Some(name) = reference.rule.as_deref().map(referenced_title_str) else {
                        continue;
                    };
                    if rule_index.iter().any(|(rule, _)| rule.name == name) {
                        ingestor
                            .ingest_edge(
                                &class_title,
                                &name,
                                EdgeType::RuleReference,
                                Some(&EdgeProperties {
                                    ref_path: Some(attribute.attribute.clone()),
                                    rule_source: Some(source.name.clone()),
                                    ..Default::default()
                                }),
                            )
                            .await
                            .map_err(Error::Graph)?;
                        stats.rule_reference_edges += 1;
                    } else {
                        stats.rule_reference_skips += 1;
                        eprintln!(
                            "Warning: rule source '{}' references rule '{}' (class {}, attribute \
                             '{}') — no such rule in this run; skipping the binding edge",
                            source.name, name, class_title, attribute.attribute
                        );
                    }
                }
            }
        }
    }

    // ── Bridge pass 3: properties + reference/extends edges per type, in
    // the JSON path's allOf-canonical order (ancestors first, each group
    // name-sorted, first occurrence wins — gap-analysis finding 1).
    let data_by_name: HashMap<&str, &sigil_model::Data> = data_index
        .iter()
        .map(|(d, _, _, _)| (d.name.as_str(), *d))
        .collect();
    for (data, _, id, _) in &data_index {
        if !bridged.contains(&data.name) {
            continue;
        }
        for attribute in ordered_bridge_attributes(data, &data_by_name) {
            let Some(prop) = attribute_property(
                attribute,
                &data.name,
                &enum_titles,
                &type_titles,
                &alias_types,
                &mut stats,
            ) else {
                continue;
            };
            ingestor
                .ingest_property(&data.name, id, &prop)
                .await
                .map_err(Error::Graph)?;
            stats.properties += 1;

            // Reference edges to bridged/existing schema nodes (the JSON
            // path's $ref shape). Builtin-typed attributes keep the
            // ref_target only — builtins have no Schema node.
            let target_name = referenced_title(&attribute.type_ref);
            let target_schema_id = json_schema_ids
                .get(&target_name)
                .cloned()
                .or_else(|| bridged_schema_ids.get(&target_name).cloned());
            if let Some(target_schema_id) = target_schema_id {
                let edge_type = if prop.is_array {
                    EdgeType::ItemsOf
                } else {
                    EdgeType::ReferencesSchema
                };
                ingestor
                    .ingest_edge(
                        &format!("{}::{}", prop.name, data.name),
                        &target_schema_id,
                        edge_type,
                        Some(&EdgeProperties {
                            ref_path: Some(target_name.clone()),
                            ..Default::default()
                        }),
                    )
                    .await
                    .map_err(Error::Graph)?;
                stats.edges += 1;
            }
        }

        // `extends` → ExtendsSchema edge (allOf composition), mirroring
        // Pass 4 of the JSON path.
        if let Some(parent) = &data.super_type {
            let parent_name = referenced_title(parent);
            let parent_exists =
                known_titles.contains(&parent_name) || bridged.contains(&parent_name);
            if parent_exists {
                ingestor
                    .ingest_edge(
                        &data.name,
                        &parent_name,
                        EdgeType::ExtendsSchema,
                        Some(&EdgeProperties {
                            composition_type: Some("allOf".to_string()),
                            ..Default::default()
                        }),
                    )
                    .await
                    .map_err(Error::Graph)?;
                stats.extends += 1;
                stats.edges += 1;
            }
        }

        // The type's `[docReference ...]` metadata → RegulatoryReference
        // edges (issue #265). Attribute-level doc references stay in the
        // SchemaNode's `rosetta_attribute_annotations` payload (attributes
        // are not nodes).
        for doc in &data.doc_references {
            emit_doc_reference_edges(
                ingestor,
                &RegulatoryOwner::Schema(data.name.clone()),
                doc,
                &reg_known,
                &mut stats,
            )
            .await?;
        }
    }

    // ── Bridge pass 4: regulatory structural edges (issue #265).
    // Best-effort: a target no declared user-file element backs is skipped
    // (sigil passes doc references through unvalidated, and backends match
    // by name + kind).
    for (corpus, _) in &reg_corpora {
        if let Some(parent) = &corpus.body {
            link_regulatory(
                ingestor,
                &RegulatoryOwner::Regulatory {
                    name: corpus.name.clone(),
                    kind: RegulatoryKind::Corpus,
                },
                &referenced_title_str(parent),
                RegulatoryKind::Body,
                RegulatoryEdgeKind::CorpusInBody,
                None,
                &reg_known,
                &mut stats,
            )
            .await?;
        }
    }
    for (source, _) in &reg_rule_sources {
        if let Some(super_source) = &source.super_source {
            link_regulatory(
                ingestor,
                &RegulatoryOwner::Regulatory {
                    name: source.name.clone(),
                    kind: RegulatoryKind::RuleSource,
                },
                &referenced_title(super_source),
                RegulatoryKind::RuleSource,
                RegulatoryEdgeKind::DerivesFrom,
                None,
                &reg_known,
                &mut stats,
            )
            .await?;
        }
    }
    for (report, _, name) in &reg_reports {
        let owner = RegulatoryOwner::Regulatory {
            name: name.clone(),
            kind: RegulatoryKind::Report,
        };
        link_regulatory(
            ingestor,
            &owner,
            &named_ref_title(&report.regulatory.body),
            RegulatoryKind::Body,
            RegulatoryEdgeKind::Reference,
            None,
            &reg_known,
            &mut stats,
        )
        .await?;
        for corpus in &report.regulatory.corpora {
            link_regulatory(
                ingestor,
                &owner,
                &named_ref_title(corpus),
                RegulatoryKind::Corpus,
                RegulatoryEdgeKind::Reference,
                None,
                &reg_known,
                &mut stats,
            )
            .await?;
        }
        for segment in &report.regulatory.segments {
            link_regulatory(
                ingestor,
                &owner,
                &named_ref_title(&segment.segment),
                RegulatoryKind::Segment,
                RegulatoryEdgeKind::Reference,
                Some(&segment.reference),
                &reg_known,
                &mut stats,
            )
            .await?;
        }
        if let Some(rule_source) = &report.rule_source {
            link_regulatory(
                ingestor,
                &owner,
                &named_ref_title(rule_source),
                RegulatoryKind::RuleSource,
                RegulatoryEdgeKind::RuleSource,
                None,
                &reg_known,
                &mut stats,
            )
            .await?;
        }
    }

    if stats.needs_review != 0 {
        eprintln!(
            "Warning: rosetta model has {} out-of-data-plane element(s) recorded as \
             needs_review (annotation declarations, basic types, aliases, library \
             functions): {}",
            stats.needs_review,
            stats.needs_review_names.join(", ")
        );
    }

    Ok(RosettaIngestOutcome { stats })
}

/// Emit the `RegulatoryReference` edges for one `[docReference ...]`
/// payload: owner → body, owner → each corpus (carrying the provision as
/// the ref path), owner → each segment (carrying the segment reference).
async fn emit_doc_reference_edges(
    ingestor: &dyn GraphIngestor,
    owner: &RegulatoryOwner,
    doc: &sigil_model::DocReference,
    reg_known: &HashSet<(String, String)>,
    stats: &mut RosettaIngestStats,
) -> crate::error::Result<()> {
    let mut edges: Vec<(String, RegulatoryKind, Option<String>)> = vec![(
        referenced_title_str(&doc.body),
        RegulatoryKind::Body,
        doc.provision.clone(),
    )];
    for corpus in &doc.corpora {
        edges.push((
            referenced_title_str(corpus),
            RegulatoryKind::Corpus,
            doc.provision.clone(),
        ));
    }
    for (segment, reference) in &doc.segments {
        edges.push((
            referenced_title_str(segment),
            RegulatoryKind::Segment,
            Some(reference.clone()),
        ));
    }
    for (target, target_kind, ref_path) in edges {
        link_regulatory(
            ingestor,
            owner,
            &target,
            target_kind,
            RegulatoryEdgeKind::Reference,
            ref_path.as_deref(),
            reg_known,
            stats,
        )
        .await?;
    }
    Ok(())
}

/// Link an owner to a regulatory node when the target was ingested this
/// run, counting the edge in the stats. Best-effort: a target no declared
/// user-file element backs is skipped (sigil does not validate
/// doc-reference targets either).
#[allow(clippy::too_many_arguments)]
async fn link_regulatory(
    ingestor: &dyn GraphIngestor,
    owner: &RegulatoryOwner,
    target: &str,
    target_kind: RegulatoryKind,
    edge_kind: RegulatoryEdgeKind,
    ref_path: Option<&str>,
    reg_known: &HashSet<(String, String)>,
    stats: &mut RosettaIngestStats,
) -> crate::error::Result<()> {
    if !reg_known.contains(&(target_kind.as_str().to_string(), target.to_string())) {
        return Ok(());
    }
    ingestor
        .ingest_regulatory_reference(owner, target, target_kind, edge_kind, ref_path)
        .await
        .map_err(Error::Graph)?;
    stats.regulatory_edges += 1;
    Ok(())
}
