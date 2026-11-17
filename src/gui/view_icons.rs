//! Small production vectors rasterized with the application's software backend.
use super::*;
use egui_software_backend::{BufferMutRef, ColorFieldOrder, EguiSoftwareRender};

#[test]
fn small_icon_stands_have_continuous_strokes_at_fractional_positions() {
    let scale = 0.68;
    for dpi in [1.0_f32, 1.25, 1.5, 2.0] {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(dpi);
        let size = egui::vec2(160.0, 80.0);
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            },
            |ui| {
                let painter = ui.painter();
                painter.rect_filled(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, size),
                    0,
                    Color32::from_rgb(21, 29, 39),
                );
                for (row, icon) in [DeviceIcon::Microphone, DeviceIcon::Display]
                    .into_iter()
                    .enumerate()
                {
                    for column in 0..4 {
                        let offset = column as f32 * 0.25;
                        paint_device(
                            painter,
                            icon,
                            egui::pos2(
                                20.0 + column as f32 * 40.0 + offset,
                                20.0 + row as f32 * 40.0 + offset,
                            ),
                            scale,
                            Color32::WHITE,
                        );
                    }
                }
            },
        );
        let width = (size.x * output.pixels_per_point) as usize;
        let height = (size.y * output.pixels_per_point) as usize;
        let mut pixels = vec![[0, 0, 0, 255]; width * height];
        EguiSoftwareRender::new(ColorFieldOrder::Rgba).render(
            &mut BufferMutRef::new(&mut pixels, width, height),
            &ctx.tessellate(output.shapes, output.pixels_per_point),
            &output.textures_delta,
            output.pixels_per_point,
        );
        if let Some(path) = std::env::var_os("DESK_SWITCH_ICON_PROOF_DIR") {
            let path = std::path::PathBuf::from(path);
            std::fs::create_dir_all(&path).unwrap();
            image::RgbaImage::from_raw(
                width as u32,
                height as u32,
                pixels.iter().flatten().copied().collect(),
            )
            .unwrap()
            .save(path.join(format!("stands-{}.png", (dpi * 100.0) as u32)))
            .unwrap();
        }
        for row in 0..2 {
            for column in 0..4 {
                let offset = column as f32 * 0.25;
                let center = egui::pos2(
                    20.0 + column as f32 * 40.0 + offset,
                    20.0 + row as f32 * 40.0 + offset,
                );
                let y = (center.y + if row == 0 { 15.0 } else { 13.0 } * scale) * dpi;
                for dx in -2..=2 {
                    let x = ((center.x + dx as f32) * dpi).floor() as usize;
                    let brightness = ((y - 1.5 * dpi).floor() as usize
                        ..=(y + 1.5 * dpi).ceil() as usize)
                        .map(|y| pixels[y * width + x][0])
                        .max()
                        .unwrap();
                    assert!(
                        brightness > 55,
                        "Detached stand: dpi={dpi}, row={row}, offset={offset}, dx={dx}, brightness={brightness}"
                    );
                }
            }
        }
    }
}
