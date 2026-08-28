//! What happened, per repository and overall.
//!
//! One failed repository never hides the others: every repository gets an entry, and the
//! summary is built from all of them.

use std::time::Duration;

use crate::git::Failure;
use crate::workspace::Blocked;

/// The result for one repository.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// Cloned by this run.
    Cloned,
    /// Already on disk with the expected `origin`; nothing was done.
    AlreadyPresent,
    /// git ran and failed.
    Failed(Failure),
    /// Never attempted: workspace preflight refused the destination.
    Blocked(Blocked),
    /// Never attempted, or interrupted, because the run was cancelled.
    Cancelled,
}

impl Outcome {
    /// Did this repository end up in the state the developer asked for?
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Cloned | Self::AlreadyPresent)
    }

    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Failed(_) | Self::Blocked(_))
    }

    /// A short label for the summary line.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Cloned => "cloned",
            Self::AlreadyPresent => "already present",
            Self::Failed(_) => "failed",
            Self::Blocked(_) => "blocked",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One repository's entry in the report.
#[derive(Debug, Clone)]
pub struct RepositoryReport {
    pub name: String,
    /// Destination relative to the workspace root.
    pub display: String,
    pub outcome: Outcome,
    pub duration: Duration,
    /// How many times git was invoked. 0 when it never was.
    pub attempts: u32,
}

/// The whole run.
#[derive(Debug, Clone)]
pub struct Report {
    pub product_name: String,
    /// In manifest order, always one entry per repository in the product.
    pub repositories: Vec<RepositoryReport>,
    /// The run was interrupted by Ctrl-C.
    pub cancelled: bool,
    pub elapsed: Duration,
}

impl Report {
    pub fn total(&self) -> usize {
        self.repositories.len()
    }

    pub fn cloned(&self) -> usize {
        self.count(|outcome| matches!(outcome, Outcome::Cloned))
    }

    pub fn already_present(&self) -> usize {
        self.count(|outcome| matches!(outcome, Outcome::AlreadyPresent))
    }

    /// Cloned plus already present: repositories that are now where they should be.
    pub fn succeeded(&self) -> usize {
        self.count(Outcome::is_success)
    }

    /// Failed plus blocked.
    pub fn failed(&self) -> usize {
        self.count(Outcome::is_failure)
    }

    pub fn cancelled_repositories(&self) -> usize {
        self.count(|outcome| matches!(outcome, Outcome::Cancelled))
    }

    /// Did every repository reach the desired state? Drives the process exit code.
    pub fn is_complete(&self) -> bool {
        !self.cancelled && self.failed() == 0 && self.cancelled_repositories() == 0
    }

    /// Repositories that did not succeed, in manifest order.
    pub fn problems(&self) -> impl Iterator<Item = &RepositoryReport> {
        self.repositories.iter().filter(|entry| !entry.outcome.is_success())
    }

    fn count(&self, predicate: impl Fn(&Outcome) -> bool) -> usize {
        self.repositories.iter().filter(|entry| predicate(&entry.outcome)).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::FailureKind;

    fn entry(name: &str, outcome: Outcome) -> RepositoryReport {
        RepositoryReport {
            name: name.into(),
            display: name.into(),
            outcome,
            duration: Duration::from_millis(1),
            attempts: 1,
        }
    }

    fn failure() -> Outcome {
        Outcome::Failed(Failure {
            kind: FailureKind::Auth,
            exit_code: Some(128),
            git_stderr: "Permission denied (publickey)".into(),
        })
    }

    fn report(repositories: Vec<RepositoryReport>, cancelled: bool) -> Report {
        Report {
            product_name: "Qeet ID".into(),
            repositories,
            cancelled,
            elapsed: Duration::from_secs(1),
        }
    }

    #[test]
    fn a_clean_run_is_complete() {
        let report =
            report(vec![entry("a", Outcome::Cloned), entry("b", Outcome::AlreadyPresent)], false);
        assert_eq!(report.total(), 2);
        assert_eq!(report.cloned(), 1);
        assert_eq!(report.already_present(), 1);
        assert_eq!(report.succeeded(), 2);
        assert_eq!(report.failed(), 0);
        assert!(report.is_complete(), "already-present must count as success");
        assert_eq!(report.problems().count(), 0);
    }

    /// The brief's worked example: six repositories, two of them broken.
    #[test]
    fn a_partial_failure_counts_both_sides() {
        let report = report(
            vec![
                entry("repo1", Outcome::Cloned),
                entry("repo2", Outcome::Cloned),
                entry("repo3", failure()),
                entry("repo4", Outcome::Cloned),
                entry("repo5", Outcome::Cloned),
                entry("repo6", Outcome::Blocked(Blocked::NotARepository)),
            ],
            false,
        );

        assert_eq!(report.succeeded(), 4);
        assert_eq!(report.failed(), 2);
        assert!(!report.is_complete(), "must not report success");

        let names: Vec<&str> = report.problems().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["repo3", "repo6"], "problems keep manifest order");
    }

    #[test]
    fn cancellation_is_never_reported_as_complete() {
        // Even if everything that ran happened to succeed.
        let report =
            report(vec![entry("a", Outcome::Cloned), entry("b", Outcome::Cancelled)], true);
        assert_eq!(report.cancelled_repositories(), 1);
        assert!(!report.is_complete());
    }

    #[test]
    fn outcome_labels_are_distinct() {
        let labels = [
            Outcome::Cloned.label(),
            Outcome::AlreadyPresent.label(),
            failure().label(),
            Outcome::Blocked(Blocked::NotARepository).label(),
            Outcome::Cancelled.label(),
        ];
        let unique: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len(), "{labels:?}");
    }
}
