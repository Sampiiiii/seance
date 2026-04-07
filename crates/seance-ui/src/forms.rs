// Owns GUI workflow state for secure surfaces, drafts, settings, and vault gating.

use seance_vault::{HostAuthRef, UnlockMethod, VaultHostProfile, VaultPasswordCredential};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnlockMode {
    Create,
    Unlock,
    Rename,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceSurface {
    Terminal,
    Settings,
    Sftp,
    Secure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SecureSection {
    Hosts,
    Credentials,
    Keys,
}

impl SecureSection {
    pub(crate) const ALL: [Self; 3] = [Self::Hosts, Self::Credentials, Self::Keys];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Hosts => "Hosts",
            Self::Credentials => "Credentials",
            Self::Keys => "Keys",
        }
    }

    pub(crate) fn subtitle(self) -> &'static str {
        match self {
            Self::Hosts => "Encrypted SSH connection profiles and auth order",
            Self::Credentials => "Stored passwords used by SSH and vault-managed secrets",
            Self::Keys => "Generated SSH keys and where they are used",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VaultModalOrigin {
    InitialSetup,
    UserAction,
    SecureAccess,
}

#[derive(Clone, Debug)]
pub(crate) struct VaultModalState {
    pub(crate) open: bool,
    pub(crate) mode: UnlockMode,
    pub(crate) origin: VaultModalOrigin,
    pub(crate) target_vault_id: Option<String>,
    pub(crate) vault_name: Zeroizing<String>,
    pub(crate) unlock_method: UnlockMethod,
    pub(crate) passphrase: Zeroizing<String>,
    pub(crate) confirm_passphrase: Zeroizing<String>,
    pub(crate) selected_field: usize,
    pub(crate) message: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) busy: bool,
    pub(crate) reveal_secret: bool,
}

impl VaultModalState {
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
        let unlock_method = if initialized
            && unlocked
            && device_unlock_attempted
            && device_unlock_message.is_none()
        {
            UnlockMethod::Device
        } else {
            UnlockMethod::Passphrase
        };

        Self {
            open: !unlocked,
            mode,
            origin: if initialized {
                VaultModalOrigin::UserAction
            } else {
                VaultModalOrigin::InitialSetup
            },
            target_vault_id: None,
            vault_name: Zeroizing::new(String::new()),
            unlock_method,
            passphrase: Zeroizing::new(String::new()),
            confirm_passphrase: Zeroizing::new(String::new()),
            selected_field: 0,
            message,
            error: None,
            busy: false,
            reveal_secret: false,
        }
    }

    pub(crate) fn open(&mut self, mode: UnlockMode, origin: VaultModalOrigin, message: String) {
        self.open = true;
        self.mode = mode;
        self.origin = origin;
        self.message = Some(message);
        self.error = None;
        self.busy = false;
        self.reveal_secret = false;
        self.unlock_method = UnlockMethod::Passphrase;
        self.target_vault_id = None;
        self.vault_name.clear();
        self.passphrase.clear();
        self.confirm_passphrase.clear();
        self.selected_field = 0;
    }

    pub(crate) fn can_close(self: &Self) -> bool {
        !matches!(self.origin, VaultModalOrigin::InitialSetup)
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        self.busy = false;
        self.error = None;
        self.target_vault_id = None;
        self.vault_name.clear();
        self.passphrase.clear();
        self.confirm_passphrase.clear();
        self.selected_field = 0;
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.open
    }

    pub(crate) fn passphrase_field_count(&self) -> usize {
        match self.mode {
            UnlockMode::Create => 3,
            UnlockMode::Rename => 1,
            UnlockMode::Unlock => {
                if self.unlock_method == UnlockMethod::Device {
                    0
                } else {
                    1
                }
            }
        }
    }

    pub(crate) fn can_submit(&self) -> bool {
        if self.busy {
            return false;
        }
        match self.mode {
            UnlockMode::Create => {
                !self.vault_name.trim().is_empty()
                    && !self.passphrase.trim().is_empty()
                    && self.passphrase == self.confirm_passphrase
                    && self.unlock_method == UnlockMethod::Passphrase
            }
            UnlockMode::Rename => !self.vault_name.trim().is_empty(),
            UnlockMode::Unlock => match self.unlock_method {
                UnlockMethod::Device => true,
                UnlockMethod::Passphrase => !self.passphrase.trim().is_empty(),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostDraftField {
    Label,
    Hostname,
    Username,
    Port,
    Notes,
}

impl HostDraftField {
    pub(crate) const ALL: [Self; 5] = [
        Self::Label,
        Self::Hostname,
        Self::Username,
        Self::Port,
        Self::Notes,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Label => "Label",
            Self::Hostname => "Hostname",
            Self::Username => "Username",
            Self::Port => "Port",
            Self::Notes => "Notes",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct HostDraftState {
    pub(crate) vault_id: Option<String>,
    pub(crate) host_id: Option<String>,
    pub(crate) label: String,
    pub(crate) hostname: String,
    pub(crate) username: String,
    pub(crate) port: String,
    pub(crate) notes: String,
    pub(crate) auth_items: Vec<HostAuthRef>,
    pub(crate) selected_field: HostDraftField,
    pub(crate) selected_auth: Option<usize>,
    pub(crate) dirty: bool,
    pub(crate) error: Option<String>,
}

impl HostDraftState {
    pub(crate) fn blank() -> Self {
        Self {
            vault_id: None,
            host_id: None,
            label: String::new(),
            hostname: String::new(),
            username: String::new(),
            port: "22".into(),
            notes: String::new(),
            auth_items: Vec::new(),
            selected_field: HostDraftField::Label,
            selected_auth: None,
            dirty: false,
            error: None,
        }
    }

    pub(crate) fn from_host(host: VaultHostProfile) -> Self {
        Self {
            vault_id: None,
            host_id: Some(host.id),
            label: host.label,
            hostname: host.hostname,
            username: host.username,
            port: host.port.to_string(),
            notes: host.notes.unwrap_or_default(),
            auth_items: host.auth_order,
            selected_field: HostDraftField::Label,
            selected_auth: None,
            dirty: false,
            error: None,
        }
    }

    pub(crate) fn parsed_port(&self) -> Option<u16> {
        self.port.trim().parse::<u16>().ok()
    }

    pub(crate) fn validation_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.label.trim().is_empty() {
            errors.push("Label is required.".into());
        }
        if self.hostname.trim().is_empty() {
            errors.push("Hostname is required.".into());
        }
        if self.username.trim().is_empty() {
            errors.push("Username is required.".into());
        }
        if self.parsed_port().is_none() {
            errors.push("Port must be a number between 1 and 65535.".into());
        }
        if self.auth_items.is_empty() {
            errors.push("At least one authentication method is required.".into());
        }
        errors
    }

    pub(crate) fn can_save(&self) -> bool {
        self.validation_errors().is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CredentialDraftField {
    Label,
    UsernameHint,
    Secret,
}

impl CredentialDraftField {
    pub(crate) const ALL: [Self; 3] = [Self::Label, Self::UsernameHint, Self::Secret];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Label => "Label",
            Self::UsernameHint => "Username Hint",
            Self::Secret => "Password",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CredentialDraftOrigin {
    Standalone,
    HostAuth,
}

#[derive(Debug, Clone)]
pub(crate) struct CredentialDraftState {
    pub(crate) vault_id: Option<String>,
    pub(crate) credential_id: Option<String>,
    pub(crate) label: String,
    pub(crate) username_hint: String,
    pub(crate) secret: String,
    pub(crate) selected_field: CredentialDraftField,
    pub(crate) dirty: bool,
    pub(crate) error: Option<String>,
    pub(crate) reveal_secret: bool,
    pub(crate) origin: CredentialDraftOrigin,
}

impl CredentialDraftState {
    pub(crate) fn blank(origin: CredentialDraftOrigin) -> Self {
        Self {
            vault_id: None,
            credential_id: None,
            label: String::new(),
            username_hint: String::new(),
            secret: String::new(),
            selected_field: CredentialDraftField::Label,
            dirty: false,
            error: None,
            reveal_secret: false,
            origin,
        }
    }

    pub(crate) fn from_credential(
        cred: VaultPasswordCredential,
        origin: CredentialDraftOrigin,
    ) -> Self {
        Self {
            vault_id: None,
            credential_id: Some(cred.id),
            label: cred.label,
            username_hint: cred.username_hint.unwrap_or_default(),
            secret: cred.secret,
            selected_field: CredentialDraftField::Label,
            dirty: false,
            error: None,
            reveal_secret: false,
            origin,
        }
    }

    pub(crate) fn validation_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.label.trim().is_empty() {
            errors.push("Label is required.".into());
        }
        if self.secret.trim().is_empty() {
            errors.push("Password is required.".into());
        }
        errors
    }

    pub(crate) fn can_save(&self) -> bool {
        self.validation_errors().is_empty()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SecureWorkspaceState {
    pub(crate) section: SecureSection,
    pub(crate) host_search: String,
    pub(crate) credential_search: String,
    pub(crate) key_search: String,
    pub(crate) selected_host_id: Option<String>,
    pub(crate) selected_credential_id: Option<String>,
    pub(crate) selected_key_id: Option<String>,
    pub(crate) host_draft: Option<HostDraftState>,
    pub(crate) credential_draft: Option<CredentialDraftState>,
    pub(crate) message: Option<String>,
    pub(crate) input_target: SecureInputTarget,
}

impl Default for SecureWorkspaceState {
    fn default() -> Self {
        Self {
            section: SecureSection::Hosts,
            host_search: String::new(),
            credential_search: String::new(),
            key_search: String::new(),
            selected_host_id: None,
            selected_credential_id: None,
            selected_key_id: None,
            host_draft: None,
            credential_draft: None,
            message: None,
            input_target: SecureInputTarget::HostSearch,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SecureInputTarget {
    HostSearch,
    CredentialSearch,
    KeySearch,
    HostDraft(HostDraftField),
    CredentialDraft(CredentialDraftField),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PendingAction {
    CloseSecureWorkspace,
    SwitchSecureSection(SecureSection),
    OpenHostDraft(Option<String>),
    OpenCredentialDraft(Option<String>, CredentialDraftOrigin),
}

#[derive(Clone, Debug)]
pub(crate) enum ConfirmDialogKind {
    DiscardChanges(PendingAction),
    BlockedDeletion {
        review_section: SecureSection,
        review_host_id: Option<String>,
    },
    DeleteVault {
        vault_id: String,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct ConfirmDialogState {
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) confirm_label: String,
    pub(crate) cancel_label: String,
    pub(crate) kind: ConfirmDialogKind,
}

impl ConfirmDialogState {
    pub(crate) fn discard_changes(pending: PendingAction) -> Self {
        Self {
            title: "Discard unsaved changes?".into(),
            message: "You have edits in progress. Continuing will discard them.".into(),
            confirm_label: "discard changes".into(),
            cancel_label: "keep editing".into(),
            kind: ConfirmDialogKind::DiscardChanges(pending),
        }
    }

    pub(crate) fn blocked_deletion(
        asset_label: &str,
        host_labels: &[String],
        review_section: SecureSection,
        review_host_id: Option<String>,
    ) -> Self {
        let joined = host_labels.join(", ");
        Self {
            title: format!("Cannot delete {asset_label}"),
            message: format!("This item is still referenced by: {joined}."),
            confirm_label: "review hosts".into(),
            cancel_label: "close".into(),
            kind: ConfirmDialogKind::BlockedDeletion {
                review_section,
                review_host_id,
            },
        }
    }

    pub(crate) fn delete_vault(vault_name: &str, db_path: &str, vault_id: String) -> Self {
        Self {
            title: format!("Delete vault '{vault_name}'?"),
            message: format!("This permanently removes the vault database at {db_path}."),
            confirm_label: "delete vault".into(),
            cancel_label: "keep vault".into(),
            kind: ConfirmDialogKind::DeleteVault { vault_id },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsSection {
    General,
    Updates,
    Appearance,
    Terminal,
    Debug,
}

impl SettingsSection {
    pub(crate) const ALL: [Self; 5] = [
        Self::General,
        Self::Updates,
        Self::Appearance,
        Self::Terminal,
        Self::Debug,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Updates => "Updates",
            Self::Appearance => "Appearance",
            Self::Terminal => "Terminal",
            Self::Debug => "Debug",
        }
    }

    pub(crate) fn subtitle(self) -> &'static str {
        match self {
            Self::General => "Resident app and window lifecycle",
            Self::Updates => "Release channel and in-app updater state",
            Self::Appearance => "Themes and overall look",
            Self::Terminal => "Shell and terminal rendering defaults",
            Self::Debug => "Performance HUD defaults",
        }
    }

    pub(crate) fn glyph(self) -> &'static str {
        match self {
            Self::General => "⚙",
            Self::Updates => "↑",
            Self::Appearance => "◑",
            Self::Terminal => "▸",
            Self::Debug => "⚡",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SettingsPanelState {
    pub(crate) section: SettingsSection,
    pub(crate) message: Option<String>,
}

impl Default for SettingsPanelState {
    fn default() -> Self {
        Self {
            section: SettingsSection::General,
            message: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_draft_requires_auth_and_core_fields() {
        let draft = HostDraftState::blank();
        assert!(!draft.can_save());
        assert!(
            draft
                .validation_errors()
                .iter()
                .any(|msg| msg.contains("authentication"))
        );
    }

    #[test]
    fn host_draft_accepts_valid_port_and_auth() {
        let mut draft = HostDraftState::blank();
        draft.label = "Prod".into();
        draft.hostname = "prod.example.com".into();
        draft.username = "root".into();
        draft.port = "22".into();
        draft.auth_items.push(HostAuthRef::Password {
            credential_id: "cred-1".into(),
        });
        assert!(draft.can_save());
    }

    #[test]
    fn credential_draft_requires_label_and_secret() {
        let draft = CredentialDraftState::blank(CredentialDraftOrigin::Standalone);
        assert!(!draft.can_save());
        assert_eq!(draft.validation_errors().len(), 2);
    }

    #[test]
    fn vault_modal_create_requires_matching_passphrases() {
        let mut modal = VaultModalState::new(false, false, false, None);
        modal.open(
            UnlockMode::Create,
            VaultModalOrigin::InitialSetup,
            "Create a recovery passphrase.".into(),
        );
        modal.passphrase = Zeroizing::new("one".into());
        modal.confirm_passphrase = Zeroizing::new("two".into());
        assert!(!modal.can_submit());
    }
}
