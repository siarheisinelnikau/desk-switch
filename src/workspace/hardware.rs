use super::Hardware;
use crate::{audio::AudioDevice, platform::MonitorInfo};
use anyhow::Result;
#[cfg(any(windows, target_os = "linux", test))]
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

#[derive(Clone, PartialEq)]
pub struct Snapshot {
    pub hardware: Hardware,
    pub inputs: Vec<InputEndpoint>,
    pub devices: super::devices::Inventory,
    pub displays: Vec<MonitorInfo>,
    pub error: Option<String>,
}
pub struct Probe {
    stop: mpsc::Sender<()>,
    results: Arc<Mutex<Option<Snapshot>>>,
}
impl Probe {
    pub fn start(ctx: eframe::egui::Context) -> Result<Self> {
        let (stop, cancel) = mpsc::channel();
        let results = Arc::new(Mutex::new(None));
        let send = Arc::clone(&results);
        std::thread::Builder::new()
            .name("Desk Switch devices".into())
            .spawn(move || {
                let mut previous = None;
                loop {
                    if cancel.try_recv().is_ok() {
                        break;
                    }
                    let (inputs, error) = match input_devices() {
                        Ok(inputs) => (inputs, None),
                        Err(error) => (Vec::new(), Some(error.to_string())),
                    };
                    let displays = crate::platform::list_displays().unwrap_or_default();
                    let mut inventory = super::devices::Inventory {
                        displays: display_inventory(&displays),
                        keyboards: inputs
                            .iter()
                            .filter(|d| d.keyboard)
                            .map(|d| d.info.clone())
                            .take(32)
                            .collect(),
                        mice: inputs
                            .iter()
                            .filter(|d| d.mouse)
                            .map(|d| d.info.clone())
                            .take(32)
                            .collect(),
                        speakers: crate::audio::devices()
                            .unwrap_or_default()
                            .into_iter()
                            .take(32)
                            .collect(),
                        microphones: crate::audio::microphones()
                            .unwrap_or_default()
                            .into_iter()
                            .take(32)
                            .collect(),
                        microphone_bridges: crate::audio::microphone_bridges()
                            .unwrap_or_default()
                            .into_iter()
                            .take(32)
                            .collect(),
                        individual_input: cfg!(target_os = "linux"),
                    };
                    for list in [
                        &mut inventory.keyboards,
                        &mut inventory.mice,
                        &mut inventory.speakers,
                        &mut inventory.microphones,
                        &mut inventory.microphone_bridges,
                    ] {
                        list.retain(|d| {
                            !d.id.is_empty()
                                && d.id.len() <= 2048
                                && !d.id.chars().any(char::is_control)
                        });
                        for device in list.iter_mut() {
                            device.name = device
                                .name
                                .chars()
                                .filter(|c| !c.is_control())
                                .take(256)
                                .collect();
                        }
                        list.sort_by(|a, b| a.id.cmp(&b.id));
                        list.dedup_by(|a, b| a.id == b.id);
                    }
                    let hardware = hardware(
                        &inventory,
                        &super::devices::Choices::default(),
                        error.is_none(),
                    );
                    let snapshot = Snapshot {
                        hardware,
                        inputs,
                        devices: inventory,
                        displays,
                        error,
                    };
                    // Replace a queued scan: a busy GUI must get the newest
                    // hotplug state, never the first now-stale device list.
                    if previous.as_ref() != Some(&snapshot) {
                        previous = Some(snapshot.clone());
                        *send.lock().unwrap() = Some(snapshot);
                        ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
                    }
                    if !matches!(
                        cancel.recv_timeout(Duration::from_secs(3)),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    ) {
                        break;
                    }
                }
            })?;
        Ok(Self { stop, results })
    }
    pub fn poll(&self) -> Option<Snapshot> {
        self.results.lock().unwrap().take()
    }
}

fn display_inventory(displays: &[MonitorInfo]) -> Vec<super::devices::Display> {
    let mut result = Vec::new();
    for display in displays {
        let mut name = display.name.clone();
        name.retain(|character| !character.is_control());
        name.truncate(name.floor_char_boundary(256));
        let display = super::devices::Display {
            id: display.id.clone(),
            name,
            fingerprint: display.fingerprint.clone(),
        };
        if display.validate().is_ok()
            && !result
                .iter()
                .any(|present: &super::devices::Display| present.id == display.id)
        {
            result.push(display);
            if result.len() == 32 {
                break;
            }
        }
    }
    result
}
impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

pub fn audio_score(output: &AudioDevice) -> u8 {
    let name = format!("{} {}", output.id, output.name).to_lowercase();
    if [
        "headphone",
        "headset",
        "наушник",
        "casque",
        "kopfhörer",
        "auricular",
        "cuffie",
        "bluez",
        "bluetooth",
    ]
    .iter()
    .any(|s| name.contains(s))
    {
        40
    } else if ["hdmi", "displayport"].iter().any(|s| name.contains(s)) {
        10
    } else {
        20
    }
}

// EDID manufacturer/product + a real serial survive a connector/input change.
// With no serial, require an exact base block instead of guessing between displays.
#[cfg(any(windows, target_os = "linux", test))]
pub fn display_key(edid: &[u8]) -> String {
    if edid.len() < 128 {
        return String::new();
    }
    let serial = edid[12..16].iter().any(|b| *b != 0);
    let descriptor = edid[54..126]
        .as_chunks::<18>()
        .0
        .iter()
        .find(|d| d[..5] == [0, 0, 0, 255, 0])
        .map(|d| {
            d[5..18]
                .iter()
                .copied()
                .filter(|b| b.is_ascii_graphic())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let bytes = if serial || !descriptor.is_empty() {
        let mut bytes = edid[8..16].to_vec();
        bytes.extend(descriptor);
        bytes
    } else {
        edid[..128].to_vec()
    };
    crate::config::hex(&Sha256::digest(bytes))
}

#[cfg(target_os = "linux")]
fn bit(value: &str, code: usize) -> bool {
    value
        .split_whitespace()
        .rev()
        .nth(code / 64)
        .and_then(|word| u64::from_str_radix(word, 16).ok())
        .is_some_and(|word| word & (1u64 << (code % 64)) != 0)
}

#[cfg(target_os = "linux")]
pub fn absolute_pointer(has_axis: impl Fn(usize) -> bool) -> bool {
    // ABS_VOLUME and other auxiliary HID axes do not make a relative mouse
    // absolute. Only position axes identify a touchpad/tablet/touchscreen.
    [0, 1, 53, 54].into_iter().any(has_axis)
}

#[cfg(target_os = "linux")]
fn input_capabilities(key: &str, rel: &str, abs: &str) -> (bool, bool) {
    if absolute_pointer(|code| bit(abs, code)) {
        return (false, false);
    }
    (
        [30, 57, 87, 88].into_iter().all(|code| bit(key, code)),
        bit(key, 272) && bit(rel, 0) && bit(rel, 1),
    )
}

#[cfg(target_os = "linux")]
pub fn input_devices() -> Result<Vec<InputEndpoint>> {
    use std::os::unix::fs::MetadataExt;
    let mut result = Vec::new();
    let mut aliases = std::collections::BTreeMap::new();
    for directory in ["/dev/input/by-path", "/dev/input/by-id"] {
        if let Ok(entries) = std::fs::read_dir(directory) {
            for entry in entries.flatten() {
                if let Ok(target) = std::fs::canonicalize(entry.path()) {
                    aliases.insert(target, entry.path());
                }
            }
        }
    }
    for entry in std::fs::read_dir("/sys/class/input")? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("event") {
            continue;
        }
        let device = entry.path().join("device");
        let label = std::fs::read_to_string(device.join("name"))
            .unwrap_or_default()
            .to_lowercase();
        let bus = std::fs::read_to_string(device.join("id/bustype")).unwrap_or_default();
        if bus.trim() == "0006"
            || ["desk switch", "desk-switch", "virtual", "reis "]
                .iter()
                .any(|s| label.contains(s))
        {
            continue;
        }
        let key = std::fs::read_to_string(device.join("capabilities/key")).unwrap_or_default();
        let rel = std::fs::read_to_string(device.join("capabilities/rel")).unwrap_or_default();
        let abs = std::fs::read_to_string(device.join("capabilities/abs")).unwrap_or_default();
        let (k, m) = input_capabilities(&key, &rel, &abs);
        if (k || m) && result.len() < 32 {
            let path = PathBuf::from("/dev/input").join(name.as_ref());
            let id = aliases
                .get(&path)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| {
                    format!(
                        "sysfs:{}",
                        std::fs::canonicalize(&device)
                            .unwrap_or(device.clone())
                            .display()
                    )
                });
            let label =
                std::fs::read_to_string(device.join("name")).unwrap_or_else(|_| name.to_string());
            result.push(InputEndpoint {
                info: AudioDevice {
                    id,
                    name: label.trim().chars().take(256).collect(),
                    default: false,
                },
                generation: std::fs::metadata(&path).ok().map(|m| (m.dev(), m.ino())),
                path,
                keyboard: k,
                mouse: m,
            });
        }
    }
    result.sort_by(|a, b| a.info.id.cmp(&b.info.id));
    Ok(result)
}

#[cfg(any(windows, test))]
fn full_windows_keyboard(keys: u32, function_keys: u32) -> bool {
    // Media keys, macro pads and a mouse's auxiliary keyboard collection do
    // not establish that a usable keyboard is attached.
    keys >= 60 && function_keys >= 12
}

#[cfg(any(windows, test))]
fn physical_windows_bus(instance: &str) -> bool {
    let instance = instance.to_ascii_uppercase();
    [
        "USB\\",
        "BTH\\",
        "BTHENUM\\",
        "BTHLEDEVICE\\",
        "ACPI\\",
        "VMBUS\\",
    ]
    .iter()
    .any(|prefix| instance.starts_with(prefix))
}

#[cfg(windows)]
fn physical_windows_device(interface: &str) -> bool {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
    let Some(instance) = interface
        .strip_prefix("\\\\?\\")
        .and_then(|s| s.split("#{").next())
    else {
        return false;
    };
    let instance = instance.replace('#', "\\");
    let wide = instance.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut node = 0;
    if unsafe { CM_Locate_DevNodeW(&mut node, wide.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) }
        != CR_SUCCESS
    {
        return false;
    }
    for _ in 0..24 {
        let mut id = [0u16; 512];
        if unsafe { CM_Get_Device_IDW(node, id.as_mut_ptr(), id.len() as u32, 0) } != CR_SUCCESS {
            return false;
        }
        let end = id.iter().position(|&c| c == 0).unwrap_or(id.len());
        if physical_windows_bus(&String::from_utf16_lossy(&id[..end])) {
            return true;
        }
        let mut parent = 0;
        if unsafe { CM_Get_Parent(&mut parent, node, 0) } != CR_SUCCESS {
            return false;
        }
        node = parent;
    }
    false
}

#[cfg(windows)]
pub fn input_devices() -> Result<Vec<InputEndpoint>> {
    use windows_sys::Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError},
        UI::Input::*,
    };
    let size = std::mem::size_of::<RAWINPUTDEVICELIST>() as u32;
    let mut devices = Vec::new();
    // A device can arrive between the count and list queries. Retry the scan
    // instead of publishing yesterday's inventory or failing the whole pair.
    for attempt in 0..4 {
        let mut count = 0;
        anyhow::ensure!(
            unsafe { GetRawInputDeviceList(std::ptr::null_mut(), &mut count, size) } != u32::MAX,
            "Could not enumerate Windows input devices: {}",
            std::io::Error::last_os_error()
        );
        anyhow::ensure!(count <= 1024, "too many input devices");
        if count == 0 {
            return Ok(Vec::new());
        }
        devices.resize(count as usize, RAWINPUTDEVICELIST::default());
        let length = unsafe { GetRawInputDeviceList(devices.as_mut_ptr(), &mut count, size) };
        if length != u32::MAX {
            devices.truncate(length as usize);
            break;
        }
        anyhow::ensure!(
            unsafe { GetLastError() } == ERROR_INSUFFICIENT_BUFFER && attempt < 3,
            "Could not refresh Windows input devices: {}",
            std::io::Error::last_os_error()
        );
    }
    let mut keyboards = Vec::new();
    let mut mice = Vec::new();
    for device in devices {
        if !matches!(device.dwType, RIM_TYPEKEYBOARD | RIM_TYPEMOUSE) {
            continue;
        }
        let mut info = RID_DEVICE_INFO {
            cbSize: std::mem::size_of::<RID_DEVICE_INFO>() as u32,
            ..Default::default()
        };
        let mut length = info.cbSize;
        if unsafe {
            GetRawInputDeviceInfoW(
                device.hDevice,
                RIDI_DEVICEINFO,
                (&mut info as *mut RID_DEVICE_INFO).cast(),
                &mut length,
            )
        } == u32::MAX
            || info.dwType != device.dwType
        {
            continue;
        }
        let supported = unsafe {
            match info.dwType {
                RIM_TYPEKEYBOARD => full_windows_keyboard(
                    info.Anonymous.keyboard.dwNumberOfKeysTotal,
                    info.Anonymous.keyboard.dwNumberOfFunctionKeys,
                ),
                RIM_TYPEMOUSE => info.Anonymous.mouse.dwNumberOfButtons > 0,
                _ => false,
            }
        };
        if !supported {
            continue;
        }
        let mut length = 0;
        if unsafe {
            GetRawInputDeviceInfoW(
                device.hDevice,
                RIDI_DEVICENAME,
                std::ptr::null_mut(),
                &mut length,
            )
        } == u32::MAX
            || !(1..=2048).contains(&length)
        {
            continue;
        }
        let mut name = vec![0u16; length as usize];
        if unsafe {
            GetRawInputDeviceInfoW(
                device.hDevice,
                RIDI_DEVICENAME,
                name.as_mut_ptr().cast(),
                &mut length,
            )
        } == u32::MAX
        {
            continue;
        }
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        let name = String::from_utf16_lossy(&name[..end]);
        if !physical_windows_device(&name) {
            continue;
        }
        if info.dwType == RIM_TYPEKEYBOARD {
            keyboards.push(name);
        } else {
            mice.push(name);
        }
    }
    let mut result = Vec::new();
    for (names, id, label, keyboard) in [
        (
            keyboards,
            "windows-keyboards",
            "Attached physical keyboards",
            true,
        ),
        (mice, "windows-mice", "Attached physical mice", false),
    ] {
        if !names.is_empty() {
            result.push(InputEndpoint {
                info: AudioDevice {
                    id: id.into(),
                    name: format!("{label} ({}): {}", names.len(), names.join(", "))
                        .chars()
                        .take(256)
                        .collect(),
                    default: true,
                },
                path: PathBuf::new(),
                keyboard,
                mouse: !keyboard,
            });
        }
    }
    Ok(result)
}

#[derive(Clone, Debug, PartialEq)]
pub struct InputEndpoint {
    pub info: AudioDevice,
    pub path: PathBuf,
    #[cfg(target_os = "linux")]
    pub generation: Option<(u64, u64)>,
    pub keyboard: bool,
    pub mouse: bool,
}

#[cfg(target_os = "macos")]
pub fn input_devices() -> Result<Vec<InputEndpoint>> {
    #[derive(serde::Deserialize)]
    struct Devices {
        keyboards: Vec<String>,
        mice: Vec<String>,
    }
    let devices: Devices = crate::macos_native::query("input-devices", &[])?;
    let mut endpoints = Vec::new();
    for (id, names, keyboard, mouse) in [
        ("macos-keyboards", devices.keyboards, true, false),
        ("macos-mice", devices.mice, false, true),
    ] {
        if !names.is_empty() {
            endpoints.push(InputEndpoint {
                info: AudioDevice {
                    id: id.into(),
                    name: format!(
                        "Attached physical {}: {}",
                        if keyboard { "keyboards" } else { "pointers" },
                        names.join(", ")
                    ),
                    default: true,
                },
                path: PathBuf::new(),
                keyboard,
                mouse,
            });
        }
    }
    Ok(endpoints)
}

#[cfg(target_os = "linux")]
pub fn inputs() -> Result<(bool, bool, Vec<PathBuf>)> {
    let devices = input_devices()?;
    Ok((
        devices.iter().any(|d| d.keyboard),
        devices.iter().any(|d| d.mouse),
        devices.into_iter().map(|d| d.path).take(16).collect(),
    ))
}

pub fn hardware(
    inventory: &super::devices::Inventory,
    choices: &super::devices::Choices,
    known: bool,
) -> Hardware {
    use super::devices::selected;
    let speaker = selected(&inventory.speakers, choices.speaker.as_deref());
    let mic = selected(&inventory.microphones, choices.microphone.as_deref());
    Hardware {
        known,
        keyboard: selected(&inventory.keyboards, choices.keyboard.as_deref()).is_some(),
        mouse: selected(&inventory.mice, choices.mouse.as_deref()).is_some(),
        audio_score: speaker.map(audio_score).unwrap_or(0),
        audio_name: speaker.map(|d| d.name.clone()).unwrap_or_default(),
        microphone_score: mic
            .map(|d| {
                if audio_score(d) == 40 {
                    80
                } else if d.default {
                    60
                } else {
                    40
                }
            })
            .unwrap_or(0),
        microphone_name: mic.map(|d| d.name.clone()).unwrap_or_default(),
        microphone_receive: selected(
            &inventory.microphone_bridges,
            choices.microphone_bridge.as_deref(),
        )
        .is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_consumer_controls_and_software_buses_do_not_supply_a_keyboard() {
        assert!(!full_windows_keyboard(8, 0));
        assert!(!full_windows_keyboard(32, 12));
        assert!(full_windows_keyboard(101, 12));
        assert!(full_windows_keyboard(256, 24));
        for physical in ["USB\\VID_1234", "BTHENUM\\VID_1234", "ACPI\\PNP0303"] {
            assert!(physical_windows_bus(physical));
        }
        for software in ["ROOT\\VHF", "SWD\\HID", "ROOT\\RDP_KBD", "HID\\VID_1234"] {
            assert!(!physical_windows_bus(software));
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn sysfs_bitmaps_are_decoded_from_the_least_significant_word() {
        assert!(bit("1 0", 64));
        assert!(!bit("1 0", 0));
        assert!(bit("3", 1));
        assert!(!bit("garbage", 30));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn composite_relative_mouse_with_auxiliary_absolute_volume_is_supported() {
        let key = "3f00733fff 0 0 483ffff17aff32d bfd4444600000000 ffff0001 130ff38b17d007 ffe77bfad9415fed fdbeffdf77cfffff fffffffffffffffe";
        assert_eq!(input_capabilities(key, "1943", "100000000"), (true, true));
        // Real pointer position axes must still never enter relative capture.
        for abs in ["1", "2", "20000000000000", "40000000000000"] {
            assert_eq!(input_capabilities(key, "1943", abs), (false, false));
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn receiver_interfaces_are_classified_separately_from_consumer_controls() {
        let keyboard = "1000000000007 ff800000000007ff febeffdff3cfffff fffffffffffffffe";
        assert_eq!(input_capabilities(keyboard, "0", "0"), (true, false));
        assert_eq!(
            input_capabilities("1f0000 0 0 0 0", "1943", "0"),
            (false, true)
        );
        let consumer = "3f00733fff 0 0 483ffff17aff32d bfd4444600000000 1 130ff38b17d000 677bfad9415fed 19ed68000004400 10000002";
        assert_eq!(
            input_capabilities(consumer, "1040", "100000000"),
            (false, false)
        );
    }
    #[test]
    fn missing_selected_hardware_pauses_only_its_capability() {
        let device = AudioDevice {
            id: "present".into(),
            name: "USB Headset".into(),
            default: true,
        };
        let inventory = super::super::devices::Inventory {
            keyboards: vec![device.clone()],
            mice: vec![device.clone()],
            speakers: vec![device.clone()],
            microphones: vec![device],
            ..Default::default()
        };
        let choices = super::super::devices::Choices {
            keyboard: Some("removed".into()),
            microphone: Some("removed".into()),
            ..Default::default()
        };
        let hardware = hardware(&inventory, &choices, true);
        assert!(!hardware.keyboard && hardware.mouse);
        assert_eq!(hardware.microphone_score, 0);
        assert_eq!(hardware.audio_score, 40);
    }

    #[test]
    fn display_enumeration_publishes_bounded_selection_metadata_without_ddc_readback_fields() {
        let display = |id: String| MonitorInfo {
            id,
            name: format!("\n{}", "ж".repeat(300)),
            screen: "fixture".into(),
            current: Some(17),
            inputs: vec![15, 17],
            error: None,
            fingerprint: "ab".repeat(32),
        };
        let mut displays = (0..40)
            .map(|index| display(format!("display-{index}")))
            .collect::<Vec<_>>();
        displays.insert(0, display("x".repeat(1025)));
        displays.insert(1, displays[2].clone());
        let published = display_inventory(&displays);
        assert_eq!(published.len(), 32);
        let inventory = super::super::devices::Inventory {
            displays: published.clone(),
            ..Default::default()
        };
        inventory.validate().unwrap();
        assert!(
            published
                .iter()
                .all(|display| display.name.len() <= 256 && !display.name.contains('\n'))
        );
        let json = serde_json::to_value(&published[0]).unwrap();
        assert!(json.get("current").is_none() && json.get("inputs").is_none());
        assert_eq!(json["fingerprint"], "ab".repeat(32));
    }
    #[test]
    fn serial_identity_survives_port_specific_edid_changes() {
        let mut a = vec![0; 128];
        a[12] = 7;
        let mut b = a.clone();
        b[20] = 1;
        assert_eq!(display_key(&a), display_key(&b));
        b[12] = 8;
        assert_ne!(display_key(&a), display_key(&b));
        a[12] = 0;
        b[12] = 0;
        assert_ne!(display_key(&a), display_key(&b));
    }
}
