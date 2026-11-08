use super::{host::Control, win_monitor};
use crate::keys::scan_code;
use crate::{
    config::Edge,
    edge::{Entry, Hit, Router, Routes},
    gate::{Gate, Route},
    wire::Input,
};
use anyhow::{Context, Result, ensure};
use std::{
    cell::RefCell,
    io,
    ptr::{null, null_mut},
    sync::Arc,
};
use tokio::sync::{mpsc, watch};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
    Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F11, VK_F12},
        WindowsAndMessaging::*,
    },
};
use windows_sys::core::w;

#[derive(Clone, Copy)]
enum Owner {
    Local,
    Suppressed,
    Remote(u64),
}

struct HookData {
    gate: Arc<Gate>,
    control: mpsc::Sender<Control>,
    input: mpsc::Sender<(u64, Input)>,
    keys: [Option<Owner>; 256],
    buttons: [Option<Owner>; 5],
    edge: Option<Edge>,
    sharing: crate::config::InputSharing,
    toggle_down: bool,
    emergency_down: bool,
    group_routes: bool,
    route_setting: watch::Receiver<Routes>,
    router: Router,
    geometry_dirty: bool,
    route_notifications: bool,
}

thread_local! {
    static DATA: RefCell<Option<HookData>> = const { RefCell::new(None) };
}

impl HookData {
    fn refresh_routes(&mut self) {
        if !self.group_routes
            || (!self.geometry_dirty && !self.route_setting.has_changed().unwrap_or(false))
        {
            return;
        }
        self.geometry_dirty = false;
        let routes = self.route_setting.borrow_and_update().clone();
        let screens = match self
            .route_notifications
            .then(win_monitor::native_screens)
            .transpose()
        {
            Ok(None) => Vec::new(),
            Ok(Some(screens)) => screens,
            Err(error) => {
                eprintln!("Windows display edges unavailable: {error:#}");
                Vec::new()
            }
        };
        if let Err(error) = self.router.configure(routes, &screens) {
            self.router = Router::default();
            eprintln!("Windows display routes rejected: {error:#}");
        }
    }

    fn emit(&self, token: u64, input: Input) {
        if self.input.try_send((token, input)).is_err() {
            self.gate.input_overflow(token);
        }
    }

    fn control(&self, token: u64, control: Control) {
        if self.control.try_send(control).is_err() {
            self.gate.control_overflow(token);
        }
    }

    fn physical(&mut self, index: usize, button: bool, down: bool, input: Option<Input>) -> bool {
        let (token, route) = self.gate.current();
        let owners = if button {
            &mut self.buttons[..]
        } else {
            &mut self.keys[..]
        };
        let slot = &mut owners[index];
        let route = if if button {
            self.sharing.mouse
        } else {
            self.sharing.keyboard
        } {
            route
        } else {
            Route::Local
        };
        let fallback = match route {
            Route::Local => Owner::Local,
            Route::Hold => Owner::Suppressed,
            Route::Remote => Owner::Remote(token),
        };
        let owner = if down {
            *slot.get_or_insert(fallback)
        } else {
            slot.take().unwrap_or(fallback)
        };
        if let (Owner::Remote(owned), Some(input)) = (owner, input)
            && owned == token
            && route == Route::Remote
        {
            self.emit(token, input);
        }
        !matches!(owner, Owner::Local)
    }

    fn idle(&self) -> bool {
        self.keys.iter().all(Option::is_none)
            && self.buttons.iter().all(Option::is_none)
            && !(1..=254).any(|vk| {
                vk != i32::from(VK_F11)
                    && vk != i32::from(VK_F12)
                    && unsafe { GetAsyncKeyState(vk) } < 0
            })
    }

    fn toggle(&self) {
        let (token, route) = self.gate.current();
        let switching = match route {
            Route::Local if self.idle() => self.gate.begin(),
            Route::Remote if self.gate.hold(token) => Some(token),
            _ => None,
        };
        if let Some(token) = switching {
            self.control(token, Control::Toggle(token));
        }
    }

    fn keyboard(&mut self, data: &KBDLLHOOKSTRUCT) -> bool {
        if data.flags & LLKHF_INJECTED != 0 {
            return false;
        }
        self.refresh_routes();
        let down = data.flags & LLKHF_UP == 0;
        if data.vkCode == u32::from(VK_F11) {
            if down && !self.emergency_down {
                self.gate.emergency();
            }
            self.emergency_down = down;
            return true;
        }
        if data.vkCode == u32::from(VK_F12) {
            if down && !self.toggle_down {
                self.toggle();
            }
            self.toggle_down = down;
            return true;
        }
        let Ok(index) = usize::try_from(data.vkCode) else {
            return false;
        };
        if index >= self.keys.len() {
            return false;
        }
        let code = scan_code(data.scanCode, data.flags & LLKHF_EXTENDED != 0, data.vkCode);
        self.physical(
            index,
            false,
            down,
            code.map(|code| Input::Key { code, down }),
        )
    }

    fn mouse(&mut self, message: u32, data: &MSLLHOOKSTRUCT) -> bool {
        if data.flags & LLMHF_INJECTED != 0 || !self.sharing.mouse {
            return false;
        }
        self.refresh_routes();
        let (token, route) = self.gate.current();
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
        if let Some((index, down)) = button {
            return self.physical(
                index,
                true,
                down,
                Some(Input::Button {
                    code: 272 + index as u16,
                    down,
                }),
            );
        }

        if message == WM_MOUSEMOVE {
            let mut current = POINT::default();
            if unsafe { GetCursorPos(&mut current) } != 0 {
                let dx = (data.pt.x - current.x).clamp(-16384, 16384);
                let dy = (data.pt.y - current.y).clamp(-16384, 16384);
                match route {
                    Route::Remote if dx != 0 || dy != 0 => self.emit(token, Input::Move { dx, dy }),
                    Route::Local if self.group_routes => {
                        let (x, y, dx, dy) =
                            (current.x.into(), current.y.into(), dx.into(), dy.into());
                        // Query async key state only at an actual configured edge.
                        // Ordinary pointer motion remains an in-memory lookup.
                        let hit = if self.router.crossing_if(x, y, dx, dy, false).is_some() {
                            let idle = !self.toggle_down && !self.emergency_down && self.idle();
                            self.router.crossing_if(x, y, dx, dy, idle)
                        } else {
                            None
                        };
                        match hit {
                            Some(Hit::Cross(crossing)) => {
                                self.control(token, Control::Boundary { token, crossing });
                                return true;
                            }
                            Some(Hit::Block) => return true,
                            None => {}
                        }
                    }
                    Route::Local if let Some(edge) = &self.edge => {
                        let handle = unsafe { MonitorFromPoint(current, MONITOR_DEFAULTTONEAREST) };
                        if let Ok(screen) = win_monitor::screen(handle)
                            && screen.name.eq_ignore_ascii_case(&edge.screen)
                            && let Some(position) = edge.side.crossing(
                                screen.rect,
                                current.x.into(),
                                current.y.into(),
                                dx.into(),
                                dy.into(),
                            )
                            && self.idle()
                            && let Some(token) = self.gate.begin()
                        {
                            self.control(token, Control::Edge { token, position });
                            return true;
                        }
                    }
                    _ => {}
                }
            }
        } else if route == Route::Remote && matches!(message, WM_MOUSEWHEEL | WM_MOUSEHWHEEL) {
            let delta = (high as i16).clamp(-4096, 4096);
            let (dx, dy) = if message == WM_MOUSEWHEEL {
                (0, -delta)
            } else {
                (delta, 0)
            };
            self.emit(token, Input::Scroll { dx, dy });
        }
        route != Route::Local
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0
        && DATA.with(|state| {
            state
                .try_borrow_mut()
                .ok()
                .and_then(|mut state| {
                    state.as_mut().map(|state| {
                        state.keyboard(unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) })
                    })
                })
                .unwrap_or(false)
        })
    {
        return 1;
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0
        && DATA.with(|state| {
            state
                .try_borrow_mut()
                .ok()
                .and_then(|mut state| {
                    state.as_mut().map(|state| {
                        state.mouse(wparam as u32, unsafe {
                            &*(lparam as *const MSLLHOOKSTRUCT)
                        })
                    })
                })
                .unwrap_or(false)
        })
    {
        return 1;
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

pub struct Hooks {
    keyboard: HHOOK,
    mouse: HHOOK,
    _notifications: Option<Notifications>,
    _coordinates: win_monitor::PhysicalCoordinates,
}

struct Notifications(HWND);

impl Drop for Notifications {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

unsafe extern "system" fn display_notification(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if matches!(
        message,
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DEVICECHANGE
    ) {
        DATA.with(|state| {
            if let Ok(mut state) = state.try_borrow_mut()
                && let Some(state) = state.as_mut()
            {
                state.geometry_dirty = true;
            }
        });
        return 0;
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn notifications() -> Result<Notifications> {
    let module = unsafe { GetModuleHandleW(null()) };
    let class = WNDCLASSW {
        lpfnWndProc: Some(display_notification),
        hInstance: module,
        lpszClassName: w!("DeskSwitchDisplayNotifications"),
        ..Default::default()
    };
    unsafe {
        RegisterClassW(&class);
    }
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.lpszClassName,
            w!("Desk Switch display notifications"),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            module,
            null(),
        )
    };
    ensure!(
        !window.is_null(),
        "cannot subscribe to Windows display changes"
    );
    Ok(Notifications(window))
}

impl Hooks {
    pub fn install(
        gate: Arc<Gate>,
        mut edge: Option<Edge>,
        sharing: crate::config::InputSharing,
        control: mpsc::Sender<Control>,
        input: mpsc::Sender<(u64, Input)>,
    ) -> Result<Self> {
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let coordinates = win_monitor::PhysicalCoordinates::enter()?;
        if let Some(edge) = &mut edge
            && edge.screen == "auto"
        {
            edge.screen = win_monitor::screens()?
                .into_iter()
                .next()
                .context("No screen is available")?
                .name;
        }
        if let Some(edge) = &edge {
            ensure!(
                win_monitor::screens()?
                    .iter()
                    .filter(|screen| screen.name.eq_ignore_ascii_case(&edge.screen))
                    .count()
                    == 1,
                "Windows edge screen '{}' is unavailable or ambiguous; run probe",
                edge.screen
            );
        }
        let mut msg = MSG::default();
        unsafe {
            PeekMessageW(&mut msg, null_mut(), 0, 0, PM_NOREMOVE);
        }
        let notifications = match notifications() {
            Ok(notifications) => Some(notifications),
            Err(error) => {
                eprintln!("Windows automatic display edges disabled: {error:#}");
                None
            }
        };
        let route_notifications = notifications.is_some();
        let module = unsafe { GetModuleHandleW(null()) };
        let keyboard = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0) };
        if keyboard.is_null() {
            return Err(io::Error::last_os_error()).context("installing the Windows keyboard hook");
        }
        let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0) };
        if mouse.is_null() {
            let error = io::Error::last_os_error();
            unsafe {
                UnhookWindowsHookEx(keyboard);
            }
            return Err(error).context("installing the Windows mouse hook");
        }
        DATA.with(|state| {
            *state.borrow_mut() = Some(HookData {
                gate,
                control,
                input,
                keys: [None; 256],
                buttons: [None; 5],
                edge,
                sharing,
                toggle_down: false,
                emergency_down: false,
                group_routes: crate::lifecycle::group_input(),
                route_setting: crate::lifecycle::routes(),
                router: Router::default(),
                geometry_dirty: true,
                route_notifications,
            });
        });
        Ok(Self {
            keyboard,
            mouse,
            _notifications: notifications,
            _coordinates: coordinates,
        })
    }

    pub fn run(&self) -> Result<()> {
        let mut msg = MSG::default();
        loop {
            match unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } {
                0 => return Ok(()),
                -1 => {
                    return Err(io::Error::last_os_error())
                        .context("Windows input message loop failed");
                }
                _ if msg.message == REQUEST_TOGGLE => {
                    DATA.with(|state| {
                        if let Some(state) = state.borrow().as_ref() {
                            state.toggle();
                        }
                    });
                }
                _ if msg.message == RESET_ENTRY => {
                    DATA.with(|state| {
                        if let Some(state) = state.borrow_mut().as_mut() {
                            state.router.reset_entry();
                        }
                    });
                }
                _ => unsafe {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                },
            }
        }
    }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        unsafe {
            UnhookWindowsHookEx(self.keyboard);
            UnhookWindowsHookEx(self.mouse);
        }
        DATA.with(|state| {
            state.borrow_mut().take();
        });
    }
}

pub fn center() -> Result<()> {
    let _coordinates = win_monitor::PhysicalCoordinates::enter()?;
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return Err(io::Error::last_os_error()).context("reading the Windows cursor");
    }
    let screen = win_monitor::screen(unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) })?;
    move_cursor(
        screen.rect.x + screen.rect.width / 2.0,
        screen.rect.y + screen.rect.height / 2.0,
    )
}

pub fn enter(edge: &Edge, position: u16) -> Result<()> {
    let screens = win_monitor::screens()?;
    let mut matches = screens
        .iter()
        .filter(|screen| screen.name.eq_ignore_ascii_case(&edge.screen));
    let screen = matches
        .next()
        .context("Windows return edge screen disappeared")?;
    ensure!(
        matches.next().is_none(),
        "Windows return edge screen is ambiguous"
    );
    let (x, y) = edge.entry(screen.rect, position);
    move_cursor(x, y)
}

pub fn enter_display(entry: Entry) -> Result<()> {
    entry.resolve(&win_monitor::native_screens()?)?;
    let (x, y) = entry.point();
    move_cursor(x, y)
}

pub fn enter_display_current(
    entry: Entry,
    activation: u64,
    routes: &watch::Receiver<Routes>,
    canceled: &std::sync::Mutex<bool>,
) -> Result<()> {
    // Enumeration can be slow and must not block route publication. Only the
    // final native mutation is linearized with cancellation and route changes.
    entry.resolve(&win_monitor::native_screens()?)?;
    let (x, y) = entry.point();
    commit_display_entry(activation, routes, canceled, || move_cursor(x, y))
}

fn commit_display_entry(
    activation: u64,
    routes: &watch::Receiver<Routes>,
    canceled: &std::sync::Mutex<bool>,
    position: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let canceled = canceled
        .lock()
        .map_err(|_| anyhow::anyhow!("Windows cursor entry was canceled"))?;
    let current = routes.borrow();
    ensure!(
        !*canceled && current.activation == activation,
        "The display activation changed before the cursor could enter"
    );
    position()
}

pub(super) fn cursor_position() -> Result<(i32, i32)> {
    let _coordinates = win_monitor::PhysicalCoordinates::enter()?;
    let mut point = POINT::default();
    ensure!(
        unsafe { GetCursorPos(&mut point) } != 0,
        "cannot read the Windows cursor"
    );
    Ok((point.x, point.y))
}

fn move_cursor(x: f64, y: f64) -> Result<()> {
    let _coordinates = win_monitor::PhysicalCoordinates::enter()?;
    if unsafe { SetCursorPos(x.round() as i32, y.round() as i32) } == 0 {
        return Err(io::Error::last_os_error()).context("positioning the Windows cursor");
    }
    Ok(())
}

pub const REQUEST_TOGGLE: u32 = WM_APP + 20;
pub const RESET_ENTRY: u32 = WM_APP + 21;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_geometry_validation_cannot_position_a_newer_activation() {
        let (send, routes) = watch::channel(Routes {
            activation: 7,
            ..Default::default()
        });
        let canceled = std::sync::Mutex::new(false);
        let mut positioned = false;
        let validate = || {
            send.send_replace(Routes {
                activation: 8,
                ..Default::default()
            });
            Ok::<_, anyhow::Error>(())
        };
        validate().unwrap();
        let result = commit_display_entry(7, &routes, &canceled, || {
            positioned = true;
            Ok(())
        });
        assert!(result.is_err());
        assert!(!positioned);
        let result = commit_display_entry(8, &routes, &canceled, || {
            positioned = true;
            Ok(())
        });
        assert!(result.is_ok() && positioned);
    }

    #[test]
    fn canceled_entry_cannot_position_even_when_geometry_generation_is_unchanged() {
        let (_send, routes) = watch::channel(Routes {
            activation: 7,
            ..Default::default()
        });
        let canceled = std::sync::Mutex::new(true);
        let result = commit_display_entry(7, &routes, &canceled, || {
            panic!("Canceled entry positioned the pointer")
        });
        assert!(result.is_err());
    }
    #[test]
    fn disabled_keyboard_passes_through_while_mouse_is_shared() {
        let gate = Arc::new(Gate::new());
        let token = gate.begin().unwrap();
        assert!(gate.forward(token));
        let (control, _) = mpsc::channel(8);
        let (input, mut events) = mpsc::channel(8);
        let mut hooks = HookData {
            gate,
            control,
            input,
            keys: [None; 256],
            buttons: [None; 5],
            edge: None,
            sharing: crate::config::InputSharing {
                keyboard: false,
                mouse: true,
            },
            toggle_down: false,
            emergency_down: false,
            group_routes: false,
            route_setting: watch::channel(Routes::default()).1,
            router: Router::default(),
            geometry_dirty: false,
            route_notifications: false,
        };
        assert!(!hooks.physical(
            30,
            false,
            true,
            Some(Input::Key {
                code: 30,
                down: true
            })
        ));
        assert!(events.try_recv().is_err());
        assert!(hooks.physical(
            0,
            true,
            true,
            Some(Input::Button {
                code: 272,
                down: true
            })
        ));
        assert!(matches!(
            events.try_recv().unwrap(),
            (
                _,
                Input::Button {
                    code: 272,
                    down: true
                }
            )
        ));
        assert!(!hooks.physical(
            30,
            false,
            false,
            Some(Input::Key {
                code: 30,
                down: false
            })
        ));
    }
}
