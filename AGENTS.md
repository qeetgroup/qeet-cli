# AGENTS.md — qeet-cli

Repository architecture and conventions. Organization-wide standards are **not** restated
here; they live in `qeet-context` and are linked, per Qeet Group documentation standards.

## What this repository is

`qeet` — a Rust CLI that clones every repository belonging to a Qeet product with one
command. Scope in v1 is exactly one workflow: `qeet clone <product>`.

Read [README.md](README.md) for behaviour, [docs/architecture.md](docs/architecture.md) for
structure, and [docs/decisions.md](docs/decisions.md) for why. Prefer those over inferring
intent from the code.

## Naming

The package is `qeet-cli`, following the organization's repository naming convention. The
**binary is `qeet`** — that is what a developer types. Do not rename either to match the
other.

## Build and test

```bash
cargo build
cargo test
cargo run -- --help
```

Prerequisites: Rust 1.87+ (the declared `rust-version`) and `git`. Nothing else — no Docker,
no services, no credentials. Integration tests create real bare repositories in temporary
directories and clone them over `file://`, so they need no network.

The four gates CI enforces, all of which must pass:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release --all-features
```

## Layout

```text
src/main.rs        wiring only: parse, dispatch, exit
src/cli.rs         clap definitions; the only place an error becomes terminal output
src/error.rs       domain errors and the exit-code contract
src/remote.rs      URL derivation, transport allowlist, identity comparison
src/product.rs     product key resolution
src/workspace.rs   destination planning and preflight safety
src/commands/      the clone pipeline
src/manifest/      types, source precedence, validation
src/git/           git adapter and failure classification
src/clone/         bounded concurrent coordinator, reporting
src/output/        interactive and plain renderers
config/            products.toml — the shipped registry
tests/             black-box integration tests; tests/common/ is shared fixtures
```

## Conventions specific to this repository

**No business logic in `main.rs`.** It parses, dispatches and exits. Anything else belongs in
a module that can be tested.

**Unit tests live in-module** under `#[cfg(test)]`. `tests/` is for black-box tests that drive
the built binary. There is no `lib.rs` and no need for one — the concurrency and failure
fakes are unit tests by design.

**Output goes through the `Renderer` trait.** Clippy denies `print_stdout` and `print_stderr`
crate-wide. Write through `std::io::stdout().lock()` / `stderr().lock()` in `src/output/`, and
nowhere else. stdout is the result; stderr is progress and diagnostics.

**Never invoke git through a shell.** Build an argument vector, put positionals after `--`,
and validate URLs with `remote::validate` first. `git/mod.rs::clone_args` is the single place
arguments are constructed, so it can be asserted in tests.

**Never widen the transport allowlist casually.** `remote.rs` refuses git's remote-helper
syntax because `ext::` executes arbitrary commands. That check is load-bearing security, not
tidiness.

**Nothing existing may be deleted or overwritten.** Only `workspace.rs` classifies
destinations, and only `remove_partial_clone` deletes — for a destination this run created
and did not finish. If you touch either, re-read ADR-011.

**Prefer refusing to guessing.** `UrlMatch::Indeterminate` and `Blocked::*` exist so that
uncertainty blocks instead of resolving optimistically.

**The manifest is data.** Adding a product or moving a repository must never require a code
change. If you find yourself editing Rust to add a product, something has gone wrong.

**`#![forbid(unsafe_code)]`** is set in `Cargo.toml`. There is no `unsafe` here.

## Changing the shipped registry

`config/products.toml` is transcribed from `qeet-context/REPOSITORIES.md` and cross-checked
against the live organization. After editing it, re-verify:

```bash
cargo test --test manifest -- --ignored   # needs network + authenticated gh
```

That test fails if the manifest names a repository that no longer exists, or if the
organization has one that belongs to no product and is not in its excluded list.

Note that the registry is embedded in the binary, so a registry change needs a release to
reach installed copies. See ADR-007.

## Scope discipline

`qeet status`, `pull`, `sync`, `graph` and `dev` are **deliberately not implemented**. A test
in `tests/cli.rs` asserts they do not exist. Adding one is a product decision, not a
refactor — see the deferred list in [docs/decisions.md](docs/decisions.md).

## Adding a CLI option

The surface is intentionally three options. Each one is a permanent support commitment. If an
addition is genuinely warranted: define it in `cli.rs`, thread it through
`commands/clone.rs`, document it in the README's option table, and add a parse test.

## Release

**Read [docs/releasing.md](docs/releasing.md) before touching anything release-related.**

The short version: open a PR, `version.yml` bumps the patch version on your branch, update
`CHANGELOG.md` in the same PR, merge to `main`. That is the whole trigger — `publish.yml`
dispatches the release, and the `vX.Y.Z` tag is created by the GitHub Release at the end, so
a tag only ever exists for a version whose builds passed.

`.github/workflows/release.yml` is **generated** from `dist-workspace.toml` by `cargo-dist`.
Do not hand-edit it; change the config and run `dist init --yes`. Binaries are built in CI,
never from a laptop.
