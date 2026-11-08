//! Official, unmodified VB-CABLE archives, available without a network request.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
    sync::mpsc,
};

pub const WEBSITE: &str = "https://vb-audio.com/Cable/";
pub const DONATE: &str = "https://vb-audio.com/Donate/index.htm";
const NOTICE: &str = include_str!("../assets/third-party/vb-cable/NOTICE.txt");
const LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum Platform {
    #[default]
    Windows,
    MacOS,
}
impl Platform {
    pub fn can_run_on(self, platform: &str) -> bool {
        matches!(
            (self, platform),
            (Self::Windows, "windows") | (Self::MacOS, "macos")
        )
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::MacOS => "macOS",
        }
    }
    pub fn archive(self) -> &'static str {
        match self {
            Self::Windows => "VBCABLE_Driver_Pack46.zip",
            Self::MacOS => "VBCable_MACDriver_Pack108.zip",
        }
    }
    fn folder(self) -> &'static str {
        match self {
            Self::Windows => "VB-CABLE-Windows-Pack46",
            Self::MacOS => "VB-CABLE-macOS-Pack108",
        }
    }
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Windows => {
                include_bytes!("../assets/third-party/vb-cable/VBCABLE_Driver_Pack46.zip")
            }
            Self::MacOS => {
                include_bytes!("../assets/third-party/vb-cable/VBCable_MACDriver_Pack108.zip")
            }
        }
    }
    fn sha256(self) -> &'static str {
        match self {
            Self::Windows => "bc44e8974191a5a2427d6c979e2dab08dbb09925f3f435dfa2f0ba3e45b6eb90",
            Self::MacOS => "e46b41c6876995403cb1da37d7c0d566f59edd83aa9fcbcdc3a205eb0b6e05c7",
        }
    }
}

fn unpack(bytes: &[u8], directory: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    ensure!(archive.len() <= 128, "Too many installer files");
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.enclosed_name().context("Unsafe installer path")?;
        ensure!(
            !name.as_os_str().is_empty()
                && name
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "Unsafe installer path"
        );
        ensure!(
            entry
                .unix_mode()
                .is_none_or(|mode| mode & 0o170000 != 0o120000),
            "Installer links are not supported"
        );
        total = total
            .checked_add(entry.size())
            .context("Invalid installer size")?;
        ensure!(total <= LIMIT, "Installer archive is too large");
        let destination = directory.join(name);
        if entry.is_dir() {
            fs::create_dir_all(destination)?;
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let copied = std::io::copy(&mut (&mut entry).take(LIMIT + 1), &mut file)?;
        ensure!(
            copied == entry.size() && copied <= LIMIT,
            "Invalid installer file size"
        );
        file.flush()?;
    }
    Ok(())
}

fn extract_at(platform: Platform, downloads: &Path) -> Result<PathBuf> {
    let bytes = platform.bytes();
    ensure!(
        format!("{:x}", Sha256::digest(bytes)) == platform.sha256(),
        "The bundled VB-CABLE archive is damaged"
    );
    let parent = downloads.join("Desk Switch");
    fs::create_dir_all(&parent)?;
    let mut destination = None;
    for number in 1..=1000 {
        let folder = parent.join(format!("{}-{number}", platform.folder()));
        match fs::create_dir(&folder) {
            Ok(()) => {
                destination = Some(folder);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let directory = destination.context("No unused installer folder is available")?;
    let result = (|| {
        fs::write(directory.join(platform.archive()), bytes)?;
        fs::write(directory.join("VB-CABLE-NOTICE.txt"), NOTICE)?;
        unpack(bytes, &directory)?;
        Ok(directory.clone())
    })();
    // This is the fresh directory created by this operation; existing files are never removed.
    if result.is_err() {
        let _ = fs::remove_dir_all(&directory);
    }
    result
}

pub struct Job {
    receive: mpsc::Receiver<Result<PathBuf, String>>,
    installing: bool,
}
impl Job {
    pub fn start(platform: Platform, ctx: eframe::egui::Context) -> Result<Self> {
        Self::operation(platform, false, ctx)
    }
    pub fn install(platform: Platform, ctx: eframe::egui::Context) -> Result<Self> {
        ensure!(
            platform.can_run_on(std::env::consts::OS),
            "Choose an installer for the selected computer's operating system"
        );
        Self::operation(platform, true, ctx)
    }
    fn operation(platform: Platform, installing: bool, ctx: eframe::egui::Context) -> Result<Self> {
        let (send, receive) = mpsc::channel();
        std::thread::Builder::new()
            .name(
                if installing {
                    "VB-CABLE installer launch"
                } else {
                    "VB-CABLE extraction"
                }
                .into(),
            )
            .spawn(move || {
                let result = dirs::download_dir()
                    .context("The Downloads folder is not available")
                    .and_then(|downloads| extract_at(platform, &downloads))
                    .and_then(|directory| {
                        if installing {
                            let installer = verified_installer(platform, &directory)?;
                            launch_installer(&installer, &directory)?;
                        }
                        Ok(directory)
                    })
                    .map_err(|error| format!("{error:#}"));
                let _ = send.send(result);
                ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
            })?;
        Ok(Self {
            receive,
            installing,
        })
    }
    pub fn installing(&self) -> bool {
        self.installing
    }
    pub fn poll(&self) -> Option<Result<PathBuf, String>> {
        match self.receive.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("The installer operation stopped".into()))
            }
        }
    }
}

fn installer_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Windows if cfg!(target_arch = "x86") => "VBCABLE_Setup.exe",
        Platform::Windows => "VBCABLE_Setup_x64.exe",
        Platform::MacOS => "VBCable_MACDriver_Pack108.dmg",
    }
}

/// Verify the complete freshly extracted package immediately before running its
/// official entry point. No executable path or argument is accepted from a peer.
fn verified_installer(platform: Platform, directory: &Path) -> Result<PathBuf> {
    ensure!(
        directory.is_absolute(),
        "The installer directory must be absolute"
    );
    let directory = directory.canonicalize()?;
    let bytes = platform.bytes();
    ensure!(
        format!("{:x}", Sha256::digest(bytes)) == platform.sha256(),
        "The bundled VB-CABLE archive is damaged"
    );
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.enclosed_name().context("Unsafe installer path")?;
        let path = directory.join(name);
        ensure!(
            !fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "An installer file was replaced with a link"
        );
        ensure!(
            path.canonicalize()?.starts_with(&directory),
            "Installer files must remain in their extracted directory"
        );
        let expected = {
            let mut data = Vec::new();
            (&mut entry).take(LIMIT + 1).read_to_end(&mut data)?;
            data
        };
        ensure!(
            fs::metadata(&path)?.len() == expected.len() as u64,
            "An extracted installer file changed. Extract a fresh saved installer and try again."
        );
        let actual = fs::read(&path)?;
        ensure!(
            actual == expected,
            "An extracted installer file changed. Extract a fresh saved installer and try again."
        );
    }
    let path = directory.join(installer_name(platform));
    ensure!(
        path.is_file(),
        "The official installer is missing from its archive"
    );
    Ok(path)
}

#[cfg(windows)]
fn shell_wide(value: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let mut units = value.encode_wide().collect::<Vec<_>>();
    // canonicalize returns extended paths, while Shell verbs expect ordinary
    // drive/UNC paths. Keep the original UTF-16 code units and argument boundary.
    if units.starts_with(&[92, 92, 63, 92, 85, 78, 67, 92]) {
        units.drain(..8);
        units.splice(0..0, [92, 92]);
    } else if units.starts_with(&[92, 92, 63, 92]) && units.get(5) == Some(&58) {
        units.drain(..4);
    }
    units.push(0);
    units
}

#[cfg(windows)]
fn launch_installer(installer: &Path, directory: &Path) -> Result<()> {
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_CANCELLED, GetLastError},
        UI::{
            Shell::{
                SEE_MASK_FLAG_LOG_USAGE, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC,
                SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
            },
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    };
    unsafe {
        // SAFETY: the apartment is initialized for this thread and the
        // matching CoUninitialize runs in the guard below, on the same thread.
        CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)
    }
    .ok()
    .context("Could not initialize the Windows installer launcher")?;
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                // SAFETY: this matches the successful CoInitializeEx above on
                // the same thread, and it runs exactly once.
                CoUninitialize();
            }
        }
    }
    let _apartment = Apartment;
    let file = shell_wide(installer.as_os_str());
    let directory = shell_wide(directory.as_os_str());
    let verb = shell_wide(std::ffi::OsStr::new("runas"));
    // SAFETY: SHELLEXECUTEINFOW is a plain C structure of flags, integers
    // and pointers, so the all-zero bit pattern is a valid value and every
    // pointer field is a null pointer.
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask =
        SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC | SEE_MASK_FLAG_LOG_USAGE;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpDirectory = directory.as_ptr();
    // The official standalone installer stays visible. Do not invent silent
    // flags or treat its launch as a completed driver installation.
    info.nShow = SW_SHOWNORMAL;
    // SAFETY: every pointer field of `info` refers to a NUL-terminated
    // buffer that outlives the call, and the structure is fully initialized.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        // SAFETY: GetLastError takes no arguments and only reads the calling
        // thread's last error code.
        let error = unsafe { GetLastError() };
        if error == ERROR_CANCELLED {
            anyhow::bail!("The Windows administrator prompt was cancelled");
        }
        return Err(std::io::Error::from_raw_os_error(error as i32))
            .context("Could not open the official VB-CABLE installer");
    }
    if !info.hProcess.is_null() {
        unsafe {
            // SAFETY: SEE_MASK_NOCLOSEPROCESS asked the shell to return the
            // process handle, and it is closed exactly once here.
            CloseHandle(info.hProcess);
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn launch_installer(installer: &Path, directory: &Path) -> Result<()> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mount = directory.join("VB-CABLE-image");
    fs::create_dir(&mount)?;
    let mut child = Command::new("/usr/bin/hdiutil")
        .args(["attach", "-readonly", "-nobrowse", "-mountpoint"])
        .arg(&mount)
        .arg(installer)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Could not mount the saved official VB-CABLE disk image")?;
    let since = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            ensure!(
                status.success(),
                "Could not mount the saved disk image. Open the extracted .dmg manually."
            );
            break;
        }
        if since.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "Mounting the saved disk image timed out. Open the extracted .dmg manually."
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let package = match mounted_package(&mount) {
        Ok(package) => package,
        Err(error) => {
            let _ = Command::new("/usr/bin/open").arg(&mount).spawn();
            return Err(error).context(
                "The disk image is open in Finder. Start its VB-CABLE installer manually",
            );
        }
    };
    let status = Command::new("/usr/bin/open")
        .args(["-b", "com.apple.installer"])
        .arg(package)
        .status()?;
    ensure!(
        status.success(),
        "Could not open the official package in macOS Installer"
    );
    // Installer may continue reading the package after open returns. Keep this
    // read-only image mounted; the user can eject it after the official setup.
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn mounted_package(mount: &Path) -> Result<PathBuf> {
    let root = mount.canonicalize()?;
    let mut directories = vec![(root.clone(), 0)];
    let mut packages = Vec::new();
    let mut visited = 0;
    while let Some((directory, depth)) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            visited += 1;
            ensure!(
                visited <= 128,
                "The disk image contains too many installer entries"
            );
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            ensure!(
                path.canonicalize()?.starts_with(&root),
                "The installer package is outside its disk image"
            );
            if path
                .extension()
                .is_some_and(|extension| extension == "pkg" || extension == "mpkg")
            {
                packages.push(path);
            } else if kind.is_dir() && depth < 2 {
                directories.push((path, depth + 1));
            }
        }
    }
    ensure!(
        packages.len() == 1,
        "The disk image does not contain exactly one installer package"
    );
    Ok(packages.pop().unwrap())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn launch_installer(_: &Path, _: &Path) -> Result<()> {
    anyhow::bail!("VB-CABLE installers can run only on their matching Windows or macOS computer")
}

pub fn open_folder(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && path.is_dir(),
        "The extracted installer folder is unavailable"
    );
    #[cfg(windows)]
    let command = "explorer.exe";
    #[cfg(target_os = "linux")]
    let command = "xdg-open";
    #[cfg(target_os = "macos")]
    let command = "open";
    std::process::Command::new(command).arg(path).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_official_archives_extract_without_overwriting_existing_files() {
        let root = std::env::temp_dir().join(format!("desk-switch-cable-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        for platform in [Platform::Windows, Platform::MacOS] {
            let first = extract_at(platform, &root).unwrap();
            assert_eq!(
                fs::read(first.join(platform.archive())).unwrap(),
                platform.bytes()
            );
            let expected = match platform {
                Platform::Windows => "VBCABLE_Setup_x64.exe",
                Platform::MacOS => "VBCable_MACDriver_Pack108.dmg",
            };
            assert!(first.join(expected).is_file());
            fs::write(first.join("keep.txt"), b"existing").unwrap();
            let second = extract_at(platform, &root).unwrap();
            assert_ne!(first, second);
            assert_eq!(fs::read(first.join("keep.txt")).unwrap(), b"existing");
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn extraction_refuses_traversal_and_never_writes_outside_its_folder() {
        use zip::{ZipWriter, write::SimpleFileOptions};
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("../outside", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"bad").unwrap();
        let data = zip.finish().unwrap().into_inner();
        let root =
            std::env::temp_dir().join(format!("desk-switch-cable-unsafe-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        assert!(unpack(&data, &root).is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn installer_entry_points_are_fixed_and_modified_packages_are_rejected_before_launch() {
        let root =
            std::env::temp_dir().join(format!("desk-switch-cable-verify-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        for platform in [Platform::Windows, Platform::MacOS] {
            let extracted = extract_at(platform, &root).unwrap();
            let executable = verified_installer(platform, &extracted).unwrap();
            assert_eq!(executable.file_name().unwrap(), installer_name(platform));
            assert_eq!(
                executable.parent().unwrap(),
                extracted.canonicalize().unwrap()
            );
            // Verifying just the EXE/DMG is insufficient: the Windows installer
            // also reads companion driver/INF files from the same directory.
            let companion = if platform == Platform::Windows {
                extracted.join("vbMmeCable64_win10.inf")
            } else {
                executable
            };
            fs::write(companion, b"modified installer content").unwrap();
            assert!(verified_installer(platform, &extracted).is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mac_package_selection_uses_only_one_package_inside_the_mounted_image() {
        let root =
            std::env::temp_dir().join(format!("desk-switch-cable-mount-{}", std::process::id()));
        let folder = root.join("VB-CABLE");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            root.join("Uninstall.command"),
            b"not an installation entry point",
        )
        .unwrap();
        assert!(mounted_package(&root).is_err());
        let package = folder.join("VBCABLE.pkg");
        fs::write(&package, b"synthetic package; never executed").unwrap();
        assert_eq!(
            mounted_package(&root).unwrap(),
            package.canonicalize().unwrap()
        );
        fs::write(root.join("Other.pkg"), b"ambiguous").unwrap();
        assert!(mounted_package(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_foreign_platform_cannot_start_an_installer_job() {
        let platform = if cfg!(target_os = "macos") {
            Platform::Windows
        } else {
            Platform::MacOS
        };
        assert!(Job::install(platform, eframe::egui::Context::default()).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn shell_paths_preserve_unicode_spaces_and_unc_paths_without_namespace_prefixes() {
        for (source, expected) in [
            (
                r"\\?\C:\Users\Иван\Downloads\Desk Switch\VBCABLE_Setup_x64.exe",
                r"C:\Users\Иван\Downloads\Desk Switch\VBCABLE_Setup_x64.exe",
            ),
            (
                r"\\?\UNC\server\share\Desk Switch\VBCABLE_Setup_x64.exe",
                r"\\server\share\Desk Switch\VBCABLE_Setup_x64.exe",
            ),
            (
                r"C:\Users\user\Downloads\VBCABLE_Setup_x64.exe",
                r"C:\Users\user\Downloads\VBCABLE_Setup_x64.exe",
            ),
        ] {
            let wide = shell_wide(std::ffi::OsStr::new(source));
            assert_eq!(wide.last(), Some(&0));
            assert_eq!(
                String::from_utf16(&wide[..wide.len() - 1]).unwrap(),
                expected
            );
        }
    }
}
