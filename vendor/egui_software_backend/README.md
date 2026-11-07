# Vendored egui software rasterizer

Source: `egui_software_backend` 0.0.2, https://github.com/DGriffin91/egui_software_backend.
License: MIT OR Apache-2.0; original license files are included.

This copy contains the CPU rasterizer only. The optional upstream window host, examples, and test-image tooling are omitted. The egui dependency is adapted to the application's pinned 0.35.0 version, and the old egui ahash re-export is replaced with the standard HashMap. Window management and Windows GDI presentation are implemented by `src/gui/software.rs`. The Windows application uses this path instead of an OpenGL or DirectX renderer.
