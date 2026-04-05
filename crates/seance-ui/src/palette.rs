use crate::theme::ThemeId;
use seance_terminal::LocalSessionHandle;
use seance_vault::HostSummary;

#[derive(Clone)]
pub enum PaletteAction {
    NewLocalTerminal,
    SwitchSession(u64),
    CloseActiveSession,
    SwitchTheme(ThemeId),
    UnlockVault,
    LockVault,
    AddSavedHost,
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
}

pub fn build_items(
    sessions: &[LocalSessionHandle],
    saved_hosts: &[HostSummary],
    active_id: u64,
    active_theme: ThemeId,
    query: &str,
    vault_unlocked: bool,
) -> Vec<PaletteItem> {
    let q = query.to_lowercase();
    let mut items = vec![PaletteItem {
        glyph: "+",
        label: "New Local Terminal".into(),
        hint: "Spawn a new shell session".into(),
        action: PaletteAction::NewLocalTerminal,
    }];

    if vault_unlocked {
        items.push(PaletteItem {
            glyph: "◈",
            label: "Add Saved Host".into(),
            hint: "Store an encrypted SSH config".into(),
            action: PaletteAction::AddSavedHost,
        });

        items.push(PaletteItem {
            glyph: "•",
            label: "Lock Vault".into(),
            hint: "Remove decrypted keys from memory".into(),
            action: PaletteAction::LockVault,
        });
    } else {
        items.push(PaletteItem {
            glyph: "•",
            label: "Unlock Vault".into(),
            hint: "Use your passphrase or enrolled device".into(),
            action: PaletteAction::UnlockVault,
        });
    }

    for session in sessions {
        if session.id() != active_id {
            items.push(PaletteItem {
                glyph: "›",
                label: format!("Switch to {}", session.title()),
                hint: format!("session #{}", session.id()),
                action: PaletteAction::SwitchSession(session.id()),
            });
        }
    }

    if !sessions.is_empty() {
        items.push(PaletteItem {
            glyph: "×",
            label: "Close Active Session".into(),
            hint: "Terminate the current terminal".into(),
            action: PaletteAction::CloseActiveSession,
        });
    }

    if vault_unlocked {
        for host in saved_hosts {
            items.push(PaletteItem {
                glyph: "→",
                label: format!("Connect Saved Host: {}", host.label),
                hint: format!("{}@{}:{}", host.username, host.hostname, host.port),
                action: PaletteAction::ConnectSavedHost(host.id.clone()),
            });
            items.push(PaletteItem {
                glyph: "✎",
                label: format!("Edit Saved Host: {}", host.label),
                hint: "Update the encrypted record".into(),
                action: PaletteAction::EditSavedHost(host.id.clone()),
            });
            items.push(PaletteItem {
                glyph: "×",
                label: format!("Delete Saved Host: {}", host.label),
                hint: "Create a tombstone for sync".into(),
                action: PaletteAction::DeleteSavedHost(host.id.clone()),
            });
        }
    }

    for &tid in ThemeId::ALL {
        if tid != active_theme {
            let theme = tid.theme();
            items.push(PaletteItem {
                glyph: "◑",
                label: format!("Theme: {}", theme.name),
                hint: "Switch appearance".into(),
                action: PaletteAction::SwitchTheme(tid),
            });
        }
    }

    if !q.is_empty() {
        items.retain(|item| item.label.to_lowercase().contains(&q));
    }

    items
}
