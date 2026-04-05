mod backend;
mod palette;
mod theme;

use std::{
    collections::{HashMap, VecDeque},
    env, fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use gpui::{
    App, Application, Bounds, Context, Div, FocusHandle, Focusable, FontWeight, KeyDownEvent,
    MouseButton, Pixels, ShapedLine, SharedString, TextRun, Window, WindowBackgroundAppearance,
    WindowBounds, WindowOptions, canvas, deferred, div, fill, font, point, prelude::*, px, size,
};
use seance_terminal::{
    SessionPerfSnapshot, TerminalCell, TerminalCellStyle, TerminalColor, TerminalGeometry,
    TerminalRow, TerminalSession,
};
use seance_vault::{
    CredentialSummary, HostAuthRef, HostSummary, KeySummary, PrivateKeyAlgorithm,
    PrivateKeySource, SecretString, VaultHostProfile, VaultPasswordCredential, VaultStore,
};
use tracing::trace;
use zeroize::Zeroizing;

use backend::UiBackend;
use palette::{PaletteAction, PaletteGroup, build_items};
use seance_ssh::{SftpEntry, SshConnectResult, SshError};
use theme::{Theme, ThemeId};

const SIDEBAR_WIDTH: f32 = 260.0;
const SIDEBAR_FONT_MONO: &str = "JetBrains Mono";
const SIDEBAR_MONO_SIZE_PX: f32 = 11.0;
const PERF_HISTORY_LIMIT: usize = 120;
const PERF_WINDOW: Duration = Duration::from_secs(1);
const TERMINAL_FONT_FAMILY: &str = "Menlo";
const TERMINAL_FONT_SIZE_PX: f32 = 13.0;
const TERMINAL_LINE_HEIGHT_PX: f32 = 19.0;
const TERMINAL_PANE_PADDING_PX: f32 = 16.0;

pub fn run(vault: VaultStore) -> Result<()> {
    let mut backend = UiBackend::new(vault)?;
    let vault_status = backend.vault_status();
    let device_unlock_attempted = vault_status.initialized;
    if vault_status.initialized {
        let _ = backend.try_unlock_with_device();
    }
    let unlocked = backend.vault_status().unlocked;
    let initial_saved_hosts = if unlocked {
        backend.list_hosts().unwrap_or_default()
    } else {
        Vec::new()
    };
    let initial_credentials = if unlocked {
        backend.list_password_credentials().unwrap_or_default()
    } else {
        Vec::new()
    };
    let initial_keys = if unlocked {
        backend.list_private_keys().unwrap_or_default()
    } else {
        Vec::new()
    };
    let initial = backend
        .spawn_local_session()
        .expect("failed to create initial local session");
    let initial_id = initial.id();

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_background: WindowBackgroundAppearance::Blurred,
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("Séance".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                cx.new(move |cx| {
                    let entity = cx.entity();
                    let focus_handle = cx.focus_handle();
                    focus_handle.focus(window);

                    let mut ws = SeanceWorkspace {
                        focus_handle,
                        sessions: vec![initial],
                        session_kinds: HashMap::from([(initial_id, SessionKind::Local)]),
                        active_session_id: initial_id,
                        backend,
                        saved_hosts: initial_saved_hosts,
                        selected_host_id: None,
                        connecting_host_id: None,
                        unlock_form: UnlockFormState::new(
                            vault_status.initialized,
                            unlocked,
                            device_unlock_attempted,
                        ),
                        host_editor: None,
                        credential_editor: None,
                        vault_panel_open: false,
                        sftp_browser: None,
                        cached_credentials: initial_credentials,
                        cached_keys: initial_keys,
                        status_message: None,
                        active_theme: ThemeId::ObsidianSmoke,
                        palette_open: false,
                        palette_query: String::new(),
                        palette_selected: 0,
                        terminal_metrics: None,
                        last_applied_geometry: None,
                        active_terminal_rows: TerminalGeometry::default().size.rows as usize,
                        terminal_surface: TerminalSurfaceState {
                            theme_id: ThemeId::ObsidianSmoke,
                            ..Default::default()
                        },
                        perf_overlay: PerfOverlayState::new(perf_mode_from_env()),
                    };
                    cx.observe_window_bounds(window, |this: &mut SeanceWorkspace, window, cx| {
                        this.apply_active_terminal_geometry(window);
                        this.invalidate_terminal_surface();
                        this.perf_overlay.mark_input(RedrawReason::TerminalUpdate);
                        cx.notify();
                    })
                    .detach();
                    ws.apply_active_terminal_geometry(window);
                    if let Some(notify_rx) = ws.active_session().and_then(|s| s.take_notify_rx()) {
                        SeanceWorkspace::schedule_session_watcher(
                            window,
                            cx,
                            entity.clone(),
                            notify_rx,
                        );
                    }
                    ws
                })
            },
        )
        .expect("failed to open Séance window");
    });
    Ok(())
}

struct SeanceWorkspace {
    focus_handle: FocusHandle,
    sessions: Vec<Arc<dyn TerminalSession>>,
    session_kinds: HashMap<u64, SessionKind>,
    active_session_id: u64,
    backend: UiBackend,
    saved_hosts: Vec<HostSummary>,
    selected_host_id: Option<String>,
    connecting_host_id: Option<String>,
    unlock_form: UnlockFormState,
    host_editor: Option<HostEditorState>,
    credential_editor: Option<CredentialEditorState>,
    vault_panel_open: bool,
    sftp_browser: Option<SftpBrowserState>,
    cached_credentials: Vec<CredentialSummary>,
    cached_keys: Vec<KeySummary>,
    status_message: Option<String>,
    active_theme: ThemeId,
    palette_open: bool,
    palette_query: String,
    palette_selected: usize,
    terminal_metrics: Option<TerminalMetrics>,
    last_applied_geometry: Option<TerminalGeometry>,
    active_terminal_rows: usize,
    terminal_surface: TerminalSurfaceState,
    perf_overlay: PerfOverlayState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionKind {
    Local,
    Remote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SftpSortMode {
    Name,
    Size,
    Modified,
}

struct SftpBrowserState {
    session_id: u64,
    session_label: String,
    current_path: String,
    entries: Vec<SftpEntry>,
    selected_index: usize,
    sort_mode: SftpSortMode,
    error: Option<String>,
    path_history: Vec<String>,
    confirm_delete: Option<String>,
    rename_target: Option<(usize, String)>,
    mkdir_input: Option<String>,
    scroll_offset: usize,
}

impl SftpBrowserState {
    fn new(session_id: u64, session_label: String, initial_path: String) -> Self {
        Self {
            session_id,
            session_label,
            current_path: initial_path,
            entries: Vec::new(),
            selected_index: 0,
            sort_mode: SftpSortMode::Name,
            error: None,
            path_history: Vec::new(),
            confirm_delete: None,
            rename_target: None,
            mkdir_input: None,
            scroll_offset: 0,
        }
    }

    fn selected_entry(&self) -> Option<&SftpEntry> {
        self.entries.get(self.selected_index)
    }

    fn sort_entries(&mut self) {
        self.entries.sort_by(|a, b| {
            match (a.is_dir, b.is_dir) {
                (true, false) => return std::cmp::Ordering::Less,
                (false, true) => return std::cmp::Ordering::Greater,
                _ => {}
            }
            match self.sort_mode {
                SftpSortMode::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SftpSortMode::Size => a.size.cmp(&b.size).reverse(),
                SftpSortMode::Modified => {
                    let am = a.modified.unwrap_or(0);
                    let bm = b.modified.unwrap_or(0);
                    am.cmp(&bm).reverse()
                }
            }
        });
    }

    fn parent_path(&self) -> Option<String> {
        let path = self.current_path.as_str();
        if path == "/" {
            return None;
        }
        let trimmed = path.trim_end_matches('/');
        match trimmed.rfind('/') {
            Some(0) => Some("/".into()),
            Some(idx) => Some(trimmed[..idx].to_string()),
            None => Some("/".into()),
        }
    }

    fn clamp_selection(&mut self) {
        if self.entries.is_empty() {
            self.selected_index = 0;
        } else if self.selected_index >= self.entries.len() {
            self.selected_index = self.entries.len() - 1;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TerminalMetrics {
    cell_width_px: f32,
    cell_height_px: f32,
    line_height_px: f32,
    font_size_px: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalGlyphPolicy {
    GroupableAscii,
    PerCellSpecial,
    WideCell,
}

#[derive(Clone, Debug)]
struct TerminalPaintFragment {
    x: Pixels,
    line: ShapedLine,
}

#[derive(Clone, Copy, Debug)]
struct TerminalPaintQuad {
    x: Pixels,
    width: Pixels,
    color: gpui::Hsla,
}

#[derive(Clone, Debug, Default)]
struct TerminalPaintRow {
    y: Pixels,
    backgrounds: Vec<TerminalPaintQuad>,
    underlines: Vec<TerminalPaintQuad>,
    fragments: Vec<TerminalPaintFragment>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TerminalRendererMetrics {
    visible_rows: usize,
    visible_cells: usize,
    fragments: usize,
    background_quads: usize,
    special_glyph_cells: usize,
    wide_cells: usize,
    shape_hits: usize,
    shape_misses: usize,
}

#[derive(Clone, Debug)]
struct TerminalSurfaceState {
    active_session_id: u64,
    snapshot_seq: u64,
    geometry: Option<TerminalGeometry>,
    theme_id: ThemeId,
    rows: Vec<TerminalPaintRow>,
    metrics: TerminalRendererMetrics,
    shape_cache: ShapeCache,
}

impl Default for TerminalSurfaceState {
    fn default() -> Self {
        Self {
            active_session_id: 0,
            snapshot_seq: 0,
            geometry: None,
            theme_id: ThemeId::ObsidianSmoke,
            rows: Vec::new(),
            metrics: TerminalRendererMetrics::default(),
            shape_cache: ShapeCache::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct ShapeCache {
    entries: HashMap<ShapeCacheKey, CachedShapeLine>,
    generation: u64,
}

#[derive(Clone, Debug)]
struct CachedShapeLine {
    line: ShapedLine,
    last_used: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ShapeCacheKey {
    text: String,
    font_size_bits: u32,
    bold: bool,
    italic: bool,
    color: HslaKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct HslaKey {
    h: u32,
    s: u32,
    l: u32,
    a: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TerminalFragmentPlan {
    text: String,
    style: TerminalCellStyle,
    glyph_policy: TerminalGlyphPolicy,
    start_col: usize,
    cell_count: usize,
}

#[derive(Clone, Debug)]
struct PreparedTerminalSurface {
    rows: Vec<TerminalPaintRow>,
    line_height_px: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnlockMode {
    Create,
    Unlock,
}

#[derive(Debug)]
struct UnlockFormState {
    mode: UnlockMode,
    passphrase: Zeroizing<String>,
    confirm_passphrase: Zeroizing<String>,
    selected_field: usize,
    message: Option<String>,
    completed: bool,
}

impl UnlockFormState {
    fn new(initialized: bool, unlocked: bool, device_unlock_attempted: bool) -> Self {
        let mode = if initialized {
            UnlockMode::Unlock
        } else {
            UnlockMode::Create
        };
        let message = if unlocked {
            Some("Vault unlocked from the local device key store.".into())
        } else if initialized && device_unlock_attempted {
            Some("Device unlock unavailable. Enter your recovery passphrase.".into())
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

    fn reset_for_unlock(&mut self) {
        self.mode = UnlockMode::Unlock;
        self.passphrase.clear();
        self.confirm_passphrase.clear();
        self.selected_field = 0;
        self.completed = false;
    }

    fn is_visible(&self) -> bool {
        !self.completed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostField {
    Label,
    Hostname,
    Username,
    Port,
    Notes,
    Auth,
}

impl HostField {
    const ALL: [Self; 6] = [
        Self::Label,
        Self::Hostname,
        Self::Username,
        Self::Port,
        Self::Notes,
        Self::Auth,
    ];

    fn title(self) -> &'static str {
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
struct HostEditorState {
    host_id: Option<String>,
    label: String,
    hostname: String,
    username: String,
    port: String,
    notes: String,
    auth_items: Vec<HostAuthRef>,
    auth_cursor: usize,
    selected_field: usize,
    message: Option<String>,
}

impl HostEditorState {
    fn blank() -> Self {
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
                "Create an encrypted SSH host. Use the Auth section to select credentials."
                    .into(),
            ),
        }
    }

    fn from_host(host: VaultHostProfile) -> Self {
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

    fn field(&self) -> HostField {
        HostField::ALL[self.selected_field.min(HostField::ALL.len() - 1)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CredentialField {
    Label,
    UsernameHint,
    Secret,
}

impl CredentialField {
    const ALL: [Self; 3] = [Self::Label, Self::UsernameHint, Self::Secret];

    fn title(self) -> &'static str {
        match self {
            Self::Label => "Label",
            Self::UsernameHint => "Username Hint",
            Self::Secret => "Password",
        }
    }
}

#[derive(Debug, Clone)]
struct CredentialEditorState {
    credential_id: Option<String>,
    label: String,
    username_hint: String,
    secret: String,
    selected_field: usize,
    message: Option<String>,
}

impl CredentialEditorState {
    fn blank() -> Self {
        Self {
            credential_id: None,
            label: String::new(),
            username_hint: String::new(),
            secret: String::new(),
            selected_field: 0,
            message: Some("Store an encrypted password credential in the vault.".into()),
        }
    }

    fn from_credential(cred: VaultPasswordCredential) -> Self {
        Self {
            credential_id: Some(cred.id),
            label: cred.label,
            username_hint: cred.username_hint.unwrap_or_default(),
            secret: cred.secret,
            selected_field: 0,
            message: Some("Edit the credential. Tab to move, Enter on Password to save.".into()),
        }
    }

    fn field(&self) -> CredentialField {
        CredentialField::ALL[self.selected_field.min(CredentialField::ALL.len() - 1)]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum UiPerfMode {
    #[default]
    Off,
    Compact,
    Expanded,
}

impl UiPerfMode {
    fn next(self) -> Self {
        match self {
            Self::Off => Self::Compact,
            Self::Compact => Self::Expanded,
            Self::Expanded => Self::Off,
        }
    }

    fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RedrawReason {
    Input,
    TerminalUpdate,
    Palette,
    UiRefresh,
    #[default]
    Unknown,
}

impl RedrawReason {
    fn label(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::TerminalUpdate => "terminal",
            Self::Palette => "palette",
            Self::UiRefresh => "ui",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct FrameStats {
    frame_count_total: u64,
    fps_1s: f32,
    frame_time_last_ms: f32,
    frame_time_avg_ms: f32,
    frame_time_p95_ms: f32,
    present_interval_last_ms: f32,
    present_interval_avg_ms: f32,
    present_interval_p95_ms: f32,
    redraw_reason: RedrawReason,
}

#[derive(Debug)]
struct PerfOverlayState {
    mode: UiPerfMode,
    last_present_timestamp: Option<Instant>,
    present_timestamps: VecDeque<Instant>,
    present_intervals: VecDeque<(Instant, Duration)>,
    render_cost_samples: VecDeque<(Instant, Duration)>,
    ui_refresh_timestamps: VecDeque<Instant>,
    terminal_refresh_timestamps: VecDeque<Instant>,
    active_session_perf_snapshot: Option<SessionPerfSnapshot>,
    frame_stats: FrameStats,
    visible_line_count: usize,
    pending_redraw_reason: RedrawReason,
}

impl PerfOverlayState {
    fn new(mode: UiPerfMode) -> Self {
        Self {
            mode,
            last_present_timestamp: None,
            present_timestamps: VecDeque::with_capacity(PERF_HISTORY_LIMIT),
            present_intervals: VecDeque::with_capacity(PERF_HISTORY_LIMIT),
            render_cost_samples: VecDeque::with_capacity(PERF_HISTORY_LIMIT),
            ui_refresh_timestamps: VecDeque::with_capacity(PERF_HISTORY_LIMIT),
            terminal_refresh_timestamps: VecDeque::with_capacity(PERF_HISTORY_LIMIT),
            active_session_perf_snapshot: None,
            frame_stats: FrameStats::default(),
            visible_line_count: 0,
            pending_redraw_reason: RedrawReason::Unknown,
        }
    }

    fn reset_sampling_window(&mut self) {
        self.last_present_timestamp = None;
        self.present_timestamps.clear();
        self.present_intervals.clear();
        self.render_cost_samples.clear();
        self.ui_refresh_timestamps.clear();
        self.terminal_refresh_timestamps.clear();
        self.frame_stats = FrameStats::default();
        self.pending_redraw_reason = RedrawReason::Unknown;
    }

    fn mark_terminal_refresh_request(
        &mut self,
        now: Instant,
        reason: RedrawReason,
        session_perf: Option<SessionPerfSnapshot>,
    ) {
        self.pending_redraw_reason = reason;
        self.active_session_perf_snapshot = session_perf;
        self.terminal_refresh_timestamps.push_back(now);
        trim_instants(&mut self.terminal_refresh_timestamps, now, PERF_WINDOW);
        self.ui_refresh_timestamps.push_back(now);
        trim_instants(&mut self.ui_refresh_timestamps, now, PERF_WINDOW);
    }

    fn mark_ui_refresh_request(&mut self, now: Instant, reason: RedrawReason) {
        self.pending_redraw_reason = reason;
        self.ui_refresh_timestamps.push_back(now);
        trim_instants(&mut self.ui_refresh_timestamps, now, PERF_WINDOW);
    }

    fn mark_input(&mut self, reason: RedrawReason) {
        self.pending_redraw_reason = reason;
    }

    fn finish_render(&mut self, started_at: Instant, ended_at: Instant) {
        self.render_cost_samples
            .push_back((ended_at, ended_at.saturating_duration_since(started_at)));
        trim_timed_durations(&mut self.render_cost_samples, ended_at, PERF_WINDOW);
        if let Some(previous) = self.last_present_timestamp.replace(ended_at) {
            self.present_intervals
                .push_back((ended_at, ended_at.saturating_duration_since(previous)));
            trim_timed_durations(&mut self.present_intervals, ended_at, PERF_WINDOW);
        }
        self.present_timestamps.push_back(ended_at);
        trim_instants(&mut self.present_timestamps, ended_at, PERF_WINDOW);
        self.frame_stats = build_frame_stats(
            self.frame_stats.frame_count_total.saturating_add(1),
            &self.render_cost_samples,
            &self.present_intervals,
            &self.present_timestamps,
            self.pending_redraw_reason,
        );
        self.pending_redraw_reason = RedrawReason::Unknown;

        trace!(
            frame_count_total = self.frame_stats.frame_count_total,
            fps_1s = self.frame_stats.fps_1s,
            frame_time_last_ms = self.frame_stats.frame_time_last_ms,
            redraw_reason = self.frame_stats.redraw_reason.label(),
            "perf render sampled"
        );
    }

    fn ui_refreshes_last_second(&self) -> usize {
        self.ui_refresh_timestamps.len()
    }

    fn terminal_refreshes_last_second(&self) -> usize {
        self.terminal_refresh_timestamps.len()
    }

    fn frames_presented_last_second(&self) -> usize {
        self.present_timestamps.len()
    }

    fn active_session_dirty(&self) -> bool {
        self.active_session_perf_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.dirty_since_last_ui_frame)
    }

    fn vt_bytes_per_second(&self) -> usize {
        self.active_session_perf_snapshot
            .as_ref()
            .map(|snapshot| snapshot.terminal.vt_bytes_processed_since_last_snapshot)
            .unwrap_or(0)
    }
}

fn perf_mode_from_env() -> UiPerfMode {
    match env::var("SEANCE_PERF_HUD") {
        Ok(value) if value.eq_ignore_ascii_case("expanded") => UiPerfMode::Expanded,
        Ok(value)
            if value == "1"
                || value.eq_ignore_ascii_case("true")
                || value.eq_ignore_ascii_case("compact") =>
        {
            UiPerfMode::Compact
        }
        _ => UiPerfMode::Off,
    }
}

fn trim_instants(samples: &mut VecDeque<Instant>, now: Instant, window: Duration) {
    while let Some(front) = samples.front().copied() {
        if now.saturating_duration_since(front) <= window {
            break;
        }
        samples.pop_front();
    }
}

fn trim_timed_durations(
    samples: &mut VecDeque<(Instant, Duration)>,
    now: Instant,
    window: Duration,
) {
    while let Some((timestamp, _)) = samples.front().copied() {
        if now.saturating_duration_since(timestamp) <= window {
            break;
        }
        samples.pop_front();
    }
}

fn build_frame_stats(
    frame_count_total: u64,
    render_cost_samples: &VecDeque<(Instant, Duration)>,
    present_intervals: &VecDeque<(Instant, Duration)>,
    present_timestamps: &VecDeque<Instant>,
    redraw_reason: RedrawReason,
) -> FrameStats {
    let frame_time_last_ms = render_cost_samples
        .back()
        .map(|(_, duration)| duration.as_secs_f32() * 1_000.0)
        .unwrap_or_default();
    let frame_time_avg_ms = average_duration_ms(render_cost_samples);
    let frame_time_p95_ms = percentile_duration_ms(render_cost_samples, 0.95);
    let present_interval_last_ms = present_intervals
        .back()
        .map(|(_, duration)| duration.as_secs_f32() * 1_000.0)
        .unwrap_or_default();
    let present_interval_avg_ms = average_duration_ms(present_intervals);
    let present_interval_p95_ms = percentile_duration_ms(present_intervals, 0.95);

    FrameStats {
        frame_count_total,
        fps_1s: normalized_fps_1s(present_timestamps),
        frame_time_last_ms,
        frame_time_avg_ms,
        frame_time_p95_ms,
        present_interval_last_ms,
        present_interval_avg_ms,
        present_interval_p95_ms,
        redraw_reason,
    }
}

fn average_duration_ms(samples: &VecDeque<(Instant, Duration)>) -> f32 {
    if samples.is_empty() {
        0.0
    } else {
        samples
            .iter()
            .map(|(_, duration)| duration.as_secs_f32())
            .sum::<f32>()
            * 1_000.0
            / samples.len() as f32
    }
}

fn normalized_fps_1s(present_timestamps: &VecDeque<Instant>) -> f32 {
    present_timestamps.len() as f32
}

fn percentile_duration_ms(samples: &VecDeque<(Instant, Duration)>, percentile: f32) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let mut millis = samples
        .iter()
        .map(|(_, sample)| sample.as_secs_f32() * 1_000.0)
        .collect::<Vec<_>>();
    millis.sort_by(f32::total_cmp);
    let index = ((millis.len() - 1) as f32 * percentile).round() as usize;
    millis[index.min(millis.len() - 1)]
}

fn frame_budget_color(frame_ms: f32, theme: &Theme) -> gpui::Hsla {
    if frame_ms <= 16.7 {
        theme.accent
    } else if frame_ms <= 25.0 {
        theme.warning
    } else {
        theme.text_secondary
    }
}

fn perf_status_color(ok: bool, theme: &Theme) -> gpui::Hsla {
    if ok { theme.accent } else { theme.warning }
}

fn perf_mode_label(mode: UiPerfMode) -> &'static str {
    match mode {
        UiPerfMode::Off => "off",
        UiPerfMode::Compact => "compact",
        UiPerfMode::Expanded => "expanded",
    }
}

fn compact_perf_strings(state: &PerfOverlayState) -> Vec<(&'static str, String)> {
    let terminal = state
        .active_session_perf_snapshot
        .as_ref()
        .map(|snapshot| &snapshot.terminal);
    vec![
        ("fps", format!("{:.0}", state.frame_stats.fps_1s)),
        (
            "frame",
            format!(
                "{:.1}/{:.1}/{:.1} ms",
                state.frame_stats.frame_time_last_ms,
                state.frame_stats.frame_time_avg_ms,
                state.frame_stats.frame_time_p95_ms
            ),
        ),
        (
            "snapshot",
            format!(
                "{:.2} ms",
                terminal
                    .map(|metrics| metrics.last_snapshot_duration.as_secs_f32() * 1_000.0)
                    .unwrap_or_default()
            ),
        ),
        (
            "rows",
            terminal
                .map(|metrics| metrics.rendered_row_count.to_string())
                .unwrap_or_else(|| "0".into()),
        ),
        (
            "cells",
            terminal
                .map(|metrics| metrics.rendered_cell_count.to_string())
                .unwrap_or_else(|| "0".into()),
        ),
    ]
}

fn expanded_perf_strings(
    state: &PerfOverlayState,
    active_session_id: u64,
    palette_open: bool,
    renderer: TerminalRendererMetrics,
) -> Vec<(&'static str, String)> {
    let terminal = state
        .active_session_perf_snapshot
        .as_ref()
        .map(|snapshot| &snapshot.terminal);
    vec![
        ("ui refresh", state.ui_refreshes_last_second().to_string()),
        (
            "terminal refresh",
            state.terminal_refreshes_last_second().to_string(),
        ),
        (
            "presented",
            state.frames_presented_last_second().to_string(),
        ),
        (
            "present/ui",
            format!(
                "{}/{}",
                state.frames_presented_last_second(),
                state.ui_refreshes_last_second()
            ),
        ),
        (
            "cadence",
            format!(
                "{:.1}/{:.1}/{:.1} ms",
                state.frame_stats.present_interval_last_ms,
                state.frame_stats.present_interval_avg_ms,
                state.frame_stats.present_interval_p95_ms
            ),
        ),
        (
            "dirty",
            if state.active_session_dirty() {
                "yes".into()
            } else {
                "no".into()
            },
        ),
        ("vt bytes", state.vt_bytes_per_second().to_string()),
        (
            "truncated",
            terminal
                .map(|metrics| metrics.truncated_row_count.to_string())
                .unwrap_or_else(|| "0".into()),
        ),
        ("session", active_session_id.to_string()),
        (
            "palette",
            if palette_open {
                "open".into()
            } else {
                "closed".into()
            },
        ),
        ("visible", state.visible_line_count.to_string()),
        (
            "reason",
            state.frame_stats.redraw_reason.label().to_string(),
        ),
        ("plan rows", renderer.visible_rows.to_string()),
        ("fragments", renderer.fragments.to_string()),
        ("bg quads", renderer.background_quads.to_string()),
        ("shape hits", renderer.shape_hits.to_string()),
        ("shape misses", renderer.shape_misses.to_string()),
    ]
}

fn local_session_display_number_for_ids(
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

impl SeanceWorkspace {
    fn theme(&self) -> Theme {
        self.active_theme.theme()
    }

    fn session_kind(&self, id: u64) -> Option<SessionKind> {
        self.session_kinds.get(&id).copied()
    }

    fn local_session_display_number(&self, id: u64) -> Option<usize> {
        let session_ids = self
            .sessions
            .iter()
            .map(|session| session.id())
            .collect::<Vec<_>>();
        local_session_display_number_for_ids(&session_ids, &self.session_kinds, id)
    }

    fn session_display_title(&self, session: &Arc<dyn TerminalSession>) -> String {
        match self.session_kind(session.id()) {
            Some(SessionKind::Local) => self
                .local_session_display_number(session.id())
                .map(|number| format!("local-{number}"))
                .unwrap_or_else(|| session.title().to_string()),
            Some(SessionKind::Remote) | None => session.title().to_string(),
        }
    }

    fn session_display_badge(&self, session: &Arc<dyn TerminalSession>, active: bool) -> String {
        if active {
            return "live".into();
        }

        match self.session_kind(session.id()) {
            Some(SessionKind::Local) => self
                .local_session_display_number(session.id())
                .map(|number| format!("#{number}"))
                .unwrap_or_else(|| format!("#{}", session.id())),
            Some(SessionKind::Remote) | None => format!("#{}", session.id()),
        }
    }

    fn palette_session_labels(&self) -> HashMap<u64, String> {
        self.sessions
            .iter()
            .map(|session| (session.id(), self.session_display_title(session)))
            .collect()
    }

    fn remote_session_ids(&self) -> Vec<u64> {
        self.sessions
            .iter()
            .filter(|s| self.session_kind(s.id()) == Some(SessionKind::Remote))
            .map(|s| s.id())
            .collect()
    }

    fn terminal_metrics(&mut self, window: &Window) -> TerminalMetrics {
        if let Some(metrics) = self.terminal_metrics {
            return metrics;
        }

        let font_size = px(TERMINAL_FONT_SIZE_PX);
        let font_id = window
            .text_system()
            .resolve_font(&font(TERMINAL_FONT_FAMILY));
        let cell_width_px = window
            .text_system()
            .ch_advance(font_id, font_size)
            .map(f32::from)
            .unwrap_or(8.0)
            .ceil()
            .max(1.0);
        let line_height_px = TERMINAL_LINE_HEIGHT_PX.ceil().max(1.0);
        let metrics = TerminalMetrics {
            cell_width_px,
            cell_height_px: line_height_px,
            line_height_px,
            font_size_px: TERMINAL_FONT_SIZE_PX,
        };
        trace!(?metrics, "measured terminal metrics");
        self.terminal_metrics = Some(metrics);
        metrics
    }

    fn apply_active_terminal_geometry(&mut self, window: &Window) {
        let Some(session) = self.active_session().cloned() else {
            self.last_applied_geometry = None;
            self.active_terminal_rows = TerminalGeometry::default().size.rows as usize;
            return;
        };

        let metrics = self.terminal_metrics(window);
        let geometry = compute_terminal_geometry(window.viewport_size(), metrics)
            .unwrap_or_else(TerminalGeometry::default);
        self.active_terminal_rows = geometry.size.rows as usize;

        if self.last_applied_geometry == Some(geometry) {
            trace!(?geometry, "skipping unchanged UI terminal geometry");
            return;
        }

        trace!(
            ?geometry,
            session_id = session.id(),
            "computed UI terminal geometry"
        );
        if let Err(error) = session.resize(geometry) {
            trace!(
                ?geometry,
                session_id = session.id(),
                error = %error,
                "failed to apply terminal geometry"
            );
            return;
        }

        self.last_applied_geometry = Some(geometry);
        self.invalidate_terminal_surface();
    }

    fn schedule_session_watcher(
        window: &mut Window,
        cx: &mut Context<Self>,
        entity: gpui::Entity<Self>,
        notify_rx: std::sync::mpsc::Receiver<()>,
    ) {
        let notify_rx = Arc::new(std::sync::Mutex::new(notify_rx));
        window
            .spawn(cx, async move |cx| {
                loop {
                    let rx = Arc::clone(&notify_rx);
                    let recv_ok = cx
                        .background_executor()
                        .spawn(async move { rx.lock().unwrap().recv().is_ok() })
                        .await;
                    if !recv_ok {
                        break;
                    }
                    while notify_rx.lock().unwrap().try_recv().is_ok() {}
                    let _ = cx.update(|window, cx| {
                        entity.update(cx, |this, _| {
                            this.take_terminal_refresh_request();
                        });
                        window.refresh();
                    });
                }
            })
            .detach();
    }

    fn take_terminal_refresh_request(&mut self) -> bool {
        let Some(session) = self.active_session() else {
            return false;
        };

        let session_perf = session.perf_snapshot();
        self.perf_overlay.active_session_perf_snapshot = Some(session_perf.clone());
        if !session_perf.dirty_since_last_ui_frame {
            return false;
        }

        self.perf_overlay.mark_terminal_refresh_request(
            Instant::now(),
            RedrawReason::TerminalUpdate,
            Some(session_perf),
        );
        true
    }

    fn invalidate_terminal_surface(&mut self) {
        self.terminal_surface.snapshot_seq = 0;
        self.terminal_surface.geometry = None;
    }

    fn sync_terminal_surface(&mut self, window: &mut Window) {
        let Some(session) = self.active_session().cloned() else {
            self.terminal_surface.rows.clear();
            self.terminal_surface.metrics = TerminalRendererMetrics::default();
            self.terminal_surface.active_session_id = 0;
            self.terminal_surface.geometry = None;
            return;
        };

        let metrics = self.terminal_metrics(window);
        let geometry = self
            .last_applied_geometry
            .unwrap_or_else(TerminalGeometry::default);
        let snapshot_seq = self
            .perf_overlay
            .active_session_perf_snapshot
            .as_ref()
            .map(|snapshot| snapshot.terminal.snapshot_seq)
            .unwrap_or(0);
        let needs_rebuild = self.terminal_surface.active_session_id != session.id()
            || self.terminal_surface.snapshot_seq != snapshot_seq
            || self.terminal_surface.geometry != Some(geometry)
            || self.terminal_surface.theme_id != self.active_theme
            || self.terminal_surface.rows.is_empty();

        if !needs_rebuild {
            return;
        }

        let snapshot = session.snapshot();
        let (rows, metrics_report) = build_terminal_surface_rows(
            &snapshot.rows,
            geometry,
            metrics,
            self.active_theme,
            &self.theme(),
            &mut self.terminal_surface.shape_cache,
            window,
        );

        self.terminal_surface.rows = rows;
        self.terminal_surface.metrics = metrics_report;
        self.terminal_surface.active_session_id = session.id();
        self.terminal_surface.snapshot_seq = snapshot_seq;
        self.terminal_surface.geometry = Some(geometry);
        self.terminal_surface.theme_id = self.active_theme;
    }

    fn toggle_perf_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next_mode = self.perf_overlay.mode.next();
        if matches!(self.perf_overlay.mode, UiPerfMode::Off) && next_mode.is_enabled() {
            self.perf_overlay.reset_sampling_window();
        }
        self.perf_overlay.mode = next_mode;
        self.perf_overlay
            .mark_ui_refresh_request(Instant::now(), RedrawReason::UiRefresh);
        window.refresh();
        cx.notify();
    }

    fn vault_unlocked(&self) -> bool {
        self.backend.vault_status().unlocked
    }

    fn refresh_saved_hosts(&mut self) {
        self.saved_hosts = if self.vault_unlocked() {
            self.backend.list_hosts().unwrap_or_default()
        } else {
            Vec::new()
        };

        if self
            .selected_host_id
            .as_ref()
            .is_some_and(|id| !self.saved_hosts.iter().any(|host| &host.id == id))
        {
            self.selected_host_id = self.saved_hosts.first().map(|host| host.id.clone());
        }
    }

    fn submit_unlock_form(&mut self, cx: &mut Context<Self>) {
        match self.unlock_form.mode {
            UnlockMode::Create => {
                if self.unlock_form.passphrase.trim().is_empty() {
                    self.unlock_form.message =
                        Some("Choose a non-empty recovery passphrase.".into());
                } else if self.unlock_form.passphrase != self.unlock_form.confirm_passphrase {
                    self.unlock_form.message = Some("Passphrases do not match yet.".into());
                } else {
                    let passphrase = SecretString::from(self.unlock_form.passphrase.to_string());
                    let result = self.backend.create_vault(&passphrase, "This Device");
                    self.unlock_form.passphrase.clear();
                    self.unlock_form.confirm_passphrase.clear();
                    match result {
                        Ok(()) => {
                            self.unlock_form.completed = true;
                            self.status_message = Some(
                                "Encrypted vault created. Device unlock is now enrolled.".into(),
                            );
                            self.refresh_saved_hosts();
                            self.refresh_vault_cache();
                        }
                        Err(err) => {
                            self.unlock_form.message = Some(err.to_string());
                        }
                    }
                }
            }
            UnlockMode::Unlock => {
                let passphrase = SecretString::from(self.unlock_form.passphrase.to_string());
                let result = self.backend.unlock_vault(&passphrase, "This Device");
                self.unlock_form.passphrase.clear();
                self.unlock_form.confirm_passphrase.clear();
                match result {
                    Ok(()) => {
                        self.unlock_form.completed = true;
                        self.status_message =
                            Some("Vault unlocked from the recovery passphrase.".into());
                        self.refresh_saved_hosts();
                        self.refresh_vault_cache();
                    }
                    Err(err) => {
                        self.unlock_form.message = Some(err.to_string());
                    }
                }
            }
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn lock_vault(&mut self, cx: &mut Context<Self>) {
        self.backend.lock_vault();
        self.saved_hosts.clear();
        self.cached_credentials.clear();
        self.cached_keys.clear();
        self.selected_host_id = None;
        self.host_editor = None;
        self.credential_editor = None;
        self.vault_panel_open = false;
        self.unlock_form.reset_for_unlock();
        self.unlock_form.message =
            Some("Vault locked. Decrypted records were cleared from memory.".into());
        self.status_message = Some("Vault locked.".into());
        self.palette_open = false;
        self.invalidate_terminal_surface();
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn begin_add_host(&mut self, cx: &mut Context<Self>) {
        if !self.vault_unlocked() {
            self.unlock_form.reset_for_unlock();
            self.unlock_form.message = Some("Unlock the vault before adding a saved host.".into());
        } else {
            self.refresh_vault_cache();
            self.host_editor = Some(HostEditorState::blank());
        }
        self.palette_open = false;
        self.perf_overlay.mark_input(RedrawReason::Palette);
        cx.notify();
    }

    fn begin_edit_host(&mut self, host_id: &str, cx: &mut Context<Self>) {
        self.refresh_vault_cache();
        match self.backend.load_host(host_id) {
            Ok(Some(host)) => {
                self.host_editor = Some(HostEditorState::from_host(host));
                self.selected_host_id = Some(host_id.into());
            }
            Ok(None) => {
                self.status_message = Some("Saved host not found.".into());
                self.refresh_saved_hosts();
            }
            Err(err) => {
                self.status_message = Some(err.to_string());
            }
        }
        self.palette_open = false;
        self.perf_overlay.mark_input(RedrawReason::Palette);
        cx.notify();
    }

    fn delete_saved_host(&mut self, host_id: &str, cx: &mut Context<Self>) {
        match self.backend.delete_host(host_id) {
            Ok(true) => {
                self.status_message = Some("Saved host tombstoned for future sync.".into());
                self.refresh_saved_hosts();
            }
            Ok(false) => {
                self.status_message = Some("Saved host already removed.".into());
            }
            Err(err) => {
                self.status_message = Some(err.to_string());
            }
        }
        self.palette_open = false;
        self.perf_overlay.mark_input(RedrawReason::Palette);
        cx.notify();
    }

    fn connect_saved_host(&mut self, host_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.connecting_host_id.is_some() {
            return;
        }

        let request = match self.backend.build_connect_request(host_id) {
            Ok(r) => r,
            Err(e) => {
                self.status_message = Some(e.to_string());
                cx.notify();
                return;
            }
        };

        self.connecting_host_id = Some(host_id.into());
        self.selected_host_id = Some(host_id.into());
        self.status_message = Some("Connecting\u{2026}".into());
        cx.notify();

        let ssh = self.backend.ssh_manager();
        let entity = cx.entity();

        window
            .spawn(cx, async move |cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { ssh.connect(request) })
                    .await;

                let _ = cx.update(|window, cx| {
                    entity.update(cx, |this, cx| {
                        this.finish_connect(result, window, cx);
                    });
                });
            })
            .detach();
    }

    fn finish_connect(
        &mut self,
        result: Result<SshConnectResult, SshError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.connecting_host_id = None;
        match result {
            Ok(result) => {
                let session: Arc<dyn TerminalSession> = result.session;
                if let Some(geometry) = self.last_applied_geometry {
                    let _ = session.resize(geometry);
                }
                self.active_session_id = session.id();
                self.session_kinds.insert(session.id(), SessionKind::Remote);
                if let Some(notify_rx) = session.take_notify_rx() {
                    Self::schedule_session_watcher(window, cx, cx.entity(), notify_rx);
                }
                self.sessions.push(session);
                self.vault_panel_open = false;
                self.status_message = Some("SSH session connected.".into());
                self.invalidate_terminal_surface();
            }
            Err(err) => {
                self.status_message = Some(err.to_string());
            }
        }
        self.palette_open = false;
        self.perf_overlay.mark_input(RedrawReason::Palette);
        cx.notify();
    }

    fn save_host_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.host_editor.as_ref() else {
            return;
        };
        let port = editor.port.trim().parse::<u16>().unwrap_or(22);
        let draft = VaultHostProfile {
            id: editor.host_id.clone().unwrap_or_default(),
            label: editor.label.trim().into(),
            hostname: editor.hostname.trim().into(),
            username: editor.username.trim().into(),
            port,
            notes: (!editor.notes.trim().is_empty()).then(|| editor.notes.trim().to_string()),
            auth_order: editor.auth_items.clone(),
        };

        match self.backend.save_host(draft) {
            Ok(summary) => {
                self.status_message = Some(format!(
                    "Saved host '{}' encrypted into the vault.",
                    summary.label
                ));
                self.host_editor = None;
                self.refresh_saved_hosts();
                self.selected_host_id = Some(summary.id);
            }
            Err(err) => {
                if let Some(editor) = self.host_editor.as_mut() {
                    editor.message = Some(err.to_string());
                }
            }
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn refresh_vault_cache(&mut self) {
        self.cached_credentials = self.backend.list_password_credentials().unwrap_or_default();
        self.cached_keys = self.backend.list_private_keys().unwrap_or_default();
    }

    fn open_vault_panel(&mut self, cx: &mut Context<Self>) {
        if !self.vault_unlocked() {
            self.unlock_form.reset_for_unlock();
            self.unlock_form.message =
                Some("Unlock the vault to manage credentials and keys.".into());
            cx.notify();
            return;
        }
        self.refresh_vault_cache();
        self.vault_panel_open = true;
        self.palette_open = false;
        cx.notify();
    }

    fn begin_edit_credential(&mut self, id: &str, cx: &mut Context<Self>) {
        match self.backend.load_password_credential(id) {
            Ok(Some(cred)) => {
                self.credential_editor = Some(CredentialEditorState::from_credential(cred));
            }
            Ok(None) => {
                self.status_message = Some("Credential not found.".into());
                self.refresh_vault_cache();
            }
            Err(err) => {
                self.status_message = Some(err.to_string());
            }
        }
        cx.notify();
    }

    fn save_credential_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.credential_editor.as_ref() else {
            return;
        };
        let draft = VaultPasswordCredential {
            id: editor.credential_id.clone().unwrap_or_default(),
            label: editor.label.trim().to_string(),
            username_hint: (!editor.username_hint.trim().is_empty())
                .then(|| editor.username_hint.trim().to_string()),
            secret: editor.secret.clone(),
        };
        match self.backend.save_password_credential(draft) {
            Ok(summary) => {
                self.status_message =
                    Some(format!("Saved credential '{}'.", summary.label));
                self.credential_editor = None;
                self.refresh_vault_cache();
            }
            Err(err) => {
                if let Some(editor) = self.credential_editor.as_mut() {
                    editor.message = Some(err.to_string());
                }
            }
        }
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn delete_credential(&mut self, id: &str, cx: &mut Context<Self>) {
        match self.backend.delete_password_credential(id) {
            Ok(true) => {
                self.status_message = Some("Credential deleted.".into());
                self.refresh_vault_cache();
            }
            Ok(false) => {
                self.status_message = Some("Credential already removed.".into());
            }
            Err(err) => {
                self.status_message = Some(err.to_string());
            }
        }
        cx.notify();
    }

    fn delete_private_key(&mut self, id: &str, cx: &mut Context<Self>) {
        match self.backend.delete_private_key(id) {
            Ok(true) => {
                self.status_message = Some("Key deleted.".into());
                self.refresh_vault_cache();
            }
            Ok(false) => {
                self.status_message = Some("Key already removed.".into());
            }
            Err(err) => {
                self.status_message = Some(err.to_string());
            }
        }
        cx.notify();
    }

    fn active_session(&self) -> Option<&Arc<dyn TerminalSession>> {
        self.sessions
            .iter()
            .find(|s| s.id() == self.active_session_id)
    }

    fn spawn_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(session) = self.backend.spawn_local_session() {
            if let Some(geometry) = self.last_applied_geometry {
                let _ = session.resize(geometry);
            }
            self.active_session_id = session.id();
            self.session_kinds.insert(session.id(), SessionKind::Local);
            if let Some(notify_rx) = session.take_notify_rx() {
                Self::schedule_session_watcher(window, cx, cx.entity(), notify_rx);
            }
            self.sessions.push(session);
            self.invalidate_terminal_surface();
            self.perf_overlay.mark_input(RedrawReason::Input);
            cx.notify();
        }
    }

    fn select_session(&mut self, id: u64, cx: &mut Context<Self>) {
        self.active_session_id = id;
        if let Some(geometry) = self.last_applied_geometry
            && let Some(session) = self.active_session()
        {
            let _ = session.resize(geometry);
            self.active_terminal_rows = geometry.size.rows as usize;
        }
        self.invalidate_terminal_surface();
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn close_session(&mut self, id: u64, cx: &mut Context<Self>) {
        self.sessions.retain(|s| s.id() != id);
        self.session_kinds.remove(&id);
        if self
            .sftp_browser
            .as_ref()
            .is_some_and(|b| b.session_id == id)
        {
            self.sftp_browser = None;
        }
        if self.active_session_id == id {
            self.active_session_id = self.sessions.last().map(|s| s.id()).unwrap_or(0);
        }
        if self.active_session_id == 0 {
            self.last_applied_geometry = None;
            self.active_terminal_rows = TerminalGeometry::default().size.rows as usize;
        }
        self.invalidate_terminal_surface();
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn toggle_palette(&mut self, cx: &mut Context<Self>) {
        if self.palette_open {
            self.palette_open = false;
        } else {
            self.palette_open = true;
            self.palette_query.clear();
            self.palette_selected = 0;
        }
        self.perf_overlay.mark_input(RedrawReason::Palette);
        cx.notify();
    }

    fn execute_palette_action(
        &mut self,
        action: PaletteAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            PaletteAction::NewLocalTerminal => self.spawn_session(window, cx),
            PaletteAction::SwitchSession(id) => self.select_session(id, cx),
            PaletteAction::CloseActiveSession => {
                let id = self.active_session_id;
                self.close_session(id, cx);
            }
            PaletteAction::SwitchTheme(tid) => {
                self.active_theme = tid;
                self.invalidate_terminal_surface();
            }
            PaletteAction::UnlockVault => {
                self.unlock_form.reset_for_unlock();
                self.unlock_form.message =
                    Some("Enter the recovery passphrase to unlock the vault.".into());
            }
            PaletteAction::LockVault => {
                self.lock_vault(cx);
                return;
            }
            PaletteAction::AddSavedHost => {
                self.begin_add_host(cx);
                return;
            }
            PaletteAction::OpenVaultPanel => {
                self.open_vault_panel(cx);
                return;
            }
            PaletteAction::AddPasswordCredential => {
                self.credential_editor = Some(CredentialEditorState::blank());
            }
            PaletteAction::EditPasswordCredential(id) => {
                self.begin_edit_credential(&id, cx);
                return;
            }
            PaletteAction::DeletePasswordCredential(id) => {
                self.delete_credential(&id, cx);
                return;
            }
            PaletteAction::ImportPrivateKey => {
                self.status_message = Some(
                    "Private key import backend is ready; UI import form is still pending.".into(),
                );
            }
            PaletteAction::GenerateEd25519Key => {
                match self
                    .backend
                    .generate_ed25519_key(format!("ed25519-{}", now_ui_suffix()))
                {
                    Ok(summary) => {
                        self.status_message =
                            Some(format!("Generated vault-backed key '{}'.", summary.label));
                        self.refresh_vault_cache();
                    }
                    Err(err) => self.status_message = Some(err.to_string()),
                }
            }
            PaletteAction::GenerateRsaKey => {
                match self
                    .backend
                    .generate_rsa_key(format!("rsa-{}", now_ui_suffix()))
                {
                    Ok(summary) => {
                        self.status_message =
                            Some(format!("Generated vault-backed key '{}'.", summary.label));
                        self.refresh_vault_cache();
                    }
                    Err(err) => self.status_message = Some(err.to_string()),
                }
            }
            PaletteAction::DeletePrivateKey(id) => {
                self.delete_private_key(&id, cx);
                return;
            }
            PaletteAction::EditSavedHost(id) => {
                self.begin_edit_host(&id, cx);
                return;
            }
            PaletteAction::DeleteSavedHost(id) => {
                self.delete_saved_host(&id, cx);
                return;
            }
            PaletteAction::ConnectSavedHost(id) => {
                self.connect_saved_host(&id, window, cx);
                return;
            }
            PaletteAction::OpenSftpBrowser(session_id) => {
                self.open_sftp_browser(session_id, cx);
                return;
            }
        }
        self.perf_overlay.mark_input(RedrawReason::Palette);
        self.palette_open = false;
        self.palette_query.clear();
        self.palette_selected = 0;
        cx.notify();
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;

        if mods.platform && mods.shift && key == "." {
            self.toggle_perf_mode(window, cx);
            return;
        }
        if self.unlock_form.is_visible() {
            self.handle_unlock_key(event, cx);
            return;
        }
        if self.credential_editor.is_some() {
            self.handle_credential_editor_key(event, cx);
            return;
        }
        if self.host_editor.is_some() {
            self.handle_host_editor_key(event, cx);
            return;
        }
        if mods.platform && key == "," {
            self.open_vault_panel(cx);
            return;
        }
        if mods.platform && key == "k" {
            self.toggle_palette(cx);
            return;
        }
        if mods.platform && key == "t" {
            self.spawn_session(window, cx);
            return;
        }
        if mods.platform && key == "w" {
            if self.active_session_id != 0 {
                let id = self.active_session_id;
                self.close_session(id, cx);
            }
            return;
        }

        if self.palette_open {
            self.handle_palette_key(event, window, cx);
            return;
        }

        if self.sftp_browser.is_some() {
            self.handle_sftp_key(event, window, cx);
            return;
        }

        if self.vault_panel_open && key == "escape" {
            self.vault_panel_open = false;
            self.perf_overlay.mark_input(RedrawReason::Input);
            cx.notify();
            return;
        }

        if let Some(bytes) = encode_keystroke(event)
            && let Some(session) = self.active_session()
        {
            let _ = session.send_input(bytes);
        }
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn handle_palette_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let key_char = event.keystroke.key_char.as_deref();

        match key {
            "escape" => {
                self.palette_open = false;
                self.palette_query.clear();
                self.palette_selected = 0;
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "up" => {
                self.palette_selected = self.palette_selected.saturating_sub(1);
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "down" => {
                let session_labels = self.palette_session_labels();
                let remote_ids = self.remote_session_ids();
                let count = build_items(
                    &self.sessions,
                    &session_labels,
                    &self.saved_hosts,
                    &self.cached_credentials,
                    &self.cached_keys,
                    self.active_session_id,
                    self.active_theme,
                    &self.palette_query,
                    self.vault_unlocked(),
                    &remote_ids,
                )
                .len();
                if self.palette_selected + 1 < count {
                    self.palette_selected += 1;
                }
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "enter" => {
                let session_labels = self.palette_session_labels();
                let remote_ids = self.remote_session_ids();
                let items = build_items(
                    &self.sessions,
                    &session_labels,
                    &self.saved_hosts,
                    &self.cached_credentials,
                    &self.cached_keys,
                    self.active_session_id,
                    self.active_theme,
                    &self.palette_query,
                    self.vault_unlocked(),
                    &remote_ids,
                );
                if let Some(item) = items.get(self.palette_selected) {
                    let action = item.action.clone();
                    self.execute_palette_action(action, window, cx);
                }
            }
            "backspace" => {
                self.palette_query.pop();
                self.palette_selected = 0;
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "tab" | "left" | "right" | "home" | "end" | "pageup" | "pagedown" => {}
            _ => {
                if let Some(ch) = key_char {
                    let m = event.keystroke.modifiers;
                    if !m.platform && !m.control && !m.function {
                        self.palette_query.push_str(ch);
                        self.palette_selected = 0;
                        self.perf_overlay.mark_input(RedrawReason::Palette);
                        cx.notify();
                    }
                }
            }
        }
    }

    fn handle_unlock_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let key_char = event.keystroke.key_char.as_deref();

        match key {
            "tab" | "down" => {
                let count = if matches!(self.unlock_form.mode, UnlockMode::Create) {
                    2
                } else {
                    1
                };
                self.unlock_form.selected_field = (self.unlock_form.selected_field + 1) % count;
            }
            "up" => {
                let count = if matches!(self.unlock_form.mode, UnlockMode::Create) {
                    2
                } else {
                    1
                };
                self.unlock_form.selected_field =
                    (self.unlock_form.selected_field + count - 1) % count;
            }
            "backspace" => {
                if self.unlock_form.selected_field == 0 {
                    self.unlock_form.passphrase.pop();
                } else {
                    self.unlock_form.confirm_passphrase.pop();
                }
            }
            "enter" => {
                self.submit_unlock_form(cx);
                return;
            }
            "escape" => {}
            _ => {
                if let Some(ch) = key_char {
                    let m = event.keystroke.modifiers;
                    if !m.platform && !m.control && !m.function {
                        if self.unlock_form.selected_field == 0 {
                            self.unlock_form.passphrase.push_str(ch);
                        } else {
                            self.unlock_form.confirm_passphrase.push_str(ch);
                        }
                    }
                }
            }
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn handle_host_editor_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let key_char = event.keystroke.key_char.as_deref();
        let mods = event.keystroke.modifiers;

        if mods.platform && key == "s" {
            self.save_host_editor(cx);
            return;
        }

        let Some(editor) = self.host_editor.as_mut() else {
            return;
        };

        let in_auth = matches!(editor.field(), HostField::Auth);

        match key {
            "escape" => {
                self.host_editor = None;
            }
            "tab" => {
                if mods.shift {
                    editor.selected_field =
                        (editor.selected_field + HostField::ALL.len() - 1) % HostField::ALL.len();
                } else {
                    editor.selected_field = (editor.selected_field + 1) % HostField::ALL.len();
                }
                editor.auth_cursor = 0;
            }
            "down" => {
                if in_auth {
                    let total = self.cached_credentials.len() + self.cached_keys.len();
                    if total > 0 {
                        editor.auth_cursor = (editor.auth_cursor + 1).min(total - 1);
                    }
                } else {
                    editor.selected_field = (editor.selected_field + 1) % HostField::ALL.len();
                    editor.auth_cursor = 0;
                }
            }
            "up" => {
                if in_auth {
                    editor.auth_cursor = editor.auth_cursor.saturating_sub(1);
                } else {
                    editor.selected_field =
                        (editor.selected_field + HostField::ALL.len() - 1) % HostField::ALL.len();
                    editor.auth_cursor = 0;
                }
            }
            "backspace" => match editor.field() {
                HostField::Label => { editor.label.pop(); }
                HostField::Hostname => { editor.hostname.pop(); }
                HostField::Username => { editor.username.pop(); }
                HostField::Port => { editor.port.pop(); }
                HostField::Notes => { editor.notes.pop(); }
                HostField::Auth => {}
            },
            "enter" | " " if in_auth => {
                self.toggle_host_auth_at_cursor();
            }
            "enter" => {
                editor.selected_field = (editor.selected_field + 1) % HostField::ALL.len();
            }
            _ => {
                if let Some(ch) = key_char {
                    if !mods.platform && !mods.control && !mods.function && !in_auth {
                        match editor.field() {
                            HostField::Label => editor.label.push_str(ch),
                            HostField::Hostname => editor.hostname.push_str(ch),
                            HostField::Username => editor.username.push_str(ch),
                            HostField::Port => {
                                if ch.chars().all(|c| c.is_ascii_digit()) {
                                    editor.port.push_str(ch);
                                }
                            }
                            HostField::Notes => editor.notes.push_str(ch),
                            HostField::Auth => {}
                        }
                    }
                }
            }
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn toggle_host_auth_at_cursor(&mut self) {
        let Some(editor) = self.host_editor.as_mut() else {
            return;
        };
        let cred_count = self.cached_credentials.len();
        let cursor = editor.auth_cursor;

        if cursor < cred_count {
            let cred = &self.cached_credentials[cursor];
            let auth_ref = HostAuthRef::Password {
                credential_id: cred.id.clone(),
            };
            if let Some(pos) = editor.auth_items.iter().position(|a| *a == auth_ref) {
                editor.auth_items.remove(pos);
            } else {
                editor.auth_items.push(auth_ref);
            }
        } else {
            let key_idx = cursor - cred_count;
            if key_idx < self.cached_keys.len() {
                let key = &self.cached_keys[key_idx];
                let matches_key =
                    |a: &HostAuthRef| matches!(a, HostAuthRef::PrivateKey { key_id, .. } if *key_id == key.id);
                if let Some(pos) = editor.auth_items.iter().position(matches_key) {
                    editor.auth_items.remove(pos);
                } else {
                    editor.auth_items.push(HostAuthRef::PrivateKey {
                        key_id: key.id.clone(),
                        passphrase_credential_id: None,
                    });
                }
            }
        }
    }

    fn handle_credential_editor_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(editor) = self.credential_editor.as_mut() else {
            return;
        };
        let key = event.keystroke.key.as_str();
        let key_char = event.keystroke.key_char.as_deref();
        let mods = event.keystroke.modifiers;

        match key {
            "escape" => {
                self.credential_editor = None;
            }
            "tab" | "down" => {
                editor.selected_field =
                    (editor.selected_field + 1) % CredentialField::ALL.len();
            }
            "up" => {
                editor.selected_field = (editor.selected_field + CredentialField::ALL.len() - 1)
                    % CredentialField::ALL.len();
            }
            "backspace" => match editor.field() {
                CredentialField::Label => { editor.label.pop(); }
                CredentialField::UsernameHint => { editor.username_hint.pop(); }
                CredentialField::Secret => { editor.secret.pop(); }
            },
            "enter" => {
                if matches!(editor.field(), CredentialField::Secret) {
                    self.save_credential_editor(cx);
                    return;
                }
                editor.selected_field =
                    (editor.selected_field + 1) % CredentialField::ALL.len();
            }
            _ => {
                if let Some(ch) = key_char {
                    if !mods.platform && !mods.control && !mods.function {
                        match editor.field() {
                            CredentialField::Label => editor.label.push_str(ch),
                            CredentialField::UsernameHint => editor.username_hint.push_str(ch),
                            CredentialField::Secret => editor.secret.push_str(ch),
                        }
                    }
                }
            }
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    // ─── Rendering ──────────────────────────────────────────

    fn sidebar_row_shell(&self, active: bool) -> Div {
        let t = self.theme();
        let row = div()
            .px(px(12.0))
            .py(px(6.0))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(px(8.0));

        if active {
            row.border_l_2()
                .border_color(t.sidebar_indicator)
                .bg(t.sidebar_row_active)
                .rounded_r_md()
                .shadow_sm()
        } else {
            row.ml(px(2.0))
                .rounded_r_md()
                .hover(|style| style.bg(t.sidebar_row_hover))
        }
    }

    fn render_sidebar_section_heading(&self, label: &'static str, meta: String) -> Div {
        let t = self.theme();

        div()
            .px(px(14.0))
            .py(px(4.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.sidebar_section_label)
                    .child(format!("-- {label}")),
            )
            .child(div().flex_1().h(px(1.0)).bg(t.accent_glow))
            .child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(t.sidebar_meta)
                    .child(meta),
            )
    }

    fn render_sidebar_header(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let mode_label = if self.selected_host_id.is_some() {
            "[ssh]"
        } else {
            "[local]"
        };

        div()
            .pt(px(36.0))
            .px(px(14.0))
            .pb(px(10.0))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.text_primary)
                            .child("séance"),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                            .text_color(t.sidebar_meta)
                            .child(mode_label),
                    ),
            )
            .child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(t.text_ghost)
                    .cursor_pointer()
                    .hover(|style| style.text_color(t.text_muted))
                    .child("^K")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_palette(cx);
                        }),
                    ),
            )
    }

    fn render_host_row(&self, host: &HostSummary, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let selected = self
            .selected_host_id
            .as_ref()
            .is_some_and(|id| id == &host.id);
        let is_connecting = self
            .connecting_host_id
            .as_ref()
            .is_some_and(|id| id == &host.id);
        let host_id = host.id.clone();
        let edit_id = host.id.clone();
        let delete_id = host.id.clone();
        let label = host.label.clone();
        let target = format!("{}@{}:{}", host.username, host.hostname, host.port);

        let mut row = self.sidebar_row_shell(selected || is_connecting)
            .child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(if selected || is_connecting {
                        t.accent
                    } else {
                        t.text_ghost
                    })
                    .child(if is_connecting {
                        "\u{2022}"
                    } else if selected {
                        ">"
                    } else {
                        " "
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if selected || is_connecting {
                                        t.text_primary
                                    } else {
                                        t.text_secondary
                                    })
                                    .line_clamp(1)
                                    .child(label),
                            )
                            .child(if is_connecting {
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(9.0))
                                    .px(px(5.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(t.accent_glow)
                                    .text_color(t.accent)
                                    .child("connecting\u{2026}")
                            } else {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .font_family(SIDEBAR_FONT_MONO)
                                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                            .text_color(t.text_ghost)
                                            .cursor_pointer()
                                            .hover(|style| style.text_color(t.text_secondary))
                                            .child("edit")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.begin_edit_host(&edit_id, cx);
                                                }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .font_family(SIDEBAR_FONT_MONO)
                                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                            .text_color(t.text_ghost)
                                            .cursor_pointer()
                                            .hover(|style| style.text_color(t.warning))
                                            .child("del")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.delete_saved_host(&delete_id, cx);
                                                }),
                                            ),
                                    )
                            }),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                            .text_color(t.sidebar_meta)
                            .line_clamp(1)
                            .child(target),
                    ),
            );

        if !is_connecting {
            row = row.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.selected_host_id = Some(host_id.clone());
                    this.connect_saved_host(&host_id, window, cx);
                }),
            );
        }

        row
    }

    fn render_session_row(
        &self,
        session: &Arc<dyn TerminalSession>,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme();
        let active = session.id() == self.active_session_id;
        let sid = session.id();
        let title = self.session_display_title(session);
        let snapshot = session.snapshot();
        let has_output = snapshot.rows.iter().any(|r| !r.plain_text().trim().is_empty());
        let preview = session_preview_text(&snapshot.rows).unwrap_or_else(|| {
            if has_output {
                "interactive session".into()
            } else {
                "waiting for output\u{2026}".into()
            }
        });
        let close_sid = sid;
        let badge = self.session_display_badge(session, active);

        self.sidebar_row_shell(active)
            .child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(if active { t.accent } else { t.text_ghost })
                    .child(if active { ">" } else { " " }),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if active {
                                        t.text_primary
                                    } else {
                                        t.text_secondary
                                    })
                                    .line_clamp(1)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(9.0))
                                    .px(px(5.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .when(active, |el| {
                                        el.bg(t.accent_glow).text_color(t.accent)
                                    })
                                    .when(!active, |el| {
                                        el.bg(t.glass_hover).text_color(t.sidebar_meta)
                                    })
                                    .child(badge),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                    .text_color(t.text_ghost)
                                    .child("$"),
                            )
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                    .text_color(t.sidebar_meta)
                                    .line_clamp(1)
                                    .child(preview),
                            ),
                    ),
            )
            .child({
                let is_remote = self.session_kind(sid) == Some(SessionKind::Remote);
                let sftp_sid = sid;
                let mut actions = div().flex().items_center().gap(px(4.0));
                if is_remote {
                    actions = actions.child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                            .text_color(t.text_ghost)
                            .cursor_pointer()
                            .hover(|style| style.text_color(t.accent))
                            .child("\u{25a4}")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.open_sftp_browser(sftp_sid, cx);
                                }),
                            ),
                    );
                }
                actions = actions.child(
                    div()
                        .font_family(SIDEBAR_FONT_MONO)
                        .text_size(px(SIDEBAR_MONO_SIZE_PX))
                        .text_color(t.text_ghost)
                        .cursor_pointer()
                        .hover(|style| style.text_color(t.text_secondary))
                        .child("x")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.close_session(close_sid, cx);
                            }),
                        ),
                );
                actions
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.select_session(sid, cx);
                }),
            )
    }

    fn render_hosts_section(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let unlocked = self.vault_unlocked();
        let meta = if unlocked {
            self.saved_hosts.len().to_string()
        } else {
            "locked".into()
        };

        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(self.render_sidebar_section_heading("hosts", meta));

        if unlocked {
            if self.saved_hosts.is_empty() {
                section = section.child(
                    div()
                        .px(px(14.0))
                        .font_family(SIDEBAR_FONT_MONO)
                        .text_size(px(SIDEBAR_MONO_SIZE_PX))
                        .text_color(t.sidebar_meta)
                        .child("no saved hosts"),
                );
            } else {
                let mut rows = div().flex().flex_col();
                for host in &self.saved_hosts {
                    rows = rows.child(self.render_host_row(host, cx));
                }
                section = section.child(rows);
            }

            section = section.child(
                div().px(px(14.0)).pt(px(2.0)).child(
                    div()
                        .font_family(SIDEBAR_FONT_MONO)
                        .text_size(px(SIDEBAR_MONO_SIZE_PX))
                        .text_color(t.text_ghost)
                        .cursor_pointer()
                        .hover(|style| style.text_color(t.text_secondary))
                        .child("+ add host")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.begin_add_host(cx);
                            }),
                        ),
                ),
            );
        } else {
            section = section.child(
                div()
                    .px(px(14.0))
                    .py(px(6.0))
                    .cursor_pointer()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(t.text_muted)
                    .hover(|style| style.text_color(t.text_secondary))
                    .child("vault locked -- unlock to view")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.unlock_form.reset_for_unlock();
                            this.unlock_form.message =
                                Some("Enter the recovery passphrase to unlock the vault.".into());
                            this.perf_overlay.mark_input(RedrawReason::Input);
                            cx.notify();
                        }),
                    ),
            );
        }

        section
    }

    fn render_sessions_section(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let mut section =
            div().flex().flex_col().gap(px(4.0)).child(
                self.render_sidebar_section_heading("sessions", self.sessions.len().to_string()),
            );

        if self.sessions.is_empty() {
            section = section.child(
                div()
                    .px(px(14.0))
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(t.sidebar_meta)
                    .child("no active sessions"),
            );
        } else {
            let mut rows = div().flex().flex_col();
            for session in &self.sessions {
                rows = rows.child(self.render_session_row(session, cx));
            }
            section = section.child(rows);
        }

        section = section.child(
            div().px(px(14.0)).pt(px(2.0)).child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(t.text_ghost)
                    .cursor_pointer()
                    .hover(|style| style.text_color(t.text_secondary))
                    .child("+ new local session")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.spawn_session(window, cx);
                        }),
                    ),
            ),
        );

        section
    }

    fn render_vault_section(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();

        let cred_count = self.cached_credentials.len();
        let key_count = self.cached_keys.len();
        let meta = format!("{} creds  {} keys", cred_count, key_count);

        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(self.render_sidebar_section_heading("vault", meta));

        let vault_action = |label: &'static str| {
            div()
                .px(px(14.0))
                .py(px(3.0))
                .cursor_pointer()
                .font_family(SIDEBAR_FONT_MONO)
                .text_size(px(SIDEBAR_MONO_SIZE_PX))
                .text_color(t.text_ghost)
                .hover(|s| s.text_color(t.text_secondary))
                .child(label)
        };

        section = section.child(
            div()
                .flex()
                .flex_col()
                .child(
                    vault_action(if self.vault_panel_open {
                        "\u{25c9} manage vault"
                    } else {
                        "\u{25cb} manage vault"
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if this.vault_panel_open {
                                this.vault_panel_open = false;
                            } else {
                                this.open_vault_panel(cx);
                            }
                            cx.notify();
                        }),
                    ),
                )
                .child(vault_action("+ add credential").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.credential_editor = Some(CredentialEditorState::blank());
                        this.perf_overlay.mark_input(RedrawReason::Input);
                        cx.notify();
                    }),
                ))
                .child(vault_action("+ generate ed25519").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        match this
                            .backend
                            .generate_ed25519_key(format!("ed25519-{}", now_ui_suffix()))
                        {
                            Ok(summary) => {
                                this.status_message =
                                    Some(format!("Generated key '{}'.", summary.label));
                                this.refresh_vault_cache();
                            }
                            Err(err) => this.status_message = Some(err.to_string()),
                        }
                        this.perf_overlay.mark_input(RedrawReason::Input);
                        cx.notify();
                    }),
                ))
                .child(vault_action("+ generate rsa").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        match this
                            .backend
                            .generate_rsa_key(format!("rsa-{}", now_ui_suffix()))
                        {
                            Ok(summary) => {
                                this.status_message =
                                    Some(format!("Generated key '{}'.", summary.label));
                                this.refresh_vault_cache();
                            }
                            Err(err) => this.status_message = Some(err.to_string()),
                        }
                        this.perf_overlay.mark_input(RedrawReason::Input);
                        cx.notify();
                    }),
                )),
        );

        section
    }

    fn render_sidebar_footer(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let vault_status = self.backend.vault_status();

        let vault_label = if vault_status.unlocked {
            "unlocked"
        } else {
            "locked"
        };

        let mut footer = div()
            .px(px(14.0))
            .pb(px(10.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(div().h(px(1.0)).bg(t.sidebar_separator))
            .child({
                let mut theme_row = div().flex().items_center().gap(px(5.0)).flex_wrap();
                let active_theme = self.active_theme;
                for &tid in ThemeId::ALL {
                    let tid_theme = tid.theme();
                    let is_active = tid == active_theme;
                    let accent_color = tid_theme.accent;
                    theme_row = theme_row.child(
                        div()
                            .w(px(10.0))
                            .h(px(10.0))
                            .rounded_full()
                            .bg(accent_color)
                            .cursor_pointer()
                            .when(is_active, |el| {
                                el.border_1()
                                    .border_color(t.text_secondary)
                                    .shadow_sm()
                            })
                            .when(!is_active, |el| {
                                el.hover(|s| s.border_1().border_color(t.sidebar_edge_bright))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.active_theme = tid;
                                            this.invalidate_terminal_surface();
                                            this.perf_overlay.mark_input(RedrawReason::Input);
                                            cx.notify();
                                        }),
                                    )
                            }),
                    );
                }
                theme_row
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .cursor_pointer()
                            .hover(|style| style.text_color(t.text_secondary))
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                    .text_color(t.sidebar_section_label)
                                    .child("vault:"),
                            )
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                    .text_color(if vault_status.unlocked {
                                        t.accent
                                    } else {
                                        t.warning
                                    })
                                    .child(vault_label),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if this.vault_unlocked() {
                                        this.lock_vault(cx);
                                    } else {
                                        this.unlock_form.reset_for_unlock();
                                        this.unlock_form.message = Some(
                                            "Enter the recovery passphrase to unlock the vault."
                                                .into(),
                                        );
                                        this.perf_overlay.mark_input(RedrawReason::Input);
                                        cx.notify();
                                    }
                                }),
                            ),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                            .text_color(t.text_ghost)
                            .cursor_pointer()
                            .hover(|style| style.text_color(t.text_muted))
                            .child("^K")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_palette(cx);
                                }),
                            ),
                    ),
            );

        if let Some(message) = self.status_message.clone() {
            footer = footer.child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(t.sidebar_meta)
                    .line_clamp(2)
                    .child(message),
            );
        }

        footer
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();

        div()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .justify_between()
            .bg(t.sidebar_bg_elevated)
            .border_r_1()
            .border_color(t.sidebar_edge)
            .shadow_lg()
            .child({
                let mut content = div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(self.render_sidebar_header(cx))
                    .child(self.render_hosts_section(cx))
                    .child(self.render_sessions_section(cx));
                if self.vault_unlocked() {
                    content = content.child(self.render_vault_section(cx));
                }
                content
            })
            .child(self.render_sidebar_footer(cx))
    }

    fn render_vault_panel(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = self.theme();

        let shell_divider = div()
            .w(px(2.0))
            .h_full()
            .border_l_1()
            .border_color(t.sidebar_edge_bright)
            .bg(t.shell_divider_glow);

        let mut content = div()
            .flex_1()
            .h_full()
            .bg(t.bg_void)
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_mouse_down(MouseButton::Left, {
                let fh = self.focus_handle.clone();
                move |_: &gpui::MouseDownEvent, window: &mut Window, _cx: &mut App| {
                    window.focus(&fh);
                }
            })
            .on_key_down(cx.listener(Self::handle_key_down))
            .p_6()
            .flex()
            .flex_col()
            .gap_6();

        content = content.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(20.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(t.text_primary)
                                .child("Vault"),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(t.text_muted)
                                .child("Manage encrypted credentials and SSH keys"),
                        ),
                )
                .child(
                    div()
                        .px_3()
                        .py(px(6.0))
                        .rounded_md()
                        .bg(t.glass_tint)
                        .border_1()
                        .border_color(t.glass_border)
                        .text_xs()
                        .text_color(t.text_secondary)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.glass_hover).text_color(t.text_primary))
                        .child("esc  back to terminal")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.vault_panel_open = false;
                                cx.notify();
                            }),
                        ),
                ),
        );

        content = content.child(self.render_vault_credentials_card(cx));
        content = content.child(self.render_vault_keys_card(cx));

        div().flex_1().h_full().flex().child(shell_divider).child(content)
    }

    fn render_vault_credentials_card(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();

        let mut card = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_xl()
            .bg(t.glass_tint)
            .border_1()
            .border_color(t.glass_border);

        card = card.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(t.text_primary)
                                .child("Credentials"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.text_muted)
                                .child(format!("{}", self.cached_credentials.len())),
                        ),
                )
                .child(
                    div()
                        .px_3()
                        .py(px(5.0))
                        .rounded_md()
                        .bg(t.accent_glow)
                        .text_xs()
                        .text_color(t.text_primary)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.accent))
                        .child("+ add credential")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.credential_editor = Some(CredentialEditorState::blank());
                                cx.notify();
                            }),
                        ),
                ),
        );

        if self.cached_credentials.is_empty() {
            card = card.child(
                div()
                    .py_4()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_ghost)
                            .child("No password credentials stored"),
                    ),
            );
        } else {
            card = card.child(
                div()
                    .h(px(1.0))
                    .bg(t.glass_border),
            );

            let mut rows = div().flex().flex_col();
            for cred in &self.cached_credentials {
                rows = rows.child(self.render_credential_row(cred, cx));
            }
            card = card.child(rows);
        }

        card
    }

    fn render_credential_row(
        &self,
        cred: &CredentialSummary,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme();
        let cred_id = cred.id.clone();
        let cred_id_del = cred.id.clone();
        let hint = cred
            .username_hint
            .as_deref()
            .unwrap_or("--");
        let truncated_id = if cred.id.len() > 8 {
            format!("{}...", &cred.id[..8])
        } else {
            cred.id.clone()
        };

        div()
            .flex()
            .items_center()
            .justify_between()
            .px_2()
            .py(px(6.0))
            .rounded_md()
            .hover(|s| s.bg(t.sidebar_row_hover))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_primary)
                            .child(cred.label.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.text_muted)
                                    .child(hint.to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_color(t.text_ghost)
                                    .child(truncated_id),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py(px(3.0))
                            .rounded(px(4.0))
                            .text_xs()
                            .text_color(t.text_ghost)
                            .cursor_pointer()
                            .hover(|s| s.text_color(t.text_secondary).bg(t.glass_hover))
                            .child("edit")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.begin_edit_credential(&cred_id, cx);
                                }),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py(px(3.0))
                            .rounded(px(4.0))
                            .text_xs()
                            .text_color(t.text_ghost)
                            .cursor_pointer()
                            .hover(|s| s.text_color(t.warning).bg(t.glass_hover))
                            .child("del")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.delete_credential(&cred_id_del, cx);
                                }),
                            ),
                    ),
            )
    }

    fn render_vault_keys_card(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();

        let mut card = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_xl()
            .bg(t.glass_tint)
            .border_1()
            .border_color(t.glass_border);

        card = card.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(t.text_primary)
                                .child("SSH Keys"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.text_muted)
                                .child(format!("{}", self.cached_keys.len())),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .px_3()
                                .py(px(5.0))
                                .rounded_md()
                                .bg(t.accent_glow)
                                .text_xs()
                                .text_color(t.text_primary)
                                .cursor_pointer()
                                .hover(|s| s.bg(t.accent))
                                .child("+ ed25519")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        match this
                                            .backend
                                            .generate_ed25519_key(format!("ed25519-{}", now_ui_suffix()))
                                        {
                                            Ok(summary) => {
                                                this.status_message =
                                                    Some(format!("Generated key '{}'.", summary.label));
                                                this.refresh_vault_cache();
                                            }
                                            Err(err) => this.status_message = Some(err.to_string()),
                                        }
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .px_3()
                                .py(px(5.0))
                                .rounded_md()
                                .bg(t.accent_glow)
                                .text_xs()
                                .text_color(t.text_primary)
                                .cursor_pointer()
                                .hover(|s| s.bg(t.accent))
                                .child("+ rsa-4096")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        match this
                                            .backend
                                            .generate_rsa_key(format!("rsa-{}", now_ui_suffix()))
                                        {
                                            Ok(summary) => {
                                                this.status_message =
                                                    Some(format!("Generated key '{}'.", summary.label));
                                                this.refresh_vault_cache();
                                            }
                                            Err(err) => this.status_message = Some(err.to_string()),
                                        }
                                        cx.notify();
                                    }),
                                ),
                        ),
                ),
        );

        if self.cached_keys.is_empty() {
            card = card.child(
                div()
                    .py_4()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_ghost)
                            .child("No SSH keys stored"),
                    ),
            );
        } else {
            card = card.child(
                div()
                    .h(px(1.0))
                    .bg(t.glass_border),
            );

            let mut rows = div().flex().flex_col();
            for key in &self.cached_keys {
                rows = rows.child(self.render_key_row(key, cx));
            }
            card = card.child(rows);
        }

        card
    }

    fn render_key_row(&self, key: &KeySummary, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let key_id_del = key.id.clone();

        let algo_label = match &key.algorithm {
            PrivateKeyAlgorithm::Ed25519 => "ED25519".to_string(),
            PrivateKeyAlgorithm::Rsa { bits } => format!("RSA-{bits}"),
        };
        let source_label = match key.source {
            PrivateKeySource::Generated => "generated",
            PrivateKeySource::Imported => "imported",
        };
        let truncated_id = if key.id.len() > 8 {
            format!("{}...", &key.id[..8])
        } else {
            key.id.clone()
        };

        div()
            .flex()
            .items_center()
            .justify_between()
            .px_2()
            .py(px(6.0))
            .rounded_md()
            .hover(|s| s.bg(t.sidebar_row_hover))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_primary)
                            .child(key.label.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .px(px(5.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(t.accent_glow)
                                    .text_xs()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_color(t.accent)
                                    .child(algo_label),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.text_muted)
                                    .child(source_label),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_color(t.text_ghost)
                                    .child(truncated_id),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py(px(3.0))
                            .rounded(px(4.0))
                            .text_xs()
                            .text_color(t.text_ghost)
                            .cursor_pointer()
                            .hover(|s| s.text_color(t.warning).bg(t.glass_hover))
                            .child("del")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.delete_private_key(&key_id_del, cx);
                                }),
                            ),
                    ),
            )
    }

    fn open_sftp_browser(&mut self, session_id: u64, cx: &mut Context<Self>) {
        let label = self
            .sessions
            .iter()
            .find(|s| s.id() == session_id)
            .map(|s| s.title().to_string())
            .unwrap_or_else(|| format!("session-{session_id}"));

        let home = match self.backend.sftp_canonicalize(session_id, ".") {
            Ok(p) => p,
            Err(_) => "/".into(),
        };

        let mut browser = SftpBrowserState::new(session_id, label, home);
        self.refresh_sftp_listing(&mut browser);
        self.sftp_browser = Some(browser);
        self.vault_panel_open = false;
        self.palette_open = false;
        cx.notify();
    }

    fn close_sftp_browser(&mut self, cx: &mut Context<Self>) {
        self.sftp_browser = None;
        cx.notify();
    }

    fn refresh_sftp_listing(&self, browser: &mut SftpBrowserState) {
        match self
            .backend
            .sftp_list_dir(browser.session_id, &browser.current_path)
        {
            Ok(entries) => {
                browser.entries = entries;
                browser.sort_entries();
                browser.error = None;
            }
            Err(err) => {
                browser.entries.clear();
                browser.error = Some(err.to_string());
            }
        }
        browser.clamp_selection();
    }

    fn sftp_navigate(&mut self, path: String, cx: &mut Context<Self>) {
        if let Some(browser) = &mut self.sftp_browser {
            browser.path_history.push(browser.current_path.clone());
            browser.current_path = path;
            browser.selected_index = 0;
            browser.scroll_offset = 0;
        }
        self.sftp_refresh_current();
        cx.notify();
    }

    fn sftp_navigate_up(&mut self, cx: &mut Context<Self>) {
        let parent = self
            .sftp_browser
            .as_ref()
            .and_then(|b| b.parent_path());
        if let Some(parent) = parent {
            self.sftp_navigate(parent, cx);
        }
    }

    fn sftp_refresh_current(&mut self) {
        if let Some(browser) = &mut self.sftp_browser {
            let session_id = browser.session_id;
            let path = browser.current_path.clone();
            match self.backend.sftp_list_dir(session_id, &path) {
                Ok(entries) => {
                    browser.entries = entries;
                    browser.sort_entries();
                    browser.error = None;
                }
                Err(err) => {
                    browser.entries.clear();
                    browser.error = Some(err.to_string());
                }
            }
            browser.clamp_selection();
        }
    }

    fn sftp_refresh(&mut self, cx: &mut Context<Self>) {
        self.sftp_refresh_current();
        cx.notify();
    }

    fn sftp_download_selected(&mut self, cx: &mut Context<Self>) {
        let (session_id, remote_path, file_name) = {
            let Some(browser) = &self.sftp_browser else {
                return;
            };
            let Some(entry) = browser.selected_entry() else {
                return;
            };
            if entry.is_dir {
                return;
            }
            (browser.session_id, entry.path.clone(), entry.name.clone())
        };

        match self.backend.sftp_read_file(session_id, &remote_path) {
            Ok(data) => {
                let downloads = dirs::download_dir()
                    .unwrap_or_else(|| PathBuf::from("."));
                let dest = downloads.join(&file_name);
                match fs::write(&dest, &data) {
                    Ok(()) => {
                        self.status_message = Some(format!(
                            "Downloaded {} ({} bytes) to {}",
                            file_name,
                            data.len(),
                            dest.display()
                        ));
                    }
                    Err(err) => {
                        self.status_message =
                            Some(format!("Failed to save {file_name}: {err}"));
                    }
                }
            }
            Err(err) => {
                self.status_message =
                    Some(format!("Download failed: {err}"));
            }
        }
        cx.notify();
    }

    fn sftp_upload_file(&mut self, local_path: &std::path::Path, cx: &mut Context<Self>) {
        let Some(browser) = &self.sftp_browser else {
            return;
        };
        let session_id = browser.session_id;
        let file_name = local_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("uploaded_file")
            .to_string();
        let remote_path = if browser.current_path == "/" {
            format!("/{file_name}")
        } else {
            format!("{}/{file_name}", browser.current_path)
        };

        match fs::read(local_path) {
            Ok(data) => {
                match self
                    .backend
                    .sftp_write_file(session_id, &remote_path, &data)
                {
                    Ok(()) => {
                        self.status_message = Some(format!("Uploaded {file_name}"));
                        self.sftp_refresh(cx);
                        return;
                    }
                    Err(err) => {
                        self.status_message =
                            Some(format!("Upload failed: {err}"));
                    }
                }
            }
            Err(err) => {
                self.status_message =
                    Some(format!("Failed to read local file: {err}"));
            }
        }
        cx.notify();
    }

    fn sftp_delete_selected(&mut self, cx: &mut Context<Self>) {
        let (session_id, path, is_dir) = {
            let Some(browser) = &self.sftp_browser else {
                return;
            };
            let Some(entry) = browser.selected_entry() else {
                return;
            };
            (browser.session_id, entry.path.clone(), entry.is_dir)
        };

        match self.backend.sftp_remove(session_id, &path, is_dir) {
            Ok(()) => {
                self.status_message = Some(format!("Deleted {path}"));
            }
            Err(err) => {
                self.status_message = Some(format!("Delete failed: {err}"));
            }
        }
        if let Some(browser) = &mut self.sftp_browser {
            browser.confirm_delete = None;
        }
        self.sftp_refresh(cx);
    }

    fn sftp_mkdir_confirm(&mut self, cx: &mut Context<Self>) {
        let (session_id, full_path) = {
            let Some(browser) = &self.sftp_browser else {
                return;
            };
            let Some(name) = &browser.mkdir_input else {
                return;
            };
            let name = name.trim();
            if name.is_empty() {
                return;
            }
            let full = if browser.current_path == "/" {
                format!("/{name}")
            } else {
                format!("{}/{name}", browser.current_path)
            };
            (browser.session_id, full)
        };

        match self.backend.sftp_mkdir(session_id, &full_path) {
            Ok(()) => {
                self.status_message = Some(format!("Created {full_path}"));
            }
            Err(err) => {
                self.status_message = Some(format!("mkdir failed: {err}"));
            }
        }
        if let Some(browser) = &mut self.sftp_browser {
            browser.mkdir_input = None;
        }
        self.sftp_refresh(cx);
    }

    fn sftp_rename_confirm(&mut self, cx: &mut Context<Self>) {
        let (session_id, old_path, new_path) = {
            let Some(browser) = &self.sftp_browser else {
                return;
            };
            let Some((idx, new_name)) = &browser.rename_target else {
                return;
            };
            let new_name = new_name.trim();
            if new_name.is_empty() {
                return;
            }
            let Some(entry) = browser.entries.get(*idx) else {
                return;
            };
            let new_full = if browser.current_path == "/" {
                format!("/{new_name}")
            } else {
                format!("{}/{new_name}", browser.current_path)
            };
            (browser.session_id, entry.path.clone(), new_full)
        };

        match self.backend.sftp_rename(session_id, &old_path, &new_path) {
            Ok(()) => {
                self.status_message = Some(format!("Renamed to {new_path}"));
            }
            Err(err) => {
                self.status_message = Some(format!("Rename failed: {err}"));
            }
        }
        if let Some(browser) = &mut self.sftp_browser {
            browser.rename_target = None;
        }
        self.sftp_refresh(cx);
    }

    fn handle_sftp_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;

        if let Some(browser) = &self.sftp_browser {
            if browser.mkdir_input.is_some() {
                match key {
                    "escape" => {
                        if let Some(b) = &mut self.sftp_browser {
                            b.mkdir_input = None;
                        }
                        cx.notify();
                        return;
                    }
                    "enter" => {
                        self.sftp_mkdir_confirm(cx);
                        return;
                    }
                    "backspace" => {
                        if let Some(b) = &mut self.sftp_browser {
                            if let Some(input) = &mut b.mkdir_input {
                                input.pop();
                            }
                        }
                        cx.notify();
                        return;
                    }
                    _ => {
                        if let Some(ch) = event.keystroke.key_char.as_deref() {
                            if let Some(b) = &mut self.sftp_browser {
                                if let Some(input) = &mut b.mkdir_input {
                                    input.push_str(ch);
                                }
                            }
                            cx.notify();
                        }
                        return;
                    }
                }
            }

            if browser.rename_target.is_some() {
                match key {
                    "escape" => {
                        if let Some(b) = &mut self.sftp_browser {
                            b.rename_target = None;
                        }
                        cx.notify();
                        return;
                    }
                    "enter" => {
                        self.sftp_rename_confirm(cx);
                        return;
                    }
                    "backspace" => {
                        if let Some(b) = &mut self.sftp_browser {
                            if let Some((_, name)) = &mut b.rename_target {
                                name.pop();
                            }
                        }
                        cx.notify();
                        return;
                    }
                    _ => {
                        if let Some(ch) = event.keystroke.key_char.as_deref() {
                            if let Some(b) = &mut self.sftp_browser {
                                if let Some((_, name)) = &mut b.rename_target {
                                    name.push_str(ch);
                                }
                            }
                            cx.notify();
                        }
                        return;
                    }
                }
            }

            if browser.confirm_delete.is_some() {
                match key {
                    "y" => {
                        self.sftp_delete_selected(cx);
                        return;
                    }
                    _ => {
                        if let Some(b) = &mut self.sftp_browser {
                            b.confirm_delete = None;
                        }
                        cx.notify();
                        return;
                    }
                }
            }
        }

        match key {
            "escape" => {
                self.close_sftp_browser(cx);
            }
            "up" | "k" if !mods.platform => {
                if let Some(b) = &mut self.sftp_browser {
                    b.selected_index = b.selected_index.saturating_sub(1);
                    if b.selected_index < b.scroll_offset {
                        b.scroll_offset = b.selected_index;
                    }
                }
                cx.notify();
            }
            "down" | "j" if !mods.platform => {
                if let Some(b) = &mut self.sftp_browser {
                    if !b.entries.is_empty() {
                        b.selected_index =
                            (b.selected_index + 1).min(b.entries.len() - 1);
                    }
                }
                cx.notify();
            }
            "enter" => {
                let action = self
                    .sftp_browser
                    .as_ref()
                    .and_then(|b| b.selected_entry())
                    .map(|e| (e.is_dir, e.path.clone(), e.name.clone()));
                if let Some((is_dir, path, name)) = action {
                    if name == ".." {
                        self.sftp_navigate_up(cx);
                    } else if is_dir {
                        self.sftp_navigate(path, cx);
                    } else {
                        self.sftp_download_selected(cx);
                    }
                }
            }
            "backspace" => {
                self.sftp_navigate_up(cx);
            }
            "delete" => {
                if let Some(b) = &mut self.sftp_browser {
                    if let Some(entry) = b.selected_entry() {
                        b.confirm_delete = Some(entry.name.clone());
                    }
                }
                cx.notify();
            }
            "n" if !mods.platform => {
                if let Some(b) = &mut self.sftp_browser {
                    b.mkdir_input = Some(String::new());
                }
                cx.notify();
            }
            "r" if !mods.platform && !mods.shift => {
                if let Some(b) = &mut self.sftp_browser {
                    if let Some(entry) = b.entries.get(b.selected_index) {
                        let idx = b.selected_index;
                        let name = entry.name.clone();
                        b.rename_target = Some((idx, name));
                    }
                }
                cx.notify();
            }
            "r" if mods.platform => {
                self.sftp_refresh(cx);
            }
            "s" if !mods.platform => {
                if let Some(b) = &mut self.sftp_browser {
                    b.sort_mode = match b.sort_mode {
                        SftpSortMode::Name => SftpSortMode::Size,
                        SftpSortMode::Size => SftpSortMode::Modified,
                        SftpSortMode::Modified => SftpSortMode::Name,
                    };
                    b.sort_entries();
                    b.clamp_selection();
                }
                cx.notify();
            }
            _ => {}
        }
    }

    fn render_sftp_panel(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let Some(browser) = &self.sftp_browser else {
            return div();
        };

        let shell_divider = div()
            .w(px(2.0))
            .h_full()
            .border_l_1()
            .border_color(t.sidebar_edge_bright)
            .bg(t.shell_divider_glow);

        let mut content = div()
            .flex_1()
            .h_full()
            .bg(t.bg_void)
            .track_focus(&self.focus_handle)
            .on_mouse_down(MouseButton::Left, {
                let fh = self.focus_handle.clone();
                move |_: &gpui::MouseDownEvent, window: &mut Window, _cx: &mut App| {
                    window.focus(&fh);
                }
            })
            .on_key_down(cx.listener(Self::handle_key_down))
            .overflow_hidden()
            .flex()
            .flex_col();

        let breadcrumb = self.render_sftp_breadcrumb(browser, &t, cx);
        content = content.child(breadcrumb);

        let toolbar = self.render_sftp_toolbar(browser, &t, cx);
        content = content.child(toolbar);

        if let Some(err) = &browser.error {
            content = content.child(
                div()
                    .px_6()
                    .py_3()
                    .bg(gpui::hsla(0.0, 0.6, 0.2, 0.3))
                    .border_b_1()
                    .border_color(gpui::hsla(0.0, 0.5, 0.3, 0.5))
                    .text_sm()
                    .text_color(gpui::hsla(0.0, 0.8, 0.7, 1.0))
                    .child(err.clone()),
            );
        }

        if let Some(name) = &browser.confirm_delete {
            content = content.child(
                div()
                    .px_6()
                    .py_3()
                    .bg(gpui::hsla(0.0, 0.4, 0.15, 0.5))
                    .border_b_1()
                    .border_color(gpui::hsla(0.0, 0.5, 0.3, 0.5))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(gpui::hsla(0.0, 0.8, 0.75, 1.0))
                            .child(format!("Delete \"{name}\"?")),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .px_2()
                            .py(px(2.0))
                            .rounded(px(3.0))
                            .bg(gpui::hsla(0.0, 0.5, 0.3, 0.6))
                            .text_color(gpui::hsla(0.0, 0.9, 0.85, 1.0))
                            .child("y confirm"),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .px_2()
                            .py(px(2.0))
                            .rounded(px(3.0))
                            .bg(t.glass_tint)
                            .text_color(t.text_muted)
                            .child("any key cancel"),
                    ),
            );
        }

        if let Some(input) = &browser.mkdir_input {
            content = content.child(
                div()
                    .px_6()
                    .py_3()
                    .bg(t.glass_tint)
                    .border_b_1()
                    .border_color(t.glass_border)
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_secondary)
                            .child("New folder:"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_sm()
                            .text_color(t.text_primary)
                            .child(if input.is_empty() {
                                "\u{2588}".to_string()
                            } else {
                                format!("{input}\u{2588}")
                            }),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child("enter confirm \u{00b7} esc cancel"),
                    ),
            );
        }

        if let Some((_, new_name)) = &browser.rename_target {
            content = content.child(
                div()
                    .px_6()
                    .py_3()
                    .bg(t.glass_tint)
                    .border_b_1()
                    .border_color(t.glass_border)
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_secondary)
                            .child("Rename to:"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_sm()
                            .text_color(t.text_primary)
                            .child(if new_name.is_empty() {
                                "\u{2588}".to_string()
                            } else {
                                format!("{new_name}\u{2588}")
                            }),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child("enter confirm \u{00b7} esc cancel"),
                    ),
            );
        }

        let header_row = div()
            .px_6()
            .py(px(6.0))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(t.glass_border)
            .child(
                div()
                    .w(px(28.0))
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_xs()
                    .text_color(t.text_ghost)
                    .child(""),
            )
            .child(
                div()
                    .flex_1()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_xs()
                    .text_color(t.text_ghost)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("NAME"),
            )
            .child(
                div()
                    .w(px(80.0))
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_xs()
                    .text_color(t.text_ghost)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_right()
                    .child("SIZE"),
            )
            .child(
                div()
                    .w(px(80.0))
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_xs()
                    .text_color(t.text_ghost)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_right()
                    .child("PERMS"),
            );
        content = content.child(header_row);

        let mut file_list = div().flex_1().flex().flex_col().overflow_hidden();

        if browser.entries.is_empty() && browser.error.is_none() {
            file_list = file_list.child(
                div()
                    .px_6()
                    .py_8()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_ghost)
                            .child("Empty directory"),
                    ),
            );
        } else {
            for (idx, entry) in browser.entries.iter().enumerate() {
                let selected = idx == browser.selected_index;
                let is_dir = entry.is_dir;
                let entry_path = entry.path.clone();

                let glyph = if entry.name == ".." {
                    "\u{2190}"
                } else if is_dir {
                    "\u{25b8}"
                } else {
                    sftp_file_glyph(&entry.name)
                };

                let glyph_color = if entry.name == ".." {
                    t.text_muted
                } else if is_dir {
                    t.accent
                } else {
                    t.text_ghost
                };

                let name_color = if selected {
                    t.text_primary
                } else if is_dir {
                    t.accent
                } else {
                    t.text_secondary
                };

                let size_str = if is_dir {
                    "\u{2014}".to_string()
                } else {
                    format_file_size(entry.size)
                };

                let perms_str = entry
                    .permissions
                    .map(format_unix_perms)
                    .unwrap_or_else(|| "\u{2014}".into());

                let row = div()
                    .px_6()
                    .py(px(4.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .when(selected, |el| {
                        el.bg(t.glass_tint)
                            .border_l_2()
                            .border_color(t.accent)
                    })
                    .when(!selected, |el| {
                        el.hover(|s| s.bg(t.glass_hover))
                    })
                    .child(
                        div()
                            .w(px(28.0))
                            .text_sm()
                            .text_color(glyph_color)
                            .child(glyph),
                    )
                    .child(
                        div()
                            .flex_1()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_sm()
                            .text_color(name_color)
                            .font_weight(if is_dir {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .line_clamp(1)
                            .child(entry.name.clone()),
                    )
                    .child(
                        div()
                            .w(px(80.0))
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .text_color(t.text_ghost)
                            .text_right()
                            .child(size_str),
                    )
                    .child(
                        div()
                            .w(px(80.0))
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .text_color(t.text_ghost)
                            .text_right()
                            .child(perms_str),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            if let Some(b) = &mut this.sftp_browser {
                                b.selected_index = idx;
                            }
                            if event.click_count == 2 {
                                let entry_info = this
                                    .sftp_browser
                                    .as_ref()
                                    .and_then(|b| b.entries.get(idx))
                                    .map(|e| (e.is_dir, e.name.clone()));
                                if let Some((is_dir, name)) = entry_info {
                                    if name == ".." {
                                        this.sftp_navigate_up(cx);
                                    } else if is_dir {
                                        this.sftp_navigate(entry_path.clone(), cx);
                                    } else {
                                        this.sftp_download_selected(cx);
                                    }
                                }
                            }
                            cx.notify();
                        }),
                    );

                file_list = file_list.child(row);
            }
        }

        content = content.child(file_list);

        let sort_label = match browser.sort_mode {
            SftpSortMode::Name => "name",
            SftpSortMode::Size => "size",
            SftpSortMode::Modified => "date",
        };
        let status = div()
            .px_6()
            .py(px(5.0))
            .border_t_1()
            .border_color(t.glass_border)
            .bg(t.glass_tint)
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child(format!("{} items", browser.entries.len())),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child(format!("sort: {sort_label} (s)")),
                    ),
            )
            .child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_xs()
                    .text_color(t.text_ghost)
                    .child(
                        "\u{2191}\u{2193} navigate \u{00b7} enter open \u{00b7} \u{232b} up \u{00b7} n mkdir \u{00b7} r rename \u{00b7} del delete",
                    ),
            );
        content = content.child(status);

        div()
            .flex_1()
            .h_full()
            .flex()
            .child(shell_divider)
            .child(content)
    }

    fn render_sftp_breadcrumb(
        &self,
        browser: &SftpBrowserState,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut breadcrumb = div()
            .px_6()
            .py_4()
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(t.glass_border);

        let mut path_row = div().flex().items_center().gap(px(2.0));

        let segments: Vec<&str> = browser
            .current_path
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        path_row = path_row.child(
            div()
                .font_family(SIDEBAR_FONT_MONO)
                .text_sm()
                .text_color(t.text_ghost)
                .cursor_pointer()
                .hover(|s| s.text_color(t.accent))
                .child("/")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.sftp_navigate("/".into(), cx);
                    }),
                ),
        );

        let mut accumulated = String::new();
        for (i, seg) in segments.iter().enumerate() {
            accumulated.push('/');
            accumulated.push_str(seg);
            let nav_path = accumulated.clone();
            let is_last = i == segments.len() - 1;

            path_row = path_row.child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_xs()
                    .text_color(t.text_ghost)
                    .child("/"),
            );

            path_row = path_row.child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_sm()
                    .text_color(if is_last {
                        t.text_primary
                    } else {
                        t.text_secondary
                    })
                    .font_weight(if is_last {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .cursor_pointer()
                    .hover(|s| s.text_color(t.accent))
                    .child(seg.to_string())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.sftp_navigate(nav_path.clone(), cx);
                        }),
                    ),
            );
        }

        breadcrumb = breadcrumb.child(path_row);

        let right_side = div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_xs()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(3.0))
                    .bg(t.accent_glow)
                    .text_color(t.accent)
                    .child(browser.session_label.clone()),
            )
            .child(
                div()
                    .px_3()
                    .py(px(6.0))
                    .rounded_md()
                    .bg(t.glass_tint)
                    .border_1()
                    .border_color(t.glass_border)
                    .text_xs()
                    .text_color(t.text_secondary)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.glass_hover).text_color(t.text_primary))
                    .child("esc  back to terminal")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.close_sftp_browser(cx);
                        }),
                    ),
            );
        breadcrumb = breadcrumb.child(right_side);

        breadcrumb
    }

    fn render_sftp_toolbar(
        &self,
        browser: &SftpBrowserState,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let has_selection = browser.selected_entry().is_some();
        let selected_is_file = browser
            .selected_entry()
            .map(|e| !e.is_dir)
            .unwrap_or(false);

        div()
            .px_6()
            .py(px(6.0))
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(t.glass_border)
            .child(sftp_toolbar_pill(
                "\u{2191} Up",
                browser.parent_path().is_some(),
                t,
                cx,
                |this, _, _, cx| {
                    this.sftp_navigate_up(cx);
                },
            ))
            .child(sftp_toolbar_pill(
                "\u{21bb} Refresh",
                true,
                t,
                cx,
                |this, _, _, cx| {
                    this.sftp_refresh(cx);
                },
            ))
            .child(sftp_toolbar_pill(
                "+ Folder",
                true,
                t,
                cx,
                |this, _, _, cx| {
                    if let Some(b) = &mut this.sftp_browser {
                        b.mkdir_input = Some(String::new());
                    }
                    cx.notify();
                },
            ))
            .child(sftp_toolbar_pill(
                "\u{21e3} Download",
                selected_is_file,
                t,
                cx,
                |this, _, _, cx| {
                    this.sftp_download_selected(cx);
                },
            ))
            .child(sftp_toolbar_pill(
                "\u{270e} Rename",
                has_selection,
                t,
                cx,
                |this, _, _, cx| {
                    if let Some(b) = &mut this.sftp_browser {
                        if let Some(entry) = b.entries.get(b.selected_index) {
                            let idx = b.selected_index;
                            let name = entry.name.clone();
                            b.rename_target = Some((idx, name));
                        }
                    }
                    cx.notify();
                },
            ))
            .child(sftp_toolbar_pill(
                "\u{00d7} Delete",
                has_selection,
                t,
                cx,
                |this, _, _, cx| {
                    if let Some(b) = &mut this.sftp_browser {
                        if let Some(entry) = b.selected_entry() {
                            b.confirm_delete = Some(entry.name.clone());
                        }
                    }
                    cx.notify();
                },
            ))
    }

    fn render_terminal_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = self.theme();

        div()
            .flex_1()
            .h_full()
            .flex()
            .child(
                div()
                    .w(px(2.0))
                    .h_full()
                    .border_l_1()
                    .border_color(t.sidebar_edge_bright)
                    .bg(t.shell_divider_glow),
            )
            .child(self.render_terminal_pane(window, cx))
    }

    fn render_terminal_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = self.theme();

        let base = div()
            .flex_1()
            .h_full()
            .bg(t.bg_void)
            .track_focus(&self.focus_handle)
            .on_mouse_down(MouseButton::Left, {
                let fh = self.focus_handle.clone();
                move |_: &gpui::MouseDownEvent, window: &mut Window, _cx: &mut App| {
                    window.focus(&fh);
                }
            })
            .on_key_down(cx.listener(Self::handle_key_down));

        if self.sessions.is_empty() || self.active_session().is_none() {
            self.perf_overlay.visible_line_count = 0;
            return base
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_4()
                .child(
                    div()
                        .text_size(px(48.0))
                        .text_color(t.text_ghost)
                        .child("◈"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(t.text_muted)
                        .child("No active sessions"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(t.glass_border)
                                .bg(t.glass_tint)
                                .text_xs()
                                .text_color(t.text_secondary)
                                .child("⌘K"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.text_muted)
                                .child("to open command palette"),
                        ),
                );
        }

        self.sync_terminal_surface(window);
        self.perf_overlay.visible_line_count = self.terminal_surface.metrics.visible_rows;
        let prepared = PreparedTerminalSurface {
            rows: self.terminal_surface.rows.clone(),
            line_height_px: self
                .terminal_metrics
                .unwrap_or(TerminalMetrics {
                    cell_width_px: 8.0,
                    cell_height_px: TERMINAL_LINE_HEIGHT_PX,
                    line_height_px: TERMINAL_LINE_HEIGHT_PX,
                    font_size_px: TERMINAL_FONT_SIZE_PX,
                })
                .line_height_px,
        };
        let exit_status = self
            .active_session()
            .and_then(|session| session.snapshot().exit_status);

        let mut term = base.p_4().child(
            canvas(
                move |_bounds, _window, _cx| prepared,
                move |bounds, prepared, window, cx| {
                    paint_terminal_surface(bounds, prepared, window, cx);
                },
            )
            .size_full(),
        );

        if let Some(exit_status) = exit_status {
            term = term.child(
                div()
                    .mt_3()
                    .text_xs()
                    .text_color(t.warning)
                    .child(format!("[process exited: {exit_status}]")),
            );
        }

        trace!(
            visible_line_count = self.perf_overlay.visible_line_count,
            visible_cell_count = self.terminal_surface.metrics.visible_cells,
            fragments = self.terminal_surface.metrics.fragments,
            background_quads = self.terminal_surface.metrics.background_quads,
            special_glyph_cells = self.terminal_surface.metrics.special_glyph_cells,
            wide_cells = self.terminal_surface.metrics.wide_cells,
            palette_open = self.palette_open,
            "rendered terminal pane"
        );

        term
    }

    fn render_palette_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        let session_labels = self.palette_session_labels();
        let remote_ids = self.remote_session_ids();
        let items = build_items(
            &self.sessions,
            &session_labels,
            &self.saved_hosts,
            &self.cached_credentials,
            &self.cached_keys,
            self.active_session_id,
            self.active_theme,
            &self.palette_query,
            self.vault_unlocked(),
            &remote_ids,
        );
        trace!(palette_items = items.len(), "rendered palette overlay");
        let selected = self.palette_selected.min(items.len().saturating_sub(1));
        let show_groups = self.palette_query.is_empty();

        let mut item_list = div().flex().flex_col().py_1();

        if items.is_empty() {
            item_list = item_list.child(
                div()
                    .py_3()
                    .flex()
                    .justify_center()
                    .text_sm()
                    .text_color(t.text_muted)
                    .child("No matching commands"),
            );
        }

        let mut prev_group: Option<PaletteGroup> = None;

        for (idx, item) in items.iter().enumerate() {
            if show_groups {
                let cur_group = item.group;
                if prev_group.map_or(true, |pg| pg != cur_group) {
                    let is_first = prev_group.is_none();
                    let mut header = div()
                        .px_4()
                        .pt(px(if is_first { 6.0 } else { 12.0 }))
                        .pb(px(4.0))
                        .flex()
                        .items_center()
                        .gap_2();

                    header = header
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(t.palette_group_label)
                                .child(cur_group.label()),
                        )
                        .child(div().flex_1().h(px(1.0)).bg(t.palette_group_separator));

                    item_list = item_list.child(header);
                    prev_group = Some(cur_group);
                }
            }

            let is_sel = idx == selected;
            let action = item.action.clone();

            let mut row = div()
                .mx_2()
                .px_2()
                .py(px(7.0))
                .rounded_lg()
                .flex()
                .items_center()
                .gap_3()
                .cursor_pointer();

            row = if is_sel {
                row.bg(t.selection_soft)
                    .child(div().w(px(2.0)).h(px(20.0)).rounded_full().bg(t.accent))
            } else {
                row.hover(|s| s.bg(t.glass_hover)).child(div().w(px(2.0)))
            };

            let label_el = if !item.match_indices.is_empty() {
                let chars: Vec<char> = item.label.chars().collect();
                let mut label_row = div().flex().items_center().text_sm();
                let mut i = 0;
                while i < chars.len() {
                    let is_match = item.match_indices.contains(&i);
                    let start = i;
                    while i < chars.len() && item.match_indices.contains(&i) == is_match {
                        i += 1;
                    }
                    let segment: String = chars[start..i].iter().collect();
                    let color = if is_match {
                        t.accent
                    } else if is_sel {
                        t.text_primary
                    } else {
                        t.text_secondary
                    };
                    label_row = label_row.child(
                        div()
                            .text_color(color)
                            .font_weight(if is_match {
                                FontWeight::BOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .child(segment),
                    );
                }
                label_row
            } else {
                div()
                    .text_sm()
                    .text_color(if is_sel {
                        t.text_primary
                    } else {
                        t.text_secondary
                    })
                    .child(item.label.clone())
            };

            let content = div().flex_1().child(label_el).child(
                div()
                    .text_xs()
                    .text_color(t.text_muted)
                    .child(item.hint.clone()),
            );

            let mut right_section = div().flex().items_center().gap_2();

            if let Some(shortcut) = item.shortcut {
                right_section = right_section.child(
                    div()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded_md()
                        .border_1()
                        .border_color(t.glass_border)
                        .bg(t.glass_tint)
                        .text_xs()
                        .text_color(t.text_ghost)
                        .child(shortcut),
                );
            }

            row = row
                .child(
                    div()
                        .w(px(22.0))
                        .flex()
                        .justify_center()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(if is_sel { t.accent } else { t.text_muted })
                        .child(item.glyph),
                )
                .child(content)
                .child(right_section)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.execute_palette_action(action.clone(), window, cx);
                    }),
                );

            item_list = item_list.child(row);
        }

        let scrollable_list = div()
            .id("palette-scroll")
            .max_h(px(420.0))
            .overflow_y_scroll()
            .child(item_list);

        let panel = div()
            .w(px(560.0))
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .rounded_xl()
            .shadow_lg()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(t.glass_border)
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.accent)
                            .font_weight(FontWeight::BOLD)
                            .child("/"),
                    )
                    .child(div().flex_1().flex().items_center().child(
                        if self.palette_query.is_empty() {
                            div()
                                .text_sm()
                                .text_color(t.text_muted)
                                .child("Search commands\u{2026}")
                        } else {
                            div()
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(t.text_primary)
                                        .child(self.palette_query.clone()),
                                )
                                .child(div().w(px(2.0)).h(px(16.0)).ml(px(1.0)).bg(t.accent))
                        },
                    ))
                    .child(
                        div()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded_md()
                            .border_1()
                            .border_color(t.glass_border)
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child("esc"),
                    ),
            )
            .child(scrollable_list)
            .child(
                div()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(t.glass_border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child("↑↓ navigate")
                            .child("↵ select")
                            .child("esc close"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child(format!("{} commands", items.len())),
                    ),
            );

        div()
            .absolute()
            .size_full()
            .bg(t.scrim)
            .flex()
            .flex_col()
            .items_center()
            .pt(px(100.0))
            .child(panel)
    }

    fn render_unlock_overlay(&self) -> impl IntoElement {
        let t = self.theme();
        let create_mode = matches!(self.unlock_form.mode, UnlockMode::Create);
        let title = if create_mode {
            "Create Vault"
        } else {
            "Unlock Vault"
        };

        let passphrase_card = unlock_field_card(
            "Passphrase",
            masked_value(&self.unlock_form.passphrase),
            self.unlock_form.selected_field == 0,
            &t,
        );

        let mut panel = div()
            .w(px(560.0))
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .rounded_xl()
            .shadow_lg()
            .p_5()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text_primary)
                            .child(title),
                    )
                    .child(
                        div().text_sm().text_color(t.text_muted).child(
                            self.unlock_form
                                .message
                                .clone()
                                .unwrap_or_else(|| "Vault status unknown.".into()),
                        ),
                    ),
            )
            .child(passphrase_card);

        if create_mode {
            panel = panel.child(unlock_field_card(
                "Confirm Passphrase",
                masked_value(&self.unlock_form.confirm_passphrase),
                self.unlock_form.selected_field == 1,
                &t,
            ));
        }

        panel = panel.child(
            div()
                .pt_2()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text_ghost)
                        .child("tab move  enter submit"),
                )
                .child(
                    div()
                        .px_3()
                        .py(px(6.0))
                        .rounded_md()
                        .bg(t.accent_glow)
                        .text_xs()
                        .text_color(t.text_primary)
                        .child(if create_mode {
                            "create vault"
                        } else {
                            "unlock vault"
                        }),
                ),
        );

        div()
            .absolute()
            .size_full()
            .bg(t.scrim)
            .flex()
            .items_center()
            .justify_center()
            .child(panel)
    }

    fn render_host_editor_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        let Some(editor) = self.host_editor.as_ref() else {
            return div();
        };

        let title = if editor.host_id.is_some() {
            "Edit Saved Host"
        } else {
            "Add Saved Host"
        };

        let text_fields: [(HostField, String); 5] = [
            (HostField::Label, editor.label.clone()),
            (HostField::Hostname, editor.hostname.clone()),
            (HostField::Username, editor.username.clone()),
            (HostField::Port, editor.port.clone()),
            (HostField::Notes, editor.notes.clone()),
        ];

        let mut panel = div()
            .w(px(620.0))
            .max_h(px(680.0))
            .overflow_hidden()
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .rounded_xl()
            .shadow_lg()
            .p_5()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_muted)
                            .child(editor.message.clone().unwrap_or_default()),
                    ),
            );

        for (idx, (field, value)) in text_fields.into_iter().enumerate() {
            panel = panel.child(editor_field_card(
                field.title(),
                value,
                idx == editor.selected_field,
                &t,
            ));
        }

        panel = panel.child(self.render_host_auth_picker(cx));

        panel = panel.child(
            div()
                .pt_2()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text_ghost)
                        .child("tab move  esc cancel  enter/space toggle auth  \u{2318}S save"),
                )
                .child(
                    div()
                        .px_3()
                        .py(px(6.0))
                        .rounded_md()
                        .bg(t.accent_glow)
                        .text_xs()
                        .text_color(t.text_primary)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.accent))
                        .child("save encrypted host")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.save_host_editor(cx);
                            }),
                        ),
                ),
        );

        div()
            .absolute()
            .size_full()
            .bg(t.scrim)
            .flex()
            .items_center()
            .justify_center()
            .child(panel)
    }

    fn render_host_auth_picker(&self, _cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let Some(editor) = self.host_editor.as_ref() else {
            return div();
        };
        let is_auth_field = editor.field() == HostField::Auth;

        let mut section = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(if is_auth_field {
                t.accent
            } else {
                t.glass_border
            })
            .bg(t.glass_tint);

        section = section.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(if is_auth_field {
                            t.accent
                        } else {
                            t.text_muted
                        })
                        .child("Authentication"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text_ghost)
                        .child(if is_auth_field {
                            "enter/space to toggle"
                        } else {
                            "tab to this section"
                        }),
                ),
        );

        if !editor.auth_items.is_empty() {
            let mut selected_list = div().flex().flex_col().gap(px(2.0));
            for (i, auth) in editor.auth_items.iter().enumerate() {
                let label = match auth {
                    HostAuthRef::Password { credential_id } => {
                        let name = self
                            .cached_credentials
                            .iter()
                            .find(|c| c.id == *credential_id)
                            .map(|c| c.label.as_str())
                            .unwrap_or("unknown");
                        format!("{}. password: {}", i + 1, name)
                    }
                    HostAuthRef::PrivateKey { key_id, .. } => {
                        let name = self
                            .cached_keys
                            .iter()
                            .find(|k| k.id == *key_id)
                            .map(|k| k.label.as_str())
                            .unwrap_or("unknown");
                        format!("{}. key: {}", i + 1, name)
                    }
                };
                selected_list = selected_list.child(
                    div()
                        .text_xs()
                        .font_family(SIDEBAR_FONT_MONO)
                        .text_color(t.accent)
                        .child(label),
                );
            }
            section = section.child(selected_list);

            section = section.child(div().h(px(1.0)).bg(t.glass_border));
        }

        let mut all_items: Vec<(String, String, bool)> = Vec::new();

        for cred in &self.cached_credentials {
            let is_selected = editor
                .auth_items
                .iter()
                .any(|a| matches!(a, HostAuthRef::Password { credential_id } if *credential_id == cred.id));
            let hint = cred.username_hint.as_deref().unwrap_or("");
            let label = if hint.is_empty() {
                format!("password: {}", cred.label)
            } else {
                format!("password: {} ({})", cred.label, hint)
            };
            all_items.push((format!("cred:{}", cred.id), label, is_selected));
        }

        for key in &self.cached_keys {
            let is_selected = editor
                .auth_items
                .iter()
                .any(|a| matches!(a, HostAuthRef::PrivateKey { key_id, .. } if *key_id == key.id));
            let algo = match &key.algorithm {
                PrivateKeyAlgorithm::Ed25519 => "ed25519",
                PrivateKeyAlgorithm::Rsa { .. } => "rsa",
            };
            let label = format!("key: {} [{}]", key.label, algo);
            all_items.push((format!("key:{}", key.id), label, is_selected));
        }

        if all_items.is_empty() {
            section = section.child(
                div()
                    .py_2()
                    .text_xs()
                    .text_color(t.text_ghost)
                    .child("No credentials or keys in vault. Add some first."),
            );
        } else {
            let mut rows = div().flex().flex_col();
            for (idx, (_item_id, label, selected)) in all_items.iter().enumerate() {
                let is_cursor = is_auth_field && idx == editor.auth_cursor;
                let glyph = if *selected { "\u{25c9}" } else { "\u{25cb}" };
                rows = rows.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(if is_cursor {
                            t.accent_glow
                        } else {
                            gpui::transparent_black()
                        })
                        .child(
                            div()
                                .text_xs()
                                .text_color(if *selected { t.accent } else { t.text_ghost })
                                .child(glyph),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family(SIDEBAR_FONT_MONO)
                                .text_color(if *selected {
                                    t.text_primary
                                } else {
                                    t.text_secondary
                                })
                                .child(label.clone()),
                        ),
                );
            }
            section = section.child(rows);
        }

        section
    }

    fn render_credential_editor_overlay(&self) -> impl IntoElement {
        let t = self.theme();
        let Some(editor) = self.credential_editor.as_ref() else {
            return div();
        };

        let title = if editor.credential_id.is_some() {
            "Edit Credential"
        } else {
            "Add Credential"
        };

        let fields = [
            (CredentialField::Label, editor.label.clone(), false),
            (CredentialField::UsernameHint, editor.username_hint.clone(), false),
            (CredentialField::Secret, editor.secret.clone(), true),
        ];

        let mut panel = div()
            .w(px(520.0))
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .rounded_xl()
            .shadow_lg()
            .p_5()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_muted)
                            .child(editor.message.clone().unwrap_or_default()),
                    ),
            );

        for (idx, (field, value, is_secret)) in fields.into_iter().enumerate() {
            let is_selected = idx == editor.selected_field;
            let display_value = if is_secret && !is_selected {
                "\u{2022}".repeat(value.len().min(20))
            } else {
                value
            };
            panel = panel.child(editor_field_card(
                field.title(),
                display_value,
                is_selected,
                &t,
            ));
        }

        panel = panel.child(
            div()
                .pt_2()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text_ghost)
                        .child("tab move  esc cancel  enter on password saves"),
                )
                .child(
                    div()
                        .px_3()
                        .py(px(6.0))
                        .rounded_md()
                        .bg(t.accent_glow)
                        .text_xs()
                        .text_color(t.text_primary)
                        .child("save credential"),
                ),
        );

        div()
            .absolute()
            .size_full()
            .bg(t.scrim)
            .flex()
            .items_center()
            .justify_center()
            .child(panel)
    }

    fn render_perf_overlay(&self) -> impl IntoElement {
        let t = self.theme();
        let stats = self.perf_overlay.frame_stats;
        let session_perf = self.perf_overlay.active_session_perf_snapshot.as_ref();
        let terminal = session_perf.map(|snapshot| &snapshot.terminal);
        let mode_label = perf_mode_label(self.perf_overlay.mode);
        let compact_rows = compact_perf_strings(&self.perf_overlay);
        let expanded_rows = expanded_perf_strings(
            &self.perf_overlay,
            self.active_session_id,
            self.palette_open,
            self.terminal_surface.metrics,
        );

        let mut panel = div()
            .absolute()
            .top(px(16.0))
            .right(px(16.0))
            .w(px(
                if matches!(self.perf_overlay.mode, UiPerfMode::Expanded) {
                    260.0
                } else {
                    220.0
                },
            ))
            .p_3()
            .rounded_lg()
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .font_family("Menlo")
            .text_xs()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_color(t.accent).child("perf"))
                    .child(div().text_color(t.text_muted).child(mode_label)),
            );

        for (label, value) in compact_rows {
            let color = match label {
                "fps" => perf_status_color(stats.fps_1s >= 30.0, &t),
                "frame" => {
                    frame_budget_color(stats.frame_time_p95_ms.max(stats.frame_time_last_ms), &t)
                }
                "snapshot" => perf_status_color(terminal.is_some(), &t),
                _ => t.text_secondary,
            };
            panel = panel.child(perf_row(label, value, color, &t));
        }

        if matches!(self.perf_overlay.mode, UiPerfMode::Expanded) {
            for (label, value) in expanded_rows {
                let color = match label {
                    "present/ui" => {
                        let ui_refreshes = self.perf_overlay.ui_refreshes_last_second();
                        let ok = ui_refreshes == 0
                            || self.perf_overlay.frames_presented_last_second() <= ui_refreshes;
                        perf_status_color(ok, &t)
                    }
                    "dirty" => perf_status_color(self.perf_overlay.active_session_dirty(), &t),
                    "palette" => perf_status_color(self.palette_open, &t),
                    _ => t.text_secondary,
                };
                panel = panel.child(perf_row(label, value, color, &t));
            }
        }

        panel
    }
}

fn perf_row(
    label: &'static str,
    value: String,
    value_color: gpui::Hsla,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(div().text_color(theme.text_muted).child(label))
        .child(div().text_color(value_color).child(value))
}

fn unlock_field_card(
    label: &'static str,
    value: String,
    selected: bool,
    theme: &Theme,
) -> impl IntoElement {
    let mut card = div()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.glass_border)
        .bg(theme.glass_tint)
        .flex()
        .flex_col()
        .gap_1();
    if selected {
        card = card.border_color(theme.accent_glow).bg(theme.glass_active);
    }

    card.child(div().text_xs().text_color(theme.text_muted).child(label))
        .child(
            div()
                .text_sm()
                .text_color(theme.text_primary)
                .child(if value.is_empty() { " ".into() } else { value }),
        )
}

fn editor_field_card(
    label: &'static str,
    value: String,
    selected: bool,
    theme: &Theme,
) -> impl IntoElement {
    let mut card = div()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.glass_border)
        .bg(theme.glass_tint)
        .flex()
        .flex_col()
        .gap_1();
    if selected {
        card = card.border_color(theme.accent_glow).bg(theme.glass_active);
    }

    card.child(div().text_xs().text_color(theme.text_muted).child(label))
        .child(
            div()
                .text_sm()
                .text_color(theme.text_primary)
                .child(if value.is_empty() { " ".into() } else { value }),
        )
}

fn masked_value(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        "•".repeat(value.chars().count())
    }
}

fn compute_terminal_geometry(
    viewport_size: gpui::Size<Pixels>,
    metrics: TerminalMetrics,
) -> Option<TerminalGeometry> {
    let pane_width_px = (f32::from(viewport_size.width) - SIDEBAR_WIDTH).max(0.0);
    let pane_height_px = f32::from(viewport_size.height).max(0.0);
    let usable_width_px = (pane_width_px - (TERMINAL_PANE_PADDING_PX * 2.0)).max(1.0);
    let usable_height_px = (pane_height_px - (TERMINAL_PANE_PADDING_PX * 2.0)).max(1.0);
    let cols = (usable_width_px / metrics.cell_width_px).floor().max(1.0) as u16;
    let rows = (usable_height_px / metrics.cell_height_px).floor().max(1.0) as u16;

    TerminalGeometry::new(
        cols,
        rows,
        usable_width_px.floor() as u16,
        usable_height_px.floor() as u16,
        metrics.cell_width_px.ceil() as u16,
        metrics.line_height_px.ceil() as u16,
    )
    .ok()
}

fn is_tui_artifact(line: &str) -> bool {
    let non_ws: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    if non_ws.is_empty() {
        return false;
    }
    let special = non_ws
        .iter()
        .filter(|c| {
            matches!(
                **c,
                '\u{2500}'..='\u{257F}'    // Box Drawing
                | '\u{2580}'..='\u{259F}'  // Block Elements
                | '\u{2800}'..='\u{28FF}'  // Braille Patterns
            )
        })
        .count();
    (special as f64 / non_ws.len() as f64) > 0.5
}

fn session_preview_text(rows: &[TerminalRow]) -> Option<String> {
    rows.iter()
        .rev()
        .map(TerminalRow::plain_text)
        .find(|line| !line.trim().is_empty() && !is_tui_artifact(line))
}

fn build_terminal_surface_rows(
    rows: &[TerminalRow],
    geometry: TerminalGeometry,
    metrics: TerminalMetrics,
    theme_id: ThemeId,
    theme: &Theme,
    shape_cache: &mut ShapeCache,
    window: &mut Window,
) -> (Vec<TerminalPaintRow>, TerminalRendererMetrics) {
    let visible_cols = geometry.size.cols as usize;
    let visible_rows = geometry.size.rows as usize;
    let start = rows.len().saturating_sub(visible_rows);
    let visible = &rows[start..];
    let mut renderer_metrics = TerminalRendererMetrics {
        visible_rows: visible.len(),
        visible_cells: visible.len() * visible_cols,
        ..Default::default()
    };
    let mut paint_rows = Vec::with_capacity(visible.len());

    for (row_index, row) in visible.iter().enumerate() {
        paint_rows.push(build_terminal_paint_row(
            row,
            row_index,
            visible_cols,
            metrics,
            theme_id,
            theme,
            shape_cache,
            window,
            &mut renderer_metrics,
        ));
    }

    (paint_rows, renderer_metrics)
}

fn build_terminal_paint_row(
    row: &TerminalRow,
    row_index: usize,
    visible_cols: usize,
    metrics: TerminalMetrics,
    theme_id: ThemeId,
    theme: &Theme,
    shape_cache: &mut ShapeCache,
    window: &mut Window,
    renderer_metrics: &mut TerminalRendererMetrics,
) -> TerminalPaintRow {
    let fragment_plans = terminal_fragment_plans(row, visible_cols, theme, renderer_metrics);
    let backgrounds = terminal_background_quads(row, visible_cols, metrics, theme);
    let underlines = terminal_underline_quads(row, visible_cols, metrics, theme);
    let mut fragments = Vec::with_capacity(fragment_plans.len());

    for plan in fragment_plans {
        if plan.text.is_empty() {
            continue;
        }
        let line = shape_terminal_fragment(
            &plan,
            metrics,
            theme_id,
            theme,
            shape_cache,
            window,
            renderer_metrics,
        );
        fragments.push(TerminalPaintFragment {
            x: px(plan.start_col as f32 * metrics.cell_width_px),
            line,
        });
    }

    renderer_metrics.fragments += fragments.len();
    renderer_metrics.background_quads += backgrounds.len() + underlines.len();

    TerminalPaintRow {
        y: px(row_index as f32 * metrics.line_height_px),
        backgrounds,
        underlines,
        fragments,
    }
}

fn terminal_fragment_plans(
    row: &TerminalRow,
    visible_cols: usize,
    theme: &Theme,
    renderer_metrics: &mut TerminalRendererMetrics,
) -> Vec<TerminalFragmentPlan> {
    let mut plans = Vec::new();
    let mut current_col = 0;
    let mut current: Option<TerminalFragmentPlan> = None;

    for cell in &row.cells {
        if current_col >= visible_cols {
            break;
        }

        let cell_width = usize::from(cell.width.max(1));
        if current_col + cell_width > visible_cols {
            break;
        }

        let glyph_policy = terminal_glyph_policy(cell);
        if matches!(glyph_policy, TerminalGlyphPolicy::PerCellSpecial) {
            renderer_metrics.special_glyph_cells += cell_width;
        }
        if matches!(glyph_policy, TerminalGlyphPolicy::WideCell) {
            renderer_metrics.wide_cells += 1;
        }

        let is_blank = cell.text.chars().all(|ch| ch == ' ');
        if is_blank {
            if let Some(plan) = current.take() {
                plans.push(plan);
            }
            current_col += cell_width;
            continue;
        }

        let should_merge = current.as_ref().is_some_and(|plan| {
            plan.style == cell.style
                && plan.glyph_policy == glyph_policy
                && plan.start_col + plan.cell_count == current_col
                && glyph_policy == TerminalGlyphPolicy::GroupableAscii
        });

        if should_merge {
            let plan = current.as_mut().expect("current fragment exists");
            plan.text.push_str(&cell.text);
            plan.cell_count += cell_width;
        } else {
            if let Some(plan) = current.take() {
                plans.push(plan);
            }
            current = Some(TerminalFragmentPlan {
                text: cell.text.clone(),
                style: cell.style,
                glyph_policy,
                start_col: current_col,
                cell_count: cell_width,
            });
        }

        current_col += cell_width;
    }

    if let Some(plan) = current.take() {
        plans.push(plan);
    }

    let _ = theme;
    plans
}

fn terminal_background_quads(
    row: &TerminalRow,
    visible_cols: usize,
    metrics: TerminalMetrics,
    theme: &Theme,
) -> Vec<TerminalPaintQuad> {
    let mut quads = Vec::new();
    let mut current_col = 0;
    let mut run_start = 0;
    let mut run_width = 0;
    let mut run_color: Option<gpui::Hsla> = None;

    for cell in &row.cells {
        if current_col >= visible_cols {
            break;
        }

        let cell_width = usize::from(cell.width.max(1));
        if current_col + cell_width > visible_cols {
            break;
        }

        let cell_color = cell.style.background.map(terminal_color_to_hsla);
        if cell_color == run_color {
            run_width += cell_width;
        } else {
            if let Some(color) = run_color {
                quads.push(TerminalPaintQuad {
                    x: px(run_start as f32 * metrics.cell_width_px),
                    width: px(run_width as f32 * metrics.cell_width_px),
                    color,
                });
            }
            run_start = current_col;
            run_width = cell_width;
            run_color = cell_color;
        }

        current_col += cell_width;
    }

    if let Some(color) = run_color {
        quads.push(TerminalPaintQuad {
            x: px(run_start as f32 * metrics.cell_width_px),
            width: px(run_width as f32 * metrics.cell_width_px),
            color,
        });
    }

    let _ = theme;
    quads
}

fn terminal_underline_quads(
    row: &TerminalRow,
    visible_cols: usize,
    metrics: TerminalMetrics,
    theme: &Theme,
) -> Vec<TerminalPaintQuad> {
    let mut quads = Vec::new();
    let mut current_col = 0;
    let mut run_start = 0;
    let mut run_width = 0;
    let mut run_color: Option<gpui::Hsla> = None;

    for cell in &row.cells {
        if current_col >= visible_cols {
            break;
        }

        let cell_width = usize::from(cell.width.max(1));
        if current_col + cell_width > visible_cols {
            break;
        }

        let cell_color = cell
            .style
            .underline
            .then(|| resolve_terminal_foreground(cell.style, theme));
        if cell_color == run_color {
            run_width += cell_width;
        } else {
            if let Some(color) = run_color {
                quads.push(TerminalPaintQuad {
                    x: px(run_start as f32 * metrics.cell_width_px),
                    width: px(run_width as f32 * metrics.cell_width_px),
                    color,
                });
            }
            run_start = current_col;
            run_width = cell_width;
            run_color = cell_color;
        }

        current_col += cell_width;
    }

    if let Some(color) = run_color {
        quads.push(TerminalPaintQuad {
            x: px(run_start as f32 * metrics.cell_width_px),
            width: px(run_width as f32 * metrics.cell_width_px),
            color,
        });
    }

    quads
}

fn shape_terminal_fragment(
    plan: &TerminalFragmentPlan,
    metrics: TerminalMetrics,
    theme_id: ThemeId,
    theme: &Theme,
    shape_cache: &mut ShapeCache,
    window: &mut Window,
    renderer_metrics: &mut TerminalRendererMetrics,
) -> ShapedLine {
    let color = resolve_terminal_foreground(plan.style, theme);
    let key = ShapeCacheKey {
        text: plan.text.clone(),
        font_size_bits: metrics.font_size_px.to_bits(),
        bold: plan.style.bold,
        italic: plan.style.italic,
        color: hsla_key(color),
    };

    if let Some(entry) = shape_cache.entries.get_mut(&key) {
        shape_cache.generation = shape_cache.generation.saturating_add(1);
        entry.last_used = shape_cache.generation;
        renderer_metrics.shape_hits += 1;
        return entry.line.clone();
    }

    renderer_metrics.shape_misses += 1;
    let mut terminal_font = font(TERMINAL_FONT_FAMILY);
    if plan.style.bold {
        terminal_font = terminal_font.bold();
    }
    if plan.style.italic {
        terminal_font = terminal_font.italic();
    }

    let line = window.text_system().shape_line(
        SharedString::from(plan.text.clone()),
        px(metrics.font_size_px),
        &[TextRun {
            len: plan.text.len(),
            font: terminal_font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );

    shape_cache.generation = shape_cache.generation.saturating_add(1);
    shape_cache.entries.insert(
        key,
        CachedShapeLine {
            line: line.clone(),
            last_used: shape_cache.generation,
        },
    );
    evict_shape_cache(shape_cache, 2_048);
    let _ = theme_id;
    line
}

fn evict_shape_cache(shape_cache: &mut ShapeCache, limit: usize) {
    if shape_cache.entries.len() <= limit {
        return;
    }

    if let Some((oldest_key, _)) = shape_cache
        .entries
        .iter()
        .min_by_key(|(_, entry)| entry.last_used)
        .map(|(key, entry)| (key.clone(), entry.last_used))
    {
        shape_cache.entries.remove(&oldest_key);
    }
}

fn hsla_key(color: gpui::Hsla) -> HslaKey {
    HslaKey {
        h: color.h.to_bits(),
        s: color.s.to_bits(),
        l: color.l.to_bits(),
        a: color.a.to_bits(),
    }
}

fn paint_terminal_surface(
    bounds: Bounds<Pixels>,
    surface: PreparedTerminalSurface,
    window: &mut Window,
    cx: &mut App,
) {
    let line_height = px(surface.line_height_px);

    for row in surface.rows {
        let row_origin = point(bounds.origin.x, bounds.origin.y + row.y);

        for background in row.backgrounds {
            window.paint_quad(fill(
                Bounds::new(
                    point(row_origin.x + background.x, row_origin.y),
                    size(background.width, line_height),
                ),
                background.color,
            ));
        }

        for fragment in row.fragments {
            let _ = fragment.line.paint(
                point(row_origin.x + fragment.x, row_origin.y),
                line_height,
                window,
                cx,
            );
        }

        for underline in row.underlines {
            window.paint_quad(fill(
                Bounds::new(
                    point(
                        row_origin.x + underline.x,
                        row_origin.y + line_height - px(1.0),
                    ),
                    size(underline.width, px(1.0)),
                ),
                underline.color,
            ));
        }
    }
}

fn terminal_glyph_policy(cell: &TerminalCell) -> TerminalGlyphPolicy {
    if cell.width > 1 {
        return TerminalGlyphPolicy::WideCell;
    }

    let mut chars = cell.text.chars();
    let Some(first) = chars.next() else {
        return TerminalGlyphPolicy::GroupableAscii;
    };

    if first.is_ascii() && !chars.any(|ch| !ch.is_ascii()) && !first.is_ascii_control() {
        return TerminalGlyphPolicy::GroupableAscii;
    }

    let _ = is_terminal_special_glyph(first);
    TerminalGlyphPolicy::PerCellSpecial
}

fn is_terminal_special_glyph(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2500..=0x257f | 0x2580..=0x259f | 0x2800..=0x28ff | 0xe000..=0xf8ff
    )
}

fn resolve_terminal_foreground(style: TerminalCellStyle, theme: &Theme) -> gpui::Hsla {
    let base = style
        .foreground
        .map(terminal_color_to_hsla)
        .unwrap_or(theme.text_primary);

    if !style.faint {
        return base;
    }

    soften_faint_terminal_foreground(base, theme)
}

fn soften_faint_terminal_foreground(base: gpui::Hsla, theme: &Theme) -> gpui::Hsla {
    let subdued = base.blend(theme.bg_void.alpha(0.62)).alpha(0.78);
    let cap = if lightness_distance(theme.text_ghost, theme.bg_void) >= 0.10 {
        theme.text_ghost
    } else {
        theme.text_muted
    };
    let subdued = if lightness_distance(subdued, theme.bg_void) < 0.10 {
        cap
    } else {
        subdued
    };

    if lightness_distance(subdued, theme.bg_void) > lightness_distance(cap, theme.bg_void) {
        subdued.blend(cap.alpha(0.55))
    } else {
        subdued
    }
}

fn lightness_distance(left: gpui::Hsla, right: gpui::Hsla) -> f32 {
    (left.l - right.l).abs()
}

fn terminal_color_to_hsla(color: TerminalColor) -> gpui::Hsla {
    gpui::rgb((u32::from(color.r) << 16) | (u32::from(color.g) << 8) | u32::from(color.b)).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;
    use seance_terminal::{TerminalCell, TerminalRow};

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

    #[test]
    fn compute_geometry_uses_viewport_minus_sidebar_and_padding() {
        let geometry = compute_terminal_geometry(
            size(px(1280.0), px(820.0)),
            TerminalMetrics {
                cell_width_px: 8.0,
                cell_height_px: 19.0,
                line_height_px: 19.0,
                font_size_px: 13.0,
            },
        )
        .expect("geometry");

        assert_eq!(geometry.pixel_size.width_px, 976);
        assert_eq!(geometry.pixel_size.height_px, 788);
        assert_eq!(geometry.size.cols, 122);
        assert_eq!(geometry.size.rows, 41);
    }

    #[test]
    fn compute_geometry_clamps_small_windows_to_one_by_one() {
        let geometry = compute_terminal_geometry(
            size(px(10.0), px(10.0)),
            TerminalMetrics {
                cell_width_px: 20.0,
                cell_height_px: 40.0,
                line_height_px: 40.0,
                font_size_px: 13.0,
            },
        )
        .expect("geometry");

        assert_eq!(geometry.size.cols, 1);
        assert_eq!(geometry.size.rows, 1);
    }

    #[test]
    fn row_plans_preserve_visible_column_count() {
        let row = TerminalRow {
            cells: vec![
                TerminalCell {
                    text: "a".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                },
                TerminalCell {
                    text: "bc".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                },
                TerminalCell {
                    text: "界".into(),
                    style: TerminalCellStyle::default(),
                    width: 2,
                },
            ],
        };

        let mut metrics = TerminalRendererMetrics::default();
        let segments =
            terminal_fragment_plans(&row, 6, &ThemeId::ObsidianSmoke.theme(), &mut metrics);

        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.cell_count)
                .sum::<usize>(),
            4
        );
    }

    #[test]
    fn special_glyphs_render_per_cell() {
        let box_cell = TerminalCell {
            text: "┌".into(),
            style: TerminalCellStyle::default(),
            width: 1,
        };
        let braille_cell = TerminalCell {
            text: "⣶".into(),
            style: TerminalCellStyle::default(),
            width: 1,
        };
        let private_use_cell = TerminalCell {
            text: "\u{e0b0}".into(),
            style: TerminalCellStyle::default(),
            width: 1,
        };
        let ascii_cell = TerminalCell {
            text: "A".into(),
            style: TerminalCellStyle::default(),
            width: 1,
        };

        assert_eq!(
            terminal_glyph_policy(&box_cell),
            TerminalGlyphPolicy::PerCellSpecial
        );
        assert_eq!(
            terminal_glyph_policy(&braille_cell),
            TerminalGlyphPolicy::PerCellSpecial
        );
        assert_eq!(
            terminal_glyph_policy(&private_use_cell),
            TerminalGlyphPolicy::PerCellSpecial
        );
        assert_eq!(
            terminal_glyph_policy(&ascii_cell),
            TerminalGlyphPolicy::GroupableAscii
        );
    }

    #[test]
    fn preview_text_uses_last_non_empty_row() {
        let rows = vec![
            TerminalRow::default(),
            TerminalRow {
                cells: vec![TerminalCell {
                    text: "prompt$".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                }],
            },
        ];

        assert_eq!(session_preview_text(&rows).as_deref(), Some("prompt$"));
    }

    #[test]
    fn tui_artifact_detects_box_drawing() {
        assert!(is_tui_artifact("┌──────────┐"));
        assert!(is_tui_artifact("│          │"));
        assert!(is_tui_artifact("└──────────┘"));
        assert!(is_tui_artifact("╰───────────────"));
    }

    #[test]
    fn tui_artifact_detects_braille() {
        assert!(is_tui_artifact("⣀⣄⣤⣶⣿⣿⣶⣤⣄⣀"));
    }

    #[test]
    fn tui_artifact_detects_block_elements() {
        assert!(is_tui_artifact("▄▄▄▄▄▄▄▄▄▄"));
        assert!(is_tui_artifact("██████████"));
    }

    #[test]
    fn tui_artifact_allows_normal_text() {
        assert!(!is_tui_artifact("prompt$"));
        assert!(!is_tui_artifact("~/code $ ls -la"));
        assert!(!is_tui_artifact("hello world"));
    }

    #[test]
    fn tui_artifact_allows_mixed_below_threshold() {
        assert!(!is_tui_artifact("status │ ok"));
    }

    #[test]
    fn tui_artifact_empty_and_whitespace() {
        assert!(!is_tui_artifact(""));
        assert!(!is_tui_artifact("   "));
    }

    #[test]
    fn preview_text_skips_tui_artifact_rows() {
        let rows = vec![
            TerminalRow {
                cells: vec![TerminalCell {
                    text: "~/code $".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                }],
            },
            TerminalRow {
                cells: vec![TerminalCell {
                    text: "╰──────────────".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                }],
            },
        ];

        assert_eq!(session_preview_text(&rows).as_deref(), Some("~/code $"));
    }

    #[test]
    fn preview_text_returns_none_when_all_rows_are_artifacts() {
        let rows = vec![
            TerminalRow {
                cells: vec![TerminalCell {
                    text: "┌──────┐".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                }],
            },
            TerminalRow {
                cells: vec![TerminalCell {
                    text: "└──────┘".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                }],
            },
        ];

        assert_eq!(session_preview_text(&rows), None);
    }

    #[test]
    fn clips_wide_cells_at_visible_edge() {
        let row = TerminalRow {
            cells: vec![
                TerminalCell {
                    text: "A".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                },
                TerminalCell {
                    text: "界".into(),
                    style: TerminalCellStyle::default(),
                    width: 2,
                },
            ],
        };

        let mut metrics = TerminalRendererMetrics::default();
        let segments =
            terminal_fragment_plans(&row, 2, &ThemeId::ObsidianSmoke.theme(), &mut metrics);

        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.cell_count)
                .sum::<usize>(),
            1
        );
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "A");
        assert_eq!(segments[0].cell_count, 1);
    }

    #[test]
    fn background_quads_merge_adjacent_cells() {
        let row = TerminalRow {
            cells: vec![
                TerminalCell {
                    text: "A".into(),
                    style: TerminalCellStyle {
                        background: Some(TerminalColor { r: 1, g: 2, b: 3 }),
                        ..TerminalCellStyle::default()
                    },
                    width: 1,
                },
                TerminalCell {
                    text: "B".into(),
                    style: TerminalCellStyle {
                        background: Some(TerminalColor { r: 1, g: 2, b: 3 }),
                        ..TerminalCellStyle::default()
                    },
                    width: 1,
                },
            ],
        };

        let quads = terminal_background_quads(
            &row,
            4,
            TerminalMetrics {
                cell_width_px: 8.0,
                cell_height_px: 19.0,
                line_height_px: 19.0,
                font_size_px: 13.0,
            },
            &ThemeId::ObsidianSmoke.theme(),
        );

        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].width, px(16.0));
    }

    #[test]
    fn faint_text_is_softened_for_ghost_text_rendering() {
        let theme = ThemeId::Bone.theme();
        let base = gpui::rgb(0x1a1816).into();

        let softened = soften_faint_terminal_foreground(base, &theme);

        assert!(lightness_distance(softened, theme.bg_void) >= 0.10);
        assert!(
            lightness_distance(softened, theme.bg_void)
                <= lightness_distance(theme.text_muted, theme.bg_void)
        );
    }

    #[test]
    fn perf_mode_cycles_through_all_states() {
        assert_eq!(UiPerfMode::Off.next(), UiPerfMode::Compact);
        assert_eq!(UiPerfMode::Compact.next(), UiPerfMode::Expanded);
        assert_eq!(UiPerfMode::Expanded.next(), UiPerfMode::Off);
    }

    #[test]
    fn frame_stats_compute_average_and_percentile() {
        let now = Instant::now();
        let render_samples = VecDeque::from(vec![
            (now + Duration::from_millis(4), Duration::from_millis(4)),
            (now + Duration::from_millis(9), Duration::from_millis(5)),
            (now + Duration::from_millis(16), Duration::from_millis(7)),
        ]);
        let cadence_samples = VecDeque::from(vec![
            (now + Duration::from_millis(10), Duration::from_millis(10)),
            (now + Duration::from_millis(43), Duration::from_millis(33)),
            (now + Duration::from_millis(93), Duration::from_millis(50)),
        ]);
        let timestamps = VecDeque::from(vec![
            now,
            now + Duration::from_millis(100),
            now + Duration::from_millis(200),
        ]);

        let stats = build_frame_stats(
            9,
            &render_samples,
            &cadence_samples,
            &timestamps,
            RedrawReason::TerminalUpdate,
        );

        assert_eq!(stats.frame_count_total, 9);
        assert!((stats.fps_1s - 3.0).abs() < 0.01);
        assert_eq!(stats.frame_time_last_ms, 7.0);
        assert!((stats.frame_time_avg_ms - 5.3333335).abs() < 0.01);
        assert_eq!(stats.frame_time_p95_ms, 7.0);
        assert_eq!(stats.present_interval_last_ms, 50.0);
        assert!((stats.present_interval_avg_ms - 31.0).abs() < 0.01);
        assert_eq!(stats.present_interval_p95_ms, 50.0);
        assert_eq!(stats.redraw_reason, RedrawReason::TerminalUpdate);
    }

    #[test]
    fn normalized_fps_counts_frames_in_window() {
        let now = Instant::now();
        let timestamps = VecDeque::from(vec![
            now,
            now + Duration::from_millis(250),
            now + Duration::from_millis(500),
        ]);

        assert!((normalized_fps_1s(&timestamps) - 3.0).abs() < 0.01);
    }

    #[test]
    fn refresh_and_present_counts_are_tracked_separately() {
        let mut state = PerfOverlayState::new(UiPerfMode::Expanded);
        let now = Instant::now();

        state.mark_terminal_refresh_request(now, RedrawReason::TerminalUpdate, None);
        state.mark_ui_refresh_request(now + Duration::from_millis(10), RedrawReason::UiRefresh);

        assert_eq!(state.ui_refreshes_last_second(), 2);
        assert_eq!(state.terminal_refreshes_last_second(), 1);
        assert_eq!(state.frames_presented_last_second(), 0);

        state.finish_render(
            now + Duration::from_millis(16),
            now + Duration::from_millis(20),
        );

        assert_eq!(state.ui_refreshes_last_second(), 2);
        assert_eq!(state.terminal_refreshes_last_second(), 1);
        assert_eq!(state.frames_presented_last_second(), 1);
    }

    #[test]
    fn redraw_reason_is_consumed_on_present() {
        let mut state = PerfOverlayState::new(UiPerfMode::Expanded);
        let now = Instant::now();

        state.mark_ui_refresh_request(now, RedrawReason::Palette);
        assert_eq!(state.pending_redraw_reason, RedrawReason::Palette);

        state.finish_render(
            now + Duration::from_millis(8),
            now + Duration::from_millis(16),
        );

        assert_eq!(state.frame_stats.redraw_reason, RedrawReason::Palette);
        assert_eq!(state.pending_redraw_reason, RedrawReason::Unknown);
    }

    #[test]
    fn compact_perf_strings_include_primary_metrics() {
        let mut state = PerfOverlayState::new(UiPerfMode::Compact);
        state.frame_stats.fps_1s = 59.0;
        state.frame_stats.frame_time_last_ms = 12.0;

        let rows = compact_perf_strings(&state);
        let labels = rows.into_iter().map(|(label, _)| label).collect::<Vec<_>>();

        assert_eq!(labels, vec!["fps", "frame", "snapshot", "rows", "cells"]);
    }

    #[test]
    fn expanded_perf_strings_include_render_insights() {
        let mut state = PerfOverlayState::new(UiPerfMode::Expanded);
        state.visible_line_count = 32;
        state.pending_redraw_reason = RedrawReason::Palette;
        state.frame_stats.redraw_reason = RedrawReason::Palette;

        let rows = expanded_perf_strings(&state, 7, true, TerminalRendererMetrics::default());
        let labels = rows.into_iter().map(|(label, _)| label).collect::<Vec<_>>();

        assert!(labels.contains(&"ui refresh"));
        assert!(labels.contains(&"terminal refresh"));
        assert!(labels.contains(&"presented"));
        assert!(labels.contains(&"present/ui"));
        assert!(labels.contains(&"cadence"));
        assert!(labels.contains(&"visible"));
        assert!(labels.contains(&"reason"));
        assert!(labels.contains(&"fragments"));
    }

    #[test]
    fn present_intervals_are_trimmed_to_perf_window() {
        let now = Instant::now();
        let mut samples = VecDeque::from(vec![
            (now - Duration::from_secs(2), Duration::from_millis(80_000)),
            (now - Duration::from_millis(800), Duration::from_millis(16)),
            (now - Duration::from_millis(100), Duration::from_millis(20)),
        ]);

        trim_timed_durations(&mut samples, now, PERF_WINDOW);

        assert_eq!(samples.len(), 2);
        assert_eq!(samples.front().unwrap().1, Duration::from_millis(16));
        assert_eq!(samples.back().unwrap().1, Duration::from_millis(20));
    }

    #[test]
    fn terminal_refresh_is_counted_as_ui_refresh() {
        let mut state = PerfOverlayState::new(UiPerfMode::Expanded);
        let now = Instant::now();

        state.mark_terminal_refresh_request(now, RedrawReason::TerminalUpdate, None);

        assert_eq!(state.ui_refreshes_last_second(), 1);
        assert_eq!(state.terminal_refreshes_last_second(), 1);
    }

    #[test]
    fn perf_mode_enable_resets_sampling_window() {
        let mut state = PerfOverlayState::new(UiPerfMode::Off);
        let now = Instant::now();

        state.mark_ui_refresh_request(now, RedrawReason::UiRefresh);
        state.finish_render(
            now + Duration::from_millis(8),
            now + Duration::from_millis(16),
        );
        state.mode = UiPerfMode::Compact;
        state.reset_sampling_window();

        assert!(state.present_timestamps.is_empty());
        assert!(state.present_intervals.is_empty());
        assert!(state.render_cost_samples.is_empty());
        assert!(state.ui_refresh_timestamps.is_empty());
        assert!(state.terminal_refresh_timestamps.is_empty());
        assert!(state.last_present_timestamp.is_none());
        assert_eq!(state.frame_stats.frame_count_total, 0);
    }

    #[test]
    fn frame_stats_ignore_stale_intervals_after_idle_gap() {
        let mut state = PerfOverlayState::new(UiPerfMode::Expanded);
        let now = Instant::now();

        state.last_present_timestamp = Some(now - Duration::from_secs(3));
        state
            .present_intervals
            .push_back((now - Duration::from_secs(3), Duration::from_secs(86)));
        state
            .render_cost_samples
            .push_back((now - Duration::from_secs(3), Duration::from_secs(40)));
        state
            .present_timestamps
            .push_back(now - Duration::from_secs(3));

        state.finish_render(
            now - Duration::from_millis(204),
            now - Duration::from_millis(200),
        );
        state.finish_render(
            now - Duration::from_millis(55),
            now - Duration::from_millis(50),
        );

        assert_eq!(state.frame_stats.frame_time_last_ms, 5.0);
        assert!(state.frame_stats.frame_time_avg_ms < 10.0);
        assert!(state.frame_stats.frame_time_p95_ms < 10.0);
        assert_eq!(state.frame_stats.present_interval_last_ms, 150.0);
        assert!(state.frame_stats.present_interval_avg_ms < 2_900.0);
        assert!(state.frame_stats.present_interval_p95_ms < 3_000.0);
    }

    #[test]
    fn presented_and_ui_refresh_are_comparable() {
        let mut state = PerfOverlayState::new(UiPerfMode::Expanded);
        let now = Instant::now();

        state.mark_ui_refresh_request(now, RedrawReason::UiRefresh);
        state.mark_ui_refresh_request(now + Duration::from_millis(10), RedrawReason::Palette);
        state.finish_render(
            now + Duration::from_millis(16),
            now + Duration::from_millis(20),
        );

        let rows = expanded_perf_strings(&state, 1, false, TerminalRendererMetrics::default());
        let ratio = rows
            .into_iter()
            .find(|(label, _)| *label == "present/ui")
            .map(|(_, value)| value)
            .unwrap();

        assert_eq!(ratio, "1/2");
    }

    #[test]
    fn render_cost_and_cadence_are_computed_separately() {
        let now = Instant::now();
        let render_samples = VecDeque::from(vec![
            (now + Duration::from_millis(4), Duration::from_millis(4)),
            (now + Duration::from_millis(9), Duration::from_millis(5)),
            (now + Duration::from_millis(16), Duration::from_millis(7)),
        ]);
        let cadence_samples = VecDeque::from(vec![
            (now + Duration::from_millis(16), Duration::from_millis(16)),
            (now + Duration::from_millis(49), Duration::from_millis(33)),
            (now + Duration::from_millis(99), Duration::from_millis(50)),
        ]);
        let timestamps = VecDeque::from(vec![
            now,
            now + Duration::from_millis(250),
            now + Duration::from_millis(500),
        ]);

        let stats = build_frame_stats(
            3,
            &render_samples,
            &cadence_samples,
            &timestamps,
            RedrawReason::Input,
        );

        assert_eq!(stats.frame_time_last_ms, 7.0);
        assert!((stats.frame_time_avg_ms - 5.3333335).abs() < 0.01);
        assert_eq!(stats.frame_time_p95_ms, 7.0);
        assert_eq!(stats.present_interval_last_ms, 50.0);
        assert!((stats.present_interval_avg_ms - 33.0).abs() < 0.01);
        assert_eq!(stats.present_interval_p95_ms, 50.0);
    }

    #[test]
    fn stale_render_cost_samples_are_trimmed_to_perf_window() {
        let now = Instant::now();
        let mut samples = VecDeque::from(vec![
            (now - Duration::from_secs(2), Duration::from_secs(2)),
            (now - Duration::from_millis(300), Duration::from_millis(4)),
            (now - Duration::from_millis(100), Duration::from_millis(5)),
        ]);

        trim_timed_durations(&mut samples, now, PERF_WINDOW);

        assert_eq!(samples.len(), 2);
        assert_eq!(average_duration_ms(&samples), 4.5);
        assert!(percentile_duration_ms(&samples, 0.95) < 6.0);
    }

    #[test]
    fn stale_cadence_samples_are_trimmed_to_perf_window() {
        let now = Instant::now();
        let mut samples = VecDeque::from(vec![
            (now - Duration::from_secs(2), Duration::from_secs(30)),
            (now - Duration::from_millis(200), Duration::from_millis(16)),
            (now - Duration::from_millis(50), Duration::from_millis(20)),
        ]);

        trim_timed_durations(&mut samples, now, PERF_WINDOW);

        assert_eq!(samples.len(), 2);
        assert_eq!(samples.front().unwrap().1, Duration::from_millis(16));
        assert_eq!(samples.back().unwrap().1, Duration::from_millis(20));
    }

    #[test]
    fn focus_like_notify_pattern_does_not_inflate_frame_cost() {
        let mut state = PerfOverlayState::new(UiPerfMode::Expanded);
        let now = Instant::now();

        state.finish_render(now, now + Duration::from_millis(4));
        state.finish_render(
            now + Duration::from_millis(80),
            now + Duration::from_millis(85),
        );
        state.finish_render(
            now + Duration::from_millis(200),
            now + Duration::from_millis(206),
        );

        assert!(state.frame_stats.frame_time_last_ms <= 6.0);
        assert!(state.frame_stats.frame_time_avg_ms <= 5.1);
        assert!(state.frame_stats.present_interval_last_ms >= 100.0);
    }
}

impl Focusable for SeanceWorkspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SeanceWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let perf_enabled = self.perf_overlay.mode.is_enabled();
        let render_started_at = perf_enabled.then(Instant::now);

        let t = self.theme();

        let main_content = if self.sftp_browser.is_some() {
            self.render_sftp_panel(window, cx)
        } else if self.vault_panel_open {
            self.render_vault_panel(window, cx)
        } else {
            self.render_terminal_shell(window, cx)
        };

        let mut root = div()
            .size_full()
            .flex()
            .bg(t.bg_deep)
            .text_color(t.text_primary)
            .child(self.render_sidebar(cx))
            .child(main_content);

        if self.palette_open {
            root = root.child(deferred(self.render_palette_overlay(cx)).with_priority(1));
        }
        if self.host_editor.is_some() {
            root = root.child(deferred(self.render_host_editor_overlay(cx)).with_priority(2));
        }
        if self.credential_editor.is_some() {
            root = root.child(deferred(self.render_credential_editor_overlay()).with_priority(5));
        }
        if self.unlock_form.is_visible() {
            root = root.child(deferred(self.render_unlock_overlay()).with_priority(3));
        }
        if self.perf_overlay.mode.is_enabled() {
            root = root.child(deferred(self.render_perf_overlay()).with_priority(4));
        }

        if let Some(started_at) = render_started_at {
            self.perf_overlay.finish_render(started_at, Instant::now());
        }

        root
    }
}

fn sftp_toolbar_pill(
    label: &'static str,
    enabled: bool,
    t: &Theme,
    cx: &mut Context<SeanceWorkspace>,
    handler: impl Fn(
            &mut SeanceWorkspace,
            &gpui::MouseDownEvent,
            &mut Window,
            &mut Context<SeanceWorkspace>,
        ) + 'static,
) -> Div {
    let pill = div()
        .font_family(SIDEBAR_FONT_MONO)
        .text_xs()
        .px_3()
        .py(px(4.0))
        .rounded(px(4.0))
        .border_1();

    if enabled {
        pill.bg(t.glass_tint)
            .border_color(t.glass_border)
            .text_color(t.text_secondary)
            .cursor_pointer()
            .hover(|s| s.bg(t.glass_hover).text_color(t.text_primary))
            .child(label)
            .on_mouse_down(MouseButton::Left, cx.listener(handler))
    } else {
        pill.bg(gpui::hsla(0.0, 0.0, 0.1, 0.3))
            .border_color(gpui::hsla(0.0, 0.0, 0.2, 0.2))
            .text_color(t.text_ghost)
            .child(label)
    }
}

fn sftp_file_glyph(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "rs" | "py" | "js" | "ts" | "c" | "cpp" | "h" | "go" | "rb" | "java" | "swift"
        | "kt" | "zig" | "hs" | "ml" | "ex" | "exs" | "sh" | "bash" | "zsh" | "fish"
        | "lua" | "pl" | "php" => "\u{2022}",
        "toml" | "yaml" | "yml" | "json" | "xml" | "ini" | "cfg" | "conf" | "env" => "\u{2261}",
        "md" | "txt" | "rst" | "org" | "tex" | "log" => "\u{2630}",
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "bmp" | "webp" | "ico" => "\u{25a3}",
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" => "\u{29c9}",
        "lock" | "key" | "pem" | "crt" | "cert" => "\u{26bf}",
        _ => "\u{25cb}",
    }
}

fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let kb = bytes as f64 / 1024.0;
    if kb < 1024.0 {
        return format!("{kb:.1} K");
    }
    let mb = kb / 1024.0;
    if mb < 1024.0 {
        return format!("{mb:.1} M");
    }
    let gb = mb / 1024.0;
    format!("{gb:.1} G")
}

fn format_unix_perms(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    let flags = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    for (bit, ch) in flags {
        if mode & bit != 0 {
            s.push(ch);
        } else {
            s.push('-');
        }
    }
    s
}

fn encode_keystroke(event: &KeyDownEvent) -> Option<Vec<u8>> {
    let key = &event.keystroke.key;
    let key_char = event.keystroke.key_char.as_deref();
    let modifiers = event.keystroke.modifiers;

    if modifiers.platform || modifiers.alt || modifiers.function {
        return None;
    }

    if modifiers.control && key.len() == 1 {
        let byte = key.as_bytes()[0].to_ascii_lowercase();
        if byte.is_ascii_lowercase() {
            return Some(vec![byte - b'a' + 1]);
        }
    }

    match key.as_str() {
        "enter" => Some(vec![b'\r']),
        "tab" => Some(vec![b'\t']),
        "backspace" => Some(vec![0x7f]),
        "escape" => Some(vec![0x1b]),
        "space" => Some(vec![b' ']),
        "up" => Some(b"\x1b[A".to_vec()),
        "down" => Some(b"\x1b[B".to_vec()),
        "right" => Some(b"\x1b[C".to_vec()),
        "left" => Some(b"\x1b[D".to_vec()),
        _ => key_char.map(|text| text.as_bytes().to_vec()),
    }
}

fn now_ui_suffix() -> i64 {
    seance_vault::now_ts()
}
