//! Phone sources inside the original workspace, without adding workspace voters.
use super::topology::{PHONE_RELAY_CAPABILITY, PhoneState, phone_id};
use crate::{
    group::model::Workspace,
    mobile::{Destination, Device, Event, Pending, Routing, Service, Trusted},
    workspace::{Node, devices::Inventory},
};
use eframe::egui;
use std::collections::{BTreeMap, VecDeque};

const GREEN: egui::Color32 = egui::Color32::from_rgb(83, 217, 177);

#[derive(Default)]
pub(super) struct Panel {
    service: Option<Service>,
    pending: VecDeque<Pending>,
    trusted: BTreeMap<String, Trusted>,
    devices: BTreeMap<String, Device>,
    status: String,
    screen_preview: bool,
}
impl Panel {
    #[cfg(debug_assertions)]
    pub fn fixture(&mut self) {
        for (key, name, connected, shared) in [
            ("aa", "Pixel phone", true, true),
            ("bb", "Galaxy phone", true, false),
            ("cc", "Tablet", false, true),
        ] {
            self.trusted.insert(
                key.repeat(32),
                Trusted {
                    name: name.into(),
                    shared,
                },
            );
            self.devices.insert(
                key.repeat(32),
                Device {
                    name: name.into(),
                    connected,
                    shared,
                },
            );
        }
        self.status = "Ready for phone remotes".into();
    }
    pub fn available(&self) -> bool {
        self.service.is_some()
    }

    pub fn screen_allowed(&self) -> bool {
        self.available() && self.screen_preview
    }
    pub fn enable(
        &mut self,
        dir: &std::path::Path,
        name: String,
        ctx: &egui::Context,
        screen_preview: bool,
    ) {
        self.screen_preview = screen_preview;
        match Service::start(dir.into(), name, ctx.clone()) {
            Ok(service) => {
                service.allow_screen(screen_preview);
                self.service = Some(service);
                self.status = "Available over Wi-Fi".into();
            }
            Err(_) => {
                self.status = "Phone control could not start. Restart Desk Switch to retry.".into()
            }
        }
    }
    pub fn tick(&mut self, quitting: bool) -> bool {
        if quitting {
            self.service = None;
            self.pending.clear();
            self.devices.clear();
            return false;
        }
        let mut show = false;
        if let Some(service) = &self.service {
            for event in service.events.try_iter().take(64) {
                match event {
                    Event::Pair(pending) => {
                        if self.pending.len() < 16
                            && !self.pending.iter().any(|item| item.key == pending.key)
                        {
                            self.pending.push_back(pending);
                            show = true;
                        }
                    }
                    Event::Status(status) => self.status = status,
                    Event::Trusted(peers) => self.trusted = peers,
                    Event::Devices(devices) => self.devices = devices,
                }
            }
        }
        self.pending.retain(|pending| !pending.reply.is_closed());
        show
    }
    pub fn confirmation(&mut self, ctx: &egui::Context) {
        if let Some(pending) = self.pending.front() {
            let mut decision = None;
            egui::Modal::new(egui::Id::new("phone-pairing")).show(ctx, |ui| {
                ui.heading("Connect phone remote");
                ui.label(format!(
                    "{} wants to use its touchpad and keyboard with this workspace.",
                    pending.name
                ));
                ui.label("Check that this code matches the code on the phone:");
                ui.label(egui::RichText::new(&pending.code).size(32.0).strong());
                ui.horizontal(|ui| {
                    if ui.button("Connect phone").clicked() {
                        decision = Some(true);
                    }
                    if ui.button("Decline").clicked() {
                        decision = Some(false);
                    }
                });
                if self.pending.len() > 1 {
                    ui.weak(format!("{} more requests waiting", self.pending.len() - 1));
                }
            });
            if let Some(accept) = decision
                && let Some(pending) = self.pending.pop_front()
            {
                let _ = pending.reply.send(accept);
            }
        }
    }
    pub fn annotate(&self, node: &mut Node) {
        // Reuse the existing device inventory. These are peripherals, not peers.
        self.inventory(&mut node.devices);
        node.diagnostics
            .logs
            .retain(|line| line != PHONE_RELAY_CAPABILITY);
        if self.available() {
            if node.diagnostics.logs.len() == crate::workspace::diagnostics::LINES {
                node.diagnostics.logs.remove(0);
            }
            node.diagnostics.logs.push(PHONE_RELAY_CAPABILITY.into());
        }
    }
    fn inventory(&self, inventory: &mut Inventory) {
        for list in [&mut inventory.keyboards, &mut inventory.mice] {
            list.retain(|device| phone_id(&device.id).is_none());
            list.truncate(32usize.saturating_sub(self.devices.len().min(16)));
            for (key, device) in self.devices.iter().take(16) {
                let prefix = if !device.connected {
                    "desk-switch-phone-offline"
                } else if device.shared {
                    "desk-switch-phone"
                } else {
                    "desk-switch-phone-local"
                };
                list.push(crate::audio::AudioDevice {
                    id: format!("{prefix}:{key}"),
                    name: device.name.clone(),
                    default: false,
                });
            }
        }
    }
    pub fn route(&self, state: Option<&Workspace>, own: &str, paused: bool) {
        let Some(service) = &self.service else { return };
        let members = state
            .into_iter()
            .flat_map(|state| state.members.values())
            .filter(|member| member.compatible())
            .map(|member| member.node.key.clone())
            .collect();
        let generation = state.map_or(0, |state| state.activation);
        let destination = destination(state, own, paused);
        service.set_routing(Routing {
            generation,
            destination,
            members,
        });
    }
    pub fn shared_devices(&mut self, ui: &mut egui::Ui, state: &Workspace, own: &str) {
        let mut rows = Vec::new();
        for member in state.members.values() {
            for device in &member.node.devices.keyboards {
                if let Some((key, status)) = phone_id(&device.id) {
                    rows.push((member, key, status, &device.name));
                }
            }
        }
        if rows.is_empty() {
            return;
        }
        ui.separator();
        ui.strong("Phone remotes");
        for (host, key, status, name) in rows {
            let connected = host.compatible() && status != PhoneState::Offline;
            let local = host.node.key == own && self.service.is_some();
            let mut shared = if local {
                self.devices.get(key).is_some_and(|device| device.shared)
            } else {
                status == PhoneState::Shared
            };
            ui.horizontal_wrapped(|ui| {
                phone_icon(ui, connected);
                ui.label(egui::RichText::new(name).strong());
                ui.weak(format!("via {}", host.node.name));
                if connected {
                    ui.colored_label(GREEN, "Connected");
                } else {
                    ui.weak("Offline");
                }
                if ui
                    .add_enabled(local, egui::Checkbox::new(&mut shared, "Shared"))
                    .on_disabled_hover_text("Manage this phone on its paired computer")
                    .changed()
                    && let Some(service) = &self.service
                    && let Err(error) = service.set_shared(key, shared)
                {
                    self.status = format!("Could not change phone sharing: {error:#}");
                }
            });
            ui.weak(if !connected {
                "Reconnect from the phone to continue."
            } else if shared {
                "Touchpad and keyboard follow the selected computer."
            } else {
                "Touchpad and keyboard control the paired computer only."
            });
        }
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        dir: &std::path::Path,
        name: String,
        enabled: &mut bool,
        screen_preview: &mut bool,
    ) -> bool {
        let mut changed = false;
        egui::Frame::group(ui.style()).inner_margin(16).show(ui, |ui| {
            ui.strong("Phone remotes · Experimental");
            ui.label("Use Android phones as touchpads and keyboards.");
            if ui.checkbox(enabled, "Allow phone remotes").changed() {
                changed = true;
                if *enabled { self.enable(dir, name, ui.ctx(), *screen_preview); }
                else {
                    self.service = None; self.pending.clear(); self.devices.clear();
                    self.status = "Phone control is off".into();
                    super::activity::disconnected(ui.ctx(), u32::MAX);
                }
            }
            if ui.checkbox(screen_preview, "Allow cursor preview").changed() {
                changed = true;
                self.screen_preview = *screen_preview;
                if let Some(service) = &self.service { service.allow_screen(*screen_preview); }
            }
            ui.weak("Cursor preview is off by default. A phone must also turn it on before any screen area is sent.");
            if !self.status.is_empty() { ui.label(&self.status); }
            ui.weak("Open Desk Switch on your phone on the same Wi-Fi. Confirm matching codes once; saved devices reconnect without another code.");
            let mut forget = None;
            for (key, peer) in &self.trusted {
                ui.horizontal_wrapped(|ui| {
                    let connected = self.devices.get(key).is_some_and(|device| device.connected);
                    phone_icon(ui, connected);
                    ui.label(&peer.name);
                    ui.weak(if connected { "Connected" } else { "Offline" });
                    if ui.button("Forget phone").clicked() { forget = Some(key.clone()); }
                });
            }
            if let Some(key) = forget && let Some(service) = &self.service {
                if service.forget(&key).is_ok() {
                    self.trusted.remove(&key); self.devices.remove(&key);
                    self.status = "Phone forgotten".into();
                } else { self.status = "Could not forget the phone".into(); }
            }
        });
        changed
    }
}

fn destination(state: Option<&Workspace>, own: &str, paused: bool) -> Destination {
    if paused {
        return Destination::Paused {
            reason: "Phone sharing is paused on this computer".into(),
        };
    }
    let Some(state) = state else {
        return Destination::Local;
    };
    if state.switch_pending() {
        return Destination::Paused {
            reason: "Waiting for the monitor and input switch".into(),
        };
    }
    let target = state
        .active_input
        .as_ref()
        .or(state.confirmed_monitor.as_ref())
        .map_or(own, String::as_str);
    let Some(member) = state
        .members
        .get(target)
        .filter(|member| member.compatible())
    else {
        return Destination::Paused {
            reason: "The selected computer is unavailable".into(),
        };
    };
    if member.node.monitor_error.is_some() {
        return Destination::Paused {
            reason: "The monitor switch failed. Retry the switch before sending phone input."
                .into(),
        };
    }
    if member.input_ready != state.activation {
        return Destination::Paused {
            reason: "Waiting for the selected computer to finish switching".into(),
        };
    }
    if target == own {
        return Destination::Local;
    }
    if !member
        .node
        .diagnostics
        .logs
        .iter()
        .any(|line| line == PHONE_RELAY_CAPABILITY)
    {
        return Destination::Paused {
            reason: format!(
                "Install the experimental Desk Switch build on {} to control it from the phone",
                member.node.name
            ),
        };
    }
    Destination::Remote {
        key: member.node.key.clone(),
        name: member.node.name.clone(),
        address: member.address,
        control_port: crate::mobile::CONTROL_PORT,
        input_port: crate::mobile::INPUT_PORT,
    }
}
fn phone_icon(ui: &mut egui::Ui, connected: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 24.0), egui::Sense::hover());
    let color = if connected {
        GREEN
    } else {
        ui.visuals().weak_text_color()
    };
    let bounds = rect.shrink2(egui::vec2(2.5, 1.0));
    super::topology::phone_glyph(ui.painter(), bounds, color, ui.visuals().panel_fill);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn phones_publish_as_peripherals_with_distinct_presence_and_sharing_states() {
        let mut panel = Panel::default();
        for (key, connected, shared) in
            [("11", true, true), ("22", true, false), ("33", false, true)]
        {
            panel.devices.insert(
                key.repeat(32),
                Device {
                    name: format!("Phone {key}"),
                    connected,
                    shared,
                },
            );
        }
        let mut inventory = Inventory::default();
        panel.inventory(&mut inventory);
        assert_eq!(inventory.keyboards, inventory.mice);
        assert_eq!(inventory.keyboards.len(), 3);
        assert_eq!(
            inventory.keyboards[0].id,
            format!("desk-switch-phone:{}", "11".repeat(32))
        );
        assert_eq!(
            inventory.keyboards[1].id,
            format!("desk-switch-phone-local:{}", "22".repeat(32))
        );
        assert_eq!(
            inventory.keyboards[2].id,
            format!("desk-switch-phone-offline:{}", "33".repeat(32))
        );
        panel.inventory(&mut inventory);
        assert_eq!(inventory.keyboards.len(), 3);
    }
    #[test]
    fn local_standalone_phone_respects_pause_without_a_workspace() {
        assert!(matches!(
            destination(None, "own", false),
            Destination::Local
        ));
        assert!(matches!(
            destination(None, "own", true),
            Destination::Paused { .. }
        ));
    }
    #[test]
    fn workspace_phone_route_requires_target_capability_and_stops_for_unavailable_members() {
        let mut state = crate::group::model::tests::fixture(2);
        let own = format!("{:064x}", 1);
        let target = format!("{:064x}", 2);
        state.active_input = Some(target.clone());
        assert!(matches!(
            destination(Some(&state), &own, false),
            Destination::Paused { .. }
        ));
        state
            .members
            .get_mut(&target)
            .unwrap()
            .node
            .diagnostics
            .logs
            .push(PHONE_RELAY_CAPABILITY.into());
        assert!(
            matches!(destination(Some(&state), &own, false), Destination::Remote { key, .. } if key == target)
        );
        state.members.get_mut(&target).unwrap().node.monitor_error =
            Some("No destination signal".into());
        assert!(matches!(
            destination(Some(&state), &own, false),
            Destination::Paused { .. }
        ));
        state.members.get_mut(&target).unwrap().node.monitor_error = None;
        state.members.get_mut(&target).unwrap().online = false;
        assert!(matches!(
            destination(Some(&state), &own, false),
            Destination::Paused { .. }
        ));
    }
}
