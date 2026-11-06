use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufRead, Write},
    sync::OnceLock,
};
use tokio::sync::{broadcast, watch};

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Switch,
    Emergency,
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
    Audio {
        playing: bool,
        message: String,
    },
}

struct Controls {
    stop: watch::Sender<bool>,
    commands: broadcast::Sender<Command>,
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
        if let Err(error) = std::thread::Builder::new()
            .name("Desk Switch commands".into())
            .spawn(move || {
                for line in io::stdin().lock().lines() {
                    let Ok(line) = line else { break };
                    match line.trim() {
                        "switch" => {
                            let _ = command_send.send(Command::Switch);
                        }
                        "emergency" => {
                            let _ = command_send.send(Command::Emergency);
                        }
                        "stop" | "quit" => {
                            stop_send.send_replace(true);
                            return;
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
        Controls { stop, commands }
    })
}

pub fn install() {
    let _ = controls();
}

pub fn commands() -> broadcast::Receiver<Command> {
    controls().commands.subscribe()
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
