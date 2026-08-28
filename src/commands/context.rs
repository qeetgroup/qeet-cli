//! The opening moves every command shares.
//!
//! Resolving a manifest, validating it, resolving a product, checking git, preparing a
//! workspace: `clone`, `status` and `update` all need some prefix of that. Doing it here
//! keeps the precedence rules and error messages identical across commands rather than
//! drifting apart one copy at a time.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::Error;
use crate::git::Git;
use crate::manifest::{Manifest, Product, source};
use crate::product;
use crate::remote::Protocol;
use crate::workspace::Workspace;

/// A loaded, validated manifest and where it came from.
pub struct Loaded {
    pub manifest: Manifest,
    /// The manifest's origin, but only when it is *not* the built-in registry. Commands show
    /// this so a stale override is never invisible.
    pub note: Option<String>,
}

/// Load and validate a manifest, following the documented precedence.
pub fn manifest(flag: Option<&Path>) -> Result<Loaded, Error> {
    let loaded = source::resolve(flag)?;
    let origin = loaded.origin.to_string();
    let manifest = Manifest::load(&loaded.text, &origin)?;
    let note = match loaded.origin {
        source::Origin::Embedded => None,
        _ => Some(origin),
    };
    Ok(Loaded { manifest, note })
}

/// Everything a command that acts on one product's repositories needs.
pub struct Resolved {
    pub note: Option<String>,
    pub product_key: String,
    pub product: Product,
    pub git: Arc<Git>,
    pub workspace: Workspace,
}

/// Resolve a product and prepare to act on its repositories.
///
/// Ordered so the cheap failures come first: an unknown product costs nothing but a listing,
/// and a missing git is one clear error rather than one per repository.
pub async fn resolve(
    product_key: &str,
    protocol: Option<Protocol>,
    manifest_flag: Option<&Path>,
) -> Result<Resolved, Error> {
    let Loaded { manifest, note } = manifest(manifest_flag)?;
    let resolved = product::resolve(&manifest, product_key)?.clone();
    let git = Arc::new(Git::discover().await?);
    let workspace = Workspace::discover().map_err(|source| Error::Workspace { source })?;
    let _ = manifest.remote.effective_protocol(protocol);

    Ok(Resolved {
        product_key: canonical_key(&manifest, product_key),
        product: resolved,
        git,
        workspace,
        note,
    })
}

/// The manifest's own spelling of the key the user typed, since lookup is case-insensitive.
fn canonical_key(manifest: &Manifest, requested: &str) -> String {
    let normalised = requested.trim().to_ascii_lowercase();
    manifest.product_keys().find(|key| *key == normalised).unwrap_or(&normalised).to_string()
}

/// Absolute destination for a repository, mirroring what `clone` would produce.
pub fn destination(workspace: &Workspace, product: &Product, relative: &str) -> PathBuf {
    match product.group_dir() {
        Some(dir) => workspace.root().join(dir).join(relative),
        None => workspace.root().join(relative),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_of(text: &str) -> Manifest {
        Manifest::load(text, "test").expect("fixture must be valid")
    }

    const TEXT: &str = r#"
schema = 1
[remote]
host = "github.com"
owner = "qeetgroup"
protocol = "ssh"
[products.id]
name = "Qeet ID"
directory = "qeet-id"
repositories = [{ name = "qeet-id-server" }]
[products.group]
name = "Org"
repositories = [{ name = "qeet-docs" }]
"#;

    #[test]
    fn canonicalises_however_the_user_typed_it() {
        let manifest = manifest_of(TEXT);
        for input in ["id", "ID", "  Id  "] {
            assert_eq!(canonical_key(&manifest, input), "id", "{input}");
        }
    }

    #[test]
    fn destinations_honour_the_group_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = Workspace::at(dir.path()).expect("workspace");
        let manifest = manifest_of(TEXT);

        assert_eq!(
            destination(&workspace, &manifest.products["id"], "qeet-id-server"),
            workspace.root().join("qeet-id/qeet-id-server")
        );
        // A product with no directory stays flat.
        assert_eq!(
            destination(&workspace, &manifest.products["group"], "qeet-docs"),
            workspace.root().join("qeet-docs")
        );
    }
}
