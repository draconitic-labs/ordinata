//! The Ordinata command line.
//!
//! This crate is a thin shell. It parses arguments, calls the library, maps a typed
//! error onto a documented exit code, and prints. It contains no numerics, no path
//! logic, and no file format: everything that could be wrong in an interesting way
//! lives in `ordinata-core`, where it can be tested without a process.
//!
//! # Exit codes
//!
//! | code | meaning |
//! |------|---------|
//! | 0    | success |
//! | 2    | the input or the project is not acceptable |
//! | 3    | the problem cannot be answered honestly |
//! | 4    | a path or the filesystem refused |
//! | 5    | a bundle is missing, malformed, or does not match its own checksums |
//! | 10   | an internal invariant was violated; this is always a bug |
//!
//! # Streams
//!
//! Standard output carries the `--json` object and nothing else. Every human-facing
//! word goes to standard error, including success messages, so that stdout can be
//! piped straight into a JSON parser while a person still watches the run.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod output;

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use ordinata_core::error::{InputError, EXIT_BUNDLE, EXIT_INTERNAL, EXIT_OK};
use ordinata_core::{spec, Bundle, Error, Problem};
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Ordinata turns a bounded weighted linear inverse problem into a portable evidence
/// bundle that anyone can verify offline.
///
/// It never touches the network, never runs another program, and never presents a
/// fitted estimate as a measurement.
#[derive(Debug, Parser)]
#[command(
    name = "ordinata",
    version,
    about,
    long_about = None,
    disable_help_subcommand = true,
    after_help = "Support: support@draconitic.com"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check a project and its data without solving anything.
    Validate {
        /// Path to project.json.
        project: PathBuf,
        /// Print one stable JSON object on standard output.
        #[arg(long)]
        json: bool,
    },
    /// Solve a project and write an evidence bundle.
    Solve {
        /// Path to project.json.
        project: PathBuf,
        /// Directory to create. It must not already exist.
        #[arg(long)]
        out: PathBuf,
        /// The observation time to record, as RFC 3339. Defaults to now.
        #[arg(long = "observed-at", value_name = "RFC3339")]
        observed_at: Option<String>,
        /// Print one stable JSON object on standard output.
        #[arg(long)]
        json: bool,
    },
    /// Check that a bundle is intact and is what it says it is.
    Verify {
        /// A bundle directory, or a ZIP archive of one.
        bundle: PathBuf,
        /// Print one stable JSON object on standard output.
        #[arg(long)]
        json: bool,
    },
    /// Write a bundle from built-in synthetic data, clearly labelled as synthetic.
    Demo {
        /// Directory to create. It must not already exist.
        #[arg(long)]
        out: PathBuf,
        /// The observation time to record, as RFC 3339. Defaults to now.
        #[arg(long = "observed-at", value_name = "RFC3339")]
        observed_at: Option<String>,
    },
}

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Command::Validate { .. } => "validate",
            Command::Solve { .. } => "solve",
            Command::Verify { .. } => "verify",
            Command::Demo { .. } => "demo",
        }
    }

    fn wants_json(&self) -> bool {
        match self {
            Command::Validate { json, .. }
            | Command::Solve { json, .. }
            | Command::Verify { json, .. } => *json,
            Command::Demo { .. } => false,
        }
    }
}

fn main() -> std::process::ExitCode {
    // A panic is an internal invariant violation, not a crash to show raw. The hook
    // keeps the message on stderr where diagnostics belong, and catching it turns the
    // panic into the documented exit code 10 rather than a platform-specific abort.
    std::panic::set_hook(Box::new(|info| {
        eprintln!("ordinata: internal invariant violated: {info}");
        eprintln!(
            "This is a bug in Ordinata, not a problem with your data. Please report it to {}.",
            spec::SUPPORT_CONTACT
        );
    }));

    let cli = Cli::parse();
    let command = cli.command.name();
    let wants_json = cli.command.wants_json();

    let outcome = std::panic::catch_unwind(move || run(cli.command));

    let code = match outcome {
        Ok(Ok(code)) => code,
        Ok(Err(error)) => {
            report_error(command, wants_json, &error);
            error.exit_code()
        }
        Err(_) => {
            if wants_json {
                let error = Error::Internal("a panic escaped the command".into());
                print_json(&output::error_json(command, &error));
            }
            EXIT_INTERNAL
        }
    };

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    std::process::ExitCode::from(code as u8)
}

fn report_error(command: &str, wants_json: bool, error: &Error) {
    if wants_json {
        print_json(&output::error_json(command, error));
    }
    eprintln!("ordinata: {error}");
    eprintln!(
        "  category {}  code {}  exit {}",
        error.category(),
        error.code(),
        error.exit_code()
    );
}

fn print_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        // Serializing our own object cannot fail, but if it somehow did, saying so is
        // better than printing nothing and exiting zero.
        Err(e) => eprintln!("ordinata: cannot render the JSON result: {e}"),
    }
}

fn run(command: Command) -> Result<i32, Error> {
    match command {
        Command::Validate { project, json } => {
            let problem = ordinata_core::load_project(&project)?;
            if json {
                print_json(&output::validate_json(&problem));
            }
            output::human_validate(&problem);
            Ok(EXIT_OK)
        }

        Command::Solve {
            project,
            out,
            observed_at,
            json,
        } => {
            let problem = ordinata_core::load_project(&project)?;
            solve_and_write("solve", &problem, &out, observed_at.as_deref(), json)
        }

        Command::Demo { out, observed_at } => {
            let problem = ordinata_core::demo_problem()?;
            solve_and_write("demo", &problem, &out, observed_at.as_deref(), false)
        }

        Command::Verify { bundle, json } => {
            let report = ordinata_core::verify_bundle(&bundle)?;
            let code = if report.passed() {
                EXIT_OK
            } else {
                EXIT_BUNDLE
            };
            if json {
                print_json(&output::verify_json(&report, code));
            }
            output::human_verify(&report);
            Ok(code)
        }
    }
}

fn solve_and_write(
    command: &str,
    problem: &Problem,
    out: &Path,
    observed_at: Option<&str>,
    json: bool,
) -> Result<i32, Error> {
    let observed_at = parse_observed_at(observed_at)?;
    let solution = ordinata_core::solve(problem)?;
    let bundle = ordinata_core::build_bundle(problem, &solution, observed_at)?;
    let written = ordinata_core::write_bundle(&bundle, out, &problem.input_paths)?;
    let written = ordinata_core::error::display_path(&written);

    let validation = validation_status(&bundle);
    if json {
        print_json(&output::solve_json(
            command,
            &bundle,
            &solution,
            &written,
            &validation,
        ));
    }
    output::human_solve(&bundle, &solution, &written, &validation);
    Ok(EXIT_OK)
}

/// Read the overall declared-check status back out of the bundle, so the CLI reports
/// exactly what the bundle says rather than recomputing it and risking disagreement.
fn validation_status(bundle: &Bundle) -> String {
    bundle
        .member("validation.json")
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        .and_then(|value| {
            value
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn parse_observed_at(value: Option<&str>) -> Result<OffsetDateTime, Error> {
    let Some(text) = value else {
        return Ok(OffsetDateTime::now_utc());
    };
    OffsetDateTime::parse(text, &Rfc3339).map_err(|e| {
        InputError::InvalidTimestamp {
            value: text.to_string(),
            detail: e.to_string(),
        }
        .into()
    })
}
