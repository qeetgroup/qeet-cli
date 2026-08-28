//! Running the real `git` executable.

use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;

use tokio::process::Command;

use super::{
    CloneRequest, Failure, FailureKind, GitClient, GitError, RepoState, classify, clone_args,
};

/// Batch mode makes `ssh` fail instead of prompting. Applied only when the developer has
/// not configured their own SSH command.
const SSH_BATCH_MODE: &str = "ssh -o BatchMode=yes";

/// A verified `git` executable.
#[derive(Debug, Clone)]
pub struct Git {
    program: OsString,
    /// `Some` when qeet supplies `GIT_SSH_COMMAND`; `None` when the developer already has
    /// one and we must not override it.
    ssh_command: Option<OsString>,
}

impl Git {
    /// Check that git works, once, before any repository is attempted.
    ///
    /// Without this, a missing git would produce one identical failure per repository --
    /// eleven confusing errors instead of one clear one.
    pub async fn discover() -> Result<Self, GitError> {
        let program = OsString::from("git");

        let output =
            Command::new(&program).arg("--version").stdin(Stdio::null()).output().await.map_err(
                |err| match err.kind() {
                    std::io::ErrorKind::NotFound => GitError::NotFound,
                    _ => GitError::Unusable(err.to_string()),
                },
            )?;

        if !output.status.success() {
            return Err(GitError::Unusable(format!(
                "`git --version` exited with {}",
                describe_status(output.status.code())
            )));
        }

        let ssh_command = decide_ssh_command(&program).await;

        Ok(Self { program, ssh_command })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);

        // Nothing qeet spawns may block on a prompt. With several clones in flight on one
        // terminal, interleaved prompts are unreadable and a single stalled child would
        // hold up the whole run. Authentication must therefore succeed or fail fast.
        command.env("GIT_TERMINAL_PROMPT", "0");
        if let Some(ssh_command) = &self.ssh_command {
            command.env("GIT_SSH_COMMAND", ssh_command);
        }

        // git must never inherit our stdin: ssh and credential helpers would read from it.
        command.stdin(Stdio::null());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        // If this task is dropped -- on Ctrl-C, or when the JoinSet is aborted -- the child
        // git process is killed rather than orphaned.
        command.kill_on_drop(true);

        command
    }
}

// `impl Future + Send` rather than `async fn`: `JoinSet` requires `Send` futures, which a
// bare `async fn` in a trait cannot promise. Writing it out is what lets qeet avoid the
// `async_trait` crate entirely.
#[allow(clippy::manual_async_fn)]
impl GitClient for Git {
    fn clone_repo(
        &self,
        request: CloneRequest,
    ) -> impl Future<Output = Result<(), Failure>> + Send {
        let mut command = self.command();
        command.args(clone_args(&request));

        async move {
            let output = match command.output().await {
                Ok(output) => output,
                Err(err) => {
                    return Err(Failure {
                        kind: FailureKind::Spawn,
                        exit_code: None,
                        git_stderr: err.to_string(),
                    });
                }
            };

            if output.status.success() {
                return Ok(());
            }

            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(Failure {
                kind: classify::classify(&stderr),
                exit_code: output.status.code(),
                git_stderr: classify::relevant_stderr(&stderr),
            })
        }
    }

    fn origin_url(
        &self,
        repository: PathBuf,
    ) -> impl Future<Output = Result<Option<String>, GitError>> + Send {
        let mut command = self.command();
        command.arg("-C").arg(&repository).args(["config", "--get", "remote.origin.url"]);

        async move {
            let output =
                command.output().await.map_err(|err| GitError::Unusable(err.to_string()))?;

            // A non-zero exit means "no origin configured" or "not a repository". Both are
            // absence of an answer, not a broken git, so the caller decides what to do.
            if !output.status.success() {
                return Ok(None);
            }

            let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(if url.is_empty() { None } else { Some(url) })
        }
    }

    fn inspect(
        &self,
        repository: PathBuf,
    ) -> impl Future<Output = Result<RepoState, GitError>> + Send {
        // One porcelain call answers branch, upstream and ahead/behind together, which is
        // both fewer processes and a consistent snapshot -- three separate calls could
        // disagree if something changed underneath them.
        let mut status = self.command();
        status.arg("-C").arg(&repository).args(["status", "--porcelain=v2", "--branch"]);

        async move {
            let output =
                status.output().await.map_err(|err| GitError::Unusable(err.to_string()))?;
            if !output.status.success() {
                return Err(GitError::Unusable(format!(
                    "`git status` failed in {}: {}",
                    repository.display(),
                    classify::relevant_stderr(&String::from_utf8_lossy(&output.stderr))
                )));
            }
            Ok(parse_status(&String::from_utf8_lossy(&output.stdout)))
        }
    }

    fn fetch(&self, repository: PathBuf) -> impl Future<Output = Result<(), Failure>> + Send {
        let mut command = self.command();
        command
            .arg("-C")
            .arg(&repository)
            // --prune keeps stale remote branches from accumulating; --tags so a release tag
            // shows up without a second call.
            .args(["fetch", "--prune", "--tags", "--quiet"]);
        async move { run_for_failure(command).await }
    }

    fn fast_forward(
        &self,
        repository: PathBuf,
    ) -> impl Future<Output = Result<(), Failure>> + Send {
        let mut command = self.command();
        command
            .arg("-C")
            .arg(&repository)
            // --ff-only is the whole safety guarantee: git refuses rather than creating a
            // merge commit, so this cannot invent history or leave a conflict behind.
            .args(["merge", "--ff-only", "--quiet"]);
        async move { run_for_failure(command).await }
    }
}

/// Run a git command, turning a non-zero exit into a classified [`Failure`].
async fn run_for_failure(mut command: Command) -> Result<(), Failure> {
    let output = match command.output().await {
        Ok(output) => output,
        Err(err) => {
            return Err(Failure {
                kind: FailureKind::Spawn,
                exit_code: None,
                git_stderr: err.to_string(),
            });
        }
    };
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(Failure {
        kind: classify::classify(&stderr),
        exit_code: output.status.code(),
        git_stderr: classify::relevant_stderr(&stderr),
    })
}

/// Parse `git status --porcelain=v2 --branch`.
///
/// The v2 format is used precisely because it is documented as stable and machine-readable;
/// v1 and the human-readable output are not.
fn parse_status(stdout: &str) -> RepoState {
    let mut state = RepoState { branch: None, upstream: None, ahead: 0, behind: 0, dirty: 0 };

    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            // git writes the literal "(detached)" rather than a branch name.
            let head = rest.trim();
            if head != "(detached)" {
                state.branch = Some(head.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("# branch.upstream ") {
            state.upstream = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            // Format: "+<ahead> -<behind>".
            for field in rest.split_whitespace() {
                match field.split_at(1) {
                    ("+", n) => state.ahead = n.parse().unwrap_or(0),
                    ("-", n) => state.behind = n.parse().unwrap_or(0),
                    _ => {}
                }
            }
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            // Every remaining line is a changed, renamed, unmerged or untracked entry.
            state.dirty += 1;
        }
    }

    state
}

/// Decide whether to supply `GIT_SSH_COMMAND`.
///
/// The developer's own SSH configuration always wins: an explicit `GIT_SSH_COMMAND` in the
/// environment, or `core.sshCommand` in any git config file, means qeet leaves it alone
/// even though that reintroduces the risk of an SSH passphrase prompt.
async fn decide_ssh_command(program: &OsString) -> Option<OsString> {
    let inherited = std::env::var_os("GIT_SSH_COMMAND");

    // Skipped entirely when the environment already answers the question.
    let configured = if inherited.as_ref().is_some_and(|value| !value.is_empty()) {
        false
    } else {
        Command::new(program)
            .args(["config", "--get", "core.sshCommand"])
            .stdin(Stdio::null())
            .output()
            .await
            .is_ok_and(|output| {
                output.status.success()
                    && !String::from_utf8_lossy(&output.stdout).trim().is_empty()
            })
    };

    decide_ssh_command_for(inherited.as_deref().and_then(|v| v.to_str()), configured)
}

/// The rule itself, separated from the two lookups that feed it so it can be tested.
fn decide_ssh_command_for(inherited: Option<&str>, configured: bool) -> Option<OsString> {
    if inherited.is_some_and(|value| !value.is_empty()) || configured {
        return None;
    }
    Some(OsString::from(SSH_BATCH_MODE))
}

fn describe_status(code: Option<i32>) -> String {
    code.map_or_else(|| "a signal".to_string(), |code| format!("code {code}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// git is a prerequisite of this project's own test suite, so this is a real check
    /// rather than a mocked one.
    #[tokio::test]
    async fn discovers_the_installed_git() {
        Git::discover().await.expect("git must be installed to run these tests");
    }

    #[tokio::test]
    async fn respects_a_developers_own_ssh_command() {
        // A configured GIT_SSH_COMMAND must win: qeet adds batch mode only when the
        // developer has expressed no preference.
        assert_eq!(
            decide_ssh_command_for(Some("ssh -i ~/.ssh/work"), false),
            None,
            "an inherited GIT_SSH_COMMAND must not be overridden"
        );
        assert_eq!(
            decide_ssh_command_for(None, true),
            None,
            "a configured core.sshCommand must not be overridden"
        );
        assert_eq!(
            decide_ssh_command_for(None, false).as_deref(),
            Some(std::ffi::OsStr::new(SSH_BATCH_MODE)),
            "with no preference expressed, batch mode prevents a hung prompt"
        );
        assert_eq!(
            decide_ssh_command_for(Some(""), false).as_deref(),
            Some(std::ffi::OsStr::new(SSH_BATCH_MODE)),
            "an empty GIT_SSH_COMMAND is not a preference"
        );
    }

    #[tokio::test]
    async fn reports_no_origin_for_a_plain_directory() {
        let git = Git::discover().await.expect("git");
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(git.origin_url(dir.path().to_path_buf()).await.expect("should not error"), None);
    }

    #[tokio::test]
    async fn reads_the_origin_of_a_real_repository() {
        let git = Git::discover().await.expect("git");
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).expect("create");

        for args in [
            vec!["init", "--quiet"],
            vec!["remote", "add", "origin", "git@github.com:qeetgroup/example.git"],
        ] {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(&args)
                .status()
                .expect("git should run");
            assert!(status.success(), "{args:?}");
        }

        assert_eq!(
            git.origin_url(repo).await.expect("should not error"),
            Some("git@github.com:qeetgroup/example.git".to_string())
        );
    }

    #[tokio::test]
    async fn a_failed_clone_is_classified_not_panicked() {
        let git = Git::discover().await.expect("git");
        let dir = tempfile::tempdir().expect("tempdir");

        let failure = git
            .clone_repo(CloneRequest {
                name: "absent".into(),
                url: format!("file://{}", dir.path().join("absent.git").display()),
                destination: dir.path().join("out"),
                git_ref: None,
            })
            .await
            .expect_err("cloning a nonexistent path must fail");

        assert_eq!(failure.kind, FailureKind::NotFound, "{failure:?}");
        assert!(!failure.git_stderr.is_empty(), "git's own words must be preserved");
        assert!(!failure.retryable(), "a missing repository must not be retried");
    }
}

#[cfg(test)]
mod status_tests {
    use super::parse_status;

    /// Real `git status --porcelain=v2 --branch` output, clean and current.
    #[test]
    fn parses_a_clean_current_repository() {
        let out = "# branch.oid 8f1b0c2\n\
                   # branch.head main\n\
                   # branch.upstream origin/main\n\
                   # branch.ab +0 -0\n";
        let s = parse_status(out);
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!((s.ahead, s.behind, s.dirty), (0, 0, 0));
        assert!(s.up_to_date());
    }

    #[test]
    fn parses_ahead_and_behind() {
        let out = "# branch.head develop\n\
                   # branch.upstream origin/develop\n\
                   # branch.ab +2 -5\n";
        let s = parse_status(out);
        assert_eq!((s.ahead, s.behind), (2, 5));
        assert!(!s.fast_forwardable(), "diverged must not be fast-forwarded");
    }

    #[test]
    fn counts_every_kind_of_change() {
        let out = "# branch.head main\n\
                   # branch.upstream origin/main\n\
                   # branch.ab +0 -0\n\
                   1 .M N... 100644 100644 100644 abc abc src/main.rs\n\
                   1 M. N... 100644 100644 100644 abc abc Cargo.toml\n\
                   2 R. N... 100644 100644 100644 abc abc R100 new\told\n\
                   u UU N... 100644 100644 100644 100644 abc abc abc conflicted.rs\n\
                   ? untracked.txt\n";
        let s = parse_status(out);
        assert_eq!(s.dirty, 5, "modified, staged, renamed, unmerged and untracked all count");
        assert!(!s.fast_forwardable());
        assert!(s.blocker().unwrap().contains("uncommitted"));
    }

    #[test]
    fn detects_a_detached_head() {
        let out = "# branch.oid 8f1b0c2\n# branch.head (detached)\n";
        let s = parse_status(out);
        assert_eq!(s.branch, None);
        assert_eq!(s.blocker().unwrap(), "detached HEAD");
    }

    #[test]
    fn a_branch_with_no_upstream_has_no_ab_line() {
        let out = "# branch.head local-only\n";
        let s = parse_status(out);
        assert_eq!(s.branch.as_deref(), Some("local-only"));
        assert_eq!(s.upstream, None);
        assert_eq!(s.blocker().unwrap(), "no upstream branch");
    }
}

#[cfg(test)]
mod live_git_tests {
    use super::*;

    /// Against a real repository, cloned from a real bare remote, with no network.
    #[tokio::test]
    async fn inspects_a_real_repository() {
        let git = Git::discover().await.expect("git");
        let dir = tempfile::tempdir().expect("tempdir");
        let bare = dir.path().join("origin.git");
        let work = dir.path().join("work");

        let run = |args: Vec<&str>, cwd: Option<&std::path::Path>| {
            let mut c = std::process::Command::new("git");
            c.args(["-c", "user.name=t", "-c", "user.email=t@e", "-c", "commit.gpgsign=false"]);
            if let Some(cwd) = cwd {
                c.arg("-C").arg(cwd);
            }
            c.args(&args);
            let out = c.output().expect("git runs");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };

        run(
            vec!["init", "--bare", "--quiet", "--initial-branch=main", bare.to_str().unwrap()],
            None,
        );
        run(vec!["clone", "--quiet", bare.to_str().unwrap(), work.to_str().unwrap()], None);
        std::fs::write(work.join("a.txt"), "one\n").expect("write");
        run(vec!["add", "-A"], Some(&work));
        run(vec!["commit", "--quiet", "-m", "one"], Some(&work));
        run(vec!["push", "--quiet", "-u", "origin", "main"], Some(&work));

        let clean = git.inspect(work.clone()).await.expect("inspect");
        assert_eq!(clean.branch.as_deref(), Some("main"));
        assert_eq!(clean.upstream.as_deref(), Some("origin/main"));
        assert!(clean.up_to_date(), "{clean:?}");

        // An untracked file makes it dirty, and therefore not fast-forwardable.
        std::fs::write(work.join("scratch.txt"), "wip\n").expect("write");
        let dirty = git.inspect(work.clone()).await.expect("inspect");
        assert_eq!(dirty.dirty, 1, "{dirty:?}");
        assert!(!dirty.fast_forwardable());
        assert!(dirty.blocker().unwrap().contains("uncommitted"));
    }

    /// fetch + fast_forward advance a clean repository that is strictly behind.
    #[tokio::test]
    async fn fast_forwards_a_repository_that_is_behind() {
        let git = Git::discover().await.expect("git");
        let dir = tempfile::tempdir().expect("tempdir");
        let bare = dir.path().join("origin.git");
        let author = dir.path().join("author");
        let follower = dir.path().join("follower");

        let run = |args: Vec<&str>, cwd: Option<&std::path::Path>| {
            let mut c = std::process::Command::new("git");
            c.args(["-c", "user.name=t", "-c", "user.email=t@e", "-c", "commit.gpgsign=false"]);
            if let Some(cwd) = cwd {
                c.arg("-C").arg(cwd);
            }
            c.args(&args);
            assert!(c.output().expect("git runs").status.success(), "git {args:?}");
        };

        run(
            vec!["init", "--bare", "--quiet", "--initial-branch=main", bare.to_str().unwrap()],
            None,
        );
        run(vec!["clone", "--quiet", bare.to_str().unwrap(), author.to_str().unwrap()], None);
        std::fs::write(author.join("a.txt"), "one\n").expect("write");
        run(vec!["add", "-A"], Some(&author));
        run(vec!["commit", "--quiet", "-m", "one"], Some(&author));
        run(vec!["push", "--quiet", "-u", "origin", "main"], Some(&author));

        // The follower starts level, then the author pushes another commit.
        run(vec!["clone", "--quiet", bare.to_str().unwrap(), follower.to_str().unwrap()], None);
        std::fs::write(author.join("b.txt"), "two\n").expect("write");
        run(vec!["add", "-A"], Some(&author));
        run(vec!["commit", "--quiet", "-m", "two"], Some(&author));
        run(vec!["push", "--quiet", "origin", "main"], Some(&author));

        // Before fetching, the follower cannot know it is behind.
        assert!(git.inspect(follower.clone()).await.expect("inspect").up_to_date());

        git.fetch(follower.clone()).await.expect("fetch should succeed");
        let behind = git.inspect(follower.clone()).await.expect("inspect");
        assert_eq!((behind.ahead, behind.behind), (0, 1), "{behind:?}");
        assert!(behind.fast_forwardable());

        git.fast_forward(follower.clone()).await.expect("fast-forward should succeed");
        assert!(follower.join("b.txt").exists(), "the new commit's file should be present");
        assert!(git.inspect(follower).await.expect("inspect").up_to_date());
    }
}
