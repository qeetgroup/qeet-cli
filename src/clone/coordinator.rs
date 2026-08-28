//! Bounded concurrent cloning.
//!
//! The whole point of `qeet clone`: instead of eleven sequential `git clone` commands, run
//! several at once -- but never an unbounded number, because 66 simultaneous git processes
//! would exhaust file descriptors and invite rate limiting from the remote.
//!
//! Repositories are independent. One failing never cancels another, and a `Ctrl-C` stops
//! everything without orphaning a git process or destroying a finished clone.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::report::{Outcome, Report, RepositoryReport};
use crate::git::{CloneRequest, GitClient};
use crate::output::Renderer;
use crate::workspace::{Blocked, Plan, State};

/// Upper bound on the default concurrency. Cloning is network-bound, so more processes
/// than this buys little and costs file descriptors and goodwill with the remote.
const MAX_DEFAULT_CONCURRENCY: usize = 8;

/// Retries are for transient transport failures only, so the ceiling stays low.
pub const DEFAULT_MAX_RETRIES: u32 = 2;

/// How the coordinator should behave.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub concurrency: NonZeroUsize,
    pub max_retries: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self { concurrency: default_concurrency(), max_retries: DEFAULT_MAX_RETRIES }
    }
}

/// One product's worth of work.
#[derive(Debug, Clone)]
pub struct Job {
    pub product_name: String,
    /// In manifest order. Blocked and already-present entries are included; they are
    /// reported without invoking git.
    pub plans: Vec<Plan>,
    /// Workspace root, re-checked before any cleanup removes anything.
    pub root: std::path::PathBuf,
}

/// Default concurrency: available parallelism, clamped to a sane range.
pub fn default_concurrency() -> NonZeroUsize {
    let available = std::thread::available_parallelism().map(NonZeroUsize::get).unwrap_or(4);
    NonZeroUsize::new(available.clamp(1, MAX_DEFAULT_CONCURRENCY)).expect("clamped to at least 1")
}

/// Clone every repository in the job, concurrently and bounded.
///
/// Always returns a report with exactly one entry per plan, whatever went wrong.
pub async fn run<G: GitClient>(
    git: Arc<G>,
    job: Job,
    options: Options,
    renderer: Arc<dyn Renderer>,
    manifest_note: Option<&str>,
) -> Report {
    let started = Instant::now();

    let names: Vec<String> = job.plans.iter().map(|plan| plan.name.clone()).collect();
    renderer.begin(&job.product_name, manifest_note, &names);

    // Entries that need no git process are settled up front, so the concurrency budget is
    // spent only on real work.
    let mut settled: HashMap<usize, RepositoryReport> = HashMap::new();
    let mut queued: Vec<usize> = Vec::new();

    for (index, plan) in job.plans.iter().enumerate() {
        let outcome = match &plan.state {
            State::AlreadyPresent => Outcome::AlreadyPresent,
            State::Blocked(blocked) => Outcome::Blocked(blocked.clone()),
            State::Create | State::FillEmpty => {
                queued.push(index);
                continue;
            }
        };
        let entry = immediate(plan, outcome);
        renderer.repository_finished(&entry);
        settled.insert(index, entry);
    }

    let semaphore = Arc::new(Semaphore::new(options.concurrency.get()));
    let mut tasks = JoinSet::new();

    for index in queued.iter().copied() {
        let plan = job.plans[index].clone();
        let git = Arc::clone(&git);
        let semaphore = Arc::clone(&semaphore);
        let renderer = Arc::clone(&renderer);
        let root = job.root.clone();
        let max_retries = options.max_retries;

        tasks.spawn(async move {
            // The permit is the concurrency bound. Held for the whole clone, including
            // retries and backoff, so a retrying repository does not let an extra one in.
            let _permit = semaphore.acquire().await.expect("the semaphore is never closed");

            renderer.repository_started(&plan.name);
            let entry = clone_one(git.as_ref(), &plan, &root, max_retries, renderer.as_ref()).await;
            (index, entry)
        });
    }

    // Ctrl-C. If the handler cannot be registered, this future never resolves rather than
    // resolving with an error and cancelling the run for no reason.
    let mut interrupted = Box::pin(async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    });
    let mut cancelled = false;

    while !tasks.is_empty() {
        tokio::select! {
            joined = tasks.join_next() => {
                match joined {
                    Some(Ok((index, entry))) => {
                        renderer.repository_finished(&entry);
                        settled.insert(index, entry);
                    }
                    // Aborted by cancellation, or the task panicked. Either way the
                    // repository is accounted for below rather than silently dropped.
                    Some(Err(_)) => {}
                    None => break,
                }
            }
            () = &mut interrupted, if !cancelled => {
                cancelled = true;
                renderer.cancelling();
                // Aborting drops each task, which drops its `Child`. Because the git
                // command was configured with `kill_on_drop`, the git processes are
                // killed rather than left running. Queued tasks never start.
                tasks.abort_all();
            }
        }
    }

    // Whatever never settled was cancelled: either aborted mid-clone or never started.
    for index in queued {
        if settled.contains_key(&index) {
            continue;
        }
        let plan = &job.plans[index];
        // An aborted clone had no chance to clean up after itself.
        remove_partial_clone(plan, &job.root);
        let entry = immediate(plan, Outcome::Cancelled);
        renderer.repository_finished(&entry);
        settled.insert(index, entry);
    }

    let repositories = (0..job.plans.len())
        .map(|index| settled.remove(&index).expect("every plan is settled exactly once"))
        .collect();

    Report { product_name: job.product_name, repositories, cancelled, elapsed: started.elapsed() }
}

/// Clone one repository, retrying only what is worth retrying.
async fn clone_one<G: GitClient>(
    git: &G,
    plan: &Plan,
    root: &Path,
    max_retries: u32,
    renderer: &dyn Renderer,
) -> RepositoryReport {
    let started = Instant::now();

    // Only the leading directories -- git creates the destination itself, which keeps
    // "who created this directory" unambiguous for cleanup.
    if let Some(parent) = plan.destination.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            return RepositoryReport {
                name: plan.name.clone(),
                display: plan.display.clone(),
                outcome: Outcome::Blocked(Blocked::Unreadable {
                    reason: format!("cannot create the parent directory: {err}"),
                }),
                duration: started.elapsed(),
                attempts: 0,
            };
        }
    }

    let request = CloneRequest {
        name: plan.name.clone(),
        url: plan.url.clone(),
        destination: plan.destination.clone(),
        git_ref: plan.git_ref.clone(),
    };

    let mut attempts = 0;
    let outcome = loop {
        attempts += 1;
        match git.clone_repo(request.clone()).await {
            Ok(()) => break Outcome::Cloned,
            Err(failure) => {
                if !failure.retryable() || attempts > max_retries {
                    remove_partial_clone(plan, root);
                    break Outcome::Failed(failure);
                }
                renderer.repository_retrying(&plan.name, attempts, failure.summary());
                tokio::time::sleep(backoff(attempts)).await;
            }
        }
    };

    RepositoryReport {
        name: plan.name.clone(),
        display: plan.display.clone(),
        outcome,
        duration: started.elapsed(),
        attempts,
    }
}

fn immediate(plan: &Plan, outcome: Outcome) -> RepositoryReport {
    RepositoryReport {
        name: plan.name.clone(),
        display: plan.display.clone(),
        outcome,
        duration: Duration::ZERO,
        attempts: 0,
    }
}

/// Remove a destination this run created and did not finish.
///
/// Deliberately narrow. Ownership is established by preflight -- the destination did not
/// exist when the run started, and `State::Create` records that -- not by inspecting the
/// contents afterwards, because git creates `.git` early enough that a half-finished clone
/// looks like a repository. A directory that already existed is never removed, nor is a
/// sibling, nor a parent directory this run created on the way down.
fn remove_partial_clone(plan: &Plan, root: &Path) {
    if !plan.state.owns_destination() {
        return;
    }
    // Re-assert containment immediately before a destructive call, independently of the
    // preflight that already checked it.
    if !plan.destination.starts_with(root) || plan.destination == root {
        return;
    }
    if !plan.destination.exists() {
        return;
    }
    let _ = std::fs::remove_dir_all(&plan.destination);
}

/// Backoff before retrying a transient failure: roughly 500ms then 1500ms, jittered.
///
/// The jitter comes from the clock rather than a random-number dependency; it only needs to
/// stop several repositories retrying in lockstep.
fn backoff(attempt: u32) -> Duration {
    let base = 500u64.saturating_mul(3u64.saturating_pow(attempt.saturating_sub(1)));
    let spread = base / 5;
    if spread == 0 {
        return Duration::from_millis(base);
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| u64::from(elapsed.subsec_nanos()));
    let offset = nanos % (spread * 2 + 1);

    Duration::from_millis(base + offset - spread)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{Failure, FailureKind, GitError};
    use crate::output::Silent;
    use crate::workspace::Workspace;
    use std::collections::HashSet;
    use std::future::Future;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Barrier;

    /// How the fake should respond to a given repository on a given attempt.
    type Behaviour = Box<dyn Fn(&str, u32) -> Result<(), Failure> + Send + Sync>;

    /// A `GitClient` that never touches the network or the filesystem, and records how it
    /// was called.
    struct FakeGit {
        behaviour: Behaviour,
        attempts: Mutex<HashMap<String, u32>>,
        in_flight: AtomicUsize,
        peak_in_flight: AtomicUsize,
        /// When set, every clone blocks here. Sized to the expected concurrency, so it can
        /// only release if that many clones really do run at the same time.
        barrier: Option<Arc<Barrier>>,
    }

    impl FakeGit {
        fn new(behaviour: Behaviour) -> Arc<Self> {
            Arc::new(Self {
                behaviour,
                attempts: Mutex::new(HashMap::new()),
                in_flight: AtomicUsize::new(0),
                peak_in_flight: AtomicUsize::new(0),
                barrier: None,
            })
        }

        fn always_ok() -> Arc<Self> {
            Self::new(Box::new(|_, _| Ok(())))
        }

        fn with_barrier(size: usize) -> Arc<Self> {
            Arc::new(Self {
                behaviour: Box::new(|_, _| Ok(())),
                attempts: Mutex::new(HashMap::new()),
                in_flight: AtomicUsize::new(0),
                peak_in_flight: AtomicUsize::new(0),
                barrier: Some(Arc::new(Barrier::new(size))),
            })
        }

        fn peak(&self) -> usize {
            self.peak_in_flight.load(Ordering::SeqCst)
        }

        fn attempts_for(&self, name: &str) -> u32 {
            self.attempts.lock().expect("lock").get(name).copied().unwrap_or(0)
        }
    }

    // Mirrors the real client: the explicit `Send` bound is what `async fn` in a trait
    // cannot express, and is why qeet needs no `async_trait` dependency.
    #[allow(clippy::manual_async_fn)]
    impl GitClient for FakeGit {
        fn clone_repo(
            &self,
            request: CloneRequest,
        ) -> impl Future<Output = Result<(), Failure>> + Send {
            let attempt = {
                let mut attempts = self.attempts.lock().expect("lock");
                let counter = attempts.entry(request.name.clone()).or_insert(0);
                *counter += 1;
                *counter
            };

            let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak_in_flight.fetch_max(current, Ordering::SeqCst);

            async move {
                if let Some(barrier) = &self.barrier {
                    barrier.wait().await;
                }
                let result = (self.behaviour)(&request.name, attempt);
                self.in_flight.fetch_sub(1, Ordering::SeqCst);
                result
            }
        }

        fn origin_url(
            &self,
            _repository: PathBuf,
        ) -> impl Future<Output = Result<Option<String>, GitError>> + Send {
            async { Ok(None) }
        }
    }

    fn transient() -> Failure {
        Failure {
            kind: FailureKind::Transient,
            exit_code: Some(128),
            git_stderr: "fatal: early EOF".into(),
        }
    }

    fn auth() -> Failure {
        Failure {
            kind: FailureKind::Auth,
            exit_code: Some(128),
            git_stderr: "Permission denied (publickey)".into(),
        }
    }

    /// Build a job of `count` creatable repositories rooted in a temporary directory.
    fn job(root: &Path, count: usize) -> Job {
        let plans = (0..count)
            .map(|index| {
                let name = format!("repo{index}");
                Plan {
                    url: format!("git@github.com:qeetgroup/{name}.git"),
                    git_ref: None,
                    destination: root.join(&name),
                    display: name.clone(),
                    name,
                    state: State::Create,
                }
            })
            .collect();

        Job { product_name: "Qeet ID".into(), plans, root: root.to_path_buf() }
    }

    fn options(concurrency: usize, max_retries: u32) -> Options {
        Options { concurrency: NonZeroUsize::new(concurrency).expect("non-zero"), max_retries }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn clones_really_run_concurrently_up_to_the_limit() {
        // The barrier is the proof. Sized to the concurrency limit, it can only release if
        // that many clones are genuinely in flight at the same moment -- so a sequential
        // implementation cannot pass this test, it stalls. The timeout turns that stall
        // into a clear failure instead of a hung CI job.
        const LIMIT: usize = 4;
        let dir = tempfile::tempdir().expect("tempdir");
        let git = FakeGit::with_barrier(LIMIT);

        let report = tokio::time::timeout(
            Duration::from_secs(10),
            run(Arc::clone(&git), job(dir.path(), 8), options(LIMIT, 0), Arc::new(Silent), None),
        )
        .await
        .expect("concurrency is not genuine: fewer than the limit ran at once");

        assert_eq!(report.cloned(), 8);
        assert_eq!(git.peak(), LIMIT, "peak concurrency should reach the limit");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_concurrency_limit_is_never_exceeded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let git = FakeGit::with_barrier(2);

        let report = tokio::time::timeout(
            Duration::from_secs(10),
            run(Arc::clone(&git), job(dir.path(), 10), options(2, 0), Arc::new(Silent), None),
        )
        .await
        .expect("should not stall");

        assert_eq!(report.cloned(), 10);
        assert_eq!(git.peak(), 2, "never more than the limit at once");
    }

    #[tokio::test]
    async fn a_concurrency_of_one_serialises() {
        let dir = tempfile::tempdir().expect("tempdir");
        let git = FakeGit::always_ok();

        let report =
            run(Arc::clone(&git), job(dir.path(), 5), options(1, 0), Arc::new(Silent), None).await;

        assert_eq!(report.cloned(), 5);
        assert_eq!(git.peak(), 1, "one at a time");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn one_failure_does_not_cancel_the_others() {
        // The brief's example: repositories 3 and 6 fail, the rest must still finish.
        let dir = tempfile::tempdir().expect("tempdir");
        let git = FakeGit::new(Box::new(|name, _| {
            if name == "repo2" || name == "repo5" { Err(auth()) } else { Ok(()) }
        }));

        let report = run(git, job(dir.path(), 6), options(3, 0), Arc::new(Silent), None).await;

        assert_eq!(report.total(), 6, "every repository is accounted for");
        assert_eq!(report.succeeded(), 4);
        assert_eq!(report.failed(), 2);
        assert!(!report.is_complete(), "a partial failure is not a success");

        let failed: Vec<&str> = report.problems().map(|entry| entry.name.as_str()).collect();
        assert_eq!(failed, ["repo2", "repo5"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_transient_failure_is_retried_until_it_succeeds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let git = FakeGit::new(Box::new(
            |_, attempt| {
                if attempt < 3 { Err(transient()) } else { Ok(()) }
            },
        ));

        let report = run(
            Arc::clone(&git),
            job(dir.path(), 1),
            options(1, DEFAULT_MAX_RETRIES),
            Arc::new(Silent),
            None,
        )
        .await;

        assert_eq!(report.cloned(), 1);
        assert_eq!(git.attempts_for("repo0"), 3, "one attempt plus two retries");
        assert_eq!(report.repositories[0].attempts, 3);
    }

    #[tokio::test(start_paused = true)]
    async fn retries_stop_at_the_ceiling() {
        let dir = tempfile::tempdir().expect("tempdir");
        let git = FakeGit::new(Box::new(|_, _| Err(transient())));

        let report = run(
            Arc::clone(&git),
            job(dir.path(), 1),
            options(1, DEFAULT_MAX_RETRIES),
            Arc::new(Silent),
            None,
        )
        .await;

        assert_eq!(report.failed(), 1);
        assert_eq!(git.attempts_for("repo0"), 3, "must not retry forever");
    }

    #[tokio::test]
    async fn failures_that_cannot_be_fixed_by_retrying_are_not_retried() {
        let dir = tempfile::tempdir().expect("tempdir");
        let git = FakeGit::new(Box::new(|_, _| Err(auth())));

        let report = run(
            Arc::clone(&git),
            job(dir.path(), 1),
            options(1, DEFAULT_MAX_RETRIES),
            Arc::new(Silent),
            None,
        )
        .await;

        assert_eq!(report.failed(), 1);
        assert_eq!(git.attempts_for("repo0"), 1, "an auth failure is attempted once");
    }

    #[tokio::test]
    async fn blocked_and_present_repositories_never_reach_git() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Any call to clone_repo would panic.
        let git = FakeGit::new(Box::new(|name, _| panic!("git must not be invoked for {name}")));

        let mut job = job(dir.path(), 2);
        job.plans[0].state = State::AlreadyPresent;
        job.plans[1].state = State::Blocked(Blocked::NotARepository);

        let report = run(git, job, options(2, 0), Arc::new(Silent), None).await;

        assert_eq!(report.already_present(), 1);
        assert_eq!(report.failed(), 1);
        assert_eq!(report.cloned(), 0);
        // An already-present repository is a success, a blocked one is not.
        assert!(!report.is_complete());
    }

    #[tokio::test]
    async fn an_all_present_run_is_complete() {
        let dir = tempfile::tempdir().expect("tempdir");
        let git = FakeGit::new(Box::new(|name, _| panic!("git must not be invoked for {name}")));

        let mut job = job(dir.path(), 3);
        for plan in &mut job.plans {
            plan.state = State::AlreadyPresent;
        }

        let report = run(git, job, options(2, 0), Arc::new(Silent), None).await;
        assert!(report.is_complete(), "re-running a finished clone must succeed");
        assert_eq!(report.already_present(), 3);
    }

    #[tokio::test]
    async fn every_plan_appears_exactly_once_and_in_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let git =
            FakeGit::new(Box::new(|name, _| if name == "repo1" { Err(auth()) } else { Ok(()) }));

        let mut job = job(dir.path(), 4);
        job.plans[3].state = State::AlreadyPresent;

        let report = run(git, job, options(4, 0), Arc::new(Silent), None).await;

        let names: Vec<&str> = report.repositories.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["repo0", "repo1", "repo2", "repo3"]);
        let unique: HashSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), names.len());
    }

    #[tokio::test]
    async fn a_partial_clone_this_run_created_is_cleaned_up() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = Workspace::at(dir.path()).expect("workspace");
        let root = workspace.root().to_path_buf();

        // Stand in for git having created the destination and then failed: git creates
        // `.git` early, so the leftover looks like a repository.
        let leftover = root.join("repo0");
        std::fs::create_dir_all(leftover.join(".git")).expect("create");

        let git = FakeGit::new(Box::new(|_, _| Err(auth())));
        let mut job = job(&root, 1);
        job.root = root.clone();

        let report = run(git, job, options(1, 0), Arc::new(Silent), None).await;

        assert_eq!(report.failed(), 1);
        assert!(!leftover.exists(), "qeet created it and did not finish it, so it goes");
    }

    #[tokio::test]
    async fn a_directory_that_already_existed_is_never_removed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = Workspace::at(dir.path()).expect("workspace");
        let root = workspace.root().to_path_buf();

        let preexisting = root.join("repo0");
        std::fs::create_dir(&preexisting).expect("create");

        let git = FakeGit::new(Box::new(|_, _| Err(auth())));
        let mut job = job(&root, 1);
        job.root = root.clone();
        // FillEmpty: it was there before this run, so it is not ours to delete.
        job.plans[0].state = State::FillEmpty;

        let report = run(git, job, options(1, 0), Arc::new(Silent), None).await;

        assert_eq!(report.failed(), 1);
        assert!(preexisting.exists(), "a pre-existing directory must survive a failure");
    }

    #[tokio::test]
    async fn a_successful_sibling_survives_another_repositorys_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = Workspace::at(dir.path()).expect("workspace");
        let root = workspace.root().to_path_buf();

        // repo0 "succeeds" and leaves a directory behind; repo1 fails.
        let survivor = root.join("repo0");
        std::fs::create_dir_all(survivor.join(".git")).expect("create");

        let git =
            FakeGit::new(Box::new(|name, _| if name == "repo1" { Err(auth()) } else { Ok(()) }));
        let mut job = job(&root, 2);
        job.root = root.clone();

        let report = run(git, job, options(2, 0), Arc::new(Silent), None).await;

        assert_eq!(report.succeeded(), 1);
        assert_eq!(report.failed(), 1);
        assert!(survivor.exists(), "cleanup must not touch a repository that succeeded");
    }

    #[test]
    fn the_default_concurrency_is_bounded() {
        let concurrency = default_concurrency().get();
        assert!(
            (1..=MAX_DEFAULT_CONCURRENCY).contains(&concurrency),
            "{concurrency} is outside 1..={MAX_DEFAULT_CONCURRENCY}"
        );
    }

    #[test]
    fn backoff_grows_and_stays_within_its_jitter_band() {
        // ~500ms then ~1500ms, each within +/-20%.
        for (attempt, base) in [(1u32, 500u64), (2, 1500)] {
            let waited = backoff(attempt).as_millis() as u64;
            let spread = base / 5;
            assert!(
                (base - spread..=base + spread).contains(&waited),
                "attempt {attempt}: {waited}ms outside {}..={}",
                base - spread,
                base + spread
            );
        }
        assert!(backoff(2) > backoff(1) - Duration::from_millis(200), "backoff should grow");
    }
}
