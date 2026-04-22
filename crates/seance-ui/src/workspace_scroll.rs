// Owns coalesced terminal scrolling, scrollbar drag dispatch, and scroll-idle link paint restoration.

use std::time::{Duration, Instant};

use gpui::{
    ClipboardItem, Context, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ScrollDelta, ScrollWheelEvent, Window,
};
use seance_config::{
    MouseTrackingScrollPolicy, MouseTrackingSelectionPolicy, TerminalRightClickPolicy,
};
use seance_terminal::{
    TerminalMouseButton, TerminalMouseEventKind, TerminalPaste, TerminalScreenKind,
    TerminalScrollCommand, TerminalSession,
};
use tracing::trace;

use crate::{
    LinkPaintMode, RepaintReasonSet, SeanceWorkspace, TerminalScrollbarDragState,
    TerminalScrollbarHit, TerminalScrollbarLayout,
    model::{TerminalDragAutoScrollDirection, TerminalDragAutoScrollState},
    ui_components::TERMINAL_PANE_PADDING_PX,
    workspace::TerminalScrollbarMouseDownOutcome,
};

const SCROLL_IDLE_LINK_RESTORE_DELAY: Duration = Duration::from_millis(80);

pub(crate) fn handle_terminal_scroll_wheel(
    this: &mut SeanceWorkspace,
    event: &ScrollWheelEvent,
    window: &mut Window,
    cx: &mut Context<SeanceWorkspace>,
) {
    let Some(session) = this.active_session() else {
        stop_drag_auto_scroll(this, "mouse-down-no-session");
        return;
    };
    let summary = session.summary();
    let line_height = this
        .terminal_metrics
        .map(|metrics| metrics.line_height_px)
        .unwrap_or_else(|| this.terminal_line_height_px())
        .max(1.0);
    let delta_rows_f = match event.delta {
        ScrollDelta::Pixels(delta) => -(f32::from(delta.y) / line_height),
        ScrollDelta::Lines(delta) => -delta.y,
    };
    if delta_rows_f.abs() < f32::EPSILON {
        return;
    }

    let local_wheel_override = summary.mouse_tracking
        && mouse_tracking_wheel_routes_locally(
            summary.active_screen,
            this.config.terminal.interaction.mouse_tracking_scroll,
            event.modifiers,
        );
    if summary.mouse_tracking && !local_wheel_override {
        accumulate_scroll_delta(
            &mut this.terminal_scroll.accumulated_row_delta,
            delta_rows_f,
        );
        let delta_rows = take_integral_scroll_rows(&mut this.terminal_scroll.accumulated_row_delta);
        if delta_rows == 0 {
            return;
        }
        let button = if delta_rows > 0 {
            TerminalMouseButton::WheelUp
        } else {
            TerminalMouseButton::WheelDown
        };
        if let Some(mouse_event) = this.terminal_mouse_event(
            event.position,
            TerminalMouseEventKind::Press,
            Some(button),
            event.modifiers,
        ) {
            for _ in 0..delta_rows.unsigned_abs() {
                let _ = session.send_mouse(mouse_event.clone());
            }
            this.request_repaint(RepaintReasonSet::TERMINAL_UPDATE, window, cx);
        }
        return;
    }

    if matches!(summary.active_screen, TerminalScreenKind::Alternate) {
        this.terminal_scroll.accumulated_row_delta = 0.0;
        this.terminal_scroll.pending_scroll_command = None;
        return;
    }

    accumulate_scroll_delta(
        &mut this.terminal_scroll.accumulated_row_delta,
        delta_rows_f,
    );
    if let Some(anchor) = this.terminal_drag_anchor
        && let Some(focus) = this.terminal_selection_point_clamped(event.position)
    {
        this.terminal_selection = Some(crate::model::TerminalSelection { anchor, focus });
    }
    schedule_scroll_flush(this, window, cx);
}

pub(crate) fn handle_terminal_mouse_down(
    this: &mut SeanceWorkspace,
    event: &MouseDownEvent,
    window: &mut Window,
    cx: &mut Context<SeanceWorkspace>,
) {
    window.focus(&this.focus_handle);

    let Some(session) = this.active_session() else {
        return;
    };
    let summary = session.summary();
    let local_mouse_override = summary.mouse_tracking
        && mouse_tracking_selection_allows_local_override(
            this.config.terminal.interaction.mouse_tracking_selection,
            event.modifiers,
        );
    let scrollbar_interactive = terminal_scrollbar_is_interactive(
        summary.active_screen,
        summary.mouse_tracking && !local_mouse_override,
    );
    if event.button == MouseButton::Left
        && scrollbar_interactive
        && let Some((layout, local_x, local_y)) =
            this.terminal_scrollbar_local_position(event.position)
        && let Some(outcome) = terminal_scrollbar_mouse_down_outcome(layout, local_x, local_y)
    {
        stop_drag_auto_scroll(this, "scrollbar-mouse-down");
        this.terminal_scrollbar_hovered = true;
        this.terminal_scrollbar_drag = Some(outcome.drag_state);
        let reason = if let Some(command) = outcome.command {
            queue_scroll_command(this, command, window, cx);
            RepaintReasonSet::TERMINAL_UPDATE
        } else {
            RepaintReasonSet::INPUT
        };
        this.request_repaint(reason, window, cx);
        return;
    }

    if event.button == MouseButton::Right && (!summary.mouse_tracking || local_mouse_override) {
        apply_terminal_right_click_policy(
            this,
            session.as_ref(),
            this.config.terminal.interaction.right_click,
            cx,
        );
        this.request_repaint(RepaintReasonSet::INPUT, window, cx);
        return;
    }

    if summary.mouse_tracking && !local_mouse_override {
        stop_drag_auto_scroll(this, "mouse-tracking-remote");
        this.clear_terminal_hovered_link();
        this.terminal_scrollbar_hovered = false;
        this.terminal_scrollbar_drag = None;
        if let Some(mouse_event) = this.terminal_mouse_event(
            event.position,
            TerminalMouseEventKind::Press,
            terminal_mouse_button(event.button),
            event.modifiers,
        ) {
            let _ = session.send_mouse(mouse_event);
        }
        this.clear_terminal_selection();
    } else if event.click_count < 2 && this.try_open_terminal_link(event, summary.active_screen) {
        stop_drag_auto_scroll(this, "opened-link");
        this.terminal_drag_anchor = None;
        this.clear_terminal_hovered_link();
        this.request_repaint(RepaintReasonSet::INPUT, window, cx);
        return;
    } else if event.button == MouseButton::Left
        && let Some(point) = this.terminal_selection_point(event.position)
    {
        stop_drag_auto_scroll(this, "manual-selection-start");
        this.clear_terminal_turn_selection();
        this.clear_terminal_hovered_link();
        if let Some(selection) = this.terminal_click_selection(point, event.click_count) {
            this.terminal_selection = Some(selection);
            this.terminal_drag_anchor = None;
        } else {
            this.terminal_selection = Some(crate::model::TerminalSelection {
                anchor: point,
                focus: point,
            });
            this.terminal_drag_anchor = Some(point);
        }
    }

    this.request_repaint(RepaintReasonSet::INPUT, window, cx);
}

pub(crate) fn handle_terminal_mouse_move(
    this: &mut SeanceWorkspace,
    event: &MouseMoveEvent,
    window: &mut Window,
    cx: &mut Context<SeanceWorkspace>,
) {
    let Some(session) = this.active_session() else {
        stop_drag_auto_scroll(this, "mouse-move-no-session");
        if this.clear_terminal_hovered_link() {
            this.request_repaint(RepaintReasonSet::INPUT, window, cx);
        }
        return;
    };
    let summary = session.summary();
    let local_mouse_override = summary.mouse_tracking
        && mouse_tracking_selection_allows_local_override(
            this.config.terminal.interaction.mouse_tracking_selection,
            event.modifiers,
        );
    let scrollbar_interactive = terminal_scrollbar_is_interactive(
        summary.active_screen,
        summary.mouse_tracking && !local_mouse_override,
    );
    if let Some(drag_state) = this.terminal_scrollbar_drag {
        stop_drag_auto_scroll(this, "scrollbar-drag-active");
        let mut state_changed = this.clear_terminal_hovered_link();
        if scrollbar_interactive && let Some(local_y) = this.terminal_local_y(event.position) {
            let command = terminal_scrollbar_drag_command(drag_state, local_y);
            queue_scroll_command(this, command, window, cx);
            this.terminal_scrollbar_hovered = true;
            this.request_repaint(RepaintReasonSet::TERMINAL_UPDATE, window, cx);
        } else {
            this.terminal_scrollbar_drag = None;
            if this.terminal_scrollbar_hovered {
                this.terminal_scrollbar_hovered = false;
                state_changed = true;
            }
            if state_changed {
                this.request_repaint(RepaintReasonSet::INPUT, window, cx);
            }
        }
        return;
    }

    if summary.mouse_tracking && !local_mouse_override {
        stop_drag_auto_scroll(this, "mouse-tracking-remote");
        let mut state_changed = this.clear_terminal_hovered_link();
        if this.terminal_scrollbar_hovered {
            this.terminal_scrollbar_hovered = false;
            state_changed = true;
        }
        if let Some(mouse_event) = this.terminal_mouse_event(
            event.position,
            TerminalMouseEventKind::Move,
            event.pressed_button.and_then(terminal_mouse_button),
            event.modifiers,
        ) {
            let _ = session.send_mouse(mouse_event);
            this.request_repaint(RepaintReasonSet::TERMINAL_UPDATE, window, cx);
        } else if state_changed {
            this.request_repaint(RepaintReasonSet::INPUT, window, cx);
        }
        return;
    }

    let scrollbar_hovered = scrollbar_interactive
        && this
            .terminal_scrollbar_local_position(event.position)
            .is_some_and(|(layout, local_x, local_y)| layout.hit_test(local_x, local_y).is_some());
    let mut state_changed = false;
    if scrollbar_hovered != this.terminal_scrollbar_hovered {
        this.terminal_scrollbar_hovered = scrollbar_hovered;
        state_changed = true;
    }

    if event.dragging() {
        state_changed |= this.clear_terminal_hovered_link();
    } else {
        state_changed |= this.update_terminal_hovered_link(event.position, event.modifiers);
    }

    if state_changed {
        this.request_repaint(RepaintReasonSet::INPUT, window, cx);
    }

    if !event.dragging() {
        stop_drag_auto_scroll(this, "drag-ended");
        return;
    }

    let Some(anchor) = this.terminal_drag_anchor else {
        stop_drag_auto_scroll(this, "missing-drag-anchor");
        return;
    };
    let Some(focus) = this.terminal_selection_point_clamped(event.position) else {
        stop_drag_auto_scroll(this, "no-clamped-focus");
        return;
    };
    this.terminal_selection = Some(crate::model::TerminalSelection { anchor, focus });
    maybe_update_drag_auto_scroll(this, event.position, window, cx);
    this.request_repaint(RepaintReasonSet::INPUT, window, cx);
}

pub(crate) fn handle_terminal_mouse_up(
    this: &mut SeanceWorkspace,
    event: &MouseUpEvent,
    window: &mut Window,
    cx: &mut Context<SeanceWorkspace>,
) {
    let Some(session) = this.active_session() else {
        this.terminal_drag_anchor = None;
        stop_drag_auto_scroll(this, "mouse-up-no-session");
        this.terminal_hovered_link = None;
        this.terminal_scrollbar_hovered = false;
        this.terminal_scrollbar_drag = None;
        return;
    };

    let summary = session.summary();
    let local_mouse_override = summary.mouse_tracking
        && mouse_tracking_selection_allows_local_override(
            this.config.terminal.interaction.mouse_tracking_selection,
            event.modifiers,
        );
    let scrollbar_interactive = terminal_scrollbar_is_interactive(
        summary.active_screen,
        summary.mouse_tracking && !local_mouse_override,
    );
    if this.terminal_scrollbar_drag.take().is_some() {
        this.terminal_drag_anchor = None;
        stop_drag_auto_scroll(this, "scrollbar-drag-end");
        this.terminal_scrollbar_hovered = scrollbar_interactive
            && this
                .terminal_scrollbar_local_position(event.position)
                .is_some_and(|(layout, local_x, local_y)| {
                    layout.hit_test(local_x, local_y).is_some()
                });
        this.request_repaint(RepaintReasonSet::INPUT, window, cx);
        return;
    }

    if summary.mouse_tracking && !local_mouse_override {
        if let Some(mouse_event) = this.terminal_mouse_event(
            event.position,
            TerminalMouseEventKind::Release,
            terminal_mouse_button(event.button),
            event.modifiers,
        ) {
            let _ = session.send_mouse(mouse_event);
        }
    }

    this.terminal_drag_anchor = None;
    stop_drag_auto_scroll(this, "mouse-up");
    this.request_repaint(RepaintReasonSet::INPUT, window, cx);
}

pub(crate) fn handle_terminal_scrollback_key(
    this: &mut SeanceWorkspace,
    event: &KeyDownEvent,
    window: &mut Window,
    cx: &mut Context<SeanceWorkspace>,
) -> bool {
    let Some(session) = this.active_session() else {
        return false;
    };
    let summary = session.summary();
    if matches!(summary.active_screen, TerminalScreenKind::Alternate) {
        return false;
    }

    let modifiers = event.keystroke.modifiers;
    if !modifiers.shift {
        return false;
    }

    let command = match event.keystroke.key.as_str() {
        "pageup" => Some(TerminalScrollCommand::PageUp),
        "pagedown" => Some(TerminalScrollCommand::PageDown),
        "home" => Some(TerminalScrollCommand::Top),
        "end" => Some(TerminalScrollCommand::Bottom),
        _ => None,
    };
    let Some(command) = command else {
        return false;
    };

    let result = match command {
        TerminalScrollCommand::Bottom => session.scroll_to_bottom(),
        _ => session.scroll_viewport(command),
    };
    if result.is_ok() {
        this.request_repaint(RepaintReasonSet::TERMINAL_UPDATE, window, cx);
    }
    true
}

pub(crate) fn terminal_scrollbar_is_interactive(
    active_screen: TerminalScreenKind,
    mouse_tracking: bool,
) -> bool {
    !mouse_tracking && matches!(active_screen, TerminalScreenKind::Primary)
}

pub(crate) fn terminal_scrollbar_mouse_down_outcome(
    layout: TerminalScrollbarLayout,
    local_x: f32,
    local_y: f32,
) -> Option<TerminalScrollbarMouseDownOutcome> {
    match layout.hit_test(local_x, local_y)? {
        TerminalScrollbarHit::Thumb => Some(TerminalScrollbarMouseDownOutcome {
            command: None,
            drag_state: layout.drag_state(local_y - layout.thumb_top_px),
        }),
        TerminalScrollbarHit::Track => {
            let drag_state = layout.drag_state(layout.center_grab_offset_y_px());
            Some(TerminalScrollbarMouseDownOutcome {
                command: Some(TerminalScrollCommand::SetOffsetRows(
                    drag_state.offset_for_pointer_y(local_y),
                )),
                drag_state,
            })
        }
    }
}

pub(crate) fn terminal_scrollbar_drag_command(
    drag_state: TerminalScrollbarDragState,
    local_y: f32,
) -> TerminalScrollCommand {
    TerminalScrollCommand::SetOffsetRows(drag_state.offset_for_pointer_y(local_y))
}

fn terminal_mouse_button(button: MouseButton) -> Option<TerminalMouseButton> {
    match button {
        MouseButton::Left => Some(TerminalMouseButton::Left),
        MouseButton::Right => Some(TerminalMouseButton::Right),
        MouseButton::Middle => Some(TerminalMouseButton::Middle),
        _ => None,
    }
}

fn mouse_tracking_wheel_routes_locally(
    active_screen: TerminalScreenKind,
    policy: MouseTrackingScrollPolicy,
    modifiers: Modifiers,
) -> bool {
    if !matches!(active_screen, TerminalScreenKind::Primary) {
        return false;
    }

    match policy {
        MouseTrackingScrollPolicy::AlwaysAppFirst => false,
        MouseTrackingScrollPolicy::HybridShiftWheelLocal => modifiers.shift,
        MouseTrackingScrollPolicy::AlwaysLocal => true,
    }
}

fn mouse_tracking_selection_allows_local_override(
    policy: MouseTrackingSelectionPolicy,
    modifiers: Modifiers,
) -> bool {
    match policy {
        MouseTrackingSelectionPolicy::Disabled => false,
        MouseTrackingSelectionPolicy::ShiftDragLocal => modifiers.shift,
        MouseTrackingSelectionPolicy::AlwaysLocal => true,
    }
}

fn apply_terminal_right_click_policy(
    this: &mut SeanceWorkspace,
    session: &dyn TerminalSession,
    policy: TerminalRightClickPolicy,
    cx: &mut Context<SeanceWorkspace>,
) {
    match policy {
        TerminalRightClickPolicy::Disabled => {}
        TerminalRightClickPolicy::PasteClipboard => {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                let _ = session.paste(TerminalPaste { text });
            }
        }
        TerminalRightClickPolicy::CopySelectionOrPaste => {
            if let Some(selection) = this.terminal_selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(selection));
                return;
            }
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                let _ = session.paste(TerminalPaste { text });
            }
        }
    }
}

fn maybe_update_drag_auto_scroll(
    this: &mut SeanceWorkspace,
    pointer: gpui::Point<gpui::Pixels>,
    window: &mut Window,
    cx: &mut Context<SeanceWorkspace>,
) {
    let Some((direction, rows_per_tick)) = terminal_drag_auto_scroll_params(this, pointer) else {
        stop_drag_auto_scroll(this, "pointer-inside-viewport");
        return;
    };

    let needs_new_loop = this
        .terminal_drag_auto_scroll
        .as_ref()
        .map_or(true, |state| {
            state.direction != direction || state.rows_per_tick != rows_per_tick
        });
    if needs_new_loop {
        this.terminal_drag_auto_scroll_epoch = this.terminal_drag_auto_scroll_epoch.wrapping_add(1);
        let epoch = this.terminal_drag_auto_scroll_epoch;
        this.terminal_drag_auto_scroll = Some(TerminalDragAutoScrollState {
            direction,
            rows_per_tick,
            pointer,
            epoch,
            frame_scheduled: false,
        });
        schedule_drag_auto_scroll_tick(this, window, cx);
        return;
    }

    if let Some(state) = this.terminal_drag_auto_scroll.as_mut() {
        state.pointer = pointer;
    }
}

fn terminal_drag_auto_scroll_params(
    this: &SeanceWorkspace,
    pointer: gpui::Point<gpui::Pixels>,
) -> Option<(TerminalDragAutoScrollDirection, isize)> {
    let geometry = this.terminal_surface.geometry?;
    let local_y = f32::from(pointer.y) - TERMINAL_PANE_PADDING_PX;
    let max_y = f32::from(geometry.pixel_size.height_px);
    if local_y < 0.0 {
        let rows_per_tick =
            terminal_drag_auto_scroll_speed(-local_y, this.terminal_line_height_px());
        return Some((TerminalDragAutoScrollDirection::Up, rows_per_tick));
    }
    if local_y > max_y {
        let rows_per_tick =
            terminal_drag_auto_scroll_speed(local_y - max_y, this.terminal_line_height_px());
        return Some((TerminalDragAutoScrollDirection::Down, rows_per_tick));
    }
    None
}

fn terminal_drag_auto_scroll_speed(overshoot_px: f32, line_height_px: f32) -> isize {
    let normalized = overshoot_px / line_height_px.max(1.0);
    (1 + normalized.floor() as isize).clamp(1, 6)
}

fn schedule_drag_auto_scroll_tick(
    this: &mut SeanceWorkspace,
    window: &mut Window,
    cx: &mut Context<SeanceWorkspace>,
) {
    let Some(state) = this.terminal_drag_auto_scroll.as_mut() else {
        return;
    };
    if state.frame_scheduled {
        return;
    }

    state.frame_scheduled = true;
    let epoch = state.epoch;
    cx.on_next_frame(window, move |this, window, cx| {
        run_drag_auto_scroll_tick(this, epoch, window, cx);
    });
}

fn run_drag_auto_scroll_tick(
    this: &mut SeanceWorkspace,
    epoch: u64,
    window: &mut Window,
    cx: &mut Context<SeanceWorkspace>,
) {
    let (direction, rows_per_tick, pointer) = {
        let Some(state) = this.terminal_drag_auto_scroll.as_mut() else {
            return;
        };
        if state.epoch != epoch {
            return;
        }
        state.frame_scheduled = false;
        (state.direction, state.rows_per_tick, state.pointer)
    };

    let Some(anchor) = this.terminal_drag_anchor else {
        stop_drag_auto_scroll(this, "drag-anchor-cleared");
        return;
    };
    let active_primary = this
        .active_session()
        .map(|session| matches!(session.summary().active_screen, TerminalScreenKind::Primary))
        .unwrap_or(false);
    if !active_primary {
        stop_drag_auto_scroll(this, "non-primary-screen");
        return;
    }

    let delta_rows = match direction {
        TerminalDragAutoScrollDirection::Up => -rows_per_tick.abs(),
        TerminalDragAutoScrollDirection::Down => rows_per_tick.abs(),
    };
    queue_scroll_command(
        this,
        TerminalScrollCommand::DeltaRows(delta_rows),
        window,
        cx,
    );
    if let Some(focus) = this.terminal_selection_point_clamped(pointer) {
        this.terminal_selection = Some(crate::model::TerminalSelection { anchor, focus });
    }
    trace!(
        epoch,
        direction = ?direction,
        rows_per_tick,
        "terminal drag auto-scroll tick dispatched"
    );
    this.request_repaint(RepaintReasonSet::TERMINAL_UPDATE, window, cx);
    schedule_drag_auto_scroll_tick(this, window, cx);
}

fn stop_drag_auto_scroll(this: &mut SeanceWorkspace, reason: &'static str) {
    if this.terminal_drag_auto_scroll.take().is_some() {
        this.terminal_drag_auto_scroll_epoch = this.terminal_drag_auto_scroll_epoch.wrapping_add(1);
        trace!(reason, "terminal drag auto-scroll stopped");
    }
}

fn queue_scroll_command(
    this: &mut SeanceWorkspace,
    command: TerminalScrollCommand,
    window: &mut Window,
    cx: &mut Context<SeanceWorkspace>,
) {
    this.terminal_scroll.pending_scroll_command = Some(command);
    schedule_scroll_flush(this, window, cx);
}

fn schedule_scroll_flush(
    this: &mut SeanceWorkspace,
    window: &mut Window,
    cx: &mut Context<SeanceWorkspace>,
) {
    if this.terminal_scroll.flush_scheduled {
        return;
    }

    this.terminal_scroll.flush_scheduled = true;
    this.terminal_scroll.pending_flush_epoch =
        this.terminal_scroll.pending_flush_epoch.wrapping_add(1);
    let epoch = this.terminal_scroll.pending_flush_epoch;
    cx.on_next_frame(window, move |this, window, cx| {
        flush_scroll_batch(this, epoch, window, cx);
    });
    this.request_repaint(RepaintReasonSet::SCROLL, window, cx);
}

fn flush_scroll_batch(
    this: &mut SeanceWorkspace,
    epoch: u64,
    window: &mut Window,
    cx: &mut Context<SeanceWorkspace>,
) {
    if this.terminal_scroll.pending_flush_epoch != epoch {
        return;
    }

    this.terminal_scroll.flush_scheduled = false;
    let Some(session) = this.active_session() else {
        this.terminal_scroll.pending_scroll_command = None;
        this.terminal_scroll.accumulated_row_delta = 0.0;
        return;
    };

    let commands = take_scroll_dispatch_commands(&mut this.terminal_scroll);
    if commands.is_empty() {
        return;
    }

    for command in commands.iter().copied() {
        match command {
            TerminalScrollCommand::Bottom => {
                let _ = session.scroll_to_bottom();
            }
            other => {
                let _ = session.scroll_viewport(other);
            }
        }
    }

    let now = Instant::now();
    this.terminal_scroll.last_scroll_dispatch_at = Some(now);
    this.terminal_scroll.scroll_batches_dispatched = this
        .terminal_scroll
        .scroll_batches_dispatched
        .saturating_add(1);
    trace!(
        epoch,
        session_id = session.id(),
        command_count = commands.len(),
        dispatched = ?commands,
        "flushed coalesced UI scroll batch"
    );

    schedule_link_restore_after_idle(this, window, cx);
}

fn schedule_link_restore_after_idle(
    this: &mut SeanceWorkspace,
    window: &mut Window,
    cx: &mut Context<SeanceWorkspace>,
) {
    this.terminal_scroll.idle_epoch = this.terminal_scroll.idle_epoch.wrapping_add(1);
    let epoch = this.terminal_scroll.idle_epoch;
    let entity = cx.entity();

    window
        .spawn(cx, async move |cx| {
            cx.background_executor()
                .spawn(async move {
                    std::thread::sleep(SCROLL_IDLE_LINK_RESTORE_DELAY);
                })
                .await;
            let _ = cx.update(move |window, cx| {
                entity.update(cx, |this, cx| {
                    restore_links_after_idle(this, epoch, window, cx);
                });
            });
        })
        .detach();
}

fn restore_links_after_idle(
    this: &mut SeanceWorkspace,
    epoch: u64,
    window: &mut Window,
    cx: &mut Context<SeanceWorkspace>,
) {
    if this.terminal_scroll.idle_epoch != epoch {
        return;
    }

    if matches!(
        this.terminal_link_paint_mode(Instant::now()),
        LinkPaintMode::Deferred
    ) {
        return;
    }

    this.invalidate_terminal_link_rows();
    trace!(epoch, "restored link paint after scroll idle");
    this.request_repaint(RepaintReasonSet::TERMINAL_UPDATE, window, cx);
}

impl SeanceWorkspace {
    pub(crate) fn terminal_link_paint_mode(&self, now: Instant) -> LinkPaintMode {
        self.terminal_scroll
            .last_scroll_dispatch_at
            .and_then(|last_dispatch| {
                (now.saturating_duration_since(last_dispatch) <= SCROLL_IDLE_LINK_RESTORE_DELAY)
                    .then_some(LinkPaintMode::Deferred)
            })
            .unwrap_or(LinkPaintMode::Normal)
    }
}

fn accumulate_scroll_delta(accumulated: &mut f32, delta_rows: f32) {
    if accumulated.signum() != 0.0
        && delta_rows.signum() != 0.0
        && accumulated.signum() != delta_rows.signum()
    {
        *accumulated = 0.0;
    }

    *accumulated += delta_rows;
}

fn take_integral_scroll_rows(accumulated: &mut f32) -> isize {
    let delta_rows = accumulated.trunc() as isize;
    *accumulated -= delta_rows as f32;
    delta_rows
}

fn take_scroll_dispatch_commands(
    accumulator: &mut crate::model::ScrollFrameAccumulator,
) -> Vec<TerminalScrollCommand> {
    let mut commands = Vec::new();
    if let Some(command) = accumulator.pending_scroll_command.take() {
        commands.push(command);
    }

    let delta_rows = take_integral_scroll_rows(&mut accumulator.accumulated_row_delta);
    if delta_rows != 0 {
        commands.push(TerminalScrollCommand::DeltaRows(delta_rows));
    }

    commands
}

#[cfg(test)]
mod tests {
    use gpui::Modifiers;

    use super::*;

    #[test]
    fn wheel_delta_accumulates_with_remainder_preserved() {
        let mut accumulated = 0.0;
        accumulate_scroll_delta(&mut accumulated, 0.6);
        accumulate_scroll_delta(&mut accumulated, 0.6);

        let dispatched = take_integral_scroll_rows(&mut accumulated);

        assert_eq!(dispatched, 1);
        assert!((accumulated - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn sign_change_resets_pending_remainder() {
        let mut accumulated = 0.8;

        accumulate_scroll_delta(&mut accumulated, -0.25);

        assert!((accumulated + 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn multiple_wheel_events_before_flush_emit_single_scroll_batch() {
        let mut accumulator = crate::model::ScrollFrameAccumulator::default();

        accumulate_scroll_delta(&mut accumulator.accumulated_row_delta, 0.9);
        accumulate_scroll_delta(&mut accumulator.accumulated_row_delta, 1.4);

        let commands = take_scroll_dispatch_commands(&mut accumulator);

        assert_eq!(commands, vec![TerminalScrollCommand::DeltaRows(2)]);
        assert!((accumulator.accumulated_row_delta - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn mouse_tracking_wheel_routes_to_remote_by_default() {
        assert!(!mouse_tracking_wheel_routes_locally(
            TerminalScreenKind::Primary,
            MouseTrackingScrollPolicy::AlwaysAppFirst,
            Modifiers::default(),
        ));
    }

    #[test]
    fn mouse_tracking_shift_wheel_can_route_to_local_scrollback() {
        assert!(mouse_tracking_wheel_routes_locally(
            TerminalScreenKind::Primary,
            MouseTrackingScrollPolicy::HybridShiftWheelLocal,
            Modifiers::shift(),
        ));
        assert!(!mouse_tracking_wheel_routes_locally(
            TerminalScreenKind::Primary,
            MouseTrackingScrollPolicy::HybridShiftWheelLocal,
            Modifiers::default(),
        ));
    }

    #[test]
    fn mouse_tracking_wheel_local_override_is_disabled_on_alternate_screen() {
        assert!(!mouse_tracking_wheel_routes_locally(
            TerminalScreenKind::Alternate,
            MouseTrackingScrollPolicy::HybridShiftWheelLocal,
            Modifiers::shift(),
        ));
    }

    #[test]
    fn mouse_tracking_selection_shift_drag_policy_requires_shift() {
        assert!(mouse_tracking_selection_allows_local_override(
            MouseTrackingSelectionPolicy::ShiftDragLocal,
            Modifiers::shift(),
        ));
        assert!(!mouse_tracking_selection_allows_local_override(
            MouseTrackingSelectionPolicy::ShiftDragLocal,
            Modifiers::default(),
        ));
    }
}
