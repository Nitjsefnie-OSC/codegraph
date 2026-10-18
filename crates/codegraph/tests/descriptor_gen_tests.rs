#[test]
fn test_descriptor_template_renders_minimal() {
    let mut tera = tera::Tera::default();
    tera.add_raw_template(
        "ui/descriptor.tera",
        include_str!("../../codegraph-generate/templates/ui/descriptor.tera"),
    )
    .unwrap();

    let mut ctx = tera::Context::new();
    ctx.insert("entity_name", "TestEntity");
    ctx.insert("domain", "testing");
    ctx.insert("path_segment", "test-entities");
    ctx.insert("operations", &vec!["create", "read", "list"]);
    ctx.insert("has_fts", &false);
    ctx.insert("fields", &Vec::<serde_json::Value>::new());
    ctx.insert("groups", &Vec::<serde_json::Value>::new());
    ctx.insert("children", &Vec::<serde_json::Value>::new());
    ctx.insert("has_workflow", &false);
    ctx.insert("workflow_field", &"");
    ctx.insert("workflow_transitions", &Vec::<serde_json::Value>::new());
    let no_wizard: Option<serde_json::Value> = None;
    ctx.insert("wizard", &no_wizard);

    let output = tera.render("ui/descriptor.tera", &ctx).unwrap();
    assert!(output.contains("EntityDescriptor"));
    assert!(output.contains("TestEntity"));
    assert!(output.contains("'create'"));
    assert!(output.contains("'read'"));
    assert!(output.contains("'list'"));
    assert!(output.contains("@crewbase/entities"));
}

/// `displayField` (issue #468 W2) renders from the resolved display field —
/// child entries and entity-ref targets alike — and falls back to `'id'`
/// (never `null`, which would break the string-typed `@crewbase/entities`
/// contract) when no display field resolved or the context key is absent.
#[test]
fn test_descriptor_display_field_resolution_and_id_fallback() {
    let mut tera = tera::Tera::default();
    tera.add_raw_template(
        "ui/descriptor.tera",
        include_str!("../../codegraph-generate/templates/ui/descriptor.tera"),
    )
    .unwrap();

    let mut ctx = tera::Context::new();
    ctx.insert("entity_name", "TestEntity");
    ctx.insert("domain", "testing");
    ctx.insert("path_segment", "test-entities");
    ctx.insert("operations", &vec!["read"]);
    ctx.insert("has_fts", &false);
    ctx.insert(
        "fields",
        &serde_json::json!([
            {
                "name": "owner_id",
                "label": "Owner",
                "input_type": "text",
                "ts_type": "string",
                "is_entity_ref": true,
                "ref_entity": "OwnerType",
                "ref_domain": "testing",
                "ref_display_field": "full_name"
            },
            {
                "name": "worker_id",
                "label": "Worker",
                "input_type": "text",
                "ts_type": "string",
                "is_entity_ref": true,
                "ref_entity": "WorkerType",
                "ref_domain": "testing"
            }
        ]),
    );
    ctx.insert("groups", &Vec::<serde_json::Value>::new());
    ctx.insert(
        "children",
        &serde_json::json!([
            {
                "name": "resolved_child",
                "label": "Resolved Child",
                "entity_name": "ResolvedChild",
                "relationship": "one-to-many",
                "inline": false,
                "display_field": "title"
            },
            {
                "name": "unresolved_child",
                "label": "Unresolved Child",
                "entity_name": "UnresolvedChild",
                "relationship": "one-to-many",
                "inline": false
            }
        ]),
    );
    ctx.insert("has_workflow", &false);
    ctx.insert("workflow_field", &"");
    ctx.insert("workflow_transitions", &Vec::<serde_json::Value>::new());
    let no_wizard: Option<serde_json::Value> = None;
    ctx.insert("wizard", &no_wizard);

    let output = tera.render("ui/descriptor.tera", &ctx).unwrap();

    assert!(
        output
            .contains("ref: { entity: 'OwnerType', domain: 'testing', displayField: 'full_name' }"),
        "resolved ref display field must render:\n{output}"
    );
    assert!(
        output.contains("ref: { entity: 'WorkerType', domain: 'testing', displayField: 'id' }"),
        "unresolved ref display field must fall back to 'id':\n{output}"
    );
    assert!(
        !output.contains("'display_name'"),
        "the hardcoded 'display_name' must be gone:\n{output}"
    );
    assert!(
        output.contains("inline: false,\n      displayField: 'title',"),
        "resolved child displayField must render:\n{output}"
    );
    assert!(
        output.contains("inline: false,\n      displayField: 'id',"),
        "unresolved child displayField must fall back to 'id':\n{output}"
    );
    assert!(
        !output.contains("displayField: null"),
        "displayField must stay a string:\n{output}"
    );
}

#[test]
fn test_shell_list_template_renders() {
    let mut tera = tera::Tera::default();
    tera.add_raw_template(
        "ui/shell_list.tera",
        include_str!("../../codegraph-generate/templates/ui/shell_list.tera"),
    )
    .unwrap();

    let mut ctx = tera::Context::new();
    ctx.insert("entity_name", "PositionOpening");
    ctx.insert("domain", "recruiting");
    ctx.insert("path_segment", "position-openings");

    let output = tera.render("ui/shell_list.tera", &ctx).unwrap();
    assert!(output.contains("EntityList"));
    assert!(output.contains("PositionOpeningDescriptor"));
    assert!(output.contains("@crewbase/entities"));
}

#[test]
fn test_shell_detail_template_renders() {
    let mut tera = tera::Tera::default();
    tera.add_raw_template(
        "ui/shell_detail.tera",
        include_str!("../../codegraph-generate/templates/ui/shell_detail.tera"),
    )
    .unwrap();

    let mut ctx = tera::Context::new();
    ctx.insert("entity_name", "PositionOpening");
    ctx.insert("domain", "recruiting");
    ctx.insert("path_segment", "position-openings");

    let output = tera.render("ui/shell_detail.tera", &ctx).unwrap();
    assert!(output.contains("EntityDetail"));
    assert!(output.contains("PositionOpeningDescriptor"));
    assert!(output.contains("data.id"));
}

#[test]
fn test_shell_create_template_renders() {
    let mut tera = tera::Tera::default();
    tera.add_raw_template(
        "ui/shell_create.tera",
        include_str!("../../codegraph-generate/templates/ui/shell_create.tera"),
    )
    .unwrap();

    let mut ctx = tera::Context::new();
    ctx.insert("entity_name", "PositionOpening");
    ctx.insert("domain", "recruiting");
    ctx.insert("path_segment", "position-openings");

    let output = tera.render("ui/shell_create.tera", &ctx).unwrap();
    assert!(output.contains("EntityForm"));
    assert!(output.contains("mode=\"create\""));
    assert!(output.contains("PositionOpeningDescriptor"));
}

#[test]
fn test_shell_edit_template_renders() {
    let mut tera = tera::Tera::default();
    tera.add_raw_template(
        "ui/shell_edit.tera",
        include_str!("../../codegraph-generate/templates/ui/shell_edit.tera"),
    )
    .unwrap();

    let mut ctx = tera::Context::new();
    ctx.insert("entity_name", "PositionOpening");
    ctx.insert("domain", "recruiting");
    ctx.insert("path_segment", "position-openings");

    let output = tera.render("ui/shell_edit.tera", &ctx).unwrap();
    assert!(output.contains("EntityForm"));
    assert!(output.contains("mode=\"edit\""));
    assert!(output.contains("data.item"));
}
