//! Repository URLs: derivation, transport validation and identity comparison.
//!
//! Every URL here eventually becomes an argument to an external `git` process, so this
//! module is a security boundary rather than a formatting helper. See [`validate`].

use std::fmt;

use serde::Deserialize;

/// Transports `qeet` is willing to hand to `git`.
///
/// Deliberately short. `git` itself accepts far more, including remote-helper transports
/// of the form `<helper>::<address>` -- and `ext::` runs an arbitrary command, which would
/// turn a manifest into remote code execution. Anything not listed here is refused.
const ALLOWED_SCHEMES: &[&str] = &["https", "http", "ssh", "git", "file"];

/// Git transport used when a repository does not override its URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum Protocol {
    Ssh,
    Https,
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ssh => "ssh",
            Self::Https => "https",
        })
    }
}

/// Why a repository URL was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlError {
    #[error("the URL is empty")]
    Empty,

    #[error("the URL starts with '-', which `git` would read as an option")]
    LeadsWithDash,

    #[error("the URL contains whitespace")]
    Whitespace,

    #[error("`{helper}::` is a git remote-helper transport, which can execute arbitrary commands")]
    RemoteHelper { helper: String },

    #[error("unsupported transport `{scheme}` (allowed: {allowed})", allowed = ALLOWED_SCHEMES.join(", "))]
    UnsupportedScheme { scheme: String },

    #[error("the URL is not a recognised git URL, scp-style address or absolute path")]
    Unrecognised,
}

/// Validate a URL before it is ever passed to `git`.
///
/// Rejects, in order: empty input, a leading `-` (which `git` would parse as an option such
/// as `--upload-pack=`), whitespace, remote-helper transports like `ext::`, unknown schemes,
/// and anything that is not a recognisable git address.
pub fn validate(raw: &str) -> Result<(), UrlError> {
    if raw.is_empty() {
        return Err(UrlError::Empty);
    }
    if raw.starts_with('-') {
        return Err(UrlError::LeadsWithDash);
    }
    if raw.chars().any(char::is_whitespace) {
        return Err(UrlError::Whitespace);
    }

    // The authority region is everything before the first '/'. A remote-helper prefix
    // (`ext::`, `transport::`) always appears there, whereas a legitimate "::" can only
    // occur later -- inside a bracketed IPv6 host or a path.
    let authority = raw.split('/').next().unwrap_or(raw);
    if let Some(index) = authority.find("::") {
        if !authority[..index].contains('[') {
            return Err(UrlError::RemoteHelper { helper: authority[..index].to_string() });
        }
    }

    match raw.find("://") {
        Some(index) => {
            let scheme = &raw[..index];
            let well_formed = !scheme.is_empty()
                && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
            if !well_formed {
                return Err(UrlError::Unrecognised);
            }
            let scheme = scheme.to_ascii_lowercase();
            if !ALLOWED_SCHEMES.contains(&scheme.as_str()) {
                return Err(UrlError::UnsupportedScheme { scheme });
            }
            // A scheme with no address after it is not usable.
            if raw[index + 3..].is_empty() {
                return Err(UrlError::Unrecognised);
            }
            Ok(())
        }
        // scp-style `[user@]host:path`, which git accepts and which has no scheme.
        None if is_scp_style(raw) => Ok(()),
        // A local path, which git clones as if it were `file://`.
        None if is_local_path(raw) => Ok(()),
        None => Err(UrlError::Unrecognised),
    }
}

/// `[user@]host:path` -- a colon before any slash, with a non-empty host and path.
fn is_scp_style(raw: &str) -> bool {
    let Some((authority, path)) = raw.split_once(':') else {
        return false;
    };
    if authority.is_empty() || path.is_empty() || authority.contains('/') {
        return false;
    }
    // A bare `C:\path` on Windows is a local path, not a host called "C".
    !(authority.len() == 1 && authority.chars().all(char::is_alphabetic))
}

fn is_local_path(raw: &str) -> bool {
    raw.starts_with('/')
        || raw.starts_with("./")
        || raw.starts_with("../")
        || raw.starts_with('\\')
        // Windows drive-absolute: C:\repo or C:/repo
        || matches!(raw.as_bytes(), [c, b':', b'/' | b'\\', ..] if c.is_ascii_alphabetic())
}

/// The remote a manifest derives clone URLs from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Remote {
    pub host: String,
    pub owner: String,
    pub protocol: Protocol,
}

impl Remote {
    /// Derive the clone URL for a repository name under the given transport.
    ///
    /// `protocol` is passed in rather than read from `self` so a per-run `--protocol`
    /// override needs no mutation of the parsed manifest.
    pub fn url_for(&self, name: &str, protocol: Protocol) -> String {
        match protocol {
            Protocol::Ssh => format!("git@{}:{}/{}.git", self.host, self.owner, name),
            Protocol::Https => format!("https://{}/{}/{}.git", self.host, self.owner, name),
        }
    }

    /// The transport a run should use: the `--protocol` override, else the manifest default.
    pub fn effective_protocol(&self, requested: Option<Protocol>) -> Protocol {
        requested.unwrap_or(self.protocol)
    }
}

/// A repository URL reduced to the identity it points at.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    /// Lowercased; empty for local paths, which have no host.
    host: String,
    /// Leading and trailing `/` and a trailing `.git` removed.
    path: String,
}

/// Result of comparing two URLs. Anything other than [`Same`](UrlMatch::Same) is treated as
/// a reason not to touch an existing directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlMatch {
    /// Both URLs confidently name the same repository.
    Same,
    /// Both URLs were understood and name different repositories.
    Different,
    /// At least one URL could not be reduced to an identity, so no claim is made.
    Indeterminate,
}

/// Compare two repository URLs semantically.
///
/// `git@github.com:qeetgroup/x.git` and `https://github.com/qeetgroup/x.git` are the same
/// repository, so raw string equality is not good enough. Where a URL cannot be reduced
/// with confidence the answer is [`UrlMatch::Indeterminate`] -- never a guess, because
/// wrongly concluding "same" is what would put a developer's existing work at risk.
pub fn compare(left: &str, right: &str) -> UrlMatch {
    match (identity(left), identity(right)) {
        (Some(l), Some(r)) if l == r => UrlMatch::Same,
        (Some(_), Some(_)) => UrlMatch::Different,
        _ => UrlMatch::Indeterminate,
    }
}

fn identity(raw: &str) -> Option<Identity> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    let (host, path) = if let Some(index) = raw.find("://") {
        let scheme = raw[..index].to_ascii_lowercase();
        let rest = &raw[index + 3..];
        if scheme == "file" {
            // file:///path -> no host, absolute path.
            ("", rest.trim_start_matches('/'))
        } else {
            let (authority, path) = rest.split_once('/')?;
            (strip_userinfo_and_port(authority), path)
        }
    } else if is_scp_style(raw) {
        let (authority, path) = raw.split_once(':')?;
        (strip_userinfo_and_port(authority), path)
    } else if is_local_path(raw) {
        ("", raw)
    } else {
        return None;
    };

    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty() {
        return None;
    }

    Some(Identity { host: host.to_ascii_lowercase(), path: path.to_string() })
}

/// `git@github.com:2222` -> `github.com`. Bracketed IPv6 hosts keep their brackets.
fn strip_userinfo_and_port(authority: &str) -> &str {
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if host.starts_with('[') {
        return host.split_once(']').map_or(host, |(h, _)| &host[..h.len() + 1]);
    }
    host.split_once(':').map_or(host, |(h, _)| h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote() -> Remote {
        Remote { host: "github.com".into(), owner: "qeetgroup".into(), protocol: Protocol::Ssh }
    }

    #[test]
    fn derives_ssh_and_https_urls() {
        assert_eq!(
            remote().url_for("qeet-id-server", Protocol::Ssh),
            "git@github.com:qeetgroup/qeet-id-server.git"
        );
        assert_eq!(
            remote().url_for("qeet-id-server", Protocol::Https),
            "https://github.com/qeetgroup/qeet-id-server.git"
        );
    }

    #[test]
    fn derived_urls_are_always_valid() {
        for protocol in [Protocol::Ssh, Protocol::Https] {
            let url = remote().url_for("qeet-id-server", protocol);
            assert_eq!(validate(&url), Ok(()), "{url}");
        }
    }

    #[test]
    fn accepts_the_transports_git_developers_actually_use() {
        for url in [
            "https://github.com/qeetgroup/qeet-id-server.git",
            "http://internal.example/repo.git",
            "ssh://git@github.com/qeetgroup/qeet-id-server.git",
            "ssh://git@github.com:2222/qeetgroup/repo.git",
            "git://example.com/repo.git",
            "file:///tmp/fixtures/repo.git",
            "git@github.com:qeetgroup/qeet-id-server.git",
            "/tmp/fixtures/repo.git",
            "./relative/repo.git",
            "C:/repos/repo.git",
        ] {
            assert_eq!(validate(url), Ok(()), "should accept {url}");
        }
    }

    #[test]
    fn rejects_remote_helper_transports() {
        // The reason this module exists: `ext::` hands a command line to a shell.
        assert_eq!(validate("ext::sh"), Err(UrlError::RemoteHelper { helper: "ext".into() }));
        assert_eq!(
            validate("transport::whatever"),
            Err(UrlError::RemoteHelper { helper: "transport".into() })
        );
    }

    #[test]
    fn rejects_option_injection() {
        assert_eq!(validate("--upload-pack=touch /tmp/pwned"), Err(UrlError::LeadsWithDash));
        assert_eq!(validate("-c"), Err(UrlError::LeadsWithDash));
    }

    #[test]
    fn rejects_whitespace_and_empty() {
        assert_eq!(validate(""), Err(UrlError::Empty));
        assert_eq!(validate("ext::sh -c 'echo pwned'"), Err(UrlError::Whitespace));
        assert_eq!(validate("https://example.com/a b.git"), Err(UrlError::Whitespace));
    }

    #[test]
    fn rejects_unknown_schemes() {
        assert_eq!(
            validate("ftp://example.com/repo.git"),
            Err(UrlError::UnsupportedScheme { scheme: "ftp".into() })
        );
        assert_eq!(
            validate("javascript://x/y"),
            Err(UrlError::UnsupportedScheme { scheme: "javascript".into() })
        );
    }

    #[test]
    fn rejects_unrecognised_addresses() {
        assert_eq!(validate("not-a-url"), Err(UrlError::Unrecognised));
        assert_eq!(validate("https://"), Err(UrlError::Unrecognised));
    }

    #[test]
    fn ipv6_hosts_are_not_mistaken_for_remote_helpers() {
        assert_eq!(validate("https://[::1]/repo.git"), Ok(()));
        assert_eq!(validate("ssh://git@[fe80::1]:22/qeetgroup/repo.git"), Ok(()));
    }

    #[test]
    fn ssh_and_https_forms_of_one_repository_are_the_same() {
        for other in [
            "https://github.com/qeetgroup/qeet-id-server.git",
            "https://github.com/qeetgroup/qeet-id-server",
            "ssh://git@github.com/qeetgroup/qeet-id-server.git",
            "https://GitHub.com/qeetgroup/qeet-id-server.git",
            "git@github.com:qeetgroup/qeet-id-server",
        ] {
            assert_eq!(
                compare("git@github.com:qeetgroup/qeet-id-server.git", other),
                UrlMatch::Same,
                "{other}"
            );
        }
    }

    #[test]
    fn different_repositories_are_different() {
        for other in [
            "git@github.com:qeetgroup/qeet-pay-server.git",
            "git@github.com:someone-else/qeet-id-server.git",
            "git@gitlab.com:qeetgroup/qeet-id-server.git",
            "https://github.com/qeetgroup/nested/qeet-id-server.git",
        ] {
            assert_eq!(
                compare("git@github.com:qeetgroup/qeet-id-server.git", other),
                UrlMatch::Different,
                "{other}"
            );
        }
    }

    #[test]
    fn a_port_does_not_change_repository_identity() {
        assert_eq!(
            compare(
                "ssh://git@github.com:2222/qeetgroup/repo.git",
                "git@github.com:qeetgroup/repo.git"
            ),
            UrlMatch::Same
        );
    }

    #[test]
    fn local_paths_and_file_urls_agree() {
        assert_eq!(compare("file:///tmp/fixtures/repo.git", "/tmp/fixtures/repo"), UrlMatch::Same);
    }

    #[test]
    fn unparseable_urls_yield_no_verdict() {
        // Never a guess: the caller must treat this as "do not touch that directory".
        assert_eq!(
            compare("git@github.com:qeetgroup/repo.git", "garbage"),
            UrlMatch::Indeterminate
        );
        assert_eq!(compare("", "git@github.com:qeetgroup/repo.git"), UrlMatch::Indeterminate);
    }
}
