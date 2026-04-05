use crate::theme::ThemeId;
use seance_terminal::LocalSessionHandle;

#[derive(Clone)]
pub enum PaletteAction {
    NewLocalTerminal,
    SwitchSession(u64),
    CloseActiveSession,
    SwitchTheme(ThemeId),
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
    active_id: u64,
    active_theme: ThemeId,
    query: &str,
) -> Vec<PaletteItem> {
    let q = query.to_lowercase();
    let mut items = vec![PaletteItem {
        glyph: "+",
        label: "New Local Terminal".into(),
        hint: "Spawn a new shell session".into(),
        action: PaletteAction::NewLocalTerminal,
    }];

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
