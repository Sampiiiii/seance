// Owns the visible viewport cache and row revision bookkeeping for terminal sessions.

use std::sync::Arc;

use crate::{
    SessionSummary, TerminalCursorState, TerminalGeometry, TerminalRow, TerminalScreenKind,
    TerminalScrollbarState, TerminalViewportSnapshot,
};

#[derive(Debug)]
pub(crate) struct ViewportCache {
    rows: Vec<Arc<TerminalRow>>,
    row_revisions: Vec<u64>,
    pub(crate) viewport_revision: u64,
    next_row_revision: u64,
    geometry: TerminalGeometry,
    pub(crate) at_bottom: bool,
    cursor: Option<TerminalCursorState>,
    scrollbar: Option<TerminalScrollbarState>,
    stable_preview_line: String,
}

impl ViewportCache {
    pub(crate) fn new(geometry: TerminalGeometry, initial_row: TerminalRow) -> Self {
        let mut cache = Self {
            rows: Vec::new(),
            row_revisions: Vec::new(),
            viewport_revision: 1,
            next_row_revision: 1,
            geometry,
            at_bottom: true,
            cursor: None,
            scrollbar: None,
            stable_preview_line: String::new(),
        };
        cache.reset_rows(vec![initial_row]);
        cache.refresh_stable_preview();
        cache
    }

    pub(crate) fn resize(&mut self, geometry: TerminalGeometry) {
        self.geometry = geometry;
    }

    pub(crate) fn geometry(&self) -> TerminalGeometry {
        self.geometry
    }

    pub(crate) fn row(&self, index: usize) -> Option<&Arc<TerminalRow>> {
        self.rows.get(index)
    }

    pub(crate) fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub(crate) fn set_cursor(&mut self, cursor: Option<TerminalCursorState>) -> bool {
        if self.cursor == cursor {
            return false;
        }
        self.cursor = cursor;
        true
    }

    pub(crate) fn set_scrollbar(&mut self, scrollbar: Option<TerminalScrollbarState>) -> bool {
        if self.scrollbar == scrollbar {
            return false;
        }
        self.scrollbar = scrollbar;
        true
    }

    pub(crate) fn reset_rows(&mut self, rows: Vec<TerminalRow>) {
        self.rows.clear();
        self.row_revisions.clear();
        for row in rows {
            self.rows.push(Arc::new(row));
            let revision = self.bump_row_revision();
            self.row_revisions.push(revision);
        }
        self.viewport_revision = self.viewport_revision.saturating_add(1);
    }

    pub(crate) fn replace_row(&mut self, index: usize, row: TerminalRow) {
        if index >= self.rows.len() {
            let target_len = index + 1;
            while self.rows.len() < target_len {
                self.rows.push(Arc::new(TerminalRow::default()));
                let revision = self.bump_row_revision();
                self.row_revisions.push(revision);
            }
        }

        self.rows[index] = Arc::new(row);
        self.row_revisions[index] = self.bump_row_revision();
    }

    pub(crate) fn bump_viewport_revision(&mut self) {
        self.viewport_revision = self.viewport_revision.saturating_add(1);
    }

    pub(crate) fn snapshot(&self) -> TerminalViewportSnapshot {
        TerminalViewportSnapshot {
            rows: Arc::from(self.rows.clone()),
            row_revisions: Arc::from(self.row_revisions.clone()),
            cursor: self.cursor,
            scrollbar: self.scrollbar,
            scroll_offset_rows: self
                .scrollbar
                .map(|scrollbar| scrollbar.offset_rows)
                .unwrap_or(0),
            revision: self.viewport_revision,
            cols: self.geometry.size.cols,
            rows_visible: self.geometry.size.rows,
        }
    }

    fn computed_preview_line(&self) -> String {
        self.rows
            .iter()
            .rev()
            .map(|row| row.plain_text())
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default()
    }

    pub(crate) fn refresh_stable_preview(&mut self) {
        self.stable_preview_line = self.computed_preview_line();
    }

    pub(crate) fn summary(
        &self,
        exit_status: Option<String>,
        scrollback_rows: usize,
        active_screen: TerminalScreenKind,
        mouse_tracking: bool,
    ) -> SessionSummary {
        SessionSummary {
            exit_status,
            preview_line: self.stable_preview_line.clone(),
            viewport_revision: self.viewport_revision,
            scrollback_rows,
            active_screen,
            mouse_tracking,
            at_bottom: self.at_bottom,
        }
    }

    fn bump_row_revision(&mut self) -> u64 {
        self.next_row_revision = self.next_row_revision.saturating_add(1);
        self.next_row_revision
    }
}
