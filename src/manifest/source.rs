//! Where the manifest comes from.
//!
//! Precedence, first hit wins:
//!
//! 1. `--manifest <PATH>`
//! 2. the `QEET_MANIFEST` environment variable
//! 3. `<config-dir>/qeet/products.toml`
//! 4. the manifest embedded in this binary
//!
//! The embedded copy is why `qeet clone id` works immediately after installation, with no
//! setup and no network call. It is a *release-time snapshot*: when the organization gains
//! or loses a repository, either the CLI is updated or one of the three overrides is used.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use super::ManifestError;

/// Environment variable holding a manifest path.
pub const ENV_VAR: &str = "QEET_MANIFEST";

/// Directory created under the platform configuration directory.
const CONFIG_SUBDIR: &str = "qeet";

/// File name looked for in that directory.
const CONFIG_FILE: &str = "products.toml";

/// The manifest compiled into this binary.
pub const EMBEDDED: &str = include_str!("../../config/products.toml");

/// Which of the four tiers a manifest was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Flag(PathBuf),
    Environment(PathBuf),
    UserConfig(PathBuf),
    Embedded,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Flag(path) => write!(f, "--manifest {}", path.display()),
            Self::Environment(path) => write!(f, "{ENV_VAR}={}", path.display()),
            Self::UserConfig(path) => write!(f, "{}", path.display()),
            Self::Embedded => f.write_str("built-in manifest"),
        }
    }
}

/// Manifest text together with where it came from.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub origin: Origin,
    pub text: String,
}

/// Resolve the manifest using the real environment.
pub fn resolve(flag: Option<&Path>) -> Result<Loaded, ManifestError> {
    resolve_with(flag, std::env::var_os(ENV_VAR), user_config_path().as_deref())
}

/// The resolution rule, with its three inputs injected so it can be tested without
/// mutating process-wide environment state.
fn resolve_with(
    flag: Option<&Path>,
    env: Option<OsString>,
    user_config: Option<&Path>,
) -> Result<Loaded, ManifestError> {
    if let Some(path) = flag {
        return read(path).map(|text| Loaded { origin: Origin::Flag(path.to_path_buf()), text });
    }

    // An empty QEET_MANIFEST is treated as unset rather than as the current directory.
    if let Some(value) = env.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        return read(&path).map(|text| Loaded { origin: Origin::Environment(path), text });
    }

    if let Some(path) = user_config {
        // Absent is normal and silent. Present but unreadable is an error: falling back
        // would quietly ignore configuration the developer deliberately put there.
        if path.exists() {
            return read(path)
                .map(|text| Loaded { origin: Origin::UserConfig(path.to_path_buf()), text });
        }
    }

    Ok(Loaded { origin: Origin::Embedded, text: EMBEDDED.to_string() })
}

fn read(path: &Path) -> Result<String, ManifestError> {
    std::fs::read_to_string(path)
        .map_err(|source| ManifestError::Read { path: path.display().to_string(), source })
}

/// `<config-dir>/qeet/products.toml`, using each platform's native convention:
/// `~/Library/Application Support` on macOS, `%APPDATA%` on Windows, `$XDG_CONFIG_HOME`
/// on Linux. `None` when there is no home directory to derive it from, in which case the
/// tier is simply skipped -- a missing home directory should not stop a clone.
pub fn user_config_path() -> Option<PathBuf> {
    use etcetera::BaseStrategy as _;

    let strategy = etcetera::base_strategy::choose_native_strategy().ok()?;
    Some(strategy.config_dir().join(CONFIG_SUBDIR).join(CONFIG_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;

    /// The shipped manifest must always be loadable. If this fails, the release is broken:
    /// every installed binary would refuse to run.
    #[test]
    fn the_embedded_manifest_is_valid() {
        let manifest =
            Manifest::load(EMBEDDED, "embedded").expect("embedded manifest must be valid");
        assert!(
            manifest.products.len() >= 8,
            "expected the real product registry, found {} products",
            manifest.products.len()
        );
        assert!(manifest.products.contains_key("id"), "`id` must resolve");
    }

    #[test]
    fn falls_back_to_the_embedded_manifest() {
        let loaded = resolve_with(None, None, None).expect("should fall back");
        assert_eq!(loaded.origin, Origin::Embedded);
        assert_eq!(loaded.text, EMBEDDED);
    }

    #[test]
    fn an_empty_environment_variable_is_treated_as_unset() {
        let loaded = resolve_with(None, Some(OsString::new()), None).expect("should fall back");
        assert_eq!(loaded.origin, Origin::Embedded);
    }

    #[test]
    fn the_flag_wins_over_everything_else() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flag = dir.path().join("flag.toml");
        let env = dir.path().join("env.toml");
        let user = dir.path().join("user.toml");
        for (path, marker) in [(&flag, "flag"), (&env, "env"), (&user, "user")] {
            std::fs::write(path, format!("# {marker}")).expect("write");
        }

        let loaded = resolve_with(Some(&flag), Some(env.clone().into_os_string()), Some(&user))
            .expect("should read the flag");
        assert_eq!(loaded.origin, Origin::Flag(flag));
        assert_eq!(loaded.text, "# flag");
    }

    #[test]
    fn the_environment_wins_over_user_configuration() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env = dir.path().join("env.toml");
        let user = dir.path().join("user.toml");
        std::fs::write(&env, "# env").expect("write");
        std::fs::write(&user, "# user").expect("write");

        let loaded = resolve_with(None, Some(env.clone().into_os_string()), Some(&user))
            .expect("should read the environment");
        assert_eq!(loaded.origin, Origin::Environment(env));
        assert_eq!(loaded.text, "# env");
    }

    #[test]
    fn user_configuration_wins_over_embedded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let user = dir.path().join("user.toml");
        std::fs::write(&user, "# user").expect("write");

        let loaded = resolve_with(None, None, Some(&user)).expect("should read user config");
        assert_eq!(loaded.origin, Origin::UserConfig(user));
        assert_eq!(loaded.text, "# user");
    }

    #[test]
    fn a_missing_user_configuration_file_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let loaded = resolve_with(None, None, Some(&dir.path().join("absent.toml")))
            .expect("absent user config should fall through");
        assert_eq!(loaded.origin, Origin::Embedded);
    }

    #[test]
    fn an_explicitly_requested_manifest_that_is_missing_is_an_error() {
        // Silently falling back would hide the developer's mistake.
        let dir = tempfile::tempdir().expect("tempdir");
        let absent = dir.path().join("absent.toml");

        let err = resolve_with(Some(&absent), None, None).expect_err("flag must not fall back");
        assert!(matches!(err, ManifestError::Read { .. }), "{err}");

        let err = resolve_with(None, Some(absent.into_os_string()), None)
            .expect_err("env var must not fall back");
        assert!(matches!(err, ManifestError::Read { .. }), "{err}");
    }
}
