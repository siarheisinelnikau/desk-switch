//! License texts embedded in standalone releases and exposed by `--licenses`.

pub const NOTICES: &str = concat!(
    "Desk Switch\nhttps://github.com/siarheisinelnikau/desk-switch\n\n",
    include_str!("../LICENSE"),
    "\n\negui_software_backend 0.0.2 — DGriffin91 and contributors\n",
    "https://github.com/DGriffin91/egui_software_backend\n",
    "The Windows CPU renderer is distributed under the MIT option of its MIT OR Apache-2.0 license.\n\n",
    include_str!("../vendor/egui_software_backend/LICENSE-MIT"),
    "\n\nm1ddc — waydabber\nhttps://github.com/waydabber/m1ddc\n",
    "Included in the macOS Apple Silicon DDC transport.\n\n",
    include_str!("../vendor/m1ddc/LICENSE"),
);
