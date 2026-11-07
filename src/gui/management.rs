use super::Desktop;
use crate::workspace::management::{self, Application, Message, State, Update};
use anyhow::{Context, Result, ensure};
use eframe::egui;
use std::time::{Duration, Instant};

pub(super) struct Pending {
    id: u64,
    since: Instant,
    pub application: Option<Application>,
    pub cable: bool,
}

pub(super) fn request_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

impl Desktop {
    pub(super) fn local_application(&self) -> Application {
        Application {
            theme: self.prefs.theme.clone(),
            language: self.prefs.language.clone(),
            autostart: self.prefs.autostart,
            auto_connect: self.prefs.auto_connect,
            auto_update: self.prefs.auto_update,
            show_remote_cursor: self.prefs.show_remote_cursor,
        }
    }
    pub(super) fn local_management(&self) -> State {
        State {
            application: self.local_application(),
            update: Update {
                version: self.update_version.clone(),
                channel_version: self.update_channel_version.clone(),
                ready: self.update_ready,
                checking: self.update_job.is_some(),
                installing: self.install_pending
                    || self.install_job.is_some()
                    || self
                        .paired_update
                        .as_ref()
                        .is_some_and(|update| update.handed_off),
                failed: self.update_failed,
                status: management::status_text(self.update_status.clone(), 2048),
                progress: self
                    .update_job
                    .as_ref()
                    .and_then(crate::updater::Job::progress),
            },
        }
    }
    pub(super) fn apply_application(
        &mut self,
        application: &Application,
        ctx: &egui::Context,
    ) -> Result<()> {
        application.validate()?;
        let previous = self.prefs.clone();
        let executable = std::env::current_exe()?;
        if application.autostart != self.prefs.autostart {
            crate::autostart::sync(application.autostart, &executable, &self.dir)?;
        }
        self.prefs.theme = application.theme.clone();
        self.prefs.language = application.language.clone();
        self.prefs.autostart = application.autostart;
        self.prefs.auto_connect = application.auto_connect;
        self.prefs.auto_update = application.auto_update;
        self.prefs.show_remote_cursor = application.show_remote_cursor;
        if let Err(error) = self.prefs.save(&self.dir) {
            let _ = crate::autostart::sync(previous.autostart, &executable, &self.dir);
            self.prefs = previous;
            return Err(error);
        }
        self.appearance_changed(ctx);
        if application.show_remote_cursor != previous.show_remote_cursor
            && let Some(worker) = self.input.as_mut().filter(|worker| worker.running())
        {
            worker.command(if application.show_remote_cursor {
                "remote-cursor on"
            } else {
                "remote-cursor off"
            })?;
        }
        Ok(())
    }
    pub(super) fn request_application(&mut self, application: Application, ctx: &egui::Context) {
        if self.settings_remote {
            let Some(expected) = self
                .pair
                .remote
                .as_ref()
                .and_then(|node| node.management.as_ref())
                .map(|state| state.application.clone())
            else {
                return;
            };
            let message = Message::SetApplication {
                id: request_id(),
                expected,
                application,
            };
            if let Err(error) = self.send_management(message, ctx) {
                self.notice = Some((error.to_string(), true));
            }
        } else if let Err(error) = self.apply_application(&application, ctx) {
            self.notice = Some((error.to_string(), true));
        }
    }
    pub(super) fn send_management(&mut self, message: Message, ctx: &egui::Context) -> Result<()> {
        ensure!(
            self.management_pending.is_none(),
            "Waiting for the other PC to acknowledge the previous change"
        );
        ensure!(
            self.pair
                .remote
                .as_ref()
                .is_some_and(|node| node.management.is_some()),
            "Reconnect a PC with remote management support before continuing"
        );
        let application = match &message {
            Message::SetApplication { application, .. } => Some(application.clone()),
            _ => None,
        };
        self.pair
            .service
            .as_ref()
            .context("The other PC is disconnected")?
            .manage(message.clone())?;
        self.management_pending = Some(Pending {
            id: message.id(),
            since: Instant::now(),
            application,
            cable: matches!(message, Message::ExtractCable { .. }),
        });
        ctx.request_repaint();
        Ok(())
    }
    pub(super) fn check_remote_updates(&mut self, ctx: &egui::Context) {
        self.peer_update_status = None;
        if let Err(error) = self.send_management(Message::CheckUpdates { id: request_id() }, ctx) {
            self.notice = Some((error.to_string(), true));
        }
    }
    pub(super) fn receive_management(&mut self, message: Message, ctx: &egui::Context) {
        if let Message::Reply { id, error } = message {
            if self
                .management_pending
                .as_ref()
                .is_some_and(|pending| pending.id == id)
            {
                let cable = self
                    .management_pending
                    .as_ref()
                    .is_some_and(|pending| pending.cable);
                self.management_pending = None;
                if cable {
                    self.cable_remote_done = error.is_none();
                    self.cable_result = error.as_ref().map(|error| Err(error.clone()));
                }
                if let Some(error) = error {
                    self.notice = Some((error, true));
                }
            }
            return;
        }
        let id = message.id();
        if let Message::ExtractCable { platform, .. } = message {
            let result = self.start_cable_job(platform, ctx);
            if result.is_ok() {
                self.cable_reply = Some(id);
                return;
            }
            if let Some(service) = &self.pair.service {
                let _ = service.manage(Message::Reply {
                    id,
                    error: result
                        .err()
                        .map(|error| management::status_text(error.to_string(), 1024)),
                });
            }
            return;
        }
        let result = (|| {
            ensure!(
                !self.quitting && !self.install_pending && self.install_job.is_none(),
                "This PC is installing an update"
            );
            match message {
                Message::CheckUpdates { .. } => self.check_updates(ctx),
                Message::SetApplication {
                    expected,
                    application,
                    ..
                } => {
                    ensure!(
                        self.local_application() == expected,
                        "Settings changed on the other PC. Review the current values and try again."
                    );
                    self.apply_application(&application, ctx)?;
                }
                Message::Reply { .. } | Message::ExtractCable { .. } => unreachable!(),
            }
            Ok::<_, anyhow::Error>(())
        })();
        let reply = Message::Reply {
            id,
            error: result
                .err()
                .map(|error| management::status_text(error.to_string(), 1024)),
        };
        if let Some(service) = &self.pair.service {
            let _ = service.manage(reply);
        }
    }
    pub(super) fn tick_management(&mut self, ctx: &egui::Context) {
        if let Some(pending) = &self.management_pending {
            let remaining = Duration::from_secs(8).saturating_sub(pending.since.elapsed());
            if remaining.is_zero() || self.pair.remote.is_none() {
                self.management_pending = None;
                self.notice = Some((
                    "The other PC did not acknowledge the request. Reconnect and try again.".into(),
                    true,
                ));
            } else {
                ctx.request_repaint_after(remaining);
            }
        }
        if let Some((version, since)) = &self.peer_restart {
            let confirmed = self
                .pair
                .remote
                .as_ref()
                .and_then(|node| node.version.as_ref())
                .is_some_and(|installed| installed.app == *version);
            let remaining = Duration::from_secs(120).saturating_sub(since.elapsed());
            if confirmed {
                self.peer_update_status = Some((
                    format!("The other PC is now running version {version}."),
                    false,
                ));
                self.peer_restart = None;
            } else if remaining.is_zero() {
                self.peer_update_status = Some((
                    "The other PC has not reconnected after installation. Check it and reconnect."
                        .into(),
                    true,
                ));
                self.peer_restart = None;
            } else {
                ctx.request_repaint_after(remaining);
            }
        }
    }
}

impl Desktop {
    fn start_cable_job(
        &mut self,
        platform: crate::cable::Platform,
        ctx: &egui::Context,
    ) -> Result<()> {
        ensure!(
            self.cable_job.is_none(),
            "Wait for the installer extraction to finish"
        );
        ensure!(
            !self.quitting && !self.install_pending && self.install_job.is_none(),
            "This PC is installing an update"
        );
        self.cable_job = Some(crate::cable::Job::start(platform, ctx.clone())?);
        self.cable_result = None;
        self.cable_remote_done = false;
        Ok(())
    }
    pub(super) fn extract_cable(&mut self, ctx: &egui::Context) {
        self.cable_result = None;
        self.cable_remote_done = false;
        let result = if self.cable_remote {
            self.send_management(
                Message::ExtractCable {
                    id: request_id(),
                    platform: self.cable_platform,
                },
                ctx,
            )
        } else {
            self.start_cable_job(self.cable_platform, ctx)
        };
        if let Err(error) = result {
            self.cable_result = Some(Err(error.to_string()));
        }
    }
    pub(super) fn tick_cable(&mut self, _ctx: &egui::Context) {
        if let Some(result) = self.cable_job.as_ref().and_then(crate::cable::Job::poll) {
            self.cable_job = None;
            if let Some(id) = self.cable_reply.take()
                && let Some(service) = &self.pair.service
            {
                let _ = service.manage(Message::Reply {
                    id,
                    error: result
                        .as_ref()
                        .err()
                        .map(|error| management::status_text(error.clone(), 1024)),
                });
            }
            self.cable_result = Some(result);
        }
    }
}
