//! `qeet repos <product>` — which repositories, and where would they land?

use std::io::Write;

use super::context;
use crate::error::Error;
use crate::output::style::{dim, heading, name};

pub async fn run(args: &crate::cli::ProductArgs) -> Result<(), Error> {
    let loaded = context::manifest(args.manifest.as_deref())?;
    let manifest = &loaded.manifest;
    let product = crate::product::resolve(manifest, &args.product)?;
    let protocol = manifest.remote.effective_protocol(args.protocol);

    let mut out = std::io::stdout().lock();
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", heading(&product.name));
    if let Some(note) = &loaded.note {
        let _ = writeln!(out, "{}", dim(format!("manifest: {note}")));
    }
    let _ = writeln!(
        out,
        "{}",
        dim(match product.group_dir() {
            Some(dir) => format!("{} repositories, into {dir}/", product.repositories.len()),
            None =>
                format!("{} repositories, into the current directory", product.repositories.len()),
        })
    );
    let _ = writeln!(out);

    let width = product.repositories.iter().map(|entry| entry.name.len()).max().unwrap_or(0);

    for entry in &product.repositories {
        let url = manifest.url_for(entry, protocol);
        let mut notes = Vec::new();
        if let Some(path) = &entry.path {
            notes.push(format!("path {path}"));
        }
        if let Some(git_ref) = &entry.git_ref {
            notes.push(format!("ref {git_ref}"));
        }
        let suffix = if notes.is_empty() {
            String::new()
        } else {
            format!("  {}", dim(format!("({})", notes.join(", "))))
        };
        let _ = writeln!(out, "  {:<width$}  {}{suffix}", name(&entry.name), dim(&url));
    }
    let _ = out.flush();
    Ok(())
}
