use crate::{
    video::{Inputs, Job},
    wire::{Message, Mode},
};
use anyhow::{Result, bail, ensure};

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
    Writing,
    AwaitSelected,
    Activating,
    Remote,
    Returning,
    Restoring,
    ReturnWriting,
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
}
impl Controller {
    pub fn new(inputs: Option<Inputs>) -> Self {
        Self {
            phase: Phase::Local,
            transfer: None,
            inputs,
            exit: None,
            upgrade: false,
        }
    }
    pub fn start(&mut self, transfer: Transfer) -> Result<Effect> {
        ensure!(self.phase == Phase::Local, "already switching");
        ensure!(
            transfer.mode != Mode::Video || self.inputs.is_some(),
            "remember the monitor connection on both PCs first"
        );
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
        ensure!(
            exit.is_none() || transfer.edge.is_some(),
            "exit edge was never armed"
        );
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
                    self.phase = Phase::Writing;
                    let inputs = self.inputs.unwrap();
                    // The currently visible PC owns the only normal input write.
                    Ok(Effect::Ddc(Job::Select {
                        from: inputs.host,
                        to: inputs.peer,
                    }))
                } else {
                    self.phase = Phase::Activating;
                    Ok(Effect::Send(Message::Activate(id)))
                }
            }
            (Phase::AwaitSelected, Message::MonitorReady(reply)) if reply == id => {
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
                    Ok(Effect::Send(Message::RestoreMonitor(id)))
                } else {
                    Ok(Effect::Send(Message::Returned(id)))
                }
            }
            (Phase::Restoring, Message::MonitorRestored(reply)) if reply == id => {
                Ok(Effect::Send(Message::Returned(id)))
            }
            (Phase::Restoring, Message::SelectFallback(reply)) if reply == id => {
                self.phase = Phase::ReturnWriting;
                let inputs = self.inputs.unwrap();
                Ok(Effect::Ddc(Job::Select {
                    from: inputs.peer,
                    to: inputs.host,
                }))
            }
            (_, Message::Rejected(reply)) if reply == id => {
                bail!("receiver rejected the transfer; check its input and monitor diagnostics")
            }
            _ => bail!("unexpected or stale switch reply: {message:?}"),
        }
    }
    pub fn written(&mut self) -> Result<Effect> {
        let transfer = self
            .transfer
            .ok_or_else(|| anyhow::anyhow!("missing transfer"))?;
        match self.phase {
            Phase::Writing => {
                self.phase = Phase::AwaitSelected;
                Ok(Effect::Send(Message::MonitorSelected(transfer.id)))
            }
            Phase::ReturnWriting => Ok(Effect::Send(Message::Returned(transfer.id))),
            _ => bail!("unexpected DDC write result"),
        }
    }
    pub fn write_failed(&mut self) -> Result<Effect> {
        // This fallback is allowed only when no input-selection call was issued.
        ensure!(
            self.phase == Phase::Writing,
            "could not select the remembered monitor input"
        );
        self.phase = Phase::AwaitSelected;
        Ok(Effect::Send(Message::SelectMonitor(
            self.transfer.unwrap().id,
        )))
    }
    pub fn finished(&mut self) -> Effect {
        let effect = Effect::Local {
            position: self.exit,
            upgrade: self.upgrade,
        };
        *self = Self::new(self.inputs);
        effect
    }
    pub fn abort(&mut self) {
        *self = Self::new(self.inputs);
    }
    pub fn id(&self) -> Option<u64> {
        self.transfer.map(|transfer| transfer.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const INPUTS: Inputs = Inputs { host: 17, peer: 15 };
    fn prepared(mode: Mode) -> Controller {
        let mut controller = Controller::new(Some(INPUTS));
        controller
            .start(Transfer {
                id: 1,
                mode,
                edge: None,
            })
            .unwrap();
        controller
    }
    #[test]
    fn active_port_writes_once_and_the_destination_does_not_read_back_ddc() {
        let mut controller = prepared(Mode::Video);
        assert_eq!(
            controller.message(Message::Ready(1)).unwrap(),
            Effect::Ddc(Job::Select { from: 17, to: 15 })
        );
        assert_eq!(
            controller.written().unwrap(),
            Effect::Send(Message::MonitorSelected(1))
        );
        assert_eq!(
            controller.message(Message::MonitorReady(1)).unwrap(),
            Effect::Send(Message::Activate(1))
        );
        assert_eq!(
            controller.message(Message::Active(1)).unwrap(),
            Effect::Forward
        );
        assert_eq!(
            controller.return_home(None, true).unwrap(),
            Effect::Send(Message::Return(1))
        );
        assert_eq!(
            controller.message(Message::Returning(1)).unwrap(),
            Effect::Send(Message::RestoreMonitor(1))
        );
        assert_eq!(
            controller.message(Message::MonitorRestored(1)).unwrap(),
            Effect::Send(Message::Returned(1))
        );
        controller.finished();
        assert!(controller.is_local());
    }
    #[test]
    fn fallback_does_not_retry_the_same_port_or_accept_stale_acknowledgements() {
        let mut controller = prepared(Mode::Video);
        controller.message(Message::Ready(1)).unwrap();
        assert_eq!(
            controller.write_failed().unwrap(),
            Effect::Send(Message::SelectMonitor(1))
        );
        assert!(controller.write_failed().is_err());
        assert!(controller.message(Message::MonitorReady(2)).is_err());
        controller.message(Message::MonitorReady(1)).unwrap();
        controller.message(Message::Active(1)).unwrap();
        controller.return_home(None, true).unwrap();
        controller.message(Message::Returning(1)).unwrap();
        assert_eq!(
            controller.message(Message::SelectFallback(1)).unwrap(),
            Effect::Ddc(Job::Select { from: 15, to: 17 })
        );
        assert_eq!(
            controller.written().unwrap(),
            Effect::Send(Message::Returned(1))
        );
    }
    #[test]
    fn input_only_round_trip_never_selects_a_monitor() {
        let mut controller = Controller::new(None);
        for id in 1..=3 {
            controller
                .start(Transfer {
                    id,
                    mode: Mode::Input,
                    edge: None,
                })
                .unwrap();
            assert_eq!(
                controller.message(Message::Ready(id)).unwrap(),
                Effect::Send(Message::Activate(id))
            );
            controller.message(Message::Active(id)).unwrap();
            controller.return_home(None, true).unwrap();
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
    #[test]
    fn disconnect_has_no_source_side_video_recovery() {
        let mut controller = prepared(Mode::Video);
        controller.message(Message::Ready(1)).unwrap();
        controller.written().unwrap();
        controller.message(Message::MonitorReady(1)).unwrap();
        controller.message(Message::Active(1)).unwrap();
        controller.abort();
        assert!(controller.is_local());
        assert_eq!(controller.id(), None);
    }
}
