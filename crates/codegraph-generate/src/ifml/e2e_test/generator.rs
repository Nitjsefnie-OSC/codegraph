//! The IFML e2e generator (issue #317 module split): per-view spec
//! assembly, POM emission (kernel + page classes), stale-file cleanup, and
//! the Playwright harness scaffold.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::GenerationEntry;
use crate::ProjectConfig;
use crate::error::Result;
use crate::traits::{GeneratedFile, GlobalGenerator, GlobalGeneratorKind};
use async_trait::async_trait;
use codegraph_config::{DomainConfig, IfmlComponentMappings};
use codegraph_core::traits::GraphQuerier;

use super::super::context::{IfmlComponent, IfmlViewContainer};
use super::super::querier::{IfmlGraphQuerier, IfmlQuerier};
use super::super::route_generator::denial_target;
use super::super::selectors::ComponentSelectors;
use super::pom::{ViewPom, pom_file_set};
use super::render::{
    package_json, playwright_config, render_spec, render_ux_spec, render_workflow_spec,
};
use super::spec_payload::{UxViewTest, ViewTestSpec, WorkflowTest};
use super::ux_plans::build_ux_plans;
use codegraph_naming::to_kebab_case;

/// Global generator emitting IFML-driven Playwright E2E specs.
///
/// One spec file per view container under `{fw}/tests/ifml/{view-kebab}.spec.ts`,
/// with test kinds selected by what the model supports:
///
/// - render (always): goto the view route, expect the heading + primary
///   component testid.
/// - click-through (schema-backed API required): seed a fixture entity via the
///   API, click a row, assert the navigation-flow target URL with the
///   parameter bindings substituted.
/// - form validation negative (typed form spec with required fields): submit
///   empty, assert the input is in the invalid state.
/// - form CRUD round trip (schema-backed API + save navigation): create a
///   fixture, load the edit form, submit a modified value, assert the save
///   navigation and API persistence.
///
/// API-dependent kinds are skipped when the bound entity has no schema in the
/// graph (e.g. `ifml-generate` without `--schemas`), so a schema-less run
/// emits render tests only.
///
/// Since #317 every spec kind drives the view through the emitted POM
/// (kernel + per-view page classes under `tests/pages/`), which is emitted
/// on the same spec-infra gate: whenever any spec is emitted.
///
/// With `project.codegen.ifml_e2e_auth` (issue #463) the generator
/// additionally emits the test-side auth bootstrap —
/// `tests/e2e/auth.setup.ts` (per-persona API-key provisioning), the
/// `tests/e2e/personas.ts` fixtures, the `{view}.auth.spec.ts` family, the
/// `tests/journeys/*.journey.spec.ts` specs, and the extended POM surface
/// (`{comp}DeleteViaMenu`/`press{Comp}Cancel`) — and moves the persona
/// coverage out of the base specs' inline `addInitScript` blocks. Issue
/// #468 W5 extends the same flag-gated family with the navigation
/// journeys (`{view}.journey.spec.ts`: the navigate() graph as chained
/// steps asserting each landing URL + visible surface), the persona
/// gating describes (denied personas hit the redirect AND a hidden
/// surface; skipped with a comment without a differing-access persona
/// pair), and the wizard POM surface (`step(name)` on xor views plus the
/// `journeyTo{Target}` hop methods). Flag off keeps every artifact
/// byte-identical.
pub struct IfmlE2eTestGenerator {
    output_dir: PathBuf,
    framework: String,
    pub(super) mappings: Option<IfmlComponentMappings>,
    /// Integrated mode (issue #468 W1b): the specs share the entity ui
    /// app's playwright run, so the IFML tree moves under `tests/ifml/` —
    /// journeys included — and the auth bootstrap lands in
    /// `tests/ifml/support/` instead of `tests/e2e/` (no collision with the
    /// entity suite's `tests/e2e/fixtures/personas.ts`).
    integrated: bool,
}

/// Where the auth-bootstrap family lives, relative to the output root.
struct SupportPaths {
    auth_setup: &'static str,
    personas: &'static str,
    /// The import specifier the IFML specs/journeys use for the personas
    /// module (relative to `tests/ifml/`).
    personas_import: &'static str,
}

const STANDALONE_SUPPORT: SupportPaths = SupportPaths {
    auth_setup: "tests/e2e/auth.setup.ts",
    personas: "tests/e2e/personas.ts",
    personas_import: "../e2e/personas",
};

const INTEGRATED_SUPPORT: SupportPaths = SupportPaths {
    auth_setup: "tests/ifml/support/auth.setup.ts",
    personas: "tests/ifml/support/personas.ts",
    personas_import: "./support/personas",
};

impl IfmlE2eTestGenerator {
    pub fn new(output_dir: &Path, framework: &str) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
            framework: framework.to_string(),
            mappings: None,
            integrated: false,
        }
    }

    pub fn with_mappings(mut self, mappings: Option<IfmlComponentMappings>) -> Self {
        self.mappings = mappings;
        self
    }

    /// Integrated mode (issue #468 W1b): journeys co-locate with the specs
    /// under `tests/ifml/` and the auth bootstrap moves to
    /// `tests/ifml/support/`.
    pub fn with_integration(mut self) -> Self {
        self.integrated = true;
        self
    }

    fn support(&self) -> &'static SupportPaths {
        if self.integrated {
            &INTEGRATED_SUPPORT
        } else {
            &STANDALONE_SUPPORT
        }
    }

    /// Journey specs: standalone they live in their own `tests/journeys/`
    /// directory; integrated they join the specs under `tests/ifml/` so the
    /// single ui playwright config's `ifml` project picks them up.
    fn journeys_dir(&self) -> PathBuf {
        if self.integrated {
            self.output_dir.join("tests").join("ifml")
        } else {
            self.output_dir.join("tests").join("journeys")
        }
    }

    pub(super) fn selectors(
        &self,
        vc: &IfmlViewContainer,
        c: &IfmlComponent,
    ) -> ComponentSelectors {
        ComponentSelectors::for_component(self.mappings.as_ref(), &vc.name, c)
    }
}

#[async_trait]
impl GlobalGenerator for IfmlE2eTestGenerator {
    fn kind(&self) -> GlobalGeneratorKind {
        GlobalGeneratorKind::IfmlE2eTest
    }

    async fn generate(
        &self,
        db: &dyn GraphQuerier,
        config: &DomainConfig,
        _generation_order: &[GenerationEntry],
        _tera: &tera::Tera,
        project: &ProjectConfig,
    ) -> Result<Vec<GeneratedFile>> {
        if self.framework != "svelte" {
            return Ok(vec![]);
        }

        let querier = IfmlGraphQuerier::new(db);
        let model = querier
            .get_ifml_model()
            .await
            .map_err(crate::error::Error::Graph)?;
        if model.view_containers.is_empty() {
            // Issue #468 finding 1: enabled-but-empty used to be silent.
            eprintln!(
                "warning: ifml_e2e_test: no IFML model in the graph \
                 (pass --ifml-files); no view specs emitted"
            );
            return Ok(vec![]);
        }

        // Test-side auth bootstrap (issue #463): flag off ⇒ no personas,
        // no auth specs, no journeys — byte-identical output.
        let auth_enabled = project.codegen.ifml_e2e_auth;

        let mut specs = Vec::new();
        let mut workflow_specs: Vec<(String, Vec<WorkflowTest>)> = Vec::new();
        // ux-rules plane (issue #303): one plan per bound entity plus one
        // ux spec per view with an eligible fallback list. Flag off → no
        // plans, no ux specs (byte-identical output).
        let ux_rules = project.ux.ux.as_ref();
        let ux_plans = match ux_rules {
            Some(rules) => build_ux_plans(db, config, &model, rules).await?,
            None => HashMap::new(),
        };
        let mut ux_specs: Vec<(String, UxViewTest)> = Vec::new();
        let human_actors: Vec<String> = if auth_enabled || model.policy.is_some() {
            let mut names: Vec<String> = db
                .get_actors()
                .await
                .map_err(crate::error::Error::Graph)?
                .into_iter()
                .filter(|actor| actor.kind.as_deref() != Some("agent"))
                .map(|actor| actor.name)
                .collect();
            // Actors repeat across per-domain policies (every .actor
            // re-imports the core ladder — the PolicyContext dedup fixed
            // the capability map, this fixes the list): one persona per
            // name, or the gating specs emit duplicate titles and
            // Playwright aborts the run (issue #468).
            names.sort();
            names.dedup();
            names
        } else {
            Vec::new()
        };
        let personas = if auth_enabled {
            super::auth::build_personas(&model, &human_actors)
        } else {
            Vec::new()
        };
        let denial = denial_target(&model);
        for vc in &model.view_containers {
            let mut spec = self
                .build_view_spec(db, config, &project.identity.api_version, &model, vc)
                .await;
            if let Some(policy) = &model.policy {
                spec.personas = self.build_persona_tests(policy, &human_actors, vc, &denial);
            }
            if has_tests(&spec) {
                specs.push(spec);
            }
            if let Some(rules) = ux_rules
                && let Some(ux_test) = self
                    .build_view_ux_test(
                        db,
                        config,
                        &project.identity.api_version,
                        vc,
                        rules,
                        &ux_plans,
                    )
                    .await
            {
                ux_specs.push((vc.name.clone(), ux_test));
            }
            let workflow_tests = self
                .build_view_workflow_tests(db, config, &project.identity.api_version, vc)
                .await;
            if !workflow_tests.is_empty() {
                workflow_specs.push((vc.name.clone(), workflow_tests));
            }
        }
        if specs.is_empty() && workflow_specs.is_empty() {
            eprintln!(
                "warning: ifml_e2e_test: model has {} view(s) but none assembled \
                 a render/click-through/validation/round-trip spec",
                model.view_containers.len()
            );
            return Ok(vec![]);
        }

        // Remove stale per-view spec files for views no longer in the model,
        // stale workflow specs whose view no longer carries workflow
        // tests (e.g. the workflow config or a mapped collection changed
        // across regenerations into the same root), stale ux specs
        // (issue #303) whose view lost its ux eligibility (flag off, view
        // removed, or a mapping now replaces the fallback), stale auth
        // specs (issue #463) whose view lost its guard or the flag turned
        // off, and stale POM page classes (#317) for removed views.
        let active_specs: HashSet<String> = model
            .view_containers
            .iter()
            .map(|vc| format!("{}.spec.ts", to_kebab_case(&vc.name)))
            .collect();
        let active_workflow_specs: HashSet<String> = workflow_specs
            .iter()
            .map(|(view_name, _)| format!("{}.workflow.spec.ts", to_kebab_case(view_name)))
            .collect();
        let active_ux_specs: HashSet<String> = ux_specs
            .iter()
            .map(|(view_name, _)| format!("{}.ux.spec.ts", to_kebab_case(view_name)))
            .collect();
        let active_auth_specs: HashSet<String> = if auth_enabled {
            model
                .view_containers
                .iter()
                .filter(|vc| !vc.roles.is_empty() || !vc.requires.is_empty())
                .map(|vc| format!("{}.auth.spec.ts", to_kebab_case(&vc.name)))
                .collect()
        } else {
            HashSet::new()
        };
        let active_pages: HashSet<String> = model
            .view_containers
            .iter()
            .map(|vc| format!("{}-page.ts", to_kebab_case(&vc.name)))
            .collect();
        let specs_dir = self.output_dir.join("tests").join("ifml");
        if specs_dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(&specs_dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !name.ends_with(".spec.ts") {
                    continue;
                }
                if name.strip_suffix(".workflow.spec.ts").is_some() {
                    if !active_workflow_specs.contains(name) {
                        let _ = std::fs::remove_file(&path);
                    }
                    continue;
                }
                if name.strip_suffix(".ux.spec.ts").is_some() {
                    if !active_ux_specs.contains(name) {
                        let _ = std::fs::remove_file(&path);
                    }
                    continue;
                }
                if name.strip_suffix(".auth.spec.ts").is_some() {
                    if !active_auth_specs.contains(name) {
                        let _ = std::fs::remove_file(&path);
                    }
                    continue;
                }
                if name.ends_with(".journey.spec.ts") {
                    // Journeys are kept/swept by the journeys pass below
                    // (integrated mode co-locates them in this directory).
                    continue;
                }
                if !active_specs.contains(name) {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        let pages_dir = self.output_dir.join("tests").join("pages");
        if pages_dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(&pages_dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                // Only page classes are view-scoped; `support/` holds the
                // stable kernel.
                if name.ends_with("-page.ts") && !active_pages.contains(name) {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }

        // The journeys (issue #463): workflow handoffs + the shell nav
        // walk. Issue #468 W5 adds the per-view navigate() walks — built
        // BEFORE the POM plans so the page classes carry the hop methods.
        let workflow_journeys = if auth_enabled {
            super::auth::build_workflow_journeys(
                db,
                config,
                &project.identity.api_version,
                &model,
                &personas,
            )
            .await
        } else {
            Vec::new()
        };
        let shell_journey = if auth_enabled {
            super::auth::build_shell_journey(&model, &personas)
        } else {
            None
        };
        let mut view_journeys = if auth_enabled {
            self.build_view_journeys(db, config, &project.identity.api_version, &model, &personas)
                .await
        } else {
            Vec::new()
        };

        // The POM plans (built once — the auth-spec assembly reads the same
        // plan the page classes render from). Journey hops (issue #468 W5)
        // thread into the source views' plans; the method names are frozen
        // in the journey payloads.
        let mut journey_steps_by_view: HashMap<String, Vec<super::pom::PomJourneyStep>> =
            HashMap::new();
        for journey in &view_journeys {
            for path in &journey.paths {
                // A hop's method lives on the page class of the view it
                // DEPARTS from (`path.views[i]`), not the journey source.
                for (departure, step) in path.views.iter().zip(&path.steps) {
                    let entry = journey_steps_by_view.entry(departure.clone()).or_default();
                    if entry.iter().any(|s| s.method == step.method) {
                        continue;
                    }
                    entry.push(super::pom::PomJourneyStep {
                        method: step.method.clone(),
                        component: step.hop.source_component.clone(),
                        row: step.hop.kind == super::spec_payload::JourneyHopKind::Row,
                        submit: step.hop.kind == super::spec_payload::JourneyHopKind::Submit,
                        fills: step.hop.fills.clone(),
                        pattern: step.hop.target_pattern.clone(),
                    });
                }
            }
        }
        let spec_by_view: HashMap<&str, &ViewTestSpec> = specs
            .iter()
            .map(|spec| (spec.view_name.as_str(), spec))
            .collect();
        let pom_plans: Vec<(String, ViewPom)> = model
            .view_containers
            .iter()
            .map(|vc| {
                let spec = spec_by_view.get(vc.name.as_str()).copied();
                let kebab = to_kebab_case(&vc.name);
                let empty_steps = Vec::new();
                let journey_steps = journey_steps_by_view.get(&vc.name).unwrap_or(&empty_steps);
                let pom = super::pom::build_view_pom(
                    config,
                    self.mappings.as_ref(),
                    vc,
                    spec,
                    ux_rules,
                    &ux_plans,
                    auth_enabled,
                    journey_steps,
                );
                (kebab, pom)
            })
            .collect();
        let pom_by_view: HashMap<&str, &ViewPom> = pom_plans
            .iter()
            .map(|(kebab, pom)| (kebab.as_str(), pom))
            .collect();

        // Resolve the journeys' arrival assertions against the POM plans
        // (the surfaces each target page class actually exposes).
        let pom_refs_by_name: HashMap<String, &ViewPom> = pom_by_view
            .iter()
            .map(|(kebab, pom)| ((*kebab).to_string(), *pom))
            .collect();
        super::assembly::resolve_journey_arrivals(&mut view_journeys, &pom_refs_by_name);
        view_journeys.retain(|journey| !journey.paths.is_empty());

        // The auth family (issue #463): one `{view}.auth.spec.ts` per
        // guarded view, built from the same payloads + POM plan.
        let auth_specs = if auth_enabled {
            model
                .view_containers
                .iter()
                .filter_map(|vc| {
                    let spec = spec_by_view.get(vc.name.as_str())?;
                    let pom = pom_by_view.get(to_kebab_case(&vc.name).as_str())?;
                    super::auth::build_auth_spec(vc, spec, pom)
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let mut files: Vec<GeneratedFile> = specs
            .iter()
            .map(|spec| GeneratedFile {
                path: self
                    .output_dir
                    .join("tests")
                    .join("ifml")
                    .join(format!("{}.spec.ts", to_kebab_case(&spec.view_name))),
                // Under the auth flag the persona tests move to the
                // `{view}.auth.spec.ts` family; a persona-only payload
                // renders an empty base describe (the file itself stays —
                // the suite layout is stable across flag toggles).
                content: render_spec(spec, !auth_enabled),
            })
            .collect();

        for (view_name, workflow_tests) in &workflow_specs {
            if let Some(vc) = model
                .view_containers
                .iter()
                .find(|vc| &vc.name == view_name)
            {
                files.push(GeneratedFile {
                    path: self
                        .output_dir
                        .join("tests")
                        .join("ifml")
                        .join(format!("{}.workflow.spec.ts", to_kebab_case(view_name))),
                    content: render_workflow_spec(vc, workflow_tests),
                });
            }
        }

        for (view_name, ux_test) in &ux_specs {
            files.push(GeneratedFile {
                path: self
                    .output_dir
                    .join("tests")
                    .join("ifml")
                    .join(format!("{}.ux.spec.ts", to_kebab_case(view_name))),
                content: render_ux_spec(view_name, ux_test),
            });
        }

        for auth_spec in &auth_specs {
            files.push(GeneratedFile {
                path: self.output_dir.join("tests").join("ifml").join(format!(
                    "{}.auth.spec.ts",
                    to_kebab_case(&auth_spec.view_name)
                )),
                content: super::auth::render_auth_spec(auth_spec, self.support().personas_import),
            });
        }

        let journeys_dir = self.journeys_dir();
        for journey in &workflow_journeys {
            files.push(GeneratedFile {
                path: journeys_dir.join(format!(
                    "{}-workflow.journey.spec.ts",
                    to_kebab_case(&journey.entity)
                )),
                content: super::auth::render_workflow_journey(
                    journey,
                    self.support().personas_import,
                ),
            });
        }
        if let Some(journey) = &shell_journey {
            files.push(GeneratedFile {
                path: journeys_dir.join("shell-nav.journey.spec.ts"),
                content: super::auth::render_shell_nav_journey(
                    journey,
                    self.support().personas_import,
                ),
            });
        }

        // The per-view navigate() walks (issue #468 W5).
        for journey in &view_journeys {
            files.push(GeneratedFile {
                path: journeys_dir.join(format!(
                    "{}.journey.spec.ts",
                    to_kebab_case(&journey.view_name)
                )),
                content: super::auth::render_view_journey(journey, self.support().personas_import),
            });
        }

        // Journey stale sweep: flag off (or a lost journey) removes the
        // file — workflow handoffs, the shell nav walk, AND the per-view
        // navigate() walks; the auth TS bootstrap likewise never lingers.
        let active_journeys: HashSet<String> = if auth_enabled {
            super::auth::journey_file_names(&workflow_journeys)
                .into_iter()
                .chain(super::auth::view_journey_file_names(&view_journeys))
                .collect()
        } else {
            HashSet::new()
        };
        let journeys_dir = self.journeys_dir();
        if journeys_dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(&journeys_dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if name.ends_with(".journey.spec.ts") && !active_journeys.contains(name) {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        if !auth_enabled {
            let support = self.support();
            for rel in [support.auth_setup, support.personas] {
                let path = self.output_dir.join(rel);
                if path.exists() {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }

        // POM infra (issue #317): kernel + one page class per view
        // container, emitted on the same spec-infra gate as the specs.
        let pom_refs: Vec<(&str, &ViewPom)> = pom_plans
            .iter()
            .map(|(kebab, pom)| (kebab.as_str(), pom))
            .collect();
        for (path, content) in pom_file_set(&pom_refs) {
            files.push(GeneratedFile {
                path: self.output_dir.join(path),
                content,
            });
        }

        // The auth bootstrap (issue #463): per-persona API-key provisioning
        // + the shared persona fixtures. Integrated mode keeps the family
        // out of the entity suite's `tests/e2e/` tree (issue #468 W1b).
        if auth_enabled && !personas.is_empty() {
            let support = self.support();
            files.push(GeneratedFile {
                path: self.output_dir.join(support.auth_setup),
                content: super::auth::render_auth_setup(&personas),
            });
            files.push(GeneratedFile {
                path: self.output_dir.join(support.personas),
                content: super::auth::render_personas(&personas),
            });
        }

        // Standalone mode owns its harness files (if-absent, so a real app's
        // hand-edits survive). Integrated mode NEVER emits them: the entity
        // scaffold's playwright.config.ts + package.json are authoritative,
        // and an if-absent check here would race the scaffold on a fresh ui
        // tree (issue #468 W1b).
        if !self.integrated {
            let config_path = self.output_dir.join("playwright.config.ts");
            if !config_path.exists() {
                let global_setup =
                    (auth_enabled && !personas.is_empty()).then_some("tests/e2e/auth.setup.ts");
                files.push(GeneratedFile {
                    path: config_path,
                    content: playwright_config(global_setup).to_string(),
                });
            }
            let package_path = self.output_dir.join("package.json");
            if !package_path.exists() {
                files.push(GeneratedFile {
                    path: package_path,
                    content: package_json().to_string(),
                });
            }
        }

        Ok(files)
    }
}

fn has_tests(spec: &ViewTestSpec) -> bool {
    spec.render.is_some()
        || !spec.personas.is_empty()
        || !spec.click_throughs.is_empty()
        || !spec.validations.is_empty()
        || !spec.round_trips.is_empty()
}
