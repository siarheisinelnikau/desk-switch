//! GTK's native XEmbed tray, also used by DownloadManager. GNOME's standard
//! AppIndicator host forwards these clicks without an application companion.
use super::{Action, State, dispatch, entries, icon_rgba};
#[path = "tray_legacy_shape.rs"]
mod shape;
use eframe::egui;
use gtk::{
    gdk, gdk_pixbuf,
    glib::{self, ControlFlow, MainContext, MainLoop, Priority, translate::*},
    prelude::*,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

enum Update {
    State(State, String),
    Stop,
}

pub(super) struct Tray {
    updates: glib::Sender<Update>,
    embedded: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Tray {
    pub fn try_new(sender: mpsc::Sender<Action>, ctx: egui::Context) -> Option<Self> {
        #[cfg(debug_assertions)]
        if std::env::var("DESK_SWITCH_PREVIEW_TRAY_BACKEND").as_deref() == Ok("sni") {
            return None;
        }
        std::env::var_os("DISPLAY")?;
        let (ready, receiver) = mpsc::sync_channel(1);
        #[allow(deprecated)]
        let (updates, messages) = MainContext::channel(Priority::DEFAULT);
        let embedded = Arc::new(AtomicBool::new(false));
        let online = embedded.clone();
        let language = crate::i18n::current_code().to_owned();
        let worker = thread::Builder::new()
            .name("Desk Switch native tray".into())
            .spawn(move || {
                gdk::set_allowed_backends("x11");
                glib::set_prgname(Some("desk-switch"));
                if gtk::init().is_err() {
                    let _ = ready.send(false);
                    return;
                }
                let pixels = gdk_pixbuf::Pixbuf::from_bytes(
                    &glib::Bytes::from_owned(icon_rgba(32)),
                    gdk_pixbuf::Colorspace::Rgb,
                    true,
                    8,
                    32,
                    32,
                    128,
                );
                // SAFETY: GTK is initialized on this thread; it returns one owned
                // GObject reference, retained and dropped on this same GTK thread.
                let icon: glib::Object = unsafe {
                    from_glib_full(
                        gtk::ffi::gtk_status_icon_new_from_pixbuf(pixels.to_glib_none().0).cast(),
                    )
                };
                icon.set_property("title", "Desk Switch");
                if !wait_for_embedding(&icon) {
                    icon.set_property("visible", false);
                    let _ = ready.send(false);
                    return;
                }
                online.store(true, Ordering::Relaxed);
                let _shape = shape::follow(&icon);
                let changed = online.clone();
                let changed_ctx = ctx.clone();
                icon.connect_notify_local(Some("embedded"), move |icon, _| {
                    changed.store(icon.property("embedded"), Ordering::Relaxed);
                    changed_ctx.request_repaint_of(egui::ViewportId::ROOT);
                });
                let menu = Rc::new(RefCell::new(menu(
                    State::default(),
                    &language,
                    &sender,
                    &ctx,
                )));
                let activate_sender = sender.clone();
                let activate_ctx = ctx.clone();
                icon.connect_local("activate", false, move |_| {
                    let (x, y) = gtk::current_event()
                        .and_then(|event| {
                            let scale = f64::from(event.window()?.scale_factor());
                            let (x, y) = event.root_coords()?;
                            Some(((x * scale) as i32, (y * scale) as i32))
                        })
                        .unwrap_or((0, 0));
                    dispatch(&activate_sender, &activate_ctx, Action::QuickOpen { x, y });
                    None
                });
                let popup_menu = menu.clone();
                icon.connect_local("popup-menu", false, move |args| {
                    let menu = popup_menu.borrow();
                    if let Some((root, rect)) = args[0]
                        .get::<glib::Object>()
                        .ok()
                        .and_then(|icon| geometry(&icon))
                    {
                        menu.popup_at_rect(
                            &root,
                            &rect,
                            gdk::Gravity::SouthEast,
                            gdk::Gravity::NorthEast,
                            gtk::current_event().as_ref(),
                        );
                    } else {
                        menu.popup_at_pointer(gtk::current_event().as_ref());
                    }
                    None
                });
                let updating_icon = icon.clone();
                let source = messages.attach(None, move |message| match message {
                    Update::State(state, language) => {
                        let previous = menu.replace(self::menu(state, &language, &sender, &ctx));
                        previous.popdown();
                        ControlFlow::Continue
                    }
                    Update::Stop => {
                        menu.borrow().popdown();
                        updating_icon.set_property("visible", false);
                        gtk::main_quit();
                        ControlFlow::Continue
                    }
                });
                if ready.send(true).is_ok() {
                    gtk::main();
                }
                source.remove();
                icon.set_property("visible", false);
                online.store(false, Ordering::Relaxed);
            })
            .ok()?;
        if receiver.recv_timeout(Duration::from_secs(3)) != Ok(true) {
            // If bootstrap outlives the deadline, dropping the readiness receiver
            // makes it clean up instead of entering an orphan GTK event loop.
            return None;
        }
        Some(Self {
            updates,
            embedded,
            worker: Some(worker),
        })
    }

    pub fn available(&self) -> bool {
        self.embedded.load(Ordering::Relaxed)
    }
    pub fn update(&self, state: State) {
        let _ = self
            .updates
            .send(Update::State(state, crate::i18n::current_code().into()));
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let _ = self.updates.send(Update::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn menu(
    state: State,
    language: &str,
    sender: &mpsc::Sender<Action>,
    ctx: &egui::Context,
) -> gtk::Menu {
    let menu = gtk::Menu::new();
    menu.set_anchor_hints(
        gdk::AnchorHints::FLIP_Y | gdk::AnchorHints::SLIDE_X | gdk::AnchorHints::SLIDE_Y,
    );
    for entry in entries(state) {
        if let Some((label, action, enabled)) = entry {
            // Native clicks already work; keep extension setup out of this menu.
            if action == Action::TraySetup {
                continue;
            }
            let item = gtk::MenuItem::with_label(&crate::i18n::tr_language(language, label));
            item.set_sensitive(enabled);
            let sender = sender.clone();
            let ctx = ctx.clone();
            item.connect_activate(move |_| dispatch(&sender, &ctx, action));
            menu.append(&item);
        } else {
            menu.append(&gtk::SeparatorMenuItem::new());
        }
    }
    menu.show_all();
    menu
}

fn wait_for_embedding(icon: &glib::Object) -> bool {
    if icon.property::<bool>("embedded") {
        return true;
    }
    let startup = MainLoop::new(None, false);
    let on_embed = startup.clone();
    let signal = icon.connect_notify_local(Some("embedded"), move |icon, _| {
        if icon.property::<bool>("embedded") {
            on_embed.quit();
        }
    });
    let on_timeout = startup.clone();
    let timeout = glib::timeout_add_local(Duration::from_millis(500), move || {
        on_timeout.quit();
        ControlFlow::Continue
    });
    startup.run();
    timeout.remove();
    icon.disconnect(signal);
    icon.property("embedded")
}

fn geometry(icon: &glib::Object) -> Option<(gdk::Window, gdk::Rectangle)> {
    let mut screen = std::ptr::null_mut();
    let mut rect = gdk::Rectangle::new(0, 0, 0, 0);
    // SAFETY: `icon` is our live GtkStatusIcon. The out-pointers remain valid
    // during the synchronous call; GTK retains ownership of the returned screen.
    if unsafe {
        gtk::ffi::gtk_status_icon_get_geometry(
            icon.as_ptr().cast(),
            &mut screen,
            rect.to_glib_none_mut().0,
            std::ptr::null_mut(),
        )
    } == 0
        || screen.is_null()
    {
        return None;
    }
    // SAFETY: the non-null borrowed screen was returned by GTK above. The wrapper
    // takes its own reference and releases it on the same GTK thread.
    let screen: gdk::Screen = unsafe { from_glib_none(screen) };
    Some((screen.root_window()?, rect))
}
