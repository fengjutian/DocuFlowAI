# Client base: Zed (source)

Zed is the project's client base. Its source is provisioned from a pinned release rather than vendored
into git, because a 500 MB tree would bloat every clone and risk GitHub's push limits.

## Provision

```powershell
powershell -ExecutionPolicy Bypass -File client/fetch-zed.ps1
```

- Resumable: rerun after an interruption and curl continues.
- Writes the tree to `client/zed` and records the pin in `client/PIN.json`.
- `client/zed` is gitignored; the pin file is tracked.

| | |
|---|---|
| Upstream | https://github.com/zed-industries/zed |
| Pin | `v1.27.0` |
| License | GPL-3.0 — distributing a product that embeds it brings GPL obligations |

## Why not a submodule or a copied tree

- **Submodule**: preferred in principle, but the clone stalled at a zero-byte pack on this network twice.
  When the network allows it, `git submodule add --depth 1` at the same tag is a drop-in replacement.
- **Copied tree in git**: hundreds of MB in history, push failures, and no supply-chain boundary.

## Build (after provisioning)

Zed is a Rust workspace pinned to Rust **1.98.1** by its own `rust-toolchain.toml`; this machine runs
1.96.0, so the first build installs the pinned toolchain and the wasm targets.

```powershell
powershell -ExecutionPolicy Bypass -File client/build-zed.ps1 -Release
```

Expect a long first build (toolchain download, hundreds of crates, tens of minutes, GBs of artifacts
under `client/zed/target`). Nothing here is part of `npm test`.

Prerequisites already present on this machine: Visual Studio 2022 Build Tools with the C++ workload,
and rustup. Missing until the script runs: Rust 1.98.1, `wasm32-wasip2`, `wasm32-unknown-unknown`.

## Status

Source provisioning is in progress/blocked on bulk transfer speed (~11 KB/s measured from GitHub on this
machine). The DocuFlow ACP endpoint (`npm run acp:serve`) already works with the **installed** Zed; the
vendored tree is for features that cannot be expressed through settings or an extension.
