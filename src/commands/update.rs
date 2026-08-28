//! `qeet update <product>` — advance what can be advanced, and touch nothing else.
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
use crate::git::{GitClient, RepoState};
use crate::output::style::{bad, dim, heading, name, ok, symbol, warn};
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
    let ctx = context::resolve(&args.product, args.protocol, args.manifest.as_deref()).await?;
    let limit = args.concurrency.unwrap_or_else(default_concurrency);

    let mut targets = Vec::new();
    for entry in &ctx.product.repositories {
        let relative = entry.path.clone().unwrap_or_else(|| entry.name.clone());
        let path = context::destination(&ctx.workspace, &ctx.product, &relative);
        targets.push((entry.name.clone(), path));
    }

    let semaphore = Arc::new(Semaphore::new(limit.get()));
    let mut tasks = JoinSet::new();

    for (index, (repo, path)) in targets.iter().cloned().enumerate() {
        let git = Arc::clone(&ctx.git);
        let semaphore = Arc::clone(&semaphore);
        let dry_run = args.dry_run;

        tasks.spawn(async move {
            let _permit = semaphore.acquire().await.expect("semaphore is never closed");
            (index, repo, update_one(git.as_ref(), &path, dry_run).await)
        });
    }

    let mut settled: std::collections::HashMap<usize, (String, Result_)> =
        std::collections::HashMap::new();
    while let Some(joined) = tasks.join_next().await {
        if let Ok((index, repo, result)) = joined {
            settled.insert(index, (repo, result));
        }
    }

    // Report in manifest order, not completion order.
    let ordered: Vec<(String, Result_)> =
        (0..targets.len()).filter_map(|i| settled.remove(&i)).collect();

    render(&ctx, args.dry_run, &ordered);
    Ok(!ordered.iter().any(|(_, r)| matches!(r, Result_::Failed(_))))
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

fn render(ctx: &context::Resolved, dry_run: bool, rows: &[(String, Result_)]) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{}{}",
        heading(&ctx.product.name),
        if dry_run {
            dim("  (dry run — nothing was changed)").to_string()
        } else {
            String::new()
        }
    );
    if let Some(note) = &ctx.note {
        let _ = writeln!(out, "{}", dim(format!("manifest: {note}")));
    }
    let _ = writeln!(out);

    let width = rows.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    let (mut advanced, mut current, mut skipped, mut missing, mut failed) = (0, 0, 0, 0, 0);

    for (repo, result) in rows {
        let line = match result {
            Result_::Advanced { from_behind } => {
                advanced += 1;
                format!(
                    "{} {}",
                    ok(symbol::OK),
                    ok(if dry_run {
                        format!("would fast-forward {from_behind} commit(s)")
                    } else {
                        format!("fast-forwarded {from_behind} commit(s)")
                    })
                )
            }
            Result_::AlreadyCurrent => {
                current += 1;
                format!("{} {}", dim(symbol::SKIP), dim("already up to date"))
            }
            Result_::Skipped(why) => {
                skipped += 1;
                format!("{} {}", warn(symbol::WARN), warn(format!("skipped: {why}")))
            }
            Result_::Missing => {
                missing += 1;
                format!("{} {}", dim(symbol::PENDING), dim("not cloned"))
            }
            Result_::Failed(why) => {
                failed += 1;
                format!("{} {}", bad(symbol::BAD), bad(why))
            }
        };
        let _ = writeln!(out, "  {:<width$}  {line}", name(repo));
    }

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {}",
        dim(format!(
            "updated {advanced}  ·  up to date {current}  ·  skipped {skipped}  ·  not cloned {missing}  ·  failed {failed}"
        ))
    );
    if skipped > 0 {
        let _ = writeln!(
            out,
            "  {}",
            dim("Skipped repositories were left exactly as they were. Resolve them by hand.")
        );
    }
    let _ = out.flush();
}
