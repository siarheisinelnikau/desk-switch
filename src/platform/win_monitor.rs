use crate::{config::hex, edge::Rect, video::{Monitor, input_codes}};
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{ffi::CStr, io, mem::size_of, ptr::{null, null_mut}};
use windows_sys::{
    Win32::{
        Devices::{DeviceAndDriverInstallation::*, Display::*},
        Foundation::{HANDLE, INVALID_HANDLE_VALUE, LPARAM, RECT},
        Graphics::Gdi::*,
        System::Registry::*,
        UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME,
    },
    core::w,
};

pub struct Screen {
    pub name: String,
    pub rect: Rect,
    handle: HMONITOR,
    gdi: [u16; 32],
}

fn wide_text(text: &[u16]) -> String {
    let len = text.iter().position(|&value| value == 0).unwrap_or(text.len());
    String::from_utf16_lossy(&text[..len])
}

pub fn screen(handle: HMONITOR) -> Result<Screen> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if unsafe { GetMonitorInfoW(handle, &mut info.monitorInfo) } == 0 {
        return Err(io::Error::last_os_error()).context("reading Windows monitor geometry");
    }
    let rect = info.monitorInfo.rcMonitor;
    Ok(Screen {
        name: wide_text(&info.szDevice),
        rect: Rect {
            x: f64::from(rect.left), y: f64::from(rect.top),
            width: f64::from(rect.right - rect.left), height: f64::from(rect.bottom - rect.top),
        },
        handle,
        gdi: info.szDevice,
    })
}

pub fn screens() -> Result<Vec<Screen>> {
    unsafe extern "system" fn collect(handle: HMONITOR, _: HDC, _: *mut RECT, output: LPARAM) -> i32 {
        let output = unsafe { &mut *(output as *mut Vec<HMONITOR>) };
        output.push(handle);
        1
    }
    let mut monitors = Vec::new();
    if unsafe { EnumDisplayMonitors(null_mut(), null(), Some(collect), &mut monitors as *mut Vec<_> as LPARAM) } == 0 {
        return Err(io::Error::last_os_error()).context("enumerating Windows displays");
    }
    monitors.into_iter().map(screen).collect()
}

struct DeviceInfo(HDEVINFO);
impl Drop for DeviceInfo {
    fn drop(&mut self) {
        unsafe { SetupDiDestroyDeviceInfoList(self.0); }
    }
}

struct RegistryKey(HKEY);
impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0); }
    }
}

fn edid(path: &[u16]) -> Result<Vec<u8>> {
    let list = unsafe { SetupDiCreateDeviceInfoList(null(), null_mut()) };
    ensure!(list != INVALID_HANDLE_VALUE, "cannot open Windows display device information");
    let list = DeviceInfo(list);
    let mut interface = SP_DEVICE_INTERFACE_DATA { cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
    if unsafe { SetupDiOpenDeviceInterfaceW(list.0, path.as_ptr(), 0, &mut interface) } == 0 {
        return Err(io::Error::last_os_error()).context("opening the physical monitor interface");
    }
    let mut size = 0;
    unsafe { SetupDiGetDeviceInterfaceDetailW(list.0, &interface, null_mut(), 0, &mut size, null_mut()); }
    ensure!((8..=8192).contains(&size), "invalid monitor interface path size");
    let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    let detail = buffer.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    #[cfg(target_arch = "x86")]
    let header_size = 6;
    #[cfg(not(target_arch = "x86"))]
    let header_size = 8;
    unsafe { (*detail).cbSize = header_size; }
    let mut device = SP_DEVINFO_DATA { cbSize: size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
    if unsafe { SetupDiGetDeviceInterfaceDetailW(list.0, &interface, detail, size, null_mut(), &mut device) } == 0 {
        return Err(io::Error::last_os_error()).context("finding the monitor device node");
    }
    let key = unsafe { SetupDiOpenDevRegKey(list.0, &device, DICS_FLAG_GLOBAL, 0, DIREG_DEV, KEY_QUERY_VALUE) };
    if key == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error()).context("opening the physical monitor EDID");
    }
    let key = RegistryKey(key);
    let mut edid = vec![0; 4096];
    let mut length = edid.len() as u32;
    let mut kind = 0;
    let status = unsafe { RegQueryValueExW(key.0, w!("EDID"), null(), &mut kind, edid.as_mut_ptr(), &mut length) };
    ensure!(status == 0 && kind == REG_BINARY && (128..=4096).contains(&length)
        && edid.starts_with(&[0, 255, 255, 255, 255, 255, 255, 0]),
        "Windows did not expose a valid physical monitor EDID (status {status})");
    edid.truncate(length as usize);
    Ok(edid)
}

fn identity(screen: &Screen) -> Result<(String, String)> {
    let mut found = None;
    for index in 0..64 {
        let mut device = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        if unsafe { EnumDisplayDevicesW(screen.gdi.as_ptr(), index, &mut device, EDD_GET_DEVICE_INTERFACE_NAME) } == 0 {
            break;
        }
        if device.StateFlags & DISPLAY_DEVICE_ACTIVE != 0 {
            ensure!(found.is_none(), "multiple physical outputs share this Windows screen");
            found = Some(device);
        }
    }
    let device = found.context("no active physical monitor interface for this Windows screen")?;
    let path = wide_text(&device.DeviceID).to_ascii_lowercase();
    ensure!(!path.is_empty(), "empty Windows monitor interface path");
    let fingerprint = hex(&Sha256::digest(edid(&device.DeviceID)?));
    Ok((format!("{path}|{fingerprint}"), wide_text(&device.DeviceString)))
}

struct Physical(PHYSICAL_MONITOR);
impl Drop for Physical {
    fn drop(&mut self) {
        unsafe { DestroyPhysicalMonitors(1, &self.0); }
    }
}

fn physical(screen: &Screen) -> Result<Physical> {
    let mut count = 0;
    if unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(screen.handle, &mut count) } == 0 {
        return Err(io::Error::last_os_error()).context("enumerating Windows DDC/CI handles");
    }
    ensure!(count == 1, "expected one physical monitor for this screen; found {count}");
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
    ensure!(matches.len() <= 1, "configured Windows physical display identity is ambiguous");
    let screen = matches.pop().context("configured monitor identity is unavailable; run probe on its active input")?;
    physical(&screen)
}

fn read(handle: HANDLE) -> Result<u16> {
    let mut value = 0;
    if unsafe { GetVCPFeatureAndVCPFeatureReply(handle, 0x60, null_mut(), &mut value, null_mut()) } == 0 {
        return Err(io::Error::last_os_error()).context("Windows DDC/CI VCP 0x60 read failed");
    }
    value.try_into().context("VCP 0x60 returned an out-of-range value")
}

pub struct Ddc(pub String);

impl Monitor for Ddc {
    fn read(&mut self) -> Result<u16> {
        read(select(&self.0)?.0.hPhysicalMonitor)
    }

    fn write(&mut self, input: u16) -> Result<()> {
        let monitor = select(&self.0)?;
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
    let mut length = 0;
    let mut codes = Vec::new();
    if unsafe { GetCapabilitiesStringLength(handle, &mut length) } != 0 && (1..=32768).contains(&length) {
        let mut buffer = vec![0; length as usize];
        if unsafe { CapabilitiesRequestAndCapabilitiesReply(handle, buffer.as_mut_ptr(), length) } != 0
            && let Ok(capabilities) = CStr::from_bytes_until_nul(&buffer)
        {
            codes = input_codes(&capabilities.to_string_lossy());
        }
    }
    if codes.is_empty() {
        println!("  Monitor did not advertise an input list; read each active input during setup.");
    } else {
        println!("  Advertised inputs: {codes:#06x?}");
    }
    Ok(())
}
