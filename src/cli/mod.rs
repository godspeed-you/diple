//! Command-line interface — argument parsing plus completion and man-page
//! generation.

pub mod args;
pub mod man_sections;

use std::io;

pub use args::CliArgs;
use clap::CommandFactory;
pub use clap_complete::Shell;

/// The clap command definition (single source for help, completions, man).
pub fn command() -> clap::Command {
    CliArgs::command()
}

/// Write a shell completion script to `out`.
pub fn generate_completions(shell: Shell, out: &mut dyn io::Write) {
    let mut cmd = command();
    clap_complete::generate(shell, &mut cmd, "diple", out);
}

/// The date stamped into the `.TH` line, as `YYYY-MM-DD`.
///
/// Deliberately *not* the current date: a page built from a Git tag must be
/// byte-identical whenever it is rebuilt, and a build date would also be stale
/// the moment the page is installed. `SOURCE_DATE_EPOCH` is the
/// reproducible-builds convention for exactly this.
///
/// When it is absent the field falls back to the crate version rather than to
/// the empty string: `clap_mangen` writes the `.TH` arguments unquoted, so an
/// empty date would silently shift `source` and `manual` one position left and
/// the page would come out titled "General Commands Manual" with
/// `diple 0.1.0` in the date slot.
fn man_date() -> String {
    let Some(epoch) = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
    else {
        return concat!("v", env!("CARGO_PKG_VERSION")).to_string();
    };
    let (y, m, d) = civil_from_days(epoch.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since the UNIX epoch to a proleptic Gregorian `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`; integer-only, total, and panic-free.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The configured [`clap_mangen::Man`] for the page.
fn man() -> clap_mangen::Man {
    clap_mangen::Man::new(command())
        .title("DIPLE")
        .section("1")
        .date(man_date())
        .source(concat!("diple ", env!("CARGO_PKG_VERSION")))
        .manual("diple Manual")
}

/// Write the roff man page to `out`.
///
/// NAME, SYNOPSIS, DESCRIPTION, the OPTIONS block (including the per-heading
/// option groups) and VERSION are rendered from the same [`clap::Command`]
/// that produces `--help`, so they cannot drift from the CLI. The
/// sections that have no counterpart in the clap model are interleaved from
/// [`man_sections`] in conventional man-page order.
pub fn generate_man(out: &mut dyn io::Write) -> io::Result<()> {
    let man = man();
    man.render_title(out)?;
    man.render_name_section(out)?;
    man.render_synopsis_section(out)?;
    man.render_description_section(out)?;
    man.render_options_section(out)?;
    for section in man_sections::sections() {
        out.write_all(section.as_bytes())?;
    }
    man.render_version_section(out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> String {
        let mut buf = Vec::new();
        generate_man(&mut buf).expect("render the man page");
        String::from_utf8(buf).expect("the page is UTF-8")
    }

    /// Every `.SH` heading in the page, in order.
    fn headings(page: &str) -> Vec<String> {
        page.lines()
            .filter_map(|l| l.strip_prefix(".SH "))
            .map(|h| h.trim().trim_matches('"').to_string())
            .collect()
    }

    #[test]
    fn required_sections_are_present() {
        let page = page();
        let found = headings(&page);
        for required in [
            "NAME",
            "SYNOPSIS",
            "DESCRIPTION",
            "OPTIONS",
            "DOCUMENT FORMATS",
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
                "missing .SH {required}; found {found:?}"
            );
        }
    }

    #[test]
    fn sections_are_in_conventional_order() {
        let found = headings(&page());
        let index = |name: &str| {
            found
                .iter()
                .position(|h| h == name)
                .unwrap_or_else(|| panic!("missing {name}"))
        };
        let order = [
            "NAME",
            "SYNOPSIS",
            "DESCRIPTION",
            "OPTIONS",
            "DOCUMENT FORMATS",
            "KEY BINDINGS",
            "CONFIGURATION",
            "EXIT STATUS",
            "ENVIRONMENT",
            "FILES",
            "EXAMPLES",
            "SEE ALSO",
            "VERSION",
        ];
        for pair in order.windows(2) {
            assert!(
                index(pair[0]) < index(pair[1]),
                "{} must precede {}: {found:?}",
                pair[0],
                pair[1]
            );
        }
    }

    /// The clap `help_heading` must not collide with the hand-written
    /// CONFIGURATION section, or a reader looking for the configuration file
    /// lands on a two-flag list instead.
    #[test]
    fn configuration_section_documents_the_file_not_the_flags() {
        let page = page();
        let found = headings(&page);
        assert_eq!(
            found.iter().filter(|h| *h == "CONFIGURATION").count(),
            1,
            "exactly one CONFIGURATION section: {found:?}"
        );
        let body = page
            .split(".SH CONFIGURATION")
            .nth(1)
            .expect("a CONFIGURATION body");
        let body = body.split("\n.SH ").next().unwrap_or(body);
        assert!(body.contains("config.toml"), "names the configuration file");
        assert!(body.contains("XDG_CONFIG_HOME"), "documents the XDG lookup");
        assert!(
            body.contains("built\\-in defaults < configuration file"),
            "documents the precedence chain"
        );
    }

    /// A reader who cannot work out why their YAML opened as Markdown must
    /// find the answer in the page: which formats exist, how one is chosen,
    /// and which flag overrules the choice.
    #[test]
    fn the_formats_section_explains_detection_and_the_override() {
        let page = page();
        let body = page
            .split(".SH \"DOCUMENT FORMATS\"")
            .nth(1)
            .expect("a DOCUMENT FORMATS body");
        let body = body.split("\n.SH ").next().unwrap_or(body);
        for needle in [
            "\\-\\-format",
            "markdown",
            "json",
            "yaml",
            ".yml",
            "\\-\\-structured\\-indent",
            "\\-\\-path",
            "[structured]",
        ] {
            assert!(body.contains(needle), "the formats section omits {needle}");
        }
    }

    /// Every configuration key the program accepts must appear in the
    /// configuration example, or a reader has no way to discover it.
    #[test]
    fn the_configuration_example_shows_every_settable_key() {
        let page = page();
        let body = page
            .split(".SH CONFIGURATION")
            .nth(1)
            .expect("a CONFIGURATION body");
        let body = body.split("\n.SH ").next().unwrap_or(body);
        for setting in crate::config::settings::ALL {
            // Sections are written as headers, the keys under them unqualified.
            let key = setting.name.rsplit('.').next().unwrap_or(setting.name);
            assert!(
                body.contains(key),
                "the configuration example omits {}",
                setting.name
            );
        }
        assert!(body.contains("[structured]"), "the section header");
    }

    /// The man page must match the CLI. The options block is generated, but
    /// this guards the whole page against a long option being added to
    /// `--help` and never reaching the page.
    #[test]
    fn every_long_option_appears_in_the_man_page() {
        let page = page();
        let mut cmd = command();
        let help = cmd.render_long_help().to_string();
        let mut checked = 0;
        for arg in command().get_arguments() {
            let Some(long) = arg.get_long() else { continue };
            if arg.is_hide_set() {
                continue;
            }
            assert!(
                help.contains(&format!("--{long}")),
                "--{long} is missing from --help"
            );
            // clap_mangen escapes every hyphen as `\-`.
            let escaped = format!("\\-\\-{}", long.replace('-', "\\-"));
            assert!(
                page.contains(&escaped),
                "--{long} is missing from the man page (looked for {escaped})"
            );
            checked += 1;
        }
        assert!(
            checked >= 18,
            "expected the whole documented flag set, got {checked}"
        );
    }

    /// A line that starts with `.` or `'` is a roff control line. Every such
    /// line in the page must be a request we meant to write.
    #[test]
    fn no_unescaped_control_lines() {
        const KNOWN: &[&str] = &[
            "TH", "SH", "SS", "TP", "PP", "IP", "HP", "LP", "br", "sp", "nf", "fi", "RS", "RE",
            "B", "I", "BR", "IR", "BI", "IB", "RB", "RI", "EX", "EE", "ie", "el", "ad", "na", "hy",
            "nh", "ne", "in", "ll", "\\\"",
        ];
        for (n, line) in page().lines().enumerate() {
            assert!(
                !line.starts_with('\''),
                "line {}: a `'` control line: {line}",
                n + 1
            );
            let Some(rest) = line.strip_prefix('.') else {
                continue;
            };
            let request = rest.split_whitespace().next().unwrap_or("");
            assert!(
                KNOWN.contains(&request),
                "line {}: unknown roff request `.{request}` \
                 (a literal line starting with `.` must be escaped with `\\&`): {line}",
                n + 1
            );
        }
    }

    #[test]
    fn no_stray_backslashes() {
        // Any backslash must introduce a roff escape we use on purpose.
        const ESCAPES: &[char] = &['-', 'f', 'e', '&', '(', '[', '\\', '"', 'h', 'v', '*', 'n'];
        for (n, line) in page().lines().enumerate() {
            let mut chars = line.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch != '\\' {
                    continue;
                }
                let next = chars.next();
                assert!(
                    next.is_some_and(|c| ESCAPES.contains(&c)),
                    "line {}: stray backslash before {next:?}: {line}",
                    n + 1
                );
            }
        }
    }

    #[test]
    fn title_line_carries_manual_and_source() {
        let page = page();
        let title = page
            .lines()
            .find(|l| l.starts_with(".TH "))
            .expect("a .TH line");
        assert!(title.contains("DIPLE"), "{title}");
        assert!(title.contains('1'), "{title}");
        assert!(title.contains("diple Manual"), "{title}");
        assert!(
            title.contains(concat!("diple ", env!("CARGO_PKG_VERSION"))),
            "{title}"
        );
    }

    #[test]
    fn man_date_is_reproducible_from_source_date_epoch() {
        // Not `man_date()` itself: that reads the process environment, which
        // is shared with every other test thread.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(1_756_000_000 / 86_400), (2025, 8, 24));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn escape_protects_hyphens_and_leading_dots() {
        assert_eq!(man_sections::escape("xdg-open"), "xdg\\-open");
        assert_eq!(man_sections::escape(".config"), "\\&.config");
        assert_eq!(man_sections::escape("a\\b"), "a\\eb");
    }
}
