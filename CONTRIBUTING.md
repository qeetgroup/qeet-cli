# Contributing to Qeet CLI

Organization-wide engineering standards — branching, commit format, review expectations —
live in `qeet-context/ENGINEERING-STANDARDS.md`. This file covers what is specific to this
repository.

## Getting set up

```bash
git clone git@github.com:qeetgroup/qeet-cli.git
cd qeet-cli
cargo build
cargo test
```

You need Rust 1.87 or newer and `git`. That is the whole list: no Docker, no services, no
credentials, no network. If `cargo test` needs anything else, that is a bug — please report
it.

## Before you open a pull request

Run the same four gates CI runs:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release --all-features
```

A warning is a failure here — `RUSTFLAGS: -D warnings` is set in CI.

## What a good change looks like

**New behaviour ships with tests.** A bug fix ships with a test that fails before it.

**Tests are deterministic and offline.** No network, no wall-clock dependence, no reliance on
your personal git credentials or on any GitHub repository existing. The existing integration
tests show the pattern: real bare repositories in a temporary directory, cloned over
`file://` by the real git executable.

**Do not use sleeps as proof of concurrency.** See
`src/clone/coordinator.rs::clones_really_run_concurrently_up_to_the_limit` — a barrier sized
to the concurrency limit means a sequential implementation stalls rather than passing.

**Read [AGENTS.md](AGENTS.md) first** if you are touching the git adapter, the URL allowlist,
or workspace preflight. Those three have invariants that are easy to break and expensive to
get wrong.

## Scope

v1 solves one problem: `qeet clone <product>`. `qeet status`, `pull`, `sync`, `graph` and
`dev` are deliberately absent, and a test asserts they stay absent.

That is not a permanent no. It is a "not until `qeet clone` is excellent". If you want to
propose one, open an issue describing the developer problem it solves before writing code —
a pull request adding a subcommand will be asked that question anyway.

Additions that are always welcome:

- A failure message that is clearer, or a `FailureKind` pattern for real git output that is
  currently classified `Unknown`.
- A workspace or path-safety case that is not yet covered.
- Removing a dependency, or simplifying something without changing behaviour.
- Documentation that was wrong or has gone stale.

## Reporting a bug

Include the `qeet --version` output, your `git --version`, your platform, the exact command,
and the output. If a manifest is involved, include it — with URLs redacted if they are
sensitive.

**Never paste a token, key, or credential into an issue.** Qeet CLI does not print them, but
git's output can contain a URL with a username in it.

## Security

Do not open a public issue for a security problem. See [SECURITY.md](SECURITY.md).
