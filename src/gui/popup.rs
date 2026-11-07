use super::{Action, Desktop};
use crate::workspace::{AudioLevel, Plan};
use eframe::egui::{self, Color32, Pos2, RichText, Vec2};
use std::sync::{Arc, Mutex, mpsc::Sender};

const WIDTH: f32 = 344.0;
const HEIGHT: f32 = 442.0;
#[cfg(debug_assertions)]
pub(super) static FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("desk-switch-quick-controls")
}

#[derive(Clone, Copy, PartialEq)]
pub enum Device {
    Audio,
    Keyboard,
    Mouse,
    Microphone,
}
#[derive(Clone, Copy, PartialEq)]
pub enum Command {
    Toggle(Device, bool),
    Level { remote: bool, level: AudioLevel },
    GroupLevel { key: [u8; 32], level: AudioLevel },
    Target([u8; 32]),
}

#[derive(Clone, Default, PartialEq)]
struct Snapshot {
    targets: Vec<([u8; 32], String, bool)>,
    connected: bool,
    can_switch: bool,
    monitor: bool,
    target: String,
    peer: String,
    output: String,
    audio: bool,
    keyboard: bool,
    mouse: bool,
    microphone: bool,
    can_disable_keyboard: bool,
    can_disable_mouse: bool,
    levels: Vec<Level>,
    notice: String,
}
#[derive(Clone, PartialEq)]
struct Level {
    group_key: Option<[u8; 32]>,
    name: String,
    remote: bool,
    system: bool,
    value: AudioLevel,
}
struct State {
    open: bool,
    seen_focus: bool,
    position: Pos2,
    snapshot: Snapshot,
}
pub struct Popup {
    state: Arc<Mutex<State>>,
    send: Sender<Action>,
}
impl Popup {
    #[cfg(debug_assertions)]
    pub(super) fn sender(&self) -> Sender<Action> {
        self.send.clone()
    }
    pub fn new(send: Sender<Action>) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                open: false,
                seen_focus: false,
                position: Pos2::ZERO,
                snapshot: Snapshot::default(),
            })),
            send,
        }
    }
    pub fn open(&self, ctx: &egui::Context, x: i32, y: i32) {
        let mut state = self.state.lock().unwrap();
        if state.open {
            state.open = false;
            ctx.send_viewport_cmd_to(id(), egui::ViewportCommand::Close);
        } else {
            state.open = true;
            #[cfg(target_os = "linux")]
            super::linux_activation::request(super::linux_activation::Role::Quick);
            state.seen_focus = false;
            state.position = position(ctx, x, y);
        }
        ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
    }
    pub fn close(&self, ctx: &egui::Context) {
        let was_open = {
            let mut state = self.state.lock().unwrap();
            let open = state.open;
            state.open = false;
            open
        };
        if was_open {
            ctx.send_viewport_cmd_to(id(), egui::ViewportCommand::Close);
        }
    }
    pub fn repaint(&self, ctx: &egui::Context) {
        if self.state.lock().unwrap().open {
            ctx.request_repaint_of(id());
        }
    }
}

fn fitted(anchor: Pos2, bounds: egui::Rect) -> Pos2 {
    let x = (anchor.x - 24.0).clamp(
        bounds.left() + 8.0,
        (bounds.right() - WIDTH - 8.0).max(bounds.left() + 8.0),
    );
    let preferred = if anchor.y - bounds.top() > HEIGHT + 16.0 {
        anchor.y - HEIGHT - 12.0
    } else {
        anchor.y + 12.0
    };
    let y = preferred.clamp(
        bounds.top() + 8.0,
        (bounds.bottom() - HEIGHT - 8.0).max(bounds.top() + 8.0),
    );
    egui::pos2(x, y)
}
fn position(ctx: &egui::Context, x: i32, y: i32) -> Pos2 {
    #[cfg(target_os = "macos")]
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        // AppKit points and CoreGraphics screen pixels differ on Retina. Read
        // the logical mouse/work area here rather than using tray-icon's
        // physical coordinates with the focused root window's scale factor.
        let screens = objc2_app_kit::NSScreen::screens(mtm);
        if let Some(primary) = screens.firstObject() {
            let frame = primary.frame();
            let top = frame.origin.y + frame.size.height;
            let mouse = objc2_app_kit::NSEvent::mouseLocation();
            if let Some(screen) = screens.iter().find(|screen| {
                let frame = screen.frame();
                mouse.x >= frame.origin.x
                    && mouse.x < frame.origin.x + frame.size.width
                    && mouse.y >= frame.origin.y
                    && mouse.y < frame.origin.y + frame.size.height
            }) {
                let area = screen.visibleFrame();
                return fitted(
                    egui::pos2(mouse.x as f32, (top - mouse.y) as f32),
                    egui::Rect::from_min_size(
                        egui::pos2(
                            area.origin.x as f32,
                            (top - area.origin.y - area.size.height) as f32,
                        ),
                        egui::vec2(area.size.width as f32, area.size.height as f32),
                    ),
                );
            }
        }
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{Foundation::POINT, Graphics::Gdi::*, UI::HiDpi::*};
        let point = POINT { x, y };
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let (mut dpi_x, mut dpi_y) = (96, 96);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let scale = dpi_x.max(96) as f32 / 96.0;
        if GetMonitorInfoW(monitor, &mut info) != 0 {
            let work = info.rcWork;
            return fitted(
                egui::pos2(x as f32 / scale, y as f32 / scale),
                egui::Rect::from_min_max(
                    egui::pos2(work.left as f32 / scale, work.top as f32 / scale),
                    egui::pos2(work.right as f32 / scale, work.bottom as f32 / scale),
                ),
            );
        }
    }
    let size = ctx
        .input(|i| i.viewport().monitor_size)
        .unwrap_or(egui::vec2(1920.0, 1080.0));
    fitted(
        egui::pos2(x as f32, y as f32),
        egui::Rect::from_min_size(Pos2::ZERO, size),
    )
}

impl Desktop {
    pub(super) fn quick_action(&mut self, command: Command) {
        if let Some(state) = self.group.workspace.clone() {
            use crate::group::net::Action as GroupAction;
            match command {
                Command::Toggle(device, enabled) => {
                    let mut value = state.settings.clone();
                    match device {
                        Device::Audio => value.audio_enabled = enabled,
                        Device::Keyboard => value.keyboard_enabled = enabled,
                        Device::Mouse => value.mouse_enabled = enabled,
                        Device::Microphone => {
                            value.microphone_enabled = enabled;
                            value.microphone_binding = None;
                        }
                    }
                    self.group_action(GroupAction::Settings {
                        expected: state.revision,
                        value,
                    });
                }
                Command::GroupLevel { key, level } => {
                    if let Some(member) = state.members.get(&crate::config::hex(&key)) {
                        let mut value = member.preferences.clone();
                        if state.plan().audio.as_ref() == Some(&member.node.key) {
                            value.output = Some(level);
                        } else {
                            value.audio = level;
                        }
                        self.group_action(GroupAction::Member {
                            key: member.node.key.clone(),
                            expected: member.preferences.clone(),
                            value,
                        });
                    }
                }
                Command::Target(key) => {
                    self.group_action(GroupAction::Activate {
                        key: crate::config::hex(&key),
                    });
                }
                Command::Level { .. } => {}
            }
            return;
        }
        let local = self.local_node();
        match command {
            Command::Toggle(Device::Keyboard, false)
                if self
                    .pair
                    .remote
                    .as_ref()
                    .is_some_and(|remote| !Plan::can_disable(&local, remote, true)) =>
            {
                return;
            }
            Command::Toggle(Device::Mouse, false)
                if self
                    .pair
                    .remote
                    .as_ref()
                    .is_some_and(|remote| !Plan::can_disable(&local, remote, false)) =>
            {
                return;
            }
            Command::Toggle(device, enabled) => match device {
                Device::Audio => self.prefs.workspace.shared.audio_enabled = enabled,
                Device::Keyboard => self.prefs.workspace.shared.keyboard_enabled = enabled,
                Device::Mouse => self.prefs.workspace.shared.mouse_enabled = enabled,
                Device::Microphone => {
                    self.prefs.workspace.shared.microphone_enabled = enabled;
                    self.prefs.workspace.shared.microphone_binding = None;
                }
            },
            Command::Level { remote, level } => {
                if level.volume > 100 {
                    return;
                }
                let key = if remote {
                    self.pair.remote.as_ref().map(|node| node.key.clone())
                } else {
                    Some(self.public_key.clone())
                };
                let Some(key) = key else { return };
                self.prefs.workspace.shared.audio_levels.insert(key, level);
            }
            Command::GroupLevel { .. } | Command::Target(_) => {}
        }
        self.shared_changed();
    }
    pub(super) fn quick_popup(&mut self, ctx: &egui::Context) {
        let open = self.quick.state.lock().unwrap().open;
        if !open || self.quitting {
            return;
        }
        let local = self.local_node();
        let remote = self.pair.remote.as_ref();
        let shared = &self.prefs.workspace.shared;
        let output = self
            .pair
            .plan
            .as_ref()
            .and_then(|p| p.audio_output.as_deref());
        let mut snapshot = Snapshot {
            connected: self.pair.synced,
            can_switch: self.input_switch_ready() || self.monitor_switch_ready(),
            monitor: self.monitor_switch_ready(),
            target: self
                .pair
                .plan
                .as_ref()
                .and_then(|p| p.input_source.as_deref())
                .map(|source| {
                    let here = if source == local.key {
                        local.input != Some(crate::lifecycle::InputStatus::Peer)
                    } else {
                        local.input == Some(crate::lifecycle::InputStatus::Here)
                    };
                    if here {
                        local.name.clone()
                    } else {
                        remote.map(|n| n.name.clone()).unwrap_or_default()
                    }
                })
                .unwrap_or_default(),
            peer: remote
                .map(|n| n.name.clone())
                .unwrap_or_else(|| crate::i18n::tr("No paired computer")),
            output: output
                .and_then(|key| {
                    [Some(&local), remote]
                        .into_iter()
                        .flatten()
                        .find(|n| n.key == key)
                })
                .map(|n| n.hardware.audio_name.clone())
                .unwrap_or_default(),
            audio: shared.audio_enabled,
            keyboard: shared.keyboard_enabled,
            mouse: shared.mouse_enabled,
            microphone: shared.microphone_enabled,
            can_disable_keyboard: remote.is_none_or(|r| Plan::can_disable(&local, r, true)),
            can_disable_mouse: remote.is_none_or(|r| Plan::can_disable(&local, r, false)),
            notice: self
                .notice
                .as_ref()
                .filter(|(_, error)| *error)
                .map(|(text, _)| text.clone())
                .unwrap_or_default(),
            ..Default::default()
        };
        for (node, is_remote) in [(Some(&local), false), (remote, true)] {
            if let Some(node) = node {
                snapshot.levels.push(Level {
                    group_key: None,
                    name: node.name.clone(),
                    remote: is_remote,
                    system: output == Some(node.key.as_str()),
                    value: shared.audio_level(&node.key),
                });
            }
        }
        if let Some(workspace) = &self.group.workspace {
            let plan = workspace.plan();
            snapshot.connected = workspace
                .members
                .values()
                .filter(|member| member.online)
                .count()
                > 1;
            snapshot.can_switch = workspace
                .members
                .values()
                .filter(|member| member.compatible())
                .count()
                > 1
                && !workspace.switch_pending();
            snapshot.audio = workspace.settings.audio_enabled;
            snapshot.keyboard = workspace.settings.keyboard_enabled;
            snapshot.mouse = workspace.settings.mouse_enabled;
            snapshot.microphone = workspace.settings.microphone_enabled;
            snapshot.monitor = false;
            snapshot.peer = format!(
                "{} {}",
                workspace
                    .members
                    .values()
                    .filter(|member| member.online)
                    .count(),
                crate::i18n::tr("computers connected")
            );
            snapshot.target = workspace
                .active_input
                .as_ref()
                .and_then(|key| workspace.members.get(key))
                .map(|member| member.node.name.clone())
                .unwrap_or_default();
            snapshot.output = plan
                .audio
                .as_ref()
                .and_then(|key| workspace.members.get(key))
                .map(|member| member.node.hardware.audio_name.clone())
                .unwrap_or_default();
            snapshot.levels.clear();
            for member in workspace.members.values() {
                let Ok(key) = crate::config::key(&member.node.key) else {
                    continue;
                };
                snapshot.targets.push((
                    key,
                    member.node.name.clone(),
                    member.compatible() && !workspace.switch_pending(),
                ));
                let system = plan.audio.as_ref() == Some(&member.node.key);
                snapshot.levels.push(Level {
                    group_key: Some(key),
                    name: member.node.name.clone(),
                    remote: member.node.key != self.public_key,
                    system,
                    value: if system {
                        member
                            .native_output
                            .or(member.preferences.output)
                            .unwrap_or_default()
                    } else {
                        member.preferences.audio
                    },
                });
            }
            snapshot.can_disable_keyboard = workspace
                .active_input
                .as_ref()
                .and_then(|key| workspace.members.get(key))
                .is_none_or(|member| {
                    plan.input.as_ref() == Some(&member.node.key) || member.node.hardware.keyboard
                });
            snapshot.can_disable_mouse = workspace
                .active_input
                .as_ref()
                .and_then(|key| workspace.members.get(key))
                .is_none_or(|member| {
                    plan.input.as_ref() == Some(&member.node.key) || member.node.hardware.mouse
                });
        }
        let position = {
            let mut state = self.quick.state.lock().unwrap();
            if state.snapshot != snapshot {
                state.snapshot = snapshot;
                ctx.request_repaint_of(id());
            }
            state.position
        };
        let state = self.quick.state.clone();
        let send = self.quick.send.clone();
        let root = ctx.clone();
        #[cfg(debug_assertions)]
        if self.preview_popup {
            return;
        }
        ctx.show_viewport_deferred(
            id(),
            egui::ViewportBuilder::default()
                .with_title("Desk Switch quick controls")
                .with_inner_size([WIDTH, HEIGHT])
                .with_position(position)
                .with_decorations(false)
                .with_resizable(false)
                .with_taskbar(false)
                .with_always_on_top()
                .with_active(true),
            move |ui, _| {
                let (snapshot, close) = {
                    let mut state = state.lock().unwrap();
                    let focused = ui.ctx().input(|i| i.viewport().focused);
                    if focused == Some(true) {
                        state.seen_focus = true;
                    }
                    let close = ui.ctx().input(|i| {
                        i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)
                    }) || (state.seen_focus && focused == Some(false));
                    (state.snapshot.clone(), close)
                };
                if close {
                    state.lock().unwrap().open = false;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    root.request_repaint_of(egui::ViewportId::ROOT);
                    return;
                }
                let popup_ctx = ui.ctx().clone();
                content(ui, &snapshot, |action| {
                    if matches!(action, Action::QuickClose | Action::Show | Action::Quit) {
                        state.lock().unwrap().open = false;
                        popup_ctx.send_viewport_cmd_to(id(), egui::ViewportCommand::Close);
                    }
                    super::tray::dispatch(&send, &root, action);
                });
            },
        );
    }
    #[cfg(debug_assertions)]
    pub(super) fn quick_preview(&mut self, ui: &mut egui::Ui) {
        self.quick.state.lock().unwrap().open = true;
        self.quick_popup(ui.ctx());
        let snapshot = self.quick.state.lock().unwrap().snapshot.clone();
        let mut commands = Vec::new();
        content(ui, &snapshot, |action| commands.push(action));
        for action in commands {
            self.action(action, ui.ctx());
        }
    }
}

fn content(ui: &mut egui::Ui, snapshot: &Snapshot, mut send: impl FnMut(Action)) {
    #[cfg(debug_assertions)]
    FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tr = crate::i18n::tr;
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
    ui.spacing_mut().slider_width = 175.0;
    ui.spacing_mut().interact_size.y = 28.0;
    egui::Frame::new()
        .fill(ui.visuals().window_fill())
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_width(WIDTH - 32.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Desk Switch").size(19.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("×").clicked() {
                        send(Action::QuickClose);
                    }
                });
            });
            ui.horizontal(|ui| {
                let color = if snapshot.connected {
                    Color32::from_rgb(76, 211, 163)
                } else {
                    ui.visuals().weak_text_color()
                };
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(10.0, 18.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.0, color);
                ui.add(egui::Label::new(&snapshot.peer).truncate());
            });
            if !snapshot.target.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{} · {}", tr("Input"), snapshot.target))
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    )
                    .truncate(),
                );
            }
            if !snapshot.targets.is_empty() {
                egui::ComboBox::from_id_salt("quick-input-target")
                    .width(WIDTH - 64.0)
                    .selected_text(tr("Switch to computer…"))
                    .show_ui(ui, |ui| {
                        for (key, name, enabled) in &snapshot.targets {
                            if ui.add_enabled(*enabled, egui::Button::new(name)).clicked() {
                                send(Action::Quick(Command::Target(*key)));
                            }
                        }
                    });
            }
            if ui
                .add_enabled(
                    snapshot.can_switch,
                    egui::Button::new(tr(if snapshot.targets.is_empty() {
                        "Switch to the other PC"
                    } else {
                        "Switch to next computer"
                    }))
                    .min_size(Vec2::new(WIDTH - 32.0, 34.0)),
                )
                .clicked()
            {
                send(if snapshot.monitor {
                    Action::MonitorSwitch
                } else {
                    Action::Switch
                });
            }
            ui.separator();
            egui::Grid::new("quick-device-toggles")
                .num_columns(2)
                .spacing([22.0, 4.0])
                .show(ui, |ui| {
                    for (index, (device, label, value, enabled)) in [
                        (Device::Audio, "Sound", snapshot.audio, true),
                        (Device::Microphone, "Microphone", snapshot.microphone, true),
                        (
                            Device::Keyboard,
                            "Keyboard",
                            snapshot.keyboard,
                            !snapshot.keyboard || snapshot.can_disable_keyboard,
                        ),
                        (
                            Device::Mouse,
                            "Mouse",
                            snapshot.mouse,
                            !snapshot.mouse || snapshot.can_disable_mouse,
                        ),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let mut value = value;
                        if ui
                            .add_enabled(enabled, egui::Checkbox::new(&mut value, tr(label)))
                            .on_disabled_hover_text(format!(
                                "{} (F11)",
                                tr("Return control before changing shared input devices.")
                            ))
                            .changed()
                        {
                            send(Action::Quick(Command::Toggle(device, value)));
                        }
                        if index % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
            if !snapshot.output.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(&snapshot.output)
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    )
                    .truncate(),
                )
                .on_hover_text(&snapshot.output);
            }
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height((ui.available_height() - 50.0).max(80.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for row in &snapshot.levels {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [120.0, 18.0],
                                egui::Label::new(RichText::new(&row.name).strong()).truncate(),
                            )
                            .on_hover_text(&row.name);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let caption = tr(if row.system {
                                        "System volume"
                                    } else {
                                        "Relative volume"
                                    });
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&caption)
                                                .small()
                                                .color(ui.visuals().weak_text_color()),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(caption);
                                },
                            );
                        });
                        let mut level = row.value;
                        let changed = ui
                            .horizontal(|ui| {
                                let slider = ui
                                    .scope(|ui| {
                                        let font = egui::TextStyle::Body.resolve(ui.style());
                                        let mute_width = ui.fonts_mut(|fonts| {
                                            fonts
                                                .layout_no_wrap(tr("Mute"), font, Color32::WHITE)
                                                .size()
                                                .x
                                        });
                                        ui.spacing_mut().slider_width =
                                            (ui.available_width() - mute_width - 106.0)
                                                .clamp(96.0, 175.0);
                                        ui.add(
                                            egui::Slider::new(&mut level.volume, 0..=100)
                                                .suffix(" %"),
                                        )
                                    })
                                    .inner
                                    .changed();
                                ui.checkbox(&mut level.muted, tr("Mute")).changed() || slider
                            })
                            .inner;
                        if changed {
                            send(Action::Quick(if let Some(key) = row.group_key {
                                Command::GroupLevel { key, level }
                            } else {
                                Command::Level {
                                    remote: row.remote,
                                    level,
                                }
                            }));
                        }
                    }
                });
            if !snapshot.notice.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(tr(&snapshot.notice)).color(ui.visuals().error_fg_color),
                    )
                    .truncate(),
                )
                .on_hover_text(tr(&snapshot.notice));
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(tr("Open Desk Switch")).clicked() {
                    send(Action::Show);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Quit application")).clicked() {
                        send(Action::Quit);
                    }
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_popup_open_close_and_quit_wake_the_hidden_main_dispatcher() {
        for action in [Action::Show, Action::QuickClose, Action::Quit] {
            let ctx = egui::Context::default();
            ctx.set_embed_viewports(false);
            // Drain the root's initial repaint requests before entering a child.
            for _ in 0..8 {
                let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
            }
            let repaints = Arc::new(Mutex::new(Vec::new()));
            let reported = repaints.clone();
            ctx.set_request_repaint_callback(move |info| {
                reported.lock().unwrap().push(info.viewport_id);
            });
            let (send, receive) = std::sync::mpsc::channel();
            let mut input = egui::RawInput {
                viewport_id: id(),
                ..Default::default()
            };
            input.viewports.insert(
                id(),
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    ..Default::default()
                },
            );
            let output = ctx.run_ui(input, |ui| {
                assert_eq!(ui.ctx().viewport_id(), id());
                repaints.lock().unwrap().clear();
                super::super::tray::dispatch(&send, ui.ctx(), action);
            });
            assert!(
                receive
                    .recv_timeout(std::time::Duration::from_secs(1))
                    .unwrap()
                    == action
            );
            assert!(
                repaints.lock().unwrap().contains(&egui::ViewportId::ROOT),
                "the hidden root must be awakened from the child viewport"
            );
            if action == Action::Show {
                let root = &output.viewport_output[&egui::ViewportId::ROOT];
                assert!(
                    root.commands
                        .contains(&egui::ViewportCommand::Visible(true))
                );
                assert!(root.commands.contains(&egui::ViewportCommand::Focus));
            }
        }
    }

    #[test]
    fn popup_stays_within_the_work_area_on_negative_and_positive_monitors() {
        for bounds in [
            egui::Rect::from_min_size(egui::pos2(-1920.0, 0.0), egui::vec2(1920.0, 1040.0)),
            egui::Rect::from_min_size(egui::pos2(0.0, -1080.0), egui::vec2(1920.0, 1080.0)),
            egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(640.0, 480.0)),
        ] {
            for anchor in [bounds.left_top(), bounds.right_bottom(), bounds.center()] {
                let popup =
                    egui::Rect::from_min_size(fitted(anchor, bounds), egui::vec2(WIDTH, HEIGHT));
                assert!(bounds.contains_rect(popup));
            }
        }
    }
}
