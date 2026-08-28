//! Domain errors and the exit-code contract.
//!
//! Exit codes are a public interface -- scripts depend on them -- so they are defined in
//! one place and tested. git's own exit code is never re-used as ours: git reports almost
//! everything as 128, which would tell a caller nothing.

use std::process::ExitCode;

use crate::git::GitError;
use crate::manifest::ManifestError;
use crate::product::UnknownProduct;

/// Every repository reached the state the developer asked for.
pub const EXIT_SUCCESS: u8 = 0;
/// One or more repositories failed, or the run was cancelled.
pub const EXIT_INCOMPLETE: u8 = 1;
/// Command-line misuse. Produced by clap, listed here so the set is documented in one place.
pub const EXIT_USAGE: u8 = 2;
/// Configuration problem: an unusable manifest, an unknown product, or no usable git.
pub const EXIT_CONFIG: u8 = 3;

/// Something stopped `qeet` before it could report per-repository results.
///
/// A partially failed clone is *not* one of these -- it produces a report, and its exit
/// code comes from that report.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Manifest(#[from] ManifestError),

    #[error(transparent)]
    UnknownProduct(#[from] UnknownProduct),

    #[error(transparent)]
    Git(#[from] GitError),

    #[error("cannot use the current directory as a workspace: {source}")]
    Workspace {
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    /// All of these are configuration problems: the developer must change something before
    /// `qeet` can do any work at all.
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Manifest(_) | Self::UnknownProduct(_) | Self::Git(_) | Self::Workspace { .. } => {
                ExitCode::from(EXIT_CONFIG)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_codes_are_distinct() {
        let codes = [EXIT_SUCCESS, EXIT_INCOMPLETE, EXIT_USAGE, EXIT_CONFIG];
        let unique: std::collections::HashSet<u8> = codes.iter().copied().collect();
        assert_eq!(unique.len(), codes.len(), "exit codes must not overlap");
    }

    #[test]
    fn every_startup_error_exits_with_the_configuration_code() {
        let errors = [
            Error::Git(GitError::NotFound),
            Error::UnknownProduct(UnknownProduct {
                requested: "xyz".into(),
                available: vec!["id".into()],
                suggestion: None,
            }),
            Error::Manifest(ManifestError::Schema { found: 99 }),
            Error::Workspace { source: std::io::Error::other("nope") },
        ];

        for error in errors {
            assert_eq!(
                format!("{:?}", error.exit_code()),
                format!("{:?}", ExitCode::from(EXIT_CONFIG)),
                "{error}"
            );
        }
    }

    #[test]
    fn startup_errors_carry_a_message_worth_printing() {
        assert!(
            Error::Git(GitError::NotFound).to_string().contains("PATH"),
            "a missing git should say what to do"
        );
    }
}
