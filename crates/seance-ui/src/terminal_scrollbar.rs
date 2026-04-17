// Owns terminal scrollbar overlay layout, hit testing, and drag mapping.

use seance_terminal::{TerminalGeometry, TerminalScrollbarState};

pub(crate) const TERMINAL_SCROLLBAR_GUTTER_WIDTH_PX: f32 = 14.0;
const GUTTER_WIDTH_PX: f32 = TERMINAL_SCROLLBAR_GUTTER_WIDTH_PX;
const TRACK_WIDTH_PX: f32 = 6.0;
const IDLE_THUMB_WIDTH_PX: f32 = 6.0;
const ACTIVE_THUMB_WIDTH_PX: f32 = 8.0;
const RIGHT_INSET_PX: f32 = 1.0;
const VERTICAL_INSET_PX: f32 = 6.0;
const MIN_THUMB_HEIGHT_PX: f32 = 36.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TerminalScrollbarLayout {
    pub(crate) gutter_left_px: f32,
    pub(crate) gutter_top_px: f32,
    pub(crate) gutter_width_px: f32,
    pub(crate) gutter_height_px: f32,
    pub(crate) track_left_px: f32,
    pub(crate) track_top_px: f32,
    pub(crate) track_width_px: f32,
    pub(crate) track_height_px: f32,
    pub(crate) thumb_top_px: f32,
    pub(crate) thumb_height_px: f32,
    pub(crate) max_offset_rows: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalScrollbarHit {
    Track,
    Thumb,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TerminalScrollbarDragState {
    pub(crate) grab_offset_y_px: f32,
    pub(crate) max_offset_rows: u64,
    pub(crate) thumb_height_px: f32,
    pub(crate) track_top_px: f32,
    pub(crate) track_height_px: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    left_px: f32,
    top_px: f32,
    width_px: f32,
    height_px: f32,
}

impl Rect {
    fn contains(self, x: f32, y: f32) -> bool {
        x >= self.left_px
            && x <= self.left_px + self.width_px
            && y >= self.top_px
            && y <= self.top_px + self.height_px
    }
}

impl TerminalScrollbarLayout {
    pub(crate) fn new(
        scrollbar: TerminalScrollbarState,
        geometry: TerminalGeometry,
    ) -> Option<Self> {
        if scrollbar.total_rows <= scrollbar.visible_rows {
            return None;
        }

        let height_px = f32::from(geometry.pixel_size.height_px).max(1.0);
        let width_px = f32::from(geometry.pixel_size.width_px).max(1.0);
        let gutter_height_px = (height_px - VERTICAL_INSET_PX * 2.0).max(0.0);
        if gutter_height_px <= 0.0 {
            return None;
        }

        let gutter_left_px = width_px;
        let gutter_top_px = VERTICAL_INSET_PX;
        let track_left_px = (gutter_left_px + GUTTER_WIDTH_PX - RIGHT_INSET_PX - TRACK_WIDTH_PX)
            .max(gutter_left_px);
        let track_top_px = gutter_top_px;
        let max_offset_rows = scrollbar.total_rows.saturating_sub(scrollbar.visible_rows);
        let total_rows = scrollbar.total_rows.max(scrollbar.visible_rows).max(1) as f32;
        let visible_ratio = scrollbar.visible_rows as f32 / total_rows;
        let thumb_height_px = (gutter_height_px * visible_ratio)
            .max(MIN_THUMB_HEIGHT_PX)
            .min(gutter_height_px);
        let thumb_travel_px = (gutter_height_px - thumb_height_px).max(0.0);
        let offset_ratio = if max_offset_rows == 0 {
            0.0
        } else {
            (scrollbar.offset_rows.min(max_offset_rows) as f32 / max_offset_rows as f32)
                .clamp(0.0, 1.0)
        };
        let thumb_top_px = track_top_px + thumb_travel_px * offset_ratio;

        Some(Self {
            gutter_left_px,
            gutter_top_px,
            gutter_width_px: GUTTER_WIDTH_PX,
            gutter_height_px,
            track_left_px,
            track_top_px,
            track_width_px: TRACK_WIDTH_PX,
            track_height_px: gutter_height_px,
            thumb_top_px,
            thumb_height_px,
            max_offset_rows,
        })
    }

    pub(crate) fn idle_thumb_width_px(self) -> f32 {
        IDLE_THUMB_WIDTH_PX
    }

    pub(crate) fn active_thumb_width_px(self) -> f32 {
        ACTIVE_THUMB_WIDTH_PX
    }

    pub(crate) fn thumb_left_px(self, width_px: f32) -> f32 {
        (self.gutter_left_px + self.gutter_width_px - RIGHT_INSET_PX - width_px)
            .max(self.gutter_left_px)
    }

    pub(crate) fn hit_test(self, x: f32, y: f32) -> Option<TerminalScrollbarHit> {
        if self.thumb_hit_rect().contains(x, y) {
            return Some(TerminalScrollbarHit::Thumb);
        }

        self.gutter_rect()
            .contains(x, y)
            .then_some(TerminalScrollbarHit::Track)
    }

    pub(crate) fn center_grab_offset_y_px(self) -> f32 {
        self.thumb_height_px * 0.5
    }

    pub(crate) fn drag_state(self, grab_offset_y_px: f32) -> TerminalScrollbarDragState {
        TerminalScrollbarDragState {
            grab_offset_y_px: grab_offset_y_px.clamp(0.0, self.thumb_height_px),
            max_offset_rows: self.max_offset_rows,
            thumb_height_px: self.thumb_height_px,
            track_top_px: self.track_top_px,
            track_height_px: self.track_height_px,
        }
    }

    #[cfg(test)]
    pub(crate) fn offset_for_pointer_y(self, pointer_y_px: f32, grab_offset_y_px: f32) -> u64 {
        self.drag_state(grab_offset_y_px)
            .offset_for_pointer_y(pointer_y_px)
    }

    fn gutter_rect(self) -> Rect {
        Rect {
            left_px: self.gutter_left_px,
            top_px: self.gutter_top_px,
            width_px: self.gutter_width_px,
            height_px: self.gutter_height_px,
        }
    }

    fn thumb_hit_rect(self) -> Rect {
        Rect {
            left_px: self.thumb_left_px(self.active_thumb_width_px()),
            top_px: self.thumb_top_px,
            width_px: self.active_thumb_width_px(),
            height_px: self.thumb_height_px,
        }
    }
}

impl TerminalScrollbarDragState {
    pub(crate) fn offset_for_pointer_y(self, pointer_y_px: f32) -> u64 {
        if self.max_offset_rows == 0 {
            return 0;
        }

        let max_thumb_top_px = (self.track_top_px + self.track_height_px - self.thumb_height_px)
            .max(self.track_top_px);
        let thumb_top_px =
            (pointer_y_px - self.grab_offset_y_px).clamp(self.track_top_px, max_thumb_top_px);
        let thumb_travel_px = (self.track_height_px - self.thumb_height_px).max(0.0);
        if thumb_travel_px <= f32::EPSILON {
            return 0;
        }

        let ratio = ((thumb_top_px - self.track_top_px) / thumb_travel_px).clamp(0.0, 1.0);
        (ratio * self.max_offset_rows as f32).round() as u64
    }
}

#[cfg(test)]
mod tests {
    use seance_terminal::{TerminalGeometry, TerminalScrollbarState};

    use super::{RIGHT_INSET_PX, TerminalScrollbarHit, TerminalScrollbarLayout};

    fn geometry() -> TerminalGeometry {
        TerminalGeometry::new(80, 24, 640, 456, 8, 19).expect("terminal geometry")
    }

    #[test]
    fn layout_hides_when_scrollback_is_not_larger_than_viewport() {
        assert_eq!(
            TerminalScrollbarLayout::new(
                TerminalScrollbarState {
                    total_rows: 24,
                    offset_rows: 0,
                    visible_rows: 24,
                },
                geometry(),
            ),
            None
        );
    }

    #[test]
    fn thumb_height_respects_minimum() {
        let layout = TerminalScrollbarLayout::new(
            TerminalScrollbarState {
                total_rows: 1_000,
                offset_rows: 0,
                visible_rows: 10,
            },
            geometry(),
        )
        .expect("layout");

        assert_eq!(layout.thumb_height_px, 36.0);
    }

    #[test]
    fn offset_positions_map_to_top_middle_and_bottom() {
        let layout = TerminalScrollbarLayout::new(
            TerminalScrollbarState {
                total_rows: 200,
                offset_rows: 88,
                visible_rows: 24,
            },
            geometry(),
        )
        .expect("layout");
        let top = TerminalScrollbarLayout::new(
            TerminalScrollbarState {
                total_rows: 200,
                offset_rows: 0,
                visible_rows: 24,
            },
            geometry(),
        )
        .expect("top layout");
        let bottom = TerminalScrollbarLayout::new(
            TerminalScrollbarState {
                total_rows: 200,
                offset_rows: 176,
                visible_rows: 24,
            },
            geometry(),
        )
        .expect("bottom layout");

        assert_eq!(top.thumb_top_px, top.track_top_px);
        assert!(layout.thumb_top_px > top.thumb_top_px);
        assert!(layout.thumb_top_px < bottom.thumb_top_px);
        assert_eq!(
            bottom.thumb_top_px,
            bottom.track_top_px + bottom.track_height_px - bottom.thumb_height_px
        );
    }

    #[test]
    fn hit_testing_distinguishes_thumb_and_track() {
        let layout = TerminalScrollbarLayout::new(
            TerminalScrollbarState {
                total_rows: 120,
                offset_rows: 20,
                visible_rows: 24,
            },
            geometry(),
        )
        .expect("layout");
        let thumb_x = layout.thumb_left_px(layout.active_thumb_width_px()) + 1.0;
        let thumb_y = layout.thumb_top_px + 1.0;
        let track_x = layout.gutter_left_px + 1.0;
        let track_y = layout.thumb_top_px + layout.thumb_height_px + 12.0;

        assert_eq!(
            layout.hit_test(thumb_x, thumb_y),
            Some(TerminalScrollbarHit::Thumb)
        );
        assert_eq!(
            layout.hit_test(track_x, track_y),
            Some(TerminalScrollbarHit::Track)
        );
        assert_eq!(layout.hit_test(layout.gutter_left_px - 2.0, track_y), None);
    }

    #[test]
    fn track_and_thumb_are_right_aligned_inside_gutter() {
        let layout = TerminalScrollbarLayout::new(
            TerminalScrollbarState {
                total_rows: 120,
                offset_rows: 20,
                visible_rows: 24,
            },
            geometry(),
        )
        .expect("layout");

        assert_eq!(
            layout.track_left_px + layout.track_width_px,
            layout.gutter_left_px + layout.gutter_width_px - RIGHT_INSET_PX
        );
        assert_eq!(
            layout.thumb_left_px(layout.active_thumb_width_px()) + layout.active_thumb_width_px(),
            layout.gutter_left_px + layout.gutter_width_px - RIGHT_INSET_PX
        );
    }

    #[test]
    fn gutter_starts_after_terminal_content_width() {
        let geometry = geometry();
        let layout = TerminalScrollbarLayout::new(
            TerminalScrollbarState {
                total_rows: 120,
                offset_rows: 20,
                visible_rows: 24,
            },
            geometry,
        )
        .expect("layout");

        assert_eq!(
            layout.gutter_left_px,
            f32::from(geometry.pixel_size.width_px)
        );
    }

    #[test]
    fn pointer_mapping_clamps_to_bounds() {
        let layout = TerminalScrollbarLayout::new(
            TerminalScrollbarState {
                total_rows: 120,
                offset_rows: 20,
                visible_rows: 24,
            },
            geometry(),
        )
        .expect("layout");
        let center_grab = layout.center_grab_offset_y_px();

        assert_eq!(
            layout.offset_for_pointer_y(layout.track_top_px - 20.0, center_grab),
            0
        );
        assert_eq!(
            layout.offset_for_pointer_y(
                layout.track_top_px + layout.track_height_px + 20.0,
                center_grab
            ),
            layout.max_offset_rows
        );
    }
}
