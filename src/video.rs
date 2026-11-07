use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{sync::mpsc, time::Duration};
use tokio::sync::oneshot;

pub const SETTLE: Duration = Duration::from_millis(600);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Inputs {
    pub host: u16,
    pub peer: u16,
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
    let read = |monitor: &mut _| {
        let result = Monitor::read(monitor);
        crate::lifecycle::report(crate::lifecycle::Event::Monitor(
            result.as_ref().ok().copied(),
        ));
        result
    };
    match job {
        Job::Read => Ok(VideoResult::Read(read(monitor)?)),
        Job::Select { from, to } => {
            let actual = read(monitor)?;
            if actual == to {
                return Ok(VideoResult::Written);
            }
            if actual != from {
                bail!("VCP 0x60 reported {actual:#06x}; expected the source {from:#06x}");
            }
            crate::lifecycle::report(crate::lifecycle::Event::Monitor(None));
            monitor.write(to)?;
            Ok(VideoResult::Written)
        }
        Job::Recover(inputs) => {
            let actual = read(monitor)?;
            if actual == inputs.peer {
                crate::lifecycle::report(crate::lifecycle::Event::Monitor(None));
                monitor.write(inputs.host)?;
            } else if actual != inputs.host {
                bail!("recovery saw an unconfigured VCP 0x60 value {actual:#06x}");
            }
            Ok(VideoResult::Written)
        }
    }
}

type Reply = oneshot::Receiver<Result<VideoResult>>;

pub struct VideoWorker(mpsc::SyncSender<(Job, oneshot::Sender<Result<VideoResult>>)>);

impl VideoWorker {
    pub fn start(monitor: impl Monitor) -> Result<Self> {
        Self::start_worker(monitor, None)
    }

    pub fn watch(monitor: impl Monitor) -> Result<Self> {
        Self::start_worker(monitor, Some(Duration::from_secs(5)))
    }

    fn start_worker(mut monitor: impl Monitor, status_interval: Option<Duration>) -> Result<Self> {
        let (send, receive) = mpsc::sync_channel::<(Job, oneshot::Sender<_>)>(1);
        std::thread::Builder::new()
            .name("DDC".into())
            .spawn(move || {
                if status_interval.is_some() {
                    let _ = execute(&mut monitor, Job::Read);
                }
                loop {
                    let job = match status_interval {
                        Some(interval) => receive.recv_timeout(interval),
                        None => receive
                            .recv()
                            .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                    };
                    match job {
                        Ok((job, reply)) => {
                            if !reply.is_closed() {
                                let _ = reply.send(execute(&mut monitor, job));
                            }
                        }
                        // Keep the diagram accurate after a monitor-menu change.
                        // All reads and writes still use this one serialized worker.
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let _ = execute(&mut monitor, Job::Read);
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })?;
        Ok(Self(send))
    }

    pub fn submit(&self, job: Job) -> Result<Reply> {
        let (send, receive) = oneshot::channel();
        self.0
            .try_send((job, send))
            .map_err(|_| anyhow::anyhow!("DDC worker busy or stopped"))?;
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
        let boundary =
            prefix.is_empty() || prefix.ends_with('(') || prefix.ends_with(char::is_whitespace);
        if boundary
            && let Some(values) = tail.trim_start().strip_prefix('(')
            && let Some((values, _)) = values.split_once(')')
        {
            let mut codes = values
                .split_whitespace()
                .map(|value| u16::from_str_radix(value, 16))
                .collect::<Result<Vec<_>, _>>()
                .unwrap_or_default();
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
    use std::sync::{
        Arc,
        atomic::{AtomicU16, AtomicUsize, Ordering::SeqCst},
    };

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
        let mut fixture = Fixture {
            selected: 1,
            monitors: [0x11, 0x11],
        };
        execute(
            &mut fixture,
            Job::Select {
                from: 0x11,
                to: 0x0f,
            },
        )
        .unwrap();
        assert_eq!(fixture.monitors, [0x11, 0x0f]);
        assert!(
            execute(
                &mut fixture,
                Job::Select {
                    from: 0x11,
                    to: 0x03
                }
            )
            .is_err()
        );
        assert_eq!(fixture.monitors, [0x11, 0x0f]);
    }

    #[test]
    fn capabilities_are_advisory() {
        assert_eq!(input_codes("vcp(10 12 60(11 0F 11) 62)"), [15, 17]);
        assert!(input_codes("vcp(10 60 62)").is_empty());
    }

    #[tokio::test]
    async fn status_watcher_only_reads_until_an_explicit_switch_job() {
        struct Observed {
            input: Arc<AtomicU16>,
            reads: Arc<AtomicUsize>,
            writes: Arc<AtomicUsize>,
        }
        impl Monitor for Observed {
            fn read(&mut self) -> Result<u16> {
                self.reads.fetch_add(1, SeqCst);
                Ok(self.input.load(SeqCst))
            }
            fn write(&mut self, input: u16) -> Result<()> {
                self.writes.fetch_add(1, SeqCst);
                self.input.store(input, SeqCst);
                Ok(())
            }
        }
        let input = Arc::new(AtomicU16::new(17));
        let reads = Arc::new(AtomicUsize::new(0));
        let writes = Arc::new(AtomicUsize::new(0));
        let worker = VideoWorker::start_worker(
            Observed {
                input: Arc::clone(&input),
                reads: Arc::clone(&reads),
                writes: Arc::clone(&writes),
            },
            Some(Duration::from_millis(10)),
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while reads.load(SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(writes.load(SeqCst), 0);
        input.store(15, SeqCst); // A monitor-menu change, with no programmatic write.
        assert!(matches!(
            worker.submit(Job::Read).unwrap().await.unwrap().unwrap(),
            VideoResult::Read(15)
        ));
        assert_eq!(writes.load(SeqCst), 0);
        worker
            .submit(Job::Select { from: 15, to: 17 })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(writes.load(SeqCst), 1);
        assert!(matches!(
            worker.submit(Job::Read).unwrap().await.unwrap().unwrap(),
            VideoResult::Read(17)
        ));
    }
}
