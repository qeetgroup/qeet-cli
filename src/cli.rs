//! Command-line surface.
//!
//! Intentionally small: one subcommand and three options. Every addition to this file is a
//! promise to keep supporting it, so v1 promises as little as it can while solving the
//! problem completely.

use std::io::Write;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use crate::commands;
use crate::error::{EXIT_INCOMPLETE, EXIT_SUCCESS, EXIT_USAGE, Error};
use crate::remote::Protocol;

/// Hard ceiling on `--concurrency`. There is no unlimited mode: 66 concurrent git
/// processes would exhaust file descriptors and invite rate limiting.
const MAX_CONCURRENCY: usize = 64;

#[derive(Debug, Parser)]
#[command(
    name = "qeet",
    version,
    about = "Clone every repository belonging to a Qeet product with one command.",
    long_about = "Qeet Group operates a polyrepo architecture in which a single product may \
consist of multiple repositories.\n\n\
The operational unit for developers is the product, but Git's operational unit is the \
individual repository. Qeet CLI bridges this mismatch by allowing developers to clone every \
repository belonging to a product through a single command.\n\n\
Example:\n\n    qeet clone id",
    disable_help_subcommand = true,
    propagate_version = true
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Clone every repository belonging to a product, concurrently.
    Clone(CloneArgs),
}

#[derive(Debug, Args)]
pub struct CloneArgs {
    /// Product to clone, e.g. `id`. Run with an unknown product to list them all.
    #[arg(value_name = "PRODUCT")]
    pub product: String,

    /// Maximum number of repositories to clone at once.
    ///
    /// Defaults to the machine's available parallelism, capped at 8.
    #[arg(long, value_name = "N", value_parser = parse_concurrency)]
    pub concurrency: Option<NonZeroUsize>,

    /// Git transport to use, overriding the manifest default.
    ///
    /// Affects only URLs derived from the manifest's `[remote]`; a repository with an
    /// explicit `url` is cloned exactly as written.
    #[arg(long, value_name = "PROTOCOL")]
    pub protocol: Option<Protocol>,

    /// Manifest to use instead of the registry built into this binary.
    #[arg(long, value_name = "PATH")]
    pub manifest: Option<PathBuf>,
}

/// Reject zero and absurd values with a message that says what to do.
fn parse_concurrency(raw: &str) -> Result<NonZeroUsize, String> {
    let value: usize = raw.parse().map_err(|_| format!("`{raw}` is not a whole number"))?;

    let value =
        NonZeroUsize::new(value).ok_or_else(|| "concurrency must be at least 1".to_string())?;

    if value.get() > MAX_CONCURRENCY {
        return Err(format!("concurrency must be at most {MAX_CONCURRENCY}"));
    }
    Ok(value)
}

/// Parse arguments, run the command, and turn the result into an exit code.
///
/// This is the only place a domain error becomes terminal output, which keeps `main` free
/// of policy and the command layer free of printing.
pub async fn run() -> ExitCode {
    // Parsed rather than `Cli::parse()`, which would exit the process itself. Owning the
    // outcome keeps every exit code in the documented set instead of inheriting clap's.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            // `--help` and `--version` are requests that were satisfied, not misuse.
            return ExitCode::from(if error.use_stderr() { EXIT_USAGE } else { EXIT_SUCCESS });
        }
    };

    match cli.command {
        Command::Clone(args) => match commands::clone::run(&args).await {
            // The report has already been rendered; the exit code is its verdict.
            Ok(report) => {
                ExitCode::from(if report.is_complete() { EXIT_SUCCESS } else { EXIT_INCOMPLETE })
            }
            Err(error) => {
                report_error(&error);
                error.exit_code()
            }
        },
    }
}

/// Startup errors go to stderr as a plain message. No backtrace, no `Error:` chain a normal
/// user cannot act on.
fn report_error(error: &Error) {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{error}");

    // `UnknownProduct` and manifest validation already render their own guidance.
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        let _ = writeln!(err, "  caused by: {cause}");
        source = cause.source();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_definition_is_valid() {
        // Catches conflicting flags, bad value names and duplicate shorts at test time
        // rather than at the user's first invocation.
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_the_primary_invocation() {
        let cli = Cli::try_parse_from(["qeet", "clone", "id"]).expect("should parse");
        let Command::Clone(args) = cli.command;
        assert_eq!(args.product, "id");
        assert_eq!(args.concurrency, None);
        assert_eq!(args.protocol, None);
        assert_eq!(args.manifest, None);
    }

    #[test]
    fn parses_every_option() {
        let cli = Cli::try_parse_from([
            "qeet",
            "clone",
            "pay",
            "--concurrency",
            "6",
            "--protocol",
            "https",
            "--manifest",
            "/tmp/products.toml",
        ])
        .expect("should parse");

        let Command::Clone(args) = cli.command;
        assert_eq!(args.product, "pay");
        assert_eq!(args.concurrency, NonZeroUsize::new(6));
        assert_eq!(args.protocol, Some(Protocol::Https));
        assert_eq!(args.manifest, Some(PathBuf::from("/tmp/products.toml")));
    }

    #[test]
    fn a_product_is_required() {
        assert!(Cli::try_parse_from(["qeet", "clone"]).is_err());
    }

    #[test]
    fn rejects_a_concurrency_of_zero() {
        let err = Cli::try_parse_from(["qeet", "clone", "id", "--concurrency", "0"])
            .expect_err("zero must be refused");
        assert!(err.to_string().contains("at least 1"), "{err}");
    }

    #[test]
    fn rejects_a_negative_or_nonsense_concurrency() {
        for value in ["-1", "many", "1.5", ""] {
            assert!(
                Cli::try_parse_from(["qeet", "clone", "id", "--concurrency", value]).is_err(),
                "`{value}` must be refused"
            );
        }
    }

    #[test]
    fn rejects_an_unbounded_concurrency() {
        let err = Cli::try_parse_from(["qeet", "clone", "id", "--concurrency", "100000"])
            .expect_err("must be capped");
        assert!(err.to_string().contains("at most 64"), "{err}");
    }

    #[test]
    fn rejects_an_unknown_protocol() {
        let err = Cli::try_parse_from(["qeet", "clone", "id", "--protocol", "ftp"])
            .expect_err("ftp is not a transport qeet offers");
        assert!(err.to_string().contains("ssh"), "should list the choices: {err}");
    }

    #[test]
    fn there_are_no_other_subcommands_in_v1() {
        // Guards the scope boundary: status, pull and sync are deliberately absent.
        for absent in ["status", "pull", "sync", "graph", "dev"] {
            assert!(
                Cli::try_parse_from(["qeet", absent]).is_err(),
                "`{absent}` must not exist in v1"
            );
        }
    }
}
