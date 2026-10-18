//! Phase 0 red pin for the `hurl_contract` domain generator (issue #463).
//!
//! Pins — byte for byte — the hurl contract suite every generated app must
//! ship so the ops api suite (`testkit api`) exercises real HTTP contracts
//! with zero hand-written test files:
//!
//! - one per-entity contract file per entity, `{nn}_{domain}_{entity}.hurl`,
//!   numbered deterministically from 10 in (domain rank, title) order —
//!   LIST envelope, CREATE 201 + id capture, required-FK parents created
//!   first via preceding POSTs with their own captures, GET-by-id field
//!   echo, zero-uuid 404, PUT roundtrip + GET verify, DELETE 204 + GET 404,
//!   `Authorization: Bearer {{api_key}}` on every request;
//! - the four authn/authz files, emitted once per run against the first
//!   tenant-scoped entity: `01_auth.hurl` (missing/garbage key → 401),
//!   `03_scope_denial_403.hurl` (read-only key: read 200, write 403
//!   INSUFFICIENT_SCOPE), `04_cross_tenant_404.hurl` (org-B GET of an
//!   org-A row → 404), and `08_rls_isolation.hurl` (the ops api-suite
//!   stage-8 convention file: org-B list never contains the org-A row).
//!
//! Plus the generator↔harness contract: the ops generator's
//! `codegraph-ops.toml` must carry the `[hurl]` section (dir, skip naming
//! the stage-8 file, org ids, `limited_key = true`) so the harness
//! provisions exactly the keys the generated files reference.
//!
//! Node-free and DB-free: runs the full pipeline over a small mox fixture
//! (2 entities — one FK-linked pair — one codelist field) and byte-compares
//! the emitted files. RED at HEAD: the generator does not exist.
//!
//! ```text
//! cargo test -p codegraph --test hurl_contract_tests
//! ```

use std::fs;
use std::path::{Path, PathBuf};

const INVENTORY_MOX: &str = r#"package inventory

/// Part lifecycle status.
enum PartStatus {
    Active as "Active" = 0
    Retired as "Retired" = 1
}

/// A replaceable part.
class WidgetPartType {
    /// Part label.
    String label
    /// Lifecycle status.
    PartStatus status
}

/// An assembled widget.
class WidgetType {
    /// Widget name.
    String name
    /// The part this widget is built from.
    refers WidgetPartType [1] part
}
"#;

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]
api_version = "v1"

[domains.inventory]
label = "Inventory"
schema_dir = "inventory"
postgres_schema = "inventory"
"#;

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
    mox: PathBuf,
}

fn write_fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("domains.toml"), DOMAINS_TOML).unwrap();
    fs::create_dir_all(root.path().join("model")).unwrap();
    fs::write(root.path().join("model/inventory.mox"), INVENTORY_MOX).unwrap();
    Fixture {
        config: root.path().join("domains.toml"),
        mox: root.path().join("model/inventory.mox"),
        root,
    }
}

fn run_args<'a>(fixture: &'a Fixture, output: &'a Path) -> codegraph::driver::RunArgs<'a> {
    codegraph::driver::RunArgs {
        schemas: None,
        classifier: None,
        config_path: &fixture.config,
        output,
        extension_points_path: None,
        profile_name: "default",
        variant: None,
        // The workspace profiles.toml: pins that `hurl_contract` is ON in
        // the default profile (the "default/fullstack ON" decision), not
        // merely registered.
        profiles_config_path: Some(PathBuf::from("../../profiles.toml")),
        no_post_gen: true,
        template_dir: &[],
        ifml_files: &[],
        openapi_files: &[],
        mox_files: std::slice::from_ref(&fixture.mox),
        rosetta_files: &[],
        ddd_files: &[],
        evt_files: &[],
        deploy_files: &[],
        ifml_framework: &[],
        ifml_components: None,
        ifml_design_system: None,
        codegraph_rev: None,
        check: false,
        ux_rules: None,
    }
}

/// Generate the fixture and return the emitted hurl directory.
async fn generate_fixture() -> (Fixture, PathBuf) {
    let fixture = write_fixture();
    let output = fixture.root.path().join("generated");
    codegraph::driver::run(run_args(&fixture, &output))
        .await
        .expect("pipeline run must succeed");
    (fixture, output.join("hurl"))
}

/// The exact hurl file set and bodies the generator must emit.
///
/// Titles sort `WidgetPartType` < `WidgetType`, so the first entity — and
/// therefore the authn/authz suite's routes — is `widget_part`. Entity
/// contract files number from 10; slots 01/03/04/08 are the fixed
/// authn/authz convention (08 = the ops api-suite stage-8 file, named in
/// `[hurl].skip` so the main loop never runs it).
fn expected_hurl_files() -> Vec<(&'static str, String)> {
    let widget_part: String =
        r#"# hurl_contract: inventory.widget_part — API contract (generated; do not edit).
# Regenerate with the codegraph pipeline. Vars: base_url, api_key.

# LIST — envelope shape
GET {{base_url}}/api/v1/inventory/widget-part?page=0&page_size=10
Authorization: Bearer {{api_key}}

HTTP 200
[Asserts]
jsonpath "$.data" exists
jsonpath "$.meta" exists

# CREATE — 201, capture the id
POST {{base_url}}/api/v1/inventory/widget-part
Authorization: Bearer {{api_key}}
{
  "label": "contract-widget_part",
  "status": "Active"
}

HTTP 201
[Captures]
widget_part_id: jsonpath "$.data.id"
[Asserts]
jsonpath "$.data.id" exists

# GET by id — field echo
GET {{base_url}}/api/v1/inventory/widget-part/{{widget_part_id}}
Authorization: Bearer {{api_key}}

HTTP 200
[Asserts]
jsonpath "$.data.label" == "contract-widget_part"
jsonpath "$.data.status" == "Active"

# GET zero-uuid — 404
GET {{base_url}}/api/v1/inventory/widget-part/00000000-0000-0000-0000-000000000000
Authorization: Bearer {{api_key}}

HTTP 404

# PUT roundtrip
PUT {{base_url}}/api/v1/inventory/widget-part/{{widget_part_id}}
Authorization: Bearer {{api_key}}
{
  "label": "contract-widget_part-v2",
  "status": "Retired"
}

HTTP 200

# GET — the update landed
GET {{base_url}}/api/v1/inventory/widget-part/{{widget_part_id}}
Authorization: Bearer {{api_key}}

HTTP 200
[Asserts]
jsonpath "$.data.label" == "contract-widget_part-v2"
jsonpath "$.data.status" == "Retired"

# DELETE — 204
DELETE {{base_url}}/api/v1/inventory/widget-part/{{widget_part_id}}
Authorization: Bearer {{api_key}}

HTTP 204

# GET — gone
GET {{base_url}}/api/v1/inventory/widget-part/{{widget_part_id}}
Authorization: Bearer {{api_key}}

HTTP 404
"#
        .to_string();

    let widget: String =
        r#"# hurl_contract: inventory.widget — API contract (generated; do not edit).
# Regenerate with the codegraph pipeline. Vars: base_url, api_key.

# LIST — envelope shape
GET {{base_url}}/api/v1/inventory/widget?page=0&page_size=10
Authorization: Bearer {{api_key}}

HTTP 200
[Asserts]
jsonpath "$.data" exists
jsonpath "$.meta" exists

# Required FK parent widget_part — created first, id captured
POST {{base_url}}/api/v1/inventory/widget-part
Authorization: Bearer {{api_key}}
{
  "label": "contract-widget_part",
  "status": "Active"
}

HTTP 201
[Captures]
parent_part_id: jsonpath "$.data.id"

# CREATE — 201, capture the id
POST {{base_url}}/api/v1/inventory/widget
Authorization: Bearer {{api_key}}
{
  "name": "contract-widget",
  "part_id": "{{parent_part_id}}"
}

HTTP 201
[Captures]
widget_id: jsonpath "$.data.id"
[Asserts]
jsonpath "$.data.id" exists

# GET by id — field echo
GET {{base_url}}/api/v1/inventory/widget/{{widget_id}}
Authorization: Bearer {{api_key}}

HTTP 200
[Asserts]
jsonpath "$.data.name" == "contract-widget"
jsonpath "$.data.part_id" == "{{parent_part_id}}"

# GET zero-uuid — 404
GET {{base_url}}/api/v1/inventory/widget/00000000-0000-0000-0000-000000000000
Authorization: Bearer {{api_key}}

HTTP 404

# PUT roundtrip
PUT {{base_url}}/api/v1/inventory/widget/{{widget_id}}
Authorization: Bearer {{api_key}}
{
  "name": "contract-widget-v2",
  "part_id": "{{parent_part_id}}"
}

HTTP 200

# GET — the update landed
GET {{base_url}}/api/v1/inventory/widget/{{widget_id}}
Authorization: Bearer {{api_key}}

HTTP 200
[Asserts]
jsonpath "$.data.name" == "contract-widget-v2"
jsonpath "$.data.part_id" == "{{parent_part_id}}"

# DELETE — 204
DELETE {{base_url}}/api/v1/inventory/widget/{{widget_id}}
Authorization: Bearer {{api_key}}

HTTP 204

# GET — gone
GET {{base_url}}/api/v1/inventory/widget/{{widget_id}}
Authorization: Bearer {{api_key}}

HTTP 404
"#
        .to_string();

    let auth: String = r#"# hurl_contract: authn — missing and garbage API keys are rejected (401).
# Generated once per run against the first tenant-scoped entity.

GET {{base_url}}/api/v1/inventory/widget-part

HTTP 401
[Asserts]
jsonpath "$.error.code" == "UNAUTHORIZED"

GET {{base_url}}/api/v1/inventory/widget-part
Authorization: Bearer not-a-real-key

HTTP 401
[Asserts]
jsonpath "$.error.code" == "UNAUTHORIZED"
"#
    .to_string();

    let scope_denial: String =
        r#"# hurl_contract: authz scope — the read-only key (api_key_limited) may read
# but every write is denied with 403 INSUFFICIENT_SCOPE. Generated once per
# run against the first tenant-scoped entity.

GET {{base_url}}/api/v1/inventory/widget-part?page=0&page_size=10
Authorization: Bearer {{api_key_limited}}

HTTP 200

POST {{base_url}}/api/v1/inventory/widget-part
Authorization: Bearer {{api_key_limited}}
{
  "label": "scope-denial-widget_part",
  "status": "Active"
}

HTTP 403
[Asserts]
jsonpath "$.error.code" == "FORBIDDEN"
jsonpath "$.error.message" contains "INSUFFICIENT_SCOPE"
"#
        .to_string();

    let cross_tenant: String =
        r#"# hurl_contract: tenant isolation — a cross-tenant read is a silent RLS
# filter (404), never a 403. Generated once per run against the first
# tenant-scoped entity.

POST {{base_url}}/api/v1/inventory/widget-part
Authorization: Bearer {{api_key_a}}
{
  "label": "org-a-widget_part",
  "status": "Active"
}

HTTP 201
[Captures]
cross_tenant_widget_part_id: jsonpath "$.data.id"

GET {{base_url}}/api/v1/inventory/widget-part/{{cross_tenant_widget_part_id}}
Authorization: Bearer {{api_key_b}}

HTTP 404
"#
        .to_string();

    let rls_isolation: String =
        r#"# hurl_contract: RLS isolation — ops api-suite stage 8 convention file.
# Runs with api_key_a / api_key_b only; the manifest [hurl].skip list keeps
# it out of the main hurl loop.

POST {{base_url}}/api/v1/inventory/widget-part
Authorization: Bearer {{api_key_a}}
{
  "label": "rls-isolation-widget_part",
  "status": "Active"
}

HTTP 201
[Captures]
rls_widget_part_id: jsonpath "$.data.id"

GET {{base_url}}/api/v1/inventory/widget-part?page=0&page_size=100
Authorization: Bearer {{api_key_b}}

HTTP 200
[Asserts]
jsonpath "$.data[*].id" not contains "{{rls_widget_part_id}}"

GET {{base_url}}/api/v1/inventory/widget-part/{{rls_widget_part_id}}
Authorization: Bearer {{api_key_a}}

HTTP 200
"#
        .to_string();

    vec![
        ("01_auth.hurl", auth),
        ("03_scope_denial_403.hurl", scope_denial),
        ("04_cross_tenant_404.hurl", cross_tenant),
        ("08_rls_isolation.hurl", rls_isolation),
        ("10_inventory_widget_part.hurl", widget_part),
        ("11_inventory_widget.hurl", widget),
    ]
}

/// The core red pin: the pipeline emits exactly the expected hurl file set,
/// byte for byte.
#[tokio::test]
async fn hurl_contract_generator_emits_the_full_contract_suite() {
    let (_fixture, hurl_dir) = generate_fixture().await;

    let mut actual: Vec<String> = fs::read_dir(&hurl_dir)
        .unwrap_or_else(|e| panic!("hurl dir must exist at {}: {e}", hurl_dir.display()))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "hurl"))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    actual.sort();

    let expected_names: Vec<&str> = expected_hurl_files().iter().map(|(n, _)| *n).collect();
    assert_eq!(
        actual, expected_names,
        "the emitted hurl file set must match the contract exactly"
    );

    for (name, expected) in expected_hurl_files() {
        let actual =
            fs::read_to_string(hurl_dir.join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"));
        assert_eq!(actual, expected, "byte mismatch in {name}");
    }
}

/// The generator↔harness contract: the ops generator's emitted
/// `codegraph-ops.toml` must parse with a `[hurl]` section aligned with the
/// generated files — dir, the stage-8 skip, org ids, and limited-key
/// provisioning (the `{{api_key_limited}}` the 03 file asserts with).
#[tokio::test]
async fn ops_manifest_carries_the_generated_hurl_section() {
    let (fixture, _) = generate_fixture().await;
    let manifest = fixture
        .root
        .path()
        .join("generated")
        .join("codegraph-ops.toml");

    let config = codegraph_ops::OpsConfig::load(&manifest)
        .unwrap_or_else(|e| panic!("emitted manifest must parse: {e}"));
    let hurl = config
        .manifest
        .hurl
        .as_ref()
        .expect("emitted manifest must carry the [hurl] section");

    assert_eq!(hurl.dir, PathBuf::from("hurl"));
    assert_eq!(
        hurl.skip,
        vec!["08_rls_isolation.hurl".to_string()],
        "the stage-8 convention file must be skipped in the main hurl loop"
    );
    assert!(
        hurl.limited_key,
        "limited-key provisioning must be on: 03_scope_denial_403.hurl asserts with \
         {{{{api_key_limited}}}}"
    );
    assert_eq!(
        hurl.org_id_a.as_deref(),
        Some("00000000-0000-0000-0000-000000000001")
    );
    assert_eq!(
        hurl.org_id_b.as_deref(),
        Some("00000000-0000-0000-0000-000000000002")
    );
}
