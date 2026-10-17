# AGENTS.md: Developer & Agent Reference Guide for TAG

This document establishes operational rules, release policies, architecture conventions, and workflows for all AI coding assistants (Antigravity, Claude Code, Codex, Aider) and human contributors working on **TAG**.

---

## 1. Critical Release & Versioning Policy (MUST READ)

> [!IMPORTANT]
> **STRICT RULE ON STABLE VS. BETA RELEASES**:
> - **Default to BETA**: Not all changes or preview builds are stable. All preview tags, test releases, and routine builds MUST use beta versioning (e.g., `v0.1.0-beta.1`, `v0.1.0-beta.2`, `v0.2.0-beta.1`).
> - **STABLE ONLY ON EXPLICIT INSTRUCTION**: **NEVER** tag, build, or publish a **STABLE** release (e.g. `v0.1.0`, `v0.2.0`) unless Jonathan explicitly instructs you to push a stable version.
> - **Pre-release Flagging**: In GitHub Releases, any version containing `-beta`, `-alpha`, or `-rc` is automatically flagged as a **Pre-release** (`prerelease: true`, `make_latest: false`). Only authorized stable releases are marked as the latest full release.
> - **crates.io Gate**: Publishing to crates.io is strictly gated to **STABLE** releases and will never execute on beta tags.

### Versioning Format
| Release Type | Tag Convention | GitHub Release State | Crates.io Publishing |
| :--- | :--- | :--- | :--- |
| **Beta (Pre-release)** | `vX.Y.Z-beta.N` (e.g. `v0.1.0-beta.1`) | `prerelease: true`, `make_latest: false` | Disabled |
| **Stable (Production)** | `vX.Y.Z` (e.g. `v0.1.0`) | `prerelease: false`, `make_latest: true` | Enabled (with `CRATES_IO_TOKEN`) |

---

## 2. Multi-Binary Workspace Architecture

TAG is composed of 13 modular crates in `crates/` producing **three** binaries in `crates/bin`:
1. `tag`: The main multiplexer, PTY process engine, and autonomous agent supervisor.
2. `cockpit-notify`: CLI tool for external scripts/hooks to send desktop and attention queue notifications to TAG.
3. `tag-notify`: Desktop notification daemon helper using `notify-rust`.

When building release archives, **all three binaries** must always be bundled together along with `README.md`.

---

## 3. In-App Versioning & Update Detection

TAG features built-in version awareness and self-updating:
- **Version Display**:
  - **Status Bar**: Displayed on the right side of the bottom status bar (`v0.1.0`).
  - **Help Overlay** (`?` / `^Space ?`): Title displays `TAG Complete Feature Guide & Help v0.1.0`.
  - **Empty State**: Centered subtitle displays `// TERMINAL AGENT ORCHESTRATOR & MULTIPLEXER  v0.1.0 //`.
  - **CLI**: `tag --version` and `tag -V`.
- **Automatic Update Detection**:
  - On launch, TAG spawns a detached background check against GitHub Releases API (`https://api.github.com/repos/TheJonathanC/TAG/releases`).
  - Users on a stable build only receive notifications for newer stable releases. Users on a beta build receive notifications for both newer betas and stable releases.
  - When an update is detected:
    - Status bar displays `[▲ v0.2.0]` badge.
    - Toast notification informs the user: `Update Available: TAG v0.2.0 is available! Press Leader+U or run 'tag update' to upgrade.`
- **Self-Updating**:
  - Users can run `tag update` from their terminal (or `tag update --beta`).
  - Shortcut `Leader+U` (`^Space U`) or Command Palette `system.update` guides users on updating.

---

## 4. Distribution Workflows

### CI Workflow (`.github/workflows/ci.yml`)
- Triggered on: Push to `main` and Pull Requests targeting `main`.
- Validations:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo check --workspace --all-targets`
  - `cargo test --workspace` on Ubuntu and macOS runners.
  - Caches Rust dependencies with `Swatinem/rust-cache`.

### Release Workflow (`.github/workflows/release.yml`)
- Triggered on:
  - Tag push matching `v*` (e.g. `v0.1.0-beta.1` or `v0.1.0`).
  - Manual trigger via `workflow_dispatch` (includes channel dropdown: `beta` vs `stable`).
- Target Matrix:
  - `x86_64-unknown-linux-gnu` (Linux x86_64 glibc)
  - `x86_64-unknown-linux-musl` (Linux x86_64 statically linked musl, runs on any Linux distribution)
  - `aarch64-unknown-linux-gnu` (Linux ARM64 / Raspberry Pi / AWS Graviton, built via `cross`)
  - `aarch64-apple-darwin` (macOS Apple Silicon M1/M2/M3/M4)
  - `x86_64-apple-darwin` (macOS Intel)
- Artifacts:
  - `tag-${VERSION}-${TARGET}.tar.gz` (containing `tag`, `cockpit-notify`, `tag-notify`, `README.md`)
  - `tag-${VERSION}-${TARGET}.tar.gz.sha256`
  - Consolidated `SHA256SUMS.txt`
- crates.io Publication:
  - Automatically triggered for stable releases when `CRATES_IO_TOKEN` repository secret is present.
  - Publishes crates in topological dependency order (`tag-core` -> foundational crates -> `tag`).

### One-Line Shell Installer (`scripts/install.sh`)
```bash
# Install latest stable release
curl -fsSL https://raw.githubusercontent.com/TheJonathanC/TAG/main/scripts/install.sh | bash

# Install latest beta release
curl -fsSL https://raw.githubusercontent.com/TheJonathanC/TAG/main/scripts/install.sh | bash -s -- --beta

# Install specific version
curl -fsSL https://raw.githubusercontent.com/TheJonathanC/TAG/main/scripts/install.sh | bash -s -- --version v0.1.0
```

---

## 5. Standard Quality Gates & Verification Commands

Before committing or pushing any changes, verify that the following checks pass cleanly:

```bash
# 1. Format check
cargo fmt --all -- --check

# 2. Clippy lint
cargo clippy --workspace --all-targets -- -D warnings

# 3. Unit and integration tests
cargo test --workspace

# 4. Verify binary versions and help flags
cargo run -p tag -- --version
cargo run -p tag -- --help
cargo run -p tag -- update --help
```
