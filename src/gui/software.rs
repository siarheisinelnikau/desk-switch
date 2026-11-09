//! Reactive egui host: CPU rasterization, retained bitmaps, Windows GDI presentation.
use eframe::egui;
use egui::{Context, ViewportBuilder, ViewportCommand, ViewportId};
use egui_software_backend::{BufferMutRef, ColorFieldOrder, EguiSoftwareRender};
use std::{
    collections::HashMap,
    num::NonZeroU32,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, OwnedDisplayHandle},
    window::{Window, WindowId},
};

#[cfg(test)]
mod smoke;

pub struct CreationContext {
    pub egui_ctx: Context,
}

pub trait App {
    fn logic(&mut self, ctx: &Context);
    fn ui(&mut self, ui: &mut egui::Ui);
    fn shutdown(&mut self) {}
    fn raw_input_hook(&mut self, _: &Context, _: &mut egui::RawInput) {}
}

struct Wake {
    id: ViewportId,
    when: Instant,
    pass: u64,
}

struct View {
    surface: softbuffer::Surface<OwnedDisplayHandle, Arc<Window>>,
    _display: softbuffer::Context<OwnedDisplayHandle>,
    window: Arc<Window>,
    input: egui_winit::State,
    info: egui::ViewportInfo,
    builder: ViewportBuilder,
    callback: Option<Arc<egui::DeferredViewportUiCallback>>,
    desired_visible: bool,
    ui_while_hidden: bool,
    next: Option<Instant>,
    last_ui: Option<Instant>,
    frame_interval: Duration,
    pixels: Vec<u32>,
    size: [u32; 2],
    primitives: Vec<egui::ClippedPrimitive>,
    pixels_per_point: f32,
    texture_epoch: u64,
    screenshots: Vec<egui::UserData>,
    focus_after_paint: bool,
    close_cancelled: bool,
    needs_present: bool,
    stats: Stats,
}

#[derive(Default, serde::Serialize)]
struct Stats {
    context_passes: u64,
    ui_passes: u64,
    logic_passes: u64,
    rasterized: u64,
    unchanged: u64,
    retained_presentations: u64,
    hidden_paints: u64,
    event_counts: HashMap<&'static str, u64>,
    repaint_causes: HashMap<String, u64>,
    changed_reasons: HashMap<&'static str, u64>,
    #[serde(skip)]
    times_ms: Vec<f64>,
}

type Creator = Box<dyn FnOnce(&CreationContext) -> Result<Box<dyn App>, String>>;

struct Host {
    ctx: Context,
    creator: Option<Creator>,
    app: Option<Box<dyn App>>,
    root_builder: ViewportBuilder,
    views: HashMap<ViewportId, View>,
    renderer: EguiSoftwareRender,
    textures: egui::TexturesDelta,
    texture_epoch: u64,
    started: Instant,
    error: Option<String>,
    trace: Option<std::path::PathBuf>,
    retired_views: Vec<serde_json::Value>,
    #[cfg(test)]
    probe: Option<smoke::Probe>,
}

impl Host {
    fn schedule(&mut self, id: ViewportId, when: Instant) {
        if let Some(view) = self.views.get_mut(&id) {
            // Coalesce pointer/repaint bursts to the actual display's refresh period.
            // Hidden logic deadlines and quiet timers are not turned into a render loop.
            let when = if view.desired_visible {
                view.last_ui
                    .map_or(when, |last| when.max(last + view.frame_interval))
            } else {
                when
            };
            view.next = Some(view.next.map_or(when, |old| old.min(when)));
        }
    }

    fn create_view(
        &mut self,
        event_loop: &ActiveEventLoop,
        id: ViewportId,
        parent: ViewportId,
        mut builder: ViewportBuilder,
        callback: Option<Arc<egui::DeferredViewportUiCallback>>,
    ) -> Result<(), String> {
        let desired_visible = builder.visible.unwrap_or(true);
        let focus_after_paint = builder.active.unwrap_or(true);
        builder.visible = Some(false); // Paint before the first show; never flash an empty bitmap.
        let window = Arc::new(
            egui_winit::create_window(&self.ctx, event_loop, &builder)
                .map_err(|e| e.to_string())?,
        );
        set_window_icons(&window)?;
        builder.visible = Some(desired_visible);
        let display = softbuffer::Context::new(event_loop.owned_display_handle())
            .map_err(|e| format!("Could not create the GDI context: {e}"))?;
        let surface = softbuffer::Surface::new(&display, window.clone())
            .map_err(|e| format!("Could not create the GDI surface: {e}"))?;
        let input = egui_winit::State::new(
            self.ctx.clone(),
            id,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        let info = egui::ViewportInfo {
            parent: (id != ViewportId::ROOT).then_some(parent),
            ..Default::default()
        };
        let frame_interval = refresh_interval(&window);
        self.views.insert(
            id,
            View {
                surface,
                _display: display,
                window,
                input,
                info,
                builder,
                callback,
                desired_visible,
                ui_while_hidden: true,
                next: Some(Instant::now()),
                last_ui: None,
                frame_interval,
                pixels: Vec::new(),
                size: [0, 0],
                primitives: Vec::new(),
                pixels_per_point: 0.0,
                texture_epoch: 0,
                screenshots: Vec::new(),
                focus_after_paint,
                close_cancelled: false,
                needs_present: true,
                stats: Stats::default(),
            },
        );
        Ok(())
    }

    fn commands(
        &mut self,
        _event_loop: &ActiveEventLoop,
        id: ViewportId,
        commands: Vec<ViewportCommand>,
    ) -> Result<(), String> {
        let Some(view) = self.views.get_mut(&id) else {
            return Ok(());
        };
        let cancel_close = commands.contains(&ViewportCommand::CancelClose);
        let mut ordinary = Vec::new();
        for command in commands {
            match command {
                ViewportCommand::Close if !cancel_close => {
                    view.info.events.push(egui::ViewportEvent::Close);
                    view.ui_while_hidden = true;
                    view.next = Some(Instant::now());
                }
                ViewportCommand::CancelClose => view.close_cancelled = true,
                ViewportCommand::Close => {}
                ViewportCommand::Visible(visible) => {
                    view.builder.visible = Some(visible);
                    if view.desired_visible != visible {
                        view.desired_visible = visible;
                        if visible {
                            view.next = Some(Instant::now());
                        } else {
                            view.window.set_visible(false);
                        }
                    }
                }
                ViewportCommand::Focus => {
                    view.focus_after_paint = true;
                }
                ViewportCommand::Minimized(false) => {
                    if view.window.is_minimized() == Some(true) {
                        // Keep a valid retained bitmap across the native restore.
                        // SW_RESTORE can reveal the window before the next UI pass.
                        if !view.pixels.is_empty() && view.size != [0, 0] {
                            present(view)?;
                            view.stats.retained_presentations += 1;
                        }
                        view.window.set_minimized(false);
                        view.next = Some(Instant::now());
                    }
                }
                command => ordinary.push(command),
            }
        }
        let mut actions = Vec::new();
        egui_winit::process_viewport_commands(
            &self.ctx,
            &mut view.info,
            ordinary,
            &view.window,
            &mut actions,
        );
        for action in actions {
            match action {
                egui_winit::ActionRequested::Screenshot(callback) => {
                    view.screenshots.push(callback)
                }
                egui_winit::ActionRequested::Copy => {
                    view.input.egui_input_mut().events.push(egui::Event::Copy)
                }
                egui_winit::ActionRequested::Cut => {
                    view.input.egui_input_mut().events.push(egui::Event::Cut)
                }
                egui_winit::ActionRequested::Paste => {
                    if let Some(text) = view.input.clipboard_text() {
                        view.input
                            .egui_input_mut()
                            .events
                            .push(egui::Event::Paste(text));
                    }
                }
            }
        }
        Ok(())
    }

    fn viewport_output(
        &mut self,
        event_loop: &ActiveEventLoop,
        source: ViewportId,
        output: egui::OrderedViewportIdMap<egui::ViewportOutput>,
    ) -> Result<(), String> {
        if source == ViewportId::ROOT {
            // Retire child bitmaps and windows only on their parent's frame.
            // GDI has no shared GPU surface that can invalidate the root here.
            let retired = self
                .views
                .keys()
                .filter(|id| **id != ViewportId::ROOT && !output.contains_key(id))
                .copied()
                .collect::<Vec<_>>();
            for id in retired {
                if let Some(mut view) = self.views.remove(&id)
                    && self.trace.is_some()
                {
                    if self.retired_views.len() == 64 {
                        self.retired_views.remove(0);
                    }
                    self.retired_views.push(view_report(&mut view));
                }
            }
        }
        for (id, output) in output {
            if !self.views.contains_key(&id) {
                // A late command to a closed child is not a window declaration.
                // Deferred children need a callback before they can be displayed.
                if id != ViewportId::ROOT && output.viewport_ui_cb.is_none() {
                    continue;
                }
                self.create_view(
                    event_loop,
                    id,
                    output.parent,
                    output.builder.clone(),
                    output.viewport_ui_cb.clone(),
                )?;
            }
            let view = self.views.get_mut(&id).unwrap();
            if output.viewport_ui_cb.is_some() || id == ViewportId::ROOT {
                view.callback = output.viewport_ui_cb;
            }
            let (mut commands, _) = view.builder.patch(output.builder);
            commands.extend(output.commands);
            self.commands(event_loop, id, commands)?;
            if output.repaint_delay != Duration::MAX
                && let Some(when) = Instant::now().checked_add(output.repaint_delay)
            {
                self.schedule(id, when);
            }
        }
        Ok(())
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop, id: ViewportId) -> Result<(), String> {
        let started = Instant::now();
        for view in self.views.values_mut() {
            egui_winit::update_viewport_info(&mut view.info, &self.ctx, &view.window, false);
        }
        let infos = self
            .views
            .iter()
            .map(|(id, view)| (*id, view.info.clone()))
            .collect();
        let view = self.views.get_mut(&id).unwrap();
        view.next = None;
        let mut raw = view.input.take_egui_input(&view.window);
        raw.time = Some(self.started.elapsed().as_secs_f64());
        raw.viewports = infos;
        view.info.events.clear();
        view.close_cancelled = false;
        let view = self.views.get_mut(&id).unwrap();
        let close_requested = raw.viewport().close_requested();
        let visible = view.desired_visible && view.window.is_minimized() != Some(true);
        let draw = visible || (id != ViewportId::ROOT && (view.ui_while_hidden || close_requested));
        view.ui_while_hidden = false;
        view.last_ui = Some(Instant::now());
        let callback = view.callback.clone();
        self.app
            .as_mut()
            .unwrap()
            .raw_input_hook(&self.ctx, &mut raw);
        let mut context_passes = 0;
        let mut logic_passes = 0;
        let mut ui_passes = 0;
        let output = self.ctx.run_ui(raw, |ui| {
            context_passes += 1;
            if id == ViewportId::ROOT {
                logic_passes += 1;
                self.app.as_mut().unwrap().logic(ui.ctx());
                if draw {
                    ui_passes += 1;
                    self.app.as_mut().unwrap().ui(ui);
                }
            } else if draw && let Some(callback) = &callback {
                ui_passes += 1;
                callback(ui);
            }
        });
        let view = self.views.get_mut(&id).unwrap();
        view.stats.context_passes += context_passes;
        view.stats.logic_passes += logic_passes;
        view.stats.ui_passes += ui_passes;
        view.input
            .handle_platform_output(&view.window, output.platform_output);
        if !output.textures_delta.is_empty() {
            self.texture_epoch += 1;
        }
        self.textures.append(output.textures_delta);
        self.viewport_output(event_loop, id, output.viewport_output)?;
        if id == ViewportId::ROOT {
            let active = self.ctx.viewport_id();
            debug_assert_eq!(active, ViewportId::ROOT);
        }
        if event_loop.exiting() {
            return Ok(());
        }
        let view = self.views.get_mut(&id).unwrap();
        if !draw && view.desired_visible {
            // A hidden logic pass may process Show. Paint the actual UI on the
            // next pass before revealing the window; never present empty shapes.
            if view.window.is_minimized() != Some(true) {
                view.next = Some(Instant::now());
            }
            return Ok(());
        }
        if close_requested && !view.close_cancelled {
            if id == ViewportId::ROOT {
                event_loop.exit();
                return Ok(());
            }
            view.desired_visible = false;
            view.window.set_visible(false);
        }
        if (!view.desired_visible || view.window.is_minimized() == Some(true))
            && view.screenshots.is_empty()
        {
            return Ok(()); // Hidden viewports never rasterize; texture uploads remain pending.
        }
        let size = view.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let dimensions = [size.width, size.height];
        let changed = view.size != dimensions
            || view.pixels_per_point != output.pixels_per_point
            || view.texture_epoch != self.texture_epoch
            || !same_scene(&view.primitives, &primitives);
        if changed {
            if self.trace.is_some() {
                let reason = if view.size != dimensions {
                    "size"
                } else if view.pixels_per_point != output.pixels_per_point {
                    "scale"
                } else if view.texture_epoch != self.texture_epoch {
                    "texture"
                } else {
                    "scene"
                };
                *view.stats.changed_reasons.entry(reason).or_default() += 1;
            }
            view.pixels
                .resize(size.width as usize * size.height as usize, 0);
            view.pixels.fill(0xff17_181b);
            self.renderer.render(
                &mut BufferMutRef::new(
                    bytemuck::cast_slice_mut(&mut view.pixels),
                    size.width as usize,
                    size.height as usize,
                ),
                &primitives,
                &self.textures,
                output.pixels_per_point,
            );
            self.textures.clear();
            view.primitives = primitives;
            view.size = dimensions;
            view.pixels_per_point = output.pixels_per_point;
            view.texture_epoch = self.texture_epoch;
            view.stats.rasterized += 1;
            if !view.desired_visible {
                view.stats.hidden_paints += 1;
            }
            present(view)?;
        } else {
            view.stats.unchanged += 1;
            if view.needs_present {
                view.stats.retained_presentations += 1;
                present(view)?;
            }
        }
        view.needs_present = false;
        if !view.screenshots.is_empty() {
            let pixels = view
                .pixels
                .iter()
                .map(|p| egui::Color32::from_rgb((p >> 16) as u8, (p >> 8) as u8, *p as u8))
                .collect();
            let image = Arc::new(egui::ColorImage::new(
                [size.width as usize, size.height as usize],
                pixels,
            ));
            for user_data in view.screenshots.drain(..) {
                view.input
                    .egui_input_mut()
                    .events
                    .push(egui::Event::Screenshot {
                        viewport_id: id,
                        user_data,
                        image: image.clone(),
                    });
            }
            view.next = Some(Instant::now());
        }
        if view.desired_visible {
            if view.window.is_visible() != Some(true) {
                view.window.set_visible(true);
            }
            if view.focus_after_paint {
                view.window.focus_window();
                view.focus_after_paint = false;
            }
        }
        if self.trace.is_some() {
            for cause in self.ctx.repaint_causes() {
                *view
                    .stats
                    .repaint_causes
                    .entry(cause.to_string())
                    .or_default() += 1;
            }
            view.stats
                .times_ms
                .push(started.elapsed().as_secs_f64() * 1000.0);
        }
        Ok(())
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: String) {
        self.error = Some(error);
        event_loop.exit();
    }

    fn write_trace(&mut self) {
        let Some(path) = &self.trace else {
            return;
        };
        let views: Vec<_> = self.views.values_mut().map(view_report).collect();
        let report = serde_json::json!({"renderer":"CPU / Windows GDI", "version":crate::compatibility::VERSION,
            "seconds":self.started.elapsed().as_secs_f64(), "views":views, "retired_views": self.retired_views});
        if let Ok(bytes) = serde_json::to_vec_pretty(&report) {
            let _ = std::fs::write(path, bytes);
        }
    }
}

fn view_report(view: &mut View) -> serde_json::Value {
    view.stats.times_ms.sort_by(f64::total_cmp);
    let n = view.stats.times_ms.len();
    serde_json::json!({"title": view.window.title(), "counters": view.stats,
        "frame_median_ms": view.stats.times_ms.get(n / 2),
        "frame_p95_ms": view.stats.times_ms.get(n.saturating_sub(1) * 95 / 100),
        "frame_max_ms": view.stats.times_ms.last()})
}

fn refresh_interval(window: &Window) -> Duration {
    let refresh = window
        .current_monitor()
        .and_then(|monitor| monitor.refresh_rate_millihertz())
        .filter(|rate| *rate > 0)
        .unwrap_or(60_000);
    Duration::from_nanos(1_000_000_000_000 / u64::from(refresh))
}

fn set_window_icons(window: &Window) -> Result<(), String> {
    use windows_sys::Win32::UI::{
        HiDpi::GetSystemMetricsForDpi,
        WindowsAndMessaging::{SM_CXICON, SM_CXSMICON},
    };
    use winit::platform::windows::WindowExtWindows;
    let dpi = (window.scale_factor() * 96.0).round().clamp(96.0, 768.0) as u32;
    let icon = |metric| {
        // SAFETY: the call only reads a system metric for the given DPI and
        // touches no memory.
        let size = unsafe { GetSystemMetricsForDpi(metric, dpi) }.clamp(16, 256) as u32;
        winit::window::Icon::from_rgba(icon_pixels(size), size, size)
            .map_err(|error| error.to_string())
    };
    window.set_window_icon(Some(icon(SM_CXSMICON)?));
    // winit's generic window icon assigns ICON_SMALL; the taskbar needs ICON_BIG.
    window.set_taskbar_icon(Some(icon(SM_CXICON)?));
    Ok(())
}

fn icon_pixels(side: u32) -> Vec<u8> {
    if [16, 20, 24, 32, 48, 64].contains(&side) {
        return super::tray::icon_rgba(side as usize);
    }
    // Windows also requests 40/80px icons at common display scales. The source
    // icon cache only has discrete variants; do not index it with an arbitrary metric.
    let source =
        image::RgbaImage::from_raw(64, 64, super::tray::icon_rgba(64)).expect("valid icon source");
    image::imageops::resize(&source, side, side, image::imageops::FilterType::Lanczos3).into_raw()
}

fn same_scene(a: &[egui::ClippedPrimitive], b: &[egui::ClippedPrimitive]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.clip_rect == b.clip_rect
                && match (&a.primitive, &b.primitive) {
                    (egui::epaint::Primitive::Mesh(a), egui::epaint::Primitive::Mesh(b)) => {
                        a.texture_id == b.texture_id
                            && a.vertices == b.vertices
                            && a.indices == b.indices
                    }
                    _ => false,
                }
        })
}

fn present(view: &mut View) -> Result<(), String> {
    let [width, height] = view.size;
    if let (Some(width), Some(height)) = (NonZeroU32::new(width), NonZeroU32::new(height)) {
        view.surface
            .resize(width, height)
            .map_err(|e| e.to_string())?;
        let mut buffer = view.surface.buffer_mut().map_err(|e| e.to_string())?;
        buffer.copy_from_slice(&view.pixels);
        buffer
            .present()
            .map_err(|e| format!("Could not present the GDI bitmap: {e}"))?;
    }
    Ok(())
}

impl ApplicationHandler<Wake> for Host {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.app.is_some() {
            return;
        }
        if let Err(error) = self.create_view(
            event_loop,
            ViewportId::ROOT,
            ViewportId::ROOT,
            self.root_builder.clone(),
            None,
        ) {
            self.fail(event_loop, error);
            return;
        }
        let cc = CreationContext {
            egui_ctx: self.ctx.clone(),
        };
        match self.creator.take().unwrap()(&cc) {
            Ok(app) => self.app = Some(app),
            Err(error) => self.fail(event_loop, error),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(id) = self
            .views
            .iter()
            .find_map(|(id, v)| (v.window.id() == window_id).then_some(*id))
        else {
            return;
        };
        let view = self.views.get_mut(&id).unwrap();
        if self.trace.is_some() {
            let category = match &event {
                WindowEvent::RedrawRequested => "expose",
                WindowEvent::CursorMoved { .. } => "pointer_move",
                WindowEvent::Focused(_) => "focus",
                WindowEvent::Resized(_) => "resize",
                WindowEvent::MouseInput { .. } => "pointer_button",
                WindowEvent::MouseWheel { .. } => "wheel",
                WindowEvent::Occluded(_) => "occlusion",
                _ => "other",
            };
            *view.stats.event_counts.entry(category).or_default() += 1;
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            let size = view.window.inner_size();
            let cached = !view.pixels.is_empty() && view.size == [size.width, size.height];
            let due = view.next.is_some_and(|next| next <= Instant::now());
            if cached && !due {
                if view.desired_visible && view.window.is_minimized() != Some(true) {
                    view.stats.retained_presentations += 1;
                    if let Err(error) = present(view) {
                        self.fail(event_loop, error);
                    }
                }
                return;
            }
            view.needs_present = true;
        }
        if matches!(
            event,
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }
        ) {
            // Keep the last painted dimensions and bitmap through minimization.
            // The actual inner size/scale comparisons invalidate changed geometry.
            view.needs_present = true;
            view.frame_interval = refresh_interval(&view.window);
        }
        if matches!(event, WindowEvent::ScaleFactorChanged { .. })
            && let Err(error) = set_window_icons(&view.window)
        {
            self.fail(event_loop, error);
            return;
        }
        if matches!(event, WindowEvent::Moved(_)) {
            view.frame_interval = refresh_interval(&view.window);
        }
        if matches!(event, WindowEvent::CloseRequested) {
            view.info.events.push(egui::ViewportEvent::Close);
        }
        let response = view.input.on_window_event(&view.window, &event);
        if response.repaint
            || matches!(
                event,
                WindowEvent::CloseRequested | WindowEvent::RedrawRequested
            )
        {
            self.schedule(id, Instant::now());
        }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, wake: Wake) {
        let pass = self.ctx.cumulative_pass_nr_for(wake.id);
        if wake.pass >= pass.saturating_sub(1) {
            self.schedule(wake.id, wake.when);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(test)]
        if let Some(mut probe) = self.probe.take() {
            probe.tick(self, event_loop);
            self.probe = Some(probe);
            if event_loop.exiting() {
                return;
            }
        }
        let now = Instant::now();
        let due: Vec<_> = self
            .views
            .iter()
            .filter_map(|(id, v)| v.next.filter(|t| *t <= now).map(|_| *id))
            .collect();
        for id in due {
            if !self.views.contains_key(&id) {
                continue;
            }
            if let Err(error) = self.frame(event_loop, id) {
                self.fail(event_loop, error);
                return;
            }
            if event_loop.exiting() {
                return;
            }
        }
        let next = self.views.values().filter_map(|v| v.next).min();
        #[cfg(test)]
        let next = next
            .into_iter()
            .chain(self.probe.as_ref().map(smoke::Probe::next))
            .min();
        event_loop.set_control_flow(next.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        // Pending hidden-only uploads are no longer needed once the application exits.
        self.textures.clear();
        self.ctx.set_request_repaint_callback(|_| {});
        if let Some(app) = &mut self.app {
            app.shutdown();
        }
        self.write_trace();
    }
}

pub fn run(
    builder: ViewportBuilder,
    creator: impl FnOnce(&CreationContext) -> Result<Box<dyn App>, String> + 'static,
) -> Result<(), String> {
    #[cfg(test)]
    return run_host(builder, Box::new(creator), None);
    #[cfg(not(test))]
    run_host(builder, Box::new(creator))
}

fn run_host(
    builder: ViewportBuilder,
    creator: Creator,
    #[cfg(test)] probe: Option<smoke::Probe>,
) -> Result<(), String> {
    let mut event_loop_builder = EventLoop::<Wake>::with_user_event();
    #[cfg(test)]
    {
        use winit::platform::windows::EventLoopBuilderExtWindows;
        event_loop_builder.with_any_thread(true);
    }
    let event_loop = event_loop_builder.build().map_err(|e| e.to_string())?;
    let ctx = Context::default();
    ctx.set_embed_viewports(false);
    let proxy = event_loop.create_proxy();
    ctx.set_request_repaint_callback(move |info| {
        if let Some(when) = Instant::now().checked_add(info.delay) {
            let _ = proxy.send_event(Wake {
                id: info.viewport_id,
                when,
                pass: info.current_cumulative_pass_nr,
            });
        }
    });
    let trace = std::env::var_os("DESK_SWITCH_RENDER_TRACE").map(Into::into);
    #[cfg(test)]
    let trace = trace.or_else(|| probe.as_ref().map(smoke::Probe::trace));
    let mut host = Host {
        ctx,
        creator: Some(creator),
        app: None,
        root_builder: builder,
        views: HashMap::new(),
        renderer: EguiSoftwareRender::new(ColorFieldOrder::Bgra),
        textures: Default::default(),
        texture_epoch: 0,
        started: Instant::now(),
        error: None,
        trace,
        retired_views: Vec::new(),
        #[cfg(test)]
        probe,
    };
    event_loop.run_app(&mut host).map_err(|e| e.to_string())?;
    host.error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh() -> egui::ClippedPrimitive {
        let mut mesh = egui::Mesh::default();
        mesh.add_colored_rect(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(20.0, 20.0)),
            egui::Color32::WHITE,
        );
        egui::ClippedPrimitive {
            clip_rect: egui::Rect::EVERYTHING,
            primitive: egui::epaint::Primitive::Mesh(mesh),
        }
    }

    #[test]
    fn retained_scene_changes_when_geometry_colors_or_clipping_change() {
        let first = vec![mesh()];
        assert!(same_scene(&first, &first.clone()));
        let mut changed = first.clone();
        changed[0].clip_rect.max.x = 5.0;
        assert!(!same_scene(&first, &changed));
        changed = first.clone();
        if let egui::epaint::Primitive::Mesh(mesh) = &mut changed[0].primitive {
            mesh.vertices[0].color = egui::Color32::RED;
        }
        assert!(!same_scene(&first, &changed));
        assert!(!same_scene(&first, &[]));
    }

    #[test]
    fn window_icons_support_fractional_scale_metrics_without_cache_panics() {
        for side in [16, 20, 24, 32, 40, 48, 64, 80, 96, 128] {
            let rgba = icon_pixels(side);
            assert_eq!(rgba.len(), side as usize * side as usize * 4);
            assert!(rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0));
            winit::window::Icon::from_rgba(rgba, side, side).unwrap();
        }
    }

    #[test]
    fn font_uploads_from_hidden_passes_are_available_on_the_first_visible_paint() {
        let ctx = Context::default();
        let mut first = ctx.run_ui(Default::default(), |ui| {
            ui.label("Retained fonts");
        });
        let mut pending = egui::TexturesDelta::default();
        pending.append(std::mem::take(&mut first.textures_delta));
        assert!(!pending.is_empty());
        let mut visible = ctx.run_ui(Default::default(), |ui| {
            ui.label("Retained fonts");
        });
        pending.append(std::mem::take(&mut visible.textures_delta));
        let primitives = ctx.tessellate(visible.shapes, visible.pixels_per_point);
        let mut pixels = vec![[0, 0, 0, 255]; 256 * 128];
        let mut renderer = EguiSoftwareRender::new(ColorFieldOrder::Bgra);
        renderer.render(
            &mut BufferMutRef::new(&mut pixels, 256, 128),
            &primitives,
            &pending,
            visible.pixels_per_point,
        );
        pending.clear();
        assert!(pixels.iter().any(|p| p[..3] != [0, 0, 0]));
    }
}
