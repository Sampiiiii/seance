use anyhow::{Result, anyhow};
use gpui::{Action, actions};
use serde_json::Value;

use crate::ThemeId;

actions!(
    seance_ui_app,
    [
        NewTerminal,
        CheckForUpdates,
        OpenCommandPalette,
        OpenPreferences,
        CloseActiveSession,
        OpenNewWindow,
        TogglePerfHud,
        QuitSeance,
        HideSeance,
        HideOtherApps,
        ShowAllApps,
    ]
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchTheme {
    pub theme_id: ThemeId,
}

impl Action for SwitchTheme {
    fn boxed_clone(&self) -> Box<dyn Action> {
        Box::new(self.clone())
    }

    fn partial_eq(&self, action: &dyn Action) -> bool {
        action.as_any().downcast_ref::<Self>() == Some(self)
    }

    fn name(&self) -> &'static str {
        Self::name_for_type()
    }

    fn name_for_type() -> &'static str
    where
        Self: Sized,
    {
        "seance_ui::SwitchTheme"
    }

    fn build(value: Value) -> Result<Box<dyn Action>>
    where
        Self: Sized,
    {
        let key = value
            .get("theme_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("missing theme_id"))?;
        let theme_id = ThemeId::from_key(key).ok_or_else(|| anyhow!("unknown theme_id"))?;
        Ok(Box::new(Self { theme_id }))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectHost {
    pub vault_id: String,
    pub host_id: String,
}

impl Action for ConnectHost {
    fn boxed_clone(&self) -> Box<dyn Action> {
        Box::new(self.clone())
    }

    fn partial_eq(&self, action: &dyn Action) -> bool {
        action.as_any().downcast_ref::<Self>() == Some(self)
    }

    fn name(&self) -> &'static str {
        Self::name_for_type()
    }

    fn name_for_type() -> &'static str
    where
        Self: Sized,
    {
        "seance_ui::ConnectHost"
    }

    fn build(value: Value) -> Result<Box<dyn Action>>
    where
        Self: Sized,
    {
        let vault_id = value
            .get("vault_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("missing vault_id"))?;
        let host_id = value
            .get("host_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("missing host_id"))?;
        Ok(Box::new(Self {
            vault_id: vault_id.to_string(),
            host_id: host_id.to_string(),
        }))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectSession {
    pub session_id: u64,
}

impl Action for SelectSession {
    fn boxed_clone(&self) -> Box<dyn Action> {
        Box::new(self.clone())
    }

    fn partial_eq(&self, action: &dyn Action) -> bool {
        action.as_any().downcast_ref::<Self>() == Some(self)
    }

    fn name(&self) -> &'static str {
        Self::name_for_type()
    }

    fn name_for_type() -> &'static str
    where
        Self: Sized,
    {
        "seance_ui::SelectSession"
    }

    fn build(value: Value) -> Result<Box<dyn Action>>
    where
        Self: Sized,
    {
        let session_id = value
            .get("session_id")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("missing session_id"))?;
        Ok(Box::new(Self { session_id }))
    }
}
