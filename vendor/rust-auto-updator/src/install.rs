#[cfg(feature = "smoke-test")]
use crate::now;
use crate::{
    Config, InstallOptions,
    download::check_cancel,
    github::Release,
    verify::{hash, verify_executable, verify_pe_subsystem},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    os::windows::{ffi::OsStrExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    ptr,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Storage::FileSystem::ReplaceFileW,
    System::Threading::{
        CREATE_NO_WINDOW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        QueryFullProcessImageNameW, WaitForSingleObject,
    },
};

#[derive(Serialize, Deserialize)]
pub(crate) struct Plan {
    pub(crate) target: PathBuf,
    pub(crate) parent_pid: u32,
    pub(crate) original_sha: String,
    pub(crate) new_sha: String,
    pub(crate) version: String,
    #[serde(default)]
    pub(crate) restart_args: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) rollback_args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) smoke_marker: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) smoke_fail_before_health: Option<bool>,
}

pub struct Prepared {
    directory: PathBuf,
    handed_off: bool,
    options: InstallOptions,
}

impl Prepared {
    /// Refresh the host's current document/session arguments after it has saved
    /// user data and before handoff. Arguments are passed directly to Command.
    pub fn set_restart_args(&mut self, arguments: Vec<String>) -> Result<(), String> {
        if self.handed_off {
            return Err("The update has already been handed off.".into());
        }
        let path = self.directory.join("update.json");
        let mut plan: Plan = serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        plan.restart_args = Some(arguments.clone());
        let bytes = serde_json::to_vec(&plan).map_err(|e| e.to_string())?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
        self.options.restart_args = arguments;
        Ok(())
    }
}
impl Drop for Prepared {
    fn drop(&mut self) {
        if !self.handed_off
            && let Some(parent) = self.directory.parent()
        {
            cleanup(&self.directory, parent, &self.options);
        }
    }
}

pub(crate) fn new_stage(parent: &Path, options: &InstallOptions) -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    new_stage_with_stamp(parent, options, stamp)
}

fn new_stage_with_stamp(
    parent: &Path,
    options: &InstallOptions,
    stamp: u128,
) -> Result<PathBuf, String> {
    static NEXT_STAGE_ID: AtomicU64 = AtomicU64::new(0);
    // Windows clock ticks can be shared by concurrent preparations. A process-
    // local sequence prevents collisions; create_dir still owns the final claim.
    for _ in 0..32 {
        let sequence = NEXT_STAGE_ID.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            "{}{}-{stamp}-{sequence}",
            options.stage_prefix,
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "The executable's folder must be writable to update in place: {error}"
                ));
            }
        }
    }
    Err("Cannot create a unique update staging folder.".into())
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
    check_cancel(cancel)?;
    if config.mode != crate::InstallMode::WindowsPortable {
        return Err("Debian packages can only be installed on Linux.".into());
    }
    let options = &config.install;
    let target = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = target
        .parent()
        .ok_or("The executable folder is unavailable.")?
        .to_path_buf();
    let directory = new_stage(&parent, options)?;
    let result = (|| {
        let path = directory.join(&options.helper_name);
        let expected = crate::download::download(release, config, &path, cancel, progress)?;
        verify_executable(
            &path,
            &expected,
            release.executable.size,
            config.architecture,
        )?;
        if config.expected_subsystem.is_some() {
            verify_pe_subsystem(&path, config.architecture, config.expected_subsystem)?;
        }
        check_cancel(cancel)?;
        let plan = Plan {
            original_sha: hash(&target)?,
            target,
            parent_pid: std::process::id(),
            new_sha: expected,
            version: release.version.to_string(),
            restart_args: Some(options.restart_args.clone()),
            rollback_args: Some(options.rollback_args.clone()),
            smoke_marker: None,
            smoke_fail_before_health: None,
        };
        fs::write(
            directory.join("update.json"),
            serde_json::to_vec(&plan).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        Ok(Prepared {
            directory: directory.clone(),
            handed_off: false,
            options: options.clone(),
        })
    })();
    if result.is_err() {
        cleanup(&directory, &parent, options);
    }
    result
}

pub fn handoff(prepared: &mut Prepared) -> Result<(), String> {
    if prepared.handed_off {
        return Err("The update has already been handed off.".into());
    }
    Command::new(prepared.directory.join(&prepared.options.helper_name))
        .arg("--apply-update")
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("Could not start the update helper: {e}"))?;
    prepared.handed_off = true;
    Ok(())
}

/// Translate an older application's new-executable helper protocol into the
/// shared, verified installation plan. The host adapter must first validate its
/// historical staging root, expected helper name and command-line shape.
/// A ready marker is created only after the modern helper has started.
pub fn apply_legacy_update(
    config: &Config,
    target: &Path,
    staged: &Path,
    expected: &str,
    parent_pid: u32,
    ready_marker: Option<&Path>,
) -> Result<(), String> {
    config.validate()?;
    let helper = std::env::current_exe().map_err(|e| e.to_string())?;
    if parent_pid == 0
        || parent_pid == std::process::id()
        || !target.is_absolute()
        || !staged.is_absolute()
        || target
            .extension()
            .is_none_or(|value| !value.eq_ignore_ascii_case("exe"))
        || expected.len() != 64
        || !expected.bytes().all(|value| value.is_ascii_hexdigit())
        || ready_marker.is_some_and(|marker| marker != staged.with_extension("ready"))
    {
        return Err("Invalid legacy update protocol.".into());
    }
    let target = fs::canonicalize(target).map_err(|e| e.to_string())?;
    let staged = fs::canonicalize(staged).map_err(|e| e.to_string())?;
    let helper = fs::canonicalize(helper).map_err(|e| e.to_string())?;
    if target == staged || target == helper || staged.parent() != helper.parent() {
        return Err("Invalid legacy update paths.".into());
    }
    validate_parent_identity(parent_pid, &target)?;
    let expected = expected.to_ascii_lowercase();
    let size = fs::metadata(&staged).map_err(|e| e.to_string())?.len();
    if size < 1024 || size > config.max_executable_size || hash(&helper)? != expected {
        return Err("The legacy helper failed verification.".into());
    }
    verify_executable(&staged, &expected, size, config.architecture)?;
    if config.expected_subsystem.is_some() {
        verify_pe_subsystem(&staged, config.architecture, config.expected_subsystem)?;
    }
    let parent = target
        .parent()
        .ok_or("The executable folder is unavailable.")?;
    let directory = new_stage(parent, &config.install)?;
    let mut prepared = Prepared {
        directory: directory.clone(),
        handed_off: false,
        options: config.install.clone(),
    };
    let destination = directory.join(&config.install.helper_name);
    fs::copy(&staged, &destination).map_err(|e| e.to_string())?;
    verify_executable(&destination, &expected, size, config.architecture)?;
    let plan = Plan {
        original_sha: hash(&target)?,
        target,
        parent_pid,
        new_sha: expected,
        version: config.current_version.to_string(),
        restart_args: Some(config.install.restart_args.clone()),
        rollback_args: Some(config.install.rollback_args.clone()),
        smoke_marker: None,
        smoke_fail_before_health: None,
    };
    fs::write(
        directory.join("update.json"),
        serde_json::to_vec(&plan).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    handoff(&mut prepared)?;
    if let Some(marker) = ready_marker {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(marker)
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Resolve the installed target for product-specific failure reporting from a
/// modern helper. Never derive a portable data directory from the helper's own
/// staging folder. Invalid or unrelated helper contexts yield no trusted path.
pub fn installation_target(config: &Config) -> Result<PathBuf, String> {
    config.validate()?;
    let helper = std::env::current_exe().map_err(|error| error.to_string())?;
    let directory = helper.parent().ok_or("Update staging folder missing.")?;
    if helper
        .file_name()
        .is_none_or(|name| !name.eq_ignore_ascii_case(&config.install.helper_name))
    {
        return Err("This executable is not the configured update helper.".into());
    }
    let path = directory.join("update.json");
    if fs::metadata(&path)
        .map_err(|error| error.to_string())?
        .len()
        > 64 * 1024
    {
        return Err("The update plan exceeds the size limit.".into());
    }
    let plan: Plan = serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let parent = plan.target.parent().ok_or("Invalid update target.")?;
    validate_stage(directory, parent, &config.install)?;
    if !plan.target.is_absolute()
        || plan
            .target
            .extension()
            .is_none_or(|ext| !ext.eq_ignore_ascii_case("exe"))
        || plan.version != config.current_version.to_string()
        || hash(&helper)? != plan.new_sha
    {
        return Err("The update plan does not match this executable.".into());
    }
    Ok(plan.target)
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn validate_parent_identity(parent_pid: u32, target: &Path) -> Result<(), String> {
    let process = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            parent_pid,
        )
    };
    if process.is_null() {
        return if unsafe { GetLastError() } == ERROR_INVALID_PARAMETER {
            Ok(())
        } else {
            Err("Could not verify the legacy update parent process.".into())
        };
    }
    let result = (|| {
        if unsafe { WaitForSingleObject(process, 0) } == WAIT_OBJECT_0 {
            return Ok(());
        }
        let mut path = vec![0u16; 32768];
        let mut length = path.len() as u32;
        if unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) } == 0 {
            return Err("Could not verify the legacy update parent executable.".into());
        }
        let path = PathBuf::from(
            String::from_utf16(&path[..length as usize]).map_err(|error| error.to_string())?,
        );
        if fs::canonicalize(path).map_err(|error| error.to_string())?
            != fs::canonicalize(target).map_err(|error| error.to_string())?
        {
            return Err("The legacy update process does not match the target executable.".into());
        }
        Ok(())
    })();
    unsafe {
        CloseHandle(process);
    }
    result
}

pub(crate) fn replace(target: &Path, ready: &Path, backup: &Path) -> Result<(), String> {
    let (target, ready, backup) = (wide_path(target), wide_path(ready), wide_path(backup));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        // SAFETY: All paths are validated, same-volume, NUL-terminated and live across this call.
        if unsafe {
            ReplaceFileW(
                target.as_ptr(),
                ready.as_ptr(),
                backup.as_ptr(),
                0,
                ptr::null(),
                ptr::null(),
            )
        } != 0
        {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if Instant::now() >= deadline {
            return Err(format!("Could not replace the executable: {error}"));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

pub(crate) fn wait_for_parent(plan: &Plan) -> Result<(), String> {
    let access = PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION;
    #[cfg(feature = "smoke-test")]
    let access =
        if plan.smoke_marker.is_some() && std::env::var_os("RUST_UPDATOR_QUERY_DENIED").is_some() {
            // Exercise a real Windows query failure while retaining the ability to wait.
            PROCESS_SYNCHRONIZE
        } else {
            access
        };
    // SAFETY: Open only the specified application process for querying and waiting, never termination.
    let process = unsafe { OpenProcess(access, 0, plan.parent_pid) };
    if process.is_null() {
        if unsafe { GetLastError() } == ERROR_INVALID_PARAMETER {
            return Ok(());
        }
        return Err("The application exited before the update helper could attach. Please retry the update.".into());
    }
    let result = (|| {
        // A terminated process may still have a valid handle but no queryable image name.
        if unsafe { WaitForSingleObject(process, 0) } == WAIT_OBJECT_0 {
            return Ok(());
        }
        let mut path = vec![0u16; 32768];
        let mut length = path.len() as u32;
        if unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) } == 0 {
            let error = std::io::Error::last_os_error();
            #[cfg(feature = "smoke-test")]
            if plan.smoke_marker.is_some()
                && std::env::var_os("RUST_UPDATOR_QUERY_DENIED").is_some()
            {
                assert_eq!(
                    error.raw_os_error(),
                    Some(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as i32)
                );
                eprintln!("PASS native process query returned access denied");
            }
            // Windows may stop exposing the image name before process teardown signals
            // the handle. Require actual termination, then verify the installed hash
            // in apply_update before touching any files.
            return wait_for_exit(process, 45000).map_err(|wait_error| {
                format!("Could not verify the application being updated: {error}. {wait_error}")
            });
        }
        let path =
            PathBuf::from(String::from_utf16(&path[..length as usize]).map_err(|e| e.to_string())?);
        if fs::canonicalize(path).map_err(|e| e.to_string())?
            != fs::canonicalize(&plan.target).map_err(|e| e.to_string())?
        {
            return Err("The update process does not match the target executable.".into());
        }
        wait_for_exit(process, 45000)
    })();
    unsafe {
        CloseHandle(process);
    }
    result
}

pub(crate) fn wait_for_exit(process: HANDLE, timeout_ms: u32) -> Result<(), String> {
    // SAFETY: The caller owns this process handle until the wait returns.
    match unsafe { WaitForSingleObject(process, timeout_ms) } {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err("The application did not exit in time. Nothing was replaced.".into()),
        _ => Err(format!(
            "Could not wait for the application to exit: {}",
            std::io::Error::last_os_error()
        )),
    }
}

pub fn apply_update(config: &Config) -> Result<(), String> {
    config.validate()?;
    let options = &config.install;
    let helper = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = helper.parent().ok_or("Update staging folder missing.")?;
    let plan: Plan = serde_json::from_slice(
        &fs::read(directory.join("update.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let parent = plan.target.parent().ok_or("Invalid update target.")?;
    validate_stage(directory, parent, options)?;
    if plan
        .target
        .extension()
        .is_none_or(|e| !e.eq_ignore_ascii_case("exe"))
        || plan.version != config.current_version.to_string()
        || ((plan.smoke_marker.is_some() || plan.smoke_fail_before_health.is_some())
            && !cfg!(feature = "smoke-test"))
    {
        return Err("The update plan does not match this executable.".into());
    }
    if hash(&helper)? != plan.new_sha {
        return Err("The update helper failed checksum verification.".into());
    }
    wait_for_parent(&plan)?;
    if hash(&plan.target)? != plan.original_sha {
        return Err(
            "The installed executable changed during the download. It was not overwritten.".into(),
        );
    }
    let ready = directory.join("ready.exe");
    let backup = directory.join("previous.exe");
    fs::copy(&helper, &ready).map_err(|e| e.to_string())?;
    fs::OpenOptions::new()
        .write(true)
        .open(&ready)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    finish_replacement(
        &plan.target,
        &ready,
        &backup,
        directory,
        || {
            let mut command = Command::new(&plan.target);
            if plan.smoke_fail_before_health == Some(true) {
                command.arg("--updater-smoke-fail");
            } else if let Some(marker) = &plan.smoke_marker {
                command.arg("--updater-smoke-new").arg(marker);
            } else {
                command.args(plan.restart_args.as_ref().unwrap_or(&options.restart_args));
                command.arg("--cleanup-update").arg(directory);
            }
            command.creation_flags(CREATE_NO_WINDOW);
            let mut child = command.spawn().map_err(|error| error.to_string())?;
            let health_timeout = if plan.smoke_fail_before_health == Some(true) {
                Some(Duration::from_secs(2))
            } else {
                options.startup_health_timeout
            };
            if let Some(timeout) = health_timeout {
                wait_for_startup_health(&mut child, directory, &plan.new_sha, timeout)?;
            } else if plan.smoke_marker.is_some()
                && !child.wait().map_err(|e| e.to_string())?.success()
            {
                return Err("Updated test child failed.".into());
            }
            Ok(())
        },
        || {
            let mut previous = Command::new(&plan.target);
            previous.args(
                plan.rollback_args
                    .as_ref()
                    .unwrap_or(&options.rollback_args),
            );
            let _ = previous.creation_flags(CREATE_NO_WINDOW).spawn();
        },
    )
}

pub(crate) fn wait_for_startup_health(
    child: &mut std::process::Child,
    directory: &Path,
    expected: &str,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        // A successful first frame remains healthy if the user closes the app
        // before our next poll. Acknowledgement therefore precedes exit checks.
        if fs::read(directory.join("healthy")).is_ok_and(|bytes| bytes == expected.as_bytes()) {
            if let Err(error) = fs::write(directory.join("health-accepted"), expected) {
                let _ = fs::write(
                    directory.join("health-timeout.txt"),
                    format!(
                        "The application acknowledged startup, but acknowledgement could not be accepted ({error}). The process and previous executable were preserved."
                    ),
                );
            }
            return Ok(());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                // The child can acknowledge between the read above and its exit.
                if fs::read(directory.join("healthy"))
                    .is_ok_and(|bytes| bytes == expected.as_bytes())
                {
                    continue;
                }
                return Err(format!(
                    "The updated application exited before startup acknowledgement ({status})."
                ));
            }
            Ok(None) => {}
            Err(error) => {
                let _ = fs::write(
                    directory.join("health-timeout.txt"),
                    format!(
                        "Could not establish whether the updated application exited ({error}). The process and previous executable were preserved."
                    ),
                );
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            // A live process may own user data. Preserve it and the rollback copy;
            // never terminate it or replace its executable merely for a timeout.
            let _ = fs::write(directory.join("health-timeout.txt"), b"The updated application did not acknowledge startup in time. It was left running and the previous executable was preserved.");
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

pub(crate) fn finish_replacement(
    target: &Path,
    ready: &Path,
    backup: &Path,
    directory: &Path,
    restart: impl FnOnce() -> Result<(), String>,
    rollback_restart: impl FnOnce(),
) -> Result<(), String> {
    if let Err(error) = replace(target, ready, backup) {
        if !target.exists() && backup.exists() {
            let _ = fs::rename(backup, target);
        }
        return Err(error);
    }
    match restart() {
        Ok(()) => Ok(()),
        Err(error) => {
            let restore = directory.join("restore.exe");
            fs::copy(backup, &restore).map_err(|e| e.to_string())?;
            replace(target, &restore, &directory.join("failed.exe"))?;
            rollback_restart();
            Err(format!(
                "The update could not restart ({error}). The previous version was restored."
            ))
        }
    }
}

pub(crate) fn validate_stage(
    directory: &Path,
    parent: &Path,
    options: &InstallOptions,
) -> Result<(), String> {
    let directory = fs::canonicalize(directory).map_err(|e| e.to_string())?;
    if !directory
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with(&options.stage_prefix))
        || directory.parent()
            != Some(
                fs::canonicalize(parent)
                    .map_err(|e| e.to_string())?
                    .as_path(),
            )
    {
        return Err("The update staging folder is outside the executable's folder.".into());
    }
    Ok(())
}

pub(crate) fn cleanup(directory: &Path, parent: &Path, options: &InstallOptions) {
    if validate_stage(directory, parent, options).is_err() {
        return;
    }
    // Only known updater files are removed. Never recursively delete a supplied directory.
    for name in [
        options.helper_name.as_str(),
        "ready.exe",
        "previous.exe",
        "restore.exe",
        "failed.exe",
        "update.json",
        "healthy",
        "health-accepted",
    ] {
        let _ = fs::remove_file(directory.join(name));
    }
    let _ = fs::remove_dir(directory);
}

pub fn cleanup_after_update(directory: PathBuf, options: InstallOptions) {
    let Ok(target) = std::env::current_exe() else {
        return;
    };
    let Some(parent) = target.parent().map(Path::to_path_buf) else {
        return;
    };
    let _ = thread::Builder::new()
        .name("update-cleanup".into())
        .spawn(move || {
            if validate_stage(&directory, &parent, &options).is_err() {
                return;
            }
            // Validation and whole-executable hashing stay off the UI thread.
            let health_sha = if options.startup_health_timeout.is_some() {
                let Ok(bytes) = fs::read(directory.join("update.json")) else {
                    return;
                };
                let Ok(plan) = serde_json::from_slice::<Plan>(&bytes) else {
                    return;
                };
                if fs::canonicalize(&plan.target).ok() != fs::canonicalize(&target).ok()
                    || hash(&target).ok().as_deref() != Some(plan.new_sha.as_str())
                {
                    return;
                }
                if fs::write(directory.join("healthy"), &plan.new_sha).is_err() {
                    return;
                }
                Some(plan.new_sha)
            } else {
                None
            };
            if let Some(expected) = health_sha {
                let mut accepted = false;
                for _ in 0..50 {
                    if fs::read(directory.join("health-accepted"))
                        .is_ok_and(|bytes| bytes == expected.as_bytes())
                    {
                        accepted = true;
                        break;
                    }
                    if directory.join("health-timeout.txt").exists() {
                        return;
                    }
                    thread::sleep(Duration::from_millis(200));
                }
                if !accepted {
                    return;
                }
            }
            for _ in 0..50 {
                cleanup(&directory, &parent, &options);
                if !directory.exists() {
                    break;
                }
                thread::sleep(Duration::from_millis(200));
            }
        });
}

#[cfg(feature = "smoke-test")]
pub fn smoke_cli(arguments: &[std::ffi::OsString], config: &Config) -> Result<bool, String> {
    if arguments.len() == 1 && arguments[0] == "--updater-smoke-fail" {
        return Err("Injected startup failure before health acknowledgement.".into());
    }
    if arguments.len() == 1 && arguments[0] == "--updater-smoke-old" {
        thread::sleep(Duration::from_millis(3000));
        return Ok(true);
    }
    if arguments.len() == 2
        && (arguments[0] == "--updater-smoke-new" || arguments[0] == "--updater-smoke-rollback")
    {
        let marker = PathBuf::from(&arguments[1]);
        if marker.file_name().is_none_or(|n| n != "restarted.txt")
            || marker
                .parent()
                .and_then(Path::file_name)
                .is_none_or(|n| !n.to_string_lossy().starts_with("updater-smoke-"))
        {
            return Err("Invalid updater test marker.".into());
        }
        let contents: &[u8] = if arguments[0] == "--updater-smoke-rollback" {
            b"rolled back"
        } else {
            b"restarted"
        };
        fs::write(marker, contents).map_err(|e| e.to_string())?;
        return Ok(true);
    }
    if arguments.len() != 1 || arguments[0] != "--test-updater" {
        return Ok(false);
    }
    smoke_replacement(config, false, false)?;
    smoke_replacement(config, true, false)?;
    smoke_replacement(config, false, true)?;
    Ok(true)
}

#[cfg(feature = "smoke-test")]
fn smoke_replacement(config: &Config, query_denied: bool, fail_health: bool) -> Result<(), String> {
    let options = &config.install;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .join("target")
        .join(format!("updater-smoke-{}-{}", std::process::id(), now()));
    fs::create_dir(&parent).map_err(|e| e.to_string())?;
    let target = parent.join(&options.helper_name);
    fs::copy(&executable, &target).map_err(|e| e.to_string())?;
    fs::OpenOptions::new()
        .append(true)
        .open(&target)
        .and_then(|mut f| f.write_all(b"old-version-fixture"))
        .map_err(|e| e.to_string())?;
    let directory = new_stage(&parent, options)?;
    fs::copy(&executable, directory.join(&options.helper_name)).map_err(|e| e.to_string())?;
    let marker = parent.join("restarted.txt");
    let original_sha = hash(&target)?;
    let new_sha = hash(&executable)?;
    if original_sha == new_sha {
        return Err("Update test fixture must differ.".into());
    }
    let mut old = Command::new(&target)
        .arg("--updater-smoke-old")
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| e.to_string())?;
    let plan = Plan {
        target: target.clone(),
        parent_pid: old.id(),
        original_sha: original_sha.clone(),
        new_sha: new_sha.clone(),
        version: config.current_version.to_string(),
        restart_args: Some(Vec::new()),
        rollback_args: Some(if fail_health {
            vec![
                "--updater-smoke-rollback".into(),
                marker.to_string_lossy().into_owned(),
            ]
        } else {
            Vec::new()
        }),
        smoke_marker: Some(marker.clone()),
        smoke_fail_before_health: fail_health.then_some(true),
    };
    fs::write(
        directory.join("update.json"),
        serde_json::to_vec(&plan).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let mut helper = Command::new(directory.join(&options.helper_name));
    helper
        .arg("--apply-update")
        .creation_flags(CREATE_NO_WINDOW);
    if query_denied {
        helper.env("RUST_UPDATOR_QUERY_DENIED", "1");
    }
    let status = helper.status().map_err(|e| e.to_string())?;
    let _ = old.wait();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if status.success() == fail_health
        || hash(&target)?
            != if fail_health {
                original_sha.clone()
            } else {
                new_sha
            }
        || hash(&directory.join("previous.exe"))? != original_sha
        || fs::read(&marker).map_err(|e| e.to_string())?
            != if fail_health {
                b"rolled back".as_slice()
            } else {
                b"restarted".as_slice()
            }
    {
        return Err("Native update replacement/restart regression failed.".into());
    }
    cleanup(&directory, &parent, options);
    fs::remove_file(&target).map_err(|e| e.to_string())?;
    fs::remove_file(marker).map_err(|e| e.to_string())?;
    fs::remove_dir(parent).map_err(|e| e.to_string())?;
    println!(
        "PASS wait, verified replacement, backup, restart and cleanup (query denied: {query_denied}, startup failure rollback: {fail_health})"
    );
    Ok(())
}

#[cfg(test)]
mod stage_directory_tests {
    use super::*;

    #[test]
    fn preparations_in_the_same_clock_tick_get_independent_folders() {
        let options = InstallOptions::default();
        let first = new_stage_with_stamp(&std::env::temp_dir(), &options, 0).unwrap();
        fs::write(first.join("existing.txt"), b"keep first preparation").unwrap();
        let second = new_stage_with_stamp(&std::env::temp_dir(), &options, 0).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            fs::read(first.join("existing.txt")).unwrap(),
            b"keep first preparation"
        );
        fs::remove_file(first.join("existing.txt")).unwrap();
        fs::remove_dir(first).unwrap();
        fs::remove_dir(second).unwrap();
    }
}
