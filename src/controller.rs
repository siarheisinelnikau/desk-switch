use crate::{
    video::{Inputs, Job},
    wire::{Message, Mode},
};
use anyhow::{Context, Result, bail};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Transfer {
    pub id: u64,
    pub mode: Mode,
    pub edge: Option<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Local,
    Preparing,
    PeerSelecting,
    Writing,
    Verifying,
    Activating,
    Remote,
    Returning,
    Restoring,
    RestoreFallback,
    Checking,
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    Send(Message),
    Ddc(Job),
    Forward,
    Local {
        position: Option<u16>,
        upgrade: bool,
    },
}

pub struct Controller {
    phase: Phase,
    transfer: Option<Transfer>,
    inputs: Option<Inputs>,
    exit: Option<u16>,
    upgrade: bool,
    restore_fallback: bool,
}

impl Controller {
    pub fn new(inputs: Option<Inputs>) -> Self {
        Self {
            phase: Phase::Local,
            transfer: None,
            inputs,
            exit: None,
            upgrade: false,
            restore_fallback: false,
        }
    }

    pub fn start(&mut self, transfer: Transfer) -> Result<Effect> {
        if self.phase != Phase::Local {
            bail!("already switching");
        }
        if transfer.mode == Mode::Video && self.inputs.is_none() {
            bail!("shared monitor and its host/receiver VCP 0x60 inputs must be configured");
        }
        self.phase = Phase::Preparing;
        self.transfer = Some(transfer);
        Ok(Effect::Send(Message::Prepare {
            id: transfer.id,
            inputs: self.inputs.filter(|_| transfer.mode == Mode::Video),
            edge: transfer.edge,
        }))
    }

    pub fn is_remote(&self) -> bool {
        self.phase == Phase::Remote
    }

    pub fn is_local(&self) -> bool {
        self.phase == Phase::Local
    }

    pub fn return_home(&mut self, exit: Option<u16>, hotkey: bool) -> Result<Effect> {
        let transfer = self
            .transfer
            .filter(|_| self.is_remote())
            .ok_or_else(|| anyhow::anyhow!("no active receiver transfer"))?;
        if exit.is_some() && transfer.edge.is_none() {
            bail!("exit edge was never armed");
        }
        self.exit = exit;
        self.upgrade = hotkey && transfer.mode == Mode::Input && self.inputs.is_some();
        self.phase = Phase::Returning;
        Ok(Effect::Send(Message::Return(transfer.id)))
    }

    pub fn message(&mut self, message: Message) -> Result<Effect> {
        let Some(Transfer { id, mode, .. }) = self.transfer else {
            bail!("unexpected message in local mode");
        };
        match (self.phase, message) {
            (Phase::Preparing, Message::Ready(reply)) if reply == id => {
                if mode == Mode::Video {
                    self.phase = Phase::PeerSelecting;
                    Ok(Effect::Send(Message::SelectMonitor(id)))
                } else {
                    self.phase = Phase::Activating;
                    Ok(Effect::Send(Message::Activate(id)))
                }
            }
            (Phase::PeerSelecting, Message::SelectFallback(reply)) if reply == id => {
                let inputs = self
                    .inputs
                    .expect("video transfers require configured inputs");
                self.phase = Phase::Writing;
                Ok(Effect::Ddc(Job::Select {
                    from: inputs.host,
                    to: inputs.peer,
                }))
            }
            (Phase::PeerSelecting | Phase::Verifying, Message::Observed { id: reply, input })
                if reply == id =>
            {
                if self.inputs.map(|inputs| inputs.peer) != Some(input) {
                    bail!("receiver did not observe its configured monitor input");
                }
                self.phase = Phase::Activating;
                Ok(Effect::Send(Message::Activate(id)))
            }
            (Phase::Activating, Message::Active(reply)) if reply == id => {
                self.phase = Phase::Remote;
                Ok(Effect::Forward)
            }
            (Phase::Returning, Message::Returning(reply)) if reply == id => {
                if mode == Mode::Video {
                    self.phase = Phase::Restoring;
                    let inputs = self.inputs.unwrap();
                    Ok(Effect::Ddc(Job::Select {
                        from: inputs.peer,
                        to: inputs.host,
                    }))
                } else {
                    Ok(Effect::Send(Message::Returned(id)))
                }
            }
            (Phase::RestoreFallback, Message::MonitorRestored(reply)) if reply == id => {
                self.phase = Phase::Checking;
                Ok(Effect::Ddc(Job::Read))
            }
            (_, Message::Rejected(reply)) if reply == id => {
                bail!(
                    "receiver rejected the transfer; check its portal, edge, and monitor diagnostics"
                )
            }
            _ => bail!("unexpected or stale switch reply: {message:?}"),
        }
    }

    pub fn written(&mut self) -> Result<Effect> {
        let transfer = self
            .transfer
            .filter(|_| matches!(self.phase, Phase::Writing | Phase::Restoring))
            .ok_or_else(|| anyhow::anyhow!("unexpected DDC write result"))?;
        if self.phase == Phase::Restoring {
            self.phase = Phase::Checking;
            Ok(Effect::Ddc(Job::Read))
        } else {
            self.phase = Phase::Verifying;
            Ok(Effect::Send(Message::Verify(transfer.id)))
        }
    }

    pub fn write_failed(&mut self) -> Result<Effect> {
        if self.phase != Phase::Restoring {
            bail!("could not select the peer monitor input");
        }
        self.restore_on_peer()
    }

    fn restore_on_peer(&mut self) -> Result<Effect> {
        let id = self.transfer.context("missing monitor transfer")?.id;
        self.restore_fallback = true;
        self.phase = Phase::RestoreFallback;
        Ok(Effect::Send(Message::RestoreMonitor(id)))
    }

    pub fn checked(&mut self, observed: Option<u16>) -> Result<Effect> {
        let transfer = self
            .transfer
            .filter(|_| self.phase == Phase::Checking)
            .ok_or_else(|| anyhow::anyhow!("unexpected DDC read result"))?;
        if self.inputs.map(|inputs| inputs.host) == observed {
            Ok(Effect::Send(Message::Returned(transfer.id)))
        } else if !self.restore_fallback {
            self.restore_on_peer()
        } else {
            Ok(Effect::Ddc(Job::Read))
        }
    }

    pub fn finished(&mut self) -> Effect {
        let effect = Effect::Local {
            position: self.exit,
            upgrade: self.upgrade,
        };
        *self = Self::new(self.inputs);
        effect
    }

    pub fn abort(&mut self) -> bool {
        let recover = matches!(
            self.transfer,
            Some(Transfer {
                mode: Mode::Video,
                ..
            })
        ) && !matches!(self.phase, Phase::Local | Phase::Preparing);
        *self = Self::new(self.inputs);
        recover
    }

    pub fn id(&self) -> Option<u64> {
        self.transfer.map(|transfer| transfer.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_only_hotkey_round_trips_without_a_monitor_or_video_upgrade() {
        let mut controller = Controller::new(None);
        for id in 1..=3 {
            assert_eq!(
                controller
                    .start(Transfer {
                        id,
                        mode: Mode::Input,
                        edge: None
                    })
                    .unwrap(),
                Effect::Send(Message::Prepare {
                    id,
                    inputs: None,
                    edge: None
                })
            );
            assert_eq!(
                controller.message(Message::Ready(id)).unwrap(),
                Effect::Send(Message::Activate(id))
            );
            assert_eq!(
                controller.message(Message::Active(id)).unwrap(),
                Effect::Forward
            );
            assert_eq!(
                controller.return_home(None, true).unwrap(),
                Effect::Send(Message::Return(id))
            );
            assert_eq!(
                controller.message(Message::Returning(id)).unwrap(),
                Effect::Send(Message::Returned(id))
            );
            assert_eq!(
                controller.finished(),
                Effect::Local {
                    position: None,
                    upgrade: false
                }
            );
        }
    }

    const INPUTS: Inputs = Inputs { host: 17, peer: 15 };

    fn setup(mode: Mode) -> Controller {
        let mut controller = Controller::new(Some(INPUTS));
        controller
            .start(Transfer {
                id: 3,
                mode,
                edge: Some(500),
            })
            .unwrap();
        controller
    }

    #[test]
    fn write_success_cannot_enable_input_or_accept_a_false_confirmation() {
        let mut controller = setup(Mode::Video);
        assert_eq!(
            controller.message(Message::Ready(3)).unwrap(),
            Effect::Send(Message::SelectMonitor(3))
        );
        assert_eq!(
            controller.message(Message::SelectFallback(3)).unwrap(),
            Effect::Ddc(Job::Select { from: 17, to: 15 })
        );
        assert_eq!(
            controller.written().unwrap(),
            Effect::Send(Message::Verify(3))
        );
        assert!(!controller.is_remote());
        assert!(controller.message(Message::Active(3)).is_err());
        assert!(
            controller
                .message(Message::Observed { id: 2, input: 15 })
                .is_err()
        );
        assert!(
            controller
                .message(Message::Observed { id: 3, input: 17 })
                .is_err()
        );
        assert!(!controller.is_remote());
        assert!(controller.abort());
        assert!(controller.is_local());
    }

    #[test]
    fn ddc_failure_or_loss_after_start_requests_recovery() {
        let mut controller = setup(Mode::Video);
        controller.message(Message::Ready(3)).unwrap();
        assert!(controller.abort());
        assert!(!controller.is_remote());

        let mut controller = setup(Mode::Video);
        controller.message(Message::Ready(3)).unwrap();
        controller.message(Message::SelectFallback(3)).unwrap();
        controller.written().unwrap();
        controller
            .message(Message::Observed { id: 3, input: 15 })
            .unwrap();
        assert_eq!(
            controller.message(Message::Active(3)).unwrap(),
            Effect::Forward
        );
        assert!(controller.abort());
        assert!(controller.is_local());
    }

    #[test]
    fn video_return_requires_a_fresh_host_read() {
        let mut controller = setup(Mode::Video);
        assert_eq!(controller.id(), Some(3));
        controller.message(Message::Ready(3)).unwrap();
        controller
            .message(Message::Observed { id: 3, input: 15 })
            .unwrap();
        controller.message(Message::Active(3)).unwrap();
        assert_eq!(
            controller.return_home(None, true).unwrap(),
            Effect::Send(Message::Return(3))
        );
        assert_eq!(
            controller.message(Message::Returning(3)).unwrap(),
            Effect::Ddc(Job::Select { from: 15, to: 17 })
        );
        assert_eq!(controller.written().unwrap(), Effect::Ddc(Job::Read));
        assert_eq!(
            controller.checked(None).unwrap(),
            Effect::Send(Message::RestoreMonitor(3))
        );
        assert_eq!(
            controller.message(Message::MonitorRestored(3)).unwrap(),
            Effect::Ddc(Job::Read)
        );
        assert_eq!(
            controller.checked(Some(15)).unwrap(),
            Effect::Ddc(Job::Read)
        );
        assert_eq!(
            controller.checked(Some(17)).unwrap(),
            Effect::Send(Message::Returned(3))
        );
        assert_eq!(
            controller.finished(),
            Effect::Local {
                position: None,
                upgrade: false
            }
        );
        assert_eq!(controller.id(), None);
    }

    #[test]
    fn input_only_edge_return_uses_no_ddc() {
        let mut controller = setup(Mode::Input);
        assert_eq!(
            controller.message(Message::Ready(3)).unwrap(),
            Effect::Send(Message::Activate(3))
        );
        assert_eq!(
            controller.message(Message::Active(3)).unwrap(),
            Effect::Forward
        );
        assert_eq!(
            controller.return_home(Some(123), false).unwrap(),
            Effect::Send(Message::Return(3))
        );
        assert_eq!(
            controller.message(Message::Returning(3)).unwrap(),
            Effect::Send(Message::Returned(3))
        );
        assert_eq!(
            controller.finished(),
            Effect::Local {
                position: Some(123),
                upgrade: false
            }
        );
        assert!(!controller.abort());

        let mut controller = setup(Mode::Input);
        controller.message(Message::Ready(3)).unwrap();
        controller.message(Message::Active(3)).unwrap();
        assert!(!controller.abort());
    }
}
