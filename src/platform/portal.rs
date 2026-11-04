use crate::{config::Edge, edge::Rect, wire::Input};
use anyhow::{Context, Result, bail, ensure};
use ashpd::desktop::{
    CreateSessionOptions, Session,
    remote_desktop::{ConnectToEISOptions, DeviceType, RemoteDesktop, SelectDevicesOptions, StartOptions},
};
use futures_util::StreamExt;
use reis::{
    ei,
    event::{Connection, Device, DeviceCapability as Cap, EiEvent},
    tokio::EiConvertEventStream,
};
use std::{collections::BTreeSet, os::unix::net::UnixStream, time::Duration};
use tokio::time::timeout;

struct Screen {
    id: String,
    rect: Rect,
    scale: f64,
    device: Device,
}

pub async fn check() -> Result<RemoteDesktop> {
    ensure!(std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland"),
        "start Desk Switch inside the logged-in Ubuntu Wayland session");
    let portal = timeout(Duration::from_secs(5), RemoteDesktop::new()).await?
        .context("opening the Wayland RemoteDesktop portal; check the session D-Bus and portal backend")?;
    let devices = timeout(Duration::from_secs(5), portal.available_device_types()).await??;
    ensure!(portal.version() >= 2, "RemoteDesktop portal version 2 with ConnectToEIS is required");
    ensure!(devices.contains(DeviceType::Keyboard | DeviceType::Pointer),
        "the Wayland portal backend does not advertise keyboard and pointer");
    println!("Wayland RemoteDesktop v{} advertises keyboard and pointer.", portal.version());
    Ok(portal)
}

pub struct Portal {
    session: Session<RemoteDesktop>,
    connection: Connection,
    pub stream: EiConvertEventStream,
    devices: Vec<Device>,
    started: Vec<Device>,
    sequence: u32,
    keys: BTreeSet<u16>,
    buttons: BTreeSet<u16>,
    edge: Option<Edge>,
    cursor: Option<(f64, f64)>,
}

impl Portal {
    pub async fn open(edge: Option<Edge>) -> Result<Self> {
        let portal = check().await?;
        let session = timeout(Duration::from_secs(8), portal.create_session(CreateSessionOptions::default())).await??;
        let result = timeout(Duration::from_secs(120), async {
            portal.select_devices(&session, SelectDevicesOptions::default()
                .set_devices(DeviceType::Keyboard | DeviceType::Pointer)).await?.response()?;
            println!("Approve keyboard and pointer access in the Ubuntu portal dialog.");
            let granted = portal.start(&session, None, StartOptions::default()).await?.response()?;
            ensure!(granted.devices().contains(DeviceType::Keyboard | DeviceType::Pointer),
                "the portal did not grant both keyboard and pointer");
            let fd = portal.connect_to_eis(&session, ConnectToEISOptions::default()).await
                .context("the Wayland portal backend could not open ConnectToEIS")?;
            let context = ei::Context::new(UnixStream::from(fd))?;
            Ok::<_, anyhow::Error>(context.handshake_tokio("Desk Switch", ei::handshake::ContextType::Sender).await?)
        }).await;
        let (connection, stream) = match result {
            Ok(Ok(ready)) => ready,
            failed => {
                let _ = timeout(Duration::from_secs(1), session.close()).await;
                return Err(failed.err().map(anyhow::Error::from)
                    .unwrap_or_else(|| anyhow::anyhow!("Wayland input consent or EIS setup failed")));
            }
        };
        let mut result = Self {
            session, connection, stream, devices: Vec::new(), started: Vec::new(),
            sequence: 0, keys: BTreeSet::new(), buttons: BTreeSet::new(), edge, cursor: None,
        };
        let ready = timeout(Duration::from_secs(5), async {
            while !result.ready() {
                let event = result.stream.next().await.context("EIS device stream closed")??;
                result.handle(event)?;
            }
            Ok::<_, anyhow::Error>(())
        }).await;
        if let Err(error) = ready.context("EIS did not expose usable keyboard and pointer devices").and_then(|value| value) {
            result.close().await;
            return Err(error);
        }
        // Absolute regions are optional and may arrive after the relative devices.
        let _ = timeout(Duration::from_millis(350), async {
            loop {
                let event = result.stream.next().await.context("EIS device stream closed")??;
                result.handle(event)?;
            }
            #[allow(unreachable_code)]
            Ok::<_, anyhow::Error>(())
        }).await.transpose()?;
        println!("Wayland EIS session ready. No screen capture was requested.");
        let screens = result.screens();
        for screen in &screens {
            println!("  EIS screen '{}' (scale {})", screen.id, screen.scale);
        }
        if let Some(edge) = &result.edge
            && result.find_screen(&edge.screen).is_err()
        {
            eprintln!("Edge screen '{}' is unavailable or ambiguous; F12 can still use relative input.", edge.screen);
        }
        Ok(result)
    }

    fn has(&self, cap: Cap) -> bool {
        self.devices.iter().any(|device| device.has_capability(cap))
    }

    pub fn ready(&self) -> bool {
        [Cap::Keyboard, Cap::Pointer, Cap::Button, Cap::Scroll].into_iter().all(|cap| self.has(cap))
    }

    pub fn handle(&mut self, event: EiEvent) -> Result<()> {
        match event {
            EiEvent::SeatAdded(event) => {
                event.seat.bind_capabilities(Cap::Keyboard | Cap::Pointer | Cap::PointerAbsolute | Cap::Button | Cap::Scroll);
                self.connection.flush()?;
            }
            EiEvent::DeviceResumed(event) => {
                self.devices.retain(|device| device.device() != event.device.device());
                self.devices.push(event.device);
            }
            EiEvent::DevicePaused(_) | EiEvent::DeviceRemoved(_) => {
                let event = event.device().context("EIS device event without a device")?;
                self.devices.retain(|device| device.device() != event.device());
                ensure!(!self.started.iter().any(|device| device.device() == event.device()),
                    "the compositor paused or removed an active EIS input device");
            }
            EiEvent::Disconnected(_) => bail!("Wayland EIS session disconnected"),
            _ => {}
        }
        Ok(())
    }

    fn screens(&self) -> Vec<Screen> {
        self.devices.iter().filter(|device| device.has_capability(Cap::PointerAbsolute))
            .flat_map(|device| device.regions().iter().filter_map(|region| {
                if region.width < 2 || region.height < 2 || !region.scale.is_finite() || region.scale <= 0.0 {
                    return None;
                }
                Some(Screen {
                    id: format!("{},{},{},{}", region.x, region.y, region.width, region.height),
                    rect: Rect {
                        x: region.x.into(), y: region.y.into(), width: region.width.into(), height: region.height.into(),
                    },
                    scale: region.scale.into(),
                    device: device.clone(),
                })
            })).collect()
    }

    fn find_screen(&self, id: &str) -> Result<Screen> {
        let mut screens = self.screens().into_iter().filter(|screen| screen.id == id);
        let screen = screens.next().context("the selected Wayland EIS edge screen is unavailable")?;
        ensure!(screens.next().is_none(), "the selected EIS region is ambiguous");
        Ok(screen)
    }

    pub fn can_start(&self, edge: Option<u16>) -> Result<()> {
        ensure!(self.ready() && self.started.is_empty(), "Wayland input devices are unavailable or busy");
        if edge.is_some() {
            self.find_screen(&self.edge.as_ref().context("configure the Ubuntu return edge")?.screen)?;
        }
        Ok(())
    }

    pub fn start(&mut self, edge: Option<u16>) -> Result<()> {
        self.can_start(edge)?;
        // Start all eligible devices. Relative and absolute pointer devices can be separate.
        self.sequence = self.sequence.wrapping_add(1);
        self.started = self.devices.clone();
        for device in &self.started {
            device.device().start_emulating(self.connection.serial(), self.sequence);
        }
        if let Some(fraction) = edge {
            let config = self.edge.as_ref().unwrap();
            let screen = self.find_screen(&config.screen)?;
            let (x, y) = config.side.entry(screen.rect, fraction);
            self.absolute(&screen, x, y)?;
            self.cursor = Some((x, y));
        }
        self.connection.flush()?;
        Ok(())
    }

    fn send<T: ei::Interface>(&self, action: impl FnOnce(T)) -> Result<()> {
        let (device, interface) = self.started.iter().find_map(|device| {
            self.devices.iter().any(|ready| ready.device() == device.device())
                .then(|| device.interface::<T>()).flatten().map(|interface| (device, interface))
        }).context("the requested EIS input interface is unavailable")?;
        action(interface);
        self.frame(device);
        Ok(())
    }

    fn frame(&self, device: &Device) {
        device.device().frame(self.connection.serial(), reis::event::get_time_usec());
    }

    fn absolute(&self, screen: &Screen, x: f64, y: f64) -> Result<()> {
        screen.device.interface::<ei::PointerAbsolute>().context("EIS absolute pointer unavailable")?
            .motion_absolute(x as f32, y as f32);
        self.frame(&screen.device);
        Ok(())
    }

    pub fn event(&mut self, event: Input) -> Result<Option<u16>> {
        ensure!(event.valid() && !self.started.is_empty(), "invalid or inactive input event");
        match event {
            Input::Key { code, down } => {
                if down == self.keys.contains(&code) { return Ok(None); }
                self.send(|key: ei::Keyboard| key.key(code.into(),
                    if down { ei::keyboard::KeyState::Press } else { ei::keyboard::KeyState::Released }))?;
                if down { self.keys.insert(code); } else { self.keys.remove(&code); }
            }
            Input::Button { code, down } => {
                if down == self.buttons.contains(&code) { return Ok(None); }
                self.send(|button: ei::Button| button.button(code.into(),
                    if down { ei::button::ButtonState::Press } else { ei::button::ButtonState::Released }))?;
                if down { self.buttons.insert(code); } else { self.buttons.remove(&code); }
            }
            Input::Scroll { dx, dy } => {
                self.send(|scroll: ei::Scroll| scroll.scroll_discrete(dx.into(), dy.into()))?;
            }
            Input::Move { dx, dy } => {
                if let Some((x, y)) = self.cursor {
                    let screens = self.screens();
                    let current = screens.iter().find(|screen| screen.rect.contains(x, y))
                        .context("Wayland edge layout changed during the transfer")?;
                    let (dx, dy) = (f64::from(dx) / current.scale, f64::from(dy) / current.scale);
                    let edge = self.edge.as_ref().unwrap();
                    if current.id == edge.screen && self.keys.is_empty() && self.buttons.is_empty()
                        && edge.side.crossed(current.rect, x, y, dx, dy)
                    {
                        return Ok(Some(edge.side.fraction(current.rect, x + dx, y + dy)));
                    }
                    let (x, y) = (x + dx, y + dy);
                    let target = screens.iter().find(|screen| screen.rect.contains(x, y)).unwrap_or(current);
                    let (x, y) = target.rect.clamp(x, y);
                    self.absolute(target, x, y)?;
                    self.cursor = Some((x, y));
                } else {
                    self.send(|pointer: ei::Pointer| pointer.motion_relative(dx as f32, dy as f32))?;
                }
            }
        }
        self.connection.flush()?;
        Ok(None)
    }

    pub fn release(&mut self) -> Result<()> {
        let mut result = Ok(());
        for code in std::mem::take(&mut self.keys) {
            result = result.and(self.send(|key: ei::Keyboard| key.key(code.into(), ei::keyboard::KeyState::Released)));
        }
        for code in std::mem::take(&mut self.buttons) {
            result = result.and(self.send(|button: ei::Button| button.button(code.into(), ei::button::ButtonState::Released)));
        }
        for device in self.started.drain(..) {
            if self.devices.iter().any(|ready| ready.device() == device.device()) {
                device.device().stop_emulating(self.connection.serial());
            }
        }
        self.cursor = None;
        result.and(self.connection.flush().map_err(Into::into))
    }

    pub async fn close(self) {
        let _ = timeout(Duration::from_secs(1), self.session.close()).await;
    }
}
