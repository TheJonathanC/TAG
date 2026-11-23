# Security Policy and Threat Model

TAG (Agent Cockpit) incorporates a secure local secrets vault and environment variable injection for agent workflows.

## Threat Model

### Assets Protected
1. **API Keys and Credentials at Rest:** Encrypted with an OWASP-recommended Argon2id-derived key (19 MiB memory, 3 iterations) and ChaCha20-Poly1305 authenticated encryption, written atomically with `0600` permissions.
2. **In-Memory Secrets:** Protected with `secrecy` wrappers and explicitly zeroized on drop and across intermediate buffers using `zeroize`.
3. **Local IPC Socket:** Access to the hook socket is restricted to the current user inside a `0700` private runtime directory (`$XDG_RUNTIME_DIR/tag/` or `~/.config/tag/run/`) with `0600` socket permissions, `SO_PEERCRED` UID verification, and a 1 MiB read bound.

### What TAG Protects Against
- **Disk Theft / Cold-Storage Inspection:** The vault file cannot be read without the user's master passphrase.
- **Accidental Leakage in Logs:** Secrets cannot be printed in `tracing` or error messages; debug formats are redacted via `secrecy::SecretString`, and logs are stored in `~/.config/tag/logs/` (`0700`).
- **Accidental Shell History Storage:** Secrets injected via process environment variables at PTY spawn time are never passed on command-line arguments or saved in `.bash_history` / `.zsh_history`.
- **Unauthorized Multi-User Access:** Other non-root users on the same Unix machine cannot read or write to the Unix domain socket, session state, or vault files.

### What TAG Does NOT Protect Against (Out-of-Scope Risks)
- **Same-UID Hostile Processes:** On Linux, any process running under the same user ID can inspect memory or read `/proc/<pid>/environ` (unless `ptrace_scope` is restricted). Environment variable injection is convenient for agent CLIs, but not immune to same-UID processes.
- **Compromised Root Account:** A privileged root user or kernel exploit can dump terminal memory or kernel state.
- **Compromised Child Agent Process:** If an agent tool or LLM is maliciously instructed to dump its own environment variables (`env` or `printenv`) to an external server, TAG cannot prevent the child process from transmitting what was explicitly injected into it.

## Best Practices
- Keep an idle auto-lock timeout configured in `config.toml`.
- Store only the credentials required for the active workspace.
- Review agent permissions carefully before granting command-execution privileges.

## Control Plane and Orchestration

TAG serves a local control socket (`tag-control-<pid>.sock`, see ADR 0005) used by `tag ctl`,
`tag mcp` and the agents TAG launches for orchestration runs.

### What it protects
- **Other users.** The socket is mode `0600` in a `0700` per-user directory, and every connection's
  peer UID is checked against the socket owner.
- **Vault secrets.** `pane.spawn` can name vault secrets to inject into a child's environment, but no
  control method returns secret values. A locked vault refuses injection (`PolicyBlocked`).
- **Confused or prompt-injected agents using TAG's tools.** Every method has a tier (`read`, `act`,
  `dangerous`). External assistants default to `read` (or `read`+`act` for `mcp`/`claude-desktop`);
  merges, kills, plan approval and run cancellation are `dangerous`. Agents launched for a run carry
  a scoped `TAG_CONTROL_TOKEN`:
  - a **worker** token can only read its own pane, test/diff its own worktree, search the brain,
    write notes and report on its own task. It cannot spawn agents, send input to other panes,
    merge or approve;
  - a **conductor** token can only manage its own run, cannot approve its own plan when approval is
    required, and can merge only under `merge_policy = "auto"` and only when the diff avoids
    `protected_paths`. Otherwise the merge waits for the human in the attention queue.
- **Untested merges.** `worktree.merge` requires the last `tests.run` in that worktree to have
  passed at the branch HEAD with a clean tree (unless the caller explicitly passes
  `require_tests: false`, which needs the `dangerous` tier). Conflicts abort the merge and leave the
  main worktree clean.
- **Accountability.** Every `act`/`dangerous` call is appended to
  `<state>/control-audit.jsonl` (time, client, role, run/task, method, params with prompts and
  pasted text truncated to 200 characters).

### What it does NOT protect against
- **Same-UID code that wants to bypass it.** Tiers and tokens are a guardrail, not a sandbox. Any
  process running as your user can connect to the socket without a token (as e.g. `ctl`, which has
  all tiers by default), read your files, or run `git merge` itself. Agents run with your
  permissions; prompt injection that makes an agent run arbitrary shell commands is out of scope for
  the control plane. Use the agents' own permission modes and sandboxes for that.
- **Prompt injection between agents.** A worker reads untrusted repository content and its output is
  shown to the conductor. TAG limits what a worker token can *do*, and the conductor reviews diffs
  and test results, but a conductor model can still be persuaded by text it reads. Keep
  `merge_policy = "ask"` (the default) and `require_plan_approval = true` for anything important.
- **Cost.** Budgets (`budget_usd`, `hard_budget`) are enforced only for usage TAG can attribute to a
  run (profiles where TAG chooses the session id, currently Claude Code). Other agents' spend is
  not counted toward a run's budget.

### Recommended settings
- Give external assistants `["read", "act"]` at most; keep `dangerous` with the human (`tag ctl` or
  the UI).
- Use `protected_paths` in `.tag/project.toml` for CI config, release scripts and agent instruction
  files (e.g. `[".github/", "AGENTS.md", "CLAUDE.md"]`).
- Review `control-audit.jsonl` after unattended runs.
