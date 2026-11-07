fn main() {
    println!("cargo:rerun-if-changed=assets/desk-switch.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=packaging/macos/NativeHelper.swift");
        println!("cargo:rerun-if-changed=packaging/macos/NativeHelper-Info.plist");
        let architecture = match std::env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
            "aarch64" => "arm64",
            "x86_64" => "x86_64",
            other => panic!("unsupported macOS architecture: {other}"),
        };
        let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap())
            .join("desk-switch-native");
        let status = std::process::Command::new("xcrun")
            .args([
                "swiftc",
                "-swift-version",
                "5",
                "-O",
                "-parse-as-library",
                "-target",
                &format!("{architecture}-apple-macos13.0"),
                "packaging/macos/NativeHelper.swift",
                "-o",
            ])
            .arg(&output)
            .args([
                "-Xlinker",
                "-sectcreate",
                "-Xlinker",
                "__TEXT",
                "-Xlinker",
                "__info_plist",
                "-Xlinker",
                "packaging/macos/NativeHelper-Info.plist",
            ])
            .status()
            .expect("macOS builds require Xcode Command Line Tools and the macOS SDK");
        assert!(
            status.success(),
            "compiling the macOS CoreGraphics/CoreAudio helper"
        );
        println!("cargo:rerun-if-changed=packaging/macos/DisplayDDC.m");
        println!("cargo:rerun-if-changed=vendor/m1ddc");
        let ddc = output.with_file_name("desk-switch-ddc");
        let mut command = std::process::Command::new("xcrun");
        command.args([
            "clang",
            "-O2",
            "-fmodules",
            "-target",
            &format!("{architecture}-apple-macos13.0"),
            "packaging/macos/DisplayDDC.m",
            "-framework",
            "Foundation",
            "-framework",
            "ApplicationServices",
            "-framework",
            "IOKit",
        ]);
        if architecture == "arm64" {
            command.args([
                "-Ivendor/m1ddc/headers",
                "vendor/m1ddc/sources/i2c.m",
                "vendor/m1ddc/sources/ioregistry.m",
                "-framework",
                "CoreDisplay",
            ]);
        }
        let status = command
            .arg("-o")
            .arg(&ddc)
            .status()
            .expect("compiling the macOS DDC helper");
        assert!(status.success(), "compiling the macOS DDC helper");
        // Exporting these exact signed bytes into the .app keeps the embedded
        // integrity check and the native helper's TCC identity consistent.
        for helper in [&output, &ddc] {
            let status = std::process::Command::new("codesign")
                .args(["--force", "--sign", "-", "--timestamp=none"])
                .arg(helper)
                .status()
                .expect("ad-hoc signing the macOS helper");
            assert!(status.success(), "ad-hoc signing the macOS helper");
        }
    }
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
