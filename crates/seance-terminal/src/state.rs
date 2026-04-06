use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

use anyhow::Result;
use tracing::trace;

use crate::model::{TerminalCell, TerminalCellStyle, TerminalGeometry, TerminalRow};

static SESSION_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Returns a unique session id for any terminal session (local shell, SSH, etc.).
pub fn next_session_id() -> u64 {
    SESSION_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Debug, Default)]
pub struct SessionSnapshot {
    pub rows: Vec<TerminalRow>,
    pub exit_status: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct TerminalRenderMetrics {
    pub snapshot_seq: u64,
    pub last_snapshot_duration: Duration,
    pub avg_snapshot_duration: Duration,
    pub max_snapshot_duration: Duration,
    pub rendered_row_count: usize,
    pub rendered_cell_count: usize,
    pub truncated_row_count: usize,
    pub vt_bytes_processed_since_last_snapshot: usize,
    pub total_vt_bytes_processed: u64,
}

#[derive(Clone, Debug, Default)]
pub struct SessionPerfSnapshot {
    pub terminal: TerminalRenderMetrics,
    pub dirty_since_last_ui_frame: bool,
}

pub trait TerminalSession: Send + Sync {
    fn id(&self) -> u64;
    fn title(&self) -> &str;
    fn snapshot(&self) -> SessionSnapshot;
    fn send_input(&self, bytes: Vec<u8>) -> Result<()>;
    fn resize(&self, geometry: TerminalGeometry) -> Result<()>;
    fn perf_snapshot(&self) -> SessionPerfSnapshot;
    fn take_notify_rx(&self) -> Option<mpsc::Receiver<()>>;
}

#[derive(Debug, Default)]
pub(crate) struct SessionPerfState {
    pub(crate) snapshot: SessionPerfSnapshot,
    snapshot_samples: u64,
    total_snapshot_duration_ns: u128,
}

#[derive(Debug)]
pub(crate) struct RenderedSnapshot {
    pub(crate) rows: Vec<TerminalRow>,
    pub(crate) rendered_cell_count: usize,
    pub(crate) truncated_row_count: usize,
}

#[derive(Clone)]
pub struct SharedSessionState {
    snapshot: Arc<Mutex<SessionSnapshot>>,
    pub(crate) perf_snapshot: Arc<Mutex<SessionPerfState>>,
    notify_tx: mpsc::SyncSender<()>,
}

impl SharedSessionState {
    pub fn new(initial_message: impl Into<String>) -> (Self, mpsc::Receiver<()>) {
        let (notify_tx, notify_rx) = mpsc::sync_channel(1);
        let state = Self {
            snapshot: Arc::new(Mutex::new(SessionSnapshot {
                rows: vec![TerminalRow {
                    cells: vec![TerminalCell {
                        text: initial_message.into(),
                        style: TerminalCellStyle::default(),
                        width: 1,
                    }],
                }],
                exit_status: None,
            })),
            perf_snapshot: Arc::new(Mutex::new(SessionPerfState::default())),
            notify_tx,
        };
        (state, notify_rx)
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        self.snapshot
            .lock()
            .expect("session snapshot poisoned")
            .clone()
    }

    pub fn perf_snapshot(&self) -> SessionPerfSnapshot {
        let mut perf = self.perf_snapshot.lock().expect("session perf poisoned");
        let snapshot = perf.snapshot.clone();
        perf.snapshot.dirty_since_last_ui_frame = false;
        snapshot
    }

    pub(crate) fn publish_render(
        &self,
        rendered_snapshot: RenderedSnapshot,
        duration: Duration,
        vt_bytes_processed_since_last_snapshot: usize,
        exit_status: Option<String>,
    ) {
        let RenderedSnapshot {
            rows,
            rendered_cell_count,
            truncated_row_count,
        } = rendered_snapshot;
        let rendered_row_count = rows.len();

        let mut state = self.snapshot.lock().expect("session snapshot poisoned");
        state.rows = rows;
        if let Some(exit_status) = exit_status {
            state.exit_status = Some(exit_status);
        }

        let mut perf = self.perf_snapshot.lock().expect("session perf poisoned");
        perf.snapshot_samples = perf.snapshot_samples.saturating_add(1);
        perf.total_snapshot_duration_ns = perf
            .total_snapshot_duration_ns
            .saturating_add(duration.as_nanos());
        let snapshot_samples = perf.snapshot_samples;
        let avg_snapshot_duration =
            duration_from_nanos(perf.total_snapshot_duration_ns / u128::from(snapshot_samples));

        {
            let metrics = &mut perf.snapshot.terminal;
            metrics.snapshot_seq = snapshot_samples;
            metrics.last_snapshot_duration = duration;
            metrics.avg_snapshot_duration = avg_snapshot_duration;
            metrics.max_snapshot_duration = metrics.max_snapshot_duration.max(duration);
            metrics.rendered_row_count = rendered_row_count;
            metrics.rendered_cell_count = rendered_cell_count;
            metrics.truncated_row_count = truncated_row_count;
            metrics.vt_bytes_processed_since_last_snapshot = vt_bytes_processed_since_last_snapshot;
            metrics.total_vt_bytes_processed = metrics
                .total_vt_bytes_processed
                .saturating_add(vt_bytes_processed_since_last_snapshot as u64);
        }
        perf.snapshot.dirty_since_last_ui_frame = true;

        let metrics = &perf.snapshot.terminal;

        trace!(
            snapshot_seq = metrics.snapshot_seq,
            snapshot_ms = duration.as_secs_f64() * 1_000.0,
            rendered_row_count = metrics.rendered_row_count,
            rendered_cell_count = metrics.rendered_cell_count,
            truncated_row_count = metrics.truncated_row_count,
            vt_bytes_processed_since_last_snapshot,
            "published terminal snapshot"
        );

        let _ = self.notify_tx.try_send(());
    }

    pub fn set_error(&self, error: &anyhow::Error) {
        let mut state = self.snapshot.lock().expect("session snapshot poisoned");
        state.rows = vec![TerminalRow {
            cells: vec![TerminalCell {
                text: format!("Failed to start session: {error:#}"),
                style: TerminalCellStyle::default(),
                width: 1,
            }],
        }];
        state.exit_status = Some("startup error".to_string());
        let _ = self.notify_tx.try_send(());
    }
}

pub(crate) fn duration_from_nanos(nanos: u128) -> Duration {
    let nanos = nanos.min(u64::MAX as u128) as u64;
    Duration::from_nanos(nanos)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn perf_snapshot_acknowledges_dirty_state() {
        let (state, _notify_rx) = SharedSessionState::new("test");
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
