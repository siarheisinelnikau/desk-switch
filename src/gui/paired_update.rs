use super::{Action, Desktop};
use crate::workspace::update::{Coordinator, Effect, Message};
use eframe::egui;

pub(super) enum DownloadAction {
    Install,
    Pair,
}

impl Desktop {
    pub(super) fn update_action_busy(&self) -> bool {
        self.paired_update.is_some()
            || self.peer_restart.is_some()
            || self.update_job.is_some()
            || self.install_job.is_some()
            || self.install_pending
            || self.quitting
    }

    fn accept_update_action(&mut self) -> bool {
        if self.update_action_busy() {
            self.notice = Some((
                "Another update operation is already in progress.".into(),
                true,
            ));
            false
        } else {
            true
        }
    }

    pub(super) fn install_selected_update(&mut self, target: &str, ctx: &egui::Context) {
        if !self.accept_update_action() {
            return;
        }
        if target == self.public_key {
            if !self.update_ready {
                return;
            }
            self.update_confirmation = false;
            self.group.update_target = None;
            self.begin_install_update(ctx);
            return;
        }
        let remote = self
            .group
            .workspace
            .as_ref()
            .and_then(|state| state.members.get(target))
            .filter(|member| member.online)
            .map(|member| member.node.clone());
        let Some(remote) = remote else {
            self.notice = Some((
                "Reconnect the selected computer before updating it.".into(),
                true,
            ));
            return;
        };
        self.start_remote_update_on(&remote, ctx);
    }

    pub(super) fn install_update(&mut self, ctx: &egui::Context) {
        if !self.accept_update_action() {
            return;
        }
        if !self.update_ready {
            self.start_remote_update(ctx);
            return;
        }
        if self.pair.remote.is_some()
            && (self.prepared_update.is_some() || self.candidate_update.is_some())
            && self.update_version.is_some()
            && self.pair.remote.as_ref().is_some_and(|remote| {
                remote.management.is_some()
                    && remote.version.as_ref().is_some_and(|installed| {
                        semver::Version::parse(&installed.app).ok()
                            < self
                                .update_version
                                .as_deref()
                                .and_then(|v| semver::Version::parse(v).ok())
                    })
            })
        {
            self.update_confirmation = true;
            self.action(Action::Show, ctx);
        } else {
            self.begin_install_update(ctx);
        }
    }

    pub(super) fn start_remote_update(&mut self, ctx: &egui::Context) {
        if !self.accept_update_action() {
            return;
        }
        let Some(remote) = self.pair.remote.clone() else {
            self.notice = Some((
                "Reconnect the selected computer before updating it.".into(),
                true,
            ));
            return;
        };
        self.start_remote_update_on(&remote, ctx);
    }

    fn start_remote_update_on(&mut self, remote: &crate::workspace::Node, ctx: &egui::Context) {
        let Some(remote) = Some(remote).filter(|node| {
            node.management
                .as_ref()
                .is_some_and(|state| state.update.can_install())
        }) else {
            self.notice = Some((
                "Check for updates on the other PC before installing.".into(),
                true,
            ));
            return;
        };
        let version = remote
            .management
            .as_ref()
            .unwrap()
            .update
            .version
            .clone()
            .unwrap();
        // Keep the recipient stable while another computer is selected in Settings.
        self.group.update_target = Some(remote.key.clone());
        self.update_confirmation = false;
        self.paired_update = Some(
            Coordinator::peer_only(
                super::management::request_id(),
                version,
                remote.diagnostics.platform == "linux",
            )
            .without_password(),
        );
        self.peer_update_status = None;
        let message = self.paired_update.as_ref().unwrap().retry().unwrap();
        self.apply_update_effects(vec![Effect::Send(message)], ctx);
    }

    pub(super) fn start_paired_update(&mut self, ctx: &egui::Context) {
        if !self.accept_update_action() {
            return;
        }
        let remote = if let Some(target) = &self.group.update_target {
            self.group
                .workspace
                .as_ref()
                .and_then(|state| state.members.get(target))
                .map(|member| member.node.clone())
                .or_else(|| {
                    self.pair
                        .remote
                        .as_ref()
                        .filter(|node| &node.key == target)
                        .cloned()
                })
        } else {
            self.pair.remote.clone()
        };
        let Some(remote) = remote else {
            self.notice = Some((
                "Reconnect the other PC before updating both computers.".into(),
                true,
            ));
            return;
        };
        self.group.update_target = Some(remote.key.clone());
        let Some(version) = self.update_version.clone() else {
            return;
        };
        if self.prepared_update.is_none() {
            if self.candidate_update.is_some() {
                self.download_action = Some(DownloadAction::Pair);
                self.prepare_update(version, ctx);
            } else {
                self.notice = Some((
                    "Paired installation requires the installed application edition.".into(),
                    true,
                ));
            }
            return;
        }
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        self.paired_update = Some(
            Coordinator::outgoing(
                id.max(1),
                version,
                cfg!(target_os = "linux"),
                remote.diagnostics.platform == "linux",
            )
            .without_password(),
        );
        let message = self.paired_update.as_ref().unwrap().retry().unwrap();
        self.apply_update_effects(vec![Effect::Send(message)], ctx);
    }

    pub(super) fn receive_update(&mut self, message: Message, ctx: &egui::Context) {
        if let Message::Prepare { id, version } = &message
            && self.paired_update.is_none()
        {
            let result = if self.update_action_busy() || self.download_action.is_some() {
                Err(anyhow::anyhow!(
                    "Another update operation is already in progress."
                ))
            } else {
                Coordinator::incoming(*id, version.clone(), false)
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
            if coordinator.remote_only() {
                return;
            }
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

    pub(super) fn fail_paired_download(&mut self, error: String, ctx: &egui::Context) {
        if self
            .paired_update
            .as_ref()
            .is_some_and(|update| !update.remote_only())
        {
            self.fail_paired_update(error, ctx);
        }
    }

    pub(super) fn apply_update_effects(&mut self, effects: Vec<Effect>, ctx: &egui::Context) {
        let remote_only = self
            .paired_update
            .as_ref()
            .is_some_and(Coordinator::remote_only);
        for effect in effects {
            match effect {
                Effect::Send(message) => {
                    let target = self
                        .group
                        .update_target
                        .clone()
                        .unwrap_or_else(|| self.peer_key.clone());
                    if let Err(error) = self.update_send_to(&target, message) {
                        self.notice = Some((error.to_string(), true));
                    }
                }
                Effect::Download => {
                    if let Some(coordinator) = &self.paired_update {
                        let version = coordinator.version.clone();
                        self.prepare_update(version, ctx);
                    }
                }
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
                    if remote_only {
                        self.peer_update_status = Some((error.clone(), true));
                    } else {
                        self.update_status = error.clone();
                    }
                    self.notice = Some((error, true));
                    if !remote_only {
                        self.update_ready = self.prepared_update.is_some();
                    }
                }
                Effect::PeerRestart(version) => {
                    self.peer_update_status = Some((
                        format!(
                            "The other PC is restarting into version {version}. This PC stays running."
                        ),
                        false,
                    ));
                    self.peer_restart = Some((version, std::time::Instant::now()));
                }
            }
        }
        if self.paired_update.as_ref().is_some_and(Coordinator::done) {
            self.paired_update = None;
            if self.peer_restart.is_none() {
                self.group.update_target = None;
            }
        }
        if let Some(coordinator) = &self.paired_update {
            let status = format!(
                "{}: {}",
                coordinator.version,
                crate::i18n::tr(coordinator.status())
            );
            if remote_only {
                self.peer_update_status = Some((status, false));
            } else {
                self.update_status = status;
            }
        }
    }

    pub(super) fn tick_paired_update(&mut self, ctx: &egui::Context) {
        if self.paired_update.is_none()
            && self.download_action.is_none()
            && self.peer_restart.is_none()
        {
            self.group.update_target = None;
            return;
        }
        let connected = self.update_target_connected(
            self.group
                .update_target
                .as_deref()
                .unwrap_or(&self.peer_key),
        );
        if self.install_job.is_none()
            && let Some(coordinator) = &mut self.paired_update
        {
            let effects = coordinator.expired(connected);
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
            ui.label(crate::i18n::tr("Both PCs download and verify the same signed version. Windows waits for Ubuntu's package installation to succeed. The installed Ubuntu edition needs no password prompt."));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(crate::i18n::tr("Update both PCs")).clicked() { self.update_confirmation = false; self.start_paired_update(ctx); }
                if ui.button(crate::i18n::tr("Only this PC")).clicked() { self.update_confirmation = false; self.begin_install_update(ctx); }
                if ui.button(crate::i18n::tr("Cancel")).clicked() { self.update_confirmation = false; }
            });
        });
    }
}
