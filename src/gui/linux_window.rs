//! Tray windows need real hide/restore and popup placement. Winit's Wayland
//! set_visible is a no-op; use X11/XWayland when the desktop provides it.
//! Input capture still uses the native Wayland portal/EIS backend.
use winit::platform::x11::EventLoopBuilderExtX11;

pub(super) fn configure<T>(
    builder: &mut winit::event_loop::EventLoopBuilder<T>,
) -> Result<(), String> {
    if !std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty()) {
        return Err(
            "The tray interface requires XWayland. Install xwayland and sign in again.".into(),
        );
    }
    builder.with_x11();
    Ok(())
}
