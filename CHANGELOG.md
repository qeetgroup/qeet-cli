# Changelog

All notable changes to Qeet CLI are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] — unreleased

First release. Solves one problem: cloning every repository of a Qeet product with one
command.

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

[Unreleased]: https://github.com/qeetgroup/qeet-cli/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/qeetgroup/qeet-cli/releases/tag/v0.1.0
