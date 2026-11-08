use crate::edge::{Crossing, Entry, Routes};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufRead, Write},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{broadcast, watch};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DisplaySwitch {
    pub activation: u64,
    pub entry: Option<Entry>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayEntry {
    pub activation: u64,
    pub entry: Entry,
}

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Switch,
    SwitchEdge(u16),
    EnterEdge(u16),
    SwitchDisplay(DisplaySwitch),
    EnterDisplay(DisplayEntry),
    RoutesChanged,
    SetupSwitch,
    Emergency,
    ReturnInput(u64),
    MicrophoneVolume(u8),
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum InputStatus {
    Waiting,
    Here,
    Peer,
    Recovering,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RecoveryReason {
    ConnectionLost,
    HeartbeatTimeout,
    LeaseExpired,
    EmergencyKey,
    InputOverflow,
    ControlOverflow,
    SwitchFailed,
}
impl std::fmt::Display for RecoveryReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ConnectionLost => "The input connection was lost",
            Self::HeartbeatTimeout => "The peer heartbeat timed out",
            Self::LeaseExpired => "The local input lease expired",
            Self::EmergencyKey => "The emergency key was pressed",
            Self::InputOverflow => "The input event queue overflowed",
            Self::ControlOverflow => "The input control queue overflowed",
            Self::SwitchFailed => "The native input operation failed",
        })
    }
}
impl std::error::Error for RecoveryReason {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputSession {
    Authenticated,
    Recovery(RecoveryReason),
}

pub fn recovery_reason(error: &anyhow::Error) -> RecoveryReason {
    if let Some(reason) = error.downcast_ref::<RecoveryReason>() {
        return *reason;
    }
    if let Some(error) = error.downcast_ref::<std::io::Error>()
        && matches!(
            error.kind(),
            std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::NotConnected
                | std::io::ErrorKind::TimedOut
        )
    {
        return RecoveryReason::ConnectionLost;
    }
    if error
        .downcast_ref::<tokio::time::error::Elapsed>()
        .is_some()
    {
        return RecoveryReason::ConnectionLost;
    }
    RecoveryReason::SwitchFailed
}

fn authenticated_events(status: InputStatus) -> [Event; 3] {
    [
        Event::Switch { error: None },
        Event::InputAuthenticated,
        Event::Input(status),
    ]
}

pub fn input_authenticated(status: InputStatus) {
    for event in authenticated_events(status) {
        report(event);
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Event {
    CycleInput,
    CycleInputEdge(u16),
    EdgeCrossed(Crossing),
    DisplayEntry {
        activation: u64,
        error: Option<String>,
    },
    InputReturned {
        activation: u64,
        completed: bool,
    },
    InputAuthenticated,
    InputRecovery {
        reason: RecoveryReason,
    },
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
    routes: watch::Sender<Routes>,
    #[cfg(windows)]
    remote_cursor: watch::Sender<bool>,
}

static CONTROLS: OnceLock<Controls> = OnceLock::new();

pub fn managed() -> bool {
    std::env::var_os("DESK_SWITCH_MANAGED").is_some()
}

pub fn group_input() -> bool {
    std::env::var_os("DESK_SWITCH_GROUP_INPUT").is_some()
}

fn controls() -> &'static Controls {
    CONTROLS.get_or_init(|| {
        let (stop, _) = watch::channel(false);
        let (commands, _) = broadcast::channel(16);
        let stop_send = stop.clone();
        let command_send = commands.clone();
        let monitor_required = Arc::new(AtomicBool::new(false));
        let required = Arc::clone(&monitor_required);
        let (routes, _) = watch::channel(Routes::default());
        let route_send = routes.clone();
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
                    if line.len() > 1024 * 1024 {
                        continue;
                    }
                    match line.trim() {
                        "switch" => {
                            let _ = command_send.send(Command::Switch);
                        }
                        value if value.starts_with("switch-edge ") => {
                            if let Ok(position) = value[12..].trim().parse::<u16>() {
                                let _ = command_send.send(Command::SwitchEdge(position));
                            }
                        }
                        value if value.starts_with("enter-edge ") => {
                            if let Ok(position) = value[11..].trim().parse::<u16>() {
                                let _ = command_send.send(Command::EnterEdge(position));
                            }
                        }
                        value if value.starts_with("routes ") => {
                            if let Ok(routes) = serde_json::from_str::<Routes>(&value[7..])
                                && routes.validate().is_ok()
                            {
                                route_send.send_replace(routes);
                                let _ = command_send.send(Command::RoutesChanged);
                            }
                        }
                        value if value.starts_with("switch-display ") => {
                            if let Ok(request) = serde_json::from_str::<DisplaySwitch>(&value[15..])
                                && request.entry.is_none_or(|entry| entry.validate().is_ok())
                            {
                                let _ = command_send.send(Command::SwitchDisplay(request));
                            }
                        }
                        value if value.starts_with("enter-display ") => {
                            if let Ok(request) = serde_json::from_str::<DisplayEntry>(&value[14..])
                                && request.entry.validate().is_ok()
                            {
                                let _ = command_send.send(Command::EnterDisplay(request));
                            }
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
                        value if value.starts_with("return-input ") => {
                            if let Ok(activation) = value[13..].parse::<u64>()
                                && activation != 0
                            {
                                let _ = command_send.send(Command::ReturnInput(activation));
                            }
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
            routes,
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

pub fn routes() -> watch::Receiver<Routes> {
    controls().routes.subscribe()
}

pub fn install_routes(routes: Routes) -> anyhow::Result<()> {
    routes.validate()?;
    let controls = controls();
    let unconfigured = controls.routes.borrow().revision == 0;
    if unconfigured {
        controls.routes.send_replace(routes);
    }
    Ok(())
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

#[cfg(test)]
mod input_session_tests {
    use super::*;

    #[test]
    fn authenticated_readiness_clears_current_error_before_publishing_the_session() {
        for status in [InputStatus::Here, InputStatus::Peer] {
            let events = authenticated_events(status);
            assert!(matches!(events[0], Event::Switch { error: None }));
            assert!(matches!(events[1], Event::InputAuthenticated));
            assert!(matches!(events[2], Event::Input(value) if value == status));
        }
    }

    #[test]
    fn recovery_intent_uses_typed_causes_instead_of_error_text() {
        let disconnected =
            anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::UnexpectedEof))
                .context("The encrypted input stream ended");
        assert_eq!(
            recovery_reason(&disconnected),
            RecoveryReason::ConnectionLost
        );
        for reason in [
            RecoveryReason::EmergencyKey,
            RecoveryReason::HeartbeatTimeout,
            RecoveryReason::LeaseExpired,
            RecoveryReason::InputOverflow,
            RecoveryReason::ControlOverflow,
        ] {
            let error = anyhow::Error::new(reason).context("Native input was restored");
            assert_eq!(recovery_reason(&error), reason);
        }
        assert_eq!(
            recovery_reason(&anyhow::anyhow!(
                "the emergency key was pressed; peer heartbeat timed out"
            )),
            RecoveryReason::SwitchFailed
        );
    }
}
