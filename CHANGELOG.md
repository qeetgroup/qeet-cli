# Changelog

All notable changes to Qeet CLI are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] — 2026-08-28

A product's repositories are now grouped on disk, and the CLI grew from one command to eight.

### Added

- **Grouping.** `qeet clone id` produces `qeet-id/qeet-id-server` rather than dropping twelve
  repositories into the current directory. The directory is manifest data, and is optional —
  `qeet clone group` stays flat, because `qeet-docs` and `qeet-apis` belong at the top of a
  workspace.
- **`qeet products`** — every product, its repository count and its group directory.
- **`qeet repos <product>`** — a product's repositories and their resolved clone URLs.
- **`qeet status <product>`** — per repository: branch, clean/dirty, ahead/behind, or not
  cloned. Read-only.
- **`qeet update <product>`** — fast-forwards only what is unambiguous, and skips and names
  everything else. `--dry-run` reports without fetching. See below.
- **`qeet doctor`** — checks git, the manifest in effect, workspace writability, which git
  identity you authenticate as, and **whether that identity can actually reach a configured
  repository**. That last check exists because a wrong SSH identity is indistinguishable from
  "repository not found".
- **`qeet self-update`** — works out how qeet was installed and hands over to that installer.
  It does not overwrite itself, which would leave Homebrew inconsistent.
- **`qeet clone all`** — every product, each with its own bounded concurrency.
- **Colour**, through one shared palette. Every coloured state also has a symbol and a word,
  so output survives being piped or screen-read. `NO_COLOR` is honoured.
- **An aggregate progress bar** with a live elapsed clock, under the per-repository spinners.

### Fixed

- **The aggregate progress bar never rendered.** Its template began with `\n`, and
  `MultiProgress` accounts for line count per bar, so a two-line bar silently drew nothing.
  Found by capturing real output through a pty: 0 bar glyphs before, 411 after. A test now
  asserts no `MultiProgress` template contains a newline.
- **A detached HEAD was considered fast-forwardable.** `fast_forwardable()` checked for an
  upstream and a behind-count but not for a branch, so a detached repository that was behind
  would have been merged into. Caught by its own test before it shipped.
- **Integration tests were reading the developer's own config.** The fixture removed
  `QEET_MANIFEST` but not the variables that locate the user config directory, so a config
  file in `$HOME` became a silent test input. The fixture now redirects `HOME`,
  `XDG_CONFIG_HOME` and `APPDATA`.
- **The documented macOS config path was wrong.** It is `~/Library/Preferences/qeet/`, not
  `~/Library/Application Support/qeet/` — `etcetera`'s Apple strategy puts `config_dir` in
  `Preferences`. `qeet doctor` now prints the resolved path so nobody has to trust the docs.

## [0.1.3] — 2026-08-28

### Changed

- **README is now visual.** Four Mermaid flowcharts — the clone pipeline, the workspace
  preflight decision tree, manifest source precedence, and the release flow — replace prose
  that asked the reader to hold a diagram in their head. Live badges replace static ones, so
  release, downloads, last-commit and CI state reflect reality.
- Dropped the problem/solution narrative in favour of a compact overview. The rationale still
  lives in [docs/architecture.md](docs/architecture.md), where it belongs.
- The platform table now distinguishes "built in CI" from "manually tested", rather than
  implying every target was run by hand.

## [0.1.2] — 2026-08-28

### Fixed

- **Documentation accuracy.** The README and ADR-016 claimed the install script touches only
  `~/.profile`. Testing the published installer against a sandboxed `HOME` showed it writes
  three shell configurations — `~/.profile`, `~/.zshrc` and
  `~/.config/fish/conf.d/` — plus an install receipt. Both documents now list every path.

### Added

- Releases now appear in the repository's **Deployments** panel, via a `release` environment
  on the publish job, matching how other Qeet repositories surface deployments.
- README: badges, contents, collapsible troubleshooting, measured concurrency figures, and a
  table of exactly what the install script writes.

### Verified

- Checksum verification **fails closed** — a byte-corrupted archive aborts the install.
- The Homebrew formula carries `sha256` for all four Unix targets.
- Known gap: the formula has no `test do` block, so `brew test qeet` reports "defines no test".

## [0.1.1] — 2026-08-28

**First published release.** `0.1.0` was developed and tagged nowhere: the distribution
pipeline landed in this version, and the automatic patch bump that pipeline introduced took
the version with it. Everything below shipped in `0.1.1`.

Solves one problem: cloning every repository of a Qeet product with one command.

### Distribution

- **Installable at last.** `brew install qeetgroup/tap/qeet`, a POSIX shell installer and a
  PowerShell installer, all fed from GitHub Releases. Installs to `~/.local/bin`; no root.
- **Five platforms** — macOS arm64/x86_64, Linux x86_64/arm64, Windows x86_64 — with
  per-archive and combined SHA-256 checksums, built and published by `cargo-dist` 0.32.0.
- **Automatic versioning**, following the qeetrix-icons convention: a PR bumps the patch
  version on its own branch, and merging to `main` releases it. A `vX.Y.Z` tag is created by
  the GitHub Release itself, so a tag only ever exists for a version whose builds passed.
- **Format, lint and tests gate the build**, so a tag on a failing commit cannot publish.
- Every third-party GitHub Action pinned to a commit SHA.

`brew install qeet` is **not** available: homebrew-core requires ≥75 stars / ≥30 forks / ≥30
watchers and refuses third-party prebuilt binaries. The tap command is the honest one.

### Added

- **`qeet clone <product>`** — resolves a product to its repositories and clones them
  concurrently through the developer's own `git` executable.
- **Data-driven product registry** (`config/products.toml`, embedded in the binary) covering
  16 products and 66 repositories, transcribed from the Qeet Group L0 repository registry and
  cross-checked against the organization. Adding a product requires no code change.
- **Manifest source precedence** — `--manifest`, then `QEET_MANIFEST`, then
  `<config-dir>/qeet/products.toml`, then the embedded registry. The embedded copy means the
  binary works with no setup and no network call.
- **Manifest validation** reporting every problem at once, with line and column for syntax
  errors: schema version, product keys and names, duplicate repositories, unknown fields,
  transport allowlisting, relative and contained paths, and colliding destinations.
- **Bounded concurrency** — `available_parallelism()` capped at 8 by default,
  `--concurrency <N>` within 1–64. No unlimited mode.
- **Workspace preflight** classifying every destination before any git process starts:
  create, fill-empty, already-present, or blocked. Nothing existing is deleted or
  overwritten, and a re-run of `qeet clone` is safe.
- **Semantic `origin` comparison**, so SSH and HTTPS forms of one repository are recognised as
  the same. Identity that cannot be established with confidence blocks rather than being
  assumed.
- **Path containment** against `..`, absolute paths, Windows roots and prefixes, symlinked
  parents and symlinked destinations.
- **Failure classification** from git's stderr into authentication, missing repository,
  missing ref, transient, workspace and unknown — driving both the message and whether a
  retry could help.
- **Conservative retry** — transient transport failures only, at most twice, ~500ms then
  ~1500ms with jitter.
- **Failure isolation** — one repository failing never cancels another; every repository
  appears in the report.
- **`Ctrl-C` cancellation** that stops queued work, kills running git processes rather than
  orphaning them, preserves completed clones, and removes only this run's partial output.
- **Two renderers** — a live per-repository display on a terminal, deterministic
  one-line-per-event output elsewhere. Result on stdout, progress and diagnostics on stderr.
- **`--protocol <ssh|https>`** to override the manifest's default transport.
- **Documented exit codes** — `0` complete, `1` incomplete or cancelled, `2` usage, `3`
  configuration.
- **158 tests** (plus one network-gated registry drift check, ignored by default),
  including a barrier-based proof that concurrency is genuine and bounded, and
  end-to-end tests that drive the real `git` binary against local bare repositories with no
  network.
- **CI** across Linux, macOS and Windows running format, lint, test and release build, plus
  an MSRV job; **release** workflow producing five target binaries with checksums.

### Security

- git is invoked with an argument vector, never through a shell, with positionals after `--`.
- Transports are allowlisted; git's remote-helper syntax is refused because `ext::` executes
  arbitrary commands.
- `GIT_TERMINAL_PROMPT=0` on spawned git processes, and SSH batch mode only when the
  developer has expressed no preference.
- No credentials are stored, printed or modified; no network service is contacted.
- `#![forbid(unsafe_code)]`.

### Deliberately not included

`qeet status`, `pull`, `sync`, `graph`, `dev`; dependency graphs; a remote registry; a backend
service; telemetry; shell completions; a Homebrew tap; JSON output. See
[docs/decisions.md](docs/decisions.md).

[Unreleased]: https://github.com/qeetgroup/qeet-cli/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/qeetgroup/qeet-cli/releases/tag/v0.2.0
[0.1.3]: https://github.com/qeetgroup/qeet-cli/releases/tag/v0.1.3
[0.1.2]: https://github.com/qeetgroup/qeet-cli/releases/tag/v0.1.2
[0.1.1]: https://github.com/qeetgroup/qeet-cli/releases/tag/v0.1.1
