//! Shared `.rosetta` starter model content + sigil verification.
//!
//! `codegraph init --rosetta` renders one `model/<domain>.rosetta` per
//! domain from the `project/rosetta_model.tera` template; `codegraph add
//! domain` renders the same template when the project is rosetta-first.
//! Every starter (and every doctor `--rosetta-files` input) is verified
//! with the sigil pipeline BEFORE anything is written: parse → lower →
//! resolve (all user files in ONE resolve call, builtins automatic) — the
//! same bridge path `ingest_rosetta_files` uses. Severity-Error diagnostics
//! are hard failures; warnings are surfaced but never fatal.

use std::collections::HashSet;

/// One named `.rosetta` source to verify (display path + text).
#[derive(Debug, Clone)]
pub struct RosettaFileCheck {
    pub name: String,
    pub text: String,
}

/// Outcome of [`verify_rosetta_sources`] plus the line-scan facts doctor
/// needs (namespaces + imports, mirroring the mox `scan_package_name` /
/// `scan_schema_imports` contracts — line-scans survive parse failures).
#[derive(Debug, Default)]
pub struct RosettaVerification {
    /// Parse/lower/resolve diagnostics with severity Error (hard failures).
    pub hard_errors: Vec<String>,
    /// Non-error sigil diagnostics (advisory).
    pub warnings: Vec<String>,
    /// `(file, namespace)` per scanned file — first `namespace ` line wins.
    pub namespaces: Vec<(String, String)>,
    /// `(file, imported namespace)` per scanned `import <ns>[.*]` line.
    pub imports: Vec<(String, String)>,
}

/// Verify `.rosetta` sources through the sigil pipeline: parse each file,
/// hard-fail on severity-Error syntax diagnostics, lower the survivors, and
/// resolve ALL lowered files in one call (hard-failing on resolution
/// Errors). Also line-scans namespaces and imports so callers can reason
/// about files that failed to parse.
pub fn verify_rosetta_sources(files: &[RosettaFileCheck]) -> RosettaVerification {
    let mut out = RosettaVerification::default();
    let mut lowered: Vec<sigil_model::ModelFile> = Vec::new();

    for file in files {
        if let Some(ns) = scan_rosetta_namespace(&file.text) {
            out.namespaces.push((file.name.clone(), ns));
        }
        for imported in scan_rosetta_imports(&file.text) {
            out.imports.push((file.name.clone(), imported));
        }
        let source = sigil_diag::SourceFile::new(file.name.clone(), file.text.clone());
        let (unit, diagnostics) = sigil_syntax::parse(&source);
        for diagnostic in &diagnostics {
            match diagnostic.severity {
                sigil_diag::Severity::Error => out.hard_errors.push(format!(
                    "{}: [{}] {}",
                    file.name, diagnostic.code, diagnostic.message
                )),
                _ => out.warnings.push(format!(
                    "{}: [{}] {}",
                    file.name, diagnostic.code, diagnostic.message
                )),
            }
        }
        let Some(unit) = unit else {
            out.hard_errors
                .push(format!("{}: parsing produced no syntax tree", file.name));
            continue;
        };
        lowered.push(sigil_syntax::lower(&file.name, &unit));
    }

    if lowered.is_empty() {
        return out;
    }
    let resolution = sigil_resolve::resolve(lowered);
    for diagnostic in &resolution.diagnostics {
        match diagnostic.severity {
            sigil_diag::Severity::Error => out.hard_errors.push(format!(
                "{}: [{}] {}",
                diagnostic.file, diagnostic.code, diagnostic.message
            )),
            _ => out.warnings.push(format!(
                "{}: [{}] {}",
                diagnostic.file, diagnostic.code, diagnostic.message
            )),
        }
    }
    out
}

/// Scan the `namespace <dotted.name>` declaration of a `.rosetta` source
/// (line-scan contract, mirroring the mox `scan_package_name`; the grammar
/// allows one namespace declaration per file).
/// Trailing `:` characters on the scanned name token are stripped before
/// checking for an empty name.
pub fn scan_rosetta_namespace(source: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("namespace ")?;
        let name: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
        let name = name.trim_end_matches(':').to_string();
        if name.is_empty() { None } else { Some(name) }
    })
}

/// Scan `import <ns>.*[ as alias]` declarations of a `.rosetta` source
/// (line-scan contract, mirroring the mox `scan_schema_imports`). Returns
/// the imported namespace with the trailing `.*` stripped.
pub fn scan_rosetta_imports(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("import ")?.trim();
            let end = rest.find(" as ").unwrap_or(rest.len());
            let ns = rest[..end].trim();
            if ns.is_empty() {
                None
            } else {
                Some(ns.trim_end_matches(".*").to_string())
            }
        })
        .collect()
}

/// The last segment of a dotted namespace (`a.b.common` → `common`).
pub fn namespace_last_segment(namespace: &str) -> &str {
    namespace.rsplit('.').next().unwrap_or(namespace)
}

/// Domains referenced by the given verifications that match no key in
/// `domain_keys`: `(file, namespace, missing last segment)` triples — the
/// doctor's silent-drop warning (compute_generation_order ignores schemas
/// whose domain is not configured).
pub fn unmatched_namespaces<'a>(
    verification: &'a RosettaVerification,
    domain_keys: &HashSet<String>,
) -> Vec<(&'a String, &'a String, String)> {
    verification
        .namespaces
        .iter()
        .filter_map(|(file, ns)| {
            let last = namespace_last_segment(ns);
            if domain_keys.contains(last) {
                None
            } else {
                Some((file, ns, last.to_string()))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STARTER: &str = "namespace demo_app.billing\nversion \"1.0.0\"\n\ntype BillingType:\n\tname string (1..1)\n\tstatus BillingStatus (1..1)\n\nenum BillingStatus:\n\tActive\n\tInactive\n";

    #[test]
    fn verify_accepts_well_formed_starter() {
        let check = verify_rosetta_sources(&[RosettaFileCheck {
            name: "model/billing.rosetta".to_string(),
            text: STARTER.to_string(),
        }]);
        assert!(check.hard_errors.is_empty(), "{:?}", check.hard_errors);
        assert_eq!(
            check.namespaces,
            vec![(
                "model/billing.rosetta".to_string(),
                "demo_app.billing".to_string()
            )]
        );
    }

    #[test]
    fn verify_rejects_syntax_errors_as_hard() {
        let check = verify_rosetta_sources(&[RosettaFileCheck {
            name: "broken.rosetta".to_string(),
            text: "namespace a.b\n\ntype Broken:\n\tname\n".to_string(),
        }]);
        assert!(
            !check.hard_errors.is_empty(),
            "attribute without type must be a hard error"
        );
    }

    #[test]
    fn verify_rejects_unresolvable_references_as_hard() {
        let check = verify_rosetta_sources(&[RosettaFileCheck {
            name: "broken.rosetta".to_string(),
            text: "namespace a.b\n\ntype Broken:\n\tname MissingType (1..1)\n".to_string(),
        }]);
        assert!(
            !check.hard_errors.is_empty(),
            "reference to a missing type must be a hard error: {:?}",
            check.hard_errors
        );
    }

    #[test]
    fn verify_still_reports_namespaces_for_broken_files() {
        let check = verify_rosetta_sources(&[RosettaFileCheck {
            name: "broken.rosetta".to_string(),
            text: "namespace a.b\n\ntype Broken:\n\tname\n".to_string(),
        }]);
        assert!(!check.hard_errors.is_empty());
        assert_eq!(
            check.namespaces,
            vec![("broken.rosetta".to_string(), "a.b".to_string())]
        );
    }

    #[test]
    fn scan_namespace_strips_description_colon() {
        assert_eq!(
            scan_rosetta_namespace("namespace demo.x: <\"docs\">\nversion \"1.0.0\"\n"),
            Some("demo.x".to_string())
        );
    }

    #[test]
    fn scan_namespace_preserves_plain_name() {
        assert_eq!(
            scan_rosetta_namespace("namespace demo.x\nversion \"1.0.0\"\n"),
            Some("demo.x".to_string())
        );
        assert_eq!(
            scan_rosetta_namespace("namespace demo.x\n: <\"docs\">\n"),
            Some("demo.x".to_string())
        );
    }

    #[test]
    fn scan_namespace_ignores_empty_name() {
        assert_eq!(scan_rosetta_namespace("namespace :\n"), None);
        assert_eq!(scan_rosetta_namespace("namespace  <\"docs\">\n"), None);
    }

    #[test]
    fn scan_namespace_preserves_internal_colons() {
        assert_eq!(
            scan_rosetta_namespace("namespace a::b\n"),
            Some("a::b".to_string())
        );
    }

    #[test]
    fn scan_namespace_preserves_trailing_dot() {
        assert_eq!(
            scan_rosetta_namespace("namespace demo.x.\n"),
            Some("demo.x.".to_string())
        );
    }

    #[test]
    fn scan_namespace_preserves_leading_colon() {
        assert_eq!(
            scan_rosetta_namespace("namespace :demo.x\n"),
            Some(":demo.x".to_string())
        );
    }

    #[test]
    fn scan_namespace_strips_colons_after_trailing_dot() {
        assert_eq!(
            scan_rosetta_namespace("namespace demo.x.:::\n"),
            Some("demo.x.".to_string())
        );
    }

    #[test]
    fn scan_namespace_strips_trailing_colon_run() {
        assert_eq!(
            scan_rosetta_namespace("namespace demo.x::: <\"docs\">\n"),
            Some("demo.x".to_string())
        );
    }

    #[test]
    fn scan_imports_strips_wildcard_and_alias() {
        let source = "namespace res.main\n\nimport res.base.*\nimport com.rosetta.model.metafields.* as meta\n\ntype Trade:\n\tparty Party (1..1)\n";
        assert_eq!(
            scan_rosetta_imports(source),
            vec![
                "res.base".to_string(),
                "com.rosetta.model.metafields".to_string()
            ]
        );
    }

    #[test]
    fn namespace_last_segment_takes_tail() {
        assert_eq!(namespace_last_segment("a.b.common"), "common");
        assert_eq!(namespace_last_segment("common"), "common");
    }

    #[test]
    fn unmatched_namespaces_flags_missing_domain_keys() {
        let check = verify_rosetta_sources(&[RosettaFileCheck {
            name: "model/x.rosetta".to_string(),
            text: STARTER.to_string(),
        }]);
        let keys: HashSet<String> = ["common".to_string()].into_iter().collect();
        let unmatched = unmatched_namespaces(&check, &keys);
        assert_eq!(unmatched.len(), 1);
        assert_eq!(unmatched[0].2, "billing");
    }

    #[test]
    fn unmatched_namespaces_accepts_description_colon() {
        let check = verify_rosetta_sources(&[RosettaFileCheck {
            name: "model/x.rosetta".to_string(),
            text:
                "namespace demo.x: <\"docs\">\nversion \"1.0.0\"\n\ntype X:\n\tname string (1..1)\n"
                    .to_string(),
        }]);
        assert!(check.hard_errors.is_empty(), "{:?}", check.hard_errors);
        assert_eq!(check.namespaces.len(), 1);
        let keys: HashSet<String> = ["x".to_string()].into_iter().collect();
        let unmatched = unmatched_namespaces(&check, &keys);
        assert!(unmatched.is_empty(), "{:?}", unmatched);
    }
}
