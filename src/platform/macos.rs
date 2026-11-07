//! CoreGraphics input workers. No custom cursor overlay is needed on macOS.
use crate::{
    config::{Edge, Identity, InputSharing, Key},
    gate::{Gate, Route},
    video::{Inputs, VideoWorker},
    wire::{Input, Mode},
};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
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
            reader_gate.emergency();
            reader_alive.store(false, Ordering::Release);
        })?;
    println!(
        "macOS input enabled. F12 switches; F11 immediately restores local input. macOS may require Fn with function keys."
    );
    let result=crate::runtime()?.block_on(async {
        let coordinator=super::host::connect(address,identity,peer,super::host::Coordinator {gate:gate.clone(),edge,video,controls,events,cursor:Cursor(process.writer.clone()),monitor_required:crate::lifecycle::monitor_required});
        let lease=async {
            let mut tick=tokio::time::interval(std::time::Duration::from_millis(20));tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {tick.tick().await;ensure!(alive.load(Ordering::Acquire),"The macOS event tap stopped");ensure!(process.child.try_wait()?.is_none(),"The macOS input helper stopped");let route=gate.current().1;process.command(serde_json::json!({"kind":"mode","value":match route{Route::Local=>"local",Route::Hold=>"hold",Route::Remote=>"remote"}}))?;}
            #[allow(unreachable_code)] Ok::<(),anyhow::Error>(())
        };
        tokio::select!{result=coordinator=>result,result=lease=>result}
    });
    gate.emergency();
    let _ = process.command(serde_json::json!({"kind":"mode","value":"local"}));
    result
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
struct Screen {
    id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}
impl Screen {
    fn rect(&self) -> crate::edge::Rect {
        crate::edge::Rect {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
}
#[derive(Deserialize)]
struct Point {
    x: f64,
    y: f64,
}
fn screens() -> Result<Vec<Screen>> {
    crate::macos_native::query("screen-geometry", &[])
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
}
impl NativeSink {
    pub fn new(edge: Option<Edge>) -> Result<Self> {
        let geometry = Arc::new(Mutex::new(screens()?));
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
                            && screens.iter().all(|screen| {
                                screen.x.is_finite()
                                    && screen.y.is_finite()
                                    && screen.width.is_finite()
                                    && screen.height.is_finite()
                                    && screen.width > 0.
                                    && screen.height > 0.
                            })
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
            self.process
                .command(serde_json::json!({"kind":"activate"}))?;
        }
        self.active = true;
        self.return_edge = position.is_some();
        Ok(())
    }
    fn event(&mut self, input: Input) -> Result<Option<u16>> {
        ensure!(
            self.active && input.valid(),
            "Input arrived outside an active macOS lease"
        );
        if let Input::Move { dx, dy } = input {
            if self.return_edge && self.keys.is_empty() && self.buttons.is_empty() {
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
            self.position.x += f64::from(dx);
            self.position.y += f64::from(dy);
            let geometry = self.geometry.lock().unwrap();
            let min_x = geometry
                .iter()
                .map(|screen| screen.x)
                .fold(f64::INFINITY, f64::min);
            let min_y = geometry
                .iter()
                .map(|screen| screen.y)
                .fold(f64::INFINITY, f64::min);
            let max_x = geometry
                .iter()
                .map(|screen| screen.x + screen.width)
                .fold(f64::NEG_INFINITY, f64::max);
            let max_y = geometry
                .iter()
                .map(|screen| screen.y + screen.height)
                .fold(f64::NEG_INFINITY, f64::max);
            if min_x < max_x && min_y < max_y {
                self.position.x = self.position.x.clamp(min_x, max_x - 1.);
                self.position.y = self.position.y.clamp(min_y, max_y - 1.);
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
            Input::Move { dx, dy } => serde_json::json!({"kind":"move","dx":dx,"dy":dy}),
            Input::Scroll { dx, dy } => serde_json::json!({"kind":"scroll","dx":dx,"dy":dy}),
        };
        self.process.command(value)?;
        Ok(None)
    }
    fn release(&mut self) -> Result<()> {
        self.active = false;
        self.return_edge = false;
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
