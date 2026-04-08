# Seance

An open-source, cross-platform SSH terminal client built with Rust, [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), and [libghostty-vt](https://github.com/Uzaaft/libghostty-rs).

> Early-stage project. Things work, things break, things change.

## What works today

- Local shell terminal (via `portable-pty` + libghostty-vt)
- SSH connections with password and private key auth (Ed25519, RSA, including encrypted keys)
- GPU-accelerated terminal rendering through GPUI
- Encrypted host vault (SQLite + ChaCha20-Poly1305, Argon2 KDF, OS keyring integration)
- Save, edit, and delete SSH hosts and credentials
- Generate and import SSH keys
- Command palette with fuzzy search
- Theming (dark/light)
- Terminal resize (local and remote)
- Performance HUD for debugging

## Architecture

Cargo workspace with resident lifecycle support and ten crates:

| Crate | Role |
| --- | --- |
| `seance-app` | Binary entry point. Elects the primary instance, starts IPC, and launches the UI host. |
| `seance-core` | Resident app state: session registry, lifecycle policy, vault/SSH service orchestration. |
| `seance-terminal` | libghostty-vt integration, terminal state, local shell via `portable-pty`. |
| `seance-ssh` | SSH client built on `russh`. Password/key auth, PTY session, resize, SFTP bootstrap. |
| `seance-ui` | GPUI window host. Terminal canvas, session list, vault management, command palette, themes, update UI. |
| `seance-updater` | Cross-platform update manager. Sparkle integration on macOS and AppImage release checks on Linux. |
| `seance-vault` | Encrypted SQLite storage for hosts, credentials, and SSH keys. |
| `seance-platform` | Cross-platform resident-app contracts, IPC protocol, and single-instance plumbing. |
| `seance-platform-macos` | macOS runtime shim for resident lifecycle integration. |
| `seance-platform-linux` | Linux runtime shim for resident lifecycle integration. |
| `seance-config` | App configuration. (Stub — not yet implemented.) |

Resident lifecycle behavior now works like this:

- the first launch becomes the primary resident process
- later launches signal the primary process over a local Unix socket instead of starting a second app instance
- sessions are owned by the resident controller, not by an individual window
- when the last window closes, the process stays alive and can reopen a new window on demand

## Prerequisites

- **Rust 1.93+** (pinned in `rust-toolchain.toml`, installed automatically by `rustup`)
- **Zig 0.15.2** for local build, clippy, and test paths that compile `seance-terminal`
- **macOS** or **Linux**

## Getting started

```bash
git clone https://github.com/sampiiiii/seance.git
cd seance
make app-run
```

`make app-run` uses `cargo run` and is fine for normal local development, but it is not a valid Touch ID test path on macOS. Touch ID-backed vault unlock requires a signed `Seance.app` bundle with keychain entitlements.

GitHub Actions installs Zig automatically for CI and release jobs that compile the vendored `libghostty-vt` stack. Local development still needs a matching Zig toolchain available on `PATH`.

Other useful targets:

```bash
make app-run
make app-run-perf
make app-run-perf-expanded
make debug-run
make debug-run-perf-expanded
make debug-lldb
make signed-build
make signed-run
make signed-debug
make logs-dir
make logs-latest
make logs-tail
make crash-latest
make check
make clippy
make test
make fmt
```

For launch-crash debugging on macOS:

```bash
make debug-run      # baseline repro with tracing + full Rust backtraces
make signed-debug   # signed-app repro for Touch ID / entitlement paths
make logs-latest    # newest launch log path
make logs-tail      # tail the newest launch log
make crash-latest   # newest macOS crash report for Seance
```

For local Touch ID verification on macOS, create a local signing file once and use the signed app path instead of `cargo run`:

```bash
cp .env.macos-signing.example .env.macos-signing
# edit .env.macos-signing with your Apple team id, Apple Development identity,
# and a macOS development provisioning profile for com.seance.app.dev
make signed-run
```

You can verify the resulting app entitlements with:

```bash
codesign -d --entitlements :- dist/dev-macos/Seance.app
security cms -D -i dist/dev-macos/Seance.app/Contents/embedded.provisionprofile
```

If you previously enrolled device unlock from an unsigned or older build, unlock once with the recovery passphrase in the signed app to re-enroll this device, then relaunch and test Touch ID.

Local macOS Touch ID setup requires:

- an App ID for `com.seance.app.dev`
- a macOS development provisioning profile for that App ID
- `APPLE_DEV_PROVISIONING_PROFILE` set in `.env.macos-signing`

`make signed-run` and `make signed-build` will automatically load `.env.macos-signing` if it exists. If it does not, they still accept explicit `APPLE_TEAM_ID`, `APPLE_DEVELOPMENT_SIGNING_IDENTITY`, and `APPLE_DEV_PROVISIONING_PROFILE` environment variables.

Release metadata and artifact naming now go through the Rust build helper:

```bash
make release-version
make release-notes VERSION=0.1.0
make release-artifacts
make release-validate RELEASE_DIR=dist/release
make release-checksums RELEASE_DIR=dist/release
```

## Release pipeline

GitHub Actions now drives CI and release packaging:

- `.github/workflows/ci.yml` runs `fmt`, `clippy`, and workspace tests on Linux and macOS
- `.github/workflows/release.yml` builds tagged releases, uploads GitHub Release assets, and publishes the Sparkle appcast to GitHub Pages
- Linux release artifacts are AppImages for `x86_64` and `aarch64`
- macOS release artifacts are a signed/notarized `dmg` plus a Sparkle update zip for Apple Silicon

The build jobs provision Zig 0.15.2 before compiling Rust because the vendored `libghostty-vt-sys` build invokes Ghostty's `zig build` path during compilation.

The canonical interface for release metadata is `cargo run -p seance-build -- <subcommand>`. The remaining scripts under `scripts/` are platform packaging wrappers for tools such as `cargo-packager`, `codesign`, `notarytool`, `linuxdeploy`, and `appimagetool`.

See [docs/RELEASE.md](docs/RELEASE.md) for the release architecture, manifest model, and diagrams.
See [docs/RELEASE-RUNBOOK.md](docs/RELEASE-RUNBOOK.md) for the operator runbook, preflight checklist, and recovery steps.

Hosted vault sync and multiplayer backend design docs live under `docs/` as well:

- [docs/VAULT-SYNC-ARCHITECTURE.md](docs/VAULT-SYNC-ARCHITECTURE.md)
- [docs/VAULT-SYNC-PROTOCOL.md](docs/VAULT-SYNC-PROTOCOL.md)
- [docs/VAULT-SYNC-DATA-MODEL.md](docs/VAULT-SYNC-DATA-MODEL.md)
- [docs/VAULT-SYNC-THREAT-MODEL.md](docs/VAULT-SYNC-THREAT-MODEL.md)
- [docs/VAULT-SYNC-RUNBOOK.md](docs/VAULT-SYNC-RUNBOOK.md)
- [docs/VAULT-MULTIPLAYER-ARCHITECTURE.md](docs/VAULT-MULTIPLAYER-ARCHITECTURE.md)

## Roadmap

Not built yet, roughly in priority order:

- Tabbed sessions
- Split panes
- Port forwarding (local + remote)
- SSH agent forwarding
- Saved command snippets
- `~/.ssh/config` import
- Jump host / proxy support
- Configurable keybindings
- Session logging
- CI pipeline

## License

Licensed under the [GNU General Public License v3.0](LICENSE).
