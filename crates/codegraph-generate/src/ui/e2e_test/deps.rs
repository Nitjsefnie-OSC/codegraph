use std::collections::HashSet;

use codegraph_config::DomainConfig;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::{PropertyNode, SchemaNode};

use super::common::collect_ui_fields;
use super::context::{CycleBreakCtx, DependencyStep, EntityRefDep};
use super::fixtures::{build_test_data_json, test_value_for_field};
use super::page::UiField;
use super::refs::{
    api_path_for_schema, apply_convention_refs, find_ref_property, resolve_ref_schema,
    unique_dep_id,
};
/// A discovered dependency node, keyed by `SchemaNode::schema_id`.
struct DepNode {
    title: String,
    dep_id: String,
    api_path: String,
    fields_json: String,
    is_array: bool,
    /// No `create` operation — POST will 405; the step is emitted tolerantly.
    optional: bool,
    /// `(fk_field_name, child_key)` for each required entity ref.
    children: Vec<(String, String)>,
}

/// Build the required-only transitive entity-ref closure for the main entity.
///
/// Returns `(entity_ref_deps, dependency_steps, errors)`:
/// - `entity_ref_deps` maps the main entity's required refs to `depIds` keys.
/// - `dependency_steps` is ordered leaf-first so every FK is created first.
/// - `errors` holds actionable messages for unsatisfiable required deps.
///
/// Required-FK cycles are NOT errors: they are broken at the cycle's
/// last-discovered leg (see `post_order_visit`), which is omitted from the
/// break-point step's create body. Self-references are always omitted —
/// a row cannot depend on itself.
pub(super) async fn build_required_dependencies(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    main_title: &str,
    main_domain: &str,
    main_props: &[PropertyNode],
    create_fields: &[UiField],
) -> (Vec<EntityRefDep>, Vec<DependencyStep>, Vec<String>) {
    let mut assigned: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut used: HashSet<String> = HashSet::new();
    let mut nodes: std::collections::HashMap<String, DepNode> = std::collections::HashMap::new();
    let mut node_order: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut entity_ref_deps: Vec<EntityRefDep> = Vec::new();

    // Seed the worklist from the main entity's required entity-ref fields.
    let mut queue: std::collections::VecDeque<(SchemaNode, String, bool)> =
        std::collections::VecDeque::new();
    for field in create_fields
        .iter()
        .filter(|f| f.is_entity_ref && f.is_required)
    {
        let Some(prop) = find_ref_property(main_props, &field.name) else {
            errors.push(format!(
                "required entity reference '{}' on {} could not be matched to a schema property",
                field.name, main_title
            ));
            continue;
        };
        match resolve_ref_schema(db, prop, main_title, Some(main_domain)).await {
            Some(target) => {
                let key = target.schema_id.clone();
                let dep_id = if let Some(existing) = assigned.get(&key) {
                    existing.clone()
                } else {
                    let id = unique_dep_id(&field.name, &mut used);
                    assigned.insert(key.clone(), id.clone());
                    id
                };
                entity_ref_deps.push(EntityRefDep {
                    field_name: field.name.clone(),
                    dep_id: dep_id.clone(),
                    api_path: api_path_for_schema(&target, config),
                    is_array: field.is_array,
                });
                // Self-reference (e.g. Job.job_id → Job): do not recurse — the
                // fixture root create omits the leg. The conditional `testData`
                // spread stays, so the FK is still set whenever the target is
                // created transitively as someone else's dependency.
                let target_is_main = target.title == main_title
                    && target.domain.as_deref().unwrap_or(main_domain) == main_domain;
                if !target_is_main {
                    queue.push_back((target, dep_id, field.is_array));
                }
            }
            None => errors.push(format!(
                "required entity reference '{}' on {} has no resolvable target schema",
                field.name, main_title
            )),
        }
    }

    // Discover the closure breadth-first; leaf-first ordering happens below.
    while let Some((target, dep_id, is_array)) = queue.pop_front() {
        let key = target.schema_id.clone();
        if nodes.contains_key(&key) {
            continue;
        }
        let target_domain = target
            .domain
            .clone()
            .unwrap_or_else(|| main_domain.to_string());
        let api_path = api_path_for_schema(&target, config);
        let fields_json =
            build_test_data_json(db, &target.title, Some(&target_domain), config).await;
        let props = db
            .get_properties_in_domain(&target.title, &target_domain)
            .await
            .unwrap_or_default();
        let dep_fields = collect_ui_fields(db, &target.title, &[], Some(&target_domain), config)
            .await
            .unwrap_or_default();

        // Required plain-uuid FK columns on the dependency (e.g. `party.case_id`
        // when creating a `Claim` whose closure pulls in `Party`) must also be
        // resolved by convention so the transitive closure stays complete.
        // Every resolved target stays marked here regardless of creatability:
        // targets without a create route become tolerant `optional` dep steps.
        let mut dep_fields = dep_fields;
        apply_convention_refs(db, config, &target_domain, &props, &mut dep_fields).await;

        // Required entity refs on this dep become child steps.
        let mut children: Vec<(String, String)> = Vec::new();
        for rf in dep_fields
            .iter()
            .filter(|f| f.is_entity_ref && f.is_required)
        {
            let Some(prop) = find_ref_property(&props, &rf.name) else {
                errors.push(format!(
                    "required entity reference '{}' on {} could not be matched to a schema property",
                    rf.name, target.title
                ));
                continue;
            };
            match resolve_ref_schema(db, prop, &target.title, Some(&target_domain)).await {
                Some(child_target) => {
                    let child_key = child_target.schema_id.clone();
                    // Self-reference (X.x_id → X): omit the leg — the row
                    // cannot depend on itself; the create proceeds without it.
                    if child_key == key {
                        continue;
                    }
                    let child_dep_id = if let Some(existing) = assigned.get(&child_key) {
                        existing.clone()
                    } else {
                        let id = unique_dep_id(&rf.name, &mut used);
                        assigned.insert(child_key.clone(), id.clone());
                        id
                    };
                    children.push((rf.name.clone(), child_key.clone()));
                    queue.push_back((child_target, child_dep_id, rf.is_array));
                }
                None => errors.push(format!(
                    "required entity reference '{}' on {} has no resolvable target schema",
                    rf.name, target.title
                )),
            }
        }

        // Required scalar fields that cannot be populated make the dep unsatisfiable.
        for f in dep_fields.iter().filter(|f| f.is_required) {
            if f.is_entity_ref || f.nested_type_name.is_some() || f.name == "id" {
                continue;
            }
            if test_value_for_field(f).is_empty() {
                errors.push(format!(
                    "required field '{}' on {} cannot be populated with test data",
                    f.name, target.title
                ));
            }
        }

        let dep_operations = crate::api::api_model::resolve_entity_operations(
            db,
            config,
            &target_domain,
            &target.title,
        )
        .await;
        let optional = !dep_operations.iter().any(|op| op == "create");

        nodes.insert(
            key.clone(),
            DepNode {
                title: target.title.clone(),
                dep_id,
                api_path,
                fields_json,
                is_array,
                optional,
                children,
            },
        );
        node_order.push(key);
    }

    // Post-order DFS over the discovered graph (leaf-first) + cycle breaking.
    // A back-edge closes a required-FK cycle; the DFS records it as the
    // cycle's LAST leg and omits it from the parent's create body instead of
    // aborting the whole `beforeAll` setup.
    let mut steps: Vec<DependencyStep> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut visiting: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = Vec::new();
    // `(parent_key, child_key, break)` — the DFS-ordered broken legs.
    let mut breaks: Vec<(String, String, CycleBreakCtx)> = Vec::new();
    let mut order: Vec<String> = Vec::new();
    for key in &node_order {
        post_order_visit(
            key,
            &nodes,
            &mut visited,
            &mut visiting,
            &mut stack,
            &mut order,
            &mut breaks,
        );
    }
    let broken_legs: HashSet<(&String, &String, &String)> =
        breaks.iter().map(|(p, c, b)| (p, &b.fk, c)).collect();
    for key in &order {
        let Some(node) = nodes.get(key) else {
            continue;
        };
        let cycle_breaks: Vec<CycleBreakCtx> = breaks
            .iter()
            .filter(|(parent, _, _)| parent == key)
            .map(|(_, _, brk)| brk.clone())
            .collect();
        let cycle_break_paths = cycle_breaks
            .iter()
            .map(|brk| brk.cycle_path.clone())
            .collect::<Vec<_>>()
            .join("; ");
        let fk_map = node
            .children
            .iter()
            .filter(|(fk, child_key)| {
                !broken_legs.contains(&(key, fk, child_key)) && nodes.contains_key(child_key)
            })
            .filter_map(|(fk, child_key)| {
                nodes
                    .get(child_key)
                    .map(|child| [fk.clone(), child.dep_id.clone()])
            })
            .collect();
        steps.push(DependencyStep {
            dep_id: node.dep_id.clone(),
            api_path: node.api_path.clone(),
            fields_json: node.fields_json.clone(),
            fk_map,
            is_array: node.is_array,
            optional: node.optional,
            cycle_breaks,
            cycle_break_paths,
        });
    }

    (entity_ref_deps, steps, errors)
}

fn post_order_visit(
    key: &str,
    nodes: &std::collections::HashMap<String, DepNode>,
    visited: &mut HashSet<String>,
    visiting: &mut HashSet<String>,
    stack: &mut Vec<String>,
    order: &mut Vec<String>,
    breaks: &mut Vec<(String, String, CycleBreakCtx)>,
) {
    if visited.contains(key) || visiting.contains(key) {
        return;
    }
    visiting.insert(key.to_string());
    stack.push(key.to_string());
    if let Some(node) = nodes.get(key) {
        for (fk, child_key) in &node.children {
            if visiting.contains(child_key) {
                // Back-edge — this leg closes a required-FK cycle. Break the
                // cycle here (the cycle's last leg) instead of aborting: the
                // parent's create omits the FK. If the API layer genuinely
                // requires it, the emitted spec surfaces a clear error.
                let child_title = nodes
                    .get(child_key)
                    .map(|n| n.title.clone())
                    .unwrap_or_else(|| child_key.clone());
                let path_start = stack.iter().position(|k| k == child_key).unwrap_or(0);
                let mut cycle_path: Vec<String> = stack[path_start..]
                    .iter()
                    .map(|k| {
                        nodes
                            .get(k)
                            .map(|n| n.title.clone())
                            .unwrap_or_else(|| k.clone())
                    })
                    .collect();
                cycle_path.push(child_title);
                breaks.push((
                    key.to_string(),
                    child_key.clone(),
                    CycleBreakCtx {
                        fk: fk.clone(),
                        on_entity: node.title.clone(),
                        cycle_path: cycle_path.join(" -> "),
                    },
                ));
                continue;
            }
            post_order_visit(child_key, nodes, visited, visiting, stack, order, breaks);
        }
    }
    stack.pop();
    visiting.remove(key);
    visited.insert(key.to_string());
    order.push(key.to_string());
}
