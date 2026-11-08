use crate::{
    config::Edge,
    edge::{Crossing, Entry, Hit, NativeScreen, Router, Routes, ScreenRect},
    wire::{Input, Mode},
};
use anyhow::{Context, Result, bail, ensure};
use ashpd::desktop::{
    CreateSessionOptions, PersistMode, Session,
    remote_desktop::{
        ConnectToEISOptions, DeviceType, RemoteDesktop, SelectDevicesOptions, StartOptions,
    },
};
use futures_util::StreamExt;
use reis::{
    ei,
    event::{Connection, Device, DeviceCapability as Cap, EiEvent},
    tokio::EiConvertEventStream,
};
use std::{collections::BTreeSet, os::unix::net::UnixStream, path::Path, time::Duration};
use tokio::time::timeout;

#[derive(Clone)]
struct Screen {
    id: String,
    rect: ScreenRect,
    scale: f64,
    device: Device,
}

type RefreshedGeometry = (Option<Vec<NativeScreen>>, Option<(i32, i32)>);

struct Refresh {
    geometry: bool,
    pointer: bool,
    geometry_stamp: u64,
    pointer_stamp: u64,
    task: tokio::task::JoinHandle<RefreshedGeometry>,
}

/// Both sources must identify exactly one region. Mirrored outputs and stale
/// compositor geometry cannot authorize an absolute entry or an edge crossing.
fn match_regions(regions: &[ScreenRect], native: &[NativeScreen]) -> Vec<NativeScreen> {
    native
        .iter()
        .filter(|screen| {
            screen.validate().is_ok()
                && regions.iter().filter(|rect| **rect == screen.rect).count() == 1
                && native
                    .iter()
                    .filter(|other| other.rect == screen.rect)
                    .count()
                    == 1
        })
        .cloned()
        .collect()
}

fn unique_region(screens: &[Screen], x: f64, y: f64) -> Option<usize> {
    let mut found = screens
        .iter()
        .enumerate()
        .filter(|(_, screen)| screen.rect.rect().contains(x, y));
    let (index, _) = found.next()?;
    found.next().is_none().then_some(index)
}

pub async fn check() -> Result<RemoteDesktop> {
    ensure!(
        std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland"),
        "start Desk Switch inside the logged-in Ubuntu Wayland session"
    );
    let portal = timeout(Duration::from_secs(5), RemoteDesktop::new())
        .await?
        .context(
            "opening the Wayland RemoteDesktop portal; check the session D-Bus and portal backend",
        )?;
    let devices = timeout(Duration::from_secs(5), portal.available_device_types()).await??;
    ensure!(
        portal.version() >= 2,
        "RemoteDesktop portal version 2 with ConnectToEIS is required"
    );
    ensure!(
        devices.contains(DeviceType::Keyboard | DeviceType::Pointer),
        "the Wayland portal backend does not advertise keyboard and pointer"
    );
    println!(
        "Wayland RemoteDesktop v{} advertises keyboard and pointer.",
        portal.version()
    );
    Ok(portal)
}

pub struct Portal {
    _permission_lock: std::fs::File,
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
    screens: Vec<Screen>,
    native: Vec<NativeScreen>,
    router: Router,
    group: bool,
    generation: u64,
    crossing: Option<Crossing>,
    geometry_stamp: u64,
    pointer_stamp: u64,
    geometry_dirty: bool,
    pointer_dirty: bool,
    refresh: Option<Refresh>,
}

impl Portal {
    pub async fn open(edge: Option<Edge>) -> Result<Self> {
        let dir = crate::config::state_dir()?;
        let permission_lock = crate::config::lock(&dir, "portal.lock")
            .context("another input session is already using this profile's Wayland permission")?;
        let portal = tokio::select! {
            result = check() => result?,
            result = crate::lifecycle::shutdown() => {
                result?;
                bail!("Wayland setup stopped");
            }
        };
        let session = timeout(
            Duration::from_secs(8),
            portal.create_session(CreateSessionOptions::default()),
        )
        .await??;
        // Restore tokens are single use. Consume the old token before requesting
        // consent and save its replacement as soon as Start succeeds.
        let restore_token = take_restore_token(&dir);
        let setup = timeout(Duration::from_secs(120), async {
            portal
                .select_devices(
                    &session,
                    SelectDevicesOptions::default()
                        .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
                        .set_persist_mode(PersistMode::ExplicitlyRevoked)
                        .set_restore_token(restore_token.as_deref()),
                )
                .await?
                .response()?;
            println!("Approve keyboard and pointer access in the Ubuntu portal dialog.");
            let granted = portal
                .start(&session, None, StartOptions::default())
                .await?
                .response()?;
            ensure!(
                granted
                    .devices()
                    .contains(DeviceType::Keyboard | DeviceType::Pointer),
                "the portal did not grant both keyboard and pointer"
            );
            if let Err(error) = save_restore_token(&dir, granted.restore_token()) {
                eprintln!("Could not save the Wayland permission for the next session: {error:#}");
            }
            let fd = portal
                .connect_to_eis(&session, ConnectToEISOptions::default())
                .await
                .context("the Wayland portal backend could not open ConnectToEIS")?;
            let context = ei::Context::new(UnixStream::from(fd))?;
            Ok::<_, anyhow::Error>(
                context
                    .handshake_tokio("Desk Switch", ei::handshake::ContextType::Sender)
                    .await?,
            )
        });
        let result = tokio::select! {
            result = setup => result,
            result = crate::lifecycle::shutdown() => {
                let _ = timeout(Duration::from_secs(1), session.close()).await;
                result?;
                bail!("Wayland setup stopped");
            }
        };
        let (connection, stream) = match result {
            Ok(Ok(ready)) => ready,
            failed => {
                let _ = timeout(Duration::from_secs(1), session.close()).await;
                return Err(match failed {
                    Ok(Err(error)) => error.context("Wayland input consent or EIS setup failed"),
                    Err(error) => anyhow::Error::from(error)
                        .context("Wayland input consent or EIS setup timed out"),
                    Ok(Ok(_)) => unreachable!(),
                });
            }
        };
        let mut result = Self {
            _permission_lock: permission_lock,
            session,
            connection,
            stream,
            devices: Vec::new(),
            started: Vec::new(),
            sequence: 0,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            edge,
            cursor: None,
            screens: Vec::new(),
            native: Vec::new(),
            router: Router::default(),
            group: crate::lifecycle::group_input(),
            generation: 0,
            crossing: None,
            geometry_stamp: 0,
            pointer_stamp: 0,
            geometry_dirty: false,
            pointer_dirty: false,
            refresh: None,
        };
        let ready = timeout(Duration::from_secs(5), async {
            while !result.ready() {
                let event = result
                    .stream
                    .next()
                    .await
                    .context("EIS device stream closed")??;
                result.handle(event)?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await;
        if let Err(error) = ready
            .context("EIS did not expose usable keyboard and pointer devices")
            .and_then(|value| value)
        {
            result.close().await;
            return Err(error);
        }
        // Absolute regions are optional and may arrive after the relative devices.
        if let Ok(Err(error)) = timeout(Duration::from_millis(350), async {
            loop {
                let event = result
                    .stream
                    .next()
                    .await
                    .context("EIS device stream closed")??;
                result.handle(event)?;
            }
            #[allow(unreachable_code)]
            Ok::<_, anyhow::Error>(())
        })
        .await
        {
            result.close().await;
            return Err(error);
        }
        println!("Wayland EIS session ready. No screen capture was requested.");
        for screen in &result.screens {
            println!("  EIS screen '{}' (scale {})", screen.id, screen.scale);
        }
        if result.group {
            // The public compositor query runs off the latency-sensitive input
            // executor. Failure leaves the relative EIS devices usable.
            let native = tokio::task::spawn_blocking(super::linux_geometry::snapshot)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            result.replace_native(native)?;
            result.geometry_dirty = false;
            let routes = crate::lifecycle::routes().borrow().clone();
            result.set_routes(routes)?;
        }
        if let Some(edge) = &result.edge
            && result.find_screen(&edge.screen).is_err()
        {
            eprintln!(
                "Edge screen '{}' is unavailable or ambiguous; F12 can still use relative input.",
                edge.screen
            );
        }
        Ok(result)
    }

    fn has(&self, cap: Cap) -> bool {
        self.devices.iter().any(|device| device.has_capability(cap))
    }

    pub fn ready(&self) -> bool {
        [Cap::Keyboard, Cap::Pointer, Cap::Button, Cap::Scroll]
            .into_iter()
            .all(|cap| self.has(cap))
    }

    pub fn handle(&mut self, event: EiEvent) -> Result<()> {
        match event {
            EiEvent::SeatAdded(event) => {
                event.seat.bind_capabilities(
                    Cap::Keyboard | Cap::Pointer | Cap::PointerAbsolute | Cap::Button | Cap::Scroll,
                );
                self.connection.flush()?;
            }
            EiEvent::DeviceResumed(event) => {
                self.devices
                    .retain(|device| device.device() != event.device.device());
                self.devices.push(event.device);
                self.regions_changed()?;
            }
            EiEvent::DevicePaused(_) | EiEvent::DeviceRemoved(_) => {
                let event = event
                    .device()
                    .context("EIS device event without a device")?;
                self.devices
                    .retain(|device| device.device() != event.device());
                let optional_absolute = self.group
                    && event.has_capability(Cap::PointerAbsolute)
                    && ![Cap::Keyboard, Cap::Pointer, Cap::Button, Cap::Scroll]
                        .into_iter()
                        .any(|cap| event.has_capability(cap));
                ensure!(
                    optional_absolute
                        || !self
                            .started
                            .iter()
                            .any(|device| device.device() == event.device()),
                    "the compositor paused or removed an active EIS input device"
                );
                self.started
                    .retain(|device| device.device() != event.device());
                self.regions_changed()?;
            }
            EiEvent::Disconnected(_) => bail!("Wayland EIS session disconnected"),
            _ => {}
        }
        Ok(())
    }

    fn regions_changed(&mut self) -> Result<()> {
        self.screens = self
            .devices
            .iter()
            .filter(|device| device.has_capability(Cap::PointerAbsolute))
            .flat_map(|device| {
                device.regions().iter().filter_map(|region| {
                    if region.width < 2
                        || region.height < 2
                        || !region.scale.is_finite()
                        || region.scale <= 0.0
                    {
                        return None;
                    }
                    let rect = ScreenRect {
                        x: i32::try_from(region.x).ok()?,
                        y: i32::try_from(region.y).ok()?,
                        width: region.width,
                        height: region.height,
                    };
                    rect.validate().ok()?;
                    Some(Screen {
                        id: format!(
                            "{},{},{},{}",
                            region.x, region.y, region.width, region.height
                        ),
                        rect,
                        scale: region.scale.into(),
                        device: device.clone(),
                    })
                })
            })
            .collect();
        if self.group {
            self.cancel_dwell();
            self.geometry_stamp = self.geometry_stamp.wrapping_add(1);
            self.geometry_dirty = true;
            self.native.clear();
            self.router.configure(self.router.routes().clone(), &[])?;
            self.cursor = None;
            // Region changes invalidate the old pointer basis. This is a
            // resume query, never a motion-time query or an unsolicited warp.
            if !self.started.is_empty() {
                self.refresh_pointer();
                self.start_absolute_devices();
                self.connection.flush()?;
            }
        }
        Ok(())
    }

    fn find_screen(&self, id: &str) -> Result<Screen> {
        if id == "auto" {
            return self
                .screens
                .iter()
                .min_by(|a, b| a.rect.x.cmp(&b.rect.x).then(a.rect.y.cmp(&b.rect.y)))
                .cloned()
                .context("Wayland не предоставил область экрана");
        }
        let mut screens = self.screens.iter().filter(|screen| screen.id == id);
        let screen = screens
            .next()
            .context("the selected Wayland EIS edge screen is unavailable")?;
        ensure!(
            screens.next().is_none(),
            "the selected EIS region is ambiguous"
        );
        Ok(screen.clone())
    }

    fn replace_native(&mut self, native: Vec<NativeScreen>) -> Result<()> {
        self.cancel_dwell();
        let regions: Vec<_> = self.screens.iter().map(|screen| screen.rect).collect();
        self.native = match_regions(&regions, &native);
        self.router
            .configure(self.router.routes().clone(), &self.native)
    }

    pub fn set_routes(&mut self, routes: Routes) -> Result<()> {
        routes.validate()?;
        if self.router.routes() == &routes {
            return Ok(());
        }
        let needs_geometry = routes.boundaries.iter().any(|boundary| {
            Entry {
                screen: boundary.screen,
                geometry: boundary.geometry,
                side: boundary.side,
                position: 0,
            }
            .resolve(&self.native)
            .is_err()
        });
        self.router.configure(routes, &self.native)?;
        self.crossing = None;
        if self.group && needs_geometry {
            self.geometry_stamp = self.geometry_stamp.wrapping_add(1);
            self.geometry_dirty = true;
            self.native.clear();
            self.router.configure(self.router.routes().clone(), &[])?;
            if !self.started.is_empty() {
                self.refresh_pointer();
            }
        }
        self.start_refresh();
        Ok(())
    }

    pub fn set_generation(&mut self, generation: u64) {
        if self.generation != generation {
            self.generation = generation;
            self.crossing = None;
            self.router.reset_entry();
        }
    }

    pub fn take_crossing(&mut self) -> Option<Crossing> {
        self.crossing.take()
    }

    pub fn cancel_dwell(&mut self) {
        self.router.cancel_dwell();
        self.crossing = None;
    }

    pub fn display_pending(&self) -> bool {
        self.group
            && (self.geometry_dirty
                || self
                    .refresh
                    .as_ref()
                    .is_some_and(|refresh| refresh.geometry))
    }

    pub fn refresh_pointer(&mut self) {
        if self.group {
            self.cancel_dwell();
            self.pointer_stamp = self.pointer_stamp.wrapping_add(1);
            self.cursor = None;
            self.pointer_dirty = true;
            self.start_refresh();
        }
    }

    fn start_refresh(&mut self) {
        if !self.group || self.refresh.is_some() || !(self.geometry_dirty || self.pointer_dirty) {
            return;
        }
        let geometry = std::mem::take(&mut self.geometry_dirty);
        let pointer = std::mem::take(&mut self.pointer_dirty);
        self.refresh = Some(Refresh {
            geometry,
            pointer,
            geometry_stamp: self.geometry_stamp,
            pointer_stamp: self.pointer_stamp,
            task: tokio::task::spawn_blocking(move || {
                let native = geometry.then(super::linux_geometry::snapshot).flatten();
                let point = if pointer {
                    match super::linux_geometry::pointer() {
                        Ok(point) => Some(point),
                        Err(error) => {
                            eprintln!("{error}");
                            None
                        }
                    }
                } else {
                    None
                };
                (native, point)
            }),
        });
    }

    /// Keep the refresh task in the adapter: callers may cancel this poll in a
    /// select without discarding a query or spawning another blocking worker.
    pub async fn poll(&mut self) -> Result<()> {
        self.start_refresh();
        if let Some(refresh) = self.refresh.as_mut() {
            tokio::select! {
                event = self.stream.next() => {
                    self.handle(event.context("EIS device stream closed")??)
                }
                result = &mut refresh.task => {
                    let refresh = self.refresh.take().unwrap();
                    let (native, point) = result.unwrap_or_default();
                    if refresh.geometry && refresh.geometry_stamp == self.geometry_stamp {
                        self.replace_native(native.unwrap_or_default())?;
                    }
                    if refresh.pointer && refresh.pointer_stamp == self.pointer_stamp && !self.started.is_empty() {
                        self.cursor = point.and_then(|(x,y)| {
                            let (x,y) = (f64::from(x),f64::from(y));
                            let index = unique_region(&self.screens,x,y)?;
                            self.native.iter().any(|native| native.rect == self.screens[index].rect)
                                .then_some((x,y))
                        });
                        self.router.reset_entry();
                    }
                    self.start_refresh();
                    Ok(())
                }
            }
        } else {
            let event = self
                .stream
                .next()
                .await
                .context("EIS device stream closed")??;
            self.handle(event)
        }
    }

    fn entry_screen(&self, entry: Entry) -> Result<Screen> {
        entry.resolve(&self.native)?;
        let mut screens = self
            .screens
            .iter()
            .filter(|screen| screen.rect == entry.screen);
        let screen = screens
            .next()
            .context("The destination EIS region is unavailable")?;
        ensure!(
            screens.next().is_none(),
            "The destination EIS region is ambiguous"
        );
        Ok(screen.clone())
    }

    pub fn can_start(&self, edge: Option<u16>, mode: Mode, require_region: bool) -> Result<()> {
        ensure!(
            self.ready() && self.started.is_empty(),
            "Wayland input devices are unavailable or busy"
        );
        if edge.is_some() {
            let config = self
                .edge
                .as_ref()
                .context("configure the Wayland return edge")?;
            ensure!(
                config.mode == mode,
                "edge modes differ between host and receiver"
            );
            if require_region {
                self.find_screen(&config.screen)?;
            }
        }
        Ok(())
    }

    pub fn start(&mut self, edge: Option<u16>, mode: Mode) -> Result<()> {
        self.start_display(edge, None, mode)
    }

    pub fn can_start_display(
        &self,
        edge: Option<u16>,
        entry: Option<Entry>,
        mode: Mode,
    ) -> Result<()> {
        self.can_start(edge, mode, mode == Mode::Input)?;
        if let Some(entry) = entry {
            ensure!(self.group, "Explicit display entry requires group input");
            self.entry_screen(entry)?;
        }
        Ok(())
    }

    pub fn start_display(
        &mut self,
        edge: Option<u16>,
        entry: Option<Entry>,
        mode: Mode,
    ) -> Result<()> {
        self.can_start_display(edge, entry, mode)?;
        self.can_start(edge, mode, true)?;
        let destination = entry.map(|entry| self.entry_screen(entry)).transpose()?;
        // Start all eligible devices. Relative and absolute pointer devices can be separate.
        self.sequence = self.sequence.wrapping_add(1);
        for cap in [Cap::Keyboard, Cap::Pointer, Cap::Button, Cap::Scroll] {
            let device = self
                .devices
                .iter()
                .find(|device| device.has_capability(cap))
                .unwrap();
            if !self
                .started
                .iter()
                .any(|started| started.device() == device.device())
            {
                self.started.push(device.clone());
            }
        }
        if edge.is_some() || self.group {
            for device in &self.devices {
                if device.has_capability(Cap::PointerAbsolute)
                    && !self
                        .started
                        .iter()
                        .any(|started| started.device() == device.device())
                {
                    self.started.push(device.clone());
                }
            }
        }
        for device in &self.started {
            device
                .device()
                .start_emulating(self.connection.serial(), self.sequence);
        }
        if let Some(fraction) = edge {
            let config = self.edge.as_ref().unwrap();
            let screen = self.find_screen(&config.screen)?;
            let (x, y) = config.entry(screen.rect.rect(), fraction);
            self.absolute(&screen, x, y)?;
            self.cursor = Some((x, y));
        } else if let (Some(entry), Some(screen)) = (entry, destination) {
            self.position_display(entry, &screen)?;
        } else if self.group {
            self.refresh_pointer();
        }
        self.connection.flush()?;
        Ok(())
    }

    fn start_absolute_devices(&mut self) {
        for device in &self.devices {
            if device.has_capability(Cap::PointerAbsolute)
                && !self
                    .started
                    .iter()
                    .any(|started| started.device() == device.device())
            {
                device
                    .device()
                    .start_emulating(self.connection.serial(), self.sequence);
                self.started.push(device.clone());
            }
        }
    }

    fn position_display(&mut self, entry: Entry, screen: &Screen) -> Result<()> {
        let (x, y) = entry.point();
        self.start_absolute_devices();
        self.absolute(screen, x, y)?;
        self.pointer_stamp = self.pointer_stamp.wrapping_add(1);
        self.pointer_dirty = false;
        self.cursor = Some((x, y));
        self.crossing = None;
        self.router.reset_entry();
        Ok(())
    }

    pub fn enter_display(&mut self, entry: Entry) -> Result<()> {
        ensure!(
            !self.started.is_empty(),
            "Wayland input devices are inactive"
        );
        let screen = self.entry_screen(entry)?;
        self.position_display(entry, &screen)?;
        self.connection.flush()?;
        Ok(())
    }

    fn send<T: ei::Interface>(&self, action: impl FnOnce(T)) -> Result<()> {
        let (device, interface) = self
            .started
            .iter()
            .find_map(|device| {
                self.devices
                    .iter()
                    .any(|ready| ready.device() == device.device())
                    .then(|| device.interface::<T>())
                    .flatten()
                    .map(|interface| (device, interface))
            })
            .context("the requested EIS input interface is unavailable")?;
        action(interface);
        self.frame(device);
        Ok(())
    }

    fn frame(&self, device: &Device) {
        let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
        let micros = now.tv_sec as u64 * 1_000_000 + now.tv_nsec as u64 / 1_000;
        device.device().frame(self.connection.serial(), micros);
    }

    fn absolute(&self, screen: &Screen, x: f64, y: f64) -> Result<()> {
        screen
            .device
            .interface::<ei::PointerAbsolute>()
            .context("EIS absolute pointer unavailable")?
            .motion_absolute(x as f32, y as f32);
        self.frame(&screen.device);
        Ok(())
    }

    pub fn event(&mut self, event: Input) -> Result<Option<u16>> {
        self.local_event(event, true)
    }

    pub fn local_event(&mut self, event: Input, detect_edge: bool) -> Result<Option<u16>> {
        ensure!(
            event.valid() && !self.started.is_empty(),
            "invalid or inactive input event"
        );
        if matches!(event, Input::Key { .. } | Input::Button { .. }) {
            // Even a press and release between two motions must start a fresh
            // dwell. Duplicate key events must not preserve an armed crossing.
            self.cancel_dwell();
        }
        match event {
            Input::Key { code, down } => {
                if down == self.keys.contains(&code) {
                    return Ok(None);
                }
                self.send(|key: ei::Keyboard| {
                    key.key(
                        code.into(),
                        if down {
                            ei::keyboard::KeyState::Press
                        } else {
                            ei::keyboard::KeyState::Released
                        },
                    )
                })?;
                if down {
                    self.keys.insert(code);
                } else {
                    self.keys.remove(&code);
                }
            }
            Input::Button { code, down } => {
                if down == self.buttons.contains(&code) {
                    return Ok(None);
                }
                self.send(|button: ei::Button| {
                    button.button(
                        code.into(),
                        if down {
                            ei::button::ButtonState::Press
                        } else {
                            ei::button::ButtonState::Released
                        },
                    )
                })?;
                if down {
                    self.buttons.insert(code);
                } else {
                    self.buttons.remove(&code);
                }
            }
            Input::Scroll { dx, dy } => {
                self.send(|scroll: ei::Scroll| scroll.scroll_discrete(dx.into(), dy.into()))?;
            }
            Input::Move { dx, dy } => {
                let current = self.cursor.and_then(|(x, y)| {
                    unique_region(&self.screens, x, y).map(|index| (x, y, index))
                });
                if let Some((x, y, index)) = current {
                    let current = &self.screens[index];
                    let (dx, dy) = (f64::from(dx) / current.scale, f64::from(dy) / current.scale);
                    let unheld = detect_edge && self.keys.is_empty() && self.buttons.is_empty();
                    let blocked = if self.group {
                        let allow = unheld && self.generation == self.router.routes().activation;
                        match self.router.crossing_if(x, y, dx, dy, allow) {
                            Some(Hit::Cross(crossing)) => {
                                self.crossing = Some(crossing);
                                true
                            }
                            Some(Hit::Block) => true,
                            None => false,
                        }
                    } else {
                        if let Some(edge) = self.edge.as_ref()
                            && unheld
                            && (current.id == edge.screen
                                || edge.screen == "auto"
                                    && current.id == self.find_screen("auto")?.id)
                            && let Some(position) =
                                edge.side.crossing(current.rect.rect(), x, y, dx, dy)
                        {
                            return Ok(Some(position));
                        }
                        false
                    };
                    let next = (x + dx, y + dy);
                    let target = if blocked {
                        current
                    } else {
                        unique_region(&self.screens, next.0, next.1)
                            .map(|index| &self.screens[index])
                            .unwrap_or(current)
                    };
                    let (x, y) = target.rect.rect().clamp(next.0, next.1);
                    self.absolute(target, x, y)?;
                    self.cursor = Some((x, y));
                } else {
                    // Unknown pointer position/geometry must not stop manual
                    // input or select an arbitrary first absolute screen.
                    self.cursor = None;
                    self.send(|pointer: ei::Pointer| {
                        pointer.motion_relative(dx as f32, dy as f32)
                    })?;
                }
            }
        }
        self.connection.flush()?;
        Ok(None)
    }

    pub fn screen_ids(&self) -> Vec<String> {
        self.screens
            .iter()
            .map(|screen| screen.id.clone())
            .collect()
    }

    pub fn start_local(&mut self) -> Result<()> {
        if let Some(edge) = &self.edge {
            let mode = edge.mode;
            self.start(Some(u16::MAX / 2), mode)?;
            let screen = self.find_screen(&self.edge.as_ref().unwrap().screen)?;
            let rect = screen.rect.rect();
            let x = rect.x + rect.width / 2.0;
            let y = rect.y + rect.height / 2.0;
            self.absolute(&screen, x, y)?;
            self.cursor = Some((x, y));
            self.connection.flush()?;
        } else {
            self.start(None, Mode::Input)?;
        }
        Ok(())
    }

    pub fn enter_local(&mut self, position: u16) -> Result<()> {
        let edge = self
            .edge
            .as_ref()
            .context("the local return edge is disabled")?;
        let screen = self.find_screen(&edge.screen)?;
        let (x, y) = edge.entry(screen.rect.rect(), position);
        self.absolute(&screen, x, y)?;
        self.cursor = Some((x, y));
        self.connection.flush()?;
        Ok(())
    }

    pub fn release(&mut self) -> Result<()> {
        self.cancel_dwell();
        let mut result = Ok(());
        for code in std::mem::take(&mut self.keys) {
            result =
                result.and(self.send(|key: ei::Keyboard| {
                    key.key(code.into(), ei::keyboard::KeyState::Released)
                }));
        }
        for code in std::mem::take(&mut self.buttons) {
            result = result.and(self.send(|button: ei::Button| {
                button.button(code.into(), ei::button::ButtonState::Released)
            }));
        }
        for device in self.started.drain(..) {
            if self
                .devices
                .iter()
                .any(|ready| ready.device() == device.device())
            {
                device.device().stop_emulating(self.connection.serial());
            }
        }
        self.cursor = None;
        self.pointer_stamp = self.pointer_stamp.wrapping_add(1);
        self.pointer_dirty = false;
        self.crossing = None;
        result.and(self.connection.flush().map_err(Into::into))
    }

    pub async fn close(self) {
        if let Some(refresh) = &self.refresh {
            refresh.task.abort();
        }
        let _ = timeout(Duration::from_secs(1), self.session.close()).await;
    }
}

const RESTORE_FILE: &str = "wayland-restore-token";

fn take_restore_token(dir: &Path) -> Option<String> {
    use std::io::Read;
    let path = dir.join(RESTORE_FILE);
    let file = std::fs::File::open(&path).ok()?;
    let mut token = String::new();
    let result = file.take(4097).read_to_string(&mut token);
    let removed = std::fs::remove_file(&path).is_ok();
    (removed && result.is_ok() && valid_restore_token(&token)).then_some(token)
}

fn valid_restore_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 4096 && !token.chars().any(char::is_control)
}

fn save_restore_token(dir: &Path, token: Option<&str>) -> Result<()> {
    if let Some(token) = token.filter(|token| valid_restore_token(token)) {
        crate::config::write_atomic(&dir.join(RESTORE_FILE), token)?;
    }
    Ok(())
}

impl super::receiver::Sink for Portal {
    async fn poll(&mut self) -> Result<()> {
        Portal::poll(self).await
    }

    fn prepare(&self, edge: Option<u16>, mode: Mode) -> Result<()> {
        self.can_start(edge, mode, mode == Mode::Input)
    }

    fn activate(&mut self, edge: Option<u16>, mode: Mode) -> Result<()> {
        self.start(edge, mode)
    }

    fn prepare_display(&self, edge: Option<u16>, entry: Option<Entry>, mode: Mode) -> Result<()> {
        self.can_start_display(edge, entry, mode)
    }

    fn activate_display(
        &mut self,
        edge: Option<u16>,
        entry: Option<Entry>,
        mode: Mode,
    ) -> Result<()> {
        self.start_display(edge, entry, mode)
    }

    fn reposition(&mut self, entry: Entry) -> Result<()> {
        self.enter_display(entry)
    }

    fn set_routes(&mut self, routes: Routes) -> Result<()> {
        Portal::set_routes(self, routes)
    }

    fn set_generation(&mut self, generation: u64) {
        Portal::set_generation(self, generation);
    }

    fn take_crossing(&mut self) -> Option<Crossing> {
        Portal::take_crossing(self)
    }

    fn display_pending(&self) -> bool {
        Portal::display_pending(self)
    }

    fn event(&mut self, event: Input) -> Result<Option<u16>> {
        self.event(event)
    }

    fn release(&mut self) -> Result<()> {
        self.release()
    }
}

#[cfg(test)]
mod permission_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn portal_restore_token_is_private_rotated_and_consumed_once() {
        let dir = std::env::temp_dir().join(format!("desk-switch-portal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        save_restore_token(&dir, Some("first-permission")).unwrap();
        assert_eq!(
            std::fs::metadata(dir.join(RESTORE_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            take_restore_token(&dir).as_deref(),
            Some("first-permission")
        );
        assert!(take_restore_token(&dir).is_none());
        save_restore_token(&dir, Some("replacement-permission")).unwrap();
        assert_eq!(
            take_restore_token(&dir).as_deref(),
            Some("replacement-permission")
        );
        save_restore_token(&dir, Some("invalid\ntoken")).unwrap();
        assert!(take_restore_token(&dir).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::*;
    use crate::edge::Side;

    fn screen(id: &str, x: i32, y: i32, epoch: u64) -> NativeScreen {
        NativeScreen {
            native_id: id.into(),
            rect: ScreenRect {
                x,
                y,
                width: 1920,
                height: 1080,
            },
            epoch,
        }
    }

    #[test]
    fn eis_binding_requires_exact_unique_region_and_native_output() {
        let left = screen("DP-1", 0, 0, 7);
        let right = screen("HDMI-1", 1920, 0, 7);
        let mirrored = screen("HDMI-2", 1920, 0, 7);

        // An ambiguous secondary screen does not disable a uniquely bound one.
        assert_eq!(
            match_regions(
                &[left.rect, right.rect],
                &[left.clone(), right.clone(), mirrored]
            ),
            vec![left.clone()]
        );
        assert_eq!(
            match_regions(
                &[left.rect, right.rect, right.rect],
                &[left.clone(), right.clone()]
            ),
            vec![left.clone()]
        );
        let mut offset_region = right.rect;
        offset_region.y = 1;
        assert_eq!(
            match_regions(&[left.rect, offset_region], &[left.clone(), right]),
            vec![left]
        );
    }

    #[test]
    fn saved_entry_is_rejected_after_same_rectangle_has_new_geometry_epoch() {
        let old = screen("DP-1", 0, 0, 7);
        let current = screen("DP-1", 0, 0, 8);
        let entry = Entry {
            screen: old.rect,
            geometry: old.epoch,
            side: Side::Bottom,
            position: u16::MAX / 4,
        };
        let mapped = match_regions(&[current.rect], &[current]);
        assert!(entry.resolve(&mapped).is_err());
        let fresh = Entry {
            geometry: 8,
            ..entry
        };
        let resolved = fresh.resolve(&mapped).unwrap();
        let (x, y) = fresh.point();
        assert!(resolved.rect.rect().contains(x, y));
        assert_eq!(y, 1080.0 - 1.0 - 16.0);
    }
}
