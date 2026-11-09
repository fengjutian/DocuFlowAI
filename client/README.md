# Client base: Zed (source)

Zed is the project's client base. Its full source tree lives at `client/zed` and is **gitignored**;
`client/PIN.json` records exactly what was provisioned so a fresh clone can be rebuilt without
guessing.

## What is provisioned

| Field | Value |
|---|---|
| Source | https://github.com/zed-industries/zed |
| Archive | local download, `main` branch (no `.git`) |
| Zed version | **1.29.0** (`crates/zed/Cargo.toml`) |
| Rust toolchain | **1.98.1** (`rust-toolchain.toml`) |
| Files | 4,404 · 91.5 MB |
| Tree digest | `f7438f1f138105039a00a92c84c0bbfa89c187c687cf4d9d9647d9c7903712cd` |

The digest is SHA-256 over `relative-path\0size` for every file, so re-provisioning can be verified
without trusting timestamps.

## Re-provision

```powershell
# from a local archive (offline, preferred here)
powershell -ExecutionPolicy Bypass -File client/fetch-zed.ps1 -FromPath C:\path\to\zed-main

# or download the pinned tag (slow on this network; codeload cannot resume)
powershell -ExecutionPolicy Bypass -File client/fetch-zed.ps1 -Tag v1.27.0
```

## Build

```powershell
powershell -ExecutionPolicy Bypass -File client/build-zed.ps1 -Release
```

Zed is a large Rust workspace (300+ crates, edition 2024). Prerequisites present on this machine:
Visual Studio 2022 Build Tools with the C++ workload, rustup. The pinned toolchain 1.98.1 and the
`wasm32-wasip2` / `wasm32-unknown-unknown` targets are installed by the first run.

Expect a long first build: hundreds of crates from crates.io plus a full Rust compile. Artifacts land
in `client/zed/target` (GBs).

## Why the tree is not committed

- 91.5 MB and 4,400 files on every clone and every CI checkout.
- The upstream archive has no `.git`, so there is no commit to pin; the tree digest is the identity.
- Zed is GPL-3.0; keeping the boundary visible matters for licensing even when the code sits in the
  working tree.

If a real fork is needed later, the clean move is a repository fork plus this directory as a
submodule, not a copy.

## What DocuFlow uses it for

Nothing yet at runtime: the installed Zed already talks to DocuFlow over ACP (`npm run install:zed`).
The source is here for features that settings or an extension cannot express.
