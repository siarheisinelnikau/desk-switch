use super::Hardware;
use crate::{audio::AudioDevice, platform::MonitorInfo};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::mpsc, time::Duration};

pub struct Snapshot {
    pub hardware: Hardware,
    pub paths: Vec<PathBuf>,
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
                    let inputs = inputs();
                    let outputs = crate::audio::devices().unwrap_or_default();
                    let output = outputs
                        .iter()
                        .find(|d| d.default)
                        .or_else(|| outputs.first());
                    let (keyboard, mouse, paths, error) = match inputs {
                        Ok((k, m, p)) => (k, m, p, None),
                        Err(e) => (false, false, Vec::new(), Some(e.to_string())),
                    };
                    let hardware = Hardware {
                        known: error.is_none(),
                        keyboard,
                        mouse,
                        audio_score: output.map(audio_score).unwrap_or(0),
                        audio_name: output
                            .map(|d| d.name.chars().take(120).collect())
                            .unwrap_or_default(),
                    };
                    let snapshot = Snapshot {
                        hardware,
                        paths,
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

fn audio_score(output: &AudioDevice) -> u8 {
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
pub fn inputs() -> Result<(bool, bool, Vec<PathBuf>)> {
    let mut keyboard = false;
    let mut mouse = false;
    let mut paths = Vec::new();
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
        keyboard |= k;
        mouse |= m;
        if (k || m) && paths.len() < 16 {
            paths.push(PathBuf::from("/dev/input").join(name.as_ref()));
        }
    }
    paths.sort();
    Ok((keyboard, mouse, paths))
}

#[cfg(windows)]
pub fn inputs() -> Result<(bool, bool, Vec<PathBuf>)> {
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
    Ok((
        devices.iter().any(|d| d.dwType == RIM_TYPEKEYBOARD),
        devices.iter().any(|d| d.dwType == RIM_TYPEMOUSE),
        Vec::new(),
    ))
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
