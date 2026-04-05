use crate::theme::ThemeId;
use std::{collections::HashMap, sync::Arc};

use seance_terminal::TerminalSession;
use seance_vault::{CredentialSummary, HostSummary, KeySummary, PrivateKeyAlgorithm};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaletteGroup {
    Sessions,
    Hosts,
    Vault,
    Appearance,
}

impl PaletteGroup {
    pub fn label(&self) -> &'static str {
        match self {
            PaletteGroup::Sessions => "SESSIONS",
            PaletteGroup::Hosts => "HOSTS",
            PaletteGroup::Vault => "VAULT",
            PaletteGroup::Appearance => "APPEARANCE",
        }
    }
}

#[derive(Clone)]
pub enum PaletteAction {
    NewLocalTerminal,
    SwitchSession(u64),
    CloseActiveSession,
    SwitchTheme(ThemeId),
    UnlockVault,
    LockVault,
    OpenVaultPanel,
    AddSavedHost,
    AddPasswordCredential,
    EditPasswordCredential(String),
    DeletePasswordCredential(String),
    #[allow(dead_code)]
    ImportPrivateKey,
    GenerateEd25519Key,
    GenerateRsaKey,
    DeletePrivateKey(String),
    EditSavedHost(String),
    DeleteSavedHost(String),
    ConnectSavedHost(String),
}

#[derive(Clone)]
pub struct PaletteItem {
    pub glyph: &'static str,
    pub label: String,
    pub hint: String,
    pub action: PaletteAction,
    pub group: PaletteGroup,
    pub shortcut: Option<&'static str>,
    pub match_indices: Vec<usize>,
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

pub fn build_items(
    sessions: &[Arc<dyn TerminalSession>],
    session_labels: &HashMap<u64, String>,
    saved_hosts: &[HostSummary],
    credentials: &[CredentialSummary],
    keys: &[KeySummary],
    active_id: u64,
    active_theme: ThemeId,
    query: &str,
    vault_unlocked: bool,
) -> Vec<PaletteItem> {
    let mut items: Vec<PaletteItem> = Vec::new();

    // --- Sessions group ---

    items.push(PaletteItem {
        glyph: "+",
        label: "New Local Terminal".into(),
        hint: "Spawn a new shell session".into(),
        action: PaletteAction::NewLocalTerminal,
        group: PaletteGroup::Sessions,
        shortcut: Some("\u{2318}T"),
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
                shortcut: None,
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
            shortcut: Some("\u{2318}W"),
            match_indices: Vec::new(),
        });
    }

    // --- Hosts group ---

    if vault_unlocked {
        items.push(PaletteItem {
            glyph: "◈",
            label: "Add Saved Host".into(),
            hint: "Store an encrypted SSH config".into(),
            action: PaletteAction::AddSavedHost,
            group: PaletteGroup::Hosts,
            shortcut: None,
            match_indices: Vec::new(),
        });

        for host in saved_hosts {
            items.push(PaletteItem {
                glyph: "→",
                label: format!("Connect: {}", host.label),
                hint: format!("{}@{}:{}", host.username, host.hostname, host.port),
                action: PaletteAction::ConnectSavedHost(host.id.clone()),
                group: PaletteGroup::Hosts,
                shortcut: None,
                match_indices: Vec::new(),
            });
            items.push(PaletteItem {
                glyph: "✎",
                label: format!("Edit: {}", host.label),
                hint: "Update the encrypted record".into(),
                action: PaletteAction::EditSavedHost(host.id.clone()),
                group: PaletteGroup::Hosts,
                shortcut: None,
                match_indices: Vec::new(),
            });
            items.push(PaletteItem {
                glyph: "×",
                label: format!("Delete: {}", host.label),
                hint: "Remove this saved host".into(),
                action: PaletteAction::DeleteSavedHost(host.id.clone()),
                group: PaletteGroup::Hosts,
                shortcut: None,
                match_indices: Vec::new(),
            });
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
            shortcut: Some("\u{2318},"),
            match_indices: Vec::new(),
        });
        items.push(PaletteItem {
            glyph: "+",
            label: "Add Password Credential".into(),
            hint: "Store an encrypted password".into(),
            action: PaletteAction::AddPasswordCredential,
            group: PaletteGroup::Vault,
            shortcut: None,
            match_indices: Vec::new(),
        });
        items.push(PaletteItem {
            glyph: "+",
            label: "Generate Ed25519 Key".into(),
            hint: "Create a new vault-backed Ed25519 key".into(),
            action: PaletteAction::GenerateEd25519Key,
            group: PaletteGroup::Vault,
            shortcut: None,
            match_indices: Vec::new(),
        });
        items.push(PaletteItem {
            glyph: "+",
            label: "Generate RSA Key".into(),
            hint: "Create a new vault-backed RSA key".into(),
            action: PaletteAction::GenerateRsaKey,
            group: PaletteGroup::Vault,
            shortcut: None,
            match_indices: Vec::new(),
        });

        for cred in credentials {
            let hint_str = cred
                .username_hint
                .as_deref()
                .unwrap_or("password credential");
            items.push(PaletteItem {
                glyph: "✎",
                label: format!("Edit: {}", cred.label),
                hint: hint_str.to_string(),
                action: PaletteAction::EditPasswordCredential(cred.id.clone()),
                group: PaletteGroup::Vault,
                shortcut: None,
                match_indices: Vec::new(),
            });
            items.push(PaletteItem {
                glyph: "×",
                label: format!("Delete: {}", cred.label),
                hint: "Remove this credential".into(),
                action: PaletteAction::DeletePasswordCredential(cred.id.clone()),
                group: PaletteGroup::Vault,
                shortcut: None,
                match_indices: Vec::new(),
            });
        }

        for key in keys {
            let algo = match &key.algorithm {
                PrivateKeyAlgorithm::Ed25519 => "Ed25519",
                PrivateKeyAlgorithm::Rsa { bits } => {
                    if *bits == 4096 { "RSA-4096" } else { "RSA" }
                }
            };
            items.push(PaletteItem {
                glyph: "×",
                label: format!("Delete Key: {}", key.label),
                hint: algo.to_string(),
                action: PaletteAction::DeletePrivateKey(key.id.clone()),
                group: PaletteGroup::Vault,
                shortcut: None,
                match_indices: Vec::new(),
            });
        }

        items.push(PaletteItem {
            glyph: "◆",
            label: "Lock Vault".into(),
            hint: "Remove decrypted keys from memory".into(),
            action: PaletteAction::LockVault,
            group: PaletteGroup::Vault,
            shortcut: None,
            match_indices: Vec::new(),
        });
    } else {
        items.push(PaletteItem {
            glyph: "◇",
            label: "Unlock Vault".into(),
            hint: "Use your passphrase or enrolled device".into(),
            action: PaletteAction::UnlockVault,
            group: PaletteGroup::Vault,
            shortcut: None,
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
                shortcut: None,
                match_indices: Vec::new(),
            });
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, mpsc};

    use anyhow::Result;
    use seance_terminal::{
        SessionPerfSnapshot, SessionSnapshot, TerminalGeometry, TerminalSession,
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

        fn snapshot(&self) -> SessionSnapshot {
            SessionSnapshot::default()
        }

        fn send_input(&self, _bytes: Vec<u8>) -> Result<()> {
            Ok(())
        }

        fn resize(&self, _geometry: TerminalGeometry) -> Result<()> {
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
            999,
            ThemeId::ObsidianSmoke,
            "",
            false,
        );

        let switch_item = items
            .into_iter()
            .find(|item| matches!(item.action, PaletteAction::SwitchSession(18)))
            .expect("switch item");

        assert_eq!(switch_item.label, "Switch to local-1");
        assert_eq!(switch_item.hint, "session #18");
    }
}
