use super::{
    host::{self, Control},
    portal::Portal,
};
use crate::{
    config::{Edge, Identity, Key},
    gate::{Gate, Route},
    video::{Inputs, VideoWorker},
    wire::Input,
};
use anyhow::{Context, Result, ensure};
use evdev::{Device, EventSummary, InputEvent, KeyCode, RelativeAxisCode, SynchronizationCode};
use futures_util::{StreamExt, future::poll_fn};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::Arc,
    task::Poll,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};

enum CursorAction {
    Toggle,
    Suspend,
    Resume(Option<u16>),
}

struct CursorCommand {
    action: CursorAction,
    reply: oneshot::Sender<Result<()>>,
}

struct PortalCursor(mpsc::Sender<CursorCommand>);

impl PortalCursor {
    async fn command(&self, action: CursorAction) -> Result<()> {
        let (reply, receive) = oneshot::channel();
        self.0
            .send(CursorCommand { action, reply })
            .await
            .context("Wayland host stopped")?;
        receive.await.context("Wayland cursor worker stopped")?
    }
}

impl host::Cursor for PortalCursor {
    async fn request_toggle(&self) -> Result<()> {
        self.command(CursorAction::Toggle).await
    }
    async fn resume(&self) -> Result<()> {
        self.command(CursorAction::Resume(None)).await
    }
    async fn center(&self) -> Result<()> {
        self.command(CursorAction::Suspend).await
    }
    async fn enter(&self, _: &Edge, position: u16) -> Result<()> {
        self.command(CursorAction::Resume(Some(position))).await
    }
}

fn open_devices(paths: &[PathBuf]) -> Result<Vec<Device>> {
    ensure!(
        !paths.is_empty(),
        "Ubuntu host needs [capture].devices with the keyboard and mouse event paths"
    );
    ensure!(
        paths.len() <= 16,
        "at most 16 capture devices may be selected"
    );
    let mut seen = BTreeSet::new();
    let mut devices = Vec::new();
    for path in paths {
        let metadata =
            std::fs::metadata(path).with_context(|| format!("opening {}", path.display()))?;
        ensure!(
            seen.insert(metadata.rdev()),
            "the same physical event device was selected twice"
        );
        let device = Device::open(path).with_context(|| format!(
            "opening {}; grant this desktop user access to this selected event device during setup", path.display()))?;
        ensure!(
            device.supported_absolute_axes().is_none(),
            "absolute devices and touchpads are not supported as the host pointer; select a relative USB/Bluetooth mouse"
        );
        devices.push(device);
    }
    ensure!(
        devices.iter().any(|device| {
            device.supported_keys().is_some_and(|keys| {
                keys.contains(KeyCode::KEY_F11) && keys.contains(KeyCode::KEY_F12)
            })
        }),
        "the selected keyboard must provide F11 and F12"
    );
    ensure!(
        devices.iter().any(|device| device
            .supported_relative_axes()
            .is_some_and(|axes| axes.contains(RelativeAxisCode::REL_X)
                && axes.contains(RelativeAxisCode::REL_Y))),
        "select a relative mouse event device"
    );
    Ok(devices)
}

#[derive(Clone, Copy, PartialEq)]
enum Owner {
    Local,
    Suppressed,
    Remote(u64),
}

struct Routing {
    gate: Arc<Gate>,
    owners: BTreeMap<(usize, u16), Owner>,
    hotkeys: BTreeSet<(usize, u16)>,
    cooldown: Instant,
}

impl Routing {
    fn new(gate: Arc<Gate>) -> Self {
        Self {
            gate,
            owners: BTreeMap::new(),
            hotkeys: BTreeSet::new(),
            cooldown: Instant::now(),
        }
    }

    fn toggle(&self) -> Option<Control> {
        let (token, route) = self.gate.current();
        let token = match route {
            Route::Local if self.owners.is_empty() => self.gate.begin(),
            Route::Remote if self.gate.hold(token) => Some(token),
            _ => None,
        };
        token.map(Control::Toggle)
    }

    fn key(
        &mut self,
        device: usize,
        code: u16,
        value: i32,
    ) -> (Option<Control>, Option<(Owner, Input)>) {
        if value == 2 {
            return (None, None);
        }
        let down = value != 0;
        if code == 87 || code == 88 {
            if !down {
                self.hotkeys.remove(&(device, code));
                return (None, None);
            }
            if !self.hotkeys.insert((device, code)) {
                return (None, None);
            }
            if code == 87 {
                self.gate.emergency();
                self.cooldown = Instant::now() + Duration::from_millis(500);
                return (None, None);
            }
            return (self.toggle(), None);
        }
        let input = if (272..=276).contains(&code) {
            Input::Button { code, down }
        } else {
            Input::Key { code, down }
        };
        if !input.valid() {
            return (None, None);
        }
        let (token, route) = self.gate.current();
        let fallback = match route {
            Route::Local => Owner::Local,
            Route::Hold => Owner::Suppressed,
            Route::Remote => Owner::Remote(token),
        };
        let key = (device, code);
        let owner = if down {
            if self.owners.contains_key(&key) {
                return (None, None);
            }
            let duplicate = self
                .owners
                .iter()
                .any(|((_, other_code), other)| *other_code == code && *other == fallback);
            self.owners.insert(key, fallback);
            if duplicate {
                return (None, None);
            }
            fallback
        } else {
            let Some(owner) = self.owners.remove(&key) else {
                return (None, None);
            };
            if self
                .owners
                .iter()
                .any(|((_, other_code), other)| *other_code == code && *other == owner)
            {
                return (None, None);
            }
            owner
        };
        (None, Some((owner, input)))
    }
}

#[derive(Default)]
struct Motion {
    dx: i32,
    dy: i32,
    wheel_x: i32,
    wheel_y: i32,
}

impl Motion {
    fn axis(&mut self, code: RelativeAxisCode, value: i32, high_resolution: [bool; 2]) {
        let target = match code {
            RelativeAxisCode::REL_X => &mut self.dx,
            RelativeAxisCode::REL_Y => &mut self.dy,
            RelativeAxisCode::REL_HWHEEL_HI_RES => &mut self.wheel_x,
            RelativeAxisCode::REL_WHEEL_HI_RES => &mut self.wheel_y,
            RelativeAxisCode::REL_HWHEEL if !high_resolution[0] => {
                self.wheel_x = self.wheel_x.saturating_add(value.saturating_mul(120));
                return;
            }
            RelativeAxisCode::REL_WHEEL if !high_resolution[1] => {
                self.wheel_y = self.wheel_y.saturating_add(value.saturating_mul(120));
                return;
            }
            _ => return,
        };
        *target = target.saturating_add(value);
    }

    fn take(&mut self) -> [Option<Input>; 2] {
        let motion = std::mem::take(self);
        [
            (motion.dx != 0 || motion.dy != 0).then_some(Input::Move {
                dx: motion.dx.clamp(-16384, 16384),
                dy: motion.dy.clamp(-16384, 16384),
            }),
            (motion.wheel_x != 0 || motion.wheel_y != 0).then_some(Input::Scroll {
                dx: motion.wheel_x.clamp(-4096, 4096) as i16,
                dy: motion.wheel_y.saturating_neg().clamp(-4096, 4096) as i16,
            }),
        ]
    }
}

pub async fn run(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    edge: Option<Edge>,
    paths: Vec<PathBuf>,
    video: Option<(Inputs, VideoWorker)>,
) -> Result<()> {
    let mut devices = open_devices(&paths)?;
    let mut portal = Portal::open(edge.clone()).await?;
    let result = async {
        portal.start_local()?;
        let mut high_resolution = Vec::new();
        for device in &mut devices {
            device.grab().context("grabbing a selected input device; another application may own it")?;
            device.set_nonblocking(true)?;
            // Ignore events queued while the portal consent dialog was open.
            let drain_deadline = Instant::now() + Duration::from_millis(100);
            loop {
                ensure!(Instant::now() < drain_deadline, "release the selected input devices while startup completes");
                match device.fetch_events() {
                    Ok(events) => { if events.count() == 0 { break; } }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) => return Err(error.into()),
                }
            }
            ensure!(device.get_key_state()?.iter().next().is_none(),
                "release all keyboard keys and mouse buttons, then start the Ubuntu host again");
            high_resolution.push([
                device.supported_relative_axes().is_some_and(|axes| axes.contains(RelativeAxisCode::REL_HWHEEL_HI_RES)),
                device.supported_relative_axes().is_some_and(|axes| axes.contains(RelativeAxisCode::REL_WHEEL_HI_RES)),
            ]);
        }
        let streams = devices.drain(..).map(Device::into_event_stream).collect::<Result<Vec<_>, _>>()?;
        let gate = Arc::new(Gate::new());
        let (control_send, controls) = mpsc::channel(8);
        let (event_send, events) = mpsc::channel(256);
        let (cursor_send, cursor_receive) = mpsc::channel(2);
        let coordinator = host::connect(address, identity, peer, host::Coordinator {
            gate: Arc::clone(&gate), edge, video, controls, events, cursor: PortalCursor(cursor_send),
        });
        println!("Ubuntu host active. F12 switches, F11 restores local input. Closing the process releases the selected devices.");
        tokio::select! {
            result = coordinator => result,
            result = capture(&mut portal, streams, high_resolution, gate, control_send, event_send, cursor_receive) => result,
        }
    }.await;
    // Drop grabs before awaiting D-Bus cleanup, including failures halfway through setup.
    drop(devices);
    let released = portal.release();
    portal.close().await;
    result.and(released)
}

async fn capture(
    portal: &mut Portal,
    mut streams: Vec<evdev::EventStream>,
    high_resolution: Vec<[bool; 2]>,
    gate: Arc<Gate>,
    controls: mpsc::Sender<Control>,
    events: mpsc::Sender<(u64, Input)>,
    mut cursor: mpsc::Receiver<CursorCommand>,
) -> Result<()> {
    let mut routing = Routing::new(Arc::clone(&gate));
    let mut motion = (0..streams.len())
        .map(|_| Motion::default())
        .collect::<Vec<_>>();
    let mut local_active = true;
    let mut resume: Option<(CursorCommand, Instant)> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if let Some((command, deadline)) = resume.take() {
                    let result = portal.start_local().and_then(|()| {
                        if let CursorAction::Resume(Some(position)) = command.action {
                            portal.enter_local(position)?;
                        }
                        Ok(())
                    });
                    match result {
                        Ok(()) => {
                            local_active = true;
                            let _ = command.reply.send(Ok(()));
                        }
                        Err(error) if Instant::now() >= deadline => {
                            let _ = command.reply.send(Err(error));
                        }
                        Err(_) => resume = Some((command, deadline)),
                    }
                }
                if gate.current().1 == Route::Local && !local_active && resume.is_none() {
                    // Emergency fallback must not depend on remote DDC or a missing EIS region.
                    // Failure exits this loop and drops the kernel grabs.
                    portal.start_local()?;
                    local_active = true;
                }
            }
            event = portal.stream.next() => portal.handle(event.context("local Wayland EIS closed")??)?,
            command = cursor.recv() => {
                let command = command.context("host coordinator stopped")?;
                match command.action {
                    CursorAction::Toggle => {
                        if let Some(control) = routing.toggle()
                            && controls.try_send(control).is_err()
                        {
                            gate.emergency();
                        }
                        if gate.current().1 != Route::Local && local_active {
                            portal.release()?;
                            local_active = false;
                        }
                        let _ = command.reply.send(Ok(()));
                    }
                    CursorAction::Suspend => {
                        if local_active {
                            portal.release()?;
                            local_active = false;
                        }
                        let _ = command.reply.send(Ok(()));
                    }
                    CursorAction::Resume(position) if local_active => {
                        let result = match position {
                            Some(position) => portal.enter_local(position),
                            None => Ok(()),
                        };
                        let _ = command.reply.send(result);
                    }
                    CursorAction::Resume(_) => {
                        ensure!(resume.is_none(), "a local cursor resume is already pending");
                        resume = Some((command, Instant::now() + Duration::from_secs(2)));
                    }
                }
            }
            result = poll_fn(|cx| {
                for (index, stream) in streams.iter_mut().enumerate() {
                    if let Poll::Ready(event) = stream.poll_event(cx) {
                        return Poll::Ready(event.map(|event| (index, event)));
                    }
                }
                Poll::Pending
            }) => {
                let (device, event): (usize, InputEvent) = result.context("selected input device disconnected")?;
                if gate.current().1 == Route::Local && !local_active {
                    portal.start_local()?;
                    local_active = true;
                }
                match event.destructure() {
                    EventSummary::Key(_, code, value) => {
                        let (control, input) = routing.key(device, code.0, value);
                        if let Some(control) = control
                            && controls.try_send(control).is_err()
                        {
                            gate.emergency();
                        }
                        if let Some((owner, input)) = input {
                            route(portal, &routing, owner, input, &events, &controls)?;
                        }
                    }
                    EventSummary::RelativeAxis(_, code, value) => motion[device].axis(code, value, high_resolution[device]),
                    EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
                        let (token, active) = gate.current();
                        let owner = match active {
                            Route::Local => Owner::Local,
                            Route::Hold => Owner::Suppressed,
                            Route::Remote => Owner::Remote(token),
                        };
                        for input in motion[device].take().into_iter().flatten() {
                            route(portal, &routing, owner, input, &events, &controls)?;
                        }
                    }
                    _ => {}
                }
                if gate.current().1 != Route::Local && local_active {
                    portal.release()?;
                    local_active = false;
                }
            }
        }
    }
}

fn route(
    portal: &mut Portal,
    routing: &Routing,
    owner: Owner,
    input: Input,
    events: &mpsc::Sender<(u64, Input)>,
    controls: &mpsc::Sender<Control>,
) -> Result<()> {
    let (token, active) = routing.gate.current();
    match owner {
        Owner::Local => {
            let idle = routing.owners.is_empty()
                && Instant::now() >= routing.cooldown
                && active == Route::Local;
            if let Some(position) = portal.local_event(input, idle)?
                && let Some(token) = routing.gate.begin()
                && controls
                    .try_send(Control::Edge { token, position })
                    .is_err()
            {
                routing.gate.local(token);
            }
        }
        Owner::Remote(owned)
            if owned == token
                && active == Route::Remote
                && events.try_send((token, input)).is_err() =>
        {
            routing.gate.local(token);
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emergency_discards_remote_held_keys_until_their_physical_release() {
        let gate = Arc::new(Gate::new());
        let mut routing = Routing::new(Arc::clone(&gate));
        let (toggle, _) = routing.key(0, 88, 1);
        assert!(matches!(toggle, Some(Control::Toggle(_))));
        let token = gate.current().0;
        assert!(gate.forward(token));
        let (_, key) = routing.key(0, 30, 1);
        assert!(matches!(key, Some((Owner::Remote(_), _))));
        routing.key(0, 87, 1);
        assert!(gate.current().1 == Route::Local);
        let (_, release) = routing.key(0, 30, 0);
        assert!(matches!(
            release,
            Some((Owner::Remote(_), Input::Key { down: false, .. }))
        ));
        let (_, fresh) = routing.key(0, 30, 1);
        assert!(matches!(fresh, Some((Owner::Local, _))));
    }

    #[test]
    fn motion_uses_one_scroll_representation_and_preserves_both_axes() {
        let mut motion = Motion::default();
        motion.axis(RelativeAxisCode::REL_X, 7, [false, true]);
        motion.axis(RelativeAxisCode::REL_Y, -3, [false, true]);
        motion.axis(RelativeAxisCode::REL_WHEEL, 1, [false, true]);
        motion.axis(RelativeAxisCode::REL_WHEEL_HI_RES, 120, [false, true]);
        motion.axis(RelativeAxisCode::REL_HWHEEL, 1, [false, true]);
        assert_eq!(
            motion.take(),
            [
                Some(Input::Move { dx: 7, dy: -3 }),
                Some(Input::Scroll { dx: 120, dy: -120 })
            ]
        );
    }
}
