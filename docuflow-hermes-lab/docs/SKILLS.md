# Skills (design 10)

A skill is an untrusted artifact: it may carry shell commands, file edits, network calls and install
steps. The registry therefore refuses anything the manifest does not declare, and authorization never
comes from the skill itself.

## Manifest

```ts
{
  skillId, version, name, kind: "platform" | "engineering" | "hermes-native",
  permissions: ["read_file", "search_code", "modify_worktree", ...],
  allowedRuntimes: ["hermes-local"] | ["*"],
  riskLevel: "low" | "medium" | "high",
  approvalPolicy: "auto" | "require_approval",
  checksum: "<sha256 of the content>",
  createdAt
}
```

Validation rejects an unknown permission, an unknown kind or risk level, a non-semver version, a
malformed checksum, and an empty `allowedRuntimes` — "runs anywhere" must be written as `*`, not left
blank.

## Versions

- `register()` inserts; a duplicate `skillId@version` is a conflict. Publishing a change means a new
  version.
- `resolve(skillId)` returns the highest non-deprecated version; `resolve(skillId, version)` returns
  exactly what was asked for or throws.
- `deprecate()` keeps the version resolvable by explicit version but stops it being chosen implicitly,
  and its authorization becomes `require_approval`.

## Authorization

`authorize(manifest, { runtimeId, policy, now })` returns `allow` / `deny` / `require_approval`:

| Condition | Result |
|---|---|
| runtime not in `allowedRuntimes` | deny |
| a declared permission is not in `policy.grantedCapabilities` | deny |
| policy snapshot expired | deny |
| `riskLevel: high` or `approvalPolicy: require_approval` | require approval |
| deprecated version | require approval |
| otherwise | allow |

## Bindings

`SkillBindingStore.bind()` records `executionId → skillId@version` plus the policy id it was authorized
under. Binding a denied authorization throws; binding is idempotent per execution and version;
`setEnabled()` keeps the record and flips the flag, mirroring design 11.5.

Bindings persist as an **append-only event log** (`bind` / `enable` / `disable`) under
`<data>/bindings/`, and `load()` folds the log back into current state. That keeps the file store
immutable while still supporting the mutable `enabled` flag.

## Not implemented

- Skill content review (design 10.2 "Skill 审核"): only the checksum is enforced, not a human approval.
- Skill execution and effectiveness evaluation.
- Loading skills into Hermes; this module decides what is *allowed*, not what the runtime loads.
