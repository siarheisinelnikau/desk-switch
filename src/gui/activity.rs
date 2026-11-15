//! Ephemeral physical input feedback. No text, history, file or network output.
use eframe::egui::{self, Vec2};

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
            wheel: -1000.0,
        }
    }
}
fn id() -> egui::Id {
    egui::Id::new("physical-device-activity")
}

#[cfg(windows)]
pub(super) fn begin_frame(ctx: &egui::Context, viewport: egui::ViewportId) {
    ctx.data_mut(|data| {
        data.remove::<bool>(id().with(viewport));
    });
}
#[cfg(windows)]
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

#[cfg(windows)]
pub(super) fn clear(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        data.remove::<Activity>(id());
    });
}

pub(super) fn read(ui: &egui::Ui) -> Activity {
    #[cfg(windows)]
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
    ui.ctx()
        .data(|data| data.get_temp::<Activity>(id()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
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
