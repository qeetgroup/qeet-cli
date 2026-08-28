//! Terminal presentation.
//!
//! Two renderers behind one trait, chosen by whether stderr is a terminal: spinners and
//! in-place updates for a developer watching, plain append-only lines for a log or CI.
//!
//! Stream discipline, because a CLI's output is an interface:
//!
//! - **stderr** carries progress and diagnostics -- transient, decorative, or a problem.
//! - **stdout** carries the result: the final summary, and nothing else.
//!
//! That is why interactivity is detected on stderr: it is where the animation would go.
//! Nothing here relies on terminal behaviour that exists only on Unix.

pub mod interactive;
pub mod plain;
pub mod style;

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::time::Duration;

use crate::clone::report::{Outcome, Report, RepositoryReport};

/// Events a clone run emits, in the order they happen.
///
/// Implementations are called from several tasks at once, hence `Send + Sync` and shared
/// references throughout.
pub trait Renderer: Send + Sync {
    /// Once, before any repository is touched. `manifest_note` names the manifest in
    /// effect when it is not the one built into the binary.
    fn begin(&self, product_name: &str, manifest_note: Option<&str>, repositories: &[String]);

    /// A repository's clone is starting.
    fn repository_started(&self, name: &str);

    /// A transient failure is about to be retried.
    fn repository_retrying(&self, name: &str, attempt: u32, reason: &str);

    /// A repository reached its final state.
    fn repository_finished(&self, entry: &RepositoryReport);

    /// Ctrl-C was received; work is being wound down.
    fn cancelling(&self);

    /// Once, with the completed report.
    fn finish(&self, report: &Report);
}

/// Choose a renderer for the current process.
pub fn detect() -> Arc<dyn Renderer> {
    if std::io::stderr().is_terminal() {
        Arc::new(interactive::Interactive::new())
    } else {
        Arc::new(plain::Plain::new())
    }
}

/// The final summary, identical in both renderers.
///
/// Counts and the headline go to stdout, because they are the result. The per-repository
/// diagnosis goes to stderr, because it is a problem report -- so `qeet clone id > log`
/// still shows failures on the terminal.
pub(super) fn write_summary(report: &Report) {
    let mut out = std::io::stdout().lock();

    let _ = writeln!(out);
    let _ = writeln!(out, "{}", headline(report));
    let _ = writeln!(out);
    let _ = writeln!(out, "  Cloned:          {}", report.cloned());
    if report.already_present() > 0 {
        let _ = writeln!(out, "  Already present: {}", report.already_present());
    }
    let _ = writeln!(out, "  Failed:          {}", report.failed());
    if report.cancelled_repositories() > 0 {
        let _ = writeln!(out, "  Cancelled:       {}", report.cancelled_repositories());
    }
    let _ = out.flush();
    drop(out);

    let problems: Vec<&RepositoryReport> = report.problems().collect();
    if problems.is_empty() {
        return;
    }

    let mut err = std::io::stderr().lock();
    let _ = writeln!(err);
    for entry in problems {
        let _ = writeln!(err, "{} ({})", entry.name, entry.outcome.label());
        match &entry.outcome {
            Outcome::Failed(failure) => {
                let _ = writeln!(err, "  {}", failure.summary());
                if let Some(code) = failure.exit_code {
                    let _ = writeln!(err, "  git exited with code {code}.");
                }
                if !failure.git_stderr.is_empty() {
                    let _ = writeln!(err);
                    let _ = writeln!(err, "  git:");
                    for line in failure.git_stderr.lines() {
                        let _ = writeln!(err, "    {line}");
                    }
                }
                write_next_steps(&mut err, failure.guidance());
            }
            Outcome::Blocked(blocked) => {
                let _ = writeln!(err, "  {}", indent_continuation(&blocked.to_string()));
                let _ = writeln!(err, "  at {}", entry.display);
                write_next_steps(&mut err, &[blocked.guidance()]);
            }
            Outcome::Cancelled => {
                let _ = writeln!(err, "  Not attempted: the run was cancelled.");
            }
            // Not a problem; `problems()` never yields these.
            Outcome::Cloned | Outcome::AlreadyPresent => {}
        }
        let _ = writeln!(err);
    }
    let _ = err.flush();
}

fn write_next_steps(err: &mut impl Write, steps: &[&str]) {
    if steps.is_empty() {
        return;
    }
    let _ = writeln!(err);
    let _ = writeln!(err, "  Next steps:");
    for step in steps {
        let _ = writeln!(err, "    - {step}");
    }
}

/// `Blocked` messages can be multi-line; keep the continuation lines aligned.
fn indent_continuation(text: &str) -> String {
    text.replace('\n', "\n  ")
}

fn headline(report: &Report) -> String {
    let (total, product) = (report.total(), &report.product_name);
    if report.cancelled {
        return format!(
            "{product}: cancelled. {} of {total} repositories completed.",
            report.succeeded()
        );
    }
    if report.failed() > 0 {
        return format!(
            "{product}: completed with errors. {} of {total} succeeded.",
            report.succeeded()
        );
    }
    format!("{product}: {total} of {total} repositories in {}.", human_duration(report.elapsed))
}

/// Compact and honest: `840ms`, `4.2s`, `1m 09s`.
pub(super) fn human_duration(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis < 1_000 {
        return format!("{millis}ms");
    }
    let seconds = duration.as_secs_f64();
    if seconds < 60.0 {
        return format!("{seconds:.1}s");
    }
    let whole = duration.as_secs();
    format!("{}m {:02}s", whole / 60, whole % 60)
}

/// A renderer that draws nothing, for tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct Silent;

#[cfg(test)]
impl Renderer for Silent {
    fn begin(&self, _product_name: &str, _note: Option<&str>, _repositories: &[String]) {}
    fn repository_started(&self, _name: &str) {}
    fn repository_retrying(&self, _name: &str, _attempt: u32, _reason: &str) {}
    fn repository_finished(&self, _entry: &RepositoryReport) {}
    fn cancelling(&self) {}
    fn finish(&self, _report: &Report) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{Failure, FailureKind};

    fn report(repositories: Vec<RepositoryReport>, cancelled: bool) -> Report {
        Report {
            product_name: "Qeet ID".into(),
            repositories,
            cancelled,
            elapsed: Duration::from_millis(4_200),
        }
    }

    fn entry(name: &str, outcome: Outcome) -> RepositoryReport {
        RepositoryReport {
            name: name.into(),
            display: name.into(),
            outcome,
            duration: Duration::from_millis(10),
            attempts: 1,
        }
    }

    #[test]
    fn a_clean_run_headline_reports_the_total_and_the_time() {
        let report = report(vec![entry("a", Outcome::Cloned)], false);
        assert_eq!(headline(&report), "Qeet ID: 1 of 1 repositories in 4.2s.");
    }

    #[test]
    fn a_failed_run_headline_says_so() {
        let report = report(
            vec![
                entry("a", Outcome::Cloned),
                entry(
                    "b",
                    Outcome::Failed(Failure {
                        kind: FailureKind::Auth,
                        exit_code: Some(128),
                        git_stderr: "Permission denied (publickey)".into(),
                    }),
                ),
            ],
            false,
        );
        assert_eq!(headline(&report), "Qeet ID: completed with errors. 1 of 2 succeeded.");
    }

    #[test]
    fn a_cancelled_run_headline_says_so_even_when_nothing_failed() {
        let report =
            report(vec![entry("a", Outcome::Cloned), entry("b", Outcome::Cancelled)], true);
        assert_eq!(headline(&report), "Qeet ID: cancelled. 1 of 2 repositories completed.");
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(human_duration(Duration::from_millis(0)), "0ms");
        assert_eq!(human_duration(Duration::from_millis(840)), "840ms");
        assert_eq!(human_duration(Duration::from_millis(4_200)), "4.2s");
        assert_eq!(human_duration(Duration::from_secs(59)), "59.0s");
        assert_eq!(human_duration(Duration::from_secs(69)), "1m 09s");
        assert_eq!(human_duration(Duration::from_secs(600)), "10m 00s");
    }

    #[test]
    fn multiline_block_reasons_stay_aligned() {
        let indented = indent_continuation("first\nsecond");
        assert_eq!(indented, "first\n  second");
    }
}
