use std::{collections::HashMap, sync::Arc};

use gpui::FocusHandle;
use seance_config::AppConfig;
use seance_core::{SessionKind, UpdateState};
use seance_terminal::{TerminalGeometry, TerminalSession};
use seance_vault::{CredentialSummary, HostSummary, KeySummary};

use crate::{
    backend::UiBackend,
    forms::{CredentialEditorState, HostEditorState, SettingsPanelState, UnlockFormState},
    perf::{PerfOverlayState, UiPerfMode},
    sftp::SftpBrowserState,
    surface::TerminalSurfaceState,
    theme::ThemeId,
};

pub(crate) struct SeanceWorkspace {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) active_session_id: u64,
    pub(crate) backend: UiBackend,
    pub(crate) config: AppConfig,
    pub(crate) saved_hosts: Vec<HostSummary>,
    pub(crate) selected_host_id: Option<String>,
    pub(crate) connecting_host_id: Option<String>,
    pub(crate) unlock_form: UnlockFormState,
    pub(crate) host_editor: Option<HostEditorState>,
    pub(crate) credential_editor: Option<CredentialEditorState>,
    pub(crate) settings_panel: SettingsPanelState,
    pub(crate) sftp_browser: Option<SftpBrowserState>,
    pub(crate) cached_credentials: Vec<CredentialSummary>,
    pub(crate) cached_keys: Vec<KeySummary>,
    pub(crate) status_message: Option<String>,
    pub(crate) update_state: UpdateState,
    pub(crate) active_theme: ThemeId,
    pub(crate) palette_open: bool,
    pub(crate) palette_query: String,
    pub(crate) palette_selected: usize,
    pub(crate) terminal_metrics: Option<TerminalMetrics>,
    pub(crate) last_applied_geometry: Option<TerminalGeometry>,
    pub(crate) active_terminal_rows: usize,
    pub(crate) terminal_surface: TerminalSurfaceState,
    pub(crate) perf_mode_env_override: Option<UiPerfMode>,
    pub(crate) perf_overlay: PerfOverlayState,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TerminalMetrics {
    pub(crate) cell_width_px: f32,
    pub(crate) cell_height_px: f32,
    pub(crate) line_height_px: f32,
    pub(crate) font_size_px: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TerminalRendererMetrics {
    pub(crate) visible_rows: usize,
    pub(crate) visible_cells: usize,
    pub(crate) fragments: usize,
    pub(crate) background_quads: usize,
    pub(crate) special_glyph_cells: usize,
    pub(crate) wide_cells: usize,
    pub(crate) shape_hits: usize,
    pub(crate) shape_misses: usize,
}

pub(crate) fn local_session_display_number_for_ids(
    session_ids: &[u64],
    session_kinds: &HashMap<u64, SessionKind>,
    target_id: u64,
) -> Option<usize> {
    let mut local_count = 0;

    for session_id in session_ids {
        if matches!(session_kinds.get(session_id), Some(SessionKind::Local)) {
            local_count += 1;
            if *session_id == target_id {
                return Some(local_count);
            }
        }
    }

    None
}

pub(crate) fn session_kind_map_from_sessions(
    sessions: &[Arc<dyn TerminalSession>],
    backend: &UiBackend,
) -> HashMap<u64, SessionKind> {
    sessions
        .iter()
        .filter_map(|session| {
            backend
                .session_kind(session.id())
                .map(|kind| (session.id(), kind))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn session_kind_map(entries: &[(u64, SessionKind)]) -> HashMap<u64, SessionKind> {
        entries.iter().copied().collect()
    }

    #[test]
    fn local_display_number_is_one_for_single_local_session() {
        let session_kinds = session_kind_map(&[(7, SessionKind::Local)]);

        assert_eq!(
            local_session_display_number_for_ids(&[7], &session_kinds, 7),
            Some(1)
        );
    }

    #[test]
    fn local_display_numbers_follow_open_local_session_order() {
        let session_kinds = session_kind_map(&[
            (7, SessionKind::Local),
            (10, SessionKind::Local),
            (14, SessionKind::Local),
        ]);

        assert_eq!(
            local_session_display_number_for_ids(&[7, 10, 14], &session_kinds, 7),
            Some(1)
        );
        assert_eq!(
            local_session_display_number_for_ids(&[7, 10, 14], &session_kinds, 10),
            Some(2)
        );
        assert_eq!(
            local_session_display_number_for_ids(&[7, 10, 14], &session_kinds, 14),
            Some(3)
        );
    }

    #[test]
    fn local_display_numbers_repack_after_middle_session_closes() {
        let session_kinds = session_kind_map(&[(7, SessionKind::Local), (14, SessionKind::Local)]);

        assert_eq!(
            local_session_display_number_for_ids(&[7, 14], &session_kinds, 7),
            Some(1)
        );
        assert_eq!(
            local_session_display_number_for_ids(&[7, 14], &session_kinds, 14),
            Some(2)
        );
    }

    #[test]
    fn local_display_numbers_stay_dense_after_reopen() {
        let session_kinds = session_kind_map(&[
            (7, SessionKind::Local),
            (14, SessionKind::Local),
            (18, SessionKind::Local),
        ]);

        assert_eq!(
            local_session_display_number_for_ids(&[7, 14, 18], &session_kinds, 18),
            Some(3)
        );
    }

    #[test]
    fn remote_sessions_do_not_consume_local_display_numbers() {
        let session_kinds = session_kind_map(&[
            (7, SessionKind::Local),
            (9, SessionKind::Remote),
            (14, SessionKind::Local),
        ]);

        assert_eq!(
            local_session_display_number_for_ids(&[7, 9, 14], &session_kinds, 7),
            Some(1)
        );
        assert_eq!(
            local_session_display_number_for_ids(&[7, 9, 14], &session_kinds, 14),
            Some(2)
        );
        assert_eq!(
            local_session_display_number_for_ids(&[7, 9, 14], &session_kinds, 9),
            None
        );
    }
}