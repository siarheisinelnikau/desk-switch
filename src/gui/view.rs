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
        } else if let Some(request) = &self.pair.outgoing {
            card(ui, |ui| {
                subheading(ui, "Подтвердите связь на другом ПК");
                ui.label(crate::i18n::tr(format!(
                    "Код подтверждения: {}",
                    request.code
                )));
                ui.spinner();
            });
        }
        card(ui, |ui| {
            self.diagram(ui);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(self.connected() && self.pair.synced, |ui| {
                    if primary(ui, "Передать клавиатуру и мышь").clicked() {
                        self.switch();
                    }
                });
                note(ui, "F12 — переключить · F11 — вернуть");
            });
            if let Some(plan) = &self.pair.plan
                && plan.input_source.is_none()
                && self.pair.synced
            {
                note(
                    ui,
                    if self.pair.hardware.input()
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
            if self.pair.input_failed
                || self.pair.audio_failed
                || self
                    .pair
                    .remote
                    .as_ref()
                    .is_some_and(|r| r.input_error.is_some() || r.audio_error.is_some())
            {
                ui.separator();
                note(
                    ui,
                    "Устройство не удалось запустить. Подтвердите системный доступ на нужном ПК; подробности — в диагностике.",
                );
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
        card(ui, |ui| {
            subheading(ui, "Общие настройки пары");
            note(
                ui,
                "Меняйте на любом ПК — второй получит настройки автоматически.",
            );
            let shared = &mut self.prefs.workspace.shared;
            for (label, id, value) in [
                ("Клавиатура и мышь", "input-owner", &mut shared.input),
                ("Где слушать оба ПК", "audio-owner", &mut shared.audio),
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
                });
            }
            ui.horizontal(|ui| {
                ui.checkbox(&mut shared.audio_enabled, crate::i18n::tr("Звук двух ПК"));
                ui.add(egui::Slider::new(&mut shared.volume, 0..=100).suffix(" %"));
                ui.checkbox(&mut shared.muted, crate::i18n::tr("Без звука"));
            });
            note(
                ui,
                "Громкость меняет только звук другого ПК. Собственный звук остаётся как в системе.",
            );
            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut shared.edge,
                    crate::i18n::tr("Переход мышью через край экрана"),
                );
                if ui
                    .small_button(crate::i18n::tr("Поменять ПК местами"))
                    .clicked()
                {
                    shared.purple_on_left = !shared.purple_on_left;
                }
            });
            if ui.small_button(crate::i18n::tr("Разорвать пару")).clicked() {
                self.forget_pair();
            }
        });
        if self.prefs.workspace.shared != before {
            self.shared_changed();
        }
    }

    fn local_monitor(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        card(ui, |ui| {
            egui::CollapsingHeader::new(crate::i18n::tr("Монитор этого ПК")).show(ui,|ui| {
                note(ui,"Каждый ПК запоминает только своё текущее подключение. Вход второго ПК придёт по сети.");
                let old_id=self.prefs.workspace.monitor.id.clone();
                let selected=self.monitors.iter().find(|m|m.id==old_id).map(|m|m.name.as_str()).unwrap_or("Монитор пока не найден");
                egui::ComboBox::from_id_salt("own-monitor").width(ui.available_width()).selected_text(crate::i18n::tr(selected)).show_ui(ui,|ui| {
                    for monitor in &self.monitors {
                        if ui.selectable_value(&mut self.prefs.workspace.monitor.id,monitor.id.clone(),&monitor.name).changed(){
                            self.prefs.workspace.monitor.name=monitor.name.clone();self.prefs.workspace.monitor.fingerprint=monitor.fingerprint.clone();
                        }
                    }
                });
                changed|=ui.checkbox(&mut self.prefs.workspace.monitor.enabled,crate::i18n::tr("Это общий монитор — переключать вместе с управлением")).changed();
                if old_id!=self.prefs.workspace.monitor.id {self.prefs.workspace.monitor.input=None;changed=true;}
                if self.prefs.workspace.monitor.enabled {
                    if self.query.is_some(){ui.spinner();}
                    else if self.prefs.workspace.monitor.input.is_some(){note(ui,"Текущее подключение этого ПК запомнено.");}
                    else {note(ui,"Не удалось прочитать текущее подключение. Включите DDC/CI в меню монитора.");}
                    if ui.small_button(crate::i18n::tr("Запомнить текущее подключение заново")).clicked(){changed=true;}
                    if self.pair.remote.is_some() && self.pair.plan.as_ref().is_some_and(|p|p.monitor.is_none()) {
                        note(ui,"Для переключения выберите этот же общий монитор на втором ПК. Оба текущих входа должны быть разными.");
                    }
                }
                note(ui,"Чтение подключения не переключает монитор. Без настройки обоих ПК передаются только клавиатура и мышь.");
            });
        });
        if changed {
            self.remember_monitor();
        }
    }

    fn diagram(&self, ui: &mut egui::Ui) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 225.0), egui::Sense::hover());
        let painter = ui.painter();
        let colors = palette(ui);
        let local = self.local_node();
        let remote = self.pair.remote.as_ref();
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
            nodes.sort_by_key(|(node, _, _)| (node.key != plan.purple) == plan.purple_on_left);
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
        let source = plan.and_then(|p| p.input_source.as_ref());
        let source_node = nodes
            .iter()
            .find(|(n, _, _)| Some(&n.key) == source)
            .map(|(n, _, c)| (*n, *c));
        let active = source_node.and_then(|(node, _)| match node.input {
            Some(super::InputStatus::Here) => Some(node.key.as_str()),
            Some(super::InputStatus::Peer) => nodes
                .iter()
                .find(|(other, _, _)| other.key != node.key)
                .map(|(n, _, _)| n.key.as_str()),
            _ => None,
        });
        let left = egui::pos2(rect.left() + rect.width() * 0.24, rect.top() + 80.0);
        let right = egui::pos2(rect.left() + rect.width() * 0.76, left.y);
        painter.line_segment(
            [left, right],
            Stroke::new(1.5, if synced { ACCENT } else { colors.line }),
        );
        let middle = egui::pos2(rect.center().x, left.y);
        painter.circle_filled(middle, 21.0, colors.card);
        painter.text(
            middle,
            egui::Align2::CENTER_CENTER,
            "↔",
            FontId::proportional(25.0),
            if synced { colors.ink } else { muted },
        );
        for index in 0..2 {
            let center = if index == 0 { left } else { right };
            let entry = nodes.get(index);
            let node = entry.map(|(n, _, _)| *n);
            let own = entry.is_some_and(|(_, own, _)| *own);
            let color = entry.map(|(_, _, color)| *color).unwrap_or(muted);
            let screen = egui::Rect::from_center_size(
                center,
                Vec2::new((rect.width() * 0.32).min(216.0), 98.0),
            );
            painter.rect_filled(screen.expand(7.0), 12, color.gamma_multiply(0.06));
            painter.rect_filled(screen, 8, colors.bg);
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
            let name = node
                .map(|n| n.name.chars().take(25).collect::<String>())
                .unwrap_or_else(|| crate::i18n::tr("Другой компьютер"));
            painter.text(
                center - Vec2::new(0.0, 10.0),
                egui::Align2::CENTER_CENTER,
                name,
                FontId::proportional(16.0),
                colors.ink,
            );
            painter.text(
                center + Vec2::new(0.0, 16.0),
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
            let icon_y = rect.top() + 177.0;
            let input_color = if !synced {
                muted
            } else if source.is_none() {
                color
            } else if node.is_some_and(|n| Some(n.key.as_str()) == active) {
                source_node.map(|(_, c)| c).unwrap_or(muted)
            } else {
                muted
            };
            let hardware = node.map(|n| &n.hardware);
            keyboard(
                painter,
                egui::pos2(center.x - 39.0, icon_y),
                if source.is_some() || hardware.is_some_and(|h| h.keyboard) {
                    input_color
                } else {
                    muted
                },
            );
            mouse(
                painter,
                egui::pos2(center.x + 4.0, icon_y),
                if source.is_some() || hardware.is_some_and(|h| h.mouse) {
                    input_color
                } else {
                    muted
                },
            );
            let audio = synced
                && plan
                    .is_some_and(|p| node.is_some_and(|n| p.audio_output.as_ref() == Some(&n.key)));
            headphones(
                painter,
                egui::pos2(center.x + 47.0, icon_y),
                if audio { color } else { muted },
            );
            if audio && nodes.iter().all(|(n, _, _)| n.audio_playing) {
                painter.circle_filled(egui::pos2(center.x + 41.0, icon_y + 24.0), 3.5, purple);
                painter.circle_filled(egui::pos2(center.x + 52.0, icon_y + 24.0), 3.5, green);
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
        for (name, worker) in [("Ввод и монитор", &self.input), ("Звук", &self.audio)]
        {
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
