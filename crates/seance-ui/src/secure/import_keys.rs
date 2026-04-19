// Key import modal flows: ~/.ssh discovery, file picker import, and PEM paste import.

use gpui::{
    ClipboardItem, Context, Div, FontWeight, MouseButton, PathPromptOptions, Window, div,
    prelude::*, px,
};
use seance_core::ImportPrivateKeyFromPathRequest;
use seance_vault::{ImportKeyRequest, PrivateKeyAlgorithm, inspect_private_key_pem};

use crate::{
    SeanceWorkspace,
    forms::{KeyImportCandidateState, KeyImportTab},
    ui_components::{primary_button, settings_action_chip},
};

impl SeanceWorkspace {
    pub(crate) fn open_key_import_modal(&mut self, tab: KeyImportTab, cx: &mut Context<Self>) {
        let Some(vault_id) = self
            .secure
            .host_draft
            .as_ref()
            .and_then(|draft| draft.vault_id.clone())
            .or_else(|| self.default_target_vault_id())
        else {
            self.show_toast("Unlock a vault before importing keys.");
            return;
        };

        self.secure.key_import_modal.open = true;
        self.secure.key_import_modal.tab = tab;
        self.secure.key_import_modal.target_vault_id = Some(vault_id.clone());
        self.secure.key_import_modal.error = None;
        self.secure.key_import_modal.message = None;
        if tab == KeyImportTab::Discover {
            self.refresh_discovered_private_keys(cx);
        }
        cx.notify();
    }

    pub(crate) fn close_key_import_modal(&mut self, cx: &mut Context<Self>) {
        self.secure.key_import_modal.open = false;
        self.secure.key_import_modal.error = None;
        self.secure.key_import_modal.message = None;
        cx.notify();
    }

    pub(crate) fn refresh_discovered_private_keys(&mut self, cx: &mut Context<Self>) {
        let Some(vault_id) = self.secure.key_import_modal.target_vault_id.clone() else {
            self.secure.key_import_modal.error = Some("Choose a vault before discovery.".into());
            cx.notify();
            return;
        };

        self.secure.key_import_modal.discover_loading = true;
        match self.backend.discover_private_key_candidates(&vault_id) {
            Ok(candidates) => {
                self.secure.key_import_modal.discover_candidates = candidates
                    .into_iter()
                    .map(|candidate| KeyImportCandidateState {
                        path: candidate.path,
                        label: candidate.label,
                        algorithm: candidate.algorithm,
                        encrypted_at_rest: candidate.encrypted_at_rest,
                        duplicate_public_key: candidate.duplicate_public_key,
                        skip_duplicate: candidate.duplicate_public_key,
                    })
                    .collect();
                self.secure.key_import_modal.error = None;
                self.secure.key_import_modal.message = Some(format!(
                    "Discovered {} candidate key file(s).",
                    self.secure.key_import_modal.discover_candidates.len()
                ));
            }
            Err(err) => {
                self.secure.key_import_modal.error = Some(err.to_string());
            }
        }
        self.secure.key_import_modal.discover_loading = false;
        cx.notify();
    }

    pub(crate) fn toggle_discovered_duplicate_policy(
        &mut self,
        candidate_index: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(candidate) = self
            .secure
            .key_import_modal
            .discover_candidates
            .get_mut(candidate_index)
        else {
            return;
        };
        candidate.skip_duplicate = !candidate.skip_duplicate;
        cx.notify();
    }

    pub(crate) fn import_discovered_candidate(
        &mut self,
        candidate_index: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(vault_id) = self.secure.key_import_modal.target_vault_id.clone() else {
            self.show_toast("Choose a vault before importing keys.");
            return;
        };
        let Some(candidate) = self
            .secure
            .key_import_modal
            .discover_candidates
            .get(candidate_index)
            .cloned()
        else {
            return;
        };

        match self.backend.import_private_keys_from_paths(
            &vault_id,
            vec![ImportPrivateKeyFromPathRequest {
                path: candidate.path,
                label: Some(candidate.label),
                skip_duplicate: candidate.skip_duplicate,
            }],
        ) {
            Ok(imported) => {
                if let Some(summary) = imported.into_iter().next() {
                    self.on_private_key_created(summary, "Imported", cx);
                } else {
                    self.show_toast("Skipped duplicate key.");
                }
                self.refresh_discovered_private_keys(cx);
            }
            Err(err) => self.show_toast(err.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn open_key_file_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picker_rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Select private key files".into()),
        });
        let entity = cx.entity();
        window
            .spawn(cx, async move |cx| {
                let picked = picker_rx.await;
                let _ = cx.update(move |_window, cx| {
                    entity.update(cx, |this, cx| {
                        match picked {
                            Ok(Ok(Some(paths))) => {
                                this.secure.key_import_modal.selected_paths = paths;
                                this.secure.key_import_modal.error = None;
                                this.secure.key_import_modal.message = Some("Selected files ready to import.".into());
                            }
                            Ok(Ok(None)) => {
                                this.secure.key_import_modal.message = Some("File selection cancelled.".into());
                            }
                            Ok(Err(err)) => {
                                this.secure.key_import_modal.error = Some(format!(
                                    "File picker unavailable ({err}). On Linux without xdg-desktop-portal, use Discover or Paste."
                                ));
                            }
                            Err(_) => {
                                this.secure.key_import_modal.error = Some("File picker was cancelled before completion.".into());
                            }
                        }
                        cx.notify();
                    });
                });
            })
            .detach();
    }

    pub(crate) fn import_selected_path_keys(&mut self, cx: &mut Context<Self>) {
        let Some(vault_id) = self.secure.key_import_modal.target_vault_id.clone() else {
            self.show_toast("Choose a vault before importing keys.");
            return;
        };
        let requests = self
            .secure
            .key_import_modal
            .selected_paths
            .iter()
            .cloned()
            .map(|path| ImportPrivateKeyFromPathRequest {
                path,
                label: None,
                skip_duplicate: true,
            })
            .collect::<Vec<_>>();

        if requests.is_empty() {
            self.show_toast("Select one or more key files first.");
            return;
        }

        match self
            .backend
            .import_private_keys_from_paths(&vault_id, requests)
        {
            Ok(imported) => {
                if imported.is_empty() {
                    self.show_toast("No keys were imported (duplicates were skipped).");
                } else {
                    for summary in imported {
                        self.on_private_key_created(summary, "Imported", cx);
                    }
                }
            }
            Err(err) => self.show_toast(err.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn paste_key_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.secure.key_import_modal.error = Some("Clipboard does not contain text.".into());
            cx.notify();
            return;
        };

        self.secure.key_import_modal.paste_private_key_pem = text;
        self.secure.key_import_modal.paste_label = format!("imported-{}", crate::now_ui_suffix());
        self.secure.key_import_modal.error = None;
        cx.notify();
    }

    pub(crate) fn import_pasted_key(&mut self, cx: &mut Context<Self>) {
        let Some(vault_id) = self.secure.key_import_modal.target_vault_id.clone() else {
            self.show_toast("Choose a vault before importing keys.");
            return;
        };

        let private_key_pem = self.secure.key_import_modal.paste_private_key_pem.trim();
        if private_key_pem.is_empty() {
            self.secure.key_import_modal.error =
                Some("Paste a private key PEM block first.".into());
            cx.notify();
            return;
        }

        let label = if self.secure.key_import_modal.paste_label.trim().is_empty() {
            format!("imported-{}", crate::now_ui_suffix())
        } else {
            self.secure.key_import_modal.paste_label.trim().to_string()
        };

        self.import_private_key_for_secure(
            &vault_id,
            ImportKeyRequest {
                label,
                private_key_pem: private_key_pem.to_string(),
            },
            cx,
        );
    }

    fn render_key_import_tab_button(
        &self,
        tab: KeyImportTab,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme();
        let active = self.secure.key_import_modal.tab == tab;
        div()
            .px(px(10.0))
            .py(px(5.0))
            .rounded_full()
            .border_1()
            .border_color(if active { t.accent } else { t.glass_border })
            .bg(if active {
                t.accent_glow
            } else {
                gpui::transparent_black()
            })
            .text_xs()
            .text_color(if active {
                t.text_primary
            } else {
                t.text_secondary
            })
            .cursor_pointer()
            .hover(|s| if active { s } else { s.bg(t.glass_hover) })
            .child(label)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.secure.key_import_modal.tab = tab;
                    this.secure.key_import_modal.error = None;
                    if tab == KeyImportTab::Discover {
                        this.refresh_discovered_private_keys(cx);
                    }
                    cx.notify();
                }),
            )
    }

    pub(crate) fn render_key_import_modal(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme();
        if !self.secure.key_import_modal.open {
            return div();
        }

        let mut body = div().flex().flex_col().gap_3();
        match self.secure.key_import_modal.tab {
            KeyImportTab::Discover => {
                let mut discovered = div().flex().flex_col().gap(px(6.0));
                for (index, candidate) in self
                    .secure
                    .key_import_modal
                    .discover_candidates
                    .iter()
                    .enumerate()
                {
                    let algo = match candidate.algorithm {
                        PrivateKeyAlgorithm::Ed25519 => "ed25519",
                        PrivateKeyAlgorithm::Rsa { bits } => {
                            if bits == 4096 {
                                "rsa-4096"
                            } else {
                                "rsa"
                            }
                        }
                    };
                    discovered = discovered.child(
                        div()
                            .p_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(t.glass_border)
                            .bg(t.glass_tint)
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(t.text_primary)
                                    .child(candidate.label.clone()),
                            )
                            .child(div().text_xs().text_color(t.text_muted).child(format!(
                                "{} · {}",
                                algo,
                                candidate.path.display()
                            )))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(6.0))
                                    .child(settings_action_chip(
                                        if candidate.encrypted_at_rest {
                                            "encrypted"
                                        } else {
                                            "unencrypted"
                                        },
                                        &t,
                                    ))
                                    .when(candidate.duplicate_public_key, |row| {
                                        row.child(
                                            settings_action_chip("duplicate", &t).on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.toggle_discovered_duplicate_policy(
                                                        index, cx,
                                                    );
                                                }),
                                            ),
                                        )
                                    })
                                    .child(
                                        settings_action_chip(
                                            if candidate.skip_duplicate {
                                                "skip duplicate"
                                            } else {
                                                "import duplicate"
                                            },
                                            &t,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.toggle_discovered_duplicate_policy(index, cx);
                                            }),
                                        ),
                                    )
                                    .child(settings_action_chip("import", &t).on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.import_discovered_candidate(index, cx);
                                        }),
                                    )),
                            ),
                    );
                }

                body = body
                    .child(div().flex().gap(px(8.0)).child(
                        settings_action_chip("refresh", &t).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.refresh_discovered_private_keys(cx);
                            }),
                        ),
                    ))
                    .child(
                        div()
                            .id("key-import-discover-scroll")
                            .max_h(px(320.0))
                            .overflow_y_scroll()
                            .child(discovered),
                    );
            }
            KeyImportTab::Files => {
                let mut selected = div().flex().flex_col().gap(px(4.0));
                for path in &self.secure.key_import_modal.selected_paths {
                    selected = selected.child(
                        div()
                            .text_xs()
                            .text_color(t.text_secondary)
                            .child(path.display().to_string()),
                    );
                }

                body = body
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(settings_action_chip("pick files", &t).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.open_key_file_picker(window, cx);
                                }),
                            ))
                            .child(
                                primary_button(
                                    "import selected",
                                    !self.secure.key_import_modal.selected_paths.is_empty(),
                                    &t,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.import_selected_path_keys(cx);
                                    }),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .id("key-import-files-scroll")
                            .max_h(px(280.0))
                            .overflow_y_scroll()
                            .child(if self.secure.key_import_modal.selected_paths.is_empty() {
                                div()
                                    .text_sm()
                                    .text_color(t.text_ghost)
                                    .child("No files selected.")
                            } else {
                                selected
                            }),
                    );
            }
            KeyImportTab::Paste => {
                let pem = self.secure.key_import_modal.paste_private_key_pem.trim();
                let parser_feedback = if pem.is_empty() {
                    "Paste PEM from clipboard to validate.".to_string()
                } else {
                    match inspect_private_key_pem(pem) {
                        Ok(inspection) => {
                            let algo = match inspection.algorithm {
                                PrivateKeyAlgorithm::Ed25519 => "ed25519",
                                PrivateKeyAlgorithm::Rsa { bits } => {
                                    if bits == 4096 {
                                        "rsa-4096"
                                    } else {
                                        "rsa"
                                    }
                                }
                            };
                            format!(
                                "Valid private key: {} ({})",
                                algo,
                                if inspection.encrypted_at_rest {
                                    "encrypted"
                                } else {
                                    "unencrypted"
                                }
                            )
                        }
                        Err(err) => format!("Invalid PEM: {err}"),
                    }
                };

                body = body
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(settings_action_chip("paste clipboard", &t).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.paste_key_from_clipboard(cx);
                                }),
                            ))
                            .child(settings_action_chip("copy PEM", &t).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        this.secure.key_import_modal.paste_private_key_pem.clone(),
                                    ));
                                }),
                            ))
                            .child(
                                primary_button("import pasted key", !pem.is_empty(), &t)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.import_pasted_key(cx);
                                        }),
                                    ),
                            ),
                    )
                    .child(div().text_xs().text_color(t.text_secondary).child(format!(
                        "Label: {}",
                        if self.secure.key_import_modal.paste_label.is_empty() {
                            "imported-<timestamp>"
                        } else {
                            self.secure.key_import_modal.paste_label.as_str()
                        }
                    )))
                    .child(
                        div()
                            .id("key-import-paste-scroll")
                            .p_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(t.glass_border)
                            .bg(t.glass_tint)
                            .max_h(px(240.0))
                            .overflow_y_scroll()
                            .text_xs()
                            .text_color(t.text_secondary)
                            .child(if pem.is_empty() {
                                "No PEM text loaded.".to_string()
                            } else {
                                pem.to_string()
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text_muted)
                            .child(parser_feedback),
                    );
            }
        }

        if let Some(error) = self.secure.key_import_modal.error.as_ref() {
            body = body.child(self.render_banner(error, true));
        }
        if let Some(message) = self.secure.key_import_modal.message.as_ref() {
            body = body.child(self.render_banner(message, false));
        }

        div()
            .absolute()
            .size_full()
            .bg(t.scrim)
            .track_focus(&self.focus_handle)
            .key_context("KeyImportDialog")
            .on_mouse_down(MouseButton::Left, {
                let focus_handle = self.focus_handle.clone();
                move |_: &gpui::MouseDownEvent, window: &mut Window, _cx: &mut gpui::App| {
                    window.focus(&focus_handle);
                }
            })
            .on_key_down(cx.listener(Self::handle_key_down))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(760.0))
                    .max_h(px(620.0))
                    .p_5()
                    .rounded_xl()
                    .bg(t.glass_strong)
                    .border_1()
                    .border_color(t.glass_border_bright)
                    .flex()
                    .flex_col()
                    .gap_4()
                    .on_mouse_down(MouseButton::Left, {
                        let focus_handle = self.focus_handle.clone();
                        move |_: &gpui::MouseDownEvent, window: &mut Window, _cx: &mut gpui::App| {
                            window.focus(&focus_handle);
                        }
                    })
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.0))
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(t.text_primary)
                                            .child("Import SSH Keys"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(t.text_muted)
                                            .child("Discover ~/.ssh, pick files, or paste PEM."),
                                    ),
                            )
                            .child(settings_action_chip("close", &t).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.close_key_import_modal(cx);
                                }),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(self.render_key_import_tab_button(
                                KeyImportTab::Discover,
                                "Discover",
                                cx,
                            ))
                            .child(self.render_key_import_tab_button(
                                KeyImportTab::Files,
                                "Files",
                                cx,
                            ))
                            .child(self.render_key_import_tab_button(
                                KeyImportTab::Paste,
                                "Paste",
                                cx,
                            )),
                    )
                    .child(body),
            )
    }
}
