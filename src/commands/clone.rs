//! `qeet clone <product>`.
//!
//! The pipeline, in order, and the order matters: everything that can fail cheaply and
//! completely fails before a single git process starts.
//!
//! ```text
//! manifest -> validate -> product -> git prerequisite -> workspace preflight
//!          -> bounded concurrent clone -> report
//! ```

use std::sync::Arc;

use crate::clone::{self, Job, Options, Report, coordinator};
use crate::error::Error;
use crate::git::Git;
use crate::manifest::{Manifest, source};
use crate::output;
use crate::product;
use crate::workspace::Workspace;

/// Run a clone, returning the report. Never returns `Err` for a repository-level failure --
/// those are in the report, which is what the exit code is derived from.
/// The pseudo-product that means "every product".
const ALL: &str = "all";

/// Only worth reporting when it is not the built-in registry; otherwise it is noise.
fn manifest_note(loaded: &source::Loaded) -> Option<String> {
    match loaded.origin {
        source::Origin::Embedded => None,
        _ => Some(loaded.origin.to_string()),
    }
}

pub async fn run(args: &crate::cli::CloneArgs) -> Result<Report, Error> {
    let loaded = source::resolve(args.manifest.as_deref())?;
    let origin = loaded.origin.to_string();
    let manifest = Manifest::load(&loaded.text, &origin)?;

    // `all` is not a product key -- it is every product at once, which is why it is handled
    // here rather than smuggled into the manifest as a pseudo-product.
    if args.product.trim().eq_ignore_ascii_case(ALL) {
        return clone_everything(&manifest, manifest_note(&loaded), args).await;
    }

    // Resolved before git or the filesystem are touched: an unknown product should cost
    // nothing but a listing.
    let resolved = product::resolve(&manifest, &args.product)?;

    // Checked once. Without this, a missing git would produce one identical failure per
    // repository instead of a single clear message.
    let git = Arc::new(Git::discover().await?);

    let workspace = Workspace::discover().map_err(|source| Error::Workspace { source })?;

    let protocol = manifest.remote.effective_protocol(args.protocol);
    let plans = workspace.plan(&manifest, resolved, protocol, git.as_ref()).await;

    let renderer = output::detect();
    let manifest_note = manifest_note(&loaded);

    let options = Options {
        concurrency: args.concurrency.unwrap_or_else(coordinator::default_concurrency),
        max_retries: coordinator::DEFAULT_MAX_RETRIES,
    };

    let job =
        Job { product_name: resolved.name.clone(), plans, root: workspace.root().to_path_buf() };

    let report =
        clone::run(git, job, options, Arc::clone(&renderer), manifest_note.as_deref()).await;

    renderer.finish(&report);
    Ok(report)
}

/// Clone every product, one after another, each with its own bounded concurrency.
///
/// Sequential *between* products on purpose: 66 repositories at once would drown the remote
/// and make the output unreadable, and the per-product bound is what makes progress legible.
/// The concurrency within each product is unchanged.
async fn clone_everything(
    manifest: &Manifest,
    manifest_note: Option<String>,
    args: &crate::cli::CloneArgs,
) -> Result<Report, Error> {
    let git = Arc::new(Git::discover().await?);
    let workspace = Workspace::discover().map_err(|source| Error::Workspace { source })?;
    let protocol = manifest.remote.effective_protocol(args.protocol);
    let renderer = output::detect();
    let options = Options {
        concurrency: args.concurrency.unwrap_or_else(coordinator::default_concurrency),
        max_retries: coordinator::DEFAULT_MAX_RETRIES,
    };

    let keys: Vec<String> = manifest.product_keys().map(str::to_string).collect();
    let mut combined: Vec<crate::clone::report::RepositoryReport> = Vec::new();
    let mut cancelled = false;
    let started = std::time::Instant::now();

    for key in &keys {
        let product = &manifest.products[key];
        let plans = workspace.plan(manifest, product, protocol, git.as_ref()).await;

        let job =
            Job { product_name: product.name.clone(), plans, root: workspace.root().to_path_buf() };
        let report = clone::run(
            Arc::clone(&git),
            job,
            options,
            Arc::clone(&renderer),
            manifest_note.as_deref(),
        )
        .await;

        cancelled |= report.cancelled;
        combined.extend(report.repositories);

        // Ctrl-C during one product stops the whole run, not just that product.
        if cancelled {
            break;
        }
    }

    let report = Report {
        product_name: format!("All products ({} of {})", combined.len(), keys.len()),
        repositories: combined,
        cancelled,
        elapsed: started.elapsed(),
    };
    renderer.finish(&report);
    Ok(report)
}
