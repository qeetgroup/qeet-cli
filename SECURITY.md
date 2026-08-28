# Security Policy

Organization-wide security requirements live in `qeet-context/SECURITY.md`. This file covers
Qeet CLI specifically.

## Reporting a vulnerability

**Do not open a public issue.** Report privately through GitHub's
[private vulnerability reporting](https://github.com/qeetgroup/qeet-cli/security/advisories/new)
for this repository.

Please include a description, reproduction steps, and the affected version. You will get an
acknowledgement, and an assessment once the report has been reviewed.

## What Qeet CLI does not do

By design, and worth stating because it defines the attack surface:

- It **stores no credentials** and reads no tokens.
- It **never prints** credentials, tokens or key material.
- It **does not modify** your SSH configuration, git credential configuration, or any git
  config file.
- It **does not send** repository URLs, product names or anything else to a network service.
  Qeet CLI is local-first: the only network traffic is git's own, to the remote you asked for.
- It contains **no `unsafe` code** (`#![forbid(unsafe_code)]`).
- It runs **no telemetry** and collects no analytics.

## The security-relevant boundaries

Three places in this codebase are load-bearing for security. Changes to them warrant a
careful review.

### Process execution — `src/git/mod.rs`, `src/git/client.rs`

git is invoked with an **argument vector**, never through a shell. Positional arguments always
follow `--`, so a URL or destination cannot be reinterpreted as an option such as
`--upload-pack=`.

### Transport allowlisting — `src/remote.rs`

Repository URLs come from a manifest, which may be supplied by `--manifest` or
`QEET_MANIFEST`, so they are treated as untrusted input.

Only `https`, `http`, `ssh`, `git` and `file` are permitted. Everything else is refused — in
particular git's remote-helper syntax `<helper>::<address>`, because **`ext::` causes git to
execute an arbitrary command**. Without that check a manifest would be a code execution
vector rather than configuration.

Also refused: any URL beginning with `-`, and any URL containing whitespace.

### Filesystem containment — `src/workspace.rs`

A destination must resolve inside the workspace. This is checked twice: syntactically during
manifest validation (absolute paths, `..`, Windows roots and prefixes), and against the
resolved filesystem during preflight, which is the only way to catch a symlinked parent
pointing out of the workspace. Comparison is component-wise, so a sibling sharing a name
prefix is not mistaken for a child.

Exactly one operation deletes anything: a destination that did not exist when the run
started, that this run created, and whose clone failed or was cancelled. Pre-existing
directories, successful siblings and parent directories are never removed.

## Non-interactive git

Qeet CLI sets `GIT_TERMINAL_PROMPT=0` on the git processes it spawns, and applies SSH batch
mode only when you have configured neither `GIT_SSH_COMMAND` nor `core.sshCommand`. This is
to prevent a stalled prompt from hanging a concurrent run, and to make authentication failure
explicit rather than silent.

**Known limitation:** a third-party credential helper that opens its own GUI is outside what
Qeet CLI can see or suppress.

## Supported versions

v1 is pre-1.0. Security fixes are made on the latest release; there are no maintained older
branches yet.
