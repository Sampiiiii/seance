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

Cargo workspace with resident lifecycle support and nine crates:

| Crate | Role |
|---|---|
| `seance-app` | Binary entry point. Elects the primary instance, starts IPC, and launches the UI host. |
| `seance-core` | Resident app state: session registry, lifecycle policy, vault/SSH service orchestration. |
| `seance-terminal` | libghostty-vt integration, terminal state, local shell via `portable-pty`. |
| `seance-ssh` | SSH client built on `russh`. Password/key auth, PTY session, resize, SFTP bootstrap. |
| `seance-ui` | GPUI window host. Terminal canvas, session list, vault management, command palette, themes. |
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
- **macOS** or **Linux**

## Getting started

```
git clone https://github.com/yourusername/seance.git
cd seance
make run
```

Other useful targets:

```
make run-perf           # Compact performance HUD
make run-perf-expanded  # Expanded performance HUD
make run-trace          # Tracing enabled
make check              # cargo check
make clippy             # Clippy with -D warnings
make test               # Run tests
make fmt                # Format code
```

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
