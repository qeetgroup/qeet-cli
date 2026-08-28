//! `qeet status <product>` — what is on disk, and how does it compare to the remote?
//!
//! Strictly read-only. Nothing here fetches, so "behind" means "behind what you last
//! fetched"; `qeet update` is the command that talks to the remote.

use std::io::Write;
use std::sync::Arc;

use super::context;
use crate::error::Error;
use crate::git::{GitClient, RepoState};
use crate::output::style::{bad, dim, heading, name, ok, symbol, warn};

/// One repository's line in the report.
enum Entry {
    Present(RepoState),
    Missing,
    /// Something is there, but it is not a repository we can read.
    Unreadable(String),
}

pub async fn run(args: &crate::cli::ProductArgs) -> Result<bool, Error> {
    let ctx = context::resolve(&args.product, args.protocol, args.manifest.as_deref()).await?;

    let mut rows = Vec::with_capacity(ctx.product.repositories.len());
    for entry in &ctx.product.repositories {
        let relative = entry.path.clone().unwrap_or_else(|| entry.name.clone());
        let path = context::destination(&ctx.workspace, &ctx.product, &relative);

        let row = if !path.exists() {
            Entry::Missing
        } else if !crate::git::is_git_repository(&path) {
            Entry::Unreadable("not a git repository".to_string())
        } else {
            match Arc::clone(&ctx.git).inspect(path.clone()).await {
                Ok(state) => Entry::Present(state),
                Err(err) => Entry::Unreadable(err.to_string()),
            }
        };
        rows.push((entry.name.clone(), row));
    }

    let mut out = std::io::stdout().lock();
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{}  {}",
        heading(&ctx.product.name),
        dim(ctx.product.group_dir().map_or_else(|| ".".to_string(), |d| format!("{d}/")))
    );
    if let Some(note) = &ctx.note {
        let _ = writeln!(out, "{}", dim(format!("manifest: {note}")));
    }
    let _ = writeln!(out);

    let width = rows.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    let mut clean = 0;
    let mut attention = 0;
    let mut missing = 0;

    for (repo, row) in &rows {
        let (symbol_text, branch, detail) = match row {
            Entry::Present(state) => {
                let branch = state.branch.clone().unwrap_or_else(|| "(detached)".to_string());
                match state.blocker() {
                    Some(blocker) => {
                        attention += 1;
                        (warn(symbol::WARN).to_string(), branch, warn(blocker).to_string())
                    }
                    None if state.behind > 0 => {
                        attention += 1;
                        (
                            warn(symbol::WARN).to_string(),
                            branch,
                            warn(format!("{} behind", state.behind)).to_string(),
                        )
                    }
                    None => {
                        clean += 1;
                        (ok(symbol::OK).to_string(), branch, dim("clean").to_string())
                    }
                }
            }
            Entry::Missing => {
                missing += 1;
                (dim(symbol::PENDING).to_string(), "—".to_string(), dim("not cloned").to_string())
            }
            Entry::Unreadable(why) => {
                attention += 1;
                (bad(symbol::BAD).to_string(), "—".to_string(), bad(why).to_string())
            }
        };
        let _ =
            writeln!(out, "  {symbol_text} {:<width$}  {:<10}  {detail}", name(repo), dim(branch));
    }

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {}",
        dim(format!("clean {clean}  ·  needs attention {attention}  ·  not cloned {missing}"))
    );
    if missing > 0 {
        let _ = writeln!(
            out,
            "  {}",
            dim(format!("`qeet clone {}` to fetch what is missing.", ctx.product_key))
        );
    }
    let _ = out.flush();

    // `status` reports; it does not judge. Only an unreadable path is a real problem, and a
    // missing clone is information, not failure -- so status exits 0 unless it could not
    // read something it should have been able to.
    Ok(!rows.iter().any(|(_, row)| matches!(row, Entry::Unreadable(_))))
}
