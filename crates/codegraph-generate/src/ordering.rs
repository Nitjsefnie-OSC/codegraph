//! Deterministic generation ordering (issue #332).
//!
//! `compute_generation_order` is the total order every per-entity generator
//! runs in and the sequence migration numbers are allocated against. Its
//! determinism contract, and the audit of every input that feeds it:
//!
//! * **Domain order** — `DomainRegistry::topological_order` (Kahn; the
//!   registry assigns node indices in sorted-name order, and every frontier
//!   batch is sorted, so tie-breaks are lexicographic regardless of the
//!   `config.domains` `HashMap`).
//! * **Entity order within a domain** — collected into a `BTreeSet`
//!   (config entities ∪ graph-discovered, minus exclude/force-VO), then with
//!   namespaces in the graph ordered by `namespace_generation_order` rank
//!   (imported-before-importer), title as tie-break. No in-degree signal
//!   participates in ordering; the Kahn pass below only orders FK
//!   dependencies and sorts every frontier and cycle fallback by title.
//! * **Graph reads** — `list_schemas` sorts by domain (grafeo
//!   `query_schemas`); duplicate titles make `title_namespace` last-wins over
//!   a stable scan order (grafeo's FxHashMap-backed storage iterates
//!   seed-fixed for identical insert sequences — ingestion is sequential and
//!   input-file-ordered). `list_all_schema_references` (grafeo boundary
//!   total order by (source, target), issue #452) only feeds
//!   membership/adjacency whose selection is re-sorted by title.
//! * **Consumers** — `all_domains_for_generation` appends entity-less
//!   custom-routes domains in sorted-name order; `pipeline::write_entity_results`
//!   allocates migration sequence numbers strictly in this Vec's order;
//!   `pipeline::run_codelist_generators` sorts codelists by name before the
//!   `CODELIST_START + idx` allocation. `parent_candidates` (grafeo boundary
//!   total order by (child, parent, field), issue #452) feed nested-route
//!   content only through per-title lookups.
//!
//! Any new consumer of this module (or new graph read feeding emission) must
//! sort at the emission boundary — never let a `HashMap`/`HashSet`/`DashMap`
//! iteration order reach a file path, a migration number, or emitted content.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use codegraph_config::DomainConfig;
use codegraph_core::traits::GraphQuerier;

use crate::error::{Error, Result};
use crate::project_config::GenerationEntry;

/// Group generation entries by domain, preserving order within each domain.
fn group_by_domain(entries: &[GenerationEntry]) -> Vec<(String, Vec<String>)> {
    let mut seen = HashSet::new();
    let mut domain_entities: HashMap<String, Vec<String>> = HashMap::new();
    let mut domain_order = Vec::new();

    let mut seen_entity_per_domain: HashSet<(String, String)> = HashSet::new();
    for entry in entries {
        if seen.insert(entry.domain.clone()) {
            domain_order.push(entry.domain.clone());
        }
        // Deduplicate entity titles within each domain
        if seen_entity_per_domain.insert((entry.domain.clone(), entry.schema_title.clone())) {
            domain_entities
                .entry(entry.domain.clone())
                .or_default()
                .push(entry.schema_title.clone());
        }
    }

    domain_order
        .into_iter()
        .map(|d| {
            let entities = domain_entities.remove(&d).unwrap_or_default();
            (d, entities)
        })
        .collect()
}

/// Domains that receive domain-level generation (router/errors/links): every
/// domain with entities in the generation order, plus entity-less domains
/// flagged `custom_routes` (zero entities — only the domain scaffold).
///
/// Entity-less custom-routes domains get the per-domain router scaffold and,
/// in workers topology, a per-domain worker crate + gateway upstream, even
/// though they have no entities to route.
///
/// Determinism (issue #332): `config.domains` is a `HashMap`, so the
/// custom-routes tail is appended in sorted-name order — a plain iteration
/// would make the (domain, entities) pair order — and with it the domain
/// phase's emission order and the generation report — vary per process.
pub fn all_domains_for_generation(
    config: &DomainConfig,
    entries: &[GenerationEntry],
) -> Vec<(String, Vec<String>)> {
    let mut result = group_by_domain(entries);
    let present: HashSet<String> = result.iter().map(|(d, _)| d.clone()).collect();
    let mut custom_routes: Vec<String> = config
        .domains
        .iter()
        .filter(|(name, entry)| entry.custom_routes && !present.contains(*name))
        .map(|(name, _)| name.clone())
        .collect();
    custom_routes.sort();
    result.extend(custom_routes.into_iter().map(|name| (name, Vec::new())));
    result
}

/// Compute the generation order using per-domain schema listing + domain order.
///
/// Previous approach used `get_entity_names()` (which deduplicates titles) then
/// `get_schema(title)` (which returns only one domain).  This lost entities whose
/// title appears in multiple domains (e.g. "OrderType" in both screening and
/// assessments).  We now use `list_schemas` per-domain so each domain gets its
/// own entry for shared titles.
pub async fn compute_generation_order(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
) -> Result<Vec<GenerationEntry>> {
    // Build domain order from config
    let domain_config_registry = codegraph_config::DomainRegistry::from_config(config.clone())
        .map_err(|e| Error::Config(e.to_string()))?;
    let domain_order = domain_config_registry
        .topological_order()
        .map_err(|e| Error::Config(e.to_string()))?;

    let domain_rank: HashMap<String, usize> = domain_order
        .iter()
        .enumerate()
        .map(|(i, d)| (d.clone(), i))
        .collect();

    // Get all schemas from the graph, grouped by domain.
    let all_schemas = db
        .list_schemas(None)
        .await
        .map_err(|e| Error::Config(e.to_string()))?;

    // Namespace plane (issue #268): when the graph carries namespaces, the
    // per-domain emission order respects `namespace_generation_order`
    // (imported-before-importer) and title claiming is scoped per
    // namespace. Namespace-less graphs skip the namespace queries'
    // effects entirely — order and claiming stay byte-identical.
    let has_namespaces = !db
        .list_namespaces()
        .await
        .map_err(|e| Error::Config(e.to_string()))?
        .is_empty();
    let ns_rank: HashMap<String, usize> = if has_namespaces {
        db.namespace_generation_order()
            .await
            .map_err(|e| Error::Config(e.to_string()))? // cycles are a hard error
            .into_iter()
            .enumerate()
            .map(|(i, n)| (n, i))
            .collect()
    } else {
        HashMap::new()
    };
    // Title → namespace of its schema (first wins; duplicates share one
    // claim below unless namespaces scope them apart).
    let title_namespace: HashMap<&str, Option<&str>> = all_schemas
        .iter()
        .map(|s| (s.title.as_str(), s.namespace.as_deref()))
        .collect();

    // Build a set of entity titles present in each domain's graph data.
    // We include all schemas that have a pg_table_name (meaning they produce
    // entity .rs files), not just is_entity=true schemas. This ensures that
    // ValueObject, CompositeWrapper, and other non-root types referenced by
    // DTO/repository code as crate::entity::<module>:: actually have files.
    // Inline/local definitions (parent_schema.is_some()) are excluded since
    // they are generated recursively as child entities from their parent.
    let mut graph_entities_by_domain: HashMap<String, HashSet<String>> = HashMap::new();
    let mut all_entity_titles: HashSet<String> = HashSet::new();
    for schema in &all_schemas {
        if schema.pg_table_name.is_empty() {
            continue;
        }
        if schema.parent_schema.is_some() {
            continue;
        }
        all_entity_titles.insert(schema.title.clone());
        let domain = schema.domain.as_deref().unwrap_or("");
        if domain.is_empty() || !domain_rank.contains_key(domain) {
            continue;
        }
        graph_entities_by_domain
            .entry(domain.to_string())
            .or_default()
            .insert(schema.title.clone());
    }

    let mut entries = Vec::new();
    let mut seen_entries = HashSet::new();
    // Title-claim key: `(namespace, title)` — with namespaces in the graph,
    // the same title in two namespaces is two types (issue #268, the
    // `disambiguate_schema_ids` scoping); the namespace component is empty
    // for namespace-less schemas AND for namespace-less graphs, so
    // keying is inert there (byte-identical back-compat).
    let mut seen_titles: HashSet<(String, String)> = HashSet::new();

    // Track which domain currently claims each title, plus whether that claim
    // came from an explicit config `entities` entry or just graph discovery.
    // When a title is claimed by a domain that only discovered it via the graph
    // (cross-domain allOf reference) and a later domain explicitly configures it,
    // the entry is reassigned to the configured domain. This keeps entity
    // generators running each title exactly once (no duplicate DDL) while
    // per-domain generators (openapi, CLI, links) attribute the entity to the
    // domain that actually owns it.
    let mut title_claim_domain: HashMap<(String, String), String> = HashMap::new();
    let mut title_claim_explicit: HashMap<(String, String), bool> = HashMap::new();

    // The claim key for a title: its schema's namespace (or "" when
    // namespace-less / unknown).
    let claim_key = |title: &str| -> (String, String) {
        (
            title_namespace
                .get(title)
                .copied()
                .flatten()
                .unwrap_or("")
                .to_string(),
            title.to_string(),
        )
    };

    for domain_name in &domain_order {
        let domain_entry = match config.domains.get(domain_name.as_str()) {
            Some(e) => e,
            None => continue,
        };

        let exclude: HashSet<&str> = domain_entry.exclude.iter().map(|s| s.as_str()).collect();
        let force_vo: HashSet<&str> = domain_entry
            .force_value_objects
            .iter()
            .map(|s| s.as_str())
            .collect();

        let graph_entities = graph_entities_by_domain
            .get(domain_name.as_str())
            .cloned()
            .unwrap_or_default();

        // Collect titles: explicitly configured entities + graph-discovered entities.
        // Explicitly configured entities are honored even when the schema
        // physically lives in another domain (cross-domain allOf references) —
        // the graph check only guards against config entries whose schema does
        // not exist at all.
        let mut domain_titles: BTreeSet<String> = BTreeSet::new();
        for title in &domain_entry.entities {
            if all_entity_titles.contains(title.as_str()) {
                domain_titles.insert(title.clone());
            }
        }
        for title in &graph_entities {
            if !exclude.contains(title.as_str()) && !force_vo.contains(title.as_str()) {
                domain_titles.insert(title.clone());
            }
        }

        // Emission order: BTreeSet's title sort when the graph has no
        // namespaces (byte-identical); with namespaces, entries are
        // ordered by their namespace's position in
        // `namespace_generation_order` (imported-before-importer;
        // namespace-less titles last), title as the tie-break.
        let mut ordered_titles: Vec<String> = domain_titles.into_iter().collect();
        if has_namespaces {
            ordered_titles.sort_by(|a, b| {
                let ra = title_namespace
                    .get(a.as_str())
                    .copied()
                    .flatten()
                    .and_then(|ns| ns_rank.get(ns).copied())
                    .unwrap_or(usize::MAX);
                let rb = title_namespace
                    .get(b.as_str())
                    .copied()
                    .flatten()
                    .and_then(|ns| ns_rank.get(ns).copied())
                    .unwrap_or(usize::MAX);
                (ra, a.as_str()).cmp(&(rb, b.as_str()))
            });
        }

        for title in &ordered_titles {
            // Skip excluded or force-VO types for this domain
            if exclude.contains(title.as_str()) || force_vo.contains(title.as_str()) {
                continue;
            }
            if !seen_entries.insert((title.clone(), domain_name.clone())) {
                continue;
            }
            let explicitly_configured = domain_entry.entities.iter().any(|e| e == title);
            let key = claim_key(title);
            match seen_titles.get(&key) {
                None => {
                    // First claim.
                    seen_titles.insert(key.clone());
                    title_claim_domain.insert(key.clone(), domain_name.clone());
                    title_claim_explicit.insert(key.clone(), explicitly_configured);
                    entries.push(GenerationEntry {
                        schema_title: title.clone(),
                        domain: domain_name.clone(),
                        pg_schema: domain_name.clone(),
                        is_cyclic: false,
                    });
                }
                Some(_) => {
                    // Already claimed. Reassign to this domain only if this domain
                    // explicitly configures the title and the current claim came
                    // from graph discovery alone (cross-domain reference).
                    let claiming_domain = title_claim_domain.get(&key).cloned().unwrap_or_default();
                    let claim_was_explicit =
                        title_claim_explicit.get(&key).copied().unwrap_or(false);
                    if explicitly_configured && !claim_was_explicit {
                        // Remove the discovery-only claim, replace with the
                        // configured domain's entry.
                        if let Some(pos) = entries
                            .iter()
                            .position(|e| e.schema_title == *title && e.domain == claiming_domain)
                        {
                            entries.remove(pos);
                        }
                        seen_titles.insert(key.clone());
                        title_claim_domain.insert(key.clone(), domain_name.clone());
                        title_claim_explicit.insert(key.clone(), true);
                        entries.push(GenerationEntry {
                            schema_title: title.clone(),
                            domain: domain_name.clone(),
                            pg_schema: domain_name.clone(),
                            is_cyclic: false,
                        });
                    }
                }
            }
        }
    }

    // Group entries by domain for intra-domain topological sorting
    let mut domain_groups: HashMap<String, Vec<GenerationEntry>> = HashMap::new();
    for entry in entries {
        domain_groups
            .entry(entry.domain.clone())
            .or_default()
            .push(entry);
    }

    // Bulk-fetch all schema→schema reference edges once, instead of N individual
    // get_referenced_schemas() calls. Build a lookup map for intra-domain sorting.
    let all_refs = db
        .list_all_schema_references()
        .await
        .map_err(|e| Error::Config(e.to_string()))?;
    let mut all_refs_map: HashMap<String, Vec<String>> = HashMap::new();
    for (src, tgt) in &all_refs {
        all_refs_map
            .entry(src.clone())
            .or_default()
            .push(tgt.clone());
    }

    // Build a set of all entity titles for transitive dependency resolution.
    let entity_titles: HashSet<String> = all_schemas
        .iter()
        .filter(|s| s.is_entity)
        .map(|s| s.title.clone())
        .collect();

    // Compute transitive entity dependencies through value-object schemas.
    // When entity A references VO V, and V references entity B, the DDL
    // generator emits a FK column on A's child table pointing to B. This
    // means B's migration must precede A's. Discover these transitive deps
    // by following reference chains through non-entity (VO) schemas.
    let mut transitive_entity_deps: HashMap<String, HashSet<String>> = HashMap::new();
    for entity_title in &entity_titles {
        let mut visited: HashSet<String> = HashSet::new();
        let mut stack: Vec<String> = Vec::new();
        // Seed with the entity's direct references
        if let Some(direct_refs) = all_refs_map.get(entity_title) {
            for r in direct_refs {
                stack.push(r.clone());
            }
        }
        while let Some(current) = stack.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            if entity_titles.contains(&current) && current != *entity_title {
                // Found an entity dependency (direct or transitive)
                transitive_entity_deps
                    .entry(entity_title.clone())
                    .or_default()
                    .insert(current.clone());
            } else if !entity_titles.contains(&current) {
                // Non-entity (value object): follow its references further
                if let Some(vo_refs) = all_refs_map.get(&current) {
                    for r in vo_refs {
                        if !visited.contains(r) {
                            stack.push(r.clone());
                        }
                    }
                }
            }
        }
    }

    // Merge transitive deps into all_refs_map so the topological sort picks them up.
    for (entity, deps) in &transitive_entity_deps {
        let entry = all_refs_map.entry(entity.clone()).or_default();
        for dep in deps {
            if !entry.contains(dep) {
                entry.push(dep.clone());
            }
        }
    }

    // For each domain, build an entity-level dependency graph and sort topologically.
    // This ensures that FK-target entities appear before entities that reference them.
    let mut sorted_entries = Vec::new();
    for domain_name in &domain_order {
        let group = match domain_groups.remove(domain_name) {
            Some(g) => g,
            None => continue,
        };

        // Build a title→index map for entities in this domain
        let title_to_idx: HashMap<String, usize> = group
            .iter()
            .enumerate()
            .map(|(i, e)| (e.schema_title.clone(), i))
            .collect();

        // Build adjacency (dependency edges) using pre-fetched reference map
        let mut in_degree = vec![0usize; group.len()];
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); group.len()];

        for (idx, entry) in group.iter().enumerate() {
            let empty = Vec::new();
            let refs = all_refs_map.get(&entry.schema_title).unwrap_or(&empty);
            for ref_title in refs {
                // Only consider intra-domain dependencies; cross-domain deps
                // are already handled by domain-level topological ordering.
                if let Some(&dep_idx) = title_to_idx.get(ref_title)
                    && dep_idx != idx
                {
                    dependents[dep_idx].push(idx);
                    in_degree[idx] += 1;
                }
            }
        }

        // Kahn's algorithm — seed queue with zero-in-degree entities, sorted
        // alphabetically for deterministic output.
        let mut queue: VecDeque<usize> = {
            let mut zeros: Vec<usize> = in_degree
                .iter()
                .enumerate()
                .filter(|(_, deg)| **deg == 0)
                .map(|(i, _)| i)
                .collect();
            zeros.sort_by(|a, b| group[*a].schema_title.cmp(&group[*b].schema_title));
            zeros.into_iter().collect()
        };

        let mut topo_order: Vec<usize> = Vec::with_capacity(group.len());
        while let Some(current) = queue.pop_front() {
            topo_order.push(current);
            // Sort dependents alphabetically before enqueuing for determinism
            let mut next: Vec<usize> = Vec::new();
            for &dep in &dependents[current] {
                in_degree[dep] -= 1;
                if in_degree[dep] == 0 {
                    next.push(dep);
                }
            }
            next.sort_by(|a, b| group[*a].schema_title.cmp(&group[*b].schema_title));
            queue.extend(next);
        }

        // Append any remaining entities involved in cycles (alphabetical fallback)
        if topo_order.len() < group.len() {
            let in_topo: HashSet<usize> = topo_order.iter().copied().collect();
            let mut remaining: Vec<usize> =
                (0..group.len()).filter(|i| !in_topo.contains(i)).collect();
            remaining.sort_by(|a, b| group[*a].schema_title.cmp(&group[*b].schema_title));
            topo_order.extend(remaining);
        }

        // Emit entries in topological order
        for idx in topo_order {
            sorted_entries.push(group[idx].clone());
        }
    }

    // Module-collision diagnostics: two titles in one domain whose stripped
    // names land on the same module (e.g. HR Open "Type"-suffix stripping
    // turning a `LifeEventType` enum and a `LifeEvent` class both into
    // `life_event`) generate duplicate handlers/openapi configs under one
    // module — a broken generated app with no hint as to why. This is only
    // fatal when both titles produce entities, which ordering cannot see
    // (generator selection lives in the profile), so warn loudly instead of
    // erroring: fixtures legitimately carry such pairs with no generators
    // enabled.
    {
        let mut by_domain_module: HashMap<(String, String), Vec<&str>> = HashMap::new();
        for e in &sorted_entries {
            let module =
                codegraph_naming::to_snake_case(&config.defaults.strip_suffix(&e.schema_title));
            by_domain_module
                .entry((e.domain.clone(), module))
                .or_default()
                .push(e.schema_title.as_str());
        }
        let mut collisions: Vec<String> = by_domain_module
            .iter()
            .filter(|(_, titles)| titles.len() > 1)
            .map(|((domain, module), titles)| {
                format!(
                    "domain `{domain}`: titles {titles:?} all map to module `{module}` (rename all but one so their stripped names differ)"
                )
            })
            .collect();
        collisions.sort();
        for collision in &collisions {
            tracing::warn!("module name collision in generation order: {collision}");
        }
    }

    Ok(sorted_entries)
}
