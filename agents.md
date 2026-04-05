# Séance — agents.md

## Overview

Séance is an open-source, cross-platform SSH terminal client inspired by Termius. It is built with **Rust**, uses **GPUI** (Zed's UI framework) for rendering, and **libghostty-vt** for terminal emulation. The project targets **macOS** and **Linux** as first-class platforms.

## Goals

* Provide a fast, native-feeling SSH client that rivals Termius on desktop
* Leverage libghostty-vt for battle-tested terminal emulation
* Use GPUI for GPU-accelerated, native UI on both platforms
* Ship as a single binary with no runtime dependencies
* Remain fully open source (MIT/Apache-2.0)

## Architecture

### Crate Structure

* `seance-app` — binary entry point, wires all crates together
* `seance-terminal` — libghostty-vt integration, terminal state, GPU grid renderer
* `seance-ssh` — SSH/SFTP client built on `russh`, session lifecycle, auth, tunnels
* `seance-ui` — GPUI-based UI shell: tabs, splits, host list, command palette, themes
* `seance-vault` — SQLite-backed storage for hosts, SSH keys, and snippets
* `seance-config` — app configuration, keybindings, theme definitions

### Key Dependencies

* `libghostty-vt-sys` — terminal VT parsing and state management
* `gpui` — UI framework (from the Zed editor project)
* `russh` / `russh-keys` / `russh-sftp` — SSH protocol
* `sqlx` (SQLite) — local encrypted storage
* `tokio` — async runtime

### Data Flow

User Input → GPUI Event → seance-ui → seance-ssh (write to channel)
↓
Remote Host (PTY)
↓
seance-ssh (read from channel)
↓
seance-terminal (libghostty-vt parse)
↓
seance-ui (GPUI render terminal grid)

## Core Features

### P0 — MVP

* Local shell terminal via libghostty-vt + GPUI renderer
* SSH connection to a remote host (password + key auth)
* Tabbed terminal sessions
* Host vault (save/edit/delete connections)
* Basic SFTP file transfer

### P1 — Usable Daily Driver

* Split panes (horizontal + vertical)
* Command palette (fuzzy search hosts, commands, actions)
* SSH agent forwarding
* Port forwarding (local + remote)
* Saved snippets (reusable commands)
* Encrypted key storage
* Theming (bundled dark/light + custom)
* Configurable keybindings

### P2 — Polish

* SSH config (`~/.ssh/config`) import
* Jump host / proxy support
* Search within terminal scrollback
* Broadcast input to multiple sessions
* Session logging / export
* Auto-reconnect on connection drop

## Platform Strategy

* **macOS** — primary development target, GPUI has strongest support here
* **Linux** — X11 and Wayland via GPUI's Linux backend
* **Windows** — deferred until GPUI Windows support matures
* **Mobile** — out of scope

## Milestones

| Milestone | Target | Deliverable |
|---|---|---|
| M0 | Week 1–2 | Window rendering a local shell via libghostty-vt + GPUI |
| M1 | Week 3–4 | SSH session piped into terminal |
| M2 | Week 5–6 | Host vault with SQLite persistence |
| M3 | Week 7–8 | Tabs and split panes |
| M4 | Week 9–10 | SFTP browser and saved snippets |
| M5 | Week 11–12 | Command palette, theming, keybindings, packaging |

## Agent Guidelines

* Always prefer existing Rust crates over writing from scratch
* Keep crate boundaries clean — no circular dependencies
* All SSH logic stays in `seance-ssh`, never in the UI layer
* Terminal state is owned by `seance-terminal`, UI only reads it for rendering
* Use `anyhow` for application errors, `thiserror` for library crate errors
* All async code runs on Tokio; GPUI runs on its own main thread
* Write integration tests for SSH against a local `sshd` in CI
* Target Rust stable, no nightly features
* Format with `rustfmt`, lint with `clippy`, deny all warnings in CI
