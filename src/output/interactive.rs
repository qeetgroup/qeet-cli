//! Progress display for a developer watching a terminal.
//!
//! One row per repository, in manifest order, updated in place. Raw git output is
//! deliberately not streamed: with several clones in flight, interleaved git progress is
//! unreadable, and the failure detail in the final summary is more useful than a live wall
//! of text.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use super::{Renderer, human_duration, write_summary};
use crate::clone::report::{Outcome, Report, RepositoryReport};

/// How often a spinner advances. Fast enough to look alive, slow enough to be quiet.
const TICK: Duration = Duration::from_millis(120);

pub struct Interactive {
    multi: MultiProgress,
    /// Repository name -> its row. Written once in `begin`, read afterwards.
    rows: Mutex<HashMap<String, ProgressBar>>,
}

impl Interactive {
    pub fn new() -> Self {
        Self {
            // MultiProgress draws to stderr, keeping stdout clean for the summary.
            multi: MultiProgress::new(),
            rows: Mutex::new(HashMap::new()),
        }
    }

    fn row(&self, name: &str) -> Option<ProgressBar> {
        self.rows.lock().ok()?.get(name).cloned()
    }
}

impl Default for Interactive {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer for Interactive {
    fn begin(&self, product_name: &str, manifest_note: Option<&str>, repositories: &[String]) {
        let _ = self.multi.println(format!("{product_name} — {} repositories", repositories.len()));
        if let Some(note) = manifest_note {
            let _ = self.multi.println(format!("manifest: {note}"));
        }
        let _ = self.multi.println("");

        let width = repositories.iter().map(String::len).max().unwrap_or(0);
        let pending = pending_style();
        let mut rows = self.rows.lock().expect("no task panics while holding this lock");

        for name in repositories {
            let bar = self.multi.add(ProgressBar::new_spinner());
            bar.set_style(pending.clone());
            bar.set_prefix(format!("{name:<width$}"));
            bar.set_message("pending");
            rows.insert(name.clone(), bar);
        }
    }

    fn repository_started(&self, name: &str) {
        if let Some(row) = self.row(name) {
            row.set_style(active_style());
            row.set_message("cloning");
            row.enable_steady_tick(TICK);
        }
    }

    fn repository_retrying(&self, name: &str, attempt: u32, reason: &str) {
        if let Some(row) = self.row(name) {
            row.set_message(format!("{reason} retrying (attempt {})", attempt + 1));
        }
    }

    fn repository_finished(&self, entry: &RepositoryReport) {
        let Some(row) = self.row(&entry.name) else {
            return;
        };
        row.disable_steady_tick();
        row.set_style(finished_style());

        let (symbol, detail) = match &entry.outcome {
            Outcome::Cloned => {
                let mut detail = format!("cloned in {}", human_duration(entry.duration));
                // Only mentioned when it happened: a silent retry hides a flaky remote.
                if entry.attempts > 1 {
                    detail.push_str(&format!(" after {} attempts", entry.attempts));
                }
                ("✓", detail)
            }
            Outcome::AlreadyPresent => ("·", "already present".to_string()),
            Outcome::Failed(failure) => ("✗", failure.summary().to_string()),
            Outcome::Blocked(_) => ("✗", "blocked".to_string()),
            Outcome::Cancelled => ("·", "cancelled".to_string()),
        };

        row.finish_with_message(format!("{symbol} {detail}"));
    }

    fn cancelling(&self) {
        let _ = self.multi.println("\ninterrupted: stopping remaining clones…");
    }

    fn finish(&self, report: &Report) {
        // Release the drawing area before the summary is written, so the two do not fight
        // over the same lines.
        let _ = self.multi.clear();
        for row in self.rows.lock().expect("lock").values() {
            row.disable_steady_tick();
        }
        write_summary(report);
    }
}

/// Styles are built per use rather than cached in a `static`: `ProgressStyle` is cheap, and
/// a fallible template belongs next to its fallback.
fn pending_style() -> ProgressStyle {
    style("  {prefix:.dim}  {msg:.dim}")
}

fn active_style() -> ProgressStyle {
    style("  {prefix}  {spinner:.cyan} {msg}").tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
}

fn finished_style() -> ProgressStyle {
    style("  {prefix}  {msg}")
}

/// `ProgressStyle::with_template` only fails on a malformed template, and every template
/// here is a literal -- but a panic in the output layer would be an absurd way to lose a
/// clone, so fall back to the default style instead.
fn style(template: &str) -> ProgressStyle {
    ProgressStyle::with_template(template).unwrap_or_else(|_| ProgressStyle::default_spinner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_is_valid() {
        // If a template were malformed, `style` would silently fall back; assert instead.
        for template in [
            "  {prefix:.dim}  {msg:.dim}",
            "  {prefix}  {spinner:.cyan} {msg}",
            "  {prefix}  {msg}",
        ] {
            assert!(
                ProgressStyle::with_template(template).is_ok(),
                "malformed template: {template}"
            );
        }
    }

    /// Drives the renderer through a whole run. Nothing is asserted about the drawing --
    /// `MultiProgress` is hidden when stderr is not a terminal, which it is not under
    /// `cargo test` -- but this catches panics, poisoned locks and missing rows.
    #[test]
    fn a_full_lifecycle_does_not_panic() {
        use crate::git::{Failure, FailureKind};

        let renderer = Interactive::new();
        let names = vec!["qeet-id-server".to_string(), "qeet-id-console".to_string()];
        renderer.begin("Qeet ID", Some("--manifest /tmp/products.toml"), &names);

        renderer.repository_started("qeet-id-server");
        renderer.repository_retrying("qeet-id-server", 1, "The connection to the remote failed.");
        renderer.repository_finished(&RepositoryReport {
            name: "qeet-id-server".into(),
            display: "qeet-id-server".into(),
            outcome: Outcome::Cloned,
            duration: Duration::from_millis(1_500),
            attempts: 2,
        });

        renderer.repository_started("qeet-id-console");
        renderer.repository_finished(&RepositoryReport {
            name: "qeet-id-console".into(),
            display: "qeet-id-console".into(),
            outcome: Outcome::Failed(Failure {
                kind: FailureKind::Auth,
                exit_code: Some(128),
                git_stderr: "Permission denied (publickey)".into(),
            }),
            duration: Duration::from_millis(200),
            attempts: 1,
        });

        renderer.cancelling();
    }

    #[test]
    fn an_unknown_repository_is_ignored_rather_than_panicking() {
        let renderer = Interactive::new();
        renderer.begin("Qeet ID", None, &[]);
        renderer.repository_started("never-registered");
        renderer.repository_retrying("never-registered", 1, "reason");
        renderer.repository_finished(&RepositoryReport {
            name: "never-registered".into(),
            display: "never-registered".into(),
            outcome: Outcome::Cancelled,
            duration: Duration::ZERO,
            attempts: 0,
        });
    }
}
