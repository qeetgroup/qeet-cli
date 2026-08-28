//! Line-oriented output for logs, pipes and CI.
//!
//! Append-only, one line per event, no animation and no cursor movement -- readable when
//! several repositories are in flight and the lines interleave.

use std::io::Write;

use super::{Renderer, write_summary};
use crate::clone::report::{Report, RepositoryReport};

#[derive(Debug, Default)]
pub struct Plain;

impl Plain {
    pub fn new() -> Self {
        Self
    }
}

impl Renderer for Plain {
    fn begin(&self, product_name: &str, manifest_note: Option<&str>, repositories: &[String]) {
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "qeet clone: {product_name}");
        let _ = writeln!(err, "{} repositories", repositories.len());
        if let Some(note) = manifest_note {
            let _ = writeln!(err, "manifest: {note}");
        }
    }

    fn repository_started(&self, name: &str) {
        let _ = writeln!(std::io::stderr().lock(), "cloning {name}...");
    }

    fn repository_retrying(&self, name: &str, attempt: u32, reason: &str) {
        let _ = writeln!(
            std::io::stderr().lock(),
            "{name}: {reason} retrying (attempt {next})",
            next = attempt + 1
        );
    }

    fn repository_finished(&self, entry: &RepositoryReport) {
        let _ = writeln!(std::io::stderr().lock(), "{}: {}", entry.name, entry.outcome.label());
    }

    fn cancelling(&self) {
        let _ = writeln!(std::io::stderr().lock(), "interrupted: stopping remaining clones");
    }

    fn finish(&self, report: &Report) {
        write_summary(report);
    }
}
