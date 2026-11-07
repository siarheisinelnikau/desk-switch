use super::Action;
use anyhow::Result;
use eframe::egui;
use std::sync::mpsc::Sender;

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) struct State {
    pub connected: bool,
    pub emergency: bool,
    pub sound: bool,
    pub monitor: bool,
    pub combined: bool,
}

impl State {
    fn switch_label(self) -> &'static str {
        if self.combined {
            "Switch monitor, keyboard and mouse"
        } else {
            "Переключить на другой ПК"
        }
    }
}

pub fn icon_rgba(size: usize) -> Vec<u8> {
    let mut rgba = vec![0; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 / size as f32;
            let fy = y as f32 / size as f32;
            let background = (0.08..0.92).contains(&fx) && (0.08..0.92).contains(&fy);
            let first = (0.19..0.57).contains(&fx) && (0.24..0.58).contains(&fy);
            let second = (0.45..0.82).contains(&fx) && (0.43..0.76).contains(&fy);
            let inner_first = (0.23..0.53).contains(&fx) && (0.28..0.53).contains(&fy);
            let inner_second = (0.49..0.78).contains(&fx) && (0.47..0.71).contains(&fy);
            let color = if (first && !inner_first) || (second && !inner_second) {
                [255, 255, 255, 255]
            } else if background {
                [82, 99, 224, 255]
            } else {
                [0, 0, 0, 0]
            };
            rgba[(y * size + x) * 4..][..4].copy_from_slice(&color);
        }
    }
    rgba
}

fn dispatch(sender: &Sender<Action>, ctx: &egui::Context, action: Action) {
    if action == Action::Show {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
    let _ = sender.send(action);
    ctx.request_repaint();
}

#[cfg(target_os = "linux")]
mod native {
    use super::*;
    use ksni::blocking::TrayMethods;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    pub struct Item {
        language: String,
        sender: Sender<Action>,
        ctx: egui::Context,
        state: State,
        available: Arc<AtomicBool>,
    }

    impl ksni::Tray for Item {
        fn id(&self) -> String {
            "desk-switch".into()
        }
        fn title(&self) -> String {
            "Desk Switch".into()
        }
        fn activate(&mut self, _: i32, _: i32) {
            dispatch(&self.sender, &self.ctx, Action::Show);
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            let mut pixels = icon_rgba(32);
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel.rotate_right(1);
            }
            vec![ksni::Icon {
                width: 32,
                height: 32,
                data: pixels,
            }]
        }
        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: "Desk Switch".into(),
                description: crate::i18n::tr_language(
                    &self.language,
                    if self.state.connected {
                        "Пара подключена"
                    } else {
                        "Настройка рабочего места"
                    },
                ),
                ..Default::default()
            }
        }
        fn watcher_online(&self) {
            self.available.store(true, Ordering::Relaxed);
        }
        fn watcher_offline(&self, _: ksni::OfflineReason) -> bool {
            self.available.store(false, Ordering::Relaxed);
            self.ctx.request_repaint();
            true
        }
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::{MenuItem, StandardItem};
            let item = |label: &str, action, enabled| {
                StandardItem {
                    label: crate::i18n::tr_language(&self.language, label),
                    enabled,
                    activate: Box::new(move |this: &mut Self| {
                        dispatch(&this.sender, &this.ctx, action)
                    }),
                    ..Default::default()
                }
                .into()
            };
            vec![
                item("Открыть Desk Switch", Action::Show, true),
                MenuItem::Separator,
                item(
                    self.state.switch_label(),
                    Action::Switch,
                    self.state.connected,
                ),
                item(
                    "Switch shared monitor",
                    Action::MonitorSwitch,
                    self.state.monitor,
                ),
                item("Set up shared monitor…", Action::MonitorSetup, true),
                item("Аварийный возврат", Action::Emergency, self.state.emergency),
                MenuItem::Separator,
                item(
                    if self.state.sound {
                        "Выключить аудиоподключение"
                    } else {
                        "Включить аудиоподключение"
                    },
                    Action::Sound,
                    true,
                ),
                MenuItem::Separator,
                item("Завершить Desk Switch", Action::Quit, true),
            ]
        }
    }

    pub struct Tray {
        handle: ksni::blocking::Handle<Item>,
        available: Arc<AtomicBool>,
    }

    impl Tray {
        pub fn new(sender: Sender<Action>, ctx: egui::Context) -> Result<Self> {
            let available = Arc::new(AtomicBool::new(true));
            let handle = Item {
                language: crate::i18n::current_code().into(),
                sender,
                ctx,
                state: State::default(),
                available: Arc::clone(&available),
            }
            .assume_sni_available(true)
            .spawn()?;
            Ok(Self { handle, available })
        }
        pub fn available(&self) -> bool {
            self.available.load(Ordering::Relaxed) && !self.handle.is_closed()
        }
        pub fn update(&mut self, state: State) {
            self.handle.update(|item| {
                item.language = crate::i18n::current_code().into();
                item.state = state;
            });
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            self.handle.shutdown();
        }
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use tray_icon::{
        Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    };

    pub struct Tray {
        _icon: TrayIcon,
        show: MenuItem,
        quit: MenuItem,
        switch: MenuItem,
        monitor: MenuItem,
        monitor_setup: MenuItem,
        emergency: MenuItem,
        sound: MenuItem,
    }

    impl Tray {
        pub fn new(sender: Sender<Action>, ctx: egui::Context) -> Result<Self> {
            let menu = Menu::new();
            let show =
                MenuItem::with_id("show", crate::i18n::tr("Открыть Desk Switch"), true, None);
            let switch = MenuItem::with_id(
                "switch",
                crate::i18n::tr("Переключить на другой ПК"),
                false,
                None,
            );
            let emergency = MenuItem::with_id(
                "emergency",
                crate::i18n::tr("Аварийный возврат"),
                false,
                None,
            );
            let monitor = MenuItem::with_id(
                "monitor",
                crate::i18n::tr("Switch shared monitor"),
                false,
                None,
            );
            let monitor_setup = MenuItem::with_id(
                "monitor-setup",
                crate::i18n::tr("Set up shared monitor…"),
                true,
                None,
            );
            let sound = MenuItem::with_id(
                "sound",
                crate::i18n::tr("Включить аудиоподключение"),
                true,
                None,
            );
            let quit =
                MenuItem::with_id("quit", crate::i18n::tr("Завершить Desk Switch"), true, None);
            menu.append_items(&[
                &show,
                &PredefinedMenuItem::separator(),
                &switch,
                &monitor,
                &monitor_setup,
                &emergency,
                &PredefinedMenuItem::separator(),
                &sound,
                &PredefinedMenuItem::separator(),
                &quit,
            ])?;
            let menu_sender = sender.clone();
            let menu_ctx = ctx.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let action = match event.id.0.as_str() {
                    "show" => Action::Show,
                    "switch" => Action::Switch,
                    "monitor" => Action::MonitorSwitch,
                    "monitor-setup" => Action::MonitorSetup,
                    "emergency" => Action::Emergency,
                    "sound" => Action::Sound,
                    "quit" => Action::Quit,
                    _ => return,
                };
                dispatch(&menu_sender, &menu_ctx, action);
            }));
            TrayIconEvent::set_event_handler(Some(move |event| {
                if matches!(
                    event,
                    TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    }
                ) {
                    dispatch(&sender, &ctx, Action::Show);
                }
            }));
            let icon = Icon::from_rgba(icon_rgba(32), 32, 32)?;
            let icon = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(true)
                .with_tooltip("Desk Switch")
                .with_icon(icon)
                .build()?;
            Ok(Self {
                _icon: icon,
                show,
                quit,
                switch,
                monitor,
                monitor_setup,
                emergency,
                sound,
            })
        }
        pub fn available(&self) -> bool {
            true
        }
        pub fn update(&mut self, state: State) {
            self.show.set_text(crate::i18n::tr("Открыть Desk Switch"));
            self.quit.set_text(crate::i18n::tr("Завершить Desk Switch"));
            self.switch.set_text(crate::i18n::tr(state.switch_label()));
            self.monitor
                .set_text(crate::i18n::tr("Switch shared monitor"));
            self.monitor_setup
                .set_text(crate::i18n::tr("Set up shared monitor…"));
            self.monitor.set_enabled(state.monitor);
            self.emergency
                .set_text(crate::i18n::tr("Аварийный возврат"));
            self.switch.set_enabled(state.connected);
            self.emergency.set_enabled(state.emergency);
            self.sound.set_text(crate::i18n::tr(if state.sound {
                "Выключить аудиоподключение"
            } else {
                "Включить аудиоподключение"
            }));
        }
    }
}

pub use native::Tray;
