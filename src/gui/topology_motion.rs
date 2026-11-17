//! Finite reversible feedback envelopes; settled and hidden views need no clock.
use super::super::activity::Activity;
use eframe::egui;

const APPEAR: f64 = 0.18;
const DISAPPEAR: f64 = 0.55;

#[derive(Clone, Copy, Default)]
pub(super) struct Fade {
    progress: f32,
    shown: bool,
    updated: Option<f64>,
}
impl Fade {
    pub fn update_until(&mut self, until: f64, now: f64) -> f32 {
        if self.shown && until < now && self.updated.is_some_and(|previous| previous < until) {
            self.update(false, until);
        }
        self.update(now < until, now)
    }
    pub fn update(&mut self, shown: bool, now: f64) -> f32 {
        if let Some(previous) = self.updated {
            let elapsed = (now - previous).max(0.0);
            let step = elapsed / if self.shown { APPEAR } else { DISAPPEAR };
            self.progress = (self.progress
                + if self.shown {
                    step as f32
                } else {
                    -step as f32
                })
            .clamp(0.0, 1.0);
        }
        self.updated = Some(now);
        self.shown = shown;
        self.opacity()
    }
    pub fn opacity(&self) -> f32 {
        self.progress * self.progress * (3.0 - 2.0 * self.progress)
    }
    pub fn pending(&self) -> bool {
        if self.shown {
            self.progress < 1.0
        } else {
            self.progress > 0.0
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Phase {
    pub time: f64,
    pub opacity: f32,
}

pub(super) struct Frame {
    pub opacity: f32,
    pub ports: [f32; 5],
    pub keys: [f32; 66],
    pub buttons: [f32; 3],
    pub wheel: f32,
    pub animating: bool,
}
#[derive(Clone)]
struct Feedback {
    scene: Fade,
    ports: [Fade; 5],
    keys: [Fade; 66],
    buttons: [Fade; 3],
    wheel: Fade,
    updated: Option<f64>,
}
impl Default for Feedback {
    fn default() -> Self {
        Self {
            scene: Fade::default(),
            ports: [Fade::default(); 5],
            keys: [Fade::default(); 66],
            buttons: [Fade::default(); 3],
            wheel: Fade::default(),
            updated: None,
        }
    }
}

pub(super) fn read(
    ui: &egui::Ui,
    now: f64,
    active_until: f64,
    ports: [bool; 5],
    activity: &Activity,
    enabled: bool,
) -> Frame {
    let id = ui
        .make_persistent_id("activity-envelopes")
        .with(ui.ctx().viewport_id());
    ui.ctx().data_mut(|data| {
        let state = data.get_temp_mut_or_default::<Feedback>(id);
        if !enabled {
            *state = Feedback::default();
        } else if state.updated.is_some_and(|previous| now - previous > 1.2) {
            // Drop expired motion after a hidden interval, but keep settled
            // held-key/button highlights intact across ordinary idle repaints.
            state.scene = Fade::default();
            state.ports = [Fade::default(); 5];
            state.wheel = Fade::default();
            for (fade, down) in state.keys.iter_mut().zip(activity.keys) {
                if !down {
                    *fade = Fade::default();
                }
            }
            for (fade, down) in state.buttons.iter_mut().zip(activity.buttons) {
                if !down {
                    *fade = Fade::default();
                }
            }
        }
        state.updated = Some(now);
        if !enabled {
            return Frame {
                opacity: 0.0,
                ports: [0.0; 5],
                keys: activity.keys.map(f32::from),
                buttons: activity.buttons.map(f32::from),
                wheel: 0.0,
                animating: false,
            };
        }
        let opacity = state.scene.update_until(active_until, now);
        let ports = std::array::from_fn(|i| {
            if ports[i] {
                state.ports[i].update_until(active_until, now)
            } else {
                state.ports[i].update(false, now)
            }
        });
        let keys = std::array::from_fn(|i| {
            if activity.keys[i] {
                state.keys[i].update(true, now)
            } else {
                state.keys[i].update_until(activity.released[i] + 0.12, now)
            }
        });
        let buttons = std::array::from_fn(|i| {
            if activity.buttons[i] {
                state.buttons[i].update(true, now)
            } else {
                state.buttons[i].update_until(activity.clicked[i] + 0.12, now)
            }
        });
        let wheel = state.wheel.update_until(activity.wheel + 0.12, now);
        let pending = state.scene.pending()
            || state.wheel.pending()
            || state.ports.iter().any(Fade::pending)
            || state.keys.iter().any(Fade::pending)
            || state.buttons.iter().any(Fade::pending);
        Frame {
            opacity,
            ports,
            keys,
            buttons,
            wheel,
            animating: pending || opacity > 0.0 || ports.iter().any(|opacity| *opacity > 0.0),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        ctx: &egui::Context,
        now: f64,
        until: f64,
        activity: &Activity,
        enabled: bool,
    ) -> Frame {
        let mut result = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                time: Some(now),
                ..Default::default()
            },
            |ui| result = Some(read(ui, now, until, [true; 5], activity, enabled)),
        );
        result.unwrap()
    }

    #[test]
    fn activity_reverses_from_its_current_opacity_and_eventually_stops_the_clock() {
        let ctx = egui::Context::default();
        let activity = Activity::default();
        assert_eq!(frame(&ctx, 0.0, 0.3, &activity, true).opacity, 0.0);
        assert_eq!(frame(&ctx, 0.2, 0.3, &activity, true).opacity, 1.0);
        frame(&ctx, 0.3, 0.3, &activity, true);
        let fading = frame(&ctx, 0.5, 0.3, &activity, true);
        assert!(fading.opacity > 0.0 && fading.opacity < 1.0);
        let reversed = frame(&ctx, 0.5, 0.9, &activity, true);
        assert_eq!(reversed.opacity, fading.opacity);
        assert_eq!(reversed.ports, fading.ports);
        assert!(frame(&ctx, 0.55, 0.9, &activity, true).opacity > fading.opacity);
        for step in 12..=32 {
            frame(&ctx, step as f64 * 0.05, 0.9, &activity, true);
        }
        let idle = frame(&ctx, 1.65, 0.9, &activity, true);
        assert_eq!(idle.opacity, 0.0);
        assert_eq!(idle.ports, [0.0; 5]);
        assert!(!idle.animating);
    }

    #[test]
    fn held_indicators_stay_static_across_idle_repaints_and_release_smoothly() {
        let ctx = egui::Context::default();
        let mut activity = Activity::default();
        activity.keys[0] = true;
        activity.buttons[0] = true;
        frame(&ctx, 0.0, -1.0, &activity, true);
        let held = frame(&ctx, 0.2, -1.0, &activity, true);
        assert_eq!(held.keys[0], 1.0);
        assert_eq!(held.buttons[0], 1.0);
        assert!(!held.animating);
        let held = frame(&ctx, 60.0, -1.0, &activity, true);
        assert_eq!(held.keys[0], 1.0);
        assert_eq!(held.buttons[0], 1.0);
        assert!(!held.animating);
        activity.keys[0] = false;
        activity.buttons[0] = false;
        activity.released[0] = 60.0;
        let release = frame(&ctx, 60.0, -1.0, &activity, true);
        assert_eq!(release.keys[0], 1.0);
        assert_eq!(release.buttons[0], 1.0);
        let fading = frame(&ctx, 60.25, -1.0, &activity, true);
        assert!(fading.keys[0] > 0.0 && fading.keys[0] < 1.0);
        assert!(fading.buttons[0] > 0.0 && fading.buttons[0] < 1.0);
        assert!(fading.animating);
        let done = frame(&ctx, 60.8, -1.0, &activity, true);
        assert_eq!(done.keys[0], 0.0);
        assert_eq!(done.buttons[0], 0.0);
        assert!(!done.animating);
    }

    #[test]
    fn hidden_and_reduced_motion_feedback_cancels_the_animation_clock() {
        let ctx = egui::Context::default();
        let activity = Activity::default();
        frame(&ctx, 0.0, 0.5, &activity, true);
        assert!(frame(&ctx, 0.2, 0.5, &activity, true).animating);
        let disabled = frame(&ctx, 0.25, 0.5, &activity, false);
        assert_eq!(disabled.opacity, 0.0);
        assert!(!disabled.animating);
        let restored = frame(&ctx, 60.0, 0.5, &activity, true);
        assert_eq!(restored.opacity, 0.0);
        assert!(!restored.animating);
    }
    #[test]
    fn an_expired_activity_deadline_finishes_even_if_a_frame_was_delayed() {
        let mut fade = Fade::default();
        fade.update_until(0.42, 0.0);
        assert_eq!(fade.update_until(0.42, 0.3), 1.0);
        assert_eq!(fade.update_until(0.42, 2.0), 0.0);
        assert!(!fade.pending());
    }
    #[test]
    fn reversing_a_fade_preserves_the_visible_value_and_changes_direction() {
        let mut fade = Fade::default();
        assert_eq!(fade.update(true, 10.0), 0.0);
        let partial = fade.update(true, 10.1);
        assert!(partial > 0.0 && partial < 1.0);
        assert_eq!(fade.update(false, 10.1), partial);
        let fading = fade.update(false, 10.2);
        assert!(fading > 0.0 && fading < partial);
        assert_eq!(fade.update(true, 10.2), fading);
        assert!(fade.update(true, 10.22) > fading);
        assert_eq!(fade.update(true, 11.0), 1.0);
        assert!(!fade.pending());
        assert_eq!(fade.update(false, 11.0), 1.0);
        assert_eq!(fade.update(false, 11.6), 0.0);
        assert!(!fade.pending());
    }
    #[test]
    fn a_new_event_after_a_long_idle_starts_at_zero_instead_of_jumping_to_full() {
        let mut fade = Fade::default();
        fade.update(false, 0.0);
        assert_eq!(fade.update(true, 3600.0), 0.0);
        assert!(fade.update(true, 3600.06) < 0.5);
    }
}
