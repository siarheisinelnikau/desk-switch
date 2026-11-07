use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum Message {
    Prepare { id: u64, version: String },
    Ready(u64),
    Commit(u64),
    Installing { id: u64, authorization: bool },
    Handoff(u64),
    Failed { id: u64, message: String },
    Cancel(u64),
}
impl Message {
    pub fn id(&self) -> u64 {
        match self {
            Self::Prepare { id, .. } | Self::Installing { id, .. } | Self::Failed { id, .. } => *id,
            Self::Ready(id) | Self::Commit(id) | Self::Handoff(id) | Self::Cancel(id) => *id,
        }
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.id() != 0, "invalid update request");
        match self {
            Self::Prepare { version, .. } => {
                ensure!(version.len() <= 48, "invalid update version");
                let version = semver::Version::parse(version)?;
                ensure!(
                    version.pre.is_empty() && version.build.is_empty(),
                    "paired updates require a stable version"
                );
            }
            Self::Failed { message, .. } => ensure!(
                message.len() <= 1024 && !message.chars().any(char::is_control),
                "invalid update status"
            ),
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    Send(Message),
    Download,
    Install,
    Quit,
    Error(String),
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Side {
    Initiator { local_first: bool },
    Receiver,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    AwaitReady,
    Download,
    Ready,
    InstallQueued,
    Installing,
    AwaitRemote,
    AwaitRemoteStart,
    SendingHandoff,
    Done,
}

pub struct Coordinator {
    pub id: u64,
    pub version: String,
    side: Side,
    phase: Phase,
    authorization: bool,
    remote_authorization: bool,
    pub handed_off: bool,
    progress: Instant,
    peer_lost: Option<Instant>,
}
impl Coordinator {
    pub fn outgoing(id: u64, version: String, local_linux: bool, remote_linux: bool) -> Self {
        Self {
            id,
            version,
            side: Side::Initiator {
                local_first: local_linux || !remote_linux,
            },
            phase: Phase::AwaitReady,
            authorization: local_linux,
            remote_authorization: remote_linux,
            handed_off: false,
            progress: Instant::now(),
            peer_lost: None,
        }
    }
    pub fn incoming(id: u64, version: String, authorization: bool) -> Result<Self> {
        Message::Prepare {
            id,
            version: version.clone(),
        }
        .validate()?;
        ensure!(
            semver::Version::parse(&version)?
                > semver::Version::parse(crate::compatibility::VERSION)?,
            "the requested update must be newer than the installed version"
        );
        Ok(Self {
            id,
            version,
            side: Side::Receiver,
            phase: Phase::Download,
            authorization,
            remote_authorization: false,
            handed_off: false,
            progress: Instant::now(),
            peer_lost: None,
        })
    }
    pub fn retry(&self) -> Option<Message> {
        Some(match self.phase {
            Phase::AwaitReady => Message::Prepare {
                id: self.id,
                version: self.version.clone(),
            },
            Phase::Ready => Message::Ready(self.id),
            Phase::InstallQueued | Phase::Installing if self.side == Side::Receiver => {
                Message::Installing {
                    id: self.id,
                    authorization: self.authorization,
                }
            }
            Phase::AwaitRemote | Phase::AwaitRemoteStart => Message::Commit(self.id),
            Phase::SendingHandoff => Message::Handoff(self.id),
            _ => return None,
        })
    }
    pub fn status(&self) -> &'static str {
        match self.phase {
            Phase::AwaitReady | Phase::Download => "Preparing the same signed update on both PCs…",
            Phase::Ready => "Update verified. Waiting for the other PC…",
            Phase::InstallQueued | Phase::Installing if self.authorization => {
                "Confirm installation in Ubuntu's system password dialog."
            }
            Phase::InstallQueued | Phase::Installing => "Installing the verified update…",
            Phase::AwaitRemote if self.remote_authorization => {
                "Confirm installation on the Ubuntu PC. This PC will update after Ubuntu accepts the installation."
            }
            Phase::AwaitRemote | Phase::AwaitRemoteStart => {
                "Waiting for the other PC to start the verified installer…"
            }
            Phase::SendingHandoff | Phase::Done => "Restarting after the update…",
        }
    }
    pub fn receive(&mut self, message: Message) -> Result<Vec<Effect>> {
        message.validate()?;
        ensure!(message.id() == self.id, "stale update request");
        self.progress = Instant::now();
        match (&self.side, self.phase, message) {
            (Side::Initiator { local_first }, Phase::AwaitReady, Message::Ready(_)) => {
                if *local_first {
                    self.phase = Phase::Installing;
                    Ok(vec![Effect::Install])
                } else {
                    self.phase = Phase::AwaitRemote;
                    Ok(vec![Effect::Send(Message::Commit(self.id))])
                }
            }
            (Side::Receiver, Phase::Ready, Message::Commit(_)) => {
                self.phase = Phase::InstallQueued;
                Ok(vec![Effect::Send(Message::Installing {
                    id: self.id,
                    authorization: self.authorization,
                })])
            }
            (Side::Receiver, Phase::InstallQueued | Phase::Installing, Message::Commit(_)) => {
                Ok(vec![Effect::Send(Message::Installing {
                    id: self.id,
                    authorization: self.authorization,
                })])
            }
            (Side::Initiator { .. }, Phase::AwaitRemoteStart, Message::Installing { .. }) => {
                self.phase = Phase::Done;
                Ok(vec![Effect::Quit])
            }
            (
                Side::Initiator { .. },
                Phase::AwaitRemote,
                Message::Installing { authorization, .. },
            ) => {
                self.remote_authorization = authorization;
                Ok(vec![])
            }
            (Side::Initiator { .. }, Phase::AwaitRemote, Message::Handoff(_)) => {
                self.phase = Phase::Installing;
                Ok(vec![Effect::Install])
            }
            (Side::Receiver, Phase::Download | Phase::Ready, Message::Prepare { version, .. })
                if version == self.version =>
            {
                Ok(self.retry().map(Effect::Send).into_iter().collect())
            }
            (_, _, Message::Failed { message, .. }) => Ok(self.fail(message, false)),
            (_, Phase::Download | Phase::Ready | Phase::AwaitReady, Message::Cancel(_)) => {
                self.phase = Phase::Done;
                Ok(vec![Effect::Error(
                    "Paired update cancelled. No installer was started on this PC.".into(),
                )])
            }
            // Replayed acknowledgements cannot start a second installation.
            (_, _, Message::Ready(_) | Message::Installing { .. } | Message::Handoff(_)) => {
                Ok(vec![])
            }
            _ => bail!("unexpected paired update command"),
        }
    }
    pub fn prepared(&mut self, version: &str) -> Vec<Effect> {
        if self.side != Side::Receiver || self.phase != Phase::Download {
            return vec![];
        }
        if version != self.version {
            return self.fail(
                "The signed release version differs from the requested paired update.".into(),
                true,
            );
        }
        self.phase = Phase::Ready;
        self.progress = Instant::now();
        vec![Effect::Send(Message::Ready(self.id))]
    }
    pub fn sent(&mut self, message: &Message) -> Vec<Effect> {
        if message.id() != self.id || self.side != Side::Receiver {
            return vec![];
        }
        if matches!(message, Message::Installing { .. }) && self.phase == Phase::InstallQueued {
            self.phase = Phase::Installing;
            self.progress = Instant::now();
            return vec![Effect::Install];
        }
        if matches!(message, Message::Handoff(_)) && self.phase == Phase::SendingHandoff {
            self.phase = Phase::Done;
            return vec![Effect::Quit];
        }
        vec![]
    }
    pub fn installed(&mut self) -> Vec<Effect> {
        if self.phase != Phase::Installing {
            return vec![];
        }
        self.handed_off = true;
        self.progress = Instant::now();
        match self.side {
            Side::Receiver => {
                self.phase = Phase::SendingHandoff;
                vec![Effect::Send(Message::Handoff(self.id))]
            }
            Side::Initiator { local_first: true } => {
                self.phase = Phase::AwaitRemoteStart;
                vec![Effect::Send(Message::Commit(self.id))]
            }
            Side::Initiator { local_first: false } => {
                self.phase = Phase::Done;
                vec![Effect::Quit]
            }
        }
    }
    pub fn fail(&mut self, mut message: String, tell_peer: bool) -> Vec<Effect> {
        message.retain(|c| !c.is_control());
        message.truncate(message.floor_char_boundary(1024));
        self.phase = Phase::Done;
        let mut effects = vec![];
        if tell_peer {
            effects.push(Effect::Send(Message::Failed {
                id: self.id,
                message: message.clone(),
            }));
        }
        effects.push(Effect::Error(message));
        if self.handed_off {
            effects.push(Effect::Quit);
        }
        effects
    }
    pub fn expired(&mut self, peer_connected: bool) -> Vec<Effect> {
        if peer_connected {
            self.peer_lost = None;
        } else {
            self.peer_lost.get_or_insert_with(Instant::now);
        }
        if self.phase == Phase::AwaitRemote
            && self.remote_authorization
            && !self
                .peer_lost
                .is_some_and(|since| since.elapsed() > Duration::from_secs(60))
        {
            return vec![];
        }
        let authorization_peer_lost = self.phase == Phase::AwaitRemote
            && self.remote_authorization
            && self
                .peer_lost
                .is_some_and(|since| since.elapsed() > Duration::from_secs(60));
        let limit = if self.handed_off {
            Duration::from_secs(15)
        } else {
            Duration::from_secs(10 * 60)
        };
        if self.phase != Phase::Done && (self.progress.elapsed() > limit || authorization_peer_lost)
        {
            self.fail("The other PC did not complete the paired update. The verified download is kept for retry.".into(), true)
        } else {
            vec![]
        }
    }
    pub fn done(&self) -> bool {
        self.phase == Phase::Done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn next_version() -> String {
        "99.0.0".into()
    }
    #[test]
    fn windows_waits_for_ubuntu_authorization_and_verified_installation() {
        let mut windows = Coordinator::outgoing(1, next_version(), false, true);
        let mut ubuntu = Coordinator::incoming(1, next_version(), true).unwrap();
        assert_eq!(ubuntu.prepared("99.0.0"), [Effect::Send(Message::Ready(1))]);
        assert_eq!(
            windows.receive(Message::Ready(1)).unwrap(),
            [Effect::Send(Message::Commit(1))]
        );
        let notice = Message::Installing {
            id: 1,
            authorization: true,
        };
        assert_eq!(
            ubuntu.receive(Message::Commit(1)).unwrap(),
            [Effect::Send(notice.clone())]
        );
        assert!(windows.receive(notice.clone()).unwrap().is_empty());
        assert_eq!(ubuntu.sent(&notice), [Effect::Install]);
        assert!(ubuntu.sent(&notice).is_empty());
        // Cancelling the Ubuntu password dialog must not update Windows.
        let effects = windows
            .receive(Message::Failed {
                id: 1,
                message: "Authorization cancelled".into(),
            })
            .unwrap();
        assert!(!effects.contains(&Effect::Install));
        assert!(windows.done());
    }
    #[test]
    fn ubuntu_first_commits_windows_only_after_its_own_install_succeeds() {
        let mut ubuntu = Coordinator::outgoing(2, next_version(), true, false);
        assert_eq!(
            ubuntu.receive(Message::Ready(2)).unwrap(),
            [Effect::Install]
        );
        assert_eq!(ubuntu.installed(), [Effect::Send(Message::Commit(2))]);
        assert_eq!(
            ubuntu
                .receive(Message::Installing {
                    id: 2,
                    authorization: false
                })
                .unwrap(),
            [Effect::Quit]
        );
    }

    #[test]
    fn windows_installs_only_after_the_remote_ubuntu_package_has_been_installed() {
        let mut windows = Coordinator::outgoing(10, next_version(), false, true);
        windows.receive(Message::Ready(10)).unwrap();
        windows
            .receive(Message::Installing {
                id: 10,
                authorization: true,
            })
            .unwrap();
        assert_eq!(windows.retry(), Some(Message::Commit(10)));
        assert!(!windows.handed_off);
        assert_eq!(
            windows.receive(Message::Handoff(10)).unwrap(),
            [Effect::Install]
        );
        assert!(windows.receive(Message::Handoff(10)).unwrap().is_empty());
        assert_eq!(windows.installed(), [Effect::Quit]);
    }

    #[test]
    fn an_open_password_dialog_can_wait_but_a_lost_ubuntu_cannot_start_windows_installation() {
        let mut windows = Coordinator::outgoing(11, next_version(), false, true);
        windows.receive(Message::Ready(11)).unwrap();
        windows
            .receive(Message::Installing {
                id: 11,
                authorization: true,
            })
            .unwrap();
        windows.progress = Instant::now() - Duration::from_secs(3600);
        assert!(windows.expired(true).is_empty());
        assert!(windows.expired(false).is_empty());
        windows.peer_lost = Some(Instant::now() - Duration::from_secs(61));
        let effects = windows.expired(false);
        assert!(windows.done());
        assert!(!effects.contains(&Effect::Install));
        assert!(effects.iter().any(|e| matches!(e, Effect::Error(_))));
    }
    #[test]
    fn receiver_requires_the_exact_signed_release_and_explicit_commit() {
        let mut peer = Coordinator::incoming(3, next_version(), false).unwrap();
        assert!(peer.receive(Message::Commit(3)).is_err());
        assert!(
            peer.prepared("99.0.1")
                .iter()
                .all(|e| *e != Effect::Install)
        );
        assert!(peer.done());
        let mut peer = Coordinator::incoming(4, next_version(), false).unwrap();
        peer.prepared("99.0.0");
        assert!(peer.receive(Message::Commit(3)).is_err());
        let notice = Message::Installing {
            id: 4,
            authorization: false,
        };
        peer.receive(Message::Commit(4)).unwrap();
        assert_eq!(peer.sent(&notice), [Effect::Install]);
        assert_eq!(peer.installed(), [Effect::Send(Message::Handoff(4))]);
        assert_eq!(peer.sent(&Message::Handoff(4)), [Effect::Quit]);
    }
}
