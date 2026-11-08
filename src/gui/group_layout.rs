//! Workspace display placement. Editing changes only an in-memory draft.
use crate::{
    edge::{ScreenRect, Side},
    group::{
        layout::{self, DisplayTarget, Layout, Point, Scene, Surface},
        model::Workspace,
    },
};
use eframe::egui::{self, Align, Color32, FontId, Pos2, Rect, Stroke, Vec2};

pub(super) struct Apply {
    pub expected: u64,
    pub layout: Layout,
}

#[derive(Default)]
pub(super) struct Editor {
    open: bool,
    expected: u64,
    draft: Option<Layout>,
    selected: String,
    neighbour: String,
    drag: Option<Drag>,
    workspace_requested: bool,
    #[cfg(test)]
    actions: Option<[(egui::Id, Rect); 2]>,
}

struct Drag {
    component: String,
    pointer: Pos2,
    origin: Point,
    before: Layout,
    transform: Transform,
}

#[derive(Clone, Copy)]
struct Transform {
    origin: Pos2,
    scale: f32,
}
impl Transform {
    fn fit(canvas: Rect, scene: &Scene) -> Self {
        let world = scene
            .surfaces
            .iter()
            .filter_map(|surface| surface.rect.as_ref())
            .map(world_rect)
            .reduce(Rect::union)
            .unwrap_or_else(|| Rect::from_min_size(Pos2::ZERO, egui::vec2(1920.0, 1080.0)));
        let room = (canvas.size() - Vec2::splat(40.0)).max(Vec2::splat(1.0));
        let scale = (room.x / world.width().max(1.0))
            .min(room.y / world.height().max(1.0))
            .clamp(0.0001, 2.0);
        Self {
            origin: canvas.center() - world.center().to_vec2() * scale,
            scale,
        }
    }
    fn point(self, point: Pos2) -> Pos2 {
        self.origin + point.to_vec2() * self.scale
    }
    fn rect(self, rect: &ScreenRect) -> Rect {
        let rect = world_rect(rect);
        Rect::from_min_max(self.point(rect.min), self.point(rect.max))
    }
}

fn world_rect(rect: &ScreenRect) -> Rect {
    Rect::from_min_size(
        egui::pos2(rect.x as f32, rect.y as f32),
        egui::vec2(rect.width as f32, rect.height as f32),
    )
}
fn tr(text: &str) -> String {
    crate::i18n::tr(text)
}

impl Editor {
    pub fn open(&mut self, workspace: &Workspace) {
        self.open = true;
        self.expected = workspace.layout.revision;
        self.draft = Some(layout::draft(workspace));
        self.drag = None;
        self.workspace_requested = false;
        if let Some(draft) = &self.draft {
            let scene = layout::scene(workspace, draft);
            self.selected = scene
                .surfaces
                .iter()
                .find(|surface| surface.shared)
                .or_else(|| scene.surfaces.first())
                .map(|surface| surface.id.clone())
                .unwrap_or_default();
        }
    }
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn close(&mut self) {
        self.open = false;
        self.draft = None;
        self.drag = None;
    }

    pub fn take_workspace_request(&mut self) -> bool {
        std::mem::take(&mut self.workspace_requested)
    }

    #[cfg(debug_assertions)]
    pub(super) fn preview_select_computer(&mut self, workspace: &Workspace, computer: &str) {
        if let Some(draft) = &self.draft
            && let Some(surface) = layout::scene(workspace, draft)
                .surfaces
                .iter()
                .find(|surface| surface.computer.as_deref() == Some(computer))
        {
            self.selected = surface.id.clone();
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, workspace: &Workspace) -> Option<Apply> {
        if !self.open {
            return None;
        }
        let mut draft = self.draft.take()?;
        if !egui::Popup::is_any_open(ctx)
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            if let Some(drag) = self.drag.take() {
                draft = drag.before;
            } else {
                self.close();
                return None;
            }
        }
        let available = ctx.content_rect();
        let size = egui::vec2(880.0, 620.0).min(available.size() - Vec2::splat(32.0));
        let mut open = true;
        let mut apply = None;
        let mut cancel = false;
        egui::Window::new(tr("Display arrangement"))
            .id(egui::Id::new("workspace-display-arrangement"))
            .open(&mut open)
            .collapsible(false)
            .default_size(size)
            .default_pos(available.center() - size * 0.5)
            .min_size([560.0, 460.0])
            .max_size((available.size() - Vec2::splat(32.0)).max(egui::vec2(320.0, 240.0)))
            .show(ctx, |ui| {
                let stale = workspace.layout.revision != self.expected;
                if stale {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.label(tr("The arrangement changed on another computer. Reload before applying."));
                        if ui.button(tr("Reload arrangement")).clicked() {
                            draft = layout::draft(workspace);
                            self.expected = workspace.layout.revision;
                            self.drag = None;
                        }
                    });
                }
                // Place the actions first so a long display list or message can
                // never scroll Apply and Cancel outside the settings window.
                ui.with_layout(egui::Layout::bottom_up(Align::Min), |ui| {
                    let valid = layout::validate(workspace, &draft);
                    let changed = draft != workspace.layout;
                    ui.horizontal_wrapped(|ui| {
                        let apply_button = ui.add_enabled(
                            !stale && valid.is_ok() && changed && self.drag.is_none(),
                            egui::Button::new(tr("Apply arrangement")),
                        );
                        if apply_button.clicked() {
                            apply = Some(Apply { expected: self.expected, layout: draft.clone() });
                        }
                        let cancel_button = ui.button(tr("Cancel"));
                        if cancel_button.clicked() {
                            cancel = true;
                        }
                        #[cfg(test)]
                        { self.actions = Some([(apply_button.id, apply_button.rect), (cancel_button.id, cancel_button.rect)]); }
                    });
                    if let Err(error) = valid {
                        ui.colored_label(ui.visuals().error_fg_color, tr(&error.to_string()));
                    }
                    ui.separator();
                    ui.with_layout(egui::Layout::top_down(Align::Min), |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("display-arrangement-content")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.label(tr("Drag displays to match your desk. Displays on the same computer move together."));
                                ui.weak(tr("Touching edges let the pointer cross. Leave a gap to keep an edge closed."));
                                let scene = layout::scene(workspace, &draft);
                                self.ensure_selection(&scene);
                                self.canvas(ui, workspace, &mut draft, &scene, stale);
                                let scene = layout::scene(workspace, &draft);
                                self.unavailable(ui, workspace, &scene);
                                if scene.surfaces.iter().any(|surface| surface.shared && surface.computer.is_none()) {
                                    ui.colored_label(ui.visuals().warn_fg_color, tr("On the computer currently visible on the shared monitor, choose Switch here in the workspace before using its screen edges."));
                                    if ui.button(tr("Return to workspace")).clicked() {
                                        self.workspace_requested = true;
                                        cancel = true;
                                    }
                                }
                                self.selection(ui, workspace, &mut draft, &scene, stale);
                            });
                    });
                });
            });
        if !open || cancel || apply.is_some() {
            self.close();
        } else {
            self.draft = Some(draft);
        }
        apply
    }

    fn ensure_selection(&mut self, scene: &Scene) {
        if !scene
            .surfaces
            .iter()
            .any(|surface| surface.id == self.selected)
        {
            self.selected = scene
                .surfaces
                .first()
                .map(|surface| surface.id.clone())
                .unwrap_or_default();
        }
        let component = scene
            .surfaces
            .iter()
            .find(|surface| surface.id == self.selected)
            .map(|surface| surface.component.as_str());
        if !scene.components.iter().any(|candidate| {
            candidate.id == self.neighbour && Some(candidate.id.as_str()) != component
        }) {
            self.neighbour = scene
                .components
                .iter()
                .find(|candidate| {
                    Some(candidate.id.as_str()) != component && candidate.bounds.is_some()
                })
                .map(|candidate| candidate.id.clone())
                .unwrap_or_default();
        }
    }

    fn canvas(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        draft: &mut Layout,
        scene: &Scene,
        stale: bool,
    ) {
        let (canvas, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 260.0),
            egui::Sense::hover(),
        );
        let painter = ui.painter().with_clip_rect(canvas);
        painter.rect_filled(canvas, 8, ui.visuals().extreme_bg_color);
        painter.rect_stroke(
            canvas,
            8,
            ui.visuals().window_stroke,
            egui::StrokeKind::Inside,
        );
        let transform = self
            .drag
            .as_ref()
            .map(|drag| drag.transform)
            .unwrap_or_else(|| Transform::fit(canvas, scene));
        let mut finish_drag = false;
        for (index, surface) in scene.surfaces.iter().enumerate() {
            let Some(geometry) = &surface.rect else {
                continue;
            };
            let rect = transform.rect(geometry);
            let selected = surface.id == self.selected;
            let color = surface
                .computer
                .as_ref()
                .and_then(|key| workspace.members.keys().position(|member| member == key))
                .map(super::view::peer_color)
                .unwrap_or(ui.visuals().weak_text_color());
            let fill = if surface.online && surface.available {
                ui.visuals().window_fill()
            } else {
                ui.visuals().faint_bg_color
            };
            painter.rect_filled(rect, 5, fill);
            painter.rect_stroke(
                rect,
                5,
                Stroke::new(
                    if selected { 2.5 } else { 1.0 },
                    if selected {
                        ui.visuals().selection.bg_fill
                    } else {
                        color.gamma_multiply(0.7)
                    },
                ),
                egui::StrokeKind::Inside,
            );
            let stripe = Rect::from_min_size(
                rect.min + egui::vec2(4.0, 4.0),
                egui::vec2((rect.width() - 8.0).max(0.0), 3.0),
            );
            painter.rect_filled(stripe, 1, color);
            if rect.width() >= 32.0 && rect.height() >= 26.0 {
                let labels_fit = rect.width() >= 44.0 && rect.height() >= 66.0;
                painter.text(
                    rect.center() - egui::vec2(0.0, if labels_fit { 17.0 } else { 0.0 }),
                    egui::Align2::CENTER_CENTER,
                    (index + 1).to_string(),
                    FontId::proportional(19.0),
                    ui.visuals().text_color(),
                );
                if labels_fit {
                    for (line, label) in canvas_labels(surface, workspace).into_iter().enumerate() {
                        paint_canvas_line(
                            &painter,
                            rect,
                            rect.center().y - 2.0 + line as f32 * 13.0,
                            label,
                            if line == 0 && ui.visuals().dark_mode {
                                color
                            } else {
                                ui.visuals().text_color()
                            },
                        );
                    }
                }
            }
            if !surface.shared
                && rect.width() >= 90.0
                && rect.height() >= 95.0
                && (!surface.online || !surface.available)
            {
                painter.text(
                    rect.center_bottom() - egui::vec2(0.0, 12.0),
                    egui::Align2::CENTER_CENTER,
                    tr(if surface.online {
                        "Unavailable"
                    } else {
                        "Offline"
                    }),
                    FontId::proportional(10.0),
                    ui.visuals().weak_text_color(),
                );
            }
            let response = ui
                .interact(
                    rect.intersect(canvas),
                    ui.make_persistent_id(("arrangement-display", &surface.id)),
                    egui::Sense::click_and_drag(),
                )
                .on_hover_cursor(if stale {
                    egui::CursorIcon::PointingHand
                } else {
                    egui::CursorIcon::Grab
                })
                .on_hover_text(surface_label(surface, workspace));
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    true,
                    surface_label(surface, workspace),
                )
            });
            if response.clicked() || response.drag_started() {
                self.selected = surface.id.clone();
                response.request_focus();
            }
            if !stale
                && response.drag_started()
                && let Some(component) = scene
                    .components
                    .iter()
                    .find(|component| component.id == surface.component)
                && let Some(pointer) = ui.input(|input| input.pointer.press_origin())
            {
                self.drag = Some(Drag {
                    component: component.id.clone(),
                    pointer,
                    origin: component.origin,
                    before: draft.clone(),
                    transform,
                });
            }
            if !stale
                && (response.dragged() || response.drag_stopped())
                && let Some(drag) = &self.drag
                && drag.component == surface.component
                && let Some(pointer) = response.interact_pointer_pos()
            {
                let delta = (pointer - drag.pointer) / drag.transform.scale;
                let origin = Point {
                    x: drag.origin.x.saturating_add(delta.x.round() as i32),
                    y: drag.origin.y.saturating_add(delta.y.round() as i32),
                };
                *draft = layout::move_component(
                    workspace,
                    &drag.before,
                    &drag.component,
                    origin,
                    (10.0 / transform.scale).round().max(1.0) as i32,
                );
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                finish_drag |= response.drag_stopped();
            }
            if selected && response.has_focus() && self.drag.is_none() && !stale {
                let step = if ui.input(|input| input.modifiers.shift) {
                    10
                } else {
                    1
                };
                let x = i32::from(ui.input(|input| input.key_pressed(egui::Key::ArrowRight)))
                    - i32::from(ui.input(|input| input.key_pressed(egui::Key::ArrowLeft)));
                let y = i32::from(ui.input(|input| input.key_pressed(egui::Key::ArrowDown)))
                    - i32::from(ui.input(|input| input.key_pressed(egui::Key::ArrowUp)));
                if (x != 0 || y != 0)
                    && let Some(component) = scene
                        .components
                        .iter()
                        .find(|component| component.id == surface.component)
                {
                    let origin = Point {
                        x: component.origin.x.saturating_add(x * step),
                        y: component.origin.y.saturating_add(y * step),
                    };
                    *draft = layout::move_component(workspace, draft, &component.id, origin, 0);
                }
            }
        }
        for contact in &scene.contacts {
            if let Some(surface) = scene
                .surfaces
                .iter()
                .find(|surface| surface.id == contact.from)
                && let Some(rect) = &surface.rect
            {
                let (from, to) = contact_line(
                    transform.rect(rect),
                    contact.side,
                    contact.start,
                    contact.end,
                );
                let enabled = surface.available
                    && scene
                        .surfaces
                        .iter()
                        .any(|target| target.id == contact.to && target.available);
                painter.line_segment(
                    [from, to],
                    Stroke::new(
                        3.0,
                        if enabled {
                            ui.visuals().selection.bg_fill
                        } else {
                            ui.visuals().weak_text_color().gamma_multiply(0.4)
                        },
                    ),
                );
            }
        }
        if finish_drag || !ui.input(|input| input.pointer.primary_down()) {
            self.drag = None;
        }
        if scene.surfaces.iter().all(|surface| surface.rect.is_none()) {
            painter.text(
                canvas.center(),
                egui::Align2::CENTER_CENTER,
                tr("Waiting for display information…"),
                FontId::proportional(14.0),
                ui.visuals().weak_text_color(),
            );
        }
        ui.weak(tr("Select a display for details. Arrow keys move the selected group; Shift moves it faster."));
    }

    fn selection(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        draft: &mut Layout,
        scene: &Scene,
        stale: bool,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("Selected display"));
            egui::ComboBox::from_id_salt("arrangement-selected-display")
                .width(320.0_f32.min(ui.available_width()))
                .selected_text(
                    scene
                        .surfaces
                        .iter()
                        .position(|surface| surface.id == self.selected)
                        .map(|index| {
                            format!(
                                "{} · {}",
                                index + 1,
                                surface_label(&scene.surfaces[index], workspace)
                            )
                        })
                        .unwrap_or_else(|| tr("No displays")),
                )
                .show_ui(ui, |ui| {
                    for (index, surface) in scene.surfaces.iter().enumerate() {
                        ui.selectable_value(
                            &mut self.selected,
                            surface.id.clone(),
                            format!("{} · {}", index + 1, surface_label(surface, workspace)),
                        );
                    }
                });
        });
        self.ensure_selection(scene);
        let Some(selected) = scene
            .surfaces
            .iter()
            .find(|surface| surface.id == self.selected)
        else {
            return;
        };
        if let Some(native) = &selected.native {
            ui.weak(format!(
                "{} × {} · {}",
                native.rect.width,
                native.rect.height,
                tr(if selected.online {
                    "Connected"
                } else {
                    "Offline"
                })
            ));
        }
        if let Some(reason) = &selected.reason
            && !(selected.shared && selected.computer.is_none())
        {
            ui.colored_label(ui.visuals().warn_fg_color, tr(reason));
        }
        if selected.shared {
            ui.weak(tr("One physical monitor is shared by its configured computers. Entering it uses the last confirmed computer."));
        }
        if scene
            .components
            .iter()
            .find(|component| component.id == selected.component)
            .is_some_and(|component| component.surfaces.len() > 1)
        {
            ui.weak(tr(if selected.component == layout::SHARED {
                "The shared monitor and its attached displays move as one group. Their relative positions come from each computer's display settings."
            } else {
                "These displays move as one group. Change their relative positions in this computer's display settings."
            }));
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("Place this group relative to"));
            egui::ComboBox::from_id_salt("arrangement-neighbour")
                .width(240.0_f32.min(ui.available_width()))
                .selected_text(
                    scene
                        .components
                        .iter()
                        .find(|component| component.id == self.neighbour)
                        .map(|component| {
                            if component.id == layout::SHARED {
                                tr(&component.label)
                            } else {
                                component.label.clone()
                            }
                        })
                        .unwrap_or_else(|| tr("No other display group")),
                )
                .show_ui(ui, |ui| {
                    for component in scene.components.iter().filter(|component| {
                        component.id != selected.component && component.bounds.is_some()
                    }) {
                        ui.selectable_value(
                            &mut self.neighbour,
                            component.id.clone(),
                            if component.id == layout::SHARED {
                                tr(&component.label)
                            } else {
                                component.label.clone()
                            },
                        );
                    }
                });
            if let Some(side) = directions(
                ui,
                !stale && selected.rect.is_some() && !self.neighbour.is_empty(),
            )
            .0
            {
                place(
                    workspace,
                    draft,
                    scene,
                    &selected.component,
                    &self.neighbour,
                    side,
                );
            }
        });
        if let Some(problem) = scene.problems.first() {
            ui.colored_label(ui.visuals().warn_fg_color, tr(problem));
        }
    }

    fn unavailable(&mut self, ui: &mut egui::Ui, workspace: &Workspace, scene: &Scene) {
        let unavailable = scene
            .surfaces
            .iter()
            .filter(|surface| surface.rect.is_none() || !surface.available)
            .collect::<Vec<_>>();
        if unavailable.is_empty() {
            return;
        }
        egui::CollapsingHeader::new(tr("Unavailable displays"))
            .id_salt("arrangement-unavailable-displays")
            .default_open(unavailable.iter().any(|surface| surface.rect.is_none()))
            .show(ui, |ui| {
                for surface in unavailable {
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .selectable_label(
                                self.selected == surface.id,
                                surface_label(surface, workspace),
                            )
                            .clicked()
                        {
                            self.selected = surface.id.clone();
                        }
                        ui.weak(
                            surface
                                .reason
                                .as_ref()
                                .map(|reason| tr(reason))
                                .unwrap_or_else(|| tr("Offline")),
                        );
                    });
                }
            });
    }
}

fn directions(ui: &mut egui::Ui, enabled: bool) -> (Option<Side>, [egui::Id; 4]) {
    let mut selected = None;
    let ids = [
        (Side::Left, "Left"),
        (Side::Right, "Right"),
        (Side::Top, "Above"),
        (Side::Bottom, "Below"),
    ]
    .map(|(side, label)| {
        let response = ui.add_enabled(enabled, egui::Button::new(tr(label)));
        if response.clicked() {
            selected = Some(side);
        }
        response.id
    });
    (selected, ids)
}

fn surface_label(surface: &Surface, workspace: &Workspace) -> String {
    if surface.shared {
        let owner = surface
            .computer
            .as_ref()
            .and_then(|key| workspace.members.get(key))
            .map(|member| member.node.name.as_str());
        return match owner {
            Some(owner) => format!("{} · {owner}", tr("Shared monitor")),
            None => format!("{} · {}", tr("Shared monitor"), tr("Monitor input unknown")),
        };
    }
    surface.label.clone()
}

fn canvas_labels(surface: &Surface, workspace: &Workspace) -> [String; 2] {
    if surface.shared {
        let owner = surface
            .computer
            .as_ref()
            .and_then(|key| workspace.members.get(key))
            .map(|member| member.node.name.clone())
            .unwrap_or_else(|| tr("Monitor input unknown"));
        return [tr("Shared monitor"), owner];
    }
    if let Some(DisplayTarget::Ordinary { computer, display }) = &surface.target
        && let Some(member) = workspace.members.get(computer)
    {
        let name = member
            .node
            .devices
            .displays
            .iter()
            .find(|found| &found.id == display)
            .map(|found| {
                if found.name.is_empty() {
                    found.id.clone()
                } else {
                    found.name.clone()
                }
            })
            .unwrap_or_else(|| tr("Unavailable"));
        return [member.node.name.clone(), name];
    }
    let owner = surface
        .computer
        .as_ref()
        .and_then(|key| workspace.members.get(key))
        .map(|member| member.node.name.clone())
        .unwrap_or_else(|| tr("Unavailable"));
    [owner, tr("Unavailable")]
}

fn paint_canvas_line(painter: &egui::Painter, rect: Rect, y: f32, text: String, color: Color32) {
    let mut job = egui::text::LayoutJob::simple(
        text,
        FontId::proportional(10.5),
        color,
        (rect.width() - 12.0).max(1.0),
    );
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let galley = painter.layout_job(job);
    painter.with_clip_rect(rect.shrink(5.0)).galley(
        egui::pos2(rect.center().x - galley.size().x * 0.5, y),
        galley,
        color,
    );
}

fn place(
    workspace: &Workspace,
    draft: &mut Layout,
    scene: &Scene,
    selected: &str,
    neighbour: &str,
    side: Side,
) {
    let Some(component) = scene
        .components
        .iter()
        .find(|component| component.id == selected)
    else {
        return;
    };
    let (Some(rect), Some(target)) = (
        component.bounds.as_ref(),
        scene
            .components
            .iter()
            .find(|component| component.id == neighbour)
            .and_then(|component| component.bounds.as_ref()),
    ) else {
        return;
    };
    let dx = i64::from(target.x) - i64::from(rect.x);
    let dy = i64::from(target.y) - i64::from(rect.y);
    let (dx, dy) = match side {
        Side::Left => (
            dx - i64::from(rect.width),
            dy + (i64::from(target.height) - i64::from(rect.height)) / 2,
        ),
        Side::Right => (
            dx + i64::from(target.width),
            dy + (i64::from(target.height) - i64::from(rect.height)) / 2,
        ),
        Side::Top => (
            dx + (i64::from(target.width) - i64::from(rect.width)) / 2,
            dy - i64::from(rect.height),
        ),
        Side::Bottom => (
            dx + (i64::from(target.width) - i64::from(rect.width)) / 2,
            dy + i64::from(target.height),
        ),
    };
    let point = Point {
        x: (i64::from(component.origin.x) + dx).clamp(i64::from(i32::MIN), i64::from(i32::MAX))
            as i32,
        y: (i64::from(component.origin.y) + dy).clamp(i64::from(i32::MIN), i64::from(i32::MAX))
            as i32,
    };
    *draft = layout::move_component(workspace, draft, &component.id, point, 0);
}

fn contact_line(rect: Rect, side: Side, start: u16, end: u16) -> (Pos2, Pos2) {
    let start = f32::from(start) / f32::from(u16::MAX);
    let end = f32::from(end) / f32::from(u16::MAX);
    match side {
        Side::Left => (
            egui::pos2(rect.left(), rect.top() + rect.height() * start),
            egui::pos2(rect.left(), rect.top() + rect.height() * end),
        ),
        Side::Right => (
            egui::pos2(rect.right(), rect.top() + rect.height() * start),
            egui::pos2(rect.right(), rect.top() + rect.height() * end),
        ),
        Side::Top => (
            egui::pos2(rect.left() + rect.width() * start, rect.top()),
            egui::pos2(rect.left() + rect.width() * end, rect.top()),
        ),
        Side::Bottom => (
            egui::pos2(rect.left() + rect.width() * start, rect.bottom()),
            egui::pos2(rect.left() + rect.width() * end, rect.bottom()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{edge::NativeScreen, workspace::devices::Display};

    fn fixture() -> Workspace {
        let mut state = crate::group::model::tests::fixture(3);
        let keys = state.members.keys().cloned().collect::<Vec<_>>();
        for (index, member) in state.members.values_mut().enumerate() {
            let id = format!("display-{index}");
            member.node.monitor.enabled = index < 2;
            member.node.monitor.id = id.clone();
            member.node.monitor.input = Some(15 + index as u16);
            member.node.devices.displays = vec![Display {
                id,
                name: "Main display".into(),
                fingerprint: "ab".repeat(32),
                native: Some(NativeScreen {
                    native_id: format!("screen-{index}"),
                    rect: ScreenRect {
                        x: 0,
                        y: 0,
                        width: 1920,
                        height: 1080,
                    },
                    epoch: index as u64 + 1,
                }),
            }];
        }
        state
            .members
            .get_mut(&keys[0])
            .unwrap()
            .node
            .devices
            .displays
            .push(Display {
                id: "portrait".into(),
                name: "Portrait display".into(),
                fingerprint: "cd".repeat(32),
                native: Some(NativeScreen {
                    native_id: "portrait".into(),
                    rect: ScreenRect {
                        x: -1080,
                        y: 0,
                        width: 1080,
                        height: 1920,
                    },
                    epoch: 4,
                }),
            });
        state.confirmed_monitor = Some(keys[0].clone());
        state.layout = layout::draft(&state);
        state.layout.revision = 3;
        state
            .layout
            .positions
            .insert(layout::SHARED.into(), Point { x: 0, y: 0 });
        state
            .layout
            .positions
            .insert(keys[2].clone(), Point { x: 1920, y: 0 });
        state
    }
    fn input(size: Vec2) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            ..Default::default()
        }
    }
    fn key(value: egui::Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key: value,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn fit_contains_varied_displays_and_direction_placement_preserves_the_rigid_group() {
        let state = fixture();
        let scene = layout::scene(&state, &state.layout);
        assert_eq!(
            scene
                .surfaces
                .iter()
                .filter(|surface| surface.shared)
                .count(),
            1
        );
        let canvas = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 260.0));
        let transform = Transform::fit(canvas, &scene);
        for rect in scene
            .surfaces
            .iter()
            .filter_map(|surface| surface.rect.as_ref())
        {
            assert!(canvas.contains_rect(transform.rect(rect)));
        }
        let neighbour = scene
            .components
            .iter()
            .find(|component| component.id != layout::SHARED)
            .unwrap();
        let reference = scene
            .surfaces
            .iter()
            .filter(|surface| surface.component == layout::SHARED)
            .map(|surface| (surface.id.clone(), surface.rect.unwrap()))
            .collect::<Vec<_>>();
        for side in [Side::Left, Side::Right, Side::Top, Side::Bottom] {
            let mut draft = state.layout.clone();
            place(
                &state,
                &mut draft,
                &scene,
                layout::SHARED,
                &neighbour.id,
                side,
            );
            layout::validate(&state, &draft).unwrap();
            let after = layout::scene(&state, &draft);
            let moved = after
                .surfaces
                .iter()
                .find(|surface| surface.id == reference[0].0)
                .unwrap()
                .rect
                .unwrap();
            let delta = (moved.x - reference[0].1.x, moved.y - reference[0].1.y);
            for (id, original) in &reference {
                let rect = after
                    .surfaces
                    .iter()
                    .find(|surface| &surface.id == id)
                    .unwrap()
                    .rect
                    .unwrap();
                assert_eq!((rect.x - original.x, rect.y - original.y), delta);
                assert_eq!((rect.width, rect.height), (original.width, original.height));
            }
        }
    }

    #[test]
    fn each_direction_button_accepts_enter_and_space() {
        for activation in [egui::Key::Enter, egui::Key::Space] {
            for (index, expected) in [Side::Left, Side::Right, Side::Top, Side::Bottom]
                .into_iter()
                .enumerate()
            {
                let ctx = egui::Context::default();
                super::super::view::style(&ctx, "dark");
                let mut ids = None;
                let _ = ctx.run_ui(input(egui::vec2(800.0, 120.0)), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ids = Some(directions(ui, true).1);
                    });
                });
                ctx.memory_mut(|memory| memory.request_focus(ids.unwrap()[index]));
                let mut requested = None;
                for pressed in [true, false] {
                    let mut raw = input(egui::vec2(800.0, 120.0));
                    raw.events.push(key(activation, pressed));
                    let _ = ctx.run_ui(raw, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            requested = directions(ui, true).0.or(requested);
                        });
                    });
                }
                assert_eq!(requested, Some(expected));
            }
        }
    }

    #[test]
    fn escape_restores_the_drag_draft_then_closes_without_modifying_the_workspace() {
        let state = fixture();
        let original = state.clone();
        let ctx = egui::Context::default();
        super::super::view::style(&ctx, "dark");
        let mut editor = Editor::default();
        editor.open(&state);
        let before = editor.draft.clone().unwrap();
        editor.draft = Some(layout::move_component(
            &state,
            &before,
            layout::SHARED,
            Point { x: 20, y: 10 },
            0,
        ));
        editor.drag = Some(Drag {
            component: layout::SHARED.into(),
            pointer: Pos2::ZERO,
            origin: Point { x: 0, y: 0 },
            before: before.clone(),
            transform: Transform {
                origin: Pos2::ZERO,
                scale: 1.0,
            },
        });
        let mut raw = input(egui::vec2(880.0, 650.0));
        raw.events.push(key(egui::Key::Escape, true));
        let _ = ctx.run_ui(raw, |ui| {
            assert!(editor.show(ui.ctx(), &state).is_none());
        });
        assert!(editor.is_open() && editor.drag.is_none());
        assert_eq!(editor.draft.as_ref(), Some(&before));
        for pressed in [false, true] {
            let mut raw = input(egui::vec2(880.0, 650.0));
            raw.events.push(key(egui::Key::Escape, pressed));
            let _ = ctx.run_ui(raw, |ui| {
                assert!(editor.show(ui.ctx(), &state).is_none());
            });
        }
        assert!(!editor.is_open());
        assert_eq!(state, original);
    }

    #[test]
    fn unavailable_displays_remain_selectable_and_footer_stays_visible_with_long_content() {
        let mut state = fixture();
        let computer = state.members.keys().last().unwrap().clone();
        for index in 1..32 {
            state
                .members
                .get_mut(&computer)
                .unwrap()
                .node
                .devices
                .displays
                .push(Display {
                    id: format!("unavailable-{index}"),
                    name: format!("Unavailable display {index}"),
                    fingerprint: String::new(),
                    native: None,
                });
        }
        let original = state.clone();
        let ctx = egui::Context::default();
        super::super::view::style(&ctx, "dark");
        let mut editor = Editor::default();
        editor.open(&state);
        let scene = layout::scene(&state, editor.draft.as_ref().unwrap());
        let unavailable = scene
            .surfaces
            .iter()
            .find(|surface| surface.rect.is_none())
            .unwrap();
        editor.selected = unavailable.id.clone();
        editor.ensure_selection(&scene);
        assert_eq!(editor.selected, unavailable.id);
        for _ in 0..3 {
            let _ = ctx.run_ui(input(egui::vec2(880.0, 650.0)), |ui| {
                assert!(editor.show(ui.ctx(), &state).is_none());
            });
        }
        let bounds = Rect::from_min_size(Pos2::ZERO, egui::vec2(880.0, 650.0));
        let actions = editor.actions.unwrap();
        for (_, rect) in actions {
            assert!(
                bounds.contains_rect(rect),
                "footer is outside the viewport: {rect:?}"
            );
        }
        ctx.memory_mut(|memory| memory.request_focus(actions[1].0));
        let mut raw = input(egui::vec2(880.0, 650.0));
        raw.events.push(key(egui::Key::Enter, true));
        let _ = ctx.run_ui(raw, |ui| {
            assert!(editor.show(ui.ctx(), &state).is_none());
        });
        assert!(!editor.is_open());
        assert_eq!(state, original);
    }
}
