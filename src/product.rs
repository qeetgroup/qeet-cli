//! Turning `id` into Qeet ID.
//!
//! Product keys are the user-facing vocabulary. Lookup is case-insensitive on the way in,
//! because `qeet clone ID` meaning something different from `qeet clone id` would be a
//! trap, while the manifest itself is held to canonical lowercase by validation.

use crate::manifest::{Manifest, Product};

/// No product matched what the user asked for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", render(self))]
pub struct UnknownProduct {
    pub requested: String,
    pub available: Vec<String>,
    /// A close match, when there is exactly one worth proposing.
    pub suggestion: Option<String>,
}

fn render(err: &UnknownProduct) -> String {
    let mut message = format!("Unknown product: {}\n", err.requested);
    if let Some(suggestion) = &err.suggestion {
        message.push_str(&format!("\nDid you mean `{suggestion}`?\n"));
    }
    message.push_str("\nAvailable products:\n");
    for key in &err.available {
        message.push_str(&format!("  {key}\n"));
    }
    message.trim_end().to_string()
}

/// Resolve a product key against a manifest.
pub fn resolve<'m>(manifest: &'m Manifest, requested: &str) -> Result<&'m Product, UnknownProduct> {
    let normalised = requested.trim().to_ascii_lowercase();

    if let Some(product) = manifest.products.get(normalised.as_str()) {
        return Ok(product);
    }

    Err(UnknownProduct {
        requested: requested.to_string(),
        available: manifest.product_keys().map(str::to_string).collect(),
        suggestion: closest(&normalised, manifest.product_keys()),
    })
}

/// The nearest product key, if one is near enough to be worth suggesting.
///
/// Hand-rolled rather than pulling in a string-similarity crate for twenty lines.
fn closest<'a>(requested: &str, candidates: impl Iterator<Item = &'a str>) -> Option<String> {
    if requested.is_empty() {
        return None;
    }

    // Tolerate roughly a third of the word being wrong, and never more than three edits --
    // beyond that a "did you mean" is noise rather than help.
    let budget = (requested.chars().count() / 3).clamp(1, 3);

    candidates
        .map(|candidate| (levenshtein(requested, candidate), candidate))
        .filter(|(distance, _)| *distance <= budget)
        .min_by_key(|(distance, candidate)| (*distance, candidate.len()))
        .map(|(_, candidate)| candidate.to_string())
}

/// Levenshtein edit distance, two rows at a time.
fn levenshtein(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];

    for (i, l) in left.chars().enumerate() {
        current[0] = i + 1;
        for (j, r) in right.iter().enumerate() {
            let substitution = previous[j] + usize::from(l != *r);
            let insertion = current[j] + 1;
            let deletion = previous[j + 1] + 1;
            current[j + 1] = substitution.min(insertion).min(deletion);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        let text = r#"
schema = 1
[remote]
host = "github.com"
owner = "qeetgroup"
protocol = "ssh"
[products.id]
name = "Qeet ID"
repositories = [{ name = "qeet-id-server" }]
[products.pay]
name = "Qeet Pay"
repositories = [{ name = "qeet-pay-server" }]
[products.people]
name = "Qeet People"
repositories = [{ name = "qeet-people-server" }]
"#;
        Manifest::load(text, "test").expect("fixture must be valid")
    }

    #[test]
    fn resolves_a_known_product() {
        let manifest = manifest();
        let product = resolve(&manifest, "id").expect("id should resolve");
        assert_eq!(product.name, "Qeet ID");
        assert_eq!(product.repositories[0].name, "qeet-id-server");
    }

    #[test]
    fn resolution_is_case_insensitive_and_trims() {
        let manifest = manifest();
        for input in ["ID", "Id", "  id  ", "iD"] {
            let product = resolve(&manifest, input).unwrap_or_else(|_| panic!("{input}"));
            assert_eq!(product.name, "Qeet ID", "{input}");
        }
    }

    #[test]
    fn an_unknown_product_lists_what_is_available() {
        let manifest = manifest();
        let err = resolve(&manifest, "xyz").expect_err("xyz must not resolve");
        assert_eq!(err.requested, "xyz");
        assert_eq!(err.available, vec!["id", "pay", "people"]);

        let message = err.to_string();
        assert!(message.contains("Unknown product: xyz"), "{message}");
        assert!(message.contains("Available products:"), "{message}");
        assert!(message.contains("  people"), "{message}");
    }

    #[test]
    fn suggests_a_near_miss() {
        let manifest = manifest();
        let err = resolve(&manifest, "poeple").expect_err("typo must not resolve");
        assert_eq!(err.suggestion.as_deref(), Some("people"));
        assert!(err.to_string().contains("Did you mean `people`?"), "{err}");
    }

    #[test]
    fn does_not_suggest_something_unrelated() {
        let manifest = manifest();
        let err = resolve(&manifest, "kubernetes").expect_err("must not resolve");
        assert_eq!(err.suggestion, None);
        assert!(!err.to_string().contains("Did you mean"), "{err}");
    }

    #[test]
    fn short_keys_still_tolerate_one_edit() {
        let manifest = manifest();
        // budget clamps to 1 for a two-character word, so "pd" -> "id" is offered...
        let err = resolve(&manifest, "pd").expect_err("must not resolve");
        assert!(err.suggestion.is_some(), "expected a suggestion for `pd`");
        // ...but two edits away from everything is not.
        let err = resolve(&manifest, "zz").expect_err("must not resolve");
        assert_eq!(err.suggestion, None);
    }

    #[test]
    fn levenshtein_matches_known_distances() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("id", "id"), 0);
        assert_eq!(levenshtein("", "pay"), 3);
        assert_eq!(levenshtein("pay", ""), 3);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("poeple", "people"), 2);
    }
}
