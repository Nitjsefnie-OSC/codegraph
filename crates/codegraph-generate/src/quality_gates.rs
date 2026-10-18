//! Post-generation structural gates (issue #468 W3/W2/W5 acceptance as
//! enforced invariants, not remembered greps).
//!
//! When the `structural_gates` profile feature is on, the run scans the
//! emitted output tree after generation and hard-fails on vocabulary that
//! the pipeline has migrated away from. Default OFF = zero behavior (the
//! repo's byte-identity discipline). The gates live HERE — at generation
//! time, against the output — so a template regression or a hand-edit to
//! generated code fails the run that produced it, with file:line.

use std::path::Path;

use crate::error::{Error, Result};
use crate::project_config::ProjectConfig;

/// One structural rule: a needle that must not appear in files under a
/// scope, with the phase that owns its absence.
struct Gate {
    /// Human name for the failure line.
    name: &'static str,
    /// Checked against files whose relative path ends with one of these.
    extensions: &'static [&'static str],
    /// Applied to the relative path (lowercased); empty = all files.
    path_contains: &'static [&'static str],
    /// The banned needle.
    needle: &'static str,
    /// Why it is banned (printed with the failure).
    because: &'static str,
}

const GATES: &[Gate] = &[
    Gate {
        name: "no-double-confirm",
        extensions: &[".ts", ".svelte", ".js"],
        path_contains: &[],
        needle: "confirm-confirm",
        because: "the `{dialog}-confirm` rule is the only dialog-button \
                  composition (issue #468 finding 3); `-confirm-confirm` is \
                  the retired double-confirm vocabulary",
    },
    Gate {
        name: "pom-uses-the-testid-module",
        extensions: &[".page.ts"],
        path_contains: &["tests/generated"],
        needle: "${this.module}-",
        because: "with testid_module on, POM locators resolve through \
                  ui/src/lib/testids.js, not `${this.module}-…` compositions \
                  (issue #468 finding 3)",
    },
];

/// Scan the emitted tree under `output_dir`. Returns the number of
/// violations found; any violation is a hard generation error naming the
/// first offenders (file:line, up to 10, then a count).
pub fn run_structural_gates(output_dir: &Path, project: &ProjectConfig) -> Result<()> {
    if !project.codegen.structural_gates {
        return Ok(());
    }
    if project.codegen.testid_module {
        // The module must exist for the POM imports to resolve.
        let module = output_dir.join("ui/src/lib/testids.ts");
        if !module.is_file() {
            return Err(Error::Config(
                "structural_gates: ui/src/lib/testids.ts is missing but \
                 testid_module is on — add the `ui_testids` generator to the \
                 profile's [ui] generators"
                    .to_string(),
            ));
        }
    }

    let mut violations: Vec<String> = Vec::new();
    let mut total = 0usize;
    walk(output_dir, output_dir, &mut violations, &mut total);

    if total == 0 {
        return Ok(());
    }
    let mut msg = format!("structural_gates: {total} violation(s) in the generated tree:");
    for v in violations.iter().take(10) {
        msg.push('\n');
        msg.push_str(v);
    }
    if total > violations.len() {
        msg.push_str(&format!("\n… and {} more", total - violations.len()));
    }
    Err(Error::Config(msg))
}

fn walk(root: &Path, dir: &Path, violations: &mut Vec<String>, total: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Generated node_modules/target trees are not pipeline output.
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if matches!(
                name,
                "node_modules" | "target" | ".graph-cache" | ".svelte-kit"
            ) {
                continue;
            }
            walk(root, &path, violations, total);
            continue;
        }
        let Some(rel) = path.strip_prefix(root).ok().map(|p| p.to_path_buf()) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().to_string();
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (line_no, line) in content.lines().enumerate().map(|(i, l)| (i + 1, l)) {
            for gate in GATES {
                if !gate.extensions.iter().any(|ext| file_name.ends_with(ext)) {
                    continue;
                }
                if !gate.path_contains.is_empty()
                    && !gate.path_contains.iter().any(|frag| rel_str.contains(frag))
                {
                    continue;
                }
                if line.contains(gate.needle) {
                    *total += 1;
                    if violations.len() < 10 {
                        violations.push(format!(
                            "  {}:{} [{}] {because}\n    {line}",
                            rel_str,
                            line_no,
                            gate.name,
                            because = gate.because
                        ));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(structural_gates: bool, testid_module: bool) -> ProjectConfig {
        ProjectConfig {
            codegen: crate::project_config::CodegenConfig {
                structural_gates,
                testid_module,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn feature_off_is_zero_behavior() {
        let dir = tempfile::TempDir::new().unwrap();
        let page = dir.path().join("x.page.ts");
        std::fs::write(&page, "[data-testid=\"${this.module}-table\"]").unwrap();
        assert!(run_structural_gates(dir.path(), &project(false, false)).is_ok());
    }

    #[test]
    fn double_confirm_vocabulary_fails_the_run() {
        let dir = tempfile::TempDir::new().unwrap();
        let page = dir.path().join("ui/src/routes/(app)/hr/task/+page.svelte");
        std::fs::create_dir_all(page.parent().unwrap()).unwrap();
        std::fs::write(
            &page,
            "<div data-testid=\"task-delete-confirm-confirm\"></div>",
        )
        .unwrap();
        let err = run_structural_gates(dir.path(), &project(true, false))
            .expect_err("double-confirm must fail");
        assert!(format!("{err}").contains("no-double-confirm"), "{err}");
        assert!(format!("{err}").contains("+page.svelte:1"), "{err}");
    }

    #[test]
    fn raw_pom_composition_fails_when_the_module_is_on() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("ui/src/lib")).unwrap();
        std::fs::write(dir.path().join("ui/src/lib/testids.ts"), "export {}").unwrap();
        let pom = dir.path().join("ui/tests/generated/hr/person.page.ts");
        std::fs::create_dir_all(pom.parent().unwrap()).unwrap();
        std::fs::write(&pom, "const t = `[data-testid=\"${this.module}-table\"]`;").unwrap();
        let err = run_structural_gates(dir.path(), &project(true, true))
            .expect_err("raw composition must fail under testid_module");
        assert!(
            format!("{err}").contains("pom-uses-the-testid-module"),
            "{err}"
        );

        // Resolving through the module passes.
        std::fs::write(
            &pom,
            "const t = `[data-testid=\"${testids.person.table}\"]`;",
        )
        .unwrap();
        assert!(run_structural_gates(dir.path(), &project(true, true)).is_ok());
    }

    #[test]
    fn module_required_when_testid_module_on() {
        let dir = tempfile::TempDir::new().unwrap();
        let err = run_structural_gates(dir.path(), &project(true, true))
            .expect_err("missing module must fail");
        assert!(format!("{err}").contains("testids.ts is missing"), "{err}");
    }
}
