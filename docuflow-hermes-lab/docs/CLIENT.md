# Client base: Zed

Zed is the client. DocuFlow is an ACP agent that Zed spawns; Hermes runs behind DocuFlow. The client
foundation in this repository is the registration + protocol layer, not a copy of Zed's source.

## Why not vendor Zed's source

| Fact | Number |
|---|---|
| Zed repository size | ~540 MB |
| Zed editor license | GPL-3.0 (copyleft); copying it into this repo changes DocuFlow's license obligations |
| Build cost | Rust, Windows SDK, wasmtime; a full build is tens of minutes and needs a toolchain upgrade path |

Forking Zed is a product decision with maintenance and licensing consequences. Integrating through its
documented extension point (external ACP agents) gets the same user-visible result — DocuFlow inside Zed
— without owning the editor.

## Zed source

The full tree is provisioned at `client/zed` from the local archive (Zed **1.29.0**, Rust 1.98.1) and
is gitignored; `client/PIN.json` records the version, file count, size and a SHA-256 tree digest.

```powershell
powershell -ExecutionPolicy Bypass -File client/fetch-zed.ps1 -FromPath C:\path\to\zed-main
powershell -ExecutionPolicy Bypass -File client/build-zed.ps1 -Release
```

A git submodule was the first choice, but the clone stalled at a zero-byte pack on this network and the
supplied archive has no `.git`. The digest is the identity of the tree; if a real fork is needed later,
use a repository fork plus a submodule rather than a copy.

## Register DocuFlow

```bash
HERMES_HTTP_BASE_URL=http://127.0.0.1:8642 npm run install:zed
```

- Writes only `agent_servers.docuflow` in Zed's settings; every other key is preserved.
- Backs up the file to `settings.json.bak-<timestamp>` before writing.
- Idempotent: a second run reports "already registered" and changes nothing.
- Refuses to touch a settings file that is not valid JSON.
- `--dry-run` reports what it would do without writing.

Settings path: `%APPDATA%\Zed\settings.json` on Windows, `~/.config/zed/settings.json` elsewhere;
override with `ZED_SETTINGS_PATH`.

The written entry launches the ACP endpoint:

```json
{
  "agent_servers": {
    "docuflow": {
      "command": "<node>",
      "args": ["<repo>/dist/runners/acp-server.js"],
      "env": { "HERMES_HTTP_BASE_URL": "...", "DOCUFLOW_DATA_DIR": "..." }
    }
  }
}
```

Restart Zed and DocuFlow appears in the agent panel. See [`ZED.md`](ZED.md) for the protocol bridge.

## What exists / what does not

| Piece | State |
|---|---|
| ACP endpoint over stdio (initialize, session/new, prompt, cancel) | done, tested |
| Event streaming to `session/update` | done |
| One-command registration with backup and idempotency | done |
| Embedded UI, custom panels, Zed-side keybindings | not started; would need a Zed extension or a fork |
| Approval dialog in the editor (`session/request_permission`) | done: allow/reject round-trip, timeout denies |
| Snapshots/policy enforcement on the ACP path | snapshots recorded per prompt; runtime enforcement still via Code Engine + sandbox |

## Extending later

If DocuFlow needs editor-side UI (diff panels, approval dialogs, task lists), the next step is a Zed
extension (Rust → WASM, `zed_extension_api`) rather than a fork. That keeps the editor upstream and
still ships DocuFlow-specific surfaces.
