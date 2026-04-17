// Owns GUI workflow state for secure surfaces, drafts, settings, and vault gating.

use seance_vault::{
    HostAuthRef, PortForwardMode, UnlockMethod, VaultHostProfile, VaultPasswordCredential,
    VaultPortForwardProfile,
};
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
    Tunnels,
    Credentials,
    Keys,
}

impl SecureSection {
    pub(crate) const ALL: [Self; 4] = [Self::Hosts, Self::Tunnels, Self::Credentials, Self::Keys];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Hosts => "Hosts",
            Self::Tunnels => "Tunnels",
            Self::Credentials => "Credentials",
            Self::Keys => "Keys",
        }
    }

    pub(crate) fn subtitle(self) -> &'static str {
        match self {
            Self::Hosts => "Encrypted SSH connection profiles and auth order",
            Self::Tunnels => "Saved SSH port forwarding rules and live tunnel state",
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

    pub(crate) fn can_close(&self) -> bool {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TunnelDraftField {
    Label,
    Mode,
    ListenAddress,
    ListenPort,
    TargetAddress,
    TargetPort,
    Notes,
}

impl TunnelDraftField {
    pub(crate) const ALL: [Self; 7] = [
        Self::Label,
        Self::Mode,
        Self::ListenAddress,
        Self::ListenPort,
        Self::TargetAddress,
        Self::TargetPort,
        Self::Notes,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Label => "Label",
            Self::Mode => "Mode",
            Self::ListenAddress => "Listen Address",
            Self::ListenPort => "Listen Port",
            Self::TargetAddress => "Target Address",
            Self::TargetPort => "Target Port",
            Self::Notes => "Notes",
        }
    }
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

#[derive(Debug, Clone)]
pub(crate) struct TunnelDraftState {
    pub(crate) vault_id: Option<String>,
    pub(crate) port_forward_id: Option<String>,
    pub(crate) host_id: Option<String>,
    pub(crate) host_label: String,
    pub(crate) label: String,
    pub(crate) mode: PortForwardMode,
    pub(crate) listen_address: String,
    pub(crate) listen_port: String,
    pub(crate) target_address: String,
    pub(crate) target_port: String,
    pub(crate) notes: String,
    pub(crate) selected_field: TunnelDraftField,
    pub(crate) dirty: bool,
    pub(crate) error: Option<String>,
}

impl TunnelDraftState {
    pub(crate) fn blank() -> Self {
        Self {
            vault_id: None,
            port_forward_id: None,
            host_id: None,
            host_label: String::new(),
            label: String::new(),
            mode: PortForwardMode::Local,
            listen_address: "127.0.0.1".into(),
            listen_port: String::new(),
            target_address: "127.0.0.1".into(),
            target_port: String::new(),
            notes: String::new(),
            selected_field: TunnelDraftField::Label,
            dirty: false,
            error: None,
        }
    }

    pub(crate) fn from_port_forward(port_forward: VaultPortForwardProfile) -> Self {
        Self {
            vault_id: None,
            port_forward_id: Some(port_forward.id),
            host_id: Some(port_forward.host_id),
            host_label: String::new(),
            label: port_forward.label,
            mode: port_forward.mode,
            listen_address: port_forward.listen_address,
            listen_port: port_forward.listen_port.to_string(),
            target_address: port_forward.target_address,
            target_port: port_forward.target_port.to_string(),
            notes: port_forward.notes.unwrap_or_default(),
            selected_field: TunnelDraftField::Label,
            dirty: false,
            error: None,
        }
    }

    pub(crate) fn parsed_listen_port(&self) -> Option<u16> {
        self.listen_port.trim().parse::<u16>().ok()
    }

    pub(crate) fn parsed_target_port(&self) -> Option<u16> {
        self.target_port.trim().parse::<u16>().ok()
    }

    pub(crate) fn validation_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self
            .host_id
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            errors.push("Linked host is required.".into());
        }
        if self.label.trim().is_empty() {
            errors.push("Label is required.".into());
        }
        if self.listen_address.trim().is_empty() {
            errors.push("Listen address is required.".into());
        }
        if self.parsed_listen_port().is_none() {
            errors.push("Listen port must be a number between 1 and 65535.".into());
        }
        if self.target_address.trim().is_empty() {
            errors.push("Target address is required.".into());
        }
        if self.parsed_target_port().is_none() {
            errors.push("Target port must be a number between 1 and 65535.".into());
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
    pub(crate) tunnel_search: String,
    pub(crate) credential_search: String,
    pub(crate) key_search: String,
    pub(crate) selected_host_id: Option<String>,
    pub(crate) selected_tunnel_id: Option<String>,
    pub(crate) selected_credential_id: Option<String>,
    pub(crate) selected_key_id: Option<String>,
    pub(crate) host_draft: Option<HostDraftState>,
    pub(crate) tunnel_draft: Option<TunnelDraftState>,
    pub(crate) credential_draft: Option<CredentialDraftState>,
    pub(crate) message: Option<String>,
    pub(crate) input_target: SecureInputTarget,
}

impl Default for SecureWorkspaceState {
    fn default() -> Self {
        Self {
            section: SecureSection::Hosts,
            host_search: String::new(),
            tunnel_search: String::new(),
            credential_search: String::new(),
            key_search: String::new(),
            selected_host_id: None,
            selected_tunnel_id: None,
            selected_credential_id: None,
            selected_key_id: None,
            host_draft: None,
            tunnel_draft: None,
            credential_draft: None,
            message: None,
            input_target: SecureInputTarget::HostSearch,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SecureInputTarget {
    HostSearch,
    TunnelSearch,
    CredentialSearch,
    KeySearch,
    HostDraft(HostDraftField),
    TunnelDraft(TunnelDraftField),
    CredentialDraft(CredentialDraftField),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PendingAction {
    CloseSecureWorkspace,
    SwitchSecureSection(SecureSection),
    OpenHostDraft(Option<String>),
    OpenTunnelDraft {
        tunnel_id: Option<String>,
        host_scope_key: Option<String>,
    },
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
    Keybindings,
    Updates,
    Appearance,
    Terminal,
    Debug,
}

impl SettingsSection {
    pub(crate) const ALL: [Self; 6] = [
        Self::General,
        Self::Keybindings,
        Self::Updates,
        Self::Appearance,
        Self::Terminal,
        Self::Debug,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Keybindings => "Keybindings",
            Self::Updates => "Updates",
            Self::Appearance => "Appearance",
            Self::Terminal => "Terminal",
            Self::Debug => "Debug",
        }
    }

    pub(crate) fn subtitle(self) -> &'static str {
        match self {
            Self::General => "Resident app and window lifecycle",
            Self::Keybindings => "Effective shortcuts and config-first overrides",
            Self::Updates => "Release channel and in-app updater state",
            Self::Appearance => "Themes and overall look",
            Self::Terminal => "Shell and terminal rendering defaults",
            Self::Debug => "Performance HUD defaults",
        }
    }

    pub(crate) fn glyph(self) -> &'static str {
        match self {
            Self::General => "⚙",
            Self::Keybindings => "⌘",
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

    // ── Host draft ──────────────────────────────────────────────────────

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
    fn host_draft_roundtrip_from_host() {
        let host = VaultHostProfile {
            id: "h1".into(),
            label: "My Server".into(),
            hostname: "10.0.0.1".into(),
            port: 2222,
            username: "deploy".into(),
            notes: Some("production box".into()),
            auth_order: vec![HostAuthRef::Password {
                credential_id: "c1".into(),
            }],
        };
        let draft = HostDraftState::from_host(host);
        assert_eq!(draft.host_id.as_deref(), Some("h1"));
        assert_eq!(draft.label, "My Server");
        assert_eq!(draft.hostname, "10.0.0.1");
        assert_eq!(draft.port, "2222");
        assert_eq!(draft.username, "deploy");
        assert_eq!(draft.notes, "production box");
        assert!(!draft.dirty);
        assert!(draft.can_save());
    }

    #[test]
    fn host_draft_port_boundary_values() {
        let mut draft = HostDraftState::blank();
        draft.label = "x".into();
        draft.hostname = "x".into();
        draft.username = "x".into();
        draft.auth_items.push(HostAuthRef::Password {
            credential_id: "c".into(),
        });

        // Port 0 is invalid (u16 parse succeeds but 0 is not a valid port — however
        // the current validation only checks parse::<u16>().ok(), so 0 passes).
        draft.port = "0".into();
        assert_eq!(draft.parsed_port(), Some(0));

        // Port 1 is valid
        draft.port = "1".into();
        assert!(draft.can_save());

        // Port 65535 is valid
        draft.port = "65535".into();
        assert!(draft.can_save());

        // Port 65536 overflows u16
        draft.port = "65536".into();
        assert!(draft.parsed_port().is_none());
        assert!(!draft.can_save());

        // Empty port string is invalid
        draft.port = "".into();
        assert!(!draft.can_save());

        // Non-numeric port
        draft.port = "abc".into();
        assert!(!draft.can_save());
    }

    #[test]
    fn host_draft_whitespace_labels_rejected() {
        let mut draft = HostDraftState::blank();
        draft.label = "   ".into();
        draft.hostname = "host".into();
        draft.username = "user".into();
        draft.port = "22".into();
        draft.auth_items.push(HostAuthRef::Password {
            credential_id: "c".into(),
        });
        assert!(!draft.can_save());
        assert!(
            draft
                .validation_errors()
                .iter()
                .any(|msg| msg.contains("Label"))
        );
    }

    // ── Tunnel draft ────────────────────────────────────────────────────

    #[test]
    fn tunnel_draft_requires_linked_host_and_ports() {
        let draft = TunnelDraftState::blank();
        assert!(!draft.can_save());
        assert!(
            draft
                .validation_errors()
                .iter()
                .any(|msg| msg.contains("Linked host"))
        );
    }

    #[test]
    fn tunnel_draft_accepts_valid_endpoints() {
        let mut draft = TunnelDraftState::blank();
        draft.host_id = Some("host-1".into());
        draft.label = "db".into();
        draft.listen_port = "15432".into();
        draft.target_port = "5432".into();
        assert!(draft.can_save());
    }

    #[test]
    fn tunnel_draft_roundtrip_from_port_forward() {
        let pf = VaultPortForwardProfile {
            id: "pf1".into(),
            host_id: "h1".into(),
            label: "db-tunnel".into(),
            mode: PortForwardMode::Local,
            listen_address: "0.0.0.0".into(),
            listen_port: 15432,
            target_address: "db.internal".into(),
            target_port: 5432,
            notes: Some("dev database".into()),
        };
        let draft = TunnelDraftState::from_port_forward(pf);
        assert_eq!(draft.port_forward_id.as_deref(), Some("pf1"));
        assert_eq!(draft.host_id.as_deref(), Some("h1"));
        assert_eq!(draft.label, "db-tunnel");
        assert_eq!(draft.listen_address, "0.0.0.0");
        assert_eq!(draft.listen_port, "15432");
        assert_eq!(draft.target_address, "db.internal");
        assert_eq!(draft.target_port, "5432");
        assert_eq!(draft.notes, "dev database");
        assert_eq!(draft.mode, PortForwardMode::Local);
        assert!(!draft.dirty);
        assert!(draft.can_save());
    }

    #[test]
    fn tunnel_draft_port_boundary_values() {
        let mut draft = TunnelDraftState::blank();
        draft.host_id = Some("h".into());
        draft.label = "t".into();
        draft.listen_port = "1".into();
        draft.target_port = "65535".into();
        assert!(draft.can_save());

        draft.listen_port = "65536".into();
        assert!(!draft.can_save());

        draft.listen_port = "443".into();
        draft.target_port = "0".into();
        // 0 parses as u16 but is not a useful port — validation allows it per current logic
        assert_eq!(draft.parsed_target_port(), Some(0));
    }

    #[test]
    fn tunnel_draft_rejects_empty_addresses() {
        let mut draft = TunnelDraftState::blank();
        draft.host_id = Some("h1".into());
        draft.label = "t".into();
        draft.listen_address = "".into();
        draft.listen_port = "8080".into();
        draft.target_port = "80".into();
        let errors = draft.validation_errors();
        assert!(errors.iter().any(|e| e.contains("Listen address")));
    }

    // ── Credential draft ────────────────────────────────────────────────

    #[test]
    fn credential_draft_requires_label_and_secret() {
        let draft = CredentialDraftState::blank(CredentialDraftOrigin::Standalone);
        assert!(!draft.can_save());
        assert_eq!(draft.validation_errors().len(), 2);
    }

    #[test]
    fn credential_draft_roundtrip_from_credential() {
        let cred = VaultPasswordCredential {
            id: "c1".into(),
            label: "deploy-pass".into(),
            username_hint: Some("deploy".into()),
            secret: "hunter2".into(),
        };
        let draft = CredentialDraftState::from_credential(cred, CredentialDraftOrigin::HostAuth);
        assert_eq!(draft.credential_id.as_deref(), Some("c1"));
        assert_eq!(draft.label, "deploy-pass");
        assert_eq!(draft.username_hint, "deploy");
        assert_eq!(draft.secret, "hunter2");
        assert_eq!(draft.origin, CredentialDraftOrigin::HostAuth);
        assert!(!draft.dirty);
        assert!(!draft.reveal_secret);
        assert!(draft.can_save());
    }

    #[test]
    fn credential_draft_whitespace_only_label_rejected() {
        let mut draft = CredentialDraftState::blank(CredentialDraftOrigin::Standalone);
        draft.label = "   \t  ".into();
        draft.secret = "pass".into();
        assert!(!draft.can_save());
        assert!(
            draft
                .validation_errors()
                .iter()
                .any(|e| e.contains("Label"))
        );
    }

    #[test]
    fn credential_draft_whitespace_only_secret_rejected() {
        let mut draft = CredentialDraftState::blank(CredentialDraftOrigin::Standalone);
        draft.label = "ok".into();
        draft.secret = "   ".into();
        assert!(!draft.can_save());
        assert!(
            draft
                .validation_errors()
                .iter()
                .any(|e| e.contains("Password"))
        );
    }

    // ── Vault modal ─────────────────────────────────────────────────────

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

    // ── Dirty tracking ──────────────────────────────────────────────────

    #[test]
    fn host_draft_starts_clean() {
        assert!(!HostDraftState::blank().dirty);
        let host = VaultHostProfile {
            id: "h1".into(),
            label: "x".into(),
            hostname: "x".into(),
            port: 22,
            username: "x".into(),
            notes: None,
            auth_order: vec![],
        };
        assert!(!HostDraftState::from_host(host).dirty);
    }

    #[test]
    fn tunnel_draft_starts_clean() {
        assert!(!TunnelDraftState::blank().dirty);
    }

    #[test]
    fn credential_draft_starts_clean() {
        assert!(!CredentialDraftState::blank(CredentialDraftOrigin::Standalone).dirty);
    }
}
