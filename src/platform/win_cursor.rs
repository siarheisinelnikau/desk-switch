//! A click-through cursor for Windows desktops without an attached physical mouse.
//! Never changes ShowCursor counters, Mouse Keys, registry or system cursor settings.
use anyhow::{Context, Result, ensure};
use std::{
    cell::RefCell,
    mem::size_of,
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use windows_sys::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::*,
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::WindowsAndMessaging::*,
    },
    core::w,
};

const CLASS: windows_sys::core::PCWSTR = w!("DeskSwitch.PointerOverlay");
const COLOR_KEY: u32 = 0x00ff00ff;

pub struct Overlay {
    active: Arc<AtomicBool>,
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl Overlay {
    pub fn start() -> Result<Self> {
        let active = Arc::new(AtomicBool::new(true));
        let enabled = Arc::clone(&active);
        let (send, receive) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("Desk Switch pointer".into())
            .spawn(move || {
                let result = unsafe { Window::create() };
                match result {
                    Ok(mut window) => {
                        let id = unsafe { GetCurrentThreadId() };
                        if send.send(Ok(id)).is_err() {
                            return;
                        }
                        let mut inventory_at = Instant::now() - Duration::from_secs(3);
                        let mut physical_mouse = true;
                        let mut message = MSG::default();
                        loop {
                            let status = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
                            if status <= 0 {
                                break;
                            }
                            if message.message == WM_TIMER {
                                if enabled.load(Ordering::Relaxed)
                                    && inventory_at.elapsed() >= Duration::from_secs(2)
                                {
                                    inventory_at = Instant::now();
                                    physical_mouse = crate::workspace::hardware::input_devices()
                                        .map(|devices| devices.iter().any(|d| d.mouse))
                                        .unwrap_or(true);
                                }
                                unsafe {
                                    window.update(enabled.load(Ordering::Relaxed), physical_mouse);
                                }
                            } else {
                                unsafe {
                                    TranslateMessage(&message);
                                    DispatchMessageW(&message);
                                }
                            }
                        }
                    }
                    Err(error) => {
                        let _ = send.send(Err(format!("{error:#}")));
                    }
                }
            })?;
        match receive
            .recv()
            .context("starting the Windows pointer window")?
        {
            Ok(thread_id) => Ok(Self {
                active,
                thread_id,
                thread: Some(thread),
            }),
            Err(error) => {
                let _ = thread.join();
                anyhow::bail!("{error}");
            }
        }
    }

    pub fn active(&self, enabled: bool) {
        self.active.store(enabled, Ordering::Relaxed);
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.active(false);
        if let Some(thread) = self.thread.take() {
            if !thread.is_finished() {
                unsafe {
                    PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
                }
            }
            let _ = thread.join();
        }
    }
}

#[derive(Clone, Copy)]
struct Drawing {
    cursor: HCURSOR,
    brush: HBRUSH,
}
thread_local! { static DRAWING: RefCell<Option<Drawing>> = const { RefCell::new(None) }; }

unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_PAINT => {
            unsafe {
                let mut paint = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut paint);
                if !dc.is_null() {
                    DRAWING.with(|state| {
                        if let Some(state) = *state.borrow() {
                            let mut rect = RECT::default();
                            GetClientRect(hwnd, &mut rect);
                            FillRect(dc, &rect, state.brush);
                            if !state.cursor.is_null() {
                                DrawIconEx(dc, 0, 0, state.cursor, 0, 0, 0, null_mut(), DI_NORMAL);
                            }
                        }
                    });
                }
                EndPaint(hwnd, &paint);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

struct Window {
    hwnd: HWND,
    brush: HBRUSH,
    cursor: HCURSOR,
    hotspot: (i32, i32),
    size: (i32, i32),
    visible: bool,
}

impl Window {
    unsafe fn create() -> Result<Self> {
        unsafe {
            let module = GetModuleHandleW(null());
            let brush = CreateSolidBrush(COLOR_KEY);
            ensure!(!brush.is_null(), "could not create the pointer background");
            let class = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: module,
                lpszClassName: CLASS,
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                DeleteObject(brush);
                anyhow::bail!(
                    "could not register the pointer window: {}",
                    std::io::Error::last_os_error()
                );
            }
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST,
                CLASS,
                w!("Desk Switch pointer"),
                WS_POPUP,
                0,
                0,
                32,
                32,
                null_mut(),
                null_mut(),
                module,
                null(),
            );
            if hwnd.is_null() {
                UnregisterClassW(CLASS, module);
                DeleteObject(brush);
                anyhow::bail!(
                    "could not create the pointer window: {}",
                    std::io::Error::last_os_error()
                );
            }
            let window = Self {
                hwnd,
                brush,
                cursor: null_mut(),
                hotspot: (0, 0),
                size: (32, 32),
                visible: false,
            };
            ensure!(
                SetLayeredWindowAttributes(hwnd, COLOR_KEY, 255, LWA_COLORKEY) != 0,
                "could not make the pointer window transparent"
            );
            ensure!(
                SetTimer(hwnd, 1, 16, None) != 0,
                "could not start the pointer refresh timer"
            );
            DRAWING.with(|state| {
                *state.borrow_mut() = Some(Drawing {
                    cursor: null_mut(),
                    brush,
                })
            });
            Ok(window)
        }
    }

    unsafe fn update(&mut self, active: bool, physical_mouse: bool) {
        unsafe {
            if !active {
                ShowWindow(self.hwnd, SW_HIDE);
                self.visible = false;
                return;
            }
            let mut info = CURSORINFO {
                cbSize: size_of::<CURSORINFO>() as u32,
                ..Default::default()
            };
            if GetCursorInfo(&mut info) == 0
                || !super::cursor_policy::fallback(
                    active,
                    physical_mouse,
                    info.flags & CURSOR_SHOWING != 0,
                    info.flags & CURSOR_SUPPRESSED != 0,
                )
            {
                ShowWindow(self.hwnd, SW_HIDE);
                self.visible = false;
                return;
            }
            if !self.visible {
                println!(
                    "Windows pointer fallback active: system cursor hidden and no physical mouse detected."
                );
                self.visible = true;
            }
            let cursor = if info.hCursor.is_null() {
                LoadCursorW(null_mut(), IDC_ARROW)
            } else {
                info.hCursor
            };
            let (hotspot, size) = if cursor == self.cursor {
                (self.hotspot, self.size)
            } else {
                geometry(cursor)
            };
            self.cursor = cursor;
            self.hotspot = hotspot;
            self.size = size;
            DRAWING.with(|state| {
                if let Some(state) = state.borrow_mut().as_mut() {
                    state.cursor = cursor;
                }
            });
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                info.ptScreenPos.x - hotspot.0,
                info.ptScreenPos.y - hotspot.1,
                size.0,
                size.1,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            InvalidateRect(self.hwnd, null(), 0);
            UpdateWindow(self.hwnd);
        }
    }
}

unsafe fn geometry(cursor: HCURSOR) -> ((i32, i32), (i32, i32)) {
    unsafe {
        let mut icon = ICONINFO::default();
        if GetIconInfo(cursor, &mut icon) == 0 {
            return ((0, 0), (32, 32));
        }
        let mut bitmap = BITMAP::default();
        let monochrome = icon.hbmColor.is_null();
        let handle = if monochrome {
            icon.hbmMask
        } else {
            icon.hbmColor
        };
        let measured = GetObjectW(
            handle,
            size_of::<BITMAP>() as i32,
            (&mut bitmap as *mut BITMAP).cast(),
        );
        let size = if measured == 0 {
            (32, 32)
        } else {
            (
                bitmap.bmWidth.clamp(1, 512),
                (bitmap.bmHeight / if monochrome { 2 } else { 1 }).clamp(1, 512),
            )
        };
        if !icon.hbmMask.is_null() {
            DeleteObject(icon.hbmMask);
        }
        if !icon.hbmColor.is_null() {
            DeleteObject(icon.hbmColor);
        }
        (
            (icon.xHotspot.min(511) as i32, icon.yHotspot.min(511) as i32),
            size,
        )
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            KillTimer(self.hwnd, 1);
            DestroyWindow(self.hwnd);
            DRAWING.with(|state| state.borrow_mut().take());
            DeleteObject(self.brush);
            UnregisterClassW(CLASS, GetModuleHandleW(null()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_window_is_hidden_click_through_and_never_activates() {
        unsafe {
            let foreground = GetForegroundWindow();
            let mut window = Window::create().unwrap();
            let handle = window.hwnd;
            let styles = GetWindowLongPtrW(handle, GWL_EXSTYLE) as u32;
            for required in [
                WS_EX_LAYERED,
                WS_EX_TRANSPARENT,
                WS_EX_NOACTIVATE,
                WS_EX_TOOLWINDOW,
            ] {
                assert_ne!(styles & required, 0);
            }
            // Do not draw or inject anything on the test desktop.
            window.update(false, false);
            assert_eq!(IsWindowVisible(handle), 0);
            assert_eq!(GetForegroundWindow(), foreground);
            drop(window);
            assert_eq!(IsWindow(handle), 0);
        }
    }
}
