use super::{Action, Desktop};
use crate::workspace::update::{Coordinator, Effect, Message};
use eframe::egui;
use std::{
    hash::BuildHasher,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

fn update_request_id() -> u64 {
    static IDS: OnceLock<AtomicU64> = OnceLock::new();
    // Both computers can initiate updates simultaneously on the same link.
    let ids = IDS.get_or_init(|| AtomicU64::new(std::hash::RandomState::new().hash_one(0u8)));
    loop {
        let id = ids.fetch_add(1, Ordering::Relaxed);
        if id != 0 {
            return id;
        }
    }
}

pub(super) enum RemoteUpdate {
    Running(Coordinator),
    Restarting { version: String, since: Instant },
    Finished { status: String, failed: bool },
}

impl RemoteUpdate {
    fn pending_version(&self) -> Option<&str> {
        match self {
            Self::Running(update) => Some(&update.version),
            Self::Restarting { version, .. } => Some(version),
            Self::Finished { .. } => None,
        }
    }

    pub(super) fn status(&self) -> (String, bool) {
        match self {
            Self::Running(update) => (
                format!("{}: {}", update.version, crate::i18n::tr(update.status())),
                false,
            ),
            Self::Restarting { version, .. } => (
                format!("The other PC is restarting into version {version}."),
                false,
            ),
            Self::Finished { status, failed } => (status.clone(), *failed),
        }
    }
}

#[derive(PartialEq)]
pub(super) enum DownloadAction {
    Install,
    Pair,
}

impl Desktop {
    pub(super) fn update_action_busy(&self) -> bool {
        self.paired_update.is_some()
            || self.update_job.is_some()
            || self.install_job.is_some()
            || self.install_pending
            || self.quitting
    }

    pub(super) fn update_target_busy(&self, target: &str) -> bool {
        if target == self.public_key {
            return self.update_action_busy();
        }
        self.quitting
            || self
                .remote_updates
                .get(target)
                .is_some_and(|update| update.pending_version().is_some())
            || (self.group.update_target.as_deref() == Some(target)
                && (self.download_action == Some(DownloadAction::Pair)
                    || self
                        .paired_update
                        .as_ref()
                        .is_some_and(Coordinator::updates_both)))
    }

    pub(super) fn clear_remote_update_result(&mut self, target: &str) {
        if matches!(
            self.remote_updates.get(target),
            Some(RemoteUpdate::Finished { .. })
        ) {
            self.remote_updates.remove(target);
        }
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
        if target == self.public_key {
            if !self.accept_update_action() || !self.update_ready {
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
        self.start_remote_update_on(&remote);
    }

    pub(super) fn install_update(&mut self, ctx: &egui::Context) {
        if !self.update_ready {
            self.start_remote_update();
            return;
        }
        if !self.accept_update_action() {
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

    pub(super) fn start_remote_update(&mut self) {
        let Some(remote) = self.pair.remote.clone() else {
            self.notice = Some((
                "Reconnect the selected computer before updating it.".into(),
                true,
            ));
            return;
        };
        self.start_remote_update_on(&remote);
    }

    fn start_remote_update_on(&mut self, remote: &crate::workspace::Node) {
        if self.update_target_busy(&remote.key) {
            return;
        }
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
        let update = &remote.management.as_ref().unwrap().update;
        let version = update.version.clone().unwrap();
        let coordinator = Coordinator::peer_only(
            update_request_id(),
            version,
            remote.diagnostics.platform == "linux",
            update.independent_install,
        )
        .without_password();
        let update = match self.update_send_to(&remote.key, coordinator.retry().unwrap()) {
            Ok(()) => RemoteUpdate::Running(coordinator),
            Err(error) => RemoteUpdate::Finished {
                status: format!("{error:#}"),
                failed: true,
            },
        };
        self.remote_updates.insert(remote.key.clone(), update);
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
        if self.update_target_busy(&remote.key) {
            return;
        }
        let Some(version) = self.update_version.clone() else {
            return;
        };
        self.group.update_target = Some(remote.key.clone());
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
        self.paired_update = Some(
            Coordinator::outgoing(
                update_request_id(),
                version,
                cfg!(target_os = "linux"),
                remote.diagnostics.platform == "linux",
            )
            .without_password(),
        );
        let message = self.paired_update.as_ref().unwrap().retry().unwrap();
        self.apply_update_effects(vec![Effect::Send(message)], ctx);
    }

    pub(super) fn receive_update_from(
        &mut self,
        source: &str,
        message: Message,
        ctx: &egui::Context,
    ) {
        if !matches!(&message, Message::Prepare { .. })
            && let Some(RemoteUpdate::Running(update)) = self.remote_updates.get_mut(source)
            && update.id == message.id()
        {
            let effects = match update.receive(message) {
                Ok(effects) => effects,
                Err(error) => update.fail(error.to_string(), true),
            };
            self.apply_remote_update_effects(source, effects);
            return;
        }
        if self.group.update_target.as_deref() == Some(source)
            && let Some(update) = &mut self.paired_update
            && update.id == message.id()
        {
            match update.receive(message) {
                Ok(effects) => self.apply_update_effects(effects, ctx),
                Err(error) => self.fail_paired_update(error.to_string(), ctx),
            }
            return;
        }
        let Message::Prepare {
            id,
            version,
            install_when_ready,
        } = message
        else {
            return;
        };
        let result = if self.update_action_busy() || self.download_action.is_some() {
            Err(anyhow::anyhow!(
                "Another update operation is already in progress."
            ))
        } else {
            Coordinator::incoming(id, version.clone(), false, install_when_ready)
        };
        match result {
            Ok(update) => {
                self.update_confirmation = false;
                self.group.update_target = Some(source.to_owned());
                self.paired_update = Some(update);
                if !install_when_ready {
                    self.group.add_open = false;
                    self.group.selected = self.public_key.clone();
                    self.page = super::Page::Application;
                    self.action(Action::Show, ctx);
                }
                let effects = if self.prepared_update.is_some()
                    && self.update_version.as_deref() == Some(&version)
                {
                    self.paired_update.as_mut().unwrap().prepared(&version)
                } else {
                    vec![Effect::Download]
                };
                self.apply_update_effects(effects, ctx);
            }
            Err(error) => {
                let _ = self.update_send_to(
                    source,
                    Message::Failed {
                        id,
                        message: crate::workspace::management::status_text(error.to_string(), 1024),
                    },
                );
            }
        }
    }

    pub(super) fn sent_update_to(&mut self, target: &str, message: &Message, ctx: &egui::Context) {
        if self.group.update_target.as_deref() == Some(target)
            && let Some(update) = &mut self.paired_update
        {
            let effects = update.sent(message);
            self.apply_update_effects(effects, ctx);
        }
    }

    pub(super) fn retry_updates_for(&self, target: &str) {
        if self.group.update_target.as_deref() == Some(target)
            && let Some(message) = self.paired_update.as_ref().and_then(Coordinator::retry)
        {
            let _ = self.update_send_to(target, message);
        }
        if let Some(RemoteUpdate::Running(update)) = self.remote_updates.get(target)
            && let Some(message) = update.retry()
        {
            let _ = self.update_send_to(target, message);
        }
    }

    fn apply_remote_update_effects(&mut self, target: &str, effects: Vec<Effect>) {
        for effect in effects {
            let update = match effect {
                Effect::Send(message) => {
                    let _ = self.update_send_to(target, message);
                    continue;
                }
                Effect::PeerRestart(version) => RemoteUpdate::Restarting {
                    version,
                    since: Instant::now(),
                },
                Effect::Error(status) => RemoteUpdate::Finished {
                    status,
                    failed: true,
                },
                Effect::Download | Effect::Install | Effect::Quit => {
                    unreachable!("Remote-only coordinator requested a local action")
                }
            };
            self.remote_updates.insert(target.to_owned(), update);
        }
    }

    pub(super) fn tick_remote_updates(&mut self, ctx: &egui::Context) {
        let pending = self
            .remote_updates
            .iter()
            .filter(|(_, update)| update.pending_version().is_some())
            .map(|(target, _)| target.clone())
            .collect::<Vec<_>>();
        for target in pending {
            let connected = self.update_target_connected(&target);
            let observed = self
                .group
                .workspace
                .as_ref()
                .and_then(|state| state.members.get(&target))
                .filter(|member| member.online)
                .map(|member| &member.node)
                .or_else(|| self.pair.remote.as_ref().filter(|node| node.key == target))
                .and_then(|node| node.version.as_ref())
                .map(|version| version.app.as_str());
            let mismatch = self
                .group
                .incompatible_workspaces
                .get(&target)
                .map(String::as_str);
            let update = self.remote_updates.get_mut(&target).unwrap();
            let version = update.pending_version().unwrap();
            if (connected && observed == Some(version)) || mismatch == Some(version) {
                *update = RemoteUpdate::Finished {
                    status: format!("The other PC is now running version {version}."),
                    failed: false,
                };
                continue;
            }
            let effects = match update {
                RemoteUpdate::Running(update) => update.expired(connected),
                RemoteUpdate::Restarting { since, .. }
                    if since.elapsed() >= Duration::from_secs(120) =>
                {
                    vec![Effect::Error("The other PC has not reconnected after installation. Check it and reconnect.".into())]
                }
                _ => Vec::new(),
            };
            self.apply_remote_update_effects(&target, effects);
        }
        if self
            .remote_updates
            .values()
            .any(|update| update.pending_version().is_some())
        {
            ctx.request_repaint_after(Duration::from_secs(1));
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
                    self.update_status = error.clone();
                    self.notice = Some((error, true));
                    self.update_ready =
                        self.prepared_update.is_some() || self.candidate_update.is_some();
                }
                Effect::PeerRestart(_) => {
                    unreachable!("Local coordinator reported a remote-only restart")
                }
            }
        }
        if self.paired_update.as_ref().is_some_and(Coordinator::done) {
            self.paired_update = None;
            self.group.update_target = None;
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
        if self.paired_update.is_none() && self.download_action.is_none() {
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
