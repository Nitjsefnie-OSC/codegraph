//! Environment-normalizing helpers for full-output byte-identity tests.
//!
//! WHY this module exists: generated `Cargo.toml` files embed values that are
//! *inputs* to generation, not generator output — the local checkout root
//! (path-mode deps like `path = "/home/<user>/git/codegraph/crates/..."`) and,
//! in git-rev mode, the generating binary's own rev (`CODEGRAPH_GIT_REV`).
//! PR #222's CI failure was exactly this: the flag-off byte-identity test
//! hashed raw bytes and failed on a different checkout. Any full-output
//! byte-identity test must run [`normalize_for_hash`] before hashing.

use sha2::{Digest, Sha256};

/// SHA-256 digest of `bytes` as a lowercase hex string.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Normalize environment-dependent values out of generated `Cargo.toml`
/// files before hashing: collapses `../`-relative checkout path deps to
/// `<CODEGRAPH_ROOT>/crates/…` (see
/// [`normalize_relative_checkout_deps`]), substitutes the local checkout
/// root (resolved from `env!("CARGO_MANIFEST_DIR")` ancestors) with
/// `<CODEGRAPH_ROOT>` and `rev = "<40-hex>"` git-rev pins with a 40-zero
/// placeholder. Non-`Cargo.toml` files pass through byte-identical.
///
/// WHY this exists: generated `Cargo.toml` files embed two values that are
/// *inputs* to generation, not generator output — the local checkout root
/// (path-mode deps like `path = "/home/<user>/git/codegraph/crates/..."`)
/// and, in git-rev mode, the generating binary's own rev
/// (`CODEGRAPH_GIT_REV`). PR #222's CI failure was exactly this: a
/// full-output byte-identity test hashed raw bytes and failed on a different
/// checkout. Any full-output byte-identity test must call this before
/// hashing; everything else in those files stays hash-guarded.
pub fn normalize_for_hash(path: &str, bytes: &[u8]) -> Vec<u8> {
    if !path.ends_with("Cargo.toml") {
        return bytes.to_vec();
    }
    let text = String::from_utf8_lossy(bytes);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("manifest dir two levels below workspace root")
        .to_string_lossy()
        .into_owned();
    let text = normalize_relative_checkout_deps(&text, &root);
    let text = text.replace(root.trim_start_matches('/'), "<CODEGRAPH_ROOT>");
    let text = text.replace(&root, "<CODEGRAPH_ROOT>");
    let needle = "rev = \"";
    let placeholder = "0".repeat(40);
    let mut out = String::with_capacity(text.len());
    let mut rest: &str = &text;
    while let Some(i) = rest.find(needle) {
        let start = i + needle.len();
        let end = start + 40;
        let is_pin = end < rest.len()
            && rest[end..].starts_with('"')
            && rest[start..end].bytes().all(|b| b.is_ascii_hexdigit());
        if is_pin {
            out.push_str(&rest[..start]);
            out.push_str(&placeholder);
            rest = &rest[end..];
        } else {
            out.push_str(&rest[..=i]);
            rest = &rest[i + needle.len()..];
        }
    }
    out.push_str(rest);
    out.into_bytes()
}

/// Canonicalize `../`-relative path deps that climb out of the generated tree
/// to the checkout (the ops testkit's `codegraph-ops` dep is the one emitter).
/// Runs BEFORE the absolute-root replaces and REQUIRES the `../` run so
/// absolute pins stay with the legacy behaviour below. `pathdiff` eats every
/// component the output tmpdir shares with the checkout root, so the
/// stripped-root replace can miss — checkout `/tmp/opencode/wt` with tmpdir
/// `/tmp/insta-*` yields `../../opencode/wt/crates/...`, which contains
/// neither spelling of the root. Collapse the run plus the root suffix to
/// `<CODEGRAPH_ROOT>/crates/` (longest suffix wins, needle must be followed
/// by `/crates/`) so the normalized form is independent of both checkout
/// location and tmpdir depth.
fn normalize_relative_checkout_deps(text: &str, root: &str) -> String {
    let components: Vec<&str> = root
        .trim_start_matches('/')
        .split('/')
        .filter(|c| !c.is_empty())
        .collect();
    let mut current = text.to_string();
    for k in 0..components.len() {
        let needle = format!("{}/crates/", components[k..].join("/"));
        let mut built = String::with_capacity(current.len());
        let mut rest: &str = &current;
        while let Some(i) = rest.find(&needle) {
            let bytes = rest.as_bytes();
            let mut start = i;
            while start >= 3 && bytes[start - 3..start] == *b"../" {
                start -= 3;
            }
            if start == i {
                built.push_str(&rest[..=i]);
                rest = &rest[i + 1..];
                continue;
            }
            built.push_str(&rest[..start]);
            built.push_str("<CODEGRAPH_ROOT>/crates/");
            rest = &rest[i + needle.len()..];
        }
        built.push_str(rest);
        current = built;
    }
    current
}

#[test]
fn absolute_checkout_path_pin_is_normalized() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let toml =
        format!("[dependencies]\ncodegraph-core = {{ path = \"{root}/crates/codegraph-core\" }}\n");
    let out = normalize_for_hash("Cargo.toml", toml.as_bytes());
    // The stripped-root replace runs before the rooted one, so the leading
    // `/` of an absolute pin survives — legacy behaviour the snapshots bake
    // in; do not "fix" it here.
    let expected = b"[dependencies]\ncodegraph-core = { path = \"/<CODEGRAPH_ROOT>/crates/codegraph-core\" }\n";
    assert_eq!(out, expected.to_vec());
}

#[test]
fn relative_checkout_path_occurrence_is_normalized() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let stripped = root.trim_start_matches('/');
    let toml = format!(
        "[dependencies]\ncodegraph-ops = {{ path = \"../../../{stripped}/crates/codegraph-ops\" }}\n"
    );
    let out = normalize_for_hash("Cargo.toml", toml.as_bytes());
    // The `../` run encodes tmpdir depth, not generator output — the
    // relative-cleanup pass collapses it so 2-up and 3-up forms hash alike.
    let expected =
        b"[dependencies]\ncodegraph-ops = { path = \"<CODEGRAPH_ROOT>/crates/codegraph-ops\" }\n";
    assert_eq!(out, expected.to_vec());
}

#[test]
fn sibling_tmpdir_relative_dep_collapses_to_canonical_form() {
    // Checkout under a directory that also contains the test tmpdir
    // (e.g. /tmp/opencode/wt vs /tmp/insta-*): pathdiff eats the shared
    // parent, so the file never contains the stripped root and only the
    // relative-cleanup pass can normalize the dep.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let components: Vec<&str> = root.trim_start_matches('/').split('/').collect();
    let tail = components[components.len() - 2..].join("/");
    let toml = format!(
        "[dependencies]\ncodegraph-ops = {{ path = \"../../{tail}/crates/codegraph-ops\" }}\n"
    );
    let out = normalize_for_hash("Cargo.toml", toml.as_bytes());
    let expected =
        b"[dependencies]\ncodegraph-ops = { path = \"<CODEGRAPH_ROOT>/crates/codegraph-ops\" }\n";
    assert_eq!(out, expected.to_vec());
}

#[test]
fn relative_dep_up_counts_collapse_to_one_canonical_form() {
    // The `../` run encodes tmpdir depth, not generator output — 2 ups and
    // 3 ups must hash identically or the baseline is checkout-dependent.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let stripped = root.trim_start_matches('/');
    let two_up = normalize_for_hash(
        "Cargo.toml",
        format!(
            "[dependencies]\ncodegraph-ops = {{ path = \"../../{stripped}/crates/codegraph-ops\" }}\n"
        )
        .as_bytes(),
    );
    let three_up = normalize_for_hash(
        "Cargo.toml",
        format!(
            "[dependencies]\ncodegraph-ops = {{ path = \"../../../{stripped}/crates/codegraph-ops\" }}\n"
        )
        .as_bytes(),
    );
    assert_eq!(two_up, three_up);
}

#[test]
fn rev_pin_is_replaced_with_zero_placeholder() {
    let toml = "[dependencies]\ncodegraph-core = { git = \"https://github.com/magick93/codegraph.git\", rev = \"1234abcd1234abcd1234abcd1234abcd1234abcd\" }\n";
    let out = normalize_for_hash("Cargo.toml", toml.as_bytes());
    let expected = "[dependencies]\ncodegraph-core = { git = \"https://github.com/magick93/codegraph.git\", rev = \"0000000000000000000000000000000000000000\" }\n";
    assert_eq!(out, expected.as_bytes().to_vec());
}

#[test]
fn cargo_toml_without_environment_pins_is_untouched() {
    let toml = b"[package]\nname = \"test-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\naxum = \"0.7\"\n";
    let out = normalize_for_hash("Cargo.toml", toml);
    assert_eq!(out, toml.to_vec());
}

#[test]
fn non_cargo_toml_file_passes_through_byte_identical() {
    let checkout_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let bytes = format!("path = \"{checkout_root}/crates/codegraph-core\"\n").into_bytes();
    let out = normalize_for_hash("src/lib.rs", &bytes);
    assert_eq!(out, bytes);
}

#[test]
fn sha256_hex_round_trips_known_digests() {
    // SHA-256 of the empty string and of "abc" (NIST test vectors).
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
