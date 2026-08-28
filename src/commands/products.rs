//! `qeet products` — what can I clone?

use std::io::Write;

use super::context;
use crate::error::Error;
use crate::output::style::{dim, heading, name};

pub async fn run(args: &crate::cli::ManifestArgs) -> Result<(), Error> {
    let loaded = context::manifest(args.manifest.as_deref())?;
    let manifest = &loaded.manifest;

    let mut out = std::io::stdout().lock();
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", heading("Qeet Products"));
    if let Some(note) = &loaded.note {
        let _ = writeln!(out, "{}", dim(format!("manifest: {note}")));
    }
    let _ = writeln!(out);

    // Widths from the data, so the columns line up whatever is in the manifest.
    let key_width = manifest.product_keys().map(str::len).max().unwrap_or(0);
    let name_width =
        manifest.products.values().map(|product| product.name.len()).max().unwrap_or(0);

    let mut repositories = 0;
    for (key, product) in &manifest.products {
        let count = product.repositories.len();
        repositories += count;
        let location = product.group_dir().unwrap_or(".");
        let _ = writeln!(
            out,
            "  {:<key_width$}  {:<name_width$}  {:>2} {}  {}",
            name(key),
            product.name,
            count,
            if count == 1 { "repo " } else { "repos" },
            dim(location),
        );
    }

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{}",
        dim(format!(
            "{} products, {repositories} repositories.  `qeet repos <product>` for detail.",
            manifest.products.len()
        ))
    );
    let _ = out.flush();
    Ok(())
}
