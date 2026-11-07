fn main() {
    println!("cargo:rerun-if-changed=assets/desk-switch.ico");
    #[cfg(windows)]
    {
        winresource::WindowsResource::new()
            .set_icon("assets/desk-switch.ico")
            .set("ProductName", "Desk Switch")
            .set("FileDescription", "Desk Switch")
            .set("InternalName", "desk-switch")
            .set(
                "OriginalFilename",
                &format!(
                    "desk-switch-v{}-windows-x86-64.exe",
                    env!("CARGO_PKG_VERSION")
                ),
            )
            .compile()
            .expect("embedding the Desk Switch application icon and version");
    }
}
