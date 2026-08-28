//! The product registry: what a product is, and which repositories belong to it.
//!
//! The manifest is data, not code. Adding a product or moving a repository between
//! products is a manifest edit -- it never requires a change to this crate.

pub mod source;
pub mod validate;

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::remote::{Protocol, Remote};

/// The only manifest schema this build understands.
///
/// A manifest declaring anything else is refused rather than interpreted, so a newer
/// manifest cannot be half-understood by an older binary.
pub const SUPPORTED_SCHEMA: u32 = 1;

/// A parsed, not-yet-validated manifest. Call [`validate::check`] before use.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub remote: Remote,
    /// Ordered by key, which is also the order products are listed to the user.
    pub products: BTreeMap<String, Product>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Product {
    /// Display name, e.g. "Qeet ID".
    pub name: String,
    /// Directory the product's repositories are grouped under, relative to the workspace --
    /// e.g. `qeet-id`, giving `qeet-id/qeet-id-server`.
    ///
    /// Optional, and absent means flat: the repositories land directly in the workspace. That
    /// is deliberate rather than an oversight -- organization-level repositories such as
    /// `qeet-docs` and `qeet-apis` belong at the top of a workspace, not nested one deeper.
    #[serde(default)]
    pub directory: Option<String>,
    pub repositories: Vec<RepositoryEntry>,
}

impl Product {
    /// The grouping directory, if this product has one.
    pub fn group_dir(&self) -> Option<&str> {
        self.directory.as_deref().filter(|dir| !dir.trim().is_empty())
    }
}

/// One repository within a product.
///
/// Only `name` is required. `url` overrides URL derivation entirely, `path` overrides the
/// destination directory, and `ref` clones a branch or tag other than the remote default.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryEntry {
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(rename = "ref", default)]
    pub git_ref: Option<String>,
}

/// A manifest that could not be read, parsed or validated.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("cannot read manifest {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// `toml`'s own message already carries `line N, column M`, so it is passed through.
    #[error("invalid manifest{}:\n{source}", location(origin))]
    Parse {
        origin: String,
        #[source]
        source: toml::de::Error,
    },

    #[error(
        "unsupported manifest schema {found} (this build of qeet supports schema {SUPPORTED_SCHEMA})"
    )]
    Schema { found: u32 },

    #[error("invalid manifest{}:\n{}", location(origin), render_issues(issues))]
    Invalid { origin: String, issues: Vec<validate::Issue> },
}

fn location(origin: &str) -> String {
    if origin.is_empty() { String::new() } else { format!(" ({origin})") }
}

fn render_issues(issues: &[validate::Issue]) -> String {
    issues.iter().map(|issue| format!("  - {issue}")).collect::<Vec<_>>().join("\n")
}

impl Manifest {
    /// Parse a manifest, then validate it. Nothing else in the crate accepts an
    /// unvalidated manifest, so this is the only way to obtain one.
    pub fn load(text: &str, origin: &str) -> Result<Self, ManifestError> {
        let manifest: Self = toml::from_str(text)
            .map_err(|source| ManifestError::Parse { origin: origin.to_string(), source })?;

        // Checked before anything else: a schema we do not understand makes every other
        // diagnostic untrustworthy.
        if manifest.schema != SUPPORTED_SCHEMA {
            return Err(ManifestError::Schema { found: manifest.schema });
        }

        let issues = validate::check(&manifest);
        if !issues.is_empty() {
            return Err(ManifestError::Invalid { origin: origin.to_string(), issues });
        }

        Ok(manifest)
    }

    /// Product keys, in the order they are shown to the user.
    pub fn product_keys(&self) -> impl Iterator<Item = &str> {
        self.products.keys().map(String::as_str)
    }

    /// The clone URL for an entry: its explicit `url`, else derived from `[remote]`.
    pub fn url_for(&self, entry: &RepositoryEntry, protocol: Protocol) -> String {
        entry.url.clone().unwrap_or_else(|| self.remote.url_for(&entry.name, protocol))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
schema = 1
[remote]
host = "github.com"
owner = "qeetgroup"
protocol = "ssh"
[products.id]
name = "Qeet ID"
repositories = [{ name = "qeet-id-server" }]
"#;

    #[test]
    fn parses_a_minimal_manifest() {
        let manifest = Manifest::load(MINIMAL, "test").expect("should parse");
        assert_eq!(manifest.schema, 1);
        assert_eq!(manifest.remote.owner, "qeetgroup");
        assert_eq!(manifest.products["id"].name, "Qeet ID");
        assert_eq!(manifest.products["id"].repositories.len(), 1);
    }

    #[test]
    fn parses_all_optional_repository_fields() {
        let text = r#"
schema = 1
[remote]
host = "github.com"
owner = "qeetgroup"
protocol = "https"
[products.id]
name = "Qeet ID"
repositories = [
  { name = "a", url = "https://example.com/a.git", path = "custom/a", ref = "develop" },
]
"#;
        let manifest = Manifest::load(text, "test").expect("should parse");
        let entry = &manifest.products["id"].repositories[0];
        assert_eq!(entry.url.as_deref(), Some("https://example.com/a.git"));
        assert_eq!(entry.path.as_deref(), Some("custom/a"));
        assert_eq!(entry.git_ref.as_deref(), Some("develop"));
    }

    #[test]
    fn malformed_toml_reports_line_and_column() {
        let err = Manifest::load("schema = = 1", "products.toml").unwrap_err();
        let message = err.to_string();
        assert!(matches!(err, ManifestError::Parse { .. }), "{message}");
        assert!(message.contains("products.toml"), "{message}");
        assert!(message.contains("line 1"), "should report a line: {message}");
    }

    #[test]
    fn rejects_an_unsupported_schema() {
        let text = MINIMAL.replace("schema = 1", "schema = 2");
        let err = Manifest::load(&text, "test").unwrap_err();
        assert!(matches!(err, ManifestError::Schema { found: 2 }), "{err}");
    }

    #[test]
    fn rejects_a_missing_schema() {
        let text = MINIMAL.replace("schema = 1\n", "");
        let err = Manifest::load(&text, "test").unwrap_err();
        assert!(matches!(err, ManifestError::Parse { .. }), "{err}");
    }

    #[test]
    fn rejects_unknown_fields_rather_than_ignoring_them() {
        // A typo'd or newer key must not silently change behaviour.
        for text in [
            MINIMAL.replace("[products.id]", "surprise = true\n[products.id]"),
            MINIMAL.replace(r#"{ name = "qeet-id-server" }"#, r#"{ name = "a", brunch = "x" }"#),
            MINIMAL.replace(
                r#"name = "Qeet ID""#,
                r#"name = "Qeet ID"
title = "extra""#,
            ),
        ] {
            let err = Manifest::load(&text, "test").unwrap_err();
            assert!(matches!(err, ManifestError::Parse { .. }), "{err}");
            assert!(err.to_string().contains("unknown field"), "{err}");
        }
    }

    #[test]
    fn derives_urls_but_prefers_an_explicit_override() {
        let text = r#"
schema = 1
[remote]
host = "github.com"
owner = "qeetgroup"
protocol = "ssh"
[products.id]
name = "Qeet ID"
repositories = [
  { name = "derived" },
  { name = "explicit", url = "https://elsewhere.example/thing.git" },
]
"#;
        let manifest = Manifest::load(text, "test").expect("should parse");
        let repos = &manifest.products["id"].repositories;
        assert_eq!(
            manifest.url_for(&repos[0], Protocol::Ssh),
            "git@github.com:qeetgroup/derived.git"
        );
        assert_eq!(
            manifest.url_for(&repos[0], Protocol::Https),
            "https://github.com/qeetgroup/derived.git"
        );
        // An explicit URL is used as written, whatever the protocol.
        assert_eq!(
            manifest.url_for(&repos[1], Protocol::Ssh),
            "https://elsewhere.example/thing.git"
        );
    }

    #[test]
    fn protocol_override_wins_over_the_manifest_default() {
        let manifest = Manifest::load(MINIMAL, "test").expect("should parse");
        assert_eq!(manifest.remote.effective_protocol(None), Protocol::Ssh);
        assert_eq!(manifest.remote.effective_protocol(Some(Protocol::Https)), Protocol::Https);
    }
}
