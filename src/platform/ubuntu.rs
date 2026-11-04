use super::portal::Portal;
use crate::{
    config::{Identity, Key},
    transport::{LEASE, Pipe},
    video::{Inputs, Job, SETTLE, VideoResult, VideoWorker},
    wire::{Input, Message, Mode},
};
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use std::{net::SocketAddr, time::Duration};
use tokio::{
    net::TcpListener,
    sync::oneshot,
    time::{Instant, MissedTickBehavior, interval},
};

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Armed,
    Reading,
    Verified,
    Active,
    Exiting,
    Returning,
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

trait Sink {
    async fn poll(&mut self) -> Result<()>;
    fn prepare(&self, edge: Option<u16>, mode: Mode) -> Result<()>;
    fn activate(&mut self, edge: Option<u16>, mode: Mode) -> Result<()>;
    fn event(&mut self, event: Input) -> Result<Option<u16>>;
    fn release(&mut self) -> Result<()>;
}

impl Sink for Portal {
    async fn poll(&mut self) -> Result<()> {
        let event = self
            .stream
            .next()
            .await
            .context("EIS device stream closed")??;
        self.handle(event)
    }

    fn prepare(&self, edge: Option<u16>, mode: Mode) -> Result<()> {
        self.can_start(edge, mode, mode == Mode::Input)
    }

    fn activate(&mut self, edge: Option<u16>, mode: Mode) -> Result<()> {
        self.start(edge, mode)
    }

    fn event(&mut self, event: Input) -> Result<Option<u16>> {
        self.event(event)
    }

    fn release(&mut self) -> Result<()> {
        self.release()
    }
}

async fn disconnect(
    sink: &mut impl Sink,
    transfer: Option<Transfer>,
    video: Option<&(Inputs, VideoWorker)>,
) -> Result<()> {
    let released = sink.release();
    if transfer.is_some_and(|active| active.mode == Mode::Video)
        && let Some((inputs, worker)) = video
    {
        eprintln!("Attempting to restore the shared monitor to Windows.");
        if let Err(error) = worker.recover(*inputs).await {
            eprintln!("Automatic video recovery could not be verified: {error:#}");
        }
    }
    released
}

pub async fn run(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    mut portal: Portal,
    video: Option<(Inputs, VideoWorker)>,
) -> Result<()> {
    let listener = TcpListener::bind(address)
        .await
        .context("binding Ubuntu listen address")?;
    println!("Listening for the pinned Windows peer on {address}.");
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    loop {
        let (stream, _) = tokio::select! {
            result = &mut shutdown => { result?; break; }
            event = portal.poll() => {
                if let Err(error) = event {
                    portal.close().await;
                    return Err(error);
                }
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
        println!("Windows peer authenticated.");
        let mut transfer = None;
        let mut last_id = 0;
        let result = tokio::select! {
            result = &mut shutdown => {
                result?;
                None
            }
            result = session(&mut pipe, &mut portal, video.as_ref(), &mut transfer, &mut last_id) => Some(result),
        };
        drop(pipe);
        if let Some(Err(error)) = &result {
            eprintln!("Session ended: {error:#}");
        }
        if let Err(error) = disconnect(&mut portal, transfer, video.as_ref()).await {
            portal.close().await;
            return Err(error.context("Wayland input was closed after a release error"));
        }
        if result.is_none() {
            break;
        }
    }
    portal.close().await;
    Ok(())
}

async fn session(
    pipe: &mut Pipe,
    sink: &mut impl Sink,
    video: Option<&(Inputs, VideoWorker)>,
    transfer: &mut Option<Transfer>,
    last_id: &mut u64,
) -> Result<()> {
    let mut lease = Instant::now() + LEASE;
    let mut next_read = Instant::now();
    let mut job: Option<oneshot::Receiver<Result<VideoResult>>> = None;
    let mut tick = interval(Duration::from_millis(25));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        let now = Instant::now();
        ensure!(now < lease, "Windows heartbeat timed out");
        ensure!(
            transfer
                .as_ref()
                .is_none_or(|active| active.stage == Stage::Active || now < active.deadline),
            "Windows did not complete the switch before the deadline"
        );
        tokio::select! {
            _ = tick.tick() => {
                let Some(active) = transfer.as_mut() else { continue };
                if let Some(pending) = &mut job {
                    match pending.try_recv() {
                        Ok(result) => {
                            job = None;
                            match active.stage {
                                Stage::Reading => {
                                    let (inputs, _) = video.unwrap();
                                    if matches!(result, Ok(VideoResult::Read(input)) if input == inputs.ubuntu) {
                                        active.stage = Stage::Verified;
                                        pipe.send(Message::Observed { id: active.id, input: inputs.ubuntu }).await?;
                                    } else {
                                        if let Err(error) = result {
                                            eprintln!("Waiting for Ubuntu-side DDC read: {error:#}");
                                        }
                                        next_read = Instant::now() + Duration::from_millis(300);
                                    }
                                }
                                Stage::Returning => {
                                    result.context("Ubuntu could not select the Windows monitor input")?;
                                    active.stage = Stage::Returned;
                                    pipe.send(Message::Returning(active.id)).await?;
                                }
                                _ => bail!("unexpected DDC job result"),
                            }
                        }
                        Err(oneshot::error::TryRecvError::Empty) => {}
                        Err(oneshot::error::TryRecvError::Closed) => bail!("Ubuntu DDC worker stopped"),
                    }
                }
                if active.stage == Stage::Reading && job.is_none() && Instant::now() >= next_read {
                    job = Some(video.unwrap().1.submit(Job::Read)?);
                }
            }
            result = sink.poll() => result?,
            message = pipe.messages.recv() => {
                let message = message.context("Windows disconnected")??;
                if let Message::Ping(nonce) = message {
                    lease = Instant::now() + LEASE;
                    pipe.send(Message::Pong(nonce)).await?;
                    continue;
                }
                if let Message::Prepare { id, inputs, edge } = message {
                    let mode = if inputs.is_some() { Mode::Video } else { Mode::Input };
                    ensure!(id > *last_id && transfer.is_none(), "stale or overlapping transfer");
                    *last_id = id;
                    if let Err(error) = sink.prepare(edge, mode) {
                        eprintln!("Transfer rejected: {error:#}");
                        pipe.send(Message::Rejected(id)).await?;
                        continue;
                    }
                    if inputs.is_some() && inputs != video.map(|(local, _)| *local) {
                        eprintln!("Transfer rejected: the configured Windows/Ubuntu input values differ between peers.");
                        pipe.send(Message::Rejected(id)).await?;
                        continue;
                    }
                    *transfer = Some(Transfer { id, mode, edge, stage: Stage::Armed, deadline: Instant::now() + SWITCH_TIMEOUT });
                    pipe.send(Message::Ready(id)).await?;
                    continue;
                }
                let active = transfer.as_mut().context("transfer message without a transfer")?;
                match message {
                    Message::Verify(id) if active.id == id && active.mode == Mode::Video && active.stage == Stage::Armed => {
                        active.stage = Stage::Reading;
                        next_read = Instant::now() + SETTLE;
                    }
                    Message::Activate(id) if active.id == id && (
                        active.stage == Stage::Verified || (active.stage == Stage::Armed && active.mode == Mode::Input)
                    ) => {
                        sink.activate(active.edge, active.mode)?;
                        active.stage = Stage::Active;
                        pipe.send(Message::Active(id)).await?;
                        println!("Input active on Ubuntu ({:?}).", active.mode);
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
                        if active.mode == Mode::Video {
                            let (inputs, worker) = video.unwrap();
                            active.stage = Stage::Returning;
                            job = Some(worker.submit(Job::Select { from: inputs.ubuntu, to: inputs.windows })?);
                        } else {
                            active.stage = Stage::Returned;
                            pipe.send(Message::Returning(id)).await?;
                        }
                    }
                    Message::Returned(id) if active.id == id && active.stage == Stage::Returned => {
                        *transfer = None;
                        println!("Input returned to Windows.");
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

    const INPUTS: Inputs = Inputs {
        windows: 17,
        ubuntu: 15,
    };
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

    async fn fixture(mode: Mode) {
        let windows = identity();
        let ubuntu = identity();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let monitor = Arc::new(FixtureMonitor::default());
        monitor.input.store(INPUTS.windows, SeqCst);
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
                // The fixture supplies the outcome of the source-side DDC write.
                monitor.input.store(INPUTS.ubuntu, SeqCst);
                pipe.send(Message::Verify(1)).await.unwrap();
                assert_eq!(
                    pipe.messages.recv().await.unwrap().unwrap(),
                    Message::Observed {
                        id: 1,
                        input: INPUTS.ubuntu
                    }
                );
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
            assert!(sink.held);
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
        assert_eq!(monitor.input.load(SeqCst), INPUTS.windows);
        if mode == Mode::Input {
            assert_eq!(monitor.calls.load(SeqCst), 0);
        } else {
            assert!(monitor.calls.load(SeqCst) >= 3);
        }
    }

    #[tokio::test]
    async fn broken_link_releases_held_input_and_recovers_shared_video() {
        fixture(Mode::Video).await;
    }

    #[tokio::test]
    async fn broken_input_only_link_releases_input_without_touching_ddc() {
        fixture(Mode::Input).await;
    }
}
