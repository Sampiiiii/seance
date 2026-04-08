// Owns the visible viewport cache and row revision bookkeeping for terminal sessions.

use std::sync::Arc;

use crate::{
    SessionSummary, TerminalCursor, TerminalGeometry, TerminalRow, TerminalScreenKind,
    TerminalViewportSnapshot,
};

#[derive(Debug)]
pub(crate) struct ViewportCache {
    rows: Vec<Arc<TerminalRow>>,
    row_revisions: Vec<u64>,
    pub(crate) viewport_revision: u64,
    next_row_revision: u64,
    geometry: TerminalGeometry,
    pub(crate) at_bottom: bool,
    cursor: Option<TerminalCursor>,
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
        };
        cache.reset_rows(vec![initial_row]);
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

    pub(crate) fn set_cursor(&mut self, cursor: Option<TerminalCursor>) {
        self.cursor = cursor;
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
            revision: self.viewport_revision,
            cols: self.geometry.size.cols,
            rows_visible: self.geometry.size.rows,
        }
    }

    pub(crate) fn preview_line(&self) -> String {
        self.rows
            .iter()
            .rev()
            .map(|row| row.plain_text())
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default()
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
            preview_line: self.preview_line(),
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
