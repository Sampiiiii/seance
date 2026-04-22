use std::{collections::HashMap, ops::Range, sync::Arc, time::Instant};

use gpui::{FocusHandle, Pixels, Point, ScrollHandle, UniformListScrollHandle};
use seance_config::AppConfig;
use seance_core::{
    ManagedVaultSummary, SessionKind, UpdateState, VaultScopedCredentialSummary,
    VaultScopedHostSummary, VaultScopedKeySummary, VaultScopedPortForwardSummary,
};
use seance_ssh::PortForwardRuntimeSnapshot;
use seance_terminal::{TerminalGeometry, TerminalScrollCommand, TerminalSession};

pub(crate) const DEFAULT_SIDEBAR_WIDTH: f32 = 260.0;
pub(crate) const MIN_SIDEBAR_WIDTH: f32 = 180.0;
pub(crate) const MAX_SIDEBAR_WIDTH: f32 = 450.0;
pub(crate) const SIDEBAR_DRAG_TARGET_PX: f32 = 6.0;
pub(crate) const SIDEBAR_DIVIDER_VISUAL_PX: f32 = 1.0;

use crate::{
    backend::UiBackend,
    connect::ConnectAttemptTracker,
    forms::{
        ConfirmDialogState, SecureWorkspaceState, SettingsPanelState, VaultModalState,
        WorkspaceSurface,
    },
    frame_pacer::FramePacer,
    perf::{PerfOverlayState, UiPerfMode},
    sftp::SftpBrowserState,
    surface::TerminalSurfaceState,
    terminal_scrollbar::TerminalScrollbarDragState,
    theme::ThemeId,
};

pub(crate) struct SeanceWorkspace {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) active_session_id: u64,
    pub(crate) backend: UiBackend,
    pub(crate) config: AppConfig,
    pub(crate) managed_vaults: Vec<ManagedVaultSummary>,
    pub(crate) saved_hosts: Vec<VaultScopedHostSummary>,
    pub(crate) selected_host_id: Option<String>,
    pub(crate) connect_attempts: ConnectAttemptTracker,
    pub(crate) surface: WorkspaceSurface,
    pub(crate) vault_modal: VaultModalState,
    pub(crate) secure: SecureWorkspaceState,
    pub(crate) confirm_dialog: Option<ConfirmDialogState>,
    pub(crate) settings_panel: SettingsPanelState,
    pub(crate) sftp_browser: Option<SftpBrowserState>,
    pub(crate) cached_credentials: Vec<VaultScopedCredentialSummary>,
    pub(crate) cached_keys: Vec<VaultScopedKeySummary>,
    pub(crate) cached_port_forwards: Vec<VaultScopedPortForwardSummary>,
    pub(crate) active_port_forwards: Vec<PortForwardRuntimeSnapshot>,
    pub(crate) update_state: UpdateState,
    pub(crate) active_theme: ThemeId,
    pub(crate) palette_open: bool,
    pub(crate) palette_query: String,
    pub(crate) palette_selected: usize,
    pub(crate) palette_scroll_handle: ScrollHandle,
    pub(crate) secure_host_list_scroll_handle: UniformListScrollHandle,
    pub(crate) secure_credential_list_scroll_handle: UniformListScrollHandle,
    pub(crate) secure_key_list_scroll_handle: UniformListScrollHandle,
    pub(crate) secure_auth_available_scroll_handle: UniformListScrollHandle,
    pub(crate) secure_filtered_host_indices: Vec<usize>,
    pub(crate) secure_filtered_credential_indices: Vec<usize>,
    pub(crate) secure_filtered_key_indices: Vec<usize>,
    pub(crate) secure_host_search_blobs: Vec<String>,
    pub(crate) secure_credential_search_blobs: Vec<String>,
    pub(crate) secure_key_search_blobs: Vec<String>,
    pub(crate) palette_text_input: crate::TextEditState,
    pub(crate) secure_text_input: crate::TextEditState,
    pub(crate) secure_text_target: Option<crate::forms::SecureInputTarget>,
    pub(crate) vault_modal_text_input: crate::TextEditState,
    pub(crate) vault_modal_text_field: Option<usize>,
    pub(crate) terminal_metrics: Option<TerminalMetrics>,
    pub(crate) last_applied_geometry: Option<TerminalGeometry>,
    pub(crate) terminal_resize_epoch: u64,
    pub(crate) active_terminal_rows: usize,
    pub(crate) terminal_surface: TerminalSurfaceState,
    pub(crate) terminal_ime: TerminalImeState,
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) perf_mode_env_override: Option<UiPerfMode>,
    pub(crate) perf_overlay: PerfOverlayState,
    pub(crate) sidebar_width: f32,
    pub(crate) sidebar_resizing: bool,
    pub(crate) terminal_selection: Option<TerminalSelection>,
    pub(crate) terminal_turn_selection: Option<TerminalTurnSelection>,
    pub(crate) terminal_drag_anchor: Option<TerminalSelectionPoint>,
    pub(crate) terminal_drag_auto_scroll: Option<TerminalDragAutoScrollState>,
    pub(crate) terminal_drag_auto_scroll_epoch: u64,
    pub(crate) terminal_hovered_link: Option<TerminalHoveredLink>,
    pub(crate) terminal_scroll: ScrollFrameAccumulator,
    pub(crate) terminal_scrollbar_hovered: bool,
    pub(crate) terminal_scrollbar_drag: Option<TerminalScrollbarDragState>,
    pub(crate) frame_pacer: FramePacer,
    pub(crate) toast: Option<ToastState>,
}

#[derive(Clone, Debug)]
pub(crate) struct ScrollFrameAccumulator {
    pub(crate) accumulated_row_delta: f32,
    pub(crate) pending_scroll_command: Option<TerminalScrollCommand>,
    pub(crate) pending_flush_epoch: u64,
    pub(crate) flush_scheduled: bool,
    pub(crate) last_scroll_dispatch_at: Option<Instant>,
    pub(crate) idle_epoch: u64,
    pub(crate) scroll_batches_dispatched: usize,
}

impl Default for ScrollFrameAccumulator {
    fn default() -> Self {
        Self {
            accumulated_row_delta: 0.0,
            pending_scroll_command: None,
            pending_flush_epoch: 0,
            flush_scheduled: false,
            last_scroll_dispatch_at: None,
            idle_epoch: 0,
            scroll_batches_dispatched: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ToastState {
    pub(crate) message: String,
    pub(crate) shown_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalSelectionPoint {
    pub(crate) row: u64,
    pub(crate) col: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalSelection {
    pub(crate) anchor: TerminalSelectionPoint,
    pub(crate) focus: TerminalSelectionPoint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminalTurnSelection {
    pub(crate) turn_id: u64,
    pub(crate) text: String,
    pub(crate) start_row: u64,
    pub(crate) end_row: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalDragAutoScrollDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TerminalDragAutoScrollState {
    pub(crate) direction: TerminalDragAutoScrollDirection,
    pub(crate) rows_per_tick: isize,
    pub(crate) pointer: Point<Pixels>,
    pub(crate) epoch: u64,
    pub(crate) frame_scheduled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminalHoveredLink {
    pub(crate) row: u64,
    pub(crate) row_revision: u64,
    pub(crate) col_range: Range<usize>,
    pub(crate) url: String,
    pub(crate) modifier_active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TerminalMetrics {
    pub(crate) cell_width_px: f32,
    pub(crate) cell_height_px: f32,
    pub(crate) line_height_px: f32,
    pub(crate) font_size_px: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TerminalImeState {
    pub(crate) marked_text: String,
    pub(crate) marked_selected_range_utf16: Option<Range<usize>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TerminalRendererMetrics {
    pub(crate) visible_rows: usize,
    pub(crate) visible_cells: usize,
    pub(crate) rebuilt_rows: usize,
    pub(crate) fragments: usize,
    pub(crate) background_quads: usize,
    pub(crate) special_glyph_cells: usize,
    pub(crate) wide_cells: usize,
    pub(crate) shape_hits: usize,
    pub(crate) shape_misses: usize,
    pub(crate) row_cache_hits: usize,
    pub(crate) row_cache_misses: usize,
    pub(crate) link_rows_deferred: usize,
    pub(crate) scroll_batches_dispatched: usize,
    pub(crate) width_mismatch_fragments: usize,
    pub(crate) cell_aligned_fallback_fragments: usize,
    pub(crate) max_width_error_milli_px: u32,
    pub(crate) total_width_error_milli_px: u64,
}

pub(crate) fn sidebar_occupied_width_px(sidebar_width: f32) -> f32 {
    sidebar_width + SIDEBAR_DRAG_TARGET_PX
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
