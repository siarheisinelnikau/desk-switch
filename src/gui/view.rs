use super::{Desktop, Page};
use eframe::egui::{self, Align, Color32, FontId, RichText, Stroke, Vec2};

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
        style.visuals.window_fill = colors.card;
        style.visuals.window_stroke = Stroke::new(1.0, colors.line);
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

fn update_row(
    ui: &mut egui::Ui,
    name: &str,
    version: Option<&crate::compatibility::Version>,
    update: &crate::workspace::management::Update,
    enabled: bool,
) -> bool {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(name).strong());
        if let Some(version) = version {
            note(
                ui,
                &format!("Version {} · protocol {}", version.app, version.protocol),
            );
        }
    });
    let status = if update.status.is_empty() {
        "Check for updates to see the latest signed version."
    } else {
        &update.status
    };
    if update.failed {
        ui.colored_label(palette(ui).error, crate::i18n::tr(status));
    } else {
        ui.label(crate::i18n::tr(status));
    }
    if let Some(progress) = &update.progress {
        let fraction = progress.downloaded as f32 / progress.total.max(1) as f32;
        ui.add(egui::ProgressBar::new(fraction).show_percentage());
    }
    ui.horizontal(|ui| {
        let clicked = ui
            .add_enabled(
                enabled && update.can_install(),
                egui::Button::new(crate::i18n::tr("Update & restart")),
            )
            .on_disabled_hover_text(crate::i18n::tr(if !enabled {
                "Reconnect the PC and wait for its current operation to finish."
            } else {
                "This PC needs a newer verified installer before it can update."
            }))
            .clicked();
        if update.checking || update.installing {
            ui.spinner();
        }
        clicked
    })
    .inner
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

#[derive(Clone, Copy, PartialEq)]
pub(super) enum DeviceIcon {
    Keyboard,
    Mouse,
    Headphones,
    Microphone,
}
impl DeviceIcon {
    pub(super) const ALL: [Self; 4] = [
        Self::Keyboard,
        Self::Mouse,
        Self::Headphones,
        Self::Microphone,
    ];
    pub(super) fn index(self) -> usize {
        self as usize
    }
    fn label(self) -> &'static str {
        match self {
            Self::Keyboard => "Keyboard",
            Self::Mouse => "Mouse",
            Self::Headphones => "Headphones / speakers",
            Self::Microphone => "Microphone",
        }
    }
}

fn paint_device(
    painter: &egui::Painter,
    icon: DeviceIcon,
    center: egui::Pos2,
    scale: f32,
    color: Color32,
) {
    let point = |x, y| center + Vec2::new(x, y) * scale;
    let stroke = |width: f32| Stroke::new((width * scale).max(0.8), color);
    match icon {
        DeviceIcon::Keyboard => {
            let rect = egui::Rect::from_center_size(center, Vec2::new(36.0, 22.0) * scale);
            painter.rect_stroke(rect, 4, stroke(1.5), egui::StrokeKind::Inside);
            for y in 0..2 {
                for x in 0..5 {
                    let p = rect.left_top()
                        + Vec2::new(6.0 + x as f32 * 5.8, 5.5 + y as f32 * 5.0) * scale;
                    painter.rect_filled(
                        egui::Rect::from_center_size(p, Vec2::new(2.6, 2.0) * scale),
                        0,
                        color,
                    );
                }
            }
            painter.line_segment([point(-8.0, 6.0), point(8.0, 6.0)], stroke(1.5));
        }
        DeviceIcon::Mouse => {
            let rect = egui::Rect::from_center_size(center, Vec2::new(17.0, 25.0) * scale);
            painter.rect_stroke(rect, 8, stroke(1.5), egui::StrokeKind::Inside);
            painter.line_segment([point(0.0, -10.0), point(0.0, -2.0)], stroke(1.5));
            painter.line_segment([point(-6.0, -2.0), point(6.0, -2.0)], stroke(1.0));
        }
        DeviceIcon::Headphones => {
            let points = (0..=18)
                .map(|i| {
                    let angle = std::f32::consts::PI * (i as f32 / 18.0);
                    point(-angle.cos() * 12.0, -angle.sin() * 12.0)
                })
                .collect::<Vec<_>>();
            painter.add(egui::Shape::line(points, stroke(2.0)));
            for offset in [-11.0, 11.0] {
                painter.rect_filled(
                    egui::Rect::from_center_size(point(offset, 4.0), Vec2::new(5.0, 12.0) * scale),
                    2,
                    color,
                );
            }
        }
        DeviceIcon::Microphone => {
            painter.rect_stroke(
                egui::Rect::from_center_size(point(0.0, -3.0), Vec2::new(9.0, 17.0) * scale),
                5,
                stroke(1.6),
                egui::StrokeKind::Inside,
            );
            for (from, to) in [
                ((-9.0, 0.0), (-9.0, 5.0)),
                ((9.0, 0.0), (9.0, 5.0)),
                ((-9.0, 5.0), (0.0, 10.0)),
                ((9.0, 5.0), (0.0, 10.0)),
                ((0.0, 10.0), (0.0, 15.0)),
                ((-5.0, 15.0), (5.0, 15.0)),
            ] {
                painter.line_segment([point(from.0, from.1), point(to.0, to.1)], stroke(1.5));
            }
        }
    }
}
fn keyboard(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    paint_device(painter, DeviceIcon::Keyboard, center, 1.0, color);
}
fn mouse(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    paint_device(painter, DeviceIcon::Mouse, center, 1.0, color);
}
fn headphones(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    paint_device(painter, DeviceIcon::Headphones, center, 1.0, color);
}
fn microphone(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    paint_device(painter, DeviceIcon::Microphone, center, 1.0, color);
}

fn monitor(painter: &egui::Painter, center: egui::Pos2, width: f32, color: Color32, bg: Color32) {
    monitor_scaled(painter, center, width, 1.0, color, bg);
}
fn monitor_scaled(
    painter: &egui::Painter,
    center: egui::Pos2,
    width: f32,
    scale: f32,
    color: Color32,
    bg: Color32,
) {
    let screen = egui::Rect::from_center_size(center, Vec2::new(width, 98.0 * scale));
    painter.rect_filled(screen.expand(7.0 * scale), 12, color.gamma_multiply(0.06));
    painter.rect_filled(screen, 8, bg);
    painter.rect_stroke(
        screen,
        8,
        Stroke::new((2.0 * scale).max(1.0), color),
        egui::StrokeKind::Inside,
    );
    painter.rect_filled(
        egui::Rect::from_center_size(
            center + Vec2::new(0.0, 55.0) * scale,
            Vec2::new(4.0, 14.0) * scale,
        ),
        1,
        color,
    );
    painter.rect_filled(
        egui::Rect::from_center_size(
            center + Vec2::new(0.0, 64.0) * scale,
            Vec2::new(44.0, 3.0) * scale,
        ),
        2,
        color,
    );
}

fn tower(painter: &egui::Painter, center: egui::Pos2, color: Color32, bg: Color32) {
    tower_scaled(painter, center, 1.0, color, bg);
}
fn tower_scaled(
    painter: &egui::Painter,
    center: egui::Pos2,
    scale: f32,
    color: Color32,
    bg: Color32,
) {
    let case = egui::Rect::from_center_size(center, Vec2::new(64.0, 94.0) * scale);
    painter.rect_filled(case.expand(7.0 * scale), 12, color.gamma_multiply(0.06));
    painter.rect_filled(case, 8, bg);
    painter.rect_stroke(
        case,
        8,
        Stroke::new((2.0 * scale).max(1.0), color),
        egui::StrokeKind::Inside,
    );
    painter.circle_stroke(
        center + Vec2::new(0.0, -23.0) * scale,
        5.0 * scale,
        Stroke::new((1.5 * scale).max(0.8), color),
    );
    for y in [2.0, 10.0, 18.0] {
        painter.line_segment(
            [
                center + Vec2::new(-16.0, y) * scale,
                center + Vec2::new(16.0, y) * scale,
            ],
            Stroke::new((1.5 * scale).max(0.8), color),
        );
    }
    painter.circle_filled(center + Vec2::new(0.0, 33.0) * scale, 2.0 * scale, color);
}

pub(super) fn peer_color(index: usize) -> Color32 {
    [
        Color32::from_rgb(76, 211, 163),
        Color32::from_rgb(175, 142, 255),
        Color32::from_rgb(92, 185, 239),
        Color32::from_rgb(244, 176, 103),
    ][index % 4]
}

#[derive(Clone, PartialEq)]
pub(super) struct DeviceSource {
    key: String,
    pub(super) name: String,
    pub(super) color: Color32,
    muted: bool,
}
#[derive(Clone, PartialEq)]
struct MonitorParticipant {
    source: DeviceSource,
    input: Option<u16>,
    online: bool,
    selected: bool,
}
#[derive(Clone, PartialEq)]
pub(super) struct SharedMonitorVisual {
    current: Option<DeviceSource>,
    requested: Option<DeviceSource>,
    pending: bool,
    error: Option<String>,
    participants: Vec<MonitorParticipant>,
}
#[derive(Clone, Default, PartialEq)]
pub(super) struct WorkspaceVisual {
    pub(super) devices: [Option<DeviceSource>; 4],
    pub(super) monitor: Option<SharedMonitorVisual>,
}
impl WorkspaceVisual {
    pub(super) fn from_workspace(state: &crate::group::model::Workspace) -> Self {
        let plan = state.plan();
        let sources = state
            .members
            .iter()
            .enumerate()
            .map(|(index, (key, member))| {
                (
                    key.as_str(),
                    DeviceSource {
                        key: key.clone(),
                        name: member.node.name.clone(),
                        color: peer_color(index),
                        muted: false,
                    },
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let source = |key: &str, muted| {
            sources.get(key).map(|source| {
                let mut source = source.clone();
                source.muted = muted;
                source
            })
        };
        let input = plan.input.as_deref().and_then(|key| source(key, false));
        let audio = plan.audio.as_deref().and_then(|key| {
            let member = &state.members[key];
            let level = member
                .native_output
                .or(member.preferences.output)
                .unwrap_or_default();
            source(key, level.muted || level.volume == 0)
        });
        let microphone = plan.microphone.as_ref().and_then(|binding| {
            source(
                &binding.computer,
                state.settings.microphone_level.muted
                    || state.settings.microphone_level.volume == 0,
            )
        });
        let monitor = state.settings.monitor_enabled.then(|| {
            let command = state.monitor_command.as_ref();
            SharedMonitorVisual {
                current: state
                    .confirmed_monitor
                    .as_deref()
                    .and_then(|key| source(key, false)),
                requested: command.and_then(|command| source(&command.destination, false)),
                pending: command.is_some_and(|command| {
                    command.result.is_none() && command.interrupted.is_none()
                }),
                error: command.and_then(|command| {
                    command
                        .result
                        .as_ref()
                        .and_then(|result| result.error.clone())
                        .or_else(|| command.interrupted.clone())
                }),
                participants: state
                    .members
                    .values()
                    .filter(|member| member.node.monitor.enabled)
                    .filter_map(|member| {
                        source(&member.node.key, false).map(|source| MonitorParticipant {
                            selected: state.confirmed_monitor.as_deref()
                                == Some(member.node.key.as_str()),
                            source,
                            input: member.node.monitor.input,
                            online: member.online,
                        })
                    })
                    .collect(),
            }
        });
        Self {
            devices: [
                state
                    .settings
                    .keyboard_enabled
                    .then(|| input.clone())
                    .flatten(),
                state.settings.mouse_enabled.then_some(input).flatten(),
                audio,
                microphone,
            ],
            monitor,
        }
    }

    pub(super) fn member_devices(
        &self,
        state: &crate::group::model::Workspace,
        key: &str,
    ) -> [Option<DeviceSource>; 4] {
        let Some(member) = state.members.get(key).filter(|member| member.compatible()) else {
            return Default::default();
        };
        std::array::from_fn(|index| {
            self.devices[index]
                .as_ref()
                .filter(|source| match index {
                    0 | 1 => state.active_input.as_deref().unwrap_or(&source.key) == key,
                    // Every compatible computer sends to the selected output.
                    2 => true,
                    _ => source.key == key || member.node.hardware.microphone_receive,
                })
                .cloned()
                .map(|mut source| {
                    if index == DeviceIcon::Headphones.index() && source.key != key {
                        source.muted |=
                            member.preferences.audio.muted || member.preferences.audio.volume == 0;
                    }
                    source
                })
        })
    }
}

pub(super) fn device_icon(
    ui: &mut egui::Ui,
    icon: DeviceIcon,
    source: Option<&DeviceSource>,
    scale: f32,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(40.0, 34.0) * scale, egui::Sense::hover());
    let color = source
        .map(|source| {
            if source.muted {
                source.color.gamma_multiply(0.55)
            } else {
                source.color
            }
        })
        .unwrap_or_else(|| ui.visuals().weak_text_color().gamma_multiply(0.5));
    paint_device(ui.painter(), icon, rect.center(), scale, color);
    if source.is_some_and(|source| source.muted) {
        ui.painter().line_segment(
            [
                rect.center() + Vec2::new(-15.0, 12.0) * scale,
                rect.center() + Vec2::new(15.0, -12.0) * scale,
            ],
            Stroke::new((1.5 * scale).max(1.0), color),
        );
    }
    response.on_hover_text(format!(
        "{} · {}{}",
        crate::i18n::tr(icon.label()),
        source
            .map(|source| source.name.clone())
            .unwrap_or_else(|| crate::i18n::tr("Not shared")),
        if source.is_some_and(|source| source.muted) {
            format!(" · {}", crate::i18n::tr("Mute"))
        } else {
            String::new()
        },
    ))
}

pub(super) fn device_checkbox(
    ui: &mut egui::Ui,
    icon: DeviceIcon,
    value: &mut bool,
    label: &str,
    source: Option<&DeviceSource>,
    enabled: bool,
) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        device_icon(ui, icon, source, 0.65);
        ui.add_enabled(enabled, egui::Checkbox::new(value, crate::i18n::tr(label)))
    })
    .inner
}

pub(super) fn shared_monitor(ui: &mut egui::Ui, visual: &SharedMonitorVisual, compact: bool) {
    let colors = palette(ui);
    let current = visual.current.as_ref();
    let color = current
        .map(|source| source.color)
        .unwrap_or(colors.muted.gamma_multiply(0.55));
    let status = if visual.error.is_some() {
        Some(crate::i18n::tr("Monitor switch failed"))
    } else if visual.pending {
        Some(format!(
            "{} · {}",
            crate::i18n::tr("Switching…"),
            visual
                .requested
                .as_ref()
                .map(|source| source.name.as_str())
                .unwrap_or("—")
        ))
    } else {
        None
    };
    let name = current
        .map(|source| source.name.clone())
        .unwrap_or_else(|| crate::i18n::tr("Monitor input unknown"));
    let layout = if !compact && ui.available_width() < 560.0 {
        egui::Layout::top_down(Align::Min)
    } else {
        egui::Layout::left_to_right(Align::Center)
    };
    ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 0.0), layout, |ui| {
        let (size, scale, width) = if compact {
            (Vec2::new(104.0, 62.0), 0.42, 94.0)
        } else {
            (Vec2::new(216.0, 126.0), 0.82, 196.0)
        };
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
        let center = egui::pos2(rect.center().x, rect.top() + 50.0 * scale);
        monitor_scaled(ui.painter(), center, width, scale, color, colors.bg);
        if !compact {
            ui.place(
                egui::Rect::from_center_size(center, Vec2::new(width - 18.0, 28.0)),
                egui::Label::new(RichText::new(crate::i18n::tr("Shared monitor")).size(14.0))
                    .truncate(),
            );
        }
        response.on_hover_text(crate::i18n::tr(
            "The monitor view updates when you switch with Desk Switch.",
        ));
        ui.vertical(|ui| {
            if compact {
                ui.strong(crate::i18n::tr("Shared monitor"));
            }
            ui.add(egui::Label::new(RichText::new(&name).color(color)).truncate())
                .on_hover_text(&name);
            if let Some(status) = &status {
                ui.add(
                    egui::Label::new(RichText::new(status).small().color(
                        if visual.error.is_some() {
                            colors.error
                        } else {
                            colors.muted
                        },
                    ))
                    .truncate(),
                )
                .on_hover_text(
                    visual
                        .error
                        .as_deref()
                        .map(crate::i18n::tr)
                        .unwrap_or_else(|| status.clone()),
                );
            }
            if !compact {
                ui.horizontal_wrapped(|ui| {
                    for participant in &visual.participants {
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::new(92.0, 76.0), egui::Sense::hover());
                        let tower_color = if participant.online {
                            participant.source.color
                        } else {
                            colors.muted.gamma_multiply(0.55)
                        };
                        if participant.selected {
                            ui.painter().rect_filled(
                                rect.shrink(1.0),
                                7,
                                tower_color.gamma_multiply(0.08),
                            );
                        }
                        tower_scaled(
                            ui.painter(),
                            egui::pos2(rect.center().x, rect.top() + 26.0),
                            0.45,
                            tower_color,
                            colors.card,
                        );
                        ui.place(
                            egui::Rect::from_min_size(
                                rect.left_top() + Vec2::new(3.0, 52.0),
                                Vec2::new(rect.width() - 6.0, 20.0),
                            ),
                            egui::Label::new(
                                RichText::new(&participant.source.name)
                                    .size(11.0)
                                    .color(tower_color),
                            )
                            .truncate(),
                        );
                        let input = participant
                            .input
                            .map(|input| match input {
                                15 => "DisplayPort".into(),
                                16 => "DisplayPort 2".into(),
                                17 => "HDMI 1".into(),
                                18 => "HDMI 2".into(),
                                _ => format!("0x{input:02x}"),
                            })
                            .unwrap_or_else(|| crate::i18n::tr("Monitor port is not configured"));
                        response.on_hover_text(format!(
                            "{} · {} · {}",
                            participant.source.name,
                            input,
                            crate::i18n::tr(if participant.online {
                                "Connected"
                            } else {
                                "Offline"
                            })
                        ));
                    }
                });
            }
        });
    });
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
        #[cfg(debug_assertions)]
        if self.preview_popup {
            self.quick_preview(root);
            return;
        }
        let ctx = root.ctx().clone();
        self.workspace_toolbar(root);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette(root).bg).inner_margin(26))
            .show(root, |ui| {
                let scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
                #[cfg(debug_assertions)]
                let scroll = if self.preview.is_some() && ui.ctx().cumulative_pass_nr() < 3 {
                    scroll.vertical_scroll_offset(self.preview_scroll)
                } else {
                    scroll
                };
                scroll.show(ui, |ui| {
                    if self.quitting {
                        title(
                            ui,
                            "Завершаем работу",
                            "Возвращаем управление и останавливаем фоновые службы…",
                        );
                        ui.spinner();
                        return;
                    }
                    if self.group.workspace.is_some() {
                        self.group_overview(ui);
                    } else {
                        self.overview(ui);
                    }
                    if self.page == Page::Overview && self.notice.is_some() {
                        ui.add_space(10.0);
                        self.notice_banner(ui);
                    }
                });
            });
        if !self.quitting {
            if !self.group.layout_editor.is_open() {
                self.settings_windows(&ctx);
            }
            self.group_add_dialog(&ctx);
            self.group_layout_dialog(&ctx);
        }
    }

    fn notice_banner(&mut self, ui: &mut egui::Ui) {
        let Some((message, error)) = self.notice.clone() else {
            return;
        };
        let colors = palette(ui);
        egui::Frame::new()
            .fill(colors.bg)
            .stroke(Stroke::new(
                1.0,
                if error { colors.error } else { colors.line },
            ))
            .corner_radius(8)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::Label::new(RichText::new(crate::i18n::tr(message)).color(if error {
                            colors.error
                        } else {
                            colors.ink
                        }))
                        .wrap(),
                    );
                    if ui.small_button(crate::i18n::tr("Скрыть")).clicked() {
                        self.notice = None;
                    }
                });
            });
        ui.add_space(10.0);
    }

    fn workspace_toolbar(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        let logo_id = egui::Id::new("desk-switch-toolbar-logo");
        let logo =
            if let Some(logo) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(logo_id)) {
                logo
            } else {
                let image =
                    egui::ColorImage::from_rgba_unmultiplied([32, 32], &super::tray::icon_rgba(32));
                let logo = ctx.load_texture("Desk Switch", image, egui::TextureOptions::LINEAR);
                ctx.data_mut(|data| data.insert_temp(logo_id, logo.clone()));
                logo
            };
        egui::Panel::top("workspace-toolbar")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(palette(root).card)
                    .stroke(Stroke::new(1.0, palette(root).line))
                    .inner_margin(egui::Margin::symmetric(24, 16)),
            )
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.image((logo.id(), Vec2::splat(32.0)));
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new("Desk Switch").size(23.0).strong());
                        ui.label(
                            RichText::new(crate::i18n::tr("YOUR COMPUTERS. ONE WORKSPACE."))
                                .size(10.0)
                                .color(palette(ui).muted),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let settings_label = crate::i18n::tr("Settings");
                        let settings = ui
                            .add_enabled(
                                !self.quitting,
                                egui::Button::new(RichText::new("⚙").size(23.0))
                                    .selected(self.page != Page::Overview)
                                    .min_size(Vec2::splat(42.0)),
                            )
                            .on_hover_text(&settings_label);
                        settings.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                !self.quitting,
                                &settings_label,
                            )
                        });
                        if settings.clicked() {
                            self.group.add_open = false;
                            self.page = Page::Application;
                        }
                        egui::Frame::new()
                            .fill(palette(ui).selection)
                            .stroke(Stroke::new(1.0, ACCENT))
                            .corner_radius(8)
                            .inner_margin(egui::Margin::symmetric(12, 7))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(format!("v{}", crate::compatibility::VERSION))
                                        .strong(),
                                );
                            })
                            .response
                            .on_hover_text(format!(
                                "{} {}",
                                crate::i18n::tr("Installed version:"),
                                crate::compatibility::VERSION,
                            ));
                        if self.group.workspace.is_some()
                            && ui
                                .add_enabled(
                                    !self.quitting,
                                    egui::Button::new(format!(
                                        "+ {}",
                                        crate::i18n::tr("Add a computer")
                                    ))
                                    .min_size(Vec2::new(0.0, 40.0)),
                                )
                                .clicked()
                        {
                            self.page = Page::Overview;
                            self.group.add_open = true;
                        }
                    });
                });
            });
    }

    fn settings_windows(&mut self, ctx: &egui::Context) {
        let available = ctx.content_rect();
        let max_size = (available.size() - Vec2::new(48.0, 132.0)).max(Vec2::new(320.0, 240.0));
        let size = Vec2::new(920.0, 780.0).min(max_size);
        let position = egui::pos2(available.center().x - size.x * 0.5, available.top() + 100.0);
        match self.page {
            Page::Application => {
                let mut open = true;
                egui::Window::new(crate::i18n::tr("Settings"))
                    .id(egui::Id::new("workspace-settings"))
                    .open(&mut open)
                    .collapsible(false)
                    .default_pos(position)
                    .default_size(size)
                    .min_size([560.0, 360.0])
                    .max_size(max_size)
                    .show(ctx, |ui| {
                        self.notice_banner(ui);
                        egui::ScrollArea::vertical()
                            .id_salt("workspace-settings-scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                if self.group.workspace.is_some() {
                                    self.group_settings(ui);
                                } else {
                                    self.application(ui);
                                }
                                ui.add_space(12.0);
                                ui.separator();
                                egui::CollapsingHeader::new(crate::i18n::tr("Advanced"))
                                    .id_salt("settings-advanced")
                                    .show(ui, |ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            if ui.button(crate::i18n::tr("Diagnostics")).clicked() {
                                                self.page = Page::Diagnostics;
                                            }
                                            if self.group.workspace.is_some()
                                                && ui
                                                    .add_enabled(
                                                        self.group.selected == self.public_key,
                                                        egui::Button::new(crate::i18n::tr(
                                                            "Retry devices",
                                                        )),
                                                    )
                                                    .on_disabled_hover_text(crate::i18n::tr(
                                                        "Select this PC to retry its devices.",
                                                    ))
                                                    .clicked()
                                            {
                                                self.group.data.retry();
                                            }
                                        });
                                    });
                            });
                    });
                if !open {
                    self.page = Page::Overview;
                }
            }
            Page::Diagnostics => {
                let mut open = true;
                egui::Window::new(crate::i18n::tr("Diagnostics"))
                    .id(egui::Id::new("workspace-diagnostics"))
                    .open(&mut open)
                    .collapsible(false)
                    .default_pos(position)
                    .default_size(size)
                    .min_size([560.0, 360.0])
                    .max_size(max_size)
                    .show(ctx, |ui| {
                        self.notice_banner(ui);
                        egui::ScrollArea::vertical()
                            .id_salt("workspace-diagnostics-scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.diagnostics(ui));
                    });
                if !open {
                    self.page = Page::Application;
                }
            }
            Page::Overview => {}
        }
    }

    fn overview(&mut self, ui: &mut egui::Ui) {
        title(
            ui,
            "Your computers. One workspace.",
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
            self.monitor_choice(ui);
            self.diagram(ui);
            let monitor_only = self
                .pair
                .plan
                .as_ref()
                .is_some_and(|p| p.input_source.is_none())
                && self.prefs.workspace.shared.monitor_enabled;
            ui.horizontal(|ui| {
                ui.add_enabled_ui(
                    if monitor_only {
                        self.monitor_switch_ready()
                    } else {
                        self.input_switch_ready()
                    },
                    |ui| {
                        let label = if monitor_only {
                            "Switch shared monitor"
                        } else if self.prefs.workspace.shared.monitor_enabled {
                            "Switch monitor, keyboard and mouse"
                        } else {
                            "Передать клавиатуру и мышь"
                        };
                        if primary(ui, label).clicked() {
                            if monitor_only {
                                self.switch_monitor(ui.ctx());
                            } else {
                                self.switch();
                            }
                        }
                    },
                );
                note(
                    ui,
                    if monitor_only {
                        "Switch back using Desk Switch on the other PC."
                    } else {
                        "F12 — переключить · F11 — вернуть"
                    },
                );
            });
            if self.prefs.workspace.shared.monitor_enabled {
                ui.horizontal(|ui| {
                    if ui
                        .small_button(crate::i18n::tr("Set up shared monitor…"))
                        .clicked()
                    {
                        self.action(super::Action::MonitorSetup, ui.ctx());
                    }
                });
            }
            if self.prefs.workspace.shared.monitor_enabled {
                let local = self.local_node();
                for node in [&local].into_iter().chain(self.pair.remote.iter()) {
                    let status =
                        if let Some(input) = node.monitor.input.filter(|_| node.monitor.enabled) {
                            format!(
                                "{} ({input:#04X})",
                                crate::i18n::tr("Connection remembered")
                            )
                        } else {
                            crate::i18n::tr("Remember the connection on this PC")
                        };
                    note(ui, &format!("{}: {status}", node.name));
                    if let Some(error) = &node.monitor_error {
                        ui.colored_label(
                            palette(ui).error,
                            format!(
                                "{}: {}",
                                node.name,
                                crate::i18n::tr(format!("Switch failed: {error}"))
                            ),
                        );
                    }
                }
                let configured =
                    self.pair.remote.as_ref().is_some_and(|r| {
                        crate::workspace::Plan::monitor_inputs(&local, r).is_some()
                    });
                if !configured {
                    if self
                        .pair
                        .plan
                        .as_ref()
                        .is_some_and(|p| p.input_source.is_some())
                        && !self
                            .pair
                            .remote
                            .as_ref()
                            .is_some_and(|r| crate::workspace::Plan::input_in_use(&local, r))
                        && ui
                            .add_enabled(
                                self.connected() && self.pair.synced,
                                egui::Button::new(crate::i18n::tr(
                                    "Switch keyboard and mouse only for setup",
                                )),
                            )
                            .clicked()
                    {
                        self.switch_input_for_setup();
                    }
                    if self.pair.remote.as_ref().is_none_or(|remote| {
                        matches!(
                            crate::workspace::Plan::monitor_setup(&local, remote),
                            crate::workspace::MonitorSetup::ThisPcMissing
                                | crate::workspace::MonitorSetup::OtherPcMissing
                        )
                    }) {
                        note(
                            ui,
                            "One shared monitor is shown in the middle. Remember this PC's video connection on each PC to enable switching.",
                        );
                    }
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
                    } else if self.pair.remote.as_ref().is_some_and(|remote| {
                        crate::workspace::Plan::monitor_setup(&local, remote)
                            == crate::workspace::MonitorSetup::OtherPcMissing
                    }) {
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
                    || self.pair.monitor_failed
                    || self.pair.audio_failed
                    || self.pair.microphone_failed
                    || self.pair.remote.as_ref().is_some_and(|r| {
                        r.input_error.is_some()
                            || r.monitor_error.is_some()
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
                            } else if peer.manageable() {
                                note(ui, "Connect to manage settings and updates. Input and audio wait for matching versions.");
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
                                peer.manageable() && !self.pair.busy && already,
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
                    note(
                        ui,
                        if output.is_some_and(|output| output.key == node.key) {
                            "System volume of the shared output"
                        } else {
                            "Relative to the shared output's system volume"
                        },
                    );
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
                "The listening PC controls system volume. The other PC is relative to it: 90% × 50% = 45%.",
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
    }

    fn monitor_choice(&mut self, ui: &mut egui::Ui) {
        let local = self.local_node();
        let active = self.pair.remote.as_ref().is_some_and(|remote| {
            crate::workspace::Plan::input_in_use(&local, remote)
                && self
                    .pair
                    .plan
                    .as_ref()
                    .is_some_and(|plan| plan.monitor.is_some())
        });
        if ui
            .add_enabled(
                !active,
                egui::Checkbox::new(
                    &mut self.prefs.workspace.shared.monitor_enabled,
                    crate::i18n::tr("Общий монитор"),
                ),
            )
            .on_disabled_hover_text(crate::i18n::tr(
                "Сначала верните управление: это единственное устройство ввода на принимающем ПК.",
            ))
            .changed()
        {
            self.shared_changed();
        }
    }

    fn local_monitor(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let open = self.monitor_setup_open;
        let local = self.local_node();
        let can_configure = self.can_remember_monitor();
        card(ui, |ui| {
            let header = egui::CollapsingHeader::new(crate::i18n::tr("Monitor connection on this PC"))
                .id_salt("local-monitor")
                .open(open.then_some(true))
                .show(ui, |ui| {
                    if !self.prefs.workspace.shared.monitor_enabled {
                        note(ui, "Select Shared monitor above to configure video input switching.");
                        return;
                    }
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
                        if old_id != self.prefs.workspace.monitor.id {
                            self.prefs.workspace.monitor.input = None;
                            self.prefs.workspace.pending_monitor = None;
                            changed = true;
                        }
                        if self.query.is_some() { ui.spinner(); }
                        else if self.prefs.workspace.pending_monitor.is_some() {
                            note(ui, "Connection saved. Return the borrowed keyboard and mouse to enable monitor switching safely.");
                        } else if self.prefs.workspace.monitor.input.is_some() {
                            note(ui, "Текущее подключение этого ПК запомнено.");
                        } else {
                            note(ui, "Video connection not set up");
                        }
                        if ui.small_button(crate::i18n::tr("Запомнить текущее подключение заново")).clicked() {
                            self.prefs.workspace.monitor.enabled = true;
                            changed = true;
                        }
                        if self.pair.remote.as_ref().is_some_and(|remote| crate::workspace::Plan::monitor_inputs(&local, remote).is_none()) {
                            note(ui, "Для переключения выберите этот же общий монитор на втором ПК. Оба текущих входа должны быть разными.");
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
                    } else {
                        remote
                            .map(|remote| {
                                crate::workspace::Plan::monitor_setup(&local, remote).caption()
                            })
                            .unwrap_or("Waiting for the other PC")
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
            let output_level = plan
                .and_then(|p| p.audio_output.as_deref())
                .map(|key| self.prefs.workspace.shared.audio_level(key));
            if audio
                && nodes.iter().all(|(n, _, _)| n.audio_playing)
                && output_level.is_some_and(|level| !level.muted && level.volume > 0)
            {
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
        if self.group.workspace.is_some() {
            self.group_diagnostics(ui);
            return;
        }
        note(ui, "Состояние служб и последние сообщения.");
        ui.add_space(12.0);
        if ui.button(crate::i18n::tr("Copy diagnostics")).clicked() {
            let mut report = format!("Desk Switch {} / protocol {}\nPair synchronized: {}\n{}\n",
                crate::compatibility::VERSION, crate::compatibility::PROTOCOL, self.pair.synced,
                serde_json::to_string_pretty(&serde_json::json!({"local":self.local_node(),"remote":self.pair.remote.as_ref().or(self.pair.last_remote.as_ref()),"peer_connected":self.pair.remote.is_some(),"pair_status":self.pair.status})).unwrap_or_default());
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
            subheading(ui, "Diagnostics from the other PC");
            if let Some(remote) = self.pair.remote.as_ref().or(self.pair.last_remote.as_ref()) {
                note(ui, &remote.name);
                if self.pair.remote.is_none() {
                    note(ui, "Offline: showing the last received diagnostics.");
                }
                egui::ScrollArea::vertical()
                    .id_salt("remote-diagnostics")
                    .max_height(230.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &remote.diagnostics.logs {
                            ui.label(crate::i18n::ui_text(
                                RichText::new(crate::i18n::tr(line)).monospace().size(12.0),
                            ));
                        }
                    });
            } else {
                note(
                    ui,
                    "Reconnect the linked PC to receive its latest diagnostics.",
                );
            }
            note(
                ui,
                "The report includes recent service logs from both linked PCs. Logs travel only over the encrypted paired connection.",
            );
        });
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
        note(
            ui,
            "Manage both PCs, their application settings and signed updates.",
        );
        ui.add_space(12.0);
        self.update_settings(ui);
        self.application_settings(ui);
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
    }

    fn settings_peer_name(&self) -> String {
        self.pair
            .remote
            .as_ref()
            .or(self.pair.last_remote.as_ref())
            .map(|node| node.name.clone())
            .or_else(|| {
                (!self.prefs.workspace.peer_name.is_empty())
                    .then(|| self.prefs.workspace.peer_name.clone())
            })
            .unwrap_or_else(|| crate::i18n::tr("Other PC"))
    }

    fn application_settings(&mut self, ui: &mut egui::Ui) {
        let peer_name = self.settings_peer_name();
        let peer = self
            .pair
            .remote
            .as_ref()
            .and_then(|node| node.management.as_ref())
            .cloned();
        card(ui, |ui| {
            subheading(ui, "Application settings");
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.settings_remote, false, crate::i18n::tr("This PC"));
                ui.add_enabled_ui(!self.peer_key.is_empty(), |ui| {
                    ui.selectable_value(&mut self.settings_remote, true, &peer_name);
                });
            });
            let windows = if self.settings_remote {
                self.pair
                    .remote
                    .as_ref()
                    .is_some_and(|node| node.diagnostics.platform == "windows")
            } else {
                cfg!(windows)
            };
            let application = if self.settings_remote {
                self.management_pending
                    .as_ref()
                    .and_then(|pending| pending.application.clone())
                    .or_else(|| peer.as_ref().map(|state| state.application.clone()))
            } else {
                Some(self.local_application())
            };
            let Some(mut application) = application else {
                note(
                    ui,
                    "Reconnect the other PC to edit its application settings.",
                );
                return;
            };
            let before = application.clone();
            let enabled = !self.settings_remote || self.management_pending.is_none();
            ui.add_enabled_ui(enabled, |ui| {
                ui.label(crate::i18n::tr("Тема"));
                choice(
                    ui,
                    "managed-theme",
                    &mut application.theme,
                    &[
                        ("system", "Как в системе"),
                        ("light", "Светлая"),
                        ("dark", "Тёмная"),
                    ],
                );
                ui.label(crate::i18n::tr("Язык интерфейса"));
                choice(
                    ui,
                    "managed-language",
                    &mut application.language,
                    &crate::i18n::LANGUAGES,
                );
                ui.checkbox(
                    &mut application.autostart,
                    crate::i18n::tr("Запускать Desk Switch в трее при входе в систему"),
                );
                ui.checkbox(
                    &mut application.auto_connect,
                    crate::i18n::tr("Подключать настроенную пару при запуске приложения"),
                );
                ui.checkbox(
                    &mut application.auto_update,
                    crate::i18n::tr("Проверять и скачивать обновления автоматически"),
                );
                if windows {
                    ui.checkbox(&mut application.show_remote_cursor, crate::i18n::tr("Show the remote cursor on Windows"));
                    note(ui, "Adds a pointer only when Windows hides its native cursor while receiving the other PC's mouse. Turn it off to use only the native Windows cursor.");
                }
            });
            if self.settings_remote && self.management_pending.is_some() {
                note(ui, "Waiting for the other PC to save the change…");
            } else {
                note(
                    ui,
                    "Changes are saved on the selected PC. Shared device settings apply to the pair.",
                );
            }
            ui.separator();
            if ui.button(crate::i18n::tr("VB-CABLE installers…")).clicked() {
                self.cable_open = true;
                self.cable_remote = self.settings_remote;
            }
            if application != before {
                self.request_application(application, ui.ctx());
            }
        });
    }

    fn update_settings(&mut self, ui: &mut egui::Ui) {
        let local = self.local_management().update;
        let remote = self
            .pair
            .remote
            .as_ref()
            .or(self.pair.last_remote.as_ref())
            .cloned();
        let connected = self.pair.remote.is_some();
        let busy =
            self.paired_update.is_some() || self.install_job.is_some() || self.install_pending;
        let peer_busy = busy || self.peer_restart.is_some() || self.management_pending.is_some();
        let peer_name = self.settings_peer_name();
        let mut check = false;
        let mut install_local = false;
        let mut install_remote = false;
        card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                subheading(ui, "Обновления");
                check = ui
                    .add_enabled(
                        !busy && self.management_pending.is_none(),
                        egui::Button::new(crate::i18n::tr("Проверить обновления")),
                    )
                    .clicked();
            });
            note(
                ui,
                "Check both linked PCs here. Each PC downloads and verifies its own installer.",
            );
            ui.separator();
            install_local = update_row(
                ui,
                &crate::i18n::tr("This PC"),
                Some(&crate::compatibility::Version::current()),
                &local,
                !busy,
            );
            if let Some(remote) = &remote {
                ui.separator();
                let mut update = remote
                    .management
                    .as_ref()
                    .map(|state| state.update.clone())
                    .unwrap_or_default();
                if let Some((status, failed)) = &self.peer_update_status {
                    update.status = status.clone();
                    update.failed = *failed;
                    update.installing |= self.peer_restart.is_some();
                } else if !connected {
                    update.status =
                        "The other PC is disconnected. Reconnect it to manage updates.".into();
                }
                install_remote = update_row(
                    ui,
                    &peer_name,
                    remote.version.as_ref(),
                    &update,
                    connected && remote.management.is_some() && !peer_busy,
                );
                if remote.management.is_none() {
                    note(
                        ui,
                        "Install a version with remote management on both PCs once to manage future updates from either computer.",
                    );
                }
            } else if !self.peer_key.is_empty() {
                ui.separator();
                subheading(ui, &peer_name);
                if let Some(found) = self
                    .discovered
                    .values()
                    .find(|peer| peer.public_key.as_deref() == Some(&self.peer_key))
                {
                    note(ui, &format!("Discovered version: {}", found.version));
                    if !found.manageable() {
                        note(
                            ui,
                            "Install a version with remote management on both PCs once to manage future updates from either computer.",
                        );
                        ui.hyperlink_to(
                            crate::i18n::tr("Download the current version for the other PC"),
                            "https://github.com/siarheisinelnikau/desk-switch/releases",
                        );
                    } else {
                        note(ui, "Reconnect the other PC to check and update it here.");
                    }
                } else {
                    note(ui, "Reconnect the other PC to check and update it here.");
                }
            }
            if let Some(error) = &self.last_version_error {
                ui.colored_label(palette(ui).error, crate::i18n::tr(error));
                note(
                    ui,
                    "Settings and updates remain available while input and audio wait for matching versions.",
                );
            }
            note(
                ui,
                "The installed Ubuntu edition updates without a password prompt. Updating only the other PC keeps this one running.",
            );
        });
        if check {
            self.check_updates(ui.ctx());
            if connected
                && remote
                    .as_ref()
                    .is_some_and(|node| node.management.is_some())
            {
                self.check_remote_updates(ui.ctx());
            }
        }
        if install_local {
            self.install_update(ui.ctx());
        }
        if install_remote {
            self.start_remote_update(ui.ctx());
        }
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
                        if ui.button(crate::i18n::tr("VB-CABLE installers…")).clicked() { self.cable_open = true; self.cable_remote = node.key != local.key; }
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

impl Desktop {
    pub(super) fn cable_dialog(&mut self, ctx: &egui::Context) {
        if !self.cable_open {
            return;
        }
        let mut open = self.cable_open;
        egui::Window::new(crate::i18n::tr("VB-CABLE installers"))
            .open(&mut open).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).collapsible(false).resizable(false).default_width(500.0)
            .show(ctx, |ui| {
                note(ui, "VB-CABLE by VB-Audio Software is donationware. Get the latest installer from the official website, or extract the saved original archive without an internet connection.");
                ui.horizontal_wrapped(|ui| {
                    ui.hyperlink_to(crate::i18n::tr("Download from VB-Audio"), crate::cable::WEBSITE);
                    ui.hyperlink_to(crate::i18n::tr("Support VB-Audio"), crate::cable::DONATE);
                });
                ui.separator();
                let busy = self.cable_job.is_some() || self.management_pending.as_ref().is_some_and(|p| p.cable);
                ui.add_enabled_ui(!busy, |ui| {
                    ui.horizontal(|ui| {
                        for platform in [crate::cable::Platform::Windows, crate::cable::Platform::MacOS] {
                            ui.selectable_value(&mut self.cable_platform, platform, platform.label());
                        }
                    });
                    note(ui, self.cable_platform.archive());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(crate::i18n::tr("Computer:"));
                        ui.selectable_value(&mut self.cable_remote, false, crate::i18n::tr("This PC"));
                        ui.add_enabled_ui(self.pair.remote.as_ref().is_some_and(|node| node.management.is_some()), |ui| {
                            let name = self.settings_peer_name();
                            ui.selectable_value(&mut self.cable_remote, true, name);
                        });
                    });
                    let platform = if self.cable_remote { self.pair.remote.as_ref().map(|node| node.diagnostics.platform.as_str()).unwrap_or("") } else { std::env::consts::OS };
                    let can_run = self.cable_platform.can_run_on(platform);
                    let online = !self.cable_remote || self.group.workspace.as_ref().is_none_or(|state| state.members.get(&self.peer_key).is_some_and(|member| member.online));
                    ui.horizontal_wrapped(|ui| {
                        if ui.add_enabled(can_run && online, egui::Button::new(crate::i18n::tr("Run official installer"))).clicked() { self.install_cable(ctx); }
                        if ui.button(crate::i18n::tr("Extract saved installer")).clicked() { self.extract_cable(ctx); }
                    });
                    if !can_run { note(ui, "Choose the matching Windows or macOS archive to run an installer on the selected computer. Ubuntu provides its virtual microphone without VB-CABLE."); }
                    note(ui, "Opens the official standalone installer on the selected computer. Approve the operating system prompt and complete its steps. Opening the installer does not confirm installation.");
                });
                if busy { note(ui, "Preparing the installer on the selected computer…"); }
                if self.cable_remote_done { note(ui, "The request completed on the other computer. For installation, finish the official installer there; extraction alone does not install the driver."); }
                if let Some(result) = &self.cable_result {
                    match result {
                        Ok(path) => {
                            ui.label(path.display().to_string());
                            if ui.button(crate::i18n::tr("Open folder")).clicked()
                                && let Err(error) = crate::cable::open_folder(path) { self.notice = Some((error.to_string(), true)); }
                        },
                        Err(error) => { ui.colored_label(palette(ui).error, crate::i18n::tr(error)); }
                    }
                }
                note(ui, "Original files are saved in Downloads / Desk Switch and remain available for manual installation. Follow the included readme; a computer restart may be required.");
            });
        self.cable_open = open;
    }
}

#[cfg(test)]
mod workspace_visual_tests {
    use super::*;
    use crate::{group::model::tests::fixture, workspace::Selection};

    fn key(index: usize) -> String {
        format!("{index:064x}")
    }

    #[test]
    fn every_audio_route_shows_the_selected_headphones_on_its_computer_card() {
        let mut workspace = fixture(4);
        for output in [key(2), key(3)] {
            workspace.settings.audio = Selection::Computer(output.clone());
            let visual = WorkspaceVisual::from_workspace(&workspace);
            let selected = visual.devices[DeviceIcon::Headphones.index()]
                .as_ref()
                .unwrap();
            assert_eq!(selected.key, output);
            for member in workspace.members.keys() {
                let routes = workspace.routes(member);
                if member != &output {
                    assert_eq!(routes.audio_output.unwrap().key, output);
                }
                let devices = visual.member_devices(&workspace, member);
                assert!(devices[DeviceIcon::Headphones.index()].as_ref() == Some(selected));
            }
        }
    }

    #[test]
    fn relative_mute_dims_only_that_sender_and_system_mute_dims_every_route() {
        let mut workspace = fixture(4);
        let sender = key(1);
        let output = key(2);
        let headphones = DeviceIcon::Headphones.index();
        for relative in [
            crate::workspace::AudioLevel {
                volume: 50,
                muted: true,
            },
            crate::workspace::AudioLevel {
                volume: 0,
                muted: false,
            },
        ] {
            workspace
                .members
                .get_mut(&sender)
                .unwrap()
                .preferences
                .audio = relative;
            let visual = WorkspaceVisual::from_workspace(&workspace);
            for member in workspace.members.keys() {
                let devices = visual.member_devices(&workspace, member);
                let source = devices[headphones].as_ref().unwrap();
                assert_eq!(source.key, output);
                assert_eq!(source.muted, member == &sender);
            }
        }
        for system in [
            crate::workspace::AudioLevel {
                volume: 90,
                muted: true,
            },
            crate::workspace::AudioLevel {
                volume: 0,
                muted: false,
            },
        ] {
            workspace.members.get_mut(&output).unwrap().native_output = Some(system);
            let visual = WorkspaceVisual::from_workspace(&workspace);
            assert!(visual.devices[headphones].as_ref().unwrap().muted);
            for member in workspace.members.keys() {
                assert!(
                    visual.member_devices(&workspace, member)[headphones]
                        .as_ref()
                        .unwrap()
                        .muted
                );
            }
        }
    }

    #[test]
    fn disabled_or_unavailable_audio_routes_stay_unshared() {
        let mut workspace = fixture(4);
        let headphones = DeviceIcon::Headphones.index();
        workspace.settings.audio = Selection::Computer(key(2));
        workspace.members.get_mut(&key(3)).unwrap().online = false;
        workspace.members.get_mut(&key(4)).unwrap().node.version = None;
        let visual = WorkspaceVisual::from_workspace(&workspace);
        for unavailable in [key(3), key(4)] {
            assert!(visual.member_devices(&workspace, &unavailable)[headphones].is_none());
        }
        workspace.settings.audio_enabled = false;
        let visual = WorkspaceVisual::from_workspace(&workspace);
        assert!(visual.devices[headphones].is_none());
        for member in workspace.members.keys() {
            assert!(visual.member_devices(&workspace, member)[headphones].is_none());
        }
        workspace.settings.audio_enabled = true;
        workspace.members.get_mut(&key(2)).unwrap().online = false;
        let visual = WorkspaceVisual::from_workspace(&workspace);
        for member in workspace.members.keys() {
            assert!(visual.member_devices(&workspace, member)[headphones].is_none());
        }
    }
}
