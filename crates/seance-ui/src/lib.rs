mod backend;
mod palette;
mod theme;

use std::{
    collections::VecDeque,
    env,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use gpui::{
    App, Application, Bounds, Context, FocusHandle, Focusable, FontWeight, KeyDownEvent,
    MouseButton, Pixels, SharedString, StyledText, TextRun, UnderlineStyle, Window,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, deferred, div, font, prelude::*, px,
    size,
};
use seance_terminal::{
    SessionPerfSnapshot, TerminalCellStyle, TerminalColor, TerminalGeometry, TerminalLine,
    TerminalSession,
};
use seance_vault::{
    HostAuthRef, HostSummary, SecretString, VaultHostProfile, VaultPasswordCredential, VaultStore,
};
use tracing::trace;
use zeroize::Zeroizing;

use backend::UiBackend;
use palette::{PaletteAction, build_items};
use theme::{Theme, ThemeId};

const SIDEBAR_WIDTH: f32 = 260.0;
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
    let initial = backend
        .spawn_local_session()
        .expect("failed to create initial local session");

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
                        active_session_id: 1,
                        backend,
                        saved_hosts: initial_saved_hosts,
                        selected_host_id: None,
                        unlock_form: UnlockFormState::new(
                            vault_status.initialized,
                            unlocked,
                            device_unlock_attempted,
                        ),
                        host_editor: None,
                        status_message: None,
                        active_theme: ThemeId::ObsidianSmoke,
                        palette_open: false,
                        palette_query: String::new(),
                        palette_selected: 0,
                        terminal_metrics: None,
                        last_applied_geometry: None,
                        active_terminal_rows: TerminalGeometry::default().size.rows as usize,
                        perf_overlay: PerfOverlayState::new(perf_mode_from_env()),
                    };
                    cx.observe_window_bounds(window, |this: &mut SeanceWorkspace, window, cx| {
                        this.apply_active_terminal_geometry(window);
                        this.perf_overlay.mark_input(RedrawReason::TerminalUpdate);
                        cx.notify();
                    })
                    .detach();
                    ws.apply_active_terminal_geometry(window);
                    ws.schedule_refresh(window, cx, entity.clone());
                    ws.schedule_perf_sampling(window, entity);
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
    active_session_id: u64,
    backend: UiBackend,
    saved_hosts: Vec<HostSummary>,
    selected_host_id: Option<String>,
    unlock_form: UnlockFormState,
    host_editor: Option<HostEditorState>,
    status_message: Option<String>,
    active_theme: ThemeId,
    palette_open: bool,
    palette_query: String,
    palette_selected: usize,
    terminal_metrics: Option<TerminalMetrics>,
    last_applied_geometry: Option<TerminalGeometry>,
    active_terminal_rows: usize,
    perf_overlay: PerfOverlayState,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TerminalMetrics {
    cell_width_px: f32,
    cell_height_px: f32,
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
    AuthOrder,
}

impl HostField {
    const ALL: [Self; 6] = [
        Self::Label,
        Self::Hostname,
        Self::Username,
        Self::Port,
        Self::Notes,
        Self::AuthOrder,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Label => "Label",
            Self::Hostname => "Hostname",
            Self::Username => "Username",
            Self::Port => "Port",
            Self::Notes => "Notes",
            Self::AuthOrder => "Auth Order",
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
    auth_order: String,
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
            auth_order: String::new(),
            selected_field: 0,
            message: Some(
                "Create an encrypted SSH config. Auth order uses password:<id> or key:<id>[:passphrase_id]."
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
            auth_order: format_auth_order(&host.auth_order),
            selected_field: 0,
            message: Some("Edit the encrypted record and press Enter on Notes to save.".into()),
        }
    }

    fn field(&self) -> HostField {
        HostField::ALL[self.selected_field.min(HostField::ALL.len() - 1)]
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
    IdleTick,
    Input,
    TerminalUpdate,
    Palette,
    #[default]
    Unknown,
}

impl RedrawReason {
    fn label(self) -> &'static str {
        match self {
            Self::IdleTick => "idle",
            Self::Input => "input",
            Self::TerminalUpdate => "terminal",
            Self::Palette => "palette",
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
    redraw_reason: RedrawReason,
}

#[derive(Debug)]
struct PerfOverlayState {
    mode: UiPerfMode,
    sampler_running: bool,
    last_frame_timestamp: Option<Instant>,
    frame_timestamps: VecDeque<Instant>,
    frame_durations: VecDeque<Duration>,
    refresh_timestamps: VecDeque<Instant>,
    active_session_perf_snapshot: Option<SessionPerfSnapshot>,
    frame_stats: FrameStats,
    refresh_requests_total: u64,
    idle_refreshes_total: u64,
    visible_line_count: usize,
    pending_redraw_reason: RedrawReason,
}

impl PerfOverlayState {
    fn new(mode: UiPerfMode) -> Self {
        Self {
            mode,
            sampler_running: false,
            last_frame_timestamp: None,
            frame_timestamps: VecDeque::with_capacity(PERF_HISTORY_LIMIT),
            frame_durations: VecDeque::with_capacity(PERF_HISTORY_LIMIT),
            refresh_timestamps: VecDeque::with_capacity(PERF_HISTORY_LIMIT),
            active_session_perf_snapshot: None,
            frame_stats: FrameStats::default(),
            refresh_requests_total: 0,
            idle_refreshes_total: 0,
            visible_line_count: 0,
            pending_redraw_reason: RedrawReason::Unknown,
        }
    }

    fn mark_refresh_request(
        &mut self,
        now: Instant,
        reason: RedrawReason,
        session_perf: Option<SessionPerfSnapshot>,
    ) {
        self.refresh_requests_total = self.refresh_requests_total.saturating_add(1);
        if matches!(reason, RedrawReason::IdleTick) {
            self.idle_refreshes_total = self.idle_refreshes_total.saturating_add(1);
        }
        self.pending_redraw_reason = reason;
        self.active_session_perf_snapshot = session_perf;
        self.refresh_timestamps.push_back(now);
        trim_instants(&mut self.refresh_timestamps, now, PERF_WINDOW);
    }

    fn mark_input(&mut self, reason: RedrawReason) {
        self.pending_redraw_reason = reason;
    }

    fn mark_frame_presented(&mut self, now: Instant) {
        if let Some(previous) = self.last_frame_timestamp.replace(now) {
            push_bounded(
                &mut self.frame_durations,
                now.saturating_duration_since(previous),
            );
        }
        self.frame_timestamps.push_back(now);
        trim_instants(&mut self.frame_timestamps, now, PERF_WINDOW);
        self.frame_stats = build_frame_stats(
            self.frame_stats.frame_count_total.saturating_add(1),
            &self.frame_durations,
            &self.frame_timestamps,
            self.pending_redraw_reason,
        );
        self.pending_redraw_reason = RedrawReason::Unknown;

        trace!(
            frame_count_total = self.frame_stats.frame_count_total,
            fps_1s = self.frame_stats.fps_1s,
            frame_time_last_ms = self.frame_stats.frame_time_last_ms,
            redraw_reason = self.frame_stats.redraw_reason.label(),
            "perf frame sampled"
        );
    }

    fn refreshes_last_second(&self) -> usize {
        self.refresh_timestamps.len()
    }

    fn frames_presented_last_second(&self) -> usize {
        self.frame_timestamps.len()
    }

    fn idle_refresh_percentage(&self) -> f32 {
        if self.refresh_requests_total == 0 {
            return 0.0;
        }

        (self.idle_refreshes_total as f32 / self.refresh_requests_total as f32) * 100.0
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

fn push_bounded(samples: &mut VecDeque<Duration>, sample: Duration) {
    if samples.len() == PERF_HISTORY_LIMIT {
        samples.pop_front();
    }
    samples.push_back(sample);
}

fn build_frame_stats(
    frame_count_total: u64,
    samples: &VecDeque<Duration>,
    frame_timestamps: &VecDeque<Instant>,
    redraw_reason: RedrawReason,
) -> FrameStats {
    let frame_time_last_ms = samples
        .back()
        .map(|duration| duration.as_secs_f32() * 1_000.0)
        .unwrap_or_default();
    let frame_time_avg_ms = if samples.is_empty() {
        0.0
    } else {
        samples.iter().map(Duration::as_secs_f32).sum::<f32>() * 1_000.0 / samples.len() as f32
    };
    let frame_time_p95_ms = percentile_duration_ms(samples, 0.95);

    FrameStats {
        frame_count_total,
        fps_1s: frame_timestamps.len() as f32,
        frame_time_last_ms,
        frame_time_avg_ms,
        frame_time_p95_ms,
        redraw_reason,
    }
}

fn percentile_duration_ms(samples: &VecDeque<Duration>, percentile: f32) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let mut millis = samples
        .iter()
        .map(|sample| sample.as_secs_f32() * 1_000.0)
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
            "lines",
            terminal
                .map(|metrics| metrics.rendered_line_count.to_string())
                .unwrap_or_else(|| "0".into()),
        ),
        (
            "spans",
            terminal
                .map(|metrics| metrics.rendered_span_count.to_string())
                .unwrap_or_else(|| "0".into()),
        ),
    ]
}

fn expanded_perf_strings(
    state: &PerfOverlayState,
    active_session_id: u64,
    palette_open: bool,
) -> Vec<(&'static str, String)> {
    let terminal = state
        .active_session_perf_snapshot
        .as_ref()
        .map(|snapshot| &snapshot.terminal);
    vec![
        ("refresh", state.refreshes_last_second().to_string()),
        (
            "presented",
            state.frames_presented_last_second().to_string(),
        ),
        ("idle", format!("{:.0}%", state.idle_refresh_percentage())),
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
                .map(|metrics| metrics.truncated_line_count.to_string())
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
    ]
}

impl SeanceWorkspace {
    fn theme(&self) -> Theme {
        self.active_theme.theme()
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
    }

    fn schedule_refresh(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        entity: gpui::Entity<Self>,
    ) {
        window
            .spawn(cx, async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(33))
                        .await;
                    let _ = cx.update(|window, cx| {
                        let _ = entity.update(cx, |this, _| {
                            let session_perf =
                                this.active_session().map(|session| session.perf_snapshot());
                            let reason = this.classify_refresh_reason(session_perf.as_ref());
                            this.perf_overlay.mark_refresh_request(
                                Instant::now(),
                                reason,
                                session_perf,
                            );
                        });
                        window.refresh();
                    });
                }
            })
            .detach();
    }

    fn schedule_perf_sampling(&self, window: &mut Window, entity: gpui::Entity<Self>) {
        if !self.perf_overlay.mode.is_enabled() || self.perf_overlay.sampler_running {
            return;
        }

        Self::schedule_perf_sampling_for(window, entity);
    }

    fn schedule_perf_sampling_for(window: &Window, entity: gpui::Entity<Self>) {
        window.on_next_frame(move |window, cx| {
            let should_continue = entity.update(cx, |this, _| {
                this.perf_overlay.sampler_running = true;
                this.perf_overlay.mark_frame_presented(Instant::now());
                this.perf_overlay.mode.is_enabled()
            });
            if should_continue {
                Self::schedule_perf_sampling_for(window, entity.clone());
            } else {
                let _ = entity.update(cx, |this, _| {
                    this.perf_overlay.sampler_running = false;
                });
            }
        });
    }

    fn classify_refresh_reason(&self, session_perf: Option<&SessionPerfSnapshot>) -> RedrawReason {
        if self.palette_open {
            RedrawReason::Palette
        } else if session_perf.is_some_and(|snapshot| snapshot.dirty_since_last_ui_frame) {
            RedrawReason::TerminalUpdate
        } else {
            RedrawReason::IdleTick
        }
    }

    fn toggle_perf_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.perf_overlay.mode = self.perf_overlay.mode.next();
        self.perf_overlay.pending_redraw_reason = RedrawReason::Input;
        if self.perf_overlay.mode.is_enabled() {
            self.perf_overlay.sampler_running = false;
            self.schedule_perf_sampling(window, cx.entity());
        } else {
            self.perf_overlay.sampler_running = false;
        }
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
        self.selected_host_id = None;
        self.host_editor = None;
        self.unlock_form.reset_for_unlock();
        self.unlock_form.message =
            Some("Vault locked. Decrypted records were cleared from memory.".into());
        self.status_message = Some("Vault locked.".into());
        self.palette_open = false;
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn begin_add_host(&mut self, cx: &mut Context<Self>) {
        if !self.vault_unlocked() {
            self.unlock_form.reset_for_unlock();
            self.unlock_form.message = Some("Unlock the vault before adding a saved host.".into());
        } else {
            self.host_editor = Some(HostEditorState::blank());
        }
        self.palette_open = false;
        self.perf_overlay.mark_input(RedrawReason::Palette);
        cx.notify();
    }

    fn begin_edit_host(&mut self, host_id: &str, cx: &mut Context<Self>) {
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

    fn connect_saved_host(&mut self, host_id: &str, cx: &mut Context<Self>) {
        self.selected_host_id = Some(host_id.into());
        match self.backend.connect_host(host_id) {
            Ok(session) => {
                if let Some(geometry) = self.last_applied_geometry {
                    let _ = session.resize(geometry);
                }
                self.active_session_id = session.id();
                self.sessions.push(session);
                self.status_message = Some("SSH session connected.".into());
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
            auth_order: parse_auth_order(&editor.auth_order),
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

    fn active_session(&self) -> Option<&Arc<dyn TerminalSession>> {
        self.sessions
            .iter()
            .find(|s| s.id() == self.active_session_id)
    }

    fn spawn_session(&mut self, cx: &mut Context<Self>) {
        if let Ok(session) = self.backend.spawn_local_session() {
            if let Some(geometry) = self.last_applied_geometry {
                let _ = session.resize(geometry);
            }
            self.active_session_id = session.id();
            self.sessions.push(session);
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
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn close_session(&mut self, id: u64, cx: &mut Context<Self>) {
        self.sessions.retain(|s| s.id() != id);
        if self.active_session_id == id {
            self.active_session_id = self.sessions.last().map(|s| s.id()).unwrap_or(0);
        }
        if self.active_session_id == 0 {
            self.last_applied_geometry = None;
            self.active_terminal_rows = TerminalGeometry::default().size.rows as usize;
        }
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

    fn execute_palette_action(&mut self, action: PaletteAction, cx: &mut Context<Self>) {
        match action {
            PaletteAction::NewLocalTerminal => self.spawn_session(cx),
            PaletteAction::SwitchSession(id) => self.select_session(id, cx),
            PaletteAction::CloseActiveSession => {
                let id = self.active_session_id;
                self.close_session(id, cx);
            }
            PaletteAction::SwitchTheme(tid) => {
                self.active_theme = tid;
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
            PaletteAction::AddPasswordCredential => {
                match self.backend.save_password_credential(VaultPasswordCredential {
                    id: String::new(),
                    label: format!("credential-{}", now_ui_suffix()),
                    username_hint: None,
                    secret: "change-me".into(),
                }) {
                    Ok(summary) => {
                        self.status_message = Some(format!(
                            "Created placeholder password credential '{}'. Edit support lands next.",
                            summary.label
                        ));
                    }
                    Err(err) => self.status_message = Some(err.to_string()),
                }
            }
            PaletteAction::ImportPrivateKey => {
                self.status_message = Some(
                    "Private key import backend is ready; UI import form is still pending."
                        .into(),
                );
            }
            PaletteAction::GenerateEd25519Key => {
                match self
                    .backend
                    .generate_ed25519_key(format!("ed25519-{}", now_ui_suffix()))
                {
                    Ok(summary) => {
                        self.status_message = Some(format!(
                            "Generated vault-backed key '{}'.",
                            summary.label
                        ));
                    }
                    Err(err) => self.status_message = Some(err.to_string()),
                }
            }
            PaletteAction::GenerateRsaKey => {
                match self.backend.generate_rsa_key(format!("rsa-{}", now_ui_suffix())) {
                    Ok(summary) => {
                        self.status_message = Some(format!(
                            "Generated vault-backed key '{}'.",
                            summary.label
                        ));
                    }
                    Err(err) => self.status_message = Some(err.to_string()),
                }
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
                self.connect_saved_host(&id, cx);
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
        if self.host_editor.is_some() {
            self.handle_host_editor_key(event, cx);
            return;
        }
        if mods.platform && key == "k" {
            self.toggle_palette(cx);
            return;
        }
        if mods.platform && key == "t" {
            self.spawn_session(cx);
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
            self.handle_palette_key(event, cx);
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

    fn handle_palette_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
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
                let count = build_items(
                    &self.sessions,
                    &self.saved_hosts,
                    self.active_session_id,
                    self.active_theme,
                    &self.palette_query,
                    self.vault_unlocked(),
                )
                .len();
                if self.palette_selected + 1 < count {
                    self.palette_selected += 1;
                }
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "enter" => {
                let items = build_items(
                    &self.sessions,
                    &self.saved_hosts,
                    self.active_session_id,
                    self.active_theme,
                    &self.palette_query,
                    self.vault_unlocked(),
                );
                if let Some(item) = items.get(self.palette_selected) {
                    let action = item.action.clone();
                    self.execute_palette_action(action, cx);
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
        let Some(editor) = self.host_editor.as_mut() else {
            return;
        };
        let key = event.keystroke.key.as_str();
        let key_char = event.keystroke.key_char.as_deref();

        match key {
            "escape" => {
                self.host_editor = None;
            }
            "tab" | "down" => {
                editor.selected_field = (editor.selected_field + 1) % HostField::ALL.len();
            }
            "up" => {
                editor.selected_field =
                    (editor.selected_field + HostField::ALL.len() - 1) % HostField::ALL.len();
            }
            "backspace" => match editor.field() {
                HostField::Label => {
                    editor.label.pop();
                }
                HostField::Hostname => {
                    editor.hostname.pop();
                }
                HostField::Username => {
                    editor.username.pop();
                }
                HostField::Port => {
                    editor.port.pop();
                }
                HostField::Notes => {
                    editor.notes.pop();
                }
                HostField::AuthOrder => {
                    editor.auth_order.pop();
                }
            },
            "enter" => {
                if matches!(editor.field(), HostField::AuthOrder) {
                    self.save_host_editor(cx);
                    return;
                }
                editor.selected_field = (editor.selected_field + 1) % HostField::ALL.len();
            }
            _ => {
                if let Some(ch) = key_char {
                    let m = event.keystroke.modifiers;
                    if !m.platform && !m.control && !m.function {
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
                            HostField::AuthOrder => editor.auth_order.push_str(ch),
                        }
                    }
                }
            }
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    // ─── Rendering ──────────────────────────────────────────

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        let vault_status = self.backend.vault_status();

        let mut session_list = div().flex().flex_col().gap_1().px_2();
        for session in &self.sessions {
            let active = session.id() == self.active_session_id;
            let sid = session.id();
            let title = session.title().to_string();
            let snapshot = session.snapshot();
            let preview = snapshot
                .lines
                .iter()
                .rev()
                .map(TerminalLine::plain_text)
                .find(|l| !l.trim().is_empty())
                .unwrap_or_else(|| "waiting for output…".into());

            let mut card = div()
                .px_3()
                .py_2()
                .rounded_lg()
                .cursor_pointer()
                .flex()
                .flex_col()
                .gap(px(2.0));
            card = if active {
                card.bg(t.glass_active)
                    .border_1()
                    .border_color(t.accent_glow)
            } else {
                card.hover(|s| s.bg(t.glass_hover))
            };

            let close_sid = sid;
            card = card
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(if active {
                                    t.accent
                                } else {
                                    t.text_ghost
                                }))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(if active {
                                            t.text_primary
                                        } else {
                                            t.text_secondary
                                        })
                                        .child(title),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.text_ghost)
                                .cursor_pointer()
                                .hover(|s| s.text_color(t.text_secondary))
                                .child("×")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.close_session(close_sid, cx);
                                    }),
                                ),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text_muted)
                        .line_clamp(1)
                        .child(preview),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.select_session(sid, cx);
                    }),
                );

            session_list = session_list.child(card);
        }

        let mut host_list = div().flex().flex_col().gap_1().px_2();
        if vault_status.unlocked {
            if self.saved_hosts.is_empty() {
                host_list = host_list.child(
                    div().px_3().py_2().rounded_lg().bg(t.glass_hover).child(
                        div()
                            .text_xs()
                            .text_color(t.text_muted)
                            .child("No saved hosts yet"),
                    ),
                );
            }

            for host in &self.saved_hosts {
                let selected = self
                    .selected_host_id
                    .as_ref()
                    .is_some_and(|id| id == &host.id);
                let host_id = host.id.clone();
                let edit_id = host.id.clone();
                let delete_id = host.id.clone();

                let mut card = div()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap(px(2.0));
                card = if selected {
                    card.bg(t.glass_active)
                        .border_1()
                        .border_color(t.accent_glow)
                } else {
                    card.hover(|s| s.bg(t.glass_hover))
                };

                card = card
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(if selected {
                                        t.text_primary
                                    } else {
                                        t.text_secondary
                                    })
                                    .child(host.label.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(t.text_ghost)
                                            .child("✎")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.begin_edit_host(&edit_id, cx);
                                                }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(t.text_ghost)
                                            .child("×")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.delete_saved_host(&delete_id, cx);
                                                }),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text_muted)
                            .line_clamp(1)
                            .child(format!("{}@{}:{}", host.username, host.hostname, host.port)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.selected_host_id = Some(host_id.clone());
                            this.connect_saved_host(&host_id, cx);
                        }),
                    );

                host_list = host_list.child(card);
            }
        } else {
            host_list = host_list.child(
                div().px_3().py_2().rounded_lg().bg(t.glass_hover).child(
                    div()
                        .text_xs()
                        .text_color(t.text_muted)
                        .child("Unlock the vault to view saved hosts"),
                ),
            );
        }

        let mut footer = div()
            .px_3()
            .pb_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .px_2()
                    .py(px(6.0))
                    .rounded_md()
                    .border_1()
                    .border_color(t.glass_border)
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.glass_hover))
                    .child(div().text_xs().text_color(t.text_ghost).child("⌘K"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text_muted)
                            .child("Command Palette"),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_palette(cx);
                        }),
                    ),
            )
            .child(
                div()
                    .px_2()
                    .py(px(4.0))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.glass_hover))
                    .child(
                        div()
                            .text_xs()
                            .text_color(if vault_status.unlocked {
                                t.accent
                            } else {
                                t.warning
                            })
                            .child("•"),
                    )
                    .child(div().text_xs().text_color(t.text_muted).child(
                        if vault_status.unlocked {
                            "Vault unlocked"
                        } else {
                            "Unlock vault"
                        },
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if this.vault_unlocked() {
                                this.lock_vault(cx);
                            } else {
                                this.unlock_form.reset_for_unlock();
                                this.unlock_form.message = Some(
                                    "Enter the recovery passphrase to unlock the vault.".into(),
                                );
                                cx.notify();
                            }
                        }),
                    ),
            )
            .child(
                div()
                    .px_2()
                    .py(px(4.0))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.glass_hover))
                    .child(div().text_xs().text_color(t.accent).child("◑"))
                    .child(div().text_xs().text_color(t.text_muted).child(t.name))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.palette_open = true;
                            this.palette_query = "theme".into();
                            this.palette_selected = 0;
                            cx.notify();
                        }),
                    ),
            );

        if let Some(message) = self.status_message.clone() {
            footer = footer.child(
                div()
                    .px_2()
                    .py(px(4.0))
                    .rounded_md()
                    .bg(t.glass_hover)
                    .text_xs()
                    .text_color(t.text_muted)
                    .child(message),
            );
        }

        div()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .justify_between()
            .bg(t.glass_tint)
            .border_r_1()
            .border_color(t.glass_border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .pt(px(38.0))
                            .px_4()
                            .pb_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_size(px(18.0)).text_color(t.accent).child("◈"))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(t.text_primary)
                                    .child("Séance"),
                            ),
                    )
                    .child(
                        div()
                            .px_3()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(t.text_muted)
                                    .child("SESSIONS"),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py(px(2.0))
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(t.text_ghost)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(t.glass_hover).text_color(t.text_muted))
                                    .child("+ new")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.spawn_session(cx);
                                        }),
                                    ),
                            ),
                    )
                    .child(session_list)
                    .child(
                        div()
                            .px_3()
                            .pt_2()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(t.text_muted)
                                    .child("VAULT"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if vault_status.unlocked {
                                        t.accent
                                    } else {
                                        t.warning
                                    })
                                    .child(if vault_status.unlocked {
                                        "unlocked"
                                    } else {
                                        "locked"
                                    }),
                            ),
                    )
                    .child(host_list),
            )
            .child(footer)
    }

    fn render_terminal_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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

        let session = self.active_session().unwrap();
        let snapshot = session.snapshot();
        let mut visible_lines = snapshot.lines;
        if visible_lines.len() > self.active_terminal_rows {
            let start = visible_lines.len() - self.active_terminal_rows;
            visible_lines.drain(0..start);
        }
        self.perf_overlay.visible_line_count = visible_lines.len();

        let mut term = base
            .p_4()
            .font_family(TERMINAL_FONT_FAMILY)
            .text_size(px(TERMINAL_FONT_SIZE_PX))
            .line_height(px(TERMINAL_LINE_HEIGHT_PX))
            .text_color(t.text_primary);

        for line in visible_lines {
            term = term.child(render_terminal_line(&line, &t));
        }

        if let Some(exit_status) = snapshot.exit_status {
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
            palette_open = self.palette_open,
            "rendered terminal pane"
        );

        term
    }

    fn render_palette_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        let items = build_items(
            &self.sessions,
            &self.saved_hosts,
            self.active_session_id,
            self.active_theme,
            &self.palette_query,
            self.vault_unlocked(),
        );
        trace!(palette_items = items.len(), "rendered palette overlay");
        let selected = self.palette_selected.min(items.len().saturating_sub(1));

        let mut item_list = div().flex().flex_col().p_2();

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

        for (idx, item) in items.iter().enumerate() {
            let is_sel = idx == selected;
            let action = item.action.clone();

            let mut row = div()
                .px_3()
                .py(px(8.0))
                .rounded_lg()
                .flex()
                .items_center()
                .gap_3()
                .cursor_pointer();

            row = if is_sel {
                row.bg(t.selection_soft)
            } else {
                row.hover(|s| s.bg(t.glass_hover))
            };

            row = row
                .child(
                    div()
                        .w(px(24.0))
                        .flex()
                        .justify_center()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(if is_sel { t.accent } else { t.text_muted })
                        .child(item.glyph),
                )
                .child(
                    div()
                        .flex_1()
                        .child(
                            div()
                                .text_sm()
                                .text_color(if is_sel {
                                    t.text_primary
                                } else {
                                    t.text_secondary
                                })
                                .child(item.label.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.text_muted)
                                .child(item.hint.clone()),
                        ),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.execute_palette_action(action.clone(), cx);
                    }),
                );

            item_list = item_list.child(row);
        }

        let panel = div()
            .w(px(540.0))
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
                            .child("›"),
                    )
                    .child(div().flex_1().flex().items_center().child(
                        if self.palette_query.is_empty() {
                            div()
                                .text_sm()
                                .text_color(t.text_muted)
                                .child("Search commands…")
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
            .child(item_list)
            .child(
                div()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(t.glass_border)
                    .flex()
                    .items_center()
                    .gap_4()
                    .text_xs()
                    .text_color(t.text_ghost)
                    .child("↑↓ navigate")
                    .child("↵ select")
                    .child("esc close"),
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

    fn render_host_editor_overlay(&self) -> impl IntoElement {
        let t = self.theme();
        let Some(editor) = self.host_editor.as_ref() else {
            return div();
        };

        let title = if editor.host_id.is_some() {
            "Edit Saved Host"
        } else {
            "Add Saved Host"
        };

        let fields = [
            (HostField::Label, editor.label.clone()),
            (HostField::Hostname, editor.hostname.clone()),
            (HostField::Username, editor.username.clone()),
            (HostField::Port, editor.port.clone()),
            (HostField::Notes, editor.notes.clone()),
            (HostField::AuthOrder, editor.auth_order.clone()),
        ];

        let mut panel = div()
            .w(px(620.0))
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

        for (idx, (field, value)) in fields.into_iter().enumerate() {
            panel = panel.child(editor_field_card(
                field.title(),
                value,
                idx == editor.selected_field,
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
                        .child("tab move  esc cancel  enter on auth order saves"),
                )
                .child(
                    div()
                        .px_3()
                        .py(px(6.0))
                        .rounded_md()
                        .bg(t.accent_glow)
                        .text_xs()
                        .text_color(t.text_primary)
                        .child("save encrypted host"),
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
                    "idle" => {
                        perf_status_color(self.perf_overlay.idle_refresh_percentage() < 80.0, &t)
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

fn render_terminal_line(line: &TerminalLine, theme: &Theme) -> StyledText {
    let (text, runs) = build_text_runs(line, theme);
    StyledText::new(text).with_runs(runs)
}

fn build_text_runs(line: &TerminalLine, theme: &Theme) -> (SharedString, Vec<TextRun>) {
    if line.spans.is_empty() {
        let text: SharedString = " ".into();
        let runs = vec![text_run("Menlo", " ", TerminalCellStyle::default(), theme)];
        return (text, runs);
    }

    let mut text = String::new();
    let mut runs = Vec::with_capacity(line.spans.len());

    for span in &line.spans {
        text.push_str(&span.text);
        runs.push(text_run("Menlo", &span.text, span.style, theme));
    }

    (text.into(), runs)
}

fn text_run(family: &'static str, text: &str, style: TerminalCellStyle, theme: &Theme) -> TextRun {
    let mut terminal_font = font(family);
    if style.bold {
        terminal_font = terminal_font.bold();
    }
    if style.italic {
        terminal_font = terminal_font.italic();
    }

    TextRun {
        len: text.len(),
        font: terminal_font,
        color: resolve_terminal_foreground(style, theme),
        background_color: style.background.map(terminal_color_to_hsla),
        underline: style.underline.then_some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(resolve_terminal_foreground(style, theme)),
            wavy: false,
        }),
        strikethrough: None,
    }
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
    use gpui::{FontStyle, FontWeight, size};
    use seance_terminal::{TerminalLine, TerminalSpan};

    #[test]
    fn compute_geometry_uses_viewport_minus_sidebar_and_padding() {
        let geometry = compute_terminal_geometry(
            size(px(1280.0), px(820.0)),
            TerminalMetrics {
                cell_width_px: 8.0,
                cell_height_px: 19.0,
                line_height_px: 19.0,
            },
        )
        .expect("geometry");

        assert_eq!(geometry.pixel_size.width_px, 988);
        assert_eq!(geometry.pixel_size.height_px, 788);
        assert_eq!(geometry.size.cols, 123);
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
            },
        )
        .expect("geometry");

        assert_eq!(geometry.size.cols, 1);
        assert_eq!(geometry.size.rows, 1);
    }

    #[test]
    fn builds_runs_with_utf8_byte_lengths() {
        let line = TerminalLine {
            spans: vec![
                TerminalSpan {
                    text: "café".into(),
                    style: TerminalCellStyle::default(),
                },
                TerminalSpan {
                    text: " 👋".into(),
                    style: TerminalCellStyle {
                        bold: true,
                        ..TerminalCellStyle::default()
                    },
                },
            ],
        };

        let (text, runs) = build_text_runs(&line, &ThemeId::ObsidianSmoke.theme());

        assert_eq!(text.as_ref(), "café 👋");
        assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
        assert_eq!(runs[0].len, "café".len());
        assert_eq!(runs[1].len, " 👋".len());
        assert_eq!(runs[1].font.weight, FontWeight::BOLD);
    }

    #[test]
    fn maps_background_and_underline_styles() {
        let line = TerminalLine {
            spans: vec![TerminalSpan {
                text: "styled".into(),
                style: TerminalCellStyle {
                    foreground: Some(TerminalColor { r: 255, g: 0, b: 0 }),
                    background: Some(TerminalColor { r: 0, g: 0, b: 0 }),
                    bold: false,
                    italic: true,
                    underline: true,
                    ..TerminalCellStyle::default()
                },
            }],
        };

        let (_text, runs) = build_text_runs(&line, &ThemeId::ObsidianSmoke.theme());

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].font.style, FontStyle::Italic);
        assert!(runs[0].background_color.is_some());
        assert!(runs[0].underline.is_some());
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
        let samples = VecDeque::from(vec![
            Duration::from_millis(10),
            Duration::from_millis(12),
            Duration::from_millis(18),
            Duration::from_millis(20),
        ]);
        let now = Instant::now();
        let timestamps = VecDeque::from(vec![now, now, now]);

        let stats = build_frame_stats(9, &samples, &timestamps, RedrawReason::TerminalUpdate);

        assert_eq!(stats.frame_count_total, 9);
        assert_eq!(stats.fps_1s, 3.0);
        assert_eq!(stats.frame_time_last_ms, 20.0);
        assert!((stats.frame_time_avg_ms - 15.0).abs() < 0.01);
        assert_eq!(stats.frame_time_p95_ms, 20.0);
        assert_eq!(stats.redraw_reason, RedrawReason::TerminalUpdate);
    }

    #[test]
    fn compact_perf_strings_include_primary_metrics() {
        let mut state = PerfOverlayState::new(UiPerfMode::Compact);
        state.frame_stats.fps_1s = 59.0;
        state.frame_stats.frame_time_last_ms = 12.0;

        let rows = compact_perf_strings(&state);
        let labels = rows.into_iter().map(|(label, _)| label).collect::<Vec<_>>();

        assert_eq!(labels, vec!["fps", "frame", "snapshot", "lines", "spans"]);
    }

    #[test]
    fn expanded_perf_strings_include_render_insights() {
        let mut state = PerfOverlayState::new(UiPerfMode::Expanded);
        state.visible_line_count = 32;
        state.pending_redraw_reason = RedrawReason::Palette;
        state.frame_stats.redraw_reason = RedrawReason::Palette;

        let rows = expanded_perf_strings(&state, 7, true);
        let labels = rows.into_iter().map(|(label, _)| label).collect::<Vec<_>>();

        assert!(labels.contains(&"refresh"));
        assert!(labels.contains(&"presented"));
        assert!(labels.contains(&"idle"));
        assert!(labels.contains(&"visible"));
        assert!(labels.contains(&"reason"));
    }
}

impl Focusable for SeanceWorkspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SeanceWorkspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();

        let mut root = div()
            .size_full()
            .flex()
            .bg(t.bg_deep)
            .text_color(t.text_primary)
            .child(self.render_sidebar(cx))
            .child(self.render_terminal_pane(cx));

        if self.palette_open {
            root = root.child(deferred(self.render_palette_overlay(cx)).with_priority(1));
        }
        if self.host_editor.is_some() {
            root = root.child(deferred(self.render_host_editor_overlay()).with_priority(2));
        }
        if self.unlock_form.is_visible() {
            root = root.child(deferred(self.render_unlock_overlay()).with_priority(3));
        }
        if self.perf_overlay.mode.is_enabled() {
            root = root.child(deferred(self.render_perf_overlay()).with_priority(4));
        }

        root
    }
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

fn format_auth_order(auth_order: &[HostAuthRef]) -> String {
    auth_order
        .iter()
        .map(|auth| match auth {
            HostAuthRef::Password { credential_id } => format!("password:{credential_id}"),
            HostAuthRef::PrivateKey {
                key_id,
                passphrase_credential_id,
            } => match passphrase_credential_id {
                Some(passphrase_id) => format!("key:{key_id}:{passphrase_id}"),
                None => format!("key:{key_id}"),
            },
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn parse_auth_order(input: &str) -> Vec<HostAuthRef> {
    input
        .split(',')
        .filter_map(|token| {
            let trimmed = token.trim();
            if trimmed.is_empty() {
                return None;
            }
            let mut parts = trimmed.split(':');
            match (parts.next(), parts.next(), parts.next()) {
                (Some("password"), Some(credential_id), None) => Some(HostAuthRef::Password {
                    credential_id: credential_id.to_string(),
                }),
                (Some("key"), Some(key_id), passphrase_credential_id) => {
                    Some(HostAuthRef::PrivateKey {
                        key_id: key_id.to_string(),
                        passphrase_credential_id: passphrase_credential_id
                            .map(|value| value.to_string())
                            .filter(|value| !value.is_empty()),
                    })
                }
                _ => None,
            }
        })
        .collect()
}

fn now_ui_suffix() -> i64 {
    seance_vault::now_ts()
}
