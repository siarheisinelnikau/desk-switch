use crate::{
    config::hex,
    edge::{NativeScreen, Rect, ScreenRect},
    video::{Monitor, input_codes},
};
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    ffi::CStr,
    io,
    mem::size_of,
    ptr::{null, null_mut},
    sync::OnceLock,
    time::Duration,
};
use windows_sys::{
    Win32::{
        Devices::{DeviceAndDriverInstallation::*, Display::*},
        Foundation::{HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        System::Registry::*,
        UI::{
            HiDpi::{
                DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                SetThreadDpiAwarenessContext,
            },
            WindowsAndMessaging::*,
        },
    },
    core::w,
};

pub struct Screen {
    pub name: String,
    pub rect: Rect,
    handle: HMONITOR,
    gdi: [u16; 32],
}

// Probe workers and input workers can inherit different process DPI contexts.
// Every geometry/position operation uses the same physical desktop coordinates.
pub(super) struct PhysicalCoordinates(DPI_AWARENESS_CONTEXT);

impl PhysicalCoordinates {
    pub(super) fn enter() -> Result<Self> {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        ensure!(
            !previous.is_null(),
            "cannot select physical Windows display coordinates"
        );
        Ok(Self(previous))
    }
}

impl Drop for PhysicalCoordinates {
    fn drop(&mut self) {
        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
    }
}

fn wide_text(text: &[u16]) -> String {
    let len = text
        .iter()
        .position(|&value| value == 0)
        .unwrap_or(text.len());
    String::from_utf16_lossy(&text[..len])
}

pub fn screen(handle: HMONITOR) -> Result<Screen> {
    let _coordinates = PhysicalCoordinates::enter()?;
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if unsafe { GetMonitorInfoW(handle, &mut info.monitorInfo) } == 0 {
        return Err(io::Error::last_os_error()).context("reading Windows monitor geometry");
    }
    let rect = info.monitorInfo.rcMonitor;
    Ok(Screen {
        name: wide_text(&info.szDevice),
        rect: Rect {
            x: f64::from(rect.left),
            y: f64::from(rect.top),
            width: f64::from(rect.right - rect.left),
            height: f64::from(rect.bottom - rect.top),
        },
        handle,
        gdi: info.szDevice,
    })
}

pub fn screens() -> Result<Vec<Screen>> {
    let _coordinates = PhysicalCoordinates::enter()?;
    unsafe extern "system" fn collect(
        handle: HMONITOR,
        _: HDC,
        _: *mut RECT,
        output: LPARAM,
    ) -> i32 {
        let output = unsafe { &mut *(output as *mut Vec<HMONITOR>) };
        output.push(handle);
        1
    }
    let mut monitors = Vec::new();
    if unsafe {
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(collect),
            &mut monitors as *mut Vec<_> as LPARAM,
        )
    } == 0
    {
        return Err(io::Error::last_os_error()).context("enumerating Windows displays");
    }
    monitors.into_iter().map(screen).collect()
}

fn native_epoch(desktop: &mut [(String, ScreenRect, String)]) -> u64 {
    desktop.sort();
    let mut hash = Sha256::new();
    hash.update(b"Desk Switch Windows physical desktop v1\0");
    for (name, rect, identity) in desktop {
        for value in [name.as_bytes(), identity.as_bytes()] {
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value);
        }
        hash.update(rect.x.to_le_bytes());
        hash.update(rect.y.to_le_bytes());
        hash.update(rect.width.to_le_bytes());
        hash.update(rect.height.to_le_bytes());
    }
    let digest = hash.finalize();
    u64::from_le_bytes(digest[..8].try_into().unwrap()).max(1)
}

/// Read-only topology snapshot. This never opens a DDC/CI monitor handle.
pub fn native_screens() -> Result<Vec<NativeScreen>> {
    let _coordinates = PhysicalCoordinates::enter()?;
    let mut desktop = Vec::new();
    let mut available = Vec::new();
    for screen in screens()? {
        let rect = ScreenRect {
            x: screen.rect.x as i32,
            y: screen.rect.y as i32,
            width: screen.rect.width as u32,
            height: screen.rect.height as u32,
        };
        rect.validate()?;
        let identity = identity(&screen).ok().map(|(id, _)| id);
        desktop.push((
            screen.name.clone(),
            rect,
            identity.clone().unwrap_or_default(),
        ));
        if identity.is_some() {
            available.push((screen.name, rect));
        }
    }
    let epoch = native_epoch(&mut desktop);
    Ok(available
        .into_iter()
        .map(|(native_id, rect)| NativeScreen {
            native_id,
            rect,
            epoch,
        })
        .collect())
}

/// The receiver has no window loop. This quiet notification thread wakes only
/// for real desktop changes; native snapshots are refreshed by the sink itself.
pub fn display_changes() -> Result<tokio::sync::watch::Receiver<u64>> {
    static CHANGES: OnceLock<std::result::Result<tokio::sync::watch::Sender<u64>, String>> =
        OnceLock::new();
    let sender = CHANGES.get_or_init(|| {
        let (changes, _) = tokio::sync::watch::channel(0u64);
        let thread_changes = changes.clone();
        let (ready, wait) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("Desk Switch display notifications".into())
            .spawn(move || {
                let module = unsafe { GetModuleHandleW(null()) };
                let class = WNDCLASSW {
                    lpfnWndProc: Some(topology_notification),
                    hInstance: module,
                    lpszClassName: w!("DeskSwitchReceiverDisplayNotifications"),
                    ..Default::default()
                };
                unsafe {
                    RegisterClassW(&class);
                }
                let mut sender = Box::new(thread_changes);
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
                        (&mut *sender as *mut tokio::sync::watch::Sender<u64>).cast(),
                    )
                };
                if window.is_null() {
                    let _ = ready.send(Err(io::Error::last_os_error().to_string()));
                    return;
                }
                if ready.send(Ok(())).is_ok() {
                    let mut message = MSG::default();
                    while unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0 {
                        unsafe {
                            DispatchMessageW(&message);
                        }
                    }
                }
                unsafe {
                    DestroyWindow(window);
                }
            })
            .map_err(|error| error.to_string())?;
        wait.recv_timeout(Duration::from_secs(3))
            .map_err(|error| {
                format!("Windows display notification thread did not start: {error}")
            })??;
        Ok(changes)
    });
    match sender {
        Ok(sender) => Ok(sender.subscribe()),
        Err(error) => bail!("{error}"),
    }
}

unsafe extern "system" fn topology_notification(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let creation = unsafe { &*(lparam as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, creation.lpCreateParams as isize);
        }
    } else if matches!(
        message,
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DEVICECHANGE
    ) {
        let sender = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) }
            as *const tokio::sync::watch::Sender<u64>;
        if let Some(sender) = unsafe { sender.as_ref() } {
            sender.send_modify(|generation| *generation = generation.wrapping_add(1));
        }
        return 0;
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

struct DeviceInfo(HDEVINFO);
impl Drop for DeviceInfo {
    fn drop(&mut self) {
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

struct RegistryKey(HKEY);
impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn edid(path: &[u16]) -> Result<Vec<u8>> {
    let list = unsafe { SetupDiCreateDeviceInfoList(null(), null_mut()) };
    ensure!(
        list != INVALID_HANDLE_VALUE as isize,
        "cannot open Windows display device information"
    );
    let list = DeviceInfo(list);
    let mut interface = SP_DEVICE_INTERFACE_DATA {
        cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
        ..Default::default()
    };
    if unsafe { SetupDiOpenDeviceInterfaceW(list.0, path.as_ptr(), 0, &mut interface) } == 0 {
        return Err(io::Error::last_os_error()).context("opening the physical monitor interface");
    }
    let mut size = 0;
    unsafe {
        SetupDiGetDeviceInterfaceDetailW(list.0, &interface, null_mut(), 0, &mut size, null_mut());
    }
    ensure!(
        (8..=8192).contains(&size),
        "invalid monitor interface path size"
    );
    let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    let detail = buffer
        .as_mut_ptr()
        .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    #[cfg(target_arch = "x86")]
    let header_size = 6;
    #[cfg(not(target_arch = "x86"))]
    let header_size = 8;
    unsafe {
        (*detail).cbSize = header_size;
    }
    let mut device = SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    };
    if unsafe {
        SetupDiGetDeviceInterfaceDetailW(list.0, &interface, detail, size, null_mut(), &mut device)
    } == 0
    {
        return Err(io::Error::last_os_error()).context("finding the monitor device node");
    }
    let key = unsafe {
        SetupDiOpenDevRegKey(
            list.0,
            &device,
            DICS_FLAG_GLOBAL,
            0,
            DIREG_DEV,
            KEY_QUERY_VALUE,
        )
    };
    if key == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error()).context("opening the physical monitor EDID");
    }
    let key = RegistryKey(key);
    let mut edid = vec![0; 4096];
    let mut length = edid.len() as u32;
    let mut kind = 0;
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            w!("EDID"),
            null(),
            &mut kind,
            edid.as_mut_ptr(),
            &mut length,
        )
    };
    ensure!(
        status == 0
            && kind == REG_BINARY
            && (128..=4096).contains(&length)
            && edid.starts_with(&[0, 255, 255, 255, 255, 255, 255, 0]),
        "Windows did not expose a valid physical monitor EDID (status {status})"
    );
    edid.truncate(length as usize);
    Ok(edid)
}

fn identity(screen: &Screen) -> Result<(String, String)> {
    let mut found = None;
    for index in 0..64 {
        let mut device = DISPLAY_DEVICEW {
            cb: size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        if unsafe {
            EnumDisplayDevicesW(
                screen.gdi.as_ptr(),
                index,
                &mut device,
                EDD_GET_DEVICE_INTERFACE_NAME,
            )
        } == 0
        {
            break;
        }
        if device.StateFlags & DISPLAY_DEVICE_ACTIVE != 0 {
            ensure!(
                found.is_none(),
                "multiple physical outputs share this Windows screen"
            );
            found = Some(device);
        }
    }
    let device = found.context("no active physical monitor interface for this Windows screen")?;
    let path = wide_text(&device.DeviceID).to_ascii_lowercase();
    ensure!(!path.is_empty(), "empty Windows monitor interface path");
    let fingerprint = hex(&Sha256::digest(edid(&device.DeviceID)?));
    Ok((
        format!("{path}|{fingerprint}"),
        wide_text(&device.DeviceString),
    ))
}

struct Physical(PHYSICAL_MONITOR);
impl Drop for Physical {
    fn drop(&mut self) {
        unsafe {
            DestroyPhysicalMonitors(1, &self.0);
        }
    }
}

fn physical(screen: &Screen) -> Result<Physical> {
    let mut count = 0;
    if unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(screen.handle, &mut count) } == 0 {
        return Err(io::Error::last_os_error()).context("enumerating Windows DDC/CI handles");
    }
    ensure!(
        count == 1,
        "expected one physical monitor for this screen; found {count}"
    );
    let mut monitor = PHYSICAL_MONITOR::default();
    if unsafe { GetPhysicalMonitorsFromHMONITOR(screen.handle, count, &mut monitor) } == 0 {
        return Err(io::Error::last_os_error()).context("opening Windows DDC/CI handle");
    }
    Ok(Physical(monitor))
}

fn select(id: &str) -> Result<Physical> {
    let mut matches = Vec::new();
    for screen in screens()? {
        if identity(&screen).is_ok_and(|(found, _)| found == id) {
            matches.push(screen);
        }
    }
    ensure!(
        matches.len() <= 1,
        "configured Windows physical display identity is ambiguous"
    );
    let screen = matches
        .pop()
        .context("configured monitor identity is unavailable; run probe on its active input")?;
    physical(&screen)
}

fn read(handle: HANDLE) -> Result<u16> {
    let mut value = 0;
    if unsafe { GetVCPFeatureAndVCPFeatureReply(handle, 0x60, null_mut(), &mut value, null_mut()) }
        == 0
    {
        return Err(io::Error::last_os_error()).context("Windows DDC/CI VCP 0x60 read failed");
    }
    value
        .try_into()
        .context("VCP 0x60 returned an out-of-range value")
}

pub struct Ddc(pub String);

impl Monitor for Ddc {
    fn read(&mut self) -> Result<u16> {
        read(select(&self.0)?.0.hPhysicalMonitor)
    }

    fn write(&mut self, input: u16) -> Result<()> {
        let monitor = select(&self.0).map_err(crate::video::not_issued)?;
        println!(
            "DDC write issued: process {}, input {input:#06x}; exactly one SetVCPFeature call.",
            std::process::id()
        );
        if unsafe { SetVCPFeature(monitor.0.hPhysicalMonitor, 0x60, input.into()) } == 0 {
            return Err(io::Error::last_os_error()).context("Windows DDC/CI VCP 0x60 write failed");
        }
        Ok(())
    }
}

pub fn probe(only: Option<&str>) -> Result<()> {
    if let Some(id) = only {
        println!("Monitor ID: {id}");
        return probe_physical(&select(id)?);
    }
    let screens = screens()?;
    if screens.is_empty() {
        bail!("no Windows displays are active");
    }
    for screen in &screens {
        println!("Screen '{}' at {:?}", screen.name, screen.rect);
        let result = identity(screen).and_then(|(id, name)| {
            println!("  Monitor ID: {id} ({name})");
            probe_physical(&physical(screen)?)
        });
        if let Err(error) = result {
            println!("  {error:#}");
        }
    }
    Ok(())
}

fn probe_physical(monitor: &Physical) -> Result<()> {
    let handle = monitor.0.hPhysicalMonitor;
    println!("  Current VCP 0x60: {:#06x}", read(handle)?);
    let codes = advertised_inputs(handle);
    if codes.is_empty() {
        println!("  Monitor did not advertise an input list; read each active input during setup.");
    } else {
        println!("  Advertised inputs: {codes:#06x?}");
    }
    Ok(())
}

fn advertised_inputs(handle: HANDLE) -> Vec<u16> {
    let mut length = 0;
    let mut codes = Vec::new();
    if unsafe { GetCapabilitiesStringLength(handle, &mut length) } != 0
        && (1..=32768).contains(&length)
    {
        let mut buffer = vec![0; length as usize];
        if unsafe { CapabilitiesRequestAndCapabilitiesReply(handle, buffer.as_mut_ptr(), length) }
            != 0
            && let Ok(capabilities) = CStr::from_bytes_until_nul(&buffer)
        {
            codes = input_codes(&capabilities.to_string_lossy());
        }
    }
    codes
}

pub fn discover() -> Result<Vec<super::MonitorInfo>> {
    let native = native_screens().unwrap_or_default();
    Ok(screens()?
        .iter()
        .map(|screen| {
            let identity = identity(screen);
            let (id, name) = match identity {
                Ok(found) => found,
                Err(error) => {
                    return super::MonitorInfo {
                        id: String::new(),
                        name: screen.name.clone(),
                        screen: screen.name.clone(),
                        current: None,
                        inputs: Vec::new(),
                        error: Some(format!("{error:#}")),
                        fingerprint: String::new(),
                        native: None,
                    };
                }
            };
            let result = physical(screen).and_then(|physical| {
                let handle = physical.0.hPhysicalMonitor;
                Ok((read(handle)?, advertised_inputs(handle)))
            });
            let (current, inputs, error) = match result {
                Ok((current, inputs)) => (Some(current), inputs, None),
                Err(error) => (None, Vec::new(), Some(format!("{error:#}"))),
            };
            super::MonitorInfo {
                id,
                name,
                screen: screen.name.clone(),
                current,
                inputs,
                error,
                fingerprint: String::new(),
                native: native
                    .iter()
                    .find(|native| native.native_id.eq_ignore_ascii_case(&screen.name))
                    .cloned(),
            }
        })
        .collect())
}

pub fn list_displays() -> Result<Vec<super::MonitorInfo>> {
    let native = native_screens().unwrap_or_default();
    Ok(screens()?
        .into_iter()
        .filter_map(|screen| {
            let (id, name) = identity(&screen).ok()?;
            let mut fingerprint = String::new();
            for index in 0..64 {
                let mut device = DISPLAY_DEVICEW {
                    cb: size_of::<DISPLAY_DEVICEW>() as u32,
                    ..Default::default()
                };
                if unsafe {
                    EnumDisplayDevicesW(
                        screen.gdi.as_ptr(),
                        index,
                        &mut device,
                        EDD_GET_DEVICE_INTERFACE_NAME,
                    )
                } == 0
                {
                    break;
                }
                if device.StateFlags & DISPLAY_DEVICE_ACTIVE != 0 {
                    if let Ok(data) = edid(&device.DeviceID) {
                        fingerprint = crate::workspace::hardware::display_key(&data);
                    }
                    break;
                }
            }
            Some(super::MonitorInfo {
                id,
                name,
                native: native
                    .iter()
                    .find(|native| native.native_id.eq_ignore_ascii_case(&screen.name))
                    .cloned(),
                screen: screen.name,
                current: None,
                inputs: Vec::new(),
                error: None,
                fingerprint,
            })
        })
        .collect())
}

pub fn read_monitor(id: &str) -> Result<super::MonitorInfo> {
    let mut display = list_displays()?
        .into_iter()
        .find(|d| d.id == id)
        .context("Выбранный монитор отключён")?;
    display.current = Some(crate::video::read_once(Ddc(id.into()))?);
    Ok(display)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop() -> Vec<(String, ScreenRect, String)> {
        vec![
            (
                "DISPLAY1".into(),
                ScreenRect {
                    x: -1920,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
                "physical-a".into(),
            ),
            (
                "DISPLAY2".into(),
                ScreenRect {
                    x: 0,
                    y: -200,
                    width: 2560,
                    height: 1440,
                },
                "physical-b".into(),
            ),
        ]
    }

    #[test]
    fn topology_epoch_is_order_independent_and_invalidates_hotplug_or_layout_changes() {
        let baseline = native_epoch(&mut desktop());
        assert_ne!(baseline, 0);
        let mut reordered = desktop();
        reordered.reverse();
        assert_eq!(baseline, native_epoch(&mut reordered));
        let mut moved = desktop();
        moved[0].1.y -= 1;
        assert_ne!(baseline, native_epoch(&mut moved));
        let mut replaced = desktop();
        replaced[0].2 = "physical-c".into();
        assert_ne!(baseline, native_epoch(&mut replaced));
        let mut removed = desktop();
        removed.pop();
        assert_ne!(baseline, native_epoch(&mut removed));
    }
}
