# `get.qeet.in`

Hosting configuration for the install endpoint. **Not deployed from this repository** — see
[../docs/releasing.md](../docs/releasing.md) for the one-time setup.

## What this is

`get.qeet.in` **redirects**; it does not host a copy of the installer.

```text
curl -fsSL https://get.qeet.in/cli | sh
        │
        ▼  302
github.com/qeetgroup/qeet-cli/releases/latest/download/qeet-cli-installer.sh
        │
        ▼  the installer then fetches, verifies and unpacks
github.com/qeetgroup/qeet-cli/releases/latest/download/qeet-cli-<target>.tar.xz
```

Redirecting rather than copying matters: the script can never drift from the release it
installs, and there is one source of truth. `curl -fsSL` already follows redirects (`-L`), so
the indirection is invisible.

There is deliberately no backend, no database, no download service and no binary storage.
GitHub Releases is the canonical artifact source.

## Why Vercel and not Cloudflare

Every Qeet property — `qeet.in`, `docs.qeet.in`, `id.qeet.in` — is served by Vercel, and
`qeet.in`'s nameservers are GoDaddy. A Cloudflare Worker custom domain would require moving
the whole zone to Cloudflare, which would touch DNS for every Qeet site. See
[../docs/decisions.md](../docs/decisions.md) ADR-018.

## Pinning a version

The endpoint always serves the installer from the **latest** release. There is no version
env var; to pin, skip the endpoint and name the release explicitly:

```bash
curl -fsSL https://github.com/qeetgroup/qeet-cli/releases/download/v0.1.0/qeet-cli-installer.sh | sh
```

That is the deterministic form CI should use, and it is what the README documents for pinning.

## Environment variables the installer honours

Read from the generated script, not assumed:

| Variable | Effect |
|---|---|
| `QEET_CLI_INSTALL_DIR` | Install somewhere other than `~/.local/bin` |
| `QEET_CLI_NO_MODIFY_PATH=1` | Do not touch `~/.profile` (see ADR-016) |
| `QEET_CLI_PRINT_VERBOSE=1` | Verbose output |
| `QEET_CLI_DOWNLOAD_URL` | Fetch archives from somewhere other than GitHub Releases |

## Files

| File | Purpose |
|---|---|
| `vercel.json` | The redirect rules. Deploy this directory as its own Vercel project. |
