//! Test-side auth bootstrap + journey specs (issue #463, `ifml_e2e_auth`).
//!
//! Everything here is presence-gated on `project.codegen.ifml_e2e_auth`:
//! flag off ⇒ none of these artifacts are emitted and the rest of the
//! generator's output is byte-identical. The pieces:
//!
//! - [`build_personas`] — one persona per human actor (policy-driven
//!   capabilities) or, without a policy, one per view role; the persona
//!   identity is the lowercase actor/role name and is stable across the
//!   emitted files (storage state name, TS identifier, journey imports).
//! - [`render_auth_setup`] — `tests/e2e/auth.setup.ts`, the Playwright
//!   `globalSetup` provisioning one API key per persona through
//!   `public.create_api_key` (psql over `DATABASE_URL`) and writing
//!   `.auth/{persona}.json` storage states carrying the apiKey, roles, and
//!   capabilities. Provisioning is best-effort: without `DATABASE_URL` (or
//!   against an unmigrated database) it warns and skips, so suites keep
//!   running under the project-level `IFML_API_KEY` header.
//! - [`render_personas`] — `tests/e2e/personas.ts`: per-persona `test.use`
//!   fixtures (storage state + Bearer header) and the ONE place seeding
//!   `__USER_ROLES__` / `__USER_CAPABILITIES__` (from generation-time
//!   policy data, so personas stay deterministic without provisioned keys).
//! - [`render_auth_spec`] — the `{view}.auth.spec.ts` family: persona
//!   allow/deny through the fixtures, denial = redirect AND denial
//!   presentation, unauthenticated/garbage-key behavior, and the per-
//!   capability control-gating titles. Issue #468 W5 adds the gating
//!   describes: personas LACKING the view's roles/capabilities are
//!   asserted to hit the denial redirect AND find the guarded surface
//!   hidden (`expectDeniedPresentation`) — rendered only when the policy
//!   provides ≥2 personas with differing access, else a skip comment.
//! - [`render_workflow_journey`] / [`render_shell_nav_journey`] — the IR
//!   derived `tests/journeys/*.journey.spec.ts`: the workflow handoff
//!   (persona A creates via the API, persona B transitions through the POM
//!   transition map, state persisted verified via an API GET) and the
//!   nav-click walk over the shell's `routes.ts` links.
//! - [`render_view_journey`] (issue #468 W5) — the `{view}.journey.spec.ts`
//!   family: each maximal navigate() path from a view as one walk whose
//!   hops drive the control the event binds to (page classes only) and
//!   assert the landing URL + visible surface, xor wizard steps included.
//!
//! Roles/capabilities ride the SAME resolution the route generator's guard
//! code renders from (`PolicyContext` effective permits), so the test-side
//! personas cannot drift from the emitted guards.

use codegraph_config::DomainConfig;
use codegraph_core::traits::GraphQuerier;
use codegraph_naming::{to_kebab_case, to_pascal_case};

use super::super::api_paths::id_param_from;
use super::super::context::{IfmlModel, IfmlViewContainer};
use super::super::route_generator::workflow_for_entity;
use super::super::route_generator::{actor_role_tokens, role_token_matches};
use super::fixtures::{
    Fixture, codelist_fixture_value, fixture_entries, js_string, schema_backed_api,
};
use super::pom::ViewPom;
use super::spec_payload::{JourneyStep, PersonaTest, ViewJourney, pick_transition};

// ── Personas ─────────────────────────────────────────────────────────────────

/// One test persona: the lowercase actor/role identity, the roles seeded
/// into `__USER_ROLES__`, and the effective capabilities seeded into
/// `__USER_CAPABILITIES__`.
#[derive(Debug, Clone)]
pub(crate) struct PersonaSpec {
    pub id: String,
    pub actor: String,
    pub roles: Vec<String>,
    pub capabilities: Vec<String>,
}

/// A valid TS identifier for a persona: lowercase, non-identifier characters
/// collapsed to `_`, digit-leading names prefixed with `_`.
pub(crate) fn persona_id(name: &str) -> String {
    let cleaned: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    match cleaned.chars().next() {
        Some(first) if first.is_ascii_digit() => format!("_{cleaned}"),
        _ => cleaned,
    }
}

/// The personas for one model: policy-driven per human actor (effective
/// permits from the same `PolicyContext` the route guards render), else one
/// per view role with empty capabilities. Roles seed BOTH forms of the
/// actor token — the policy token (`ROLE_CAPABILITIES` key for `can()`)
/// and its snake_case form (the token the `.ifml` `roles:` guards compare,
/// e.g. crewbase's `roles: [member]` vs actor `Member`).
pub(crate) fn build_personas(model: &IfmlModel, human_actors: &[String]) -> Vec<PersonaSpec> {
    match &model.policy {
        Some(policy) => policy
            .actors
            .iter()
            .filter(|(name, _)| human_actors.contains(name))
            .map(|(name, caps)| PersonaSpec {
                id: persona_id(name),
                actor: name.clone(),
                roles: actor_role_tokens(name),
                capabilities: caps.clone(),
            })
            .collect(),
        None => {
            let mut roles: Vec<String> = Vec::new();
            for vc in &model.view_containers {
                for role in &vc.roles {
                    if !roles.contains(role) {
                        roles.push(role.clone());
                    }
                }
            }
            roles.sort();
            roles
                .into_iter()
                .map(|role| PersonaSpec {
                    id: persona_id(&role),
                    actor: role.clone(),
                    roles: vec![role.clone()],
                    capabilities: Vec::new(),
                })
                .collect()
        }
    }
}

// ── tests/e2e/auth.setup.ts ──────────────────────────────────────────────────

/// The `tests/e2e/auth.setup.ts` globalSetup for one persona list.
pub(crate) fn render_auth_setup(personas: &[PersonaSpec]) -> String {
    let mut s = String::new();
    s.push_str("// Generated by codegraph. DO NOT EDIT.\n");
    s.push_str("//\n");
    s.push_str("// Playwright globalSetup for the IFML auth suites (issue #463): provisions\n");
    s.push_str("// one API key per persona through public.create_api_key (psql over\n");
    s.push_str("// DATABASE_URL) and writes the .auth/{persona}.json storage states carrying\n");
    s.push_str("// the apiKey, roles, and capabilities. Without DATABASE_URL (or against an\n");
    s.push_str("// unmigrated database) provisioning is skipped with a warning and personas\n");
    s.push_str("// fall back to the suite-level IFML_API_KEY header.\n\n");
    s.push_str("import { execFileSync } from 'node:child_process';\n");
    s.push_str("import { mkdirSync, writeFileSync } from 'node:fs';\n");
    s.push_str("import { join } from 'node:path';\n\n");
    s.push_str("interface PersonaSeed {\n\tname: string;\n\tstorageState: string;\n\troles: string[];\n\tcapabilities: string[];\n}\n\n");
    s.push_str("const PERSONAS: PersonaSeed[] = [\n");
    for persona in personas {
        s.push_str("\t{\n");
        s.push_str(&format!("\t\tname: '{}',\n", js_string(&persona.id)));
        s.push_str(&format!(
            "\t\tstorageState: '.auth/{}.json',\n",
            js_string(&persona.id)
        ));
        s.push_str(&format!(
            "\t\troles: [{}],\n",
            persona
                .roles
                .iter()
                .map(|r| format!("'{}'", js_string(r)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        s.push_str(&format!(
            "\t\tcapabilities: [{}],\n",
            persona
                .capabilities
                .iter()
                .map(|c| format!("'{}'", js_string(c)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        s.push_str("\t},\n");
    }
    s.push_str("];\n\n");
    s.push_str("const ORGANIZATION_ID = '00000000-0000-0000-0000-000000000001';\n");
    s.push_str(
        "const SCOPES = '[{\"entity_type\":\"*\",\"entity_id\":\"*\",\"action\":\"*\"}]';\n\n",
    );
    s.push_str("export default async function globalSetup(): Promise<void> {\n");
    s.push_str("\tconst databaseUrl = process.env.DATABASE_URL;\n");
    s.push_str("\tif (!databaseUrl) {\n");
    s.push_str("\t\tconsole.warn('auth.setup: DATABASE_URL is not set — skipping API key provisioning');\n");
    s.push_str("\t\treturn;\n\t}\n");
    s.push_str("\tmkdirSync(join(__dirname, '.auth'), { recursive: true });\n");
    s.push_str("\tfor (const persona of PERSONAS) {\n");
    s.push_str("\t\tlet apiKey: string;\n");
    s.push_str("\t\ttry {\n");
    s.push_str("\t\t\tapiKey = provisionApiKey(databaseUrl, `e2e-${persona.name}`);\n");
    s.push_str("\t\t} catch (error) {\n");
    s.push_str("\t\t\tconsole.warn(`auth.setup: no API key for ${persona.name} (${error})`);\n");
    s.push_str("\t\t\tcontinue;\n\t\t}\n");
    s.push_str("\t\tconst state = {\n");
    s.push_str("\t\t\tcookies: [] as unknown[],\n");
    s.push_str("\t\t\torigins: [] as unknown[],\n");
    s.push_str("\t\t\tapiKey,\n");
    s.push_str("\t\t\troles: persona.roles,\n");
    s.push_str("\t\t\tcapabilities: persona.capabilities,\n");
    s.push_str("\t\t};\n");
    s.push_str("\t\twriteFileSync(join(__dirname, persona.storageState), `${JSON.stringify(state, null, 2)}\\n`);\n");
    s.push_str("\t}\n}\n\n");
    s.push_str("function provisionApiKey(databaseUrl: string, name: string): string {\n");
    s.push_str("\tconst sql = `SELECT public.create_api_key('${ORGANIZATION_ID}'::uuid, '${name}', '${SCOPES}'::jsonb)::text;`;\n");
    s.push_str("\tconst output = execFileSync(\n");
    s.push_str("\t\t'psql',\n");
    s.push_str("\t\t[databaseUrl, '--no-align', '--tuples-only', '--command', sql],\n");
    s.push_str("\t\t{ encoding: 'utf8' },\n");
    s.push_str("\t)\n\t\t.trim();\n");
    s.push_str("\tconst parsed = JSON.parse(output) as { key?: string };\n");
    s.push_str("\tif (!parsed.key) {\n");
    s.push_str("\t\tthrow new Error('create_api_key returned no key');\n\t}\n");
    s.push_str("\treturn parsed.key;\n}\n");
    s
}

// ── tests/e2e/personas.ts ────────────────────────────────────────────────────

/// The `tests/e2e/personas.ts` fixture module for one persona list: the
/// per-persona constants, `asPersona` (describe-scope `test.use` binding),
/// `personaContext` (ad-hoc journey contexts), and `apiKeyFor`.
pub(crate) fn render_personas(personas: &[PersonaSpec]) -> String {
    let mut s = String::new();
    s.push_str("// Generated by codegraph. DO NOT EDIT.\n");
    s.push_str("//\n");
    s.push_str("// Per-persona test fixtures for the IFML auth suites (issue #463). ONE\n");
    s.push_str("// place carries the storage state, the Authorization Bearer header, and the\n");
    s.push_str("// __USER_ROLES__ / __USER_CAPABILITIES__ seeding; suites bind a persona\n");
    s.push_str("// with asPersona() at describe scope, journeys use personaContext().\n\n");
    s.push_str("import { existsSync, readFileSync } from 'node:fs';\n");
    s.push_str("import { dirname, join } from 'node:path';\n");
    s.push_str("import { fileURLToPath } from 'node:url';\n");
    s.push_str("// ESM package: __dirname is not defined (issue #468 W1b integration).\n");
    s.push_str("const __dirname = dirname(fileURLToPath(import.meta.url));\n");
    s.push_str("import { test, type Browser, type BrowserContext } from '@playwright/test';\n\n");
    s.push_str("export interface PersonaSpec {\n\tid: string;\n\tactor: string;\n\troles: string[];\n\tcapabilities: string[];\n}\n\n");
    s.push_str("const PERSONAS: Record<string, PersonaSpec> = {\n");
    for persona in personas {
        s.push_str(&format!(
            "\t{0}: {{ id: '{0}', actor: '{1}', roles: [{2}], capabilities: [{3}] }},\n",
            persona.id,
            js_string(&persona.actor),
            persona
                .roles
                .iter()
                .map(|r| format!("'{}'", js_string(r)))
                .collect::<Vec<_>>()
                .join(", "),
            persona
                .capabilities
                .iter()
                .map(|c| format!("'{}'", js_string(c)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    s.push_str("};\n\n");
    s.push_str("function storageStatePath(spec: PersonaSpec): string {\n");
    s.push_str("\treturn join(__dirname, '.auth', `${spec.id}.json`);\n}\n\n");
    s.push_str("function storedApiKey(spec: PersonaSpec): string {\n");
    s.push_str("\ttry {\n");
    s.push_str("\t\tconst raw = JSON.parse(readFileSync(storageStatePath(spec), 'utf8')) as {\n");
    s.push_str("\t\t\tapiKey?: string;\n\t\t};\n");
    s.push_str("\t\treturn raw.apiKey ?? '';\n");
    s.push_str("\t} catch {\n");
    s.push_str("\t\treturn '';\n\t}\n}\n\n");
    s.push_str("function storageStateFor(spec: PersonaSpec): string | undefined {\n");
    s.push_str(
        "\treturn existsSync(storageStatePath(spec)) ? storageStatePath(spec) : undefined;\n}\n\n",
    );
    s.push_str("function headersFor(spec: PersonaSpec): Record<string, string> | undefined {\n");
    s.push_str("\tconst apiKey = storedApiKey(spec);\n");
    s.push_str("\treturn apiKey ? { Authorization: `Bearer ${apiKey}` } : undefined;\n}\n\n");
    s.push_str(
        "function seedPersona(context: BrowserContext, spec: PersonaSpec): Promise<void> {\n",
    );
    s.push_str("\treturn context.addInitScript(\n");
    s.push_str("\t\t([roles, capabilities]) => {\n");
    s.push_str("\t\t\t(globalThis as any).__USER_ROLES__ = roles;\n");
    s.push_str("\t\t\t(globalThis as any).__USER_CAPABILITIES__ = capabilities;\n");
    s.push_str("\t\t},\n");
    s.push_str("\t\t[spec.roles, spec.capabilities],\n");
    s.push_str("\t);\n}\n\n");
    s.push_str("/** Bind a suite to one persona: storage state, Bearer header, and the\n");
    s.push_str(" * role/capability seeding — call at describe scope, once per describe. */\n");
    s.push_str("export function asPersona(spec: PersonaSpec): void {\n");
    s.push_str("\ttest.use({\n");
    s.push_str("\t\tstorageState: storageStateFor(spec),\n");
    s.push_str("\t\textraHTTPHeaders: headersFor(spec),\n");
    s.push_str("\t});\n");
    s.push_str("\ttest.beforeEach(async ({ context }) => {\n");
    s.push_str("\t\tawait seedPersona(context, spec);\n");
    s.push_str("\t});\n}\n\n");
    s.push_str("/** An ad-hoc persona context for journeys that hand off mid-test. */\n");
    s.push_str("export async function personaContext(\n\tbrowser: Browser,\n\tspec: PersonaSpec,\n): Promise<BrowserContext> {\n");
    s.push_str("\tconst context = await browser.newContext({\n");
    s.push_str("\t\tstorageState: storageStateFor(spec),\n");
    s.push_str("\t\textraHTTPHeaders: headersFor(spec),\n");
    s.push_str("\t});\n");
    s.push_str("\tawait seedPersona(context, spec);\n");
    s.push_str("\treturn context;\n}\n\n");
    s.push_str("/** The persona's provisioned API key ('' when auth.setup.ts did not run). */\n");
    s.push_str("export function apiKeyFor(spec: PersonaSpec): string {\n");
    s.push_str("\treturn storedApiKey(spec);\n}\n\n");
    s.push_str("export function persona(id: string): PersonaSpec {\n");
    s.push_str("\tconst spec = PERSONAS[id];\n");
    s.push_str("\tif (!spec) {\n");
    s.push_str("\t\tthrow new Error(`unknown persona: ${id}`);\n\t}\n");
    s.push_str("\treturn spec;\n}\n\n");
    for persona in personas {
        s.push_str(&format!(
            "export const {} = persona('{}');\n",
            persona.id,
            js_string(&persona.id)
        ));
    }
    s
}

// ── {view}.auth.spec.ts ──────────────────────────────────────────────────────

/// The auth-family payload for one guarded view: the persona cases (the
/// same `PersonaTest` data the base specs render inline when the flag is
/// off) plus the denial/capability surfaces.
#[derive(Debug, Clone)]
pub(crate) struct AuthTestSpec {
    pub view_name: String,
    pub label: String,
    /// Whether the page class exposes a denial presentation surface
    /// (`expectDeniedPresentation`) — a permitted root or heading exists.
    pub denial_presentation: bool,
    pub personas: Vec<PersonaTest>,
    pub required_capabilities: Vec<String>,
    /// Persona-gate coverage (issue #468 W5): present only when the policy
    /// provides ≥2 personas with DIFFERING access (at least one permitted,
    /// one denied) — the denied side then gets its explicit gating tests.
    pub gate: Option<GatePlan>,
}

/// The denied personas of one guarded view.
#[derive(Debug, Clone)]
pub(crate) struct GatePlan {
    /// `(persona id, actor)` pairs lacking the view's roles/capabilities.
    pub denied: Vec<(String, String)>,
}

/// Build the auth payload for one guarded view from its spec payloads and
/// the view's POM plan. `None` for unguarded views.
pub(crate) fn build_auth_spec(
    vc: &IfmlViewContainer,
    spec: &super::spec_payload::ViewTestSpec,
    pom: &ViewPom,
) -> Option<AuthTestSpec> {
    if vc.roles.is_empty() && vc.requires.is_empty() {
        return None;
    }
    pom.denial_target.as_ref()?;
    // Gating coverage only when access actually differs across personas
    // (issue #468 W5): with no permitted/denied split a deny assertion
    // would be vacuous.
    let permitted = spec.personas.iter().filter(|p| p.permitted).count();
    let denied: Vec<(String, String)> = spec
        .personas
        .iter()
        .filter(|p| !p.permitted)
        .map(|p| (persona_id(&p.actor), p.actor.clone()))
        .collect();
    let gate = (permitted > 0 && !denied.is_empty()).then_some(GatePlan { denied });
    Some(AuthTestSpec {
        view_name: vc.name.clone(),
        label: spec.label.clone(),
        denial_presentation: pom.primary_root.is_some() || pom.heading.is_some(),
        personas: spec.personas.clone(),
        required_capabilities: vc.requires.clone(),
        gate,
    })
}

/// Render a view's `{view}.auth.spec.ts`: the unauthenticated/garbage-key
/// behavior, the persona allow/deny tests through the `asPersona` fixtures,
/// and the per-capability gating titles. `personas_import` is the module
/// specifier for the personas fixture (issue #468 W1b integrated mode moves
/// it to `./support/personas`; standalone is `../e2e/personas`).
pub(crate) fn render_auth_spec(spec: &AuthTestSpec, personas_import: &str) -> String {
    let class = format!("{}Page", to_pascal_case(&spec.view_name));
    let kebab = to_kebab_case(&spec.view_name);
    let mut used: Vec<String> = Vec::new();
    for persona in &spec.personas {
        let id = persona_id(&persona.actor);
        if !used.contains(&id) {
            used.push(id);
        }
    }
    if let Some(gate) = &spec.gate {
        for (id, _) in &gate.denied {
            if !used.contains(id) {
                used.push(id.clone());
            }
        }
    }
    let mut s = String::new();
    s.push_str("// Generated by codegraph. DO NOT EDIT.\n");
    s.push_str(&format!(
        "//\n// IFML Playwright E2E auth tests for view {} (issue #463).\n\
         // Personas drive through tests/e2e/personas.ts — the role/capability\n\
         // seeding lives there, so the specs carry zero inline addInitScript.\n\n",
        spec.view_name
    ));
    s.push_str("import { test, expect } from '@playwright/test';\n\n");
    let imports = {
        let mut all = used.clone();
        all.push("asPersona".to_string());
        all.join(", ")
    };
    s.push_str(&format!(
        "import {{ {imports} }} from '{personas_import}';\n"
    ));
    s.push_str(&format!(
        "import {{ {class} }} from '../pages/{kebab}-page';\n\n"
    ));

    s.push_str(&format!(
        "test.describe('{} auth', () => {{\n",
        js_string(&spec.label)
    ));
    s.push_str(
        "\ttest('unauthenticated visit is redirected to the denial target', async ({ page }) => {\n",
    );
    s.push_str(&format!("\t\tconst ui = new {class}(page);\n"));
    s.push_str("\t\tawait ui.open();\n");
    s.push_str("\t\tawait ui.expectDenied();\n");
    if spec.denial_presentation {
        s.push_str("\t\tawait ui.expectDeniedPresentation('denied visitors must see the denial target, not the guarded page');\n");
    }
    s.push_str("\t});\n\n");
    s.push_str(
        "\ttest('a garbage API key is rejected as unauthenticated', async ({ page }) => {\n",
    );
    s.push_str(&format!("\t\tconst ui = new {class}(page);\n"));
    s.push_str("\t\tawait page.setExtraHTTPHeaders({ Authorization: 'Bearer garbage-key' });\n");
    s.push_str("\t\tawait ui.open();\n");
    s.push_str("\t\tawait ui.expectDenied();\n");
    if spec.denial_presentation {
        s.push_str("\t\tawait ui.expectDeniedPresentation('a garbage key must not present the guarded page');\n");
    }
    s.push_str("\t});\n");
    s.push_str("});\n\n");

    let gate = spec.required_capabilities.first().cloned();
    let mut capability_done = false;
    for persona in &spec.personas {
        let id = persona_id(&persona.actor);
        s.push_str(&format!(
            "test.describe('{} as {}', () => {{\n",
            js_string(&spec.label),
            id
        ));
        s.push_str(&format!("\tasPersona({id});\n"));
        s.push_str(&format!(
            "\ttest('actor {} {} {}', async ({{ page }}) => {{\n",
            js_string(&persona.actor),
            if persona.permitted {
                "views"
            } else {
                "is redirected from"
            },
            js_string(&persona.label)
        ));
        s.push_str(&format!("\t\tconst ui = new {class}(page);\n"));
        s.push_str("\t\tawait ui.open();\n");
        if persona.permitted {
            let surface = if persona.assert_heading || persona.primary_testid.is_none() {
                "\t\tawait expect(ui.heading()).toBeVisible();\n"
            } else {
                "\t\tawait expect(ui.primaryRoot()).toBeVisible();\n"
            };
            s.push_str(surface);
            if let Some(component) = &persona.control_component {
                s.push_str(&format!(
                    "\t\tawait expect(ui.{component}Submit()).toBeVisible();\n"
                ));
            }
        } else {
            s.push_str("\t\tawait ui.expectDenied();\n");
            if spec.denial_presentation {
                let reason = match &gate {
                    Some(capability) => format!(
                        "actor {} is denied — {} gating",
                        js_string(&persona.actor),
                        js_string(capability)
                    ),
                    None => format!(
                        "actor {} is denied by the view guard",
                        js_string(&persona.actor)
                    ),
                };
                s.push_str(&format!(
                    "\t\tawait ui.expectDeniedPresentation('{reason}');\n"
                ));
            }
        }
        s.push_str("\t});\n");
        if persona.permitted && !capability_done {
            for capability in &spec.required_capabilities {
                s.push_str(&format!(
                    "\ttest('{} gates {}', async ({{ page }}) => {{\n",
                    js_string(capability),
                    js_string(&format!("'{}'", spec.label))
                ));
                s.push_str(&format!("\t\tconst ui = new {class}(page);\n"));
                s.push_str("\t\tawait ui.open();\n");
                if persona.assert_heading || persona.primary_testid.is_none() {
                    s.push_str("\t\tawait expect(ui.heading()).toBeVisible();\n");
                } else {
                    s.push_str("\t\tawait expect(ui.primaryRoot()).toBeVisible();\n");
                }
                if let Some(component) = &persona.control_component {
                    s.push_str(&format!(
                        "\t\tawait expect(ui.{component}Submit()).toBeVisible();\n"
                    ));
                }
                s.push_str("\t});\n");
            }
            capability_done = true;
        }
        s.push_str("});\n\n");
    }

    // Persona gating (issue #468 W5): personas LACKING the view's
    // roles/capabilities are asserted to hit the load guard's denial
    // redirect AND to find the guarded surface hidden — the allow side is
    // the per-persona describe above.
    match &spec.gate {
        Some(gate) => {
            for (id, actor) in &gate.denied {
                s.push_str(&format!(
                    "test.describe('{} gated from {}', () => {{\n",
                    js_string(&spec.label),
                    id
                ));
                s.push_str(&format!("\tasPersona({id});\n"));
                s.push_str(&format!(
                    "\ttest('{} is gated from {}', async ({{ page }}) => {{\n",
                    id,
                    js_string(&spec.label)
                ));
                s.push_str(&format!("\t\tconst ui = new {class}(page);\n"));
                s.push_str("\t\tawait ui.open();\n");
                s.push_str("\t\tawait ui.expectDenied();\n");
                if spec.denial_presentation {
                    let reason = match spec.required_capabilities.first() {
                        Some(capability) => format!(
                            "actor {} is denied — {} gating",
                            js_string(actor),
                            js_string(capability)
                        ),
                        None => format!("actor {} is denied by the view guard", js_string(actor)),
                    };
                    s.push_str(&format!(
                        "\t\tawait ui.expectDeniedPresentation('{reason}');\n"
                    ));
                }
                s.push_str("\t});\n");
                s.push_str("});\n\n");
            }
        }
        None => {
            s.push_str(
                "// Gating coverage (issue #468 W5) skipped: the policy provides no\n\
                 // persona pair with differing access for this view — a deny\n\
                 // assertion here would be vacuous.\n\n",
            );
        }
    }

    while s.ends_with("\n\n") {
        s.pop();
    }
    s.push('\n');
    s
}

// ── tests/journeys/*.journey.spec.ts ─────────────────────────────────────────

/// The workflow-handoff journey payload: persona A creates through the API,
/// persona B transitions through the POM map, an API GET verifies the
/// persisted state.
#[derive(Debug)]
pub(crate) struct WorkflowJourney {
    pub entity: String,
    pub view_name: String,
    pub component: String,
    pub base_path: String,
    pub data_literal: String,
    pub id_param: String,
    pub transition_to: String,
    pub creator: String,
    pub driver: String,
}

/// The shell nav-click journey payload: entry view plus the persona it
/// drives as (if any).
#[derive(Debug)]
pub(crate) struct ShellNavJourney {
    pub view_name: String,
    pub label: String,
    pub persona: Option<String>,
}

/// First permitted persona for `vc`, preferring `!= excluded`.
fn journey_persona(
    personas: &[PersonaSpec],
    vc: &IfmlViewContainer,
    excluded: Option<&str>,
) -> Option<String> {
    let permitted = |p: &PersonaSpec| {
        let roles_ok = vc.roles.is_empty()
            || vc
                .roles
                .iter()
                .any(|role| role_token_matches(role, &p.actor));
        let caps_ok =
            vc.requires.is_empty() || vc.requires.iter().any(|c| p.capabilities.contains(c));
        roles_ok && caps_ok
    };
    personas
        .iter()
        .filter(|p| Some(p.id.as_str()) != excluded)
        .find(|p| permitted(p))
        .or_else(|| personas.iter().find(|p| Some(p.id.as_str()) != excluded))
        .or_else(|| personas.first())
        .map(|p| p.id.clone())
}

/// The workflow journeys for one model: one per id-param'd form/details
/// component whose entity carries a workflow with action endpoints and a
/// schema-backed create+read API.
pub(crate) async fn build_workflow_journeys(
    db: &dyn GraphQuerier,
    config: &DomainConfig,
    api_version: &str,
    model: &IfmlModel,
    personas: &[PersonaSpec],
) -> Vec<WorkflowJourney> {
    let mut out = Vec::new();
    for vc in &model.view_containers {
        let Some(view_id_param) = id_param_from(&vc.params) else {
            continue;
        };
        for c in &vc.components {
            let is_transition_host =
                super::super::selectors::is_form(c) || c.component_type == "details";
            if !is_transition_host {
                continue;
            }
            let Some(entity) = c.entity.as_deref() else {
                continue;
            };
            let Some(workflow) = workflow_for_entity(config, entity) else {
                continue;
            };
            if !workflow.generate_action_endpoints {
                continue;
            }
            let Some(api) = schema_backed_api(db, config, entity, api_version).await else {
                continue;
            };
            if !api.has_create || !api.has_read {
                continue;
            }
            let Some((_, to)) = pick_transition(&workflow) else {
                continue;
            };
            let creator = journey_persona(personas, vc, None).unwrap_or_default();
            let driver =
                journey_persona(personas, vc, Some(&creator)).unwrap_or_else(|| creator.clone());
            let mut entries = fixture_entries(c);
            for (field, value) in entries.iter_mut() {
                if *field == workflow.status_field {
                    *value = format!("'{}'", js_string(&workflow.initial_state));
                    continue;
                }
                if let Some(code) = codelist_fixture_value(db, Some(entity), field).await {
                    *value = format!("'{}'", js_string(&code));
                }
            }
            out.push(WorkflowJourney {
                entity: entity.to_string(),
                view_name: vc.name.clone(),
                component: c.name.clone(),
                base_path: api.base_path,
                data_literal: Fixture {
                    base_path: String::new(),
                    entries,
                }
                .data_literal(),
                id_param: view_id_param.clone(),
                transition_to: to,
                creator,
                driver,
            });
        }
    }
    out
}

/// The shell nav journey: first landmark view without params (falling back
/// to any param-less view, then the first view) as the entry hop.
pub(crate) fn build_shell_journey(
    model: &IfmlModel,
    personas: &[PersonaSpec],
) -> Option<ShellNavJourney> {
    let entry = model
        .view_containers
        .iter()
        .find(|vc| vc.is_landmark && vc.params.is_empty())
        .or_else(|| model.view_containers.iter().find(|vc| vc.params.is_empty()))
        .or_else(|| model.view_containers.first())?;
    Some(ShellNavJourney {
        view_name: entry.name.clone(),
        label: entry.label.clone().unwrap_or_else(|| entry.name.clone()),
        persona: personas.first().map(|p| p.id.clone()),
    })
}

/// Render `{entity-kebab}-workflow.journey.spec.ts`. `personas_import` is
/// the personas fixture module specifier (see [`render_auth_spec`]).
pub(crate) fn render_workflow_journey(journey: &WorkflowJourney, personas_import: &str) -> String {
    let class = format!("{}Page", to_pascal_case(&journey.view_name));
    let kebab = to_kebab_case(&journey.view_name);
    let component = &journey.component;
    let var = format!("{}Id", to_kebab_case(&journey.entity).replace('-', "_"));
    let api_const = format!(
        "{}_API",
        to_kebab_case(&journey.entity)
            .replace('-', "_")
            .to_uppercase()
    );
    let data_const = format!(
        "{}_DATA",
        to_kebab_case(&journey.entity)
            .replace('-', "_")
            .to_uppercase()
    );
    let mut s = String::new();
    s.push_str("// Generated by codegraph. DO NOT EDIT.\n");
    s.push_str(&format!(
        "//\n// IFML workflow journey (issue #463): {} creates a {} through the API with\n\
         // its Bearer apiKey, {} transitions it through the POM transition map,\n\
         // and the persisted state is verified via an API GET.\n\n",
        journey.creator, journey.entity, journey.driver
    ));
    s.push_str("import { expect, test } from '@playwright/test';\n\n");
    s.push_str(&format!(
        "import {{ apiKeyFor, personaContext, {}, {} }} from '{personas_import}';\n",
        journey.creator, journey.driver
    ));
    s.push_str(&format!(
        "import {{ {class} }} from '../pages/{kebab}-page';\n\n"
    ));
    s.push_str(&format!("const {api_const} = '{}';\n", journey.base_path));
    s.push_str(&format!(
        "const {data_const} = {};\n\n",
        journey.data_literal
    ));
    s.push_str(&format!(
        "test.describe('{} workflow journey', () => {{\n",
        js_string(&journey.entity)
    ));
    s.push_str(&format!(
        "\ttest('journey: {} creates a {}, {} transitions it to {}', async ({{ browser, request }}) => {{\n",
        journey.creator,
        js_string(&journey.entity.to_lowercase()),
        journey.driver,
        js_string(&journey.transition_to)
    ));
    s.push_str(&format!(
        "\t\tconst creatorKey = apiKeyFor({creator});\n",
        creator = journey.creator
    ));
    s.push_str("\t\tconst created = await (\n");
    s.push_str(&format!("\t\t\tawait request.post({api_const}, {{\n"));
    s.push_str(
        "\t\t\t\theaders: creatorKey ? { Authorization: `Bearer ${creatorKey}` } : undefined,\n",
    );
    s.push_str(&format!("\t\t\t\tdata: {data_const},\n"));
    s.push_str("\t\t\t})\n\t\t).json();\n");
    s.push_str(&format!(
        "\t\tconst {var_name} = String(created.data?.id ?? created.id);\n",
        var_name = var
    ));
    s.push_str(&format!(
        "\t\tconst context = await personaContext(browser, {driver});\n",
        driver = journey.driver
    ));
    s.push_str("\t\tconst page = await context.newPage();\n");
    s.push_str(&format!("\t\tconst ui = new {class}(page);\n"));
    s.push_str(&format!(
        "\t\tawait ui.open({{ {param}: {var_name} }});\n",
        param = journey.id_param,
        var_name = var
    ));
    s.push_str(&format!(
        "\t\tawait ui.transition{pascal}To('{}');\n",
        js_string(&journey.transition_to),
        pascal = to_pascal_case(component)
    ));
    s.push_str(&format!(
        "\t\tawait ui.expect{pascal}State('{}');\n",
        js_string(&journey.transition_to),
        pascal = to_pascal_case(component)
    ));
    s.push_str("\t\tconst fetched = await (\n");
    s.push_str("\t\t\tawait request.get(`${");
    s.push_str(&api_const);
    s.push_str("}/${");
    s.push_str(&var);
    s.push_str("}/workflow`, {\n");
    s.push_str(
        "\t\t\t\theaders: creatorKey ? { Authorization: `Bearer ${creatorKey}` } : undefined,\n",
    );
    s.push_str("\t\t\t})\n\t\t).json();\n");
    s.push_str(&format!(
        "\t\texpect(fetched.data?.current_state ?? fetched.data?.status).toBe('{}');\n",
        js_string(&journey.transition_to)
    ));
    s.push_str("\t\tawait context.close();\n");
    s.push_str("\t});\n");
    s.push_str("});\n");
    s
}

/// Render `shell-nav.journey.spec.ts`. `personas_import` is the personas
/// fixture module specifier (see [`render_auth_spec`]).
pub(crate) fn render_shell_nav_journey(journey: &ShellNavJourney, personas_import: &str) -> String {
    let class = format!("{}Page", to_pascal_case(&journey.view_name));
    let kebab = to_kebab_case(&journey.view_name);
    let mut s = String::new();
    s.push_str("// Generated by codegraph. DO NOT EDIT.\n");
    s.push_str("//\n");
    s.push_str("// IFML shell navigation journey (issue #463): walks the app by clicking the\n");
    s.push_str("// links the shell renders from src/lib/routes.ts — never goto()/open() to\n");
    s.push_str("// the targets.\n\n");
    s.push_str("import { expect, test } from '@playwright/test';\n\n");
    match &journey.persona {
        Some(persona) => s.push_str(&format!(
            "import {{ asPersona, {persona} }} from '{personas_import}';\n"
        )),
        None => s.push_str(&format!(
            "import {{ asPersona }} from '{personas_import}';\n"
        )),
    }
    s.push_str("import { routeMap } from '../../src/lib/routes';\n");
    s.push_str(&format!(
        "import {{ {class} }} from '../pages/{kebab}-page';\n\n"
    ));
    s.push_str("test.describe('shell navigation journey', () => {\n");
    if let Some(persona) = &journey.persona {
        s.push_str(&format!("\tasPersona({persona});\n"));
    }
    s.push_str(
        "\ttest('journey: walks the shell by clicking its routes.ts links', async ({ page }) => {\n",
    );
    s.push_str(&format!("\t\tconst entry = new {class}(page);\n"));
    s.push_str("\t\tawait entry.open();\n");
    s.push_str("\t\tfor (const name of Object.keys(routeMap)) {\n");
    s.push_str(
        "\t\t\tconst link = page.getByRole('link', { name: routeMap[name].label }).first();\n",
    );
    s.push_str("\t\t\tif ((await link.count()) === 0) {\n");
    s.push_str("\t\t\t\tcontinue;\n\t\t\t}\n");
    s.push_str("\t\t\tawait link.click();\n");
    s.push_str("\t\t\tawait page.waitForLoadState('domcontentloaded');\n");
    s.push_str("\t\t\tconst back = page.getByRole('link', { name: '");
    s.push_str(&js_string(&journey.label));
    s.push_str("' }).first();\n");
    s.push_str("\t\t\tif (await back.count()) {\n");
    s.push_str("\t\t\t\tawait back.click();\n");
    s.push_str("\t\t\t\tawait page.waitForLoadState('domcontentloaded');\n");
    s.push_str("\t\t\t}\n\t\t}\n");
    s.push_str("\t\tconst links = page.getByRole('link');\n");
    s.push_str("\t\texpect(await links.count(), 'the shell renders routes.ts links').toBeGreaterThan(0);\n");
    s.push_str("\t});\n");
    s.push_str("});\n");
    s
}

/// The journey file names (relative to the svelte output root) for one
/// model — the stale-sweep keep-set.
pub(crate) fn journey_file_names(journeys: &[WorkflowJourney]) -> Vec<String> {
    let mut names: Vec<String> = journeys
        .iter()
        .map(|j| format!("{}-workflow.journey.spec.ts", to_kebab_case(&j.entity)))
        .collect();
    names.push("shell-nav.journey.spec.ts".to_string());
    names.sort();
    names.dedup();
    names
}

// ── {view}.journey.spec.ts (issue #468 W5) ───────────────────────────────────

/// Render a view's `{view}.journey.spec.ts`: each maximal navigate() path
/// as one walk — every hop drives the control the IFML event binds to
/// through the source page class, then asserts the landing URL and the
/// target's visible surface (xor wizard steps included). `personas_import`
/// is the personas fixture module specifier (see [`render_auth_spec`]).
pub(crate) fn render_view_journey(journey: &ViewJourney, personas_import: &str) -> String {
    let source_class = format!("{}Page", to_pascal_case(&journey.view_name));
    let source_kebab = to_kebab_case(&journey.view_name);
    let mut s = String::new();
    s.push_str("// Generated by codegraph. DO NOT EDIT.\n");
    s.push_str(&format!(
        "//\n// IFML navigation journeys for view {} (issue #468 W5): the view's\n\
         // navigate() graph walked as steps — every hop clicks the control the\n\
         // event binds to through the page classes, then asserts the landing\n\
         // route and its visible surface. Chains cap at 4 hops; prefixes of\n\
         // longer walks are not repeated.\n\n",
        journey.view_name
    ));
    s.push_str("import { expect, test } from '@playwright/test';\n\n");
    let persona = journey
        .persona
        .as_deref()
        .expect("journey payloads always carry a permitted persona");
    s.push_str(&format!(
        "import {{ asPersona, {persona} }} from '{personas_import}';\n"
    ));

    // Page-class imports: the source class plus every walked target class,
    // in walk order.
    let mut classes: Vec<(String, String)> = vec![(source_class.clone(), source_kebab)];
    for step in journey.paths.iter().flat_map(|p| &p.steps) {
        let class = format!("{}Page", to_pascal_case(&step.hop.target_view));
        let kebab = to_kebab_case(&step.hop.target_view);
        if !classes.iter().any(|(c, _)| *c == class) {
            classes.push((class, kebab));
        }
    }
    for (class, kebab) in &classes {
        s.push_str(&format!(
            "import {{ {class} }} from '../pages/{kebab}-page';\n"
        ));
    }
    s.push('\n');

    s.push_str(&format!(
        "test.describe('{} journeys', () => {{\n",
        js_string(&journey.label)
    ));
    for path in &journey.paths {
        let destination = path.views.last().cloned().unwrap_or_default();
        let title = path.views.join(" → ");
        s.push_str(&format!(
            "\ttest.describe('walks to {destination}', () => {{\n"
        ));
        s.push_str(&format!("\t\tasPersona({persona});\n"));
        s.push_str(&format!(
            "\t\ttest('journey: {title}', async ({{ page{request} }}) => {{\n",
            request = if journey.open_seed.is_some() || !journey.seeds.is_empty() {
                ", request"
            } else {
                ""
            }
        ));

        // Row-data seeds: fire-and-create with an ok assertion (the row
        // hops need rows to click).
        for (index, seed) in journey.seeds.iter().enumerate() {
            s.push_str(&format!(
                "\t\t\tconst rowSeed{index} = await request.post('{}', {{ data: {} }});\n",
                seed.base_path,
                seed.data_literal()
            ));
            s.push_str(
                &format!("\t\t\texpect(rowSeed{index}.ok(), 'journey fixture entity should be created').toBeTruthy();\n"),
            );
        }

        // The open seed (param'd source views) captures its id for open().
        if let (Some(param), Some(seed)) = (&journey.open_param, &journey.open_seed) {
            s.push_str(&format!(
                "\t\t\tconst seeded = await (await request.post('{}', {{ data: {} }})).json();\n",
                seed.base_path,
                seed.data_literal()
            ));
            s.push_str("\t\t\tconst seededId = String(seeded.data?.id ?? seeded.id);\n");
            s.push_str(&format!("\t\t\tlet ui = new {source_class}(page);\n"));
            s.push_str(&format!("\t\t\tawait ui.open({{ {param}: seededId }});\n"));
        } else {
            s.push_str(&format!("\t\t\tlet ui = new {source_class}(page);\n"));
            s.push_str("\t\t\tawait ui.open();\n");
        }

        // The hops: drive via the page class of the view each hop DEPARTS
        // from (the walking `ui` rebinds mid-walk), then assert the landing
        // surface through the target page class.
        let mut current_view = journey.view_name.clone();
        for (departure, step) in path.views.iter().zip(&path.steps) {
            let JourneyStep {
                hop,
                method,
                arrival,
                arrival_heading,
                target_steps,
            } = step;
            if *departure != current_view {
                let departure_class = format!("{}Page", to_pascal_case(departure));
                s.push_str(&format!("\t\t\tui = new {departure_class}(page);\n"));
                current_view = departure.clone();
            }
            s.push_str(&format!("\t\t\tawait ui.{method}();\n"));
            s.push_str(&format!(
                "\t\t\tawait expect(page).toHaveURL(new RegExp('{}'));\n",
                js_string(&hop.target_pattern)
            ));
            if *arrival || !target_steps.is_empty() {
                let target_class = format!("{}Page", to_pascal_case(&hop.target_view));
                let var = journey_page_var(&hop.target_view);
                s.push_str(&format!("\t\t\tconst {var} = new {target_class}(page);\n"));
                if *arrival {
                    if *arrival_heading {
                        s.push_str(&format!(
                            "\t\t\tawait expect({var}.heading()).toBeVisible();\n"
                        ));
                    } else {
                        s.push_str(&format!(
                            "\t\t\tawait expect({var}.primaryRoot()).toBeVisible();\n"
                        ));
                    }
                }
            }
            for name in target_steps {
                let var = journey_page_var(&hop.target_view);
                s.push_str(&format!(
                    "\t\t\tawait expect({var}.step('{}')).toBeVisible();\n",
                    js_string(name)
                ));
            }
        }
        s.push_str("\t\t});\n");
        s.push_str("\t});\n");
    }
    s.push_str("});\n");
    s
}

/// A short in-spec variable for a walked page class (`refund_request_list`
/// for `RefundRequestList`).
fn journey_page_var(view_name: &str) -> String {
    to_kebab_case(view_name).replace('-', "_")
}

/// The view-journey file names (relative to the svelte output root) for
/// one model — the stale-sweep keep-set.
pub(crate) fn view_journey_file_names(journeys: &[ViewJourney]) -> Vec<String> {
    journeys
        .iter()
        .map(|j| format!("{}.journey.spec.ts", to_kebab_case(&j.view_name)))
        .collect()
}
