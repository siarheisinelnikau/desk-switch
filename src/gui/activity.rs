//! Ephemeral local and received input feedback. No text or persistent history.
use eframe::egui::{self, Vec2};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct Remote {
    latest: crate::feedback::Snapshot,
    pending: bool,
    arrived: Instant,
    visual: Activity,
}
#[derive(Default)]
pub(super) struct Bus {
    sources: BTreeMap<u32, Remote>,
    views: BTreeSet<egui::ViewportId>,
}
fn bus(ctx: &egui::Context) -> Arc<Mutex<Bus>> {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<Bus>>>(id().with("received"))
            .clone()
    })
}
pub(crate) fn receive(ctx: &egui::Context, source: u32, mut snapshot: crate::feedback::Snapshot) {
    let bus = bus(ctx);
    let views = {
        let mut bus = bus.lock().unwrap();
        if bus.sources.len() >= 32 && !bus.sources.contains_key(&source) {
            return;
        }
        if bus.views.is_empty() {
            snapshot.released = [0; 4];
            snapshot.released_buttons = 0;
            snapshot.motion = None;
            snapshot.wheel = false;
        }
        let entry = bus.sources.entry(source).or_insert_with(|| Remote {
            latest: Default::default(),
            pending: false,
            arrived: Instant::now(),
            visual: Activity::default(),
        });
        if entry.pending {
            entry.latest.merge(snapshot);
        } else {
            entry.latest = snapshot;
        }
        entry.pending = true;
        entry.arrived = Instant::now();
        bus.views.clone()
    };
    for viewport in views {
        ctx.request_repaint_of(viewport);
    }
}
pub(crate) fn disconnected(ctx: &egui::Context, source: u32) {
    let bus = bus(ctx);
    let views = {
        let mut bus = bus.lock().unwrap();
        bus.sources.remove(&source);
        bus.views.clone()
    };
    for viewport in views {
        ctx.request_repaint_of(viewport);
    }
}

pub(super) const KEYS: [u16; 66] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 58, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 43, 28, 42, 44, 45, 46, 47, 48, 49, 50,
    51, 52, 53, 54, 103, 111, 29, 125, 56, 57, 100, 97, 105, 108, 106, 96,
];

#[derive(Clone)]
pub(super) struct Activity {
    pub keys: [bool; 66],
    pub released: [f64; 66],
    pub direction: Vec2,
    pub moved: f64,
    pub buttons: [bool; 3],
    pub clicked: [f64; 3],
    pub wheel: f64,
}
impl Default for Activity {
    fn default() -> Self {
        Self {
            keys: [false; 66],
            released: [-1000.0; 66],
            direction: Vec2::ZERO,
            moved: -1000.0,
            buttons: [false; 3],
            clicked: [-1000.0; 3],
            wheel: -1000.0,
        }
    }
}
fn id() -> egui::Id {
    egui::Id::new("physical-device-activity")
}

#[cfg(any(windows, target_os = "linux"))]
pub(super) fn begin_frame(ctx: &egui::Context, viewport: egui::ViewportId) {
    ctx.data_mut(|data| {
        data.remove::<bool>(id().with(viewport));
    });
    bus(ctx).lock().unwrap().views.remove(&viewport);
}
#[cfg(any(windows, target_os = "linux"))]
pub(super) fn interested(ctx: &egui::Context, viewport: egui::ViewportId) -> bool {
    ctx.data(|data| data.get_temp::<bool>(id().with(viewport)).unwrap_or(false))
}

#[cfg(windows)]
pub(super) fn record(
    ctx: &egui::Context,
    event: winit::event::DeviceEvent,
    time: f64,
) -> std::time::Duration {
    use winit::{
        event::{DeviceEvent, ElementState},
        platform::scancode::PhysicalKeyExtScancode,
    };
    let wake = ctx.data_mut(|data| {
        let state = data.get_temp_mut_or_default::<Activity>(id());
        match event {
            DeviceEvent::Key(key) => {
                if let Some(scan) = key.physical_key.to_scancode()
                    && let Some(code) = crate::keys::scan_code(scan & 0xff, scan & 0xff00 != 0, 0)
                {
                    state.key(code, key.state == ElementState::Pressed, time);
                }
            }
            DeviceEvent::MouseMotion { delta: (dx, dy) } => {
                let delta = Vec2::new(dx as f32, dy as f32);
                if delta.length_sq() > 0.5 {
                    state.direction =
                        (state.direction * 0.4 + delta.normalized() * 0.6).normalized();
                    state.moved = time;
                }
            }
            DeviceEvent::Button {
                button,
                state: pressed,
            } if button < 3 => {
                state.buttons[button as usize] = pressed == ElementState::Pressed;
                if pressed == ElementState::Released {
                    state.clicked[button as usize] = time;
                }
            }
            DeviceEvent::MouseWheel { .. } => state.wheel = time,
            _ => {}
        }
        let key = id().with("last-input-wake");
        let last = data.get_temp::<f64>(key).unwrap_or(-1000.0);
        let wake = time - last >= 0.06;
        if wake {
            data.insert_temp(key, time);
        }
        wake
    });
    // The host schedules every interested visible viewport, including the tray
    // popup. A trailing repaint preserves releases inside the coalescing window.
    if wake {
        std::time::Duration::ZERO
    } else {
        std::time::Duration::from_millis(60)
    }
}

impl Activity {
    pub(super) fn key(&mut self, code: u16, down: bool, time: f64) {
        for (index, physical) in KEYS.iter().enumerate() {
            if *physical == code {
                self.keys[index] = down;
                if !down {
                    self.released[index] = time;
                }
            }
        }
    }
}

#[cfg(any(windows, target_os = "linux"))]
pub(super) fn clear(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        data.remove::<Activity>(id());
    });
    let bus = bus(ctx);
    let mut bus = bus.lock().unwrap();
    bus.views.clear();
    for remote in bus.sources.values_mut() {
        remote.visual = Activity::default();
        remote.latest.released = [0; 4];
        remote.latest.released_buttons = 0;
        remote.latest.motion = None;
        remote.latest.wheel = false;
        remote.pending = true;
    }
}

pub(super) fn read(ui: &egui::Ui) -> Activity {
    #[cfg(any(windows, target_os = "linux"))]
    {
        let viewport = ui.ctx().viewport_id();
        ui.ctx()
            .data_mut(|data| data.insert_temp(id().with(viewport), true));
    }
    #[cfg(not(windows))]
    {
        let (events, time, delta) =
            ui.input(|input| (input.events.clone(), input.time, input.pointer.delta()));
        ui.ctx().data_mut(|data| {
            let state = data.get_temp_mut_or_default::<Activity>(id());
            for event in &events {
                if let egui::Event::Key { key, pressed, .. } = event {
                    let code = match key {
                        egui::Key::A => 30,
                        egui::Key::B => 48,
                        egui::Key::C => 46,
                        egui::Key::D => 32,
                        egui::Key::E => 18,
                        egui::Key::F => 33,
                        egui::Key::G => 34,
                        egui::Key::H => 35,
                        egui::Key::I => 23,
                        egui::Key::J => 36,
                        egui::Key::K => 37,
                        egui::Key::L => 38,
                        egui::Key::M => 50,
                        egui::Key::N => 49,
                        egui::Key::O => 24,
                        egui::Key::P => 25,
                        egui::Key::Q => 16,
                        egui::Key::R => 19,
                        egui::Key::S => 31,
                        egui::Key::T => 20,
                        egui::Key::U => 22,
                        egui::Key::V => 47,
                        egui::Key::W => 17,
                        egui::Key::X => 45,
                        egui::Key::Y => 21,
                        egui::Key::Z => 44,
                        egui::Key::Space => 57,
                        egui::Key::Enter => 28,
                        egui::Key::Backspace => 14,
                        egui::Key::ArrowUp => 103,
                        egui::Key::ArrowDown => 108,
                        egui::Key::ArrowLeft => 105,
                        egui::Key::ArrowRight => 106,
                        _ => continue,
                    };
                    state.key(code, *pressed, time);
                }
            }
            if delta.length_sq() > 0.5 {
                state.direction = delta.normalized();
                state.moved = time;
            }
        });
    }
    let mut result = ui
        .ctx()
        .data(|data| data.get_temp::<Activity>(id()).unwrap_or_default());
    let bus = bus(ui.ctx());
    let mut bus = bus.lock().unwrap();
    bus.views.insert(ui.ctx().viewport_id());
    let now = ui.input(|input| input.time);
    for remote in bus.sources.values_mut() {
        if remote.pending {
            for (index, code) in KEYS.iter().copied().enumerate() {
                remote.visual.keys[index] = remote.latest.key(code);
                if remote.latest.was_released(code)
                    && remote.arrived.elapsed() < Duration::from_millis(650)
                {
                    remote.visual.released[index] = now;
                }
            }
            remote.visual.buttons = std::array::from_fn(|i| remote.latest.buttons & (1 << i) != 0);
            if remote.arrived.elapsed() < Duration::from_millis(650) {
                for i in 0..3 {
                    if remote.latest.released_buttons & (1 << i) != 0 {
                        remote.visual.clicked[i] = now;
                    }
                }
                if let Some([dx, dy]) = remote.latest.motion {
                    let delta = Vec2::new(dx as f32, dy as f32);
                    if delta.length_sq() > 0.5 {
                        remote.visual.direction = delta.normalized();
                        remote.visual.moved = now;
                    }
                }
                if remote.latest.wheel {
                    remote.visual.wheel = now;
                }
            }
            remote.pending = false;
        }
        for i in 0..KEYS.len() {
            result.keys[i] |= remote.visual.keys[i];
            result.released[i] = result.released[i].max(remote.visual.released[i]);
        }
        for i in 0..3 {
            result.buttons[i] |= remote.visual.buttons[i];
            result.clicked[i] = result.clicked[i].max(remote.visual.clicked[i]);
        }
        if remote.visual.moved > result.moved {
            result.moved = remote.visual.moved;
            result.direction = remote.visual.direction;
        }
        result.wheel = result.wheel.max(remote.visual.wheel);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &egui::Context, time: f64) -> Activity {
        let mut result = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                ..Default::default()
            },
            |ui| result = Some(read(ui)),
        );
        result.unwrap()
    }

    #[test]
    fn accepted_remote_events_reach_the_visible_view_and_quick_taps_survive_coalescing() {
        use crate::wire::Input;
        let ctx = egui::Context::default();
        frame(&ctx, 1.0);
        let mut reporter = crate::feedback::Reporter::default();
        for event in [
            Input::Key {
                code: 30,
                down: true,
            },
            Input::Key {
                code: 30,
                down: false,
            },
            Input::Button {
                code: 272,
                down: true,
            },
            Input::Button {
                code: 272,
                down: false,
            },
        ] {
            receive(&ctx, 42, reporter.record(event, Instant::now()).unwrap());
        }
        let state = frame(&ctx, 2.0);
        assert!(!state.keys[KEYS.iter().position(|code| *code == 30).unwrap()]);
        assert_eq!(
            state.released[KEYS.iter().position(|code| *code == 30).unwrap()],
            2.0
        );
        assert!(!state.buttons[0]);
        assert_eq!(state.clicked[0], 2.0);
        disconnected(&ctx, 42);
        assert!(bus(&ctx).lock().unwrap().sources.is_empty());
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn hidden_input_keeps_current_holds_without_replaying_transient_activity_on_restore() {
        use crate::wire::Input;
        let ctx = egui::Context::default();
        frame(&ctx, 1.0);
        let mut reporter = crate::feedback::Reporter::default();
        receive(
            &ctx,
            1,
            reporter
                .record(Input::Move { dx: 6, dy: -3 }, Instant::now())
                .unwrap(),
        );
        clear(&ctx);
        receive(
            &ctx,
            1,
            reporter
                .record(
                    Input::Key {
                        code: 42,
                        down: true,
                    },
                    Instant::now(),
                )
                .unwrap(),
        );
        let state = frame(&ctx, 2.0);
        assert!(state.keys[KEYS.iter().position(|code| *code == 42).unwrap()]);
        assert!(state.moved < 0.0);
        assert!(state.released.iter().all(|time| *time < 0.0));
        disconnected(&ctx, 1);
        assert!(frame(&ctx, 3.0).keys.iter().all(|down| !down));
    }
    #[test]
    fn physical_feedback_keeps_only_current_keys_and_short_release_timestamps() {
        let mut state = Activity::default();
        state.key(30, true, 1.0);
        let a = KEYS.iter().position(|key| *key == 30).unwrap();
        assert!(state.keys[a]);
        assert_eq!(state.keys.iter().filter(|down| **down).count(), 1);
        state.key(30, false, 2.0);
        assert!(!state.keys[a]);
        assert_eq!(state.released[a], 2.0);
        state.key(900, true, 3.0);
        assert!(state.keys.iter().all(|down| !down));
    }

    #[cfg(windows)]
    #[test]
    fn raw_physical_keys_and_directional_mouse_reports_drive_the_visual_state() {
        use winit::{
            event::{DeviceEvent, ElementState, RawKeyEvent},
            keyboard::{KeyCode, PhysicalKey},
        };
        let ctx = egui::Context::default();
        record(
            &ctx,
            DeviceEvent::Key(RawKeyEvent {
                physical_key: PhysicalKey::Code(KeyCode::KeyA),
                state: ElementState::Pressed,
            }),
            1.0,
        );
        record(&ctx, DeviceEvent::MouseMotion { delta: (8.0, -4.0) }, 1.0);
        record(
            &ctx,
            DeviceEvent::Button {
                button: 0,
                state: ElementState::Pressed,
            },
            1.0,
        );
        let state = ctx.data(|data| data.get_temp::<Activity>(id()).unwrap());
        assert!(state.keys[KEYS.iter().position(|key| *key == 30).unwrap()]);
        assert!(state.direction.x > 0.0 && state.direction.y < 0.0);
        assert!(state.buttons[0]);
        clear(&ctx);
        assert!(ctx.data(|data| data.get_temp::<Activity>(id()).is_none()));
    }
}
