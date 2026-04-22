// Owns terminal metrics, geometry application, deferred resize scheduling, and terminal surface sync for the workspace.

use std::sync::Arc;
use std::time::Instant;

use gpui::{Context, Window, font, px};
use seance_observability::{
    RENDER_TRACE_TARGET, RenderCause, RenderDomain, RenderPath, RenderPhase, RenderTraceScope,
};
use seance_terminal::TerminalGeometry;
use std::hash::{Hash, Hasher};
use tracing::{trace, warn};

use crate::{
    LinkPaintMode, RepaintReasonSet, SeanceWorkspace, TerminalMetrics, TerminalRendererMetrics,
    perf::RedrawReason,
    terminal_paint::{
        build_row_paint_template, row_paint_cache_get, row_paint_cache_insert, row_paint_cache_key,
    },
    ui_components::compute_terminal_geometry,
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
            .max(1.0);
        let line_height_px = line_height_px.max(1.0);
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
        let geometry_trace = RenderTraceScope::new(
            RenderDomain::Ui,
            RenderPath::TerminalGeometryRefresh,
            RenderCause::Input,
        );
        let _apply_phase = geometry_trace.phase(RenderPhase::Apply);
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
            warn!(
                ?geometry,
                session_id = session.id(),
                error = %error,
                "failed to apply terminal geometry; invalidating surface to avoid stale paint"
            );
            // Even though the session rejected the resize, GPUI is already
            // committed to the new bounds. Leave `last_applied_geometry` set
            // to the requested geometry so we don't busy-retry every frame,
            // and throw away the old paint state so the next sync doesn't
            // render rows below the prompt at stale metrics.
            self.last_applied_geometry = Some(geometry);
            self.invalidate_terminal_surface();
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
        let geometry_trace = RenderTraceScope::new(
            RenderDomain::Ui,
            RenderPath::TerminalGeometryRefresh,
            RenderCause::Input,
        );
        let _schedule_phase = geometry_trace.phase(RenderPhase::Schedule);
        self.terminal_resize_epoch = self.terminal_resize_epoch.wrapping_add(1);
        let epoch = self.terminal_resize_epoch;
        trace!(
            epoch,
            active_session_id = self.active_session_id,
            "scheduled deferred terminal geometry refresh"
        );
        trace!(
            target: RENDER_TRACE_TARGET,
            render_domain = RenderDomain::Ui.as_str(),
            render_path = RenderPath::TerminalGeometryRefresh.as_str(),
            render_cause = RenderCause::Input.as_str(),
            render_phase = RenderPhase::Schedule.as_str(),
            epoch,
            active_session_id = self.active_session_id,
            "scheduled deferred terminal geometry refresh"
        );

        cx.on_next_frame(window, move |this, window, cx| {
            this.apply_scheduled_terminal_geometry_refresh(epoch, window, cx);
        });
        self.request_repaint(RepaintReasonSet::TERMINAL_UPDATE, window, cx);
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
        self.request_repaint(RepaintReasonSet::TERMINAL_UPDATE, window, cx);
    }

    pub(crate) fn take_terminal_refresh_request(&mut self) -> bool {
        let refresh_trace = RenderTraceScope::new(
            RenderDomain::Ui,
            RenderPath::TerminalRefreshRequest,
            RenderCause::TerminalUpdate,
        );
        let Some(session) = self.active_session() else {
            return false;
        };

        let session_perf = session.perf_snapshot();
        self.perf_overlay.active_session_perf_snapshot = Some(session_perf.clone());
        if !session_perf.dirty_since_last_ui_frame {
            let _fast_path = refresh_trace.phase(RenderPhase::FastPath);
            trace!(
                target: RENDER_TRACE_TARGET,
                render_domain = RenderDomain::Ui.as_str(),
                render_path = RenderPath::TerminalRefreshRequest.as_str(),
                render_cause = RenderCause::TerminalUpdate.as_str(),
                render_phase = RenderPhase::FastPath.as_str(),
                session_id = session.id(),
                dirty_since_last_ui_frame = false,
                "skipped terminal refresh request"
            );
            return false;
        }

        self.perf_overlay.mark_terminal_refresh_request(
            Instant::now(),
            RedrawReason::TerminalUpdate,
            Some(session_perf),
        );
        trace!(
            target: RENDER_TRACE_TARGET,
            render_domain = RenderDomain::Ui.as_str(),
            render_path = RenderPath::TerminalRefreshRequest.as_str(),
            render_cause = RenderCause::TerminalUpdate.as_str(),
            render_phase = RenderPhase::Summary.as_str(),
            session_id = session.id(),
            "accepted terminal refresh request"
        );
        true
    }

    pub(crate) fn invalidate_terminal_surface(&mut self) {
        // Clear everything the partial-rebuild path keys off so the next
        // `sync_terminal_surface` enters the `full_rebuild` branch and can't
        // pull rows materialized at old metrics into `rows_scratch`. See
        // `TerminalSurfaceState::mark_invalidated` for the list of fields.
        self.terminal_surface.mark_invalidated();
    }

    /// Targeted invalidation used when we just need to re-run link detection
    /// on already-rendered rows. Rather than discarding the entire surface we
    /// bump the revision of rows that currently carry link ranges (or had link
    /// paint deferred this frame) so only those get rebuilt on the next sync.
    pub(crate) fn invalidate_terminal_link_rows(&mut self) {
        if self.terminal_surface.row_revisions.is_empty() {
            return;
        }

        self.terminal_surface.link_paint_mode = LinkPaintMode::Normal;

        let rows = self.terminal_surface.rows.clone();
        for (index, revision) in self.terminal_surface.row_revisions.iter_mut().enumerate() {
            let needs_rebuild = rows
                .get(index)
                .map(|row| {
                    !row.link_ranges.is_empty()
                        || !row.link_highlights.is_empty()
                        || !row.link_underlines.is_empty()
                })
                .unwrap_or(true);
            if needs_rebuild {
                *revision = revision.wrapping_add(1).max(1);
            }
        }
    }

    pub(crate) fn sync_terminal_surface(&mut self, window: &mut Window) {
        let render_cause = self.perf_overlay.pending_render_cause();
        let surface_trace = RenderTraceScope::new(
            RenderDomain::Terminal,
            RenderPath::TerminalSurfaceSync,
            render_cause,
        );
        let Some(session) = self.active_session() else {
            self.terminal_surface.rows = Arc::from(Vec::<crate::TerminalPaintRow>::new());
            self.terminal_surface.row_revisions.clear();
            self.terminal_surface.cursor = None;
            self.terminal_surface.scrollbar = None;
            self.terminal_surface.metrics = TerminalRendererMetrics::default();
            self.terminal_surface.active_session_id = 0;
            self.terminal_surface.geometry = None;
            self.terminal_surface.link_paint_mode = LinkPaintMode::Normal;
            self.terminal_hovered_link = None;
            self.terminal_scrollbar_hovered = false;
            self.terminal_scrollbar_drag = None;
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
        let metrics_fingerprint = terminal_metrics_fingerprint(&metrics);
        let geometry = self
            .last_applied_geometry
            .unwrap_or_else(TerminalGeometry::default);
        let viewport = session.viewport_snapshot();
        let link_paint_mode = self.terminal_link_paint_mode(Instant::now());
        {
            let _reconcile_phase = surface_trace.phase(RenderPhase::Reconcile);
            if matches!(link_paint_mode, LinkPaintMode::Deferred) {
                self.terminal_hovered_link = None;
            } else {
                self.reconcile_terminal_hovered_link(
                    &viewport,
                    session.summary().active_screen,
                    session.summary().mouse_tracking,
                    window.modifiers(),
                );
            }
        }
        let rows_unchanged = self.terminal_surface.row_revisions == viewport.row_revisions.as_ref();
        let full_rebuild = self.terminal_surface.active_session_id != session.id()
            || self.terminal_surface.geometry != Some(geometry)
            || self.terminal_surface.theme_id != self.active_theme
            || self.terminal_surface.link_paint_mode != link_paint_mode
            || self.terminal_surface.metrics_fingerprint != metrics_fingerprint
            || self.terminal_surface.rows.is_empty();
        if !full_rebuild && rows_unchanged {
            let _fast_path = surface_trace.phase(RenderPhase::FastPath);
            self.terminal_surface.active_session_id = session.id();
            self.terminal_surface.viewport_revision = viewport.revision;
            self.terminal_surface.geometry = Some(geometry);
            self.terminal_surface.theme_id = self.active_theme;
            self.terminal_surface.link_paint_mode = link_paint_mode;
            self.terminal_surface.metrics.scroll_batches_dispatched =
                self.terminal_scroll.scroll_batches_dispatched;
            self.terminal_surface.cursor = viewport.cursor;
            self.terminal_surface.scrollbar = viewport.scrollbar;
            if self.terminal_surface.scrollbar.is_none() {
                self.terminal_scrollbar_hovered = false;
                self.terminal_scrollbar_drag = None;
            }
            trace!(
                target: RENDER_TRACE_TARGET,
                render_domain = RenderDomain::Terminal.as_str(),
                render_path = RenderPath::TerminalSurfaceSync.as_str(),
                render_cause = render_cause.as_str(),
                render_phase = RenderPhase::FastPath.as_str(),
                session_id = session.id(),
                viewport_revision = viewport.revision,
                "reused existing terminal surface"
            );
            return;
        }

        let font_family = self.config.terminal.font_family.clone();
        let theme = self.theme();
        let target_rows = viewport.rows.len();
        {
            let paint_rows = &mut self.terminal_surface.rows_scratch;
            if full_rebuild {
                paint_rows.clear();
                paint_rows.resize(target_rows, crate::TerminalPaintRow::default());
            } else {
                paint_rows.truncate(target_rows);
                let have = paint_rows.len();
                if have < target_rows {
                    // Always push fresh defaults for grow slots. The row
                    // revision compare below forces a rebuild for these
                    // indices this same frame, so the placeholder is only
                    // visible for <1 frame worst case and we never reuse
                    // rows materialized at old metrics.
                    paint_rows.resize(target_rows, crate::TerminalPaintRow::default());
                }
            }
        }

        let mut metrics_report = TerminalRendererMetrics {
            visible_rows: viewport.rows.len(),
            visible_cells: viewport.rows.len() * geometry.size.cols as usize,
            scroll_batches_dispatched: self.terminal_scroll.scroll_batches_dispatched,
            ..Default::default()
        };

        {
            let _row_phase = surface_trace.phase(RenderPhase::IterateRows);
            let active_theme = self.active_theme;
            let row_revisions = &self.terminal_surface.row_revisions;
            let row_template_cache = &mut self.terminal_surface.row_template_cache;
            let shape_cache = &mut self.terminal_surface.shape_cache;
            let paint_rows = &mut self.terminal_surface.rows_scratch;

            for (row_index, row) in viewport.rows.iter().enumerate() {
                let row_changed = full_rebuild
                    || row_revisions.get(row_index).copied()
                        != viewport.row_revisions.get(row_index).copied();
                if row_changed {
                    let _cache_lookup_phase = surface_trace.phase(RenderPhase::CacheLookup);
                    let cache_key = row_paint_cache_key(
                        row,
                        geometry.size.cols as usize,
                        metrics,
                        active_theme,
                        font_family.as_str(),
                        link_paint_mode,
                    );
                    let template = if let Some(template) =
                        row_paint_cache_get(row_template_cache, &cache_key)
                    {
                        metrics_report.row_cache_hits =
                            metrics_report.row_cache_hits.saturating_add(1);
                        template
                    } else {
                        metrics_report.row_cache_misses =
                            metrics_report.row_cache_misses.saturating_add(1);
                        let template = build_row_paint_template(
                            row,
                            geometry.size.cols as usize,
                            metrics,
                            active_theme,
                            &theme,
                            font_family.as_str(),
                            shape_cache,
                            link_paint_mode,
                            render_cause,
                            window,
                            &mut metrics_report,
                        );
                        row_paint_cache_insert(row_template_cache, cache_key, template.clone());
                        template
                    };
                    paint_rows[row_index] = template.materialize(row_index, metrics.line_height_px);
                    metrics_report.rebuilt_rows += 1;
                } else if let Some(existing) = paint_rows.get(row_index) {
                    metrics_report.fragments += existing.fragments.len();
                    metrics_report.background_quads += existing.backgrounds.len()
                        + existing.link_highlights.len()
                        + existing.underlines.len()
                        + existing.link_underlines.len();
                }
            }
        }

        self.terminal_surface.rows = Arc::from(self.terminal_surface.rows_scratch.as_slice());
        self.terminal_surface.metrics = metrics_report;
        self.terminal_surface.metrics_fingerprint = metrics_fingerprint;
        self.terminal_surface.active_session_id = session.id();
        self.terminal_surface.viewport_revision = viewport.revision;
        self.terminal_surface.row_revisions = viewport.row_revisions.as_ref().to_vec();
        self.terminal_surface.geometry = Some(geometry);
        self.terminal_surface.theme_id = self.active_theme;
        self.terminal_surface.link_paint_mode = link_paint_mode;
        self.terminal_surface.cursor = viewport.cursor;
        self.terminal_surface.scrollbar = viewport.scrollbar;
        if self.terminal_surface.scrollbar.is_none() {
            self.terminal_scrollbar_hovered = false;
            self.terminal_scrollbar_drag = None;
        }
        if self.terminal_surface.rows.is_empty() {
            self.terminal_hovered_link = None;
        }

        trace!(
            target: RENDER_TRACE_TARGET,
            render_domain = RenderDomain::Terminal.as_str(),
            render_path = RenderPath::TerminalSurfaceSync.as_str(),
            render_cause = render_cause.as_str(),
            render_phase = RenderPhase::Summary.as_str(),
            session_id = session.id(),
            visible_rows = self.terminal_surface.metrics.visible_rows,
            rebuilt_rows = self.terminal_surface.metrics.rebuilt_rows,
            fragments = self.terminal_surface.metrics.fragments,
            row_cache_hits = self.terminal_surface.metrics.row_cache_hits,
            row_cache_misses = self.terminal_surface.metrics.row_cache_misses,
            link_rows_deferred = self.terminal_surface.metrics.link_rows_deferred,
            scroll_batches_dispatched = self.terminal_surface.metrics.scroll_batches_dispatched,
            "terminal surface sync summary"
        );
    }
}

/// Cheap stable fingerprint of the rendering metrics that drive row paint
/// layout. Any change here invalidates cached row paint rows because their
/// y-positions and glyph advances were computed against these values.
fn terminal_metrics_fingerprint(metrics: &TerminalMetrics) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    metrics.cell_width_px.to_bits().hash(&mut hasher);
    metrics.cell_height_px.to_bits().hash(&mut hasher);
    metrics.line_height_px.to_bits().hash(&mut hasher);
    metrics.font_size_px.to_bits().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_metrics(cell_width_px: f32, line_height_px: f32, font_size_px: f32) -> TerminalMetrics {
        TerminalMetrics {
            cell_width_px,
            cell_height_px: line_height_px,
            line_height_px,
            font_size_px,
        }
    }

    #[test]
    fn metrics_fingerprint_is_stable_for_identical_metrics() {
        let a = make_metrics(8.0, 16.0, 13.0);
        let b = make_metrics(8.0, 16.0, 13.0);
        assert_eq!(
            terminal_metrics_fingerprint(&a),
            terminal_metrics_fingerprint(&b),
        );
    }

    #[test]
    fn metrics_fingerprint_diverges_on_dpi_or_font_change() {
        let baseline = make_metrics(8.0, 16.0, 13.0);
        let dpi_bump = make_metrics(9.6, 19.2, 13.0);
        let font_bump = make_metrics(8.0, 16.0, 14.0);
        let baseline_fp = terminal_metrics_fingerprint(&baseline);
        assert_ne!(baseline_fp, terminal_metrics_fingerprint(&dpi_bump));
        assert_ne!(baseline_fp, terminal_metrics_fingerprint(&font_bump));
    }
}
