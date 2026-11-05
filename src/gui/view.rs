use super::{Action, Desktop, Page, QueryKind, config};
use eframe::egui::{self, Align, Color32, FontId, RichText, Stroke, Vec2};

const BG: Color32 = Color32::from_rgb(244, 246, 250);
const NAVY: Color32 = Color32::from_rgb(26, 34, 57);
const INK: Color32 = Color32::from_rgb(34, 44, 67);
const MUTED: Color32 = Color32::from_rgb(115, 125, 145);
const LINE: Color32 = Color32::from_rgb(225, 230, 239);
const ACCENT: Color32 = Color32::from_rgb(82, 99, 224);
const GREEN: Color32 = Color32::from_rgb(35, 139, 109);

pub fn style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    let mut style = (*ctx.style_of(egui::Theme::Light)).clone();
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, FontId::proportional(26.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(12.0));
    style.spacing.item_spacing = Vec2::new(10.0, 10.0);
    style.spacing.button_padding = Vec2::new(14.0, 9.0);
    style.spacing.interact_size.y = 34.0;
    style.spacing.slider_width = 240.0;
    style.visuals.override_text_color = Some(INK);
    style.visuals.panel_fill = BG;
    style.visuals.extreme_bg_color = Color32::WHITE;
    style.visuals.selection.bg_fill = ACCENT;
    style.visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.widgets.inactive.bg_fill = LINE;
    style.visuals.slider_trailing_fill = true;
    style.visuals.widgets.inactive.weak_bg_fill = Color32::WHITE;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
    style.visuals.widgets.inactive.corner_radius = 7.into();
    style.visuals.widgets.hovered.corner_radius = 7.into();
    style.visuals.widgets.active.corner_radius = 7.into();
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(234, 237, 252);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    ctx.set_style_of(egui::Theme::Light, style);
}

fn card<R>(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(12)
        .inner_margin(20)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui)
        })
        .inner
}

fn title(ui: &mut egui::Ui, heading: &str, detail: &str) {
    ui.heading(RichText::new(heading).strong());
    ui.label(RichText::new(detail).color(MUTED));
    ui.add_space(12.0);
}

fn subheading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(17.0).strong());
}

fn note(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(MUTED));
}

fn primary(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE))
            .fill(ACCENT)
            .stroke(Stroke::NONE)
            .min_size(Vec2::new(0.0, 40.0)),
    )
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) -> bool {
    ui.label(label);
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(f32::INFINITY),
    )
    .changed()
}

fn choice(ui: &mut egui::Ui, id: &str, value: &mut String, options: &[(&str, &str)]) -> bool {
    let before = value.clone();
    let caption = options
        .iter()
        .find(|(key, _)| *key == value)
        .map(|(_, text)| *text)
        .unwrap_or(value);
    egui::ComboBox::from_id_salt(id)
        .width(ui.available_width().max(160.0))
        .selected_text(caption)
        .show_ui(ui, |ui| {
            for (key, text) in options {
                ui.selectable_value(value, (*key).into(), *text);
            }
        });
    before != *value
}

fn badge(ui: &mut egui::Ui, label: &str, active: bool) {
    let color = if active { GREEN } else { MUTED };
    egui::Frame::new()
        .fill(if active {
            Color32::from_rgb(233, 247, 241)
        } else {
            BG
        })
        .corner_radius(20)
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(7.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, color);
                ui.label(RichText::new(label).size(12.0).color(color));
            });
        });
}

impl Desktop {
    pub(super) fn render(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        egui::Panel::left("navigation")
            .exact_size(200.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(NAVY).inner_margin(20))
            .show(root, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), egui::Sense::hover());
                    let painter = ui.painter();
                    painter.rect_filled(rect, 7, ACCENT);
                    painter.rect_stroke(
                        rect.shrink(7.0),
                        2,
                        Stroke::new(1.5, Color32::WHITE),
                        egui::StrokeKind::Inside,
                    );
                    ui.label(
                        RichText::new("Desk Switch")
                            .color(Color32::WHITE)
                            .size(19.0)
                            .strong(),
                    );
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new("ДВА ПК. ОДНО МЕСТО.")
                        .color(Color32::from_rgb(143, 154, 183))
                        .size(10.0),
                );
                ui.add_space(38.0);
                for (page, label) in [
                    (Page::Overview, "Обзор"),
                    (Page::Devices, "Компьютеры"),
                    (Page::Monitor, "Монитор и ввод"),
                    (Page::Sound, "Звук"),
                    (Page::Application, "Запуск и обновления"),
                    (Page::Diagnostics, "Диагностика"),
                ] {
                    let selected = self.page == page;
                    let button = egui::Button::new(RichText::new(label).color(if selected {
                        Color32::WHITE
                    } else {
                        Color32::from_rgb(170, 181, 207)
                    }))
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
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("Завершить").color(Color32::from_rgb(170, 181, 207)),
                            )
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::NONE),
                        )
                        .clicked()
                    {
                        self.action(Action::Quit, &ctx);
                    }
                    ui.label(
                        RichText::new(
                            if self.tray.as_ref().is_some_and(super::tray::Tray::available) {
                                "Работает в системном трее"
                            } else {
                                "Окно управления открыто"
                            },
                        )
                        .size(11.0)
                        .color(Color32::from_rgb(143, 154, 183)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("v{} · Rust", env!("CARGO_PKG_VERSION")))
                            .size(11.0)
                            .color(Color32::from_rgb(105, 119, 151)),
                    );
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(28))
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
                            Page::Devices => self.computers(ui),
                            Page::Monitor => self.monitor(ui),
                            Page::Sound => self.sound(ui),
                            Page::Application => self.application(ui),
                            Page::Diagnostics => self.diagnostics(ui),
                        }
                        if let Some((message, error)) = self.notice.clone() {
                            ui.add_space(12.0);
                            egui::Frame::new()
                                .fill(if error {
                                    Color32::from_rgb(255, 239, 235)
                                } else {
                                    Color32::from_rgb(232, 239, 255)
                                })
                                .corner_radius(8)
                                .inner_margin(12)
                                .show(ui, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(RichText::new(message).color(if error {
                                            Color32::from_rgb(162, 69, 53)
                                        } else {
                                            ACCENT
                                        }));
                                        if ui.small_button("Скрыть").clicked() {
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
            "Ваше рабочее место",
            "Монитор, клавиатура и мышь — между двумя компьютерами.",
        );
        let (status, connected) = self.input_label();
        card(ui, |ui| {
            ui.horizontal(|ui| {
                subheading(ui, "Управление компьютерами");
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    badge(ui, status, connected)
                });
            });
            self.diagram(ui);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(self.connected() && self.form.monitor_enabled, |ui| {
                    if primary(ui, "Переключить на другой ПК").clicked() {
                        self.switch();
                    }
                });
                ui.label(RichText::new("F12").monospace().color(MUTED));
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .button(if self.input_running() {
                            "Отключить пару"
                        } else if self.public_key.is_empty() || self.peer_key.is_empty() {
                            "Настроить пару"
                        } else {
                            "Подключить пару"
                        })
                        .clicked()
                    {
                        if !self.input_running()
                            && (self.public_key.is_empty() || self.peer_key.is_empty())
                        {
                            self.page = Page::Devices;
                        } else {
                            self.toggle_input();
                        }
                    }
                });
            });
            note(
                ui,
                "Меняется вход выбранного монитора и направление клавиатуры и мыши.",
            );
        });
        ui.add_space(6.0);
        ui.columns(2, |columns| {
            card(&mut columns[0], |ui| {
                subheading(ui, "Общий монитор");
                ui.label(if self.form.monitor_enabled {
                    "Выбран один физический монитор"
                } else {
                    "Настройте входы двух ПК"
                });
                note(ui, "DDC/CI · HDMI / DisplayPort");
                if ui.button("Настроить монитор").clicked() {
                    self.page = Page::Monitor;
                }
            });
            card(&mut columns[1], |ui| {
                subheading(ui, "Звук второго ПК");
                ui.label(if self.audio_running() {
                    "Аудиослужба включена"
                } else {
                    "Выключен"
                });
                note(ui, "Работает независимо от переключения");
                if ui.button("Настроить звук").clicked() {
                    self.page = Page::Sound;
                }
            });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.input_running(),
                    egui::Button::new("Аварийный возврат · F11"),
                )
                .clicked()
            {
                self.action(Action::Emergency, ui.ctx());
            }
            note(ui, "Вернуть управление на ПК с физической клавиатурой.");
        });
        if self.public_key.is_empty() || self.peer_key.is_empty() {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                note(
                    ui,
                    "Первый запуск? Сначала свяжите два компьютера по публичным ключам.",
                );
                if ui.link("Открыть настройку пары").clicked() {
                    self.page = Page::Devices;
                }
            });
        }
    }

    fn diagram(&self, ui: &mut egui::Ui) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 205.0), egui::Sense::hover());
        let painter = ui.painter();
        let here_active = self.connected()
            && self
                .input
                .as_ref()
                .is_some_and(|w| w.input_status == Some(super::InputStatus::Here));
        let other_active = self.connected() && !here_active;
        let left = egui::pos2(rect.left() + rect.width() * 0.24, rect.top() + 77.0);
        let right = egui::pos2(rect.left() + rect.width() * 0.76, left.y);
        let screen_size = Vec2::new((rect.width() * 0.26).min(180.0), 99.0);
        let link_color = if self.connected() { ACCENT } else { LINE };
        painter.line_segment([left, right], Stroke::new(2.0, link_color));
        painter.circle_filled(rect.center() - Vec2::new(0.0, 25.5), 17.0, Color32::WHITE);
        painter.text(
            rect.center() - Vec2::new(0.0, 25.5),
            egui::Align2::CENTER_CENTER,
            "↔",
            FontId::proportional(23.0),
            link_color,
        );
        for (center, active, label, detail) in [
            (
                left,
                here_active,
                "Этот компьютер",
                if self.prefs.role == "host" {
                    "Клавиатура и мышь здесь"
                } else {
                    "Принимает управление"
                },
            ),
            (
                right,
                other_active,
                "Другой компьютер",
                if self.peer_key.is_empty() {
                    "Ожидает сопряжения"
                } else {
                    "Ключ сохранён"
                },
            ),
        ] {
            let screen = egui::Rect::from_center_size(center, screen_size);
            painter.rect_filled(
                screen.expand(5.0),
                10,
                if active {
                    Color32::from_rgb(233, 237, 255)
                } else {
                    BG
                },
            );
            painter.rect_stroke(
                screen,
                7,
                Stroke::new(
                    2.0,
                    if active {
                        ACCENT
                    } else {
                        Color32::from_rgb(175, 186, 205)
                    },
                ),
                egui::StrokeKind::Inside,
            );
            painter.rect_filled(
                screen.shrink(8.0),
                3,
                if active {
                    Color32::from_rgb(222, 228, 255)
                } else {
                    Color32::from_rgb(232, 236, 244)
                },
            );
            painter.line_segment(
                [
                    egui::pos2(center.x, screen.bottom()),
                    egui::pos2(center.x, screen.bottom() + 13.0),
                ],
                Stroke::new(2.0, MUTED),
            );
            painter.line_segment(
                [
                    egui::pos2(center.x - 21.0, screen.bottom() + 14.0),
                    egui::pos2(center.x + 21.0, screen.bottom() + 14.0),
                ],
                Stroke::new(2.0, MUTED),
            );
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                if active {
                    "Управление здесь"
                } else {
                    "ПК"
                },
                FontId::proportional(13.0),
                if active { ACCENT } else { MUTED },
            );
            painter.text(
                egui::pos2(center.x, rect.bottom() - 47.0),
                egui::Align2::CENTER_CENTER,
                label,
                FontId::proportional(15.0),
                INK,
            );
            painter.text(
                egui::pos2(center.x, rect.bottom() - 25.0),
                egui::Align2::CENTER_CENTER,
                detail,
                FontId::proportional(12.0),
                MUTED,
            );
        }
    }

    fn computers(&mut self, ui: &mut egui::Ui) {
        title(
            ui,
            "Два доверенных компьютера",
            "Один раз обменяйтесь публичными ключами напрямую между ПК.",
        );
        card(ui, |ui| {
            subheading(ui, "1. Ключ этого компьютера");
            if self.public_key.is_empty() {
                note(
                    ui,
                    "Создаётся локально. Приватная часть остаётся на этом компьютере.",
                );
                if primary(ui, "Создать ключ").clicked() {
                    let result = config::create_identity(&self.dir).map(|identity| {
                        self.public_key = config::hex(&identity.public);
                    });
                    self.message(result, "Ключ создан. Передайте публичную часть другому ПК.");
                }
            } else {
                ui.label(RichText::new(&self.public_key).monospace().size(12.0));
                if ui.button("Копировать публичный ключ").clicked() {
                    ui.ctx().copy_text(self.public_key.clone());
                }
            }
        });
        card(ui, |ui| {
            subheading(ui, "2. Ключ другого компьютера");
            if self.peer_key.is_empty() {
                ui.add(
                    egui::TextEdit::multiline(&mut self.pair_entry)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("64 символа публичного ключа другого ПК"),
                );
                if ui
                    .add_enabled(
                        !self.public_key.is_empty(),
                        egui::Button::new("Связать компьютеры"),
                    )
                    .clicked()
                {
                    let result = config::pair(&self.dir, self.pair_entry.trim()).and_then(|()| {
                        self.peer_key = config::hex(&config::load_peer(&self.dir)?);
                        self.pair_entry.clear();
                        Ok(())
                    });
                    self.message(
                        result,
                        "Ключ другого компьютера закреплён. Выполните сопряжение и на нём.",
                    );
                }
            } else {
                badge(ui, "Ключ другого ПК закреплён", true);
                ui.label(RichText::new(&self.peer_key).monospace().size(12.0));
            }
        });
        card(ui, |ui| {
            subheading(ui, "3. Роль и подключение");
            ui.add_enabled_ui(!self.input_running(), |ui| {
                ui.label("К этому компьютеру подключена физическая клавиатура и мышь?");
                self.dirty |= choice(ui, "role", &mut self.prefs.role, &[
                    ("host", "Да — передавать управление"), ("receiver", "Нет — принимать управление"),
                ]);
                let receiver = self.prefs.role == "receiver";
                self.dirty |= field(ui, if receiver { "Локальный адрес для приёма ввода" } else { "Адрес принимающего ПК" }, &mut self.prefs.address, "192.168.1.20:45876");
                note(ui, "Числовой IP и порт. Сначала запустите принимающий ПК, затем ПК с клавиатурой.");
                #[cfg(target_os = "linux")]
                if !receiver {
                    ui.separator();
                    ui.label("Клавиатура и относительная мышь · пути /dev/input/by-id/");
                    self.dirty |= ui.add(egui::TextEdit::multiline(&mut self.form.capture_devices)
                        .desired_width(f32::INFINITY).desired_rows(3).hint_text("Один event-путь устройства на строку")).changed();
                    note(ui, "Нужен доступ к выбранным устройствам и согласие в системном диалоге Wayland.");
                }
            });
            self.save_button(ui);
        });
    }

    fn monitor(&mut self, ui: &mut egui::Ui) {
        title(
            ui,
            "Монитор и ввод",
            "Переключайте один общий экран. Остальные мониторы сохраняют свои входы.",
        );
        card(ui, |ui| {
            ui.horizontal(|ui| {
                subheading(ui, "Общий физический монитор");
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled(self.query.is_none(), egui::Button::new("Найти мониторы"))
                        .clicked()
                    {
                        self.scan(QueryKind::Monitors);
                    }
                    if self.query.is_some() {
                        ui.spinner();
                    }
                });
            });
            ui.add_enabled_ui(!self.input_running(), |ui| {
                self.dirty |= ui.checkbox(&mut self.form.monitor_enabled, "Переключать вход монитора вместе с управлением").changed();
                ui.add_enabled_ui(self.form.monitor_enabled, |ui| {
                    let selected = self.monitors.iter().find(|monitor| monitor.id == self.form.monitor_id)
                        .map(|monitor| monitor.name.as_str()).unwrap_or("Выберите монитор из списка");
                    egui::ComboBox::from_id_salt("monitor").width(ui.available_width()).selected_text(selected).show_ui(ui, |ui| {
                        for monitor in &self.monitors {
                            self.dirty |= ui.selectable_value(&mut self.form.monitor_id, monitor.id.clone(), &monitor.name).changed();
                        }
                    });
                    self.dirty |= field(ui, "Локальный ID выбранного монитора", &mut self.form.monitor_id, "Заполняется при выборе из списка");
                    if let Some(monitor) = self.monitors.iter().find(|monitor| monitor.id == self.form.monitor_id) {
                        if let Some(current) = monitor.current { note(ui, &format!("Текущий вход: {current:02X} (VCP 60)")); }
                        if !monitor.inputs.is_empty() {
                            note(ui, &format!("Доступные коды: {}", monitor.inputs.iter().map(|code| format!("{code:02X}")).collect::<Vec<_>>().join(", ")));
                        }
                        if let Some(error) = &monitor.error { ui.colored_label(Color32::from_rgb(168, 83, 62), error); }
                    }
                    ui.columns(2, |columns| {
                        self.dirty |= field(&mut columns[0], "Вход ПК с клавиатурой · hex", &mut self.form.host_code, "Код из проверки монитора");
                        self.dirty |= field(&mut columns[1], "Вход второго ПК · hex", &mut self.form.peer_code, "Код из проверки монитора");
                    });
                    note(ui, "Включите DDC/CI в меню монитора. По очереди выберите вход каждого ПК вручную и запишите его текущий код. Укажите те же два кода на обоих ПК.");
                });
            });
        });
        card(ui, |ui| {
            subheading(ui, "Переход через край экрана");
            ui.add_enabled_ui(!self.input_running(), |ui| {
                self.dirty |= choice(
                    ui,
                    "edge_mode",
                    &mut self.form.edge_mode,
                    &[
                        ("off", "Выключен"),
                        ("input", "Только клавиатура и мышь"),
                        ("video", "Монитор, клавиатура и мышь"),
                    ],
                );
                ui.add_enabled_ui(self.form.edge_mode != "off", |ui| {
                    self.dirty |= field(
                        ui,
                        "Экран для перехода",
                        &mut self.form.edge_screen,
                        if cfg!(windows) {
                            r"\\.\DISPLAY2"
                        } else {
                            "ID области EIS"
                        },
                    );
                    #[cfg(target_os = "linux")]
                    {
                        if ui
                            .add_enabled(
                                self.query.is_none(),
                                egui::Button::new("Запросить области Wayland…"),
                            )
                            .clicked()
                        {
                            self.scan(QueryKind::Wayland);
                        }
                        note(
                            ui,
                            "Откроется системный диалог доступа. Проверка не отправляет ввод.",
                        );
                        for id in &self.wayland_screens {
                            if ui
                                .selectable_label(self.form.edge_screen == *id, id)
                                .clicked()
                            {
                                self.form.edge_screen = id.clone();
                                self.dirty = true;
                            }
                        }
                    }
                    #[cfg(windows)]
                    for monitor in &self.monitors {
                        if ui
                            .selectable_label(
                                self.form.edge_screen == monitor.screen,
                                &monitor.screen,
                            )
                            .clicked()
                        {
                            self.form.edge_screen = monitor.screen.clone();
                            self.dirty = true;
                        }
                    }
                    self.dirty |= choice(
                        ui,
                        "edge_side",
                        &mut self.form.edge_side,
                        &[
                            ("left", "Левый край"),
                            ("right", "Правый край"),
                            ("top", "Верхний край"),
                            ("bottom", "Нижний край"),
                        ],
                    );
                });
            });
            self.save_button(ui);
        });
    }

    fn sound(&mut self, ui: &mut egui::Ui) {
        title(
            ui,
            "Звук второго компьютера",
            "Слушайте удалённый и местный звук в одних наушниках.",
        );
        card(ui, |ui| {
            ui.horizontal(|ui| {
                subheading(ui, "Независимое аудиоподключение");
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    let playing = self.audio.as_ref().is_some_and(|w| w.audio_playing);
                    badge(
                        ui,
                        if playing {
                            if self.prefs.audio_mode == "send" {
                                "Передаётся"
                            } else {
                                "Воспроизводится"
                            }
                        } else if self.audio_running() {
                            "Ожидание соединения"
                        } else {
                            "Выключен"
                        },
                        playing,
                    );
                });
            });
            ui.add_enabled_ui(!self.audio_running(), |ui| {
                ui.label("Роль этого компьютера для звука");
                self.dirty |= choice(ui, "audio_mode", &mut self.prefs.audio_mode, &[
                    ("receive", "Принимать звук — наушники здесь"), ("send", "Передавать системный звук"),
                ]);
                let receive = self.prefs.audio_mode == "receive";
                self.dirty |= field(ui, if receive { "Адрес передающего звук ПК" } else { "Локальный адрес аудиосервера" }, &mut self.prefs.audio_address, "192.168.1.20:45877");
                ui.label(if receive { "Устройство воспроизведения" } else { "Системный микс выбранного устройства" });
                let selected = if receive { &mut self.prefs.audio_output } else { &mut self.prefs.audio_source };
                let caption = self.devices.iter().find(|device| device.id == *selected).map(|d| d.name.as_str())
                    .unwrap_or(if selected.is_empty() { "По умолчанию" } else { selected.as_str() });
                egui::ComboBox::from_id_salt("audio_device").width(ui.available_width()).selected_text(caption).show_ui(ui, |ui| {
                    self.dirty |= ui.selectable_value(selected, String::new(), "По умолчанию").changed();
                    for device in &self.devices {
                        self.dirty |= ui.selectable_value(selected, device.id.clone(), format!("{}{}", device.name, if device.default { " · по умолчанию" } else { "" })).changed();
                    }
                });
                if ui.add_enabled(self.query.is_none(), egui::Button::new("Обновить аудиоустройства")).clicked() { self.scan(QueryKind::Audio); }
                note(ui, "Сначала включите передачу звука на другом ПК. Для аудио используйте отдельный порт.");
            });
            ui.separator();
            if self.prefs.audio_mode == "receive" {
                ui.label("Громкость только удалённого звука");
                ui.horizontal(|ui| {
                    let changed = ui
                        .add(egui::Slider::new(&mut self.prefs.volume, 0..=100).suffix(" %"))
                        .changed();
                    let mute_changed = ui.checkbox(&mut self.prefs.muted, "Без звука").changed();
                    if changed || mute_changed {
                        self.sound_mix();
                    }
                });
            }
            if primary(
                ui,
                if self.audio_running() {
                    "Выключить аудиоподключение"
                } else {
                    "Включить аудиоподключение"
                },
            )
            .clicked()
            {
                self.toggle_audio();
            }
            note(
                ui,
                "Звук продолжает работать при переключении монитора. Выберите выход, который не исчезает при смене HDMI/DP.",
            );
        });
    }

    fn save_button(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.add_enabled_ui(!self.input_running(), |ui| {
            if primary(ui, "Сохранить настройки").clicked() {
                let result = self.save();
                self.message(result, "Настройки сохранены.");
            }
            if self.dirty {
                note(
                    ui,
                    "Есть изменения. При подключении настройки сохранятся автоматически.",
                );
            }
        });
        if self.input_running() {
            note(
                ui,
                "Отключите пару, чтобы изменить настройки ввода и монитора.",
            );
        }
    }

    fn diagnostics(&mut self, ui: &mut egui::Ui) {
        title(ui, "Диагностика", "Состояние служб и последние сообщения.");
        card(ui, |ui| {
            subheading(ui, "Профиль этого компьютера");
            ui.label(
                RichText::new(self.dir.display().to_string())
                    .monospace()
                    .size(12.0),
            );
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
                            ui.label(RichText::new(line).monospace().size(12.0));
                        }
                    });
            });
        }
    }

    fn application(&mut self, ui: &mut egui::Ui) {
        title(
            ui,
            "Запуск и обновления",
            "Автозапуск, единая версия на обоих ПК и подписанные обновления.",
        );
        card(ui, |ui| {
            subheading(ui, "При входе в систему");
            ui.checkbox(
                &mut self.prefs.autostart,
                "Запускать Desk Switch в трее при входе в систему",
            );
            ui.checkbox(
                &mut self.prefs.auto_connect,
                "Подключать настроенную пару при запуске приложения",
            );
            ui.checkbox(
                &mut self.prefs.auto_audio,
                "Включать настроенное аудиоподключение при запуске",
            );
            note(
                ui,
                "Подключение начинается после сопряжения и настройки. На Ubuntu может потребоваться системное подтверждение доступа.",
            );
            if primary(ui, "Сохранить настройки запуска").clicked() {
                self.save_application();
            }
        });
        card(ui, |ui| {
            subheading(ui, "Версии двух компьютеров");
            ui.label(format!(
                "Этот компьютер: {} · протокол {}",
                crate::compatibility::VERSION,
                crate::compatibility::PROTOCOL
            ));
            let peer = [&self.input, &self.audio]
                .into_iter()
                .flatten()
                .find_map(|worker| worker.peer_version.as_deref());
            ui.label(format!(
                "Другой компьютер: {}",
                peer.unwrap_or("версия появится после подключения")
            ));
            if let Some(error) = &self.last_version_error {
                ui.colored_label(Color32::from_rgb(162, 69, 53), error);
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
                    "Проверять и скачивать обновления автоматически",
                )
                .changed()
            {
                let result = self.prefs.save(&self.dir);
                self.message(result, "Настройка обновлений сохранена.");
            }
            note(
                ui,
                "Проверяем при запуске и каждые 6 часов. Устанавливаем при следующем запуске, до подключения ввода.",
            );
            ui.label(&self.update_status);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.update_job.is_none(),
                        egui::Button::new("Проверить обновления"),
                    )
                    .clicked()
                {
                    self.check_updates(ui.ctx());
                }
                if ui
                    .add_enabled(
                        self.update_ready,
                        egui::Button::new("Обновить и перезапустить"),
                    )
                    .clicked()
                {
                    self.install_update(ui.ctx());
                }
                if self.update_job.is_some() {
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
