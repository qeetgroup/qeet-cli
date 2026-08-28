//! The `git` adapter.
//!
//! `qeet` orchestrates git; it does not reimplement it. Everything a developer has already
//! configured -- SSH keys and agent, credential helpers, `insteadOf` rewrites, proxies,
//! signing -- keeps working because the real `git` executable does the work.

pub mod classify;
pub mod client;

use std::future::Future;
use std::path::{Path, PathBuf};

pub use classify::FailureKind;
pub use client::Git;

/// One repository to clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneRequest {
    /// Repository name, used in messages.
    pub name: String,
    /// Already validated by [`crate::remote::validate`].
    pub url: String,
    pub destination: PathBuf,
    /// Branch or tag to clone, when the manifest pins one.
    pub git_ref: Option<String>,
}

/// Why a clone did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    /// git's exit code, absent when git could not be run at all.
    pub exit_code: Option<i32>,
    /// The lines of git's own stderr worth showing, verbatim. Never invented.
    pub git_stderr: String,
}

impl Failure {
    /// A one-line summary in the CLI's voice.
    pub fn summary(&self) -> &'static str {
        self.kind.summary()
    }

    /// Concrete next steps. Deliberately short: enough to fix it, not a wall of text.
    pub fn guidance(&self) -> &'static [&'static str] {
        self.kind.guidance()
    }

    pub fn retryable(&self) -> bool {
        self.kind.retryable()
    }
}

/// `git` itself is unusable, which is a configuration problem rather than a clone failure.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("`git` was not found. Install git and make sure it is on your PATH.")]
    NotFound,

    #[error("`git` could not be run: {0}")]
    Unusable(String),
}

/// What `qeet` asks of git.
///
/// A trait so the clone coordinator can be tested without a network or a real remote. It
/// uses return-position `impl Future` with an explicit `Send` bound rather than the
/// `async_trait` crate: `JoinSet` requires `Send` futures, which a bare `async fn` in a
/// trait cannot promise.
pub trait GitClient: Send + Sync + 'static {
    /// Clone one repository. Returns `Ok(())` on success; never panics on a git failure.
    fn clone_repo(&self, request: CloneRequest)
    -> impl Future<Output = Result<(), Failure>> + Send;

    /// The `origin` URL of an existing repository, or `None` when it has no `origin`.
    ///
    /// Used by workspace preflight to decide whether a directory already holds the
    /// repository we were about to clone.
    fn origin_url(
        &self,
        repository: PathBuf,
    ) -> impl Future<Output = Result<Option<String>, GitError>> + Send;

    /// Inspect an existing repository: branch, cleanliness, and how it sits relative to its
    /// upstream. Read-only.
    fn inspect(
        &self,
        repository: PathBuf,
    ) -> impl Future<Output = Result<RepoState, GitError>> + Send;

    /// `git fetch` the repository's remote. Touches no working tree.
    fn fetch(&self, repository: PathBuf) -> impl Future<Output = Result<(), Failure>> + Send;

    /// Fast-forward the current branch to its upstream. Refuses to create a merge commit, so
    /// it can only succeed where the result is unambiguous.
    fn fast_forward(&self, repository: PathBuf)
    -> impl Future<Output = Result<(), Failure>> + Send;
}

/// A snapshot of an existing repository, as `status` and `update` see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoState {
    /// Current branch, or `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Upstream tracking ref, e.g. `origin/main`. `None` when the branch tracks nothing.
    pub upstream: Option<String>,
    /// Commits the branch has that its upstream does not.
    pub ahead: u32,
    /// Commits the upstream has that the branch does not.
    pub behind: u32,
    /// Number of changed entries in `git status --porcelain`, staged or not, including
    /// untracked files.
    pub dirty: u32,
}

impl RepoState {
    /// Can this repository be advanced without any judgement call?
    ///
    /// Only when it is clean, tracks an upstream, has nothing of its own to lose, and is
    /// strictly behind. Everything else is a decision for the developer, not for qeet.
    pub fn fast_forwardable(&self) -> bool {
        // `branch.is_some()` is not redundant: a detached HEAD can still report an upstream
        // and a behind count, and fast-forwarding one is not a meaningful operation. Caught
        // by `everything_ambiguous_is_refused`.
        self.branch.is_some()
            && self.upstream.is_some()
            && self.dirty == 0
            && self.ahead == 0
            && self.behind > 0
    }

    /// Already exactly where its upstream is.
    pub fn up_to_date(&self) -> bool {
        self.dirty == 0 && self.ahead == 0 && self.behind == 0
    }

    /// Why qeet will not touch this repository, if it will not.
    pub fn blocker(&self) -> Option<String> {
        if self.dirty > 0 {
            return Some(format!(
                "{} uncommitted change{}",
                self.dirty,
                if self.dirty == 1 { "" } else { "s" }
            ));
        }
        if self.branch.is_none() {
            return Some("detached HEAD".to_string());
        }
        if self.upstream.is_none() {
            return Some("no upstream branch".to_string());
        }
        if self.ahead > 0 && self.behind > 0 {
            return Some(format!("diverged: {} ahead, {} behind", self.ahead, self.behind));
        }
        if self.ahead > 0 {
            return Some(format!(
                "{} commit{} not pushed",
                self.ahead,
                if self.ahead == 1 { "" } else { "s" }
            ));
        }
        None
    }
}

/// Build the argument vector for a clone.
///
/// Separate from process spawning so the exact arguments can be asserted in tests. Note
/// the `--`: it stops git reading a URL or destination as an option, which together with
/// [`crate::remote::validate`] closes off argument injection.
pub fn clone_args(request: &CloneRequest) -> Vec<std::ffi::OsString> {
    use std::ffi::OsString;

    let mut args: Vec<OsString> = vec!["clone".into(), "--progress".into()];
    if let Some(git_ref) = &request.git_ref {
        args.push("--branch".into());
        args.push(git_ref.into());
    }
    args.push("--".into());
    args.push(request.url.clone().into());
    args.push(request.destination.clone().into_os_string());
    args
}

/// Does this directory hold a git repository?
///
/// `.git` may be a directory or, in a worktree or submodule, a file.
pub fn is_git_repository(path: &Path) -> bool {
    path.join(".git").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> CloneRequest {
        CloneRequest {
            name: "qeet-id-server".into(),
            url: "git@github.com:qeetgroup/qeet-id-server.git".into(),
            destination: PathBuf::from("/work/qeet-id-server"),
            git_ref: None,
        }
    }

    #[test]
    fn builds_a_plain_clone() {
        assert_eq!(
            clone_args(&request()),
            [
                "clone",
                "--progress",
                "--",
                "git@github.com:qeetgroup/qeet-id-server.git",
                "/work/qeet-id-server",
            ]
        );
    }

    #[test]
    fn builds_a_clone_pinned_to_a_ref() {
        let request = CloneRequest { git_ref: Some("develop".into()), ..request() };
        assert_eq!(
            clone_args(&request),
            [
                "clone",
                "--progress",
                "--branch",
                "develop",
                "--",
                "git@github.com:qeetgroup/qeet-id-server.git",
                "/work/qeet-id-server",
            ]
        );
    }

    #[test]
    fn the_url_always_sits_after_a_double_dash() {
        // Even a hostile URL that slipped past validation cannot become an option.
        let request = CloneRequest { url: "--upload-pack=touch /tmp/pwned".into(), ..request() };
        let args = clone_args(&request);
        let separator = args.iter().position(|arg| arg == "--").expect("`--` must be present");
        let url = args
            .iter()
            .position(|arg| arg == "--upload-pack=touch /tmp/pwned")
            .expect("url must be present");
        assert!(separator < url, "the URL must follow `--`: {args:?}");
    }

    #[test]
    fn detects_a_repository_by_its_git_entry() {
        let dir = tempfile::tempdir().expect("tempdir");

        assert!(!is_git_repository(dir.path()));

        // A worktree or submodule has `.git` as a file, not a directory.
        std::fs::write(dir.path().join(".git"), "gitdir: /elsewhere").expect("write");
        assert!(is_git_repository(dir.path()));
    }
}

#[cfg(test)]
mod state_tests {
    use super::RepoState;

    fn state(dirty: u32, ahead: u32, behind: u32, upstream: bool, branch: bool) -> RepoState {
        RepoState {
            branch: branch.then(|| "main".to_string()),
            upstream: upstream.then(|| "origin/main".to_string()),
            ahead,
            behind,
            dirty,
        }
    }

    #[test]
    fn only_a_clean_repository_strictly_behind_can_be_fast_forwarded() {
        assert!(state(0, 0, 3, true, true).fast_forwardable());
    }

    #[test]
    fn everything_ambiguous_is_refused() {
        // Each of these could lose work or need a merge, so qeet declines and says why.
        for (label, s) in [
            ("dirty", state(2, 0, 3, true, true)),
            ("diverged", state(0, 2, 3, true, true)),
            ("ahead only", state(0, 2, 0, true, true)),
            ("no upstream", state(0, 0, 3, false, true)),
            ("detached", state(0, 0, 3, true, false)),
            ("already current", state(0, 0, 0, true, true)),
        ] {
            assert!(!s.fast_forwardable(), "{label} must not be fast-forwarded");
        }
    }

    #[test]
    fn a_clean_current_repository_needs_nothing() {
        let s = state(0, 0, 0, true, true);
        assert!(s.up_to_date());
        assert_eq!(s.blocker(), None);
    }

    #[test]
    fn blockers_name_the_actual_problem() {
        assert_eq!(state(1, 0, 0, true, true).blocker().unwrap(), "1 uncommitted change");
        assert_eq!(state(3, 0, 0, true, true).blocker().unwrap(), "3 uncommitted changes");
        assert_eq!(state(0, 0, 0, true, false).blocker().unwrap(), "detached HEAD");
        assert_eq!(state(0, 0, 0, false, true).blocker().unwrap(), "no upstream branch");
        assert_eq!(state(0, 2, 4, true, true).blocker().unwrap(), "diverged: 2 ahead, 4 behind");
        assert_eq!(state(0, 1, 0, true, true).blocker().unwrap(), "1 commit not pushed");
    }

    /// Dirtiness is reported before anything else: it is the one that risks losing work.
    #[test]
    fn dirty_takes_priority_over_divergence() {
        assert!(state(2, 5, 5, true, true).blocker().unwrap().contains("uncommitted"));
    }
}
