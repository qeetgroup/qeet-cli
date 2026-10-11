//! `qeet update <product>...` — advance what can be advanced, and touch nothing else.
//!
//! Also spelled `qeet pull`. Takes one product, several, or `all`.
//!
//! This is the only command that changes an existing repository, so it is deliberately the
//! most conservative one. A repository is fast-forwarded **only** when the result is
//! unambiguous: clean, on a branch, tracking an upstream, nothing of its own unpushed, and
//! strictly behind. Anything else is skipped and named.
//!
//! `git merge --ff-only` is what enforces that at the git level: it refuses rather than
//! creating a merge commit, so this cannot invent history or leave a conflict behind.

use std::io::Write;
use std::sync::Arc;

use super::context;
use crate::clone::coordinator::default_concurrency;
use crate::error::Error;
use crate::git::{Git, GitClient, RepoState};
use crate::manifest::Product;
use crate::output::style::{bad, dim, heading, name, ok, symbol, warn};
use crate::product;
use crate::workspace::Workspace;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

enum Result_ {
    Advanced { from_behind: u32 },
    AlreadyCurrent,
    Skipped(String),
    Missing,
    Failed(String),
}

pub async fn run(args: &crate::cli::UpdateArgs) -> Result<bool, Error> {
    let context::Loaded { manifest, note } = context::manifest(args.manifest.as_deref())?;

    // Resolved before git or the filesystem are touched: a typo among several products
    // should cost nothing but a listing, not half an update.
    let products = product::resolve_many(&manifest, &args.products)?;
    let git = Arc::new(Git::discover().await?);
    let workspace = Workspace::discover().map_err(|source| Error::Workspace { source })?;
    let limit = args.concurrency.unwrap_or_else(default_concurrency);

    // One bounded pool across every product, rather than one product at a time. The bound
    // is what protects the remote and it is the same either way, but a product with one
    // repository no longer leaves slots idle that the next product could be using.
    let semaphore = Arc::new(Semaphore::new(limit.get()));
    let mut tasks = JoinSet::new();
    // Per product, per repository: the name now, the result once it settles.
    let mut slots: Vec<Vec<(String, Option<Result_>)>> = Vec::with_capacity(products.len());

    for (p, (_, product)) in products.iter().enumerate() {
        let mut repos = Vec::with_capacity(product.repositories.len());
        for (r, entry) in product.repositories.iter().enumerate() {
            let relative = entry.path.as_deref().unwrap_or(&entry.name);
            let path = context::destination(&workspace, product, relative);
            let git = Arc::clone(&git);
            let semaphore = Arc::clone(&semaphore);
            let dry_run = args.dry_run;

            tasks.spawn(async move {
                let _permit = semaphore.acquire().await.expect("semaphore is never closed");
                (p, r, update_one(git.as_ref(), &path, dry_run).await)
            });
            repos.push((entry.name.clone(), None));
        }
        slots.push(repos);
    }

    let mut report = Report::new(args.dry_run, note, products.len(), &workspace);
    let mut next = 0;
    while let Some(joined) = tasks.join_next().await {
        if let Ok((p, r, result)) = joined {
            slots[p][r].1 = Some(result);
        }
        // Rendered in manifest order, not completion order: each product as soon as it, and
        // every product before it, has settled.
        while slots.get(next).is_some_and(|repos| repos.iter().all(|(_, result)| result.is_some()))
        {
            report.product(&workspace, products[next], settle(std::mem::take(&mut slots[next])));
            next += 1;
        }
    }
    // Every task has ended. Anything still unrendered had one that did not finish, and is
    // reported as such rather than silently dropped from the report.
    for (p, repos) in slots.into_iter().enumerate().skip(next) {
        report.product(&workspace, products[p], settle(repos));
    }

    Ok(report.finish())
}

fn settle(repos: Vec<(String, Option<Result_>)>) -> Vec<(String, Result_)> {
    repos
        .into_iter()
        .map(|(repo, result)| {
            (repo, result.unwrap_or_else(|| Result_::Failed("did not finish".to_string())))
        })
        .collect()
}

/// Fetch, then advance only if the resulting state is unambiguous.
async fn update_one<G: GitClient>(git: &G, path: &std::path::Path, dry_run: bool) -> Result_ {
    if !path.exists() {
        return Result_::Missing;
    }
    if !crate::git::is_git_repository(path) {
        return Result_::Failed("not a git repository".to_string());
    }

    // Checked *before* fetching: if the tree is dirty there is no point touching the remote,
    // and refusing early keeps the reason honest.
    let before = match git.inspect(path.to_path_buf()).await {
        Ok(state) => state,
        Err(err) => return Result_::Failed(err.to_string()),
    };
    if let Some(blocker) = before.blocker() {
        return Result_::Skipped(blocker);
    }

    if dry_run {
        // Report against what is already known locally rather than fetching, so --dry-run
        // genuinely touches nothing at all.
        return describe(&before);
    }

    if let Err(failure) = git.fetch(path.to_path_buf()).await {
        return Result_::Failed(format!("{} {}", failure.summary(), failure.git_stderr));
    }

    let after = match git.inspect(path.to_path_buf()).await {
        Ok(state) => state,
        Err(err) => return Result_::Failed(err.to_string()),
    };
    // Re-checked after the fetch: fetching can reveal divergence that did not exist before.
    if let Some(blocker) = after.blocker() {
        return Result_::Skipped(blocker);
    }
    if after.up_to_date() {
        return Result_::AlreadyCurrent;
    }
    if !after.fast_forwardable() {
        return Result_::AlreadyCurrent;
    }

    match git.fast_forward(path.to_path_buf()).await {
        Ok(()) => Result_::Advanced { from_behind: after.behind },
        Err(failure) => Result_::Failed(format!("{} {}", failure.summary(), failure.git_stderr)),
    }
}

fn describe(state: &RepoState) -> Result_ {
    if state.fast_forwardable() {
        Result_::Advanced { from_behind: state.behind }
    } else {
        Result_::AlreadyCurrent
    }
}

/// How many repositories ended up in each state.
#[derive(Default)]
struct Tally {
    advanced: usize,
    current: usize,
    skipped: usize,
    missing: usize,
    failed: usize,
}

impl Tally {
    fn count(&mut self, result: &Result_) {
        match result {
            Result_::Advanced { .. } => self.advanced += 1,
            Result_::AlreadyCurrent => self.current += 1,
            Result_::Skipped(_) => self.skipped += 1,
            Result_::Missing => self.missing += 1,
            Result_::Failed(_) => self.failed += 1,
        }
    }

    fn absorb(&mut self, other: &Self) {
        self.advanced += other.advanced;
        self.current += other.current;
        self.skipped += other.skipped;
        self.missing += other.missing;
        self.failed += other.failed;
    }

    fn summary(&self) -> String {
        format!(
            "updated {}  ·  up to date {}  ·  skipped {}  ·  not cloned {}  ·  failed {}",
            self.advanced, self.current, self.skipped, self.missing, self.failed
        )
    }
}

/// Renders each product as its results arrive, and keeps the totals the closing lines and
/// the exit code need.
struct Report {
    dry_run: bool,
    /// Shown once, under the first heading, then taken.
    note: Option<String>,
    products: usize,
    /// The workspace root, for saying where something was not found.
    root: String,
    total: Tally,
    /// Products with nothing cloned at all, when updating several.
    absent: Vec<String>,
}

impl Report {
    fn new(dry_run: bool, note: Option<String>, products: usize, workspace: &Workspace) -> Self {
        Self {
            dry_run,
            note,
            products,
            root: workspace.root().display().to_string(),
            total: Tally::default(),
            absent: Vec::new(),
        }
    }

    fn several(&self) -> bool {
        self.products > 1
    }

    fn product(
        &mut self,
        workspace: &Workspace,
        (key, product): (&str, &Product),
        rows: Vec<(String, Result_)>,
    ) {
        let mut tally = Tally::default();
        for (_, result) in &rows {
            tally.count(result);
        }
        self.total.absorb(&tally);

        // Across several products, one that was never cloned here is a word in a list, not
        // a block of "not cloned" lines -- `all` would otherwise bury the real results.
        if self.several() && !rows.is_empty() && tally.missing == rows.len() {
            self.absent.push(key.to_string());
            return;
        }

        let mut out = std::io::stdout().lock();
        let dir = context::product_dir(workspace, product).display().to_string();
        self.title(&mut out, &product.name, Some(&dir));
        let _ = writeln!(out);

        let width = rows.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        for (repo, result) in &rows {
            let _ = writeln!(out, "  {:<width$}  {}", name(repo), line(result, self.dry_run));
        }

        let _ = writeln!(out);
        let _ = writeln!(out, "  {}", dim(tally.summary()));
        let _ = out.flush();
    }

    /// Close the report, and say whether every repository reached a state it should have.
    fn finish(mut self) -> bool {
        let mut out = std::io::stdout().lock();
        if self.several() {
            self.title(&mut out, &format!("{} products", self.products), None);
            let _ = writeln!(out);
            let _ = writeln!(out, "  {}", dim(self.total.summary()));
            if !self.absent.is_empty() {
                let _ = writeln!(
                    out,
                    "  {}",
                    dim(format!(
                        "Not cloned in {}: {}. `qeet clone <product>` to fetch one.",
                        self.root,
                        self.absent.join(", ")
                    ))
                );
            }
        }
        if self.total.skipped > 0 {
            let _ = writeln!(
                out,
                "  {}",
                dim("Skipped repositories were left exactly as they were. Resolve them by hand.")
            );
        }
        let _ = out.flush();
        self.total.failed == 0
    }

    /// A section heading with where it lives, the dry-run warning, and the manifest note
    /// the first time.
    fn title(&mut self, out: &mut impl Write, text: &str, dir: Option<&str>) {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{}{}{}",
            heading(text),
            dir.map(|dir| format!("  {}", dim(dir))).unwrap_or_default(),
            if self.dry_run {
                dim("  (dry run — nothing was changed)").to_string()
            } else {
                String::new()
            }
        );
        if let Some(note) = self.note.take() {
            let _ = writeln!(out, "{}", dim(format!("manifest: {note}")));
        }
    }
}

fn line(result: &Result_, dry_run: bool) -> String {
    match result {
        Result_::Advanced { from_behind } => format!(
            "{} {}",
            ok(symbol::OK),
            ok(if dry_run {
                format!("would fast-forward {from_behind} commit(s)")
            } else {
                format!("fast-forwarded {from_behind} commit(s)")
            })
        ),
        Result_::AlreadyCurrent => format!("{} {}", dim(symbol::SKIP), dim("already up to date")),
        Result_::Skipped(why) => {
            format!("{} {}", warn(symbol::WARN), warn(format!("skipped: {why}")))
        }
        Result_::Missing => format!("{} {}", dim(symbol::PENDING), dim("not cloned")),
        Result_::Failed(why) => format!("{} {}", bad(symbol::BAD), bad(why)),
    }
}
