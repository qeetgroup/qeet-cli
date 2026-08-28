# Releasing Qeet CLI

How a release happens, what it produces, and what to do when it goes wrong.

`cargo-dist` owns the pipeline. `.github/workflows/release.yml` is **generated** from
`dist-workspace.toml` — never hand-edit it. Change the config and run `dist init --yes`.

---

## The flow

```text
   open a PR
       │
       ▼  version.yml
   patch version bumped on your branch (visible in the PR diff)
       │
       ▼  merge to main
   publish.yml  ── version already released? ──► stop
       │
       ▼  dispatches release.yml with tag vX.Y.Z
   validate.yml  ── fmt, clippy, test ──► fails? nothing is built
       │
       ▼
   build 5 targets → archives + checksums → installers + formula
       │
       ▼
   GitHub Release created ── which is what creates the vX.Y.Z tag
```

Two properties follow from that shape, and both are deliberate:

- **A `vX.Y.Z` tag only ever exists for a version whose builds passed.** The tag is created
  by the Release, at the end, not pushed at the start.
- **Merging without a version change releases nothing.** `publish.yml` stops if the version
  in `Cargo.toml` is already released, so it is safe to re-run and safe to merge docs-only
  PRs.

### Normal release

1. Open a PR. `version.yml` bumps the patch version on your branch and commits it, so the
   version you are shipping is reviewable in the diff.
   - Want a **minor or major** bump? Edit `version` in `Cargo.toml` yourself. `version.yml`
     only acts when the PR's version still equals `main`'s, so a manual bump is left alone.
2. Update `CHANGELOG.md` in the same PR.
3. Merge. That is the whole release trigger.
4. Watch it: `gh run list --workflow=release.yml`

### Releasing by hand

If `publish.yml` did not fire, or you are re-running a failed release:

```bash
gh workflow run release.yml --ref main --field tag=v0.1.1
```

The `tag` input defaults to `dry-run`, which plans and builds but does not publish — useful
for checking the pipeline without shipping anything.

### Deployments and the `release` environment

`publish.yml`'s `release` job declares a GitHub [environment](https://docs.github.com/en/actions/how-tos/deploy/configure-and-manage-deployments/manage-environments)
named `release`. That is what makes each release appear in the repository's
**Deployments / Environments** panel, linked to the GitHub Release it produced — the same way
`qeetrix-icons` uses `npm-publish` and `qeet-notify-server` uses `production`.

A deployment record is only created when something is actually released: the `check` job
decides, and the `release` job (which carries the environment) is skipped when the version is
already published.

**To require a human before any release**, add a required reviewer to the environment:

```text
Settings → Environments → release → Required reviewers
```

Nothing else changes; the release simply waits for approval before dispatching.

### Why a dispatch rather than a tag push

A tag pushed with `GITHUB_TOKEN` does **not** trigger another workflow, while
`workflow_dispatch` always does. This is
[documented GitHub behaviour](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow),
and it is why `publish.yml` dispatches instead of tagging. The alternative would be a PAT
purely to make a tag push trigger CI, which is a credential for no good reason.

---

## What a release produces

Attached to the GitHub Release:

| Artifact | Notes |
|---|---|
| `qeet-cli-aarch64-apple-darwin.tar.xz` | macOS, Apple silicon |
| `qeet-cli-x86_64-apple-darwin.tar.xz` | macOS, Intel |
| `qeet-cli-x86_64-unknown-linux-gnu.tar.xz` | Linux x86_64 |
| `qeet-cli-aarch64-unknown-linux-gnu.tar.xz` | Linux arm64, built natively on `ubuntu-24.04-arm` |
| `qeet-cli-x86_64-pc-windows-msvc.zip` | Windows x86_64 |
| `<each archive>.sha256` | Per-archive checksum |
| `sha256.sum` | Combined checksums |
| `qeet-cli-installer.sh` | POSIX shell installer |
| `qeet-cli-installer.ps1` | PowerShell installer |
| `qeet.rb` | Homebrew formula, with checksums filled in |
| `source.tar.gz` | Source snapshot |

Each archive contains the binary plus `README.md`, `LICENSE` and `CHANGELOG.md`, under a
single top-level directory.

Note the names carry **no version** — that is cargo-dist's convention, and it is what makes
`releases/latest/download/<name>` a stable URL. The version lives in the release tag.

---

## Homebrew

### Current state

```bash
brew install qeetgroup/tap/qeet     # works
brew install qeet                    # does NOT work — do not document it as if it does
```

The bare name requires [homebrew-core](https://github.com/Homebrew/homebrew-core), which is
not yet reachable. Two independent gates:

| Gate | Requirement | qeet-cli |
|---|---|---|
| Notability | ≥75 stars, or ≥30 forks, or ≥30 watchers | 0 / 0 / 0 |
| Build source | Must build from source; core refuses third-party prebuilt binaries | ships prebuilt binaries |

The second gate is worth noting: reaching the final target is not only a popularity
threshold, it needs a **different formula** that compiles from source.

### Updating the tap after a release

Automatic publishing is **off** (see below), so for now:

```bash
VERSION=0.1.1
gh release download "v$VERSION" --repo qeetgroup/qeet-cli --pattern 'qeet.rb' --dir /tmp --clobber

git clone git@github-qg:qeetgroup/homebrew-tap.git /tmp/homebrew-tap
mkdir -p /tmp/homebrew-tap/Formula
cp /tmp/qeet.rb /tmp/homebrew-tap/Formula/qeet.rb

cd /tmp/homebrew-tap
git add Formula/qeet.rb
git commit -m "qeet $VERSION"
git push
```

Never hand-edit a version or checksum in the formula, and never commit a binary to the tap.
The formula from the release already carries the correct URLs and checksums.

### Enabling automatic Homebrew publishing

Two things must happen first, and the second is a decision rather than a task.

1. **A token.** dist's publish job pushes to another repository, which `GITHUB_TOKEN` cannot
   do. Mint a PAT with `repo` scope and add it:

   ```bash
   gh secret set HOMEBREW_TAP_TOKEN --repo qeetgroup/qeet-cli
   ```

2. **Attribution.** dist's generated job commits to the tap as
   `axo bot <admin+bot@axo.dev>`. Qeet Group requires commits in its repositories to be
   attributed to a Qeet identity, so this should be replaced with a custom publish job
   (`publish-jobs = ["./publish-homebrew"]`) that pushes with
   `github-actions[bot]` — the same identity `version.yml` already uses.

Then uncomment `publish-jobs` in `dist-workspace.toml` and run `dist init --yes`.

---

## get.qeet.in

**Entirely optional.** Both install paths already work without it —
`brew install qeetgroup/tap/qeet` and the GitHub Releases installer URL. `get.qeet.in` buys a
shorter command and nothing else. If it is never set up, remove this section and
`install/`; nothing else depends on it.

The endpoint redirects to the latest release's installer, so the script can never drift from
the release it installs.

```text
get.qeet.in/cli      →  releases/latest/download/qeet-cli-installer.sh
get.qeet.in/cli.ps1  →  releases/latest/download/qeet-cli-installer.ps1
```

### Status

| Step | State |
|---|---|
| DNS: `get` CNAME → Vercel at GoDaddy | **done** — resolves to `d29cb206743296cb.vercel-dns-017.com` |
| Vercel project claiming `get.qeet.in` | **outstanding** — no project owns the domain, so there is no certificate and nothing is served |

### One-time setup

`install/vercel.json` holds the redirect rules. Deploy that directory as its own Vercel
project and attach the domain:

```bash
cd install
npx vercel link          # create a new project, e.g. "qeet-get"
npx vercel domains add get.qeet.in
npx vercel --prod
```

Or via the dashboard: **New Project → import `qeetgroup/qeet-cli`, root directory
`install/` → Settings → Domains → add `get.qeet.in`**. Vercel issues the certificate once it
sees the CNAME, which is already in place.

### Verify

```bash
curl -I https://get.qeet.in/cli      # expect 302 to the release asset
curl -fsSL https://get.qeet.in/cli | head -5
```

Until that returns 302, the README documents the GitHub Releases URL instead. **Never
document a URL that does not resolve.**

---

## When a release goes wrong

### Validation failed

Nothing was built and no tag exists. Fix the commit, merge again. `publish.yml` will
dispatch a fresh release because the version is still unreleased.

### Builds failed on one platform

No Release and no tag are created — `host` runs only after every build succeeds. Fix and
re-run:

```bash
gh workflow run release.yml --ref main --field tag=v0.1.1
```

### A tag exists but there is no Release

`publish.yml` refuses to proceed in this state and says so, because releasing again would be
ambiguous. Decide deliberately:

```bash
# Abandon the half-finished attempt and let the version be released cleanly
git push --delete origin v0.1.1
gh workflow run release.yml --ref main --field tag=v0.1.1
```

### A bad version shipped

Binaries are immutable once downloaded, so there is no true recall. Ship a fix forward:
bump the patch version in a PR and merge. If the release is actively harmful, additionally
mark it as a pre-release or delete it so `releases/latest` stops pointing at it — which also
changes what `get.qeet.in/cli` serves.

---

## Prerequisites for maintainers

```bash
cargo install cargo-dist --locked --version 0.32.0
```

Useful before pushing anything release-related:

```bash
dist plan                                        # what a release would produce
dist build --artifacts=global                    # installers + formula, no compilation
dist build --artifacts=local --target <host>     # a real archive for your platform
```

`dist build --artifacts=local` without `--target` tries to cross-compile every platform and
will ask for `cargo-xwin` and `cargo-zigbuild`. In CI each target builds on its own runner,
so those are never needed there.
