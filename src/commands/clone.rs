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
pub async fn run(args: &crate::cli::CloneArgs) -> Result<Report, Error> {
    let loaded = source::resolve(args.manifest.as_deref())?;
    let origin = loaded.origin.to_string();
    let manifest = Manifest::load(&loaded.text, &origin)?;

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
    // Only worth saying when it is not the built-in registry: otherwise it is noise.
    let manifest_note = match loaded.origin {
        source::Origin::Embedded => None,
        _ => Some(origin),
    };

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
