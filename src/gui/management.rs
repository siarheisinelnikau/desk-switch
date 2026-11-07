use super::Desktop;
use crate::workspace::management::{self, Application, Message, State, Update};
use anyhow::{Result, ensure};
use eframe::egui;
use std::time::{Duration, Instant};

pub(super) struct Pending {
    id: u64,
    since: Instant,
    pub application: Option<Application>,
    pub cable: bool,
    timeout: Duration,
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
        self.quick.repaint(ctx);
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
        self.management_send_to(&self.peer_key, message.clone())?;
        self.management_pending = Some(Pending {
            id: message.id(),
            since: Instant::now(),
            application,
            cable: matches!(
                message,
                Message::ExtractCable { .. } | Message::InstallCable { .. }
            ),
            timeout: if matches!(message, Message::ReadMonitor { .. }) {
                Duration::from_secs(30)
            } else if matches!(
                message,
                Message::ExtractCable { .. } | Message::InstallCable { .. }
            ) {
                Duration::from_secs(300)
            } else {
                Duration::from_secs(8)
            },
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
        if let Message::ReadMonitor { monitor, .. } = &message {
            let result = self.start_monitor_read(monitor, ctx);
            if result.is_ok() {
                let source = self
                    .group
                    .reply_target
                    .clone()
                    .unwrap_or_else(|| self.peer_key.clone());
                self.group.monitor_read_reply = Some((source, id));
                return;
            }
            self.management_reply(Message::Reply {
                id,
                error: result
                    .err()
                    .map(|error| management::status_text(format!("{error:#}"), 1024)),
            });
            return;
        }
        if let Message::ExtractCable { platform, .. } | Message::InstallCable { platform, .. } =
            &message
        {
            let result = self.start_cable_job(
                *platform,
                matches!(message, Message::InstallCable { .. }),
                ctx,
            );
            if result.is_ok() {
                self.cable_reply = Some(id);
                self.group.cable_target = self.group.reply_target.clone();
                return;
            }
            {
                self.management_reply(Message::Reply {
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
                Message::SetMonitor {
                    expected, monitor, ..
                } => {
                    ensure!(
                        !self.group.monitor_busy() && self.query.is_none(),
                        "Wait for the current monitor operation to finish"
                    );
                    ensure!(
                        self.prefs.workspace.monitor == expected,
                        "Monitor settings changed. Review the current values and try again."
                    );
                    ensure!(
                        !monitor.enabled
                            || self.monitors.iter().any(|found| found.id == monitor.id),
                        "The selected monitor is unavailable"
                    );
                    self.prefs.workspace.monitor = monitor;
                    self.prefs.save(&self.dir)?;
                }
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
                Message::Reply { .. }
                | Message::ReadMonitor { .. }
                | Message::ExtractCable { .. }
                | Message::InstallCable { .. } => unreachable!(),
            }
            Ok::<_, anyhow::Error>(())
        })();
        let reply = Message::Reply {
            id,
            error: result
                .err()
                .map(|error| management::status_text(error.to_string(), 1024)),
        };
        self.management_reply(reply);
    }
    pub(super) fn tick_management(&mut self, ctx: &egui::Context) {
        if let Some(pending) = &self.management_pending {
            let remaining = pending.timeout.saturating_sub(pending.since.elapsed());
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
                self.group.update_target = None;
            } else if remaining.is_zero() {
                self.peer_update_status = Some((
                    "The other PC has not reconnected after installation. Check it and reconnect."
                        .into(),
                    true,
                ));
                self.peer_restart = None;
                self.group.update_target = None;
            } else {
                ctx.request_repaint_after(remaining);
            }
        }
    }
}

impl Desktop {
    pub(super) fn start_monitor_read(&mut self, monitor: &str, ctx: &egui::Context) -> Result<()> {
        ensure!(
            !self.quitting && !self.install_pending && self.install_job.is_none(),
            "This PC is installing an update"
        );
        let busy = self.query.is_some()
            || self.group.monitor_busy()
            || self.monitor_running()
            || self
                .input
                .as_ref()
                .is_some_and(|worker| worker.running() && worker.monitoring);
        validate_monitor_read(
            &self.monitors,
            &self.prefs.workspace.monitor.id,
            monitor,
            busy,
        )?;
        self.scan(super::QueryKind::Monitors);
        ensure!(
            self.query
                .as_ref()
                .is_some_and(|(kind, _)| matches!(kind, super::QueryKind::Monitors)),
            "Could not start the monitor read"
        );
        ctx.request_repaint();
        Ok(())
    }

    pub(super) fn finish_monitor_read(&mut self, result: &Result<()>, ctx: &egui::Context) {
        if let Some((target, id)) = self.group.monitor_read_reply.take() {
            let reply = Message::Reply {
                id,
                error: result
                    .as_ref()
                    .err()
                    .map(|error| management::status_text(format!("{error:#}"), 1024)),
            };
            if let Err(error) = self.management_send_to(&target, reply) {
                self.notice = Some((error.to_string(), true));
            }
        }
        // Publish the recorded input on the next group tick, after the query's
        // result has been applied to preferences in this tick.
        ctx.request_repaint();
    }

    fn start_cable_job(
        &mut self,
        platform: crate::cable::Platform,
        installing: bool,
        ctx: &egui::Context,
    ) -> Result<()> {
        ensure!(
            self.cable_job.is_none(),
            "Wait for the current installer operation to finish"
        );
        ensure!(
            !self.quitting && !self.install_pending && self.install_job.is_none(),
            "This PC is installing an update"
        );
        self.cable_job = Some(if installing {
            crate::cable::Job::install(platform, ctx.clone())?
        } else {
            crate::cable::Job::start(platform, ctx.clone())?
        });
        self.cable_result = None;
        self.cable_remote_done = false;
        Ok(())
    }
    pub(super) fn extract_cable(&mut self, ctx: &egui::Context) {
        self.request_cable(false, ctx);
    }
    pub(super) fn install_cable(&mut self, ctx: &egui::Context) {
        self.request_cable(true, ctx);
    }
    fn request_cable(&mut self, installing: bool, ctx: &egui::Context) {
        self.cable_result = None;
        self.cable_remote_done = false;
        let result = if self.cable_remote {
            self.send_management(
                if installing {
                    Message::InstallCable {
                        id: request_id(),
                        platform: self.cable_platform,
                    }
                } else {
                    Message::ExtractCable {
                        id: request_id(),
                        platform: self.cable_platform,
                    }
                },
                ctx,
            )
        } else {
            self.start_cable_job(self.cable_platform, installing, ctx)
        };
        if let Err(error) = result {
            self.cable_result = Some(Err(error.to_string()));
        }
    }
    pub(super) fn tick_cable(&mut self, _ctx: &egui::Context) {
        let installing = self
            .cable_job
            .as_ref()
            .is_some_and(crate::cable::Job::installing);
        if let Some(result) = self.cable_job.as_ref().and_then(crate::cable::Job::poll) {
            self.cable_job = None;
            if let Some(id) = self.cable_reply.take() {
                self.management_reply(Message::Reply {
                    id,
                    error: result
                        .as_ref()
                        .err()
                        .map(|error| management::status_text(error.clone(), 1024)),
                });
                self.group.cable_target = None;
            }
            if installing && result.is_ok() {
                self.notice = Some(("The official installer was opened. Complete its steps on that computer; restart it if the installer requests it.".into(), false));
            }
            self.cable_result = Some(result);
        }
    }
}

fn validate_monitor_read(
    monitors: &[crate::platform::MonitorInfo],
    selected: &str,
    requested: &str,
    busy: bool,
) -> Result<()> {
    ensure!(!busy, "Wait for the current monitor operation to finish");
    ensure!(
        selected == requested && !requested.is_empty(),
        "Monitor settings changed. Review the selected monitor and try again."
    );
    ensure!(
        monitors.iter().any(|monitor| monitor.id == requested),
        "The selected monitor is unavailable"
    );
    Ok(())
}

#[cfg(test)]
mod monitor_read_tests {
    use super::*;

    #[test]
    fn manual_read_requires_the_saved_available_monitor_and_no_active_ddc_operation() {
        let monitor = |id: &str| crate::platform::MonitorInfo {
            id: id.into(),
            name: id.into(),
            screen: "fixture".into(),
            current: None,
            inputs: Vec::new(),
            error: None,
            fingerprint: String::new(),
        };
        let displays = vec![monitor("first"), monitor("second")];
        validate_monitor_read(&displays, "first", "first", false).unwrap();
        assert!(validate_monitor_read(&displays, "first", "first", true).is_err());
        assert!(validate_monitor_read(&displays, "first", "second", false).is_err());
        assert!(validate_monitor_read(&displays, "missing", "missing", false).is_err());
        assert!(validate_monitor_read(&[], "first", "first", false).is_err());
    }
}
