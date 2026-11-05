use super::{host::Control, win_monitor};
use crate::keys::scan_code;
use crate::{
    config::Edge,
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
use tokio::sync::mpsc;
use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, POINT, WPARAM},
    Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F11, VK_F12},
        WindowsAndMessaging::*,
    },
};

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
    toggle_down: bool,
    emergency_down: bool,
}

thread_local! {
    static DATA: RefCell<Option<HookData>> = const { RefCell::new(None) };
}

impl HookData {
    fn emit(&self, token: u64, input: Input) {
        if self.input.try_send((token, input)).is_err() {
            self.gate.local(token);
        }
    }

    fn control(&self, token: u64, control: Control) {
        if self.control.try_send(control).is_err() {
            self.gate.local(token);
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

    fn keyboard(&mut self, data: &KBDLLHOOKSTRUCT) -> bool {
        if data.flags & LLKHF_INJECTED != 0 {
            return false;
        }
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
        if data.flags & LLMHF_INJECTED != 0 {
            return false;
        }
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
}

impl Hooks {
    pub fn install(
        gate: Arc<Gate>,
        edge: Option<Edge>,
        control: mpsc::Sender<Control>,
        input: mpsc::Sender<(u64, Input)>,
    ) -> Result<Self> {
        let dpi =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        ensure!(
            dpi != 0 || edge.is_none(),
            "per-monitor DPI awareness could not be enabled for edge mode"
        );
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
                toggle_down: false,
                emergency_down: false,
            });
        });
        Ok(Self { keyboard, mouse })
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
    let (x, y) = edge.side.entry(screen.rect, position);
    move_cursor(x, y)
}

fn move_cursor(x: f64, y: f64) -> Result<()> {
    if unsafe { SetCursorPos(x.round() as i32, y.round() as i32) } == 0 {
        return Err(io::Error::last_os_error()).context("positioning the Windows cursor");
    }
    Ok(())
}
