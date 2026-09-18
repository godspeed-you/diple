//! Shared helpers for the integration tests.
//!
//! This is a plain `mod common;` include rather than the crate's `testing`
//! module: integration tests link `diple` as an *external* crate, built
//! without `cfg(test)`, so `crate::testing` is invisible from here. A
//! subdirectory `mod.rs` is not compiled as its own test target, so nothing
//! here needs a `#[test]`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::prelude::*;
use diple::document::{self, DocumentKind};

/// Path to a fixture document.
pub fn fixture(name: &str) -> PathBuf {
    fixtures_dir().join(name)
}

/// The fixtures directory.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The names (file stems) of every Markdown fixture, sorted.
///
/// Kept as-is for the Markdown-only callers; [`fixture_names_with_extension`]
/// is the format-aware sibling now that the corpus is not only Markdown.
pub fn fixture_names() -> Vec<String> {
    fixture_names_with_extension("md")
}

/// The names (file stems) of every fixture with the given extension, sorted.
///
/// The extension is matched case-insensitively and without a leading dot
/// (`"md"`, `"json"`, `"yaml"`). Panics when the corpus has no such fixture:
/// a test asking for a format that is not there is a test that silently
/// asserts nothing.
pub fn fixture_names_with_extension(ext: &str) -> Vec<String> {
    let mut names: Vec<String> = fixture_paths_with_extension(ext)
        .into_iter()
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no {ext} fixtures");
    names
}

/// The full paths of every fixture with the given extension, sorted.
pub fn fixture_paths_with_extension(ext: &str) -> Vec<PathBuf> {
    let want = ext.trim_start_matches('.').to_ascii_lowercase();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(fixtures_dir())
        .expect("fixtures dir")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.to_ascii_lowercase() == want)
        })
        .collect();
    paths.sort();
    paths
}

/// Every fixture whose extension names a format diple knows, paired with that
/// format, sorted by path.
///
/// This is the whole corpus seen the way `document::load` sees it: a fixture
/// added to `tests/fixtures/` is picked up by the tests that iterate this
/// without anyone having to list it.
pub fn fixtures_by_format() -> Vec<(PathBuf, DocumentKind)> {
    let mut out: Vec<(PathBuf, DocumentKind)> = std::fs::read_dir(fixtures_dir())
        .expect("fixtures dir")
        .map(|e| e.expect("dir entry").path())
        .filter_map(|p| {
            let kind = document::format::from_extension(p.to_str()?)?;
            Some((p, kind))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!out.is_empty(), "no fixtures");
    out
}

/// The `diple` binary with the developer's environment neutralised.
///
/// Without this, a developer (or a CI image) that exports `CLICOLOR_FORCE=1`
/// fails `color_never_emits_no_escape_sequences`, and one that exports
/// `DIPLE_THEME` changes what every test renders. `env_remove` rather than
/// `env_clear`: clearing everything would also take `PATH` and the loader
/// variables `Command::cargo_bin` itself resolves the binary through.
///
/// Use this only where a test must pass its own `--config`; everywhere else
/// use [`diple`], which also adds `--no-config`.
pub fn command() -> Command {
    let mut cmd = Command::cargo_bin("diple").expect("the diple binary");
    for var in ["DIPLE_THEME", "NO_COLOR", "CLICOLOR_FORCE"] {
        cmd.env_remove(var);
    }
    cmd
}

/// [`command`] plus `--no-config`: the developer's `config.toml` is ignored.
pub fn diple() -> Command {
    let mut cmd = command();
    cmd.arg("--no-config");
    cmd
}

/// The body of an insta snapshot file — everything after the YAML header.
///
/// The header is the block between the first two `---` lines; insta also
/// strips the value's trailing newline when it writes the file, so the body is
/// returned exactly as stored and callers compare against a trimmed value.
pub fn snapshot_body(name: &str) -> Option<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.snap"));
    let text = std::fs::read_to_string(path).ok()?;
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some(rest[end + "\n---\n".len()..].to_string())
}
