//! Integration tests for the binary.
//!
//! Every test runs the real `diple` executable with a non-tty stdout, which
//! selects the plain-text output path.

use std::io::Write;
use std::process::{Command, Stdio};

mod common;

use common::{command, diple, fixture};

#[test]
fn reads_a_file() {
    let out = diple()
        .arg(fixture("readme.md"))
        .output()
        .expect("run diple");
    assert!(out.status.success(), "status: {:?}", out.status);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.is_empty(), "the document was rendered");
}

#[test]
fn reads_stdin() {
    let mut child = diple()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn diple");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"# Piped\n\nBody text.\n")
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Piped"), "got: {text}");
    assert!(text.contains("Body text."), "got: {text}");
}

#[test]
fn piping_into_head_does_not_panic() {
    // `diple FILE | head -1`: stdout closes early; diple must exit quietly.
    let mut producer = diple()
        .arg(fixture("readme.md"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn diple");
    let stdout = producer.stdout.take().expect("stdout");
    let head = Command::new("head")
        .arg("-1")
        .stdin(Stdio::from(stdout))
        .output()
        .expect("run head");
    let out = producer.wait_with_output().expect("wait");
    assert!(head.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "stderr: {stderr}");
    assert!(
        out.status.success() || out.status.code().is_none(),
        "status: {:?}",
        out.status
    );
}

#[test]
fn width_affects_the_plain_output() {
    let narrow = diple()
        .args(["--width", "40"])
        .arg(fixture("readme.md"))
        .output()
        .expect("run");
    let wide = diple()
        .args(["--width", "120"])
        .arg(fixture("readme.md"))
        .output()
        .expect("run");
    let narrow_max = String::from_utf8_lossy(&narrow.stdout)
        .lines()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0);
    let wide_max = String::from_utf8_lossy(&wide.stdout)
        .lines()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0);
    assert!(
        narrow_max <= 40,
        "narrow lines are at most 40: {narrow_max}"
    );
    assert!(
        wide_max > narrow_max,
        "wider output uses the extra columns ({wide_max} vs {narrow_max})"
    );
}

#[test]
fn color_never_emits_no_escape_sequences() {
    for args in [vec!["--color", "never"], vec![]] {
        let out = diple()
            .args(&args)
            .arg(fixture("mixed-formatting.md"))
            .output()
            .expect("run");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            !text.contains('\u{1b}'),
            "non-interactive output must never leak ANSI (args {args:?})"
        );
    }
}

#[test]
fn color_always_emits_balanced_escape_sequences() {
    // `diple doc.md --color always | less -R` must be coloured even though
    // stdout is not a terminal.
    let out = diple()
        .args(["--color", "always", "--width", "80"])
        .arg(fixture("mixed-formatting.md"))
        .output()
        .expect("run");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    let opens = text.matches('\u{1b}').count();
    let resets = text.matches("\u{1b}[0m").count();
    assert!(opens > 0, "--color always must emit escape sequences");
    assert_eq!(
        opens - resets,
        resets,
        "every SGR sequence must be closed by a reset"
    );
    assert!(
        text.ends_with("\u{1b}[0m\n") || !text.trim_end().ends_with('\u{1b}'),
        "output must not end inside an escape sequence"
    );
    // Stripping the escapes reproduces the uncoloured rendering byte for byte.
    let plain = diple()
        .args(["--color", "never", "--width", "80"])
        .arg(fixture("mixed-formatting.md"))
        .output()
        .expect("run");
    assert_eq!(strip_ansi(&text), String::from_utf8_lossy(&plain.stdout));
}

/// Remove every `ESC [ … m` sequence.
fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('\u{1b}') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        match after.find('m') {
            Some(end) => rest = &after[end + 1..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

#[test]
fn no_ansi_leaks_for_any_fixture() {
    // Nothing may leak into the shell.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for entry in std::fs::read_dir(dir).expect("fixtures") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let out = diple().arg(&path).output().expect("run");
        assert!(out.status.success(), "{}: {:?}", path.display(), out.status);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            !text.contains('\u{1b}'),
            "{} leaked an escape sequence",
            path.display()
        );
    }
}

#[test]
fn a_missing_file_exits_with_a_clear_message() {
    let out = diple().arg("definitely-not-here.md").output().expect("run");
    assert_eq!(out.status.code(), Some(1), "runtime error");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("definitely-not-here.md"), "{stderr}");
    assert!(stderr.contains("cannot read"), "{stderr}");
}

#[test]
fn a_broken_config_exits_two_naming_the_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[table]\nmode = \"diagonal\"\n").expect("write config");
    let out = command()
        .args(["--check-config", "--config"])
        .arg(&path)
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2), "usage/config error");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("mode"), "the key is named: {stderr}");
    assert!(stderr.contains("diagonal"), "the value is quoted: {stderr}");
}

#[test]
fn check_config_accepts_a_valid_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "theme = \"dark\"\nmouse = false\n").expect("write");
    let out = command()
        .args(["--check-config", "--config"])
        .arg(&path)
        .output()
        .expect("run");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("configuration ok"));
}

#[test]
fn an_unknown_keybinding_action_exits_two() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[keys]\nfly_away = \"x\"\n").expect("write");
    let out = command()
        .arg("--config")
        .arg(&path)
        .arg(fixture("readme.md"))
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("fly_away"));
}

#[test]
fn print_capabilities_runs() {
    let out = diple().arg("--print-capabilities").output().expect("run");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("color"), "{text}");
    assert!(text.contains("images"), "{text}");
}

#[test]
fn generators_produce_output() {
    let man = diple().arg("--generate-man").output().expect("run");
    assert!(man.status.success());
    assert!(man.stdout.len() > 200, "man page is non-empty");
    assert!(String::from_utf8_lossy(&man.stdout).contains("diple"));

    for shell in ["bash", "zsh", "fish"] {
        let out = diple()
            .args(["--generate-completions", shell])
            .output()
            .expect("run");
        assert!(out.status.success(), "{shell}");
        assert!(out.stdout.len() > 100, "{shell} completions are non-empty");
    }
}

#[test]
fn invalid_usage_exits_two() {
    let out = diple()
        .args(["--color", "sometimes"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn help_and_version_exit_zero() {
    for flag in ["--help", "--version"] {
        let out = diple().arg(flag).output().expect("run");
        assert!(out.status.success(), "{flag}");
        assert!(!out.stdout.is_empty(), "{flag}");
    }
}

#[test]
fn invalid_utf8_is_decoded_lossily() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("broken.md");
    let mut bytes = b"# Broken\n\ntext ".to_vec();
    bytes.push(0xff);
    bytes.extend_from_slice(b" more\n");
    std::fs::write(&path, bytes).expect("write");
    let out = diple().arg(&path).output().expect("run");
    assert!(out.status.success(), "a bad byte is not fatal");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Broken"), "{text}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid UTF-8"));
}

#[test]
fn a_document_with_a_mermaid_block_still_renders() {
    // A missing external dependency never blocks the document.
    let out = diple()
        .env("PATH", "")
        .arg(fixture("mermaid.md"))
        .output()
        .expect("run");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.is_empty());
}

#[test]
fn an_unsupported_diagram_falls_back_to_its_source() {
    // The non-interactive path has no `s` key, so the marker alone
    // would make the diagram unreachable. Marker *and* source must both be
    // printed.
    let out = diple()
        .env("PATH", "")
        .args(["--width", "80"])
        .arg(fixture("mermaid.md"))
        .output()
        .expect("run");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("[Mermaid diagram could not be rendered]"),
        "the marker is still shown: {text}"
    );
    for line in ["sequenceDiagram", "Alice->>Bob: Hello Bob", "Bob-->>Alice"] {
        assert!(
            text.contains(line),
            "the source line {line:?} is missing:\n{text}"
        );
    }
}

#[test]
fn unicode_filenames_and_content_work() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("δοκιμή-文档.md");
    std::fs::write(&path, "# Ünïcödé\n\n日本語のテキスト。\n").expect("write");
    let out = diple().arg(&path).output().expect("run");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Ünïcödé"), "{text}");
    assert!(text.contains("日本語のテキスト"), "{text}");
}

// --- man page ---------------------------------------------------------------

fn man_page() -> String {
    let out = diple().arg("--generate-man").output().expect("run");
    assert!(out.status.success());
    String::from_utf8(out.stdout).expect("the man page is UTF-8")
}

fn man_headings(page: &str) -> Vec<String> {
    page.lines()
        .filter_map(|l| l.strip_prefix(".SH "))
        .map(|h| h.trim().trim_matches('"').to_string())
        .collect()
}

#[test]
fn man_page_has_every_required_section() {
    let page = man_page();
    let found = man_headings(&page);
    for required in [
        "NAME",
        "SYNOPSIS",
        "DESCRIPTION",
        "OPTIONS",
        "KEY BINDINGS",
        "CONFIGURATION",
        "EXIT STATUS",
        "ENVIRONMENT",
        "FILES",
        "EXAMPLES",
        "SEE ALSO",
        "VERSION",
    ] {
        assert!(
            found.iter().any(|h| h == required),
            "the generated man page is missing .SH {required}; found {found:?}"
        );
    }
}

#[test]
fn man_page_documents_the_real_program() {
    let page = man_page();
    // The clap `help_heading` must no longer claim the CONFIGURATION name.
    assert!(page.contains(".SH \"CONFIGURATION OPTIONS\""), "{page}");
    for expected in [
        "config.toml",
        "XDG_CONFIG_HOME",
        "DIPLE_CONFIG",
        "DIPLE_THEME",
        "DIPLE_MERMAID",
        "NO_COLOR",
        "CLICOLOR_FORCE",
        "COLORTERM",
        "LC_CTYPE",
        "mermaid",
        "scroll_down",
        "toggle_fold",
        "core.pager",
        "\\-\\-print\\-capabilities",
        "less (1)",
        "diple Manual",
    ] {
        assert!(
            page.contains(expected),
            "the generated man page never mentions {expected:?}"
        );
    }
}

#[test]
fn man_page_covers_every_long_option() {
    let page = man_page();
    let help = diple().arg("--help").output().expect("run").stdout;
    let help = String::from_utf8(help).expect("help is UTF-8");
    let mut checked = 0;
    for word in help.split(|c: char| c.is_whitespace() || c == ',' || c == '=') {
        let Some(long) = word.strip_prefix("--") else {
            continue;
        };
        let long = long.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'));
        if long.is_empty() {
            continue;
        }
        let escaped = format!("\\-\\-{}", long.replace('-', "\\-"));
        assert!(
            page.contains(&escaped),
            "--{long} is in --help but not in the man page (looked for {escaped})"
        );
        checked += 1;
    }
    assert!(
        checked >= 18,
        "expected the whole documented flag set, got {checked}"
    );
}

#[test]
fn man_page_has_no_stray_control_lines() {
    const KNOWN: &[&str] = &[
        "TH", "SH", "SS", "TP", "PP", "IP", "HP", "LP", "br", "sp", "nf", "fi", "RS", "RE", "B",
        "I", "BR", "IR", "BI", "IB", "RB", "RI", "EX", "EE", "ie", "el", "ad", "na", "hy", "nh",
        "ne", "in", "ll", "\\\"",
    ];
    for (n, line) in man_page().lines().enumerate() {
        assert!(!line.starts_with('\''), "line {}: {line}", n + 1);
        let Some(rest) = line.strip_prefix('.') else {
            continue;
        };
        let request = rest.split_whitespace().next().unwrap_or("");
        assert!(
            KNOWN.contains(&request),
            "line {}: unescaped leading dot / unknown request `.{request}`: {line}",
            n + 1
        );
    }
}

#[test]
fn man_page_date_follows_source_date_epoch() {
    let out = diple()
        .arg("--generate-man")
        .env("SOURCE_DATE_EPOCH", "1756000000")
        .output()
        .expect("run");
    let page = String::from_utf8(out.stdout).expect("UTF-8");
    let title = page
        .lines()
        .find(|l| l.starts_with(".TH "))
        .expect("a .TH line");
    assert!(
        title.contains("2025-08-24"),
        "SOURCE_DATE_EPOCH must decide the date, got: {title}"
    );
    assert!(title.contains("diple Manual"), "{title}");
}

#[test]
fn key_hints_flags_are_accepted() {
    for flag in ["--key-hints", "--no-key-hints"] {
        let out = diple()
            .arg(flag)
            .arg(fixture("readme.md"))
            .output()
            .expect("run diple");
        assert!(out.status.success(), "{flag}: status {:?}", out.status);
        assert!(
            !String::from_utf8_lossy(&out.stdout).is_empty(),
            "{flag}: the document was still rendered"
        );
    }
}

#[test]
fn key_hints_is_a_valid_configuration_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "key_hints = true\n[keys]\ntoggle_key_hints = \"f2\"\n",
    )
    .expect("write");
    let out = command()
        .args(["--check-config", "--config"])
        .arg(&path)
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("configuration ok"));
}

// --- the binary's output versus the library's -----------------------------

/// Fixtures whose `--width 80` binary output is *not* compared to a snapshot,
/// with the reason. Anything not listed here must be compared, so a renamed or
/// new fixture fails loudly instead of quietly dropping out of the check.
const NOT_COMPARED: &[(&str, &str)] = &[(
    "mermaid",
    "the snapshot is laid out with `NoDiagrams`, which prints the diagram \
     source; the binary wires up a real Mermaid backend and prints rendered \
     diagrams, so the two are legitimately different renderings",
)];

/// Fixtures whose snapshot lives at a width other than 80, with that width.
///
/// Legitimate only because `render_snapshots::WIDTH_INVARIANT` asserts these
/// fixtures render identically at 40, 80 and 120 — the binary is still run at
/// `--width 80`.
const SNAPSHOT_AT_OTHER_WIDTH: &[(&str, usize)] = &[("narrow-table", 40)];

/// The binary's non-interactive output is byte-for-byte the library's.
///
/// `reads_a_file` only asserted "some bytes came out" and
/// `no_ansi_leaks_for_any_fixture` only "no escape byte came out", so nothing
/// pinned what the binary actually prints. This is the test that would have
/// caught the literal `###` heading-marker leak at the binary level, and it
/// closes the `print_plain` option-mapping gap from the outside: if
/// `LayoutOptions::apply_config` ever stops being reached from `print_plain`,
/// every fixture here diverges at once.
#[test]
fn plain_output_matches_the_render_snapshots_byte_for_byte() {
    let mut compared = 0usize;
    for name in common::fixture_names() {
        if let Some((_, why)) = NOT_COMPARED.iter().find(|(n, _)| *n == name) {
            assert!(!why.is_empty());
            continue;
        }
        let snapshot_width = SNAPSHOT_AT_OTHER_WIDTH
            .iter()
            .find(|(n, _)| *n == name)
            .map_or(80, |(_, w)| *w);
        let snapshot = common::snapshot_body(&format!("render_snapshots__{name}-{snapshot_width}"))
            .unwrap_or_else(|| {
                panic!(
                    "fixture {name} has no {snapshot_width}-width snapshot; either add one, \
                     list it in SNAPSHOT_AT_OTHER_WIDTH, or explain it in NOT_COMPARED"
                )
            });

        let out = diple()
            // The snapshots are laid out with `unicode: true`; the binary
            // derives that from the locale, which a CI image need not set.
            // Pinning the *input* keeps this test about the option mapping.
            .env("DIPLE_UNICODE", "1")
            .args(["--color", "never", "--width", "80"])
            .arg(fixture(&format!("{name}.md")))
            .output()
            .expect("run diple");
        assert!(out.status.success(), "{name}: {:?}", out.status);
        let text = String::from_utf8(out.stdout).expect("the output is UTF-8");
        assert_eq!(
            text, snapshot,
            "{name}: the binary's --width 80 output differs from \
             render_snapshots__{name}-{snapshot_width}"
        );
        compared += 1;
    }
    assert!(
        compared >= 8,
        "expected the whole fixture set minus {} exclusions, compared {compared}",
        NOT_COMPARED.len()
    );
}

/// Every name in the two exclusion tables still names a fixture.
///
/// Without this, renaming `mermaid.md` would silently turn its entry into a
/// no-op and the fixture would drop out of the comparison unnoticed.
#[test]
fn the_snapshot_comparison_tables_name_real_fixtures() {
    let names = common::fixture_names();
    for (name, _) in NOT_COMPARED {
        assert!(
            names.iter().any(|n| n == name),
            "NOT_COMPARED names {name}, which is not a fixture any more"
        );
    }
    for (name, _) in SNAPSHOT_AT_OTHER_WIDTH {
        assert!(
            names.iter().any(|n| n == name),
            "SNAPSHOT_AT_OTHER_WIDTH names {name}, which is not a fixture any more"
        );
    }
}

// ---------------------------------------------------------------------------
// Structured documents at the command line (spec §23.3)
//
// Everything below runs the real binary with a non-tty stdout, which is the
// path a pipeline takes. What is asserted is what a user would see: the
// document, the exit code, and which stream the words came out of.
// ---------------------------------------------------------------------------

/// Run diple over `text` on stdin, with the given extra arguments.
fn piped(args: &[&str], text: &str) -> std::process::Output {
    let mut child = diple()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn diple");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(text.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait")
}

#[test]
fn a_json_file_renders_as_a_structured_document() {
    let out = diple().arg(fixture("nested.json")).output().expect("run");
    assert!(out.status.success(), "status: {:?}", out.status);
    let text = String::from_utf8_lossy(&out.stdout);
    // Semantic rendering, not the source: keys unquoted, values quoted,
    // nesting by indentation.
    assert!(text.contains("  name: \"diple\""), "got: {text}");
    assert!(!text.contains("\u{1b}"), "no escapes in a pipeline");
}

#[test]
fn a_yaml_file_renders_as_a_structured_document() {
    let out = diple()
        .arg(fixture("k8s-deployment.yaml"))
        .output()
        .expect("run");
    assert!(out.status.success(), "status: {:?}", out.status);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("apiVersion: apps/v1"), "got: {text}");
    assert!(!text.contains("\u{1b}"));
}

#[test]
fn json_and_structured_yaml_are_detected_on_stdin() {
    let json = piped(&[], r#"{"metadata": {"name": "nginx"}, "replicas": 3}"#);
    assert!(json.status.success());
    let text = String::from_utf8_lossy(&json.stdout);
    assert!(text.contains("name: \"nginx\""), "read as JSON: {text}");

    // The shape `kubectl -o yaml` produces: nested mappings, no ambiguity.
    let yaml = piped(&[], "metadata:\n  name: nginx\nspec:\n  replicas: 3\n");
    assert!(yaml.status.success());
    let text = String::from_utf8_lossy(&yaml.stdout);
    assert!(text.contains("name: nginx"), "read as YAML: {text}");
    assert!(!text.contains('"'), "YAML keeps its own quoting: {text}");
}

/// AC-06: a permissive YAML parser must not steal ordinary prose.
#[test]
fn ambiguous_stdin_stays_markdown() {
    for source in ["hello\n", "- one\n- two\n", "title: hello\n", "42\n"] {
        let out = piped(&[], source);
        assert!(out.status.success(), "{source:?}");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            !text.contains("\u{2500}") && !text.contains('{'),
            "{source:?} was claimed by a structured parser: {text}"
        );
    }
    // A Markdown list renders as bullets, which YAML would not produce.
    let out = piped(&[], "- one\n- two\n");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("one") && text.contains("two"), "got: {text}");
}

/// AC-07: an explicit format beats the name and the content alike.
#[test]
fn an_explicit_format_overrides_name_and_content() {
    let out = piped(&["--format", "yaml", "-"], "metadata:\n  name: nginx\n");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("name: nginx"));

    let out = piped(&["--format", "json", "-"], r#"{"a": 1}"#);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("a: 1"));

    // A YAML file read as Markdown is prose, not a document tree.
    let out = diple()
        .args(["--format", "markdown"])
        .arg(fixture("k8s-deployment.yaml"))
        .output()
        .expect("run");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.contains("\u{25bc}") && !text.contains("\u{25b6}"),
        "Markdown has no container fold markers here: {text}"
    );
}

/// AC-17 and §17.3: a stated format that does not parse is an error on
/// stderr with a non-zero status — never a silent fallback to Markdown.
#[test]
fn an_invalid_structured_document_reports_and_exits_non_zero() {
    for (file, format) in [("invalid.json", "JSON"), ("invalid.yaml", "YAML")] {
        let out = diple().arg(fixture(file)).output().expect("run");
        assert_eq!(out.status.code(), Some(1), "{file}: {:?}", out.status);
        assert!(out.stdout.is_empty(), "{file} rendered something anyway");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(format), "{file}: names the format: {err}");
        assert!(err.contains(file), "{file}: names the source: {err}");
        assert!(
            err.lines().any(|l| l.contains(&format!("{file}:"))
                && l.rsplit(':').take(2).all(|p| p.parse::<usize>().is_ok())),
            "{file}: gives a line and column: {err}"
        );
        assert!(!err.contains("panicked"), "{file}: {err}");
    }
}

/// An unparsable document offered only as a guess falls back rather than
/// failing: nothing had claimed it (§17.2).
#[test]
fn a_guess_that_does_not_parse_falls_back_to_markdown() {
    let out = piped(&[], "{\"a\": 1,\n");
    assert!(out.status.success(), "status: {:?}", out.status);
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"a\""));
}

#[test]
fn a_structured_document_survives_a_closed_pipe() {
    let mut producer = diple()
        .arg(fixture("k8s-deployment.yaml"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn diple");
    let head = Command::new("head")
        .arg("-1")
        .stdin(Stdio::from(producer.stdout.take().expect("stdout")))
        .output()
        .expect("run head");
    let out = producer.wait_with_output().expect("wait");
    assert!(!head.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&out.stderr).is_empty(),
        "closing the pipe is not an error"
    );
}

#[test]
fn the_structured_presentation_flags_reach_the_output() {
    let indent = |n: &str| {
        let out = diple()
            .args(["--structured-indent", n])
            .arg(fixture("nested.json"))
            .output()
            .expect("run");
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert!(indent("2").contains("\n  name:"), "{}", indent("2"));
    assert!(indent("6").contains("\n      name:"), "{}", indent("6"));

    // An out-of-range value is a usage error, like every other bad flag.
    let out = diple()
        .args(["--structured-indent", "0"])
        .arg(fixture("nested.json"))
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2));
}

/// AC-18 at the command line: no fixture may put an escape on the terminal,
/// whatever format it is.
#[test]
fn no_structured_fixture_leaks_an_escape() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for entry in std::fs::read_dir(dir).expect("fixtures") {
        let path = entry.expect("entry").path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "json" | "yaml" | "yml") {
            continue;
        }
        let out = diple().arg(&path).output().expect("run");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            !text.contains('\u{1b}'),
            "{} leaked an escape sequence",
            path.display()
        );
    }
}
