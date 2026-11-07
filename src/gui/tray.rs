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
    use std::sync::LazyLock;
    static ICONS: LazyLock<std::collections::BTreeMap<usize, Vec<u8>>> = LazyLock::new(|| {
        [
            (
                16,
                include_bytes!("../../assets/icons/desk-switch-16.png").as_slice(),
            ),
            (
                20,
                include_bytes!("../../assets/icons/desk-switch-20.png").as_slice(),
            ),
            (
                24,
                include_bytes!("../../assets/icons/desk-switch-24.png").as_slice(),
            ),
            (
                32,
                include_bytes!("../../assets/icons/desk-switch-32.png").as_slice(),
            ),
            (
                48,
                include_bytes!("../../assets/icons/desk-switch-48.png").as_slice(),
            ),
            (
                64,
                include_bytes!("../../assets/icons/desk-switch-64.png").as_slice(),
            ),
        ]
        .into_iter()
        .map(|(size, png)| {
            (
                size,
                eframe::icon_data::from_png_bytes(png)
                    .expect("valid application icon")
                    .rgba,
            )
        })
        .collect()
    });
    ICONS
        .get(&size)
        .expect("supported native icon size")
        .clone()
}

pub(super) fn dispatch(sender: &Sender<Action>, ctx: &egui::Context, action: Action) {
    if action == Action::Show {
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Minimized(false),
        );
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
    }
    let _ = sender.send(action);
    // Context clones share the active viewport. A child popup must explicitly
    // wake ROOT, whose logic drains commands even while its window is hidden.
    ctx.request_repaint_of(egui::ViewportId::ROOT);
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
        const MENU_ON_ACTIVATE: bool = false;
        fn id(&self) -> String {
            "desk-switch".into()
        }
        fn title(&self) -> String {
            "Desk Switch".into()
        }
        fn activate(&mut self, x: i32, y: i32) {
            dispatch(&self.sender, &self.ctx, Action::QuickOpen { x, y });
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
                item("Quick controls", Action::QuickOpen { x: 0, y: 0 }, true),
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

#[cfg(any(windows, target_os = "macos"))]
mod native {
    use super::*;
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
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
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    position,
                    ..
                } = event
                {
                    dispatch(
                        &sender,
                        &ctx,
                        Action::QuickOpen {
                            x: position.x as i32,
                            y: position.y as i32,
                        },
                    );
                }
            }));
            #[cfg(windows)]
            let metric = unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                    windows_sys::Win32::UI::WindowsAndMessaging::SM_CXSMICON,
                )
            }
            .max(16) as usize;
            #[cfg(target_os = "macos")]
            let metric = 32usize;
            let size = [16usize, 20, 24, 32, 48, 64]
                .into_iter()
                .min_by_key(|size| size.abs_diff(metric))
                .unwrap();
            let icon = Icon::from_rgba(icon_rgba(size), size as u32, size as u32)?;
            let icon = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
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
        #[cfg(debug_assertions)]
        pub(crate) fn preview_label(&self) {
            let _ = self._icon.set_tooltip(Some("Desk Switch preview"));
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
