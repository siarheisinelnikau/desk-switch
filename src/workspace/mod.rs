pub mod devices;
pub mod hardware;
pub mod net;

use crate::{config, lifecycle::InputStatus, video::Inputs};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const PORT: u16 = 45878;
pub const INPUT_PORT: u16 = 45876;
pub const AUDIO_PORT: u16 = 45877;
pub const MICROPHONE_PORT: u16 = 45879;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Hardware {
    pub known: bool,
    pub keyboard: bool,
    pub mouse: bool,
    pub audio_score: u8,
    pub audio_name: String,
    pub microphone_score: u8,
    pub microphone_name: String,
    pub microphone_receive: bool,
}
impl Hardware {
    pub fn input(&self) -> bool {
        self.keyboard && self.mouse
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Monitor {
    pub enabled: bool,
    pub id: String,
    pub name: String,
    pub fingerprint: String,
    pub input: Option<u16>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum Selection {
    #[default]
    Auto,
    Computer(String),
}

#[derive(Clone, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Revision {
    pub counter: u64,
    pub author: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Shared {
    pub revision: Revision,
    pub input: Selection,
    pub audio: Selection,
    pub audio_enabled: bool,
    pub microphone_enabled: bool,
    pub microphone: Selection,
    pub microphone_level: AudioLevel,
    pub microphone_binding: Option<devices::Binding>,
    pub devices: BTreeMap<String, devices::Choices>,
    pub volume: u8,
    pub muted: bool,
    pub audio_levels: BTreeMap<String, AudioLevel>,
    pub keyboard_enabled: bool,
    pub mouse_enabled: bool,
    pub monitor_enabled: bool,
    pub edge: bool,
    pub purple_on_left: bool,
}
impl Default for Shared {
    fn default() -> Self {
        Self {
            revision: Revision::default(),
            input: Selection::Auto,
            audio: Selection::Auto,
            audio_enabled: true,
            microphone_enabled: false,
            microphone: Selection::Auto,
            microphone_level: AudioLevel::default(),
            microphone_binding: None,
            devices: BTreeMap::new(),
            volume: 50,
            muted: false,
            audio_levels: BTreeMap::new(),
            keyboard_enabled: true,
            mouse_enabled: true,
            monitor_enabled: true,
            edge: false,
            purple_on_left: true,
        }
    }
}
impl Shared {
    pub fn devices_for(&self, key: &str) -> devices::Choices {
        self.devices.get(key).cloned().unwrap_or_default()
    }
    pub fn audio_level(&self, source: &str) -> AudioLevel {
        self.audio_levels.get(source).copied().unwrap_or_default()
    }

    pub fn prepare_audio_levels(&mut self, a: &str, b: &str, output: Option<&str>) -> bool {
        let mut changed = false;
        for source in [a, b] {
            if !self.audio_levels.contains_key(source) {
                let level = if Some(source) == output {
                    AudioLevel::default()
                } else {
                    AudioLevel {
                        volume: self.volume,
                        muted: self.muted,
                    }
                };
                self.audio_levels.insert(source.into(), level);
                changed = true;
            }
        }
        changed
    }
    pub fn changed(&mut self, author: &str) {
        self.revision.counter = self.revision.counter.saturating_add(1);
        self.revision.author = author.into();
    }
    pub fn merge(&mut self, other: &Self) -> bool {
        if other.revision > self.revision {
            *self = other.clone();
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct AudioLevel {
    pub volume: u8,
    pub muted: bool,
}
impl Default for AudioLevel {
    fn default() -> Self {
        Self {
            volume: 100,
            muted: false,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Preferences {
    pub shared: Shared,
    pub monitor: Monitor,
    pub pending_monitor: Option<Monitor>,
    pub peer_name: String,
    pub last_address: String,
    pub manual_address: String,
}

impl Preferences {
    pub fn record_monitor(&mut self, monitor: Monitor, borrowed_input: bool) {
        if borrowed_input {
            self.pending_monitor = Some(monitor);
        } else {
            self.monitor = monitor;
            self.pending_monitor = None;
        }
    }

    pub fn apply_pending_monitor(&mut self, input_is_local: bool) -> bool {
        if !input_is_local {
            return false;
        }
        let Some(monitor) = self.pending_monitor.take() else {
            return false;
        };
        if self.shared.monitor_enabled && monitor.id == self.monitor.id {
            self.monitor = monitor;
        }
        true
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Node {
    pub key: String,
    pub name: String,
    pub hardware: Hardware,
    #[serde(default)]
    pub devices: devices::Inventory,
    pub monitor: Monitor,
    pub shared: Shared,
    pub input: Option<InputStatus>,
    #[serde(default)]
    pub monitor_input: Option<u16>,
    pub audio_playing: bool,
    #[serde(default)]
    pub microphone_active: bool,
    #[serde(default)]
    pub microphone_error: Option<String>,
    pub audio_error: Option<String>,
    pub input_error: Option<String>,
    pub ack: String,
}
impl Node {
    pub fn validate(&self, own: &str, peer: &str) -> Result<()> {
        ensure!(
            self.key == peer && own != peer,
            "unexpected computer identity"
        );
        ensure!(
            self.name.len() <= 256 && self.name.chars().all(|c| !c.is_control()),
            "invalid computer name"
        );
        ensure!(
            self.hardware.audio_score <= 100
                && self.hardware.audio_name.len() <= 512
                && self.hardware.microphone_score <= 100
                && self.hardware.microphone_name.len() <= 512,
            "invalid audio information"
        );
        ensure!(
            self.monitor.id.len() <= 1024
                && self.monitor.name.len() <= 256
                && self.monitor.fingerprint.len() <= 128,
            "invalid monitor information"
        );
        ensure!(self.monitor.input != Some(0), "invalid monitor input");
        ensure!(self.shared.volume <= 100, "invalid volume");
        self.devices.validate()?;
        if let Some(binding) = &self.shared.microphone_binding {
            ensure!(
                [own, peer].contains(&binding.computer.as_str())
                    && !binding.device.is_empty()
                    && binding.device.len() <= 2048
                    && !binding.device.chars().any(char::is_control),
                "invalid microphone binding"
            );
        }

        ensure!(
            self.shared.microphone_level.volume <= 100 && self.shared.devices.len() <= 2,
            "invalid device settings"
        );
        for (key, choice) in &self.shared.devices {
            ensure!([own, peer].contains(&key.as_str()), "unknown device owner");
            for id in [
                &choice.keyboard,
                &choice.mouse,
                &choice.speaker,
                &choice.microphone,
                &choice.microphone_bridge,
            ]
            .into_iter()
            .flatten()
            {
                ensure!(
                    !id.is_empty() && id.len() <= 2048 && !id.chars().any(char::is_control),
                    "invalid device selection"
                );
            }
        }

        ensure!(
            self.shared.audio_levels.len() <= 2
                && self
                    .shared
                    .audio_levels
                    .iter()
                    .all(|(key, level)| [own, peer].contains(&key.as_str()) && level.volume <= 100),
            "invalid source audio levels"
        );
        ensure!(
            self.shared.revision.counter < u64::MAX
                && (self.shared.revision.author.is_empty()
                    || [own, peer].contains(&self.shared.revision.author.as_str())),
            "invalid settings revision"
        );
        for choice in [
            &self.shared.input,
            &self.shared.audio,
            &self.shared.microphone,
        ] {
            if let Selection::Computer(key) = choice {
                ensure!(
                    [own, peer].contains(&key.as_str()),
                    "unknown computer in settings"
                );
            }
        }
        ensure!(
            self.ack.is_empty()
                || (self.ack.len() == 64 && self.ack.bytes().all(|c| c.is_ascii_hexdigit())),
            "invalid synchronization acknowledgement"
        );
        for error in [&self.audio_error, &self.input_error, &self.microphone_error]
            .into_iter()
            .flatten()
        {
            ensure!(error.len() <= 2048, "status is too long");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Plan {
    pub purple: String,
    pub green: String,
    pub input_source: Option<String>,
    pub keyboard_enabled: bool,
    pub mouse_enabled: bool,
    pub audio_output: Option<String>,
    pub microphone_source: Option<String>,
    pub monitor: Option<Inputs>,
    pub edge: bool,
    pub purple_on_left: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct InputDevices<'a> {
    pub keyboard: Option<&'a str>,
    pub mouse: Option<&'a str>,
}

impl Plan {
    pub fn input_in_use(a: &Node, b: &Node) -> bool {
        let source_key = Self::build(a, b).input_source;
        [(a, b), (b, a)].into_iter().any(|(source, target)| {
            source_key.as_ref() == Some(&source.key)
                && source.input == Some(InputStatus::Peer)
                && target.input == Some(InputStatus::Here)
                && source.hardware.input()
        })
    }

    pub fn can_disable(a: &Node, b: &Node, keyboard: bool) -> bool {
        ![(a, b), (b, a)].into_iter().any(|(source, target)| {
            (source.input == Some(InputStatus::Peer) || target.input == Some(InputStatus::Here))
                && source.hardware.input()
                && !(if keyboard {
                    target.hardware.keyboard
                } else {
                    target.hardware.mouse
                })
        })
    }

    pub fn shared_monitor(a: &Node, b: &Node) -> bool {
        a.monitor.enabled
            && b.monitor.enabled
            && !a.monitor.fingerprint.is_empty()
            && a.monitor.fingerprint == b.monitor.fingerprint
    }

    pub fn common_monitor(a: &Node, b: &Node) -> bool {
        let shared = if a.shared.revision >= b.shared.revision {
            &a.shared
        } else {
            &b.shared
        };
        shared.monitor_enabled
    }

    // Local/other mapping is also available when both PCs have their own input
    // devices and no keyboard forwarding service is needed.
    pub fn monitor_inputs(a: &Node, b: &Node) -> Option<Inputs> {
        let shared = if a.shared.revision >= b.shared.revision {
            &a.shared
        } else {
            &b.shared
        };
        if !shared.monitor_enabled || !Self::shared_monitor(a, b) {
            return None;
        }
        let (host, peer) = (a.monitor.input?, b.monitor.input?);
        (host != 0 && peer != 0 && host != peer).then_some(Inputs { host, peer })
    }

    pub fn monitor_only_available(a: &Node, b: &Node) -> bool {
        a.hardware.input() && b.hardware.input() && Self::monitor_inputs(a, b).is_some()
    }

    pub fn monitor_owner<'a>(a: &'a Node, b: &'a Node, synced: bool) -> Option<&'a str> {
        if !synced
            || !Self::shared_monitor(a, b)
            || !a
                .monitor
                .input
                .zip(b.monitor.input)
                .is_some_and(|(a, b)| a != 0 && b != 0 && a != b)
        {
            return None;
        }
        let observed = match (a.monitor_input, b.monitor_input) {
            (Some(a), Some(b)) if a != b => return None,
            (Some(input), _) | (_, Some(input)) => Some(input),
            _ => None,
        }?;
        [a, b]
            .into_iter()
            .find(|n| n.monitor.input == Some(observed))
            .map(|n| n.key.as_str())
    }

    pub fn input_devices<'a>(
        &self,
        here: &'a Node,
        other: &'a Node,
        synced: bool,
    ) -> InputDevices<'a> {
        if !synced {
            return InputDevices::default();
        }
        let (source, target) = if self.input_source.as_ref() == Some(&here.key) {
            (here, other)
        } else {
            (other, here)
        };
        let forwarded = self.input_source.as_ref() == Some(&source.key)
            && source.hardware.input()
            && source.input == Some(InputStatus::Peer)
            && target.input == Some(InputStatus::Here)
            && source.input_error.is_none()
            && target.input_error.is_none();
        if forwarded {
            return if here.key == target.key {
                InputDevices {
                    keyboard: if self.keyboard_enabled {
                        Some(source.key.as_str())
                    } else {
                        here.hardware.keyboard.then_some(here.key.as_str())
                    },
                    mouse: if self.mouse_enabled {
                        Some(source.key.as_str())
                    } else {
                        here.hardware.mouse.then_some(here.key.as_str())
                    },
                }
            } else {
                InputDevices {
                    keyboard: (!self.keyboard_enabled && here.hardware.keyboard)
                        .then_some(here.key.as_str()),
                    mouse: (!self.mouse_enabled && here.hardware.mouse)
                        .then_some(here.key.as_str()),
                }
            };
        }
        // Presence is independent of capture permissions and connection readiness.
        InputDevices {
            keyboard: here.hardware.keyboard.then_some(here.key.as_str()),
            mouse: here.hardware.mouse.then_some(here.key.as_str()),
        }
    }

    pub fn build(a: &Node, b: &Node) -> Self {
        let (purple, green) = if a.key < b.key { (a, b) } else { (b, a) };
        let shared = if a.shared.revision >= b.shared.revision {
            &a.shared
        } else {
            &b.shared
        };
        let input_source = match &shared.input {
            Selection::Computer(key) => [a, b]
                .into_iter()
                .find(|n| &n.key == key && n.hardware.input()),
            Selection::Auto => match (a.hardware.input(), b.hardware.input()) {
                (true, false) => Some(a),
                (false, true) => Some(b),
                _ => None,
            },
        }
        .filter(|_| {
            shared.keyboard_enabled
                || shared.mouse_enabled
                || shared.monitor_enabled
                    && Self::shared_monitor(a, b)
                    && a.monitor
                        .input
                        .zip(b.monitor.input)
                        .is_some_and(|(a, b)| a != 0 && b != 0 && a != b)
        })
        .filter(|source| {
            let target = if source.key == a.key { b } else { a };
            (shared.keyboard_enabled || target.hardware.keyboard)
                && (shared.mouse_enabled || target.hardware.mouse)
        })
        .map(|n| n.key.clone());
        let audio_output = if !shared.audio_enabled {
            None
        } else {
            match &shared.audio {
                Selection::Computer(key) => [a, b]
                    .into_iter()
                    .find(|n| &n.key == key && n.hardware.audio_score > 0),
                Selection::Auto => [purple, green]
                    .into_iter()
                    .filter(|n| n.hardware.audio_score > 0)
                    .max_by_key(|n| {
                        (
                            n.hardware.audio_score,
                            input_source.as_ref() == Some(&n.key),
                            n.key == purple.key,
                        )
                    }),
            }
            .map(|n| n.key.clone())
        };
        let microphone_source = if shared.microphone_enabled && !shared.microphone_level.muted {
            let candidates = [purple, green];
            let source = if let Some(binding) = &shared.microphone_binding {
                candidates.into_iter().find(|n| {
                    n.key == binding.computer
                        && n.hardware.microphone_score > 0
                        && n.devices.microphones.iter().any(|d| d.id == binding.device)
                })
            } else {
                match &shared.microphone {
                    Selection::Computer(key) => candidates
                        .into_iter()
                        .find(|n| &n.key == key && n.hardware.microphone_score > 0),
                    Selection::Auto => candidates
                        .into_iter()
                        .filter(|n| n.hardware.microphone_score > 0)
                        .max_by_key(|n| {
                            (
                                n.hardware.microphone_score,
                                audio_output.as_ref() == Some(&n.key),
                                n.key == purple.key,
                            )
                        }),
                }
            };
            source
                .filter(|source| {
                    (if source.key == a.key { b } else { a })
                        .hardware
                        .microphone_receive
                })
                .map(|n| n.key.clone())
        } else {
            None
        };
        let monitor = input_source.as_ref().and_then(|source| {
            if source == &a.key {
                Self::monitor_inputs(a, b)
            } else {
                Self::monitor_inputs(b, a)
            }
        });
        Self {
            purple: purple.key.clone(),
            green: green.key.clone(),
            input_source,
            keyboard_enabled: shared.keyboard_enabled,
            mouse_enabled: shared.mouse_enabled,
            audio_output,
            microphone_source,
            monitor,
            edge: shared.edge && shared.mouse_enabled,
            purple_on_left: shared.purple_on_left,
        }
    }
    pub fn digest(&self, a: &Node, b: &Node) -> String {
        let (a, b) = if a.key < b.key { (a, b) } else { (b, a) };
        // Runtime status/volume never reconfigure services. Both sides acknowledge
        // the same physical devices, local monitor mappings and routing decision.
        let bytes = serde_json::to_vec(&(
            self,
            &a.hardware,
            &b.hardware,
            &a.monitor,
            &b.monitor,
            &a.devices,
            &b.devices,
            if a.shared.revision >= b.shared.revision {
                &a.shared.devices
            } else {
                &b.shared.devices
            },
            if a.shared.revision >= b.shared.revision {
                &a.shared.microphone_binding
            } else {
                &b.shared.microphone_binding
            },
        ))
        .unwrap();
        config::hex(&Sha256::digest(bytes))
    }
}

pub fn pairing_code(handshake: &[u8]) -> String {
    let hash = Sha256::digest(handshake);
    format!(
        "{:03} {:03}",
        u16::from_be_bytes([hash[0], hash[1]]) % 1000,
        u16::from_be_bytes([hash[2], hash[3]]) % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remembering_video_while_input_is_borrowed_does_not_activate_ddc_until_input_returns() {
        let (mut a, mut b) = (node("a"), node("b"));
        a.hardware.keyboard = true;
        a.hardware.mouse = true;
        for n in [&mut a, &mut b] {
            n.monitor.enabled = true;
            n.monitor.fingerprint = "common-display".into();
        }
        a.monitor.input = Some(17);
        b.monitor.id = "other-connector".into();
        let mut prefs = Preferences {
            monitor: b.monitor.clone(),
            ..Default::default()
        };
        let saved = Monitor {
            input: Some(15),
            ..b.monitor.clone()
        };
        prefs.record_monitor(saved, true);
        b.monitor = prefs.monitor.clone();
        assert!(Plan::build(&a, &b).monitor.is_none());
        assert!(!prefs.apply_pending_monitor(false));
        assert_eq!(prefs.monitor.input, None);
        assert!(prefs.apply_pending_monitor(true));
        b.monitor = prefs.monitor;
        assert_eq!(
            Plan::build(&a, &b).monitor,
            Some(Inputs { host: 17, peer: 15 })
        );
    }
    #[test]
    fn common_monitor_layout_does_not_depend_on_ddc_setup_or_input_readiness() {
        let (mut a, mut b) = (node("a"), node("b"));
        assert!(Plan::common_monitor(&a, &b));
        assert!(Plan::common_monitor(&b, &a));
        assert!(!Plan::shared_monitor(&a, &b));
        assert_eq!(Plan::monitor_inputs(&a, &b), None);
        assert_eq!(Plan::monitor_owner(&a, &b, true), None);
        a.monitor.enabled = true;
        a.monitor.fingerprint = "display".into();
        a.monitor.input = Some(17);
        assert!(Plan::common_monitor(&a, &b));
        assert_eq!(Plan::monitor_inputs(&a, &b), None);
        b.shared.monitor_enabled = false;
        b.shared.changed("b");
        assert!(!Plan::common_monitor(&a, &b));
        assert!(!Plan::common_monitor(&b, &a));
    }
    #[test]
    fn shared_monitor_is_available_when_both_pcs_have_their_own_keyboard_and_mouse() {
        let (mut a, mut b) = (node("a"), node("b"));
        for (node, input) in [(&mut a, 17), (&mut b, 15)] {
            node.hardware.keyboard = true;
            node.hardware.mouse = true;
            node.monitor.enabled = true;
            node.monitor.fingerprint = "same-physical-display".into();
            node.monitor.input = Some(input);
        }
        assert_eq!(Plan::build(&a, &b).input_source, None);
        assert!(Plan::monitor_only_available(&a, &b));
        b.hardware.keyboard = false;
        assert!(!Plan::monitor_only_available(&a, &b));
        b.hardware.keyboard = true;
        assert_eq!(
            Plan::monitor_inputs(&a, &b),
            Some(Inputs { host: 17, peer: 15 })
        );
        assert_eq!(
            Plan::monitor_inputs(&b, &a),
            Some(Inputs { host: 15, peer: 17 })
        );
        b.monitor.input = Some(17);
        assert_eq!(Plan::monitor_inputs(&a, &b), None);
        b.monitor.input = Some(0);
        assert_eq!(Plan::monitor_inputs(&a, &b), None);
        b.monitor.input = Some(15);
        b.monitor.fingerprint = "different-display".into();
        assert_eq!(Plan::monitor_inputs(&a, &b), None);
    }
    pub fn node(key: &str) -> Node {
        Node {
            key: key.into(),
            name: key.into(),
            hardware: Hardware::default(),
            devices: devices::Inventory::default(),
            monitor: Monitor::default(),
            shared: Shared::default(),
            input: None,
            monitor_input: None,
            audio_playing: false,
            microphone_active: false,
            microphone_error: None,
            audio_error: None,
            input_error: None,
            ack: String::new(),
        }
    }
    #[test]
    fn microphone_is_opt_in_requires_a_receiver_and_never_rebinds_after_unplug() {
        let mut a = node("a");
        let mut b = node("b");
        a.hardware.microphone_score = 80;
        b.hardware.microphone_score = 60;
        a.hardware.microphone_receive = true;
        a.devices.microphones.push(crate::audio::AudioDevice {
            id: "headset-mic".into(),
            name: "Headset".into(),
            default: true,
        });
        b.devices.microphones.push(crate::audio::AudioDevice {
            id: "webcam-mic".into(),
            name: "Webcam".into(),
            default: true,
        });
        assert!(Plan::build(&a, &b).microphone_source.is_none());
        a.shared.microphone_enabled = true;
        a.shared.changed("a");
        assert!(
            Plan::build(&a, &b).microphone_source.is_none(),
            "Windows needs a virtual capture bridge"
        );
        b.hardware.microphone_receive = true;
        assert_eq!(Plan::build(&a, &b).microphone_source.as_deref(), Some("a"));
        a.shared.microphone_binding = Some(devices::Binding {
            computer: "a".into(),
            device: "headset-mic".into(),
        });
        let plan = Plan::build(&a, &b);
        let digest = plan.digest(&a, &b);
        a.shared.microphone_level.volume = 37;
        assert_eq!(
            Plan::build(&a, &b).digest(&a, &b),
            digest,
            "gain updates must not restart input or capture"
        );
        a.devices.microphones.clear();
        a.hardware.microphone_score = 0;
        assert!(
            Plan::build(&a, &b).microphone_source.is_none(),
            "do not switch to the other PC's webcam"
        );
        a.shared.microphone_binding = None;
        assert_eq!(Plan::build(&a, &b).microphone_source.as_deref(), Some("b"));
        a.shared.microphone_level.muted = true;
        assert!(Plan::build(&a, &b).microphone_source.is_none());
    }

    #[test]
    fn both_views_agree_on_colours_and_hotplug_roles() {
        let mut a = node("a");
        let mut b = node("b");
        a.hardware.keyboard = true;
        a.hardware.mouse = true;
        a.hardware.audio_score = 20;
        b.hardware.audio_score = 40;
        let first = Plan::build(&a, &b);
        assert_eq!(first, Plan::build(&b, &a));
        assert_eq!(first.digest(&a, &b), first.digest(&b, &a));
        assert_eq!(first.input_source.as_deref(), Some("a"));
        assert_eq!(first.audio_output.as_deref(), Some("b"));
        b.hardware.keyboard = true;
        b.hardware.mouse = true;
        assert!(Plan::build(&a, &b).input_source.is_none());
        a.hardware.keyboard = false;
        a.hardware.mouse = false;
        assert_eq!(Plan::build(&a, &b).input_source.as_deref(), Some("b"));
        assert_ne!(first.digest(&a, &b), Plan::build(&a, &b).digest(&a, &b));
    }
    #[test]
    fn input_presence_survives_pending_or_failed_sharing_and_tracks_hotplug() {
        let mut a = node("a");
        let mut b = node("b");
        a.hardware.keyboard = true;
        a.hardware.mouse = true;
        let plan = Plan::build(&a, &b);
        let own = InputDevices {
            keyboard: Some("a"),
            mouse: Some("a"),
        };
        assert_eq!(plan.input_devices(&a, &b, false), InputDevices::default());
        assert_eq!(plan.input_devices(&a, &b, true), own);
        a.input_error = Some("permission denied".into());
        assert_eq!(plan.input_devices(&a, &b, true), own);
        a.input_error = None;
        a.input = Some(InputStatus::Peer);
        assert_eq!(plan.input_devices(&a, &b, true), own);
        b.input = Some(InputStatus::Here);
        assert_eq!(plan.input_devices(&a, &b, true), InputDevices::default());
        assert_eq!(plan.input_devices(&b, &a, true), own);
        b.input = None;
        assert_eq!(plan.input_devices(&a, &b, true), own);
        assert_eq!(plan.input_devices(&b, &a, true), InputDevices::default());
        a.hardware.keyboard = false;
        a.hardware.mouse = false;
        b.hardware.keyboard = true;
        b.hardware.mouse = true;
        // Old transport status cannot create phantom devices after unplugging.
        assert_eq!(plan.input_devices(&a, &b, true), InputDevices::default());
        let plan = Plan::build(&b, &a);
        assert_eq!(plan.input_source.as_deref(), Some("b"));
        assert_eq!(
            plan.input_devices(&b, &a, true),
            InputDevices {
                keyboard: Some("b"),
                mouse: Some("b")
            }
        );
        a.hardware.keyboard = true;
        let plan = Plan::build(&a, &b);
        assert_eq!(
            plan.input_devices(&a, &b, true),
            InputDevices {
                keyboard: Some("a"),
                mouse: None
            }
        );
    }
    #[test]
    fn one_setting_change_converges_including_concurrent_edits() {
        let mut a = Shared::default();
        let mut b = a.clone();
        a.audio = Selection::Computer("a".into());
        a.changed("a");
        b.input = Selection::Computer("b".into());
        b.changed("b");
        let original = a.clone();
        a.merge(&b);
        b.merge(&original);
        assert_eq!(a, b);
        a.volume = 80;
        a.changed("a");
        b.merge(&a);
        assert_eq!(b.volume, 80);
    }
    #[test]
    fn monitor_mapping_comes_from_each_pcs_own_active_input() {
        let mut a = node("a");
        let mut b = node("b");
        a.hardware.keyboard = true;
        a.hardware.mouse = true;
        a.monitor = Monitor {
            enabled: true,
            id: "local-A".into(),
            fingerprint: "same display".into(),
            input: Some(17),
            ..Default::default()
        };
        b.monitor = Monitor {
            enabled: true,
            id: "local-B".into(),
            fingerprint: "same display".into(),
            input: Some(15),
            ..Default::default()
        };
        assert_eq!(
            Plan::build(&a, &b).monitor,
            Some(Inputs { host: 17, peer: 15 })
        );
        b.shared.input = Selection::Computer("b".into());
        b.shared.changed("b");
        b.hardware.keyboard = true;
        b.hardware.mouse = true;
        assert_eq!(
            Plan::build(&a, &b).monitor,
            Some(Inputs { host: 15, peer: 17 })
        );
        b.monitor.fingerprint = "different display".into();
        assert!(Plan::build(&a, &b).monitor.is_none());
    }

    #[test]
    fn shared_monitor_colour_uses_confirmed_video_input_independently_of_input_handoff() {
        let mut a = node("a");
        let mut b = node("b");
        for (node, input) in [(&mut a, 17), (&mut b, 15)] {
            node.monitor.enabled = true;
            node.monitor.fingerprint = "shared display".into();
            node.monitor.input = Some(input);
        }
        a.monitor_input = Some(17);
        assert!(Plan::shared_monitor(&a, &b));
        assert_eq!(Plan::monitor_owner(&a, &b, false), None);
        assert_eq!(Plan::monitor_owner(&a, &b, true), Some("a"));
        a.input = Some(InputStatus::Peer);
        b.input = Some(InputStatus::Here);
        assert_eq!(Plan::monitor_owner(&a, &b, true), Some("a"));
        b.monitor_input = Some(15);
        assert_eq!(Plan::monitor_owner(&a, &b, true), None);
        a.monitor_input = None;
        assert_eq!(Plan::monitor_owner(&a, &b, true), Some("b"));
        assert_eq!(Plan::monitor_owner(&b, &a, true), Some("b"));
        b.monitor_input = Some(99);
        assert_eq!(Plan::monitor_owner(&a, &b, true), None);
        b.monitor_input = None;
        assert_eq!(Plan::monitor_owner(&a, &b, true), None);
        a.monitor_input = Some(17);
        b.monitor.input = None;
        assert_eq!(Plan::monitor_owner(&a, &b, true), None);
        b.monitor.fingerprint = "another display".into();
        assert!(!Plan::shared_monitor(&a, &b));
    }

    #[test]
    fn source_audio_levels_survive_speaker_changes_and_sync_from_either_pc() {
        let mut a = node("a");
        let mut b = node("b");
        a.hardware.audio_score = 40;
        b.hardware.audio_score = 20;
        let mut shared = Shared::default();
        assert!(shared.prepare_audio_levels("a", "b", Some("a")));
        assert_eq!(shared.audio_level("a").volume, 100);
        assert_eq!(shared.audio_level("b").volume, 50);
        shared.audio_levels.get_mut("a").unwrap().volume = 90;
        shared.audio_levels.get_mut("b").unwrap().muted = true;
        shared.changed("a");
        b.shared.merge(&shared);
        a.shared = shared.clone();
        let plan = Plan::build(&a, &b);
        b.shared.audio_levels.get_mut("b").unwrap().volume = 25;
        b.shared.changed("b");
        a.shared.merge(&b.shared);
        assert_eq!(plan.digest(&a, &b), Plan::build(&a, &b).digest(&a, &b));
        a.shared.audio = Selection::Computer("b".into());
        a.shared.changed("a");
        b.shared.merge(&a.shared);
        assert!(!a.shared.prepare_audio_levels("a", "b", Some("b")));
        assert_eq!(a.shared.audio_levels, b.shared.audio_levels);
        assert_eq!(a.shared.audio_level("a").volume, 90);
        assert_eq!(
            a.shared.audio_level("b"),
            AudioLevel {
                volume: 25,
                muted: true
            }
        );
        assert_eq!(Plan::build(&a, &b).audio_output.as_deref(), Some("b"));
    }

    #[test]
    fn sharing_cannot_remove_the_receivers_only_keyboard_or_mouse() {
        let mut a = node("a");
        let mut b = node("b");
        a.hardware.keyboard = true;
        a.hardware.mouse = true;
        a.input = Some(InputStatus::Peer);
        b.input = Some(InputStatus::Here);
        assert!(!Plan::can_disable(&a, &b, true));
        assert!(!Plan::can_disable(&b, &a, false));
        b.input = None;
        assert!(!Plan::can_disable(&a, &b, true));
        a.input = Some(InputStatus::Here);
        b.input = Some(InputStatus::Here);
        assert!(!Plan::can_disable(&a, &b, true));
        a.input = Some(InputStatus::Peer);
        a.shared.keyboard_enabled = false;
        a.shared.changed("a");
        assert!(Plan::build(&a, &b).input_source.is_none());
        b.hardware.keyboard = true;
        assert!(Plan::can_disable(&a, &b, true));
        assert!(!Plan::can_disable(&a, &b, false));
        let plan = Plan::build(&a, &b);
        assert_eq!(plan.input_source.as_deref(), Some("a"));
        assert_eq!(plan.input_devices(&a, &b, true).keyboard, Some("a"));
        assert_eq!(plan.input_devices(&b, &a, true).keyboard, Some("b"));
        assert_eq!(plan.input_devices(&b, &a, true).mouse, Some("a"));
        a.input = Some(InputStatus::Here);
        b.input = Some(InputStatus::Peer);
        assert!(Plan::can_disable(&a, &b, true));
        assert!(Plan::can_disable(&a, &b, false));
        a.shared.mouse_enabled = false;
        a.shared.monitor_enabled = false;
        a.shared.changed("a");
        assert!(Plan::build(&a, &b).input_source.is_none());
        assert!(a.shared.audio_enabled);
    }
}
