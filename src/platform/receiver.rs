use crate::{
    config::{Identity, Key},
    transport::{LEASE, Pipe},
    video::{Inputs, Job, SETTLE, VideoResult, VideoWorker},
    wire::{Input, Message, Mode},
};
use anyhow::{Context, Result, bail, ensure};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    net::TcpListener,
    sync::oneshot,
    time::{Instant, MissedTickBehavior, interval},
};

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Armed,
    Selecting,
    Settling,
    RestoreSettling,
    Ready,
    Active,
    Exiting,
    Restoring,
    Returned,
}

struct Transfer {
    id: u64,
    mode: Mode,
    edge: Option<u16>,
    stage: Stage,
    deadline: Instant,
}

const SWITCH_TIMEOUT: Duration = Duration::from_secs(15);

pub trait Sink {
    async fn poll(&mut self) -> Result<()>;
    fn prepare(&self, edge: Option<u16>, mode: Mode) -> Result<()>;
    fn activate(&mut self, edge: Option<u16>, mode: Mode) -> Result<()>;
    fn event(&mut self, event: Input) -> Result<Option<u16>>;
    fn release(&mut self) -> Result<()>;
}

async fn disconnect(
    sink: &mut impl Sink,
    transfer: Option<Transfer>,
    video: Option<&(Inputs, VideoWorker)>,
) -> Result<()> {
    let released = sink.release();
    if transfer.is_some_and(|active| {
        active.mode == Mode::Video && matches!(active.stage, Stage::Active | Stage::Exiting)
    }) && let Some((inputs, worker)) = video
    {
        eprintln!("Attempting to restore the shared monitor to host.");
        if let Err(error) = worker.recover(*inputs).await {
            eprintln!("Automatic video recovery command failed: {error:#}");
        }
    }
    released
}

pub async fn run(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    sink: &mut impl Sink,
    video: Option<(Inputs, VideoWorker)>,
) -> Result<()> {
    let listener = TcpListener::bind(address)
        .await
        .context("binding receiver listen address")?;
    println!("Listening for the pinned host peer on {address}.");
    crate::lifecycle::report(crate::lifecycle::Event::Input(
        crate::lifecycle::InputStatus::Waiting,
    ));
    let shutdown = crate::lifecycle::shutdown();
    tokio::pin!(shutdown);
    loop {
        let (stream, _) = tokio::select! {
            result = &mut shutdown => { result?; break; }
            event = sink.poll() => {
                event?;
                continue;
            }
            result = listener.accept() => result?,
        };
        let mut pipe = match Pipe::connect(stream, &identity, &peer, false).await {
            Ok(pipe) => pipe,
            Err(error) => {
                eprintln!("Discarded unauthenticated connection: {error:#}");
                continue;
            }
        };
        println!("host peer authenticated.");
        crate::lifecycle::report(crate::lifecycle::Event::Input(
            crate::lifecycle::InputStatus::Peer,
        ));
        let mut transfer = None;
        let mut last_id = 0;
        let result = tokio::select! {
            result = &mut shutdown => {
                result?;
                None
            }
            result = session(&mut pipe, sink, video.as_ref(), &mut transfer, &mut last_id) => Some(result),
        };
        drop(pipe);
        if let Some(Err(error)) = &result {
            eprintln!("Session ended: {error:#}");
            if transfer.is_some() {
                crate::lifecycle::report(crate::lifecycle::Event::Switch {
                    error: Some(format!("{error:#}")),
                });
            }
        }
        if let Err(error) = disconnect(sink, transfer, video.as_ref()).await {
            return Err(error.context("input adapter failed to release its held keys/buttons"));
        }
        if result.is_none() {
            break;
        }
        crate::lifecycle::report(crate::lifecycle::Event::Input(
            crate::lifecycle::InputStatus::Waiting,
        ));
    }
    Ok(())
}

async fn session(
    pipe: &mut Pipe,
    sink: &mut impl Sink,
    video: Option<&(Inputs, VideoWorker)>,
    transfer: &mut Option<Transfer>,
    last_id: &mut u64,
) -> Result<()> {
    let mut commands = crate::lifecycle::commands();
    let mut lease = Instant::now() + LEASE;
    let mut settle_until = Instant::now();
    let mut job: Option<oneshot::Receiver<Result<VideoResult>>> = None;
    let mut tick = interval(Duration::from_millis(25));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        let now = Instant::now();
        ensure!(now < lease, "host heartbeat timed out");
        ensure!(
            transfer
                .as_ref()
                .is_none_or(|active| active.stage == Stage::Active || now < active.deadline),
            "host did not complete the switch before the deadline"
        );
        tokio::select! {
            command = commands.recv() => match command {
                Ok(crate::lifecycle::Command::Switch) => pipe.send(Message::SwitchRequested).await?,
                Ok(crate::lifecycle::Command::SetupSwitch) => pipe.send(Message::SetupSwitchRequested).await?,
                Ok(crate::lifecycle::Command::Emergency) => bail!("local emergency recovery requested"),
                Ok(crate::lifecycle::Command::MicrophoneVolume(_)) | Err(_) => {}
            },
            _ = tick.tick() => {
                let Some(active) = transfer.as_mut() else { continue };
                if let Some(pending) = &mut job {
                    match pending.try_recv() {
                        Ok(result) => {
                            job = None;
                            match active.stage {
                                Stage::Selecting => {
                                    result.context("fallback monitor selection failed; no retry was sent")?;
                                    active.stage = Stage::Settling;
                                    settle_until = Instant::now() + SETTLE;
                                },
                                Stage::Restoring => match result {
                                    Ok(VideoResult::Written) => {
                                        active.stage = Stage::RestoreSettling;
                                        settle_until = Instant::now() + SETTLE;
                                    },
                                    Err(error) if !crate::video::write_attempted(&error) => {
                                        active.stage = Stage::Returned;
                                        pipe.send(Message::SelectFallback(active.id)).await?;
                                    },
                                    Err(error) => return Err(error.context("monitor return failed after issuance; no retry was sent")),
                                    _ => bail!("invalid monitor selection result"),
                                },
                                _ => bail!("unexpected DDC job result"),
                            }
                        }
                        Err(oneshot::error::TryRecvError::Empty) => {}
                        Err(oneshot::error::TryRecvError::Closed) => bail!("receiver DDC worker stopped"),
                    }
                }
                if job.is_none() && Instant::now() >= settle_until {
                    if active.stage == Stage::Settling {
                        active.stage = Stage::Ready;
                        crate::lifecycle::report(crate::lifecycle::Event::Monitor(Some(video.unwrap().0.peer)));
                        pipe.send(Message::MonitorReady(active.id)).await?;
                    } else if active.stage == Stage::RestoreSettling {
                        active.stage = Stage::Returned;
                        pipe.send(Message::MonitorRestored(active.id)).await?;
                    }
                }
            }
            result = sink.poll() => result?,
            message = pipe.messages.recv() => {
                let message = message.context("host disconnected")??;
                if let Message::Ping(nonce) = message {
                    lease = Instant::now() + LEASE;
                    pipe.send(Message::Pong(nonce)).await?;
                    continue;
                }
                if let Message::Prepare { id, inputs, edge } = message {
                    let mode = if inputs.is_some() { Mode::Video } else { Mode::Input };
                    ensure!(id > *last_id && transfer.is_none(), "stale or overlapping transfer");
                    *last_id = id;
                    crate::lifecycle::report(crate::lifecycle::Event::Switch { error: None });
                    if let Err(error) = sink.prepare(edge, mode) {
                        eprintln!("Transfer rejected: {error:#}");
                        pipe.send(Message::Rejected(id)).await?;
                        continue;
                    }
                    if inputs.is_some() && inputs != video.map(|(local, _)| *local) {
                        eprintln!("Transfer rejected: the configured host/receiver input values differ between peers.");
                        pipe.send(Message::Rejected(id)).await?;
                        continue;
                    }
                    *transfer = Some(Transfer { id, mode, edge, stage: Stage::Armed, deadline: Instant::now() + SWITCH_TIMEOUT });
                    pipe.send(Message::Ready(id)).await?;
                    continue;
                }
                let active = transfer.as_mut().context("transfer message without a transfer")?;
                match message {
                    Message::SelectMonitor(id) if active.id == id && active.mode == Mode::Video && active.stage == Stage::Armed => {
                        active.stage = Stage::Selecting;
                        let (inputs, worker) = video.unwrap();
                        job = Some(worker.submit(Job::Select { from: inputs.host, to: inputs.peer })?);
                    }
                    Message::MonitorSelected(id) if active.id == id && active.mode == Mode::Video && active.stage == Stage::Armed => {
                        active.stage = Stage::Settling;
                        settle_until = Instant::now() + SETTLE;
                    }
                    Message::Activate(id) if active.id == id && (
                        active.stage == Stage::Ready || (active.stage == Stage::Armed && active.mode == Mode::Input)
                    ) => {
                        sink.activate(active.edge, active.mode)?;
                        active.stage = Stage::Active;
                        pipe.send(Message::Active(id)).await?;
                        crate::lifecycle::report(crate::lifecycle::Event::Input(crate::lifecycle::InputStatus::Here));
                        println!("Input active on receiver ({:?}).", active.mode);
                    }
                    Message::Event(input) if active.stage == Stage::Active => {
                        if let Some(position) = sink.event(input)? {
                            active.stage = Stage::Exiting;
                            active.deadline = Instant::now() + SWITCH_TIMEOUT;
                            pipe.send(Message::ExitEdge { id: active.id, position }).await?;
                        }
                    }
                    Message::Event(_) if active.stage == Stage::Exiting => {}
                    Message::Return(id) if active.id == id && matches!(active.stage, Stage::Active | Stage::Exiting) => {
                        sink.release()?;
                        active.deadline = Instant::now() + SWITCH_TIMEOUT;
                        // Release input before the active receiver selects the saved host port.
                        active.stage = Stage::Returned;
                        pipe.send(Message::Returning(id)).await?;
                    }
                    Message::RestoreMonitor(id) if active.id == id && active.mode == Mode::Video && active.stage == Stage::Returned => {
                        let (inputs, worker) = video.unwrap();
                        active.stage = Stage::Restoring;
                        job = Some(worker.submit(Job::Select { from: inputs.peer, to: inputs.host })?);
                    }
                    Message::Returned(id) if active.id == id && active.stage == Stage::Returned => {
                        *transfer = None;
                        crate::lifecycle::report(crate::lifecycle::Event::Input(crate::lifecycle::InputStatus::Peer));
                        println!("Input returned to host.");
                    }
                    _ => bail!("unexpected, stale or out-of-phase command"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::NOISE, video::Monitor};
    use snow::Builder;
    use std::sync::{
        Arc,
        atomic::{AtomicU16, AtomicUsize, Ordering::SeqCst},
    };
    use tokio::net::TcpStream;

    const INPUTS: Inputs = Inputs { host: 17, peer: 15 };
    const KEY: Input = Input::Key {
        code: 30,
        down: true,
    };

    #[derive(Default)]
    struct SyntheticInput {
        active: bool,
        held: bool,
        releases: usize,
    }

    impl Sink for SyntheticInput {
        async fn poll(&mut self) -> Result<()> {
            std::future::pending().await
        }

        fn prepare(&self, _: Option<u16>, _: Mode) -> Result<()> {
            ensure!(!self.active, "input is already active");
            Ok(())
        }

        fn activate(&mut self, _: Option<u16>, _: Mode) -> Result<()> {
            self.active = true;
            Ok(())
        }

        fn event(&mut self, event: Input) -> Result<Option<u16>> {
            assert!(self.active && event == KEY);
            self.held = true;
            Ok(None)
        }

        fn release(&mut self) -> Result<()> {
            if self.held {
                self.releases += 1;
                self.held = false;
            }
            self.active = false;
            Ok(())
        }
    }

    #[derive(Default)]
    struct FixtureMonitor {
        input: AtomicU16,
        calls: AtomicUsize,
    }

    impl Monitor for Arc<FixtureMonitor> {
        fn read(&mut self) -> Result<u16> {
            self.calls.fetch_add(1, SeqCst);
            Ok(self.input.load(SeqCst))
        }

        fn write(&mut self, input: u16) -> Result<()> {
            self.calls.fetch_add(1, SeqCst);
            self.input.store(input, SeqCst);
            Ok(())
        }
    }

    fn identity() -> Identity {
        let pair = Builder::new(NOISE.parse().unwrap())
            .generate_keypair()
            .unwrap();
        Identity {
            private: pair.private.try_into().unwrap(),
            public: pair.public.try_into().unwrap(),
        }
    }

    async fn fixture(mode: Mode, restore_on_active_port: bool) {
        let windows = identity();
        let ubuntu = identity();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let monitor = Arc::new(FixtureMonitor::default());
        monitor.input.store(INPUTS.host, SeqCst);
        let worker = (INPUTS, VideoWorker::start(Arc::clone(&monitor)).unwrap());
        let client = async {
            let stream = TcpStream::connect(address).await.unwrap();
            let mut pipe = Pipe::connect(stream, &windows, &ubuntu.public, true)
                .await
                .unwrap();
            pipe.send(Message::Ping(1)).await.unwrap();
            assert_eq!(
                pipe.messages.recv().await.unwrap().unwrap(),
                Message::Pong(1)
            );
            pipe.send(Message::Prepare {
                id: 1,
                inputs: (mode == Mode::Video).then_some(INPUTS),
                edge: Some(123),
            })
            .await
            .unwrap();
            assert_eq!(
                pipe.messages.recv().await.unwrap().unwrap(),
                Message::Ready(1)
            );
            if mode == Mode::Video {
                pipe.send(Message::SelectMonitor(1)).await.unwrap();
                assert_eq!(
                    pipe.messages.recv().await.unwrap().unwrap(),
                    Message::MonitorReady(1)
                );
                assert_eq!(monitor.input.load(SeqCst), INPUTS.peer);
            }
            pipe.send(Message::Activate(1)).await.unwrap();
            assert_eq!(
                pipe.messages.recv().await.unwrap().unwrap(),
                Message::Active(1)
            );
            pipe.send(Message::Event(KEY)).await.unwrap();
            pipe.send(Message::Ping(2)).await.unwrap();
            assert_eq!(
                pipe.messages.recv().await.unwrap().unwrap(),
                Message::Pong(2)
            );
            if restore_on_active_port {
                pipe.send(Message::Return(1)).await.unwrap();
                assert_eq!(
                    pipe.messages.recv().await.unwrap().unwrap(),
                    Message::Returning(1)
                );
                // The receiver releases the key before selecting the host's input.
                assert_eq!(monitor.input.load(SeqCst), INPUTS.peer);
                pipe.send(Message::RestoreMonitor(1)).await.unwrap();
                assert_eq!(
                    pipe.messages.recv().await.unwrap().unwrap(),
                    Message::MonitorRestored(1)
                );
                assert_eq!(monitor.input.load(SeqCst), INPUTS.host);
                pipe.send(Message::Returned(1)).await.unwrap();
                pipe.send(Message::Ping(3)).await.unwrap();
                assert_eq!(
                    pipe.messages.recv().await.unwrap().unwrap(),
                    Message::Pong(3)
                );
            }
        };
        let server = async {
            let mut pipe = Pipe::connect(
                listener.accept().await.unwrap().0,
                &ubuntu,
                &windows.public,
                false,
            )
            .await
            .unwrap();
            let mut sink = SyntheticInput::default();
            let mut transfer = None;
            assert!(
                session(&mut pipe, &mut sink, Some(&worker), &mut transfer, &mut 0)
                    .await
                    .is_err()
            );
            assert_eq!(sink.held, !restore_on_active_port);
            disconnect(&mut sink, transfer, Some(&worker))
                .await
                .unwrap();
            assert!(!sink.active && !sink.held);
            assert_eq!(sink.releases, 1);
        };
        tokio::time::timeout(Duration::from_secs(4), async {
            tokio::join!(client, server);
        })
        .await
        .unwrap();
        assert_eq!(monitor.input.load(SeqCst), INPUTS.host);
        if mode == Mode::Input {
            assert_eq!(monitor.calls.load(SeqCst), 0);
        } else {
            assert_eq!(
                monitor.calls.load(SeqCst),
                2,
                "one selection and one return/recovery; no readback"
            );
        }
    }

    #[tokio::test]
    async fn broken_link_releases_held_input_and_recovers_shared_video() {
        fixture(Mode::Video, false).await;
    }

    #[tokio::test]
    async fn broken_input_only_link_releases_input_without_touching_ddc() {
        fixture(Mode::Input, false).await;
    }

    #[tokio::test]
    async fn return_releases_held_input_before_the_active_port_fallback() {
        fixture(Mode::Video, true).await;
    }
}
