use super::{Action, Desktop};
use crate::workspace::update::{Coordinator, Effect, Message};
use eframe::egui;

impl Desktop {
    pub(super) fn install_update(&mut self, ctx: &egui::Context) {
        if self.paired_update.is_some() || self.install_job.is_some() || self.install_pending {
            return;
        }
        if self.pair.remote.is_some()
            && self.prepared_update.is_some()
            && self.update_version.is_some()
        {
            self.update_confirmation = true;
            self.action(Action::Show, ctx);
        } else {
            self.begin_install_update(ctx);
        }
    }

    pub(super) fn start_paired_update(&mut self, ctx: &egui::Context) {
        let Some(remote) = &self.pair.remote else {
            self.notice = Some((
                "Reconnect the other PC before updating both computers.".into(),
                true,
            ));
            return;
        };
        let Some(version) = self.update_version.clone() else {
            return;
        };
        if self.prepared_update.is_none() {
            return;
        }
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        self.paired_update = Some(Coordinator::outgoing(
            id.max(1),
            version,
            cfg!(target_os = "linux"),
            remote.diagnostics.platform == "linux",
        ));
        let message = self.paired_update.as_ref().unwrap().retry().unwrap();
        self.apply_update_effects(vec![Effect::Send(message)], ctx);
    }

    pub(super) fn receive_update(&mut self, message: Message, ctx: &egui::Context) {
        if let Message::Prepare { id, version } = &message
            && self.paired_update.is_none()
        {
            let result = if self.install_job.is_some() || self.install_pending || self.quitting {
                Err(anyhow::anyhow!("This PC is already installing an update"))
            } else {
                Coordinator::incoming(*id, version.clone(), cfg!(target_os = "linux"))
            };
            match result {
                Ok(coordinator) => {
                    self.update_confirmation = false;
                    self.paired_update = Some(coordinator);
                    self.page = super::Page::Application;
                    self.action(Action::Show, ctx);
                    let effects = if self.prepared_update.is_some()
                        && self.update_version.as_deref() == Some(version)
                    {
                        self.paired_update.as_mut().unwrap().prepared(version)
                    } else {
                        vec![Effect::Download]
                    };
                    self.apply_update_effects(effects, ctx);
                }
                Err(error) => self.apply_update_effects(
                    vec![Effect::Send(Message::Failed {
                        id: *id,
                        message: error.to_string(),
                    })],
                    ctx,
                ),
            }
            return;
        }
        let Some(coordinator) = &mut self.paired_update else {
            return;
        };
        if coordinator.id != message.id() {
            if let Message::Prepare { id, .. } = message {
                self.apply_update_effects(
                    vec![Effect::Send(Message::Failed {
                        id,
                        message: "Another paired update is already in progress".into(),
                    })],
                    ctx,
                );
            }
            return;
        }
        match coordinator.receive(message) {
            Ok(effects) => self.apply_update_effects(effects, ctx),
            Err(error) => self.fail_paired_update(error.to_string(), ctx),
        }
    }

    pub(super) fn sent_update(&mut self, message: &Message, ctx: &egui::Context) {
        if let Some(coordinator) = &mut self.paired_update {
            let effects = coordinator.sent(message);
            self.apply_update_effects(effects, ctx);
        }
    }

    pub(super) fn paired_download_ready(&mut self, ctx: &egui::Context) {
        if let Some(coordinator) = &mut self.paired_update {
            let effects = if self.prepared_update.is_some() {
                coordinator.prepared(self.update_version.as_deref().unwrap_or_default())
            } else {
                coordinator.fail(
                    "Paired installation requires the installed .deb or Windows .exe edition."
                        .into(),
                    true,
                )
            };
            self.apply_update_effects(effects, ctx);
        }
    }

    pub(super) fn fail_paired_update(&mut self, error: String, ctx: &egui::Context) {
        if let Some(coordinator) = &mut self.paired_update {
            let effects = coordinator.fail(error, true);
            self.apply_update_effects(effects, ctx);
        }
    }

    pub(super) fn apply_update_effects(&mut self, effects: Vec<Effect>, ctx: &egui::Context) {
        for effect in effects {
            match effect {
                Effect::Send(message) => {
                    if let Some(service) = &self.pair.service {
                        if let Err(error) = service.update(message) {
                            self.notice = Some((error.to_string(), true));
                        }
                    } else {
                        self.notice = Some((
                            "Waiting to reconnect before continuing the paired update.".into(),
                            true,
                        ));
                    }
                }
                Effect::Download => self.check_updates(ctx),
                Effect::Install => {
                    if self.prepared_update.is_some() {
                        self.begin_install_update(ctx);
                    } else {
                        self.fail_paired_update(
                            "The verified installer is unavailable. Check for updates again."
                                .into(),
                            ctx,
                        );
                    }
                }
                Effect::Quit => self.action(Action::Quit, ctx),
                Effect::Error(error) => {
                    self.update_status = error.clone();
                    self.notice = Some((error, true));
                    self.update_ready = self.prepared_update.is_some();
                }
            }
        }
        if self.paired_update.as_ref().is_some_and(Coordinator::done) {
            self.paired_update = None;
        }
        if let Some(coordinator) = &self.paired_update {
            self.update_status = format!(
                "{}: {}",
                coordinator.version,
                crate::i18n::tr(coordinator.status())
            );
        }
    }

    pub(super) fn tick_paired_update(&mut self, ctx: &egui::Context) {
        if self.install_job.is_none()
            && let Some(coordinator) = &mut self.paired_update
        {
            let effects = coordinator.expired(self.pair.remote.is_some());
            self.apply_update_effects(effects, ctx);
        }
    }

    pub(super) fn paired_update_dialog(&mut self, ctx: &egui::Context) {
        if !self.update_confirmation {
            return;
        }
        egui::Modal::new(egui::Id::new("paired-update-confirmation")).show(ctx, |ui| {
            ui.set_max_width(480.0);
            ui.heading(crate::i18n::tr("Update the linked PC too?"));
            ui.label(crate::i18n::tr("Both PCs will download and verify the same version. Ubuntu requires its system password on that PC; Windows waits for Ubuntu's installation to succeed."));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(crate::i18n::tr("Update both PCs")).clicked() { self.update_confirmation = false; self.start_paired_update(ctx); }
                if ui.button(crate::i18n::tr("Only this PC")).clicked() { self.update_confirmation = false; self.begin_install_update(ctx); }
                if ui.button(crate::i18n::tr("Cancel")).clicked() { self.update_confirmation = false; }
            });
        });
    }
}
