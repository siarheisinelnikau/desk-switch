use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
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

pub fn select_once(mut monitor: impl Monitor, input: u16) -> Result<()> {
    anyhow::ensure!(input != 0, "Select a valid monitor input");
    write(&mut monitor, input)
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

#[derive(Debug)]
struct WriteAttempted;
impl std::fmt::Display for WriteAttempted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "a monitor input command was issued; verification will not resend it"
        )
    }
}
impl std::error::Error for WriteAttempted {}

pub fn write_attempted(error: &anyhow::Error) -> bool {
    error.is::<WriteAttempted>()
}

#[derive(Debug)]
struct NotIssued;
impl std::fmt::Display for NotIssued {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no monitor input command was issued")
    }
}
impl std::error::Error for NotIssued {}
pub fn not_issued(error: anyhow::Error) -> anyhow::Error {
    error.context(NotIssued)
}

fn write(monitor: &mut impl Monitor, input: u16) -> Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COMMAND: AtomicU64 = AtomicU64::new(1);
    println!(
        "Monitor selection request #{}: select {input:#06x}; automatic readback is disabled.",
        COMMAND.fetch_add(1, Ordering::Relaxed)
    );
    match monitor.write(input) {
        Err(error) if error.is::<NotIssued>() => Err(error),
        result => result.context(WriteAttempted),
    }
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
        Job::Select { from: _, to } => {
            crate::lifecycle::report(crate::lifecycle::Event::Monitor(None));
            write(monitor, to)?;
            // This is the accepted selection command, not a readback measurement.
            crate::lifecycle::report(crate::lifecycle::Event::Monitor(Some(to)));
            Ok(VideoResult::Written)
        }
        Job::Recover(inputs) => {
            crate::lifecycle::report(crate::lifecycle::Event::Monitor(None));
            write(monitor, inputs.host)?;
            crate::lifecycle::report(crate::lifecycle::Event::Monitor(Some(inputs.host)));
            Ok(VideoResult::Written)
        }
    }
}

pub fn read_once(mut monitor: impl Monitor) -> Result<u16> {
    match execute(&mut monitor, Job::Read)? {
        VideoResult::Read(value) => Ok(value),
        _ => bail!("invalid explicit monitor read"),
    }
}

type Reply = oneshot::Receiver<Result<VideoResult>>;

pub struct VideoWorker {
    send: mpsc::SyncSender<(Job, oneshot::Sender<Result<VideoResult>>)>,
    busy: Arc<AtomicBool>,
}

impl VideoWorker {
    pub fn start(monitor: impl Monitor) -> Result<Self> {
        Self::start_worker(monitor)
    }

    pub fn watch(monitor: impl Monitor) -> Result<Self> {
        Self::start_worker(monitor)
    }

    fn start_worker(mut monitor: impl Monitor) -> Result<Self> {
        let (send, receive) = mpsc::sync_channel::<(Job, oneshot::Sender<_>)>(1);
        let busy = Arc::new(AtomicBool::new(false));
        let pending = busy.clone();
        std::thread::Builder::new()
            .name("DDC".into())
            .spawn(move || {
                // Startup and idle never issue DDC calls. Calibration is explicit.
                for (job, reply) in receive {
                    if !reply.is_closed() {
                        let result = execute(&mut monitor, job);
                        pending.store(false, Ordering::Release);
                        let _ = reply.send(result);
                    } else {
                        pending.store(false, Ordering::Release);
                    }
                }
            })?;
        Ok(Self { send, busy })
    }

    pub fn submit(&self, job: Job) -> Result<Reply> {
        let (send, receive) = oneshot::channel();
        anyhow::ensure!(
            self.busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            "DDC worker already has an input command in flight"
        );
        if self.send.try_send((job, send)).is_err() {
            self.busy.store(false, Ordering::Release);
            bail!("DDC worker busy or stopped");
        }
        Ok(receive)
    }

    pub async fn recover(&self, inputs: Inputs) -> Result<()> {
        // An unfinished native call must not accumulate another selection.
        self.submit(Job::Recover(inputs))?.await??;
        Ok(())
    }
}

#[cfg(any(windows, target_os = "linux", test))]
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
        execute(
            &mut fixture,
            Job::Select {
                from: 0x11,
                to: 0x03,
            },
        )
        .unwrap();
        assert_eq!(fixture.monitors, [0x11, 0x03]);
    }

    #[test]
    fn capabilities_are_advisory() {
        assert_eq!(input_codes("vcp(10 12 60(11 0F 11) 62)"), [15, 17]);
        assert!(input_codes("vcp(10 60 62)").is_empty());
    }

    #[tokio::test]
    async fn startup_and_idle_never_access_ddc_and_calibration_is_explicit() {
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
        let worker = VideoWorker::start_worker(Observed {
            input: Arc::clone(&input),
            reads: Arc::clone(&reads),
            writes: Arc::clone(&writes),
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(writes.load(SeqCst), 0);
        assert_eq!(reads.load(SeqCst), 0);
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

    #[tokio::test]
    async fn cancelled_slow_write_cannot_accumulate_another_monitor_command() {
        struct Slow {
            started: mpsc::Sender<()>,
            finish: mpsc::Receiver<()>,
            writes: Arc<AtomicUsize>,
        }
        impl Monitor for Slow {
            fn read(&mut self) -> Result<u16> {
                panic!("an input selection must not read the monitor");
            }
            fn write(&mut self, _: u16) -> Result<()> {
                self.writes.fetch_add(1, SeqCst);
                self.started.send(())?;
                self.finish.recv()?;
                Ok(())
            }
        }
        let (started, receive) = mpsc::channel();
        let (finish, wait) = mpsc::channel();
        let writes = Arc::new(AtomicUsize::new(0));
        let worker = VideoWorker::start(Slow {
            started,
            finish: wait,
            writes: writes.clone(),
        })
        .unwrap();
        let reply = worker.submit(Job::Select { from: 17, to: 15 }).unwrap();
        receive.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(reply); // A timed-out or disconnected session abandons its reply.
        assert!(worker.submit(Job::Select { from: 15, to: 17 }).is_err());
        assert!(
            worker
                .submit(Job::Recover(Inputs { host: 17, peer: 15 }))
                .is_err()
        );
        finish.send(()).unwrap();
        while worker.busy.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
        assert_eq!(writes.load(SeqCst), 1);
    }

    #[test]
    fn driver_errors_after_issuance_forbid_a_second_endpoint_attempt() {
        struct Failing(bool);
        impl Monitor for Failing {
            fn read(&mut self) -> Result<u16> {
                panic!("no automatic monitor reads");
            }
            fn write(&mut self, _: u16) -> Result<()> {
                let error = anyhow::anyhow!("simulated native failure");
                Err(if self.0 { not_issued(error) } else { error })
            }
        }
        let job = Job::Select { from: 17, to: 15 };
        assert!(write_attempted(
            &execute(&mut Failing(false), job).unwrap_err()
        ));
        assert!(!write_attempted(
            &execute(&mut Failing(true), job).unwrap_err()
        ));
    }
}
