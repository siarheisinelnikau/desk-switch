//! Shared workspace illustration. Motion runs only while the scene is visible.
use super::view::{DeviceIcon, WorkspaceVisual, paint_device, peer_color};
use crate::group::model::Workspace;
use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};

const CYAN: Color32 = Color32::from_rgb(75, 196, 244);
const GREEN: Color32 = Color32::from_rgb(83, 217, 177);

#[derive(Clone, Default, PartialEq)]
pub(super) struct Scene {
    peers: Vec<Peer>,
    ports: [Port; 5],
    pending: bool,
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
}
impl MouseWave {
    fn update(&mut self, direction: Vec2, active: bool, now: f64) {
        let dt = (now - self.updated).clamp(0.0, 0.1) as f32;
        self.updated = now;
        if direction.length_sq() > 0.5 {
            // Follow the shortest arc, including across the -PI/PI boundary.
            let difference = (direction.angle() - self.angle + std::f32::consts::PI)
                .rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            self.angle += difference * (1.0 - (-dt / 0.18).exp());
        }
        let target = if active { 1.0 } else { 0.0 };
        self.opacity += (target - self.opacity) * (1.0 - (-dt / 0.14).exp());
        if self.opacity < 0.001 {
            self.opacity = 0.0;
        }
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
        });
        wave.update(activity.direction, now - activity.moved < 0.65, now);
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
        Self {
            peers,
            ports,
            pending: state.switch_pending(),
        }
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
            let height = if compact { 202.0 } else { 340.0 } + (rows - 1) as f32 * row_height;
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
            let recent_input = now - activity.moved < 0.85
                || now - activity.wheel < 0.6
                || activity.released.iter().any(|t| now - t < 0.6);
            let animating = !reduced
                && !ui.input(|i| i.viewport().minimized.unwrap_or(false))
                && (now < transition || recent_input);
            if animating {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(60));
            }
            let phase = animating.then_some((now % 120.0) as f32);
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
                let active_phase = if port.muted || port.error {
                    None
                } else {
                    phase
                };
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
            for (index, port) in scene.ports.iter().enumerate() {
                let center = devices[index];
                let color = port
                    .source
                    .as_ref()
                    .and_then(|key| scene.peers.iter().find(|p| &p.key == key))
                    .map(|p| super::view::contrast_color(p.color, dark))
                    .unwrap_or(muted.gamma_multiply(0.5));
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
                            &activity,
                            now,
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
                            &activity,
                            &mouse_wave,
                            phase,
                        );
                    }
                    if matches!(index, 0 | 4)
                        && port.source.is_some()
                        && !port.muted
                        && let Some(time) = phase
                    {
                        waveform(&painter, icon_center, scale, color, time, index);
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
    phase: Option<f32>,
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
    if let Some(time) = phase {
        // Both segments carry a slow continuous stream. The receiving segment
        // follows the source segment in the same host -> device -> peer direction.
        let t =
            (time / 3.8 - index as f32 * 0.06 - if receiving { 0.5 } else { 0.0 }).rem_euclid(1.0);
        let head = point(points, t);
        let trail_start = (t - 0.10).max(0.0);
        let trail = (0..=8)
            .map(|i| point(points, trail_start + (t - trail_start) * i as f32 / 8.0))
            .collect();
        if t > 0.0001 {
            painter.add(egui::Shape::line(
                trail,
                Stroke::new(if compact { 1.8 } else { 2.5 }, color),
            ));
        }
        arrow(
            painter,
            head,
            point(points, (t + 0.015).min(1.0)) - point(points, (t - 0.015).max(0.0)),
            if painter.ctx().theme() == egui::Theme::Dark {
                Color32::WHITE
            } else {
                color
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
    phase: Option<f32>,
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
        if let Some(time) = phase {
            let x = -23.0 + (time / 4.0).fract() * 46.0;
            p.line_segment(
                [pt(x - 4.0, 11.0), pt(x + 4.0, 11.0)],
                Stroke::new(1.8 * scale, color),
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
        if let Some(time) = phase {
            p.circle_stroke(
                pt(0.0, 1.0),
                (5.3 + (time * 1.8).sin() * 0.9) * scale,
                Stroke::new(scale, color.gamma_multiply(0.5)),
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
    phase: Option<f32>,
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
    let shift = phase.map(|t| (t * 2.0).sin() * 3.0 * scale).unwrap_or(0.0);
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
    activity: &super::activity::Activity,
    now: f64,
    phase: Option<f32>,
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
    let sweep = phase.map(|time| (time / 5.0).fract() * 14.0);
    let mut bottom_x = -55.0;
    for index in 0..66 {
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
        let recent = (now - activity.released[index]).clamp(0.0, 0.32) < 0.32;
        let lit = activity.keys[index] || recent;
        let glow = sweep.is_some_and(|s| (s - (index % 14) as f32).abs() < 1.2);
        let rect = Rect::from_min_size(
            center + egui::vec2(x, y) * scale,
            egui::vec2(width, 5.0) * scale,
        );
        if lit {
            p.rect_filled(rect.expand(scale), 1, color.gamma_multiply(0.22));
        }
        p.rect_filled(
            rect,
            1,
            if lit {
                Color32::from_rgb(219, 249, 255)
            } else {
                color.gamma_multiply(if glow { 0.8 } else { 0.28 })
            },
        );
    }
}

fn mouse_motion(
    p: &egui::Painter,
    center: Pos2,
    scale: f32,
    color: Color32,
    activity: &super::activity::Activity,
    wave: &MouseWave,
    phase: Option<f32>,
) {
    let now = wave.updated;
    let Some(time) = phase else {
        return;
    };
    let wheel = center + egui::vec2(0.0, -7.0 + (time * 1.5).sin()) * scale;
    p.line_segment(
        [
            wheel - egui::vec2(0.0, 1.7 * scale),
            wheel + egui::vec2(0.0, 1.7 * scale),
        ],
        Stroke::new(
            1.8 * scale,
            if now - activity.wheel < 0.4 {
                Color32::WHITE
            } else {
                color
            },
        ),
    );
    for (index, x) in [-3.5, 3.5].into_iter().enumerate() {
        if activity.buttons[index] {
            p.rect_filled(
                Rect::from_center_size(
                    center + egui::vec2(x, -6.0) * scale,
                    egui::vec2(4.0, 6.0) * scale,
                ),
                2,
                Color32::WHITE,
            );
        }
    }
    if wave.opacity > 0.0 {
        for index in 0..3 {
            let progress = (time * 0.7 + index as f32 / 3.0).fract();
            let distance = 16.0 + progress * 15.0;
            let opacity = wave.opacity * (std::f32::consts::PI * progress).sin().powi(2);
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

    #[test]
    fn mouse_waves_turn_across_the_short_arc_and_fade_without_visibility_steps() {
        let initial = 179.0_f32.to_radians();
        let target = Vec2::angled((-179.0_f32).to_radians());
        let mut wave = MouseWave {
            angle: initial,
            opacity: 0.0,
            updated: 0.0,
        };
        wave.update(target, true, 0.016);
        assert!(wave.angle > initial && wave.angle < initial + 2.0_f32.to_radians());
        assert!(wave.opacity > 0.0 && wave.opacity < 1.0);
        for frame in 2..=100 {
            wave.update(target, true, frame as f64 * 0.016);
        }
        assert!(wave.opacity > 0.99);
        let before = wave.opacity;
        wave.update(target, false, 1.616);
        assert!(wave.opacity > 0.0 && wave.opacity < before);
        for frame in 102..=220 {
            wave.update(target, false, frame as f64 * 0.016);
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

    #[test]
    fn visible_motion_keeps_running_and_a_minimized_scene_stops_scheduling_frames() {
        let ctx = egui::Context::default();
        let scene = Scene::from_workspace(&crate::group::model::tests::fixture(2), "");
        let draw = |time, minimized| {
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
                show(ui, &scene, false, true);
            })
        };
        for time in [0.0, 4.0, 60.0] {
            let active = draw(time, false);
            assert!(
                active.viewport_output[&egui::ViewportId::ROOT].repaint_delay
                    < std::time::Duration::MAX
            );
        }
        for _ in 0..8 {
            draw(61.0, true);
        }
        let hidden = draw(61.0, true);
        assert_eq!(
            hidden.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
            std::time::Duration::MAX
        );
    }
}
