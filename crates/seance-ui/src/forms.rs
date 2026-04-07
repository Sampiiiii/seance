use seance_vault::{HostAuthRef, VaultHostProfile, VaultPasswordCredential};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnlockMode {
    Create,
    Unlock,
}

#[derive(Debug)]
pub(crate) struct UnlockFormState {
    pub(crate) mode: UnlockMode,
    pub(crate) passphrase: Zeroizing<String>,
    pub(crate) confirm_passphrase: Zeroizing<String>,
    pub(crate) selected_field: usize,
    pub(crate) message: Option<String>,
    pub(crate) completed: bool,
}

impl UnlockFormState {
    pub(crate) fn new(
        initialized: bool,
        unlocked: bool,
        device_unlock_attempted: bool,
        device_unlock_message: Option<&str>,
    ) -> Self {
        let mode = if initialized {
            UnlockMode::Unlock
        } else {
            UnlockMode::Create
        };
        let message = if unlocked {
            Some("Vault unlocked from the local device key store.".into())
        } else if initialized && device_unlock_attempted {
            Some(
                device_unlock_message
                    .unwrap_or("Device unlock unavailable. Enter your recovery passphrase.")
                    .to_string(),
            )
        } else if initialized {
            Some("Unlock the vault to decrypt saved hosts.".into())
        } else {
            Some("Create a recovery passphrase for the encrypted vault.".into())
        };

        Self {
            mode,
            passphrase: Zeroizing::new(String::new()),
            confirm_passphrase: Zeroizing::new(String::new()),
            selected_field: 0,
            message,
            completed: unlocked,
        }
    }

    pub(crate) fn reset_for_unlock(&mut self) {
        self.mode = UnlockMode::Unlock;
        self.passphrase.clear();
        self.confirm_passphrase.clear();
        self.selected_field = 0;
        self.completed = false;
    }

    pub(crate) fn is_visible(&self) -> bool {
        !self.completed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostField {
    Label,
    Hostname,
    Username,
    Port,
    Notes,
    Auth,
}

impl HostField {
    pub(crate) const ALL: [Self; 6] = [
        Self::Label,
        Self::Hostname,
        Self::Username,
        Self::Port,
        Self::Notes,
        Self::Auth,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Label => "Label",
            Self::Hostname => "Hostname",
            Self::Username => "Username",
            Self::Port => "Port",
            Self::Notes => "Notes",
            Self::Auth => "Authentication",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct HostEditorState {
    pub(crate) host_id: Option<String>,
    pub(crate) label: String,
    pub(crate) hostname: String,
    pub(crate) username: String,
    pub(crate) port: String,
    pub(crate) notes: String,
    pub(crate) auth_items: Vec<HostAuthRef>,
    pub(crate) auth_cursor: usize,
    pub(crate) selected_field: usize,
    pub(crate) message: Option<String>,
}

impl HostEditorState {
    pub(crate) fn blank() -> Self {
        Self {
            host_id: None,
            label: String::new(),
            hostname: String::new(),
            username: String::new(),
            port: "22".into(),
            notes: String::new(),
            auth_items: Vec::new(),
            auth_cursor: 0,
            selected_field: 0,
            message: Some(
                "Create an encrypted SSH host. Use the Auth section to select credentials.".into(),
            ),
        }
    }

    pub(crate) fn from_host(host: VaultHostProfile) -> Self {
        Self {
            host_id: Some(host.id),
            label: host.label,
            hostname: host.hostname,
            username: host.username,
            port: host.port.to_string(),
            notes: host.notes.unwrap_or_default(),
            auth_items: host.auth_order,
            auth_cursor: 0,
            selected_field: 0,
            message: Some("Edit the host record. Tab to Auth and toggle credentials.".into()),
        }
    }

    pub(crate) fn field(&self) -> HostField {
        HostField::ALL[self.selected_field.min(HostField::ALL.len() - 1)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CredentialField {
    Label,
    UsernameHint,
    Secret,
}

impl CredentialField {
    pub(crate) const ALL: [Self; 3] = [Self::Label, Self::UsernameHint, Self::Secret];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Label => "Label",
            Self::UsernameHint => "Username Hint",
            Self::Secret => "Password",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CredentialEditorState {
    pub(crate) credential_id: Option<String>,
    pub(crate) label: String,
    pub(crate) username_hint: String,
    pub(crate) secret: String,
    pub(crate) selected_field: usize,
    pub(crate) message: Option<String>,
}

impl CredentialEditorState {
    pub(crate) fn blank() -> Self {
        Self {
            credential_id: None,
            label: String::new(),
            username_hint: String::new(),
            secret: String::new(),
            selected_field: 0,
            message: Some("Store an encrypted password credential in the vault.".into()),
        }
    }

    pub(crate) fn from_credential(cred: VaultPasswordCredential) -> Self {
        Self {
            credential_id: Some(cred.id),
            label: cred.label,
            username_hint: cred.username_hint.unwrap_or_default(),
            secret: cred.secret,
            selected_field: 0,
            message: Some("Edit the credential. Tab to move, Enter on Password to save.".into()),
        }
    }

    pub(crate) fn field(&self) -> CredentialField {
        CredentialField::ALL[self.selected_field.min(CredentialField::ALL.len() - 1)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsSection {
    General,
    Updates,
    Appearance,
    Terminal,
    Debug,
    Vault,
}

impl SettingsSection {
    pub(crate) const ALL: [Self; 6] = [
        Self::General,
        Self::Updates,
        Self::Appearance,
        Self::Terminal,
        Self::Debug,
        Self::Vault,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Updates => "Updates",
            Self::Appearance => "Appearance",
            Self::Terminal => "Terminal",
            Self::Debug => "Debug",
            Self::Vault => "Vault",
        }
    }

    pub(crate) fn subtitle(self) -> &'static str {
        match self {
            Self::General => "Resident app and window lifecycle",
            Self::Updates => "Release channel and in-app updater state",
            Self::Appearance => "Themes and overall look",
            Self::Terminal => "Shell and terminal rendering defaults",
            Self::Debug => "Performance HUD defaults",
            Self::Vault => "Encrypted credentials and SSH keys",
        }
    }

    pub(crate) fn glyph(self) -> &'static str {
        match self {
            Self::General => "⚙",
            Self::Updates => "↑",
            Self::Appearance => "◑",
            Self::Terminal => "▸",
            Self::Debug => "⚡",
            Self::Vault => "◆",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SettingsPanelState {
    pub(crate) open: bool,
    pub(crate) section: SettingsSection,
    pub(crate) message: Option<String>,
}

impl Default for SettingsPanelState {
    fn default() -> Self {
        Self {
            open: false,
            section: SettingsSection::General,
            message: None,
        }
    }
}
