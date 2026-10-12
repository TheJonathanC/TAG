# Security Policy and Threat Model

TAG (Agent Cockpit) incorporates a secure local secrets vault and environment variable injection for agent workflows.

## Threat Model

### Assets Protected
1. **API Keys and Credentials at Rest:** Encrypted with Argon2id-derived key and ChaCha20-Poly1305 authenticated encryption.
2. **In-Memory Secrets:** Protected with `secrecy` wrappers and zeroized on drop using `zeroize`.
3. **Local IPC Socket:** Access to `cockpit.sock` is restricted to the current user (Unix file permissions `0600` inside a private runtime directory).

### What TAG Protects Against
- **Disk Theft / Cold-Storage Inspection:** The vault file cannot be read without the user's master passphrase.
- **Accidental Leakage in Logs:** Secrets cannot be printed in `tracing` or error messages; debug formats are redacted via `secrecy::SecretString`.
- **Accidental Shell History Storage:** Secrets injected via process environment variables at PTY spawn time are never passed on command-line arguments or saved in `.bash_history` / `.zsh_history`.
- **Unauthorized Multi-User Access:** Other non-root users on the same Unix machine cannot read the Unix domain socket or vault files.

### What TAG Does NOT Protect Against (Out-of-Scope Risks)
- **Same-UID Hostile Processes:** On Linux, any process running under the same user ID can inspect memory or read `/proc/<pid>/environ` (unless `ptrace_scope` is restricted). Environment variable injection is convenient for agent CLIs, but not immune to same-UID processes.
- **Compromised Root Account:** A privileged root user or kernel exploit can dump terminal memory or kernel state.
- **Compromised Child Agent Process:** If an agent tool or LLM is maliciously instructed to dump its own environment variables (`env` or `printenv`) to an external server, TAG cannot prevent the child process from transmitting what was explicitly injected into it.

## Best Practices
- Keep an idle auto-lock timeout configured in `config.toml`.
- Store only the credentials required for the active workspace.
- Review agent permissions carefully before granting command-execution privileges.
