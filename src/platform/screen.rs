//! Bounded cursor-region capture on explicit request from a paired phone.
#[cfg(any(windows, target_os = "linux", test))]
use crate::mobile::screen::MAX_PNG;
use crate::mobile::screen::Options;
#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::Result;
#[cfg(any(windows, target_os = "linux", test))]
use anyhow::ensure;
#[cfg(any(windows, target_os = "linux", test))]
use image::{ImageEncoder, Rgb, RgbImage};

/// Only application-defined text is safe to send back to a remote viewer.
pub(crate) fn failure_status(error: &anyhow::Error) -> &'static str {
    #[cfg(target_os = "linux")]
    if let Some(error) = error.downcast_ref::<GnomeCaptureError>() {
        return error.0;
    }
    let _ = error;
    "Could not capture this desktop"
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct GnomeCaptureError(&'static str);

#[cfg(target_os = "linux")]
impl std::fmt::Display for GnomeCaptureError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str(self.0)
    }
}

#[cfg(target_os = "linux")]
impl std::error::Error for GnomeCaptureError {}

#[cfg(target_os = "linux")]
fn gnome_capture_error(error: zbus::Error) -> anyhow::Error {
    use zbus::DBusError;

    let fdo_name;
    let name = match &error {
        zbus::Error::MethodError(name, _, _) => Some(name.as_str()),
        zbus::Error::FDO(error) => {
            fdo_name = error.name();
            Some(fdo_name.as_str())
        }
        _ => None,
    };
    let status = match name {
        Some(
            "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner",
        ) => {
            "Enable Desk Switch Tray in GNOME Extensions on the computer. If it is missing after installation, sign out and sign back in."
        }
        Some(
            "org.freedesktop.DBus.Error.UnknownMethod"
            | "org.freedesktop.DBus.Error.UnknownInterface"
            | "org.freedesktop.DBus.Error.UnknownObject",
        ) => {
            "Install the current Desk Switch package on the computer, then sign out and sign back in to load the GNOME companion."
        }
        Some("io.github.siarheisinelnikau.DeskSwitch.Clipboard.CursorPreviewLocked") => {
            "Unlock the computer to resume the preview."
        }
        Some("io.github.siarheisinelnikau.DeskSwitch.Clipboard.CursorPreviewUnsupported") => {
            "This GNOME session does not support cursor preview."
        }
        Some(
            "org.freedesktop.DBus.Error.AccessDenied"
            | "io.github.siarheisinelnikau.DeskSwitch.Clipboard.CursorPreviewAccessDenied",
        ) => {
            "GNOME could not verify Desk Switch. Reopen the installed Desk Switch app on the computer."
        }
        _ => "GNOME could not capture the cursor area.",
    };
    anyhow::Error::new(error).context(GnomeCaptureError(status))
}

pub(crate) fn available() -> bool {
    #[cfg(windows)]
    {
        true
    }
    #[cfg(target_os = "linux")]
    {
        if wayland() {
            let desktop = std::env::var("XDG_CURRENT_DESKTOP")
                .unwrap_or_default()
                .to_ascii_lowercase();
            desktop.contains("gnome") || desktop.contains("ubuntu")
        } else {
            std::env::var_os("DISPLAY").is_some()
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}
#[derive(Default)]
pub(crate) struct Capture {
    #[cfg(target_os = "linux")]
    connection: Option<Linux>,
}
impl Capture {
    pub(crate) fn frame(&mut self, options: Options) -> Result<Vec<u8>> {
        options.validate()?;
        #[cfg(windows)]
        let image = windows_capture(options)?;
        #[cfg(target_os = "linux")]
        let image = self.linux_capture(options)?;
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = options;
            anyhow::bail!("Cursor preview is unavailable on this platform");
        }
        #[cfg(any(windows, target_os = "linux"))]
        encode(image)
    }
}

#[cfg(any(windows, target_os = "linux", test))]
fn encode(mut image: RgbImage) -> Result<Vec<u8>> {
    loop {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png).write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )?;
        if png.len() <= MAX_PNG {
            return Ok(png);
        }
        ensure!(
            image.width() > 128 || image.height() > 96,
            "Cursor preview exceeds its size limit"
        );
        let width = (image.width() * 3 / 4).max(128).min(image.width());
        let height = (image.height() * 3 / 4).max(96).min(image.height());
        image =
            image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg(any(windows, target_os = "linux", test))]
struct Region {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    cursor_x: u32,
    cursor_y: u32,
}
#[cfg(any(windows, target_os = "linux", test))]
fn region(point: [i32; 2], monitor: [i32; 4], options: Options) -> Result<Region> {
    let [left, top, right, bottom] = monitor.map(i64::from);
    ensure!(
        right > left && bottom > top,
        "The pointer display has no usable region"
    );
    let width = i64::from(options.width).min(right - left);
    let height = i64::from(options.height).min(bottom - top);
    let px = i64::from(point[0]).clamp(left, right - 1);
    let py = i64::from(point[1]).clamp(top, bottom - 1);
    let x = (px - width / 2).clamp(left, right - width);
    let y = (py - height / 2).clamp(top, bottom - height);
    Ok(Region {
        x: x.try_into()?,
        y: y.try_into()?,
        width: width.try_into()?,
        height: height.try_into()?,
        cursor_x: (px - x).try_into()?,
        cursor_y: (py - y).try_into()?,
    })
}
#[cfg(any(windows, target_os = "linux"))]
fn crosshair(image: &mut RgbImage, region: Region) {
    let (x, y) = (region.cursor_x as i32, region.cursor_y as i32);
    for distance in -7i32..=7 {
        for (px, py) in [(x + distance, y), (x, y + distance)] {
            for (ox, oy, color) in [
                (0, 0, [255, 255, 255]),
                (-1, -1, [10, 20, 30]),
                (1, 1, [10, 20, 30]),
            ] {
                let (px, py) = (px + ox, py + oy);
                if px >= 0 && py >= 0 && px < image.width() as i32 && py < image.height() as i32 {
                    image.put_pixel(px as u32, py as u32, Rgb(color));
                }
            }
        }
    }
}

#[cfg(windows)]
fn windows_capture(options: Options) -> Result<RgbImage> {
    use std::{mem::size_of, ptr::null_mut};
    use windows_sys::Win32::{
        Foundation::POINT,
        Graphics::Gdi::*,
        System::StationsAndDesktops::*,
        UI::{HiDpi::*, WindowsAndMessaging::GetPhysicalCursorPos},
    };
    struct Dpi(DPI_AWARENESS_CONTEXT);
    impl Drop for Dpi {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: restore only the current thread's previously returned DPI context.
                unsafe {
                    SetThreadDpiAwarenessContext(self.0);
                }
            }
        }
    }
    struct Gdi {
        screen: HDC,
        memory: HDC,
        bitmap: HBITMAP,
        previous: HGDIOBJ,
    }
    impl Drop for Gdi {
        fn drop(&mut self) {
            // SAFETY: each handle belongs to this capture; restore selection before deleting its bitmap/DC.
            unsafe {
                if !self.previous.is_null() && self.previous as isize != -1 {
                    SelectObject(self.memory, self.previous);
                }
                if !self.bitmap.is_null() {
                    DeleteObject(self.bitmap);
                }
                if !self.memory.is_null() {
                    DeleteDC(self.memory);
                }
                if !self.screen.is_null() {
                    ReleaseDC(null_mut(), self.screen);
                }
            }
        }
    }
    // SAFETY: OpenInputDesktop returns a process-owned handle, closed below after a bounded name read.
    let desktop = unsafe { OpenInputDesktop(0, 0, DESKTOP_READOBJECTS) };
    ensure!(
        !desktop.is_null(),
        "Cursor preview is unavailable while the desktop is locked"
    );
    let mut desktop_name = [0u16; 128];
    let mut needed = 0;
    // SAFETY: the UTF-16 destination is writable for the supplied byte count, and the handle is live.
    let read = unsafe {
        GetUserObjectInformationW(
            desktop,
            UOI_NAME,
            desktop_name.as_mut_ptr().cast(),
            size_of::<[u16; 128]>() as u32,
            &mut needed,
        )
    };
    // SAFETY: close the handle returned by OpenInputDesktop exactly once.
    unsafe {
        CloseDesktop(desktop);
    }
    let end = desktop_name
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(desktop_name.len());
    ensure!(
        read != 0 && String::from_utf16_lossy(&desktop_name[..end]).eq_ignore_ascii_case("default"),
        "Cursor preview is unavailable on a secure desktop"
    );
    // SAFETY: change only this capture worker's DPI context, restoring it with the guard.
    let _dpi =
        Dpi(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) });
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: point is a live writable POINT structure.
    let located = unsafe { GetPhysicalCursorPos(&mut point) };
    ensure!(located != 0, "The pointer position is unavailable");
    // SAFETY: the point is initialized and MonitorFromPoint returns a borrowed monitor handle.
    let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    // SAFETY: MONITORINFO contains only integer/RECT fields, valid when zero-initialized.
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    // SAFETY: monitor is borrowed from Windows and info has its required initialized size.
    let found = unsafe { GetMonitorInfoW(monitor, &mut info) };
    ensure!(found != 0, "The pointer display is unavailable");
    let rect = info.rcMonitor;
    let area = region(
        [point.x, point.y],
        [rect.left, rect.top, rect.right, rect.bottom],
        options,
    )?;
    // SAFETY: null HWND requests the current desktop's screen DC; ownership is tracked by Gdi.
    let screen = unsafe { GetDC(null_mut()) };
    let mut resources = Gdi {
        screen,
        memory: null_mut(),
        bitmap: null_mut(),
        previous: null_mut(),
    };
    ensure!(
        !resources.screen.is_null(),
        "The desktop capture context is unavailable"
    );
    // SAFETY: the live screen DC is only borrowed to initialize a compatible memory DC.
    resources.memory = unsafe { CreateCompatibleDC(resources.screen) };
    ensure!(
        !resources.memory.is_null(),
        "The preview capture context could not be allocated"
    );
    // SAFETY: BITMAPINFO consists of integer/color fields for which zero is valid.
    let mut bitmap: BITMAPINFO = unsafe { std::mem::zeroed() };
    bitmap.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bitmap.bmiHeader.biWidth = area.width as i32;
    bitmap.bmiHeader.biHeight = -(area.height as i32);
    bitmap.bmiHeader.biPlanes = 1;
    bitmap.bmiHeader.biBitCount = 32;
    bitmap.bmiHeader.biCompression = BI_RGB;
    let mut bits = null_mut();
    // SAFETY: bitmap dimensions are bounded at 640x480; bits receives the owned DIB's live buffer.
    resources.bitmap = unsafe {
        CreateDIBSection(
            resources.memory,
            &bitmap,
            DIB_RGB_COLORS,
            &mut bits,
            null_mut(),
            0,
        )
    };
    ensure!(
        !resources.bitmap.is_null() && !bits.is_null(),
        "The preview bitmap could not be allocated"
    );
    // SAFETY: the live DIB is selected only into its owned memory DC and restored by the guard.
    resources.previous = unsafe { SelectObject(resources.memory, resources.bitmap) };
    ensure!(
        !resources.previous.is_null() && resources.previous as isize != -1,
        "The preview bitmap could not be selected"
    );
    // SAFETY: source/destination DCs are live and coordinates stay inside the selected display/bitmap.
    let copied = unsafe {
        BitBlt(
            resources.memory,
            0,
            0,
            area.width as i32,
            area.height as i32,
            resources.screen,
            area.x,
            area.y,
            SRCCOPY | CAPTUREBLT,
        )
    };
    ensure!(copied != 0, "The desktop could not be captured");
    // SAFETY: complete queued GDI writes before reading the DIB's mapped bytes.
    let flushed = unsafe { GdiFlush() };
    ensure!(flushed != 0, "The desktop capture did not complete");
    let length = area.width as usize * area.height as usize * 4;
    // SAFETY: the bounded 32-bit DIB has at least width*height*4 bytes and stays selected/alive until after the copy.
    let pixels = unsafe { std::slice::from_raw_parts(bits.cast::<u8>(), length) };
    let mut image = RgbImage::new(area.width, area.height);
    for (out, pixel) in image.pixels_mut().zip(pixels.as_chunks::<4>().0) {
        *out = Rgb([pixel[2], pixel[1], pixel[0]]);
    }
    crosshair(&mut image, area);
    Ok(image)
}

#[cfg(target_os = "linux")]
fn wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}
#[cfg(target_os = "linux")]
enum Linux {
    Gnome(zbus::blocking::Connection),
    X11(Box<x11rb::rust_connection::RustConnection>, usize),
}
#[cfg(target_os = "linux")]
impl Capture {
    fn linux_capture(&mut self, options: Options) -> Result<RgbImage> {
        use x11rb::{
            connection::Connection,
            protocol::xproto::{ConnectionExt, ImageFormat, ImageOrder},
        };
        if self.connection.is_none() {
            self.connection = Some(if wayland() {
                Linux::Gnome(
                    zbus::blocking::connection::Builder::session()?
                        .method_timeout(std::time::Duration::from_secs(2))
                        .build()?,
                )
            } else {
                let (connection, screen) = x11rb::connect(None)?;
                Linux::X11(Box::new(connection), screen)
            });
        }
        let result = (|| match self.connection.as_ref().unwrap() {
            Linux::Gnome(connection) => {
                let proxy = zbus::blocking::Proxy::new(
                    connection,
                    "io.github.siarheisinelnikau.DeskSwitch.Clipboard",
                    "/io/github/siarheisinelnikau/DeskSwitch/Clipboard",
                    "io.github.siarheisinelnikau.DeskSwitch.Clipboard",
                )
                .map_err(gnome_capture_error)?;
                let bytes: Vec<u8> = proxy
                    .call(
                        "CaptureCursorRegion",
                        &(u32::from(options.width), u32::from(options.height)),
                    )
                    .map_err(gnome_capture_error)?;
                ensure!(
                    bytes.len() <= 2 * 1024 * 1024,
                    "The GNOME preview exceeds its limit"
                );
                let mut reader =
                    image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
                let mut limits = image::Limits::default();
                limits.max_image_width = Some(640);
                limits.max_image_height = Some(480);
                limits.max_alloc = Some(2 * 1024 * 1024);
                reader.limits(limits);
                Ok(reader.decode()?.to_rgb8())
            }
            Linux::X11(connection, index) => {
                let setup = connection.setup();
                let screen = &setup.roots[*index];
                let pointer = connection.query_pointer(screen.root)?.reply()?;
                let area = region(
                    [i32::from(pointer.root_x), i32::from(pointer.root_y)],
                    [
                        0,
                        0,
                        i32::from(screen.width_in_pixels),
                        i32::from(screen.height_in_pixels),
                    ],
                    options,
                )?;
                let format = setup
                    .pixmap_formats
                    .iter()
                    .find(|format| format.depth == screen.root_depth)
                    .context("The X11 image format is unavailable")?;
                ensure!(
                    matches!(format.bits_per_pixel, 24 | 32) && format.scanline_pad != 0,
                    "The X11 pixel format is unsupported"
                );
                let visual = screen
                    .allowed_depths
                    .iter()
                    .flat_map(|depth| &depth.visuals)
                    .find(|visual| visual.visual_id == screen.root_visual)
                    .context("The X11 visual is unavailable")?;
                let reply = connection
                    .get_image(
                        ImageFormat::Z_PIXMAP,
                        screen.root,
                        area.x as i16,
                        area.y as i16,
                        area.width as u16,
                        area.height as u16,
                        u32::MAX,
                    )?
                    .reply()?;
                let pad = usize::from(format.scanline_pad);
                let stride =
                    (area.width as usize * usize::from(format.bits_per_pixel)).div_ceil(pad) * pad
                        / 8;
                ensure!(
                    reply.data.len() >= stride * area.height as usize,
                    "The X11 image is incomplete"
                );
                let channels = usize::from(format.bits_per_pixel / 8);
                let mut image = RgbImage::new(area.width, area.height);
                for (y, row) in reply
                    .data
                    .chunks_exact(stride)
                    .take(area.height as usize)
                    .enumerate()
                {
                    for (x, pixel) in row
                        .chunks_exact(channels)
                        .take(area.width as usize)
                        .enumerate()
                    {
                        let mut bytes = [0; 4];
                        let value = if setup.image_byte_order == ImageOrder::LSB_FIRST {
                            bytes[..channels].copy_from_slice(pixel);
                            u32::from_le_bytes(bytes)
                        } else {
                            bytes[4 - channels..].copy_from_slice(pixel);
                            u32::from_be_bytes(bytes)
                        };
                        let component = |mask: u32| -> u8 {
                            if mask == 0 {
                                0
                            } else {
                                (((value & mask) >> mask.trailing_zeros()) * 255
                                    / (mask >> mask.trailing_zeros()))
                                    as u8
                            }
                        };
                        image.put_pixel(
                            x as u32,
                            y as u32,
                            Rgb([
                                component(visual.red_mask),
                                component(visual.green_mask),
                                component(visual.blue_mask),
                            ]),
                        );
                    }
                }
                crosshair(&mut image, area);
                Ok(image)
            }
        })();
        if result.is_err() {
            self.connection = None;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn gnome_capture_errors_are_actionable_and_hide_dbus_details() {
        let missing = gnome_capture_error(zbus::Error::FDO(Box::new(
            zbus::fdo::Error::ServiceUnknown("private D-Bus details".into()),
        )));
        assert!(failure_status(&missing).contains("Enable Desk Switch Tray"));
        assert!(!failure_status(&missing).contains("private D-Bus details"));

        let stale = gnome_capture_error(zbus::Error::FDO(Box::new(
            zbus::fdo::Error::UnknownMethod("private D-Bus details".into()),
        )));
        assert!(failure_status(&stale).contains("Install the current Desk Switch package"));
    }

    #[test]
    fn cursor_region_clamps_to_negative_monitor_edges_and_small_displays() {
        let options = Options::default();
        let area = region([-1900, -30], [-1920, -100, 0, 980], options).unwrap();
        assert_eq!(area.x, -1920);
        assert_eq!(area.y, -100);
        assert_eq!((area.cursor_x, area.cursor_y), (20, 70));
        let area = region([99, 70], [0, 0, 100, 80], options).unwrap();
        assert_eq!((area.width, area.height), (100, 80));
        assert_eq!((area.cursor_x, area.cursor_y), (99, 70));
        assert!(region([0, 0], [0, 0, 0, 100], options).is_err());
    }
    #[test]
    fn adversarial_rgb_frames_are_downscaled_under_the_encrypted_record_budget() {
        let mut seed = 17u32;
        let image = RgbImage::from_fn(640, 480, |_, _| {
            let mut rgb = [0; 3];
            for value in &mut rgb {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *value = seed as u8;
            }
            Rgb(rgb)
        });
        let png = encode(image).unwrap();
        assert!(png.len() <= MAX_PNG);
        let decoded = image::load_from_memory(&png).unwrap();
        assert!(decoded.width() <= 640 && decoded.height() <= 480);
        assert!(decoded.width() >= 128 && decoded.height() >= 96);
    }
}
