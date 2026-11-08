use super::{receiver::Sink, win_input, win_monitor};
use crate::{
    config::Edge,
    edge::{Crossing, Entry, Hit, Router, Routes},
    keys::windows_key,
    wire::{Input, Mode},
};
use anyhow::{Context, Result, ensure};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    mem::size_of,
    ptr::{null, null_mut},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::{
        HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
        Input::KeyboardAndMouse::*,
        WindowsAndMessaging::*,
    },
};

const MOTION_TAG: usize = 0xd530_0000;
const MOTION_MASK: usize = 0xfff0_0000;
const PENDING_MOTION: usize = 256;

#[derive(Clone, Copy)]
struct MotionIntent {
    tag: usize,
    generation: u64,
    dx: i32,
    dy: i32,
}

/// OS-ordered cursor coordinates, rather than a prediction from network deltas.
/// All changes are in memory; the low-level hooks never wait for geometry I/O.
struct MotionState {
    router: Router,
    generation: u64,
    active: bool,
    point: (i32, i32),
    keys: [bool; 256],
    buttons: [bool; 5],
    crossing: Option<Crossing>,
    serial: usize,
    pending: [Option<MotionIntent>; PENDING_MOTION],
}

impl MotionState {
    fn new(point: (i32, i32), keys: [bool; 256], buttons: [bool; 5]) -> Self {
        Self {
            router: Router::default(),
            generation: 0,
            active: false,
            point,
            keys,
            buttons,
            crossing: None,
            serial: 0,
            pending: [None; PENDING_MOTION],
        }
    }

    fn register(&mut self, dx: i32, dy: i32) -> Result<usize> {
        self.serial = self.serial.wrapping_add(1) & !MOTION_MASK;
        let tag = MOTION_TAG | self.serial;
        let slot = &mut self.pending[self.serial % PENDING_MOTION];
        ensure!(slot.is_none(), "Windows native motion queue is full");
        *slot = Some(MotionIntent {
            tag,
            generation: self.generation,
            dx,
            dy,
        });
        Ok(tag)
    }

    fn cancel(&mut self, tag: usize) {
        let slot = &mut self.pending[(tag & !MOTION_MASK) % PENDING_MOTION];
        if slot.is_some_and(|intent| intent.tag == tag) {
            *slot = None;
        }
    }

    fn mouse(&mut self, message: u32, data: &MSLLHOOKSTRUCT) -> bool {
        let high = (data.mouseData >> 16) as u16;
        let button = match message {
            WM_LBUTTONDOWN | WM_LBUTTONUP => Some((0, message == WM_LBUTTONDOWN)),
            WM_RBUTTONDOWN | WM_RBUTTONUP => Some((1, message == WM_RBUTTONDOWN)),
            WM_MBUTTONDOWN | WM_MBUTTONUP => Some((2, message == WM_MBUTTONDOWN)),
            WM_XBUTTONDOWN | WM_XBUTTONUP => Some((
                if high == XBUTTON1 { 3 } else { 4 },
                message == WM_XBUTTONDOWN,
            )),
            _ => None,
        };
        if let Some((button, down)) = button {
            self.buttons[button] = down;
        }
        if message != WM_MOUSEMOVE {
            return false;
        }
        let target = (data.pt.x, data.pt.y);
        if data.flags & LLMHF_INJECTED == 0 || data.dwExtraInfo & MOTION_MASK != MOTION_TAG {
            self.point = target;
            return false;
        }
        let slot = &mut self.pending[(data.dwExtraInfo & !MOTION_MASK) % PENDING_MOTION];
        let Some(intent) = slot.filter(|intent| intent.tag == data.dwExtraInfo) else {
            self.point = target;
            return false;
        };
        *slot = None;
        self.actual_motion(intent, target)
    }

    fn actual_motion(&mut self, intent: MotionIntent, target: (i32, i32)) -> bool {
        if !self.active || intent.generation != self.generation {
            return true;
        }
        let (x, y) = (f64::from(self.point.0), f64::from(self.point.1));
        let (actual_x, actual_y) = (f64::from(target.0) - x, f64::from(target.1) - y);
        // At a physical outer edge Windows clamps the actual point. Retain only
        // that axis's intent so a zero-distance native event can still exit.
        let clamped = |side| {
            self.router.routes().boundaries.iter().any(|boundary| {
                if boundary.side != side || !boundary.screen.rect().contains(x, y) {
                    return false;
                }
                let screen = boundary.screen.rect();
                match side {
                    crate::edge::Side::Left => x == screen.x,
                    crate::edge::Side::Right => x == screen.right() - 1.0,
                    crate::edge::Side::Top => y == screen.y,
                    crate::edge::Side::Bottom => y == screen.bottom() - 1.0,
                }
            })
        };
        let dx = if actual_x == 0.0
            && intent.dx != 0
            && clamped(if intent.dx < 0 {
                crate::edge::Side::Left
            } else {
                crate::edge::Side::Right
            }) {
            f64::from(intent.dx)
        } else {
            actual_x
        };
        let dy = if actual_y == 0.0
            && intent.dy != 0
            && clamped(if intent.dy < 0 {
                crate::edge::Side::Top
            } else {
                crate::edge::Side::Bottom
            }) {
            f64::from(intent.dy)
        } else {
            actual_y
        };
        let allow = !self.keys.iter().any(|held| *held)
            && !self.buttons.iter().any(|held| *held)
            && self.router.routes().activation == self.generation;
        match self.router.crossing_if(x, y, dx, dy, allow) {
            Some(Hit::Cross(crossing)) => {
                self.crossing = Some(crossing);
                true
            }
            Some(Hit::Block) => true,
            None => {
                self.point = target;
                false
            }
        }
    }
}

struct HookState {
    state: Arc<Mutex<MotionState>>,
    changed: tokio::sync::watch::Sender<u64>,
}
thread_local! { static MOTION_HOOK: RefCell<Option<HookState>> = const {RefCell::new(None)}; }

unsafe extern "system" fn native_motion_hook(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    let blocked = code >= 0
        && MOTION_HOOK.with(|hook| {
            let hook = hook.borrow();
            let Some(hook) = hook.as_ref() else {
                return false;
            };
            let (blocked, crossed) = {
                let mut state = hook
                    .state
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                let before = state.crossing;
                let blocked =
                    state.mouse(message as u32, unsafe { &*(data as *const MSLLHOOKSTRUCT) });
                (blocked, state.crossing != before)
            };
            if crossed {
                hook.changed
                    .send_modify(|value| *value = value.wrapping_add(1));
            }
            blocked
        });
    if blocked {
        1
    } else {
        unsafe { CallNextHookEx(null_mut(), code, message, data) }
    }
}

unsafe extern "system" fn native_key_hook(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code >= 0 {
        let data = unsafe { &*(data as *const KBDLLHOOKSTRUCT) };
        MOTION_HOOK.with(|hook| {
            if let Some(hook) = hook.borrow().as_ref() {
                let mut state = hook
                    .state
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                if let Some(held) = state.keys.get_mut(data.vkCode as usize) {
                    *held = data.flags & LLKHF_UP == 0;
                }
            }
        });
    }
    unsafe { CallNextHookEx(null_mut(), code, message, data) }
}

struct MotionGuard {
    state: Arc<Mutex<MotionState>>,
    changed: tokio::sync::watch::Receiver<u64>,
    thread: u32,
    running: Arc<AtomicBool>,
}

impl MotionGuard {
    fn start() -> Result<Self> {
        let point = win_input::cursor_position()?;
        let mut keys = [false; 256];
        for (code, held) in keys.iter_mut().enumerate() {
            // Low-level callbacks distinguish the left/right modifiers.
            if ![
                VK_LBUTTON,
                VK_RBUTTON,
                VK_MBUTTON,
                VK_XBUTTON1,
                VK_XBUTTON2,
                VK_SHIFT,
                VK_CONTROL,
                VK_MENU,
            ]
            .map(usize::from)
            .contains(&code)
            {
                *held = unsafe { GetAsyncKeyState(code as i32) } < 0;
            }
        }
        let buttons = [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON, VK_XBUTTON1, VK_XBUTTON2]
            .map(|code| unsafe { GetAsyncKeyState(i32::from(code)) } < 0);
        let state = Arc::new(Mutex::new(MotionState::new(point, keys, buttons)));
        let worker_state = Arc::clone(&state);
        let running = Arc::new(AtomicBool::new(false));
        let worker_running = Arc::clone(&running);
        let (changed, receiver) = tokio::sync::watch::channel(0u64);
        let (ready, wait) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("Desk Switch native motion guard".into())
            .spawn(move || {
                let setup = (|| -> Result<_> {
                    let coordinates = win_monitor::PhysicalCoordinates::enter()?;
                    let mut message = MSG::default();
                    unsafe {
                        PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
                    }
                    MOTION_HOOK.with(|hook| {
                        *hook.borrow_mut() = Some(HookState {
                            state: worker_state,
                            changed,
                        })
                    });
                    let module = unsafe { GetModuleHandleW(null()) };
                    let mouse = unsafe {
                        SetWindowsHookExW(WH_MOUSE_LL, Some(native_motion_hook), module, 0)
                    };
                    ensure!(
                        !mouse.is_null(),
                        "cannot install Windows native motion guard"
                    );
                    let keyboard = unsafe {
                        SetWindowsHookExW(WH_KEYBOARD_LL, Some(native_key_hook), module, 0)
                    };
                    if keyboard.is_null() {
                        unsafe {
                            UnhookWindowsHookEx(mouse);
                        }
                        anyhow::bail!("cannot observe held Windows inputs for display edges");
                    }
                    Ok((coordinates, mouse, keyboard))
                })();
                match setup {
                    Ok((_coordinates, mouse, keyboard)) => {
                        worker_running.store(true, Ordering::Release);
                        if ready.send(Ok(unsafe { GetCurrentThreadId() })).is_ok() {
                            let mut message = MSG::default();
                            while unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0 {}
                        }
                        worker_running.store(false, Ordering::Release);
                        unsafe {
                            UnhookWindowsHookEx(mouse);
                            UnhookWindowsHookEx(keyboard);
                        }
                    }
                    Err(error) => {
                        let _ = ready.send(Err(format!("{error:#}")));
                    }
                }
                MOTION_HOOK.with(|hook| {
                    hook.borrow_mut().take();
                });
            })?;
        let thread = wait
            .recv_timeout(Duration::from_secs(3))
            .context("Windows motion guard did not start")?
            .map_err(|error| anyhow::anyhow!(error))?;
        Ok(Self {
            state,
            changed: receiver,
            thread,
            running,
        })
    }

    fn configure(&self, routes: Routes, screens: &[crate::edge::NativeScreen]) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.router.configure(routes, screens)?;
        state.crossing = None;
        Ok(())
    }

    fn active(&self, active: bool) -> Result<()> {
        let point = if active {
            Some(win_input::cursor_position()?)
        } else {
            None
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.active = active;
        if let Some(point) = point {
            state.point = point;
        }
        state.crossing = None;
        state.router.reset_entry();
        Ok(())
    }

    fn send_move(&self, dx: i32, dy: i32) -> Result<()> {
        ensure!(
            self.running.load(Ordering::Acquire),
            "Windows native motion guard stopped"
        );
        let tag = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .register(dx, dy)?;
        let mut input = encode(Input::Move { dx, dy })?;
        input.Anonymous.mi.dwExtraInfo = tag;
        let result = send(input);
        if result.is_err() {
            self.state
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .cancel(tag);
        }
        result
    }
}

impl Drop for MotionGuard {
    fn drop(&mut self) {
        let _ = self.active(false);
        unsafe {
            PostThreadMessageW(self.thread, WM_QUIT, 0, 0);
        }
    }
}

pub struct NativeSink {
    active: bool,
    edge: Option<Edge>,
    return_edge: bool,
    keys: BTreeSet<u16>,
    buttons: BTreeSet<u16>,
    inject: fn(Input) -> Result<()>,
    pointer: Option<super::win_cursor::Overlay>,
    pointer_failed: bool,
    cursor_setting: tokio::sync::watch::Receiver<bool>,
    group_routes: bool,
    router: Router,
    generation: u64,
    crossing: Option<Crossing>,
    geometry_setting: Option<tokio::sync::watch::Receiver<u64>>,
    motion: Option<MotionGuard>,
}

impl NativeSink {
    pub fn new(mut edge: Option<Edge>) -> Result<Self> {
        // Preserve physical coordinates for the receiver's cursor overlay too.
        // Scoped native operations also work when process awareness was set earlier.
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        if let Some(edge) = &mut edge
            && edge.screen == "auto"
        {
            edge.screen = win_monitor::screens()?
                .into_iter()
                .next()
                .context("No screen is available")?
                .name;
        }
        let group_routes = crate::lifecycle::group_input();
        let motion = if group_routes {
            match MotionGuard::start() {
                Ok(motion) => Some(motion),
                Err(error) => {
                    eprintln!("Windows automatic display edges disabled: {error:#}");
                    None
                }
            }
        } else {
            None
        };
        Ok(Self {
            active: false,
            edge,
            return_edge: false,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            inject,
            pointer: None,
            pointer_failed: false,
            cursor_setting: crate::lifecycle::remote_cursor(),
            group_routes,
            router: Router::default(),
            generation: 0,
            crossing: None,
            geometry_setting: match win_monitor::display_changes() {
                Ok(changes) => Some(changes),
                Err(error) => {
                    eprintln!("Windows automatic display edges disabled: {error:#}");
                    None
                }
            },
            motion,
        })
    }

    fn refresh_routes(&mut self, routes: Routes) -> Result<()> {
        routes.validate()?;
        let screens = if self.geometry_setting.is_some() {
            match win_monitor::native_screens() {
                Ok(screens) => screens,
                Err(error) => {
                    eprintln!("Windows display edges unavailable: {error:#}");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        if let Some(motion) = &self.motion {
            motion.configure(routes.clone(), &screens)?;
        }
        self.router.configure(routes, &screens)
    }

    fn validate_edge(&self, position: Option<u16>, mode: Mode, check_screen: bool) -> Result<()> {
        if position.is_some() {
            let edge = self
                .edge
                .as_ref()
                .context("configure the receiver's return edge")?;
            ensure!(
                edge.mode == mode,
                "edge modes differ between host and receiver"
            );
            if check_screen {
                ensure!(
                    win_monitor::screens()?
                        .iter()
                        .filter(|screen| screen.name.eq_ignore_ascii_case(&edge.screen))
                        .count()
                        == 1,
                    "the selected Windows return screen is unavailable or ambiguous"
                );
            }
        }
        Ok(())
    }

    fn receiving_mouse(&mut self) {
        // Unit tests use an injected sink and must never create a native window.
        if cfg!(test) || !*self.cursor_setting.borrow() {
            return;
        }
        if self.pointer.is_none() && !self.pointer_failed {
            match super::win_cursor::Overlay::start() {
                Ok(pointer) => self.pointer = Some(pointer),
                Err(error) => {
                    self.pointer_failed = true;
                    eprintln!("Windows pointer fallback could not start: {error:#}");
                }
            }
        }
        if let Some(pointer) = &self.pointer {
            pointer.active(true);
            pointer.refresh();
        }
    }
}

fn encode(event: Input) -> Result<INPUT> {
    let mut input = INPUT::default();
    match event {
        Input::Key { code, down } => {
            let (scan, extended, virtual_key) =
                windows_key(code).context("unsupported Windows physical key code")?;
            let mut flags = if virtual_key { 0 } else { KEYEVENTF_SCANCODE };
            if extended {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            if !down {
                flags |= KEYEVENTF_KEYUP;
            }
            input.r#type = INPUT_KEYBOARD;
            input.Anonymous.ki = KEYBDINPUT {
                wVk: if virtual_key { scan } else { 0 },
                wScan: if virtual_key { 0 } else { scan },
                dwFlags: flags,
                ..Default::default()
            };
        }
        event => {
            let mut mouse = MOUSEINPUT::default();
            match event {
                Input::Move { dx, dy } => {
                    // SendInput queues motion. Reading GetCursorPos for every delta
                    // can repeatedly observe the same old position and lose distance.
                    // Native relative motion preserves the complete input stream and
                    // uses this PC's normal pointer speed and acceleration settings.
                    mouse.dx = dx;
                    mouse.dy = dy;
                    mouse.dwFlags = MOUSEEVENTF_MOVE;
                }
                Input::Button { code, down } => {
                    mouse.dwFlags = match (code, down) {
                        (272, true) => MOUSEEVENTF_LEFTDOWN,
                        (272, false) => MOUSEEVENTF_LEFTUP,
                        (273, true) => MOUSEEVENTF_RIGHTDOWN,
                        (273, false) => MOUSEEVENTF_RIGHTUP,
                        (274, true) => MOUSEEVENTF_MIDDLEDOWN,
                        (274, false) => MOUSEEVENTF_MIDDLEUP,
                        (275 | 276, true) => MOUSEEVENTF_XDOWN,
                        (275 | 276, false) => MOUSEEVENTF_XUP,
                        _ => anyhow::bail!("unsupported Windows mouse button"),
                    };
                    if code >= 275 {
                        mouse.mouseData = if code == 275 { 1 } else { 2 };
                    }
                }
                Input::Scroll { dx, dy } => {
                    ensure!(dx == 0 || dy == 0, "encode each scroll axis separately");
                    if dy != 0 {
                        mouse.dwFlags = MOUSEEVENTF_WHEEL;
                        mouse.mouseData = (-i32::from(dy)) as u32;
                    } else {
                        mouse.dwFlags = MOUSEEVENTF_HWHEEL;
                        mouse.mouseData = i32::from(dx) as u32;
                    }
                }
                _ => unreachable!(),
            }
            input.r#type = INPUT_MOUSE;
            input.Anonymous.mi = mouse;
        }
    }
    Ok(input)
}

fn inject(event: Input) -> Result<()> {
    if let Input::Scroll { dx, dy } = event
        && dx != 0
        && dy != 0
    {
        inject(Input::Scroll { dx, dy: 0 })?;
        return inject(Input::Scroll { dx: 0, dy });
    }
    let input = encode(event)?;
    send(input)
}

fn send(input: INPUT) -> Result<()> {
    ensure!(
        unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) } == 1,
        "Windows rejected SendInput; check the interactive desktop and application privileges"
    );
    Ok(())
}

impl Sink for NativeSink {
    async fn poll(&mut self) -> Result<()> {
        tokio::select! {
            changed = self.cursor_setting.changed() => {
                changed.context("cursor setting channel closed")?;
                let enabled = *self.cursor_setting.borrow_and_update();
                if !enabled {
                    self.pointer = None;
                    self.pointer_failed = false;
                } else if self.active {
                    self.receiving_mouse();
                }
            }
            changed = async {
                match self.geometry_setting.as_mut() {
                    Some(changes) => changes.changed().await,
                    None => std::future::pending().await,
                }
            } => {
                if changed.is_err() {
                    self.geometry_setting = None;
                }
                self.crossing = None;
                self.refresh_routes(self.router.routes().clone())?;
            }
            changed = async {
                match self.motion.as_mut() {
                    Some(motion) => motion.changed.changed().await,
                    None => std::future::pending().await,
                }
            } => {
                if changed.is_err() {
                    self.motion = None;
                    eprintln!("Windows automatic display edges disabled: native motion guard stopped");
                }
            }
        }
        Ok(())
    }

    fn prepare(&self, edge: Option<u16>, mode: Mode) -> Result<()> {
        ensure!(!self.active, "Windows input is already active");
        self.validate_edge(edge, mode, mode == Mode::Input)
    }

    fn activate(&mut self, position: Option<u16>, mode: Mode) -> Result<()> {
        self.validate_edge(position, mode, true)?;
        if let Some(position) = position {
            win_input::enter(self.edge.as_ref().unwrap(), position)?;
        }
        self.return_edge = position.is_some();
        self.active = true;
        self.pointer_failed = false;
        self.router.reset_entry();
        if let Some(motion) = &self.motion {
            motion.active(true)?;
        }
        Ok(())
    }

    fn prepare_display(&self, edge: Option<u16>, entry: Option<Entry>, mode: Mode) -> Result<()> {
        ensure!(!self.active, "Windows input is already active");
        ensure!(
            edge.is_none() || entry.is_none(),
            "mixed Windows display entry modes"
        );
        if let Some(entry) = entry {
            entry.resolve(&win_monitor::native_screens()?)?;
            Ok(())
        } else {
            self.validate_edge(edge, mode, mode == Mode::Input)
        }
    }

    fn activate_display(
        &mut self,
        edge: Option<u16>,
        entry: Option<Entry>,
        mode: Mode,
    ) -> Result<()> {
        if let Some(entry) = entry {
            ensure!(edge.is_none(), "mixed Windows display entry modes");
            win_input::enter_display(entry)?;
            self.return_edge = false;
            self.active = true;
            self.pointer_failed = false;
            self.crossing = None;
            self.router.reset_entry();
            if let Some(motion) = &self.motion {
                motion.active(true)?;
            }
            Ok(())
        } else {
            self.activate(edge, mode)
        }
    }

    fn reposition(&mut self, entry: Entry) -> Result<()> {
        ensure!(self.active, "Windows input is not active");
        win_input::enter_display(entry)?;
        self.crossing = None;
        self.router.reset_entry();
        if let Some(motion) = &self.motion {
            motion.active(true)?;
        }
        if let Some(pointer) = &self.pointer {
            pointer.refresh();
        }
        Ok(())
    }

    fn set_routes(&mut self, routes: Routes) -> Result<()> {
        self.crossing = None;
        self.refresh_routes(routes)
    }

    fn set_generation(&mut self, generation: u64) {
        self.generation = generation;
        self.crossing = None;
        if let Some(motion) = &self.motion {
            let mut state = motion
                .state
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            state.generation = generation;
            state.crossing = None;
        }
    }

    fn take_crossing(&mut self) -> Option<Crossing> {
        self.crossing.take().or_else(|| {
            self.motion.as_ref().and_then(|motion| {
                motion
                    .state
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .crossing
                    .take()
            })
        })
    }

    fn event(&mut self, event: Input) -> Result<Option<u16>> {
        ensure!(
            self.active && event.valid(),
            "invalid or inactive Windows input event"
        );
        if let Input::Move { dx, dy } = event
            && !self.group_routes
            && self.return_edge
            && self.keys.is_empty()
            && self.buttons.is_empty()
        {
            let edge = self.edge.as_ref().unwrap();
            let screens = win_monitor::screens()?;
            let screen = screens
                .iter()
                .find(|screen| screen.name.eq_ignore_ascii_case(&edge.screen))
                .context("the Windows return screen disappeared")?;
            let (x, y) = win_input::cursor_position()?;
            if let Some(position) =
                edge.side
                    .crossing(screen.rect, x.into(), y.into(), dx.into(), dy.into())
            {
                return Ok(Some(position));
            }
        }
        match event {
            Input::Key { code, down } => {
                if down == self.keys.contains(&code) {
                    return Ok(None);
                }
                (self.inject)(event)?;
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
                (self.inject)(event)?;
                if down {
                    self.buttons.insert(code);
                } else {
                    self.buttons.remove(&code);
                }
            }
            Input::Move { dx, dy } if self.group_routes && self.motion.is_some() => {
                self.motion.as_ref().unwrap().send_move(dx, dy)?;
            }
            _ => (self.inject)(event)?,
        }
        if !matches!(event, Input::Key { .. }) {
            self.receiving_mouse();
        }
        Ok(None)
    }

    fn release(&mut self) -> Result<()> {
        if let Some(motion) = &self.motion {
            motion.active(false)?;
        }
        if let Some(pointer) = &self.pointer {
            pointer.active(false);
        }
        let mut result = Ok(());
        for code in std::mem::take(&mut self.keys) {
            result = result.and((self.inject)(Input::Key { code, down: false }));
        }
        for code in std::mem::take(&mut self.buttons) {
            result = result.and((self.inject)(Input::Button { code, down: false }));
        }
        self.active = false;
        self.return_edge = false;
        self.crossing = None;
        self.router.reset_entry();
        result
    }
}

impl Drop for NativeSink {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::{Boundary, NativeScreen, ScreenRect, Side};
    use std::sync::Mutex;

    fn routed_state() -> MotionState {
        let screen = NativeScreen {
            native_id: "test-display".into(),
            rect: ScreenRect {
                x: -1920,
                y: -200,
                width: 1920,
                height: 1080,
            },
            epoch: 41,
        };
        let routes = Routes {
            revision: 7,
            activation: 9,
            boundaries: vec![
                Boundary {
                    id: 1,
                    screen: screen.rect,
                    geometry: screen.epoch,
                    side: Side::Top,
                    start: 0,
                    end: u16::MAX,
                },
                Boundary {
                    id: 2,
                    screen: screen.rect,
                    geometry: screen.epoch,
                    side: Side::Right,
                    start: 0,
                    end: u16::MAX,
                },
            ],
        };
        let mut router = Router::default();
        router.configure(routes, &[screen]).unwrap();
        let mut state = MotionState::new((-4, 300), [false; 256], [false; 5]);
        state.router = router;
        state.active = true;
        state.generation = 9;
        state
    }

    fn native_move(state: &mut MotionState, dx: i32, dy: i32, actual: (i32, i32)) -> bool {
        let tag = state.register(dx, dy).unwrap();
        state.mouse(
            WM_MOUSEMOVE,
            &MSLLHOOKSTRUCT {
                pt: windows_sys::Win32::Foundation::POINT {
                    x: actual.0,
                    y: actual.1,
                },
                flags: LLMHF_INJECTED,
                dwExtraInfo: tag,
                ..Default::default()
            },
        )
    }

    #[test]
    fn typed_edges_work_without_legacy_edge_entry_and_wait_for_held_inputs() {
        let mut state = routed_state();
        state.keys[30] = true;
        assert!(native_move(&mut state, 2, 0, (0, 300)));
        assert!(state.crossing.take().is_none());
        state.keys[30] = false;
        state.buttons[0] = true;
        assert!(native_move(&mut state, 2, 0, (0, 300)));
        assert!(state.crossing.take().is_none());
        state.buttons[0] = false;
        assert!(native_move(&mut state, 2, 0, (0, 300)));
        let crossed = state.crossing.take().unwrap();
        assert_eq!(
            (crossed.revision, crossed.activation, crossed.boundary),
            (7, 9, 2)
        );
        assert!(state.active);
        // One hit waits for the controller; continuing outward cannot enqueue
        // several activations, and moving inward remains native.
        assert!(native_move(&mut state, 2, 0, (0, 300)));
        assert!(state.crossing.take().is_none());
        assert!(!native_move(&mut state, -20, 0, (-24, 300)));
    }

    #[test]
    fn stale_activation_blocks_crossing_until_matching_generation_arrives() {
        let mut state = routed_state();
        state.point = (-900, -200);
        state.generation = 8;
        assert!(native_move(&mut state, 0, -3, (-900, -204)));
        assert!(state.crossing.take().is_none());
        state.generation = 9;
        assert!(native_move(&mut state, 0, -3, (-900, -204)));
        assert_eq!(state.crossing.take().unwrap().boundary, 1);
    }

    #[test]
    fn accelerated_native_motion_is_guarded_before_entering_an_inactive_output() {
        let mut state = routed_state();
        // The raw2px delta would stop at-2, inside the ordinary screen. The OS
        // actually accelerated it to4px, beyond its last pixel(-1).
        assert!(
            state
                .router
                .crossing_if(-4.0, 300.0, 2.0, 0.0, true)
                .is_none()
        );
        assert!(native_move(&mut state, 2, 0, (0, 300)));
        assert_eq!(state.crossing.unwrap().boundary, 2);
        assert_eq!(state.point, (-4, 300));
    }

    #[test]
    fn queued_native_positions_preserve_motion_until_the_actual_boundary() {
        let mut state = routed_state();
        state.point = (-12, 300);
        let tags = (0..3)
            .map(|_| state.register(2, 0).unwrap())
            .collect::<Vec<_>>();
        for (index, actual) in [(-8, 300), (-4, 300), (0, 300)].into_iter().enumerate() {
            let blocked = state.mouse(
                WM_MOUSEMOVE,
                &MSLLHOOKSTRUCT {
                    pt: windows_sys::Win32::Foundation::POINT {
                        x: actual.0,
                        y: actual.1,
                    },
                    flags: LLMHF_INJECTED,
                    dwExtraInfo: tags[index],
                    ..Default::default()
                },
            );
            assert_eq!(blocked, index == 2);
        }
        assert_eq!(state.point, (-4, 300));
        assert_eq!(state.crossing.unwrap().boundary, 2);
        assert!(state.pending.iter().all(Option::is_none));
    }

    #[test]
    fn clamped_outer_edge_still_crosses_and_old_queued_activation_cannot_move() {
        let mut state = routed_state();
        state.point = (-1, 300);
        assert!(native_move(&mut state, 2, 0, (-1, 300)));
        assert!(state.crossing.take().is_some());
        let stale = state.register(-2, 0).unwrap();
        state.generation += 1;
        assert!(state.mouse(
            WM_MOUSEMOVE,
            &MSLLHOOKSTRUCT {
                pt: windows_sys::Win32::Foundation::POINT { x: -5, y: 300 },
                flags: LLMHF_INJECTED,
                dwExtraInfo: stale,
                ..Default::default()
            }
        ));
        assert_eq!(state.point, (-1, 300));
    }

    #[test]
    fn subpixel_native_motion_does_not_predict_a_crossing_before_the_real_edge() {
        let mut state = routed_state();
        state.point = (-2, 300);
        assert!(!native_move(&mut state, 2, 0, (-2, 300)));
        assert!(state.crossing.is_none());
        assert_eq!(state.point, (-2, 300));
    }

    #[test]
    fn motion_tags_are_bounded_and_do_not_guard_other_applications_or_local_mouse() {
        let mut state = routed_state();
        for _ in 0..PENDING_MOTION {
            state.register(1, 0).unwrap();
        }
        assert!(state.register(1, 0).is_err());
        assert!(!state.mouse(
            WM_MOUSEMOVE,
            &MSLLHOOKSTRUCT {
                pt: windows_sys::Win32::Foundation::POINT { x: 50, y: 300 },
                flags: LLMHF_INJECTED,
                dwExtraInfo: 42,
                ..Default::default()
            }
        ));
        assert!(state.crossing.is_none());
    }

    #[test]
    fn receiver_releases_every_held_input_after_an_injection_failure() {
        static EVENTS: Mutex<Vec<Input>> = Mutex::new(Vec::new());
        fn fake(event: Input) -> Result<()> {
            EVENTS.lock().unwrap().push(event);
            ensure!(
                !matches!(
                    event,
                    Input::Key {
                        code: 31,
                        down: true
                    }
                ),
                "synthetic SendInput failure"
            );
            Ok(())
        }
        let mut sink = NativeSink {
            active: false,
            edge: None,
            return_edge: false,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            inject: fake,
            pointer: None,
            pointer_failed: false,
            cursor_setting: tokio::sync::watch::channel(true).1,
            group_routes: false,
            router: Router::default(),
            generation: 0,
            crossing: None,
            geometry_setting: None,
            motion: None,
        };
        sink.prepare(None, Mode::Input).unwrap();
        sink.activate(None, Mode::Input).unwrap();
        sink.event(Input::Key {
            code: 30,
            down: true,
        })
        .unwrap();
        sink.event(Input::Button {
            code: 272,
            down: true,
        })
        .unwrap();
        assert!(
            sink.event(Input::Key {
                code: 31,
                down: true
            })
            .is_err()
        );
        sink.release().unwrap();
        let events = EVENTS.lock().unwrap();
        assert!(events.contains(&Input::Key {
            code: 30,
            down: false
        }));
        assert!(events.contains(&Input::Button {
            code: 272,
            down: false
        }));
        assert!(!sink.active && sink.keys.is_empty() && sink.buttons.is_empty());
    }

    #[test]
    fn queued_motion_preserves_distance_and_button_order_without_cursor_readback() {
        let events = [
            Input::Move { dx: 7, dy: -3 },
            Input::Move { dx: -2, dy: 5 },
            Input::Move { dx: 100, dy: 20 },
            Input::Button {
                code: 272,
                down: true,
            },
            Input::Move { dx: -10, dy: -8 },
        ];
        // Encode the entire burst before the OS applies any event. In the old
        // path every absolute target could be based on the same stale cursor.
        let queued = events
            .into_iter()
            .map(encode)
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let mut position = (-1920, 50);
        let mut button_position = None;
        for input in queued {
            assert_eq!(input.r#type, INPUT_MOUSE);
            let mouse = unsafe { input.Anonymous.mi };
            if mouse.dwFlags & MOUSEEVENTF_MOVE != 0 {
                assert_eq!(mouse.dwFlags & MOUSEEVENTF_ABSOLUTE, 0);
                position.0 += mouse.dx;
                position.1 += mouse.dy;
            } else if mouse.dwFlags & MOUSEEVENTF_LEFTDOWN != 0 {
                button_position = Some(position);
            }
        }
        assert_eq!(button_position, Some((-1815, 72)));
        assert_eq!(position, (-1825, 64));
    }
}
