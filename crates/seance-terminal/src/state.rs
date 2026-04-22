use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use anyhow::Result;
use tracing::trace;

use crate::{
    SessionSummary, TerminalGeometry, TerminalGridSelection, TerminalKeyEvent, TerminalMouseEvent,
    TerminalPaste, TerminalRow, TerminalScrollCommand, TerminalTextEvent, TerminalTurnSnapshot,
    TerminalViewportSnapshot,
};

static SESSION_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Returns a unique session id for any terminal session (local shell, SSH, etc.).
pub fn next_session_id() -> u64 {
    SESSION_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GhosttyDirtyState {
    #[default]
    Clean,
    Partial,
    Full,
    Error,
}

#[derive(Clone, Debug, Default)]
pub struct TerminalRenderMetrics {
    pub snapshot_seq: u64,
    pub snapshot_count_total: u64,
    pub snapshot_rate_1s: f32,
    pub snapshot_interval_last_ms: f32,
    pub snapshot_interval_avg_ms: f32,
    pub snapshot_interval_p95_ms: f32,
    pub last_snapshot_duration: Duration,
    pub avg_snapshot_duration: Duration,
    pub max_snapshot_duration: Duration,
    pub p95_snapshot_duration_ms: f32,
    pub rendered_row_count: usize,
    pub rendered_cell_count: usize,
    pub dirty_row_count: usize,
    pub ghostty_dirty_state: GhosttyDirtyState,
    pub vt_bytes_processed_since_last_snapshot: usize,
    pub total_vt_bytes_processed: u64,
    pub viewport_revision: u64,
    pub scrollback_rows: usize,
    pub at_bottom: bool,
    pub transcript_dropped_events: u64,
}

#[derive(Clone, Debug, Default)]
pub struct SessionPerfSnapshot {
    pub terminal: TerminalRenderMetrics,
    pub dirty_since_last_ui_frame: bool,
}

pub trait TerminalSession: Send + Sync {
    fn id(&self) -> u64;
    fn title(&self) -> &str;
    fn summary(&self) -> SessionSummary;
    fn viewport_snapshot(&self) -> TerminalViewportSnapshot;
    /// Sends raw bytes to the backing PTY or SSH channel.
    ///
    /// This is a low-level escape hatch for programmatic input. Ordinary terminal typing from the
    /// UI should use `send_text`, and non-text terminal keys should use `send_key`.
    fn send_input(&self, bytes: Vec<u8>) -> Result<()>;
    /// Sends printable or composed text that should be written as terminal input bytes.
    fn send_text(&self, event: TerminalTextEvent) -> Result<()>;
    /// Sends non-text terminal keys and modifier-driven terminal control-key combinations.
    fn send_key(&self, event: TerminalKeyEvent) -> Result<()>;
    fn send_mouse(&self, event: TerminalMouseEvent) -> Result<()>;
    fn paste(&self, paste: TerminalPaste) -> Result<()>;
    fn resize(&self, geometry: TerminalGeometry) -> Result<()>;
    fn scroll_viewport(&self, command: TerminalScrollCommand) -> Result<()>;
    fn scroll_to_bottom(&self) -> Result<()>;
    fn copy_selection_text(&self, _selection: TerminalGridSelection) -> Result<String> {
        Err(anyhow::anyhow!(
            "copy selection text is not supported for this session"
        ))
    }
    fn previous_turn(&self) -> Result<Option<TerminalTurnSnapshot>> {
        Err(anyhow::anyhow!(
            "previous turn lookup is not supported for this session"
        ))
    }
    fn copy_active_screen_text(&self) -> Result<String> {
        Err(anyhow::anyhow!(
            "copy active screen text is not supported for this session"
        ))
    }
    fn perf_snapshot(&self) -> SessionPerfSnapshot;
    fn take_notify_rx(&self) -> Option<mpsc::Receiver<()>>;
}

#[derive(Debug, Default)]
pub(crate) struct SessionPerfState {
    pub(crate) snapshot: SessionPerfSnapshot,
    snapshot_samples: u64,
    total_snapshot_duration_ns: u128,
    snapshot_timestamps: VecDeque<Instant>,
    snapshot_intervals: VecDeque<(Instant, Duration)>,
    snapshot_duration_samples: VecDeque<(Instant, Duration)>,
    last_snapshot_timestamp: Option<Instant>,
}

#[derive(Clone)]
pub struct SharedSessionState {
    summary: Arc<Mutex<SessionSummary>>,
    viewport: Arc<Mutex<TerminalViewportSnapshot>>,
    pub(crate) perf_snapshot: Arc<Mutex<SessionPerfState>>,
    notify_tx: mpsc::SyncSender<()>,
}

pub(crate) struct PublishedViewport {
    pub(crate) viewport_snapshot: TerminalViewportSnapshot,
    pub(crate) summary: SessionSummary,
    pub(crate) duration: Duration,
    pub(crate) ghostty_dirty_state: GhosttyDirtyState,
    pub(crate) dirty_row_count: usize,
    pub(crate) rendered_cell_count: usize,
    pub(crate) vt_bytes_processed_since_last_snapshot: usize,
    pub(crate) transcript_dropped_events: u64,
}

impl SharedSessionState {
    pub fn new(
        initial_message: impl Into<String>,
        geometry: TerminalGeometry,
    ) -> (Self, mpsc::Receiver<()>) {
        let initial_message = initial_message.into();
        let initial_row = Arc::new(TerminalRow {
            cells: vec![crate::TerminalCell {
                text: initial_message.clone(),
                style: crate::TerminalCellStyle::default(),
                width: 1,
            }],
        });
        let (notify_tx, notify_rx) = mpsc::sync_channel(1);
        let state = Self {
            summary: Arc::new(Mutex::new(SessionSummary {
                preview_line: initial_message,
                viewport_revision: 1,
                at_bottom: true,
                ..SessionSummary::default()
            })),
            viewport: Arc::new(Mutex::new(TerminalViewportSnapshot {
                rows: Arc::from(vec![initial_row]),
                row_revisions: Arc::from(vec![1_u64]),
                cursor: None,
                scrollbar: None,
                scroll_offset_rows: 0,
                revision: 1,
                cols: geometry.size.cols,
                rows_visible: geometry.size.rows,
            })),
            perf_snapshot: Arc::new(Mutex::new(SessionPerfState::default())),
            notify_tx,
        };
        (state, notify_rx)
    }

    pub fn summary(&self) -> SessionSummary {
        self.summary
            .lock()
            .expect("session summary poisoned")
            .clone()
    }

    pub fn viewport_snapshot(&self) -> TerminalViewportSnapshot {
        self.viewport
            .lock()
            .expect("session viewport poisoned")
            .clone()
    }

    pub fn perf_snapshot(&self) -> SessionPerfSnapshot {
        let mut perf = self.perf_snapshot.lock().expect("session perf poisoned");
        let snapshot = perf.snapshot.clone();
        perf.snapshot.dirty_since_last_ui_frame = false;
        snapshot
    }

    pub(crate) fn publish_viewport(&self, published: PublishedViewport) {
        let now = Instant::now();
        let PublishedViewport {
            viewport_snapshot,
            summary,
            duration,
            ghostty_dirty_state,
            dirty_row_count,
            rendered_cell_count,
            vt_bytes_processed_since_last_snapshot,
            transcript_dropped_events,
        } = published;
        let rendered_row_count = viewport_snapshot.row_count();
        let viewport_revision = viewport_snapshot.revision;
        let scrollback_rows = summary.scrollback_rows;
        let at_bottom = summary.at_bottom;

        *self.summary.lock().expect("session summary poisoned") = summary;
        *self.viewport.lock().expect("session viewport poisoned") = viewport_snapshot;

        let mut perf = self.perf_snapshot.lock().expect("session perf poisoned");
        perf.snapshot_samples = perf.snapshot_samples.saturating_add(1);
        perf.total_snapshot_duration_ns = perf
            .total_snapshot_duration_ns
            .saturating_add(duration.as_nanos());
        let snapshot_samples = perf.snapshot_samples;
        let avg_snapshot_duration =
            duration_from_nanos(perf.total_snapshot_duration_ns / u128::from(snapshot_samples));
        perf.snapshot_duration_samples.push_back((now, duration));
        if let Some(previous) = perf.last_snapshot_timestamp.replace(now) {
            let interval = now.saturating_duration_since(previous);
            if interval <= PERF_WINDOW {
                perf.snapshot_intervals.push_back((now, interval));
            }
        }
        perf.snapshot_timestamps.push_back(now);
        trim_instants(&mut perf.snapshot_timestamps, now, PERF_WINDOW);
        trim_timed_durations(&mut perf.snapshot_intervals, now, PERF_WINDOW);
        trim_timed_durations(&mut perf.snapshot_duration_samples, now, PERF_WINDOW);
        let snapshot_rate_1s = perf.snapshot_timestamps.len() as f32;
        let snapshot_interval_last_ms = perf
            .snapshot_intervals
            .back()
            .map(|(_, interval)| interval.as_secs_f32() * 1_000.0)
            .unwrap_or_default();
        let snapshot_interval_avg_ms = average_duration_ms(&perf.snapshot_intervals);
        let snapshot_interval_p95_ms = percentile_duration_ms(&perf.snapshot_intervals, 0.95);
        let p95_snapshot_duration_ms =
            percentile_duration_ms(&perf.snapshot_duration_samples, 0.95);

        {
            let metrics = &mut perf.snapshot.terminal;
            metrics.snapshot_seq = snapshot_samples;
            metrics.snapshot_count_total = snapshot_samples;
            metrics.snapshot_rate_1s = snapshot_rate_1s;
            metrics.snapshot_interval_last_ms = snapshot_interval_last_ms;
            metrics.snapshot_interval_avg_ms = snapshot_interval_avg_ms;
            metrics.snapshot_interval_p95_ms = snapshot_interval_p95_ms;
            metrics.last_snapshot_duration = duration;
            metrics.avg_snapshot_duration = avg_snapshot_duration;
            metrics.max_snapshot_duration = metrics.max_snapshot_duration.max(duration);
            metrics.p95_snapshot_duration_ms = p95_snapshot_duration_ms;
            metrics.rendered_row_count = rendered_row_count;
            metrics.rendered_cell_count = rendered_cell_count;
            metrics.dirty_row_count = dirty_row_count;
            metrics.ghostty_dirty_state = ghostty_dirty_state;
            metrics.vt_bytes_processed_since_last_snapshot = vt_bytes_processed_since_last_snapshot;
            metrics.total_vt_bytes_processed = metrics
                .total_vt_bytes_processed
                .saturating_add(vt_bytes_processed_since_last_snapshot as u64);
            metrics.viewport_revision = viewport_revision;
            metrics.scrollback_rows = scrollback_rows;
            metrics.at_bottom = at_bottom;
            metrics.transcript_dropped_events = transcript_dropped_events;
        }
        perf.snapshot.dirty_since_last_ui_frame = true;

        let metrics = &perf.snapshot.terminal;

        trace!(
            snapshot_seq = metrics.snapshot_seq,
            snapshot_ms = duration.as_secs_f64() * 1_000.0,
            viewport_revision = metrics.viewport_revision,
            rendered_row_count = metrics.rendered_row_count,
            rendered_cell_count = metrics.rendered_cell_count,
            dirty_row_count = metrics.dirty_row_count,
            vt_bytes_processed_since_last_snapshot,
            "published terminal viewport"
        );

        let _ = self.notify_tx.try_send(());
    }

    pub fn set_error(&self, error: &anyhow::Error, geometry: TerminalGeometry) {
        let error_text = format!("Failed to start session: {error:#}");
        let row = Arc::new(TerminalRow {
            cells: vec![crate::TerminalCell {
                text: error_text.clone(),
                style: crate::TerminalCellStyle::default(),
                width: 1,
            }],
        });

        *self.summary.lock().expect("session summary poisoned") = SessionSummary {
            exit_status: Some("startup error".to_string()),
            preview_line: error_text,
            viewport_revision: 1,
            at_bottom: true,
            ..SessionSummary::default()
        };
        *self.viewport.lock().expect("session viewport poisoned") = TerminalViewportSnapshot {
            rows: Arc::from(vec![row]),
            row_revisions: Arc::from(vec![1_u64]),
            cursor: None,
            scrollbar: None,
            scroll_offset_rows: 0,
            revision: 1,
            cols: geometry.size.cols,
            rows_visible: geometry.size.rows,
        };
        let _ = self.notify_tx.try_send(());
    }
}

pub(crate) fn duration_from_nanos(nanos: u128) -> Duration {
    let nanos = nanos.min(u64::MAX as u128) as u64;
    Duration::from_nanos(nanos)
}

const PERF_WINDOW: Duration = Duration::from_secs(1);

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

fn average_duration_ms(samples: &VecDeque<(Instant, Duration)>) -> f32 {
    if samples.is_empty() {
        0.0
    } else {
        samples
            .iter()
            .map(|(_, duration)| duration.as_secs_f32())
            .sum::<f32>()
            / samples.len() as f32
            * 1_000.0
    }
}

fn percentile_duration_ms(samples: &VecDeque<(Instant, Duration)>, percentile: f32) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let mut durations = samples
        .iter()
        .map(|(_, duration)| duration.as_secs_f32() * 1_000.0)
        .collect::<Vec<_>>();
    durations.sort_by(|a, b| a.total_cmp(b));
    let index =
        ((durations.len().saturating_sub(1)) as f32 * percentile.clamp(0.0, 1.0)).round() as usize;
    durations[index]
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn perf_snapshot_acknowledges_dirty_state() {
        let (state, _notify_rx) = SharedSessionState::new("test", TerminalGeometry::default());
        {
            let mut perf = state.perf_snapshot.lock().unwrap();
            perf.snapshot.dirty_since_last_ui_frame = true;
            perf.snapshot.terminal.snapshot_seq = 3;
        }

        let first = state.perf_snapshot();
        let second = state.perf_snapshot();

        assert!(first.dirty_since_last_ui_frame);
        assert!(!second.dirty_since_last_ui_frame);
        assert_eq!(second.terminal.snapshot_seq, 3);
    }

    #[test]
    fn duration_average_is_computed_from_nanos() {
        assert_eq!(
            duration_from_nanos(1_500_000),
            Duration::from_nanos(1_500_000)
        );
    }
}
