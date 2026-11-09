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
        Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM},
        Graphics::Gdi::*,
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::WindowsAndMessaging::*,
    },
    core::w,
};

const CLASS: windows_sys::core::PCWSTR = w!("DeskSwitch.PointerOverlay");

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
            // SAFETY: thread_id is published only after that thread created its
            // message queue, and it is cleared again when the thread ends, so the
            // post targets a live queue. The message carries only the constant
            // REFRESH id and no pointers, so nothing is borrowed across the call.
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
        // SAFETY: The closure is invoked on the dedicated pointer thread that owns
        // the window it creates, runs the matching message loop, and destroys the
        // window again, so no handle is used from another thread.
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
                        // SAFETY: `message` is a writable local MSG and the null
                        // window handle selects this thread's own message queue,
                        // which no other thread reads. The call only creates the
                        // queue and leaves `message` untouched.
                        unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE) };
                        wake.thread_id
                            // SAFETY: GetCurrentThreadId returns the identifier of
                            // this thread, which is published solely to target this
                            // thread's own message queue. It takes no pointers and
                            // cannot fail.
                            .store(unsafe { GetCurrentThreadId() }, Ordering::Release);
                        WAKE.with(|state| *state.borrow_mut() = Some(wake.clone()));
                        // SAFETY: `window.hwnd` is the overlay window owned by this
                        // thread. The notifications bind to that window and are
                        // dropped before it, so they never outlive the handle they
                        // were registered against.
                        let _notifications = unsafe { Notifications::start(window.hwnd) };
                        let mut physical_mouse = physical_mouse();
                        let mut inventory_dirty = false;
                        // Device enumeration and window creation must never block
                        // the input/heartbeat task, including first motion and disable.
                        wake.refresh();
                        while !wake.closed.load(Ordering::Acquire) {
                            // SAFETY: `message` is a writable local MSG and the null
                            // window handle selects this thread's own message queue.
                            // The call blocks until a message for this thread is
                            // stored in `message`.
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
                                // SAFETY: The overlay window was created on this
                                // thread and is only updated from this message
                                // loop, so the window handle and its bitmap
                                // surface cannot be used concurrently.
                                unsafe {
                                    if let Err(error) = window.update(active, physical_mouse) {
                                        eprintln!("Windows pointer fallback failed: {error:#}");
                                        break;
                                    }
                                }
                            } else {
                                // SAFETY: `message` was filled by GetMessageW on
                                // this thread, so its payload is still owned by
                                // this queue entry. TranslateMessage only posts to
                                // this thread's queue and DispatchMessageW calls
                                // the window procedure registered for this class.
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
                // SAFETY: thread_id identifies the pointer thread that still owns
                // its message queue. The WM_QUIT message carries only the constant
                // message id and no pointers, and a failed post only means the
                // thread already left its loop.
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
// SAFETY: This callback is registered with SetWinEventHook and is invoked by
// the OS on the thread that installed the hook, so it must use the callback
// calling convention. It only reads thread-local state and never dereferences
// the hook handle or the event arguments.
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
    // SAFETY: The caller must pass a live window handle that stays valid for the
    // whole lifetime of the returned notifications, because the device
    // notification is bound to that window and the window is needed to receive
    // the device-change messages.
    unsafe fn start(hwnd: HWND) -> Self {
        use windows_sys::Win32::{
            Devices::HumanInterfaceDevice::GUID_DEVINTERFACE_MOUSE, UI::Accessibility::*,
        };
        // SAFETY: `hwnd` is the overlay window that outlives these notifications,
        // and `filter` is a fully initialized local whose pointer is only read
        // for the duration of the call. The returned hook and device-notification
        // handles are stored in `Self` and released in its `Drop`.
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
        // SAFETY: Both handles were returned by SetWinEventHook and
        // RegisterDeviceNotificationW and are stored in this struct, so they are
        // still valid here. Each is released exactly once because `Drop` runs a
        // single time, and neither handle is used afterwards.
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

// SAFETY: This is the window procedure registered for the overlay class, so it
// must use the required window-procedure calling convention. Windows passes the
// live window handle and the message parameters, which are only forwarded.
unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        // SAFETY: The handle and parameters are the ones Windows passed for this
        // live overlay window, and DefWindowProcW interprets them without
        // retaining any reference beyond the call.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// A top-down premultiplied BGRA bitmap, owned by the pointer thread.
struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
    size: (i32, i32),
    xor: Vec<(usize, [u8; 3])>,
    background: Option<Box<Surface>>,
    screen: HDC,
}
impl Surface {
    // SAFETY: The returned `Surface` owns a private memory DC and its own DIB
    // section, and the caller must keep it alive while its pixels are used:
    // `bits` points into that DIB, which `Drop` deletes before the memory can
    // be released. The dimensions are validated inside this function.
    unsafe fn new(size: (i32, i32)) -> Result<Self> {
        // SAFETY: `info` is a fully initialized local BITMAPINFO whose pointer is
        // only read for the duration of the call, and `&mut surface.bits` is a
        // valid writable pointer to the `bits` field of the live `surface`. The
        // created DC, the DIB and the replaced stock object are checked for
        // null and for HGDI_ERROR before they are stored and released in `Drop`.
        unsafe {
            ensure!(
                (1..=512).contains(&size.0) && (1..=512).contains(&size.1),
                "invalid cursor dimensions"
            );
            let mut surface = Self {
                dc: CreateCompatibleDC(null_mut()),
                bitmap: null_mut(),
                previous: null_mut(),
                bits: null_mut(),
                size,
                xor: Vec::new(),
                background: None,
                screen: null_mut(),
            };
            ensure!(
                !surface.dc.is_null(),
                "could not create the cursor bitmap context"
            );
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size.0,
                    biHeight: -size.1,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            surface.bitmap = CreateDIBSection(
                surface.dc,
                &info,
                DIB_RGB_COLORS,
                (&mut surface.bits as *mut *mut u8).cast(),
                null_mut(),
                0,
            );
            ensure!(
                !surface.bitmap.is_null() && !surface.bits.is_null(),
                "could not create the cursor bitmap"
            );
            let previous = SelectObject(surface.dc, surface.bitmap);
            ensure!(
                !previous.is_null() && previous as isize != -1,
                "could not select the cursor bitmap"
            );
            surface.previous = previous;
            Ok(surface)
        }
    }
    fn bytes(&self) -> &[u8] {
        // SAFETY: `bits` points at the 32-bpp DIB section that was created with
        // exactly `size.0 * size.1` pixels, and that bitmap stays alive and
        // selected in `self.dc` while `self` is borrowed, so the slice covers
        // exactly the allocated pixels.
        unsafe { std::slice::from_raw_parts(self.bits, (self.size.0 * self.size.1 * 4) as usize) }
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: The DIB section behind `bits` is 32 bpp with `size.0 * size.1`
        // pixels and stays alive while `&mut self` borrows it, so the mutable
        // slice stays inside the allocation and no other reference exists.
        unsafe {
            std::slice::from_raw_parts_mut(self.bits, (self.size.0 * self.size.1 * 4) as usize)
        }
    }
    // SAFETY: `cursor` must be a live system cursor handle for the duration of
    // the call, because it is drawn into this surface's own memory DC. The
    // handle is borrowed only; ownership stays with the system.
    unsafe fn paint(&mut self, cursor: HCURSOR, background: u8) -> Result<()> {
        for pixel in self.bytes_mut().as_chunks_mut::<4>().0.iter_mut() {
            pixel.copy_from_slice(&[background, background, background, 255]);
        }
        // SAFETY: `self.dc` is this surface's own memory DC with its 32-bpp DIB
        // still selected, `cursor` is a live system cursor that DrawIconEx does
        // not take ownership of, and the null brush is permitted because
        // DI_NORMAL does not need one. GdiFlush only drains the batch of this
        // thread.
        unsafe {
            ensure!(
                DrawIconEx(
                    self.dc,
                    0,
                    0,
                    cursor,
                    self.size.0,
                    self.size.1,
                    0,
                    null_mut(),
                    DI_NORMAL
                ) != 0,
                "could not draw the system cursor"
            );
            GdiFlush();
        }
        Ok(())
    }
    // SAFETY: `cursor` must stay a live system cursor handle for the whole call
    // because it is measured and drawn twice. The two sampling surfaces own
    // their own DCs and DIB sections, and only the black one is returned.
    unsafe fn cursor(cursor: HCURSOR, size: (i32, i32)) -> Result<Self> {
        // SAFETY: `black` and `white` own the DC and DIB sections created by
        // `Surface::new`, so their pixel slices stay valid and refer to
        // distinct buffers. The cursor handle is only read, and `white` is
        // dropped at the end of this scope while `black` is returned.
        unsafe {
            // Recover coverage from Windows' own rendering on black and white.
            // This also supports legacy masks which have no stored alpha channel.
            let mut black = Self::new(size)?;
            let mut white = Self::new(size)?;
            black.paint(cursor, 0)?;
            white.paint(cursor, 255)?;
            let mut xor = Vec::new();
            for (index, (pixel, light)) in black
                .bytes_mut()
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(white.bytes().as_chunks::<4>().0.iter())
                .enumerate()
            {
                if (0..3).any(|channel| light[channel] < pixel[channel]) {
                    xor.push((index, [pixel[0], pixel[1], pixel[2]]));
                    pixel[3] = 255;
                    continue;
                }
                let transmission = (0..3)
                    .map(|i| light[i].saturating_sub(pixel[i]))
                    .max()
                    .unwrap();
                pixel[3] = 255 - transmission;
                for channel in &mut pixel[..3] {
                    *channel = (*channel).min(255 - transmission);
                }
            }
            black.xor = xor;
            Ok(black)
        }
    }
    fn resolve_xor(&mut self, background: &[u8]) {
        assert_eq!(background.len(), self.bytes().len());
        for (index, mask) in &self.xor {
            for channel in 0..3 {
                // SAFETY: Every index in `self.xor` was recorded while scanning
                // the DIB pixels, so `index * 4 + channel` stays below
                // `size.0 * size.1 * 4`, the length of the DIB allocation.
                // `background` was asserted to have that same length, so the
                // read at the same offset is in bounds as well.
                unsafe {
                    self.bits
                        .add(index * 4 + channel)
                        .write(background[index * 4 + channel] ^ mask[channel]);
                }
            }
        }
    }
    // SAFETY: The caller must pass screen coordinates inside the virtual
    // desktop, because the background rectangle is read from the screen DC at
    // that position. The sampled surface owns its own DC and DIB section, and
    // the screen DC is released in `Drop`.
    unsafe fn sample_background(&mut self, x: i32, y: i32) -> Result<()> {
        if self.xor.is_empty() {
            return Ok(());
        }
        // SAFETY: `background` is a `Surface` created with the same size, so its
        // DIB holds exactly `size.0 * size.1` 32-bpp pixels and `background.bits`
        // stays valid while `background` is borrowed; the slice length is taken
        // from that same buffer. `self.screen` is the process-wide screen DC
        // and is released in `Drop`.
        unsafe {
            if self.screen.is_null() {
                self.screen = GetDC(null_mut());
            }
            ensure!(
                !self.screen.is_null(),
                "could not read the background behind an inverted cursor"
            );
            if self.background.is_none() {
                self.background = Some(Box::new(Self::new(self.size)?));
            }
            let background = self.background.as_mut().unwrap();
            // SRCCOPY excludes layered windows. Never sample our own pointer.
            ensure!(
                BitBlt(
                    background.dc,
                    0,
                    0,
                    self.size.0,
                    self.size.1,
                    self.screen,
                    x,
                    y,
                    SRCCOPY
                ) != 0,
                "could not render the background-inverting system cursor"
            );
            GdiFlush();
            let bytes = std::slice::from_raw_parts(background.bits, background.bytes().len());
            self.resolve_xor(bytes);
        }
        Ok(())
    }
}
impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: Every handle here was created by this `Surface` and is
        // released exactly once: the previous stock object is restored into the
        // DC before the DIB is deleted, the DC is deleted before the screen DC
        // is released, and none of the handles is used after this point.
        unsafe {
            if !self.previous.is_null() {
                SelectObject(self.dc, self.previous);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
            if !self.dc.is_null() {
                DeleteDC(self.dc);
            }
            if !self.screen.is_null() {
                ReleaseDC(null_mut(), self.screen);
            }
        }
    }
}

struct Window {
    hwnd: HWND,
    surface: Option<Surface>,
    #[cfg(test)]
    bitmap_updates: usize,
    cursor: HCURSOR,
    hotspot: (i32, i32),
    size: (i32, i32),
    visible: bool,
    position: Option<(i32, i32)>,
}

impl Window {
    // SAFETY: The window must be created on the thread that will own it,
    // because its window procedure, device notifications and messages are
    // delivered to the creating thread's queue. The returned window is
    // destroyed by `Drop` on that same thread.
    unsafe fn create() -> Result<Self> {
        // SAFETY: `class` is a fully initialized local WNDCLASSW whose class
        // name is a static NUL-terminated wide string, and `module` is this
        // process's module handle, which stays loaded for the process lifetime.
        // The window procedure has the required calling convention, and the
        // created window is checked for null before it is stored.
        unsafe {
            let module = GetModuleHandleW(null());
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
                anyhow::bail!(
                    "could not create the pointer window: {}",
                    std::io::Error::last_os_error()
                );
            }
            let window = Self {
                hwnd,
                surface: None,
                #[cfg(test)]
                bitmap_updates: 0,
                cursor: null_mut(),
                hotspot: (0, 0),
                size: (32, 32),
                visible: false,
                position: None,
            };
            Ok(window)
        }
    }

    // SAFETY: This runs on the thread that created the window, so the window
    // handle and the cached surface are only ever touched from one thread. The
    // system cursor handle read from `CURSORINFO` is borrowed, not owned.
    unsafe fn update(&mut self, active: bool, physical_mouse: bool) -> Result<()> {
        // SAFETY: `info` is a writable local CURSORINFO whose `cbSize` matches
        // the struct, and GetCursorInfo only fills it with borrowed system
        // state: the returned `hCursor` is a shared system cursor that is never
        // destroyed here.
        unsafe {
            if !active {
                self.hide();
                return Ok(());
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
                return Ok(());
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
            self.render(cursor, info.ptScreenPos.x, info.ptScreenPos.y)?;
            Ok(())
        }
    }

    // SAFETY: The window handle is owned by this `Window`, and this method
    // runs on the thread that created it, so the hide cannot race with the
    // render path that uploads the layered bitmap.
    unsafe fn hide(&mut self) {
        if self.visible {
            // SAFETY: `self.hwnd` is the window this `Window` owns and it stays
            // alive until `Drop` destroys it; SW_HIDE only changes the
            // visibility state of that same window.
            unsafe {
                ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
        }
    }

    // SAFETY: `cursor` must stay a live system cursor handle for the call,
    // because it is measured and drawn synchronously. The window and its
    // surface are owned by this `Window` and are only touched on the thread
    // that created them.
    unsafe fn render(&mut self, cursor: HCURSOR, x: i32, y: i32) -> Result<()> {
        // SAFETY: `surface.dc` is the surface's own memory DC with its 32-bpp
        // DIB still selected, which is exactly the source that
        // UpdateLayeredWindow reads, and the destination, extent, source point
        // and blend function are fully initialized locals. `self.hwnd` is the
        // live window owned by this `Window`.
        unsafe {
            let changed = cursor != self.cursor;
            if changed {
                let (hotspot, size) = geometry(cursor);
                self.surface = Some(Surface::cursor(cursor, size)?);
                self.cursor = cursor;
                self.hotspot = hotspot;
                self.size = size;
            }
            let position = (x - self.hotspot.0, y - self.hotspot.1);
            let moved = self.position != Some(position);
            let surface = self.surface.as_mut().unwrap();
            if changed || (moved && !surface.xor.is_empty()) {
                surface.sample_background(position.0, position.1)?;
                let destination = POINT {
                    x: position.0,
                    y: position.1,
                };
                let extent = SIZE {
                    cx: self.size.0,
                    cy: self.size.1,
                };
                let source = POINT::default();
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
                ensure!(
                    UpdateLayeredWindow(
                        self.hwnd,
                        null_mut(),
                        &destination,
                        &extent,
                        surface.dc,
                        &source,
                        0,
                        &blend,
                        ULW_ALPHA
                    ) != 0,
                    "could not upload the transparent system cursor"
                );
                #[cfg(test)]
                {
                    self.bitmap_updates += 1;
                }
            }
            if changed || moved || !self.visible {
                ensure!(
                    SetWindowPos(
                        self.hwnd,
                        HWND_TOPMOST,
                        position.0,
                        position.1,
                        self.size.0,
                        self.size.1,
                        SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOSIZE | SWP_NOREDRAW
                    ) != 0,
                    "could not position the system cursor"
                );
                self.position = Some(position);
                self.visible = true;
            }
            Ok(())
        }
    }
}

// SAFETY: `cursor` must be a live cursor or icon handle for the call.
// GetIconInfo returns bitmaps whose handles this function owns and deletes
// before returning, so no GDI object is leaked and none is used afterwards.
unsafe fn geometry(cursor: HCURSOR) -> ((i32, i32), (i32, i32)) {
    // SAFETY: `icon` and `bitmap` are writable locals, and the byte count
    // passed to GetObjectW matches `size_of::<BITMAP>()`. The bitmaps returned
    // in `icon` are owned by this function and deleted exactly once before it
    // returns, and a failed GetIconInfo is handled by the fallback path.
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
        // SAFETY: `self.hwnd` is the window created by `Window::create` and is
        // destroyed exactly once here. It was created on the thread that drops
        // it, which is what DestroyWindow requires.
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_cursor_alpha_matches_windows_rendering_on_light_and_dark_backgrounds() {
        // SAFETY: The test runs on a single thread: the cursor handle loaded from
        // the system is only read and never destroyed, and every surface created
        // inside owns its own DC and DIB section until it is dropped at the end
        // of the iteration.
        unsafe {
            for (name, id) in [
                ("arrow", IDC_ARROW),
                ("hand", IDC_HAND),
                ("ibeam", IDC_IBEAM),
            ] {
                let cursor = LoadCursorW(null_mut(), id);
                let (_, size) = geometry(cursor);
                let mut rendered = Surface::cursor(cursor, size).unwrap();
                let mut mismatches = 0;
                for background in [0, 255] {
                    let mut reference = Surface::new(size).unwrap();
                    reference.paint(cursor, background).unwrap();
                    let backdrop = [background, background, background, 255]
                        .repeat((size.0 * size.1) as usize);
                    rendered.resolve_xor(&backdrop);
                    for (pixel, expected) in rendered
                        .bytes()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .zip(reference.bytes().as_chunks::<4>().0)
                    {
                        for channel in 0..3 {
                            let composite = u16::from(pixel[channel])
                                + (u16::from(background) * u16::from(255 - pixel[3]) + 127) / 255;
                            if composite.abs_diff(u16::from(expected[channel])) > 2 {
                                mismatches += 1;
                            }
                        }
                    }
                }
                eprintln!("System cursor {name}: {size:?}, {mismatches} differing channels");
                if let Some(directory) = std::env::var_os("DESK_SWITCH_CURSOR_CAPTURE") {
                    let directory = std::path::PathBuf::from(directory);
                    assert!(directory.is_absolute());
                    std::fs::create_dir_all(&directory).unwrap();
                    let rgba = rendered
                        .bytes()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(|pixel| {
                            let unpremultiply = |value: u8| {
                                if pixel[3] == 0 {
                                    0
                                } else {
                                    ((u32::from(value) * 255 + u32::from(pixel[3]) / 2)
                                        / u32::from(pixel[3]))
                                    .min(255) as u8
                                }
                            };
                            [
                                unpremultiply(pixel[2]),
                                unpremultiply(pixel[1]),
                                unpremultiply(pixel[0]),
                                pixel[3],
                            ]
                        })
                        .collect();
                    image::RgbaImage::from_raw(size.0 as u32, size.1 as u32, rgba)
                        .unwrap()
                        .save(directory.join(format!("cursor-{name}.png")))
                        .unwrap();
                    if name == "ibeam" {
                        let mut timings = Vec::new();
                        for _ in 0..64 {
                            let since = std::time::Instant::now();
                            rendered.sample_background(64, 64).unwrap();
                            timings.push(since.elapsed().as_micros());
                        }
                        timings.sort_unstable();
                        eprintln!(
                            "Inverted cursor background sampling: p50 {} us, p95 {} us, max {} us",
                            timings[32], timings[60], timings[63]
                        );
                    }
                }
                assert_eq!(mismatches, 0, "system cursor {name} changed appearance");
            }
        }
    }

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
        // SAFETY: The overlay window is created, used and destroyed on this test
        // thread, so its handle is never used from another thread. The system
        // cursor handle loaded here is borrowed and never destroyed.
        unsafe {
            let mut window = Window::create().unwrap();
            let handle = window.hwnd;
            assert_ne!(GetForegroundWindow(), handle);
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
            window.render(cursor, x, y).unwrap();
            assert_ne!(GetForegroundWindow(), handle);
            let initial = window.bitmap_updates;
            assert!(initial > 0);
            let since = std::time::Instant::now();
            for offset in 0..1000 {
                window.render(cursor, x - offset, y).unwrap();
                assert_ne!(GetForegroundWindow(), handle);
            }
            assert_eq!(
                window.bitmap_updates, initial,
                "moving a cached cursor must not repaint its bitmap"
            );
            assert!(since.elapsed() < std::time::Duration::from_secs(2));
            for _ in 0..1000 {
                window.render(cursor, x - 999, y).unwrap();
            }
            assert_eq!(window.bitmap_updates, initial);
            // Hidden state has no timer or queued repaint.

            window.update(false, false).unwrap();
            assert_eq!(IsWindowVisible(handle), 0);
            // Another application may legitimately gain focus during this test.
            // This overlay must never become the foreground window itself.
            assert_ne!(GetForegroundWindow(), handle);
            drop(window);
            assert_eq!(IsWindow(handle), 0);
        }
    }
}
