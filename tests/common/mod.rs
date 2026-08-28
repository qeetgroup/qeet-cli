// Each integration test file compiles this module separately, so a helper used by one
// file looks dead to the others.
#![allow(dead_code)]

//! Shared fixtures for the integration tests.
//!
//! A `tests/common/` subdirectory is not itself a test target, which is the conventional
//! way to share helpers between integration tests.
//!
//! Everything here is real: real bare git repositories on disk, cloned over `file://` by
//! the real `git` executable, driven through the real `qeet` binary. No network, no
//! credentials, no mocking -- so these tests run identically on a laptop and in CI.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use tempfile::TempDir;

/// A temporary world: some bare repositories to clone from, and a workspace to clone into.
pub struct Fixture {
    root: TempDir,
    pub remotes: PathBuf,
    pub work: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let root = TempDir::new().expect("create temp dir");
        let remotes = root.path().join("remotes");
        let work = root.path().join("work");
        std::fs::create_dir_all(&remotes).expect("create remotes dir");
        std::fs::create_dir_all(&work).expect("create work dir");
        Self { root, remotes, work }
    }

    /// The workspace path, resolved the same way `qeet` resolves it.
    pub fn work(&self) -> PathBuf {
        self.work.canonicalize().expect("canonicalize workspace")
    }

    /// Create a bare repository with one commit on `main`, plus the extra branches given.
    ///
    /// Returns a `file://` URL for it.
    pub fn bare_repo(&self, name: &str, extra_branches: &[&str]) -> String {
        let bare = self.remotes.join(format!("{name}.git"));
        git(self.root.path(), &["init", "--quiet", "--bare", "--initial-branch=main"])
            .args([bare.to_str().expect("utf-8 path")])
            .assert_ok();

        // A seed working copy, so the bare repository has real history to clone.
        let seed = self.root.path().join(format!("seed-{name}"));
        std::fs::create_dir_all(&seed).expect("create seed dir");
        git(&seed, &["init", "--quiet", "--initial-branch=main"]).assert_ok();
        std::fs::write(seed.join("README.md"), format!("# {name}\n")).expect("write readme");
        git(&seed, &["add", "-A"]).assert_ok();
        git(&seed, &["commit", "--quiet", "-m", "initial commit"]).assert_ok();

        for branch in extra_branches {
            git(&seed, &["branch", branch]).assert_ok();
        }

        git(&seed, &["remote", "add", "origin"])
            .args([bare.to_str().expect("utf-8 path")])
            .assert_ok();
        git(&seed, &["push", "--quiet", "--all", "origin"]).assert_ok();

        file_url(&bare)
    }

    /// Write a manifest into the fixture and return its path.
    pub fn manifest(&self, body: &str) -> PathBuf {
        let path = self.root.path().join("products.toml");
        std::fs::write(&path, body).expect("write manifest");
        path
    }

    /// A manifest for one product built from `(name, url)` pairs.
    pub fn manifest_for(&self, product: &str, repositories: &[(&str, String)]) -> PathBuf {
        let entries = repositories
            .iter()
            .map(|(name, url)| format!("  {{ name = \"{name}\", url = \"{url}\" }},\n"))
            .collect::<String>();

        self.manifest(&format!(
            "schema = 1\n\
             [remote]\n\
             host = \"example.invalid\"\n\
             owner = \"fixture\"\n\
             protocol = \"https\"\n\
             [products.{product}]\n\
             name = \"Test Product\"\n\
             repositories = [\n{entries}]\n"
        ))
    }

    /// `qeet`, run from the workspace directory, fully isolated from the developer's machine.
    ///
    /// Isolation is not optional here. qeet's manifest precedence includes a user config
    /// directory, so without redirecting the variables that locate it, a config file in the
    /// developer's home directory silently becomes an input to every test -- which is exactly
    /// how `repos_honours_the_protocol_override` started failing once one existed.
    pub fn qeet(&self) -> Command {
        let mut command = Command::cargo_bin("qeet").expect("qeet binary should be built");
        command.current_dir(&self.work);

        command.env_remove("QEET_MANIFEST");
        // Point every platform's config-directory root at a directory that has no config in
        // it: `$HOME` for the Apple and XDG strategies, `%APPDATA%` for Windows.
        let empty = self.root.path().join("fake-home");
        std::fs::create_dir_all(&empty).expect("create fake home");
        command.env("HOME", &empty);
        command.env("XDG_CONFIG_HOME", empty.join(".config"));
        command.env("APPDATA", &empty);
        command
    }

    /// Is this path inside the workspace a git repository qeet cloned?
    pub fn is_cloned(&self, name: &str) -> bool {
        let path = self.work.join(name);
        path.join(".git").exists() && path.join("README.md").exists()
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.work.join(name)
    }
}

/// A `file://` URL for a local path, correct on Windows as well as Unix.
pub fn file_url(path: &Path) -> String {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let text = resolved.to_string_lossy().replace('\\', "/");
    // Windows canonicalisation yields a `\\?\C:\...` verbatim prefix, which is not a URL.
    let text = text.trim_start_matches("//?/").to_string();
    if text.starts_with('/') { format!("file://{text}") } else { format!("file:///{text}") }
}

/// A `git` invocation in `dir`, with identity and hooks pinned so it behaves the same on
/// any developer machine and in CI.
pub fn git(dir: &Path, args: &[&str]) -> StdCommand {
    let mut command = StdCommand::new("git");
    command
        .current_dir(dir)
        .args(["-c", "user.name=qeet-cli tests"])
        .args(["-c", "user.email=tests@qeet.invalid"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args);
    command
}

/// Run a `git` command and fail the test loudly if it did not succeed.
pub trait AssertOk {
    fn assert_ok(&mut self);
}

impl AssertOk for StdCommand {
    fn assert_ok(&mut self) {
        let output = self.output().expect("git should be installed and runnable");
        assert!(
            output.status.success(),
            "git {:?} failed:\n{}",
            self.get_args().collect::<Vec<_>>(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
