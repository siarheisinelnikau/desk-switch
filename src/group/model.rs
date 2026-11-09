//! A workspace contains independently managed PCs, with one active input target.
//! Audio fans in to the chosen listening PC; the microphone fans out to receivers.
use super::layout::{self, DisplayTarget, Layout};
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
pub const PROTOCOL: u16 = 3;

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
    pub edge_delay_ms: u16,
    pub edge_protect_corners: bool,
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
            edge_delay_ms: crate::edge::Policy::default().dwell_ms,
            edge_protect_corners: true,
            clipboard_enabled: false,
        }
    }
}
impl Settings {
    pub fn validate(&self, members: &BTreeMap<Key, Member>) -> Result<()> {
        crate::edge::Policy {
            dwell_ms: self.edge_delay_ms,
            protect_corners: self.edge_protect_corners,
            paused: false,
        }
        .validate()?;
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MonitorResult {
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MonitorCommand {
    pub id: u64,
    pub writer: Key,
    pub destination: Key,
    pub connection: String,
    pub input: u16,
    pub deadline_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<MonitorResult>,
}

fn monitor_message(error: &str) -> Result<()> {
    ensure!(
        error.len() <= 2048 && !error.chars().any(char::is_control),
        "Invalid monitor result"
    );
    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Authority {
    pub workspace_id: Key,
    pub term: u64,
    pub index: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Workspace {
    pub hub: Key,
    #[serde(default)]
    pub authority: Option<Authority>,
    pub revision: u64,
    pub settings: Settings,
    #[serde(default)]
    pub edge_paused: bool,
    #[serde(default)]
    pub edge_policy_revision: u64,
    pub members: BTreeMap<Key, Member>,
    pub active_input: Option<Key>,
    #[serde(default)]
    pub input_intent: Option<Key>,
    #[serde(default)]
    pub input_request: u64,
    #[serde(default)]
    pub active_input_entry: Option<u16>,
    #[serde(default)]
    pub activation: u64,
    pub active_monitor: Option<Key>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_monitor: Option<Key>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor_command: Option<MonitorCommand>,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub active_display: Option<DisplayTarget>,
    #[serde(default)]
    pub display_entry: Option<crate::edge::Entry>,
}
impl Workspace {
    pub fn workspace_id(&self) -> &str {
        self.authority
            .as_ref()
            .map(|authority| authority.workspace_id.as_str())
            .unwrap_or(&self.hub)
    }

    fn next_revision(&self) -> Result<u64> {
        self.revision
            .checked_add(1)
            .filter(|value| *value < u64::MAX)
            .context("Workspace revision exhausted")
    }
    pub fn validate(&self) -> Result<()> {
        if let Some(authority) = &self.authority {
            valid_key(&authority.workspace_id)?;
            ensure!(
                authority.term > 0 && authority.index > 0,
                "Invalid workspace authority"
            );
        }
        self.layout.validate_shape()?;
        ensure!(
            self.edge_policy_revision <= self.revision,
            "Invalid edge policy revision"
        );
        ensure!(
            self.input_request <= self.activation,
            "Invalid input request"
        );
        if let Some(target) = &self.active_display {
            ensure!(
                self.members.contains_key(target.computer())
                    && self.active_input.as_deref() == Some(target.computer()),
                "Invalid active display target"
            );
        }
        if let Some(entry) = self.display_entry {
            entry.validate()?;
        }
        valid_key(&self.hub)?;
        ensure!(
            self.members.contains_key(&self.hub),
            "The workspace controller is missing"
        );
        ensure!(self.revision < u64::MAX, "Workspace revision exhausted");
        for (key, member) in &self.members {
            ensure!(*key == member.node.key, "Computer identity mismatch");
            member.validate(&self.hub)?;
            ensure!(
                member.input_ready <= self.activation,
                "Invalid monitor acknowledgement"
            );
        }
        for key in [
            &self.active_input,
            &self.input_intent,
            &self.active_monitor,
            &self.confirmed_monitor,
        ]
        .into_iter()
        .flatten()
        {
            ensure!(self.members.contains_key(key), "Unknown active computer");
        }
        if let Some(command) = &self.monitor_command {
            ensure!(
                command.id > 0
                    && command.id <= self.activation
                    && command.input != 0
                    && command.deadline_ms > 0,
                "Invalid monitor command"
            );
            ensure!(
                !command.connection.is_empty()
                    && command.connection.len() <= 1024
                    && !command.connection.chars().any(char::is_control),
                "Invalid monitor command connection"
            );
            ensure!(
                self.members.contains_key(&command.writer)
                    && self.members.contains_key(&command.destination),
                "Unknown monitor command participant"
            );
            if command.result.is_none() {
                ensure!(
                    command.id == self.activation
                        && self.active_input.as_ref() == Some(&command.destination),
                    "Stale pending monitor command"
                );
            }
            for error in [
                command.interrupted.as_ref(),
                command
                    .result
                    .as_ref()
                    .and_then(|result| result.error.as_ref()),
            ]
            .into_iter()
            .flatten()
            {
                monitor_message(error)?;
            }
        }
        self.settings.validate(&self.members)
    }
    pub fn pause_edges(&mut self, paused: bool) -> Result<()> {
        if self.edge_paused != paused {
            self.revision = self.next_revision()?;
            self.edge_policy_revision = self.revision;
            self.edge_paused = paused;
        }
        Ok(())
    }
    pub fn remove_member(&mut self, key: &str) -> Result<()> {
        ensure!(key != self.hub, "The coordinator cannot remove itself");
        ensure!(self.members.contains_key(key), "Unknown computer");
        ensure!(
            !self.switch_pending(),
            "Wait for the monitor switch to finish"
        );
        let revision = self.next_revision()?;
        if self.layout.positions.contains_key(key) {
            self.layout.revision = self
                .layout
                .revision
                .checked_add(1)
                .filter(|revision| *revision < u64::MAX)
                .context("Display arrangement revision exhausted")?;
            self.layout.positions.remove(key);
        }
        let borrowed_source = self.plan().input.as_deref() == Some(key)
            || matches!(&self.settings.input, Selection::Computer(source) if source == key);
        self.members.remove(key);
        for selection in [
            &mut self.settings.input,
            &mut self.settings.audio,
            &mut self.settings.microphone,
        ] {
            if matches!(selection, Selection::Computer(computer) if computer == key) {
                *selection = Selection::Auto;
            }
        }
        if self
            .settings
            .microphone_binding
            .as_ref()
            .is_some_and(|binding| binding.computer == key)
        {
            self.settings.microphone_enabled = false;
            self.settings.microphone_binding = None;
        }
        for monitor in [&mut self.active_monitor, &mut self.confirmed_monitor] {
            if monitor.as_deref() == Some(key) {
                *monitor = None;
            }
        }
        if self
            .monitor_command
            .as_ref()
            .is_some_and(|command| command.writer == key || command.destination == key)
        {
            self.monitor_command = None;
        }
        if borrowed_source || self.active_input.as_deref() == Some(key) {
            self.return_input()?;
        } else if self.input_intent.as_deref() == Some(key) {
            self.input_intent = self.active_input.clone();
        }
        self.revision = revision;
        self.validate()
    }

    pub fn set_settings(&mut self, expected: u64, settings: Settings) -> Result<()> {
        ensure!(
            self.revision == expected,
            "Workspace settings changed. Review them and try again."
        );
        settings.validate(&self.members)?;
        ensure!(
            !self.switch_pending() || settings.monitor_enabled == self.settings.monitor_enabled,
            "The previous switch is still finishing. Wait for the monitor before switching again."
        );
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
        if settings.edge != self.settings.edge
            || settings.edge_delay_ms != self.settings.edge_delay_ms
            || settings.edge_protect_corners != self.settings.edge_protect_corners
            || settings.mouse_enabled != self.settings.mouse_enabled
        {
            self.edge_policy_revision = revision;
        }
        self.settings = settings;
        self.revision = revision;
        Ok(())
    }
    pub fn set_layout(&mut self, expected: u64, mut value: Layout) -> Result<()> {
        ensure!(
            self.layout.revision == expected,
            "The display arrangement changed. Review it and try again."
        );
        ensure!(
            !self.switch_pending(),
            "Wait for the previous switch before saving the display arrangement"
        );
        layout::validate(self, &value)?;
        value.revision = expected
            .checked_add(1)
            .filter(|revision| *revision < u64::MAX)
            .context("Display arrangement revision exhausted")?;
        self.revision = self.next_revision()?;
        self.layout = value;
        Ok(())
    }
    pub fn reconcile_input(&mut self) {
        if self.switch_pending() {
            return;
        }
        // Older workspaces stored only the active target. Preserve that choice
        // before a disconnected member forces borrowed input to fail open.
        if self.input_intent.is_none() {
            self.input_intent = self.active_input.clone();
        }
        let available = |key: &&Key| self.members.get(*key).is_some_and(Member::compatible);
        let intended = self
            .input_intent
            .as_ref()
            .filter(available)
            .filter(|key| self.members[*key].node.monitor_error.is_none())
            .cloned();
        let destination = intended
            .or_else(|| self.active_input.as_ref().filter(available).cloned())
            .or_else(|| self.plan().input);
        if self.active_input == destination {
            return;
        }
        let Some(activation) = self.activation.checked_add(1) else {
            return;
        };
        let Ok(revision) = self.next_revision() else {
            return;
        };
        self.active_input = destination.clone();
        self.active_input_entry = None;
        self.active_display = None;
        self.display_entry = None;
        self.activation = activation;
        self.revision = revision;
        if let Some(destination) = destination {
            // Restoring input never replays a physical monitor command. Its
            // last confirmed selection and any command result remain intact.
            self.members.get_mut(&destination).unwrap().input_ready = activation;
        }
    }
    pub fn activate_display(
        &mut self,
        requester: &str,
        target: DisplayTarget,
        entry: Option<crate::edge::Entry>,
    ) -> Result<()> {
        let native = layout::native_target(self, &target)?;
        let entry = entry.unwrap_or(crate::edge::Entry {
            screen: native.rect,
            geometry: native.epoch,
            side: crate::edge::Side::Left,
            position: u16::MAX / 2,
        });
        entry.validate()?;
        ensure!(
            entry.screen == native.rect && entry.geometry == native.epoch,
            "The destination display geometry changed"
        );
        let computer = target.computer().to_owned();
        let shared = matches!(&target, DisplayTarget::Shared { .. });
        self.activate_target(
            requester,
            &computer,
            None,
            shared,
            Some(target),
            Some(entry),
        )
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
    pub fn update_node(
        &mut self,
        key: &str,
        mut node: Node,
        native: Option<AudioLevel>,
        ready: u64,
    ) -> Result<()> {
        ensure!(key == node.key, "Computer identity mismatch");
        ensure!(ready <= self.activation, "Invalid monitor acknowledgement");
        ensure!(
            native.is_none_or(|level| level.volume <= 100),
            "Invalid system volume"
        );
        let mut member = self.members.get(key).context("Unknown computer")?.clone();
        let monitor_binding_changed = member.node.monitor.enabled != node.monitor.enabled
            || member.node.monitor.id != node.monitor.id
            || member.node.monitor.input != node.monitor.input;
        // Monitor completion belongs to the authenticated command executor.
        // Ordinary Node snapshots cannot grant readiness or erase its failure.
        if let Some(command) = &self.monitor_command {
            if command.destination == key {
                node.monitor_error = command
                    .result
                    .as_ref()
                    .and_then(|result| result.error.clone())
                    .or_else(|| command.interrupted.clone());
            }
        } else if self.active_input.as_deref() == Some(key) {
            node.monitor_error = member.node.monitor_error.clone();
        }
        member.node = node;
        if let Some(native) = native {
            member.native_output = Some(native);
        }
        member.validate(&self.hub)?;
        self.members.insert(key.into(), member);
        if monitor_binding_changed {
            self.confirmed_monitor = None;
        }
        Ok(())
    }
    /// An interrupted writer must acknowledge cancellation before another PC is
    /// allowed to issue a physical command, even if its connection is offline.
    pub fn switch_pending(&self) -> bool {
        self.monitor_command
            .as_ref()
            .is_some_and(|command| command.result.is_none())
    }
    pub fn activate(
        &mut self,
        requester: &str,
        destination: &str,
        position: Option<u16>,
    ) -> Result<()> {
        self.activate_target(requester, destination, position, true, None, None)
    }
    fn activate_target(
        &mut self,
        requester: &str,
        destination: &str,
        position: Option<u16>,
        shared: bool,
        target: Option<DisplayTarget>,
        entry: Option<crate::edge::Entry>,
    ) -> Result<()> {
        ensure!(
            !self.switch_pending(),
            "The previous switch is still finishing. Wait for the monitor before switching again."
        );
        let member = self
            .members
            .get(destination)
            .filter(|member| member.compatible())
            .context("The destination is offline or needs an update")?;
        let monitor = shared && self.settings.monitor_enabled && member.node.monitor.enabled;
        if monitor {
            self.monitor_input(destination)?;
        }
        // Even a matching target is a new explicit intent: F11 may have
        // released the native route while its controller was unreachable.
        // Physical monitor writes remain independently guarded below.
        let revision = self.next_revision()?;
        let activation = self
            .activation
            .checked_add(1)
            .context("Too many input switches")?;
        let explicit_pc_change = target.is_none()
            && position.is_none()
            && self.active_input.as_deref() != Some(destination);
        let command = if monitor
            && (self.confirmed_monitor.as_deref() != Some(destination) || explicit_pc_change)
        {
            let eligible = |key: &str| {
                self.members.get(key).is_some_and(|member| {
                    member.compatible()
                        && member.node.monitor.enabled
                        && !member.node.monitor.id.is_empty()
                })
            };
            let writer = self
                .confirmed_monitor
                .as_deref()
                .filter(|key| eligible(key))
                .unwrap_or(requester);
            ensure!(
                eligible(writer),
                "Use a PC connected to the shared monitor to select its next input."
            );
            Some(MonitorCommand {
                id: activation,
                writer: writer.into(),
                destination: destination.into(),
                connection: self.members[writer].node.monitor.id.clone(),
                input: self.monitor_input(destination)?,
                deadline_ms: now_ms().saturating_add(25_000),
                interrupted: None,
                result: None,
            })
        } else {
            None
        };
        self.active_input = Some(destination.into());
        if self.edge_paused {
            self.edge_policy_revision = revision;
        }
        self.edge_paused = false;
        self.input_intent = Some(destination.into());
        self.active_input_entry = position;
        self.active_display = target;
        self.display_entry = entry;
        if monitor {
            self.active_monitor = Some(destination.into());
        }
        let member = self.members.get_mut(destination).unwrap();
        member.node.monitor_error = None;
        member.input_ready = if command.is_some() { 0 } else { activation };
        self.monitor_command = command;
        self.revision = revision;
        self.activation = activation;
        self.input_request = activation;
        Ok(())
    }
    pub fn monitor_result(&mut self, writer: &str, id: u64, error: Option<String>) -> Result<()> {
        if let Some(error) = &error {
            monitor_message(error)?;
        }
        ensure!(
            self.members.contains_key(writer) && id > 0 && id <= self.activation,
            "Unexpected monitor command result"
        );
        if id < self.activation
            && self
                .monitor_command
                .as_ref()
                .is_none_or(|command| command.id != id)
        {
            // An authenticated acknowledgement may still be queued after a
            // return/new request. It cannot change a later command's state.
            return Ok(());
        }
        let command = self
            .monitor_command
            .as_ref()
            .context("No monitor command is pending")?;
        ensure!(
            command.writer == writer && command.id == id,
            "Unexpected monitor command result"
        );
        let error = if let Some(interrupted) = &command.interrupted {
            Some(error.unwrap_or_else(|| interrupted.clone()))
        } else {
            error
        };
        if let Some(result) = &command.result {
            ensure!(
                result.error == error,
                "Monitor command already completed with another result"
            );
            return Ok(());
        }
        let destination = command.destination.clone();
        self.confirmed_monitor = error.is_none().then(|| destination.clone());
        let member = self.members.get_mut(&destination).unwrap();
        member.node.monitor_error = error.clone();
        member.input_ready = id;
        self.monitor_command.as_mut().unwrap().result = Some(MonitorResult { error });
        Ok(())
    }
    pub fn expire_monitor(&mut self) {
        if let Some(command) = &mut self.monitor_command
            && command.result.is_none()
            && command.interrupted.is_none()
            && now_ms() >= command.deadline_ms
        {
            let error="Monitor command timed out. Waiting for its writer to confirm cancellation; no retry was sent.".to_owned();
            command.interrupted = Some(error.clone());
            self.confirmed_monitor = None;
            self.members
                .get_mut(&command.destination)
                .unwrap()
                .node
                .monitor_error = Some(error);
        }
    }
    /// Release borrowed input after its local fail-open, preserving the last
    /// physical monitor selection. Only an explicit Activate may request DDC.
    pub fn return_input(&mut self) -> Result<()> {
        ensure!(
            !self.switch_pending(),
            "The previous switch is still finishing. Wait for the monitor before switching again."
        );
        let source = self.plan().input;
        let revision = self.next_revision()?;
        let activation = self
            .activation
            .checked_add(1)
            .context("Too many input switches")?;
        self.active_input = source.clone();
        if self.edge_paused {
            self.edge_policy_revision = revision;
        }
        self.edge_paused = false;
        self.input_intent = source.clone();
        self.active_input_entry = None;
        self.active_display = None;
        self.display_entry = None;
        self.revision = revision;
        self.activation = activation;
        self.input_request = activation;
        if let Some(source) = source {
            self.members.get_mut(&source).unwrap().input_ready = activation;
        }
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
                        id: format!("display-{index}"),
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
            authority: None,
            revision: 0,
            settings: Settings::default(),
            edge_paused: false,
            edge_policy_revision: 0,
            members,
            active_input: None,
            input_intent: None,
            input_request: 0,
            active_input_entry: None,
            activation: 0,
            active_monitor: None,
            confirmed_monitor: None,
            monitor_command: None,
            layout: Default::default(),
            active_display: None,
            display_entry: None,
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
    fn failed_switches_preserve_input_monitor_entry_and_counters() {
        for position in [None, Some(12_345)] {
            let mut state = fixture(3);
            state.settings.monitor_enabled = true;
            state.active_input = Some(key(1));
            state.active_monitor = Some(key(1));
            state.active_input_entry = Some(999);
            let original = state.clone();
            assert!(state.activate(&key(1), &key(4), position).is_err());
            assert_eq!(state, original);

            state.members.get_mut(&key(2)).unwrap().node.monitor.input = Some(15);
            let original = state.clone();
            assert!(state.activate(&key(1), &key(2), position).is_err());
            assert_eq!(state, original);

            state.members.get_mut(&key(2)).unwrap().node.monitor.input = Some(16);
            state.revision = u64::MAX - 1;
            let original = state.clone();
            assert!(state.activate(&key(1), &key(2), position).is_err());
            assert_eq!(state, original);

            state.revision = 0;
            state.activation = u64::MAX;
            state.members.get_mut(&key(1)).unwrap().input_ready = u64::MAX;
            let original = state.clone();
            assert!(state.activate(&key(1), &key(2), position).is_err());
            assert_eq!(state, original);
        }
    }

    #[test]
    fn switches_wait_for_terminal_monitor_ack_and_allow_one_explicit_retry() {
        let mut state = fixture(3);
        state.settings.monitor_enabled = true;
        state.activate(&key(1), &key(2), Some(12_345)).unwrap();
        assert!(state.switch_pending());
        let pending = state.clone();
        let mut disabled = state.settings.clone();
        disabled.monitor_enabled = false;
        assert!(state.set_settings(state.revision, disabled).is_err());
        assert_eq!(
            state, pending,
            "Disabling video cannot bypass an in-flight monitor command"
        );
        assert!(state.activate(&key(1), &key(3), None).is_err());
        assert!(state.activate(&key(1), &key(2), Some(12_345)).is_err());
        assert_eq!(state, pending);

        let mut node = state.members[&key(2)].node.clone();
        node.monitor_error = Some("A failure from an earlier attempt".into());
        state.update_node(&key(2), node.clone(), None, 0).unwrap();
        assert!(state.switch_pending());
        assert!(state.members[&key(2)].node.monitor_error.is_none());
        state.update_node(&key(2), node, None, 1).unwrap();
        assert!(
            state.switch_pending(),
            "A target snapshot cannot acknowledge the writer's command"
        );
        assert!(state.monitor_result(&key(2), 1, None).is_err());
        state
            .monitor_result(&key(1), 1, Some("/dev/i2c-5: EIO".into()))
            .unwrap();
        assert!(!state.switch_pending());
        assert!(state.confirmed_monitor.is_none());
        state.activate(&key(1), &key(2), Some(12_345)).unwrap();
        assert_eq!(state.monitor_command.as_ref().unwrap().writer, key(1));
        assert_eq!(state.activation, 2);
        assert!(state.switch_pending());
        assert!(state.members[&key(2)].node.monitor_error.is_none());
        assert!(state.activate(&key(1), &key(2), Some(12_345)).is_err());

        let mut node = state.members[&key(2)].node.clone();
        node.input_error = Some("Native input is still reconnecting".into());
        state.update_node(&key(2), node, None, 2).unwrap();
        assert!(state.switch_pending());
        state.monitor_result(&key(1), 2, None).unwrap();
        assert!(
            !state.switch_pending(),
            "Input authentication must not hold the monitor gate"
        );
        let completed = state.clone();
        state.activate(&key(1), &key(2), Some(12_345)).unwrap();
        assert!(state.monitor_command.is_none());
        assert_eq!(state.confirmed_monitor, completed.confirmed_monitor);
        assert_eq!(state.activation, completed.activation + 1);
        assert_eq!(state.input_request, state.activation);

        state.activate(&key(1), &key(3), None).unwrap();
        assert_eq!(
            state.monitor_command.as_ref().unwrap().writer,
            key(2),
            "The visible PC writes, independently of keyboard ownership"
        );
        assert!(state.switch_pending());
        state.members.get_mut(&key(3)).unwrap().online = false;
        assert!(
            state.switch_pending(),
            "Offline status cannot permit overlapping physical commands"
        );
        state.monitor_command.as_mut().unwrap().deadline_ms = 1;
        state.expire_monitor();
        assert!(state.switch_pending());
        assert!(state.activate(&key(1), &key(1), None).is_err());
        state
            .monitor_result(&key(2), state.activation, None)
            .unwrap();
        assert!(!state.switch_pending());
        assert!(state.confirmed_monitor.is_none());
        state.activate(&key(1), &key(1), None).unwrap();
        let before = state.clone();
        let node = state.members[&key(1)].node.clone();
        assert!(
            state
                .update_node(&key(1), node, None, state.activation + 1)
                .is_err()
        );
        assert_eq!(state, before);
    }

    #[test]
    fn failed_input_returns_preserve_the_monitor_and_cannot_bypass_an_inflight_switch() {
        let mut state = fixture(3);
        state.settings.monitor_enabled = true;
        state.activate(&key(1), &key(2), Some(123)).unwrap();
        let pending = state.clone();
        assert!(state.return_input().is_err());
        assert_eq!(state, pending);
        let node = state.members[&key(2)].node.clone();
        state
            .update_node(&key(2), node, None, state.activation)
            .unwrap();
        assert!(state.switch_pending());
        state
            .monitor_result(&key(1), state.activation, None)
            .unwrap();
        let monitor = state.active_monitor.clone();
        state.return_input().unwrap();
        assert_eq!(state.active_input, Some(key(1)));
        assert_eq!(state.active_monitor, monitor);
        assert_eq!(state.active_input_entry, None);
        assert_eq!(state.activation, 2);
        assert_eq!(state.members[&key(1)].input_ready, state.activation);
        assert!(!state.switch_pending());
        let returned = state.clone();
        state.return_input().unwrap();
        assert_eq!(state.active_input, returned.active_input);
        assert_eq!(state.active_monitor, returned.active_monitor);
        assert_eq!(state.monitor_command, returned.monitor_command);
        assert_eq!(state.input_request, returned.input_request + 1);
    }

    #[test]
    fn delayed_monitor_results_cannot_modify_later_generations() {
        let mut state = fixture(3);
        state.settings.monitor_enabled = true;
        state.activate(&key(1), &key(2), None).unwrap();
        state.monitor_result(&key(1), 1, None).unwrap();
        state.return_input().unwrap();
        let returned = state.clone();
        state.monitor_result(&key(1), 1, None).unwrap();
        assert_eq!(state, returned);
        state.activate(&key(1), &key(3), None).unwrap();
        let pending = state.clone();
        state.monitor_result(&key(1), 1, None).unwrap();
        assert_eq!(state, pending);
        assert!(
            state
                .monitor_result(&key(1), 1, Some("bad\0error".into()))
                .is_err()
        );
        assert!(state.monitor_result(&key(1), 3, None).is_err());
        assert!(state.monitor_result(&key(2), 4, None).is_err());
        assert_eq!(state, pending);
        state
            .monitor_result(&key(2), 3, Some("I/O error after issuance".into()))
            .unwrap();
        let failed = state.clone();
        state.monitor_result(&key(1), 1, None).unwrap();
        assert_eq!(state, failed);
    }

    #[test]
    fn legacy_failed_destination_is_not_treated_as_a_confirmed_writer() {
        let mut previous = fixture(3);
        previous.settings.monitor_enabled = true;
        previous.active_input = Some(key(2));
        previous.active_monitor = Some(key(2));
        previous.activation = 7;
        previous.members.get_mut(&key(2)).unwrap().input_ready = 7;
        previous
            .members
            .get_mut(&key(2))
            .unwrap()
            .node
            .monitor_error = Some("DDCRC_RETRIES(EIO)".into());
        let mut json = serde_json::to_value(previous).unwrap();
        json.as_object_mut().unwrap().remove("confirmed_monitor");
        json.as_object_mut().unwrap().remove("monitor_command");
        let mut state: Workspace = serde_json::from_value(json).unwrap();
        state.validate().unwrap();
        assert!(state.monitor_command.is_none() && state.confirmed_monitor.is_none());
        state.activate(&key(1), &key(2), None).unwrap();
        let command = state.monitor_command.as_ref().unwrap();
        assert_eq!(command.writer, key(1));
        assert_eq!(command.destination, key(2));
        assert_eq!(command.input, 16);
        assert_eq!(state.members[&key(2)].input_ready, 0);
        assert!(state.switch_pending());
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
        workspace.update_node(&key(2), node, None, 0).unwrap();
        assert_eq!(workspace.members[&key(2)].preferences.audio.volume, 37);
        assert!(!workspace.settings.microphone_enabled);
        assert_eq!(workspace.revision, 0);
        let wrong_identity = workspace.members[&key(3)].node.clone();
        assert!(
            workspace
                .update_node(&key(2), wrong_identity, None, 0)
                .is_err()
        );
    }

    #[test]
    fn explicit_pc_change_selects_the_monitor_once_even_when_confirmation_was_cached() {
        let mut state = fixture(2);
        state.settings.monitor_enabled = true;
        let source = key(1);
        let target = key(2);
        state.activate(&source, &target, None).unwrap();
        state
            .monitor_result(&source, state.activation, None)
            .unwrap();
        state.return_input().unwrap();
        assert_eq!(state.active_input.as_ref(), Some(&source));
        assert_eq!(state.confirmed_monitor.as_ref(), Some(&target));

        state.activate(&source, &target, None).unwrap();
        let command = state.monitor_command.clone().unwrap();
        assert_eq!(command.destination, target);
        assert_eq!(command.writer, target);
        assert_eq!(command.input, 16);
        assert!(state.switch_pending());
        assert_eq!(state.members[&target].input_ready, 0);
        let pending = state.clone();
        assert!(state.activate(&source, &target, None).is_err());
        assert_eq!(state, pending);
        state.monitor_result(&target, command.id, None).unwrap();
        assert_eq!(state.members[&target].input_ready, state.activation);
        let completed = state.activation;
        state.activate(&source, &target, None).unwrap();
        assert!(state.monitor_command.is_none());
        assert_eq!(state.input_request, completed + 1);
        state.validate().unwrap();
    }

    #[test]
    fn monitor_binding_changes_invalidate_confirmation_but_cosmetic_updates_do_not() {
        let mut original = fixture(2);
        original.confirmed_monitor = Some(key(2));
        let mut cosmetic = original.members[&key(1)].node.clone();
        cosmetic.monitor.name = "A clearer monitor name".into();
        original.update_node(&key(1), cosmetic, None, 0).unwrap();
        assert_eq!(original.confirmed_monitor, Some(key(2)));
        for changed in 0..3 {
            let mut state = original.clone();
            let mut node = state.members[&key(1)].node.clone();
            match changed {
                0 => node.monitor.id = "another-connector".into(),
                1 => node.monitor.input = Some(99),
                _ => node.monitor.enabled = false,
            }
            state.update_node(&key(1), node, None, 0).unwrap();
            assert!(state.confirmed_monitor.is_none());
            assert!(state.monitor_command.is_none());
        }
    }

    #[test]
    fn temporary_disconnect_restores_input_intent_without_repeating_monitor_commands() {
        let mut state = fixture(2);
        state.settings.monitor_enabled = true;
        let source = key(1);
        let target = key(2);
        state.activate(&source, &target, None).unwrap();
        state
            .monitor_result(&source, state.activation, None)
            .unwrap();
        let monitor = state.active_monitor.clone();
        let confirmed = state.confirmed_monitor.clone();
        let command = state.monitor_command.clone();
        let selected = state.activation;
        state.members.get_mut(&target).unwrap().online = false;
        state.reconcile_input();
        assert_eq!(state.active_input.as_ref(), Some(&source));
        assert_eq!(state.input_intent.as_ref(), Some(&target));
        assert!(state.activation > selected);
        assert_eq!(state.members[&source].input_ready, state.activation);
        assert_eq!(state.monitor_command, command);

        // The intention survives a restart while the destination is offline.
        let mut restored: Workspace =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        let fallback = restored.activation;
        restored.members.get_mut(&target).unwrap().online = true;
        restored.reconcile_input();
        assert_eq!(restored.active_input.as_ref(), Some(&target));
        assert_eq!(restored.input_intent.as_ref(), Some(&target));
        assert!(restored.activation > fallback);
        assert_eq!(restored.members[&target].input_ready, restored.activation);
        assert_eq!(restored.active_monitor, monitor);
        assert_eq!(restored.confirmed_monitor, confirmed);
        assert_eq!(restored.monitor_command, command);
        restored.validate().unwrap();

        state.return_input().unwrap();
        state.members.get_mut(&target).unwrap().online = true;
        state.reconcile_input();
        assert_eq!(state.active_input.as_ref(), Some(&source));
        assert_eq!(state.input_intent.as_ref(), Some(&source));
        assert_eq!(state.monitor_command, command);
        assert_eq!(state.active_monitor, monitor);
        state.validate().unwrap();
    }

    #[test]
    fn recovery_intent_cannot_bypass_pending_or_failed_monitor_selection() {
        let mut state = fixture(2);
        state.settings.monitor_enabled = true;
        let source = key(1);
        let target = key(2);
        state.activate(&source, &target, None).unwrap();
        let pending = state.clone();
        state.members.get_mut(&target).unwrap().online = false;
        state.reconcile_input();
        assert_eq!(state.activation, pending.activation);
        assert_eq!(state.active_input, pending.active_input);
        assert_eq!(state.monitor_command, pending.monitor_command);
        state
            .monitor_result(&source, state.activation, Some("DDCRC_RETRIES(EIO)".into()))
            .unwrap();
        state.reconcile_input();
        assert_eq!(state.active_input.as_ref(), Some(&source));
        let fallback = state.clone();
        state.members.get_mut(&target).unwrap().online = true;
        state.reconcile_input();
        assert_eq!(state.active_input.as_ref(), Some(&source));
        assert_eq!(state.input_intent.as_ref(), Some(&target));
        assert_eq!(state.activation, fallback.activation);
        assert_eq!(state.monitor_command, fallback.monitor_command);
        state.validate().unwrap();
    }

    #[test]
    fn physical_input_hotplug_moves_auto_owner_without_changing_the_monitor_or_manual_choice() {
        let mut state = fixture(2);
        let windows = key(1);
        let ubuntu = key(2);
        state.active_input = Some(ubuntu.clone());
        state.active_monitor = Some(ubuntu.clone());
        let monitor = state.active_monitor.clone();
        let activation = state.activation;
        assert_eq!(state.plan().input, Some(windows.clone()));

        let mut unplugged = state.members[&windows].node.clone();
        let keyboards = std::mem::take(&mut unplugged.devices.keyboards);
        let mice = std::mem::take(&mut unplugged.devices.mice);
        unplugged.hardware = workspace::hardware::hardware(
            &unplugged.devices,
            &state.members[&windows].preferences.devices,
            true,
        );
        state.update_node(&windows, unplugged, None, 0).unwrap();
        state.reconcile_input();
        assert!(state.plan().input.is_none());
        assert!(state.routes(&windows).input_targets.is_empty());
        assert!(state.routes(&ubuntu).input_source.is_none());

        let mut attached = state.members[&ubuntu].node.clone();
        attached.devices.keyboards = keyboards;
        attached.devices.mice = mice;
        attached.hardware = workspace::hardware::hardware(
            &attached.devices,
            &state.members[&ubuntu].preferences.devices,
            true,
        );
        state.update_node(&ubuntu, attached, None, 0).unwrap();
        state.reconcile_input();
        assert_eq!(state.plan().input, Some(ubuntu.clone()));
        assert_eq!(state.routes(&windows).input_source.unwrap().key, ubuntu);
        assert_eq!(state.routes(&ubuntu).input_targets[0].key, windows);
        assert_eq!(state.active_input.as_ref(), Some(&ubuntu));
        assert_eq!(state.active_monitor, monitor);
        assert_eq!(state.activation, activation);
        assert!(state.monitor_command.is_none());
        state.validate().unwrap();

        state.settings.input = Selection::Computer(windows.clone());
        state.reconcile_input();
        assert!(state.plan().input.is_none());
        assert_eq!(state.settings.input, Selection::Computer(windows));
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
