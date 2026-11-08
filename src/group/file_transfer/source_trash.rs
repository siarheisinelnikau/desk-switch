//! Recoverable source cleanup after a verified, explicitly granted Move.
//!
//! Only an owned quarantine capability is filesystem authority here. The
//! original path is recorded for restoration, never opened or deleted. A
//! platform failure leaves the payload or its verified Trash location intact.
#[cfg(any(target_os = "linux", test))]
use anyhow::Context;
use anyhow::{Result, bail, ensure};
#[cfg(any(target_os = "linux", windows, test))]
use cap_fs_ext::DirExt;
#[cfg(any(target_os = "linux", test))]
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
#[cfg(any(target_os = "linux", test))]
use cap_std::fs::OpenOptions;
use cap_std::fs::{Dir, Metadata};
#[cfg(any(target_os = "linux", windows, test))]
use cap_std::{ambient_authority, fs::DirBuilder};
use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "linux", test))]
use std::ffi::OsStr;
#[cfg(any(target_os = "linux", windows, test))]
use std::path::Component;
use std::path::{Path, PathBuf};

#[cfg(windows)]
mod windows;

pub type Identity = (u64, u64);
pub(super) const FORMAT: u16 = 1;
pub(super) const PAYLOAD: &str = "payload";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TrashBackend {
    Freedesktop,
    WindowsRecycle,
}

/// Persist this plan in the source Move journal before calling `commit`.
/// Windows anchors its source volume root; the Shell selects the actual bin
/// and item name, recorded only after the returned object is verified.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrashPlan {
    pub format: u16,
    pub backend: TrashBackend,
    pub token: String,
    pub original: PathBuf,
    pub source_identity: Identity,
    pub quarantine_identity: Identity,
    pub target: PathBuf,
    pub info: Option<PathBuf>,
    pub target_parent_identity: Identity,
    pub info_parent_identity: Option<Identity>,
    pub deletion_date: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrashReceipt {
    pub plan: TrashPlan,
    pub item: PathBuf,
    pub item_identity: Identity,
}

// Receipts are short-lived status values. Boxing would add an allocation to
// every inspection while the largest variant is only a few hundred bytes.
#[allow(clippy::large_enum_variant)]
// Recycling a native Move source is only driven by the Windows and Linux paste
// backends; on macOS the receipt variants are never constructed.
#[cfg_attr(target_os = "macos", allow(dead_code))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrashStatus {
    Pending,
    Committed(TrashReceipt),
    NeedsRecovery { paths: Vec<PathBuf>, detail: String },
}

#[cfg(test)]
thread_local! {
    static TEST_TRASH: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, windows))]
#[derive(Clone)]
pub(super) struct HostedNativePermit {
    fixture: PathBuf,
}

#[cfg(all(test, windows))]
thread_local! {
    static HOSTED_NATIVE: std::cell::RefCell<Option<HostedNativePermit>> = const { std::cell::RefCell::new(None) };
}

/// Only the ignored hosted-native tests may opt out of fake Trash. The scope
/// is a fresh synthetic runner fixture, never an arbitrary user directory.
#[cfg(all(test, windows))]
pub(crate) fn with_hosted_native_trash<T>(fixture: &Path, action: impl FnOnce() -> T) -> Result<T> {
    use std::os::windows::fs::MetadataExt;
    ensure!(
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && std::env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted")
            && std::env::var("DESK_SWITCH_NATIVE_CLIPBOARD_TEST").as_deref() == Ok("1"),
        "Native Trash tests require an explicitly authorized disposable hosted runner"
    );
    ensure!(
        TEST_TRASH.with(|slot| slot.borrow().is_none()),
        "A fake Trash fixture cannot be converted into a native test scope"
    );
    let runner = PathBuf::from(std::env::var_os("RUNNER_TEMP").context("RUNNER_TEMP is missing")?);
    ensure!(
        runner.is_absolute() && fixture.is_absolute(),
        "Native Trash fixture paths must be absolute"
    );
    let runner = runner.canonicalize()?;
    let metadata = std::fs::symlink_metadata(fixture)?;
    ensure!(
        metadata.is_dir() && metadata.file_attributes() & 0x400 == 0,
        "The native Trash fixture must be a real directory"
    );
    let fixture = fixture.canonicalize()?;
    let name = fixture
        .file_name()
        .and_then(|name| name.to_str())
        .context("Invalid native fixture")?;
    ensure!(
        fixture.parent() == Some(runner.as_path())
            && name.starts_with("desk-switch-")
            && name.len() <= 128
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "The native Trash fixture is outside its unique runner scope"
    );
    Ok(with_hosted_native_permit(
        HostedNativePermit { fixture },
        action,
    ))
}

#[cfg(all(test, windows))]
pub(super) fn hosted_native_permit() -> Result<HostedNativePermit> {
    HOSTED_NATIVE
        .with(|slot| slot.borrow().clone())
        .context("Real system Trash is disabled outside an explicitly scoped native test")
}

/// A worker may inherit only a permit already constructed by the hosted guard.
#[cfg(all(test, windows))]
pub(super) fn with_hosted_native_permit<T>(
    permit: HostedNativePermit,
    action: impl FnOnce() -> T,
) -> T {
    struct Restore(Option<HostedNativePermit>);
    impl Drop for Restore {
        fn drop(&mut self) {
            HOSTED_NATIVE.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let previous = HOSTED_NATIVE.with(|slot| slot.replace(Some(permit)));
    let _restore = Restore(previous);
    action()
}

#[cfg(all(test, windows))]
pub(super) fn require_hosted_native_scope(quarantine: &Dir) -> Result<()> {
    let permit = hosted_native_permit()?;
    windows::require_test_quarantine_scope(quarantine, &permit.fixture)
}

/// Tests opt into an isolated fake Trash explicitly. Ordinary unit tests can
/// never reach the user's Trash or the Windows Shell backend.
#[cfg(test)]
pub(crate) fn with_test_trash<T>(directory: &Path, action: impl FnOnce() -> T) -> T {
    struct Restore(Option<PathBuf>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_TRASH.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let previous = TEST_TRASH.with(|slot| slot.replace(Some(directory.to_path_buf())));
    let _restore = Restore(previous);
    action()
}

pub fn prepare(
    quarantine: &Dir,
    original: &Path,
    expected: Identity,
    token: &str,
) -> Result<TrashPlan> {
    validate_token(token)?;
    ensure!(
        original.is_absolute(),
        "The original file location is invalid"
    );
    require_private_dir(quarantine)?;
    ensure!(
        identity(&quarantine.symlink_metadata(PAYLOAD)?)? == expected,
        "The retained source changed before Trash preparation"
    );
    #[cfg(all(test, windows))]
    if HOSTED_NATIVE.with(|slot| slot.borrow().is_some()) {
        require_hosted_native_scope(quarantine)?;
        return windows::prepare(quarantine, original, expected, token);
    }
    #[cfg(test)]
    {
        let root = TEST_TRASH
            .with(|slot| slot.borrow().clone())
            .context("Real system Trash is disabled in unit tests")?;
        prepare_freedesktop(quarantine, original, expected, token, &root)
    }
    #[cfg(all(not(test), target_os = "linux"))]
    {
        let root = dirs::data_dir()
            .context("The user's Trash location is unavailable")?
            .join("Trash");
        prepare_freedesktop(quarantine, original, expected, token, &root)
    }
    #[cfg(all(not(test), windows))]
    {
        windows::prepare(quarantine, original, expected, token)
    }
    #[cfg(all(not(test), not(any(target_os = "linux", windows))))]
    bail!("Recoverable source cleanup is unavailable on this operating system")
}

pub fn commit(quarantine: &Dir, plan: &TrashPlan) -> Result<TrashReceipt> {
    validate_plan(quarantine, plan, false)?;
    match plan.backend {
        TrashBackend::Freedesktop => {
            #[cfg(any(target_os = "linux", test))]
            {
                commit_freedesktop(quarantine, plan)
            }
            #[cfg(not(any(target_os = "linux", test)))]
            {
                bail!("This Trash plan belongs to another operating system")
            }
        }
        TrashBackend::WindowsRecycle => {
            #[cfg(windows)]
            {
                windows::commit(quarantine, plan)
            }
            #[cfg(not(windows))]
            {
                bail!("Native Recycle Bin operations are unavailable in this context")
            }
        }
    }
}

/// Read-only reconciliation. This never repeats a source rename or asks the OS
/// to recycle another item after a crash or a duplicated destination receipt.
pub fn inspect(quarantine: &Dir, plan: &TrashPlan) -> Result<TrashStatus> {
    validate_plan(quarantine, plan, false)?;
    match plan.backend {
        TrashBackend::Freedesktop => {
            #[cfg(any(target_os = "linux", test))]
            {
                inspect_freedesktop(quarantine, plan)
            }
            #[cfg(not(any(target_os = "linux", test)))]
            {
                bail!("This Trash plan belongs to another operating system")
            }
        }
        TrashBackend::WindowsRecycle => {
            #[cfg(windows)]
            {
                windows::inspect(quarantine, plan)
            }
            #[cfg(not(windows))]
            {
                bail!("Native Recycle Bin operations are unavailable in this context")
            }
        }
    }
}

fn validate_token(token: &str) -> Result<()> {
    ensure!(
        token.len() == 32
            && token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Invalid source Trash operation identity"
    );
    Ok(())
}

pub(super) fn identity(metadata: &Metadata) -> Result<Identity> {
    ensure!(
        !metadata.file_type().is_symlink() && (metadata.is_dir() || metadata.is_file()),
        "The retained source contains an unsupported filesystem object"
    );
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "Source Trash must not follow a reparse point"
        );
    }
    Ok((
        cap_fs_ext::MetadataExt::dev(metadata),
        cap_fs_ext::MetadataExt::ino(metadata),
    ))
}

pub(super) fn require_private_dir(directory: &Dir) -> Result<()> {
    let metadata = directory.dir_metadata()?;
    identity(&metadata)?;
    ensure!(metadata.is_dir(), "Source Trash storage is not a folder");
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0,
            "Source Trash storage must be private to this user"
        );
    }
    #[cfg(all(windows, not(test)))]
    windows::verify_private_directory(directory)?;
    #[cfg(all(windows, test))]
    if HOSTED_NATIVE.with(|slot| slot.borrow().is_some()) {
        require_hosted_native_scope(directory)?;
        windows::verify_private_directory(directory)?;
    }
    Ok(())
}

/// Initialize only a fresh, exclusively created quarantine child. Existing
/// recovery directories and source parents must use verification alone.
pub(super) fn protect_new_private_dir(directory: &Dir) -> Result<()> {
    #[cfg(all(windows, not(test)))]
    windows::protect_new_private_directory(directory)?;
    #[cfg(all(windows, test))]
    if HOSTED_NATIVE.with(|slot| slot.borrow().is_some()) {
        require_hosted_native_scope(directory)?;
        windows::protect_new_private_directory(directory)?;
    }
    require_private_dir(directory)
}

pub(super) fn validate_plan(
    quarantine: &Dir,
    plan: &TrashPlan,
    require_payload: bool,
) -> Result<()> {
    validate_token(&plan.token)?;
    ensure!(
        plan.format == FORMAT
            && plan.original.is_absolute()
            && plan.target.is_absolute()
            && plan.info.as_ref().is_none_or(|path| path.is_absolute()),
        "The saved source Trash plan is invalid"
    );
    require_private_dir(quarantine)?;
    ensure!(
        identity(&quarantine.dir_metadata()?)? == plan.quarantine_identity,
        "The retained source recovery folder was replaced"
    );
    #[cfg(test)]
    {
        #[cfg(windows)]
        let native = HOSTED_NATIVE.with(|slot| slot.borrow().is_some());
        #[cfg(not(windows))]
        let native = false;
        if native {
            #[cfg(windows)]
            require_hosted_native_scope(quarantine)?;
            ensure!(
                plan.backend == TrashBackend::WindowsRecycle,
                "Wrong native test Trash backend"
            );
        } else {
            let root = TEST_TRASH
                .with(|slot| slot.borrow().clone())
                .context("Real system Trash is disabled in unit tests")?;
            ensure!(
                plan.backend == TrashBackend::Freedesktop
                    && plan.target.parent().and_then(Path::parent) == Some(root.as_path()),
                "The test Trash plan is outside its explicit fixture"
            );
        }
    }
    match quarantine.symlink_metadata(PAYLOAD) {
        Ok(metadata) => ensure!(
            identity(&metadata)? == plan.source_identity,
            "The retained source payload was replaced"
        ),
        Err(error) if !require_payload && error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// Walk every directory component through no-follow capabilities. This is
/// used only for recorded Trash storage, never the original source path.
#[cfg(any(target_os = "linux", windows, test))]
pub(super) fn open_absolute_dir(path: &Path, create: bool) -> Result<Dir> {
    ensure!(path.is_absolute(), "The Trash storage path is not absolute");
    let mut root = PathBuf::new();
    let mut names = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                #[cfg(windows)]
                ensure!(
                    matches!(
                        prefix.kind(),
                        std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                    ),
                    "Network and device Trash paths are unsupported"
                );
                root.push(prefix.as_os_str());
            }
            Component::RootDir => root.push(component.as_os_str()),
            Component::Normal(name) => names.push(name.to_os_string()),
            _ => bail!("Relative Trash path components are unsupported"),
        }
    }
    let mut directory = Dir::open_ambient_dir(root, ambient_authority())?;
    for name in names {
        if create {
            let builder = {
                #[cfg(unix)]
                {
                    use cap_std::fs::DirBuilderExt;
                    let mut builder = DirBuilder::new();
                    builder.mode(0o700);
                    builder
                }
                #[cfg(not(unix))]
                {
                    DirBuilder::new()
                }
            };
            match directory.create_dir_with(&name, &builder) {
                Ok(()) => sync_directory(&directory)?,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        directory = directory.open_dir_nofollow(&name)?;
        ensure!(
            directory.dir_metadata()?.is_dir(),
            "The Trash path is not a folder"
        );
        identity(&directory.dir_metadata()?)?;
    }
    Ok(directory)
}

#[cfg(any(target_os = "linux", windows, test))]
fn sync_directory(directory: &Dir) -> Result<()> {
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .follow(FollowSymlinks::No)
            .custom_flags(libc::O_DIRECTORY);
        directory.open_with(".", &options)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
fn prepare_freedesktop(
    quarantine: &Dir,
    original: &Path,
    expected: Identity,
    token: &str,
    root: &Path,
) -> Result<TrashPlan> {
    let trash = open_absolute_dir(root, true)?;
    require_private_dir(&trash)?;
    let files_path = root.join("files");
    let info_path = root.join("info");
    let files = open_absolute_dir(&files_path, true)?;
    let info = open_absolute_dir(&info_path, true)?;
    require_private_dir(&files)?;
    require_private_dir(&info)?;
    let files_identity = identity(&files.dir_metadata()?)?;
    ensure!(
        files_identity.0 == expected.0,
        "This source volume has no supported same-volume Trash; its recovery copy was retained"
    );
    let name = format!("desk-switch-{token}");
    let plan = TrashPlan {
        format: FORMAT,
        backend: TrashBackend::Freedesktop,
        token: token.into(),
        original: original.to_path_buf(),
        source_identity: expected,
        quarantine_identity: identity(&quarantine.dir_metadata()?)?,
        target: files_path.join(&name),
        info: Some(info_path.join(format!("{name}.trashinfo"))),
        target_parent_identity: files_identity,
        info_parent_identity: Some(identity(&info.dir_metadata()?)?),
        deletion_date: deletion_date()?,
    };
    ensure!(
        !entry_exists(&files, OsStr::new(&name))?
            && !entry_exists(&info, OsStr::new(&format!("{name}.trashinfo")))?,
        "The source Trash destination already exists; no item was replaced"
    );
    Ok(plan)
}

#[cfg(any(target_os = "linux", test))]
fn trash_directories(plan: &TrashPlan) -> Result<(Dir, Dir)> {
    let files_path = plan.target.parent().context("Invalid Trash item parent")?;
    let info_path = plan
        .info
        .as_ref()
        .and_then(|path| path.parent())
        .context("The Trash information location is missing")?;
    ensure!(
        files_path.file_name() == Some(OsStr::new("files"))
            && info_path.file_name() == Some(OsStr::new("info"))
            && files_path.parent() == info_path.parent()
            && plan.target.file_name() == Some(OsStr::new(&format!("desk-switch-{}", plan.token)))
            && plan.info.as_ref().and_then(|path| path.file_name())
                == Some(OsStr::new(&format!("desk-switch-{}.trashinfo", plan.token))),
        "The saved source Trash layout is invalid"
    );
    let files = open_absolute_dir(files_path, false)?;
    let info = open_absolute_dir(info_path, false)?;
    require_private_dir(&files)?;
    require_private_dir(&info)?;
    ensure!(
        identity(&files.dir_metadata()?)? == plan.target_parent_identity
            && Some(identity(&info.dir_metadata()?)?) == plan.info_parent_identity
            && plan.target_parent_identity.0 == plan.source_identity.0,
        "The source Trash storage was moved or replaced"
    );
    Ok((files, info))
}

#[cfg(any(target_os = "linux", test))]
fn info_bytes(plan: &TrashPlan) -> Result<Vec<u8>> {
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        plan.original.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let path = plan.original.to_string_lossy().as_bytes().to_vec();
    let mut encoded = String::new();
    for byte in path {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(&mut encoded, "%{byte:02X}")?;
        }
    }
    ensure!(
        plan.deletion_date.len() == 19
            && plan
                .deletion_date
                .bytes()
                .all(|byte| { byte.is_ascii_digit() || b"-T:".contains(&byte) }),
        "The saved source Trash date is invalid"
    );
    Ok(format!(
        "[Trash Info]\nPath={encoded}\nDeletionDate={}\nX-DeskSwitch-Cut={}\nX-DeskSwitch-Identity={}:{}\nX-DeskSwitch-Quarantine={}:{}\n",
        plan.deletion_date,
        plan.token,
        plan.source_identity.0,
        plan.source_identity.1,
        plan.quarantine_identity.0,
        plan.quarantine_identity.1
    )
    .into_bytes())
}

#[cfg(any(target_os = "linux", test))]
fn read_info(directory: &Dir, name: &OsStr) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = directory.open_with(name, &options)?;
    let metadata = file.metadata()?;
    identity(&metadata)?;
    ensure!(
        metadata.is_file() && metadata.len() <= 64 * 1024,
        "The source Trash information file is invalid"
    );
    ensure!(
        cap_fs_ext::MetadataExt::nlink(&metadata) == 1,
        "The source Trash information has an unexpected hard link"
    );
    let mut data = Vec::new();
    file.take(64 * 1024 + 1).read_to_end(&mut data)?;
    ensure!(data.len() <= 64 * 1024, "Trash information is too large");
    Ok(data)
}

#[cfg(any(target_os = "linux", test))]
fn commit_freedesktop(quarantine: &Dir, plan: &TrashPlan) -> Result<TrashReceipt> {
    use std::io::Write;
    let (files, info) = trash_directories(plan)?;
    let name = plan.target.file_name().context("Invalid Trash item name")?;
    let info_name = plan
        .info
        .as_ref()
        .and_then(|path| path.file_name())
        .context("Invalid Trash information name")?;
    let data = info_bytes(plan)?;
    // A committed crash recovery may be acknowledged without touching another
    // source. A present payload is never overwritten by the target or vice versa.
    if !entry_exists(quarantine, OsStr::new(PAYLOAD))? {
        return match inspect_freedesktop(quarantine, plan)? {
            TrashStatus::Committed(receipt) => Ok(receipt),
            _ => bail!("The retained source needs recovery; no operation was repeated"),
        };
    }
    ensure!(
        !entry_exists(&files, name)?,
        "The source Trash destination exists; no item was replaced"
    );
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match info.open_with(info_name, &options) {
        Ok(mut file) => {
            file.write_all(&data)?;
            file.sync_all()?;
            sync_directory(&info)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => ensure!(
            read_info(&info, info_name)? == data,
            "The existing Trash information belongs to another operation"
        ),
        Err(error) => return Err(error.into()),
    }
    validate_plan(quarantine, plan, true)?;
    super::storage::source_move::rename_for_trash(
        quarantine,
        OsStr::new(PAYLOAD),
        &files,
        name,
        plan.source_identity,
    )?;
    // Never unlink a mismatched target: every byte remains recoverable even if
    // another local writer changed a source between the checks and rename.
    ensure!(
        identity(&files.symlink_metadata(name)?)? == plan.source_identity,
        "A changed source was retained in Trash and needs recovery"
    );
    sync_directory(quarantine)?;
    sync_directory(&files)?;
    ensure!(
        read_info(&info, info_name)? == data,
        "Source Trash information changed after the move"
    );
    Ok(TrashReceipt {
        plan: plan.clone(),
        item: plan.target.clone(),
        item_identity: plan.source_identity,
    })
}

#[cfg(any(target_os = "linux", test))]
fn inspect_freedesktop(quarantine: &Dir, plan: &TrashPlan) -> Result<TrashStatus> {
    let (files, info) = trash_directories(plan)?;
    let name = plan.target.file_name().context("Invalid Trash item name")?;
    let info_name = plan
        .info
        .as_ref()
        .and_then(|path| path.file_name())
        .context("Invalid Trash information name")?;
    let payload_present = entry_exists(quarantine, OsStr::new(PAYLOAD))?;
    let target_present = entry_exists(&files, name)?;
    if payload_present && !target_present {
        return Ok(TrashStatus::Pending);
    }
    if !payload_present
        && target_present
        && identity(&files.symlink_metadata(name)?)? == plan.source_identity
        && read_info(&info, info_name)? == info_bytes(plan)?
    {
        return Ok(TrashStatus::Committed(TrashReceipt {
            plan: plan.clone(),
            item: plan.target.clone(),
            item_identity: plan.source_identity,
        }));
    }
    Ok(TrashStatus::NeedsRecovery {
        paths: vec![plan.target.clone()],
        detail: "The source Trash state is ambiguous; no original operation was repeated".into(),
    })
}

#[cfg(any(target_os = "linux", test))]
fn entry_exists(directory: &Dir, name: &OsStr) -> Result<bool> {
    match directory.symlink_metadata(name) {
        Ok(metadata) => {
            identity(&metadata)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(any(target_os = "linux", test))]
fn deletion_date() -> Result<String> {
    #[cfg(unix)]
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let stamp = libc::time_t::try_from(now)?;
        let mut local: libc::tm = unsafe { std::mem::zeroed() };
        ensure!(
            !unsafe { libc::localtime_r(&stamp, &mut local) }.is_null(),
            "The local Trash timestamp is unavailable"
        );
        let year = local.tm_year + 1900;
        ensure!((1970..=9999).contains(&year), "Invalid local Trash year");
        Ok(format!(
            "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            local.tm_mon + 1,
            local.tm_mday,
            local.tm_hour,
            local.tm_min,
            local.tm_sec
        ))
    }
    #[cfg(not(unix))]
    {
        // Only the explicit fake Trash test backend uses this on Windows.
        Ok("1970-01-01T00:00:00".into())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn trash_info_encodes_control_non_utf8_and_reserved_path_bytes() {
        let plan = TrashPlan {
            format: FORMAT,
            backend: TrashBackend::Freedesktop,
            token: "0123456789abcdef0123456789abcdef".into(),
            original: PathBuf::from(std::ffi::OsString::from_vec(
                b"/home/user/space %/#\n\xff/file".to_vec(),
            )),
            source_identity: (1, 2),
            quarantine_identity: (1, 3),
            target: "/unused/files/item".into(),
            info: Some("/unused/info/item.trashinfo".into()),
            target_parent_identity: (1, 4),
            info_parent_identity: Some((1, 5)),
            deletion_date: "2026-11-08T12:34:56".into(),
        };
        let data = String::from_utf8(info_bytes(&plan).unwrap()).unwrap();
        assert!(data.contains("\nPath=/home/user/space%20%25/%23%0A%FF/file\n"));
        assert_eq!(data.lines().count(), 6);
        assert!(data.contains("\nX-DeskSwitch-Identity=1:2\n"));
        assert!(data.ends_with("X-DeskSwitch-Quarantine=1:3\n"));
    }
}
