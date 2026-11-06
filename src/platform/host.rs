use crate::{
    config::{Edge, Identity, Key},
    controller::{Controller, Effect, Transfer},
    gate::{Gate, Route},
    transport::{LEASE, PING_PERIOD, Pipe},
    video::{Inputs, Job, SETTLE, VideoResult, VideoWorker},
    wire::{Input, Message, Mode},
};
use anyhow::{Context, Result, bail, ensure};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot},
    time::{Instant, MissedTickBehavior, interval, sleep, timeout},
};

const SWITCH_TIMEOUT: Duration = Duration::from_secs(15);

pub enum Control {
    Toggle(u64),
    Edge { token: u64, position: u16 },
}

pub trait Cursor {
    async fn request_toggle(&self) -> Result<()>;
    async fn center(&self) -> Result<()>;
    async fn enter(&self, edge: &Edge, position: u16) -> Result<()>;
    async fn resume(&self) -> Result<()> {
        Ok(())
    }
}

pub struct Coordinator<C> {
    pub gate: Arc<Gate>,
    pub edge: Option<Edge>,
    pub video: Option<(Inputs, VideoWorker)>,
    pub controls: mpsc::Receiver<Control>,
    pub events: mpsc::Receiver<(u64, Input)>,
    pub cursor: C,
}

pub async fn connect<C: Cursor>(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    mut io: Coordinator<C>,
) -> Result<()> {
    let shutdown = crate::lifecycle::shutdown();
    tokio::pin!(shutdown);
    loop {
        let connection = tokio::select! {
            result = &mut shutdown => { result?; break; }
            result = async {
                let stream = timeout(Duration::from_secs(2), TcpStream::connect(address)).await??;
                Pipe::connect(stream, &identity, &peer, true).await
            } => result,
        };
        match connection {
            Ok(mut pipe) => {
                println!("peer authenticated.");
                crate::lifecycle::report(crate::lifecycle::Event::Input(
                    crate::lifecycle::InputStatus::Here,
                ));
                let mut handoff = Handoff::new(io.video.as_ref().map(|(inputs, _)| *inputs));
                let result = tokio::select! {
                    result = &mut shutdown => { result?; None }
                    result = handoff.run(&mut pipe, &mut io) => Some(result),
                };
                io.gate.emergency();
                crate::lifecycle::report(crate::lifecycle::Event::Input(
                    crate::lifecycle::InputStatus::Recovering,
                ));
                drop(pipe);
                // A timeout may leave a Win32 DDC call in flight. Never race a new transfer
                // with the old job or its recovery. The hook remains local while we wait.
                let recover = handoff.logic.abort();
                drop(handoff);
                if let Some(Err(error)) = &result {
                    eprintln!("host input restored: {error:#}");
                }
                if recover && let Some((inputs, worker)) = &io.video {
                    sleep(SETTLE).await;
                    if let Err(error) = worker.recover(*inputs).await {
                        eprintln!("Host-side video recovery could not be verified: {error:#}");
                    }
                }
                if result.is_none() {
                    break;
                }
            }
            Err(error) => {
                io.gate.emergency();
                crate::lifecycle::report(crate::lifecycle::Event::Input(
                    crate::lifecycle::InputStatus::Waiting,
                ));
                eprintln!("Waiting for peer: {error:#}");
            }
        }
        tokio::select! {
            result = &mut shutdown => { result?; break; }
            _ = sleep(Duration::from_secs(1)) => {}
        }
    }
    Ok(())
}

struct Handoff {
    logic: Controller,
    token: Option<u64>,
    deadline: Option<Instant>,
    id: u64,
    job: Option<(Job, oneshot::Receiver<Result<VideoResult>>)>,
    next_job: Option<(Instant, Job)>,
}

impl Handoff {
    fn new(inputs: Option<Inputs>) -> Self {
        Self {
            logic: Controller::new(inputs),
            token: None,
            deadline: None,
            id: 0,
            job: None,
            next_job: None,
        }
    }

    fn start(&mut self, token: u64, mode: Mode, edge: Option<u16>) -> Result<Effect> {
        self.token = Some(token);
        self.deadline = Some(Instant::now() + SWITCH_TIMEOUT);
        self.id += 1;
        self.logic.start(Transfer {
            id: self.id,
            mode,
            edge,
        })
    }

    fn returning(&mut self, exit: Option<u16>, hotkey: bool) -> Result<Effect> {
        self.deadline = Some(Instant::now() + SWITCH_TIMEOUT);
        self.logic.return_home(exit, hotkey)
    }

    async fn run(&mut self, pipe: &mut Pipe, io: &mut Coordinator<impl Cursor>) -> Result<()> {
        let mut tick = interval(Duration::from_millis(20));
        let mut ping = interval(PING_PERIOD);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        ping.set_missed_tick_behavior(MissedTickBehavior::Skip);
        crate::lifecycle::report(crate::lifecycle::Event::Input(
            crate::lifecycle::InputStatus::Here,
        ));
        let mut commands = crate::lifecycle::commands();
        let mut last_ping = 0;
        let mut last_pong = 0;
        let mut lease = Instant::now() + LEASE;
        loop {
            ensure!(Instant::now() < lease, "peer heartbeat timed out");
            ensure!(
                self.deadline.is_none_or(|time| Instant::now() < time),
                "switch confirmation timed out"
            );
            if let Some(token) = self.token {
                ensure!(
                    io.gate.current().0 == token,
                    "emergency key, input overflow or local lease expired"
                );
            }
            let effect = tokio::select! {
                command = commands.recv() => {
                    match command {
                        Ok(crate::lifecycle::Command::Switch) => io.cursor.request_toggle().await?,
                        Ok(crate::lifecycle::Command::Emergency) => io.gate.emergency(),
                        Err(_) => {}
                    }
                    continue;
                }
                _ = ping.tick() => {
                    last_ping += 1;
                    pipe.send(Message::Ping(last_ping)).await?;
                    continue;
                }
                _ = tick.tick() => {
                    let Some(effect) = self.poll_job(io)? else { continue };
                    effect
                }
                control = io.controls.recv() => {
                    let control = control.context("host hook stopped")?;
                    match control {
                        Control::Toggle(token) if io.gate.current() == (token, Route::Hold) => {
                            if self.logic.is_local() {
                                self.start(token, if io.video.is_some() {Mode::Video} else {Mode::Input}, None)?
                            } else if self.logic.is_remote() {
                                self.returning(None, true)?
                            } else { continue; }
                        }
                        Control::Edge { token, position } if io.gate.current() == (token, Route::Hold) && self.logic.is_local() => {
                            let mode = io.edge.as_ref().context("host edge is disabled")?.mode;
                            self.start(token, mode, Some(position))?
                        }
                        _ => continue,
                    }
                }
                message = pipe.messages.recv() => {
                    let message = message.context("peer disconnected")??;
                    match message {
                        Message::SwitchRequested => {
                            io.cursor.request_toggle().await?;
                            continue;
                        }
                        Message::Pong(nonce) => {
                            ensure!(nonce > last_pong && nonce <= last_ping, "invalid heartbeat reply");
                            last_pong = nonce;
                            lease = Instant::now() + LEASE;
                            if let Some(token) = self.token {
                                io.gate.renew(token);
                            }
                            continue;
                        }
                        Message::ExitEdge { id, position } if self.logic.is_remote() && self.logic.id() == Some(id) => {
                            let token = self.token.context("missing host route token")?;
                            // A simultaneous F12 has already put the hook in Hold and queued a return.
                            if !io.gate.hold(token) { continue; }
                            self.returning(Some(position), false)?
                        }
                        Message::ExitEdge { id, .. } if self.logic.id() == Some(id) => continue,
                        _ => self.logic.message(message)?,
                    }
                }
                event = io.events.recv() => {
                    let (token, event) = event.context("host input hook stopped")?;
                    if self.logic.is_remote() && self.token == Some(token) && io.gate.current() == (token, Route::Remote) {
                        pipe.send(Message::Event(event)).await?;
                    }
                    continue;
                }
            };
            self.apply(effect, pipe, io).await?;
        }
    }

    fn poll_job(&mut self, io: &Coordinator<impl Cursor>) -> Result<Option<Effect>> {
        if let Some((when, job)) = self.next_job
            && Instant::now() >= when
        {
            self.next_job = None;
            self.submit(io, job)?;
        }
        let Some((job, pending)) = &mut self.job else {
            return Ok(None);
        };
        let result = match pending.try_recv() {
            Ok(result) => result,
            Err(oneshot::error::TryRecvError::Empty) => return Ok(None),
            Err(oneshot::error::TryRecvError::Closed) => bail!("host DDC worker stopped"),
        };
        let job = *job;
        self.job = None;
        match job {
            Job::Select { .. } => {
                result.context("host could not select the peer monitor input")?;
                Ok(Some(self.logic.written()?))
            }
            Job::Read => {
                let observed = match result {
                    Ok(VideoResult::Read(value)) => Some(value),
                    Err(error) => {
                        eprintln!("Waiting for Host-side DDC read: {error:#}");
                        None
                    }
                    _ => bail!("invalid host DDC response"),
                };
                Ok(Some(self.logic.checked(observed)?))
            }
            Job::Recover(_) => bail!("unexpected recovery job"),
        }
    }

    fn submit(&mut self, io: &Coordinator<impl Cursor>, job: Job) -> Result<()> {
        let (_, worker) = io
            .video
            .as_ref()
            .context("configure the shared monitor before switching video")?;
        self.job = Some((job, worker.submit(job)?));
        Ok(())
    }

    async fn apply(
        &mut self,
        effect: Effect,
        pipe: &mut Pipe,
        io: &Coordinator<impl Cursor>,
    ) -> Result<()> {
        let token = self.token.context("missing host route token")?;
        ensure!(io.gate.current().0 == token, "host route was revoked");
        match effect {
            Effect::Send(message) => {
                pipe.send(message).await?;
                if matches!(message, Message::Returned(_)) {
                    let Effect::Local { position, upgrade } = self.logic.finished() else {
                        unreachable!()
                    };
                    if upgrade {
                        let Effect::Send(prepare) = self.start(token, Mode::Video, None)? else {
                            unreachable!()
                        };
                        pipe.send(prepare).await?;
                    } else {
                        if let (Some(position), Some(edge)) = (position, &io.edge) {
                            io.cursor.enter(edge, position).await?;
                        } else {
                            io.cursor.resume().await?;
                        }
                        io.gate.local(token);
                        self.token = None;
                        self.deadline = None;
                        crate::lifecycle::report(crate::lifecycle::Event::Input(
                            crate::lifecycle::InputStatus::Here,
                        ));
                        println!("Input returned to host.");
                    }
                }
            }
            Effect::Ddc(Job::Read) => {
                self.next_job = Some((Instant::now() + SETTLE, Job::Read));
            }
            Effect::Ddc(job) => self.submit(io, job)?,
            Effect::Forward => {
                io.cursor.center().await?;
                ensure!(
                    io.gate.forward(token),
                    "host route was revoked before activation"
                );
                self.deadline = None;
                crate::lifecycle::report(crate::lifecycle::Event::Input(
                    crate::lifecycle::InputStatus::Peer,
                ));
                println!("Input active on peer.");
            }
            Effect::Local { .. } => bail!("unexpected local routing effect"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::NOISE, video::Monitor};
    use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering::SeqCst};
    use tokio::net::TcpListener;

    struct FixtureCursor {
        gate: Arc<Gate>,
        controls: mpsc::Sender<Control>,
        toggles: Arc<AtomicUsize>,
    }

    impl Cursor for FixtureCursor {
        async fn request_toggle(&self) -> Result<()> {
            self.toggles.fetch_add(1, SeqCst);
            let (token, route) = self.gate.current();
            let token = match route {
                Route::Local => self.gate.begin(),
                Route::Remote if self.gate.hold(token) => Some(token),
                _ => None,
            };
            if let Some(token) = token {
                self.controls.send(Control::Toggle(token)).await?;
            }
            Ok(())
        }
        async fn center(&self) -> Result<()> {
            Ok(())
        }
        async fn enter(&self, _: &Edge, _: u16) -> Result<()> {
            Ok(())
        }
    }

    struct FixtureMonitor(Arc<AtomicU16>);
    impl Monitor for FixtureMonitor {
        fn read(&mut self) -> Result<u16> {
            Ok(self.0.load(SeqCst))
        }
        fn write(&mut self, input: u16) -> Result<()> {
            self.0.store(input, SeqCst);
            Ok(())
        }
    }

    fn identity() -> Identity {
        let pair = snow::Builder::new(NOISE.parse().unwrap())
            .generate_keypair()
            .unwrap();
        Identity {
            private: pair.private.try_into().unwrap(),
            public: pair.public.try_into().unwrap(),
        }
    }

    async fn next(pipe: &mut Pipe) -> Message {
        loop {
            let message = pipe.messages.recv().await.unwrap().unwrap();
            if let Message::Ping(nonce) = message {
                pipe.send(Message::Pong(nonce)).await.unwrap();
            } else {
                return message;
            }
        }
    }

    #[tokio::test]
    async fn authenticated_tray_request_switches_video_and_input_in_both_directions() {
        let host = identity();
        let peer = identity();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let gate = Arc::new(Gate::new());
        let input = Arc::new(AtomicU16::new(17));
        let toggles = Arc::new(AtomicUsize::new(0));
        let (control_send, controls) = mpsc::channel(8);
        let (_event_send, events) = mpsc::channel(8);
        let inputs = Inputs { host: 17, peer: 15 };
        let mut io = Coordinator {
            gate: Arc::clone(&gate),
            edge: None,
            video: Some((
                inputs,
                VideoWorker::start(FixtureMonitor(Arc::clone(&input))).unwrap(),
            )),
            controls,
            events,
            cursor: FixtureCursor {
                gate: Arc::clone(&gate),
                controls: control_send,
                toggles: Arc::clone(&toggles),
            },
        };
        let source = async {
            let stream = TcpStream::connect(address).await.unwrap();
            let mut pipe = Pipe::connect(stream, &host, &peer.public, true)
                .await
                .unwrap();
            assert!(
                Handoff::new(Some(inputs))
                    .run(&mut pipe, &mut io)
                    .await
                    .is_err()
            );
        };
        let destination = async {
            let mut pipe = Pipe::connect(
                listener.accept().await.unwrap().0,
                &peer,
                &host.public,
                false,
            )
            .await
            .unwrap();
            pipe.send(Message::SwitchRequested).await.unwrap();
            assert_eq!(
                next(&mut pipe).await,
                Message::Prepare {
                    id: 1,
                    inputs: Some(inputs),
                    edge: None
                }
            );
            assert_eq!(input.load(SeqCst), inputs.host);
            pipe.send(Message::Ready(1)).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::Verify(1));
            assert_eq!(input.load(SeqCst), inputs.peer);
            assert!(gate.current().1 == Route::Hold);
            pipe.send(Message::Observed {
                id: 1,
                input: inputs.peer,
            })
            .await
            .unwrap();
            assert_eq!(next(&mut pipe).await, Message::Activate(1));
            pipe.send(Message::Active(1)).await.unwrap();
            while gate.current().1 != Route::Remote {
                sleep(Duration::from_millis(1)).await;
            }
            pipe.send(Message::SwitchRequested).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::Return(1));
            input.store(inputs.host, SeqCst);
            pipe.send(Message::Returning(1)).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::Returned(1));
            while gate.current().1 != Route::Local {
                sleep(Duration::from_millis(1)).await;
            }
        };
        timeout(Duration::from_secs(4), async {
            tokio::join!(source, destination);
        })
        .await
        .unwrap();
        assert_eq!(input.load(SeqCst), inputs.host);
        assert!(gate.current().1 == Route::Local);
        assert_eq!(toggles.load(SeqCst), 2);
    }
}
