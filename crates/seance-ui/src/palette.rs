use crate::{connect::PendingConnectSummary, theme::ThemeId};
use seance_config::{
    COMMAND_APP_OPEN_PREFERENCES, COMMAND_SESSION_CLOSE_ACTIVE, COMMAND_SESSION_NEW_LOCAL,
};
use std::{collections::HashMap, sync::Arc};

use seance_core::{
    VaultScopedCredentialSummary, VaultScopedHostSummary, VaultScopedKeySummary,
    VaultScopedPortForwardSummary,
};
use seance_ssh::PortForwardRuntimeSnapshot;
use seance_terminal::TerminalSession;
use seance_vault::PrivateKeyAlgorithm;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PaletteGroup {
    Sessions,
    Hosts,
    Tunnels,
    Vault,
    Appearance,
    System,
}

impl PaletteGroup {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            PaletteGroup::Sessions => "SESSIONS",
            PaletteGroup::Hosts => "HOSTS",
            PaletteGroup::Tunnels => "TUNNELS",
            PaletteGroup::Vault => "VAULT",
            PaletteGroup::Appearance => "APPEARANCE",
            PaletteGroup::System => "SYSTEM",
        }
    }
}

#[derive(Clone)]
pub(crate) enum PaletteAction {
    NewLocalTerminal,
    CheckForUpdates,
    InstallAvailableUpdate,
    SwitchSession(u64),
    CloseActiveSession,
    SwitchTheme(ThemeId),
    UnlockVault,
    LockVault,
    OpenVaultPanel,
    AddSavedHost,
    OpenNewHostWizard,
    AddPasswordCredential,
    EditPasswordCredential {
        vault_id: String,
        credential_id: String,
    },
    DeletePasswordCredential {
        vault_id: String,
        credential_id: String,
    },
    GenerateEd25519Key,
    GenerateRsaKey,
    ImportPrivateKeyFiles,
    DiscoverPrivateKeys,
    PastePrivateKey,
    DeletePrivateKey {
        vault_id: String,
        key_id: String,
    },
    EditSavedHost {
        vault_id: String,
        host_id: String,
    },
    DeleteSavedHost {
        vault_id: String,
        host_id: String,
    },
    CancelSavedHostConnect {
        attempt_id: u64,
    },
    ConnectSavedHost {
        vault_id: String,
        host_id: String,
    },
    OpenSftpBrowser(u64),
    OpenTunnelManager,
    OpenHostTunnelSettings {
        vault_id: String,
        host_id: String,
    },
    StartTunnel {
        vault_id: String,
        port_forward_id: String,
    },
    StopTunnel {
        tunnel_scope_key: String,
    },
    OpenPreferences,
}

#[derive(Clone)]
pub(crate) struct PaletteItem {
    pub(crate) glyph: &'static str,
    pub(crate) label: String,
    pub(crate) hint: String,
    pub(crate) action: PaletteAction,
    pub(crate) group: PaletteGroup,
    pub(crate) shortcut_command: Option<&'static str>,
    pub(crate) match_indices: Vec<usize>,
}

#[derive(Clone)]
pub(crate) enum PaletteRow {
    Header(PaletteGroup),
    Item {
        palette_index: usize,
        item: PaletteItem,
    },
}

pub(crate) struct PaletteViewModel {
    pub(crate) items: Vec<PaletteItem>,
    pub(crate) rows: Vec<PaletteRow>,
    pub(crate) item_to_row: Vec<usize>,
    pub(crate) row_to_item: Vec<Option<usize>>,
}

#[derive(Clone, Copy)]
pub(crate) enum PageDirection {
    Up,
    Down,
}

fn fuzzy_score(haystack: &str, needle: &str) -> Option<(i32, Vec<usize>)> {
    if needle.is_empty() {
        return Some((0, Vec::new()));
    }

    let hay: Vec<char> = haystack.chars().collect();
    let need: Vec<char> = needle.chars().collect();
    let mut score: i32 = 0;
    let mut indices = Vec::with_capacity(need.len());
    let mut hay_idx = 0;
    let mut prev_match = false;
    let mut consecutive = 0i32;

    for &nc in &need {
        let nc_lower = nc.to_lowercase().next().unwrap_or(nc);
        let mut found = false;
        while hay_idx < hay.len() {
            let hc = hay[hay_idx];
            let hc_lower = hc.to_lowercase().next().unwrap_or(hc);
            hay_idx += 1;

            if hc_lower == nc_lower {
                indices.push(hay_idx - 1);

                if prev_match {
                    consecutive += 1;
                    score += 4 + consecutive;
                } else {
                    consecutive = 0;
                    score += 1;
                }

                let is_word_start = hay_idx == 1
                    || hay.get(hay_idx.wrapping_sub(2)).map_or(false, |prev| {
                        *prev == ' ' || *prev == '_' || *prev == '-' || *prev == ':'
                    });
                if is_word_start {
                    score += 6;
                }

                if hay_idx - 1 == 0 {
                    score += 8;
                }

                if hc == nc {
                    score += 1;
                }

                prev_match = true;
                found = true;
                break;
            } else {
                prev_match = false;
                consecutive = 0;
            }
        }
        if !found {
            return None;
        }
    }

    Some((score, indices))
}

pub(crate) fn build_items(
    sessions: &[Arc<dyn TerminalSession>],
    session_labels: &HashMap<u64, String>,
    saved_hosts: &[VaultScopedHostSummary],
    pending_connects: &[PendingConnectSummary],
    credentials: &[VaultScopedCredentialSummary],
    keys: &[VaultScopedKeySummary],
    port_forwards: &[VaultScopedPortForwardSummary],
    active_port_forwards: &[PortForwardRuntimeSnapshot],
    active_id: u64,
    active_theme: ThemeId,
    query: &str,
    vault_unlocked: bool,
    remote_session_ids: &[u64],
    update_state: &seance_core::UpdateState,
) -> Vec<PaletteItem> {
    let mut items: Vec<PaletteItem> = Vec::new();
    let pending_by_scope = pending_connects
        .iter()
        .map(|pending| (pending.host_scope_key.as_str(), pending))
        .collect::<HashMap<_, _>>();
    let active_tunnels = active_port_forwards
        .iter()
        .map(|snapshot| (snapshot.id.as_str(), snapshot))
        .collect::<HashMap<_, _>>();

    // --- Sessions group ---

    items.push(PaletteItem {
        glyph: "+",
        label: "New Local Terminal".into(),
        hint: "Spawn a new shell session".into(),
        action: PaletteAction::NewLocalTerminal,
        group: PaletteGroup::Sessions,
        shortcut_command: Some(COMMAND_SESSION_NEW_LOCAL),
        match_indices: Vec::new(),
    });

    for session in sessions {
        if session.id() != active_id {
            let label = session_labels
                .get(&session.id())
                .cloned()
                .unwrap_or_else(|| session.title().to_string());
            items.push(PaletteItem {
                glyph: "›",
                label: format!("Switch to {label}"),
                hint: format!("session #{}", session.id()),
                action: PaletteAction::SwitchSession(session.id()),
                group: PaletteGroup::Sessions,
                shortcut_command: None,
                match_indices: Vec::new(),
            });
        }
    }

    if !sessions.is_empty() {
        items.push(PaletteItem {
            glyph: "×",
            label: "Close Active Session".into(),
            hint: "Terminate the current terminal".into(),
            action: PaletteAction::CloseActiveSession,
            group: PaletteGroup::Sessions,
            shortcut_command: Some(COMMAND_SESSION_CLOSE_ACTIVE),
            match_indices: Vec::new(),
        });
    }

    for session in sessions {
        if remote_session_ids.contains(&session.id()) {
            let label = session_labels
                .get(&session.id())
                .cloned()
                .unwrap_or_else(|| session.title().to_string());
            items.push(PaletteItem {
                glyph: "\u{25a4}",
                label: format!("Browse Files: {label}"),
                hint: "Open SFTP file browser".into(),
                action: PaletteAction::OpenSftpBrowser(session.id()),
                group: PaletteGroup::Sessions,
                shortcut_command: None,
                match_indices: Vec::new(),
            });
        }
    }

    // --- Hosts group ---

    if vault_unlocked {
        items.push(PaletteItem {
            glyph: "◈",
            label: "Add Saved Host".into(),
            hint: "Store an encrypted SSH config".into(),
            action: PaletteAction::AddSavedHost,
            group: PaletteGroup::Hosts,
            shortcut_command: None,
            match_indices: Vec::new(),
        });
        items.push(PaletteItem {
            glyph: "☰",
            label: "New Host Wizard".into(),
            hint: "Guided host setup with auth and review".into(),
            action: PaletteAction::OpenNewHostWizard,
            group: PaletteGroup::Hosts,
            shortcut_command: None,
            match_indices: Vec::new(),
        });

        for host in saved_hosts {
            let scope_key = crate::workspace::host_scope_key(&host.vault_id, &host.host.id);
            let target = format!(
                "{}@{}:{}",
                host.host.username, host.host.hostname, host.host.port
            );
            if let Some(pending) = pending_by_scope.get(scope_key.as_str()) {
                items.push(PaletteItem {
                    glyph: "×",
                    label: format!(
                        "Cancel Connect: {} [{}]",
                        pending.host_label, host.vault_name
                    ),
                    hint: format!("connecting to {target}"),
                    action: PaletteAction::CancelSavedHostConnect {
                        attempt_id: pending.id,
                    },
                    group: PaletteGroup::Hosts,
                    shortcut_command: None,
                    match_indices: Vec::new(),
                });
            } else {
                items.push(PaletteItem {
                    glyph: "→",
                    label: format!("Connect: {} [{}]", host.host.label, host.vault_name),
                    hint: target.clone(),
                    action: PaletteAction::ConnectSavedHost {
                        vault_id: host.vault_id.clone(),
                        host_id: host.host.id.clone(),
                    },
                    group: PaletteGroup::Hosts,
                    shortcut_command: None,
                    match_indices: Vec::new(),
                });
            }
            items.push(PaletteItem {
                glyph: "✎",
                label: format!("Edit: {} [{}]", host.host.label, host.vault_name),
                hint: "Update the encrypted record".into(),
                action: PaletteAction::EditSavedHost {
                    vault_id: host.vault_id.clone(),
                    host_id: host.host.id.clone(),
                },
                group: PaletteGroup::Hosts,
                shortcut_command: None,
                match_indices: Vec::new(),
            });
            items.push(PaletteItem {
                glyph: "×",
                label: format!("Delete: {} [{}]", host.host.label, host.vault_name),
                hint: "Remove this saved host".into(),
                action: PaletteAction::DeleteSavedHost {
                    vault_id: host.vault_id.clone(),
                    host_id: host.host.id.clone(),
                },
                group: PaletteGroup::Hosts,
                shortcut_command: None,
                match_indices: Vec::new(),
            });
        }
    }

    // --- Tunnels group ---

    if vault_unlocked {
        items.push(PaletteItem {
            glyph: "⇄",
            label: "Open Tunnel Manager".into(),
            hint: "Manage saved port forwarding rules".into(),
            action: PaletteAction::OpenTunnelManager,
            group: PaletteGroup::Tunnels,
            shortcut_command: None,
            match_indices: Vec::new(),
        });

        let mut host_tunnel_targets = HashMap::<(&str, &str), usize>::new();
        for port_forward in port_forwards {
            host_tunnel_targets
                .entry((&port_forward.vault_id, &port_forward.host_id))
                .and_modify(|count| *count += 1)
                .or_insert(1);

            let scope_key = crate::workspace::item_scope_key(
                &port_forward.vault_id,
                &port_forward.port_forward.id,
            );
            let target = format!(
                "{}:{} -> {}:{}",
                port_forward.port_forward.listen_address,
                port_forward.port_forward.listen_port,
                port_forward.port_forward.target_address,
                port_forward.port_forward.target_port
            );
            if active_tunnels.contains_key(scope_key.as_str()) {
                items.push(PaletteItem {
                    glyph: "×",
                    label: format!(
                        "Stop Tunnel: {} [{} / {}]",
                        port_forward.port_forward.label,
                        port_forward.host_label,
                        port_forward.vault_name
                    ),
                    hint: target.clone(),
                    action: PaletteAction::StopTunnel {
                        tunnel_scope_key: scope_key,
                    },
                    group: PaletteGroup::Tunnels,
                    shortcut_command: None,
                    match_indices: Vec::new(),
                });
            } else {
                items.push(PaletteItem {
                    glyph: "→",
                    label: format!(
                        "Start Tunnel: {} [{} / {}]",
                        port_forward.port_forward.label,
                        port_forward.host_label,
                        port_forward.vault_name
                    ),
                    hint: target.clone(),
                    action: PaletteAction::StartTunnel {
                        vault_id: port_forward.vault_id.clone(),
                        port_forward_id: port_forward.port_forward.id.clone(),
                    },
                    group: PaletteGroup::Tunnels,
                    shortcut_command: None,
                    match_indices: Vec::new(),
                });
            }
        }

        for host in saved_hosts {
            if host_tunnel_targets.contains_key(&(host.vault_id.as_str(), host.host.id.as_str())) {
                items.push(PaletteItem {
                    glyph: "⚙",
                    label: format!(
                        "Host Tunnel Settings: {} [{}]",
                        host.host.label, host.vault_name
                    ),
                    hint: "Jump to linked port forwarding rules".into(),
                    action: PaletteAction::OpenHostTunnelSettings {
                        vault_id: host.vault_id.clone(),
                        host_id: host.host.id.clone(),
                    },
                    group: PaletteGroup::Tunnels,
                    shortcut_command: None,
                    match_indices: Vec::new(),
                });
            }
        }
    }

    // --- Vault group ---

    if vault_unlocked {
        items.push(PaletteItem {
            glyph: "◈",
            label: "Open Vault Panel".into(),
            hint: "Manage credentials and SSH keys".into(),
            action: PaletteAction::OpenVaultPanel,
            group: PaletteGroup::Vault,
            shortcut_command: None,
            match_indices: Vec::new(),
        });
        items.push(PaletteItem {
            glyph: "+",
            label: "Add Password Credential".into(),
            hint: "Store an encrypted password".into(),
            action: PaletteAction::AddPasswordCredential,
            group: PaletteGroup::Vault,
            shortcut_command: None,
            match_indices: Vec::new(),
        });
        items.push(PaletteItem {
            glyph: "+",
            label: "Generate Ed25519 Key".into(),
            hint: "Create a new vault-backed Ed25519 key".into(),
            action: PaletteAction::GenerateEd25519Key,
            group: PaletteGroup::Vault,
            shortcut_command: None,
            match_indices: Vec::new(),
        });
        items.push(PaletteItem {
            glyph: "+",
            label: "Generate RSA Key".into(),
            hint: "Create a new vault-backed RSA key".into(),
            action: PaletteAction::GenerateRsaKey,
            group: PaletteGroup::Vault,
            shortcut_command: None,
            match_indices: Vec::new(),
        });
        items.push(PaletteItem {
            glyph: "⤓",
            label: "Import Key Files".into(),
            hint: "Choose one or more private key files".into(),
            action: PaletteAction::ImportPrivateKeyFiles,
            group: PaletteGroup::Vault,
            shortcut_command: None,
            match_indices: Vec::new(),
        });
        items.push(PaletteItem {
            glyph: "⌂",
            label: "Discover ~/.ssh Keys".into(),
            hint: "Scan ~/.ssh and import available private keys".into(),
            action: PaletteAction::DiscoverPrivateKeys,
            group: PaletteGroup::Vault,
            shortcut_command: None,
            match_indices: Vec::new(),
        });
        items.push(PaletteItem {
            glyph: "¶",
            label: "Paste Private Key".into(),
            hint: "Import a PEM key from clipboard text".into(),
            action: PaletteAction::PastePrivateKey,
            group: PaletteGroup::Vault,
            shortcut_command: None,
            match_indices: Vec::new(),
        });

        for cred in credentials {
            let hint_str = cred
                .credential
                .username_hint
                .as_deref()
                .unwrap_or("password credential");
            items.push(PaletteItem {
                glyph: "✎",
                label: format!("Edit: {} [{}]", cred.credential.label, cred.vault_name),
                hint: hint_str.to_string(),
                action: PaletteAction::EditPasswordCredential {
                    vault_id: cred.vault_id.clone(),
                    credential_id: cred.credential.id.clone(),
                },
                group: PaletteGroup::Vault,
                shortcut_command: None,
                match_indices: Vec::new(),
            });
            items.push(PaletteItem {
                glyph: "×",
                label: format!("Delete: {} [{}]", cred.credential.label, cred.vault_name),
                hint: "Remove this credential".into(),
                action: PaletteAction::DeletePasswordCredential {
                    vault_id: cred.vault_id.clone(),
                    credential_id: cred.credential.id.clone(),
                },
                group: PaletteGroup::Vault,
                shortcut_command: None,
                match_indices: Vec::new(),
            });
        }

        for key in keys {
            let algo = match &key.key.algorithm {
                PrivateKeyAlgorithm::Ed25519 => "Ed25519",
                PrivateKeyAlgorithm::Rsa { bits } => {
                    if *bits == 4096 {
                        "RSA-4096"
                    } else {
                        "RSA"
                    }
                }
            };
            items.push(PaletteItem {
                glyph: "×",
                label: format!("Delete Key: {} [{}]", key.key.label, key.vault_name),
                hint: algo.to_string(),
                action: PaletteAction::DeletePrivateKey {
                    vault_id: key.vault_id.clone(),
                    key_id: key.key.id.clone(),
                },
                group: PaletteGroup::Vault,
                shortcut_command: None,
                match_indices: Vec::new(),
            });
        }

        items.push(PaletteItem {
            glyph: "◆",
            label: "Lock Vault".into(),
            hint: "Remove decrypted keys from memory".into(),
            action: PaletteAction::LockVault,
            group: PaletteGroup::Vault,
            shortcut_command: None,
            match_indices: Vec::new(),
        });
    } else {
        items.push(PaletteItem {
            glyph: "◇",
            label: "Unlock Vault".into(),
            hint: "Use your passphrase or enrolled device".into(),
            action: PaletteAction::UnlockVault,
            group: PaletteGroup::Vault,
            shortcut_command: None,
            match_indices: Vec::new(),
        });
    }

    // --- Appearance group ---

    for &tid in ThemeId::ALL {
        if tid != active_theme {
            let theme = tid.theme();
            items.push(PaletteItem {
                glyph: "◑",
                label: format!("Theme: {}", theme.name),
                hint: "Switch appearance".into(),
                action: PaletteAction::SwitchTheme(tid),
                group: PaletteGroup::Appearance,
                shortcut_command: None,
                match_indices: Vec::new(),
            });
        }
    }

    // --- System group ---

    items.push(PaletteItem {
        glyph: "⚙",
        label: "Open Preferences".into(),
        hint: "App settings and configuration".into(),
        action: PaletteAction::OpenPreferences,
        group: PaletteGroup::System,
        shortcut_command: Some(COMMAND_APP_OPEN_PREFERENCES),
        match_indices: Vec::new(),
    });

    items.push(PaletteItem {
        glyph: "↑",
        label: "Check for Updates".into(),
        hint: "Query the stable release channel".into(),
        action: PaletteAction::CheckForUpdates,
        group: PaletteGroup::System,
        shortcut_command: None,
        match_indices: Vec::new(),
    });

    if matches!(update_state, seance_core::UpdateState::Available(_)) {
        items.push(PaletteItem {
            glyph: "⇪",
            label: "Install Available Update".into(),
            hint: "Download and install the newest release".into(),
            action: PaletteAction::InstallAvailableUpdate,
            group: PaletteGroup::System,
            shortcut_command: None,
            match_indices: Vec::new(),
        });
    }

    // --- Filtering / fuzzy search ---

    if !query.is_empty() {
        let mut scored: Vec<(i32, PaletteItem)> = items
            .into_iter()
            .filter_map(|mut item| {
                let label_result = fuzzy_score(&item.label, query);
                let hint_result = fuzzy_score(&item.hint, query);

                match (label_result, hint_result) {
                    (Some((ls, indices)), _) => {
                        item.match_indices = indices;
                        Some((ls + 10, item))
                    }
                    (None, Some((hs, _))) => Some((hs, item)),
                    (None, None) => None,
                }
            })
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0));
        items = scored.into_iter().map(|(_, item)| item).collect();
    }

    items
}

pub(crate) fn flatten_items(items: Vec<PaletteItem>, show_groups: bool) -> PaletteViewModel {
    let mut rows = Vec::new();
    let mut item_to_row = Vec::with_capacity(items.len());
    let mut row_to_item = Vec::new();
    let mut prev_group = None;

    for (palette_index, item) in items.iter().cloned().enumerate() {
        if show_groups && prev_group != Some(item.group) {
            rows.push(PaletteRow::Header(item.group));
            row_to_item.push(None);
            prev_group = Some(item.group);
        }

        item_to_row.push(rows.len());
        rows.push(PaletteRow::Item {
            palette_index,
            item,
        });
        row_to_item.push(Some(palette_index));
    }

    PaletteViewModel {
        items,
        rows,
        item_to_row,
        row_to_item,
    }
}

pub(crate) fn find_item_at_or_after(
    row_to_item: &[Option<usize>],
    target_row: usize,
) -> Option<usize> {
    row_to_item
        .iter()
        .skip(target_row)
        .flatten()
        .copied()
        .next()
}

pub(crate) fn find_item_at_or_before(
    row_to_item: &[Option<usize>],
    target_row: usize,
) -> Option<usize> {
    row_to_item
        .iter()
        .take(target_row.saturating_add(1))
        .rev()
        .flatten()
        .copied()
        .next()
}

pub(crate) fn page_target_index(
    row_to_item: &[Option<usize>],
    item_to_row: &[usize],
    current_item: usize,
    visible_row_span: usize,
    direction: PageDirection,
) -> usize {
    if item_to_row.is_empty() {
        return 0;
    }

    let current_row = item_to_row[current_item.min(item_to_row.len().saturating_sub(1))];
    let span = visible_row_span.max(1);
    let target_row = match direction {
        PageDirection::Down => current_row
            .saturating_add(span)
            .min(row_to_item.len().saturating_sub(1)),
        PageDirection::Up => current_row.saturating_sub(span),
    };

    match direction {
        PageDirection::Down => find_item_at_or_after(row_to_item, target_row)
            .or_else(|| find_item_at_or_before(row_to_item, row_to_item.len().saturating_sub(1)))
            .unwrap_or(0),
        PageDirection::Up => find_item_at_or_before(row_to_item, target_row)
            .or_else(|| find_item_at_or_after(row_to_item, 0))
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, mpsc};

    use anyhow::Result;
    use seance_terminal::{
        SessionPerfSnapshot, SessionSummary, TerminalGeometry, TerminalKeyEvent,
        TerminalMouseEvent, TerminalPaste, TerminalScrollCommand, TerminalSession,
        TerminalTextEvent, TerminalViewportSnapshot,
    };

    struct StubSession {
        id: u64,
        title: String,
    }

    impl TerminalSession for StubSession {
        fn id(&self) -> u64 {
            self.id
        }

        fn title(&self) -> &str {
            &self.title
        }

        fn summary(&self) -> SessionSummary {
            SessionSummary::default()
        }

        fn viewport_snapshot(&self) -> TerminalViewportSnapshot {
            TerminalViewportSnapshot::default()
        }

        fn send_input(&self, _bytes: Vec<u8>) -> Result<()> {
            Ok(())
        }

        fn send_text(&self, _event: TerminalTextEvent) -> Result<()> {
            Ok(())
        }

        fn send_key(&self, _event: TerminalKeyEvent) -> Result<()> {
            Ok(())
        }

        fn send_mouse(&self, _event: TerminalMouseEvent) -> Result<()> {
            Ok(())
        }

        fn paste(&self, _paste: TerminalPaste) -> Result<()> {
            Ok(())
        }

        fn resize(&self, _geometry: TerminalGeometry) -> Result<()> {
            Ok(())
        }

        fn scroll_viewport(&self, _command: TerminalScrollCommand) -> Result<()> {
            Ok(())
        }

        fn scroll_to_bottom(&self) -> Result<()> {
            Ok(())
        }

        fn perf_snapshot(&self) -> SessionPerfSnapshot {
            SessionPerfSnapshot::default()
        }

        fn take_notify_rx(&self) -> Option<mpsc::Receiver<()>> {
            None
        }
    }

    #[test]
    fn fuzzy_exact_prefix() {
        let (score, indices) = fuzzy_score("New Local Terminal", "new").unwrap();
        assert!(score > 0);
        assert_eq!(indices, vec![0, 1, 2]);
    }

    #[test]
    fn fuzzy_skips() {
        let result = fuzzy_score("Generate Ed25519 Key", "genkey");
        assert!(result.is_some());
        let (_, indices) = result.unwrap();
        assert_eq!(indices[0], 0); // 'G'
    }

    #[test]
    fn fuzzy_no_match() {
        assert!(fuzzy_score("Lock Vault", "zzz").is_none());
    }

    #[test]
    fn build_items_uses_ui_session_labels_for_palette_entries() {
        let sessions: Vec<Arc<dyn TerminalSession>> = vec![Arc::new(StubSession {
            id: 18,
            title: "local-18".into(),
        })];
        let session_labels = HashMap::from([(18, "local-1".to_string())]);

        let items = build_items(
            &sessions,
            &session_labels,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            999,
            ThemeId::ObsidianSmoke,
            "",
            false,
            &[],
            &seance_core::UpdateState::Idle,
        );

        let switch_item = items
            .into_iter()
            .find(|item| matches!(item.action, PaletteAction::SwitchSession(18)))
            .expect("switch item");

        assert_eq!(switch_item.label, "Switch to local-1");
        assert_eq!(switch_item.hint, "session #18");
    }

    #[test]
    fn flatten_items_includes_group_headers_and_row_mappings() {
        let items = vec![
            PaletteItem {
                glyph: "+",
                label: "New Local Terminal".into(),
                hint: "Spawn a new shell session".into(),
                action: PaletteAction::NewLocalTerminal,
                group: PaletteGroup::Sessions,
                shortcut_command: None,
                match_indices: Vec::new(),
            },
            PaletteItem {
                glyph: "◈",
                label: "Add Saved Host".into(),
                hint: "Store an encrypted SSH config".into(),
                action: PaletteAction::AddSavedHost,
                group: PaletteGroup::Hosts,
                shortcut_command: None,
                match_indices: Vec::new(),
            },
        ];

        let model = flatten_items(items, true);

        assert_eq!(model.item_to_row, vec![1, 3]);
        assert_eq!(model.row_to_item, vec![None, Some(0), None, Some(1)]);
        assert!(matches!(
            model.rows[0],
            PaletteRow::Header(PaletteGroup::Sessions)
        ));
        assert!(matches!(
            model.rows[2],
            PaletteRow::Header(PaletteGroup::Hosts)
        ));
    }

    #[test]
    fn flatten_items_without_groups_keeps_identity_row_mapping() {
        let items = vec![
            PaletteItem {
                glyph: "+",
                label: "One".into(),
                hint: "first".into(),
                action: PaletteAction::NewLocalTerminal,
                group: PaletteGroup::Sessions,
                shortcut_command: None,
                match_indices: Vec::new(),
            },
            PaletteItem {
                glyph: "+",
                label: "Two".into(),
                hint: "second".into(),
                action: PaletteAction::OpenPreferences,
                group: PaletteGroup::System,
                shortcut_command: None,
                match_indices: Vec::new(),
            },
        ];

        let model = flatten_items(items, false);

        assert_eq!(model.item_to_row, vec![0, 1]);
        assert_eq!(model.row_to_item, vec![Some(0), Some(1)]);
        assert_eq!(model.rows.len(), 2);
    }

    #[test]
    fn page_down_skips_headers_and_lands_on_next_item() {
        let row_to_item = vec![None, Some(0), Some(1), None, Some(2), Some(3)];
        let item_to_row = vec![1, 2, 4, 5];

        let next = page_target_index(&row_to_item, &item_to_row, 0, 2, PageDirection::Down);

        assert_eq!(next, 2);
    }

    #[test]
    fn page_up_skips_headers_and_lands_on_previous_item() {
        let row_to_item = vec![None, Some(0), Some(1), None, Some(2), Some(3)];
        let item_to_row = vec![1, 2, 4, 5];

        let previous = page_target_index(&row_to_item, &item_to_row, 3, 2, PageDirection::Up);

        assert_eq!(previous, 1);
    }

    #[test]
    fn page_navigation_clamps_at_boundaries() {
        let row_to_item = vec![None, Some(0), Some(1), None, Some(2)];
        let item_to_row = vec![1, 2, 4];

        assert_eq!(
            page_target_index(&row_to_item, &item_to_row, 0, 8, PageDirection::Up),
            0
        );
        assert_eq!(
            page_target_index(&row_to_item, &item_to_row, 2, 8, PageDirection::Down),
            2
        );
    }

    #[test]
    fn row_lookup_finds_first_and_last_selectable_items() {
        let row_to_item = vec![None, Some(0), Some(1), None, Some(2)];

        assert_eq!(find_item_at_or_after(&row_to_item, 0), Some(0));
        assert_eq!(
            find_item_at_or_before(&row_to_item, row_to_item.len() - 1),
            Some(2)
        );
    }
}
