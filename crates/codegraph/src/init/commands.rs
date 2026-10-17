//! Command implementations for `codegraph init` / `doctor` / `add domain`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::init::context::{ProjectFeatures, ProjectTemplateContext};

/// Non-interactive options for `codegraph init`. When `name` is None the
/// command prompts interactively on stdin.
#[derive(Debug, Clone)]
pub struct InitArgs {
    /// Project name (kebab-case). None = prompt.
    pub name: Option<String>,
    /// Directory to create the project in (default: `./{name}`).
    pub output_dir: PathBuf,
    /// Domain names (snake or kebab). Default: `["common"]`.
    pub domains: Vec<String>,
    pub database_target: String,
    pub persistence_provider: String,
    pub deployment_topology: String,
    pub grpc: bool,
    pub ifml: bool,
    pub ops: bool,
    /// Rosetta-first scaffold: `model/<domain>.rosetta` starters instead of
    /// `.mox`, plus rosetta-first profiles/justfile/ops-manifest wiring.
    pub rosetta: bool,
    /// Codegraph git rev to pin (default: the running binary's embedded rev).
    pub rev: Option<String>,
    /// Use local path deps for codegraph crates instead of git+rev.
    pub codegraph_path: Option<PathBuf>,
    /// Overwrite existing files.
    pub force: bool,
    /// Additional template directories (later dirs take precedence).
    pub template_dirs: Vec<PathBuf>,
}

/// Normalize a raw project name to kebab-case. Handles "My App" → "my-app",
/// "Already_Snake" → "already-snake", and passes "demo-app" through unchanged.
fn normalize_project_name(raw: &str) -> String {
    let snake: String = heck::ToSnakeCase::to_snake_case(raw.trim());
    heck::ToKebabCase::to_kebab_case(snake.as_str())
}

/// Normalize a raw domain name to snake_case ("Billing" → "billing",
/// "Order Items" → "order_items").
fn normalize_domain_name(raw: &str) -> String {
    heck::ToSnakeCase::to_snake_case(raw.trim())
}

/// Resolve the target project directory.
///
/// Convention: `--output` names the *parent* directory. The default `"."`
/// yields `./{name}`; an explicit `--output /tmp/foo` yields
/// `/tmp/foo/{name}`. When the output dir's basename already equals the
/// project name it is treated as the project dir itself.
fn resolve_target_dir(output_dir: &Path, name: &str) -> PathBuf {
    if output_dir.as_os_str().is_empty() || output_dir == Path::new(".") {
        PathBuf::from(name)
    } else if output_dir.file_name().and_then(|f| f.to_str()) == Some(name) {
        output_dir.to_path_buf()
    } else {
        output_dir.join(name)
    }
}

/// Return the final paths (relative to `target`) that already exist on disk.
fn would_overwrite(target: &Path, files: &[(PathBuf, String)]) -> Vec<PathBuf> {
    files
        .iter()
        .map(|(rel, _)| target.join(rel))
        .filter(|p| p.exists())
        .collect()
}

/// Prompt for a project name on stdin until a non-blank value is given.
fn prompt_project_name() -> Result<String> {
    loop {
        print!("Project name (kebab-case): ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            eprintln!("project name cannot be empty — try again");
            continue;
        }
        return Ok(normalize_project_name(trimmed));
    }
}

/// Print a compact grouped file listing for the scaffold summary.
fn print_file_tree(rel_paths: &[PathBuf]) {
    let mut sorted: Vec<&PathBuf> = rel_paths.iter().collect();
    sorted.sort();
    let mut last_dir: Option<PathBuf> = None;
    for rel in sorted {
        let dir = rel.parent().map(|d| d.to_path_buf()).unwrap_or_default();
        if last_dir.as_deref() != Some(dir.as_path()) {
            let label = if dir.as_os_str().is_empty() {
                ".".to_string()
            } else {
                dir.display().to_string()
            };
            println!("{label}");
            last_dir = Some(dir);
        }
        let name = rel
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        println!("  {name}");
    }
}

/// Scaffold a new consumer project. Refuses to overwrite existing files
/// unless `force`; writes only inside `output_dir` (path containment guard).
pub fn cmd_init(args: &InitArgs) -> Result<()> {
    let name = match &args.name {
        Some(raw) => normalize_project_name(raw),
        None => prompt_project_name()?,
    };
    if name.is_empty() {
        return Err(Error::Config("project name cannot be empty".to_string()));
    }

    let target_dir = resolve_target_dir(&args.output_dir, &name);
    fs::create_dir_all(&target_dir)?;
    let canonical_target = target_dir.canonicalize().map_err(|e| {
        Error::Config(format!(
            "cannot resolve project dir '{}': {e}",
            target_dir.display()
        ))
    })?;

    let rev = match &args.rev {
        Some(r) if !r.trim().is_empty() => r.trim().to_string(),
        _ => {
            let embedded = crate::rev::codegraph_rev();
            if embedded.is_empty() {
                eprintln!(
                    "WARN: no --rev given and this binary has no embedded git rev; \
                     the generated Cargo.toml will pin no codegraph revision"
                );
            }
            embedded.to_string()
        }
    };

    let codegraph_path = match &args.codegraph_path {
        Some(p) => Some(p.canonicalize().map_err(|e| {
            Error::Config(format!(
                "--codegraph-path '{}' is not accessible: {e}",
                p.display()
            ))
        })?),
        None => None,
    };

    let domains: Vec<String> = args
        .domains
        .iter()
        .map(|d| normalize_domain_name(d))
        .collect();

    let features = ProjectFeatures {
        grpc: args.grpc,
        ifml: args.ifml,
        ops: args.ops,
        rosetta: args.rosetta,
    };

    let ctx = ProjectTemplateContext::new(
        &name,
        &domains,
        &rev,
        codegraph_path.as_deref(),
        &args.database_target,
        &args.persistence_provider,
        &args.deployment_topology,
        features,
    );

    let tera = if args.template_dirs.is_empty() {
        let td = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
        crate::generate::template_engine::create_tera(&td)?
    } else {
        let dirs: Vec<&Path> = args.template_dirs.iter().map(|p| p.as_path()).collect();
        crate::generate::template_engine::create_tera_with_overrides(&dirs)?
    };

    let files = ctx.render(&tera).map_err(Error::Template)?;

    // Rosetta starters are sigil-verified BEFORE anything is written — a
    // broken starter template is a generation-time hard error, never a
    // broken scaffold on disk (the mox compile-verify precedent).
    if args.rosetta {
        let rosetta_sources: Vec<crate::init::rosetta_model::RosettaFileCheck> = files
            .iter()
            .filter(|(p, _)| p.extension().and_then(|e| e.to_str()) == Some("rosetta"))
            .map(|(p, c)| crate::init::rosetta_model::RosettaFileCheck {
                name: p.display().to_string(),
                text: c.clone(),
            })
            .collect();
        let verification = crate::init::rosetta_model::verify_rosetta_sources(&rosetta_sources);
        if !verification.hard_errors.is_empty() {
            return Err(Error::Config(format!(
                "starter rosetta model(s) failed sigil verification — refusing to write: {}",
                verification.hard_errors.join("; ")
            )));
        }
    }

    if !args.force {
        let existing = would_overwrite(&target_dir, &files);
        if !existing.is_empty() {
            let mut msg = String::from("refusing to overwrite existing files (use --force):\n");
            for path in &existing {
                msg.push_str(&format!("  {}\n", path.display()));
            }
            return Err(Error::Config(msg.trim_end().to_string()));
        }
    }

    let mut written = Vec::with_capacity(files.len());
    for (rel, content) in &files {
        let final_path = target_dir.join(rel);
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
            let canonical_parent = parent.canonicalize().map_err(|e| {
                Error::Config(format!(
                    "cannot resolve parent dir '{}': {e}",
                    parent.display()
                ))
            })?;
            if !canonical_parent.starts_with(&canonical_target) {
                return Err(Error::Config(format!(
                    "refusing to write outside the project dir: '{}'",
                    final_path.display()
                )));
            }
        }
        fs::write(&final_path, content)?;
        written.push(rel.clone());
    }

    print_file_tree(&written);
    println!();
    println!(
        "Project scaffolded in {} ({} files). Next: just generate",
        target_dir.display(),
        written.len()
    );
    Ok(())
}

/// Options for `codegraph doctor`.
#[derive(Debug, Clone)]
pub struct DoctorArgs {
    /// domains.toml path (default: "domains.toml").
    pub config: PathBuf,
    /// JSON schemas dir. None = not provided: in mox mode the check
    /// degrades to an info line (mox-first projects carry no schemas
    /// directory); without mox files it stays a hard failure.
    pub schemas: Option<PathBuf>,
    /// classifier.toml path. None = not provided: only a hard failure when
    /// the schemas dir (when given) contains JSON schemas.
    pub classifier: Option<PathBuf>,
    /// profiles.toml path (optional; skipped when absent and profile is default).
    pub profiles_config: Option<PathBuf>,
    /// rexlang .mox domain model files (optional). When present each
    /// package must match a domains.toml domain, and the JSON schemas
    /// check degrades to an info line (mox-first projects).
    pub mox_files: Vec<PathBuf>,
    /// Rosetta (Rune DSL) .rosetta model files (optional). Verified through
    /// the sigil pipeline (parse → lower → resolve); a namespace whose last
    /// segment matches no domains.toml key warns (compute_generation_order
    /// silently drops such schemas).
    pub rosetta_files: Vec<PathBuf>,
    /// rexlang .ddd design files (optional). Compile-verified against their
    /// imported .mox domains (the same read+collect+compile the ingest
    /// path uses); error-severity diagnostics are hard failures.
    pub ddd_files: Vec<PathBuf>,
    /// rexlang .evt event-contract files (optional). Compile-verified
    /// against their imported .mox domains (the same read+collect+compile
    /// the ingest path uses); error-severity diagnostics are hard failures.
    pub evt_files: Vec<PathBuf>,
    /// rexlang .deploy deployment files (optional). Compile-verified
    /// against their imported .ddd designs and .ifml flows (the same
    /// read+resolve+compile the ingest path uses), then cross-checked
    /// against the resolved BuildPlan topology and the domains.toml
    /// component coverage; error-severity diagnostics are hard failures.
    pub deploy_files: Vec<PathBuf>,
    /// rexlang .ifml interaction-model files (optional). Parse-verified
    /// through the same `rex_ifml::parse_ifml_file` the ingest path uses;
    /// `import "….actor"` statements are resolved read-only through
    /// `resolve_actor_policy` (a policy the run path would silently skip
    /// is a hard failure here). Parse errors are hard failures.
    pub ifml_files: Vec<PathBuf>,
}

/// Outcome counts for a doctor run. `model_warnings` isolates the
/// model-source checks (schemas / classifier / mox): the intentional
/// mox-first new-project shape must produce ZERO of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoctorSummary {
    pub hard_failures: usize,
    pub soft_warnings: usize,
    pub model_warnings: usize,
}

/// Extract every `rev = "<sha>"` value from lines that reference the
/// magick93/codegraph.git dependency (rev may sit on the following line).
fn extract_codegraph_revs(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut revs = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("magick93/codegraph.git") {
            continue;
        }
        let mut window = String::from(*line);
        if let Some(next) = lines.get(i + 1) {
            window.push(' ');
            window.push_str(next);
        }
        let mut rest = window.as_str();
        while let Some(pos) = rest.find("rev") {
            let after = &rest[pos + 3..];
            let Some(eq) = after.find('=') else { break };
            let after_eq = after[eq + 1..].trim_start();
            let Some(stripped) = after_eq.strip_prefix('"') else {
                break;
            };
            let Some(end) = stripped.find('"') else { break };
            if !stripped[..end].is_empty() {
                revs.push(stripped[..end].to_string());
            }
            rest = &stripped[end + 1..];
        }
    }
    revs
}

/// Compare the Cargo.toml codegraph rev pins against this binary's embedded
/// rev. Returns the number of warnings raised.
fn check_codegraph_rev() -> usize {
    let cargo_toml = match std::env::current_dir() {
        Ok(cwd) => cwd.join("Cargo.toml"),
        Err(_) => PathBuf::from("Cargo.toml"),
    };
    if !cargo_toml.is_file() {
        println!("WARN no Cargo.toml in current directory — rev check skipped");
        return 1;
    }
    let text = match fs::read_to_string(&cargo_toml) {
        Ok(t) => t,
        Err(e) => {
            println!("WARN cannot read {} — {e}", cargo_toml.display());
            return 1;
        }
    };
    let revs = extract_codegraph_revs(&text);
    if revs.is_empty() {
        if text.contains("magick93/codegraph.git") {
            println!("WARN Cargo.toml pins codegraph crates without a git rev (branch/tag deps)");
            println!("     hint: pin codegraph deps with rev = \"<sha>\"");
            return 1;
        }
        if text.contains("codegraph") && text.contains("path =") {
            println!("PASS Cargo.toml uses local codegraph path deps (development mode)");
            return 0;
        }
        println!("WARN Cargo.toml pins codegraph crates without a git rev (branch/tag deps)");
        println!("     hint: pin codegraph deps with rev = \"<sha>\"");
        return 1;
    }
    let embedded = crate::rev::codegraph_rev();
    if embedded.is_empty() {
        println!("WARN this binary has no embedded rev — cannot compare Cargo.toml pins");
        return 1;
    }
    if revs.iter().any(|r| r != embedded) {
        println!("WARN Cargo.toml pins codegraph rev {revs:?} but this binary is {embedded}");
        println!("     hint: update the rev pins to match the codegraph binary you generate with");
        1
    } else {
        println!("PASS Cargo.toml pins codegraph rev {embedded}");
        0
    }
}

/// Validate `--mox-files` for doctor: every file must compile with the rex
/// compiler and every package must resolve to a domains.toml domain.
/// `import schema` targets (issue #230) are validated too — a missing or
/// invalid target is a hard failure naming the import path and the .mox
/// that declares it. Returns the (hard_failures, soft_warnings) contributed.
///
/// The files compile as ONE workspace (`compile_files` — the `run`
/// pipeline's semantics): every file lowers against the union namespace of
/// all packages, so cross-package type references resolve the same way they
/// do at generation time. Per-file isolation would false-fail models whose
/// packages reference each other (the union resolves bare names uniquely
/// across packages; a single-file compile sees only its own).
fn check_mox_files(
    mox_files: &[PathBuf],
    domain_config: Option<&codegraph_config::config::DomainConfig>,
) -> (usize, usize) {
    let mut hard = 0;
    let mut soft = 0;
    // Read every file and validate its `import schema` targets; imports
    // resolve relative to each file's directory (unchanged, per file).
    let mut sources: Vec<(String, String)> = Vec::new();
    let mut schema_imports = rex_driver::SchemaImports::new();
    let mut import_counts: HashMap<String, usize> = HashMap::new();
    for path in mox_files {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                hard += 1;
                println!("FAIL mox — cannot read {}: {e}", path.display());
                continue;
            }
        };
        let mox_path = path.display().to_string();
        let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let mut import_failures = 0usize;
        let mut import_count = 0usize;
        for decl in crate::ingest::mox_ingest::scan_schema_imports(&text) {
            let abs_path = base_dir.join(&decl.path);
            let json = match fs::read_to_string(&abs_path) {
                Ok(json) => json,
                Err(e) => {
                    hard += 1;
                    import_failures += 1;
                    println!(
                        "FAIL mox — {mox_path}: import schema '{}' cannot be read: {e}",
                        decl.path
                    );
                    println!(
                        "     hint: create the file or fix the path (resolved relative to the .mox file's directory)"
                    );
                    continue;
                }
            };
            if let Err(e) = serde_json::from_str::<serde_json::Value>(&json) {
                hard += 1;
                import_failures += 1;
                println!(
                    "FAIL mox — {mox_path}: import schema '{}' is not valid JSON: {e}",
                    decl.path
                );
                println!("     hint: fix the JSON syntax in {}", abs_path.display());
                continue;
            }
            schema_imports.insert(&mox_path, &decl.path, json);
            import_count += 1;
        }
        if import_failures > 0 {
            continue;
        }
        import_counts.insert(mox_path.clone(), import_count);
        sources.push((mox_path, text));
    }
    if sources.is_empty() {
        return (hard, soft);
    }

    // Union compile: one workspace, diagnostics tagged with their file path.
    let compilation = rex_driver::compile_files(
        &sources,
        &rex_driver::DomainImports {
            schemas: schema_imports,
            sigil: rex_driver::SigilImports::new(),
        },
    );
    for (p, diagnostic) in &compilation.diagnostics {
        println!("WARN mox diagnostic in {p}: {}", diagnostic.message);
        soft += 1;
    }
    // Attribute hard failures per file: a file carrying at least one
    // Error-severity diagnostic does not compile (mirrors the per-file
    // `model: None` gate; the union model is None whenever any file erred).
    let mut error_files: HashSet<String> = HashSet::new();
    for (p, diagnostic) in &compilation.diagnostics {
        if diagnostic.severity == rex_driver::Severity::Error {
            error_files.insert(p.clone());
        }
    }
    let model = compilation.model;
    for (mox_path, text) in &sources {
        let import_count = import_counts.get(mox_path).copied().unwrap_or(0);
        if model.is_none() && error_files.is_empty() {
            // Model blocked without path-attributed errors — fail every file
            // (the conservative fallback; mirrors the per-file gate).
            hard += 1;
            println!("FAIL mox — {mox_path} does not compile");
            println!("     hint: fix the rexlang syntax errors reported above");
            continue;
        }
        if error_files.contains(mox_path) {
            hard += 1;
            println!("FAIL mox — {mox_path} does not compile");
            println!("     hint: fix the rexlang syntax errors reported above");
            continue;
        }
        let Some(config) = domain_config else {
            // domains.toml already reported a hard failure above.
            continue;
        };
        // The file's own package declaration (parse errors above already
        // failed the file; a compiling file has exactly one package).
        let Some(package_name) = declared_package(text) else {
            hard += 1;
            println!("FAIL mox — {mox_path} declares no `package`");
            println!("     hint: add a package clause, e.g. `package com.example.model`");
            continue;
        };
        let (_, matched) = crate::ingest::mox_ingest::resolve_domain(config, &package_name);
        if matched {
            if import_count > 0 {
                println!(
                    "PASS mox — {mox_path} compiles; every package matches domains.toml; \
                     {import_count} schema import(s) resolved"
                );
            } else {
                println!("PASS mox — {mox_path} compiles; every package matches domains.toml");
            }
        } else {
            hard += 1;
            println!("FAIL mox — package with no matching domains.toml entry: {package_name}");
            println!("     hint: add a [domains.<name>] entry or rename the package");
        }
    }
    (hard, soft)
}

/// The `package` declaration of a `.mox` source (the parser requires it at
/// the top of the file; a compiling file always carries one).
fn declared_package(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let trimmed = line.trim_start();
        trimmed.strip_prefix("package ").map(|rest| {
            rest.split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_end_matches(';')
                .to_string()
        })
    })
}

/// Validate `--rosetta-files` for doctor: every file must pass the sigil
/// pipeline (parse → lower → resolve; severity-Error diagnostics are hard
/// failures, mirroring check_mox_files). A file namespace whose last
/// segment matches no domains.toml key is a WARNING (not a hard failure):
/// compute_generation_order silently drops schemas whose domain is not
/// configured, so the project would generate nothing for it. `import <ns>.*`
/// declarations are line-scanned: an imported namespace with no file among
/// --rosetta-files whose last segment also matches no domain key warns.
/// Returns the (hard_failures, soft_warnings) contributed.
fn check_rosetta_files(
    rosetta_files: &[PathBuf],
    domain_config: Option<&codegraph_config::config::DomainConfig>,
) -> (usize, usize) {
    let mut hard = 0;
    let mut soft = 0;

    // Sigil rev observability: the bridge's parse/lower/resolve behavior is
    // pinned to the sigil git rev; surface it so doctor output is
    // reproducible evidence.
    let sigil = crate::rev::sigil_rev();
    if sigil.is_empty() {
        soft += 1;
        println!(
            "WARN sigil — this binary carries no sigil rev pin (Cargo.lock has no sigil-model entry)"
        );
        println!("     hint: rebuild so Cargo.lock pins the sigil git dependency");
    } else {
        println!("INFO sigil — sigil-model rev {sigil}");
    }

    let mut sources = Vec::new();
    let mut file_texts: Vec<(String, String)> = Vec::new();
    for path in rosetta_files {
        match fs::read_to_string(path) {
            Ok(text) => {
                let name = path.display().to_string();
                file_texts.push((name.clone(), text.clone()));
                sources.push(crate::init::rosetta_model::RosettaFileCheck { name, text });
            }
            Err(e) => {
                hard += 1;
                println!("FAIL rosetta — cannot read {}: {e}", path.display());
                continue;
            }
        }
    }

    let verification = crate::init::rosetta_model::verify_rosetta_sources(&sources);
    for error in &verification.hard_errors {
        hard += 1;
        println!("FAIL rosetta — {error}");
        println!("     hint: fix the Rosetta syntax/resolution error reported above");
    }
    for warning in &verification.warnings {
        soft += 1;
        println!("WARN rosetta diagnostic — {warning}");
    }

    let domain_keys: std::collections::HashSet<String> = domain_config
        .map(|config| config.domains.keys().cloned().collect())
        .unwrap_or_default();
    let provided_namespaces: std::collections::HashSet<String> = verification
        .namespaces
        .iter()
        .map(|(_, ns)| ns.clone())
        .collect();

    if !domain_keys.is_empty() {
        for (file, ns, missing) in
            crate::init::rosetta_model::unmatched_namespaces(&verification, &domain_keys)
        {
            soft += 1;
            println!(
                "WARN rosetta — {file}: namespace '{ns}' has no domains.toml entry \
                 (last segment '{missing}' matches no domain key); its schemas are \
                 silently dropped from generation"
            );
            println!("     hint: add a [domains.{missing}] entry or rename the namespace");
        }
        for (file, imported) in &verification.imports {
            if provided_namespaces.contains(imported) {
                continue;
            }
            let last = crate::init::rosetta_model::namespace_last_segment(imported);
            if !domain_keys.contains(last) {
                soft += 1;
                println!(
                    "WARN rosetta — {file}: import '{imported}.*' resolves to no \
                     --rosetta-files entry and no domains.toml domain"
                );
                println!(
                    "     hint: pass the file declaring namespace '{imported}' via \
                     --rosetta-files or add a [domains.{last}] entry"
                );
            }
        }
    } else {
        // domains.toml already reported a hard failure above; still surface
        // the namespaces so the operator sees what would be dropped.
        for (file, ns, _missing) in
            crate::init::rosetta_model::unmatched_namespaces(&verification, &domain_keys)
        {
            soft += 1;
            println!(
                "WARN rosetta — {file}: namespace '{ns}' cannot be checked \
                 (domains.toml unparseable)"
            );
        }
    }

    (hard, soft)
}

/// Validate `--ddd-files` for doctor (issue #449): every design compiles
/// against its imported .mox domains through the SAME read+collect+compile
/// helper the ingest path uses, so doctor and ingest cannot drift.
/// Error-severity diagnostics (or a missing artifact) are hard failures
/// with the per-file rendered output; warnings count as soft. Returns the
/// (hard_failures, soft_warnings) contributed.
fn check_ddd_files(ddd_files: &[PathBuf]) -> (usize, usize) {
    let mut hard = 0;
    let mut soft = 0;
    for path in ddd_files {
        match crate::ingest::ddd_ingest::read_and_compile_ddd(path) {
            Ok(compiled) => {
                let rendered = crate::ingest::ddd_ingest::render_ddd_diagnostics(&compiled);
                if !rendered.is_empty() {
                    println!("{rendered}");
                }
                let has_errors = compiled
                    .compilation
                    .diagnostics
                    .iter()
                    .any(|(_, diagnostic)| diagnostic.is_error());
                if has_errors || compiled.compilation.model.is_none() {
                    hard += 1;
                    println!("FAIL ddd — {} does not compile", path.display());
                    println!("     hint: fix the rexlang design errors reported above");
                    continue;
                }
                soft += compiled.compilation.diagnostics.len();
                if let Some(model) = &compiled.compilation.model {
                    let designs: usize = model.modules.iter().map(|m| m.designs.len()).sum();
                    let repositories: usize = model
                        .modules
                        .iter()
                        .map(|m| m.designs.iter().filter(|d| d.repository.is_some()).count())
                        .sum();
                    let services: usize = model.modules.iter().map(|m| m.services.len()).sum();
                    let searches: usize = model.modules.iter().map(|m| m.searches.len()).sum();
                    println!(
                        "PASS ddd — {} compiles ({} design(s), {} repositor(y/ies), \
                         {} service(s), {} search(es))",
                        path.display(),
                        designs,
                        repositories,
                        services,
                        searches
                    );
                }
            }
            Err(e) => {
                hard += 1;
                println!("FAIL ddd — {e}");
                println!("     hint: check the .ddd file and its imported .mox domains");
            }
        }
    }
    (hard, soft)
}

/// Validate `--evt-files` for doctor (issue #454): every event contract
/// compiles against its imported .mox domains through the SAME
/// read+collect+compile helper the ingest path uses, so doctor and ingest
/// cannot drift. Error-severity diagnostics (or a missing artifact) are
/// hard failures with the per-file rendered output; warnings count as
/// soft. Returns the (hard_failures, soft_warnings) contributed.
fn check_evt_files(evt_files: &[PathBuf]) -> (usize, usize) {
    let mut hard = 0;
    let mut soft = 0;
    for path in evt_files {
        match crate::ingest::evt_ingest::read_and_compile_evt(path) {
            Ok(compiled) => {
                let rendered = crate::ingest::evt_ingest::render_evt_diagnostics(&compiled);
                if !rendered.is_empty() {
                    println!("{rendered}");
                }
                let has_errors = compiled
                    .compilation
                    .diagnostics
                    .iter()
                    .any(|(_, diagnostic)| diagnostic.is_error());
                if has_errors || compiled.compilation.model.is_none() {
                    hard += 1;
                    println!("FAIL evt — {} does not compile", path.display());
                    println!("     hint: fix the rexlang event-contract errors reported above");
                    continue;
                }
                soft += compiled.compilation.diagnostics.len();
                if let Some(model) = &compiled.compilation.model {
                    println!(
                        "PASS evt — {} compiles ({} event(s), {} channel(s), {} subscription(s))",
                        path.display(),
                        model.events.len(),
                        model.channels.len(),
                        model.subscriptions.len()
                    );
                }
            }
            Err(e) => {
                hard += 1;
                println!("FAIL evt — {e}");
                println!("     hint: check the .evt file and its imported .mox domains");
            }
        }
    }
    (hard, soft)
}

/// Validate `--deploy-files` for doctor (rexlang #49): every deployment
/// file resolves its imported .ddd designs and .ifml flows and compiles
/// through the SAME read+resolve+compile helper the ingest path uses, so
/// doctor and ingest cannot drift. Error-severity diagnostics (or a
/// missing artifact) are hard failures with the per-file rendered output;
/// warnings count as soft. ADDITIVE (the `check_evt_files` policy): never
/// feeds model_warnings. Once the files compile, two cross-checks run:
/// profile target vs resolved BuildPlan topology (hard) and domains.toml
/// component coverage (soft). Returns the (hard_failures, soft_warnings)
/// contributed.
fn check_deploy_files(
    deploy_files: &[PathBuf],
    build_plan: Option<&crate::profile::BuildPlan>,
    domain_config: Option<&codegraph_config::config::DomainConfig>,
) -> (usize, usize) {
    let mut hard = 0;
    let mut soft = 0;
    let mut compiled_models: Vec<codegraph_core::types::DeployModelGraph> = Vec::new();
    for path in deploy_files {
        match crate::ingest::deploy_ingest::read_and_resolve_deploy(path) {
            Ok(compiled) => {
                let rendered = crate::ingest::deploy_ingest::render_deploy_diagnostics(&compiled);
                if !rendered.is_empty() {
                    println!("{rendered}");
                }
                let has_errors = compiled.diagnostics.iter().any(|(_, d)| d.is_error());
                if has_errors || compiled.graph.is_none() {
                    hard += 1;
                    println!("FAIL deploy — {} does not compile", path.display());
                    println!("     hint: fix the rexlang deployment errors reported above");
                    continue;
                }
                soft += compiled.diagnostics.len();
                let graph = compiled.graph.expect("checked above");
                println!(
                    "PASS deploy — {} compiles ({} application(s), {} profile(s), \
                     {} deployment(s))",
                    path.display(),
                    graph.applications.len(),
                    graph.profiles.len(),
                    graph.deployments.len()
                );
                compiled_models.push(graph);
            }
            Err(e) => {
                hard += 1;
                println!("FAIL deploy — {e}");
                println!("     hint: check the .deploy file and its imported .ddd/.ifml files");
            }
        }
    }

    // Cross-checks are only meaningful against models that compiled.
    if !compiled_models.is_empty() {
        hard += check_deploy_topology(&compiled_models, build_plan);
        soft += check_deploy_component_coverage(&compiled_models, domain_config);
    }
    (hard, soft)
}

/// Parse-verify the .ifml interaction-model plane (additive, like
/// `check_evt_files`: never feeds model_warnings). Each file goes through
/// the SAME `rex_ifml::parse_ifml_file` the ingest path uses, so doctor
/// and ingest cannot drift. A parse error is a hard failure. `import
/// "….actor"` statements resolve read-only through
/// `resolve_actor_policy` — the ingest path warns-and-continues on an
/// unresolvable policy, so doctor is where a silent skip becomes a hard
/// failure instead. Returns the (hard_failures, soft_warnings)
/// contributed.
fn check_ifml_files(ifml_files: &[PathBuf]) -> (usize, usize) {
    let mut hard = 0;
    let mut soft = 0;
    for path in ifml_files {
        match rex_ifml::parse_ifml_file(path) {
            Ok(model) => {
                soft += model.actors.len();
                println!(
                    "PASS ifml — {} parses ({} view(s), {} action(s), {} actor(s))",
                    path.display(),
                    model.views.len(),
                    model.actions.len(),
                    model.actors.len()
                );
                if model.imports.is_empty() {
                    continue;
                }
                let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
                let mut missing = Vec::new();
                for raw in &model.imports {
                    let resolved = base_dir.join(raw);
                    if !resolved.is_file() {
                        missing.push(raw.clone());
                    }
                }
                if !missing.is_empty() {
                    hard += 1;
                    println!(
                        "FAIL ifml — {} imports unresolvable file(s): {}",
                        path.display(),
                        missing.join(", ")
                    );
                    println!(
                        "     hint: the run path warns and continues past a missing import, \
                         so the view guards would silently lose their policy"
                    );
                    continue;
                }
                // Only .actor/.json imports carry policy; .mox imports feed
                // entity resolution elsewhere and are not resolve_actor_policy's
                // business (it would warn "unsupported extension" on them).
                let policy_imports: Vec<String> = model
                    .imports
                    .iter()
                    .filter(|raw| matches!(raw.rsplit('.').next(), Some("actor") | Some("json")))
                    .cloned()
                    .collect();
                let policy =
                    crate::ifml_actor_import::resolve_actor_policy(&policy_imports, base_dir);
                match policy {
                    Some(p) => println!(
                        "PASS ifml — {} policy import(s) resolve ({} actor(s), {} capability(ies))",
                        policy_imports.len(),
                        p.actors.len(),
                        p.capabilities.len()
                    ),
                    None => {
                        hard += 1;
                        println!(
                            "FAIL ifml — {} imports policy file(s) but none compile",
                            path.display()
                        );
                        println!(
                            "     hint: check the .actor sources for compile errors \
                             (the run path warns and continues past them)"
                        );
                    }
                }
            }
            Err(e) => {
                hard += 1;
                println!("FAIL ifml — {} does not parse: {e}", path.display());
                println!("     hint: fix the rexlang interaction-model errors above");
            }
        }
    }
    (hard, soft)
}

/// Profile/target vs topology cross-check (rexlang #49): a
/// `cloudflareWorkers` profile needs the Workers deployment topology and
/// vice versa — a deploy file and a profiles.toml that disagree describe
/// scaffolds that cannot host the declared components. Skipped when no
/// profiles.toml resolved (nothing to disagree with). Returns the hard
/// failures contributed.
fn check_deploy_topology(
    models: &[codegraph_core::types::DeployModelGraph],
    build_plan: Option<&crate::profile::BuildPlan>,
) -> usize {
    use crate::profile::DeploymentTopology;
    let Some(plan) = build_plan else {
        return 0;
    };
    let topology = plan.deployment_topology();
    let has_workers_profile = models
        .iter()
        .any(|m| m.profiles.iter().any(|p| p.target == "cloudflareWorkers"));
    match (has_workers_profile, topology) {
        (true, other) if other != DeploymentTopology::Workers => {
            println!(
                "FAIL deploy — a cloudflareWorkers profile needs the Workers deployment \
                 topology (profiles.toml says {})",
                other.as_str()
            );
            println!(
                "     hint: set deployment_topology = \"workers\" in profiles.toml \
                 (or retarget the profile)"
            );
            1
        }
        (false, DeploymentTopology::Workers) => {
            println!(
                "FAIL deploy — the Workers deployment topology needs a cloudflareWorkers \
                 profile; no .deploy model declares one"
            );
            println!("     hint: add a profile with target: cloudflareWorkers to the .deploy file");
            1
        }
        _ => 0,
    }
}

/// Component-coverage cross-check (rexlang #49): every domains.toml domain
/// that declares entities should appear as an application component in the
/// deployment models — a domain with no component has nowhere to run in
/// the declared architecture. Advisory (WARN, never FAIL): a deployment
/// may legitimately be narrower than the full model during rollout.
/// Returns the soft warnings contributed.
fn check_deploy_component_coverage(
    models: &[codegraph_core::types::DeployModelGraph],
    domain_config: Option<&codegraph_config::config::DomainConfig>,
) -> usize {
    let Some(config) = domain_config else {
        return 0;
    };
    let declared: HashSet<String> = models
        .iter()
        .flat_map(|m| m.applications.iter())
        .flat_map(|a| a.components.iter().map(|c| c.name.clone()))
        .collect();
    let mut warnings = 0;
    let mut names: Vec<&String> = config
        .domains
        .iter()
        .filter(|(_, entry)| !entry.entities.is_empty())
        .map(|(name, _)| name)
        .collect();
    names.sort();
    for name in names {
        if declared.contains(name) {
            continue;
        }
        warnings += 1;
        println!(
            "WARN deploy — domain '{name}' declares entities but no application component \
             named '{name}'"
        );
        println!(
            "     hint: add a component '{name}' to the .deploy application \
             (or accept the gap: the domain may be intentionally outside this deployment)"
        );
    }
    warnings
}

/// Validate an existing consumer project. Prints pass/fail checks and
/// returns Err when any hard check fails; Ok carries the outcome counts.
pub fn cmd_doctor(args: &DoctorArgs) -> Result<DoctorSummary> {
    let mut hard_failures: usize = 0;
    let mut soft_warnings: usize = 0;
    let mut model_warnings: usize = 0;

    println!("codegraph doctor");

    let domain_config = codegraph_config::config::parse_domain_config(&args.config);
    match &domain_config {
        Ok(config) => println!(
            "PASS domains.toml — {} domain(s) configured",
            config.domains.len()
        ),
        Err(e) => {
            hard_failures += 1;
            println!("FAIL domains.toml — {e}");
            println!("     hint: fix TOML syntax in {}", args.config.display());
        }
    }

    // Scan the schemas dir (when provided) up front: the classifier verdict
    // depends on whether JSON schemas are present.
    let schemas_dir = args.schemas.as_deref();
    let mut schemas_dir_exists = false;
    let mut schemas_has_json = false;
    if let Some(dir) = schemas_dir
        && dir.is_dir()
    {
        schemas_dir_exists = true;
        schemas_has_json = walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"));
    }

    match &args.classifier {
        Some(classifier) => {
            match codegraph_classifier::config::parse_classifier_config(classifier) {
                Ok(config) => println!(
                    "PASS classifier.toml — {} naming rule(s)",
                    config.naming_rules.len()
                ),
                Err(e) => {
                    hard_failures += 1;
                    println!("FAIL classifier.toml — {e}");
                    println!("     hint: fix TOML syntax in {}", classifier.display());
                }
            }
        }
        None if schemas_has_json => {
            hard_failures += 1;
            println!("FAIL classifier.toml — not provided but JSON schemas are present");
            println!("     hint: pass --classifier (JSON schemas need classification rules)");
        }
        None => {
            println!(
                "INFO classifier.toml — not provided (no JSON schemas; \
                 mox-first projects need no classifier)"
            );
        }
    }

    let profiles_path = args
        .profiles_config
        .clone()
        .unwrap_or_else(|| PathBuf::from("profiles.toml"));
    // The resolved plan feeds the deploy cross-checks below (profile
    // target vs topology); absent when profiles.toml is missing (nothing
    // to cross-check against).
    let mut build_plan: Option<crate::profile::BuildPlan> = None;
    if profiles_path.exists() {
        match crate::profile::load_and_resolve_profile(&profiles_path, "default", None).and_then(
            |resolved| {
                let registry = crate::profile::CapabilityRegistry::new();
                crate::profile::BuildPlan::from_profile(&resolved, &registry)
            },
        ) {
            Ok(plan) => {
                println!(
                    "PASS profiles.toml — {} entity, {} domain, {} global generator(s)",
                    plan.entity_generators.len(),
                    plan.domain_generators.len(),
                    plan.global_generators.len()
                );
                build_plan = Some(plan);
            }
            Err(e) => {
                hard_failures += 1;
                println!("FAIL profiles.toml — {e}");
                println!(
                    "     hint: fix profile config in {}",
                    profiles_path.display()
                );
            }
        }
    } else {
        soft_warnings += 1;
        println!("WARN no profiles.toml — running all generators");
        println!("     hint: run `codegraph init` to scaffold one");
    }

    let mox_mode = !args.mox_files.is_empty();
    let rosetta_mode = !args.rosetta_files.is_empty();
    let model_mode = mox_mode || rosetta_mode;
    if schemas_dir_exists {
        if schemas_has_json {
            println!(
                "PASS schemas — {} contains JSON schema(s)",
                schemas_dir.unwrap().display()
            );
        } else if model_mode {
            soft_warnings += 1;
            model_warnings += 1;
            let label = if rosetta_mode {
                "rosetta-first"
            } else {
                "mox-first"
            };
            println!(
                "WARN schemas — no *.json files under {} ({label} project)",
                schemas_dir.unwrap().display()
            );
        } else {
            hard_failures += 1;
            println!(
                "FAIL schemas — no *.json files under {}",
                schemas_dir.unwrap().display()
            );
            println!("     hint: add JSON schemas or run `codegraph add domain <name>`");
        }
    } else if model_mode {
        if rosetta_mode {
            println!("INFO schemas — rosetta-first project; no JSON schemas directory");
        } else {
            println!("INFO schemas — mox-first project; no JSON schemas directory");
        }
    } else {
        hard_failures += 1;
        match schemas_dir {
            Some(dir) => println!("FAIL schemas — {} does not exist", dir.display()),
            None => println!("FAIL schemas — no schemas directory and no --mox-files"),
        }
        println!("     hint: create the directory and add JSON schemas");
    }

    if mox_mode {
        let (hard, soft) = check_mox_files(&args.mox_files, domain_config.as_ref().ok());
        hard_failures += hard;
        soft_warnings += soft;
        model_warnings += soft;
        // Structured incompleteness reporting (issue #279): advisory WARN
        // lines in the shared vocabulary — hard failures stay above.
        let findings = crate::doctor_extras::scan_mox_incompleteness(&args.mox_files);
        for finding in &findings {
            println!("{}", crate::doctor_extras::report_line(finding));
        }
        let incomplete = findings.len();
        soft_warnings += incomplete;
        model_warnings += incomplete;
    } else if rosetta_mode {
        let (hard, soft) = check_rosetta_files(&args.rosetta_files, domain_config.as_ref().ok());
        hard_failures += hard;
        soft_warnings += soft;
        model_warnings += soft;
    } else if schemas_has_json {
        soft_warnings += 1;
        model_warnings += 1;
        println!("WARN no .mox files — JSON schemas are the primary model source");
        println!(
            "     hint: consider migrating: codegraph migrate --schemas {} --output <dir>",
            schemas_dir.unwrap().display()
        );
    }

    // The .ddd design plane is additive to any model source: compile-verify
    // each design when provided (issue #449). Not a model-source check, so
    // it never feeds model_warnings.
    if !args.ddd_files.is_empty() {
        let (hard, soft) = check_ddd_files(&args.ddd_files);
        hard_failures += hard;
        soft_warnings += soft;
    }

    // The .evt events plane is additive to any model source:
    // compile-verify each contract when provided (issue #454). Not a
    // model-source check, so it never feeds model_warnings.
    if !args.evt_files.is_empty() {
        let (hard, soft) = check_evt_files(&args.evt_files);
        hard_failures += hard;
        soft_warnings += soft;
    }

    // The .ifml interaction plane is additive to any model source:
    // parse-verify each file when provided (issue #468 phase 1), and
    // resolve its policy imports read-only. Not a model-source check, so
    // it never feeds model_warnings.
    if !args.ifml_files.is_empty() {
        let (hard, soft) = check_ifml_files(&args.ifml_files);
        hard_failures += hard;
        soft_warnings += soft;
    }

    // The .deploy deployment plane is additive to any model source:
    // compile-verify each file when provided (rexlang #49), plus the
    // topology and component-coverage cross-checks once they compile. Not
    // a model-source check, so it never feeds model_warnings.
    if !args.deploy_files.is_empty() {
        let (hard, soft) = check_deploy_files(
            &args.deploy_files,
            build_plan.as_ref(),
            domain_config.as_ref().ok(),
        );
        hard_failures += hard;
        soft_warnings += soft;
    }

    if let Ok(config) = domain_config.as_ref() {
        let checks =
            crate::doctor_dependencies::check_domain_dependencies(config, args.config.parent());
        if !checks.is_empty() {
            let (hard, soft) = crate::doctor_dependencies::print_dependency_checks(&checks);
            hard_failures += hard;
            soft_warnings += soft;
        }
    }

    let mut manifest_candidates = vec![PathBuf::from("codegraph-ops.toml")];
    if let Some(parent) = args.config.parent().filter(|p| !p.as_os_str().is_empty()) {
        manifest_candidates.push(parent.join("codegraph-ops.toml"));
    }
    if let Some(parent) = args.schemas.as_ref().and_then(|s| s.parent())
        && !parent.as_os_str().is_empty()
    {
        manifest_candidates.push(parent.join("codegraph-ops.toml"));
    }
    match manifest_candidates.iter().find(|p| p.is_file()) {
        Some(path) => match codegraph_ops::config::OpsConfig::load(path) {
            Ok(config) => println!(
                "PASS codegraph-ops.toml — app '{}', output {}",
                config.manifest.app_name,
                config.app_dir.display()
            ),
            Err(e) => {
                hard_failures += 1;
                println!("FAIL codegraph-ops.toml — {e}");
                println!("     hint: fix manifest syntax in {}", path.display());
            }
        },
        None => {
            soft_warnings += 1;
            println!("WARN no codegraph-ops.toml — run generation with the ops generator");
        }
    }

    soft_warnings += check_codegraph_rev();

    match codegraph_ops::env::resolve_psql() {
        Some(psql) => println!("PASS psql — {}", psql.display()),
        None => {
            soft_warnings += 1;
            println!("WARN psql not found");
            println!(
                "     hint: install postgresql-client or set PSQL_PATH (required by the api suite)"
            );
        }
    }
    match codegraph_ops::env::resolve_npx() {
        Some(npx) => println!("PASS npx — {}", npx.display()),
        None => {
            soft_warnings += 1;
            println!("WARN npx not found");
            println!("     hint: install node — npx is used for supabase/playwright");
        }
    }
    match std::process::Command::new("hurl").arg("--version").output() {
        Ok(_) => println!("PASS hurl — on PATH"),
        Err(_) => {
            soft_warnings += 1;
            println!("WARN hurl not found");
            println!("     hint: install hurl for api contract tests");
        }
    }

    match std::env::var("APP_DATABASE_URL") {
        Ok(_) => println!("PASS APP_DATABASE_URL — app_user pool enabled"),
        Err(_) => {
            soft_warnings += 1;
            println!("WARN APP_DATABASE_URL not set — server runs in legacy mode");
            println!(
                "     hint: set APP_DATABASE_URL to postgres://app_user:<pass>@host/db so request context rides the statement payload (#169)"
            );
        }
    }

    println!();
    if hard_failures > 0 {
        Err(Error::Config(format!(
            "doctor: {hard_failures} hard check(s) failed, {soft_warnings} warning(s)"
        )))
    } else {
        println!("doctor: all hard checks passed ({soft_warnings} warning(s))");
        Ok(DoctorSummary {
            hard_failures,
            soft_warnings,
            model_warnings,
        })
    }
}

/// Append a `[domains.<name>]` entry to `config_path` and create the
/// starter model (verified before write). Rosetta-first projects (detected
/// via `model/*.rosetta` or the `--rosetta` flag) get `model/<name>.rosetta`
/// sigil-verified through parse/lower/resolve; everything else gets the
/// .mox starter (compile-verified, init precedent). Neither mode creates a
/// `schemas/` directory. Refuses duplicate domains.
pub fn cmd_add_domain(config_path: &Path, domain_name: &str, rosetta: bool) -> Result<()> {
    let name = normalize_domain_name(domain_name);
    if name.is_empty() {
        return Err(Error::Config("domain name cannot be empty".to_string()));
    }

    let existing = codegraph_config::config::parse_domain_config(config_path)
        .map_err(|e| Error::Config(format!("parse '{}': {e}", config_path.display())))?;
    if existing.domains.contains_key(&name) {
        return Err(Error::Config(format!(
            "domain '{name}' already exists in {}",
            config_path.display()
        )));
    }

    let raw = fs::read_to_string(config_path)
        .map_err(|e| Error::Config(format!("read '{}': {e}", config_path.display())))?;
    let mut value: toml::Value = toml::from_str(&raw)
        .map_err(|e| Error::Config(format!("parse '{}': {e}", config_path.display())))?;

    let root = value.as_table_mut().ok_or_else(|| {
        Error::Config(format!(
            "'{}' is not a TOML document",
            config_path.display()
        ))
    })?;
    if !root.contains_key("domains") {
        root.insert(
            "domains".to_string(),
            toml::Value::Table(toml::Table::new()),
        );
    }
    let domains_table = root
        .get_mut("domains")
        .and_then(|d| d.as_table_mut())
        .ok_or_else(|| {
            Error::Config(format!(
                "'domains' in {} is not a TOML table",
                config_path.display()
            ))
        })?;

    let label: String = heck::ToTitleCase::to_title_case(name.as_str());
    let mut entry = toml::Table::new();
    entry.insert("label".to_string(), toml::Value::String(label.clone()));
    entry.insert("schema_dir".to_string(), toml::Value::String(name.clone()));
    entry.insert(
        "postgres_schema".to_string(),
        toml::Value::String(name.clone()),
    );
    domains_table.insert(name.clone(), toml::Value::Table(entry));

    let new_content = toml::to_string_pretty(&value)
        .map_err(|e| Error::Config(format!("serialize '{}': {e}", config_path.display())))?;

    let re_parsed = codegraph_config::config::parse_domain_config_str(&new_content)
        .map_err(|e| Error::Config(format!("re-parse domains.toml after append: {e}")))?;
    if !re_parsed.domains.contains_key(&name) {
        return Err(Error::Config(format!(
            "internal error: appended domain '{name}' missing after round-trip"
        )));
    }

    fs::write(config_path, &new_content)?;

    // Starter model: shared template, verified before write. Rosetta-first
    // projects (model/*.rosetta present or --rosetta) get a .rosetta starter
    // namespaced `{app_name}.{domain}`; everything else keeps the .mox
    // starter. The hint names the codegraph binary — the consumer wrapper's
    // name is not known here, and `codegraph run --mox-files …` works in any
    // project.
    let model_dir = match config_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join("model"),
        _ => PathBuf::from("model"),
    };
    let rosetta_mode = rosetta
        || model_dir.read_dir().is_ok_and(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("rosetta"))
        });
    let model_path = if rosetta_mode {
        let app_name = re_parsed.defaults.app_name.clone();
        let model_content =
            super::model_starter::starter_model_rosetta(&name, &label, &app_name, "codegraph")
                .map_err(Error::Config)?;
        let rosetta_rel = format!("model/{name}.rosetta");
        let verification = super::rosetta_model::verify_rosetta_sources(&[
            super::rosetta_model::RosettaFileCheck {
                name: rosetta_rel.clone(),
                text: model_content.clone(),
            },
        ]);
        if !verification.hard_errors.is_empty() {
            return Err(Error::Config(format!(
                "starter model '{rosetta_rel}' failed sigil verification — refusing to write: {}",
                verification.hard_errors.join("; ")
            )));
        }
        fs::create_dir_all(&model_dir)?;
        let path = model_dir.join(format!("{name}.rosetta"));
        if !path.exists() {
            fs::write(&path, model_content)?;
        }
        path
    } else {
        let model_content = super::model_starter::starter_model_mox(&name, &label, "codegraph")
            .map_err(Error::Config)?;
        let mox_rel = format!("model/{name}.mox");
        let compilation = rex_driver::compile_files(
            &[(mox_rel.clone(), model_content.clone())],
            &rex_driver::DomainImports::default(),
        );
        if compilation.model.is_none() {
            return Err(Error::Config(format!(
                "starter model '{mox_rel}' does not compile — refusing to write"
            )));
        }
        fs::create_dir_all(&model_dir)?;
        let path = model_dir.join(format!("{name}.mox"));
        if !path.exists() {
            fs::write(&path, model_content)?;
        }
        path
    };

    println!("Added domain '{name}' (label {label}, schema_dir {name}, postgres_schema {name})");
    println!("Updated {}", config_path.display());
    println!("Created {}", model_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn normalizes_project_names_to_kebab() {
        assert_eq!(normalize_project_name("My App"), "my-app");
        assert_eq!(normalize_project_name("demo-app"), "demo-app");
        assert_eq!(normalize_project_name("Already_Snake"), "already-snake");
        assert_eq!(normalize_project_name("  Spaces Around  "), "spaces-around");
    }

    #[test]
    fn normalizes_domain_names_to_snake() {
        assert_eq!(normalize_domain_name("Billing"), "billing");
        assert_eq!(normalize_domain_name("Order Items"), "order_items");
        assert_eq!(normalize_domain_name("pay-roll"), "pay_roll");
    }

    #[test]
    fn resolve_target_dir_uses_name_under_parent() {
        assert_eq!(
            resolve_target_dir(Path::new("."), "demo-app"),
            PathBuf::from("demo-app")
        );
        assert_eq!(
            resolve_target_dir(Path::new("/tmp/foo"), "demo-app"),
            PathBuf::from("/tmp/foo/demo-app")
        );
        assert_eq!(
            resolve_target_dir(Path::new("/tmp/foo/demo-app"), "demo-app"),
            PathBuf::from("/tmp/foo/demo-app")
        );
    }

    #[test]
    fn would_overwrite_only_reports_existing_files() {
        let dir = TempDir::new().unwrap();
        let existing = dir.path().join("keep.toml");
        fs::write(&existing, "x").unwrap();
        let files = vec![
            (PathBuf::from("keep.toml"), String::new()),
            (PathBuf::from("fresh.toml"), String::new()),
        ];
        let conflicts = would_overwrite(dir.path(), &files);
        assert_eq!(conflicts, vec![existing]);
    }

    #[test]
    fn extract_codegraph_revs_finds_pins_on_same_and_next_line() {
        let text = r#"
[dependencies]
codegraph = { git = "https://github.com/magick93/codegraph.git", rev = "aaaa" }
codegraph-config = { git = "https://github.com/magick93/codegraph.git",
                     rev = "bbbb" }
other = "0.1"
"#;
        let revs = extract_codegraph_revs(text);
        assert_eq!(revs, vec!["aaaa".to_string(), "bbbb".to_string()]);
    }

    #[test]
    fn add_domain_appends_and_rejects_duplicates() {
        let dir = TempDir::new().unwrap();
        let config = dir.path().join("domains.toml");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/domains.toml");
        fs::copy(&fixture, &config).unwrap();

        cmd_add_domain(&config, "Billing", false).unwrap();

        let parsed = codegraph_config::config::parse_domain_config(&config).unwrap();
        assert!(parsed.domains.contains_key("billing"));
        assert_eq!(parsed.domains["billing"].label, "Billing");
        assert_eq!(parsed.domains["billing"].schema_dir, "billing");
        assert_eq!(parsed.domains["billing"].postgres_schema, "billing");

        let model = dir.path().join("model/billing.mox");
        assert!(model.is_file(), "add domain must create model/billing.mox");
        let content = fs::read_to_string(&model).unwrap();
        let compilation = rex_driver::compile_files(
            &[("model/billing.mox".to_string(), content)],
            &rex_driver::DomainImports::default(),
        );
        assert!(
            compilation.model.is_some(),
            "starter model must compile: {:?}",
            compilation.diagnostics
        );
        assert_eq!(compilation.model.unwrap().packages[0].name, "billing");

        assert!(!dir.path().join("schemas").exists());

        let err = cmd_add_domain(&config, "billing", false).unwrap_err();
        assert!(format!("{err}").contains("already exists"), "{err}");
    }
}
