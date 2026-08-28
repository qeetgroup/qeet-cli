# Technical research and decisions

Why Qeet CLI v1 is the way it is. Dates and versions are as verified on **2026-08-28**.

---

## Research basis

The v1 design was informed by how existing multi-repository tools behave, and by what they
cost their users.

**Google's `repo`** is the most complete answer to this problem: a manifest in a separate
repository, XML, with per-project revision pinning for reproducibility, and `repo sync`
rather than `git pull` because only the tool knows which branch each project should track.
Two lessons taken: the manifest belongs outside the code, and pinning a revision is what
makes a working copy reproducible. One lesson deliberately *not* taken: `repo`'s scope. It
manages a whole workflow, and its manifest format carries that weight.

**GitHub CLI (`gh`)** shows the opposite tradeoff: a small surface, no manifest, and it
delegates entirely to the user's existing git and credential setup. `gh repo clone` never
tries to own authentication. That is the model Qeet CLI follows.

**`meta` and similar multi-repo wrappers** show the failure mode to avoid: a plugin surface
that grows until the tool is a build system. Their concurrency is also usually unbounded,
which is fine for five repositories and hostile at sixty.

Common failure modes these tools have taught, and what v1 does about each:

| Failure mode | What Qeet CLI does |
|---|---|
| Clobbering an existing working copy | Preflight classifies every destination; nothing existing is written or removed |
| Hanging on a credential prompt | `GIT_TERMINAL_PROMPT=0` always; SSH batch mode when the developer has no preference |
| Unbounded process spawning | `Semaphore`-bounded, no unlimited mode |
| One failure aborting the batch | Per-task failure isolation |
| Opaque errors (`exit status 128`) | Classification from stderr, plus git's own words and next steps |
| Progress that corrupts piped output | Progress on stderr, result on stdout, plain renderer off a TTY |
| Orphaned child processes on Ctrl-C | `abort_all()` plus `kill_on_drop(true)` |

---

## ADR-001 — Rust

Native binaries with no runtime to install, strong cross-platform process and path handling,
and a mature CLI ecosystem. Startup cost matters for a tool run interactively many times a
day, which rules out anything with an interpreter or VM start-up.

`#![forbid(unsafe_code)]` is enforced through `Cargo.toml` lints. There is no `unsafe` in the
crate and no reason for any.

**MSRV: 1.87.** Verified by building and running the whole suite on a real 1.87 toolchain, and
enforced by a CI job. Every direct dependency declares 1.85 except `etcetera`, which requires
1.87; requiring a compiler from May 2025 is cheaper than pinning a maintained dependency
backwards.

## ADR-002 — TOML, and not YAML

The manifest is structured developer configuration in a Rust project, and `toml` (1.1.4) is
Cargo's own parser: 1.x stable since March 2026, with `Error` carrying line and column for
free.

**This is worth recording because the organization's own context repository uses YAML**, so
TOML looks like a divergence. It is not an aesthetic choice. The Rust serde YAML situation as
of August 2026:

| Crate | State |
|---|---|
| `serde_yaml` | Deprecated by its author on 2024-03-25 |
| `serde_yml` | **RUSTSEC-2025-0068** — unsound and unmaintained; repository archived after unsoundness reports |
| `serde_yaml_ng` | Last release 2024-05-26 |
| `serde_norway` | Last release 2024-12-21 |
| `serde-saphyr` | Actively maintained (1.1.0, 2026-08-15) but young |

Adding YAML would mean either an unmaintained dependency, one with a security advisory, or a
young one — for a file that three people will ever edit. TOML avoids the question entirely.
No YAML parsing is implemented, and none should be added for v1.

## ADR-003 — Orchestrate git, never reimplement it

`git` is invoked as a child process. No `git2`, no `gix`, no protocol implementation.

Developers already have SSH keys, an ssh-agent, credential helpers, `insteadOf` rewrites,
proxy settings, enterprise configuration and commit signing. A library implementation would
have to reproduce all of it, and would get some of it wrong. Delegating means every one of
those keeps working for free, and it is why Qeet CLI needs no authentication code at all.

The cost is accepted honestly: Qeet CLI depends on git being installed, cannot report
per-object progress, and has to infer failure causes from stderr text.

## ADR-004 — Bounded concurrency

`tokio::sync::Semaphore` + `JoinSet`, default `available_parallelism()` clamped to `[1, 8]`,
overridable within `[1, 64]`.

Cloning is network-bound, so past roughly eight concurrent transfers there is little to gain
and real cost: file descriptors, memory, and rate limiting from the remote. **There is no
unlimited mode**, because on the largest product that would mean twelve git processes and on
a future all-products command sixty-six.

The permit is held across retries and backoff, so a retrying repository does not quietly
raise the effective concurrency.

## ADR-005 — Conservative retry

Only transient transport failures are retried: at most twice, ~500 ms then ~1500 ms, jittered
from the clock rather than by adding a random-number dependency.

Authentication failures, missing repositories, missing refs and unrecognised failures are
never retried. None of them will produce a different answer on a second attempt, and retrying
turns one clear error into three slow ones.

Classification comes from git's **stderr**, not its exit code, because git reports nearly
everything as 128.

## ADR-006 — Flat workspace layout

`qeet clone id` clones into the current directory with no product directory in between.

Chosen by product decision. It is safe here because repository names are unique across the
organization — the names already encode the product (`qeet-id-server`), so a grouping
directory would only repeat it. The consequence is documented: running the command in the
wrong directory scatters repositories into it, which is why preflight never overwrites
anything.

## ADR-007 — Embedded manifest, no remote registry

The manifest is compiled in with `include_str!`, so `qeet clone id` works the moment the
binary is installed — no setup step, no config file to write, no network call to resolve a
product. A test parses the embedded copy, so a broken registry cannot ship.

The tradeoff is stated plainly rather than hidden: **the embedded registry is a release-time
snapshot.** When the organization changes, either Qeet CLI is released again or a developer
points `--manifest`, `QEET_MANIFEST`, or their config directory at a newer file. Those three
overrides exist precisely so nobody is blocked waiting for a release.

A remote registry service is deliberately deferred. It would make a local, offline-capable
tool depend on an endpoint being up, to solve a problem that a release already solves.

## ADR-008 — URL policy as a security boundary

Repository URLs become arguments to an external executable, so they are untrusted input.

Transports are allowlisted to `https`, `http`, `ssh`, `git` and `file`. Everything else is
refused — in particular git's remote-helper syntax `<helper>::<address>`, because **`ext::`
executes an arbitrary command**. Without this check, a manifest would be a remote code
execution vector rather than a configuration file.

Also refused: any URL starting with `-` (which could smuggle `--upload-pack=`), and any URL
containing whitespace. Positional arguments always follow `--`. Commands are always built as
argument vectors; no shell is ever involved.

Bracketed IPv6 hosts are exempted from the `::` check, so `https://[::1]/repo.git` is not
mistaken for a remote helper.

## ADR-009 — Semantic origin comparison, with a third answer

Comparison reduces both URLs to `(host, path)`. It returns `Same`, `Different`, or
`Indeterminate`, and only `Same` allows a destination to be skipped as already present.

String equality would be wrong — `git@github.com:qeetgroup/x.git` and
`https://github.com/qeetgroup/x.git` are the same repository. But over-eager normalisation
would be worse: concluding "same repository" when it is not is the one mistake that could
cost a developer work. So anything that cannot be reduced with confidence blocks. The
asymmetry between the two error directions is the whole point.

## ADR-010 — Non-interactive git, without touching the developer's setup

`GIT_TERMINAL_PROMPT=0` is always set on spawned git processes. This is not a preference: with
several clones in flight sharing one terminal, interleaved prompts are unreadable, and one
stalled child holds up the entire run. Authentication must succeed or fail fast.

SSH batch mode is applied **only** when the developer has set neither `GIT_SSH_COMMAND` nor
`core.sshCommand` — one cached `git config --get` probe decides. Their configuration always
wins, even though that reintroduces the possibility of a passphrase prompt.

Qeet CLI stores no credentials, reads no tokens, and modifies no SSH or credential
configuration. Third-party credential helpers that open a GUI of their own are outside what
it can control, and that is stated as a limitation rather than papered over.

## ADR-011 — Ownership-based cleanup

A destination is removed only if preflight saw it absent, this run created it, and the clone
failed or was cancelled.

Ownership is established by preflight plus construction, **not** by inspecting the directory
afterwards. git creates `.git` early during a clone, so a half-finished clone looks like a
valid repository — content inspection would reach the wrong conclusion. Pre-existing
directories, successful siblings, and parent directories created on the way down are never
removed.

## ADR-012 — Two renderers, and stream discipline

`stdout` carries the result (the summary). `stderr` carries progress and diagnostics. So
`qeet clone id > summary.txt` still shows failures on the terminal.

Interactivity is detected on **stderr**, because that is where progress is drawn. `indicatif`
hides its bars off a TTY, but a hidden progress bar is not useful CI output — the plain
renderer emits one deterministic line per event instead.

Raw git output is not streamed. Several interleaved `--progress` streams are unreadable, and
the filtered stderr in the failure report is more useful than a live wall of text.

## ADR-013 — No `async_trait`, no `tokio-util`

`GitClient` uses return-position `impl Future<Output = …> + Send` and the coordinator is
generic over `G: GitClient` rather than using `dyn`. `JoinSet` requires `Send` futures, which
a bare `async fn` in a trait cannot promise — writing the bound out is what removes the need
for `async_trait` at all. Clippy's `manual_async_fn` is allowed at those two sites with that
reason recorded inline.

Cancellation needs no `CancellationToken`: `JoinSet::abort_all()` plus `kill_on_drop(true)`
gives identical semantics with one less dependency.

`num_cpus` is unnecessary — `std::thread::available_parallelism()` is in the standard library.
`strsim` is unnecessary — the "did you mean" suggestion is twenty lines of Levenshtein.

## ADR-014 — Hand-written release workflow rather than cargo-dist

`cargo-dist` was evaluated and is healthy (v0.32.0, May 2026; repository active). It was not
adopted for two reasons.

First, Qeet Group engineering standards require third-party GitHub Actions to be pinned to a
commit SHA. `cargo-dist` generates a workflow it owns, which must be regenerated on version
bumps and does not pin that way.

Second, the release requirements here are modest — five targets, archives, checksums, a
GitHub Release — and a fifty-line workflow that a reviewer can read end to end is worth more
than a generated one that has to be kept in sync.

The workflow refuses to build if the git tag and `Cargo.toml` version disagree, so versioning
cannot drift. Shell installer scripts and a Homebrew tap are **deferred, not done**.

## ADR-015 — Manifest data is real, and public by explicit approval

`config/products.toml` contains the actual Qeet Group registry — 16 products, 66
repositories — transcribed from the L0 registry (`qeet-context/REPOSITORIES.md`, verified
2026-08-28) and cross-checked against the live organization. Nothing was invented.

Two corrections to the illustrative names used in the original brief, confirmed against the
organization: the GitHub organization is **`qeetgroup`** (lowercase), and role suffixes are
`-server`, `-console`, `-login`, `-website`, `-auth`, `-docs`, `-go`, `-node`, `-react`,
`-deploy`, `-files` — there is no `qeet-id-api` or `qeet-id-web`.

**Metadata exposure was decided explicitly, not by default.** Publishing this repository
publicly with the complete registry embedded discloses the names of 21 repositories that are
currently private: the 17 `*-files` specification repositories, `qeet-context`,
`qeet-people-server`, and the pre-launch `qeet-ai-server`, `-aiservice`, `-console` and
`-deploy`. That disclosure was **approved by the repository owner** as the deliberate
tradeoff for a registry that works out of the box.

No credentials, tokens, keys, or URLs beyond those derivable from `host` + `owner` + `name`
are committed, under this or any other model.

Two organization repositories are intentionally absent from every product, and the drift test
fails if a third appears without a reason:

| Repository | Why |
|---|---|
| `qeetrix` | Archived historical monorepo, superseded by the six `qeetrix-*` repositories |
| `.github` | Organization profile; a `.github` clone directory would be hidden |

### One finding worth recording

The L0 registry states that no `qeet-<product>-context` repository exists yet.
**`qeet-id-context` does exist** — it was found by cross-checking the transcription against
the GitHub API, and is mapped under `id`. This is registry drift, reported rather than
silently reconciled.

---

## Deliberately deferred

Not built, and not to be built until `qeet clone` is excellent: `qeet status`, `pull`, `sync`,
`graph`, `dev`; dependency graphs; a remote registry; a Qeet backend or web dashboard;
telemetry; secrets management; shell completions; a Homebrew tap; JSON output; multi-SCM
support; and any form of build or CI orchestration.

The architecture leaves room for the first few. That is not a commitment to build them.
