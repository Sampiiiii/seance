// Owns perf HUD mode transitions and the HUD-only display cadence probe loop.

use std::time::Instant;

use gpui::{Context, Window};

use crate::{
    RepaintReasonSet, SeanceWorkspace,
    perf::UiPerfMode,
};

impl SeanceWorkspace {
    pub(crate) fn apply_perf_mode(
        &mut self,
        next_mode: UiPerfMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous_mode = self.perf_overlay.mode;

        if previous_mode == next_mode {
            if next_mode.is_enabled() {
                self.ensure_display_probe(window, cx);
            }
            self.request_repaint(RepaintReasonSet::UI_STATE, window, cx);
            return;
        }

        if !previous_mode.is_enabled() && next_mode.is_enabled() {
            self.perf_overlay.reset_sampling_window();
            self.perf_overlay.start_display_probe();
        } else if previous_mode.is_enabled() && !next_mode.is_enabled() {
            self.perf_overlay.stop_display_probe();
            self.perf_overlay.reset_sampling_window();
        } else if next_mode.is_enabled() && !self.perf_overlay.display_probe_enabled {
            self.perf_overlay.start_display_probe();
        }

        self.perf_overlay.mode = next_mode;
        if next_mode.is_enabled() {
            self.ensure_display_probe(window, cx);
        }

        self.request_repaint(RepaintReasonSet::UI_STATE, window, cx);
    }

    pub(crate) fn ensure_display_probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.perf_overlay.mode.is_enabled() {
            return;
        }

        if !self.perf_overlay.display_probe_enabled {
            self.perf_overlay.start_display_probe();
        }

        let Some(epoch) = self.perf_overlay.schedule_display_probe_callback() else {
            return;
        };

        cx.on_next_frame(window, move |this, window, cx| {
            this.sample_display_probe(epoch, window, cx);
        });
        self.perf_overlay.mark_display_probe();
        window.refresh();
    }

    pub(crate) fn sample_display_probe(
        &mut self,
        epoch: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.perf_overlay.begin_display_probe_callback(epoch) {
            return;
        }

        self.perf_overlay.record_display_probe(Instant::now());
        let display_hz = self.perf_overlay.frame_stats.display_hz_1s;
        if display_hz > 0.0 {
            self.apply_display_hz_hint(display_hz);
        }
        if self.perf_overlay.mode.is_enabled() {
            self.ensure_display_probe(window, cx);
        }
    }
}
