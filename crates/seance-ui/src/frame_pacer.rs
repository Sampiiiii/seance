// Coalesces repaint requests so UI paint cadence is decoupled from terminal
// publish cadence. Every producer of a redraw (terminal watcher, scroll
// coalescer, input handlers, resize, etc.) routes through [`SeanceWorkspace::request_repaint`],
// which accumulates reasons into a single pending flag and defers to the next
// GPUI frame tick. At most one `window.refresh()` / `cx.notify()` pair is
// issued per display frame, regardless of how many publishes or events arrive.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui::{Context, Window};

use crate::{SeanceWorkspace, perf::RedrawReason};

const DEFAULT_TARGET_INTERVAL: Duration = Duration::from_micros(7_500);
const MIN_TARGET_INTERVAL: Duration = Duration::from_micros(3_500);
const MAX_TARGET_INTERVAL: Duration = Duration::from_micros(16_800);
const MAX_FRAME_DEFERS: u8 = 2;

// Pacer self-calibration: each `flush_frame_pacer` callback is delivered on a
// display tick (either a flush or a defer). We record deltas between
// consecutive ticks and, after accumulating enough samples, derive the display
// Hz and update `target_interval`. This lets the pacer track ProMotion /
// fullscreen transitions without requiring the HUD probe to be active, and
// costs nothing when the app is idle (no ticks, no samples).
const CALIBRATION_SAMPLE_CAPACITY: usize = 16;
const CALIBRATION_MIN_SAMPLES: usize = 6;
const CALIBRATION_MAX_GAP: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RepaintReasonSet(u8);

impl RepaintReasonSet {
    pub(crate) const TERMINAL_UPDATE: Self = Self(1 << 0);
    pub(crate) const SCROLL: Self = Self(1 << 1);
    pub(crate) const INPUT: Self = Self(1 << 2);
    pub(crate) const UI_STATE: Self = Self(1 << 3);
    pub(crate) const PALETTE: Self = Self(1 << 4);
    pub(crate) const DISPLAY_PROBE: Self = Self(1 << 5);

    pub(crate) fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub(crate) fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0 && other.0 != 0
    }

    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub(crate) fn clear(&mut self) {
        self.0 = 0;
    }

    /// Collapses the accumulated reasons to a single `RedrawReason` in priority
    /// order so the perf HUD still reports the dominant cause of the repaint.
    pub(crate) fn dominant_reason(self) -> RedrawReason {
        if self.contains(Self::TERMINAL_UPDATE) {
            RedrawReason::TerminalUpdate
        } else if self.contains(Self::SCROLL) {
            RedrawReason::TerminalUpdate
        } else if self.contains(Self::PALETTE) {
            RedrawReason::Palette
        } else if self.contains(Self::INPUT) {
            RedrawReason::Input
        } else if self.contains(Self::UI_STATE) {
            RedrawReason::UiRefresh
        } else if self.contains(Self::DISPLAY_PROBE) {
            RedrawReason::DisplayProbe
        } else {
            RedrawReason::Unknown
        }
    }
}

#[derive(Debug)]
pub(crate) struct FramePacer {
    pub(crate) pending: RepaintReasonSet,
    pub(crate) frame_scheduled: bool,
    pub(crate) last_flush_at: Option<Instant>,
    pub(crate) target_interval: Duration,
    pub(crate) requests_total: u64,
    pub(crate) coalesced_total: u64,
    pub(crate) flushes_total: u64,
    pub(crate) defers_total: u64,
    pub(crate) pending_defers: u8,
    pub(crate) requests_window: u64,
    pub(crate) flushes_window: u64,
    pub(crate) defers_window: u64,
    pub(crate) window_started_at: Option<Instant>,
    pub(crate) calibration_last_tick_at: Option<Instant>,
    pub(crate) calibration_samples: VecDeque<Duration>,
    pub(crate) calibrated_display_hz: Option<f32>,
}

impl Default for FramePacer {
    fn default() -> Self {
        Self {
            pending: RepaintReasonSet::default(),
            frame_scheduled: false,
            last_flush_at: None,
            target_interval: DEFAULT_TARGET_INTERVAL,
            requests_total: 0,
            coalesced_total: 0,
            flushes_total: 0,
            defers_total: 0,
            pending_defers: 0,
            requests_window: 0,
            flushes_window: 0,
            defers_window: 0,
            window_started_at: None,
            calibration_last_tick_at: None,
            calibration_samples: VecDeque::with_capacity(CALIBRATION_SAMPLE_CAPACITY),
            calibrated_display_hz: None,
        }
    }
}

impl FramePacer {
    pub(crate) fn note_request(&mut self, reason: RepaintReasonSet, now: Instant) {
        self.pending.insert(reason);
        self.requests_total = self.requests_total.saturating_add(1);
        self.bump_window(now);
        self.requests_window = self.requests_window.saturating_add(1);
    }

    pub(crate) fn note_flush(&mut self, now: Instant) {
        self.flushes_total = self.flushes_total.saturating_add(1);
        self.flushes_window = self.flushes_window.saturating_add(1);
        self.last_flush_at = Some(now);
        self.bump_window(now);
    }

    pub(crate) fn note_defer(&mut self, now: Instant) {
        self.defers_total = self.defers_total.saturating_add(1);
        self.defers_window = self.defers_window.saturating_add(1);
        self.bump_window(now);
    }

    pub(crate) fn note_coalesce(&mut self) {
        self.coalesced_total = self.coalesced_total.saturating_add(1);
    }

    /// Records a display-tick arrival time (either a flush or a deferred
    /// callback). When enough samples have been collected, updates
    /// `target_interval` to match the observed display Hz. Returns the
    /// inferred display Hz if a new calibration landed.
    pub(crate) fn record_display_tick(&mut self, now: Instant) -> Option<f32> {
        let previous = self.calibration_last_tick_at.replace(now);
        let Some(prev) = previous else {
            return None;
        };

        let delta = now.saturating_duration_since(prev);
        // Reject gaps caused by idle pauses -- we only want samples from
        // steady-state display ticks (back-to-back callbacks).
        if delta.is_zero() || delta > CALIBRATION_MAX_GAP {
            self.calibration_samples.clear();
            return None;
        }

        if self.calibration_samples.len() == CALIBRATION_SAMPLE_CAPACITY {
            self.calibration_samples.pop_front();
        }
        self.calibration_samples.push_back(delta);

        if self.calibration_samples.len() < CALIBRATION_MIN_SAMPLES {
            return None;
        }

        let total: Duration = self.calibration_samples.iter().copied().sum();
        let count = self.calibration_samples.len() as u32;
        let avg = total / count;
        let avg_secs = avg.as_secs_f32();
        if avg_secs <= 0.0 {
            return None;
        }
        let hz = (1.0_f32 / avg_secs).round();
        if !hz.is_finite() || hz < 24.0 {
            return None;
        }

        // Only publish when the inferred Hz shifts meaningfully (e.g. the
        // 60 -> 120 ProMotion transition). Avoids flapping the target interval
        // on every sample.
        let changed = match self.calibrated_display_hz {
            Some(prev_hz) => (prev_hz - hz).abs() >= 8.0,
            None => true,
        };
        if !changed {
            return None;
        }
        self.calibrated_display_hz = Some(hz);
        self.set_target_hz(hz);
        Some(hz)
    }

    pub(crate) fn set_target_hz(&mut self, display_hz: f32) {
        if !display_hz.is_finite() || display_hz < 24.0 {
            self.target_interval = DEFAULT_TARGET_INTERVAL;
            return;
        }
        let interval = Duration::from_secs_f32(1.0 / display_hz);
        self.target_interval = interval.clamp(MIN_TARGET_INTERVAL, MAX_TARGET_INTERVAL);
    }

    pub(crate) fn should_defer(&self, now: Instant) -> bool {
        if self.pending_defers >= MAX_FRAME_DEFERS {
            return false;
        }
        let Some(last) = self.last_flush_at else {
            return false;
        };
        now.saturating_duration_since(last) < self.target_interval
    }

    pub(crate) fn requests_per_second_window(&self) -> f32 {
        self.window_rate(self.requests_window)
    }

    pub(crate) fn flushes_per_second_window(&self) -> f32 {
        self.window_rate(self.flushes_window)
    }

    pub(crate) fn defers_per_second_window(&self) -> f32 {
        self.window_rate(self.defers_window)
    }

    fn window_rate(&self, count: u64) -> f32 {
        let Some(started) = self.window_started_at else {
            return 0.0;
        };
        let elapsed = started.elapsed().as_secs_f32().max(0.25);
        count as f32 / elapsed
    }

    fn bump_window(&mut self, now: Instant) {
        let started = self.window_started_at.get_or_insert(now);
        if now.saturating_duration_since(*started) > Duration::from_secs(1) {
            *started = now;
            self.requests_window = 0;
            self.flushes_window = 0;
            self.defers_window = 0;
        }
    }
}

impl SeanceWorkspace {
    /// Records a repaint intent and schedules a single coalesced frame tick.
    ///
    /// All producers of "please redraw" -- terminal watcher, scroll, input
    /// handlers, resize, watchers -- should call this instead of invoking
    /// `cx.notify()` / `window.refresh()` directly. The pacer guarantees at
    /// most one actual refresh is issued per display frame while still marking
    /// the HUD perf state with the dominant reason.
    pub(crate) fn request_repaint(
        &mut self,
        reason: RepaintReasonSet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        if !self.frame_pacer.pending.is_empty() && self.frame_pacer.frame_scheduled {
            self.frame_pacer.note_coalesce();
        }
        self.frame_pacer.note_request(reason, now);
        self.mark_perf_reason(reason, now);

        if self.frame_pacer.frame_scheduled {
            return;
        }
        self.frame_pacer.frame_scheduled = true;
        self.frame_pacer.pending_defers = 0;
        cx.on_next_frame(window, move |this, window, cx| {
            this.flush_frame_pacer(window, cx);
        });
    }

    pub(crate) fn flush_frame_pacer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        // Every invocation of this callback lands on a display tick (GPUI
        // delivers it via `on_next_frame`), so using these timestamps to
        // calibrate the pacer tracks the real display cadence -- including
        // fullscreen / ProMotion transitions -- without any HUD-side probe.
        self.frame_pacer.record_display_tick(now);

        if self.frame_pacer.should_defer(now) {
            self.frame_pacer.pending_defers = self.frame_pacer.pending_defers.saturating_add(1);
            self.frame_pacer.note_defer(now);
            cx.on_next_frame(window, move |this, window, cx| {
                this.flush_frame_pacer(window, cx);
            });
            return;
        }

        self.frame_pacer.frame_scheduled = false;
        self.frame_pacer.pending_defers = 0;

        if self.frame_pacer.pending.is_empty() {
            return;
        }

        let reason = self.frame_pacer.pending.dominant_reason();
        self.frame_pacer.pending.clear();
        self.frame_pacer.note_flush(now);
        self.mark_perf_refresh(reason, now);

        cx.notify();
        window.refresh();
    }

    fn mark_perf_reason(&mut self, reason: RepaintReasonSet, _now: Instant) {
        let label = reason.dominant_reason();
        if matches!(label, RedrawReason::DisplayProbe) {
            self.perf_overlay.mark_display_probe();
        } else {
            self.perf_overlay.mark_input(label);
        }
    }

    fn mark_perf_refresh(&mut self, reason: RedrawReason, now: Instant) {
        match reason {
            RedrawReason::TerminalUpdate => {
                let snapshot = self.active_session().map(|session| session.perf_snapshot());
                self.perf_overlay
                    .mark_terminal_refresh_request(now, reason, snapshot);
            }
            RedrawReason::DisplayProbe => {
                // Display probes drive their own cadence; do not inflate the
                // presented-fps counter.
                self.perf_overlay.mark_display_probe();
            }
            other => {
                self.perf_overlay.mark_ui_refresh_request(now, other);
            }
        }
    }

    pub(crate) fn apply_display_hz_hint(&mut self, display_hz: f32) {
        self.frame_pacer.set_target_hz(display_hz);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_collapse_to_expected_dominant_label() {
        let mut set = RepaintReasonSet::default();
        set.insert(RepaintReasonSet::INPUT);
        assert_eq!(set.dominant_reason(), RedrawReason::Input);

        set.insert(RepaintReasonSet::TERMINAL_UPDATE);
        assert_eq!(set.dominant_reason(), RedrawReason::TerminalUpdate);

        set.insert(RepaintReasonSet::PALETTE);
        // Terminal updates outrank palette repaints.
        assert_eq!(set.dominant_reason(), RedrawReason::TerminalUpdate);
    }

    #[test]
    fn contains_handles_empty_sets() {
        let set = RepaintReasonSet::default();
        assert!(!set.contains(RepaintReasonSet::INPUT));
    }

    #[test]
    fn should_defer_requires_pending_and_recent_flush() {
        let mut pacer = FramePacer::default();
        let now = Instant::now();
        assert!(!pacer.should_defer(now));

        pacer.note_flush(now);
        pacer.target_interval = Duration::from_millis(8);
        assert!(pacer.should_defer(now));

        pacer.pending_defers = MAX_FRAME_DEFERS;
        assert!(!pacer.should_defer(now));
    }

    #[test]
    fn record_display_tick_calibrates_after_enough_samples() {
        let mut pacer = FramePacer::default();
        let start = Instant::now();
        // Simulate a 120 Hz display: ~8.333 ms between ticks.
        let tick = Duration::from_micros(8_333);

        let mut t = start;
        let mut calibrated = None;
        for _ in 0..CALIBRATION_MIN_SAMPLES + 2 {
            t += tick;
            if let Some(hz) = pacer.record_display_tick(t) {
                calibrated = Some(hz);
            }
        }
        let hz = calibrated.expect("calibration should publish a rate");
        assert!((hz - 120.0).abs() < 2.0, "expected ~120 Hz, got {hz}");
        assert!(pacer.target_interval <= Duration::from_micros(8_500));
    }

    #[test]
    fn record_display_tick_resets_on_idle_gap() {
        let mut pacer = FramePacer::default();
        let start = Instant::now();
        let tick = Duration::from_micros(8_333);
        let mut t = start;
        for _ in 0..4 {
            t += tick;
            pacer.record_display_tick(t);
        }
        // First call seeds `calibration_last_tick_at`; subsequent calls
        // produce N-1 samples.
        assert_eq!(pacer.calibration_samples.len(), 3);
        // Simulate an idle pause that exceeds the calibration gap threshold.
        t += CALIBRATION_MAX_GAP + Duration::from_millis(50);
        pacer.record_display_tick(t);
        assert!(pacer.calibration_samples.is_empty());
    }

    #[test]
    fn target_hz_clamped_to_bounds() {
        let mut pacer = FramePacer::default();
        pacer.set_target_hz(500.0);
        assert_eq!(pacer.target_interval, MIN_TARGET_INTERVAL);

        pacer.set_target_hz(30.0);
        assert_eq!(pacer.target_interval, MAX_TARGET_INTERVAL);

        pacer.set_target_hz(0.0);
        assert_eq!(pacer.target_interval, DEFAULT_TARGET_INTERVAL);
    }
}
