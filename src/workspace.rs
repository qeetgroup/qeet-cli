//! Where repositories go, and whether it is safe to put them there.
//!
//! The workspace is one fixed folder, `~/QG`, whatever directory qeet is run from -- so
//! `qeet pull all` works from anywhere, and every developer's layout is the same.
//! [`ENV_VAR`] moves it. Within it, a product's repositories are grouped under the
//! product's directory: `~/QG/qeet-id/qeet-id-server/`.
//!
//! Every destination is classified *before* any git process starts. Nothing that already
//! exists is ever deleted or overwritten, and when this module cannot establish what a
//! directory is, it refuses to touch it.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::git::{self, GitClient};
use crate::manifest::{Manifest, Product};
use crate::remote::{self, Protocol, UrlMatch};

/// The workspace folder in the home directory, unless [`ENV_VAR`] says otherwise.
///
/// The home directory rather than the Desktop: macOS (iCloud) and Windows (OneDrive) both
/// commonly sync the Desktop, and syncing a `.git` directory is how repositories get slow
/// and corrupted.
pub const DEFAULT_DIRECTORY: &str = "QG";

/// Set to put the workspace somewhere other than `~/QG`.
pub const ENV_VAR: &str = "QEET_WORKSPACE";

/// Where the workspace is, and what decided that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    /// Chosen by [`ENV_VAR`] rather than being the default.
    pub from_env: bool,
}

impl Location {
    /// The workspace location for this process.
    pub fn resolve() -> io::Result<Self> {
        locate(std::env::var_os(ENV_VAR), etcetera::home_dir().ok(), std::env::current_dir)
    }
}

/// [`Location::resolve`], with the environment passed in so it can be tested.
fn locate(
    env: Option<OsString>,
    home: Option<PathBuf>,
    current_dir: impl FnOnce() -> io::Result<PathBuf>,
) -> io::Result<Location> {
    // Empty is treated as unset, as an empty `QEET_MANIFEST` is, rather than as "here".
    if let Some(value) = env.filter(|value| !value.is_empty()) {
        let path = expand_home(PathBuf::from(value), home.as_deref());
        let path = if path.is_absolute() { path } else { current_dir()?.join(path) };
        return Ok(Location { path, from_env: true });
    }

    let home = home.ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "cannot find your home directory")
    })?;
    Ok(Location { path: home.join(DEFAULT_DIRECTORY), from_env: false })
}

/// `~` means the home directory even where no shell expanded it: a quoted
/// `QEET_WORKSPACE="~/code/qg"`, or Windows, where no shell ever does.
fn expand_home(path: PathBuf, home: Option<&Path>) -> PathBuf {
    let mut components = path.components();
    match (components.next(), home) {
        (Some(Component::Normal(first)), Some(home)) if first == "~" => {
            let rest = components.as_path();
            if rest.as_os_str().is_empty() { home.to_path_buf() } else { home.join(rest) }
        }
        _ => path,
    }
}

/// An I/O error that names its path. The workspace is no longer simply "here", so an
/// error that does not say where would leave the developer guessing.
fn naming(path: &Path, err: io::Error) -> io::Error {
    io::Error::new(err.kind(), format!("{}: {err}", path.display()))
}

/// The directory repositories are cloned into, resolved.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

/// What should happen to one destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Nothing is there. qeet creates it, and therefore owns it: if the clone fails, the
    /// directory it created is removed again.
    Create,
    /// The directory exists and is empty. git can clone into it, but qeet did not create
    /// it and will not remove it.
    FillEmpty,
    /// The repository is already here and its `origin` is the one we would have cloned.
    /// Nothing to do -- this is what makes re-running `qeet clone` safe.
    AlreadyPresent,
    /// Refused. Nothing is read further, written, or removed.
    Blocked(Blocked),
}

impl State {
    /// Did qeet create this directory, and may it therefore clean it up on failure?
    pub fn owns_destination(&self) -> bool {
        matches!(self, Self::Create)
    }
}

/// Why a destination was refused. Each variant names the evidence, so the developer can
/// check the conclusion themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    /// A git repository, but pointing somewhere else.
    DifferentRepository { origin: String, expected: String },
    /// A git repository whose identity could not be established with confidence. Treated
    /// as blocking, because assuming "same repository" is the one wrong guess that could
    /// lose a developer's work.
    UnverifiableRepository { origin: Option<String> },
    /// Occupied by something that is not a git repository.
    NotARepository,
    /// Occupied by a file, or by something that is not a directory at all.
    NotADirectory,
    /// The destination resolves outside the workspace -- via `..`, an absolute path, or a
    /// symlinked parent.
    OutsideWorkspace { resolved: PathBuf },
    /// The path could not be inspected, e.g. a dangling symlink or a permission error.
    Unreadable { reason: String },
    /// The URL is not one qeet will hand to git. Should be unreachable for a validated
    /// manifest; kept because URLs are treated as untrusted at every layer.
    UnusableUrl { reason: String },
}

impl fmt::Display for Blocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DifferentRepository { origin, expected } => write!(
                f,
                "a different repository is already here\n  found:    {origin}\n  expected: {expected}"
            ),
            Self::UnverifiableRepository { origin: Some(origin) } => write!(
                f,
                "a git repository is already here, but qeet cannot confirm it is the same one\n  found: {origin}"
            ),
            Self::UnverifiableRepository { origin: None } => {
                f.write_str("a git repository is already here, but it has no `origin` remote")
            }
            Self::NotARepository => {
                f.write_str("the directory is not empty and is not a git repository")
            }
            Self::NotADirectory => f.write_str("something that is not a directory is in the way"),
            Self::OutsideWorkspace { resolved } => {
                write!(f, "resolves outside the workspace, to {}", resolved.display())
            }
            Self::Unreadable { reason } => write!(f, "cannot be inspected: {reason}"),
            Self::UnusableUrl { reason } => write!(f, "the repository URL is not usable: {reason}"),
        }
    }
}

impl Blocked {
    /// The single most useful next step for this kind of block.
    pub fn guidance(&self) -> &'static str {
        match self {
            Self::DifferentRepository { .. } => {
                "Move or rename that directory, or point the manifest at the right repository."
            }
            Self::UnverifiableRepository { .. } => {
                "Check that repository's `origin`, then move it aside if it is not the one you want."
            }
            Self::NotARepository | Self::NotADirectory => {
                "Move or remove what is in the way, then run qeet again."
            }
            Self::OutsideWorkspace { .. } => "Fix the `path` for this repository in the manifest.",
            Self::Unreadable { .. } => {
                "Check the path's permissions and whether it is a broken symlink."
            }
            Self::UnusableUrl { .. } => "Fix the `url` for this repository in the manifest.",
        }
    }
}

/// One repository, resolved to a destination and a decision.
#[derive(Debug, Clone)]
pub struct Plan {
    pub name: String,
    pub url: String,
    pub git_ref: Option<String>,
    /// Absolute path git will be pointed at.
    pub destination: PathBuf,
    /// Destination relative to the workspace root, for display.
    pub display: String,
    pub state: State,
}

impl Workspace {
    /// The workspace, created first if it does not exist yet. For `clone`, which writes
    /// into it.
    pub fn create() -> io::Result<Self> {
        let location = Location::resolve()?;
        std::fs::create_dir_all(&location.path)
            .and_then(|()| Self::at(&location.path))
            .map_err(|err| naming(&location.path, err))
    }

    /// The workspace as it is, for the commands that act only on what is already there.
    ///
    /// A workspace that does not exist yet is not an error -- it is one with nothing cloned
    /// in it -- and looking at it must not create it.
    pub fn discover() -> io::Result<Self> {
        let location = Location::resolve()?;
        match Self::at(&location.path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self { root: location.path }),
            other => other.map_err(|err| naming(&location.path, err)),
        }
    }

    /// A workspace rooted at an explicit directory.
    ///
    /// The root is canonicalised so containment checks compare real paths -- on macOS,
    /// `/tmp` is a symlink to `/private/tmp`, and every check below would be wrong without
    /// this.
    ///
    /// `dunce::canonicalize` rather than [`Path::canonicalize`], and this is load-bearing on
    /// Windows: the standard library returns a verbatim path (`\\?\C:\...`), which git
    /// refuses with `could not create work tree dir ... Invalid argument` -- so every clone
    /// on Windows fails without it. It must also be the *same* function everywhere in this
    /// module, or `starts_with` would compare a simplified root against a verbatim path and
    /// judge every destination to be outside the workspace.
    pub fn at(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self { root: dunce::canonicalize(root)? })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Classify every repository of a product. Performs no writes.
    pub async fn plan<G: GitClient>(
        &self,
        manifest: &Manifest,
        product: &Product,
        protocol: Protocol,
        git: &G,
    ) -> Vec<Plan> {
        let mut plans = Vec::with_capacity(product.repositories.len());

        // Repositories are grouped under the product's directory when it has one, so
        // `qeet clone id` produces `qeet-id/qeet-id-server`. A `path` override is relative to
        // that same directory, not to the workspace root, so the grouping always holds.
        let group = product.group_dir();

        for entry in &product.repositories {
            let within = entry.path.clone().unwrap_or_else(|| entry.name.clone());
            let relative = match group {
                Some(dir) => format!("{dir}/{within}"),
                None => within,
            };
            let url = manifest.url_for(entry, protocol);

            let (destination, state) = match self.destination_for(Path::new(&relative)) {
                Ok(destination) => {
                    let state = match remote::validate(&url) {
                        Err(err) => {
                            State::Blocked(Blocked::UnusableUrl { reason: err.to_string() })
                        }
                        Ok(()) => self.inspect(&destination, &url, git).await,
                    };
                    (destination, state)
                }
                Err(blocked) => (self.root.join(&relative), State::Blocked(blocked)),
            };

            plans.push(Plan {
                name: entry.name.clone(),
                url,
                git_ref: entry.git_ref.clone(),
                display: self.relative_display(&destination),
                destination,
                state,
            });
        }

        plans
    }

    /// Resolve a relative destination, refusing anything that escapes the workspace.
    ///
    /// Two things are checked: the path is syntactically contained, and the deepest part of
    /// it that already exists still resolves inside the root. The second catches a
    /// symlinked parent directory, which the first cannot see.
    fn destination_for(&self, relative: &Path) -> Result<PathBuf, Blocked> {
        let candidate = self.root.join(relative);

        for component in relative.components() {
            match component {
                Component::Normal(_) | Component::CurDir => {}
                // `..`, a leading `/`, or a Windows prefix such as `C:` or `\\server\share`.
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(Blocked::OutsideWorkspace { resolved: candidate });
                }
            }
        }

        let (existing, tail) = split_at_existing(&candidate);
        let mut resolved = dunce::canonicalize(&existing)
            .map_err(|err| Blocked::Unreadable { reason: err.to_string() })?;
        resolved.extend(tail);

        // Component-wise, so a sibling named `qg-evil` is not mistaken for a child of `qg`.
        if !resolved.starts_with(&self.root) {
            return Err(Blocked::OutsideWorkspace { resolved });
        }

        Ok(candidate)
    }

    /// Classify an individual destination.
    async fn inspect<G: GitClient>(&self, destination: &Path, url: &str, git: &G) -> State {
        // `symlink_metadata` does not follow links, so a symlink is detected rather than
        // silently followed.
        match std::fs::symlink_metadata(destination) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return State::Create,
            Err(err) => {
                return State::Blocked(Blocked::Unreadable { reason: err.to_string() });
            }
            Ok(_) => {}
        }

        // Something is there. Resolve it fully: this is what makes a symlink pointing
        // outside the workspace, or a dangling one, impossible to write through.
        let real = match dunce::canonicalize(destination) {
            Ok(real) => real,
            Err(err) => {
                return State::Blocked(Blocked::Unreadable { reason: err.to_string() });
            }
        };
        if !real.starts_with(&self.root) {
            return State::Blocked(Blocked::OutsideWorkspace { resolved: real });
        }
        if !real.is_dir() {
            return State::Blocked(Blocked::NotADirectory);
        }

        match is_empty_dir(&real) {
            Err(err) => {
                return State::Blocked(Blocked::Unreadable { reason: err.to_string() });
            }
            // git clones happily into an existing empty directory.
            Ok(true) => return State::FillEmpty,
            Ok(false) => {}
        }

        if !git::is_git_repository(&real) {
            return State::Blocked(Blocked::NotARepository);
        }

        let origin = match git.origin_url(real).await {
            Ok(origin) => origin,
            // git itself misbehaving here is not evidence about the directory.
            Err(err) => {
                return State::Blocked(Blocked::Unreadable { reason: err.to_string() });
            }
        };

        match &origin {
            Some(found) => match remote::compare(url, found) {
                UrlMatch::Same => State::AlreadyPresent,
                UrlMatch::Different => State::Blocked(Blocked::DifferentRepository {
                    origin: found.clone(),
                    expected: url.to_string(),
                }),
                // Not a guess in either direction.
                UrlMatch::Indeterminate => {
                    State::Blocked(Blocked::UnverifiableRepository { origin })
                }
            },
            None => State::Blocked(Blocked::UnverifiableRepository { origin: None }),
        }
    }

    fn relative_display(&self, destination: &Path) -> String {
        destination.strip_prefix(&self.root).unwrap_or(destination).display().to_string()
    }
}

/// Split a path into its deepest existing ancestor and the components below it.
fn split_at_existing(candidate: &Path) -> (PathBuf, Vec<OsString>) {
    let mut tail = Vec::new();
    let mut current = candidate.to_path_buf();

    while !current.exists() {
        let Some(name) = current.file_name().map(OsString::from) else {
            break;
        };
        let Some(parent) = current.parent().map(Path::to_path_buf) else {
            break;
        };
        if parent.as_os_str().is_empty() {
            break;
        }
        tail.push(name);
        current = parent;
    }

    tail.reverse();
    (current, tail)
}

fn is_empty_dir(path: &Path) -> std::io::Result<bool> {
    Ok(std::fs::read_dir(path)?.next().is_none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{CloneRequest, Failure, GitError};
    use std::future::Future;

    /// A git client that only answers `origin_url`, from a fixed table. Preflight must not
    /// clone anything, so `clone_repo` refuses to be called.
    struct Origins(Vec<(PathBuf, String)>);

    // Mirrors the real client: the explicit `Send` bound is what `async fn` in a trait
    // cannot express, and is why qeet needs no `async_trait` dependency.
    #[allow(clippy::manual_async_fn)]
    impl GitClient for Origins {
        fn clone_repo(
            &self,
            _request: CloneRequest,
        ) -> impl Future<Output = Result<(), Failure>> + Send {
            async { panic!("preflight must not clone") }
        }

        fn origin_url(
            &self,
            repository: PathBuf,
        ) -> impl Future<Output = Result<Option<String>, GitError>> + Send {
            let found =
                self.0.iter().find(|(path, _)| *path == repository).map(|(_, url)| url.clone());
            async move { Ok(found) }
        }

        fn inspect(
            &self,
            _repository: PathBuf,
        ) -> impl Future<Output = Result<crate::git::RepoState, GitError>> + Send {
            async { panic!("preflight must not inspect repositories") }
        }

        fn fetch(&self, _repository: PathBuf) -> impl Future<Output = Result<(), Failure>> + Send {
            async { panic!("preflight must not fetch") }
        }

        fn fast_forward(
            &self,
            _repository: PathBuf,
        ) -> impl Future<Output = Result<(), Failure>> + Send {
            async { panic!("preflight must not fast-forward") }
        }
    }

    fn manifest_with(repositories: &str) -> Manifest {
        let text = format!(
            r#"
schema = 1
[remote]
host = "github.com"
owner = "qeetgroup"
protocol = "ssh"
[products.id]
name = "Qeet ID"
repositories = [{repositories}]
"#
        );
        Manifest::load(&text, "test").expect("fixture must be valid")
    }

    async fn plan_one(root: &Path, repositories: &str, origins: Origins) -> Plan {
        let manifest = manifest_with(repositories);
        let workspace = Workspace::at(root).expect("workspace");
        let product = &manifest.products["id"];
        let mut plans = workspace.plan(&manifest, product, Protocol::Ssh, &origins).await;
        assert_eq!(plans.len(), 1, "expected exactly one plan");
        plans.remove(0)
    }

    /// Case A: nothing there.
    #[tokio::test]
    async fn a_missing_destination_is_created() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plan = plan_one(dir.path(), r#"{ name = "repo" }"#, Origins(vec![])).await;

        assert_eq!(plan.state, State::Create);
        assert!(plan.state.owns_destination(), "qeet created it, so qeet may clean it up");
        assert_eq!(plan.display, "repo");
        assert_eq!(plan.url, "git@github.com:qeetgroup/repo.git");
    }

    /// Case B: exists and empty -- git accepts this.
    #[tokio::test]
    async fn an_empty_destination_is_filled_but_not_owned() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("repo")).expect("create");

        let plan = plan_one(dir.path(), r#"{ name = "repo" }"#, Origins(vec![])).await;
        assert_eq!(plan.state, State::FillEmpty);
        assert!(!plan.state.owns_destination(), "a pre-existing directory is not ours to remove");
    }

    /// Case C: already cloned, matching origin -- the idempotency case.
    #[tokio::test]
    async fn a_matching_repository_is_already_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).expect("create");
        let real = dunce::canonicalize(&repo).expect("canonicalize");

        // Recorded over HTTPS, requested over SSH: the same repository.
        let plan = plan_one(
            dir.path(),
            r#"{ name = "repo" }"#,
            Origins(vec![(real, "https://github.com/qeetgroup/repo.git".into())]),
        )
        .await;

        assert_eq!(
            plan.state,
            State::AlreadyPresent,
            "a matching repository must not be re-cloned"
        );
    }

    /// Case C': a repository, but the wrong one.
    #[tokio::test]
    async fn a_different_repository_blocks_and_names_both_sides() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).expect("create");
        let real = dunce::canonicalize(&repo).expect("canonicalize");

        let plan = plan_one(
            dir.path(),
            r#"{ name = "repo" }"#,
            Origins(vec![(real, "git@github.com:someone/else.git".into())]),
        )
        .await;

        let State::Blocked(Blocked::DifferentRepository { origin, expected }) = &plan.state else {
            panic!("expected DifferentRepository, got {:?}", plan.state);
        };
        assert_eq!(origin, "git@github.com:someone/else.git");
        assert_eq!(expected, "git@github.com:qeetgroup/repo.git");
    }

    #[tokio::test]
    async fn a_repository_with_no_origin_is_not_assumed_to_match() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("repo/.git")).expect("create");

        let plan = plan_one(dir.path(), r#"{ name = "repo" }"#, Origins(vec![])).await;
        assert_eq!(plan.state, State::Blocked(Blocked::UnverifiableRepository { origin: None }));
    }

    #[tokio::test]
    async fn an_unparseable_origin_blocks_rather_than_guessing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).expect("create");
        let real = dunce::canonicalize(&repo).expect("canonicalize");

        let plan =
            plan_one(dir.path(), r#"{ name = "repo" }"#, Origins(vec![(real, "not-a-url".into())]))
                .await;

        assert!(
            matches!(
                plan.state,
                State::Blocked(Blocked::UnverifiableRepository { origin: Some(_) })
            ),
            "{:?}",
            plan.state
        );
    }

    /// Case D: occupied by something that is not a repository.
    #[tokio::test]
    async fn a_non_repository_directory_blocks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).expect("create");
        std::fs::write(repo.join("notes.txt"), "mine").expect("write");

        let plan = plan_one(dir.path(), r#"{ name = "repo" }"#, Origins(vec![])).await;
        assert_eq!(plan.state, State::Blocked(Blocked::NotARepository));
    }

    #[tokio::test]
    async fn a_file_in_the_way_blocks() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("repo"), "in the way").expect("write");

        let plan = plan_one(dir.path(), r#"{ name = "repo" }"#, Origins(vec![])).await;
        assert_eq!(plan.state, State::Blocked(Blocked::NotADirectory));
    }

    #[tokio::test]
    async fn a_custom_path_is_honoured() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plan =
            plan_one(dir.path(), r#"{ name = "repo", path = "services/api" }"#, Origins(vec![]))
                .await;

        assert_eq!(plan.state, State::Create);
        assert_eq!(plan.destination, dunce::canonicalize(dir.path()).unwrap().join("services/api"));
    }

    #[tokio::test]
    async fn a_sibling_with_a_shared_prefix_is_not_treated_as_inside() {
        // The bug a string `starts_with` would have: /root/qg vs /root/qg-evil.
        let parent = tempfile::tempdir().expect("tempdir");
        let root = parent.path().join("qg");
        std::fs::create_dir(&root).expect("create");
        std::fs::create_dir(parent.path().join("qg-evil")).expect("create");

        let workspace = Workspace::at(&root).expect("workspace");
        let err = workspace
            .destination_for(Path::new("../qg-evil/repo"))
            .expect_err("must refuse to escape");
        assert!(matches!(err, Blocked::OutsideWorkspace { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn a_symlinked_parent_pointing_outside_is_refused() {
        let parent = tempfile::tempdir().expect("tempdir");
        let root = parent.path().join("workspace");
        let outside = parent.path().join("outside");
        std::fs::create_dir(&root).expect("create");
        std::fs::create_dir(&outside).expect("create");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("escape")).expect("symlink");
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(&outside, root.join("escape")).is_err() {
            return; // Windows needs privileges for this; skip rather than fail.
        }

        let workspace = Workspace::at(&root).expect("workspace");
        let err = workspace
            .destination_for(Path::new("escape/repo"))
            .expect_err("must refuse to write through the symlink");
        assert!(matches!(err, Blocked::OutsideWorkspace { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn a_symlinked_destination_pointing_outside_is_refused() {
        let parent = tempfile::tempdir().expect("tempdir");
        let root = parent.path().join("workspace");
        let outside = parent.path().join("outside");
        std::fs::create_dir(&root).expect("create");
        std::fs::create_dir(&outside).expect("create");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("repo")).expect("symlink");
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(&outside, root.join("repo")).is_err() {
            return;
        }

        let plan = plan_one(&root, r#"{ name = "repo" }"#, Origins(vec![])).await;
        assert!(
            matches!(plan.state, State::Blocked(Blocked::OutsideWorkspace { .. })),
            "{:?}",
            plan.state
        );
    }

    #[tokio::test]
    async fn a_dangling_symlink_is_reported_not_written_through() {
        let dir = tempfile::tempdir().expect("tempdir");

        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("nowhere"), dir.path().join("repo"))
            .expect("symlink");
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(dir.path().join("nowhere"), dir.path().join("repo"))
            .is_err()
        {
            return;
        }

        let plan = plan_one(dir.path(), r#"{ name = "repo" }"#, Origins(vec![])).await;
        assert!(
            matches!(plan.state, State::Blocked(Blocked::Unreadable { .. })),
            "{:?}",
            plan.state
        );
    }

    /// Regression: `std::fs::canonicalize` yields a Windows verbatim path (`\\?\C:\...`)
    /// that git refuses outright, which made every clone on Windows fail. The path handed to
    /// git must never carry that prefix.
    #[test]
    fn no_path_uses_the_windows_verbatim_prefix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = Workspace::at(dir.path()).expect("workspace");

        let root = workspace.root().to_string_lossy().to_string();
        assert!(
            !root.starts_with("\\\\?\\"),
            "the workspace root must be in a form git accepts, got {root}"
        );

        let destination =
            workspace.destination_for(Path::new("qeet-id-server")).expect("should resolve");
        let destination = destination.to_string_lossy().to_string();
        assert!(
            !destination.starts_with("\\\\?\\"),
            "the destination handed to git must be in a form git accepts, got {destination}"
        );
    }

    /// `qeet clone id` groups under the product's directory.
    #[tokio::test]
    async fn repositories_are_grouped_under_the_product_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let text = r#"
schema = 1
[remote]
host = "github.com"
owner = "qeetgroup"
protocol = "ssh"
[products.id]
name = "Qeet ID"
directory = "qeet-id"
repositories = [{ name = "qeet-id-server" }, { name = "qeet-id-console" }]
"#;
        let manifest = Manifest::load(text, "test").expect("fixture must be valid");
        let workspace = Workspace::at(dir.path()).expect("workspace");
        let plans = workspace
            .plan(&manifest, &manifest.products["id"], Protocol::Ssh, &Origins(vec![]))
            .await;

        let displays: Vec<&str> = plans.iter().map(|p| p.display.as_str()).collect();
        assert_eq!(displays, ["qeet-id/qeet-id-server", "qeet-id/qeet-id-console"]);
        for plan in &plans {
            assert_eq!(
                plan.destination.parent(),
                Some(workspace.root().join("qeet-id").as_path()),
                "every repository sits inside the group directory"
            );
            assert_eq!(plan.state, State::Create);
        }
    }

    /// A product with no `directory` stays flat -- which is how the organization-level
    /// repositories (qeet-docs, qeet-apis) are meant to land.
    #[tokio::test]
    async fn a_product_without_a_directory_stays_flat() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plan = plan_one(dir.path(), r#"{ name = "qeet-docs" }"#, Origins(vec![])).await;
        assert_eq!(plan.display, "qeet-docs");

        // Compared against the root the workspace itself computed, not a second
        // canonicalisation of the same directory. `std::fs::canonicalize` returns a Windows
        // verbatim path (\\?\C:\...) while production uses `dunce::canonicalize`, so
        // canonicalising independently here compares two different spellings of one path --
        // which is exactly how this test failed on Windows and nowhere else.
        let workspace = Workspace::at(dir.path()).expect("workspace");
        assert_eq!(plan.destination.parent(), Some(workspace.root()));
    }

    /// A `path` override is relative to the group directory, so grouping always holds.
    #[tokio::test]
    async fn a_path_override_is_relative_to_the_group_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let text = r#"
schema = 1
[remote]
host = "github.com"
owner = "qeetgroup"
protocol = "ssh"
[products.id]
name = "Qeet ID"
directory = "qeet-id"
repositories = [{ name = "qeet-id-server", path = "services/api" }]
"#;
        let manifest = Manifest::load(text, "test").expect("fixture must be valid");
        let workspace = Workspace::at(dir.path()).expect("workspace");
        let plans = workspace
            .plan(&manifest, &manifest.products["id"], Protocol::Ssh, &Origins(vec![]))
            .await;

        assert_eq!(plans[0].display, "qeet-id/services/api");
        assert_eq!(plans[0].destination, workspace.root().join("qeet-id/services/api"));
    }

    /// A group directory that tries to escape is refused, like any other path.
    #[tokio::test]
    async fn an_escaping_group_directory_is_refused() {
        let text = r#"
schema = 1
[remote]
host = "github.com"
owner = "qeetgroup"
protocol = "ssh"
[products.id]
name = "Qeet ID"
directory = "../outside"
repositories = [{ name = "repo" }]
"#;
        let err = Manifest::load(text, "test").expect_err("must be refused");
        assert!(err.to_string().contains("contains `..`"), "{err}");
    }

    #[tokio::test]
    async fn the_flat_layout_puts_repositories_directly_in_the_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manifest =
            manifest_with(r#"{ name = "qeet-id-server" }, { name = "qeet-id-console" }"#);
        let workspace = Workspace::at(dir.path()).expect("workspace");

        let plans = workspace
            .plan(&manifest, &manifest.products["id"], Protocol::Ssh, &Origins(vec![]))
            .await;

        let displays: Vec<&str> = plans.iter().map(|plan| plan.display.as_str()).collect();
        assert_eq!(displays, ["qeet-id-server", "qeet-id-console"]);
        for plan in &plans {
            assert_eq!(plan.destination.parent(), Some(workspace.root()));
        }
    }

    // -- where the workspace is --------------------------------------------------------

    /// An absolute path on this platform: Windows needs a drive for `/srv` to be absolute.
    fn abs(path: &str) -> PathBuf {
        PathBuf::from(if cfg!(windows) { format!("C:{path}") } else { path.to_string() })
    }

    fn home() -> Option<PathBuf> {
        Some(abs("/home/dev"))
    }

    fn unused_cwd() -> io::Result<PathBuf> {
        panic!("an absolute or default location must not consult the current directory")
    }

    #[test]
    fn defaults_to_qg_in_the_home_directory_wherever_qeet_runs() {
        let location = locate(None, home(), unused_cwd).expect("should resolve");
        assert_eq!(location, Location { path: abs("/home/dev/QG"), from_env: false });
    }

    #[test]
    fn the_environment_variable_moves_it() {
        let location = locate(Some(abs("/srv/code").into()), home(), unused_cwd).expect("resolve");
        assert_eq!(location, Location { path: abs("/srv/code"), from_env: true });
    }

    #[test]
    fn an_empty_variable_is_treated_as_unset() {
        let location = locate(Some("".into()), home(), unused_cwd).expect("should resolve");
        assert_eq!(location.path, abs("/home/dev/QG"));
        assert!(!location.from_env);
    }

    /// No shell expanded these, so qeet must: a quoted value, or any value on Windows.
    #[test]
    fn a_leading_tilde_means_the_home_directory() {
        for (value, expected) in [("~", "/home/dev"), ("~/code/qg", "/home/dev/code/qg")] {
            let location = locate(Some(value.into()), home(), unused_cwd).expect("resolve");
            assert_eq!(location.path, abs(expected), "{value}");
        }
        // `~user` is somebody else's home, which qeet does not guess at.
        let location =
            locate(Some("~bob/qg".into()), home(), || Ok(PathBuf::from("/cwd"))).expect("resolve");
        assert_eq!(location.path, PathBuf::from("/cwd/~bob/qg"));
    }

    #[test]
    fn a_relative_variable_is_resolved_against_the_current_directory() {
        let location =
            locate(Some("work".into()), home(), || Ok(PathBuf::from("/cwd"))).expect("resolve");
        assert_eq!(location.path, PathBuf::from("/cwd/work"));
    }

    #[test]
    fn no_home_directory_is_an_error_unless_the_variable_is_set() {
        let err = locate(None, None, unused_cwd).expect_err("nowhere to put it");
        assert!(err.to_string().contains("home directory"), "{err}");

        let location = locate(Some(abs("/srv/code").into()), None, unused_cwd).expect("resolve");
        assert_eq!(location.path, abs("/srv/code"));
    }
}
