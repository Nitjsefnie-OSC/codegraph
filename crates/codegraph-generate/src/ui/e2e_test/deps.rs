use std::collections::HashSet;

use codegraph_config::DomainConfig;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::{PropertyNode, SchemaNode};

use super::common::collect_ui_fields;
use super::context::{DependencyStep, EntityRefDep};
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
/// - `errors` holds actionable messages for unsatisfiable/cyclic required deps.
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
                queue.push_back((target, dep_id, field.is_array));
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

    // Post-order DFS over the discovered graph (leaf-first) + cycle detection.
    let mut steps: Vec<DependencyStep> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut visiting: HashSet<String> = HashSet::new();
    let mut order: Vec<String> = Vec::new();
    for key in &node_order {
        post_order_visit(
            key,
            &nodes,
            &mut visited,
            &mut visiting,
            &mut order,
            &mut errors,
        );
    }
    for key in &order {
        let Some(node) = nodes.get(key) else {
            continue;
        };
        let fk_map = node
            .children
            .iter()
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
        });
    }

    (entity_ref_deps, steps, errors)
}

fn post_order_visit(
    key: &str,
    nodes: &std::collections::HashMap<String, DepNode>,
    visited: &mut HashSet<String>,
    visiting: &mut HashSet<String>,
    order: &mut Vec<String>,
    errors: &mut Vec<String>,
) {
    if visited.contains(key) {
        return;
    }
    if !visiting.insert(key.to_string()) {
        // Already on the current DFS stack — a required-dependency cycle.
        let title = nodes
            .get(key)
            .map(|n| n.title.clone())
            .unwrap_or_else(|| key.to_string());
        let msg = format!(
            "required dependency cycle detected involving {} — break the required FK cycle or make one ref optional",
            title
        );
        if !errors.iter().any(|e| e == &msg) {
            errors.push(msg);
        }
        return;
    }
    if let Some(node) = nodes.get(key) {
        for (_, child_key) in &node.children {
            post_order_visit(child_key, nodes, visited, visiting, order, errors);
        }
    }
    visiting.remove(key);
    visited.insert(key.to_string());
    order.push(key.to_string());
}
