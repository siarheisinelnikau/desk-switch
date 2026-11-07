//! Opt-in native host integration. Synthetic offscreen windows only; no device,
//! network, tray, clipboard, DDC or system input workers are started.
use super::*;
use std::{path::PathBuf, sync::Mutex};

fn child() -> ViewportId {
    ViewportId::from_hash_of("software-host-smoke-child")
}
fn stale() -> ViewportId {
    ViewportId::from_hash_of("software-host-smoke-closed")
}

#[derive(Default)]
struct Fixture {
    popup: bool,
    revision: u32,
    logic: u64,
}
struct SmokeApp {
    shared: Arc<Mutex<Fixture>>,
}
impl App for SmokeApp {
    fn logic(&mut self, ctx: &Context) {
        let mut state = self.shared.lock().unwrap();
        state.logic += 1;
        if state.popup {
            let revision = state.revision;
            ctx.show_viewport_deferred(
                child(),
                ViewportBuilder::default()
                    .with_title("Desk Switch host probe popup")
                    .with_inner_size([280.0, 180.0])
                    .with_position([-15_000.0, -15_000.0])
                    .with_active(false)
                    .with_decorations(false)
                    .with_taskbar(false),
                move |ui, _| {
                    draw(ui, "Popup content", revision, Color32::from_rgb(45, 31, 59));
                },
            );
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui) {
        let state = self.shared.lock().unwrap();
        draw(
            ui,
            "Main window content",
            state.revision,
            Color32::from_rgb(22, 31, 50),
        );
    }
}
fn draw(ui: &mut egui::Ui, title: &str, revision: u32, color: Color32) {
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(color).inner_margin(18))
        .show(ui, |ui| {
            ui.heading(title);
            ui.label(format!("Fresh native bitmap · revision {revision}"));
            ui.label("Latin, Кириллица, 12345");
            let _ = ui.button("Test control");
        });
}

use egui::Color32;
#[derive(Clone, Copy, serde::Serialize)]
struct Count {
    context: u64,
    ui: u64,
    raster: u64,
    present: u64,
}
impl Count {
    fn of(view: &View) -> Self {
        Self {
            context: view.stats.context_passes,
            ui: view.stats.ui_passes,
            raster: view.stats.rasterized,
            present: view.stats.rasterized + view.stats.retained_presentations,
        }
    }
    fn delta(self, before: Self) -> Self {
        Self {
            context: self.context - before.context,
            ui: self.ui - before.ui,
            raster: self.raster - before.raster,
            present: self.present - before.present,
        }
    }
}

pub(super) struct Probe {
    output: PathBuf,
    shared: Arc<Mutex<Fixture>>,
    completed: Arc<Mutex<bool>>,
    deadline: Instant,
    next: Instant,
    stage: u8,
    cycles: u8,
    before: HashMap<ViewportId, Count>,
    samples: Vec<serde_json::Value>,
}
impl Probe {
    pub(super) fn next(&self) -> Instant {
        self.next.min(self.deadline)
    }
    pub(super) fn trace(&self) -> PathBuf {
        self.output.join("native-host-trace.json")
    }
    fn after(&mut self, millis: u64) {
        self.next = Instant::now() + Duration::from_millis(millis);
    }
    fn baseline(&mut self, host: &Host) {
        self.before = host
            .views
            .iter()
            .map(|(id, view)| (*id, Count::of(view)))
            .collect();
    }
    fn sample(&mut self, host: &Host, name: &str) -> Result<(), String> {
        let mut views = Vec::new();
        for (id, before) in &self.before {
            let view = host
                .views
                .get(id)
                .ok_or_else(|| format!("{name}: viewport disappeared"))?;
            let change = Count::of(view).delta(*before);
            if change.context != 0 || change.ui != 0 || change.raster != 0 || change.present != 0 {
                return Err(format!(
                    "{name}: unchanged viewport kept rendering: {}",
                    serde_json::to_string(&change).unwrap()
                ));
            }
            views.push(serde_json::json!({"title":view.window.title(),"delta":change}));
        }
        self.samples
            .push(serde_json::json!({"state":name,"seconds":5,"views":views}));
        Ok(())
    }
    fn bitmap(&self, host: &Host, id: ViewportId, name: &str) -> Result<(), String> {
        let view = host
            .views
            .get(&id)
            .ok_or_else(|| format!("{name}: viewport missing"))?;
        if view.pixels.is_empty() || view.size.contains(&0) {
            return Err(format!("{name}: bitmap missing"));
        }
        let unique = view
            .pixels
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len();
        if unique < 32 {
            return Err(format!("{name}: blank/invalid bitmap ({unique} colors)"));
        }
        let rgba = view
            .pixels
            .iter()
            .flat_map(|pixel| [(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8, 255])
            .collect::<Vec<_>>();
        image::save_buffer(
            self.output.join(format!("{name}.png")),
            &rgba,
            view.size[0],
            view.size[1],
            image::ColorType::Rgba8,
        )
        .map_err(|error| error.to_string())
    }
    pub(super) fn tick(&mut self, host: &mut Host, event_loop: &ActiveEventLoop) {
        if Instant::now() >= self.deadline {
            host.fail(event_loop, "Native host smoke deadline exceeded".into());
            return;
        }
        if Instant::now() < self.next {
            return;
        }
        if let Err(error) = self.step(host, event_loop) {
            host.fail(event_loop, error);
        }
    }
    fn step(&mut self, host: &mut Host, event_loop: &ActiveEventLoop) -> Result<(), String> {
        match self.stage {
            0 => {
                if !host.views.contains_key(&child()) {
                    self.after(50);
                    return Ok(());
                }
                self.bitmap(host, ViewportId::ROOT, "both-main")?;
                self.bitmap(host, child(), "both-popup")?;
                if host
                    .views
                    .values()
                    .any(|view| view.window.is_visible() != Some(true))
                {
                    return Err("Initial painted windows were not shown".into());
                }
                self.baseline(host);
                self.after(5000);
            }
            1 => {
                self.sample(host, "visible-both-idle")?;
                host.ctx
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Visible(false));
                self.after(800);
            }
            2 => {
                if host.views[&ViewportId::ROOT].window.is_visible() != Some(false) {
                    return Err("Root did not hide".into());
                }
                self.baseline(host);
                self.after(5000);
            }
            3 => {
                self.sample(host, "hidden-main-idle")?;
                self.shared.lock().unwrap().revision += 1;
                let before = host.views[&ViewportId::ROOT].stats.ui_passes;
                self.before.clear();
                self.before
                    .insert(ViewportId::ROOT, Count::of(&host.views[&ViewportId::ROOT]));
                host.ctx.send_viewport_cmd_to(
                    ViewportId::ROOT,
                    ViewportCommand::Title("Desk Switch probe updated while hidden".into()),
                );
                self.samples
                    .push(serde_json::json!({"hidden_before_ui":before}));
                self.after(400);
            }
            4 => {
                let root = &host.views[&ViewportId::ROOT];
                let difference = Count::of(root).delta(self.before[&ViewportId::ROOT]);
                if root.window.title() != "Desk Switch probe updated while hidden"
                    || difference.ui != 0
                    || difference.raster != 0
                    || difference.context == 0
                {
                    return Err("Hidden dispatcher failed or constructed a full UI scene".into());
                }
                host.ctx
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Visible(true));
                host.ctx
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Minimized(false));
                self.after(800);
            }
            5 => {
                self.bitmap(host, ViewportId::ROOT, "restored-main")?;
                if host.views[&ViewportId::ROOT].window.is_visible() != Some(true) {
                    return Err("Root did not restore".into());
                }
                self.baseline(host);
                self.after(5000);
            }
            6 => {
                self.sample(host, "restored-both-idle")?;
                host.ctx
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Minimized(true));
                self.after(400);
            }
            7 => {
                if host.views[&ViewportId::ROOT].window.is_minimized() != Some(true) {
                    return Err("Root did not minimize".into());
                }
                self.baseline(host);
                self.stage = 20;
                self.after(5000);
            }
            8 => {
                if host.views[&ViewportId::ROOT].window.is_minimized() != Some(false) {
                    return Err("Root remained minimized".into());
                }
                self.bitmap(host, ViewportId::ROOT, "after-minimize-main")?;
                self.shared.lock().unwrap().popup = false;
                host.ctx.request_repaint_of(ViewportId::ROOT);
                self.after(300);
            }
            9 => {
                if host.views.len() != 1 {
                    return Err("Closed popup window was not retired".into());
                }
                host.ctx
                    .send_viewport_cmd_to(stale(), ViewportCommand::Visible(true));
                host.ctx
                    .send_viewport_cmd_to(stale(), ViewportCommand::Close);
                host.ctx.request_repaint_of(ViewportId::ROOT);
                self.after(300);
            }
            10 => {
                if host.views.len() != 1 {
                    return Err("Late child commands created an empty native window".into());
                }
                self.shared.lock().unwrap().popup = true;
                host.ctx.request_repaint_of(ViewportId::ROOT);
                self.after(400);
            }
            11 => {
                self.bitmap(host, child(), &format!("reopened-popup-{}", self.cycles))?;
                self.shared.lock().unwrap().popup = false;
                host.ctx.request_repaint_of(ViewportId::ROOT);
                self.after(300);
            }
            12 => {
                if host.views.len() != 1 {
                    return Err("Repeated popup close leaked a window".into());
                }
                self.cycles += 1;
                if self.cycles < 3 {
                    self.stage = 9;
                    self.after(50);
                } else {
                    host.ctx
                        .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Visible(false));
                    self.after(300);
                }
            }
            13 => {
                if host.views[&ViewportId::ROOT].window.is_visible() != Some(false) {
                    return Err("Final root hide failed".into());
                }
                let report = serde_json::json!({"renderer":"CPU rasterizer / softbuffer Windows GDI","native_windows":"offscreen, active=false","samples":self.samples,"reopen_cycles":self.cycles,"native_input":false,"services":false});
                std::fs::write(
                    self.output.join("native-host-smoke.json"),
                    serde_json::to_vec_pretty(&report).unwrap(),
                )
                .map_err(|error| error.to_string())?;
                *self.completed.lock().unwrap() = true;
                host.ctx
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
                self.after(1000);
            }
            21 => {
                self.sample(host, "minimized-main-idle")?;
                host.ctx
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Visible(true));
                host.ctx
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Minimized(false));
                self.stage = 7;
                self.after(800);
            }
            _ => {
                host.fail(event_loop, "Hidden Quit did not exit the host".into());
            }
        }
        self.stage += 1;
        Ok(())
    }
}

#[test]
#[ignore = "opens finite isolated native windows; run explicitly on Windows"]
fn native_two_window_lifecycle_stays_painted_and_sleeps_when_settled() {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        ".git/qa-native-host-{}-{suffix}",
        std::process::id()
    ));
    std::fs::create_dir_all(&output).unwrap();
    let shared = Arc::new(Mutex::new(Fixture {
        popup: true,
        ..Default::default()
    }));
    let completed = Arc::new(Mutex::new(false));
    let probe = Probe {
        output: output.clone(),
        shared: shared.clone(),
        completed: completed.clone(),
        deadline: Instant::now() + Duration::from_secs(40),
        next: Instant::now() + Duration::from_secs(1),
        stage: 0,
        cycles: 0,
        before: HashMap::new(),
        samples: Vec::new(),
    };
    let builder = ViewportBuilder::default()
        .with_title("Desk Switch host probe main")
        .with_inner_size([520.0, 320.0])
        .with_position([-16_000.0, -16_000.0])
        .with_active(false)
        .with_taskbar(false);
    let result = run_host(
        builder,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            cc.egui_ctx
                .style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.0);
            Ok(Box::new(SmokeApp { shared }))
        }),
        Some(probe),
    );
    println!("Native host artifacts: {}", output.display());
    result.unwrap();
    assert!(
        *completed.lock().unwrap(),
        "Host exited before completing the bounded probe"
    );
}
