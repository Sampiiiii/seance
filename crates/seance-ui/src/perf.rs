use std::{
    collections::VecDeque,
    env,
    time::{Duration, Instant},
};

use seance_config::{AppConfig, PerfHudDefault};
use seance_terminal::SessionPerfSnapshot;
use tracing::trace;

const PERF_HISTORY_LIMIT: usize = 120;
pub(crate) const PERF_WINDOW: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum UiPerfMode {
    #[default]
    Off,
    Compact,
    Expanded,
}

impl UiPerfMode {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Off => Self::Compact,
            Self::Compact => Self::Expanded,
            Self::Expanded => Self::Off,
        }
    }

    pub(crate) fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }
}

impl From<PerfHudDefault> for UiPerfMode {
    fn from(value: PerfHudDefault) -> Self {
        match value {
            PerfHudDefault::Off => Self::Off,
            PerfHudDefault::Compact => Self::Compact,
            PerfHudDefault::Expanded => Self::Expanded,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RedrawReason {
    Input,
    TerminalUpdate,
    Palette,
    UiRefresh,
    #[default]
    Unknown,
}

impl RedrawReason {
    pub(crate) fn label(self) -> &'static str {
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
pub(crate) struct FrameStats {
    pub(crate) frame_count_total: u64,
    pub(crate) fps_1s: f32,
    pub(crate) frame_time_last_ms: f32,
    pub(crate) frame_time_avg_ms: f32,
    pub(crate) frame_time_p95_ms: f32,
    pub(crate) present_interval_last_ms: f32,
    pub(crate) present_interval_avg_ms: f32,
    pub(crate) present_interval_p95_ms: f32,
    pub(crate) redraw_reason: RedrawReason,
}

#[derive(Debug)]
pub(crate) struct PerfOverlayState {
    pub(crate) mode: UiPerfMode,
    pub(crate) last_present_timestamp: Option<Instant>,
    pub(crate) present_timestamps: VecDeque<Instant>,
    pub(crate) present_intervals: VecDeque<(Instant, Duration)>,
    pub(crate) render_cost_samples: VecDeque<(Instant, Duration)>,
    pub(crate) ui_refresh_timestamps: VecDeque<Instant>,
    pub(crate) terminal_refresh_timestamps: VecDeque<Instant>,
    pub(crate) active_session_perf_snapshot: Option<SessionPerfSnapshot>,
    pub(crate) frame_stats: FrameStats,
    pub(crate) visible_line_count: usize,
    pub(crate) pending_redraw_reason: RedrawReason,
}

impl PerfOverlayState {
    pub(crate) fn new(mode: UiPerfMode) -> Self {
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

    pub(crate) fn reset_sampling_window(&mut self) {
        self.last_present_timestamp = None;
        self.present_timestamps.clear();
        self.present_intervals.clear();
        self.render_cost_samples.clear();
        self.ui_refresh_timestamps.clear();
        self.terminal_refresh_timestamps.clear();
        self.frame_stats = FrameStats::default();
        self.pending_redraw_reason = RedrawReason::Unknown;
    }

    pub(crate) fn mark_terminal_refresh_request(
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

    pub(crate) fn mark_ui_refresh_request(&mut self, now: Instant, reason: RedrawReason) {
        self.pending_redraw_reason = reason;
        self.ui_refresh_timestamps.push_back(now);
        trim_instants(&mut self.ui_refresh_timestamps, now, PERF_WINDOW);
    }

    pub(crate) fn mark_input(&mut self, reason: RedrawReason) {
        self.pending_redraw_reason = reason;
    }

    pub(crate) fn finish_render(&mut self, started_at: Instant, ended_at: Instant) {
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

    pub(crate) fn ui_refreshes_last_second(&self) -> usize {
        self.ui_refresh_timestamps.len()
    }

    pub(crate) fn terminal_refreshes_last_second(&self) -> usize {
        self.terminal_refresh_timestamps.len()
    }

    pub(crate) fn frames_presented_last_second(&self) -> usize {
        self.present_timestamps.len()
    }

    pub(crate) fn active_session_dirty(&self) -> bool {
        self.active_session_perf_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.dirty_since_last_ui_frame)
    }

    pub(crate) fn vt_bytes_per_second(&self) -> usize {
        self.active_session_perf_snapshot
            .as_ref()
            .map(|snapshot| snapshot.terminal.vt_bytes_processed_since_last_snapshot)
            .unwrap_or(0)
    }
}

pub(crate) fn perf_mode_override_from_env() -> Option<UiPerfMode> {
    match env::var("SEANCE_PERF_HUD") {
        Ok(value) if value.eq_ignore_ascii_case("expanded") => Some(UiPerfMode::Expanded),
        Ok(value)
            if value == "1"
                || value.eq_ignore_ascii_case("true")
                || value.eq_ignore_ascii_case("compact") =>
        {
            Some(UiPerfMode::Compact)
        }
        _ => None,
    }
}

pub(crate) fn perf_mode_from_config(config: &AppConfig) -> UiPerfMode {
    perf_mode_override_from_env().unwrap_or(config.debug.perf_hud_default.into())
}

pub(crate) fn trim_instants(samples: &mut VecDeque<Instant>, now: Instant, window: Duration) {
    while let Some(front) = samples.front().copied() {
        if now.saturating_duration_since(front) <= window {
            break;
        }
        samples.pop_front();
    }
}

pub(crate) fn trim_timed_durations(
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

pub(crate) fn build_frame_stats(
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

pub(crate) fn average_duration_ms(samples: &VecDeque<(Instant, Duration)>) -> f32 {
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

pub(crate) fn normalized_fps_1s(present_timestamps: &VecDeque<Instant>) -> f32 {
    present_timestamps.len() as f32
}

pub(crate) fn percentile_duration_ms(
    samples: &VecDeque<(Instant, Duration)>,
    percentile: f32,
) -> f32 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        time::{Duration, Instant},
    };

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
