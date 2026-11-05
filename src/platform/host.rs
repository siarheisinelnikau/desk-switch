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
    let shutdown = tokio::signal::ctrl_c();
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
                println!("peer peer authenticated.");
                let mut handoff = Handoff::new(io.video.as_ref().map(|(inputs, _)| *inputs));
                let result = tokio::select! {
                    result = &mut shutdown => { result?; None }
                    result = handoff.run(&mut pipe, &mut io) => Some(result),
                };
                io.gate.emergency();
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
                                self.start(token, Mode::Video, None)?
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
                println!("Input active on peer.");
            }
            Effect::Local { .. } => bail!("unexpected local routing effect"),
        }
        Ok(())
    }
}
