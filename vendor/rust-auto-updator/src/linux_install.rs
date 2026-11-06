//! Debian installs use apt/polkit; dpkg-owned executables are never overwritten.
use crate::{Config, InstallMode, InstallOptions, Release, download, hash};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Serialize, Deserialize)]
struct Plan {
    target: PathBuf,
    parent_pid: u32,
    parent_start: String,
    package: String,
    version: String,
    architecture: String,
    digest: String,
    size: u64,
    helper_digest: String,
    restart_args: Vec<String>,
    installed: bool,
}

pub struct Prepared {
    directory: PathBuf,
    config: Box<Config>,
    handed_off: bool,
}

impl Prepared {
    pub fn set_restart_args(&mut self, arguments: Vec<String>) -> Result<(), String> {
        if self.handed_off {
            return Err("The update has already been handed off.".into());
        }
        let mut plan = read_plan(&self.directory, &self.config)?;
        plan.restart_args = arguments;
        write_plan(&self.directory, &plan)
    }
}
impl Drop for Prepared {
    fn drop(&mut self) {
        if !self.handed_off {
            cleanup(&self.directory, &self.config.install);
        }
    }
}

fn package_name(config: &Config) -> Result<&str, String> {
    match &config.mode {
        InstallMode::DebianPackage { package } => Ok(package),
        _ => Err("Linux installation requires a Debian package configuration.".into()),
    }
}

fn output(mut command: Command) -> Result<String, String> {
    let result = command.output().map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr)
            .chars()
            .take(4000)
            .collect());
    }
    String::from_utf8(result.stdout).map_err(|e| e.to_string())
}

/// Require the running application to belong to the configured installed package.
/// Source builds and copied binaries must first be installed through their .deb.
pub fn installation_target(config: &Config) -> Result<PathBuf, String> {
    config.validate()?;
    let target = std::env::current_exe().map_err(|e| e.to_string())?;
    validate_target(&target, package_name(config)?)?;
    Ok(target)
}

fn validate_target(target: &Path, package: &str) -> Result<(), String> {
    let mut command = Command::new("/usr/bin/dpkg-query");
    command.args(["--listfiles", package]);
    let files = output(command)?;
    if !target.is_absolute()
        || !target.is_file()
        || !files.lines().any(|line| Path::new(line) == target)
    {
        return Err("Install this application from its .deb before using in-app updates.".into());
    }
    Ok(())
}

fn installed_version(package: &str) -> Result<String, String> {
    let mut command = Command::new("/usr/bin/dpkg-query");
    command.args([
        "--show",
        "--showformat=${db:Status-Status}\n${Version}\n",
        package,
    ]);
    let value = output(command)?;
    let mut lines = value.lines();
    if lines.next() != Some("installed") {
        return Err("The Debian package is not fully installed.".into());
    }
    lines
        .next()
        .map(str::to_owned)
        .ok_or("The installed package has no version.".into())
}

fn verify_package(
    path: &Path,
    digest: &str,
    size: u64,
    package: &str,
    version: &str,
    architecture: &str,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() != size || hash(path)? != digest {
        return Err(
            "The Debian package failed size/SHA-256 verification. Nothing was installed.".into(),
        );
    }
    let mut command = Command::new("/usr/bin/dpkg-deb");
    command
        .args([
            "--show",
            "--showformat=${Package}\n${Version}\n${Architecture}\n",
        ])
        .arg(path);
    let metadata = output(command)?;
    if metadata.lines().collect::<Vec<_>>() != [package, version, architecture] {
        return Err(
            "The Debian package name, version or architecture does not match the release.".into(),
        );
    }
    Ok(())
}

fn new_stage(options: &InstallOptions) -> Result<PathBuf, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for _ in 0..32 {
        let path = std::env::temp_dir().join(format!(
            "{}{}-{stamp}-{}",
            options.stage_prefix,
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("Cannot create a private update staging directory.".into())
}

fn valid_stage(directory: &Path, options: &InstallOptions) -> bool {
    let Ok(metadata) = fs::symlink_metadata(directory) else {
        return false;
    };
    // SAFETY: geteuid has no arguments or side effects.
    let uid = unsafe { libc::geteuid() };
    metadata.is_dir()
        && metadata.uid() == uid
        && metadata.mode() & 0o077 == 0
        && directory
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(&options.stage_prefix))
        && directory.parent().and_then(|p| p.canonicalize().ok())
            == std::env::temp_dir().canonicalize().ok()
}

fn cleanup(directory: &Path, options: &InstallOptions) {
    if valid_stage(directory, options) {
        let _ = fs::remove_dir_all(directory);
    }
}

fn write_plan(directory: &Path, plan: &Plan) -> Result<(), String> {
    let data = serde_json::to_vec(plan).map_err(|e| e.to_string())?;
    if data.len() > 64 * 1024 {
        return Err("Update restart arguments are too large.".into());
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(directory.join("update.json"))
        .map_err(|e| e.to_string())?;
    file.write_all(&data)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())
}

fn read_plan(directory: &Path, config: &Config) -> Result<Plan, String> {
    config.validate()?;
    if !valid_stage(directory, &config.install) {
        return Err("Invalid private update directory.".into());
    }
    let path = directory.join("update.json");
    let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err("Invalid update plan file.".into());
    }
    let plan: Plan = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let version = semver::Version::parse(&plan.version).map_err(|e| e.to_string())?;
    if plan.package != package_name(config)?
        || plan.architecture != config.architecture.debian_name()
        || !version.pre.is_empty()
        || version.cmp_precedence(&config.current_version).is_le()
        || plan.parent_pid == 0
        || plan.size < 1024
        || plan.size > config.max_executable_size
        || [&plan.digest, &plan.helper_digest]
            .iter()
            .any(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        || plan
            .restart_args
            .iter()
            .any(|s| s.contains('\0') || s == "--apply-update" || s == "--cleanup-update")
    {
        return Err("The update plan does not match this application.".into());
    }
    validate_target(&plan.target, &plan.package)?;
    Ok(plan)
}

fn process_start(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<_> = stat.rsplit_once(") ")?.1.split_whitespace().collect();
    // A launcher may leave its exited child as a zombie until it next reaps.
    // The executable is already gone; waiting for /proc removal delays restart.
    if matches!(fields.first().copied(), Some("Z" | "X")) {
        return None;
    }
    fields.get(19).map(|value| (*value).to_owned())
}

pub(crate) fn prepare(
    release: &Release,
    config: &Config,
    progress: impl FnMut(u64, u64),
) -> Result<Prepared, String> {
    prepare_cancellable(release, config, &AtomicBool::new(false), progress)
}
pub(crate) fn prepare_cancellable(
    release: &Release,
    config: &Config,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<Prepared, String> {
    config.validate()?;
    download::check_cancel(cancel)?;
    let target = installation_target(config)?;
    let prepared = Prepared {
        directory: new_stage(&config.install)?,
        config: Box::new(config.clone()),
        handed_off: false,
    };
    let archive = prepared.directory.join("ready.deb");
    let digest = download::download(release, config, &archive, cancel, progress)?;
    verify_package(
        &archive,
        &digest,
        release.size(),
        package_name(config)?,
        &release.version.to_string(),
        config.architecture.debian_name(),
    )?;
    download::check_cancel(cancel)?;
    let helper = prepared.directory.join(&config.install.helper_name);
    fs::copy(&target, &helper).map_err(|e| e.to_string())?;
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    let plan = Plan {
        target,
        parent_pid: std::process::id(),
        parent_start: process_start(std::process::id())
            .ok_or("Cannot identify the application process.")?,
        package: package_name(config)?.into(),
        version: release.version.to_string(),
        architecture: config.architecture.debian_name().into(),
        digest,
        size: release.size(),
        helper_digest: hash(&helper)?,
        restart_args: config.install.restart_args.clone(),
        installed: false,
    };
    write_plan(&prepared.directory, &plan)?;
    download::check_cancel(cancel)?;
    Ok(prepared)
}

fn package_command(archive: &Path) -> Command {
    let mut command = Command::new("/usr/bin/pkexec");
    command
        .args([
            "/usr/bin/apt-get",
            "-o",
            "DPkg::Lock::Timeout=120",
            "--assume-yes",
            "--no-remove",
            "install",
            "--",
        ])
        .arg(archive);
    command.stdin(Stdio::null());
    // pkexec closes inherited descriptors individually on older polkit builds.
    // Desktop runtimes can inherit a billion-descriptor limit; bound it only
    // in the child to avoid a minutes-long CPU loop before the password dialog.
    // SAFETY: only async-signal-safe libc calls run after fork, with stack data.
    unsafe {
        command.pre_exec(|| {
            let mut limits = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limits) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            limits.rlim_cur = limits.rlim_cur.min(4096);
            limits.rlim_max = limits.rlim_max.min(4096);
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limits) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
}

/// Blocking system-package install. Call on a worker, keep the host alive on
/// error/cancel, and close only after success. The restart helper waits for exit.
pub fn handoff(prepared: &mut Prepared) -> Result<(), String> {
    if prepared.handed_off {
        return Err("The update has already been handed off.".into());
    }
    let mut plan = read_plan(&prepared.directory, &prepared.config)?;
    let helper = prepared
        .directory
        .join(&prepared.config.install.helper_name);
    if hash(&helper)? != plan.helper_digest {
        return Err("The restart helper changed after verification.".into());
    }
    if !plan.installed {
        let archive = prepared.directory.join("ready.deb");
        verify_package(
            &archive,
            &plan.digest,
            plan.size,
            &plan.package,
            &plan.version,
            &plan.architecture,
        )?;
        let result = package_command(&archive)
            .output()
            .map_err(|e| format!("Cannot start the system installer: {e}"))?;
        if !result.status.success() {
            let details = String::from_utf8_lossy(&result.stderr);
            return Err(format!(
                "Package installation was cancelled or failed ({}). The current application remains open. {}",
                result.status,
                details.chars().take(4000).collect::<String>()
            ));
        }
        if installed_version(&plan.package)? != plan.version {
            return Err("apt finished without installing the expected version. The current application remains open.".into());
        }
        plan.installed = true;
        write_plan(&prepared.directory, &plan)?;
    }
    Command::new(helper).arg("--apply-update").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().map_err(|e| format!("The package was installed, but restarting failed: {e}. Restart the application manually."))?;
    prepared.handed_off = true;
    Ok(())
}

pub fn apply_update(config: &Config) -> Result<(), String> {
    let helper = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = helper.parent().ok_or("The helper has no directory.")?;
    if helper.file_name() != Some(std::ffi::OsStr::new(&config.install.helper_name)) {
        return Err("The update helper is not staged.".into());
    }
    let plan = read_plan(directory, config)?;
    if !plan.installed
        || hash(&helper)? != plan.helper_digest
        || installed_version(&plan.package)? != plan.version
    {
        return Err("The expected Debian update has not been installed.".into());
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    while process_start(plan.parent_pid).as_deref() == Some(&plan.parent_start) {
        if Instant::now() >= deadline {
            return Err("The application did not exit. Restart it manually.".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Command::new(&plan.target)
        .args(&plan.restart_args)
        .spawn()
        .map_err(|e| format!("Cannot restart the installed application: {e}"))?;
    cleanup(directory, &config.install);
    Ok(())
}

pub fn apply_legacy_update(
    _: &Config,
    _: &Path,
    _: &Path,
    _: &str,
    _: u32,
    _: Option<&Path>,
) -> Result<(), String> {
    Err("The legacy executable-replacement protocol is Windows-only.".into())
}
pub fn cleanup_after_update(directory: PathBuf, options: InstallOptions) {
    cleanup(&directory, &options);
}
#[cfg(feature = "smoke-test")]
pub fn smoke_cli(_: &[std::ffi::OsString], _: &Config) -> Result<bool, String> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_release;

    fn config() -> Config {
        let mut config = Config::debian("example", "sample", "1.0.0", "sample-app").unwrap();
        config.allow_asset_digest = true;
        config
    }
    struct Fixture(PathBuf, InstallOptions);
    impl Fixture {
        fn new() -> Self {
            let options = config().install;
            Self(new_stage(&options).unwrap(), options)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            cleanup(&self.0, &self.1);
        }
    }

    fn release(config: &Config) -> Release {
        let name = config.executable_name(&"1.1.0".parse().unwrap());
        parse_release(&serde_json::json!({
            "tag_name":"v1.1.0", "html_url":format!("{}/releases/tag/v1.1.0",config.repository_url()),
            "draft":false,"prerelease":false,
            "assets":[{"name":name,"size":2048,"digest":format!("sha256:{}","ab".repeat(32)),
                "browser_download_url":format!("{}/releases/download/v1.1.0/{name}",config.repository_url())}]
        }).to_string(),config).unwrap().unwrap()
    }

    #[test]
    fn validates_real_deb_identity_size_and_digest_before_installing() {
        let fixture = Fixture::new();
        let root = fixture.0.join("package");
        fs::create_dir_all(root.join("DEBIAN")).unwrap();
        fs::create_dir_all(root.join("usr/bin")).unwrap();
        let architecture = config().architecture.debian_name();
        fs::write(root.join("DEBIAN/control"),format!("Package: sample-app\nVersion: 1.1.0\nArchitecture: {architecture}\nMaintainer: Tests <test@example.invalid>\nDescription: Isolated updater fixture\n")).unwrap();
        fs::copy("/usr/bin/true", root.join("usr/bin/sample-app")).unwrap();
        let archive = fixture.0.join("ready.deb");
        let result = Command::new("/usr/bin/dpkg-deb")
            .args(["--build", "--root-owner-group"])
            .arg(&root)
            .arg(&archive)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let digest = hash(&archive).unwrap();
        let size = fs::metadata(&archive).unwrap().len();
        verify_package(&archive, &digest, size, "sample-app", "1.1.0", architecture).unwrap();
        for (package, version, arch) in [
            ("another-app", "1.1.0", architecture),
            ("sample-app", "1.2.0", architecture),
            ("sample-app", "1.1.0", "wrong"),
        ] {
            assert!(verify_package(&archive, &digest, size, package, version, arch).is_err());
        }
        assert!(
            verify_package(
                &archive,
                &digest,
                size + 1,
                "sample-app",
                "1.1.0",
                architecture
            )
            .is_err()
        );
        assert!(
            verify_package(
                &archive,
                &"00".repeat(32),
                size,
                "sample-app",
                "1.1.0",
                architecture
            )
            .is_err()
        );
        fs::write(&archive, b"not a debian package").unwrap();
        assert!(
            verify_package(
                &archive,
                &hash(&archive).unwrap(),
                19,
                "sample-app",
                "1.1.0",
                architecture
            )
            .is_err()
        );
    }

    #[test]
    fn cancelled_or_foreign_download_never_starts_network_or_package_management() {
        let config = config();
        let release = release(&config);
        let error = prepare_cancellable(&release, &config, &AtomicBool::new(true), |_, _| {
            panic!("unexpected progress")
        })
        .err()
        .unwrap();
        assert!(error.contains("cancelled"));
        let fixture = Fixture::new();
        let destination = fixture.0.join("download.deb");
        let mut foreign = config.clone();
        foreign.repository = "different".into();
        assert!(
            download::download(
                &release,
                &foreign,
                &destination,
                &AtomicBool::new(false),
                |_, _| {}
            )
            .is_err()
        );
        assert!(!destination.exists());
    }

    #[test]
    fn package_command_keeps_paths_as_arguments_and_never_allows_removals() {
        let path = Path::new("/tmp/private update/ready.deb");
        let command = package_command(path);
        assert_eq!(command.get_program(), "/usr/bin/pkexec");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args[0], "/usr/bin/apt-get");
        assert!(args.contains(&std::ffi::OsStr::new("--no-remove")));
        assert_eq!(args[args.len() - 2], "--");
        assert_eq!(args[args.len() - 1], path.as_os_str());
    }

    #[test]
    fn staging_cleanup_is_private_bounded_and_never_follows_symlinks() {
        let fixture = Fixture::new();
        assert!(valid_stage(&fixture.0, &fixture.1));
        assert_eq!(fs::metadata(&fixture.0).unwrap().mode() & 0o777, 0o700);
        let outside = Fixture::new();
        fs::write(outside.0.join("keep"), b"keep").unwrap();
        std::os::unix::fs::symlink(&outside.0, fixture.0.join("link")).unwrap();
        cleanup(&fixture.0, &fixture.1);
        assert_eq!(fs::read(outside.0.join("keep")).unwrap(), b"keep");
        assert!(!valid_stage(Path::new("/tmp"), &fixture.1));
    }

    #[test]
    fn an_exited_unreaped_parent_does_not_delay_restart() {
        let mut child = Command::new("/usr/bin/true").spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while process_start(child.id()).is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        let identity = process_start(child.id());
        child.wait().unwrap();
        assert!(identity.is_none());
        assert!(process_start(std::process::id()).is_some());
    }

    #[test]
    fn source_builds_cannot_modify_an_installed_package() {
        assert!(installation_target(&config()).is_err());
    }
}
