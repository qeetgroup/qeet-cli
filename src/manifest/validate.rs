//! Manifest validation.
//!
//! Everything checkable without touching the filesystem is checked here, before any
//! process is spawned, and *all* problems are reported at once -- fixing a 66-repository
//! manifest one error per run would be miserable. The filesystem half of path safety
//! (symlinks, real containment) belongs to [`crate::workspace`], which knows the root.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::{Component, Path};

use super::Manifest;
use crate::remote;

/// One problem with a manifest, addressed to the person who has to fix it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// Where the problem is, in manifest terms: `products.id.repositories[3]`.
    pub at: String,
    pub problem: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.problem)
    }
}

fn issue(at: impl Into<String>, problem: impl Into<String>) -> Issue {
    Issue { at: at.into(), problem: problem.into() }
}

/// Check a manifest. An empty result means valid.
///
/// The schema version is checked by [`Manifest::load`](super::Manifest::load) before this
/// runs, so it is not repeated here.
pub fn check(manifest: &Manifest) -> Vec<Issue> {
    let mut issues = Vec::new();

    check_remote(manifest, &mut issues);

    if manifest.products.is_empty() {
        issues.push(issue("products", "no products are defined"));
    }

    for (key, product) in &manifest.products {
        check_product_key(key, &mut issues);

        let at = format!("products.{key}");
        if product.name.trim().is_empty() {
            issues.push(issue(&at, "`name` is empty"));
        }
        if product.repositories.is_empty() {
            issues.push(issue(&at, "`repositories` is empty; a product needs at least one"));
            continue;
        }

        let mut seen_names = HashSet::new();
        // destination -> the repositories that want it, so a collision names both sides.
        let mut destinations: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for (index, entry) in product.repositories.iter().enumerate() {
            let at = format!("{at}.repositories[{index}]");

            if entry.name.trim().is_empty() {
                issues.push(issue(&at, "`name` is empty"));
            } else if !seen_names.insert(&entry.name) {
                issues.push(issue(
                    &at,
                    format!("duplicate repository `{}` within this product", entry.name),
                ));
            }

            check_url(manifest, entry, &at, &mut issues);

            if let Some(path) = entry.path.as_deref() {
                check_path(path, &at, &mut issues);
            }
            if let Some(git_ref) = entry.git_ref.as_deref() {
                check_ref(git_ref, &at, &mut issues);
            }

            let destination = entry.path.clone().unwrap_or_else(|| entry.name.clone());
            destinations
                .entry(normalise_destination(&destination))
                .or_default()
                .push(entry.name.clone());
        }

        for (destination, claimants) in destinations {
            if claimants.len() > 1 {
                issues.push(issue(
                    &at,
                    format!(
                        "{} repositories resolve to the same destination `{destination}`: {}",
                        claimants.len(),
                        claimants.join(", ")
                    ),
                ));
            }
        }
    }

    issues
}

fn check_remote(manifest: &Manifest, issues: &mut Vec<Issue>) {
    // A repository without an explicit `url` needs both of these to derive one.
    let any_derived = manifest
        .products
        .values()
        .flat_map(|product| &product.repositories)
        .any(|entry| entry.url.is_none());

    if manifest.remote.host.trim().is_empty() {
        issues.push(issue("remote.host", "is empty"));
    }
    if manifest.remote.owner.trim().is_empty() && any_derived {
        issues.push(issue(
            "remote.owner",
            "is empty, but repositories without an explicit `url` need it to derive one",
        ));
    }
}

/// Product keys are the user-facing vocabulary (`qeet clone id`), so they are held to a
/// canonical shape: lowercase, digits and internal hyphens.
fn check_product_key(key: &str, issues: &mut Vec<Issue>) {
    let at = format!("products.{key}");
    if key.is_empty() {
        issues.push(issue("products", "a product key is empty"));
        return;
    }
    let shaped = key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !key.starts_with('-')
        && !key.ends_with('-');
    if !shaped {
        issues
            .push(issue(at, "product keys must be lowercase letters, digits and internal hyphens"));
    }
}

fn check_url(
    manifest: &Manifest,
    entry: &super::RepositoryEntry,
    at: &str,
    issues: &mut Vec<Issue>,
) {
    match &entry.url {
        Some(url) => {
            if let Err(err) = remote::validate(url) {
                issues.push(issue(at, format!("`url` is not usable: {err}")));
            }
        }
        None => {
            // Derived URLs are validated too. A hostile `remote.host` or a repository name
            // containing a slash would otherwise reach `git` unchecked.
            if entry.name.trim().is_empty() {
                return;
            }
            for protocol in [remote::Protocol::Ssh, remote::Protocol::Https] {
                let derived = manifest.remote.url_for(&entry.name, protocol);
                if let Err(err) = remote::validate(&derived) {
                    issues.push(issue(
                        at,
                        format!(
                            "the URL derived for {protocol} (`{derived}`) is not usable: {err}"
                        ),
                    ));
                }
            }
        }
    }
}

/// Syntactic path safety. [`crate::workspace`] repeats the containment check against the
/// resolved filesystem, which is the only place symlinks can be accounted for.
fn check_path(path: &str, at: &str, issues: &mut Vec<Issue>) {
    if path.trim().is_empty() {
        issues.push(issue(at, "`path` is empty"));
        return;
    }

    let candidate = Path::new(path);
    if candidate.is_absolute() {
        issues.push(issue(at, format!("`path` must be relative, but `{path}` is absolute")));
        return;
    }

    for component in candidate.components() {
        match component {
            Component::ParentDir => {
                issues.push(issue(
                    at,
                    format!("`path` must stay inside the workspace, but `{path}` contains `..`"),
                ));
                return;
            }
            // `\\server\share` and `C:` are absolute-ish on Windows; `Path::is_absolute`
            // does not catch a bare prefix or a root with no prefix.
            Component::Prefix(_) | Component::RootDir => {
                issues.push(issue(at, format!("`path` must be relative, but `{path}` is rooted")));
                return;
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
}

fn check_ref(git_ref: &str, at: &str, issues: &mut Vec<Issue>) {
    if git_ref.trim().is_empty() {
        issues.push(issue(at, "`ref` is empty"));
    } else if git_ref.starts_with('-') {
        // Would be read by `git clone --branch` as an option.
        issues.push(issue(at, format!("`ref` must not start with '-': `{git_ref}`")));
    } else if git_ref.chars().any(char::is_whitespace) {
        issues.push(issue(at, format!("`ref` must not contain whitespace: `{git_ref}`")));
    }
}

/// Compare destinations by path components, so `./a` and `a` are recognised as one place.
fn normalise_destination(path: &str) -> String {
    Path::new(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use crate::manifest::{Manifest, ManifestError};

    /// Build a manifest whose `[products.id].repositories` is the given TOML array body.
    fn with_repositories(repositories: &str) -> Result<Manifest, ManifestError> {
        let text = format!(
            r#"
schema = 1
[remote]
host = "github.com"
owner = "qeetgroup"
protocol = "ssh"
[products.id]
name = "Qeet ID"
repositories = [{repositories}]
"#
        );
        Manifest::load(&text, "test")
    }

    fn issues_of(result: Result<Manifest, ManifestError>) -> String {
        match result {
            Ok(_) => panic!("expected validation to fail"),
            Err(err @ ManifestError::Invalid { .. }) => err.to_string(),
            Err(other) => panic!("expected a validation failure, got: {other}"),
        }
    }

    #[test]
    fn accepts_a_well_formed_product() {
        with_repositories(r#"{ name = "a" }, { name = "b" }"#).expect("should be valid");
    }

    #[test]
    fn rejects_an_empty_repository_list() {
        let message = issues_of(with_repositories(""));
        assert!(message.contains("`repositories` is empty"), "{message}");
    }

    #[test]
    fn rejects_duplicate_repository_names() {
        let message = issues_of(with_repositories(r#"{ name = "a" }, { name = "a" }"#));
        assert!(message.contains("duplicate repository `a`"), "{message}");
    }

    #[test]
    fn rejects_an_empty_repository_name() {
        let message = issues_of(with_repositories(r#"{ name = "" }"#));
        assert!(message.contains("`name` is empty"), "{message}");
    }

    #[test]
    fn rejects_dangerous_urls() {
        let message = issues_of(with_repositories(r#"{ name = "a", url = "ext::sh" }"#));
        assert!(message.contains("remote-helper transport"), "{message}");

        let message = issues_of(with_repositories(r#"{ name = "a", url = "--upload-pack=x" }"#));
        assert!(message.contains("starts with '-'"), "{message}");

        let message = issues_of(with_repositories(r#"{ name = "a", url = "ftp://h/r.git" }"#));
        assert!(message.contains("unsupported transport"), "{message}");
    }

    #[test]
    fn rejects_absolute_and_escaping_paths() {
        let message = issues_of(with_repositories(r#"{ name = "a", path = "/etc/passwd" }"#));
        assert!(message.contains("must be relative"), "{message}");

        let message = issues_of(with_repositories(r#"{ name = "a", path = "../outside" }"#));
        assert!(message.contains("contains `..`"), "{message}");

        let message = issues_of(with_repositories(r#"{ name = "a", path = "nested/../../out" }"#));
        assert!(message.contains("contains `..`"), "{message}");
    }

    #[test]
    fn rejects_colliding_destinations() {
        let message = issues_of(with_repositories(
            r#"{ name = "a", path = "shared" }, { name = "b", path = "shared" }"#,
        ));
        assert!(message.contains("same destination `shared`"), "{message}");
        assert!(message.contains("a, b"), "should name both sides: {message}");
    }

    #[test]
    fn detects_a_collision_between_a_path_and_a_bare_name() {
        let message =
            issues_of(with_repositories(r#"{ name = "a" }, { name = "b", path = "./a" }"#));
        assert!(message.contains("same destination `a`"), "{message}");
    }

    #[test]
    fn rejects_a_bad_ref() {
        let message = issues_of(with_repositories(r#"{ name = "a", ref = "--exec=x" }"#));
        assert!(message.contains("must not start with '-'"), "{message}");
    }

    #[test]
    fn rejects_a_non_canonical_product_key() {
        let text = r#"
schema = 1
[remote]
host = "github.com"
owner = "qeetgroup"
protocol = "ssh"
[products.ID]
name = "Qeet ID"
repositories = [{ name = "a" }]
"#;
        let message = issues_of(Manifest::load(text, "test"));
        assert!(message.contains("must be lowercase"), "{message}");
    }

    #[test]
    fn reports_every_problem_at_once() {
        let message = issues_of(with_repositories(
            r#"{ name = "a", url = "ext::sh" }, { name = "a", path = "/abs" }"#,
        ));
        assert!(message.contains("remote-helper"), "{message}");
        assert!(message.contains("duplicate repository"), "{message}");
        assert!(message.contains("must be relative"), "{message}");
    }
}
