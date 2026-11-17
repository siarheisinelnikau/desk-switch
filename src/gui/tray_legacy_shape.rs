//! Clip only GNOME's dedicated socket for this application's legacy icon.
//! This follows DownloadManager's XEmbed transparency fix using safe X11 calls.
use gtk::glib::{self, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use x11rb::{
    connection::Connection,
    protocol::{
        shape::{ConnectionExt as _, SK, SO},
        xproto::{ClipOrdering, ConnectionExt as _, Rectangle},
    },
};

pub(super) struct Watch {
    icon: glib::WeakRef<glib::Object>,
    signals: Vec<glib::SignalHandlerId>,
    pending: Rc<RefCell<Option<glib::SourceId>>>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(icon) = self.icon.upgrade() {
            for signal in self.signals.drain(..) {
                icon.disconnect(signal);
            }
        }
        if let Some(source) = self.pending.borrow_mut().take() {
            source.remove();
        }
    }
}

pub(super) fn follow(icon: &glib::Object) -> Option<Watch> {
    if !std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .contains("gnome")
    {
        return None;
    }
    let pending = Rc::new(RefCell::new(None));
    let applied = Rc::new(Cell::new(None));
    let mut signals = Vec::new();
    let changed_pending = pending.clone();
    let changed_applied = applied.clone();
    signals.push(icon.connect_notify_local(Some("embedded"), move |icon, _| {
        queue(icon, &changed_pending, &changed_applied)
    }));
    let changed_pending = pending.clone();
    let changed_applied = applied.clone();
    signals.push(icon.connect_local("size-changed", false, move |args| {
        if let Ok(icon) = args[0].get::<glib::Object>() {
            queue(&icon, &changed_pending, &changed_applied);
        }
        Some(false.to_value())
    }));
    queue(icon, &pending, &applied);
    Some(Watch {
        icon: icon.downgrade(),
        signals,
        pending,
    })
}

fn queue(
    icon: &glib::Object,
    pending: &Rc<RefCell<Option<glib::SourceId>>>,
    applied: &Rc<Cell<Option<(u32, u16)>>>,
) {
    if pending.borrow().is_some() {
        return;
    }
    let icon = icon.downgrade();
    let completion = pending.clone();
    let applied = applied.clone();
    let source = glib::idle_add_local_once(move || {
        completion.borrow_mut().take();
        if let Some(icon) = icon.upgrade() {
            let _ = clip(&icon, &applied);
        }
    });
    *pending.borrow_mut() = Some(source);
}

fn clip(icon: &glib::Object, applied: &Cell<Option<(u32, u16)>>) -> anyhow::Result<()> {
    if !icon.property::<bool>("embedded") {
        applied.set(None);
        return Ok(());
    }
    // SAFETY: the callback holds our live GtkStatusIcon on its owning GTK/X11
    // thread. The result is an XID, not a borrowed native object pointer.
    let plug: u32 = unsafe { gtk::ffi::gtk_status_icon_get_x11_window_id(icon.as_ptr().cast()) };
    let (x11, _) = x11rb::connect(None)?;
    let tree = x11.query_tree(plug)?.reply()?;
    if tree.parent == tree.root {
        return Ok(());
    }
    let socket = tree.parent;
    let outer_tree = x11.query_tree(socket)?.reply()?;
    let outer = x11.get_geometry(socket)?.reply()?;
    let inner = x11.get_geometry(plug)?.reply()?;
    let attributes = x11.get_window_attributes(socket)?.reply()?;
    let side = outer.width;
    if outer_tree.parent != tree.root
        || outer_tree.children != [plug]
        || !attributes.override_redirect
        || outer.depth != 24
        || outer.border_width != 0
        || !(8..=128).contains(&side)
        || outer.height != side
        || inner.width != side
        || inner.height != side
        || inner.x != 0
        || inner.y != 0
        || applied.get() == Some((socket, side))
    {
        return Ok(());
    }
    // An empty input shape identifies the host's dedicated clone socket. Never
    // change the shape of a normal application, panel or shared tray window.
    if !x11
        .shape_get_rectangles(socket, SK::INPUT)?
        .reply()?
        .rectangles
        .is_empty()
    {
        return Ok(());
    }
    let image = image::RgbaImage::from_raw(32, 32, super::icon_rgba(32)).unwrap();
    let image = image::imageops::resize(
        &image,
        side.into(),
        side.into(),
        image::imageops::FilterType::Triangle,
    );
    let mut rows = Vec::new();
    for y in 0..side {
        let mut x = 0;
        while x < side {
            if image.get_pixel(x.into(), y.into())[3] < 128 {
                x += 1;
                continue;
            }
            let start = x;
            while x < side && image.get_pixel(x.into(), y.into())[3] >= 128 {
                x += 1;
            }
            rows.push(Rectangle {
                x: start as i16,
                y: y as i16,
                width: x - start,
                height: 1,
            });
        }
    }
    x11.shape_rectangles(
        SO::SET,
        SK::BOUNDING,
        ClipOrdering::Y_SORTED,
        socket,
        0,
        0,
        &rows,
    )?
    .check()?;
    x11.flush()?;
    applied.set(Some((socket, side)));
    Ok(())
}
