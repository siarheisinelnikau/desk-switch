use crate::{
    config::{Edge, Identity, Key},
    controller::{Controller, Effect, Transfer},
    edge::{Crossing, Entry},
    gate::{Gate, Route},
    lifecycle::RecoveryReason,
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

fn current_routes() -> (u64, u64) {
    let routes = crate::lifecycle::routes();
    let routes = routes.borrow();
    (routes.activation, routes.revision)
}

pub enum Control {
    Toggle(u64),
    Edge { token: u64, position: u16 },
    Boundary { token: u64, crossing: Crossing },
}

pub trait Cursor {
    async fn request_toggle(&self) -> Result<()>;
    async fn center(&self) -> Result<()>;
    async fn enter(&self, edge: &Edge, position: u16) -> Result<()>;
    async fn enter_display(&self, _activation: u64, _entry: Entry) -> Result<()> {
        bail!("Display entry is unavailable on this platform")
    }
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
    pub monitor_required: fn() -> bool,
}

pub async fn connect<C: Cursor>(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    io: Coordinator<C>,
) -> Result<()> {
    let gate = io.gate.clone();
    let result = connect_mode(address, identity, peer, io, crate::lifecycle::group_input()).await;
    gate.restore_local();
    result
}

async fn connect_mode<C: Cursor>(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    mut io: Coordinator<C>,
    group: bool,
) -> Result<()> {
    let shutdown = crate::lifecycle::shutdown();
    tokio::pin!(shutdown);
    loop {
        let connection = async {
            let stream = timeout(Duration::from_secs(2), TcpStream::connect(address)).await??;
            Pipe::connect(stream, &identity, &peer, true).await
        };
        tokio::pin!(connection);
        let connection = loop {
            tokio::select! {
                result = &mut shutdown => { result?; return Ok(()); }
                control = io.controls.recv(), if group => {
                    waiting_group_control(control.context("host hook stopped")?, &io.gate);
                }
                _ = io.gate.manual_emergency() => report_recovery(RecoveryReason::EmergencyKey),
                result = &mut connection => break result,
            }
        };
        match connection {
            Ok(mut pipe) => {
                println!("peer authenticated.");
                let mut handoff = Handoff::new(io.video.as_ref().map(|(inputs, _)| *inputs));
                handoff.group = group;
                let result = tokio::select! {
                    result = &mut shutdown => { result?; None }
                    result = handoff.run(&mut pipe, &mut io) => Some(result),
                };
                if let Some(Err(error)) = &result {
                    let reason = if io.gate.take_manual_emergency() {
                        RecoveryReason::EmergencyKey
                    } else {
                        crate::lifecycle::recovery_reason(error)
                    };
                    report_recovery(reason);
                }
                io.gate.restore_local();
                crate::lifecycle::report(crate::lifecycle::Event::Input(
                    crate::lifecycle::InputStatus::Recovering,
                ));
                drop(pipe);
                if handoff.logic.id().is_some()
                    && let Some(Err(error)) = &result
                {
                    crate::lifecycle::report(crate::lifecycle::Event::Switch {
                        error: Some(format!("{error:#}")),
                    });
                }
                // The active receiver alone may restore video after a lost link.
                // Two independent recovery writers race and can retrain the display twice.
                handoff.logic.abort();
                drop(handoff);
                if let Some(Err(error)) = &result {
                    eprintln!("host input restored: {error:#}");
                }
                if result.is_none() {
                    break;
                }
            }
            Err(error) => {
                // No remote route exists before authentication. A group hotkey
                // may already be queued with Hold, so leave that token for the
                // waiting-control handler instead of invalidating its cycle.
                if !group {
                    io.gate.restore_local();
                }
                crate::lifecycle::report(crate::lifecycle::Event::Input(
                    crate::lifecycle::InputStatus::Waiting,
                ));
                eprintln!("Waiting for peer: {error:#}");
            }
        }
        let backoff = sleep(Duration::from_secs(1));
        tokio::pin!(backoff);
        loop {
            tokio::select! {
                result = &mut shutdown => { result?; return Ok(()); }
                control = io.controls.recv(), if group => {
                    waiting_group_control(control.context("host hook stopped")?, &io.gate);
                }
                _ = io.gate.manual_emergency() => report_recovery(RecoveryReason::EmergencyKey),
                _ = &mut backoff => break,
            }
        }
    }
    Ok(())
}

fn report_recovery(reason: RecoveryReason) {
    crate::lifecycle::report(crate::lifecycle::Event::InputRecovery { reason });
}

fn waiting_group_control(control: Control, gate: &Gate) {
    if let Control::Boundary { token, crossing } = control {
        if gate.current() == (token, Route::Local) {
            crate::lifecycle::report(crate::lifecycle::Event::EdgeCrossed(crossing));
        }
        return;
    }
    let (token, event) = match control {
        Control::Toggle(token) => (token, crate::lifecycle::Event::CycleInput),
        Control::Edge { token, position } => {
            (token, crate::lifecycle::Event::CycleInputEdge(position))
        }
        Control::Boundary { .. } => unreachable!(),
    };
    if gate.current() == (token, Route::Hold) {
        gate.local(token);
        crate::lifecycle::report(event);
    }
}

pub(super) fn display_entry_result(activation: u64, result: Result<()>) {
    let error = result.err().map(|error| {
        let mut text = format!("{error:#}");
        text.retain(|character| !character.is_control());
        text.truncate(text.floor_char_boundary(1024));
        text
    });
    crate::lifecycle::report(crate::lifecycle::Event::DisplayEntry { activation, error });
}

struct Handoff {
    group: bool,
    group_requested: bool,
    group_entry: Option<u16>,
    display_switch: Option<crate::lifecycle::DisplaySwitch>,
    group_generation: u64,
    positioning: Option<(u64, u64)>,
    return_requested: bool,
    canceling: bool,
    return_activation: Option<u64>,
    logic: Controller,
    token: Option<u64>,
    deadline: Option<Instant>,
    id: u64,
    job: Option<(Job, oneshot::Receiver<Result<VideoResult>>)>,
    next_send: Option<(Instant, Message)>,
    allow_setup: bool,
    pending_event: Option<(u64, crate::wire::Input)>,
}

struct Heartbeat {
    sent: u64,
    received: u64,
    lease: Instant,
    last_reply: Instant,
    pending: std::collections::VecDeque<Message>,
}
impl Heartbeat {
    fn new() -> Self {
        Self {
            sent: 0,
            received: 0,
            lease: Instant::now() + LEASE,
            last_reply: Instant::now(),
            pending: Default::default(),
        }
    }
    fn check(&self) -> Result<()> {
        if Instant::now() >= self.lease {
            return Err(
                anyhow::Error::new(RecoveryReason::HeartbeatTimeout).context(format!(
                    "peer heartbeat timed out (last reply {} ms ago)",
                    self.last_reply.elapsed().as_millis()
                )),
            );
        }
        Ok(())
    }
    async fn ping(&mut self, pipe: &mut Pipe) -> Result<()> {
        self.sent += 1;
        pipe.send(Message::Ping(self.sent)).await
    }
    fn pong(&mut self, nonce: u64, gate: &Gate, token: Option<u64>) -> Result<()> {
        ensure!(
            nonce > self.received && nonce <= self.sent,
            "invalid heartbeat reply"
        );
        self.received = nonce;
        self.last_reply = Instant::now();
        self.lease = self.last_reply + LEASE;
        if let Some(token) = token {
            gate.renew(token);
        }
        Ok(())
    }
    async fn cursor(
        &mut self,
        operation: impl std::future::Future<Output = Result<()>>,
        pipe: &mut Pipe,
        gate: &Gate,
        token: u64,
    ) -> Result<()> {
        let mut ping = interval(PING_PERIOD);
        ping.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let deadline = tokio::time::sleep(SWITCH_TIMEOUT);
        tokio::pin!(operation, deadline);
        loop {
            self.check()?;
            tokio::select! {
                biased;
                result = &mut operation => return result,
                _ = gate.manual_emergency() => return Err(RecoveryReason::EmergencyKey.into()),
                _ = &mut deadline => bail!("Windows/Wayland cursor operation timed out while the peer remained connected"),
                _ = ping.tick() => self.ping(pipe).await?,
                message = pipe.messages.recv() => match message.context("peer disconnected during cursor positioning")?? {
                    Message::Pong(nonce) => self.pong(nonce, gate, Some(token))?,
                    message => {
                        ensure!(self.pending.len() < 16, "too many control messages during cursor positioning");
                        self.pending.push_back(message);
                    }
                }
            }
        }
    }
}

async fn next_message(
    pending: &mut std::collections::VecDeque<Message>,
    messages: &mut mpsc::Receiver<Result<Message>>,
) -> Option<Result<Message>> {
    match pending.pop_front() {
        Some(message) => Some(Ok(message)),
        None => messages.recv().await,
    }
}

impl Handoff {
    fn new(inputs: Option<Inputs>) -> Self {
        Self {
            group: crate::lifecycle::group_input(),
            group_requested: false,
            group_entry: None,
            display_switch: None,
            group_generation: 0,
            positioning: None,
            return_requested: false,
            canceling: false,
            return_activation: None,
            logic: Controller::new(inputs),
            token: None,
            deadline: None,
            id: 0,
            job: None,
            next_send: None,
            allow_setup: false,
            pending_event: None,
        }
    }

    fn start(&mut self, token: u64, mode: Mode, edge: Option<u16>) -> Result<Effect> {
        crate::lifecycle::report(crate::lifecycle::Event::Switch { error: None });
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

    fn request_group_return(&mut self, gate: &Gate) -> Result<Option<Effect>> {
        if !self.group || self.logic.is_local() {
            return Ok(None);
        }
        self.return_requested = true;
        if self.logic.is_remote() {
            let token = self.token.context("missing active input token")?;
            if gate.hold(token) {
                return self.returning(None, false).map(Some);
            }
        }
        Ok(None)
    }

    async fn run(&mut self, pipe: &mut Pipe, io: &mut Coordinator<impl Cursor>) -> Result<()> {
        let mut tick = interval(Duration::from_millis(20));
        let mut ping = interval(PING_PERIOD);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        ping.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut commands = crate::lifecycle::commands();
        crate::lifecycle::input_authenticated(crate::lifecycle::InputStatus::Here);
        let mut heartbeat = Heartbeat::new();
        loop {
            if io.gate.take_manual_emergency() {
                report_recovery(RecoveryReason::EmergencyKey);
            }
            heartbeat.check()?;
            ensure!(
                self.deadline.is_none_or(|time| Instant::now() < time),
                "switch confirmation timed out"
            );
            if let Some(token) = self.token
                && io.gate.current().0 != token
            {
                return Err(
                    anyhow::Error::new(io.gate.recovery_reason()).context(format!(
                        "host input route was revoked: {}",
                        io.gate.reason()
                    )),
                );
            }
            let effect = tokio::select! {
                _ = io.gate.manual_emergency() => {
                    if self.logic.id().is_some() { return Err(RecoveryReason::EmergencyKey.into()); }
                    report_recovery(RecoveryReason::EmergencyKey);
                    continue;
                }
                command = commands.recv() => {
                    match command {
                        Ok(crate::lifecycle::Command::Switch) => { self.group_requested = true; self.group_entry = None; self.display_switch = None; io.cursor.request_toggle().await?; }
                        Ok(crate::lifecycle::Command::SwitchEdge(position)) if self.group && io.edge.is_some() => {
                            self.group_requested = true;
                            self.group_entry = Some(position);
                            self.display_switch = None;
                            io.cursor.request_toggle().await?;
                        }
                        Ok(crate::lifecycle::Command::EnterEdge(position)) if self.group && self.logic.is_local() => {
                            if let Some(edge) = &io.edge { io.cursor.enter(edge, position).await?; }
                        }
                        Ok(crate::lifecycle::Command::SwitchEdge(_) | crate::lifecycle::Command::EnterEdge(_)) => {},
                        Ok(crate::lifecycle::Command::SwitchDisplay(request)) if self.group => {
                            if request.activation == 0 || request.activation != current_routes().0 {
                                display_entry_result(request.activation, Err(anyhow::anyhow!("The display activation changed before input could switch")));
                                continue;
                            }
                            if !self.logic.is_local() {
                                display_entry_result(request.activation, Err(anyhow::anyhow!("Input is still switching; try the display again")));
                                continue;
                            }
                            self.group_requested = true;
                            self.group_entry = None;
                            self.group_generation = request.activation;
                            self.display_switch = Some(request);
                            io.cursor.request_toggle().await?;
                        }
                        Ok(crate::lifecycle::Command::EnterDisplay(request)) if self.group => {
                            if request.activation == 0 || request.activation != current_routes().0 || request.activation < self.group_generation {
                                display_entry_result(request.activation, Err(anyhow::anyhow!("The display activation changed before the cursor could enter")));
                            } else if self.logic.is_local() {
                                self.group_generation = request.activation;
                                let token = io.gate.current().0;
                                let result = heartbeat.cursor(current_display_operation(request.activation, io.cursor.enter_display(request.activation, request.entry)), pipe, &io.gate, token).await;
                                display_entry_result(request.activation, result);
                            } else if self.logic.is_remote() {
                                let id = self.logic.id().context("missing active transfer")?;
                                self.group_generation = request.activation;
                                self.positioning = Some((id, request.activation));
                                pipe.send(Message::Reposition { id, generation: request.activation, entry: request.entry }).await?;
                            } else {
                                display_entry_result(request.activation, Err(anyhow::anyhow!("Input is still switching; try the display again")));
                            }
                        }
                        Ok(crate::lifecycle::Command::RoutesChanged) if self.group => {
                            self.group_generation = self.group_generation.max(current_routes().0);
                        }
                        Ok(crate::lifecycle::Command::SwitchDisplay(_) | crate::lifecycle::Command::EnterDisplay(_) | crate::lifecycle::Command::RoutesChanged) => {},
                        Ok(crate::lifecycle::Command::SetupSwitch) => { self.allow_setup = true; io.cursor.request_toggle().await?; }
                        Ok(crate::lifecycle::Command::Emergency) => io.gate.emergency(),
                        Ok(crate::lifecycle::Command::ReturnInput(activation)) => {
                            if !self.group || activation != current_routes().0 {
                                crate::lifecycle::report(crate::lifecycle::Event::InputReturned { activation, completed: false });
                            } else if self.logic.is_local() {
                                crate::lifecycle::report(crate::lifecycle::Event::Input(crate::lifecycle::InputStatus::Here));
                                crate::lifecycle::report(crate::lifecycle::Event::InputReturned { activation, completed: true });
                            } else {
                                self.return_activation = Some(activation);
                                if let Some(effect) = self.request_group_return(&io.gate)? {
                                    self.apply(effect, pipe, io, &mut heartbeat).await?;
                                }
                            }
                        }
                        Ok(crate::lifecycle::Command::MicrophoneVolume(_)) | Err(_) => {}
                    }
                    continue;
                }
                _ = ping.tick() => {
                    heartbeat.ping(pipe).await?;
                    continue;
                }
                _ = tick.tick() => {
                    let Some(effect) = self.poll_job()? else { continue };
                    effect
                }
                control = io.controls.recv() => {
                    let control = control.context("host hook stopped")?;
                    match control {
                        Control::Toggle(token) if io.gate.current() == (token, Route::Hold) => {
                            if self.group && !std::mem::take(&mut self.group_requested) {
                                crate::lifecycle::report(crate::lifecycle::Event::CycleInput);
                                if self.logic.is_remote() {
                                    self.returning(None, true)?
                                } else {
                                    io.gate.local(token);
                                    continue;
                                }
                            } else {
                                if let Some(request) = self.display_switch
                                    && request.activation != current_routes().0
                                {
                                    io.gate.local(token);
                                    self.display_switch = None;
                                    display_entry_result(request.activation, Err(anyhow::anyhow!("The display activation changed before input could switch")));
                                    continue;
                                }
                                let entry = self.group_entry.take();
                                let setup = std::mem::take(&mut self.allow_setup);
                                if self.logic.is_local() {
                                    if io.video.is_none() && (io.monitor_required)() && !setup {
                                        io.gate.local(token);
                                        crate::lifecycle::report(crate::lifecycle::Event::Switch { error: Some("Remember the video connection on both PCs before switching the shared monitor.".into()) });
                                        continue;
                                    }
                                    self.start(token, if self.display_switch.is_some() || io.video.is_none() {Mode::Input} else {Mode::Video}, entry)?
                                } else if self.logic.is_remote() {
                                    self.returning(None, true)?
                                } else { continue; }
                            }
                        }
                        Control::Edge { token, position } if io.gate.current() == (token, Route::Hold) && self.logic.is_local() => {
                            if self.group {
                                io.gate.local(token);
                                crate::lifecycle::report(crate::lifecycle::Event::CycleInputEdge(position));
                                continue;
                            }
                            if io.video.is_none() && (io.monitor_required)() {
                                io.gate.local(token);
                                continue;
                            }
                            let mode = io.edge.as_ref().context("host edge is disabled")?.mode;
                            self.start(token, mode, Some(position))?
                        }
                        Control::Boundary { token, crossing } if self.group && self.logic.is_local() && io.gate.current() == (token, Route::Local) => {
                            crate::lifecycle::report(crate::lifecycle::Event::EdgeCrossed(crossing));
                            continue;
                        }
                        _ => continue,
                    }
                }
                message = next_message(&mut heartbeat.pending, &mut pipe.messages) => {
                    let message = message.context("peer disconnected")??;
                    match message {
                        Message::SwitchRequested => {
                            io.cursor.request_toggle().await?;
                            continue;
                        }
                        Message::SetupSwitchRequested => {
                            self.allow_setup = true;
                            io.cursor.request_toggle().await?;
                            continue;
                        }
                        Message::MonitorRestored(_) => {
                            if let Some((inputs, _)) = &io.video { crate::lifecycle::report(crate::lifecycle::Event::Monitor(Some(inputs.host))); }
                            self.logic.message(message)?
                        }
                        Message::Pong(nonce) => {
                            heartbeat.pong(nonce, &io.gate, self.token)?;
                            continue;
                        }
                        Message::Crossed { id, crossing } if self.group => {
                            if self.logic.is_remote() && self.logic.id() == Some(id)
                                && crossing.activation == self.group_generation
                                && crossing.activation == current_routes().0
                                && crossing.revision == current_routes().1
                                && crossing.boundary != 0
                            {
                                crate::lifecycle::report(crate::lifecycle::Event::EdgeCrossed(crossing));
                            }
                            continue;
                        }
                        Message::Positioned { id, generation } if self.group => {
                            if self.positioning == Some((id, generation)) {
                                self.positioning = None;
                                display_entry_result(generation, Ok(()));
                            }
                            continue;
                        }
                        Message::RepositionRejected { id, generation } if self.group => {
                            if self.positioning == Some((id, generation)) {
                                self.positioning = None;
                                display_entry_result(generation, Err(anyhow::anyhow!("The destination display changed or cursor entry is unavailable; input remains active")));
                            }
                            continue;
                        }
                        Message::Returning(id) if self.canceling && self.logic.id() == Some(id) => {
                            pipe.send(Message::Returned(id)).await?;
                            self.logic.abort();
                            self.finish_local(pipe, io, &mut heartbeat, None).await?;
                            continue;
                        }
                        Message::Rejected(id) if self.return_requested && self.logic.id() == Some(id) => {
                            // An obsolete preparation can be rejected after the
                            // controller has already returned focus to this PC.
                            self.logic.abort();
                            self.finish_local(pipe, io, &mut heartbeat, None).await?;
                            continue;
                        }
                        Message::ExitEdge { id, position } if self.logic.is_remote() && self.logic.id() == Some(id) => {
                            let token = self.token.context("missing host route token")?;
                            // A simultaneous F12 has already put the hook in Hold and queued a return.
                            if !io.gate.hold(token) { continue; }
                            if self.group { crate::lifecycle::report(crate::lifecycle::Event::CycleInputEdge(position)); }
                            self.returning(Some(position), false)?
                        }
                        Message::ExitEdge { id, .. } if self.logic.id() == Some(id) => continue,
                        _ => self.logic.message(message)?,
                    }
                }
                event = next_event(&mut self.pending_event, &mut io.events) => {
                    let (token, event) = event.context("host input hook stopped")?;
                    if self.logic.is_remote() && self.token == Some(token) && io.gate.current() == (token, Route::Remote) {
                        let event = coalesce_motion(token, event, &mut io.events, &mut self.pending_event);
                        pipe.send(Message::Event(event)).await?;
                    }
                    continue;
                }
            };
            self.apply(effect, pipe, io, &mut heartbeat).await?;
        }
    }

    fn poll_job(&mut self) -> Result<Option<Effect>> {
        if let Some((when, message)) = self.next_send
            && Instant::now() >= when
        {
            self.next_send = None;
            return Ok(Some(Effect::Send(message)));
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
                match result {
                    Ok(VideoResult::Written) => {
                        let effect = self.logic.written()?;
                        if let Effect::Send(message @ Message::Returned(_)) = effect {
                            self.next_send = Some((Instant::now() + SETTLE, message));
                            Ok(None)
                        } else {
                            Ok(Some(effect))
                        }
                    }
                    Err(error) if !crate::video::write_attempted(&error) => {
                        self.logic.write_failed().map(Some).with_context(|| {
                            format!("monitor selection could not be issued: {error:#}")
                        })
                    }
                    Err(error) => Err(error
                        .context("Monitor input command failed after issuance; no retry was sent")),
                    _ => bail!("invalid monitor selection response"),
                }
            }
            Job::Read => bail!("input switching must not issue DDC readback"),
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
        heartbeat: &mut Heartbeat,
    ) -> Result<()> {
        let token = self.token.context("missing host route token")?;
        ensure!(io.gate.current().0 == token, "host route was revoked");
        match effect {
            Effect::Send(message) => {
                let message = if self.return_requested && self.display_switch.is_some() {
                    if let Message::Activate(id) = message {
                        // The receiver is only armed. Release it without ever
                        // activating input or starting a new native session.
                        self.canceling = true;
                        Message::Return(id)
                    } else {
                        message
                    }
                } else {
                    message
                };
                let message = match (message, self.display_switch) {
                    (
                        Message::Prepare {
                            id, inputs: None, ..
                        },
                        Some(request),
                    ) => Message::PrepareDisplay {
                        id,
                        generation: request.activation,
                        entry: request.entry,
                    },
                    (message, _) => message,
                };
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
                        self.finish_local(pipe, io, heartbeat, position).await?;
                    }
                }
            }
            Effect::Ddc(job) => self.submit(io, job)?,
            Effect::Forward => {
                if self.return_requested {
                    let Effect::Send(message) = self.returning(None, false)? else {
                        unreachable!()
                    };
                    pipe.send(message).await?;
                    return Ok(());
                }
                ensure!(
                    self.display_switch
                        .is_none_or(|request| request.activation == current_routes().0),
                    "The display activation changed before input became active"
                );
                heartbeat
                    .cursor(io.cursor.center(), pipe, &io.gate, token)
                    .await?;
                ensure!(
                    io.gate.forward(token),
                    "host route was revoked before activation"
                );
                self.deadline = None;
                if self.display_switch.is_some() {
                    display_entry_result(self.group_generation, Ok(()));
                }
                crate::lifecycle::report(crate::lifecycle::Event::Input(
                    crate::lifecycle::InputStatus::Peer,
                ));
                println!("Input active on peer.");
            }
            Effect::Local { .. } => bail!("unexpected local routing effect"),
        }
        Ok(())
    }

    async fn finish_local(
        &mut self,
        pipe: &mut Pipe,
        io: &Coordinator<impl Cursor>,
        heartbeat: &mut Heartbeat,
        position: Option<u16>,
    ) -> Result<()> {
        let token = self.token.context("missing input return token")?;
        if let (Some(position), Some(edge)) = (position, &io.edge) {
            heartbeat
                .cursor(io.cursor.enter(edge, position), pipe, &io.gate, token)
                .await?;
        } else {
            heartbeat
                .cursor(io.cursor.resume(), pipe, &io.gate, token)
                .await?;
        }
        io.gate.local(token);
        self.token = None;
        self.deadline = None;
        self.display_switch = None;
        self.return_requested = false;
        self.canceling = false;
        self.positioning = None;
        crate::lifecycle::report(crate::lifecycle::Event::Input(
            crate::lifecycle::InputStatus::Here,
        ));
        if let Some(activation) = self.return_activation.take() {
            crate::lifecycle::report(crate::lifecycle::Event::InputReturned {
                activation,
                completed: true,
            });
        }
        println!("Input returned to host.");
        Ok(())
    }
}

async fn next_event(
    pending: &mut Option<(u64, crate::wire::Input)>,
    events: &mut mpsc::Receiver<(u64, crate::wire::Input)>,
) -> Option<(u64, crate::wire::Input)> {
    match pending.take() {
        Some(event) => Some(event),
        None => events.recv().await,
    }
}

async fn current_display_operation(
    activation: u64,
    operation: impl std::future::Future<Output = Result<()>>,
) -> Result<()> {
    current_display_operation_with(activation, operation, crate::lifecycle::routes()).await
}

async fn current_display_operation_with(
    activation: u64,
    operation: impl std::future::Future<Output = Result<()>>,
    mut routes: tokio::sync::watch::Receiver<crate::edge::Routes>,
) -> Result<()> {
    tokio::pin!(operation);
    loop {
        ensure!(
            routes.borrow().activation == activation,
            "The display activation changed before the cursor could enter"
        );
        tokio::select! {
            biased;
            changed = routes.changed() => { changed.context("display routes channel closed")?; }
            result = &mut operation => return result,
        }
    }
}

fn coalesce_motion(
    token: u64,
    event: crate::wire::Input,
    events: &mut mpsc::Receiver<(u64, crate::wire::Input)>,
    pending: &mut Option<(u64, crate::wire::Input)>,
) -> crate::wire::Input {
    use crate::wire::Input;
    let Input::Move { mut dx, mut dy } = event else {
        return event;
    };
    // Do not wait for more input or cross a key/button boundary. Only collapse
    // motion already queued, preserving distance and discrete-event order.
    for _ in 0..63 {
        let Ok(next) = events.try_recv() else {
            break;
        };
        if let (next_token, Input::Move { dx: x, dy: y }) = next
            && next_token == token
            && let (Some(x), Some(y)) = (dx.checked_add(x), dy.checked_add(y))
            && (Input::Move { dx: x, dy: y }).valid()
        {
            dx = x;
            dy = y;
        } else {
            *pending = Some(next);
            break;
        }
    }
    Input::Move { dx, dy }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::NOISE, video::Monitor};
    use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering::SeqCst};
    use tokio::net::TcpListener;

    #[test]
    fn expired_peer_heartbeat_has_a_typed_transient_reason() {
        let mut heartbeat = Heartbeat::new();
        heartbeat.lease = Instant::now() - Duration::from_millis(1);
        let error = heartbeat.check().unwrap_err();
        assert_eq!(
            crate::lifecycle::recovery_reason(&error),
            RecoveryReason::HeartbeatTimeout
        );
        assert!(format!("{error:#}").contains("last reply"));
    }

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
    async fn group_hotkeys_and_edge_fail_open_while_peer_authentication_is_stalled() {
        let host = identity();
        let peer = identity();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let gate = Arc::new(Gate::new());
        let (send, controls) = mpsc::channel(8);
        let (_event_send, events) = mpsc::channel(8);
        let io = Coordinator {
            gate: gate.clone(),
            edge: None,
            video: None,
            controls,
            events,
            cursor: FixtureCursor {
                gate: gate.clone(),
                controls: send.clone(),
                toggles: Arc::new(AtomicUsize::new(0)),
            },
            monitor_required: || false,
        };
        let source = connect_mode(address, host, peer.public, io, true);
        let destination = async {
            let (_stalled_connection, _) = listener.accept().await.unwrap();
            for edge in [false, true] {
                let token = gate.begin().unwrap();
                send.send(if edge {
                    Control::Edge {
                        token,
                        position: 21_845,
                    }
                } else {
                    Control::Toggle(token)
                })
                .await
                .unwrap();
                timeout(Duration::from_millis(250), async {
                    while gate.current().1 != Route::Local {
                        sleep(Duration::from_millis(1)).await;
                    }
                })
                .await
                .expect(
                    "group control waited for the heartbeat lease before restoring local input",
                );
                assert!(
                    !gate.forward(token),
                    "a late auth completion could reuse the released token"
                );
            }
        };
        timeout(Duration::from_secs(2),async {
            tokio::select! { result=source=>panic!("authentication unexpectedly finished: {result:?}"),_=destination=>{} }
        }).await.unwrap();
    }

    #[tokio::test]
    async fn group_cycle_returns_active_input_without_breaking_the_authenticated_session() {
        let host = identity();
        let peer = identity();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let gate = Arc::new(Gate::new());
        let toggles = Arc::new(AtomicUsize::new(0));
        let (send, controls) = mpsc::channel(8);
        let (_event_send, events) = mpsc::channel(8);
        let mut io = Coordinator {
            gate: gate.clone(),
            edge: Some(Edge {
                mode: Mode::Input,
                screen: "auto".into(),
                side: crate::edge::Side::Right,
            }),
            video: None,
            controls,
            events,
            cursor: FixtureCursor {
                gate: gate.clone(),
                controls: send,
                toggles: toggles.clone(),
            },
            monitor_required: || false,
        };
        let source = async {
            let mut pipe = Pipe::connect(
                TcpStream::connect(address).await.unwrap(),
                &host,
                &peer.public,
                true,
            )
            .await
            .unwrap();
            let mut handoff = Handoff::new(None);
            handoff.group = true;
            // Model one explicit group activation arriving from the GUI. Later
            // F12 requests are forwarded to the group and return this link home.
            handoff.group_requested = true;
            handoff.group_entry = Some(21_845);
            assert!(handoff.run(&mut pipe, &mut io).await.is_err());
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
                    inputs: None,
                    edge: Some(21_845)
                }
            );
            pipe.send(Message::Ready(1)).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::Activate(1));
            pipe.send(Message::Active(1)).await.unwrap();
            while gate.current().1 != Route::Remote {
                sleep(Duration::from_millis(1)).await;
            }
            pipe.send(Message::SwitchRequested).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::Return(1));
            pipe.send(Message::Returning(1)).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::Returned(1));
            while gate.current().1 != Route::Local {
                sleep(Duration::from_millis(1)).await;
            }
            // No reconnect or second switch is necessary while the group picks
            // its next destination; the existing heartbeat continues normally.
            match pipe.messages.recv().await.unwrap().unwrap() {
                Message::Ping(nonce) => pipe.send(Message::Pong(nonce)).await.unwrap(),
                message => panic!("unexpected command after group return: {message:?}"),
            }
        };
        timeout(Duration::from_secs(3), async {
            tokio::join!(source, destination);
        })
        .await
        .unwrap();
        assert_eq!(toggles.load(SeqCst), 2);
        assert!(gate.current().1 == Route::Local);
    }

    #[tokio::test]
    async fn group_hotkey_is_actionable_during_reconnect_backoff() {
        let host = identity();
        let host_public = host.public;
        let peer = identity();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let gate = Arc::new(Gate::new());
        let (send, controls) = mpsc::channel(8);
        let (_event_send, events) = mpsc::channel(8);
        let io = Coordinator {
            gate: gate.clone(),
            edge: None,
            video: None,
            controls,
            events,
            cursor: FixtureCursor {
                gate: gate.clone(),
                controls: send.clone(),
                toggles: Arc::new(AtomicUsize::new(0)),
            },
            monitor_required: || false,
        };
        let source = connect_mode(address, host, peer.public, io, true);
        let destination = async {
            let pipe = Pipe::connect(
                listener.accept().await.unwrap().0,
                &peer,
                &host_public,
                false,
            )
            .await
            .unwrap();
            // Observe cleanup through a pending route, rather than treating
            // ordinary network loss as if the user had pressed F11.
            let disconnected = gate.begin().unwrap();
            drop(pipe);
            timeout(Duration::from_secs(1), async {
                while gate.current().0 == disconnected || gate.current().1 != Route::Local {
                    sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("disconnect cleanup did not restore the pending route");
            assert!(
                !gate.take_manual_emergency(),
                "network cleanup manufactured a manual F11 request"
            );
            let token = gate.begin().unwrap();
            send.send(Control::Toggle(token)).await.unwrap();
            timeout(Duration::from_millis(250), async {
                while gate.current().1 != Route::Local {
                    sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("F12 was held for the reconnect backoff");
            assert!(!gate.forward(token));
        };
        timeout(Duration::from_secs(3),async {
            tokio::select!{result=source=>panic!("host reconnect loop exited: {result:?}"),_=destination=>{}}
        }).await.unwrap();
    }

    #[test]
    fn queued_group_control_cannot_revoke_a_newer_route() {
        let gate = Gate::new();
        let stale = gate.begin().unwrap();
        gate.emergency();
        let current = gate.begin().unwrap();
        waiting_group_control(Control::Toggle(stale), &gate);
        assert!(gate.current() == (current, Route::Hold));
        waiting_group_control(
            Control::Edge {
                token: current,
                position: 2_048,
            },
            &gate,
        );
        assert!(gate.current().1 == Route::Local);
        assert!(!gate.forward(current));
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
            monitor_required: || false,
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
            assert_eq!(next(&mut pipe).await, Message::MonitorSelected(1));
            assert_eq!(input.load(SeqCst), inputs.peer);
            assert!(gate.current().1 == Route::Hold);
            pipe.send(Message::MonitorReady(1)).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::Activate(1));
            pipe.send(Message::Active(1)).await.unwrap();
            while gate.current().1 != Route::Remote {
                sleep(Duration::from_millis(1)).await;
            }
            pipe.send(Message::SwitchRequested).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::Return(1));
            pipe.send(Message::Returning(1)).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::RestoreMonitor(1));
            input.store(inputs.host, SeqCst);
            pipe.send(Message::MonitorRestored(1)).await.unwrap();
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

    #[tokio::test]
    async fn incomplete_video_blocks_normal_switch_but_explicit_setup_can_lend_and_return_input() {
        let host = identity();
        let peer = identity();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let gate = Arc::new(Gate::new());
        let toggles = Arc::new(AtomicUsize::new(0));
        let (control_send, controls) = mpsc::channel(8);
        let (_keep_events, events) = mpsc::channel(8);
        let mut io = Coordinator {
            monitor_required: || true,
            gate: gate.clone(),
            edge: None,
            video: None,
            controls,
            events,
            cursor: FixtureCursor {
                gate: gate.clone(),
                controls: control_send,
                toggles: toggles.clone(),
            },
        };
        let source = async {
            let stream = TcpStream::connect(address).await.unwrap();
            let mut pipe = Pipe::connect(stream, &host, &peer.public, true)
                .await
                .unwrap();
            assert!(Handoff::new(None).run(&mut pipe, &mut io).await.is_err());
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
            loop {
                let message = pipe.messages.recv().await.unwrap().unwrap();
                let Message::Ping(nonce) = message else {
                    panic!("unconfigured video silently lent input: {message:?}")
                };
                pipe.send(Message::Pong(nonce)).await.unwrap();
                if toggles.load(SeqCst) == 1 && gate.current().1 == Route::Local {
                    break;
                }
            }
            pipe.send(Message::SetupSwitchRequested).await.unwrap();
            assert_eq!(
                next(&mut pipe).await,
                Message::Prepare {
                    id: 1,
                    inputs: None,
                    edge: None
                }
            );
            pipe.send(Message::Ready(1)).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::Activate(1));
            pipe.send(Message::Active(1)).await.unwrap();
            while gate.current().1 != Route::Remote {
                sleep(Duration::from_millis(1)).await;
            }
            pipe.send(Message::SwitchRequested).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::Return(1));
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
        assert!(gate.current().1 == Route::Local);
        assert_eq!(toggles.load(SeqCst), 3);
    }

    #[test]
    fn queued_motion_preserves_distance_button_order_and_session_boundaries() {
        let (send, mut events) = mpsc::channel(128);
        let mut pending = None;
        for _ in 0..50 {
            send.try_send((7, Input::Move { dx: 3, dy: -2 })).unwrap();
        }
        let boundary = Input::Button {
            code: 272,
            down: true,
        };
        send.try_send((7, boundary)).unwrap();
        send.try_send((8, Input::Move { dx: 2, dy: 2 })).unwrap();
        assert_eq!(
            coalesce_motion(7, Input::Move { dx: 1, dy: 1 }, &mut events, &mut pending),
            Input::Move { dx: 151, dy: -99 }
        );
        assert_eq!(pending.take(), Some((7, boundary)));
        assert_eq!(
            events.try_recv().unwrap(),
            (8, Input::Move { dx: 2, dy: 2 })
        );
        send.try_send((7, Input::Move { dx: 16000, dy: 0 }))
            .unwrap();
        assert_eq!(
            coalesce_motion(
                7,
                Input::Move { dx: 1000, dy: 0 },
                &mut events,
                &mut pending
            ),
            Input::Move { dx: 1000, dy: 0 }
        );
        assert_eq!(pending, Some((7, Input::Move { dx: 16000, dy: 0 })));
    }
    #[tokio::test]
    async fn slow_native_cursor_operation_keeps_the_existing_input_lease_alive() {
        let a = identity();
        let b = identity();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (a_stream, b_stream) = timeout(Duration::from_secs(2), async {
            tokio::join!(
                TcpStream::connect(listener.local_addr().unwrap()),
                listener.accept()
            )
        })
        .await
        .expect("localhost fixture did not connect");
        let (a_pipe, b_pipe) = tokio::join!(
            Pipe::connect(a_stream.unwrap(), &a, &b.public, true),
            Pipe::connect(b_stream.unwrap().0, &b, &a.public, false)
        );
        let mut local = a_pipe.unwrap();
        let mut remote = b_pipe.unwrap();
        let gate = Gate::new();
        let token = gate.begin().unwrap();
        let (_route_send, route_recv) = tokio::sync::watch::channel(crate::edge::Routes {
            activation: 7,
            ..Default::default()
        });
        let source = async {
            let mut heartbeat = Heartbeat::new();
            heartbeat
                .cursor(
                    current_display_operation_with(
                        7,
                        async {
                            sleep(LEASE + Duration::from_millis(400)).await;
                            Ok(())
                        },
                        route_recv,
                    ),
                    &mut local,
                    &gate,
                    token,
                )
                .await
                .unwrap();
            assert!(heartbeat.received >= 5);
            assert!(gate.current() == (token, Route::Hold));
            assert!(gate.forward(token));
        };
        let peer = async {
            while let Some(Ok(message)) = remote.messages.recv().await {
                if let Message::Ping(nonce) = message {
                    remote.send(Message::Pong(nonce)).await.unwrap();
                }
            }
        };
        tokio::select! { _ = source => {}, _ = peer => panic!("peer closed early") }
    }

    #[tokio::test]
    async fn a_new_activation_cancels_a_slow_entry_before_it_can_warp() {
        let (send, receive) = tokio::sync::watch::channel(crate::edge::Routes {
            activation: 7,
            ..Default::default()
        });
        let entered = AtomicUsize::new(0);
        let operation = current_display_operation_with(
            7,
            async {
                sleep(Duration::from_millis(100)).await;
                entered.fetch_add(1, SeqCst);
                Ok(())
            },
            receive,
        );
        let change = async {
            sleep(Duration::from_millis(10)).await;
            send.send_replace(crate::edge::Routes {
                activation: 8,
                ..Default::default()
            });
        };
        let (result, ()) = tokio::join!(operation, change);
        assert!(result.is_err());
        assert_eq!(entered.load(SeqCst), 0);
    }

    #[tokio::test]
    async fn group_return_cancels_armed_input_then_reuses_the_same_authenticated_connection() {
        let host = identity();
        let peer = identity();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let gate = Arc::new(Gate::new());
        let (controls, control_recv) = mpsc::channel(8);
        let (_keep_events, events) = mpsc::channel(8);
        let io = Coordinator {
            gate: gate.clone(),
            edge: None,
            video: None,
            controls: control_recv,
            events,
            cursor: FixtureCursor {
                gate: gate.clone(),
                controls,
                toggles: Arc::new(AtomicUsize::new(0)),
            },
            monitor_required: || false,
        };
        let source = async {
            let mut pipe = Pipe::connect(
                TcpStream::connect(address).await.unwrap(),
                &host,
                &peer.public,
                true,
            )
            .await
            .unwrap();
            let mut heartbeat = Heartbeat::new();
            let mut handoff = Handoff::new(None);
            handoff.group = true;
            handoff.display_switch = Some(crate::lifecycle::DisplaySwitch {
                activation: 4,
                entry: None,
            });
            let token = gate.begin().unwrap();
            let prepare = handoff.start(token, Mode::Input, None).unwrap();
            handoff
                .apply(prepare, &mut pipe, &io, &mut heartbeat)
                .await
                .unwrap();
            assert!(handoff.request_group_return(&gate).unwrap().is_none());
            let ready = handoff.logic.message(next(&mut pipe).await).unwrap();
            handoff
                .apply(ready, &mut pipe, &io, &mut heartbeat)
                .await
                .unwrap();
            assert!(handoff.canceling);
            assert_eq!(next(&mut pipe).await, Message::Returning(1));
            pipe.send(Message::Returned(1)).await.unwrap();
            handoff.logic.abort();
            handoff
                .finish_local(&mut pipe, &io, &mut heartbeat, None)
                .await
                .unwrap();
            assert!(gate.current().1 == Route::Local && handoff.logic.is_local());
            // Start and gracefully return another transfer on the very same
            // encrypted socket. Neither return revokes the native gate.
            let token = gate.begin().unwrap();
            let prepare = handoff.start(token, Mode::Input, None).unwrap();
            handoff
                .apply(prepare, &mut pipe, &io, &mut heartbeat)
                .await
                .unwrap();
            for _ in 0..2 {
                let effect = handoff.logic.message(next(&mut pipe).await).unwrap();
                handoff
                    .apply(effect, &mut pipe, &io, &mut heartbeat)
                    .await
                    .unwrap();
            }
            assert!(gate.current() == (token, Route::Remote));
            let returned = handoff.request_group_return(&gate).unwrap().unwrap();
            handoff
                .apply(returned, &mut pipe, &io, &mut heartbeat)
                .await
                .unwrap();
            let returned = handoff.logic.message(next(&mut pipe).await).unwrap();
            handoff
                .apply(returned, &mut pipe, &io, &mut heartbeat)
                .await
                .unwrap();
            assert!(gate.current().1 == Route::Local && handoff.logic.is_local());
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
            assert_eq!(
                next(&mut pipe).await,
                Message::PrepareDisplay {
                    id: 1,
                    generation: 4,
                    entry: None
                }
            );
            pipe.send(Message::Ready(1)).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::Return(1));
            pipe.send(Message::Returning(1)).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::Returned(1));
            assert_eq!(
                next(&mut pipe).await,
                Message::Prepare {
                    id: 2,
                    inputs: None,
                    edge: None
                }
            );
            pipe.send(Message::Ready(2)).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::Activate(2));
            pipe.send(Message::Active(2)).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::Return(2));
            pipe.send(Message::Returning(2)).await.unwrap();
            assert_eq!(next(&mut pipe).await, Message::Returned(2));
        };
        timeout(Duration::from_secs(3), async {
            tokio::join!(source, destination);
        })
        .await
        .unwrap();
    }
}
