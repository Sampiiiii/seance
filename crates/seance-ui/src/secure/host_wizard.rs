// New Host wizard flow layered on top of the existing host draft pipeline.

use gpui::{Context, Div, MouseButton, Window, div, prelude::*, px};

use crate::{
    SeanceWorkspace,
    forms::{HostWizardStep, SecureSection},
    ui_components::{primary_button, settings_action_chip},
};

impl SeanceWorkspace {
    pub(crate) fn begin_new_host_wizard(&mut self, cx: &mut Context<Self>) {
        self.open_secure_workspace(SecureSection::Hosts, cx);
        self.activate_host_draft(None, cx);
        if self.secure.host_draft.is_none() {
            return;
        }
        self.secure.host_wizard.open = true;
        self.secure.host_wizard.step = HostWizardStep::ConnectionIdentity;
        self.secure.host_wizard.save_and_connect_requested = false;
        self.secure.host_wizard.message = Some("Step 1 of 3: connection identity.".into());
        self.show_toast("Started New Host Wizard.");
        cx.notify();
    }

    pub(crate) fn close_host_wizard(&mut self, cx: &mut Context<Self>) {
        self.secure.host_wizard.open = false;
        self.secure.host_wizard.save_and_connect_requested = false;
        self.secure.host_wizard.message = None;
        cx.notify();
    }

    pub(crate) fn host_wizard_next_step(&mut self, cx: &mut Context<Self>) {
        self.secure.host_wizard.step = match self.secure.host_wizard.step {
            HostWizardStep::ConnectionIdentity => HostWizardStep::AuthenticationSetup,
            HostWizardStep::AuthenticationSetup => HostWizardStep::Review,
            HostWizardStep::Review => HostWizardStep::Review,
        };
        self.secure.host_wizard.message = Some(match self.secure.host_wizard.step {
            HostWizardStep::ConnectionIdentity => "Step 1 of 3: connection identity.".into(),
            HostWizardStep::AuthenticationSetup => "Step 2 of 3: authentication setup.".into(),
            HostWizardStep::Review => "Step 3 of 3: review and save.".into(),
        });
        cx.notify();
    }

    pub(crate) fn host_wizard_previous_step(&mut self, cx: &mut Context<Self>) {
        self.secure.host_wizard.step = match self.secure.host_wizard.step {
            HostWizardStep::ConnectionIdentity => HostWizardStep::ConnectionIdentity,
            HostWizardStep::AuthenticationSetup => HostWizardStep::ConnectionIdentity,
            HostWizardStep::Review => HostWizardStep::AuthenticationSetup,
        };
        self.secure.host_wizard.message = Some(match self.secure.host_wizard.step {
            HostWizardStep::ConnectionIdentity => "Step 1 of 3: connection identity.".into(),
            HostWizardStep::AuthenticationSetup => "Step 2 of 3: authentication setup.".into(),
            HostWizardStep::Review => "Step 3 of 3: review and save.".into(),
        });
        cx.notify();
    }

    pub(crate) fn host_wizard_save(
        &mut self,
        save_and_connect: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scope = self
            .secure
            .host_draft
            .as_ref()
            .and_then(|draft| draft.vault_id.clone().zip(draft.host_id.clone()));

        self.save_host_draft(cx);

        if !save_and_connect {
            self.close_host_wizard(cx);
            return;
        }

        let next_scope = self
            .secure
            .host_draft
            .as_ref()
            .and_then(|draft| draft.vault_id.clone().zip(draft.host_id.clone()))
            .or(scope);
        if let Some((vault_id, host_id)) = next_scope {
            self.start_connect_attempt(&vault_id, &host_id, window, cx);
            self.close_host_wizard(cx);
            self.surface = crate::forms::WorkspaceSurface::Terminal;
        }
    }

    pub(crate) fn render_host_wizard_stepper(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        if !self.secure.host_wizard.open {
            return div();
        }

        let step_label = match self.secure.host_wizard.step {
            HostWizardStep::ConnectionIdentity => "1/3 identity",
            HostWizardStep::AuthenticationSetup => "2/3 auth",
            HostWizardStep::Review => "3/3 review",
        };

        div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(t.accent)
            .bg(t.accent_glow)
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_primary)
                            .child("New Host Wizard"),
                    )
                    .child(
                        div().text_xs().text_color(t.text_secondary).child(
                            self.secure
                                .host_wizard
                                .message
                                .clone()
                                .unwrap_or_else(|| step_label.to_string()),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(settings_action_chip("back", &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.host_wizard_previous_step(cx);
                        }),
                    ))
                    .child(settings_action_chip("next", &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.host_wizard_next_step(cx);
                        }),
                    ))
                    .child(primary_button("save", true, &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.host_wizard_save(false, window, cx);
                        }),
                    ))
                    .child(primary_button("save + connect", true, &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.host_wizard_save(true, window, cx);
                        }),
                    ))
                    .child(settings_action_chip("close", &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.close_host_wizard(cx);
                        }),
                    )),
            )
    }
}
