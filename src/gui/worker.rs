use crate::lifecycle::{Event, InputStatus};
use anyhow::{Context, Result, ensure};
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Input,
    Monitor,
    AudioSend,
    AudioReceive,
    MicrophoneSend,
    MicrophoneReceive,
    Query,
}

#[derive(Default)]
pub(super) struct OutputLevel {
    observed: Option<crate::workspace::AudioLevel>,
    pending: Option<crate::workspace::AudioLevel>,
    request: u64,
    pub changed: Option<crate::workspace::AudioLevel>,
}

impl OutputLevel {
    fn receive(&mut self, request: u64, level: crate::workspace::AudioLevel) {
        // A delayed poll result must not undo a newer slider command.
        if request == self.request && level.volume <= 100 {
            self.observed = Some(level);
            self.pending = None;
            self.changed = Some(level);
        }
    }

    fn needs_change(&self, level: crate::workspace::AudioLevel) -> bool {
        self.observed.is_some()
            && self.pending != Some(level)
            && (self.pending.is_some() || self.observed != Some(level))
    }
}

pub struct Worker {
    pub cycle_input: bool,
    pub cycle_input_position: Option<u16>,
    pub crossing: Option<crate::edge::Crossing>,
    pub display_error: Option<(u64, String)>,
    pub return_results: VecDeque<(u64, bool)>,
    pub kind: Kind,
    child: Option<Child>,
    input: Option<ChildStdin>,
    events: Receiver<String>,
    pub logs: VecDeque<String>,
    pub input_status: Option<InputStatus>,
    pub monitor_input: Option<u16>,
    pub monitoring: bool,
    pub switch_error: Option<String>,
    pub new_switch_error: Option<String>,
    monitor_required: Option<bool>,
    pub audio_playing: bool,
    pub(super) output_level: OutputLevel,
    pub peer_version: Option<String>,
    pub version_error: Option<String>,
    pub stopping: bool,
    stopping_since: Option<Instant>,
    pub exit_ok: Option<bool>,
    pub data: Option<serde_json::Value>,
    started: Instant,
    events_closed: bool,
    query_timeout: Duration,
    interactive_query: bool,
}

impl Worker {
    pub fn start(
        kind: Kind,
        args: &[String],
        dir: &Path,
        ctx: &eframe::egui::Context,
    ) -> Result<Self> {
        let executable = std::env::current_exe()?;
        let mut command = Command::new(executable);
        command.args(args).env("DESK_SWITCH_STATE", dir);
        if kind == Kind::Input {
            if dir.join("group-input").exists() {
                command.env("DESK_SWITCH_GROUP_INPUT", "1");
            }
            let prefs = super::model::Preferences::load(dir)?;
            command.env(
                "DESK_SWITCH_REMOTE_CURSOR",
                if prefs.show_remote_cursor {
                    "on"
                } else {
                    "off"
                },
            );
        }
        let interactive_query = args.first().is_some_and(|arg| arg == "input-access");
        if kind != Kind::Query || interactive_query {
            command.env("DESK_SWITCH_MANAGED", "1");
        } else {
            command.env_remove("DESK_SWITCH_MANAGED");
        }
        let mut worker = Self::spawn_waking(kind, command, Some(ctx.clone()))?;
        worker.interactive_query = interactive_query;
        if args
            .first()
            .is_some_and(|arg| arg == "portal-check" || arg == "input-access")
        {
            worker.query_timeout = Duration::from_secs(140);
        }
        Ok(worker)
    }

    #[cfg(test)]
    fn spawn(kind: Kind, command: Command) -> Result<Self> {
        Self::spawn_waking(kind, command, None)
    }

    fn spawn_waking(
        kind: Kind,
        mut command: Command,
        ctx: Option<eframe::egui::Context>,
    ) -> Result<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn().context("запуск фоновой службы")?;
        let input = child.stdin.take();
        let (send, events) = mpsc::sync_channel(256);
        for stream in [
            Box::new(child.stdout.take().unwrap()) as Box<dyn std::io::Read + Send>,
            Box::new(child.stderr.take().unwrap()) as Box<dyn std::io::Read + Send>,
        ] {
            let send = send.clone();
            let ctx = ctx.clone();
            std::thread::Builder::new()
                .name("Desk Switch service log".into())
                .spawn(move || {
                    for line in BufReader::new(stream).lines() {
                        let Ok(mut line) = line else { break };
                        let structured = line.starts_with("@desk-switch");
                        let limit = if structured { 65536 } else { 8192 };
                        if line.len() > limit {
                            line.truncate(line.floor_char_boundary(limit));
                        }
                        let sent = if structured {
                            if send.send(line).is_err() {
                                break;
                            }
                            true
                        } else {
                            send.try_send(line).is_ok()
                        };
                        if sent && let Some(ctx) = &ctx {
                            ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
                        }
                    }
                    if let Some(ctx) = &ctx {
                        ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
                    }
                })?;
        }
        Ok(Self {
            crossing: None,
            display_error: None,
            return_results: VecDeque::new(),
            cycle_input: false,
            cycle_input_position: None,
            kind,
            child: Some(child),
            input,
            events,
            logs: VecDeque::new(),
            input_status: None,
            monitor_input: None,
            monitoring: false,
            switch_error: None,
            new_switch_error: None,
            monitor_required: None,
            audio_playing: false,
            output_level: OutputLevel::default(),
            peer_version: None,
            version_error: None,
            stopping: false,
            stopping_since: None,
            exit_ok: None,
            data: None,
            started: Instant::now(),
            events_closed: false,
            query_timeout: Duration::from_secs(18),
            interactive_query: false,
        })
    }

    pub fn settled(&self) -> bool {
        !self.running() && self.events_closed
    }

    pub fn running(&self) -> bool {
        self.child.is_some()
    }

    pub fn needs_poll(&self) -> bool {
        self.running() && (self.stopping || self.events_closed)
    }

    pub fn next_poll(&self) -> Duration {
        if self.needs_poll() {
            Duration::from_millis(200)
        } else {
            self.query_timeout.saturating_sub(self.started.elapsed())
        }
    }

    pub fn set_output_level(&mut self, level: crate::workspace::AudioLevel) -> Result<()> {
        if self.output_level.needs_change(level) {
            let request = self
                .output_level
                .request
                .checked_add(1)
                .context("too many system volume changes")?;
            self.command(&format!(
                "system-level {request} {} {}",
                level.volume, level.muted
            ))?;
            self.output_level.request = request;
            self.output_level.pending = Some(level);
            self.output_level.changed = None;
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub fn age(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn connected(&self) -> bool {
        self.running()
            && !self.stopping
            && matches!(
                self.input_status,
                Some(InputStatus::Here | InputStatus::Peer)
            )
    }

    pub fn command(&mut self, command: &str) -> Result<()> {
        ensure!(self.running(), "служба уже остановлена");
        let input = self
            .input
            .as_mut()
            .context("канал управления службой закрыт")?;
        writeln!(input, "{command}")?;
        input.flush()?;
        Ok(())
    }

    pub fn require_monitor(&mut self, required: bool) -> Result<()> {
        if self.monitor_required != Some(required) {
            self.command(if required {
                "monitor-required on"
            } else {
                "monitor-required off"
            })?;
            self.monitor_required = Some(required);
        }
        Ok(())
    }

    pub fn set_routes(&mut self, routes: &crate::edge::Routes) -> Result<()> {
        routes.validate()?;
        self.command(&format!("routes {}", serde_json::to_string(routes)?))
    }

    pub fn return_input(&mut self, activation: u64) -> Result<()> {
        ensure!(activation != 0, "Invalid display activation");
        self.command(&format!("return-input {activation}"))
    }

    pub fn switch_display(
        &mut self,
        activation: u64,
        entry: Option<crate::edge::Entry>,
    ) -> Result<()> {
        ensure!(activation != 0, "Invalid display activation");
        if let Some(entry) = entry {
            entry.validate()?;
        }
        self.command(&format!(
            "switch-display {}",
            serde_json::to_string(&crate::lifecycle::DisplaySwitch { activation, entry })?
        ))
    }

    pub fn enter_display(&mut self, activation: u64, entry: crate::edge::Entry) -> Result<()> {
        ensure!(activation != 0, "Invalid display activation");
        entry.validate()?;
        self.command(&format!(
            "enter-display {}",
            serde_json::to_string(&crate::lifecycle::DisplayEntry { activation, entry })?
        ))
    }

    pub fn stop(&mut self) {
        if !self.running() || self.stopping {
            return;
        }
        let command = if self.kind == Kind::AudioReceive {
            "quit"
        } else {
            "stop"
        };
        if self.command(command).is_err() {
            self.input.take();
        }
        self.stopping = true;
        self.stopping_since = Some(Instant::now());
    }

    pub fn force_stop(&mut self) {
        self.input.take();
        if let Some(child) = &mut self.child {
            let _ = child.kill();
        }
    }

    fn line(&mut self, line: String) {
        if let Some(json) = line.strip_prefix("@desk-switch ")
            && let Ok(event) = serde_json::from_str::<Event>(json)
        {
            match event {
                Event::CycleInput => {
                    self.cycle_input = true;
                    self.cycle_input_position = None;
                }
                Event::CycleInputEdge(position) => {
                    self.cycle_input = true;
                    self.cycle_input_position = Some(position);
                }
                Event::EdgeCrossed(crossing) => {
                    if crossing.revision != 0 && crossing.boundary != 0 {
                        self.crossing = Some(crossing);
                    }
                }
                Event::DisplayEntry { activation, error } => {
                    self.display_error = error.map(|error| (activation, error));
                }
                Event::InputReturned {
                    activation,
                    completed,
                } => {
                    if self.return_results.len() == 32 {
                        self.return_results.pop_front();
                    }
                    self.return_results.push_back((activation, completed));
                }
                Event::PeerVersion {
                    app,
                    protocol,
                    compatible,
                } => {
                    self.peer_version = app.clone();
                    self.version_error = if compatible {
                        None
                    } else {
                        Some(
                            crate::compatibility::Mismatch {
                                local: crate::compatibility::Version::current(),
                                peer: app.zip(protocol).map(|(app, protocol)| {
                                    crate::compatibility::Version { app, protocol }
                                }),
                            }
                            .to_string(),
                        )
                    };
                }
                Event::Input(status) => self.input_status = Some(status),
                Event::Monitor(input) => self.monitor_input = input,
                Event::Switch { error } => {
                    self.switch_error = error.clone();
                    self.new_switch_error = error.clone();
                    if let Some(error) = error {
                        if self.logs.len() == 120 {
                            self.logs.pop_front();
                        }
                        self.logs.push_back(format!("Switch failed: {error}"));
                    }
                }
                Event::Audio { playing, .. } => self.audio_playing = playing && self.running(),
                Event::OutputLevel { request, level } => self.output_level.receive(request, level),
            }
            return;
        }
        if let Some(json) = line.strip_prefix("@desk-switch-data ")
            && let Ok(value) = serde_json::from_str(json)
        {
            self.data = Some(value);
            return;
        }
        if self.logs.len() == 120 {
            self.logs.pop_front();
        }
        self.logs.push_back(line);
    }

    pub fn poll(&mut self) -> bool {
        let was_running = self.running();
        if self
            .stopping_since
            .is_some_and(|time| time.elapsed() > Duration::from_secs(6))
        {
            self.force_stop();
            self.stopping_since = None;
        }
        for _ in 0..256 {
            match self.events.try_recv() {
                Ok(line) => self.line(line),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.events_closed = true;
                    break;
                }
            }
        }
        if self.kind == Kind::Query
            && !self.stopping
            && self.started.elapsed() > self.query_timeout
            && let Some(child) = &mut self.child
        {
            if self.interactive_query {
                self.input.take();
            } else {
                let _ = child.kill();
            }
            self.logs
                .push_back("Проверка устройства превысила время ожидания.".into());
            self.stopping = true;
            self.stopping_since = Some(Instant::now());
        }
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.exit_ok = Some(status.success());
                    self.child.take();
                    self.input.take();
                    self.audio_playing = false;
                }
                Ok(None) => {}
                Err(error) => {
                    self.logs.push_back(error.to_string());
                    self.input.take();
                }
            }
        }
        was_running && !self.running() && !self.stopping && self.exit_ok == Some(false)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Managed children treat EOF as cooperative shutdown, including GUI crashes.
        self.input.take();
        if self.kind == Kind::Query
            && !self.interactive_query
            && let Some(child) = &mut self.child
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_feedback_adopts_startup_and_media_keys_but_ignores_stale_commands() {
        use crate::workspace::AudioLevel;
        let mut output = OutputLevel::default();
        let saved = AudioLevel {
            volume: 100,
            muted: false,
        };
        assert!(!output.needs_change(saved));
        let actual = AudioLevel {
            volume: 37,
            muted: false,
        };
        output.receive(0, actual);
        assert_eq!(output.changed.take(), Some(actual));
        assert!(!output.needs_change(actual));
        let desired = AudioLevel {
            volume: 90,
            muted: false,
        };
        assert!(output.needs_change(desired));
        output.request = 1;
        output.pending = Some(desired);
        assert!(!output.needs_change(desired));
        output.receive(0, actual);
        assert_eq!(output.observed, Some(actual));
        assert!(output.changed.is_none());
        output.receive(1, desired);
        assert_eq!(output.changed.take(), Some(desired));
        let media_key = AudioLevel {
            volume: 88,
            muted: true,
        };
        output.receive(1, media_key);
        assert_eq!(output.changed.take(), Some(media_key));
        assert!(!output.needs_change(media_key));
        output.receive(
            1,
            AudioLevel {
                volume: 101,
                muted: false,
            },
        );
        assert_eq!(output.observed, Some(media_key));
    }

    #[test]
    fn service_version_errors_use_the_same_localized_message_as_pairing() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.arg("--help");
        let mut worker = Worker::spawn(Kind::Query, command).unwrap();
        for (app, protocol) in [(Some("0.4.2".into()), Some(4)), (None, None)] {
            worker.line(format!(
                "@desk-switch {}",
                serde_json::to_string(&Event::PeerVersion {
                    app,
                    protocol,
                    compatible: false
                })
                .unwrap()
            ));
            let message = worker.version_error.as_ref().unwrap();
            let english = crate::i18n::tr_language("en", message);
            assert!(english.starts_with("Incompatible versions:"), "{english}");
            assert!(
                !english
                    .chars()
                    .any(|c| ('\u{0400}'..='\u{04ff}').contains(&c))
            );
            assert!(crate::i18n::tr_language("ru", message).starts_with("Несовместимые версии:"));
        }
    }

    #[test]
    fn statuses_cannot_be_forged_by_an_ordinary_log_line() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.arg("--help");
        let mut worker = Worker::spawn(Kind::Query, command).unwrap();
        worker.line("peer authenticated.".into());
        assert!(worker.input_status.is_none());
        worker.line(format!(
            "@desk-switch {}",
            serde_json::to_string(&Event::Input(InputStatus::Peer)).unwrap()
        ));
        assert_eq!(worker.input_status, Some(InputStatus::Peer));
    }

    #[cfg(unix)]
    #[test]
    fn commands_stop_a_managed_fixture() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "read action; test \"$action\" = switch || exit 2; read action; test \"$action\" = stop"]);
        let mut worker = Worker::spawn(Kind::Input, command).unwrap();
        worker.command("switch").unwrap();
        worker.stop();
        let deadline = Instant::now() + Duration::from_secs(2);
        while worker.running() && Instant::now() < deadline {
            worker.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(worker.exit_ok, Some(true));
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_child_is_reported_once_so_retry_can_replace_it() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 1"]);
        let mut worker = Worker::spawn(Kind::Input, command).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut failures = 0;
        while worker.running() && Instant::now() < deadline {
            failures += usize::from(worker.poll());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(failures, 1);
        assert_eq!(worker.exit_ok, Some(false));
        // Retaining logs must not re-latch an already acknowledged failure.
        assert!(!worker.poll());
    }

    #[cfg(unix)]
    #[test]
    fn closing_the_gui_pipe_notifies_the_child_and_query_output_is_fully_drained() {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "if read action; then exit 2; fi; printf '@desk-switch-data [1,2]\\n'",
        ]);
        let mut worker = Worker::spawn(Kind::Input, command).unwrap();
        worker.input.take();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !worker.settled() && Instant::now() < deadline {
            worker.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(worker.settled());
        assert_eq!(worker.exit_ok, Some(true));
        assert_eq!(worker.data, Some(serde_json::json!([1, 2])));
    }

    #[cfg(unix)]
    #[test]
    fn an_unresponsive_child_cannot_block_quit_forever() {
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let mut worker = Worker::spawn(Kind::Input, command).unwrap();
        worker.stop();
        worker.stopping_since = Some(Instant::now() - Duration::from_secs(7));
        let deadline = Instant::now() + Duration::from_secs(2);
        while worker.running() && Instant::now() < deadline {
            worker.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!worker.running());
    }
}
