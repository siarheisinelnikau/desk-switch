use super::Hardware;
use crate::{audio::AudioDevice, platform::MonitorInfo};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::mpsc, time::Duration};

pub struct Snapshot {
    pub hardware: Hardware,
    pub inputs: Vec<InputEndpoint>,
    pub devices: super::devices::Inventory,
    pub displays: Vec<MonitorInfo>,
    pub error: Option<String>,
}
pub struct Probe {
    stop: mpsc::Sender<()>,
    results: mpsc::Receiver<Snapshot>,
}
impl Probe {
    pub fn start(ctx: eframe::egui::Context) -> Result<Self> {
        let (stop, cancel) = mpsc::channel();
        let (send, results) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("Desk Switch devices".into())
            .spawn(move || {
                loop {
                    if cancel.try_recv().is_ok() {
                        break;
                    }
                    let (inputs, error) = match input_devices() {
                        Ok(inputs) => (inputs, None),
                        Err(error) => (Vec::new(), Some(error.to_string())),
                    };
                    let mut inventory = super::devices::Inventory {
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
                        displays: crate::platform::list_displays().unwrap_or_default(),
                        error,
                    };
                    if send.try_send(snapshot).is_err() && cancel.try_recv().is_ok() {
                        break;
                    }
                    ctx.request_repaint();
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
        self.results.try_recv().ok()
    }
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
                path,
                keyboard: k,
                mouse: m,
            });
        }
    }
    result.sort_by(|a, b| a.info.id.cmp(&b.info.id));
    Ok(result)
}

#[cfg(windows)]
pub fn input_devices() -> Result<Vec<InputEndpoint>> {
    use windows_sys::Win32::UI::Input::*;
    let mut count = 0;
    let size = std::mem::size_of::<RAWINPUTDEVICELIST>() as u32;
    anyhow::ensure!(
        unsafe { GetRawInputDeviceList(std::ptr::null_mut(), &mut count, size) } != u32::MAX,
        "Не удалось проверить устройства ввода"
    );
    anyhow::ensure!(count <= 1024, "too many input devices");
    let mut devices = vec![RAWINPUTDEVICELIST::default(); count as usize];
    let length = unsafe { GetRawInputDeviceList(devices.as_mut_ptr(), &mut count, size) };
    anyhow::ensure!(
        length != u32::MAX,
        "Список устройств изменился. Повторяем проверку."
    );
    devices.truncate(length as usize);
    let mut result = Vec::new();
    for (kind, id, name) in [
        (
            RIM_TYPEKEYBOARD,
            "windows-keyboards",
            "All attached keyboards",
        ),
        (RIM_TYPEMOUSE, "windows-mice", "All attached mice"),
    ] {
        if devices.iter().any(|d| d.dwType == kind) {
            result.push(InputEndpoint {
                info: AudioDevice {
                    id: id.into(),
                    name: name.into(),
                    default: true,
                },
                path: PathBuf::new(),
                keyboard: kind == RIM_TYPEKEYBOARD,
                mouse: kind == RIM_TYPEMOUSE,
            });
        }
    }
    Ok(result)
}

#[derive(Clone, Debug)]
pub struct InputEndpoint {
    pub info: AudioDevice,
    pub path: PathBuf,
    pub keyboard: bool,
    pub mouse: bool,
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
