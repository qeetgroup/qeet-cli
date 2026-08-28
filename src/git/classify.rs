//! Turning a git failure into something a developer can act on.
//!
//! git reports almost everything as exit code 128, so the exit code alone is close to
//! useless -- `Error: process exited with code 128` is exactly the message this module
//! exists to avoid. The classification comes from git's stderr, and it decides two things:
//! what the user is told, and whether a retry could possibly help.

/// What went wrong, in categories that imply different responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Credentials or key were rejected, or git had no way to ask for them.
    Auth,
    /// The remote says there is no such repository -- which GitHub also reports when you
    /// simply cannot see a private one.
    NotFound,
    /// The repository is there but the requested branch or tag is not.
    InvalidRef,
    /// Network or transport trouble that might not happen again.
    Transient,
    /// Something local blocked the clone, e.g. the destination is not empty.
    Workspace,
    /// Unrecognised. Reported verbatim and never retried.
    Unknown,
    /// git could not be started at all.
    Spawn,
}

impl FailureKind {
    pub fn summary(self) -> &'static str {
        match self {
            Self::Auth => "Git authentication failed.",
            Self::NotFound => "The repository does not exist, or you cannot access it.",
            Self::InvalidRef => "The requested branch or tag does not exist on the remote.",
            Self::Transient => "The connection to the remote failed.",
            Self::Workspace => "Git refused to write to the destination.",
            Self::Unknown => "Git failed.",
            Self::Spawn => "Git could not be started.",
        }
    }

    pub fn guidance(self) -> &'static [&'static str] {
        match self {
            Self::Auth => &[
                "Check your SSH key or credential helper for this host.",
                "For SSH, `ssh -T git@github.com` should greet you by username.",
                "qeet runs git non-interactively, so git cannot prompt for a password.",
            ],
            Self::NotFound => &[
                "Confirm the repository name in the manifest.",
                "If it is private, confirm your account has access to it.",
            ],
            Self::InvalidRef => &["Check the `ref` for this repository in the manifest."],
            Self::Transient => &[
                "Check your network connection or proxy configuration.",
                "This kind of failure is retried automatically; it did not recover.",
            ],
            Self::Workspace => &["Inspect the destination directory, then run qeet again."],
            Self::Unknown => &["The git output above is the whole story qeet has."],
            Self::Spawn => &["Install git and make sure it is on your PATH."],
        }
    }

    /// Only transient transport failures are worth another attempt. Retrying an
    /// authentication failure or a missing repository just wastes the developer's time and
    /// hammers the remote.
    pub fn retryable(self) -> bool {
        matches!(self, Self::Transient)
    }
}

/// Longest, most specific patterns first: `could not read Username` also contains
/// `Username`, and an auth diagnosis is more useful than a generic one.
const PATTERNS: &[(&str, FailureKind)] = &[
    // --- authentication and authorisation ---
    ("permission denied (publickey)", FailureKind::Auth),
    ("permission denied (password", FailureKind::Auth),
    ("authentication failed", FailureKind::Auth),
    ("could not read username", FailureKind::Auth),
    ("could not read password", FailureKind::Auth),
    ("terminal prompts disabled", FailureKind::Auth),
    ("host key verification failed", FailureKind::Auth),
    ("permission denied, please try again", FailureKind::Auth),
    ("invalid username or password", FailureKind::Auth),
    ("access denied", FailureKind::Auth),
    ("403 forbidden", FailureKind::Auth),
    ("you do not have permission", FailureKind::Auth),
    // --- missing repository ---
    ("repository not found", FailureKind::NotFound),
    ("does not appear to be a git repository", FailureKind::NotFound),
    ("not found: did you run git update-server-info", FailureKind::NotFound),
    ("no such file or directory", FailureKind::NotFound),
    ("404 not found", FailureKind::NotFound),
    // --- missing ref ---
    ("remote branch", FailureKind::InvalidRef),
    ("could not find remote ref", FailureKind::InvalidRef),
    ("couldn't find remote ref", FailureKind::InvalidRef),
    // --- transient transport trouble ---
    ("could not resolve host", FailureKind::Transient),
    ("could not resolve hostname", FailureKind::Transient),
    ("connection reset by peer", FailureKind::Transient),
    ("connection timed out", FailureKind::Transient),
    ("connection refused", FailureKind::Transient),
    ("connection closed by remote host", FailureKind::Transient),
    ("operation timed out", FailureKind::Transient),
    ("timed out", FailureKind::Transient),
    ("network is unreachable", FailureKind::Transient),
    ("temporary failure in name resolution", FailureKind::Transient),
    ("the remote end hung up unexpectedly", FailureKind::Transient),
    ("rpc failed", FailureKind::Transient),
    ("early eof", FailureKind::Transient),
    ("unexpected disconnect", FailureKind::Transient),
    ("ssl_read", FailureKind::Transient),
    ("gnutls_handshake() failed", FailureKind::Transient),
    ("openssl ssl_read", FailureKind::Transient),
    ("502 bad gateway", FailureKind::Transient),
    ("503 service unavailable", FailureKind::Transient),
    ("504 gateway", FailureKind::Transient),
    ("failed to connect to", FailureKind::Transient),
    // --- local destination trouble ---
    ("already exists and is not an empty directory", FailureKind::Workspace),
    ("permission denied (os error 13)", FailureKind::Workspace),
    ("no space left on device", FailureKind::Workspace),
    ("read-only file system", FailureKind::Workspace),
];

/// Classify a git failure from its stderr.
///
/// Matching is case-insensitive because git's wording varies across versions, platforms and
/// the `ssh` implementation in use.
pub fn classify(stderr: &str) -> FailureKind {
    let haystack = stderr.to_ascii_lowercase();

    // An explicit "remote branch X not found" is an InvalidRef, but the bare word
    // "remote branch" also appears in unrelated advice, so require the negative too.
    for (pattern, kind) in PATTERNS {
        if !haystack.contains(pattern) {
            continue;
        }
        if *kind == FailureKind::InvalidRef
            && *pattern == "remote branch"
            && !haystack.contains("not found")
        {
            continue;
        }
        return *kind;
    }

    FailureKind::Unknown
}

/// The lines of git's stderr worth putting in front of a developer.
///
/// git is chatty, and `--progress` fills stderr with carriage-return progress updates.
/// Showing all of it for every failed repository would bury the one line that matters.
pub fn relevant_stderr(stderr: &str) -> String {
    const MAX_LINES: usize = 6;
    const PREFIXES: &[&str] = &[
        "fatal:",
        "error:",
        "warning:",
        "remote:",
        "ssh:",
        "git:",
        "permission denied",
        "could not",
        "unable to",
        "access denied",
        "host key",
    ];

    let interesting: Vec<&str> = stderr
        // Progress updates are separated by '\r', so split on both.
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            let lowered = line.to_ascii_lowercase();
            PREFIXES.iter().any(|prefix| lowered.starts_with(prefix))
        })
        .collect();

    // Nothing matched: fall back to the last few non-empty lines rather than showing
    // nothing at all, because an unrecognised failure is exactly when the raw text matters.
    let lines: Vec<&str> = if interesting.is_empty() {
        let all: Vec<&str> =
            stderr.split(['\n', '\r']).map(str::trim).filter(|line| !line.is_empty()).collect();
        all.iter().rev().take(MAX_LINES).rev().copied().collect()
    } else {
        interesting.into_iter().take(MAX_LINES).collect()
    };

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real messages, as git and ssh actually emit them.
    #[test]
    fn classifies_authentication_failures() {
        for stderr in [
            "git@github.com: Permission denied (publickey).\r\nfatal: Could not read from remote repository.",
            "fatal: Authentication failed for 'https://github.com/qeetgroup/qeet-id-server.git/'",
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
            "Host key verification failed.\nfatal: Could not read from remote repository.",
            "remote: HTTP Basic: Access denied",
        ] {
            assert_eq!(classify(stderr), FailureKind::Auth, "{stderr}");
        }
    }

    #[test]
    fn classifies_missing_repositories() {
        for stderr in [
            "remote: Repository not found.\nfatal: repository 'https://github.com/qeetgroup/nope.git/' not found",
            "fatal: '/tmp/nope.git' does not appear to be a git repository",
        ] {
            assert_eq!(classify(stderr), FailureKind::NotFound, "{stderr}");
        }
    }

    #[test]
    fn classifies_missing_refs() {
        for stderr in [
            "fatal: Remote branch nope not found in upstream origin",
            "fatal: couldn't find remote ref refs/heads/nope",
        ] {
            assert_eq!(classify(stderr), FailureKind::InvalidRef, "{stderr}");
        }
    }

    #[test]
    fn classifies_transient_transport_failures() {
        for stderr in [
            "ssh: Could not resolve hostname github.com: nodename nor servname provided",
            "fatal: unable to access 'https://github.com/x.git/': Failed to connect to github.com port 443: Operation timed out",
            "error: RPC failed; curl 56 Recv failure: Connection reset by peer\nfatal: early EOF",
            "fatal: The remote end hung up unexpectedly",
        ] {
            assert_eq!(classify(stderr), FailureKind::Transient, "{stderr}");
        }
    }

    #[test]
    fn classifies_destination_problems() {
        let stderr = "fatal: destination path 'qeet-id-server' already exists and is not an empty directory.";
        assert_eq!(classify(stderr), FailureKind::Workspace);
    }

    #[test]
    fn unrecognised_output_is_unknown() {
        assert_eq!(classify("fatal: something nobody has seen before"), FailureKind::Unknown);
        assert_eq!(classify(""), FailureKind::Unknown);
    }

    #[test]
    fn only_transient_failures_are_retried() {
        assert!(FailureKind::Transient.retryable());
        for kind in [
            FailureKind::Auth,
            FailureKind::NotFound,
            FailureKind::InvalidRef,
            FailureKind::Workspace,
            FailureKind::Unknown,
            FailureKind::Spawn,
        ] {
            assert!(!kind.retryable(), "{kind:?} must not be retried");
        }
    }

    #[test]
    fn every_kind_has_a_summary_and_guidance() {
        for kind in [
            FailureKind::Auth,
            FailureKind::NotFound,
            FailureKind::InvalidRef,
            FailureKind::Transient,
            FailureKind::Workspace,
            FailureKind::Unknown,
            FailureKind::Spawn,
        ] {
            assert!(!kind.summary().is_empty(), "{kind:?}");
            assert!(!kind.guidance().is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn extracts_the_useful_lines_and_drops_progress_noise() {
        let stderr = "Cloning into 'x'...\rreceiving objects:  10% (1/10)\rreceiving objects: 100% (10/10)\r\nremote: Repository not found.\nfatal: repository not found";
        let relevant = relevant_stderr(stderr);
        assert!(relevant.contains("remote: Repository not found."), "{relevant}");
        assert!(relevant.contains("fatal: repository not found"), "{relevant}");
        assert!(!relevant.contains("receiving objects"), "{relevant}");
    }

    #[test]
    fn falls_back_to_raw_output_when_nothing_matches() {
        // An unrecognised failure is exactly when hiding git's words would be worst.
        let relevant = relevant_stderr("something\nnobody\nrecognises");
        assert!(relevant.contains("recognises"), "{relevant}");
    }

    #[test]
    fn caps_how_much_is_shown() {
        let stderr = (0..50).map(|i| format!("fatal: line {i}")).collect::<Vec<_>>().join("\n");
        assert!(relevant_stderr(&stderr).lines().count() <= 6);
    }
}
