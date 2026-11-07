use super::{Action, Desktop, Page};
use eframe::egui::{self, Align, Color32, FontId, RichText, Stroke, Vec2};

const NAVY: Color32 = Color32::from_rgb(26, 34, 57);
const ACCENT: Color32 = Color32::from_rgb(82, 99, 224);

struct Palette {
    bg: Color32,
    card: Color32,
    ink: Color32,
    muted: Color32,
    line: Color32,
    green: Color32,
    selection: Color32,
    error: Color32,
}
impl Palette {
    fn new(dark: bool) -> Self {
        if dark {
            Self {
                bg: Color32::from_rgb(17, 22, 32),
                card: Color32::from_rgb(25, 32, 46),
                ink: Color32::from_rgb(231, 236, 246),
                muted: Color32::from_rgb(159, 173, 195),
                line: Color32::from_rgb(52, 65, 87),
                green: Color32::from_rgb(76, 211, 163),
                selection: Color32::from_rgb(41, 55, 83),
                error: Color32::from_rgb(249, 149, 128),
            }
        } else {
            Self {
                bg: Color32::from_rgb(244, 246, 250),
                card: Color32::WHITE,
                ink: Color32::from_rgb(34, 44, 67),
                muted: Color32::from_rgb(115, 125, 145),
                line: Color32::from_rgb(225, 230, 239),
                green: Color32::from_rgb(35, 139, 109),
                selection: Color32::from_rgb(233, 247, 241),
                error: Color32::from_rgb(162, 69, 53),
            }
        }
    }
}
fn palette(ui: &egui::Ui) -> Palette {
    Palette::new(ui.visuals().dark_mode)
}

pub fn style(ctx: &egui::Context, preference: &str) {
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(match preference {
        "light" => egui::SystemTheme::Light,
        "dark" => egui::SystemTheme::Dark,
        _ => egui::SystemTheme::SystemDefault,
    }));
    ctx.set_theme(match preference {
        "light" => egui::ThemePreference::Light,
        "dark" => egui::ThemePreference::Dark,
        _ => egui::ThemePreference::System,
    });
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let colors = Palette::new(theme == egui::Theme::Dark);
        let mut style = (*ctx.style_of(theme)).clone();
        for (text, size) in [
            (egui::TextStyle::Body, 14.0),
            (egui::TextStyle::Button, 14.0),
            (egui::TextStyle::Heading, 26.0),
            (egui::TextStyle::Small, 12.0),
        ] {
            style.text_styles.insert(text, FontId::proportional(size));
        }
        style.spacing.item_spacing = Vec2::new(10.0, 10.0);
        style.spacing.button_padding = Vec2::new(14.0, 9.0);
        style.spacing.interact_size.y = 34.0;
        style.spacing.slider_width = 240.0;
        style.visuals.override_text_color = None;
        let widgets = &mut style.visuals.widgets;
        for widget in [
            &mut widgets.noninteractive,
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
            &mut widgets.open,
        ] {
            widget.fg_stroke.color = colors.ink;
        }
        style.visuals.panel_fill = colors.bg;
        style.visuals.extreme_bg_color = colors.card;
        style.visuals.selection.bg_fill = ACCENT;
        style.visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        style.visuals.widgets.inactive.bg_fill = colors.line;
        style.visuals.slider_trailing_fill = true;
        style.visuals.widgets.inactive.weak_bg_fill = colors.card;
        style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, colors.line);
        style.visuals.widgets.inactive.corner_radius = 7.into();
        style.visuals.widgets.hovered.corner_radius = 7.into();
        style.visuals.widgets.active.corner_radius = 7.into();
        style.visuals.widgets.hovered.weak_bg_fill = colors.selection;
        style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
        ctx.set_style_of(theme, style);
    }
}

fn card<R>(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(palette(ui).card)
        .stroke(Stroke::new(1.0, palette(ui).line))
        .corner_radius(12)
        .inner_margin(20)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui)
        })
        .inner
}

fn title(ui: &mut egui::Ui, heading: &str, detail: &str) {
    ui.heading(crate::i18n::ui_text(
        RichText::new(crate::i18n::tr(heading)).strong(),
    ));
    ui.label(crate::i18n::ui_text(
        RichText::new(crate::i18n::tr(detail)).color(palette(ui).muted),
    ));
    ui.add_space(12.0);
}

fn subheading(ui: &mut egui::Ui, text: &str) {
    ui.label(crate::i18n::ui_text(
        RichText::new(crate::i18n::tr(text)).size(17.0).strong(),
    ));
}

fn note(ui: &mut egui::Ui, text: &str) {
    ui.label(crate::i18n::ui_text(
        RichText::new(crate::i18n::tr(text))
            .size(12.0)
            .color(palette(ui).muted),
    ));
}

fn primary(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(crate::i18n::ui_text(
            RichText::new(crate::i18n::tr(text)).color(Color32::WHITE),
        ))
        .fill(ACCENT)
        .stroke(Stroke::NONE)
        .min_size(Vec2::new(0.0, 40.0)),
    )
}

fn choice(ui: &mut egui::Ui, id: &str, value: &mut String, options: &[(&str, &str)]) -> bool {
    let before = value.clone();
    let caption = options
        .iter()
        .find(|(key, _)| *key == value)
        .map(|(_, text)| *text)
        .unwrap_or(value);
    egui::ComboBox::from_id_salt(id)
        .height(320.0)
        .width(ui.available_width().max(160.0))
        .selected_text(crate::i18n::ui_text(caption))
        .show_ui(ui, |ui| {
            for (key, text) in options {
                ui.selectable_value(value, (*key).into(), crate::i18n::ui_text(*text));
            }
        });
    before != *value
}

fn badge(ui: &mut egui::Ui, label: &str, active: bool) {
    let color = if active {
        palette(ui).green
    } else {
        palette(ui).muted
    };
    egui::Frame::new()
        .fill(if active {
            palette(ui).selection
        } else {
            palette(ui).bg
        })
        .corner_radius(20)
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.spacing_mut().interact_size.y = 16.0;
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(7.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, color);
                ui.label(crate::i18n::ui_text(
                    RichText::new(crate::i18n::tr(label))
                        .size(12.0)
                        .color(color),
                ));
            });
        });
}

fn keyboard(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let rect = egui::Rect::from_center_size(center, Vec2::new(36.0, 22.0));
    painter.rect_stroke(rect, 4, Stroke::new(1.5, color), egui::StrokeKind::Inside);
    for y in 0..2 {
        for x in 0..5 {
            let p = rect.left_top() + Vec2::new(6.0 + x as f32 * 5.8, 5.5 + y as f32 * 5.0);
            painter.rect_filled(
                egui::Rect::from_center_size(p, Vec2::new(2.6, 2.0)),
                0,
                color,
            );
        }
    }
    painter.line_segment(
        [center + Vec2::new(-8.0, 6.0), center + Vec2::new(8.0, 6.0)],
        Stroke::new(1.5, color),
    );
}
fn mouse(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let rect = egui::Rect::from_center_size(center, Vec2::new(17.0, 25.0));
    painter.rect_stroke(rect, 8, Stroke::new(1.5, color), egui::StrokeKind::Inside);
    painter.line_segment(
        [center - Vec2::new(0.0, 10.0), center - Vec2::new(0.0, 2.0)],
        Stroke::new(1.5, color),
    );
    painter.line_segment(
        [center - Vec2::new(6.0, 2.0), center + Vec2::new(6.0, -2.0)],
        Stroke::new(1.0, color),
    );
}
fn headphones(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let points = (0..=18)
        .map(|i| {
            let angle = std::f32::consts::PI * (i as f32 / 18.0);
            center + Vec2::new(-angle.cos() * 12.0, -angle.sin() * 12.0)
        })
        .collect::<Vec<_>>();
    painter.add(egui::Shape::line(points, Stroke::new(2.0, color)));
    for offset in [-11.0, 11.0] {
        painter.rect_filled(
            egui::Rect::from_center_size(center + Vec2::new(offset, 4.0), Vec2::new(5.0, 12.0)),
            2,
            color,
        );
    }
}

fn microphone(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    painter.rect_stroke(
        egui::Rect::from_center_size(center - Vec2::new(0.0, 3.0), Vec2::new(9.0, 17.0)),
        5,
        Stroke::new(1.6, color),
        egui::StrokeKind::Inside,
    );
    painter.line_segment(
        [center + Vec2::new(-9.0, 0.0), center + Vec2::new(-9.0, 5.0)],
        Stroke::new(1.5, color),
    );
    painter.line_segment(
        [center + Vec2::new(9.0, 0.0), center + Vec2::new(9.0, 5.0)],
        Stroke::new(1.5, color),
    );
    painter.line_segment(
        [center + Vec2::new(-9.0, 5.0), center + Vec2::new(0.0, 10.0)],
        Stroke::new(1.5, color),
    );
    painter.line_segment(
        [center + Vec2::new(9.0, 5.0), center + Vec2::new(0.0, 10.0)],
        Stroke::new(1.5, color),
    );
    painter.line_segment(
        [center + Vec2::new(0.0, 10.0), center + Vec2::new(0.0, 15.0)],
        Stroke::new(1.5, color),
    );
    painter.line_segment(
        [
            center + Vec2::new(-5.0, 15.0),
            center + Vec2::new(5.0, 15.0),
        ],
        Stroke::new(1.5, color),
    );
}

fn monitor(painter: &egui::Painter, center: egui::Pos2, width: f32, color: Color32, bg: Color32) {
    let screen = egui::Rect::from_center_size(center, Vec2::new(width, 98.0));
    painter.rect_filled(screen.expand(7.0), 12, color.gamma_multiply(0.06));
    painter.rect_filled(screen, 8, bg);
    painter.rect_stroke(screen, 8, Stroke::new(2.0, color), egui::StrokeKind::Inside);
    painter.rect_filled(
        egui::Rect::from_center_size(center + Vec2::new(0.0, 55.0), Vec2::new(4.0, 14.0)),
        1,
        color,
    );
    painter.rect_filled(
        egui::Rect::from_center_size(center + Vec2::new(0.0, 64.0), Vec2::new(44.0, 3.0)),
        2,
        color,
    );
}

fn tower(painter: &egui::Painter, center: egui::Pos2, color: Color32, bg: Color32) {
    let case = egui::Rect::from_center_size(center, Vec2::new(64.0, 94.0));
    painter.rect_filled(case.expand(7.0), 12, color.gamma_multiply(0.06));
    painter.rect_filled(case, 8, bg);
    painter.rect_stroke(case, 8, Stroke::new(2.0, color), egui::StrokeKind::Inside);
    painter.circle_stroke(center + Vec2::new(0.0, -23.0), 5.0, Stroke::new(1.5, color));
    for y in [2.0, 10.0, 18.0] {
        painter.line_segment(
            [center + Vec2::new(-16.0, y), center + Vec2::new(16.0, y)],
            Stroke::new(1.5, color),
        );
    }
    painter.circle_filled(center + Vec2::new(0.0, 33.0), 2.0, color);
}

fn device_picker(
    ui: &mut egui::Ui,
    salt: &str,
    label: &str,
    choice: &mut Option<String>,
    devices: &[crate::audio::AudioDevice],
    automatic_name: Option<&str>,
) {
    ui.horizontal(|ui| {
        ui.label(crate::i18n::tr(label));
        let caption = choice
            .as_ref()
            .map(|id| {
                devices
                    .iter()
                    .find(|d| &d.id == id)
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| crate::i18n::tr("Selected device unavailable"))
            })
            .unwrap_or_else(|| {
                let automatic = crate::i18n::tr("Автоматически");
                automatic_name
                    .map(str::to_owned)
                    .or_else(|| {
                        devices
                            .iter()
                            .find(|d| d.default)
                            .or_else(|| (devices.len() == 1).then(|| &devices[0]))
                            .map(|d| d.name.clone())
                    })
                    .map(|name| format!("{automatic} · {name}"))
                    .unwrap_or(automatic)
            });
        egui::ComboBox::from_id_salt(salt)
            .width(320.0)
            .selected_text(caption)
            .show_ui(ui, |ui| {
                ui.selectable_value(choice, None, crate::i18n::tr("Автоматически"));
                for device in devices {
                    ui.selectable_value(choice, Some(device.id.clone()), &device.name);
                }
            });
    });
}

impl Desktop {
    pub(super) fn render(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        egui::Panel::left("navigation")
            .exact_size(180.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(NAVY).inner_margin(18))
            .show(root, |ui| {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Desk Switch")
                        .size(22.0)
                        .strong()
                        .color(Color32::WHITE),
                );
                ui.label(
                    RichText::new(crate::i18n::tr("ДВА ПК. ОДНО МЕСТО."))
                        .size(10.0)
                        .color(Color32::from_rgb(150, 160, 184)),
                );
                ui.add_space(36.0);
                for (page, label) in [
                    (Page::Overview, "Рабочее место"),
                    (Page::Application, "Настройки"),
                    (Page::Diagnostics, "Диагностика"),
                ] {
                    let selected = self.page == page;
                    let button = egui::Button::new(RichText::new(crate::i18n::tr(label)).color(
                        if selected {
                            Color32::WHITE
                        } else {
                            Color32::from_rgb(170, 181, 207)
                        },
                    ))
                    .fill(if selected {
                        Color32::from_rgb(52, 65, 103)
                    } else {
                        Color32::TRANSPARENT
                    })
                    .stroke(Stroke::NONE)
                    .min_size(Vec2::new(ui.available_width(), 43.0));
                    if ui.add(button).clicked() {
                        self.page = page;
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(Align::LEFT), |ui| {
                    if ui.button(crate::i18n::tr("Закрыть приложение")).clicked() {
                        self.action(Action::Quit, &ctx);
                    }
                    if ui
                        .add_enabled(
                            self.tray.as_ref().is_some_and(super::tray::Tray::available),
                            egui::Button::new(crate::i18n::tr("Свернуть в трей")),
                        )
                        .clicked()
                    {
                        self.action(Action::Hide, &ctx);
                    }
                    ui.label(
                        RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                            .size(11.0)
                            .color(Color32::from_rgb(143, 154, 183)),
                    );
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette(root).bg).inner_margin(26))
            .show(root, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.quitting {
                            title(
                                ui,
                                "Завершаем работу",
                                "Возвращаем управление и останавливаем фоновые службы…",
                            );
                            ui.spinner();
                            return;
                        }
                        match self.page {
                            Page::Overview => self.overview(ui),
                            Page::Application => self.application(ui),
                            Page::Diagnostics => self.diagnostics(ui),
                        }
                        if let Some((message, error)) = self.notice.clone() {
                            ui.add_space(10.0);
                            card(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(RichText::new(crate::i18n::tr(message)).color(
                                        if error {
                                            palette(ui).error
                                        } else {
                                            palette(ui).ink
                                        },
                                    ));
                                    if ui.small_button(crate::i18n::tr("Скрыть")).clicked() {
                                        self.notice = None;
                                    }
                                });
                            });
                        }
                    });
            });
    }

    fn overview(&mut self, ui: &mut egui::Ui) {
        title(
            ui,
            "Два компьютера. Всё вместе.",
            "Свяжите ПК один раз. Устройства и настройки согласуются автоматически.",
        );
        card(ui, |ui| {
            ui.horizontal(|ui| {
                let changed = ui
                    .checkbox(
                        &mut self.prefs.discovery,
                        RichText::new(crate::i18n::tr("Доступен в сети")).strong(),
                    )
                    .changed();
                if changed {
                    self.discovery_error = None;
                    self.availability_changed(ui.ctx());
                }
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    badge(
                        ui,
                        if self.pair.synced {
                            "Пара подключена"
                        } else if self.pair.remote.is_some() {
                            "Синхронизация…"
                        } else if self.prefs.discovery {
                            "Ищем компьютеры…"
                        } else {
                            "Не в сети"
                        },
                        self.pair.synced,
                    );
                });
            });
            if !self.pair.synced {
                note(
                    ui,
                    "Включите на обоих ПК: приложение одновременно показывает этот ПК и ищет другие.",
                );
            }
        });
        ui.add_space(6.0);
        if let Some(request) = self.pair.request.clone() {
            card(ui, |ui| {
                subheading(
                    ui,
                    &format!(
                        "{} {}",
                        request.name,
                        crate::i18n::tr("хочет связать компьютеры")
                    ),
                );
                note(ui, &format!("Код подтверждения: {}", request.code));
                ui.horizontal(|ui| {
                    if primary(ui, "Принять").clicked() {
                        self.approve_pair(true);
                    }
                    if ui.button(crate::i18n::tr("Отклонить")).clicked() {
                        self.approve_pair(false);
                    }
                });
            });
        } else if let Some(request) = self.pair.outgoing.clone() {
            card(ui, |ui| {
                subheading(ui, "Подтвердите связь на другом ПК");
                ui.label(crate::i18n::tr(format!(
                    "Код подтверждения: {}",
                    request.code
                )));
                ui.spinner();
                if ui.button(crate::i18n::tr("Cancel connection")).clicked() {
                    self.cancel_pairing(ui.ctx());
                }
            });
        } else if self.pair.busy {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(crate::i18n::tr("Connecting to the other PC…"));
                });
                if ui.button(crate::i18n::tr("Cancel connection")).clicked() {
                    self.cancel_pairing(ui.ctx());
                }
            });
        }
        card(ui, |ui| {
            self.diagram(ui);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(self.connected() && self.pair.synced, |ui| {
                    let label = if self.pair.plan.as_ref().is_some_and(|p| p.monitor.is_some()) {
                        "Switch monitor, keyboard and mouse"
                    } else {
                        "Передать клавиатуру и мышь"
                    };
                    if primary(ui, label).clicked() {
                        self.switch();
                    }
                });
                note(ui, "F12 — переключить · F11 — вернуть");
            });
            ui.horizontal(|ui| {
                if self.pair.remote.as_ref().is_some_and(|r| {
                    crate::workspace::Plan::monitor_inputs(&self.local_node(), r).is_some()
                }) && ui
                    .add_enabled(
                        self.monitor_switch_ready(),
                        egui::Button::new(crate::i18n::tr("Switch shared monitor")),
                    )
                    .on_disabled_hover_text(crate::i18n::tr(
                        "Waiting for the monitor and input services to be ready on both PCs.",
                    ))
                    .clicked()
                {
                    self.action(super::Action::MonitorSwitch, ui.ctx());
                }
                if ui
                    .small_button(crate::i18n::tr("Set up shared monitor…"))
                    .clicked()
                {
                    self.action(super::Action::MonitorSetup, ui.ctx());
                }
            });
            if self.prefs.workspace.shared.monitor_enabled {
                let local = self.local_node();
                let configured =
                    self.pair.remote.as_ref().is_some_and(|r| {
                        crate::workspace::Plan::monitor_inputs(&local, r).is_some()
                    });
                if !configured {
                    note(
                        ui,
                        "One shared monitor is shown in the middle. Remember this PC's video connection on each PC to enable switching.",
                    );
                    let allowed = self.can_remember_monitor();
                    if self.prefs.workspace.pending_monitor.is_some() {
                        note(
                            ui,
                            "Connection saved. Return the borrowed keyboard and mouse to enable monitor switching safely.",
                        );
                    } else if self.prefs.workspace.monitor.input.is_none() {
                        let disabled = if !allowed {
                            "Сначала верните управление: это единственное устройство ввода на принимающем ПК."
                        } else if self.query.is_some() {
                            "Checking the video connection…"
                        } else {
                            "Монитор пока не найден"
                        };
                        if ui
                            .add_enabled(
                                allowed && self.query.is_none() && !self.monitors.is_empty(),
                                egui::Button::new(crate::i18n::tr(
                                    "Remember this PC's video connection",
                                )),
                            )
                            .on_disabled_hover_text(crate::i18n::tr(disabled))
                            .clicked()
                        {
                            if self
                                .monitors
                                .iter()
                                .any(|m| m.id == self.prefs.workspace.monitor.id)
                            {
                                self.prefs.workspace.monitor.enabled = true;
                                self.remember_monitor();
                            } else {
                                self.action(super::Action::MonitorSetup, ui.ctx());
                            }
                        }
                    } else {
                        note(
                            ui,
                            "This PC's connection is remembered. Complete the same setup on the other PC.",
                        );
                    }
                }
            }
            if self.pair.plan.as_ref().is_some_and(|p| p.monitor.is_some()) {
                note(
                    ui,
                    "To switch back, open Desk Switch in the destination PC's tray and select the switch action there.",
                );
            }
            if let Some(plan) = &self.pair.plan
                && plan.input_source.is_none()
                && self.pair.synced
            {
                note(
                    ui,
                    if !plan.keyboard_enabled || !plan.mouse_enabled {
                        "Включите передачу нужных устройств или подключите их к принимающему ПК. Источник можно выбрать ниже."
                    } else if self.pair.hardware.input()
                        && self
                            .pair
                            .remote
                            .as_ref()
                            .is_some_and(|r| r.hardware.input())
                    {
                        "Клавиатура и мышь есть на обоих ПК — каждый управляет собой. Источник можно выбрать ниже."
                    } else {
                        "Подключите клавиатуру и мышь к одному из ПК. Проверяем устройства каждые 3 секунды."
                    },
                );
            }
            if self.pair.remote.is_some()
                && (self.pair.input_failed
                    || self.pair.audio_failed
                    || self.pair.microphone_failed
                    || self.pair.remote.as_ref().is_some_and(|r| {
                        r.input_error.is_some()
                            || r.audio_error.is_some()
                            || r.microphone_error.is_some()
                    }))
            {
                ui.separator();
                note(
                    ui,
                    "The pair is connected. A device needs attention; see the message below or Diagnostics.",
                );
                for node in std::iter::once(self.local_node()).chain(self.pair.remote.clone()) {
                    if node.input_error.as_deref().is_some_and(|e| {
                        e.contains("Permission denied") || e.contains("os error 13")
                    }) {
                        ui.colored_label(
                            palette(ui).error,
                            format!(
                                "{}: {}",
                                node.name,
                                crate::i18n::tr(
                                    "Keyboard and mouse access is required on this PC."
                                )
                            ),
                        );
                    }
                }
                #[cfg(target_os = "linux")]
                if self.prefs.role == "host"
                    && !self.form.capture_devices.is_empty()
                    && self.local_node().input_error.as_deref().is_some_and(|e| {
                        e.contains("Permission denied") || e.contains("os error 13")
                    })
                {
                    note(
                        ui,
                        "Ubuntu remembers permission for the selected keyboard and mouse after one system approval, including USB reconnection and restart. Wayland permission is separate.",
                    );
                    if ui
                        .add_enabled(
                            self.query.is_none(),
                            egui::Button::new(crate::i18n::tr("Allow keyboard and mouse access")),
                        )
                        .clicked()
                    {
                        self.scan(super::QueryKind::InputAccess);
                    }
                }
                if ui
                    .button(crate::i18n::tr("Повторить подключение устройств"))
                    .clicked()
                {
                    self.retry_devices();
                }
            }
        });
        ui.add_space(6.0);
        if self.pair.remote.is_some() {
            self.routing(ui);
        } else {
            card(ui, |ui| {
                subheading(ui, "Компьютеры рядом");
                if !self.prefs.discovery {
                    note(
                        ui,
                        "Включите доступность в сети на обоих ПК, затем нажмите «Связать».",
                    );
                } else if self.discovered.is_empty() {
                    note(
                        ui,
                        "Ищем компьютеры… На втором ПК тоже включите доступность в сети.",
                    );
                }
                if let Some(error) = &self.discovery_error {
                    ui.colored_label(palette(ui).error, crate::i18n::tr(error));
                    if ui.button(crate::i18n::tr("Повторить поиск")).clicked() {
                        self.discovery_error = None;
                        self.stop_pair();
                        self.availability_changed(ui.ctx());
                    }
                }
                let peers = self.discovered.values().cloned().collect::<Vec<_>>();
                for peer in peers {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&peer.name).strong());
                            if peer.compatible() {
                                note(ui, "Готов к подключению");
                            } else {
                                ui.colored_label(
                                    palette(ui).error,
                                    crate::i18n::tr(
                                        "Несовместимая версия — обновите этот или другой ПК",
                                    ),
                                );
                            }
                            note(
                                ui,
                                &format!("Версия {} · протокол {}", peer.version, peer.protocol),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            let already = self.peer_key.is_empty()
                                || peer.public_key.as_ref() == Some(&self.peer_key);
                            ui.add_enabled_ui(
                                peer.compatible() && !self.pair.busy && already,
                                |ui| {
                                    if primary(ui, "Связать").clicked() {
                                        self.link_peer(&peer);
                                    }
                                },
                            );
                        });
                    });
                }
                ui.separator();
                egui::CollapsingHeader::new(crate::i18n::tr("Connect by IP address"))
                    .show(ui, |ui| {
                        note(ui, "Use the other PC's IP, or IP:port. Network availability must be on at both PCs.");
                        let mut connect = false;
                        ui.horizontal(|ui| {
                            let edit = ui.add(egui::TextEdit::singleline(&mut self.prefs.workspace.manual_address)
                                .hint_text("192.168.1.20").char_limit(128).desired_width(260.0));
                            connect = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            connect |= ui.add_enabled(self.prefs.discovery && !self.pair.busy,
                                egui::Button::new(crate::i18n::tr("Связать"))).clicked();
                        });
                        if connect && self.prefs.discovery && !self.pair.busy { self.link_address(); }
                        note(ui, "Different subnets need a route or VPN; discovery is optional for this connection.");
                    });
                if !self.pair.status.is_empty() {
                    ui.label(crate::i18n::tr(&self.pair.status));
                }
                if !self.peer_key.is_empty() {
                    note(
                        ui,
                        &format!("Связанный ПК: {}", self.prefs.workspace.peer_name),
                    );
                    if ui.small_button(crate::i18n::tr("Разорвать пару")).clicked() {
                        self.forget_pair();
                    }
                }
                note(
                    ui,
                    "Запрос → «Принять» на другом ПК → готово. Ключами приложение обменивается само.",
                );
            });
        }
        ui.add_space(6.0);
        self.local_monitor(ui);
    }

    fn routing(&mut self, ui: &mut egui::Ui) {
        use crate::workspace::Selection;
        let remote = self.pair.remote.as_ref().unwrap().clone();
        let local = self.local_node();
        let before = self.prefs.workspace.shared.clone();
        let mut setup_monitor = false;
        card(ui, |ui| {
            subheading(ui, "Общие устройства");
            note(
                ui,
                "Меняйте на любом ПК — второй получит настройки автоматически.",
            );
            let shared = &mut self.prefs.workspace.shared;
            ui.horizontal(|ui| {
                for (keyboard, enabled, label) in [
                    (true, &mut shared.keyboard_enabled, "Клавиатура"),
                    (false, &mut shared.mouse_enabled, "Мышь"),
                ] {
                    ui.add_enabled(crate::workspace::Plan::can_disable(&local, &remote, keyboard),
                        egui::Checkbox::new(enabled, crate::i18n::tr(label)))
                        .on_disabled_hover_text(crate::i18n::tr("Сначала верните управление: это единственное устройство ввода на принимающем ПК."));
                }
                let mut enabled = shared.monitor_enabled;
                if ui.add_enabled(!crate::workspace::Plan::input_in_use(&local, &remote)
                    && crate::workspace::Plan::can_disable(&local,&remote,true)
                    && crate::workspace::Plan::can_disable(&local,&remote,false),
                    egui::Checkbox::new(&mut enabled, crate::i18n::tr("Общий монитор")))
                    .on_disabled_hover_text(crate::i18n::tr("Сначала верните управление: это единственное устройство ввода на принимающем ПК.")).changed() {
                    shared.monitor_enabled = enabled;
                    setup_monitor = enabled && (!local.monitor.enabled || local.monitor.input.is_none());
                }
            });
            if !crate::workspace::Plan::can_disable(&local, &remote, true)
                || !crate::workspace::Plan::can_disable(&local, &remote, false)
            {
                note(
                    ui,
                    "Сначала верните управление: это единственное устройство ввода на принимающем ПК.",
                );
            }
            for (label, id, value) in [
                ("Клавиатура и мышь", "input-owner", &mut shared.input),
                ("Общая акустика", "audio-owner", &mut shared.audio),
            ] {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::tr(label));
                    let caption = match value {
                        Selection::Auto => crate::i18n::tr("Автоматически"),
                        Selection::Computer(key) => {
                            if key == &local.key {
                                local.name.clone()
                            } else {
                                remote.name.clone()
                            }
                        }
                    };
                    ui.add_enabled_ui(
                        id != "input-owner"
                            || (crate::workspace::Plan::can_disable(&local, &remote, true)
                                && crate::workspace::Plan::can_disable(&local, &remote, false)
                                && !crate::workspace::Plan::input_in_use(&local, &remote)),
                        |ui| {
                            egui::ComboBox::from_id_salt(id)
                                .width(260.0)
                                .selected_text(caption)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        value,
                                        Selection::Auto,
                                        crate::i18n::tr("Автоматически"),
                                    );
                                    ui.selectable_value(
                                        value,
                                        Selection::Computer(local.key.clone()),
                                        &local.name,
                                    );
                                    ui.selectable_value(
                                        value,
                                        Selection::Computer(remote.key.clone()),
                                        &remote.name,
                                    );
                                });
                        },
                    );
                });
            }
            ui.checkbox(
                &mut shared.audio_enabled,
                crate::i18n::tr("Использовать общую акустику"),
            );
            let output = self
                .pair
                .plan
                .as_ref()
                .and_then(|p| p.audio_output.as_deref())
                .and_then(|key| [&local, &remote].into_iter().find(|n| n.key == key));
            if let Some(output) = output {
                note(ui, &format!("Выход: {}", output.hardware.audio_name));
            }
            ui.add_enabled_ui(shared.audio_enabled && output.is_some(), |ui| {
                for (node, label) in [(&local, "Этот компьютер"), (&remote, "Другой компьютер")]
                {
                    let color = if !self.pair.synced {
                        palette(ui).muted
                    } else if self
                        .pair
                        .plan
                        .as_ref()
                        .is_some_and(|plan| plan.purple == node.key)
                    {
                        Color32::from_rgb(167, 139, 250)
                    } else {
                        Color32::from_rgb(66, 211, 157)
                    };
                    ui.colored_label(color, format!("{} · {}", crate::i18n::tr(label), node.name));
                    let mut level = shared.audio_level(&node.key);
                    let changed = ui
                        .horizontal(|ui| {
                            let volume = ui
                                .add(egui::Slider::new(&mut level.volume, 0..=100).suffix(" %"))
                                .changed();
                            ui.checkbox(&mut level.muted, crate::i18n::tr("Без звука"))
                                .changed()
                                || volume
                        })
                        .inner;
                    if changed {
                        shared.audio_levels.insert(node.key.clone(), level);
                    }
                }
            });
            note(
                ui,
                "Каждый ползунок меняет звук своего ПК. Настройки одинаковы на обоих компьютерах и сохраняются при переносе наушников.",
            );
            ui.separator();
            let mic_before = (shared.microphone_enabled, shared.microphone.clone());
            ui.checkbox(
                &mut shared.microphone_enabled,
                crate::i18n::tr("Shared microphone"),
            );
            if shared.microphone_enabled {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::tr("Microphone source"));
                    let caption = match &shared.microphone {
                        Selection::Auto => crate::i18n::tr("Автоматически"),
                        Selection::Computer(key) => {
                            if key == &local.key {
                                local.name.clone()
                            } else {
                                remote.name.clone()
                            }
                        }
                    };
                    egui::ComboBox::from_id_salt("microphone-owner")
                        .selected_text(caption)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut shared.microphone,
                                Selection::Auto,
                                crate::i18n::tr("Автоматически"),
                            );
                            for node in [&local, &remote] {
                                ui.selectable_value(
                                    &mut shared.microphone,
                                    Selection::Computer(node.key.clone()),
                                    &node.name,
                                );
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Slider::new(&mut shared.microphone_level.volume, 0..=100)
                            .suffix(" %"),
                    );
                    ui.checkbox(
                        &mut shared.microphone_level.muted,
                        crate::i18n::tr("Mute microphone"),
                    );
                });
                note(
                    ui,
                    "Select Desk Switch microphone in your calling app. On Windows select the virtual cable's recording endpoint (CABLE Output).",
                );
                if self
                    .pair
                    .plan
                    .as_ref()
                    .is_some_and(|p| p.microphone_source.is_none())
                    && !shared.microphone_level.muted
                {
                    note(
                        ui,
                        "Microphone paused: check the selected microphone and the receiving PC's virtual input in Settings.",
                    );
                } else if local.microphone_active && remote.microphone_active {
                    note(ui, "Shared microphone active");
                }
                note(
                    ui,
                    "Mute stops microphone capture. Local apps can still use their physical microphone.",
                );
            }
            if mic_before != (shared.microphone_enabled, shared.microphone.clone()) {
                shared.microphone_binding = None;
            }
            ui.horizontal(|ui| {
                ui.add_enabled(
                    shared.mouse_enabled,
                    egui::Checkbox::new(
                        &mut shared.edge,
                        crate::i18n::tr("Переход мышью через край экрана"),
                    ),
                );
                if shared.edge {
                    let purple = local.key < remote.key;
                    let mut local_left = purple == shared.purple_on_left;
                    egui::ComboBox::from_id_salt("local-edge")
                        .selected_text(crate::i18n::tr(if local_left {
                            "Правый край"
                        } else {
                            "Левый край"
                        }))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut local_left,
                                false,
                                crate::i18n::tr("Левый край"),
                            );
                            ui.selectable_value(
                                &mut local_left,
                                true,
                                crate::i18n::tr("Правый край"),
                            );
                        });
                    shared.purple_on_left = purple == local_left;
                }
            });
            if ui.small_button(crate::i18n::tr("Разорвать пару")).clicked() {
                self.forget_pair();
            }
        });
        if self.prefs.workspace.shared != before {
            self.shared_changed();
        }
        if setup_monitor {
            self.prefs.workspace.monitor.enabled = true;
            self.monitor_setup_open = true;
            self.remember_monitor();
        }
    }

    fn local_monitor(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let open = self.monitor_setup_open;
        let local = self.local_node();
        let can_configure = self.can_remember_monitor();
        card(ui, |ui| {
            let header = egui::CollapsingHeader::new(crate::i18n::tr("Общий монитор"))
                .id_salt("local-monitor")
                .open(open.then_some(true))
                .show(ui, |ui| {
                    note(ui, "Each PC remembers its own video connection. Set this up once on each PC while its picture is visible.");
                    ui.add_enabled_ui(can_configure, |ui| {
                        let old_id = self.prefs.workspace.monitor.id.clone();
                        let selected = self.monitors.iter().find(|m| m.id == old_id)
                            .map(|m| m.name.as_str()).unwrap_or("Монитор пока не найден");
                        egui::ComboBox::from_id_salt("own-monitor")
                            .width(ui.available_width()).selected_text(crate::i18n::tr(selected))
                            .show_ui(ui, |ui| {
                                for monitor in &self.monitors {
                                    if ui.selectable_value(&mut self.prefs.workspace.monitor.id,
                                        monitor.id.clone(), &monitor.name).changed() {
                                        self.prefs.workspace.monitor.name = monitor.name.clone();
                                        self.prefs.workspace.monitor.fingerprint = monitor.fingerprint.clone();
                                    }
                                }
                            });
                        if ui.checkbox(&mut self.prefs.workspace.monitor.enabled,
                            crate::i18n::tr("This monitor is connected to both PCs")).changed() {
                            if self.prefs.workspace.monitor.enabled {
                                self.prefs.workspace.shared.monitor_enabled = true;
                                self.shared_changed();
                            }
                            changed = true;
                        }
                        if old_id != self.prefs.workspace.monitor.id {
                            self.prefs.workspace.monitor.input = None;
                            self.prefs.workspace.pending_monitor = None;
                            changed = true;
                        }
                        if self.prefs.workspace.monitor.enabled {
                            if self.query.is_some() { ui.spinner(); }
                            else if self.prefs.workspace.pending_monitor.is_some() {
                                note(ui, "Connection saved. Return the borrowed keyboard and mouse to enable monitor switching safely.");
                            } else if self.prefs.workspace.monitor.input.is_some() {
                                note(ui, "Текущее подключение этого ПК запомнено.");
                            } else {
                                note(ui, "Не удалось прочитать текущее подключение. Включите DDC/CI в меню монитора.");
                            }
                            if ui.small_button(crate::i18n::tr("Запомнить текущее подключение заново")).clicked() { changed = true; }
                            if self.pair.remote.as_ref().is_some_and(|remote| crate::workspace::Plan::monitor_inputs(&local, remote).is_none()) {
                                note(ui, "Для переключения выберите этот же общий монитор на втором ПК. Оба текущих входа должны быть разными.");
                            }
                        }
                    });
                    if !can_configure {
                        note(ui, "Сначала верните управление: это единственное устройство ввода на принимающем ПК.");
                    }
                    note(ui, "Чтение подключения не переключает монитор. Без настройки обоих ПК передаются только клавиатура и мышь.");
                });
            if open {
                header
                    .header_response
                    .scroll_to_me(Some(egui::Align::Center));
            }
        });
        self.monitor_setup_open = false;
        if changed {
            self.remember_monitor();
        }
    }

    fn diagram(&self, ui: &mut egui::Ui) {
        let local = self.local_node();
        let remote = self.pair.remote.as_ref();
        let shared_monitor = remote
            .map(|r| crate::workspace::Plan::common_monitor(&local, r))
            .unwrap_or(self.prefs.workspace.shared.monitor_enabled);
        let (rect, _) = ui.allocate_exact_size(
            Vec2::new(
                ui.available_width(),
                if shared_monitor { 250.0 } else { 225.0 },
            ),
            egui::Sense::hover(),
        );
        let painter = ui.painter();
        let colors = palette(ui);
        let plan = self.pair.plan.as_ref();
        let synced = self.pair.synced;
        let purple = Color32::from_rgb(167, 139, 250);
        let green = Color32::from_rgb(66, 211, 157);
        let muted = colors.muted.gamma_multiply(0.5);
        let mut nodes = vec![(&local, true, muted)];
        if let Some(remote) = remote {
            nodes.push((remote, false, muted));
        }
        if let Some(plan) = plan {
            for (node, _, color) in &mut nodes {
                *color = if synced {
                    if node.key == plan.purple {
                        purple
                    } else {
                        green
                    }
                } else {
                    muted
                };
            }
        }
        let device_color = |key: Option<&str>| {
            nodes
                .iter()
                .find(|(node, _, _)| Some(node.key.as_str()) == key)
                .map(|(_, _, color)| *color)
                .unwrap_or(muted)
        };
        let side = if shared_monitor { 0.16 } else { 0.24 };
        let left = egui::pos2(
            rect.left() + rect.width() * side,
            rect.top() + if shared_monitor { 90.0 } else { 80.0 },
        );
        let right = egui::pos2(rect.right() - rect.width() * side, left.y);
        let middle = egui::pos2(rect.center().x, left.y);
        if shared_monitor {
            let owner =
                remote.and_then(|r| crate::workspace::Plan::monitor_owner(&local, r, synced));
            for (index, center) in [left, right].into_iter().enumerate() {
                let active = nodes
                    .get(index)
                    .is_some_and(|(n, _, _)| Some(n.key.as_str()) == owner);
                painter.line_segment(
                    [center, middle],
                    Stroke::new(
                        if active { 2.0 } else { 1.0 },
                        if active {
                            device_color(owner)
                        } else {
                            colors.line
                        },
                    ),
                );
            }
            monitor(
                painter,
                middle,
                (rect.width() * 0.29).min(208.0),
                device_color(owner),
                colors.bg,
            );
            painter.text(
                middle - Vec2::new(0.0, 12.0),
                egui::Align2::CENTER_CENTER,
                crate::i18n::tr("Общий монитор"),
                FontId::proportional(14.0),
                colors.ink,
            );
            let caption = nodes
                .iter()
                .find(|(n, _, _)| Some(n.key.as_str()) == owner)
                .map(|(n, _, _)| n.name.chars().take(22).collect::<String>())
                .unwrap_or_else(|| {
                    crate::i18n::tr(if remote.is_none() {
                        "Waiting for the other PC"
                    } else if remote.is_some_and(|r| {
                        crate::workspace::Plan::monitor_inputs(&local, r).is_some()
                    }) {
                        "Проверяем видеовход…"
                    } else {
                        "Video connection not set up"
                    })
                });
            painter.text(
                middle + Vec2::new(0.0, 14.0),
                egui::Align2::CENTER_CENTER,
                caption,
                FontId::proportional(12.0),
                if owner.is_some() {
                    device_color(owner)
                } else {
                    colors.muted
                },
            );
        } else {
            painter.line_segment(
                [left, right],
                Stroke::new(1.5, if synced { ACCENT } else { colors.line }),
            );
            painter.circle_filled(middle, 21.0, colors.card);
            painter.text(
                middle,
                egui::Align2::CENTER_CENTER,
                "↔",
                FontId::proportional(25.0),
                if synced { colors.ink } else { muted },
            );
        }
        for index in 0..2 {
            let center = if index == 0 { left } else { right };
            let entry = nodes.get(index);
            let node = entry.map(|(n, _, _)| *n);
            let own = entry.is_some_and(|(_, own, _)| *own);
            let color = entry.map(|(_, _, color)| *color).unwrap_or(muted);
            if shared_monitor {
                tower(painter, center, color, colors.bg);
            } else {
                monitor(
                    painter,
                    center,
                    (rect.width() * 0.32).min(216.0),
                    color,
                    colors.bg,
                );
            }
            let name = node
                .map(|n| {
                    n.name
                        .chars()
                        .take(if shared_monitor { 22 } else { 25 })
                        .collect::<String>()
                })
                .unwrap_or_else(|| crate::i18n::tr("Другой компьютер"));
            painter.text(
                center + Vec2::new(0.0, if shared_monitor { 65.0 } else { -10.0 }),
                egui::Align2::CENTER_CENTER,
                name,
                FontId::proportional(if shared_monitor { 14.0 } else { 16.0 }),
                colors.ink,
            );
            painter.text(
                center + Vec2::new(0.0, if shared_monitor { 84.0 } else { 16.0 }),
                egui::Align2::CENTER_CENTER,
                crate::i18n::tr(if own {
                    "Этот компьютер"
                } else if synced {
                    "Другой компьютер"
                } else {
                    "Ожидает подключения"
                }),
                FontId::proportional(11.0),
                color,
            );
            let icon_y = rect.top() + if shared_monitor { 210.0 } else { 177.0 };
            let devices = node
                .zip(remote)
                .zip(plan)
                .map(|((node, remote), plan)| {
                    plan.input_devices(node, if own { remote } else { &local }, synced)
                })
                .unwrap_or_default();
            keyboard(
                painter,
                egui::pos2(center.x - 62.0, icon_y),
                device_color(devices.keyboard),
            );
            mouse(
                painter,
                egui::pos2(center.x - 19.0, icon_y),
                device_color(devices.mouse),
            );
            let audio = synced
                && plan
                    .is_some_and(|p| node.is_some_and(|n| p.audio_output.as_ref() == Some(&n.key)));
            headphones(
                painter,
                egui::pos2(center.x + 24.0, icon_y),
                if audio { color } else { muted },
            );
            let microphone_origin = if synced {
                if node.is_some_and(|n| n.microphone_active) {
                    plan.and_then(|p| p.microphone_source.as_deref())
                } else {
                    node.filter(|n| n.hardware.microphone_score > 0)
                        .map(|n| n.key.as_str())
                }
            } else {
                None
            };
            microphone(
                painter,
                egui::pos2(center.x + 66.0, icon_y),
                device_color(microphone_origin),
            );
            if audio && nodes.iter().all(|(n, _, _)| n.audio_playing) {
                for (offset, origin, color) in [
                    (18.0, plan.map(|p| p.purple.as_str()), purple),
                    (29.0, plan.map(|p| p.green.as_str()), green),
                ] {
                    let level = self
                        .prefs
                        .workspace
                        .shared
                        .audio_level(origin.unwrap_or_default());
                    if !level.muted && level.volume > 0 {
                        painter.circle_filled(
                            egui::pos2(center.x + offset, icon_y + 24.0),
                            3.5,
                            color,
                        );
                    }
                }
            }
        }
        painter.text(
            egui::pos2(rect.center().x, rect.top() + 12.0),
            egui::Align2::CENTER_CENTER,
            crate::i18n::tr(if synced {
                "Цвет устройства показывает, с какого оно ПК"
            } else {
                "До подключения и синхронизации всё серое"
            }),
            FontId::proportional(11.0),
            colors.muted,
        );
    }

    fn diagnostics(&mut self, ui: &mut egui::Ui) {
        title(ui, "Диагностика", "Состояние служб и последние сообщения.");
        if ui.button(crate::i18n::tr("Copy diagnostics")).clicked() {
            let mut report = format!("Desk Switch {} / protocol {}\nPair synchronized: {}\n{}\n",
                crate::compatibility::VERSION, crate::compatibility::PROTOCOL, self.pair.synced,
                serde_json::to_string_pretty(&serde_json::json!({"local":self.local_node(),"remote":self.pair.remote,"pair_status":self.pair.status})).unwrap_or_default());
            for (name, worker) in [
                ("Input", &self.input),
                ("Monitor", &self.monitor),
                ("Audio", &self.audio),
                ("Microphone", &self.microphone),
            ] {
                report.push_str(&format!("\n{name}:\n"));
                if let Some(worker) = worker {
                    for line in &worker.logs {
                        report.push_str(line);
                        report.push('\n');
                    }
                }
            }
            ui.ctx().copy_text(report);
        }
        card(ui, |ui| {
            subheading(ui, "Профиль этого компьютера");
            ui.label(crate::i18n::ui_text(
                RichText::new(crate::i18n::tr(self.dir.display().to_string()))
                    .monospace()
                    .size(12.0),
            ));
            if let Some(error) = &self.tray_error {
                note(ui, &format!("Системный трей недоступен: {error}"));
            }
            if !self.tray.as_ref().is_some_and(super::tray::Tray::available) {
                note(
                    ui,
                    "Для трея в GNOME нужен индикатор AppIndicator/StatusNotifier. Окно остаётся доступным без него.",
                );
            }
            #[cfg(target_os = "linux")]
            note(
                ui,
                "Wayland: RemoteDesktop v2 + EIS. Монитор: ddcutil и доступ к I²C. Звук: PipeWire и WirePlumber.",
            );
        });
        for (name, worker) in [
            ("Ввод и монитор", &self.input),
            ("Общий монитор", &self.monitor),
            ("Звук", &self.audio),
            ("Shared microphone", &self.microphone),
        ] {
            card(ui, |ui| {
                subheading(ui, name);
                let Some(worker) = worker else {
                    note(ui, "Служба ещё не запускалась.");
                    return;
                };
                note(
                    ui,
                    if worker.running() {
                        "Работает"
                    } else {
                        "Остановлена"
                    },
                );
                egui::ScrollArea::vertical()
                    .id_salt(name)
                    .max_height(230.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &worker.logs {
                            ui.label(crate::i18n::ui_text(
                                RichText::new(crate::i18n::tr(line)).monospace().size(12.0),
                            ));
                        }
                    });
            });
        }
    }

    fn application(&mut self, ui: &mut egui::Ui) {
        title(
            ui,
            "Настройки",
            "Автозапуск, единая версия на обоих ПК и подписанные обновления.",
        );
        let appearance = (self.prefs.theme.clone(), self.prefs.language.clone());
        card(ui, |ui| {
            subheading(ui, "Внешний вид и язык");
            ui.label(crate::i18n::ui_text("Тема"));
            choice(
                ui,
                "appearance-theme",
                &mut self.prefs.theme,
                &[
                    ("system", "Как в системе"),
                    ("light", "Светлая"),
                    ("dark", "Тёмная"),
                ],
            );
            ui.label(crate::i18n::ui_text("Язык интерфейса"));
            choice(
                ui,
                "appearance-language",
                &mut self.prefs.language,
                &crate::i18n::LANGUAGES,
            );
            note(
                ui,
                "Изменения применяются сразу и сохраняются автоматически.",
            );
        });
        if appearance != (self.prefs.theme.clone(), self.prefs.language.clone()) {
            self.appearance_changed(ui.ctx());
        }
        self.device_settings(ui);
        #[cfg(target_os = "linux")]
        card(ui, |ui| {
            subheading(ui, "Keyboard and mouse permissions");
            note(
                ui,
                "Approve once to remember access to the selected devices after USB reconnection and restart. Devices without a serial number are remembered by model and input interface.",
            );
            if ui
                .add_enabled(
                    self.query.is_none() && !self.input_permission_paths().is_empty(),
                    egui::Button::new(crate::i18n::tr("Remember keyboard and mouse access")),
                )
                .clicked()
            {
                self.scan(super::QueryKind::InputAccess);
            }
        });
        card(ui, |ui| {
            subheading(ui, "При входе в систему");
            let changed = ui
                .checkbox(
                    &mut self.prefs.autostart,
                    crate::i18n::tr("Запускать Desk Switch в трее при входе в систему"),
                )
                .changed()
                | ui.checkbox(
                    &mut self.prefs.auto_connect,
                    crate::i18n::tr("Подключать настроенную пару при запуске приложения"),
                )
                .changed();
            if changed {
                self.save_application();
            }
            note(
                ui,
                "Подключение начинается после сопряжения и настройки. На Ubuntu может потребоваться системное подтверждение доступа.",
            );
        });
        card(ui, |ui| {
            subheading(ui, "Версии двух компьютеров");
            ui.label(crate::i18n::ui_text(format!(
                "Этот компьютер: {} · протокол {}",
                crate::compatibility::VERSION,
                crate::compatibility::PROTOCOL
            )));
            let peer = self
                .pair
                .remote
                .as_ref()
                .map(|_| crate::compatibility::VERSION)
                .or_else(|| {
                    [&self.input, &self.audio]
                        .into_iter()
                        .flatten()
                        .find_map(|worker| worker.peer_version.as_deref())
                });
            ui.label(crate::i18n::ui_text(format!(
                "Другой компьютер: {}",
                peer.map(str::to_owned)
                    .unwrap_or_else(|| crate::i18n::tr("версия появится после подключения"))
            )));
            if let Some(error) = &self.last_version_error {
                ui.colored_label(palette(ui).error, crate::i18n::ui_text(error));
            }
            note(
                ui,
                "Приложения сверяют точную версию и протокол. При несовпадении ввод и звук не передаются.",
            );
        });
        card(ui, |ui| {
            subheading(ui, "Обновления");
            if ui
                .checkbox(
                    &mut self.prefs.auto_update,
                    crate::i18n::ui_text("Проверять и скачивать обновления автоматически"),
                )
                .changed()
            {
                let result = self.prefs.save(&self.dir);
                self.message(result, "Настройка обновлений сохранена.");
            }
            note(
                ui,
                "Проверяем при запуске и каждые 6 часов. Для установки нажмите «Обновить и перезапустить». На Ubuntu появится системный запрос пароля.",
            );
            ui.label(crate::i18n::ui_text(&self.update_status));
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.update_job.is_none()
                            && self.install_job.is_none()
                            && !self.install_pending,
                        egui::Button::new(crate::i18n::ui_text("Проверить обновления")),
                    )
                    .clicked()
                {
                    self.check_updates(ui.ctx());
                }
                if ui
                    .add_enabled(
                        self.update_ready,
                        egui::Button::new(crate::i18n::ui_text("Обновить и перезапустить")),
                    )
                    .clicked()
                {
                    self.install_update(ui.ctx());
                }
                if self.update_job.is_some() || self.install_job.is_some() || self.install_pending {
                    ui.spinner();
                }
            });
            note(
                ui,
                "Обновления подписаны ключом проекта. Релиз содержит сборки Windows и Ubuntu одной версии.",
            );
        });
    }
}

impl Desktop {
    fn device_settings(&mut self, ui: &mut egui::Ui) {
        let local = self.local_node();
        let mut nodes = vec![local.clone()];
        if let Some(remote) = &self.pair.remote {
            nodes.push(remote.clone());
        }
        let before = self.prefs.workspace.shared.devices.clone();
        card(ui, |ui| {
            subheading(ui, "Device selection");
            note(
                ui,
                "Auto is enough for most setups. Device choices synchronize between both PCs.",
            );
            if !self.prefs.discovery {
                note(ui, "Turn on network availability to detect devices.");
            }
            for node in nodes {
                if node.key.is_empty() {
                    continue;
                }
                egui::CollapsingHeader::new(&node.name).id_salt((&node.key,"devices")).default_open(node.key == local.key).show(ui, |ui| {
                    let mut choices = self.prefs.workspace.shared.devices_for(&node.key);
                    let original = choices.clone();
                    let can_change = self.pair.remote.as_ref().is_none_or(|r| crate::workspace::Plan::can_disable(&local,r,true)
                        && crate::workspace::Plan::can_disable(&local,r,false) && !crate::workspace::Plan::input_in_use(&local,r));
                    ui.add_enabled_ui(can_change && node.devices.individual_input, |ui| {
                        device_picker(ui, &format!("{}-keyboard",node.key), "Клавиатура", &mut choices.keyboard, &node.devices.keyboards, None);
                        device_picker(ui, &format!("{}-mouse",node.key), "Мышь", &mut choices.mouse, &node.devices.mice, None);
                    });
                    if !can_change { note(ui, "Return control before changing shared input devices."); }
                    if !node.devices.individual_input { note(ui, "Windows shares all attached keyboards and mice as a group."); }
                    device_picker(ui, &format!("{}-speaker",node.key), "Shared speaker / headphones", &mut choices.speaker, &node.devices.speakers, None);
                    note(ui, "This output receives the other PC's audio. Local apps must use the same output to hear both PCs.");
                    let microphone_before = choices.microphone.clone();
                    let automatic_mic = self.prefs.workspace.shared.microphone_binding.as_ref().filter(|b| b.computer == node.key)
                        .map(|binding| node.devices.microphones.iter().find(|d| d.id == binding.device)
                            .map(|d| d.name.clone()).unwrap_or_else(|| crate::i18n::tr("Selected device unavailable")));
                    device_picker(ui, &format!("{}-microphone",node.key), "Microphone", &mut choices.microphone, &node.devices.microphones, automatic_mic.as_deref());
                    if !node.devices.individual_input {
                        device_picker(ui, &format!("{}-bridge",node.key), "Virtual microphone input", &mut choices.microphone_bridge, &node.devices.microphone_bridges, None);
                    }
                    if microphone_before != choices.microphone
                        && self.prefs.workspace.shared.microphone_binding.as_ref().is_some_and(|b| b.computer == node.key) {
                        self.prefs.workspace.shared.microphone_binding = None;
                    }
                    if node.hardware.known && !node.devices.individual_input && node.devices.microphone_bridges.is_empty() {
                        note(ui, "Receiving a microphone on Windows needs a virtual cable. Install VB-CABLE, then select CABLE Output in your calling app.");
                        ui.hyperlink_to(crate::i18n::tr("VB-CABLE setup"), "https://vb-audio.com/Cable/");
                    }
                    if !node.devices.individual_input && node.devices.microphone_bridges.iter().any(|d| d.default) {
                        note(ui, "Keep your regular headphones as the Windows output; select CABLE Output only as the calling app's microphone.");
                    }
                    if choices != original { self.prefs.workspace.shared.devices.insert(node.key.clone(), choices); }
                });
            }
            note(
                ui,
                "A missing selected device pauses sharing; another microphone is never opened automatically.",
            );
        });
        if before != self.prefs.workspace.shared.devices {
            self.shared_changed();
        }
    }
}
