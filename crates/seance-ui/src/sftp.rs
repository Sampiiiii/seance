// Owns the SFTP browser state machine, file operations, and related rendering.

use std::{fs, path::PathBuf};

use gpui::{App, Context, Div, FontWeight, MouseButton, Window, div, prelude::*, px};
use seance_ssh::SftpEntry;

use crate::{
    SIDEBAR_FONT_MONO, SeanceWorkspace,
    theme::Theme,
    ui_components::{format_file_size, format_unix_perms, sftp_file_glyph},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SftpSortMode {
    Name,
    Size,
    Modified,
}

pub(crate) struct SftpBrowserState {
    session_id: u64,
    session_label: String,
    current_path: String,
    entries: Vec<SftpEntry>,
    selected_index: usize,
    sort_mode: SftpSortMode,
    error: Option<String>,
    path_history: Vec<String>,
    confirm_delete: Option<String>,
    rename_target: Option<(usize, String)>,
    mkdir_input: Option<String>,
    scroll_offset: usize,
}

impl SftpBrowserState {
    pub(crate) fn new(session_id: u64, session_label: String, initial_path: String) -> Self {
        Self {
            session_id,
            session_label,
            current_path: initial_path,
            entries: Vec::new(),
            selected_index: 0,
            sort_mode: SftpSortMode::Name,
            error: None,
            path_history: Vec::new(),
            confirm_delete: None,
            rename_target: None,
            mkdir_input: None,
            scroll_offset: 0,
        }
    }

    pub(crate) fn session_id(&self) -> u64 {
        self.session_id
    }

    fn selected_entry(&self) -> Option<&SftpEntry> {
        self.entries.get(self.selected_index)
    }

    fn sort_entries(&mut self) {
        self.entries.sort_by(|a, b| {
            match (a.is_dir, b.is_dir) {
                (true, false) => return std::cmp::Ordering::Less,
                (false, true) => return std::cmp::Ordering::Greater,
                _ => {}
            }
            match self.sort_mode {
                SftpSortMode::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SftpSortMode::Size => a.size.cmp(&b.size).reverse(),
                SftpSortMode::Modified => {
                    let am = a.modified.unwrap_or(0);
                    let bm = b.modified.unwrap_or(0);
                    am.cmp(&bm).reverse()
                }
            }
        });
    }

    fn parent_path(&self) -> Option<String> {
        let path = self.current_path.as_str();
        if path == "/" {
            return None;
        }
        let trimmed = path.trim_end_matches('/');
        match trimmed.rfind('/') {
            Some(0) => Some("/".into()),
            Some(idx) => Some(trimmed[..idx].to_string()),
            None => Some("/".into()),
        }
    }

    fn clamp_selection(&mut self) {
        if self.entries.is_empty() {
            self.selected_index = 0;
        } else if self.selected_index >= self.entries.len() {
            self.selected_index = self.entries.len() - 1;
        }
    }

    fn apply_listing_result(&mut self, result: anyhow::Result<Vec<SftpEntry>>) {
        match result {
            Ok(entries) => {
                self.entries = entries;
                self.sort_entries();
                self.error = None;
            }
            Err(err) => {
                self.entries.clear();
                self.error = Some(err.to_string());
            }
        }
        self.clamp_selection();
    }
}

impl SeanceWorkspace {
    pub(crate) fn open_sftp_browser(&mut self, session_id: u64, cx: &mut Context<Self>) {
        let label = self
            .sessions()
            .iter()
            .find(|s| s.id() == session_id)
            .map(|s| s.title().to_string())
            .unwrap_or_else(|| format!("session-{session_id}"));

        let home = match self.backend.sftp_canonicalize(session_id, ".") {
            Ok(path) => path,
            Err(_) => "/".into(),
        };

        let mut browser = SftpBrowserState::new(session_id, label, home);
        self.refresh_sftp_listing(&mut browser);
        self.sftp_browser = Some(browser);
        self.settings_panel.open = false;
        self.palette_open = false;
        cx.notify();
    }

    pub(crate) fn close_sftp_browser(&mut self, cx: &mut Context<Self>) {
        self.sftp_browser = None;
        cx.notify();
    }

    fn refresh_sftp_listing(&self, browser: &mut SftpBrowserState) {
        browser.apply_listing_result(
            self.backend
                .sftp_list_dir(browser.session_id, &browser.current_path),
        );
    }

    fn sftp_navigate(&mut self, path: String, cx: &mut Context<Self>) {
        if let Some(browser) = &mut self.sftp_browser {
            browser.path_history.push(browser.current_path.clone());
            browser.current_path = path;
            browser.selected_index = 0;
            browser.scroll_offset = 0;
        }
        self.sftp_refresh_current();
        cx.notify();
    }

    fn sftp_navigate_up(&mut self, cx: &mut Context<Self>) {
        let parent = self
            .sftp_browser
            .as_ref()
            .and_then(|browser| browser.parent_path());
        if let Some(parent) = parent {
            self.sftp_navigate(parent, cx);
        }
    }

    fn sftp_refresh_current(&mut self) {
        if let Some(browser) = &mut self.sftp_browser {
            let session_id = browser.session_id;
            let path = browser.current_path.clone();
            browser.apply_listing_result(self.backend.sftp_list_dir(session_id, &path));
        }
    }

    fn sftp_refresh(&mut self, cx: &mut Context<Self>) {
        self.sftp_refresh_current();
        cx.notify();
    }

    fn sftp_download_selected(&mut self, cx: &mut Context<Self>) {
        let (session_id, remote_path, file_name) = {
            let Some(browser) = &self.sftp_browser else {
                return;
            };
            let Some(entry) = browser.selected_entry() else {
                return;
            };
            if entry.is_dir {
                return;
            }
            (browser.session_id, entry.path.clone(), entry.name.clone())
        };

        match self.backend.sftp_read_file(session_id, &remote_path) {
            Ok(data) => {
                let downloads = dirs::download_dir().unwrap_or_else(|| PathBuf::from("."));
                let dest = downloads.join(&file_name);
                match fs::write(&dest, &data) {
                    Ok(()) => {
                        self.show_toast(format!(
                            "Downloaded {} ({} bytes) to {}",
                            file_name,
                            data.len(),
                            dest.display()
                        ));
                    }
                    Err(err) => {
                        self.show_toast(format!("Failed to save {file_name}: {err}"));
                    }
                }
            }
            Err(err) => {
                self.show_toast(format!("Download failed: {err}"));
            }
        }
        cx.notify();
    }

    #[allow(dead_code)]
    fn sftp_upload_file(&mut self, local_path: &std::path::Path, cx: &mut Context<Self>) {
        let Some(browser) = &self.sftp_browser else {
            return;
        };
        let session_id = browser.session_id;
        let file_name = local_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("uploaded_file")
            .to_string();
        let remote_path = if browser.current_path == "/" {
            format!("/{file_name}")
        } else {
            format!("{}/{file_name}", browser.current_path)
        };

        match fs::read(local_path) {
            Ok(data) => match self
                .backend
                .sftp_write_file(session_id, &remote_path, &data)
            {
                Ok(()) => {
                    self.show_toast(format!("Uploaded {file_name}"));
                    self.sftp_refresh(cx);
                    return;
                }
                Err(err) => {
                    self.show_toast(format!("Upload failed: {err}"));
                }
            },
            Err(err) => {
                self.show_toast(format!("Failed to read local file: {err}"));
            }
        }
        cx.notify();
    }

    fn sftp_delete_selected(&mut self, cx: &mut Context<Self>) {
        let (session_id, path, is_dir) = {
            let Some(browser) = &self.sftp_browser else {
                return;
            };
            let Some(entry) = browser.selected_entry() else {
                return;
            };
            (browser.session_id, entry.path.clone(), entry.is_dir)
        };

        match self.backend.sftp_remove(session_id, &path, is_dir) {
            Ok(()) => {
                self.show_toast(format!("Deleted {path}"));
            }
            Err(err) => {
                self.show_toast(format!("Delete failed: {err}"));
            }
        }
        if let Some(browser) = &mut self.sftp_browser {
            browser.confirm_delete = None;
        }
        self.sftp_refresh(cx);
    }

    fn sftp_mkdir_confirm(&mut self, cx: &mut Context<Self>) {
        let (session_id, full_path) = {
            let Some(browser) = &self.sftp_browser else {
                return;
            };
            let Some(name) = &browser.mkdir_input else {
                return;
            };
            let name = name.trim();
            if name.is_empty() {
                return;
            }
            let full_path = if browser.current_path == "/" {
                format!("/{name}")
            } else {
                format!("{}/{name}", browser.current_path)
            };
            (browser.session_id, full_path)
        };

        match self.backend.sftp_mkdir(session_id, &full_path) {
            Ok(()) => {
                self.show_toast(format!("Created {full_path}"));
            }
            Err(err) => {
                self.show_toast(format!("mkdir failed: {err}"));
            }
        }
        if let Some(browser) = &mut self.sftp_browser {
            browser.mkdir_input = None;
        }
        self.sftp_refresh(cx);
    }

    fn sftp_rename_confirm(&mut self, cx: &mut Context<Self>) {
        let (session_id, old_path, new_path) = {
            let Some(browser) = &self.sftp_browser else {
                return;
            };
            let Some((idx, new_name)) = &browser.rename_target else {
                return;
            };
            let new_name = new_name.trim();
            if new_name.is_empty() {
                return;
            }
            let Some(entry) = browser.entries.get(*idx) else {
                return;
            };
            let new_path = if browser.current_path == "/" {
                format!("/{new_name}")
            } else {
                format!("{}/{new_name}", browser.current_path)
            };
            (browser.session_id, entry.path.clone(), new_path)
        };

        match self.backend.sftp_rename(session_id, &old_path, &new_path) {
            Ok(()) => {
                self.show_toast(format!("Renamed to {new_path}"));
            }
            Err(err) => {
                self.show_toast(format!("Rename failed: {err}"));
            }
        }
        if let Some(browser) = &mut self.sftp_browser {
            browser.rename_target = None;
        }
        self.sftp_refresh(cx);
    }

    pub(crate) fn handle_sftp_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;

        if let Some(browser) = &self.sftp_browser {
            if browser.mkdir_input.is_some() {
                match key {
                    "escape" => {
                        if let Some(browser) = &mut self.sftp_browser {
                            browser.mkdir_input = None;
                        }
                        cx.notify();
                        return;
                    }
                    "enter" => {
                        self.sftp_mkdir_confirm(cx);
                        return;
                    }
                    "backspace" => {
                        if let Some(browser) = &mut self.sftp_browser
                            && let Some(input) = &mut browser.mkdir_input
                        {
                            input.pop();
                        }
                        cx.notify();
                        return;
                    }
                    _ => {
                        if let Some(ch) = event.keystroke.key_char.as_deref() {
                            if let Some(browser) = &mut self.sftp_browser
                                && let Some(input) = &mut browser.mkdir_input
                            {
                                input.push_str(ch);
                            }
                            cx.notify();
                        }
                        return;
                    }
                }
            }

            if browser.rename_target.is_some() {
                match key {
                    "escape" => {
                        if let Some(browser) = &mut self.sftp_browser {
                            browser.rename_target = None;
                        }
                        cx.notify();
                        return;
                    }
                    "enter" => {
                        self.sftp_rename_confirm(cx);
                        return;
                    }
                    "backspace" => {
                        if let Some(browser) = &mut self.sftp_browser
                            && let Some((_, name)) = &mut browser.rename_target
                        {
                            name.pop();
                        }
                        cx.notify();
                        return;
                    }
                    _ => {
                        if let Some(ch) = event.keystroke.key_char.as_deref() {
                            if let Some(browser) = &mut self.sftp_browser
                                && let Some((_, name)) = &mut browser.rename_target
                            {
                                name.push_str(ch);
                            }
                            cx.notify();
                        }
                        return;
                    }
                }
            }

            if browser.confirm_delete.is_some() {
                match key {
                    "y" => {
                        self.sftp_delete_selected(cx);
                        return;
                    }
                    _ => {
                        if let Some(browser) = &mut self.sftp_browser {
                            browser.confirm_delete = None;
                        }
                        cx.notify();
                        return;
                    }
                }
            }
        }

        match key {
            "escape" => {
                self.close_sftp_browser(cx);
            }
            "up" | "k" if !mods.platform => {
                if let Some(browser) = &mut self.sftp_browser {
                    browser.selected_index = browser.selected_index.saturating_sub(1);
                    if browser.selected_index < browser.scroll_offset {
                        browser.scroll_offset = browser.selected_index;
                    }
                }
                cx.notify();
            }
            "down" | "j" if !mods.platform => {
                if let Some(browser) = &mut self.sftp_browser
                    && !browser.entries.is_empty()
                {
                    browser.selected_index =
                        (browser.selected_index + 1).min(browser.entries.len() - 1);
                }
                cx.notify();
            }
            "enter" => {
                let action = self
                    .sftp_browser
                    .as_ref()
                    .and_then(|browser| browser.selected_entry())
                    .map(|entry| (entry.is_dir, entry.path.clone(), entry.name.clone()));
                if let Some((is_dir, path, name)) = action {
                    if name == ".." {
                        self.sftp_navigate_up(cx);
                    } else if is_dir {
                        self.sftp_navigate(path, cx);
                    } else {
                        self.sftp_download_selected(cx);
                    }
                }
            }
            "backspace" => {
                self.sftp_navigate_up(cx);
            }
            "delete" => {
                if let Some(browser) = &mut self.sftp_browser
                    && let Some(entry) = browser.selected_entry()
                {
                    browser.confirm_delete = Some(entry.name.clone());
                }
                cx.notify();
            }
            "n" if !mods.platform => {
                if let Some(browser) = &mut self.sftp_browser {
                    browser.mkdir_input = Some(String::new());
                }
                cx.notify();
            }
            "r" if !mods.platform && !mods.shift => {
                if let Some(browser) = &mut self.sftp_browser
                    && let Some(entry) = browser.entries.get(browser.selected_index)
                {
                    let idx = browser.selected_index;
                    let name = entry.name.clone();
                    browser.rename_target = Some((idx, name));
                }
                cx.notify();
            }
            "r" if mods.platform => {
                self.sftp_refresh(cx);
            }
            "s" if !mods.platform => {
                if let Some(browser) = &mut self.sftp_browser {
                    browser.sort_mode = match browser.sort_mode {
                        SftpSortMode::Name => SftpSortMode::Size,
                        SftpSortMode::Size => SftpSortMode::Modified,
                        SftpSortMode::Modified => SftpSortMode::Name,
                    };
                    browser.sort_entries();
                    browser.clamp_selection();
                }
                cx.notify();
            }
            _ => {}
        }
    }

    pub(crate) fn render_sftp_panel(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let Some(browser) = &self.sftp_browser else {
            return div();
        };

        let mut content = div()
            .flex_1()
            .h_full()
            .bg(t.bg_void)
            .track_focus(&self.focus_handle)
            .on_mouse_down(MouseButton::Left, {
                let focus_handle = self.focus_handle.clone();
                move |_: &gpui::MouseDownEvent, window: &mut Window, _cx: &mut App| {
                    window.focus(&focus_handle);
                }
            })
            .on_key_down(cx.listener(Self::handle_key_down))
            .overflow_hidden()
            .flex()
            .flex_col();

        content = content.child(self.render_sftp_breadcrumb(browser, &t, cx));
        content = content.child(self.render_sftp_toolbar(browser, &t, cx));

        if let Some(err) = &browser.error {
            content = content.child(
                div()
                    .px_6()
                    .py_3()
                    .bg(gpui::hsla(0.0, 0.6, 0.2, 0.3))
                    .border_b_1()
                    .border_color(gpui::hsla(0.0, 0.5, 0.3, 0.5))
                    .text_sm()
                    .text_color(gpui::hsla(0.0, 0.8, 0.7, 1.0))
                    .child(err.clone()),
            );
        }

        if let Some(name) = &browser.confirm_delete {
            content = content.child(
                div()
                    .px_6()
                    .py_3()
                    .bg(gpui::hsla(0.0, 0.4, 0.15, 0.5))
                    .border_b_1()
                    .border_color(gpui::hsla(0.0, 0.5, 0.3, 0.5))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(gpui::hsla(0.0, 0.8, 0.75, 1.0))
                            .child(format!("Delete \"{name}\"?")),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .px_2()
                            .py(px(2.0))
                            .rounded(px(3.0))
                            .bg(gpui::hsla(0.0, 0.5, 0.3, 0.6))
                            .text_color(gpui::hsla(0.0, 0.9, 0.85, 1.0))
                            .child("y confirm"),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .px_2()
                            .py(px(2.0))
                            .rounded(px(3.0))
                            .bg(t.glass_tint)
                            .text_color(t.text_muted)
                            .child("any key cancel"),
                    ),
            );
        }

        if let Some(input) = &browser.mkdir_input {
            content = content.child(
                div()
                    .px_6()
                    .py_3()
                    .bg(t.glass_tint)
                    .border_b_1()
                    .border_color(t.glass_border)
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_secondary)
                            .child("New folder:"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_sm()
                            .text_color(t.text_primary)
                            .child(if input.is_empty() {
                                "\u{2588}".to_string()
                            } else {
                                format!("{input}\u{2588}")
                            }),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child("enter confirm · esc cancel"),
                    ),
            );
        }

        if let Some((_, new_name)) = &browser.rename_target {
            content = content.child(
                div()
                    .px_6()
                    .py_3()
                    .bg(t.glass_tint)
                    .border_b_1()
                    .border_color(t.glass_border)
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_secondary)
                            .child("Rename to:"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_sm()
                            .text_color(t.text_primary)
                            .child(if new_name.is_empty() {
                                "\u{2588}".to_string()
                            } else {
                                format!("{new_name}\u{2588}")
                            }),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child("enter confirm · esc cancel"),
                    ),
            );
        }

        let header_row = div()
            .px_6()
            .py(px(6.0))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(t.glass_border)
            .child(
                div()
                    .w(px(28.0))
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_xs()
                    .text_color(t.text_ghost)
                    .child(""),
            )
            .child(
                div()
                    .flex_1()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_xs()
                    .text_color(t.text_ghost)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("NAME"),
            )
            .child(
                div()
                    .w(px(80.0))
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_xs()
                    .text_color(t.text_ghost)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_right()
                    .child("SIZE"),
            )
            .child(
                div()
                    .w(px(80.0))
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_xs()
                    .text_color(t.text_ghost)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_right()
                    .child("PERMS"),
            );
        content = content.child(header_row);

        let mut file_list = div().flex_1().flex().flex_col().overflow_hidden();

        if browser.entries.is_empty() && browser.error.is_none() {
            file_list = file_list.child(
                div()
                    .px_6()
                    .py_8()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_ghost)
                            .child("Empty directory"),
                    ),
            );
        } else {
            for (idx, entry) in browser.entries.iter().enumerate() {
                let selected = idx == browser.selected_index;
                let is_dir = entry.is_dir;
                let entry_path = entry.path.clone();

                let glyph = if entry.name == ".." {
                    "\u{2190}"
                } else if is_dir {
                    "\u{25b8}"
                } else {
                    sftp_file_glyph(&entry.name)
                };

                let glyph_color = if entry.name == ".." {
                    t.text_muted
                } else if is_dir {
                    t.accent
                } else {
                    t.text_ghost
                };

                let name_color = if selected {
                    t.text_primary
                } else if is_dir {
                    t.accent
                } else {
                    t.text_secondary
                };

                let size_str = if is_dir {
                    "—".to_string()
                } else {
                    format_file_size(entry.size)
                };

                let perms_str = entry
                    .permissions
                    .map(format_unix_perms)
                    .unwrap_or_else(|| "—".into());

                let row = div()
                    .px_6()
                    .py(px(4.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .when(selected, |element| {
                        element.bg(t.glass_tint).border_l_2().border_color(t.accent)
                    })
                    .when(!selected, |element| {
                        element.hover(|style| style.bg(t.glass_hover))
                    })
                    .child(
                        div()
                            .w(px(28.0))
                            .text_sm()
                            .text_color(glyph_color)
                            .child(glyph),
                    )
                    .child(
                        div()
                            .flex_1()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_sm()
                            .text_color(name_color)
                            .font_weight(if is_dir {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .line_clamp(1)
                            .child(entry.name.clone()),
                    )
                    .child(
                        div()
                            .w(px(80.0))
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .text_color(t.text_ghost)
                            .text_right()
                            .child(size_str),
                    )
                    .child(
                        div()
                            .w(px(80.0))
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .text_color(t.text_ghost)
                            .text_right()
                            .child(perms_str),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            if let Some(browser) = &mut this.sftp_browser {
                                browser.selected_index = idx;
                            }
                            if event.click_count == 2 {
                                let entry_info = this
                                    .sftp_browser
                                    .as_ref()
                                    .and_then(|browser| browser.entries.get(idx))
                                    .map(|entry| (entry.is_dir, entry.name.clone()));
                                if let Some((is_dir, name)) = entry_info {
                                    if name == ".." {
                                        this.sftp_navigate_up(cx);
                                    } else if is_dir {
                                        this.sftp_navigate(entry_path.clone(), cx);
                                    } else {
                                        this.sftp_download_selected(cx);
                                    }
                                }
                            }
                            cx.notify();
                        }),
                    );

                file_list = file_list.child(row);
            }
        }

        content = content.child(file_list);

        let sort_label = match browser.sort_mode {
            SftpSortMode::Name => "name",
            SftpSortMode::Size => "size",
            SftpSortMode::Modified => "date",
        };
        let status = div()
            .px_6()
            .py(px(5.0))
            .border_t_1()
            .border_color(t.glass_border)
            .bg(t.glass_tint)
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child(format!("{} items", browser.entries.len())),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child(format!("sort: {sort_label} (s)")),
                    ),
            )
            .child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_xs()
                    .text_color(t.text_ghost)
                    .child("↑↓ navigate · enter open · ⌫ up · n mkdir · r rename · del delete"),
            );
        content = content.child(status);

        div()
            .flex_1()
            .h_full()
            .flex()
            .child(content)
    }

    fn render_sftp_breadcrumb(
        &self,
        browser: &SftpBrowserState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut breadcrumb = div()
            .px_6()
            .py_4()
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(theme.glass_border);

        let mut path_row = div().flex().items_center().gap(px(2.0));

        let segments: Vec<&str> = browser
            .current_path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .collect();

        path_row = path_row.child(
            div()
                .font_family(SIDEBAR_FONT_MONO)
                .text_sm()
                .text_color(theme.text_ghost)
                .cursor_pointer()
                .hover(|style| style.text_color(theme.accent))
                .child("/")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.sftp_navigate("/".into(), cx);
                    }),
                ),
        );

        let mut accumulated = String::new();
        for (index, segment) in segments.iter().enumerate() {
            accumulated.push('/');
            accumulated.push_str(segment);
            let nav_path = accumulated.clone();
            let is_last = index == segments.len() - 1;

            path_row = path_row.child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_xs()
                    .text_color(theme.text_ghost)
                    .child("/"),
            );

            path_row = path_row.child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_sm()
                    .text_color(if is_last {
                        theme.text_primary
                    } else {
                        theme.text_secondary
                    })
                    .font_weight(if is_last {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.accent))
                    .child(segment.to_string())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.sftp_navigate(nav_path.clone(), cx);
                        }),
                    ),
            );
        }

        breadcrumb = breadcrumb.child(path_row);

        let right_side = div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_xs()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(3.0))
                    .bg(theme.accent_glow)
                    .text_color(theme.accent)
                    .child(browser.session_label.clone()),
            )
            .child(
                div()
                    .px_3()
                    .py(px(6.0))
                    .rounded_md()
                    .bg(theme.glass_tint)
                    .border_1()
                    .border_color(theme.glass_border)
                    .text_xs()
                    .text_color(theme.text_secondary)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.glass_hover).text_color(theme.text_primary))
                    .child("esc  back to terminal")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.close_sftp_browser(cx);
                        }),
                    ),
            );
        breadcrumb = breadcrumb.child(right_side);

        breadcrumb
    }

    fn render_sftp_toolbar(
        &self,
        browser: &SftpBrowserState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let has_selection = browser.selected_entry().is_some();
        let selected_is_file = browser
            .selected_entry()
            .map(|entry| !entry.is_dir)
            .unwrap_or(false);

        div()
            .px_6()
            .py(px(6.0))
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(theme.glass_border)
            .child(sftp_toolbar_pill(
                "↑ Up",
                browser.parent_path().is_some(),
                theme,
                cx,
                |this, _, _, cx| {
                    this.sftp_navigate_up(cx);
                },
            ))
            .child(sftp_toolbar_pill(
                "↻ Refresh",
                true,
                theme,
                cx,
                |this, _, _, cx| {
                    this.sftp_refresh(cx);
                },
            ))
            .child(sftp_toolbar_pill(
                "+ Folder",
                true,
                theme,
                cx,
                |this, _, _, cx| {
                    if let Some(browser) = &mut this.sftp_browser {
                        browser.mkdir_input = Some(String::new());
                    }
                    cx.notify();
                },
            ))
            .child(sftp_toolbar_pill(
                "⇣ Download",
                selected_is_file,
                theme,
                cx,
                |this, _, _, cx| {
                    this.sftp_download_selected(cx);
                },
            ))
            .child(sftp_toolbar_pill(
                "✎ Rename",
                has_selection,
                theme,
                cx,
                |this, _, _, cx| {
                    if let Some(browser) = &mut this.sftp_browser
                        && let Some(entry) = browser.entries.get(browser.selected_index)
                    {
                        let idx = browser.selected_index;
                        let name = entry.name.clone();
                        browser.rename_target = Some((idx, name));
                    }
                    cx.notify();
                },
            ))
            .child(sftp_toolbar_pill(
                "× Delete",
                has_selection,
                theme,
                cx,
                |this, _, _, cx| {
                    if let Some(browser) = &mut this.sftp_browser
                        && let Some(entry) = browser.selected_entry()
                    {
                        browser.confirm_delete = Some(entry.name.clone());
                    }
                    cx.notify();
                },
            ))
    }
}

fn sftp_toolbar_pill(
    label: &'static str,
    enabled: bool,
    theme: &Theme,
    cx: &mut Context<SeanceWorkspace>,
    handler: impl Fn(
        &mut SeanceWorkspace,
        &gpui::MouseDownEvent,
        &mut Window,
        &mut Context<SeanceWorkspace>,
    ) + 'static,
) -> Div {
    let pill = div()
        .font_family(SIDEBAR_FONT_MONO)
        .text_xs()
        .px_3()
        .py(px(4.0))
        .rounded(px(4.0))
        .border_1();

    if enabled {
        pill.bg(theme.glass_tint)
            .border_color(theme.glass_border)
            .text_color(theme.text_secondary)
            .cursor_pointer()
            .hover(|style| style.bg(theme.glass_hover).text_color(theme.text_primary))
            .child(label)
            .on_mouse_down(MouseButton::Left, cx.listener(handler))
    } else {
        pill.bg(gpui::hsla(0.0, 0.0, 0.1, 0.3))
            .border_color(gpui::hsla(0.0, 0.0, 0.2, 0.2))
            .text_color(theme.text_ghost)
            .child(label)
    }
}
