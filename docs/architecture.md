# Architecture

## The shape of the problem

Qeet Group's organization is flat: 68 repositories, no nesting, `main` everywhere. A product
is a *set* of those repositories, and that set exists only as knowledge — in a registry
document, or in a developer's head.

Qeet CLI's whole job is to turn that set into a local working copy safely and quickly. So the
architecture is a pipeline with one rule: **everything that can fail cheaply fails before
anything expensive or destructive happens.**

## Command flow

```text
qeet clone id
     │
     ▼
cli.rs                     parse arguments; clap owns misuse (exit 2)
     │
     ▼
manifest/source.rs         --manifest → QEET_MANIFEST → config dir → embedded
     │
     ▼
manifest/mod.rs            TOML parse, deny_unknown_fields, schema check
     │
     ▼
manifest/validate.rs       all problems at once, before any process runs
     │
     ▼
product.rs                 "id" → Qeet ID, or list what exists + did-you-mean
     │
     ▼
git/client.rs              `git --version` once: one clear error, not twelve
     │
     ▼
workspace.rs               classify every destination; still no writes
     │
     ▼
clone/coordinator.rs       Semaphore + JoinSet, bounded concurrency
     │       ┌──────────────┬──────────────┐
     │       ▼              ▼              ▼
     │   git clone      git clone      git clone      ← real `git`, argv, no shell
     │       └──────────────┴──────────────┘
     ▼
clone/report.rs            one entry per repository, always
     │
     ▼
output/                    interactive or plain, chosen by whether stderr is a terminal
     │
     ▼
error.rs                   report → exit code
```

The ordering is the design. By the time the first git process starts, the manifest is valid,
the product exists, git works, and every destination has been classified. A run that is going
to fail for a configuration reason fails in milliseconds, having written nothing.

## Module responsibilities

| Module | Owns | Deliberately does not own |
|---|---|---|
| `main.rs` | Nothing but wiring: parse, dispatch, exit. | Any policy at all. |
| `cli.rs` | The argument surface and error→exit mapping. | What a clone means. |
| `error.rs` | Domain errors and the exit-code contract. | Rendering. |
| `manifest/` | Types, source precedence, parsing, validation. | Filesystem state. |
| `remote.rs` | URL derivation, the transport allowlist, identity comparison. | Spawning anything. |
| `product.rs` | Key → product, and the "did you mean" suggestion. | Repositories on disk. |
| `workspace.rs` | Destinations, containment, and preflight classification. | Cloning. |
| `git/` | Argument construction, process spawning, failure classification. | Concurrency, retry. |
| `clone/` | Bounded concurrency, retry, cancellation, aggregation. | How git is invoked. |
| `output/` | Two renderers and one shared summary. | Deciding what happened. |

`remote.rs` is the only module outside the §33 layout in the original specification. URL
derivation, the transport allowlist and identity normalisation are one cohesive concern with
three consumers — manifest validation, workspace preflight and git argument construction —
and splitting it across them would duplicate the parsing that both the security check and
the identity check depend on.

## The git boundary

```rust
pub trait GitClient: Send + Sync + 'static {
    fn clone_repo(&self, request: CloneRequest)
        -> impl Future<Output = Result<(), Failure>> + Send;
    fn origin_url(&self, repository: PathBuf)
        -> impl Future<Output = Result<Option<String>, GitError>> + Send;
}
```

This trait is why the coordinator can be tested without a network, a remote, or credentials.
The coordinator is generic over `G: GitClient` rather than using `dyn`, so there is no need
for a trait-object crate.

The return type is written out as `impl Future + Send` rather than declared `async fn`
because `JoinSet` requires `Send` futures and a bare `async fn` in a trait cannot promise
that. That single detail is what lets Qeet CLI avoid `async_trait` entirely.

Invocation is always an argument vector, never a shell string:

```text
git clone --progress -- <url> <destination>
git clone --progress --branch <ref> -- <url> <destination>
```

The `--` matters: together with the transport allowlist in `remote.rs` it closes off
argument injection, so a manifest cannot smuggle `--upload-pack=…` into a git command line.

## Workspace safety

Two independent checks, because neither is sufficient alone:

1. **Syntactic containment** — `manifest/validate.rs` rejects absolute paths, `..`, and
   Windows roots and prefixes. Catches a bad manifest before anything touches the disk.
2. **Resolved containment** — `workspace.rs` resolves the deepest existing ancestor of the
   destination and asserts the result is still under the canonicalised root. Catches what
   syntax cannot see: a symlinked parent directory pointing out of the workspace.

Comparison is component-wise (`Path::starts_with`), so a sibling named `qg-evil` is not
mistaken for a child of `qg` the way a string prefix test would be.

The root is canonicalised at startup. On macOS `/tmp` is a symlink to `/private/tmp`, and
every containment check would be wrong without it.

### Ownership, and what may be deleted

Exactly one thing may ever be removed: a destination that

- did not exist when the run started (recorded as `State::Create`), **and**
- this run created, **and**
- the clone for it failed or was cancelled.

That is established by *preflight plus construction*, not by inspecting the directory
afterwards — git creates `.git` early enough that a half-finished clone looks like a
repository, so content inspection would draw the wrong conclusion. A directory that already
existed, a sibling that succeeded, and any parent directory created on the way down are all
out of scope for removal, permanently.

### Identity, and refusing to guess

`origin` comparison reduces both URLs to `(host, path)` — stripping scheme differences,
userinfo, port, and a trailing `.git` — so `git@github.com:qeetgroup/x.git` and
`https://github.com/qeetgroup/x.git` compare equal.

The comparison has three outcomes, not two: `Same`, `Different`, and **`Indeterminate`**.
Anything other than `Same` blocks. Wrongly concluding "different" costs a developer one
puzzled minute; wrongly concluding "same" could cost them work. The asymmetry is deliberate.

## Concurrency

A `tokio::sync::Semaphore` sized to the limit, plus a `JoinSet`. Each task holds its permit
for the whole clone *including retries and backoff*, so a repository that is waiting to retry
does not let an extra clone in behind it.

Bounded, always. 66 simultaneous git processes would exhaust file descriptors and invite rate
limiting; there is no unlimited mode to reach for.

Failures are per task. A task that fails resolves normally with a failed outcome — it does
not propagate, and it never cancels a sibling.

### Cancellation

`tokio::signal::ctrl_c()` sits in the coordinator's `select!` loop. On interrupt:

1. `JoinSet::abort_all()` — which also aborts tasks still queued on the semaphore, so nothing
   new starts.
2. Each aborted task drops its `Child`, and because every command is configured with
   `kill_on_drop(true)`, the git process is killed rather than orphaned.
3. Anything that never settled is reported as cancelled, and its partial output is cleaned up
   under the ownership rule above.
4. Exit `1`.

If the signal handler cannot be registered, the future never resolves rather than resolving
with an error — a run must not cancel itself because signal handling was unavailable.

## Retry

Classification comes from git's stderr, not its exit code: git reports almost everything as
128. Only `Transient` — resolution failures, resets, timeouts, `RPC failed`, `early EOF` — is
retried, at most twice, with ~500 ms then ~1500 ms of jittered backoff.

Authentication failures, missing repositories and missing refs are never retried. Retrying
them wastes the developer's time and hammers the remote for an answer that will not change.
`Unknown` is not retried either: conservative by default.

## Error reporting

A failed repository produces the failure kind in Qeet CLI's voice, git's own stderr filtered
to the lines that matter, and two or three concrete next steps. Never a Rust backtrace, never
a bare `exit code 128`, and never git's chatter in full — with several clones interleaved,
`--progress` output would bury the one line that explains the problem.

## Testing strategy

Four layers, and the middle two are the interesting ones.

- **Unit tests, in-module.** URL validation and identity, manifest validation, failure
  classification, preflight states, backoff bounds, exit-code mapping.
- **Concurrency, proved not assumed.** The fake `GitClient` holds a `tokio::sync::Barrier`
  sized to the concurrency limit. It can only release if that many clones are genuinely in
  flight at the same instant, so a sequential implementation *stalls* rather than passing. A
  timeout converts that stall into a clear failure instead of a hung CI job, and an atomic
  peak counter asserts the limit is never exceeded. No sleeps are used as evidence.
- **Real git, no network.** The integration tests create real bare repositories in temporary
  directories and clone them over `file://` with the real git executable, through the real
  `qeet` binary. That covers argument construction, process spawning, concurrency, cleanup
  and exit codes deterministically, with no credentials and no GitHub access.
- **Registry drift.** One ignored test compares `config/products.toml` against the live
  organization via `gh`. Ignored by default because organization standards say tests must not
  depend on the network.

## Where this could go

The manifest is already data, and git is already behind a trait. `qeet status`, `qeet pull`
and a remote registry would each fit without restructuring. None of them is built, and none
should be until `qeet clone` is excellent.
