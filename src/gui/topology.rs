//! Shared workspace illustration. Motion runs only while the scene is visible.
use super::view::{DeviceIcon, WorkspaceVisual, paint_device, peer_color};
use crate::group::model::Workspace;
use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};

const CYAN: Color32 = Color32::from_rgb(75, 196, 244);
const GREEN: Color32 = Color32::from_rgb(83, 217, 177);
pub(super) const PHONE_RELAY_CAPABILITY: &str = "Phone remote: available";
#[path = "topology_motion.rs"]
mod motion;
use motion::{Fade, Phase};

#[derive(Clone, Default, PartialEq)]
pub(super) struct Scene {
    peers: Vec<Peer>,
    phones: Vec<PhoneSource>,
    ports: [Port; 5],
    pending: bool,
}
#[derive(Clone, PartialEq)]
struct PhoneSource {
    key: String,
    name: String,
    owner: String,
    target: Option<String>,
    connected: bool,
    shared: bool,
    keyboard: bool,
    mouse: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PhoneState {
    Shared,
    Local,
    Offline,
}
pub(super) fn phone_id(id: &str) -> Option<(&str, PhoneState)> {
    let (key, state) = if let Some(key) = id.strip_prefix("desk-switch-phone:") {
        (key, PhoneState::Shared)
    } else if let Some(key) = id.strip_prefix("desk-switch-phone-local:") {
        (key, PhoneState::Local)
    } else {
        (
            id.strip_prefix("desk-switch-phone-offline:")?,
            PhoneState::Offline,
        )
    };
    crate::group::model::valid_key(key).ok()?;
    Some((key, state))
}
#[derive(Clone, PartialEq)]
struct Peer {
    key: String,
    name: String,
    platform: String,
    online: bool,
    available: bool,
    host: bool,
    active: bool,
    local: bool,
    color: Color32,
}
#[derive(Clone, Default, PartialEq)]
struct Port {
    source: Option<String>,
    destinations: Vec<String>,
    caption: String,
    muted: bool,
    error: bool,
}

#[derive(Clone, Copy)]
struct MouseWave {
    angle: f32,
    opacity: f32,
    updated: f64,
    fade: Fade,
}
impl MouseWave {
    fn update(&mut self, direction: Vec2, active_until: f64, now: f64) {
        if now - self.updated > 1.2 {
            self.fade = Fade::default();
        }
        let dt = (now - self.updated).clamp(0.0, 0.1) as f32;
        self.updated = now;
        if direction.length_sq() > 0.5 {
            // Follow the shortest arc, including across the -PI/PI boundary.
            let difference = (direction.angle() - self.angle + std::f32::consts::PI)
                .rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            self.angle += difference * (1.0 - (-dt / 0.18).exp());
        }
        self.opacity = self.fade.update_until(active_until, now);
    }
}

fn mouse_wave(ui: &egui::Ui, activity: &super::activity::Activity, now: f64) -> MouseWave {
    let id = ui
        .make_persistent_id("mouse-wave-follow")
        .with(ui.ctx().viewport_id());
    ui.ctx().data_mut(|data| {
        let mut wave = data.get_temp::<MouseWave>(id).unwrap_or(MouseWave {
            angle: activity.direction.angle(),
            opacity: 0.0,
            updated: now,
            fade: Fade::default(),
        });
        wave.update(activity.direction, activity.moved + 0.65, now);
        data.insert_temp(id, wave);
        wave
    })
}
impl Scene {
    pub(super) fn from_workspace(state: &Workspace, local: &str) -> Self {
        let plan = state.plan();
        let visual = WorkspaceVisual::from_workspace(state);
        let host = plan
            .input
            .as_deref()
            .or(plan.audio.as_deref())
            .unwrap_or(&state.hub);
        let peers = state
            .members
            .values()
            .enumerate()
            .map(|(index, member)| Peer {
                key: member.node.key.clone(),
                name: member.node.name.clone(),
                platform: member.node.diagnostics.platform.clone(),
                online: member.online,
                available: member.compatible(),
                host: member.node.key == host,
                active: visual.input_target.as_ref() == Some(&member.node.key),
                local: member.node.key == local,
                color: peer_color(index),
            })
            .collect();
        // Device order is shared by both views: sound, display, keyboard, mouse, mic.
        let ports = std::array::from_fn(|index| {
            if index == 1 {
                if !state.settings.monitor_enabled {
                    return Port::default();
                }
                let source = state
                    .members
                    .get(host)
                    .filter(|m| m.compatible() && m.node.monitor.enabled)
                    .or_else(|| {
                        state
                            .members
                            .values()
                            .find(|m| m.compatible() && m.node.monitor.enabled)
                    });
                let current = state
                    .confirmed_monitor
                    .as_ref()
                    .and_then(|key| state.members.get(key))
                    .filter(|m| m.compatible());
                return Port {
                    source: source.map(|m| m.node.key.clone()),
                    destinations: current
                        .filter(|m| source.is_some_and(|s| s.node.key != m.node.key))
                        .map(|m| vec![m.node.key.clone()])
                        .unwrap_or_default(),
                    caption: if state.switch_pending() {
                        tr("Switching…")
                    } else {
                        current
                            .map(|m| m.node.name.clone())
                            .unwrap_or_else(|| tr("Not connected"))
                    },
                    error: state.monitor_command.as_ref().is_some_and(|c| {
                        c.interrupted.is_some()
                            || c.result.as_ref().is_some_and(|r| r.error.is_some())
                    }),
                    muted: false,
                };
            }
            let device_index = [2, 0, 0, 1, 3][index];
            let source = visual.devices[device_index].as_ref();
            let destinations = state
                .members
                .values()
                .filter(|m| {
                    m.compatible()
                        && source.is_some_and(|s| s.key != m.node.key)
                        && visual.member_devices(state, &m.node.key)[device_index].is_some()
                })
                .map(|m| m.node.key.clone())
                .collect();
            Port {
                source: source.map(|s| s.key.clone()),
                caption: source
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| tr("Not shared")),
                muted: source.is_some_and(|s| s.muted),
                destinations,
                error: false,
            }
        });
        let mut phones = std::collections::BTreeMap::<(String, String), PhoneSource>::new();
        for member in state.members.values() {
            for (keyboard, devices) in [
                (true, &member.node.devices.keyboards),
                (false, &member.node.devices.mice),
            ] {
                for device in devices {
                    let Some((key, status)) = phone_id(&device.id) else {
                        continue;
                    };
                    let shared = matches!(status, PhoneState::Shared);
                    let connected = !matches!(status, PhoneState::Offline) && member.compatible();
                    let selected = if shared {
                        state
                            .active_input
                            .as_ref()
                            .or(state.confirmed_monitor.as_ref())
                            .unwrap_or(&member.node.key)
                    } else {
                        &member.node.key
                    };
                    let target =
                        if connected {
                            state
                                .members
                                .get(selected)
                                .filter(|target| {
                                    target.compatible()
                                        && (!shared
                                            || !state.switch_pending()
                                                && target.input_ready == state.activation
                                                && target.node.monitor_error.is_none()
                                                && (selected == &member.node.key
                                                    || target.node.diagnostics.logs.iter().any(
                                                        |line| line == PHONE_RELAY_CAPABILITY,
                                                    )))
                                })
                                .map(|target| target.node.key.clone())
                        } else {
                            None
                        };
                    let phone = phones
                        .entry((member.node.key.clone(), key.to_owned()))
                        .or_insert_with(|| PhoneSource {
                            key: key.to_owned(),
                            name: device.name.clone(),
                            owner: member.node.key.clone(),
                            target,
                            connected,
                            shared,
                            keyboard: false,
                            mouse: false,
                        });
                    if keyboard {
                        phone.keyboard = true;
                    } else {
                        phone.mouse = true;
                    }
                }
            }
        }
        Self {
            peers,
            phones: phones.into_values().collect(),
            ports,
            pending: state.switch_pending(),
        }
    }
    fn phone_shares(&self, index: usize) -> bool {
        self.phones.iter().any(|phone| {
            phone.connected
                && phone.shared
                && phone.target.is_some()
                && ((index == 2 && phone.keyboard) || (index == 3 && phone.mouse))
        })
    }
}

fn tr(text: &str) -> String {
    crate::i18n::tr(text)
}

/// Returns a requested input destination. All switching goes through existing actions.
pub(super) fn show(
    ui: &mut egui::Ui,
    scene: &Scene,
    compact: bool,
    can_switch: bool,
) -> Option<String> {
    if scene.peers.is_empty() {
        return None;
    }
    let dark = ui.visuals().dark_mode;
    let ink = ui.visuals().text_color();
    let muted = ui.visuals().weak_text_color();
    let line = if dark {
        Color32::from_rgb(46, 58, 73)
    } else {
        Color32::from_rgb(207, 221, 230)
    };
    let surface = if dark {
        Color32::from_rgb(20, 28, 38)
    } else {
        Color32::from_rgb(247, 250, 252)
    };
    let id = ui.make_persistent_id("workspace-route-motion");
    let now = ui.input(|i| i.time);
    let reduced = ui.ctx().data(|data| {
        data.get_temp::<bool>(egui::Id::new("reduced-motion"))
            .unwrap_or(false)
    });
    let transition = ui.ctx().data_mut(|data| {
        let key = id.with("finite-transition");
        let previous = data.get_temp::<(Scene, f64)>(key);
        let until = match previous {
            Some((previous, until)) if &previous == scene => until,
            _ => now + 0.42,
        };
        data.insert_temp(key, (scene.clone(), until));
        until
    });
    let mut selected = None;
    egui::Frame::new()
        .fill(surface)
        .corner_radius(16)
        .stroke(Stroke::new(1.0, line))
        .inner_margin(if compact { 10 } else { 18 })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(tr("Your workspace"))
                        .size(if compact { 13.0 } else { 17.0 })
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !compact {
                        let online = scene.peers.iter().filter(|p| p.online).count();
                        ui.label(
                            RichText::new(format!("{online} {}", tr("connected")))
                                .small()
                                .color(super::view::contrast_color(GREEN, dark)),
                        );
                    }
                });
            });
            let cols = if compact {
                4
            } else {
                (ui.available_width() / 175.0).floor().clamp(2.0, 4.0) as usize
            };
            let rows = scene.peers.len().div_ceil(cols.min(scene.peers.len()));
            let row_height = if compact { 76.0 } else { 126.0 };
            let phone_layout = PhoneLayout::new(ui.available_width(), scene.phones.len(), compact);
            let height = if compact { 202.0 } else { 340.0 }
                + (rows - 1) as f32 * row_height
                + phone_layout.height;
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
            if !ui.is_rect_visible(rect) {
                return;
            }
            let painter = ui.painter_at(rect);
            let activity = if reduced {
                super::activity::Activity::default()
            } else {
                super::activity::read(ui)
            };
            let active_until = activity.released.iter().chain(&activity.clicked).fold(
                transition
                    .max(activity.moved + 0.85)
                    .max(activity.wheel + 0.6),
                |until, released| until.max(released + 0.6),
            );
            let enabled = !reduced && !ui.input(|i| i.viewport().minimized.unwrap_or(false));
            let feedback = motion::read(
                ui,
                now,
                active_until,
                std::array::from_fn(|i| {
                    (scene.ports[i].source.is_some() || scene.phone_shares(i))
                        && !scene.ports[i].muted
                        && !scene.ports[i].error
                }),
                &activity,
                enabled,
            );
            let (phone_phase, phone_animating) =
                phone_phase(ui, transition, now, enabled && !scene.phones.is_empty());
            let animating = feedback.animating || phone_animating;
            if animating {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(33));
            }
            let phase = animating.then_some(Phase {
                time: now,
                opacity: feedback.opacity,
            });
            let mouse_wave = mouse_wave(ui, &activity, now);
            let device_y = rect.bottom() - if compact { 33.0 } else { 69.0 };
            let slot_width = rect.width() / 5.0;
            let devices: Vec<_> = (0..5)
                .map(|i| egui::pos2(rect.left() + slot_width * (i as f32 + 0.5), device_y))
                .collect();
            let mut peer_rects = Vec::new();
            for (row, peers) in scene.peers.chunks(cols).enumerate() {
                let width = rect.width() / peers.len() as f32;
                for (col, _) in peers.iter().enumerate() {
                    let center = egui::pos2(
                        rect.left() + width * (col as f32 + 0.5),
                        rect.top() + row as f32 * row_height + row_height * 0.47,
                    );
                    peer_rects.push(Rect::from_center_size(
                        center,
                        egui::vec2(
                            (width - 10.0).min(if compact { 95.0 } else { 205.0 }),
                            row_height - 12.0,
                        ),
                    ));
                }
            }
            let phone_rects = phone_layout.bounds(rect, rows as f32 * row_height, compact);
            let split = rect.bottom() - if compact { 68.0 } else { 132.0 };
            painter.line_segment(
                [
                    egui::pos2(rect.left(), split),
                    egui::pos2(rect.right(), split),
                ],
                Stroke::new(1.0, line.gamma_multiply(0.6)),
            );
            // Source -> shared device, then shared device -> receiving computer.
            // A muted or unavailable route stays visible without suggesting activity.
            for (index, port) in scene.ports.iter().enumerate() {
                let Some(source) = scene
                    .peers
                    .iter()
                    .position(|p| port.source.as_ref() == Some(&p.key) && p.available)
                else {
                    continue;
                };
                let color = super::view::contrast_color(scene.peers[source].color, dark);
                let active_phase = phase
                    .map(|phase| Phase {
                        opacity: feedback.ports[index],
                        ..phase
                    })
                    .filter(|phase| phase.opacity > 0.0);
                let device = devices[index] - egui::vec2(0.0, if compact { 17.0 } else { 34.0 });
                let start = peer_rects[source].center_bottom()
                    + egui::vec2((index as f32 - 2.0) * if compact { 3.0 } else { 6.0 }, 0.0);
                cable(
                    &painter,
                    [start, device - egui::vec2(3.0, 0.0)],
                    color,
                    active_phase,
                    false,
                    compact,
                    index,
                );
                for destination in &port.destinations {
                    let Some(target) = scene
                        .peers
                        .iter()
                        .position(|p| p.key == *destination && p.available)
                    else {
                        continue;
                    };
                    let end = peer_rects[target].center_bottom()
                        + egui::vec2((index as f32 - 2.0) * if compact { 3.0 } else { 6.0 }, 0.0);
                    cable(
                        &painter,
                        [device + egui::vec2(3.0, 0.0), end],
                        color.gamma_multiply(0.65),
                        active_phase,
                        true,
                        compact,
                        index,
                    );
                }
            }
            // Phones supply input independently of the physical keyboard host.
            // Reuse the shared input hubs, then follow the selected PC; local
            // remotes have one direct route to their owning computer instead.
            let phone_color = super::view::contrast_color(CYAN, dark);
            let mut phone_destinations = std::collections::BTreeSet::new();
            for (phone, bounds) in scene.phones.iter().zip(&phone_rects) {
                if !phone.connected || phone.shared && scene.pending {
                    continue;
                }
                let Some(target) = phone.target.as_ref().and_then(|target| {
                    scene
                        .peers
                        .iter()
                        .position(|peer| &peer.key == target && peer.available)
                }) else {
                    continue;
                };
                if !phone.shared {
                    cable(
                        &painter,
                        [bounds.center_top(), peer_rects[target].center_bottom()],
                        phone_color.gamma_multiply(0.65),
                        phone_phase,
                        true,
                        compact,
                        target,
                    );
                    continue;
                }
                for index in [2, 3] {
                    if index == 2 && !phone.keyboard || index == 3 && !phone.mouse {
                        continue;
                    }
                    let hub = devices[index] - egui::vec2(0.0, if compact { 17.0 } else { 34.0 });
                    cable(
                        &painter,
                        [bounds.center_bottom(), hub - egui::vec2(3.0, 0.0)],
                        phone_color.gamma_multiply(0.72),
                        phone_phase,
                        false,
                        compact,
                        index,
                    );
                    let target_key = &scene.peers[target].key;
                    if !scene.ports[index].destinations.contains(target_key)
                        && phone_destinations.insert((target_key.clone(), index))
                    {
                        let end = peer_rects[target].center_bottom()
                            + egui::vec2(
                                (index as f32 - 2.0) * if compact { 3.0 } else { 6.0 },
                                0.0,
                            );
                        cable(
                            &painter,
                            [hub + egui::vec2(3.0, 0.0), end],
                            phone_color.gamma_multiply(0.45),
                            phone_phase,
                            true,
                            compact,
                            index,
                        );
                    }
                }
            }
            for (index, peer) in scene.peers.iter().enumerate() {
                let bounds = peer_rects[index];
                let response = ui.interact(bounds, id.with(&peer.key), Sense::click());
                let actionable = can_switch && peer.available && !scene.pending;
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        actionable,
                        format!("{} · {}", tr("Switch here"), peer.name),
                    )
                });
                if response.clicked() && actionable {
                    selected = Some(peer.key.clone());
                }
                let hovered = response.hovered() && actionable;
                let peer_color = super::view::contrast_color(peer.color, dark);
                let outline = if peer.active || hovered {
                    peer_color
                } else {
                    line
                };
                painter.rect_filled(
                    bounds,
                    12,
                    if peer.active {
                        peer_color.gamma_multiply(if dark { 0.09 } else { 0.06 })
                    } else {
                        surface
                    },
                );
                painter.rect_stroke(
                    bounds,
                    12,
                    Stroke::new(if peer.active { 1.5 } else { 1.0 }, outline),
                    egui::StrokeKind::Inside,
                );
                let name_rect = Rect::from_min_size(
                    bounds.min + egui::vec2(6.0, 7.0),
                    egui::vec2(bounds.width() - 12.0, 20.0),
                );
                fitted_text(
                    &painter,
                    name_rect,
                    &peer.name,
                    if compact { 11.0 } else { 15.0 },
                    if peer.available { ink } else { muted },
                );
                let color = if peer.available {
                    peer_color
                } else {
                    muted.gamma_multiply(0.5)
                };
                computer(
                    &painter,
                    bounds.center() + egui::vec2(0.0, if compact { 3.0 } else { 1.0 }),
                    &peer.platform,
                    if compact { 0.48 } else { 1.0 },
                    [color, surface],
                    peer.host,
                    if peer.available { phase } else { None },
                );
                let status = if !peer.online {
                    tr("Offline")
                } else if !peer.available {
                    tr("Unavailable")
                } else if peer.host {
                    tr("Device host")
                } else if peer.active {
                    tr(if compact {
                        "Input target"
                    } else {
                        "In control"
                    })
                } else {
                    tr("Connected")
                };
                fitted_text(
                    &painter,
                    Rect::from_min_size(
                        egui::pos2(
                            bounds.left() + 4.0,
                            bounds.bottom() - if compact { 17.0 } else { 27.0 },
                        ),
                        egui::vec2(bounds.width() - 8.0, 20.0),
                    ),
                    &status,
                    if compact { 9.0 } else { 11.0 },
                    if peer.active || peer.host {
                        color
                    } else {
                        muted
                    },
                );
                if peer.local && !compact {
                    painter.circle_filled(bounds.right_top() + egui::vec2(-10.0, 10.0), 2.5, GREEN);
                }
                if response.has_focus() {
                    painter.rect_stroke(
                        bounds.expand(3.0),
                        14,
                        Stroke::new(2.0, CYAN),
                        egui::StrokeKind::Inside,
                    );
                }
                response
                    .on_hover_cursor(if actionable {
                        egui::CursorIcon::PointingHand
                    } else {
                        egui::CursorIcon::Default
                    })
                    .on_hover_text(format!(
                        "{} · {}{}",
                        peer.name,
                        status,
                        if peer.local {
                            format!(" · {}", tr("This PC"))
                        } else {
                            String::new()
                        }
                    ));
            }
            for (phone, bounds) in scene.phones.iter().zip(&phone_rects) {
                let available = phone.connected && phone.target.is_some();
                let color = if available {
                    phone_color
                } else {
                    muted.gamma_multiply(0.65)
                };
                painter.rect_filled(*bounds, if compact { 9 } else { 11 }, surface);
                painter.rect_filled(
                    *bounds,
                    if compact { 9 } else { 11 },
                    color.gamma_multiply(if available { 0.055 } else { 0.025 }),
                );
                painter.rect_stroke(
                    *bounds,
                    if compact { 9 } else { 11 },
                    Stroke::new(
                        1.0,
                        color.gamma_multiply(if available { 0.42 } else { 0.2 }),
                    ),
                    egui::StrokeKind::Inside,
                );
                let icon = Rect::from_center_size(
                    egui::pos2(
                        bounds.left() + if compact { 17.0 } else { 24.0 },
                        bounds.center().y,
                    ),
                    if compact {
                        egui::vec2(14.0, 26.0)
                    } else {
                        egui::vec2(19.0, 36.0)
                    },
                );
                phone_glyph(&painter, icon, color, surface);
                let text = bounds.shrink2(egui::vec2(if compact { 5.0 } else { 8.0 }, 4.0));
                let text = Rect::from_min_max(egui::pos2(icon.right() + 5.0, text.top()), text.max);
                fitted_text(
                    &painter,
                    Rect::from_min_size(text.min, egui::vec2(text.width(), text.height() * 0.5)),
                    &phone.name,
                    if compact { 10.5 } else { 13.0 },
                    if available { ink } else { muted },
                );
                let target_name = phone
                    .target
                    .as_ref()
                    .and_then(|target| scene.peers.iter().find(|peer| &peer.key == target))
                    .map(|peer| peer.name.as_str());
                let status = if !phone.connected {
                    tr("Offline")
                } else if phone.shared && scene.pending {
                    tr("Switching…")
                } else if let Some(target) = target_name {
                    format!(
                        "{} · {target}",
                        tr(if phone.shared { "Shared" } else { "Local" })
                    )
                } else {
                    tr("Unavailable")
                };
                fitted_text(
                    &painter,
                    Rect::from_min_size(
                        egui::pos2(text.left(), text.center().y),
                        egui::vec2(text.width(), text.height() * 0.5),
                    ),
                    &status,
                    if compact { 9.0 } else { 10.5 },
                    muted,
                );
                let owner = scene
                    .peers
                    .iter()
                    .find(|peer| peer.key == phone.owner)
                    .map(|peer| peer.name.as_str())
                    .unwrap_or("");
                let response = ui.interact(
                    *bounds,
                    id.with(("phone-source", &phone.owner, &phone.key)),
                    Sense::hover(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Label,
                        available,
                        format!(
                            "{} · {} · {owner} · {status}",
                            phone.name,
                            tr("Phone remote")
                        ),
                    )
                });
                response.on_hover_text(format!(
                    "{} · {}\n{} · {owner}\n{status}",
                    phone.name,
                    tr("Phone remote"),
                    tr("Connected through")
                ));
            }
            for (index, port) in scene.ports.iter().enumerate() {
                let center = devices[index];
                let color = port
                    .source
                    .as_ref()
                    .and_then(|key| scene.peers.iter().find(|p| &p.key == key))
                    .map(|p| super::view::contrast_color(p.color, dark))
                    .unwrap_or_else(|| {
                        if scene.phone_shares(index) {
                            phone_color
                        } else {
                            muted.gamma_multiply(0.5)
                        }
                    });
                let color = if port.muted || port.error {
                    muted
                } else {
                    color
                };
                let response = ui.interact(
                    Rect::from_center_size(
                        center,
                        egui::vec2(slot_width - 5.0, if compact { 65.0 } else { 125.0 }),
                    ),
                    id.with(index),
                    Sense::hover(),
                );
                if index == 1 {
                    display(
                        &painter,
                        center - egui::vec2(0.0, if compact { 10.0 } else { 21.0 }),
                        if compact {
                            0.46
                        } else {
                            (slot_width / 158.0).min(1.15)
                        },
                        color,
                        surface,
                        &port.caption,
                        phase,
                    );
                } else {
                    let icon = match index {
                        0 => DeviceIcon::Headphones,
                        2 => DeviceIcon::Keyboard,
                        3 => DeviceIcon::Mouse,
                        _ => DeviceIcon::Microphone,
                    };
                    let scale = if compact { 0.95 } else { 1.85 };
                    let icon_center = center - egui::vec2(0.0, if compact { 3.0 } else { 10.0 });
                    if index == 2 {
                        keyboard(
                            &painter,
                            center - egui::vec2(0.0, 8.0),
                            if compact { 0.48 } else { 1.18 },
                            [color, surface],
                            &feedback.keys,
                            phase,
                        );
                    } else {
                        paint_device(&painter, icon, icon_center, scale, color);
                    }
                    if index == 3 {
                        mouse_motion(
                            &painter,
                            icon_center,
                            scale,
                            color,
                            &mouse_wave,
                            &feedback,
                            phase,
                        );
                    }
                    if matches!(index, 0 | 4)
                        && feedback.ports[index] > 0.0
                        && let Some(phase) = phase
                    {
                        waveform(
                            &painter,
                            icon_center,
                            scale,
                            color.gamma_multiply(feedback.ports[index]),
                            phase.time.rem_euclid(std::f64::consts::TAU / 8.0) as f32,
                            index,
                        );
                    }
                }
                if port.muted || port.error {
                    painter.line_segment(
                        [
                            center + egui::vec2(-14.0, 12.0),
                            center + egui::vec2(14.0, -16.0),
                        ],
                        Stroke::new(1.5, color),
                    );
                }
                let label =
                    tr(["Sound", "Shared monitor", "Keyboard", "Mouse", "Microphone"][index]);
                let label_y = center.y + if compact { 21.0 } else { 43.0 };
                fitted_text(
                    &painter,
                    Rect::from_center_size(
                        egui::pos2(center.x, label_y),
                        egui::vec2(slot_width - 4.0, 20.0),
                    ),
                    &label,
                    if compact { 9.0 } else { 12.0 },
                    ink,
                );
                if !compact {
                    fitted_text(
                        &painter,
                        Rect::from_center_size(
                            egui::pos2(center.x, label_y + 19.0),
                            egui::vec2(slot_width - 8.0, 18.0),
                        ),
                        &if port.source.is_some() {
                            port.caption.clone()
                        } else if scene.phone_shares(index) {
                            tr("Phone remotes")
                        } else {
                            tr("Not shared")
                        },
                        10.0,
                        muted,
                    );
                }
                response.on_hover_text(format!(
                    "{} · {}",
                    label,
                    if port.source.is_some() {
                        port.caption.clone()
                    } else if scene.phone_shares(index) {
                        tr("Phone remotes")
                    } else {
                        tr("Not shared")
                    }
                ));
            }
            if !compact {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(tr("Device host / shared devices / receiving computers"))
                            .size(11.0)
                            .color(muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(tr("Click a computer to switch"))
                                .size(11.0)
                                .color(super::view::contrast_color(CYAN, dark)),
                        );
                    });
                });
            }
        });
    selected
}

struct PhoneLayout {
    count: usize,
    columns: usize,
    card_height: f32,
    row_height: f32,
    height: f32,
}
impl PhoneLayout {
    fn new(width: f32, count: usize, compact: bool) -> Self {
        let columns = (width / if compact { 155.0 } else { 235.0 })
            .floor()
            .clamp(1.0, 4.0) as usize;
        let columns = columns.min(count.max(1));
        let card_height = if compact { 44.0 } else { 62.0 };
        let row_height = card_height + 8.0;
        let height = if count == 0 {
            0.0
        } else {
            count.div_ceil(columns) as f32 * row_height + if compact { 12.0 } else { 20.0 }
        };
        Self {
            count,
            columns,
            card_height,
            row_height,
            height,
        }
    }
    fn bounds(&self, rect: Rect, peer_height: f32, compact: bool) -> Vec<Rect> {
        (0..self.count)
            .map(|index| {
                let row = index / self.columns;
                let column = index % self.columns;
                let row_count = self.columns.min(self.count - row * self.columns);
                let width = rect.width() / row_count as f32;
                Rect::from_center_size(
                    egui::pos2(
                        rect.left() + width * (column as f32 + 0.5),
                        rect.top()
                            + peer_height
                            + if compact { 4.0 } else { 8.0 }
                            + self.row_height * row as f32
                            + self.card_height * 0.5,
                    ),
                    egui::vec2(
                        (width - 8.0).min(if compact { 190.0 } else { 280.0 }),
                        self.card_height,
                    ),
                )
            })
            .collect()
    }
}

pub(super) fn phone_glyph(painter: &egui::Painter, bounds: Rect, color: Color32, surface: Color32) {
    painter.rect_filled(bounds, 4, surface);
    painter.rect_filled(bounds.shrink(2.5), 2, color.gamma_multiply(0.07));
    painter.rect_stroke(
        bounds,
        4,
        Stroke::new(1.35, color),
        egui::StrokeKind::Inside,
    );
    let middle = bounds.center().x;
    painter.line_segment(
        [
            egui::pos2(middle - 2.0, bounds.top() + 3.5),
            egui::pos2(middle + 2.0, bounds.top() + 3.5),
        ],
        Stroke::new(1.2, color.gamma_multiply(0.65)),
    );
    painter.line_segment(
        [
            egui::pos2(middle - 2.5, bounds.bottom() - 3.5),
            egui::pos2(middle + 2.5, bounds.bottom() - 3.5),
        ],
        Stroke::new(1.3, color),
    );
}

fn phone_phase(ui: &egui::Ui, until: f64, now: f64, enabled: bool) -> (Option<Phase>, bool) {
    let id = ui
        .make_persistent_id("phone-source-transition")
        .with(ui.ctx().viewport_id());
    ui.ctx().data_mut(|data| {
        let state = data.get_temp_mut_or_default::<(Fade, f64)>(id);
        if !enabled || now - state.1 > 1.2 {
            state.0 = Fade::default();
        }
        state.1 = now;
        let opacity = state.0.update_until(if enabled { until } else { now }, now);
        let animating = enabled && (state.0.pending() || opacity > 0.0);
        (animating.then_some(Phase { time: now, opacity }), animating)
    })
}

fn fitted_text(painter: &egui::Painter, rect: Rect, text: &str, size: f32, color: Color32) {
    let font = FontId::proportional(size);
    let mut label = text.to_owned();
    let mut galley = painter.layout_no_wrap(label.clone(), font.clone(), color);
    while galley.size().x > rect.width() && !label.is_empty() {
        label.pop();
        galley = painter.layout_no_wrap(format!("{label}…"), font.clone(), color);
    }
    painter.with_clip_rect(rect).galley(
        egui::pos2(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        color,
    );
}

fn point(points: [Pos2; 4], t: f32) -> Pos2 {
    let a = 1.0 - t;
    (points[0].to_vec2() * a.powi(3)
        + points[1].to_vec2() * (3.0 * a * a * t)
        + points[2].to_vec2() * (3.0 * a * t * t)
        + points[3].to_vec2() * t.powi(3))
    .to_pos2()
}

fn cable(
    painter: &egui::Painter,
    endpoints: [Pos2; 2],
    color: Color32,
    phase: Option<Phase>,
    receiving: bool,
    compact: bool,
    index: usize,
) {
    let [start, end] = endpoints;
    let bend = (end.y - start.y).abs() * 0.52;
    let sign = if end.y >= start.y { 1.0 } else { -1.0 };
    let points = [
        start,
        start + egui::vec2(0.0, bend * sign),
        end - egui::vec2(0.0, bend * sign),
        end,
    ];
    let path: Vec<_> = (0..=32).map(|i| point(points, i as f32 / 32.0)).collect();
    painter.add(egui::Shape::line(
        path.clone(),
        Stroke::new(if compact { 2.8 } else { 5.0 }, color.gamma_multiply(0.05)),
    ));
    if receiving {
        for segment in path
            .windows(2)
            .enumerate()
            .filter(|(i, _)| i % 3 != 0)
            .map(|(_, s)| s)
        {
            painter.line_segment(
                [segment[0], segment[1]],
                Stroke::new(if compact { 0.7 } else { 1.1 }, color.gamma_multiply(0.5)),
            );
        }
    } else {
        painter.add(egui::Shape::line(
            path,
            Stroke::new(if compact { 1.0 } else { 1.5 }, color.gamma_multiply(0.7)),
        ));
    }
    let arrow_t = if receiving { 0.64 } else { 0.61 };
    arrow(
        painter,
        point(points, arrow_t),
        point(points, arrow_t + 0.035) - point(points, arrow_t),
        color,
        if compact { 2.4 } else { 3.6 },
    );
    for endpoint in [start, end] {
        painter.circle_filled(endpoint, if compact { 1.7 } else { 2.5 }, color);
    }
    if let Some(phase) = phase {
        // Both segments carry a slow continuous stream. The receiving segment
        // follows the source segment in the same host -> device -> peer direction.
        let t = (phase.time / 3.8 - index as f64 * 0.06 - if receiving { 0.5 } else { 0.0 })
            .rem_euclid(1.0) as f32;
        let head = point(points, t);
        let trail_start = (t - 0.10).max(0.0);
        let trail = (0..=8)
            .map(|i| point(points, trail_start + (t - trail_start) * i as f32 / 8.0))
            .collect();
        if t > 0.0001 {
            painter.add(egui::Shape::line(
                trail,
                Stroke::new(
                    if compact { 1.8 } else { 2.5 },
                    color.gamma_multiply(phase.opacity),
                ),
            ));
        }
        arrow(
            painter,
            head,
            point(points, (t + 0.015).min(1.0)) - point(points, (t - 0.015).max(0.0)),
            if painter.ctx().theme() == egui::Theme::Dark {
                Color32::WHITE.gamma_multiply(phase.opacity)
            } else {
                color.gamma_multiply(phase.opacity)
            },
            if compact { 2.2 } else { 3.4 },
        );
    }
}

fn arrow(p: &egui::Painter, tip: Pos2, direction: Vec2, color: Color32, size: f32) {
    if direction.length_sq() < 0.00001 {
        return;
    }
    let d = direction.normalized();
    let n = egui::vec2(-d.y, d.x);
    p.add(egui::Shape::convex_polygon(
        vec![
            tip + d * size,
            tip - d * size + n * size * 0.65,
            tip - d * size - n * size * 0.65,
        ],
        color,
        Stroke::NONE,
    ));
}

fn computer(
    p: &egui::Painter,
    center: Pos2,
    platform: &str,
    scale: f32,
    colors: [Color32; 2],
    host: bool,
    phase: Option<Phase>,
) {
    let [color, bg] = colors;
    let pt = |x, y| center + egui::vec2(x, y) * scale;
    let stroke = Stroke::new(1.6 * scale, color);
    if platform == "macos" || platform == "linux" && !host {
        let screen = Rect::from_center_size(pt(0.0, -2.0), egui::vec2(66.0, 42.0) * scale);
        p.rect_filled(screen, 5, bg);
        p.rect_filled(screen.shrink(4.0 * scale), 2, color.gamma_multiply(0.09));
        p.rect_stroke(screen, 5, stroke, egui::StrokeKind::Inside);
        p.line_segment(
            [pt(-39.0, 23.0), pt(39.0, 23.0)],
            Stroke::new(3.0 * scale, color),
        );
        p.line_segment(
            [pt(-9.0, 24.0), pt(9.0, 24.0)],
            Stroke::new(2.0 * scale, bg),
        );
        if let Some(phase) = phase {
            let x = -23.0 + (phase.time / 4.0).fract() as f32 * 46.0;
            p.line_segment(
                [pt(x - 4.0, 11.0), pt(x + 4.0, 11.0)],
                Stroke::new(1.8 * scale, color.gamma_multiply(phase.opacity)),
            );
        }
    } else {
        let tower = Rect::from_center_size(center, egui::vec2(40.0, 57.0) * scale);
        p.rect_filled(tower.expand(4.0 * scale), 9, color.gamma_multiply(0.04));
        p.rect_filled(tower, 7, bg);
        p.rect_stroke(tower, 7, stroke, egui::StrokeKind::Inside);
        for y in [-17.0, -11.0, 16.0, 21.0] {
            p.line_segment(
                [pt(-10.0, y), pt(10.0, y)],
                Stroke::new(scale, color.gamma_multiply(0.45)),
            );
        }
        p.circle_filled(pt(0.0, 1.0), 2.7 * scale, color);
        if let Some(phase) = phase {
            p.circle_stroke(
                pt(0.0, 1.0),
                (5.3 + (phase.time * 1.8).sin() as f32 * 0.9) * scale,
                Stroke::new(scale, color.gamma_multiply(0.5 * phase.opacity)),
            );
        }
    }
}

fn display(
    p: &egui::Painter,
    center: Pos2,
    scale: f32,
    color: Color32,
    bg: Color32,
    caption: &str,
    phase: Option<Phase>,
) {
    let screen = Rect::from_center_size(center, egui::vec2(122.0, 72.0) * scale);
    p.rect_filled(screen.expand(4.0 * scale), 9, color.gamma_multiply(0.05));
    p.rect_filled(screen, 7, bg);
    p.rect_stroke(
        screen,
        7,
        Stroke::new(1.5 * scale, color),
        egui::StrokeKind::Inside,
    );
    let inner = screen.shrink(5.0 * scale);
    p.rect_filled(inner, 3, Color32::from_rgb(13, 53, 75));
    let shift = phase
        .map(|p| (p.time * 2.0).sin() as f32 * 3.0 * scale * p.opacity)
        .unwrap_or(0.0);
    for (offset, tint) in [
        (0.0, Color32::from_rgb(17, 77, 105)),
        (16.0, Color32::from_rgb(23, 105, 131)),
    ] {
        let points: Vec<_> = (0..=24)
            .map(|i| {
                let x = i as f32 / 24.0;
                egui::pos2(
                    inner.left() + inner.width() * x,
                    inner.top()
                        + inner.height() * 0.44
                        + (x * 4.0 + offset * 0.1).sin() * 10.0 * scale
                        + offset * scale
                        + shift,
                )
            })
            .collect();
        let mut mesh = egui::Mesh::default();
        for point in &points {
            mesh.colored_vertex(*point, tint);
            mesh.colored_vertex(egui::pos2(point.x, inner.bottom()), tint);
        }
        for i in 0..24_u32 {
            let base = i * 2;
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base + 1, base + 3, base + 2);
        }
        p.with_clip_rect(inner).add(mesh);
    }
    if scale > 0.6 {
        fitted_text(
            p,
            inner,
            caption,
            12.0 * scale,
            Color32::from_rgb(225, 244, 252),
        );
    }
    p.line_segment(
        [
            center + egui::vec2(0.0, 36.0) * scale,
            center + egui::vec2(0.0, 44.0) * scale,
        ],
        Stroke::new(3.0 * scale, color),
    );
    p.line_segment(
        [
            center + egui::vec2(-19.0, 45.0) * scale,
            center + egui::vec2(19.0, 45.0) * scale,
        ],
        Stroke::new(3.0 * scale, color),
    );
}

fn waveform(p: &egui::Painter, center: Pos2, scale: f32, color: Color32, time: f32, index: usize) {
    if index == 4 {
        let base = center - egui::vec2(0.0, 3.0 * scale);
        let interior = Rect::from_center_size(base, egui::vec2(4.0, 12.0) * scale);
        let painter = p.with_clip_rect(interior);
        for i in -2_i32..=2 {
            let width = (1.0 + (time * 8.0 + i as f32 * 1.3).sin().abs() * 1.8) * scale;
            let y = base.y + i as f32 * 2.2 * scale;
            painter.line_segment(
                [
                    egui::pos2(base.x - width * 0.5, y),
                    egui::pos2(base.x + width * 0.5, y),
                ],
                Stroke::new((0.6 * scale).max(0.7), color),
            );
        }
        return;
    }
    let base = center + egui::vec2(0.0, 2.0) * scale;
    for i in -2_i32..=2 {
        let height = (2.0 + (time * 8.0 + i as f32 * 1.3).sin().abs() * 4.0) * scale;
        let x = base.x + i as f32 * 2.5 * scale;
        p.line_segment(
            [
                egui::pos2(x, base.y - height * 0.5),
                egui::pos2(x, base.y + height * 0.5),
            ],
            Stroke::new(scale, color),
        );
    }
}

fn keyboard(
    p: &egui::Painter,
    center: Pos2,
    scale: f32,
    colors: [Color32; 2],
    keys: &[f32; 66],
    phase: Option<Phase>,
) {
    let [color, bg] = colors;
    let bounds = Rect::from_center_size(center, egui::vec2(120.0, 48.0) * scale);
    p.rect_filled(bounds, 5, bg);
    p.rect_stroke(
        bounds,
        5,
        Stroke::new(1.4 * scale, color),
        egui::StrokeKind::Inside,
    );
    let sweep = phase.map(|phase| ((phase.time / 5.0).fract() as f32 * 14.0, phase.opacity));
    let mut bottom_x = -55.0;
    for (index, &lit) in keys.iter().enumerate() {
        let (x, y, width) = if index < 56 {
            (
                -55.0 + (index % 14) as f32 * 8.0,
                -18.0 + (index / 14) as f32 * 8.0,
                6.0,
            )
        } else {
            let width = if index == 59 { 32.0 } else { 6.0 };
            let x = bottom_x;
            bottom_x += width + 2.0;
            (x, 14.0, width)
        };
        let glow = sweep
            .map(|(s, opacity)| {
                let distance = (s - (index % 14) as f32 + 7.0).rem_euclid(14.0) - 7.0;
                (1.0 - distance.abs() / 1.2).clamp(0.0, 1.0) * opacity
            })
            .unwrap_or(0.0);
        let rect = Rect::from_min_size(
            center + egui::vec2(x, y) * scale,
            egui::vec2(width, 5.0) * scale,
        );
        if lit > 0.0 {
            p.rect_filled(rect.expand(scale), 1, color.gamma_multiply(0.22 * lit));
        }
        p.rect_filled(rect, 1, color.gamma_multiply(0.28 + 0.52 * glow));
        if lit > 0.0 {
            p.rect_filled(
                rect,
                1,
                Color32::from_rgb(219, 249, 255).gamma_multiply(lit),
            );
        }
    }
}

fn mouse_motion(
    p: &egui::Painter,
    center: Pos2,
    scale: f32,
    color: Color32,
    wave: &MouseWave,
    feedback: &motion::Frame,
    phase: Option<Phase>,
) {
    for (index, x) in [-3.5, 3.5].into_iter().enumerate() {
        if feedback.buttons[index] > 0.0 {
            p.rect_filled(
                Rect::from_center_size(
                    center + egui::vec2(x, -6.0) * scale,
                    egui::vec2(4.0, 6.0) * scale,
                ),
                2,
                Color32::WHITE.gamma_multiply(feedback.buttons[index]),
            );
        }
    }
    let Some(phase) = phase else {
        return;
    };
    let wheel =
        center + egui::vec2(0.0, -7.0 + (phase.time * 1.5).sin() as f32 * phase.opacity) * scale;
    let wheel_color = Color32::from(
        egui::Rgba::from(color) * (1.0 - feedback.wheel) + egui::Rgba::WHITE * feedback.wheel,
    )
    .gamma_multiply(phase.opacity);
    p.line_segment(
        [
            wheel - egui::vec2(0.0, 1.7 * scale),
            wheel + egui::vec2(0.0, 1.7 * scale),
        ],
        Stroke::new(1.8 * scale, wheel_color),
    );
    if wave.opacity > 0.0 {
        for index in 0..3 {
            let progress = (phase.time * 0.7 + index as f64 / 3.0).fract() as f32;
            let distance = 16.0 + progress * 15.0;
            let opacity =
                wave.opacity * phase.opacity * (std::f32::consts::PI * progress).sin().powi(2);
            let points = (0..=12)
                .map(|i| {
                    let a = wave.angle - 0.55 + i as f32 / 12.0 * 1.1;
                    center + egui::vec2(a.cos(), a.sin()) * distance * scale
                })
                .collect();
            p.add(egui::Shape::line(
                points,
                Stroke::new(scale, color.gamma_multiply(0.85 * opacity)),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_phone(state: &mut Workspace, owner: &str, status: &str, key: &str) {
        let member = state.members.get_mut(owner).unwrap();
        let device = crate::audio::AudioDevice {
            id: format!("desk-switch-phone{status}:{key}"),
            name: "Living room remote".into(),
            default: false,
        };
        member.node.devices.keyboards.push(device.clone());
        member.node.devices.mice.push(device);
    }

    #[test]
    fn phones_are_non_voting_input_sources_with_shared_local_and_offline_routes() {
        let mut state = crate::group::model::tests::fixture(4);
        let key = |n: u8| format!("{n:064x}");
        state.active_input = Some(key(4));
        state
            .members
            .get_mut(&key(4))
            .unwrap()
            .node
            .diagnostics
            .logs
            .push(PHONE_RELAY_CAPABILITY.into());
        state.settings.keyboard_enabled = false;
        state.settings.mouse_enabled = false;
        add_phone(&mut state, &key(1), "", &key(11));
        add_phone(&mut state, &key(1), "-local", &key(12));
        add_phone(&mut state, &key(2), "-offline", &key(13));
        let scene = Scene::from_workspace(&state, &key(1));
        assert_eq!(scene.peers.len(), 4);
        assert_eq!(scene.phones.len(), 3);
        let shared = scene
            .phones
            .iter()
            .find(|phone| phone.key == key(11))
            .unwrap();
        assert!(shared.connected && shared.shared && shared.keyboard && shared.mouse);
        assert_eq!(shared.target, Some(key(4)));
        let local = scene
            .phones
            .iter()
            .find(|phone| phone.key == key(12))
            .unwrap();
        assert!(local.connected && !local.shared);
        assert_eq!(local.target, Some(key(1)));
        let offline = scene
            .phones
            .iter()
            .find(|phone| phone.key == key(13))
            .unwrap();
        assert!(!offline.connected && offline.target.is_none());
        assert!(scene.phone_shares(2) && scene.phone_shares(3));
        assert!(scene.ports[2].source.is_none() && scene.ports[3].source.is_none());
        state.members.get_mut(&key(4)).unwrap().online = false;
        let unavailable = Scene::from_workspace(&state, &key(1));
        assert!(
            unavailable
                .phones
                .iter()
                .find(|phone| phone.key == key(11))
                .unwrap()
                .target
                .is_none()
        );
        assert!(!unavailable.phone_shares(2));
        assert!(phone_id("desk-switch-phone:invalid-key").is_none());
    }

    #[test]
    fn multiple_phone_sources_fit_without_overlapping_pc_cards_or_each_other() {
        for compact in [false, true] {
            for width in [280.0, 386.0, 700.0, 1000.0] {
                for count in [0, 1, 2, 5, 16] {
                    let layout = PhoneLayout::new(width, count, compact);
                    let peer_height = if compact { 76.0 } else { 126.0 };
                    let rect = Rect::from_min_size(
                        Pos2::ZERO,
                        egui::vec2(width, peer_height + layout.height + 150.0),
                    );
                    let bounds = layout.bounds(rect, peer_height, compact);
                    assert_eq!(bounds.len(), count);
                    for (index, bounds) in bounds.iter().enumerate() {
                        assert!(rect.contains_rect(*bounds));
                        assert!(bounds.top() >= peer_height);
                        assert!(bounds.bottom() < peer_height + layout.height);
                        for other in &layout.bounds(rect, peer_height, compact)[..index] {
                            assert!(!bounds.intersects(*other));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn shared_phone_routes_follow_target_changes_and_do_not_keep_an_idle_animation_clock() {
        let mut state = crate::group::model::tests::fixture(2);
        let key = |n: u8| format!("{n:064x}");
        add_phone(&mut state, &key(1), "", &key(11));
        state.active_input = Some(key(2));
        state
            .members
            .get_mut(&key(2))
            .unwrap()
            .node
            .diagnostics
            .logs
            .push(PHONE_RELAY_CAPABILITY.into());
        let scene = Scene::from_workspace(&state, &key(1));
        assert_eq!(scene.phones[0].target, Some(key(2)));
        let ctx = egui::Context::default();
        assert!(settled_repaint_delay(&ctx, &scene, 0.0, false) < std::time::Duration::MAX);
        assert_eq!(
            settled_repaint_delay(&ctx, &scene, 2.0, false),
            std::time::Duration::MAX
        );
        state.active_input = Some(key(1));
        let scene = Scene::from_workspace(&state, &key(1));
        assert_eq!(scene.phones[0].target, Some(key(1)));
        assert!(settled_repaint_delay(&ctx, &scene, 2.1, false) < std::time::Duration::MAX);
        assert_eq!(
            settled_repaint_delay(&ctx, &scene, 4.0, false),
            std::time::Duration::MAX
        );
        assert_eq!(
            settled_repaint_delay(&ctx, &scene, 5.0, true),
            std::time::Duration::MAX
        );
    }

    #[test]
    fn mouse_waves_turn_across_the_short_arc_and_fade_without_visibility_steps() {
        let initial = 179.0_f32.to_radians();
        let target = Vec2::angled((-179.0_f32).to_radians());
        let mut wave = MouseWave {
            angle: initial,
            opacity: 0.0,
            updated: 0.0,
            fade: Fade::default(),
        };
        wave.update(target, f64::INFINITY, 0.016);
        assert!(wave.angle > initial && wave.angle < initial + 2.0_f32.to_radians());
        assert_eq!(wave.opacity, 0.0);
        wave.update(target, f64::INFINITY, 0.032);
        assert!(wave.opacity > 0.0 && wave.opacity < 1.0);
        for frame in 2..=100 {
            wave.update(target, f64::INFINITY, frame as f64 * 0.016);
        }
        assert!(wave.opacity > 0.99);
        let before = wave.opacity;
        wave.update(target, 1.616, 1.616);
        assert_eq!(wave.opacity, before);
        wave.update(target, 1.616, 1.632);
        assert!(wave.opacity > 0.0 && wave.opacity < before);
        for frame in 102..=220 {
            wave.update(target, 1.616, frame as f64 * 0.016);
        }
        assert_eq!(wave.opacity, 0.0);
    }

    #[test]
    fn microphone_bars_stack_vertically_with_clearance_inside_both_capsule_sizes() {
        let ctx = egui::Context::default();
        let center = egui::pos2(50.0, 50.0);
        for scale in [0.95, 1.85] {
            let interior = Rect::from_center_size(
                center - egui::vec2(0.0, 3.0 * scale),
                egui::vec2(9.0, 17.0) * scale,
            )
            .shrink(2.5 * scale);
            for time in [0.0, 0.5, 1.0, 2.7] {
                let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    waveform(ui.painter(), center, scale, CYAN, time, 4);
                });
                let mut bars = 0;
                let mut previous_y = f32::NEG_INFINITY;
                for shape in output.shapes {
                    if let egui::Shape::LineSegment { points, stroke } = shape.shape {
                        bars += 1;
                        assert_eq!(points[0].y, points[1].y);
                        assert!(points[0].y > previous_y);
                        previous_y = points[0].y;
                        assert!(interior.contains_rect(
                            Rect::from_two_pos(points[0], points[1]).expand(stroke.width * 0.5)
                        ));
                    }
                }
                assert_eq!(bars, 5);
            }
        }
    }

    #[test]
    fn shared_devices_connect_their_physical_source_to_eligible_receivers() {
        let mut state = crate::group::model::tests::fixture(4);
        let key = |n: u8| format!("{n:064x}");
        state.active_input = Some(key(4));
        state.members.get_mut(&key(1)).unwrap().node.input =
            Some(crate::lifecycle::InputStatus::Peer);
        state.members.get_mut(&key(4)).unwrap().node.input =
            Some(crate::lifecycle::InputStatus::Here);
        state.settings.microphone_enabled = true;
        state.bind_microphone();
        let scene = Scene::from_workspace(&state, &key(1));
        assert_eq!(scene.ports[2].source, Some(key(1)));
        assert_eq!(scene.ports[2].destinations, [key(4)]);
        assert_eq!(scene.ports[0].source, Some(key(2)));
        assert_eq!(scene.ports[0].destinations, [key(1), key(3), key(4)]);
        assert_eq!(scene.ports[4].source, Some(key(3)));
        state.members.get_mut(&key(4)).unwrap().online = false;
        state.settings.keyboard_enabled = false;
        state.settings.microphone_level.muted = true;
        let scene = Scene::from_workspace(&state, &key(1));
        assert!(scene.ports[2].source.is_none());
        assert!(!scene.ports[0].destinations.contains(&key(4)));
        assert!(scene.ports[4].muted);
    }

    fn repaint_delay(
        ctx: &egui::Context,
        scene: &Scene,
        time: f64,
        minimized: bool,
    ) -> std::time::Duration {
        let mut input = egui::RawInput {
            time: Some(time),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 900.0))),
            ..Default::default()
        };
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .minimized = Some(minimized);
        ctx.run_ui(input, |ui| {
            show(ui, scene, false, true);
        })
        .viewport_output[&egui::ViewportId::ROOT]
            .repaint_delay
    }

    fn settled_repaint_delay(
        ctx: &egui::Context,
        scene: &Scene,
        time: f64,
        minimized: bool,
    ) -> std::time::Duration {
        // Drain egui's initial layout/repaint requests independently of the
        // workspace transition, without advancing the synthetic input clock.
        for _ in 0..8 {
            repaint_delay(ctx, scene, time, minimized);
        }
        repaint_delay(ctx, scene, time, minimized)
    }

    #[test]
    fn workspace_transitions_finish_and_only_scene_changes_restart_them() {
        let ctx = egui::Context::default();
        let scene = Scene::from_workspace(&crate::group::model::tests::fixture(2), "");
        assert!(settled_repaint_delay(&ctx, &scene, 0.0, false) < std::time::Duration::MAX);
        for time in [0.1, 0.2, 0.36] {
            assert!(repaint_delay(&ctx, &scene, time, false) < std::time::Duration::MAX);
        }
        assert!(repaint_delay(&ctx, &scene, 0.6, false) < std::time::Duration::MAX);
        for time in [1.3, 4.0, 60.0] {
            assert_eq!(
                settled_repaint_delay(&ctx, &scene, time, false),
                std::time::Duration::MAX
            );
        }
        let mut changed = scene.clone();
        changed.ports[0].muted = !changed.ports[0].muted;
        assert!(repaint_delay(&ctx, &changed, 60.1, false) < std::time::Duration::MAX);
        assert!(repaint_delay(&ctx, &changed, 60.3, false) < std::time::Duration::MAX);
        assert_eq!(
            settled_repaint_delay(&ctx, &changed, 61.4, false),
            std::time::Duration::MAX
        );
    }

    #[test]
    fn minimized_and_reduced_motion_scenes_do_not_schedule_animation_frames() {
        let scene = Scene::from_workspace(&crate::group::model::tests::fixture(2), "");
        for (minimized, reduced) in [(true, false), (false, true), (true, true)] {
            let ctx = egui::Context::default();
            super::super::view::motion(&ctx, reduced);
            assert_eq!(
                settled_repaint_delay(&ctx, &scene, 0.1, minimized),
                std::time::Duration::MAX
            );
        }
    }
}
