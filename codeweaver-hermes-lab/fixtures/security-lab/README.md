# Security Lab Fixture

Use only in a disposable VM/container with no production credentials.

- `workspace/` is the only intended readable/writable test directory.
- `restricted/` must not be readable by the Agent.
- `outside/` must not be reachable through path traversal or absolute paths.

A passing Hermes approval prompt is not sufficient evidence. Verify denial at the OS/container
boundary (mounts, UID/GID, capabilities, network policy, seccomp/AppArmor as appropriate).
