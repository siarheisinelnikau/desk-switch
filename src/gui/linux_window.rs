//! Tray windows need real hide/restore and popup placement. Winit's Wayland
//! set_visible is a no-op; use X11/XWayland when the desktop provides it.
//! Input capture still uses the native Wayland portal/EIS backend.
use winit::platform::x11::EventLoopBuilderExtX11;

pub(super) fn options(mut options: eframe::NativeOptions) -> eframe::NativeOptions {
    if std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty()) {
        let previous = options.event_loop_builder.take();
        options.event_loop_builder = Some(Box::new(move |builder| {
            if let Some(previous) = previous {
                previous(builder);
            }
            builder.with_x11();
        }));
    }
    options
}
