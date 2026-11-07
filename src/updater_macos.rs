//! Pinned-signature, complete .app updates. The handoff helper runs outside the
//! replaced bundle, waits for the desktop lock and rolls back a failed startup.
use super::*;
use std::{
    collections::BTreeSet,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
};

pub(super) struct Prepared {
    archive: PathBuf,
    envelope: Envelope,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Ticket {
    envelope: Envelope,
    archive: PathBuf,
    app: PathBuf,
    state: PathBuf,
    id: String,
}
static HEALTH: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

fn unique() -> String {
    format!(
        "{:x}{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}
fn private(directory: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(directory)?;
    ensure!(
        !fs::symlink_metadata(directory)?.file_type().is_symlink(),
        "The update directory must not be a link"
    );
    Ok(())
}
fn app() -> Result<PathBuf> {
    let executable = std::env::current_exe()?.canonicalize()?;
    let macos = executable
        .parent()
        .context("The application executable has no parent")?;
    let contents = macos
        .parent()
        .context("The application bundle is incomplete")?;
    let app = contents
        .parent()
        .context("The application bundle is incomplete")?;
    ensure!(
        macos.file_name().is_some_and(|part| part == "MacOS")
            && contents.file_name().is_some_and(|part| part == "Contents")
            && app.extension().is_some_and(|part| part == "app"),
        "Install the complete Desk Switch.app bundle in a writable Applications folder before updating"
    );
    Ok(app.to_path_buf())
}
fn writable(parent: &Path) -> Result<()> {
    let probe = parent.join(format!(".desk-switch-write-check-{}", unique()));
    let file=fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&probe).context("This application folder is not writable. Move Desk Switch.app to ~/Applications for password-free updates, then retry")?;
    drop(file);
    fs::remove_file(probe)?;
    Ok(())
}
pub(super) fn download(
    dir: &Path,
    candidate: &Candidate,
    mut progress: impl FnMut(u64, u64),
) -> Result<Outcome> {
    let envelope = candidate.envelope.clone();
    let release = &candidate.release;
    let artifact = release
        .targets
        .get(target()?)
        .context("This release has no signed application for this Mac architecture")?;
    let updates = dir.join("updates");
    private(&updates)?;
    let _lock = crate::config::lock(&updates, "macos-download.lock")?;
    let archive = updates.join(format!("macos-{}-{}.zip", release.version, artifact.sha256));
    if checksum(&archive, artifact).is_err() {
        let partial = updates.join(format!(".macos-download-{}", unique()));
        let result = (|| -> Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&partial)?;
            let mut response = client()?
                .get(https(&artifact.url)?)
                .send()?
                .error_for_status()?;
            let mut downloaded = 0u64;
            let mut buffer = [0u8; 65536];
            progress(0, artifact.size);
            loop {
                let count = response.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                downloaded = downloaded
                    .checked_add(count as u64)
                    .context("The update size overflowed")?;
                ensure!(
                    downloaded <= artifact.size,
                    "The download exceeds its signed size"
                );
                file.write_all(&buffer[..count])?;
                progress(downloaded, artifact.size);
            }
            file.sync_all()?;
            drop(file);
            checksum(&partial, artifact)?;
            fs::rename(&partial, &archive)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(partial);
        }
        result?;
    }
    let validation = updates.join(format!(".macos-validate-{}", unique()));
    let result = extract(&archive, &validation, release);
    let _ = fs::remove_dir_all(&validation);
    result?;
    Ok(Outcome::Prepared(
        release.version.clone(),
        super::Prepared {
            macos: Prepared { archive, envelope },
        },
    ))
}

fn entry(name: &str) -> Result<PathBuf> {
    ensure!(
        name.is_ascii() && !name.contains(['\\', '\0', ':']) && !name.chars().any(char::is_control),
        "Unsafe macOS update path"
    );
    let name = name.strip_suffix('/').unwrap_or(name);
    let parts: Vec<_> = name.split('/').collect();
    ensure!(
        parts.first() == Some(&"Desk Switch.app")
            && parts
                .iter()
                .all(|part| !part.is_empty() && !matches!(*part, "." | "..")),
        "The update must contain exactly one Desk Switch.app bundle"
    );
    Ok(parts.iter().collect())
}
fn macho(bytes: &[u8]) -> Result<()> {
    let cpu = if cfg!(target_arch = "aarch64") {
        0x0100000cu32
    } else {
        0x01000007
    };
    ensure!(
        bytes.len() >= 32
            && bytes[..4] == [0xcf, 0xfa, 0xed, 0xfe]
            && u32::from_le_bytes(bytes[4..8].try_into().unwrap()) == cpu
            && u32::from_le_bytes(bytes[12..16].try_into().unwrap()) == 2,
        "The update contains a wrong-architecture or invalid Mach-O executable"
    );
    Ok(())
}
fn verify_codesign(bundle: &Path) -> Result<()> {
    let mut child = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let errors = child.stderr.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut data = Vec::new();
        let _ = errors.take(16384).read_to_end(&mut data);
        data
    });
    let since = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if since.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Checking the application signature timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let errors = reader.join().unwrap_or_default();
    ensure!(
        status.success(),
        "The downloaded application signature is invalid: {}",
        String::from_utf8_lossy(&errors)
    );
    Ok(())
}
fn extract(archive: &Path, destination: &Path, release: &Release) -> Result<PathBuf> {
    ensure!(
        !destination.exists(),
        "The update staging directory already exists"
    );
    private(destination)?;
    let mut archive = zip::ZipArchive::new(fs::File::open(archive)?)?;
    ensure!(
        !archive.is_empty() && archive.len() <= 4096,
        "Invalid macOS bundle entry count"
    );
    let mut total = 0u64;
    let mut paths = BTreeSet::new();
    let mut prefixes = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        let relative = entry(file.name())?;
        let name = relative.to_string_lossy();
        ensure!(
            paths.insert(name.to_ascii_lowercase()),
            "Duplicate macOS update path"
        );
        let mut prefix = PathBuf::new();
        for component in relative.components() {
            prefix.push(component.as_os_str());
            let text = prefix.to_string_lossy().into_owned();
            if let Some(previous) = prefixes.insert(text.to_ascii_lowercase(), text.clone()) {
                ensure!(previous == text, "Case-colliding macOS update path");
            }
        }
        let kind = file.unix_mode().unwrap_or(0) & 0o170000;
        ensure!(
            matches!(kind, 0 | 0o100000 | 0o040000) && (kind != 0o040000 || file.is_dir()),
            "The macOS update may contain only regular files and directories"
        );
        ensure!(
            matches!(
                file.compression(),
                zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
            ),
            "Unsupported macOS update compression"
        );
        total = total
            .checked_add(file.size())
            .context("The bundle size overflowed")?;
        ensure!(
            file.size() <= MAX_BINARY && total <= 256 * 1024 * 1024,
            "The expanded macOS update is too large"
        );
        let target = destination.join(&relative);
        if file.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        fs::create_dir_all(target.parent().unwrap())?;
        let executable = matches!(
            relative.to_str(),
            Some(
                "Desk Switch.app/Contents/MacOS/desk-switch"
                    | "Desk Switch.app/Contents/Helpers/desk-switch-native"
                    | "Desk Switch.app/Contents/Helpers/desk-switch-ddc"
            )
        );
        if executable {
            ensure!(
                file.unix_mode().unwrap_or(0) & 0o111 != 0,
                "The macOS update executable is not marked executable"
            );
        }
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(if executable { 0o755 } else { 0o644 })
            .open(&target)?;
        let expected = file.size();
        let written = std::io::copy(&mut file.by_ref().take(expected + 1), &mut output)?;
        ensure!(
            written == expected,
            "A macOS update entry has an invalid expanded size"
        );
        output.sync_all()?;
    }
    let bundle = destination.join("Desk Switch.app");
    let executable = bundle.join("Contents/MacOS/desk-switch");
    let main = fs::read(&executable)?;
    macho(&main)?;
    binary_version(&executable, release)?;
    for name in ["desk-switch-native", "desk-switch-ddc"] {
        let helper = fs::read(bundle.join("Contents/Helpers").join(name))?;
        macho(&helper)?;
        ensure!(
            main.windows(helper.len()).any(|part| part == helper),
            "A macOS helper differs from its signed embedded copy"
        );
    }
    let info: serde_json::Value = crate::macos_native::query(
        "bundle-info",
        &[bundle
            .join("Contents/Info.plist")
            .to_string_lossy()
            .into_owned()],
    )?;
    for (key, value) in [
        ("CFBundleIdentifier", "com.deskswitch.Desktop"),
        ("CFBundleExecutable", "desk-switch"),
        ("CFBundlePackageType", "APPL"),
        ("CFBundleShortVersionString", release.version.as_str()),
        ("CFBundleVersion", release.version.as_str()),
    ] {
        ensure!(
            info[key].as_str() == Some(value),
            "The macOS bundle identity or version does not match the signed release"
        );
    }
    verify_codesign(&bundle)?;
    Ok(bundle)
}

impl Prepared {
    pub(super) fn handoff(&mut self, state: &Path) -> Result<()> {
        let state = state.canonicalize()?;
        let app = app()?;
        writable(app.parent().unwrap())?;
        let release = verified(&self.envelope, PUBLIC_KEY)?;
        checksum(
            &self.archive,
            release
                .targets
                .get(target()?)
                .context("The signed Mac target is missing")?,
        )?;
        let id = unique();
        let updates = state.join("updates");
        private(&updates)?;
        let ticket = updates.join(format!("mac-apply-{id}.json"));
        let data = Ticket {
            envelope: self.envelope.clone(),
            archive: self.archive.canonicalize()?,
            app,
            state: state.clone(),
            id: id.clone(),
        };
        crate::config::write_atomic(&ticket, &serde_json::to_string(&data)?)?;
        let helper = updates.join(format!("mac-apply-{id}"));
        fs::copy(std::env::current_exe()?, &helper)?;
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700))?;
        let log = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(ticket.with_extension("log"))?;
        let mut child = Command::new(&helper)
            .arg("internal-macos-update")
            .arg(&ticket)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()
            .context("Starting the macOS bundle installer")?;
        let since = Instant::now();
        let ready = ticket.with_extension("ready");
        loop {
            if ready.is_file() {
                return Ok(());
            }
            if let Some(status) = child.try_wait()? {
                bail!(
                    "macOS update preparation failed ({status}): {}",
                    fs::read_to_string(updates.join("last-error.txt")).unwrap_or_default()
                );
            }
            if since.elapsed() > Duration::from_secs(60) {
                let _ = child.kill();
                let _ = child.wait();
                bail!(
                    "macOS update preparation timed out; the running application has been retained"
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

pub(super) fn cli(args: &mut Vec<String>) -> Result<bool> {
    if args
        .first()
        .is_some_and(|arg| arg == "internal-macos-update")
    {
        ensure!(args.len() == 2, "The macOS update ticket is missing");
        apply(Path::new(&args[1]))?;
        return Ok(true);
    }
    if args.first().is_some_and(|arg| arg == "--apply-update") {
        bail!("Use the signed .app update action in Desk Switch settings on macOS");
    }
    if args.first().is_some_and(|arg| arg == "gui")
        && args.len() >= 3
        && args[args.len() - 2] == "--macos-update-health"
    {
        let path = PathBuf::from(args.pop().unwrap());
        args.pop();
        *HEALTH.lock().unwrap() = Some(path);
    }
    Ok(false)
}
pub(super) fn acknowledge(state: &Path) {
    if let Some(path) = HEALTH.lock().unwrap().take()
        && path.parent() == Some(state.join("updates").as_path())
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("mac-apply-") && name.ends_with(".health"))
    {
        let _ = crate::config::write_atomic(&path, crate::compatibility::VERSION);
    }
}
fn start(executable: &Path, state: &Path, health: Option<&Path>) -> Result<std::process::Child> {
    let mut command = Command::new(executable);
    // A visible first frame is required for the startup-health acknowledgment.
    command.args(["gui", "--state-dir"]).arg(state);
    if let Some(health) = health {
        command.arg("--macos-update-health").arg(health);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command.spawn()?)
}
fn apply(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && fs::metadata(path)?.len() <= MAX_MANIFEST * 2,
        "Invalid macOS update ticket"
    );
    let ticket: Ticket = serde_json::from_slice(&fs::read(path)?)?;
    ensure!(
        ticket.id.len() <= 48
            && !ticket.id.is_empty()
            && ticket.id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Invalid macOS update identifier"
    );
    ensure!(
        ticket.state.is_absolute()
            && ticket.state.canonicalize()? == ticket.state
            && path
                == ticket
                    .state
                    .join("updates")
                    .join(format!("mac-apply-{}.json", ticket.id))
            && ticket.archive.parent() == Some(ticket.state.join("updates").as_path()),
        "Invalid macOS update paths"
    );
    let result = apply_inner(path, &ticket);
    if let Err(error) = &result {
        let _ = crate::config::write_atomic(
            &ticket.state.join("updates/last-error.txt"),
            &format!("{error:#}"),
        );
        // Also cover failures between the desktop closing and the first new
        // process starting. Only restart an exact copy of this old executable,
        // never an arbitrary path from an edited/stale update ticket.
        let executable = ticket.app.join("Contents/MacOS/desk-switch");
        let same_binary = (|| -> Result<bool> {
            let current = std::env::current_exe()?;
            let size = fs::metadata(&current)?.len();
            ensure!(size <= MAX_BINARY, "The update helper is too large");
            Ok(fs::metadata(&executable)?.len() == size
                && fs::read(&executable)? == fs::read(current)?)
        })();
        if same_binary.unwrap_or(false)
            && let Ok(lock) = crate::config::lock(&ticket.state, "desktop.lock")
        {
            drop(lock);
            let _ = start(&executable, &ticket.state, None);
        }
    }
    result
}
fn apply_inner(path: &Path, ticket: &Ticket) -> Result<()> {
    let _lock = crate::config::lock(&ticket.state.join("updates"), "macos-install.lock")?;
    let release = verified(&ticket.envelope, PUBLIC_KEY)?;
    ensure!(
        semver::Version::parse(&release.version)?
            > semver::Version::parse(crate::compatibility::VERSION)?,
        "Refusing an old macOS update"
    );
    checksum(
        &ticket.archive,
        release
            .targets
            .get(target()?)
            .context("The signed Mac target is missing")?,
    )?;
    ensure!(
        ticket.app.is_absolute()
            && ticket.app.extension().is_some_and(|part| part == "app")
            && ticket.app.canonicalize()? == ticket.app,
        "Invalid macOS application destination"
    );
    let parent = ticket
        .app
        .parent()
        .context("The application has no installation directory")?;
    writable(parent)?;
    let staging = parent.join(format!(".desk-switch-{}-staging", ticket.id));
    let backup = parent.join(format!(".desk-switch-{}-previous.app", ticket.id));
    ensure!(!backup.exists(), "The update backup already exists");
    let prepared = match extract(&ticket.archive, &staging, &release) {
        Ok(bundle) => bundle,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    crate::config::write_atomic(&path.with_extension("ready"), "ready")?;
    let since = Instant::now();
    let locks = loop {
        let result = (|| -> Result<_> {
            Ok([
                crate::config::lock(&ticket.state, "desktop.lock")?,
                crate::config::lock(&ticket.state, "run.lock")?,
                crate::config::lock(&ticket.state, "audio.lock")?,
            ])
        })();
        match result {
            Ok(locks) => break locks,
            Err(_) if since.elapsed() < Duration::from_secs(45) => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(error
                    .context("Desk Switch services did not stop; the application was retained"));
            }
        }
    };
    fs::rename(&ticket.app, &backup).context("Backing up the complete application bundle")?;
    if let Err(error) = fs::rename(&prepared, &ticket.app) {
        let restored = fs::rename(&backup, &ticket.app);
        let _ = fs::remove_dir_all(&staging);
        restored.context("The previous application could not be restored")?;
        return Err(anyhow::Error::from(error)
            .context("Installing the new application failed; the previous bundle was restored"));
    }
    drop(locks);
    let health = path.with_extension("health");
    let executable = ticket.app.join("Contents/MacOS/desk-switch");
    let started = start(&executable, &ticket.state, Some(&health));
    let mut child = started.ok();
    let since = Instant::now();
    let mut healthy = false;
    while let Some(process) = child.as_mut() {
        if fs::read_to_string(&health).ok().as_deref() == Some(release.version.as_str()) {
            healthy = true;
            break;
        }
        if !matches!(process.try_wait(), Ok(None)) || since.elapsed() > Duration::from_secs(30) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !healthy {
        if let Some(child) = child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let failed = parent.join(format!(".desk-switch-{}-failed.app", ticket.id));
        fs::rename(&ticket.app, &failed)?;
        fs::rename(&backup, &ticket.app)
            .context("Restoring the previous complete application bundle")?;
        let _ = fs::remove_dir_all(&failed);
        let _ = fs::remove_dir_all(&staging);
        bail!("The updated application did not confirm startup; the previous bundle was restored");
    }
    let _ = fs::remove_dir_all(&backup);
    let _ = fs::remove_dir_all(&staging);
    let _ = fs::remove_file(ticket.state.join("updates/last-error.txt"));
    for file in [path.to_path_buf(), path.with_extension("ready"), health] {
        let _ = fs::remove_file(file);
    }
    let helper = path.with_extension("");
    if std::env::current_exe().ok().as_ref() == Some(&helper) {
        let _ = fs::remove_file(helper);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_paths_cannot_escape_or_alias_the_bundle() {
        for name in [
            "../Desk Switch.app/x",
            "/Desk Switch.app/x",
            "Desk Switch.app/../x",
            "Desk Switch.app//x",
            "Desk Switch.app/a\\b",
            "Desk Switch.app/a\0b",
            "Other.app/Contents/x",
            "Desk Switch.app/资源",
        ] {
            assert!(entry(name).is_err(), "{name}");
        }
        assert_eq!(
            entry("Desk Switch.app/Contents/MacOS/desk-switch").unwrap(),
            PathBuf::from("Desk Switch.app/Contents/MacOS/desk-switch")
        );
    }
    #[test]
    fn macho_requires_the_native_executable_architecture() {
        let mut header = [0u8; 32];
        header[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        header[4..8].copy_from_slice(
            &(if cfg!(target_arch = "aarch64") {
                0x0100000cu32
            } else {
                0x01000007u32
            })
            .to_le_bytes(),
        );
        header[12..16].copy_from_slice(&2u32.to_le_bytes());
        assert!(macho(&header).is_ok());
        header[4] ^= 1;
        assert!(macho(&header).is_err());
        assert!(macho(&[]).is_err());
    }
}
