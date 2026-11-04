use anyhow::{Result, bail};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

pub const SETTLE: Duration = Duration::from_millis(600);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Inputs {
    pub windows: u16,
    pub ubuntu: u16,
}

pub trait Monitor: Send + 'static {
    fn read(&mut self) -> Result<u16>;
    fn write(&mut self, input: u16) -> Result<()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Job {
    Read,
    Select { from: u16, to: u16 },
    Recover(Inputs),
}

#[derive(Debug)]
pub enum VideoResult {
    Read(u16),
    Written,
}

fn execute(monitor: &mut impl Monitor, job: Job) -> Result<VideoResult> {
    match job {
        Job::Read => Ok(VideoResult::Read(monitor.read()?)),
        Job::Select { from, to } => {
            let actual = monitor.read()?;
            if actual != from {
                bail!("VCP 0x60 reported {actual:#06x}; expected the source {from:#06x}");
            }
            monitor.write(to)?;
            Ok(VideoResult::Written)
        }
        Job::Recover(inputs) => {
            let actual = monitor.read()?;
            if actual == inputs.ubuntu {
                monitor.write(inputs.windows)?;
            } else if actual != inputs.windows {
                bail!("recovery saw an unconfigured VCP 0x60 value {actual:#06x}");
            }
            Ok(VideoResult::Written)
        }
    }
}

type Reply = oneshot::Receiver<Result<VideoResult>>;

pub struct VideoWorker(mpsc::Sender<(Job, oneshot::Sender<Result<VideoResult>>)>);

impl VideoWorker {
    pub fn start(mut monitor: impl Monitor) -> Result<Self> {
        let (send, mut receive) = mpsc::channel::<(Job, oneshot::Sender<_>)>(1);
        std::thread::Builder::new().name("DDC".into()).spawn(move || {
            while let Some((job, reply)) = receive.blocking_recv() {
                if !reply.is_closed() {
                    let _ = reply.send(execute(&mut monitor, job));
                }
            }
        })?;
        Ok(Self(send))
    }

    pub fn submit(&self, job: Job) -> Result<Reply> {
        let (send, receive) = oneshot::channel();
        self.0.try_send((job, send)).map_err(|_| anyhow::anyhow!("DDC worker busy or stopped"))?;
        Ok(receive)
    }

    pub async fn recover(&self, inputs: Inputs) -> Result<()> {
        // The worker serializes recovery after any still-running write.
        self.submit(Job::Recover(inputs))?.await??;
        Ok(())
    }
}

pub fn input_codes(capabilities: &str) -> Vec<u16> {
    let mut remaining = capabilities;
    while let Some((prefix, tail)) = remaining.split_once("60") {
        remaining = tail;
        let boundary = prefix.is_empty()
            || prefix.ends_with('(')
            || prefix.ends_with(char::is_whitespace);
        if boundary
            && let Some(values) = tail.trim_start().strip_prefix('(')
            && let Some((values, _)) = values.split_once(')')
        {
            let mut codes = values.split_whitespace()
                .map(|value| u16::from_str_radix(value, 16))
                .collect::<Result<Vec<_>, _>>().unwrap_or_default();
            codes.sort_unstable();
            codes.dedup();
            return codes;
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        selected: usize,
        monitors: [u16; 2],
    }

    impl Monitor for Fixture {
        fn read(&mut self) -> Result<u16> {
            Ok(self.monitors[self.selected])
        }
        fn write(&mut self, input: u16) -> Result<()> {
            self.monitors[self.selected] = input;
            Ok(())
        }
    }

    #[test]
    fn writes_only_to_the_selected_monitor() {
        let mut fixture = Fixture { selected: 1, monitors: [0x11, 0x11] };
        execute(&mut fixture, Job::Select { from: 0x11, to: 0x0f }).unwrap();
        assert_eq!(fixture.monitors, [0x11, 0x0f]);
        assert!(execute(&mut fixture, Job::Select { from: 0x11, to: 0x03 }).is_err());
        assert_eq!(fixture.monitors, [0x11, 0x0f]);
    }

    #[test]
    fn capabilities_are_advisory() {
        assert_eq!(input_codes("vcp(10 12 60(11 0F 11) 62)"), [15, 17]);
        assert!(input_codes("vcp(10 60 62)").is_empty());
    }
}
