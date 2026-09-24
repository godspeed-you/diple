//! `diple` — an interactive terminal reader for structured documents.
//!
//! Flow: parse CLI → load and merge configuration → handle the diagnostic and
//! generator flags → read the input (file or stdin) → decide its format and
//! parse it → detect terminal capabilities → run the interactive pager, or
//! print the plain rendered document when the output is not a terminal.
//!
//! Exit codes: `0` success, `1` runtime error — including a document that
//! does not parse — and `2` usage or configuration error.

use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use diple::app::{self, App, AppEnv, AppOptions, Workspace};
use diple::cli::CliArgs;
use diple::config::{self, Config, KeyMap};
use diple::document::{DocumentError, DocumentModel, SourceDocument};
use diple::layout::{Layout, LayoutOptions};
use diple::terminal::{self, lifecycle, Capabilities};

/// Exit code for a usage or configuration problem.
const EXIT_USAGE: u8 = 2;
/// Exit code for a runtime failure.
const EXIT_FAILURE: u8 = 1;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        // Sanitised: a message can carry a file name, and a file name can
        // carry an escape sequence.
        Err(Failure::Usage(message)) => {
            eprintln!("diple: {}", diple::util::text::sanitize(&message));
            ExitCode::from(EXIT_USAGE)
        }
        Err(Failure::Runtime(message)) => {
            eprintln!("diple: {}", diple::util::text::sanitize(&message));
            ExitCode::from(EXIT_FAILURE)
        }
        // The report is several lines of quoted source, so it is printed as
        // it stands rather than squeezed behind a `diple:` prefix.
        Err(Failure::Document(error)) => {
            eprintln!("{}", error.report());
            ExitCode::from(EXIT_FAILURE)
        }
        Err(Failure::BrokenPipe) => ExitCode::SUCCESS,
    }
}

/// How `run` can fail; each variant maps to one exit code.
enum Failure {
    /// Usage or configuration error (exit 2).
    Usage(String),
    /// Runtime error (exit 1).
    Runtime(String),
    /// The document did not parse as the format it was read as (exit 1).
    ///
    /// Separate from [`Failure::Runtime`] because the message is a positioned
    /// report with a source excerpt, not a sentence.
    Document(Box<DocumentError>),
    /// stdout was closed (`diple x.md | head`) — not an error.
    BrokenPipe,
}

impl From<DocumentError> for Failure {
    fn from(error: DocumentError) -> Failure {
        Failure::Document(Box::new(error))
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Failure {
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            Failure::BrokenPipe
        } else {
            Failure::Runtime(error.to_string())
        }
    }
}

fn run() -> Result<ExitCode, Failure> {
    let started = Instant::now();
    let args = match CliArgs::try_parse() {
        Ok(args) => args,
        Err(error) => {
            // clap prints help/version to stdout and errors to stderr itself.
            let _ = error.print();
            return Ok(if error.use_stderr() {
                ExitCode::from(EXIT_USAGE)
            } else {
                ExitCode::SUCCESS
            });
        }
    };

    // Hidden generators used by packaging; they must work without a terminal.
    if let Some(shell) = args.generate_completions {
        let mut out = std::io::stdout();
        diple::cli::generate_completions(shell, &mut out);
        out.flush()?;
        return Ok(ExitCode::SUCCESS);
    }
    if args.generate_man {
        let mut out = std::io::stdout();
        diple::cli::generate_man(&mut out)?;
        out.flush()?;
        return Ok(ExitCode::SUCCESS);
    }

    let loaded = config::load(args.config.as_deref(), args.no_config)
        .map_err(|e| Failure::Usage(e.to_string()))?;
    let cfg = loaded
        .config
        .merged(&args)
        .map_err(|e| Failure::Usage(e.to_string()))?;
    // Keybindings are part of the configuration, so `--check-config` must
    // validate them too.
    let keymap = KeyMap::from_overrides(cfg.keys.iter().map(|(k, v)| (k.as_str(), v)))
        .map_err(|e| Failure::Usage(e.to_string()))?;
    if args.check_config {
        let mut out = std::io::stdout();
        match &loaded.path {
            Some(path) => writeln!(out, "configuration ok: {}", path.display())?,
            None => writeln!(out, "configuration ok: built-in defaults")?,
        }
        return Ok(ExitCode::SUCCESS);
    }

    let mut overrides = terminal::CapabilityOverrides::from_config(&cfg);
    overrides.width = args.width;
    let caps = terminal::detect(&overrides);

    if args.print_capabilities {
        let mut out = std::io::stdout();
        write!(out, "{}", caps.describe())?;
        out.flush()?;
        return Ok(ExitCode::SUCCESS);
    }

    let (name, text) = read_input(args.file.as_deref())?;
    // The format the whole session reads with: `--format` when it was given,
    // otherwise the configured default, otherwise detection. The workspace
    // keeps it so that `:open` decides exactly as the command line did.
    let format = cfg.format.request();
    // Deliberately before the terminal is taken over: a document that does
    // not parse is reported on a terminal that was never left, so there is no
    // state to restore and no path on which a raw-mode terminal can survive
    // the process.
    let loaded = diple::document::load(format, SourceDocument::new(name.clone(), text))?;
    let doc = loaded.model;
    if args.debug {
        eprintln!(
            "diple: parsed {} nodes as {}{} in {:?}",
            doc.node_count(),
            loaded.format.label(),
            if loaded.detected { " (detected)" } else { "" },
            started.elapsed()
        );
    }

    let interactive = std::io::stdout().is_terminal()
        && !is_dumb_terminal()
        && (lifecycle::stdin_is_tty() || lifecycle::open_input_tty().is_ok());

    if !interactive {
        return print_plain(&doc, &cfg, &caps, args.width).map(|()| ExitCode::SUCCESS);
    }

    let color = app::color_level(cfg.color, &caps);
    let theme = app::resolve_theme(&cfg.theme, color);
    let diagrams = app::diagram_provider(&doc, &cfg, &caps, usize::from(caps.size.0));

    let term_size = caps.size;
    // `AppEnv` takes the capabilities by value; the workspace needs them too,
    // to build every document opened later exactly like this one.
    let caps_for_workspace = caps.clone();
    lifecycle::install_panic_hook();
    let options = lifecycle::TerminalOptions {
        alternate_screen: true,
        mouse: cfg.mouse && caps.mouse,
        hide_cursor: true,
        keyboard_enhancement: false,
    };
    let app = App::new(
        doc,
        cfg.clone(),
        keymap.clone(),
        AppEnv {
            caps,
            theme,
            color,
            diagrams,
        },
        AppOptions {
            filename: name,
            size: term_size,
            width_override: args.width,
            debug: args.debug,
        },
    );
    // The first document is the whole workspace until `:open` adds another.
    let mut workspace = Workspace::new(
        app,
        cfg,
        keymap,
        caps_for_workspace,
        args.width,
        args.debug,
        format,
    );
    if args.debug {
        eprintln!("diple: first frame ready after {:?}", started.elapsed());
    }

    let mut guard = lifecycle::TerminalGuard::enter(options)
        .map_err(|e| Failure::Runtime(format!("cannot enter the terminal UI: {e}")))?;
    let result = app::events::run(&mut workspace);
    // Restore before reporting anything, on every exit path.
    let restored = guard.restore();
    result.map_err(Failure::from)?;
    restored.map_err(|e| Failure::Runtime(e.to_string()))?;
    Ok(ExitCode::SUCCESS)
}

/// `TERM=dumb` disables the interactive UI (graceful degradation).
fn is_dumb_terminal() -> bool {
    matches!(std::env::var("TERM").as_deref(), Ok("dumb") | Ok(""))
}

/// Read the document from a file or stdin.
///
/// A lone `-` is stdin, as it is for every other filter, which is what lets
/// `diple --format yaml -` state the format of piped input.
///
/// Invalid UTF-8 is converted lossily with a warning rather than failing
/// (render as best effort).
fn read_input(file: Option<&Path>) -> Result<(String, String), Failure> {
    match file.filter(|path| path.as_os_str() != "-") {
        Some(path) => {
            let bytes = std::fs::read(path)
                .map_err(|e| Failure::Runtime(format!("cannot read {}: {e}", path.display())))?;
            Ok((
                path.display().to_string(),
                decode(bytes, &path.display().to_string()),
            ))
        }
        None => {
            let mut bytes = Vec::new();
            std::io::stdin()
                .read_to_end(&mut bytes)
                .map_err(|e| Failure::Runtime(format!("cannot read from stdin: {e}")))?;
            Ok(("<stdin>".to_string(), decode(bytes, "<stdin>")))
        }
    }
}

fn decode(bytes: Vec<u8>, name: &str) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            eprintln!(
                "diple: {}: invalid UTF-8, decoding lossily",
                diple::util::text::sanitize(name)
            );
            String::from_utf8_lossy(error.as_bytes()).into_owned()
        }
    }
}

/// Non-interactive output: the plain rendered document on stdout.
///
/// This is what makes `diple file.md | head`, `diple response.json | grep`
/// and CI usage work. A closed stdout is silently accepted.
///
/// The output is the whole document in source order: no fold state is passed,
/// so every Markdown section and every JSON/YAML container is expanded, and
/// nothing here depends on the terminal or on where a reader happened to be
/// looking. Two runs over the same bytes therefore print the same bytes.
fn print_plain(
    doc: &DocumentModel,
    cfg: &Config,
    caps: &Capabilities,
    width: Option<u16>,
) -> Result<(), Failure> {
    let color = app::color_level(cfg.color, caps);
    let theme = app::resolve_theme(&cfg.theme, color);
    let mut width = usize::from(width.unwrap_or(caps.size.0)).max(1);
    // `max_width` narrows this path too, so `diple doc.md | less -R` gets the
    // same measure as the interactive view. Centring does not apply: it is a
    // property of the screen, and this output goes to a pipe or a file.
    if cfg.max_width > 0 {
        width = width.min(usize::from(cfg.max_width));
    }
    let diagrams = app::diagram_provider(doc, cfg, caps, width);

    let mut opts = LayoutOptions::new(width, &theme);
    opts.apply_config(cfg);
    opts.diagrams = &diagrams;
    opts.images = false;
    opts.unicode = caps.unicode_box;
    // Spelled out rather than left to the default, because the interactive
    // path sets it explicitly and a silent disagreement between the two is
    // exactly what `apply_config` exists to prevent. `true` is the right value
    // here: this path has no `[n]`/jump affordance, so a footnote reference
    // whose definition section were dropped would be unreachable text.
    opts.footnotes = true;
    // Without colour the styling is dropped anyway (`to_ansi_text` returns
    // exactly `to_plain_text` at `ColorLevel::None`), so highlighting the
    // document would cost tens of milliseconds per language for output nobody
    // can tell apart. `--color always` still highlights everything.
    opts.lazy_code = color == diple::render::theme::ColorLevel::None;
    let tree = Layout::build(doc, &opts);

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    // `--color always` must emit styling here too, so that `diple doc.md
    // --color always | less -R` works. `never` and `auto` on a non-terminal
    // stdout resolve to `ColorLevel::None`, for which `to_ansi_text` returns
    // exactly `to_plain_text` — no escape can leak .
    let rendered = diple::render::to_ansi_text(&tree, color);
    match out.write_all(rendered.as_bytes()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return Ok(()),
        Err(e) => return Err(Failure::from(e)),
    }
    match out.flush() {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(Failure::from(e)),
    }
}
