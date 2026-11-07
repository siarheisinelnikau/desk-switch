//! A click-through cursor for Windows desktops without an attached physical mouse.
//! Never changes ShowCursor counters, Mouse Keys, registry or system cursor settings.
use anyhow::{Result, ensure};
use std::{
    cell::RefCell,
    mem::size_of,
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread::JoinHandle,
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

const REFRESH: u32 = WM_APP + 0x3b;

#[derive(Clone)]
struct Wake {
    active: Arc<AtomicBool>,
    queued: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    thread_id: Arc<AtomicU32>,
}
impl Wake {
    fn refresh(&self) {
        let thread_id = self.thread_id.load(Ordering::Acquire);
        if thread_id == 0 || self.closed.load(Ordering::Acquire) {
            return;
        }
        // Retain only the latest position, never a backlog of obsolete frames.
        if !self.queued.swap(true, Ordering::AcqRel)
            && unsafe { PostThreadMessageW(thread_id, REFRESH, 0, 0) } == 0
        {
            self.queued.store(false, Ordering::Release);
        }
    }
}
thread_local! { static WAKE: RefCell<Option<Wake>> = const { RefCell::new(None) }; }

pub struct Overlay {
    wake: Wake,
    thread: Option<JoinHandle<()>>,
}
impl Overlay {
    pub fn start() -> Result<Self> {
        Self::start_with(|| unsafe { Window::create() })
    }
    fn start_with(create: impl FnOnce() -> Result<Window> + Send + 'static) -> Result<Self> {
        let wake = Wake {
            active: Arc::new(AtomicBool::new(false)),
            queued: Arc::new(AtomicBool::new(false)),
            closed: Arc::new(AtomicBool::new(false)),
            thread_id: Arc::new(AtomicU32::new(0)),
        };
        let worker = wake.clone();
        let thread = std::thread::Builder::new()
            .name("Desk Switch pointer".into())
            .spawn(move || {
                let wake = worker;
                let result = create();
                match result {
                    Ok(mut window) => {
                        // Publish the destination only after its message queue exists.
                        let mut message = MSG::default();
                        unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE) };
                        wake.thread_id
                            .store(unsafe { GetCurrentThreadId() }, Ordering::Release);
                        WAKE.with(|state| *state.borrow_mut() = Some(wake.clone()));
                        let _notifications = unsafe { Notifications::start(window.hwnd) };
                        let mut physical_mouse = physical_mouse();
                        let mut inventory_dirty = false;
                        // Device enumeration and window creation must never block
                        // the input/heartbeat task, including first motion and disable.
                        wake.refresh();
                        while !wake.closed.load(Ordering::Acquire) {
                            let status = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
                            if status <= 0 {
                                break;
                            }
                            if message.message == WM_DEVICECHANGE {
                                inventory_dirty = true;
                                wake.refresh();
                            } else if message.message == REFRESH {
                                wake.queued.store(false, Ordering::Release);
                                let active = wake.active.load(Ordering::Acquire);
                                if active && inventory_dirty {
                                    physical_mouse = self::physical_mouse();
                                    inventory_dirty = false;
                                }
                                unsafe {
                                    window.update(active, physical_mouse);
                                }
                            } else {
                                unsafe {
                                    TranslateMessage(&message);
                                    DispatchMessageW(&message);
                                }
                            }
                        }
                        WAKE.with(|state| state.borrow_mut().take());
                    }
                    Err(error) => {
                        eprintln!("Windows pointer fallback could not start: {error:#}");
                    }
                }
                wake.closed.store(true, Ordering::Release);
                wake.thread_id.store(0, Ordering::Release);
            })?;
        Ok(Self {
            wake,
            thread: Some(thread),
        })
    }
    pub fn active(&self, enabled: bool) {
        if self.wake.active.swap(enabled, Ordering::AcqRel) != enabled {
            self.wake.refresh();
        }
    }
    pub fn refresh(&self) {
        self.wake.refresh();
    }
}
impl Drop for Overlay {
    fn drop(&mut self) {
        self.active(false);
        self.wake.closed.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            if !thread.is_finished() {
                unsafe {
                    let thread_id = self.wake.thread_id.load(Ordering::Acquire);
                    if thread_id != 0 {
                        PostThreadMessageW(thread_id, WM_QUIT, 0, 0);
                    }
                }
                // The thread owns its window and cleans up after its current
                // native operation. Input release must not wait for a driver.
            } else {
                let _ = thread.join();
            }
        }
    }
}
fn physical_mouse() -> bool {
    crate::workspace::hardware::input_devices()
        .map(|devices| devices.iter().any(|device| device.mouse))
        .unwrap_or(true)
}
unsafe extern "system" fn cursor_changed(
    _: windows_sys::Win32::UI::Accessibility::HWINEVENTHOOK,
    _: u32,
    _: HWND,
    object: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    if object == OBJID_CURSOR {
        WAKE.with(|state| {
            if let Some(wake) = state.borrow().as_ref()
                && wake.active.load(Ordering::Acquire)
            {
                wake.refresh();
            }
        });
    }
}
struct Notifications {
    cursor: windows_sys::Win32::UI::Accessibility::HWINEVENTHOOK,
    devices: HDEVNOTIFY,
}
impl Notifications {
    unsafe fn start(hwnd: HWND) -> Self {
        use windows_sys::Win32::{
            Devices::HumanInterfaceDevice::GUID_DEVINTERFACE_MOUSE, UI::Accessibility::*,
        };
        unsafe {
            let cursor = SetWinEventHook(
                EVENT_OBJECT_SHOW,
                EVENT_OBJECT_NAMECHANGE,
                null_mut(),
                Some(cursor_changed),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            );
            let filter = DEV_BROADCAST_DEVICEINTERFACE_W {
                dbcc_size: size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32,
                dbcc_devicetype: DBT_DEVTYP_DEVICEINTERFACE,
                dbcc_classguid: GUID_DEVINTERFACE_MOUSE,
                ..Default::default()
            };
            let devices = RegisterDeviceNotificationW(
                hwnd,
                (&filter as *const DEV_BROADCAST_DEVICEINTERFACE_W).cast(),
                DEVICE_NOTIFY_WINDOW_HANDLE,
            );
            Self { cursor, devices }
        }
    }
}
impl Drop for Notifications {
    fn drop(&mut self) {
        unsafe {
            if !self.cursor.is_null() {
                windows_sys::Win32::UI::Accessibility::UnhookWinEvent(self.cursor);
            }
            if !self.devices.is_null() {
                UnregisterDeviceNotification(self.devices);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Drawing {
    cursor: HCURSOR,
    brush: HBRUSH,
}
#[cfg(test)]
thread_local! { static PAINT_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
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
            #[cfg(test)]
            PAINT_COUNT.with(|count| count.set(count.get() + 1));
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
    position: Option<(i32, i32)>,
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
            // Keep the class for this process. A disabled overlay can finish
            // asynchronously while a new one is starting on another thread.
            static REGISTERED: std::sync::OnceLock<std::result::Result<(), String>> =
                std::sync::OnceLock::new();
            let registered = REGISTERED.get_or_init(|| {
                if RegisterClassW(&class) == 0 {
                    Err(std::io::Error::last_os_error().to_string())
                } else {
                    Ok(())
                }
            });
            if let Err(error) = registered {
                DeleteObject(brush);
                anyhow::bail!("could not register the pointer window: {error}");
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
                position: None,
            };
            ensure!(
                SetLayeredWindowAttributes(hwnd, COLOR_KEY, 255, LWA_COLORKEY) != 0,
                "could not make the pointer window transparent"
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
                self.hide();
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
                self.hide();
                return;
            }
            if !self.visible {
                println!(
                    "Windows pointer fallback active: system cursor hidden and no physical mouse detected."
                );
            }
            let cursor = if info.hCursor.is_null() {
                LoadCursorW(null_mut(), IDC_ARROW)
            } else {
                info.hCursor
            };
            self.render(cursor, info.ptScreenPos.x, info.ptScreenPos.y);
        }
    }

    unsafe fn hide(&mut self) {
        if self.visible {
            unsafe {
                ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
        }
    }

    unsafe fn render(&mut self, cursor: HCURSOR, x: i32, y: i32) {
        unsafe {
            let changed = cursor != self.cursor;
            let (hotspot, size) = if cursor == self.cursor {
                (self.hotspot, self.size)
            } else {
                geometry(cursor)
            };
            self.cursor = cursor;
            self.hotspot = hotspot;
            self.size = size;
            let position = (x - hotspot.0, y - hotspot.1);
            if changed || self.position != Some(position) || !self.visible {
                SetWindowPos(
                    self.hwnd,
                    HWND_TOPMOST,
                    position.0,
                    position.1,
                    size.0,
                    size.1,
                    SWP_NOACTIVATE
                        | SWP_SHOWWINDOW
                        | if changed {
                            0
                        } else {
                            SWP_NOSIZE | SWP_NOREDRAW
                        },
                );
                self.position = Some(position);
                self.visible = true;
            }
            if changed {
                DRAWING.with(|state| {
                    if let Some(state) = state.borrow_mut().as_mut() {
                        state.cursor = cursor;
                    }
                });
                InvalidateRect(self.hwnd, null(), 0);
                UpdateWindow(self.hwnd);
            }
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
            DestroyWindow(self.hwnd);
            DRAWING.with(|state| state.borrow_mut().take());
            DeleteObject(self.brush);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_and_disable_do_not_wait_for_native_initialization() {
        let (started, ready) = std::sync::mpsc::sync_channel(1);
        let (finish, wait) = std::sync::mpsc::sync_channel(1);
        let overlay = Overlay::start_with(move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
            anyhow::bail!("synthetic cancelled pointer initialization")
        })
        .unwrap();
        ready
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let wake = overlay.wake.clone();
        overlay.active(true);
        overlay.refresh();
        drop(overlay); // Must return while the fake native operation is blocked.
        assert!(wake.closed.load(Ordering::Acquire));
        assert_eq!(wake.thread_id.load(Ordering::Acquire), 0);
        finish.send(()).unwrap();
    }

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
            // Draw only outside the virtual desktop; never inject or move the system pointer.
            let cursor = LoadCursorW(null_mut(), IDC_ARROW);
            let x = GetSystemMetrics(SM_XVIRTUALSCREEN) - 4096;
            let y = GetSystemMetrics(SM_YVIRTUALSCREEN) - 4096;
            window.render(cursor, x, y);
            let initial = PAINT_COUNT.with(|count| count.get());
            assert!(initial > 0);
            let since = std::time::Instant::now();
            for offset in 0..1000 {
                window.render(cursor, x - offset, y);
            }
            assert_eq!(
                PAINT_COUNT.with(|count| count.get()),
                initial,
                "moving a cached cursor must not repaint its bitmap"
            );
            assert!(since.elapsed() < std::time::Duration::from_secs(2));
            for _ in 0..1000 {
                window.render(cursor, x - 999, y);
            }
            assert_eq!(PAINT_COUNT.with(|count| count.get()), initial);
            // Hidden state has no timer or queued repaint.

            window.update(false, false);
            assert_eq!(IsWindowVisible(handle), 0);
            assert_eq!(GetForegroundWindow(), foreground);
            drop(window);
            assert_eq!(IsWindow(handle), 0);
        }
    }
}
