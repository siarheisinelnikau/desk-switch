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
    AudioSend,
    AudioReceive,
    Query,
}

pub struct Worker {
    pub kind: Kind,
    child: Option<Child>,
    input: Option<ChildStdin>,
    events: Receiver<String>,
    pub logs: VecDeque<String>,
    pub input_status: Option<InputStatus>,
    pub audio_playing: bool,
    pub peer_version: Option<String>,
    pub version_error: Option<String>,
    pub stopping: bool,
    pub exit_ok: Option<bool>,
    pub data: Option<serde_json::Value>,
    started: Instant,
    events_closed: bool,
    query_timeout: Duration,
}

impl Worker {
    pub fn start(kind: Kind, args: &[String], dir: &Path) -> Result<Self> {
        let executable = std::env::current_exe()?;
        let mut command = Command::new(executable);
        command.args(args).env("DESK_SWITCH_STATE", dir);
        if kind != Kind::Query {
            command.env("DESK_SWITCH_MANAGED", "1");
        } else {
            command.env_remove("DESK_SWITCH_MANAGED");
        }
        let mut worker = Self::spawn(kind, command)?;
        if args.first().is_some_and(|arg| arg == "portal-check") {
            worker.query_timeout = Duration::from_secs(140);
        }
        Ok(worker)
    }

    fn spawn(kind: Kind, mut command: Command) -> Result<Self> {
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
                        if structured {
                            if send.send(line).is_err() {
                                break;
                            }
                        } else {
                            let _ = send.try_send(line);
                        }
                    }
                })?;
        }
        Ok(Self {
            kind,
            child: Some(child),
            input,
            events,
            logs: VecDeque::new(),
            input_status: None,
            audio_playing: false,
            peer_version: None,
            version_error: None,
            stopping: false,
            exit_ok: None,
            data: None,
            started: Instant::now(),
            events_closed: false,
            query_timeout: Duration::from_secs(18),
        })
    }

    pub fn settled(&self) -> bool {
        !self.running() && self.events_closed
    }

    pub fn running(&self) -> bool {
        self.child.is_some()
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
    }

    fn line(&mut self, line: String) {
        if let Some(json) = line.strip_prefix("@desk-switch ")
            && let Ok(event) = serde_json::from_str::<Event>(json)
        {
            match event {
                Event::PeerVersion {
                    app,
                    protocol,
                    compatible,
                } => {
                    self.peer_version = app.clone();
                    self.version_error = if compatible {
                        None
                    } else {
                        Some(format!(
                            "Версии не совпадают: этот ПК {} (протокол {}), другой ПК {} (протокол {}). Обновите оба ПК.",
                            crate::compatibility::VERSION,
                            crate::compatibility::PROTOCOL,
                            app.as_deref().unwrap_or("старый клиент"),
                            protocol
                                .map(|p| p.to_string())
                                .unwrap_or_else(|| "неизвестен".into())
                        ))
                    };
                }
                Event::Input(status) => self.input_status = Some(status),
                Event::Audio { playing, .. } => self.audio_playing = playing && self.running(),
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

    pub fn poll(&mut self) {
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
            && self.started.elapsed() > self.query_timeout
            && let Some(child) = &mut self.child
        {
            let _ = child.kill();
            self.logs
                .push_back("Проверка устройства превысила время ожидания.".into());
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
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Managed children treat EOF as cooperative shutdown, including GUI crashes.
        self.input.take();
        if self.kind == Kind::Query
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
}
