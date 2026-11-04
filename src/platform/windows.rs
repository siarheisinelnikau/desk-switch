use super::win_input::{self, Control, Hooks};
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
use windows_sys::Win32::{System::Threading::GetCurrentThreadId, UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT}};

const SWITCH_TIMEOUT: Duration = Duration::from_secs(15);

pub fn run(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    edge: Option<Edge>,
    video: Option<(Inputs, VideoWorker)>,
) -> Result<()> {
    let gate = Arc::new(Gate::new());
    let (control_tx, controls) = mpsc::channel(8);
    let (input_tx, events) = mpsc::channel(256);
    let hooks = Hooks::install(Arc::clone(&gate), edge.clone(), control_tx, input_tx)?;
    let hook_thread = unsafe { GetCurrentThreadId() };
    let runtime = crate::runtime()?;
    let thread = std::thread::Builder::new().name("Desk Switch coordinator".into()).spawn(move || {
        let result = runtime.block_on(connect(address, identity, peer, Coordinator {
            gate, edge, video, controls, events,
        }));
        unsafe { PostThreadMessageW(hook_thread, WM_QUIT, 0, 0); }
        result
    })?;
    println!("F12 switches the shared monitor and input. F11 immediately restores Windows input.");
    hooks.run()?;
    thread.join().map_err(|_| anyhow::anyhow!("Windows coordinator stopped unexpectedly"))?
}

struct Coordinator {
    gate: Arc<Gate>,
    edge: Option<Edge>,
    video: Option<(Inputs, VideoWorker)>,
    controls: mpsc::Receiver<Control>,
    events: mpsc::Receiver<(u64, Input)>,
}

async fn connect(address: SocketAddr, identity: Identity, peer: Key, mut io: Coordinator) -> Result<()> {
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
                println!("Ubuntu peer authenticated.");
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
                    eprintln!("Windows input restored: {error:#}");
                }
                if recover && let Some((inputs, worker)) = &io.video {
                    sleep(SETTLE).await;
                    if let Err(error) = worker.recover(*inputs).await {
                        eprintln!("Windows-side video recovery could not be verified: {error:#}");
                    }
                }
                if result.is_none() { break; }
            }
            Err(error) => {
                io.gate.emergency();
                eprintln!("Waiting for Ubuntu: {error:#}");
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
            logic: Controller::new(inputs), token: None, deadline: None, id: 0, job: None, next_job: None,
        }
    }

    fn start(&mut self, token: u64, mode: Mode, edge: Option<u16>) -> Result<Effect> {
        self.token = Some(token);
        self.deadline = Some(Instant::now() + SWITCH_TIMEOUT);
        self.id += 1;
        self.logic.start(Transfer { id: self.id, mode, edge })
    }

    fn returning(&mut self, exit: Option<u16>, hotkey: bool) -> Result<Effect> {
        self.deadline = Some(Instant::now() + SWITCH_TIMEOUT);
        self.logic.return_home(exit, hotkey)
    }

    async fn run(&mut self, pipe: &mut Pipe, io: &mut Coordinator) -> Result<()> {
        let mut tick = interval(Duration::from_millis(20));
        let mut ping = interval(PING_PERIOD);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        ping.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_ping = 0;
        let mut last_pong = 0;
        let mut lease = Instant::now() + LEASE;
        loop {
            ensure!(Instant::now() < lease, "Ubuntu heartbeat timed out");
            ensure!(self.deadline.is_none_or(|time| Instant::now() < time), "switch confirmation timed out");
            if let Some(token) = self.token {
                ensure!(io.gate.current().0 == token, "emergency key, input overflow or local lease expired");
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
                    let control = control.context("Windows hook stopped")?;
                    match control {
                        Control::Toggle(token) if io.gate.current() == (token, Route::Hold) => {
                            if self.logic.is_local() {
                                self.start(token, Mode::Video, None)?
                            } else if self.logic.is_remote() {
                                self.returning(None, true)?
                            } else { continue; }
                        }
                        Control::Edge { token, position } if io.gate.current() == (token, Route::Hold) && self.logic.is_local() => {
                            let mode = io.edge.as_ref().context("Windows edge is disabled")?.mode;
                            self.start(token, mode, Some(position))?
                        }
                        _ => continue,
                    }
                }
                message = pipe.messages.recv() => {
                    let message = message.context("Ubuntu disconnected")??;
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
                            let token = self.token.context("missing Windows route token")?;
                            // A simultaneous F12 has already put the hook in Hold and queued a return.
                            if !io.gate.hold(token) { continue; }
                            self.returning(Some(position), false)?
                        }
                        Message::ExitEdge { id, .. } if self.logic.id() == Some(id) => continue,
                        _ => self.logic.message(message)?,
                    }
                }
                event = io.events.recv() => {
                    let (token, event) = event.context("Windows input hook stopped")?;
                    if self.logic.is_remote() && self.token == Some(token) && io.gate.current() == (token, Route::Remote) {
                        pipe.send(Message::Event(event)).await?;
                    }
                    continue;
                }
            };
            self.apply(effect, pipe, io).await?;
        }
    }

    fn poll_job(&mut self, io: &Coordinator) -> Result<Option<Effect>> {
        if let Some((when, job)) = self.next_job && Instant::now() >= when {
            self.next_job = None;
            self.submit(io, job)?;
        }
        let Some((job, pending)) = &mut self.job else { return Ok(None) };
        let result = match pending.try_recv() {
            Ok(result) => result,
            Err(oneshot::error::TryRecvError::Empty) => return Ok(None),
            Err(oneshot::error::TryRecvError::Closed) => bail!("Windows DDC worker stopped"),
        };
        let job = *job;
        self.job = None;
        match job {
            Job::Select { .. } => {
                result.context("Windows could not select the Ubuntu monitor input")?;
                Ok(Some(self.logic.written()?))
            }
            Job::Read => {
                let observed = match result {
                    Ok(VideoResult::Read(value)) => Some(value),
                    Err(error) => {
                        eprintln!("Waiting for Windows-side DDC read: {error:#}");
                        None
                    }
                    _ => bail!("invalid Windows DDC response"),
                };
                Ok(Some(self.logic.checked(observed)?))
            }
            Job::Recover(_) => bail!("unexpected recovery job"),
        }
    }

    fn submit(&mut self, io: &Coordinator, job: Job) -> Result<()> {
        let (_, worker) = io.video.as_ref().context("configure the shared monitor before switching video")?;
        self.job = Some((job, worker.submit(job)?));
        Ok(())
    }

    async fn apply(&mut self, effect: Effect, pipe: &mut Pipe, io: &Coordinator) -> Result<()> {
        let token = self.token.context("missing Windows route token")?;
        ensure!(io.gate.current().0 == token, "Windows route was revoked");
        match effect {
            Effect::Send(message) => {
                pipe.send(message).await?;
                if matches!(message, Message::Returned(_)) {
                    let Effect::Local { position, upgrade } = self.logic.finished() else { unreachable!() };
                    if upgrade {
                        let Effect::Send(prepare) = self.start(token, Mode::Video, None)? else { unreachable!() };
                        pipe.send(prepare).await?;
                    } else {
                        if let (Some(position), Some(edge)) = (position, &io.edge)
                            && let Err(error) = win_input::enter(edge, position)
                        {
                            eprintln!("Windows cursor placement failed: {error:#}");
                        }
                        io.gate.local(token);
                        self.token = None;
                        self.deadline = None;
                        println!("Input returned to Windows.");
                    }
                }
            }
            Effect::Ddc(Job::Read) => {
                self.next_job = Some((Instant::now() + SETTLE, Job::Read));
            }
            Effect::Ddc(job) => self.submit(io, job)?,
            Effect::Forward => {
                win_input::center()?;
                ensure!(io.gate.forward(token), "Windows route was revoked before activation");
                self.deadline = None;
                println!("Input active on Ubuntu.");
            }
            Effect::Local { .. } => bail!("unexpected local routing effect"),
        }
        Ok(())
    }
}
