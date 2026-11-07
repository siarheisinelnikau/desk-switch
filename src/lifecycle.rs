use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufRead, Write},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{broadcast, watch};

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Switch,
    SetupSwitch,
    Emergency,
    MicrophoneVolume(u8),
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum InputStatus {
    Waiting,
    Here,
    Peer,
    Recovering,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Event {
    PeerVersion {
        app: Option<String>,
        protocol: Option<u16>,
        compatible: bool,
    },
    Input(InputStatus),
    Monitor(Option<u16>),
    Switch {
        error: Option<String>,
    },
    Audio {
        playing: bool,
        message: String,
    },
    OutputLevel {
        request: u64,
        level: crate::workspace::AudioLevel,
    },
}

struct Controls {
    stop: watch::Sender<bool>,
    commands: broadcast::Sender<Command>,
    monitor_required: Arc<AtomicBool>,
    #[cfg(windows)]
    remote_cursor: watch::Sender<bool>,
}

static CONTROLS: OnceLock<Controls> = OnceLock::new();

pub fn managed() -> bool {
    std::env::var_os("DESK_SWITCH_MANAGED").is_some()
}

fn controls() -> &'static Controls {
    CONTROLS.get_or_init(|| {
        let (stop, _) = watch::channel(false);
        let (commands, _) = broadcast::channel(16);
        let stop_send = stop.clone();
        let command_send = commands.clone();
        let monitor_required = Arc::new(AtomicBool::new(false));
        let required = Arc::clone(&monitor_required);
        #[cfg(windows)]
        let (remote_cursor, _) =
            watch::channel(std::env::var("DESK_SWITCH_REMOTE_CURSOR").as_deref() != Ok("off"));
        #[cfg(windows)]
        let cursor_send = remote_cursor.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("Desk Switch commands".into())
            .spawn(move || {
                for line in io::stdin().lock().lines() {
                    let Ok(line) = line else { break };
                    match line.trim() {
                        "switch" => {
                            let _ = command_send.send(Command::Switch);
                        }
                        "setup-switch" => {
                            let _ = command_send.send(Command::SetupSwitch);
                        }
                        "monitor-required on" => required.store(true, Ordering::Release),
                        "monitor-required off" => required.store(false, Ordering::Release),
                        #[cfg(windows)]
                        "remote-cursor on" => {
                            cursor_send.send_replace(true);
                        }
                        #[cfg(windows)]
                        "remote-cursor off" => {
                            cursor_send.send_replace(false);
                        }
                        "emergency" => {
                            let _ = command_send.send(Command::Emergency);
                        }
                        "stop" | "quit" => {
                            stop_send.send_replace(true);
                            return;
                        }
                        value if value.starts_with("volume ") => {
                            if let Ok(volume) = value[7..].trim().parse::<u8>()
                                && volume <= 100
                            {
                                let _ = command_send.send(Command::MicrophoneVolume(volume));
                            }
                        }
                        _ => {}
                    }
                }
                // The GUI owns the anonymous pipe. Its disappearance releases the service.
                if managed() {
                    stop_send.send_replace(true);
                }
            })
        {
            eprintln!("Cannot start the service command reader: {error}");
            stop.send_replace(true);
        }
        Controls {
            stop,
            commands,
            monitor_required,
            #[cfg(windows)]
            remote_cursor,
        }
    })
}

pub fn install() {
    let _ = controls();
}

pub fn commands() -> broadcast::Receiver<Command> {
    controls().commands.subscribe()
}

pub fn monitor_required() -> bool {
    controls().monitor_required.load(Ordering::Acquire)
}

#[cfg(windows)]
pub fn remote_cursor() -> watch::Receiver<bool> {
    controls().remote_cursor.subscribe()
}

pub async fn shutdown() -> io::Result<()> {
    let mut stop = controls().stop.subscribe();
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        _ = async {
            while !*stop.borrow_and_update() {
                if stop.changed().await.is_err() { break; }
            }
        } => Ok(()),
    }
}

pub fn report(event: Event) {
    if managed()
        && let Ok(json) = serde_json::to_string(&event)
    {
        // A closed GUI log pipe must not interrupt input/video recovery.
        let _ = writeln!(io::stdout().lock(), "@desk-switch {json}");
    }
}
