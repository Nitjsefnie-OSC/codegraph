use crate::ProjectConfig;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use codegraph_config::{DomainConfig, IfmlComponentMappings};
use codegraph_core::traits::GraphQuerier;

use crate::GenerationEntry;
use crate::error::Result;
use crate::render_template;
use crate::traits::{GeneratedFile, GlobalGenerator, GlobalGeneratorKind};

use self::context::build_page_context;
use self::load::build_load_context;
use self::ux::{UxGeneration, resolve_generation_ux};
use self::workflow::js_quote;
use super::querier::{IfmlGraphQuerier, IfmlQuerier};

mod context;
mod hydration;
mod load;
mod render;
mod roles;
#[cfg(test)]
mod tests;
mod ux;
mod workflow;

pub(crate) use hydration::{actor_role_tokens, role_token_matches};

pub use context::{
    ControlGateContext, LayoutSvelteContext, PageComponentContext, PageLoadComponentContext,
    PageLoadContext, PageSvelteContext, PageUxContext, RenderButton, RenderChart, RenderColumn,
    RenderColumnUx, RenderContainer, RenderEvent, RenderForm, RenderGroup, RenderImport,
    RenderInputField, RenderMapping, RenderModal, RenderNavItem, RenderRowMenuEvent,
    RenderShellNav, RenderSubmit, RenderTable, RenderTimeline, RenderTransition, RenderViewParam,
    RenderWorkflow, TableLayout,
};
pub(crate) use render::{
    mapped_container_testid, modal_wrapper_active, modal_wrapper_testid, shell_nav,
};
pub(crate) use workflow::workflow_for_entity;

/// The shared control-inference entry over `(rust_type, field_name)` pairs
/// (`fields_with_types`), re-exported for conformance testing (issue #201).
pub use super::control_core::control_for_field;

/// The SvelteKit route group the view pages live under in integrated mode
/// (issue #468 W1b): a sibling of the entity ui app's `(app)`/`(auth)`
/// groups, so the entity auth-guard layout does not wrap views — views gate
/// themselves via their +page.ts guards. Route groups never appear in URLs.
pub const ROUTE_GROUP: &str = "(views)";

pub struct IfmlRouteGenerator {
    output_dir: PathBuf,
    framework: String,
    output_paths: super::output_paths::OutputPaths,
    mappings: Option<IfmlComponentMappings>,
}

impl IfmlRouteGenerator {
    pub fn new(output_dir: &Path, framework: &str) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
            framework: framework.to_string(),
            output_paths: super::output_paths::OutputPaths::for_framework(framework),
            mappings: None,
        }
    }

    /// Integrated-mode layout (issue #468 W1b): view pages land under the
    /// SvelteKit route group (`src/routes/(views)/{view}/+page.svelte`).
    /// URL routes, the navigation map, and the `$lib` helpers are unchanged.
    pub fn with_route_root(mut self, route_root: &str) -> Self {
        self.output_paths = super::output_paths::OutputPaths::for_framework_with_route_root(
            &self.framework,
            Some(route_root),
        );
        self
    }

    pub fn with_mappings(mut self, mappings: Option<IfmlComponentMappings>) -> Self {
        self.mappings = mappings;
        self
    }

    /// Framework layout path for the landmark shell; `None` for frameworks
    /// without a layout convention in this slice. Integrated mode scopes the
    /// shell layout to the route group (`src/routes/(views)/+layout.svelte`)
    /// so it never wraps — or collides with — the entity app's own layouts.
    fn route_layout(&self) -> Option<PathBuf> {
        match self.framework.as_str() {
            "svelte" => Some(match self.output_paths.route_root() {
                Some(group) => PathBuf::from(format!("src/routes/{group}/+layout.svelte")),
                None => PathBuf::from("src/routes/+layout.svelte"),
            }),
            _ => None,
        }
    }

    /// Integrated mode (issue #468 W1b): the registry scopes the route
    /// generator to the `(views)` route group exactly when the IFML tree
    /// integrates into the entity ui app; standalone plans keep the default
    /// route root. The generator therefore derives the mode from its own
    /// paths — no builder threading needed.
    pub(crate) fn integrated(&self) -> bool {
        self.output_paths.route_root() == Some(ROUTE_GROUP)
    }

    /// The role-hydration layouts (issue #468): integrated mode only, and
    /// only when at least one view is guarded (something must read the
    /// role surface). Standalone stays byte-identical.
    fn hydration(&self, model: &super::context::IfmlModel) -> Option<Vec<GeneratedFile>> {
        if !self.integrated() {
            return None;
        }
        let group = self.output_paths.route_root()?;
        hydration::hydration_files(&self.output_dir, group, model)
    }

    /// Layout render context when a landmark view exists and a `shell`
    /// mapping resolves; `None` emits no layout (byte-identical no-pack
    /// output).
    fn layout_context(&self, model: &super::context::IfmlModel) -> Option<LayoutSvelteContext> {
        self.route_layout()?;
        shell_nav(&model.view_containers, self.mappings.as_ref())
            .map(|shell| LayoutSvelteContext { shell })
    }

    /// The `$lib/roles` helper when at least one view declares roles or
    /// capability requirements and the framework has the SvelteKit load
    /// convention; `None` otherwise. An existing file is never overwritten.
    /// Content varies: with an ingested policy the helper embeds the
    /// actor→effective-capabilities map; with requirements but no policy
    /// `can()` consults the runtime `__USER_CAPABILITIES__` override only.
    fn roles_helper(&self, model: &super::context::IfmlModel) -> Option<GeneratedFile> {
        if self.framework != "svelte" {
            return None;
        }
        let any_roles = model.view_containers.iter().any(|vc| !vc.roles.is_empty());
        let any_requires = model
            .view_containers
            .iter()
            .any(|vc| !vc.requires.is_empty());
        if !any_roles && !any_requires {
            return None;
        }
        let path = self.output_dir.join("src/lib/roles.ts");
        if path.exists() {
            return None;
        }
        let content = match &model.policy {
            Some(policy) => policy_roles_ts(policy),
            None if any_requires => REQUIRES_ONLY_ROLES_TS.to_string(),
            None => ROLES_TS.to_string(),
        };
        Some(GeneratedFile { path, content })
    }
}

#[async_trait]
impl GlobalGenerator for IfmlRouteGenerator {
    fn kind(&self) -> GlobalGeneratorKind {
        GlobalGeneratorKind::IfmlRoute
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        config: &DomainConfig,
        _generation_order: &[GenerationEntry],
        tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        let querier = IfmlGraphQuerier::new(db);
        let model = querier
            .get_ifml_model()
            .await
            .map_err(crate::error::Error::Graph)?;

        if model.view_containers.is_empty() {
            // Issue #468 finding 1: an enabled route generator over an
            // empty IFML model used to exit silently — the profile looked
            // wired while emitting nothing. Say so instead.
            eprintln!(
                "warning: ifml_route: no IFML model in the graph \
                 (pass --ifml-files); no view routes emitted"
            );
            return Ok(vec![]);
        }

        let mut files = vec![];

        let page_template = format!("ifml/{}/page.tera", self.framework);
        let load_template = format!("ifml/{}/page_load.tera", self.framework);

        let modal_targets: HashSet<String> = model
            .view_containers
            .iter()
            .filter(|vc| modal_wrapper_active(vc.is_modal, &vc.name, self.mappings.as_ref()))
            .map(|vc| vc.name.clone())
            .collect();
        let denial = denial_target(&model);

        // Issue #301: resolve the ux plane ONCE per generation — one plan
        // per distinct bound entity drives the timeline layouts and the
        // advisory diagnostics (printed through the stderr warning channel,
        // deduped like the entity pipeline's ui-page generator). Flag off ⇒
        // no plans, no lines, byte-identical output.
        let (ux_plans, ux_diag_lines) =
            resolve_generation_ux(db, config, &model, project.ux.ux.as_ref()).await?;
        for line in &ux_diag_lines {
            eprintln!("warning: ux-rules: {line}");
        }
        let ux_generation = project.ux.ux.as_ref().map(|rules| UxGeneration {
            rules,
            plans: &ux_plans,
        });

        for vc in ordered_view_containers(&model) {
            let ctx = build_page_context(
                db,
                config,
                &project.identity.api_version,
                vc,
                self.mappings.as_ref(),
                &modal_targets,
                ux_generation.as_ref(),
                project,
            )
            .await?;

            match render_template(tera, &page_template, &ctx) {
                Ok(content) => files.push(GeneratedFile {
                    path: self
                        .output_dir
                        .join((self.output_paths.route_page)(&vc.name)),
                    content,
                }),
                Err(e) => eprintln!(
                    "warning: ifml_route: {} render failed for view '{}': {e}",
                    page_template, vc.name
                ),
            }

            if let Some(ref route_load_fn) = self.output_paths.route_load {
                let load_ctx = build_load_context(
                    &project.identity.api_version,
                    vc,
                    &ctx.components,
                    &denial,
                    self.output_paths.route_root() == Some(ROUTE_GROUP),
                );
                match render_template(tera, &load_template, &load_ctx) {
                    Ok(content) => files.push(GeneratedFile {
                        path: self.output_dir.join(route_load_fn(&vc.name)),
                        content,
                    }),
                    Err(e) => eprintln!(
                        "warning: ifml_route: {} render failed for view '{}': {e}",
                        load_template, vc.name
                    ),
                }
            }
        }

        if let Some(roles_helper) = self.roles_helper(&model) {
            files.push(roles_helper);
        }

        if let Some(hydration) = self.hydration(&model) {
            files.extend(hydration);
        }

        if let Some(ctx) = self.layout_context(&model) {
            let layout_template = format!("ifml/{}/layout.tera", self.framework);
            match render_template(tera, &layout_template, &ctx) {
                Ok(content) => {
                    if let Some(rel) = self.route_layout() {
                        files.push(GeneratedFile {
                            path: self.output_dir.join(rel),
                            content,
                        });
                    }
                }
                Err(e) => eprintln!("warning: ifml_route: {layout_template} render failed: {e}"),
            }
        }

        Ok(files)
    }
}

/// `$lib/roles` helper emitted once per project when any view carries roles.
/// Consumers with real auth populate `__USER_ROLES__` (e.g. from
/// `+layout.ts` server data); unset means no roles and every guarded page
/// redirects to the denial target.
const ROLES_TS: &str = r#"// Generated by codegraph. DO NOT EDIT.
export function currentRoles(): string[] {
	return (globalThis as any).__USER_ROLES__ ?? [];
}
"#;

/// `$lib/roles` helper variant for models with capability requirements but
/// no ingested policy: `can()` consults the runtime `__USER_CAPABILITIES__`
/// override only.
const REQUIRES_ONLY_ROLES_TS: &str = r#"// Generated by codegraph. DO NOT EDIT.
export function currentRoles(): string[] {
	return (globalThis as any).__USER_ROLES__ ?? [];
}

export function can(capability: string): boolean {
	const held: string[] = (globalThis as any).__USER_CAPABILITIES__ ?? [];
	return held.includes(capability);
}
"#;

/// `$lib/roles` helper for models with an ingested policy: the
/// generation-time actor→effective-capabilities map (resolved through
/// `extends` with forbid-wins) unioned with the runtime
/// `__USER_CAPABILITIES__` override.
fn policy_roles_ts(policy: &super::context::PolicyContext) -> String {
    let mut ts = String::from(
        "// Generated by codegraph. DO NOT EDIT.\n\
         export function currentRoles(): string[] {\n\
         \treturn (globalThis as any).__USER_ROLES__ ?? [];\n\
         }\n\n\
         const ROLE_CAPABILITIES: Record<string, string[]> = {\n",
    );
    for (actor, caps) in &policy.actors {
        let list = caps
            .iter()
            .map(|cap| js_quote(cap))
            .collect::<Vec<_>>()
            .join(", ");
        ts.push_str(&format!("\t{}: [{}],\n", js_quote(actor), list));
    }
    ts.push_str(
        "};\n\n\
         export function can(capability: string): boolean {\n\
         \tconst held = new Set<string>((globalThis as any).__USER_CAPABILITIES__ ?? []);\n\
         \tfor (const role of currentRoles()) {\n\
         \t\tfor (const cap of ROLE_CAPABILITIES[role] ?? []) {\n\
         \t\t\theld.add(cap);\n\
         \t\t}\n\
         \t}\n\
         \treturn held.has(capability);\n\
         }\n",
    );
    ts
}

/// The denial redirect target for guarded views: the first view (graph
/// order) carrying neither roles nor capability requirements — a page any
/// denied visitor can see — else `/`.
pub(crate) fn denial_target(model: &super::context::IfmlModel) -> String {
    model
        .view_containers
        .iter()
        .find(|vc| vc.roles.is_empty() && vc.requires.is_empty())
        .map(|vc| format!("/{}", vc.name.to_lowercase()))
        .unwrap_or_else(|| "/".to_string())
}

/// Order view containers by the model's computed generation order
/// (targets before sources). Containers absent from the order keep their
/// graph order at the end.
fn ordered_view_containers(
    model: &super::context::IfmlModel,
) -> Vec<&super::context::IfmlViewContainer> {
    let position: HashMap<&str, usize> = model
        .generation_order
        .iter()
        .enumerate()
        .map(|(i, name)| (name.as_str(), i))
        .collect();
    let mut containers: Vec<&super::context::IfmlViewContainer> =
        model.view_containers.iter().collect();
    containers.sort_by_key(|vc| {
        position
            .get(vc.name.as_str())
            .copied()
            .unwrap_or(usize::MAX)
    });
    containers
}
