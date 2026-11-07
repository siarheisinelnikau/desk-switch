//! A workspace contains independently managed PCs, with one active input target.
//! Audio fans in to the chosen listening PC; the microphone fans out to receivers.
use crate::{
    compatibility::Version,
    config,
    workspace::{self, AudioLevel, Node, Selection, devices},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::SocketAddr};

pub type Key = String;
pub const PORT: u16 = 45880;
pub const PROTOCOL: u16 = 1;

pub fn valid_key(key: &str) -> Result<()> {
    ensure!(
        key.len() == 64
            && key
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "Invalid computer identity"
    );
    ensure!(config::key(key)? != [0; 32], "Invalid computer identity");
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct MemberPreferences {
    pub devices: devices::Choices,
    pub audio: AudioLevel,
    pub output: Option<AudioLevel>,
}
impl Default for MemberPreferences {
    fn default() -> Self {
        Self {
            devices: Default::default(),
            audio: AudioLevel {
                volume: 50,
                muted: false,
            },
            output: None,
        }
    }
}
impl MemberPreferences {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.audio.volume <= 100, "Invalid relative audio level");
        ensure!(
            self.output.is_none_or(|level| level.volume <= 100),
            "Invalid system audio level"
        );
        for id in [
            &self.devices.keyboard,
            &self.devices.mouse,
            &self.devices.speaker,
            &self.devices.microphone,
            &self.devices.microphone_bridge,
        ]
        .into_iter()
        .flatten()
        {
            ensure!(
                !id.is_empty() && id.len() <= 2048 && !id.chars().any(char::is_control),
                "Invalid device selection"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Member {
    pub node: Node,
    pub address: SocketAddr,
    pub online: bool,
    pub native_output: Option<AudioLevel>,
    pub preferences: MemberPreferences,
    #[serde(default)]
    pub input_ready: u64,
}
impl Member {
    pub fn validate(&self, hub: &str) -> Result<()> {
        valid_key(&self.node.key)?;
        self.preferences.validate()?;
        ensure!(
            self.native_output.is_none_or(|level| level.volume <= 100),
            "Invalid system volume"
        );
        ensure!(
            self.address.port() != 0
                && !self.address.ip().is_unspecified()
                && !self.address.ip().is_multicast(),
            "Invalid computer address"
        );
        // The legacy node validator also checks inventories, errors and application
        // preferences. Global settings travel separately, never as a per-PC claim.
        let mut node = self.node.clone();
        node.shared = Default::default();
        let fallback = if node.key == "ff".repeat(32) {
            "ee".repeat(32)
        } else {
            "ff".repeat(32)
        };
        let other = if hub != node.key { hub } else { &fallback };
        node.validate(other, &self.node.key)?;
        Ok(())
    }
    pub fn compatible(&self) -> bool {
        self.online
            && self
                .node
                .version
                .as_ref()
                .is_some_and(|version| Version::current().require(Some(version)).is_ok())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Settings {
    pub input: Selection,
    pub keyboard_enabled: bool,
    pub mouse_enabled: bool,
    pub audio: Selection,
    pub audio_enabled: bool,
    pub microphone: Selection,
    pub microphone_binding: Option<devices::Binding>,
    pub microphone_enabled: bool,
    pub microphone_level: AudioLevel,
    pub monitor_enabled: bool,
    pub edge: bool,
    pub clipboard_enabled: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self::from(&workspace::Shared::default())
    }
}
impl From<&workspace::Shared> for Settings {
    fn from(value: &workspace::Shared) -> Self {
        Self {
            input: value.input.clone(),
            keyboard_enabled: value.keyboard_enabled,
            mouse_enabled: value.mouse_enabled,
            audio: value.audio.clone(),
            audio_enabled: value.audio_enabled,
            microphone: value.microphone.clone(),
            microphone_binding: value.microphone_binding.clone(),
            microphone_enabled: value.microphone_enabled,
            microphone_level: value.microphone_level,
            monitor_enabled: value.monitor_enabled,
            edge: value.edge,
            clipboard_enabled: false,
        }
    }
}
impl Settings {
    pub fn validate(&self, members: &BTreeMap<Key, Member>) -> Result<()> {
        ensure!(
            self.microphone_level.volume <= 100,
            "Invalid microphone level"
        );
        for choice in [&self.input, &self.audio, &self.microphone] {
            if let Selection::Computer(key) = choice {
                ensure!(
                    members.contains_key(key),
                    "The selected computer is not in this workspace"
                );
            }
        }
        if let Some(binding) = &self.microphone_binding {
            ensure!(
                members.contains_key(&binding.computer),
                "Unknown microphone owner"
            );
            ensure!(
                !binding.device.is_empty()
                    && binding.device.len() <= 2048
                    && !binding.device.chars().any(char::is_control),
                "Invalid microphone binding"
            );
            if let Selection::Computer(selected) = &self.microphone {
                ensure!(
                    selected == &binding.computer,
                    "Microphone binding belongs to another computer"
                );
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Workspace {
    pub hub: Key,
    pub revision: u64,
    pub settings: Settings,
    pub members: BTreeMap<Key, Member>,
    pub active_input: Option<Key>,
    #[serde(default)]
    pub active_input_entry: Option<u16>,
    #[serde(default)]
    pub activation: u64,
    pub active_monitor: Option<Key>,
}
impl Workspace {
    fn next_revision(&self) -> Result<u64> {
        self.revision
            .checked_add(1)
            .filter(|value| *value < u64::MAX)
            .context("Workspace revision exhausted")
    }
    pub fn validate(&self) -> Result<()> {
        valid_key(&self.hub)?;
        ensure!(
            self.members.contains_key(&self.hub),
            "The workspace controller is missing"
        );
        ensure!(self.revision < u64::MAX, "Workspace revision exhausted");
        for (key, member) in &self.members {
            ensure!(*key == member.node.key, "Computer identity mismatch");
            member.validate(&self.hub)?;
        }
        for key in [&self.active_input, &self.active_monitor]
            .into_iter()
            .flatten()
        {
            ensure!(self.members.contains_key(key), "Unknown active computer");
        }
        self.settings.validate(&self.members)
    }
    pub fn set_settings(&mut self, expected: u64, settings: Settings) -> Result<()> {
        ensure!(
            self.revision == expected,
            "Workspace settings changed. Review them and try again."
        );
        settings.validate(&self.members)?;
        let previous = self.plan();
        if let Some(active) = &self.active_input
            && previous
                .input
                .as_ref()
                .is_some_and(|source| source != active)
        {
            let destination = self
                .members
                .get(active)
                .context("Active computer is unavailable")?;
            ensure!(
                settings.keyboard_enabled || destination.node.hardware.keyboard,
                "Return the shared keyboard before disabling it"
            );
            ensure!(
                settings.mouse_enabled || destination.node.hardware.mouse,
                "Return the shared mouse before disabling it"
            );
        }
        let revision = self.next_revision()?;
        self.settings = settings;
        self.revision = revision;
        Ok(())
    }
    pub fn set_member(
        &mut self,
        key: &str,
        expected: &MemberPreferences,
        preferences: MemberPreferences,
    ) -> Result<()> {
        preferences.validate()?;
        let revision = self.next_revision()?;
        let member = self
            .members
            .get_mut(key)
            .context("The computer is no longer in this workspace")?;
        ensure!(
            &member.preferences == expected,
            "Device preferences changed. Review them and try again."
        );
        member.preferences = preferences;
        self.revision = revision;
        Ok(())
    }
    pub fn update_node(&mut self, key: &str, node: Node) -> Result<()> {
        ensure!(key == node.key, "Computer identity mismatch");
        let mut member = self.members.get(key).context("Unknown computer")?.clone();
        member.node = node;
        member.validate(&self.hub)?;
        self.members.get_mut(key).unwrap().node = member.node;
        Ok(())
    }
    pub fn monitor_input(&self, destination: &str) -> Result<u16> {
        ensure!(self.settings.monitor_enabled, "Shared monitor is off");
        let member = self.members.get(destination).context("Unknown computer")?;
        ensure!(
            member.compatible(),
            "The destination is offline or needs an update"
        );
        ensure!(
            member.node.monitor.enabled,
            "Remember the destination's monitor connection first"
        );
        let input = member
            .node
            .monitor
            .input
            .filter(|input| *input != 0)
            .context("Remember the destination's monitor connection first")?;
        ensure!(
            !self
                .members
                .values()
                .any(|other| other.node.key != destination
                    && other.node.monitor.enabled
                    && other.node.monitor.input == Some(input)),
            "Two computers have the same saved monitor input"
        );
        Ok(input)
    }
    pub fn plan(&self) -> Plan {
        let ready = |member: &&Member| member.compatible() && member.node.hardware.known;
        let selected = |selection: &Selection, eligible: Vec<&Member>| -> Option<Key> {
            match selection {
                Selection::Computer(key) => eligible
                    .iter()
                    .any(|member| member.node.key == *key)
                    .then(|| key.clone()),
                Selection::Auto => eligible.first().map(|member| member.node.key.clone()),
            }
        };
        let inputs = self
            .members
            .values()
            .filter(ready)
            .filter(|member| {
                (!self.settings.keyboard_enabled
                    || (member.node.hardware.keyboard
                        && devices::selected(
                            &member.node.devices.keyboards,
                            member.preferences.devices.keyboard.as_deref(),
                        )
                        .is_some()))
                    && (!self.settings.mouse_enabled
                        || (member.node.hardware.mouse
                            && devices::selected(
                                &member.node.devices.mice,
                                member.preferences.devices.mouse.as_deref(),
                            )
                            .is_some()))
            })
            .collect::<Vec<_>>();
        let input = (self.settings.keyboard_enabled || self.settings.mouse_enabled)
            .then(|| selected(&self.settings.input, inputs))
            .flatten();
        let mut outputs = self
            .members
            .values()
            .filter(ready)
            .filter(|member| {
                !member.node.devices.speakers.is_empty()
                    && devices::selected(
                        &member.node.devices.speakers,
                        member.preferences.devices.speaker.as_deref(),
                    )
                    .is_some()
            })
            .collect::<Vec<_>>();
        outputs.sort_by(|a, b| {
            b.node
                .hardware
                .audio_score
                .cmp(&a.node.hardware.audio_score)
                .then_with(|| a.node.key.cmp(&b.node.key))
        });
        let audio = self
            .settings
            .audio_enabled
            .then(|| selected(&self.settings.audio, outputs))
            .flatten();
        let microphone = self
            .settings
            .microphone_enabled
            .then(|| self.settings.microphone_binding.clone())
            .flatten()
            .filter(|binding| {
                self.members.get(&binding.computer).is_some_and(|member| {
                    member.compatible()
                        && member
                            .node
                            .devices
                            .microphones
                            .iter()
                            .any(|device| device.id == binding.device)
                })
            });
        Plan {
            input,
            audio,
            microphone,
        }
    }
    pub fn bind_microphone(&mut self) {
        if !self.settings.microphone_enabled || self.settings.microphone_binding.is_some() {
            return;
        }
        let mut candidates = self
            .members
            .values()
            .filter(|member| member.compatible() && member.node.hardware.known)
            .filter(|member| match &self.settings.microphone {
                Selection::Auto => true,
                Selection::Computer(key) => member.node.key == *key,
            })
            .filter_map(|member| {
                devices::selected(
                    &member.node.devices.microphones,
                    member.preferences.devices.microphone.as_deref(),
                )
                .map(|device| (member, device))
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|(a, _), (b, _)| {
            b.node
                .hardware
                .microphone_score
                .cmp(&a.node.hardware.microphone_score)
                .then_with(|| a.node.key.cmp(&b.node.key))
        });
        if let Some((member, device)) = candidates.first() {
            self.settings.microphone_binding = Some(devices::Binding {
                computer: member.node.key.clone(),
                device: device.id.clone(),
            });
        }
    }
    pub fn routes(&self, local: &str) -> Routes {
        let plan = self.plan();
        let compatible = |member: &&Member| member.compatible();
        let mut routes = Routes::default();
        if !self.members.get(local).is_some_and(Member::compatible) {
            return routes;
        }
        if plan.input.as_deref() == Some(local) {
            routes.input_targets = self
                .members
                .values()
                .filter(compatible)
                .filter(|member| member.node.key != local)
                .map(Route::from)
                .collect();
        } else if let Some(source) = plan.input.as_ref().and_then(|key| self.members.get(key)) {
            routes.input_source = Some(Route::from(source));
        }
        if plan.audio.as_deref() == Some(local) {
            routes.audio_sources = self
                .members
                .values()
                .filter(compatible)
                .filter(|member| member.node.key != local)
                .map(Route::from)
                .collect();
        } else if let Some(output) = plan.audio.as_ref().and_then(|key| self.members.get(key)) {
            routes.audio_output = Some(Route::from(output));
        }
        if let Some(binding) = &plan.microphone {
            if binding.computer == local {
                routes.microphone_targets = self
                    .members
                    .values()
                    .filter(compatible)
                    .filter(|member| {
                        member.node.key != local && member.node.hardware.microphone_receive
                    })
                    .map(Route::from)
                    .collect();
            } else if self
                .members
                .get(local)
                .is_some_and(|member| member.node.hardware.microphone_receive)
            {
                routes.microphone_source = self.members.get(&binding.computer).map(Route::from);
            }
        }
        if self.settings.clipboard_enabled
            && self.members.get(local).is_some_and(Member::compatible)
        {
            routes.clipboard_peers = self
                .members
                .values()
                .filter(compatible)
                .filter(|member| member.node.key != local)
                .map(Route::from)
                .collect();
        }
        routes
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Plan {
    pub input: Option<Key>,
    pub audio: Option<Key>,
    pub microphone: Option<devices::Binding>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Route {
    pub key: Key,
    pub address: SocketAddr,
}
impl From<&Member> for Route {
    fn from(member: &Member) -> Self {
        Self {
            key: member.node.key.clone(),
            address: member.address,
        }
    }
}
#[derive(Default)]
pub struct Routes {
    pub input_source: Option<Route>,
    pub input_targets: Vec<Route>,
    pub audio_output: Option<Route>,
    pub audio_sources: Vec<Route>,
    pub microphone_source: Option<Route>,
    pub microphone_targets: Vec<Route>,
    pub clipboard_peers: Vec<Route>,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn key(index: usize) -> String {
        format!("{index:064x}")
    }
    fn device(id: &str) -> crate::audio::AudioDevice {
        crate::audio::AudioDevice {
            id: id.into(),
            name: id.into(),
            default: true,
        }
    }
    pub(crate) fn fixture(count: usize) -> Workspace {
        let members = (1..=count)
            .map(|index| {
                let mut inventory = devices::Inventory {
                    speakers: vec![device("speakers")],
                    microphones: vec![device("microphone")],
                    microphone_bridges: vec![device("bridge")],
                    ..Default::default()
                };
                if index == 1 {
                    inventory.keyboards.push(device("keyboard"));
                    inventory.mice.push(device("mouse"));
                }
                let node = Node {
                    key: key(index),
                    name: format!("Computer {index}"),
                    version: Some(Version::current()),
                    management: None,
                    hardware: workspace::Hardware {
                        known: true,
                        keyboard: index == 1,
                        mouse: index == 1,
                        audio_score: if index == 2 { 99 } else { 10 },
                        microphone_score: if index == 3 { 99 } else { 10 },
                        microphone_receive: index != 3,
                        ..Default::default()
                    },
                    devices: inventory,
                    monitor: workspace::Monitor {
                        enabled: true,
                        input: Some(index as u16 + 14),
                        ..Default::default()
                    },
                    shared: workspace::Shared::default(),
                    input: None,
                    monitor_input: None,
                    monitor_error: None,
                    audio_playing: false,
                    microphone_active: false,
                    microphone_error: None,
                    audio_error: None,
                    input_error: None,
                    ack: String::new(),
                    diagnostics: Default::default(),
                };
                (
                    node.key.clone(),
                    Member {
                        node,
                        address: format!("127.0.0.1:{}", 46000 + index).parse().unwrap(),
                        online: true,
                        native_output: Some(AudioLevel::default()),
                        preferences: Default::default(),
                        input_ready: 0,
                    },
                )
            })
            .collect();
        Workspace {
            hub: key(1),
            revision: 0,
            settings: Settings::default(),
            members,
            active_input: None,
            active_input_entry: None,
            activation: 0,
            active_monitor: None,
        }
    }

    #[test]
    fn shares_input_audio_and_microphone_with_more_than_two_computers() {
        for count in [3, 7, 65] {
            let mut workspace = fixture(count);
            workspace.settings.microphone_enabled = true;
            workspace.bind_microphone();
            workspace.validate().unwrap();
            let plan = workspace.plan();
            assert_eq!(plan.input, Some(key(1)));
            assert_eq!(plan.audio, Some(key(2)));
            assert_eq!(plan.microphone.unwrap().computer, key(3));
            let owner = workspace.routes(&key(1));
            assert_eq!(owner.input_targets.len(), count - 1);
            assert_eq!(owner.audio_output.unwrap().key, key(2));
            let headphones = workspace.routes(&key(2));
            assert_eq!(headphones.audio_sources.len(), count - 1);
            assert_eq!(headphones.input_source.unwrap().key, key(1));
            let microphone = workspace.routes(&key(3));
            assert_eq!(microphone.microphone_targets.len(), count - 1);
            assert!(workspace.routes(&key(count)).clipboard_peers.is_empty());
            workspace.settings.clipboard_enabled = true;
            assert_eq!(
                workspace.routes(&key(count)).clipboard_peers.len(),
                count - 1
            );
        }
    }

    #[test]
    fn settings_edits_are_atomic_and_reject_stale_revisions() {
        let mut workspace = fixture(3);
        let mut settings = workspace.settings.clone();
        settings.clipboard_enabled = true;
        workspace.set_settings(0, settings).unwrap();
        let accepted = workspace.clone();
        assert!(workspace.set_settings(0, Settings::default()).is_err());
        assert_eq!(workspace, accepted);
        let member = workspace.members[&key(3)].preferences.clone();
        let mut changed = member.clone();
        changed.audio.volume = 23;
        workspace.set_member(&key(3), &member, changed).unwrap();
        let accepted = workspace.clone();
        assert!(
            workspace
                .set_member(&key(3), &member, MemberPreferences::default())
                .is_err()
        );
        assert_eq!(workspace, accepted);
        workspace.revision = u64::MAX - 1;
        let exhausted = workspace.clone();
        assert!(
            workspace
                .set_settings(workspace.revision, Settings::default())
                .is_err()
        );
        assert_eq!(workspace, exhausted);
    }

    #[test]
    fn source_status_does_not_override_another_computers_preferences() {
        let mut workspace = fixture(3);
        workspace
            .members
            .get_mut(&key(2))
            .unwrap()
            .preferences
            .audio
            .volume = 37;
        let mut node = workspace.members[&key(2)].node.clone();
        node.shared.audio_levels.insert(
            key(1),
            AudioLevel {
                volume: 1,
                muted: true,
            },
        );
        node.shared.microphone_enabled = true;
        workspace.update_node(&key(2), node).unwrap();
        assert_eq!(workspace.members[&key(2)].preferences.audio.volume, 37);
        assert!(!workspace.settings.microphone_enabled);
        assert_eq!(workspace.revision, 0);
        let wrong_identity = workspace.members[&key(3)].node.clone();
        assert!(workspace.update_node(&key(2), wrong_identity).is_err());
    }

    #[test]
    fn offline_microphone_never_silently_switches_to_another_physical_device() {
        let mut workspace = fixture(4);
        workspace.settings.microphone_enabled = true;
        workspace.bind_microphone();
        let binding = workspace.settings.microphone_binding.clone();
        workspace.members.get_mut(&key(3)).unwrap().online = false;
        workspace.bind_microphone();
        assert_eq!(workspace.settings.microphone_binding, binding);
        assert!(workspace.plan().microphone.is_none());
        assert!(workspace.routes(&key(3)).microphone_targets.is_empty());
        workspace.members.get_mut(&key(3)).unwrap().online = true;
        assert_eq!(workspace.plan().microphone, binding);
    }

    #[test]
    fn offline_or_incompatible_computers_receive_no_routes() {
        let mut workspace = fixture(4);
        workspace.settings.clipboard_enabled = true;
        workspace.members.get_mut(&key(3)).unwrap().online = false;
        workspace
            .members
            .get_mut(&key(4))
            .unwrap()
            .node
            .version
            .as_mut()
            .unwrap()
            .app = "old".into();
        assert_eq!(workspace.routes(&key(1)).input_targets.len(), 1);
        assert_eq!(workspace.routes(&key(2)).audio_sources.len(), 1);
        assert_eq!(workspace.routes(&key(1)).clipboard_peers.len(), 1);
        let offline = workspace.routes(&key(3));
        assert!(offline.clipboard_peers.is_empty());
        assert!(offline.input_source.is_none());
        assert!(offline.audio_output.is_none());
    }

    #[test]
    fn monitor_requires_a_unique_saved_input_and_settings_cannot_strand_borrowed_input() {
        let mut workspace = fixture(3);
        workspace.settings.monitor_enabled = true;
        assert_eq!(workspace.monitor_input(&key(3)).unwrap(), 17);
        workspace
            .members
            .get_mut(&key(2))
            .unwrap()
            .node
            .monitor
            .input = Some(17);
        assert!(workspace.monitor_input(&key(3)).is_err());
        workspace.active_input = Some(key(3));
        let original = workspace.settings.clone();
        let mut settings = original.clone();
        settings.keyboard_enabled = false;
        assert!(workspace.set_settings(0, settings).is_err());
        assert_eq!(workspace.settings, original);
        workspace
            .members
            .get_mut(&key(1))
            .unwrap()
            .preferences
            .devices
            .keyboard = Some("missing".into());
        assert!(workspace.plan().input.is_none());
    }
}
