// Owns terminal metrics, geometry application, deferred resize scheduling, and terminal surface sync for the workspace.

use std::sync::Arc;
use std::time::Instant;

use gpui::{Context, Window, font, px};
use seance_terminal::TerminalGeometry;
use tracing::trace;

use crate::{
    SeanceWorkspace, TerminalMetrics, TerminalRendererMetrics, perf::RedrawReason,
    terminal_paint::build_terminal_paint_row, ui_components::compute_terminal_geometry,
};

impl SeanceWorkspace {
    pub(crate) fn terminal_metrics(&mut self, window: &Window) -> TerminalMetrics {
        if let Some(metrics) = self.terminal_metrics {
            return metrics;
        }

        let font_family = self.config.terminal.font_family.clone();
        let font_size_px = self.terminal_font_size_px();
        let line_height_px = self.terminal_line_height_px();
        let font_size = px(font_size_px);
        let font_id = window.text_system().resolve_font(&font(font_family));
        let cell_width_px = window
            .text_system()
            .ch_advance(font_id, font_size)
            .map(f32::from)
            .unwrap_or(8.0)
            .ceil()
            .max(1.0);
        let line_height_px = line_height_px.ceil().max(1.0);
        let metrics = TerminalMetrics {
            cell_width_px,
            cell_height_px: line_height_px,
            line_height_px,
            font_size_px,
        };
        trace!(?metrics, "measured terminal metrics");
        self.terminal_metrics = Some(metrics);
        metrics
    }

    pub(crate) fn expected_active_terminal_geometry(
        &mut self,
        window: &Window,
    ) -> Option<TerminalGeometry> {
        self.active_session()?;

        let metrics = self.terminal_metrics(window);
        Some(
            compute_terminal_geometry(window.viewport_size(), metrics, self.sidebar_width)
                .unwrap_or_else(TerminalGeometry::default),
        )
    }

    pub(crate) fn apply_active_terminal_geometry(&mut self, window: &Window) {
        let Some(session) = self.active_session() else {
            self.last_applied_geometry = None;
            self.active_terminal_rows = TerminalGeometry::default().size.rows as usize;
            return;
        };

        let geometry = self
            .expected_active_terminal_geometry(window)
            .unwrap_or_else(TerminalGeometry::default);
        self.active_terminal_rows = geometry.size.rows as usize;

        if self.last_applied_geometry == Some(geometry) {
            trace!(
                ?geometry,
                session_id = session.id(),
                "skipping unchanged UI terminal geometry"
            );
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

    pub(crate) fn schedule_active_terminal_geometry_refresh(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal_resize_epoch = self.terminal_resize_epoch.wrapping_add(1);
        let epoch = self.terminal_resize_epoch;
        trace!(
            epoch,
            active_session_id = self.active_session_id,
            "scheduled deferred terminal geometry refresh"
        );

        cx.on_next_frame(window, move |this, window, cx| {
            this.apply_scheduled_terminal_geometry_refresh(epoch, window, cx);
        });
        cx.notify();
        window.refresh();
    }

    pub(crate) fn apply_scheduled_terminal_geometry_refresh(
        &mut self,
        epoch: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.terminal_resize_epoch != epoch {
            return;
        }

        self.apply_active_terminal_geometry(window);
        self.perf_overlay.mark_input(RedrawReason::TerminalUpdate);
        cx.notify();
        window.refresh();
    }

    pub(crate) fn take_terminal_refresh_request(&mut self) -> bool {
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

    pub(crate) fn invalidate_terminal_surface(&mut self) {
        self.terminal_surface.viewport_revision = 0;
        self.terminal_surface.row_revisions.clear();
        self.terminal_surface.geometry = None;
    }

    pub(crate) fn sync_terminal_surface(&mut self, window: &mut Window) {
        let Some(session) = self.active_session() else {
            self.terminal_surface.rows = Arc::from(Vec::<crate::TerminalPaintRow>::new());
            self.terminal_surface.row_revisions.clear();
            self.terminal_surface.metrics = TerminalRendererMetrics::default();
            self.terminal_surface.active_session_id = 0;
            self.terminal_surface.geometry = None;
            return;
        };

        if let Some(expected_geometry) = self.expected_active_terminal_geometry(window)
            && self.last_applied_geometry != Some(expected_geometry)
        {
            trace!(
                ?expected_geometry,
                last_applied = ?self.last_applied_geometry,
                session_id = session.id(),
                "recovering stale terminal geometry during surface sync"
            );
            self.apply_active_terminal_geometry(window);
        }

        let metrics = self.terminal_metrics(window);
        let geometry = self
            .last_applied_geometry
            .unwrap_or_else(TerminalGeometry::default);
        let viewport = session.viewport_snapshot();
        let full_rebuild = self.terminal_surface.active_session_id != session.id()
            || self.terminal_surface.viewport_revision != viewport.revision
            || self.terminal_surface.geometry != Some(geometry)
            || self.terminal_surface.theme_id != self.active_theme
            || self.terminal_surface.rows.is_empty();

        if !full_rebuild && self.terminal_surface.row_revisions == viewport.row_revisions.as_ref() {
            return;
        }

        let font_family = self.config.terminal.font_family.clone();
        let mut paint_rows = if full_rebuild {
            vec![crate::TerminalPaintRow::default(); viewport.rows.len()]
        } else {
            self.terminal_surface.rows.as_ref().to_vec()
        };
        if paint_rows.len() != viewport.rows.len() {
            paint_rows.resize(viewport.rows.len(), crate::TerminalPaintRow::default());
        }

        let mut metrics_report = TerminalRendererMetrics {
            visible_rows: viewport.rows.len(),
            visible_cells: viewport.rows.len() * geometry.size.cols as usize,
            ..Default::default()
        };

        for (row_index, row) in viewport.rows.iter().enumerate() {
            let row_changed = full_rebuild
                || self.terminal_surface.row_revisions.get(row_index).copied()
                    != viewport.row_revisions.get(row_index).copied();
            if row_changed {
                paint_rows[row_index] = build_terminal_paint_row(
                    row,
                    row_index,
                    geometry.size.cols as usize,
                    metrics,
                    self.active_theme,
                    &self.theme(),
                    font_family.as_str(),
                    &mut self.terminal_surface.shape_cache,
                    window,
                    &mut metrics_report,
                );
                metrics_report.rebuilt_rows += 1;
            } else if let Some(existing) = paint_rows.get(row_index) {
                metrics_report.fragments += existing.fragments.len();
                metrics_report.background_quads +=
                    existing.backgrounds.len() + existing.underlines.len();
            }
        }

        self.terminal_surface.rows = Arc::from(paint_rows);
        self.terminal_surface.metrics = metrics_report;
        self.terminal_surface.active_session_id = session.id();
        self.terminal_surface.viewport_revision = viewport.revision;
        self.terminal_surface.row_revisions = viewport.row_revisions.as_ref().to_vec();
        self.terminal_surface.geometry = Some(geometry);
        self.terminal_surface.theme_id = self.active_theme;
    }
}
