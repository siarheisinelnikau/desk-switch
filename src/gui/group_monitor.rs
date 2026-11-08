//! Exactly one named writer executes each monitor command. Persisted commands
//! are observations, never permission to replay a hardware write after restart.
use super::{
    Desktop,
    worker::{Kind, Worker},
};
use crate::{
    config,
    group::{
        model::{self, MonitorCommand, Workspace},
        net::Action,
    },
};
use anyhow::{Context, Result, ensure};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct Attempt {
    pub hub: String,
    pub id: u64,
    pub writer: String,
    pub destination: String,
    pub connection: String,
    pub input: u16,
}
impl Attempt {
    fn new(state: &Workspace, command: &MonitorCommand) -> Self {
        Self {
            hub: state.workspace_id().to_owned(),
            id: command.id,
            writer: command.writer.clone(),
            destination: command.destination.clone(),
            connection: command.connection.clone(),
            input: command.input,
        }
    }
}

/// Return false for an already-recorded or older attempt. The private record is
/// committed before spawning, including cancellations that must never replay.
fn record(dir: &Path, attempt: &Attempt) -> Result<bool> {
    model::valid_key(&attempt.hub)?;
    let path = dir.join(format!("monitor-attempt-{}.json", attempt.hub));
    match std::fs::metadata(&path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file() && metadata.len() <= 4096,
                "Invalid monitor issuance record"
            );
            let previous: Attempt = serde_json::from_slice(&std::fs::read(&path)?)
                .context("Reading the monitor issuance record")?;
            ensure!(
                previous.hub == attempt.hub && previous.writer == attempt.writer,
                "Monitor issuance identity changed"
            );
            if previous.id >= attempt.id {
                return Ok(false);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    config::write_atomic(&path, &serde_json::to_string(attempt)?)?;
    Ok(true)
}

fn bounded(text: &str, limit: usize) -> String {
    let mut result = String::new();
    for character in text.chars() {
        if character.is_control() {
            continue;
        }
        if result.len() + character.len_utf8() > limit {
            break;
        }
        result.push(character);
    }
    result
}

fn failure(state: &Workspace, attempt: &Attempt, detail: &str) -> String {
    let name = |key: &str| {
        state
            .members
            .get(key)
            .map(|member| bounded(&member.node.name, 80))
            .unwrap_or_else(|| bounded(key, 80))
    };
    let monitor = bounded(&attempt.connection, 160);
    let prefix = format!(
        "Monitor {} -> {}, input 0x{:04x}, connection {}: ",
        name(&attempt.writer),
        name(&attempt.destination),
        attempt.input,
        monitor
    );
    format!("{prefix}{}", bounded(detail, 2048 - prefix.len().min(2048)))
}

impl Desktop {
    pub(super) fn report_group_monitor_history(&self, node: &mut crate::workspace::Node) {
        for detail in &self.group.monitor_history {
            let mut remaining = detail.as_str();
            while !remaining.is_empty() {
                let end = remaining.floor_char_boundary(288);
                node.diagnostics.line("Monitor", &remaining[..end]);
                remaining = &remaining[end..];
            }
        }
    }
    pub(super) fn tick_group_monitor(&mut self, state: &Workspace, ctx: &egui::Context) {
        let initial = self.group.monitor_observed.as_deref() != Some(state.workspace_id());
        self.group.monitor_observed = Some(state.workspace_id().to_owned());
        let command = state
            .monitor_command
            .as_ref()
            .filter(|command| command.writer == self.public_key);
        let fresh = state
            .monitor_command
            .as_ref()
            .map(|command| (state.workspace_id().to_owned(), command.id));
        let success = state.monitor_command.as_ref().is_some_and(|command| {
            command
                .result
                .as_ref()
                .is_some_and(|result| result.error.is_none())
        });
        if ((fresh.is_some() && fresh != self.group.monitor_notice_id) || success)
            && self
                .notice
                .as_ref()
                .is_some_and(|(message, error)| message == "Monitor switch failed" && *error)
        {
            self.notice = None;
        }
        self.group.monitor_notice_id = fresh;
        if initial && let Some(command) = command.filter(|command| command.result.is_none()) {
            self.group.monitor_loaded = Some((state.workspace_id().to_owned(), command.id));
        }
        // Readiness is controller-owned. Old snapshots are accepted for staged
        // upgrades, but cannot cause this version to issue an implicit DDC call.
        if let Some(member) = state.members.get(&self.public_key) {
            self.group.input_ready = member.input_ready;
            self.group.monitor_error = member.node.monitor_error.clone();
        }
        if state.monitor_command.is_none()
            && state.active_input.as_deref() == Some(&self.public_key)
            && (!state.settings.monitor_enabled
                || !state.members[&self.public_key].node.monitor.enabled)
        {
            self.group.input_ready = state.activation;
        }

        if let Some(worker) = &mut self.group.monitor_job {
            if command.is_some_and(|command| command.interrupted.is_some()) && !worker.stopping {
                worker.force_stop();
                worker.stopping = true;
            }
            worker.poll();
            if worker.settled() {
                if let Some(attempt) = self.group.monitor_attempt.clone() {
                    let interrupted = command.and_then(|command| command.interrupted.as_deref());
                    if worker.exit_ok == Some(true) && interrupted.is_none() {
                        self.group.monitor_wait =
                            Some((attempt.id, Instant::now() + crate::video::SETTLE));
                    } else {
                        let details = worker
                            .logs
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            .join(" | ");
                        let detail = interrupted
                            .map(|reason| format!("{reason} | {details}"))
                            .unwrap_or_else(|| {
                                if details.is_empty() {
                                    "Monitor selection failed; no retry was sent.".into()
                                } else {
                                    details
                                }
                            });
                        let error = failure(state, &attempt, &detail);
                        if self.group.monitor_history.len() == 2 {
                            self.group.monitor_history.pop_front();
                        }
                        self.group.monitor_history.push_back(error.clone());
                        self.notice = Some(("Monitor switch failed".into(), true));
                        self.group.monitor_result = Some((attempt, Some(error)));
                    }
                }
                self.group.monitor_job = None;
            } else {
                ctx.request_repaint_after_for(worker.next_poll(), egui::ViewportId::ROOT);
            }
        }
        if let Some((id, until)) = self.group.monitor_wait {
            if let Some(reason) = command.and_then(|command| command.interrupted.as_ref()) {
                if let Some(attempt) = self.group.monitor_attempt.clone() {
                    self.group.monitor_result =
                        Some((attempt.clone(), Some(failure(state, &attempt, reason))));
                }
                self.group.monitor_wait = None;
            } else if Instant::now() >= until {
                if let Some(attempt) = self
                    .group
                    .monitor_attempt
                    .clone()
                    .filter(|attempt| attempt.id == id)
                {
                    self.group.monitor_result = Some((attempt, None));
                }
                self.group.monitor_wait = None;
            } else {
                ctx.request_repaint_after_for(
                    until.saturating_duration_since(Instant::now()),
                    egui::ViewportId::ROOT,
                );
            }
        }
        if let Some(command) = command.filter(|command| command.result.is_none())
            && state.authority.is_some()
        {
            let attempt = Attempt::new(state, command);
            let handled = self.group.monitor_attempt.as_ref() == Some(&attempt);
            if !handled
                && self.group.monitor_job.is_none()
                && self.group.monitor_wait.is_none()
                && !self.quitting
                && !matches!(self.query, Some((super::QueryKind::Monitors, _)))
            {
                self.group.monitor_attempt = Some(attempt.clone());
                let result = (|| {
                    let fresh = record(&self.dir, &attempt)?;
                    ensure!(
                        self.group.monitor_loaded.as_ref()
                            != Some(&(attempt.hub.clone(), attempt.id)),
                        "An unfinished monitor command was loaded after startup; it was not replayed. Select the visible monitor input manually, then retry explicitly."
                    );
                    ensure!(
                        fresh,
                        "This monitor command was already recorded; it was not issued again. Select the visible monitor input manually, then retry explicitly."
                    );
                    ensure!(
                        command.interrupted.is_none(),
                        "{}",
                        command.interrupted.as_deref().unwrap_or_default()
                    );
                    let id = &attempt.connection;
                    ensure!(
                        !id.is_empty(),
                        "The monitor writer has no configured connection"
                    );
                    let args = vec![
                        "monitor-select".into(),
                        id.clone(),
                        attempt.input.to_string(),
                    ];
                    self.group.monitor_job =
                        Some(Worker::start(Kind::Query, &args, &self.dir, ctx)?);
                    Ok::<(), anyhow::Error>(())
                })();
                if let Err(error) = result {
                    let error = failure(state, &attempt, &format!("{error:#}"));
                    if self.group.monitor_history.len() == 2 {
                        self.group.monitor_history.pop_front();
                    }
                    self.group.monitor_history.push_back(error.clone());
                    self.notice = Some(("Monitor switch failed".into(), true));
                    self.group.monitor_result = Some((attempt, Some(error)));
                }
            }
        }
        if let Some((attempt, error)) = self.group.monitor_result.clone() {
            let relevant = attempt.hub == state.workspace_id()
                && command.is_some_and(|command| {
                    command.id == attempt.id
                        && command.destination == attempt.destination
                        && command.result.is_none()
                });
            if !relevant {
                self.group.monitor_result = None;
                self.group.monitor_reply_at = None;
            } else if self
                .group
                .monitor_reply_at
                .is_none_or(|until| until <= Instant::now())
            {
                if let Some(service) = &self.group.service {
                    let _ = service.action(Action::MonitorResult {
                        id: attempt.id,
                        error,
                    });
                }
                self.group.monitor_reply_at = Some(Instant::now() + Duration::from_secs(2));
            }
            if let Some(until) = self.group.monitor_reply_at {
                ctx.request_repaint_after_for(
                    until.saturating_duration_since(Instant::now()),
                    egui::ViewportId::ROOT,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coordinator_handover_preserves_the_monitor_attempt_identity() {
        let mut state = crate::group::model::tests::fixture(3);
        state.settings.monitor_enabled = true;
        let original = state.hub.clone();
        let destination = state.members.keys().nth(1).unwrap().clone();
        state.activate(&original, &destination, None).unwrap();
        state.authority = Some(crate::group::model::Authority {
            workspace_id: original,
            term: 1,
            index: 1,
        });
        let before = Attempt::new(&state, state.monitor_command.as_ref().unwrap());
        let dir = std::env::temp_dir().join(format!(
            "desk-switch-monitor-handover-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(record(&dir, &before).unwrap());
        state.hub = state.members.keys().nth(2).unwrap().clone();
        state.authority.as_mut().unwrap().term = 2;
        state.authority.as_mut().unwrap().index = 2;
        assert_eq!(
            Attempt::new(&state, state.monitor_command.as_ref().unwrap()),
            before
        );
        assert!(
            !record(
                &dir,
                &Attempt::new(&state, state.monitor_command.as_ref().unwrap())
            )
            .unwrap()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn issuance_is_persistent_monotonic_and_scoped_to_the_controller() {
        let dir = std::env::temp_dir().join(format!(
            "desk-switch-monitor-ledger-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut attempt = Attempt {
            hub: "11".repeat(32),
            id: 1,
            writer: "22".repeat(32),
            destination: "33".repeat(32),
            connection: "test-display".into(),
            input: 17,
        };
        assert!(record(&dir, &attempt).unwrap());
        assert!(!record(&dir, &attempt).unwrap());
        attempt.destination = "44".repeat(32);
        assert!(!record(&dir, &attempt).unwrap());
        attempt.id = 2;
        assert!(record(&dir, &attempt).unwrap());
        attempt.id = 1;
        assert!(!record(&dir, &attempt).unwrap());
        attempt.hub = "55".repeat(32);
        assert!(record(&dir, &attempt).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn full_error_context_is_bounded_unicode_safe_and_has_no_control_characters() {
        let state = crate::group::model::tests::fixture(3);
        let writer = state.workspace_id().to_owned();
        let destination = state.members.keys().nth(1).unwrap().clone();
        let attempt = Attempt {
            hub: state.workspace_id().to_owned(),
            id: 1,
            writer,
            destination,
            connection: "test-display".into(),
            input: 17,
        };
        let error = failure(
            &state,
            &attempt,
            &format!("/dev/i2c-5: EIO\n\r\0{}", "Ж".repeat(4096)),
        );
        assert!(error.len() <= 2048 && !error.chars().any(char::is_control));
        assert!(error.contains("0x0011") && error.contains("/dev/i2c-5") && error.contains("EIO"));
    }
}
