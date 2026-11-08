//! CoreGraphics input workers. No custom cursor overlay is needed on macOS.
use crate::{
    config::{Edge, Identity, InputSharing, Key},
    edge::{Crossing, Entry, Hit, NativeScreen, Router, Routes},
    gate::{Gate, Route},
    video::{Inputs, VideoWorker},
    wire::{Input, Mode},
};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    collections::{BTreeSet, VecDeque},
    io::{BufRead, BufReader, Write},
    net::SocketAddr,
    os::fd::AsRawFd,
    process::{Child, ChildStdin, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::mpsc;

#[path = "macos_geometry.rs"]
pub(super) mod geometry;
use geometry::Screen;

struct Process {
    child: Child,
    writer: Arc<Mutex<ChildStdin>>,
}
impl Process {
    fn start(operation: &str, args: &[String]) -> Result<(Self, std::process::ChildStdout)> {
        let mut child = crate::macos_native::command(operation)?
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let writer = Arc::new(Mutex::new(
            child
                .stdin
                .take()
                .context("macOS input pipe is unavailable")?,
        ));
        // A suspended helper must never block the Rust lease or shutdown
        // path on a full pipe. Every control message fits in PIPE_BUF.
        let descriptor = writer.lock().unwrap().as_raw_fd();
        // SAFETY: the descriptor is open and the calls only read and update its
        // status flags.
        let nonblocking = unsafe {
            let flags = libc::fcntl(descriptor, libc::F_GETFL);
            flags >= 0 && libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) >= 0
        };
        if !nonblocking {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::last_os_error().into());
        }
        let output = child
            .stdout
            .take()
            .context("macOS input events are unavailable")?;
        let errors = child.stderr.take().unwrap();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut first = true;
            for line in BufReader::new(errors).lines() {
                let Ok(line) = line else { break };
                if first {
                    first = false;
                    let result = serde_json::from_str::<serde_json::Value>(&line)
                        .ok()
                        .and_then(|value| value["ready"].as_str().map(str::to_owned))
                        .ok_or_else(|| line.clone());
                    let _ = send.send(result);
                } else {
                    eprintln!("macOS input: {line}");
                }
            }
        });
        match receive.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok(label)) => println!("{label}"),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!(
                    "{}",
                    match result {
                        Ok(Err(error)) => error,
                        _ => "The macOS input helper did not become ready".into(),
                    }
                );
            }
        }
        Ok((Self { child, writer }, output))
    }
    fn command(&self, value: serde_json::Value) -> Result<()> {
        command(&self.writer, value)
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.command(serde_json::json!({"kind":"stop"}));
        // Give the native main loop time to release injected keys/buttons and
        // restore a hidden source cursor before using a forced process stop.
        let since = std::time::Instant::now();
        while since.elapsed() < std::time::Duration::from_millis(300) {
            if self.child.try_wait().is_ok_and(|status| status.is_some()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn command(writer: &Arc<Mutex<ChildStdin>>, value: serde_json::Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(&value)?;
    bytes.push(b'\n');
    ensure!(bytes.len() <= 512, "The native input command is too large");
    let mut writer = writer.lock().unwrap();
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

#[derive(Deserialize)]
struct NativeEvent {
    kind: String,
    code: Option<u16>,
    down: Option<bool>,
    dx: Option<i32>,
    dy: Option<i32>,
    remote: Option<bool>,
    position: Option<u16>,
    crossing: Option<Crossing>,
}
struct PendingEntry(Arc<Mutex<bool>>);
impl Drop for PendingEntry {
    fn drop(&mut self) {
        if let Ok(mut canceled) = self.0.lock() {
            *canceled = true;
        }
    }
}
struct Cursor(Arc<Mutex<ChildStdin>>);
impl super::host::Cursor for Cursor {
    async fn request_toggle(&self) -> Result<()> {
        command(&self.0, serde_json::json!({"kind":"toggle"}))
    }
    async fn center(&self) -> Result<()> {
        command(&self.0, serde_json::json!({"kind":"center"}))
    }
    async fn enter(&self, edge: &Edge, position: u16) -> Result<()> {
        command(
            &self.0,
            serde_json::json!({"kind":"enter","position":position,"screen":edge.screen,"side":side(if crate::lifecycle::group_input(){crate::edge::Side::Left}else{edge.side})}),
        )
    }
    async fn enter_display(&self, activation: u64, entry: Entry) -> Result<()> {
        let routes = crate::lifecycle::routes();
        let canceled = Arc::new(Mutex::new(false));
        let _pending = PendingEntry(canceled.clone());
        tokio::task::spawn_blocking(move || {
            geometry::enter_current(entry, activation, &routes, &canceled)
        })
        .await??;
        command(&self.0, serde_json::json!({"kind":"reset-entry"}))
    }
    async fn resume(&self) -> Result<()> {
        command(&self.0, serde_json::json!({"kind":"mode","value":"local"}))
    }
}

pub fn run(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    edge: Option<Edge>,
    sharing: InputSharing,
    video: Option<(Inputs, VideoWorker)>,
) -> Result<()> {
    let mut arguments = vec![sharing.keyboard.to_string(), sharing.mouse.to_string()];
    if let Some(edge) = &edge {
        arguments.push(edge.screen.clone());
        arguments.push(side(edge.side).into());
    }
    if crate::lifecycle::group_input() {
        arguments.push("--group".into());
    }
    let (mut process, output) = Process::start("input-capture", &arguments)?;
    let gate = Arc::new(Gate::new());
    let (control_send, controls) = mpsc::channel(8);
    let (event_send, events) = mpsc::channel(256);
    let reader_gate = gate.clone();
    let alive = Arc::new(AtomicBool::new(true));
    let reader_alive = alive.clone();
    std::thread::Builder::new()
        .name("Desk Switch macOS input capture".into())
        .spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                if line.len() > 1024 {
                    continue;
                }
                let Ok(event) = serde_json::from_str::<NativeEvent>(&line) else {
                    continue;
                };
                if event.kind == "emergency" {
                    reader_gate.emergency();
                    continue;
                }
                if event.kind == "boundary" {
                    let (token, route) = reader_gate.current();
                    if route == Route::Local
                        && let Some(crossing) = event.crossing
                        && control_send
                            .try_send(super::host::Control::Boundary { token, crossing })
                            .is_err()
                    {
                        reader_gate.control_overflow(token);
                    }
                    continue;
                }
                if event.kind == "edge" {
                    if let Some(position) = event.position
                        && let Some(token) = reader_gate.begin()
                        && control_send
                            .try_send(super::host::Control::Edge { token, position })
                            .is_err()
                    {
                        reader_gate.control_overflow(token);
                    }
                    continue;
                }
                if event.kind == "toggle" {
                    let (token, route) = reader_gate.current();
                    let token = match route {
                        Route::Local => reader_gate.begin(),
                        Route::Remote if reader_gate.hold(token) => Some(token),
                        _ => None,
                    };
                    if let Some(token) = token
                        && control_send
                            .try_send(super::host::Control::Toggle(token))
                            .is_err()
                    {
                        reader_gate.control_overflow(token);
                    }
                    continue;
                }
                if event.remote != Some(true) {
                    continue;
                }
                let (token, route) = reader_gate.current();
                if route != Route::Remote {
                    continue;
                }
                let input = match event.kind.as_str() {
                    "key" if sharing.keyboard => event
                        .code
                        .and_then(super::macos_keys::to_wire)
                        .zip(event.down)
                        .map(|(code, down)| Input::Key { code, down }),
                    "button" if sharing.mouse => event
                        .code
                        .filter(|code| *code <= 4)
                        .zip(event.down)
                        .map(|(code, down)| Input::Button {
                            code: 272 + code,
                            down,
                        }),
                    "move" if sharing.mouse => event
                        .dx
                        .zip(event.dy)
                        .map(|(dx, dy)| Input::Move { dx, dy }),
                    "scroll" if sharing.mouse => event.dx.zip(event.dy).and_then(|(dx, dy)| {
                        Some(Input::Scroll {
                            dx: dx.try_into().ok()?,
                            dy: dy.try_into().ok()?,
                        })
                    }),
                    _ => None,
                };
                if let Some(input) = input.filter(|input| input.valid())
                    && event_send.try_send((token, input)).is_err()
                {
                    reader_gate.input_overflow(token);
                }
            }
            reader_gate.restore_local();
            reader_alive.store(false, Ordering::Release);
        })?;
    println!(
        "macOS input enabled. F12 switches; F11 immediately restores local input. macOS may require Fn with function keys."
    );
    let result=crate::runtime()?.block_on(async {
        let coordinator=super::host::connect(address,identity,peer,super::host::Coordinator {gate:gate.clone(),edge,video,controls,events,cursor:Cursor(process.writer.clone()),monitor_required:crate::lifecycle::monitor_required});
        let lease=async {
            let mut tick=tokio::time::interval(std::time::Duration::from_millis(20));tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut routes = crate::lifecycle::routes();
            let mut policy = route_commands(&routes.borrow().clone());
            loop {
                tick.tick().await;
                ensure!(alive.load(Ordering::Acquire),"The macOS event tap stopped");
                ensure!(process.child.try_wait()?.is_none(),"The macOS input helper stopped");
                let route=gate.current().1;
                process.command(serde_json::json!({"kind":"mode","value":match route{Route::Local=>"local",Route::Hold=>"hold",Route::Remote=>"remote"}}))?;
                if routes.has_changed().unwrap_or(false) { policy = route_commands(&routes.borrow_and_update().clone()); }
                // Route records fit PIPE_BUF individually. Bounded batches
                // keep large layouts off the hot lease/network path.
                for _ in 0..8 {
                    let Some(next) = policy.front() else {break};
                    match process.command(next.clone()) {
                        Ok(()) => {policy.pop_front();}
                        Err(error) if error.downcast_ref::<std::io::Error>().is_some_and(|error| error.kind()==std::io::ErrorKind::WouldBlock) => break,
                        Err(error) => return Err(error),
                    }
                }
            }
            #[allow(unreachable_code)] Ok::<(),anyhow::Error>(())
        };
        tokio::select!{result=coordinator=>result,result=lease=>result}
    });
    gate.restore_local();
    let _ = process.command(serde_json::json!({"kind":"mode","value":"local"}));
    result
}

fn route_commands(routes: &Routes) -> VecDeque<serde_json::Value> {
    if !crate::lifecycle::group_input() {
        return VecDeque::new();
    }
    std::iter::once(serde_json::json!({"kind":"routes-start"}))
        .chain(routes.boundaries.iter().map(|boundary| serde_json::json!({"kind":"routes-add","boundary":boundary})))
        .chain(std::iter::once(serde_json::json!({"kind":"routes-commit","revision":routes.revision,"activation":routes.activation,"policy":routes.policy})))
        .collect()
}

fn side(side: crate::edge::Side) -> &'static str {
    match side {
        crate::edge::Side::Left => "left",
        crate::edge::Side::Right => "right",
        crate::edge::Side::Top => "top",
        crate::edge::Side::Bottom => "bottom",
    }
}
#[derive(Deserialize)]
struct Point {
    x: f64,
    y: f64,
}
pub struct NativeSink {
    process: Process,
    active: bool,
    keys: BTreeSet<u16>,
    buttons: BTreeSet<u16>,
    edge: Option<Edge>,
    geometry: Arc<Mutex<Vec<Screen>>>,
    position: Point,
    return_edge: bool,
    alive: Arc<AtomicBool>,
    emergency: Arc<AtomicBool>,
    lease: std::time::Instant,
    router: Router,
    native: Vec<NativeScreen>,
    generation: u64,
    crossing: Option<Crossing>,
}
impl NativeSink {
    pub fn new(edge: Option<Edge>) -> Result<Self> {
        let initial = geometry::screens()?;
        let native = geometry::validate(&initial)?;
        let geometry = Arc::new(Mutex::new(initial));
        let (process, output) = Process::start("input-playback", &[])?;
        let alive = Arc::new(AtomicBool::new(true));
        let emergency = Arc::new(AtomicBool::new(false));
        let reader_alive = alive.clone();
        let reader_emergency = emergency.clone();
        let reader_geometry = geometry.clone();
        std::thread::Builder::new()
            .name("Desk Switch macOS input receiver".into())
            .spawn(move || {
                for line in BufReader::new(output).lines() {
                    let Ok(line) = line else { break };
                    if line.len() > 64 * 1024 {
                        continue;
                    }
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                        if value["kind"] == "emergency" {
                            reader_emergency.store(true, Ordering::Release);
                        }
                        if value["kind"] == "geometry"
                            && let Ok(screens) =
                                serde_json::from_value::<Vec<Screen>>(value["screens"].clone())
                            && screens.len() <= 128
                            && geometry::validate(&screens).is_ok()
                        {
                            *reader_geometry.lock().unwrap() = screens;
                        }
                    }
                }
                reader_alive.store(false, Ordering::Release);
            })?;
        Ok(Self {
            process,
            active: false,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            edge,
            geometry,
            position: Point { x: 0., y: 0. },
            return_edge: false,
            alive,
            emergency,
            lease: std::time::Instant::now(),
            router: Router::default(),
            native,
            generation: 0,
            crossing: None,
        })
    }
    fn edge_screen(&self) -> Result<crate::edge::Rect> {
        let geometry = self.geometry.lock().unwrap();
        let edge = self
            .edge
            .as_ref()
            .context("Configure the macOS return edge")?;
        geometry
            .iter()
            .find(|screen| edge.screen == "auto" || screen.id == edge.screen)
            .map(Screen::rect)
            .context("The selected macOS return screen is unavailable")
    }
    fn refresh_geometry(&mut self) -> Result<()> {
        let geometry = self.geometry.lock().unwrap();
        if geometry.len() == self.native.len()
            && geometry.first().map(|screen| screen.epoch)
                == self.native.first().map(|screen| screen.epoch)
        {
            return Ok(());
        }
        let native = geometry::validate(&geometry)?;
        if native != self.native {
            self.native = native;
            self.crossing = None;
            self.router
                .configure(self.router.routes().clone(), &self.native)?;
        }
        Ok(())
    }
    fn position_display(&mut self, entry: Entry) -> Result<()> {
        self.refresh_geometry()?;
        entry.resolve(&self.native)?;
        let (x, y) = entry.point();
        geometry::position(x, y)?;
        self.process
            .command(serde_json::json!({"kind":"activate","x":x,"y":y,"warp":false}))?;
        self.position = Point { x, y };
        self.return_edge = false;
        self.crossing = None;
        self.router.reset_entry();
        Ok(())
    }
}
impl super::receiver::Sink for NativeSink {
    async fn poll(&mut self) -> Result<()> {
        loop {
            ensure!(
                self.alive.load(Ordering::Acquire) && self.process.child.try_wait()?.is_none(),
                "The macOS input receiver stopped"
            );
            ensure!(
                !self.emergency.swap(false, Ordering::AcqRel),
                "macOS input restored by F11 or an expired local injection lease"
            );
            if self.lease.elapsed() >= std::time::Duration::from_millis(50) {
                self.process.command(serde_json::json!({"kind":"lease"}))?;
                self.lease = std::time::Instant::now();
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
    fn prepare(&self, position: Option<u16>, mode: Mode) -> Result<()> {
        ensure!(!self.active, "macOS input is already active");
        if position.is_some() {
            ensure!(
                self.edge.as_ref().is_some_and(|edge| edge.mode == mode)
                    && self.edge_screen().is_ok(),
                "Configure a matching macOS return edge on an available screen before using edge switching"
            );
        }
        Ok(())
    }
    fn activate(&mut self, position: Option<u16>, mode: Mode) -> Result<()> {
        self.prepare(position, mode)?;
        if let Some(position) = position {
            let edge = self.edge.as_ref().unwrap();
            let (x, y) = edge.entry(self.edge_screen()?, position);
            self.position = Point { x, y };
            self.process
                .command(serde_json::json!({"kind":"activate","x":x,"y":y}))?;
        } else {
            // The existing native worker adopts the cursor in place. No
            // process launch or WindowServer query blocks network heartbeats.
            let (x, y) = geometry::cursor()?;
            self.position = Point { x, y };
            self.process
                .command(serde_json::json!({"kind":"activate","x":x,"y":y,"warp":false}))?;
        }
        self.active = true;
        self.return_edge = position.is_some();
        Ok(())
    }
    fn prepare_display(
        &self,
        position: Option<u16>,
        entry: Option<Entry>,
        mode: Mode,
    ) -> Result<()> {
        ensure!(
            !(position.is_some() && entry.is_some()),
            "A display entry cannot also use a legacy edge"
        );
        self.prepare(position, mode)?;
        if let Some(entry) = entry {
            entry.resolve(&geometry::validate(&self.geometry.lock().unwrap())?)?;
        }
        Ok(())
    }
    fn activate_display(
        &mut self,
        position: Option<u16>,
        entry: Option<Entry>,
        mode: Mode,
    ) -> Result<()> {
        self.prepare_display(position, entry, mode)?;
        if let Some(entry) = entry {
            self.position_display(entry)?;
            self.active = true;
            Ok(())
        } else {
            self.activate(position, mode)
        }
    }
    fn reposition(&mut self, entry: Entry) -> Result<()> {
        ensure!(self.active, "macOS input is not active");
        self.position_display(entry)
    }
    fn set_routes(&mut self, routes: Routes) -> Result<()> {
        self.refresh_geometry()?;
        self.crossing = None;
        self.router.configure(routes, &self.native)
    }
    fn set_generation(&mut self, generation: u64) {
        if self.generation != generation {
            self.generation = generation;
            self.crossing = None;
            self.router.reset_entry();
        }
    }
    fn take_crossing(&mut self) -> Option<Crossing> {
        self.crossing.take()
    }
    fn event(&mut self, input: Input) -> Result<Option<u16>> {
        ensure!(
            self.active && input.valid(),
            "Input arrived outside an active macOS lease"
        );
        self.refresh_geometry()?;
        if matches!(input, Input::Key { .. } | Input::Button { .. }) {
            self.router.cancel_dwell();
            self.crossing = None;
        }
        if let Input::Move { dx, dy } = input {
            let group = crate::lifecycle::group_input();
            let blocked = if group {
                let allow = self.keys.is_empty()
                    && self.buttons.is_empty()
                    && self.generation == self.router.routes().activation;
                match self.router.crossing_if(
                    self.position.x,
                    self.position.y,
                    dx.into(),
                    dy.into(),
                    allow,
                ) {
                    Some(Hit::Cross(crossing)) => {
                        self.crossing = Some(crossing);
                        true
                    }
                    Some(Hit::Block) => true,
                    None => false,
                }
            } else {
                false
            };
            if !group && self.return_edge && self.keys.is_empty() && self.buttons.is_empty() {
                let edge = self.edge.as_ref().unwrap();
                if let Some(position) = edge.side.crossing(
                    self.edge_screen()?,
                    self.position.x,
                    self.position.y,
                    dx.into(),
                    dy.into(),
                ) {
                    return Ok(Some(position));
                }
            }
            let current = self.native.iter().find(|screen| {
                screen
                    .rect
                    .rect()
                    .contains(self.position.x, self.position.y)
            });
            let next = (
                self.position.x + f64::from(dx),
                self.position.y + f64::from(dy),
            );
            let target = if blocked {
                None
            } else {
                self.native
                    .iter()
                    .find(|screen| screen.rect.rect().contains(next.0, next.1))
            };
            if let Some(target) = target.or(current) {
                let rect = target.rect.rect();
                self.position = Point {
                    x: next.0.clamp(rect.x, rect.right() - 1.),
                    y: next.1.clamp(rect.y, rect.bottom() - 1.),
                };
            } else {
                self.position = Point {
                    x: next.0,
                    y: next.1,
                };
            }
        }
        let value = match input {
            Input::Key { code, down } => {
                let native = super::macos_keys::from_wire(code)
                    .with_context(|| format!("macOS cannot inject physical key code {code}"))?;
                if down {
                    self.keys.insert(code);
                } else {
                    self.keys.remove(&code);
                }
                serde_json::json!({"kind":"key","code":native,"down":down})
            }
            Input::Button { code, down } => {
                if down {
                    self.buttons.insert(code);
                } else {
                    self.buttons.remove(&code);
                }
                serde_json::json!({"kind":"button","code":code-272,"down":down})
            }
            Input::Move { .. } => {
                serde_json::json!({"kind":"move","x":self.position.x,"y":self.position.y})
            }
            Input::Scroll { dx, dy } => serde_json::json!({"kind":"scroll","dx":dx,"dy":dy}),
        };
        self.process.command(value)?;
        Ok(None)
    }
    fn release(&mut self) -> Result<()> {
        self.router.cancel_dwell();
        self.active = false;
        self.return_edge = false;
        self.crossing = None;
        self.keys.clear();
        self.buttons.clear();
        self.process.command(serde_json::json!({"kind":"release"}))
    }
}
impl Drop for NativeSink {
    fn drop(&mut self) {
        let _ = super::receiver::Sink::release(self);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn largest_boundary_control_fits_an_atomic_pipe_write() {
        let boundary = crate::edge::Boundary {
            id: u64::MAX,
            geometry: u64::MAX,
            screen: crate::edge::ScreenRect {
                x: i32::MIN,
                y: i32::MIN,
                width: u32::MAX,
                height: u32::MAX,
            },
            side: crate::edge::Side::Bottom,
            start: u16::MAX - 1,
            end: u16::MAX,
        };
        boundary.validate().unwrap();
        let value = serde_json::json!({"kind":"routes-add","boundary":boundary});
        assert!(serde_json::to_vec(&value).unwrap().len() < 512);
        assert!(serde_json::to_vec(&serde_json::json!({"kind":"routes-commit","revision":u64::MAX,"activation":u64::MAX,"policy":crate::edge::Policy {dwell_ms:1000,protect_corners:true,paused:true}})).unwrap().len()<512);
    }
}
