//! Recoverable Windows recycling of an already verified quarantine payload.
//!
//! The only mutating Shell entry point in this module is the documented
//! `ITransferSource::RecycleItem`. The source path is obtained from the owned
//! quarantine handle, never from `TrashPlan::original`. The durable plan pins
//! the local NTFS volume root. The Shell receives its virtual Recycle Bin item
//! and may initialize its own per-user bin; this module never creates or writes
//! the Recycle Bin's private file format.
//!
//! API contracts:
//! - https://learn.microsoft.com/windows/win32/api/shobjidl_core/nf-shobjidl_core-itransfersource-recycleitem
//! - https://learn.microsoft.com/windows/win32/api/shobjidl_core/nf-shobjidl_core-ishellitem-bindtohandler
//! - https://learn.microsoft.com/windows/win32/api/shobjidl_core/nf-shobjidl_core-itransferadvisesink-filefailure
//! - https://learn.microsoft.com/windows/win32/api/shobjidl_core/nf-shobjidl_core-iknownfolder-getshellitem

use super::{
    FORMAT, Identity, PAYLOAD, TrashBackend, TrashPlan, TrashReceipt, TrashStatus, identity,
    open_absolute_dir, require_private_dir, validate_plan,
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use cap_std::fs::{Dir, MetadataExt};
use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree, PROPERTYKEY};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT, SetSecurityInfo,
};
use windows::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce,
    GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
    GetTokenInformation, IsValidAcl, IsValidSid, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SYSTEM, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_MODE, FILE_SHARE_READ, GetDriveTypeW, GetFileInformationByHandle,
    GetFinalPathNameByHandleW, GetVolumeInformationW, GetVolumePathNameW, OPEN_EXISTING,
    READ_CONTROL, ReOpenFile, VOLUME_NAME_DOS, WRITE_DAC, WRITE_OWNER,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{
    BHID_Transfer, COPYENGINE_E_USER_CANCELLED, FOLDERID_RecycleBinFolder, IKnownFolderManager,
    IShellItem, ITransferAdviseSink, ITransferAdviseSink_Impl, ITransferSource, KnownFolderManager,
    SHCreateItemFromParsingName, SIGDN_FILESYSPATH, TSF_NORMAL,
};
use windows::core::{BOOL, Error as WindowsError, HRESULT, PCWSTR, PWSTR, Ref, implement};

const SHELL_WAIT: Duration = Duration::from_secs(15);
const SCAN_TIME: Duration = Duration::from_millis(250);
const MAX_RECYCLE_ENTRIES: usize = 4096;
const MAX_NATIVE_PATH: usize = 32768;
// Shell parsing of extended-length paths is not a reliable contract. Leave
// these payloads quarantined instead of converting an ambiguous path.
const MAX_SHELL_PATH: usize = 259;
const MAX_TOKEN_BYTES: usize = 65536;
const LOCAL_FIXED_DRIVE: u32 = 3;

// A timed-out call keeps this lease and its quarantine capability until its
// thread really exits. A hung Shell handler cannot accumulate more workers or
// cause a second operation on the same payload.
static SHELL_RUNNING: AtomicBool = AtomicBool::new(false);

struct ShellLease;

impl ShellLease {
    fn acquire() -> Result<Self> {
        SHELL_RUNNING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| anyhow!("a previous Windows recycling operation still needs recovery"))?;
        Ok(Self)
    }
}

impl Drop for ShellLease {
    fn drop(&mut self) {
        SHELL_RUNNING.store(false, Ordering::Release);
    }
}

pub(super) fn prepare(
    quarantine: &Dir,
    original: &Path,
    expected: Identity,
    token: &str,
) -> Result<TrashPlan> {
    #[cfg(test)]
    super::require_hosted_native_scope(quarantine)?;
    ensure!(
        !token.is_empty()
            && token.len() <= 128
            && token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
        "invalid quarantine recovery token"
    );
    require_private_dir(quarantine)?;
    let quarantine_identity = identity(&quarantine.dir_metadata()?)?;
    ensure!(
        identity(&quarantine.symlink_metadata(PAYLOAD)?)? == expected,
        "quarantine payload identity changed before recycling"
    );
    let quarantine_path = handle_path(quarantine)?;
    let quarantine_pins = pin_directory_path(&quarantine_path)?;
    ensure!(
        quarantine_pins.identity() == quarantine_identity,
        "quarantine path no longer names the owned directory"
    );
    let payload = open_existing(
        &quarantine_path.join(PAYLOAD),
        FILE_SHARE_READ | FILE_SHARE_DELETE,
        false,
    )?;
    verify_payload(&payload, expected)?;
    ensure!(
        expected.0 == quarantine_identity.0,
        "quarantine and payload are on different volumes"
    );
    let anchor = volume_anchor(&quarantine_path, expected.0)?;
    let _existing_recycle = existing_recycle_parent(&anchor)?;
    Ok(TrashPlan {
        format: FORMAT,
        backend: TrashBackend::WindowsRecycle,
        token: token.to_owned(),
        // Display metadata only. No operation in this module opens this path.
        original: original.to_owned(),
        source_identity: expected,
        quarantine_identity,
        target: anchor.path.clone(),
        info: None,
        target_parent_identity: anchor.pins.identity(),
        info_parent_identity: None,
        deletion_date: String::new(),
    })
}

pub(super) fn commit(quarantine: &Dir, plan: &TrashPlan) -> Result<TrashReceipt> {
    #[cfg(test)]
    let native_permit = {
        super::require_hosted_native_scope(quarantine)?;
        super::hosted_native_permit()?
    };
    validate_windows_plan(quarantine, plan, true)?;
    let lease = ShellLease::acquire()?;
    let quarantine = quarantine.try_clone()?;
    let plan = plan.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let (send, receive) = mpsc::sync_channel(1);
    let _worker = std::thread::Builder::new()
        .name("source-recycle-sta".to_owned())
        .spawn(move || {
            let _lease = lease;
            #[cfg(test)]
            let result = super::with_hosted_native_permit(native_permit, || {
                commit_sta(&quarantine, &plan, worker_cancelled)
            });
            #[cfg(not(test))]
            let result = commit_sta(&quarantine, &plan, worker_cancelled);
            // The native phase and its pin/COM cleanup have actually ended.
            // Publish the result only after allowing a subsequent inspector.
            drop(_lease);
            // A detached, late result is rediscovered by exact identity from
            // the durable plan. It must never trigger an original-path retry.
            let _ = send.send(result);
        })
        .context("could not start the Windows recycling worker")?;
    match receive.recv_timeout(SHELL_WAIT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            cancelled.store(true, Ordering::Release);
            bail!("Windows recycling timed out; the protected quarantine needs recovery")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            bail!(
                "Windows recycling worker stopped without a verified receipt; recovery is required"
            )
        }
    }
}

pub(super) fn inspect(quarantine: &Dir, plan: &TrashPlan) -> Result<TrashStatus> {
    #[cfg(test)]
    super::require_hosted_native_scope(quarantine)?;
    let recovery_paths = || {
        let mut paths = vec![plan.target.clone()];
        if let Ok(path) = handle_path(quarantine) {
            paths.insert(0, path.join(PAYLOAD));
        }
        paths
    };
    // The Shell may still be processing after the caller's bounded wait. Do
    // not issue another native operation or report a transient absence as a
    // completed recycle while that worker retains the protected path.
    let _lease = match ShellLease::acquire() {
        Ok(lease) => lease,
        Err(_) => {
            return Ok(TrashStatus::NeedsRecovery {
                paths: recovery_paths(),
                detail:
                    "a Windows recycling operation is still active; inspect again after it exits"
                        .to_owned(),
            });
        }
    };
    match inspect_inner(quarantine, plan) {
        Ok(status) => Ok(status),
        Err(error) => Ok(TrashStatus::NeedsRecovery {
            paths: recovery_paths(),
            detail: format!("Windows recycling could not be verified: {error:#}"),
        }),
    }
}

fn inspect_inner(quarantine: &Dir, plan: &TrashPlan) -> Result<TrashStatus> {
    validate_windows_plan(quarantine, plan, false)?;
    let quarantine_path = handle_path(quarantine)?;
    let quarantine_pins = pin_directory_path(&quarantine_path)?;
    ensure!(
        quarantine_pins.identity() == plan.quarantine_identity,
        "quarantine path identity changed"
    );
    let anchor = volume_anchor(&quarantine_path, plan.source_identity.0)?;
    verify_anchor_plan(&anchor, plan)?;
    let payload = match quarantine.symlink_metadata(PAYLOAD) {
        Ok(metadata) => Some(identity(&metadata)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let recycle = existing_recycle_parent(&anchor)?;
    let recycled = match &recycle {
        Some(recycle) => find_recycled_item(recycle, plan.source_identity)?,
        None => None,
    };
    match (payload, recycled) {
        (Some(actual), None) if actual == plan.source_identity => Ok(TrashStatus::Pending),
        (None, Some(item)) => {
            ensure_payload_absent(quarantine)?;
            verify_payload(&item.file, plan.source_identity)?;
            Ok(TrashStatus::Committed(TrashReceipt {
                plan: plan.clone(),
                item: item.path,
                item_identity: plan.source_identity,
            }))
        }
        (Some(_), _) => bail!("a conflicting payload or Recycle Bin identity requires recovery"),
        (None, None) => bail!("the payload is absent and no exact recycled identity was found"),
    }
}

fn commit_sta(
    quarantine: &Dir,
    plan: &TrashPlan,
    cancelled: Arc<AtomicBool>,
) -> Result<TrashReceipt> {
    let deadline = Instant::now() + SHELL_WAIT;
    let _apartment = Apartment::new()?;
    validate_windows_plan(quarantine, plan, true)?;
    let quarantine_path = handle_path(quarantine)?;
    // Each component is opened from the volume root without following reparse
    // points. Denying both write and delete sharing prevents ancestor rename
    // and reparse mutation while a path-based Shell handler is using it. These
    // pins stay in this worker even if its caller has already timed out.
    let quarantine_pins = pin_directory_path(&quarantine_path)?;
    ensure!(
        quarantine_pins.identity() == plan.quarantine_identity,
        "quarantine path no longer names the owned directory"
    );
    let anchor = volume_anchor(&quarantine_path, plan.source_identity.0)?;
    verify_anchor_plan(&anchor, plan)?;
    let previous_recycle = existing_recycle_parent(&anchor)?;
    let payload_path = quarantine_path.join(PAYLOAD);
    let payload = open_existing(&payload_path, FILE_SHARE_READ | FILE_SHARE_DELETE, false)?;
    verify_payload(&payload, plan.source_identity)?;

    let source_parent = shell_item(&quarantine_path)?;
    let source_item = shell_item(&payload_path)?;
    let destination = virtual_recycle_item()?;
    // BHID_Transfer is documented for folders: use the owned source parent,
    // not an untrusted file association or a handler on the payload itself.
    let transfer: ITransferSource =
        unsafe { source_parent.BindToHandler(None::<&IBindCtx>, &BHID_Transfer) }
            .context("the source folder has no supported recycling handler")?;
    let sink: ITransferAdviseSink = SilentTransferSink {
        cancelled: Arc::clone(&cancelled),
        deadline,
    }
    .into();
    let cookie = unsafe { transfer.Advise(&sink) }
        .context("the recycling handler cannot suppress interactive decisions")?;
    let _advice = AdvisorySession {
        transfer: &transfer,
        cookie,
    };

    ensure!(
        !cancelled.load(Ordering::Acquire) && Instant::now() < deadline,
        "Windows recycling was cancelled before its critical phase"
    );
    quarantine_pins.revalidate()?;
    anchor.pins.revalidate()?;
    if let Some(recycle) = &previous_recycle {
        recycle.pins.revalidate()?;
    }
    ensure!(
        native_identity(quarantine)?.0 == plan.quarantine_identity
            && identity(&quarantine.symlink_metadata(PAYLOAD)?)? == plan.source_identity,
        "the owned quarantine changed before the recycling call"
    );
    verify_payload(&payload, plan.source_identity)?;

    // No RemoveItem, IFileOperation delete, copy/delete flag, overwrite flag,
    // recycle-if-possible flag, elevation, or permanent-delete fallback exists.
    let recycled = unsafe { transfer.RecycleItem(&source_item, &destination, TSF_NORMAL.0 as u32) }
        .context(
            "Windows did not return a recycled item; inspect the quarantine before retrying",
        )?;

    let item = shell_item_path(&recycled)?;
    let recycle = existing_recycle_parent(&anchor)?.with_context(|| {
        format!(
            "the returned item at {} has no verifiable per-user Recycle Bin",
            item.display()
        )
    })?;
    if let Some(previous) = &previous_recycle {
        ensure!(
            previous.pins.identity() == recycle.pins.identity(),
            "the existing Recycle Bin parent changed during recycling"
        );
    }

    // A success HRESULT is insufficient. Require the actual returned Shell
    // item to resolve directly inside the pinned per-user Recycle Bin and to
    // have exactly the payload's original volume/file ID.
    let parent = item
        .parent()
        .context("recycled item has no filesystem parent")?;
    ensure!(
        ascii_path_eq(parent, &recycle.path),
        "the returned item at {} is outside the verified Recycle Bin parent; recovery is required",
        item.display()
    );
    let name = item
        .file_name()
        .context("recycled item has no filesystem name")?;
    validate_component(name)?;
    let item = recycle.path.join(name);
    let recycled_handle = open_existing(&item, FILE_SHARE_READ, false).with_context(|| {
        format!(
            "could not verify the returned recycled item at {}",
            item.display()
        )
    })?;
    verify_payload(&recycled_handle, plan.source_identity).with_context(|| {
        format!(
            "the returned recycled item at {} needs recovery",
            item.display()
        )
    })?;
    verify_payload(&payload, plan.source_identity)?;
    ensure_payload_absent(quarantine)?;
    quarantine_pins.revalidate()?;
    anchor.pins.revalidate()?;
    recycle.pins.revalidate()?;
    Ok(TrashReceipt {
        plan: plan.clone(),
        item,
        item_identity: plan.source_identity,
    })
}

fn validate_windows_plan(quarantine: &Dir, plan: &TrashPlan, payload: bool) -> Result<()> {
    ensure!(
        plan.format == FORMAT
            && plan.backend == TrashBackend::WindowsRecycle
            && plan.info.is_none()
            && plan.info_parent_identity.is_none(),
        "invalid Windows recycling recovery plan"
    );
    validate_plan(quarantine, plan, payload)?;
    require_private_dir(quarantine)?;
    ensure!(
        identity(&quarantine.dir_metadata()?)? == plan.quarantine_identity,
        "owned quarantine identity does not match the recovery plan"
    );
    Ok(())
}

fn ensure_payload_absent(quarantine: &Dir) -> Result<()> {
    match quarantine.symlink_metadata(PAYLOAD) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("could not verify the quarantined payload's absence"),
        Ok(_) => bail!("the quarantine still contains a payload; recycling is unverified"),
    }
}

struct RecycleParent {
    path: PathBuf,
    directory: Dir,
    pins: DirectoryPins,
}

struct VolumeAnchor {
    path: PathBuf,
    directory: Dir,
    pins: DirectoryPins,
    user_sid: String,
}

fn volume_anchor(quarantine_path: &Path, volume: u64) -> Result<VolumeAnchor> {
    let quarantine_wide = wide_path(quarantine_path)?;
    let mut root_buffer = vec![0u16; MAX_NATIVE_PATH];
    unsafe { GetVolumePathNameW(PCWSTR(quarantine_wide.as_ptr()), &mut root_buffer) }
        .context("could not establish the quarantine volume root")?;
    let root = path_from_terminated(&root_buffer)?;
    validate_dos_path(&root)?;
    // Mounted folders, remote paths, removable media and 128-bit ReFS file IDs
    // are deliberately unsupported by this 64-bit identity recovery format.
    ensure!(
        root.components().count() == 2,
        "mounted-folder volumes need manual recovery"
    );
    let root_wide = wide_path(&root)?;
    ensure!(
        unsafe { GetDriveTypeW(PCWSTR(root_wide.as_ptr())) } == LOCAL_FIXED_DRIVE,
        "this source volume has no supported local Recycle Bin"
    );
    let mut serial = 0u32;
    let mut filesystem = [0u16; 32];
    unsafe {
        GetVolumeInformationW(
            PCWSTR(root_wide.as_ptr()),
            None,
            Some(&mut serial),
            None,
            None,
            Some(&mut filesystem),
        )
    }
    .context("could not establish a stable local volume identity")?;
    let filesystem = OsString::from_wide(terminated(&filesystem)?);
    ensure!(
        serial as u64 == volume && filesystem == OsStr::new("NTFS"),
        "recycling recovery requires a matching local NTFS volume"
    );
    let pins = pin_directory_path(&root)?;
    ensure!(
        pins.identity().0 == volume,
        "the volume root identity differs from the quarantine volume"
    );
    let directory = open_absolute_dir(&root, false)?;
    ensure!(
        identity(&directory.dir_metadata()?)? == pins.identity(),
        "the volume root changed"
    );
    Ok(VolumeAnchor {
        path: root,
        directory,
        pins,
        user_sid: process_user_sid()?,
    })
}

fn existing_recycle_parent(anchor: &VolumeAnchor) -> Result<Option<RecycleParent>> {
    let root_metadata = match anchor.directory.symlink_metadata("$Recycle.Bin") {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let root_identity = identity(&root_metadata)?;
    let recycle_root = anchor.path.join("$Recycle.Bin");
    let root_pins = pin_directory_path(&recycle_root)?;
    ensure!(
        root_pins.identity() == root_identity && root_identity.0 == anchor.pins.identity().0,
        "the Recycle Bin root does not match the pinned volume"
    );
    let root_attributes = native_identity(
        &root_pins
            .entries
            .last()
            .context("missing Recycle Bin root")?
            .file,
    )?
    .1;
    ensure!(
        root_attributes.dwFileAttributes & (FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0)
            == FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0,
        "the existing Recycle Bin root is not recognizable"
    );
    let root_directory = open_absolute_dir(&recycle_root, false)?;
    ensure!(
        identity(&root_directory.dir_metadata()?)? == root_identity,
        "the Recycle Bin root changed"
    );
    let parent_metadata = match root_directory.symlink_metadata(&anchor.user_sid) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let parent_identity = identity(&parent_metadata)?;
    let path = recycle_root.join(&anchor.user_sid);
    let pins = pin_directory_path(&path)?;
    ensure!(
        pins.identity() == parent_identity && parent_identity.0 == anchor.pins.identity().0,
        "the per-user Recycle Bin does not match the pinned volume"
    );
    ensure!(
        owner_sid(&pins.entries.last().context("missing Recycle Bin pin")?.file)?
            == anchor.user_sid,
        "the Recycle Bin parent is not owned by the process user"
    );
    let directory = open_absolute_dir(&path, false)?;
    ensure!(
        identity(&directory.dir_metadata()?)? == pins.identity(),
        "the Recycle Bin parent changed while being opened"
    );
    Ok(Some(RecycleParent {
        path,
        directory,
        pins,
    }))
}

fn verify_anchor_plan(anchor: &VolumeAnchor, plan: &TrashPlan) -> Result<()> {
    ensure!(
        ascii_path_eq(&anchor.path, &plan.target)
            && anchor.pins.identity() == plan.target_parent_identity
            && plan.target_parent_identity.0 == plan.source_identity.0,
        "the source volume root no longer matches the recycling recovery plan"
    );
    Ok(())
}

struct RecycledItem {
    path: PathBuf,
    file: File,
}

fn find_recycled_item(recycle: &RecycleParent, expected: Identity) -> Result<Option<RecycledItem>> {
    let deadline = Instant::now() + SCAN_TIME;
    let mut found: Option<(PathBuf, File)> = None;
    for (index, entry) in recycle.directory.entries()?.enumerate() {
        ensure!(
            index < MAX_RECYCLE_ENTRIES && Instant::now() < deadline,
            "the bounded Recycle Bin identity scan could not finish"
        );
        let name = entry?.file_name();
        validate_component(&name)?;
        let metadata = recycle.directory.symlink_metadata(&name)?;
        // Other recycled items may themselves be links. They cannot be this
        // validated non-reparse payload, and are never followed or opened.
        if metadata.is_symlink() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        {
            continue;
        }
        if identity(&metadata)? != expected {
            continue;
        }
        ensure!(
            found.is_none(),
            "multiple recycled names match the payload identity"
        );
        let path = recycle.path.join(name);
        let file = open_existing(&path, FILE_SHARE_READ, false)?;
        verify_payload(&file, expected)?;
        found = Some((path, file));
    }
    recycle.pins.revalidate()?;
    if let Some((path, file)) = found {
        verify_payload(&file, expected)?;
        Ok(Some(RecycledItem { path, file }))
    } else {
        Ok(None)
    }
}

struct DirectoryPin {
    file: File,
    identity: Identity,
}

struct DirectoryPins {
    entries: Vec<DirectoryPin>,
}

impl DirectoryPins {
    fn identity(&self) -> Identity {
        self.entries
            .last()
            .expect("directory pins include their root")
            .identity
    }

    fn revalidate(&self) -> Result<()> {
        for pin in &self.entries {
            let (actual, information) = native_identity(&pin.file)?;
            ensure!(
                actual == pin.identity
                    && information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0,
                "a protected path ancestor changed identity"
            );
        }
        Ok(())
    }
}

fn pin_directory_path(path: &Path) -> Result<DirectoryPins> {
    validate_dos_path(path)?;
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        bail!("a protected absolute drive path is required")
    };
    let Prefix::Disk(letter) = prefix.kind() else {
        bail!("only ordinary local drive paths can be pinned")
    };
    ensure!(
        matches!(components.next(), Some(Component::RootDir)),
        "missing drive root"
    );
    let mut current = PathBuf::from(format!("{}:\\", char::from(letter)));
    let mut entries = Vec::new();
    let file = open_existing(&current, FILE_SHARE_READ, true)?;
    let (root_identity, _) = native_identity(&file)?;
    entries.push(DirectoryPin {
        file,
        identity: root_identity,
    });
    for component in components {
        let Component::Normal(name) = component else {
            bail!("a protected path contains a non-normal component")
        };
        validate_component(name)?;
        current.push(name);
        let file = open_existing(&current, FILE_SHARE_READ, true)?;
        let (identity, _) = native_identity(&file)?;
        ensure!(
            identity.0 == root_identity.0,
            "a protected path crosses a volume boundary"
        );
        entries.push(DirectoryPin { file, identity });
    }
    Ok(DirectoryPins { entries })
}

fn open_existing(path: &Path, sharing: FILE_SHARE_MODE, directory: bool) -> Result<File> {
    let wide = wide_path(path)?;
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_READ_ATTRIBUTES.0 | READ_CONTROL.0,
            sharing,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
    }
    .context("could not protect an existing filesystem object")?;
    // Ownership is transferred exactly once; std::fs::File closes the handle
    // on every error and on normal return.
    let file = unsafe { File::from_raw_handle(handle.0) };
    let (_, information) = native_identity(&file)?;
    ensure!(
        !directory || information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0,
        "a protected path ancestor is not a directory"
    );
    Ok(file)
}

fn native_identity(file: &impl AsRawHandle) -> Result<(Identity, BY_HANDLE_FILE_INFORMATION)> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information) }
        .context("could not read the filesystem object's stable identity")?;
    ensure!(
        information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0,
        "reparse points are not supported in recycling paths"
    );
    let index = ((information.nFileIndexHigh as u64) << 32) | information.nFileIndexLow as u64;
    ensure!(
        index != 0,
        "the filesystem object has no usable file identity"
    );
    Ok((
        (information.dwVolumeSerialNumber as u64, index),
        information,
    ))
}

fn verify_payload(file: &File, expected: Identity) -> Result<()> {
    let (actual, information) = native_identity(file)?;
    ensure!(
        actual == expected,
        "the payload's volume/file identity does not match"
    );
    ensure!(
        information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0
            || information.nNumberOfLinks == 1,
        "a multiply-linked payload cannot be recycled unambiguously"
    );
    Ok(())
}

fn handle_path(file: &impl AsRawHandle) -> Result<PathBuf> {
    let mut path = vec![0u16; MAX_NATIVE_PATH];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            HANDLE(file.as_raw_handle()),
            &mut path,
            windows::Win32::Storage::FileSystem::GETFINALPATHNAMEBYHANDLE_FLAGS(
                FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0,
            ),
        )
    } as usize;
    ensure!(
        length != 0 && length < path.len(),
        "the owned handle has no bounded DOS path"
    );
    path.truncate(length);
    ordinary_dos_path(&path)
}

fn ordinary_dos_path(wide: &[u16]) -> Result<PathBuf> {
    let prefix: Vec<u16> = "\\\\?\\".encode_utf16().collect();
    let wide = wide.strip_prefix(prefix.as_slice()).unwrap_or(wide);
    let path = PathBuf::from(OsString::from_wide(wide));
    validate_dos_path(&path)?;
    Ok(path)
}

fn validate_dos_path(path: &Path) -> Result<()> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    ensure!(
        !wide.is_empty() && wide.len() <= MAX_SHELL_PATH && !wide.contains(&0),
        "the filesystem path is not supported by the bounded Shell parser"
    );
    let mut components = path.components();
    ensure!(
        matches!(components.next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)))
            && matches!(components.next(), Some(Component::RootDir)),
        "only ordinary absolute local drive paths are supported"
    );
    for component in components {
        let Component::Normal(name) = component else {
            bail!("path contains a non-normal component")
        };
        validate_component(name)?;
    }
    Ok(())
}

fn validate_component(name: &OsStr) -> Result<()> {
    let wide: Vec<u16> = name.encode_wide().collect();
    ensure!(
        !wide.is_empty()
            && wide != [b'.' as u16]
            && wide != [b'.' as u16, b'.' as u16]
            && !matches!(wide.last(), Some(value) if *value == b'.' as u16 || *value == b' ' as u16)
            && !wide.iter().any(
                |value| *value < 32 || [b'\\' as u16, b'/' as u16, b':' as u16].contains(value)
            ),
        "a filesystem name cannot be passed unambiguously to the Shell"
    );
    if let Some(name) = name.to_str() {
        let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
        let numbered = stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(
                    number,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            });
        ensure!(
            !matches!(
                stem.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
            ) && !numbered,
            "DOS device-name aliases are not supported in recycling paths"
        );
    }
    Ok(())
}

fn wide_path(path: &Path) -> Result<Vec<u16>> {
    validate_dos_path(path)?;
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    Ok(wide)
}

fn ascii_path_eq(left: &Path, right: &Path) -> bool {
    // The only use is the ASCII volume-root/$Recycle.Bin/SID parent. Unicode
    // case folding or alternate namespace aliases are not accepted.
    match (left.to_str(), right.to_str()) {
        (Some(left), Some(right)) if left.is_ascii() && right.is_ascii() => {
            left.eq_ignore_ascii_case(right)
        }
        _ => false,
    }
}

fn terminated(buffer: &[u16]) -> Result<&[u16]> {
    let end = buffer
        .iter()
        .position(|value| *value == 0)
        .context("unterminated Windows string")?;
    Ok(&buffer[..end])
}

fn path_from_terminated(buffer: &[u16]) -> Result<PathBuf> {
    ordinary_dos_path(terminated(buffer)?)
}

fn shell_item(path: &Path) -> Result<IShellItem> {
    let wide = wide_path(path)?;
    unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None::<&IBindCtx>) }
        .context("could not bind the protected filesystem object to the Shell")
}

fn virtual_recycle_item() -> Result<IShellItem> {
    // SHGetKnownFolderItem excludes virtual folders. The documented known
    // folder manager and GetShellItem expose this actual namespace location
    // without manufacturing a physical $Recycle.Bin directory or private data.
    let manager: IKnownFolderManager = unsafe {
        CoCreateInstance(
            &KnownFolderManager,
            None::<&windows::core::IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }
    .context("the Windows known-folder manager is unavailable")?;
    let folder = unsafe { manager.GetFolder(&FOLDERID_RecycleBinFolder) }
        .context("the Windows Recycle Bin namespace is unavailable")?;
    ensure!(
        unsafe { folder.GetId() }? == FOLDERID_RecycleBinFolder,
        "the known-folder manager returned another namespace"
    );
    unsafe { folder.GetShellItem(0) }
        .context("could not obtain the actual Recycle Bin namespace item")
}

fn shell_item_path(item: &IShellItem) -> Result<PathBuf> {
    let name = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }
        .context("the recycled Shell item has no verifiable filesystem path")?;
    let allocation = TaskAllocation(name.0.cast());
    ensure!(
        !name.is_null(),
        "the recycled Shell item returned an empty path"
    );
    let mut length = 0;
    while length < MAX_NATIVE_PATH && unsafe { *name.0.add(length) } != 0 {
        length += 1;
    }
    ensure!(
        length < MAX_NATIVE_PATH,
        "the recycled Shell item path is unbounded"
    );
    let wide = unsafe { std::slice::from_raw_parts(name.0, length) };
    let path = ordinary_dos_path(wide);
    drop(allocation);
    path
}

struct TaskAllocation(*mut c_void);

impl Drop for TaskAllocation {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.cast_const())) };
    }
}

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe { LocalFree(Some(HLOCAL(self.0))) };
    }
}

struct TokenHandle(HANDLE);

impl Drop for TokenHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn process_user_sid() -> Result<String> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .context("could not establish the process user for recycling")?;
    let token = TokenHandle(token);
    let mut required = 0u32;
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut required) };
    ensure!(
        required as usize >= size_of::<TOKEN_USER>() && required as usize <= MAX_TOKEN_BYTES,
        "the process user token has no bounded identity"
    );
    // usize storage provides the native pointer alignment required by
    // TOKEN_USER; its SID points into this buffer until conversion completes.
    let words = (required as usize).div_ceil(size_of::<usize>());
    let mut buffer = vec![0usize; words];
    unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            required,
            &mut required,
        )
    }
    .context("could not read the process user identity")?;
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    sid_string(user.User.Sid)
}

fn sid_string(sid: PSID) -> Result<String> {
    let mut value = PWSTR::null();
    unsafe { ConvertSidToStringSidW(sid, &mut value) }
        .context("could not read a filesystem owner SID")?;
    let _allocation = LocalAllocation(value.0.cast());
    ensure!(!value.is_null(), "the filesystem owner SID is empty");
    let mut length = 0;
    while length <= 184 && unsafe { *value.0.add(length) } != 0 {
        length += 1;
    }
    ensure!(length <= 184, "the filesystem owner SID is unbounded");
    let value = String::from_utf16(unsafe { std::slice::from_raw_parts(value.0, length) })?;
    ensure!(
        value.starts_with("S-1-")
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'S' || byte == b'-'),
        "the filesystem owner SID is not a safe directory name"
    );
    Ok(value)
}

fn owner_sid(file: &File) -> Result<String> {
    let mut owner = PSID::default();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let result = unsafe {
        GetSecurityInfo(
            HANDLE(file.as_raw_handle()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            Some(&mut descriptor),
        )
    };
    let _allocation = LocalAllocation(descriptor.0);
    ensure!(
        result.0 == 0 && !owner.0.is_null(),
        "the Recycle Bin owner cannot be verified"
    );
    sid_string(owner)
}

/// Read-only proof required before detaching a Windows source into quarantine.
/// A directory that inherited grants for other users must be made private by
/// its creator, before the source move; recycling never changes those ACLs.
pub(super) fn verify_private_directory(directory: &Dir) -> Result<()> {
    #[cfg(test)]
    super::require_hosted_native_scope(directory)?;
    let (_, information) = native_identity(directory)?;
    ensure!(
        information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0,
        "the source quarantine is not a directory"
    );
    let user = process_user_sid()?;
    let mut owner = PSID::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let result = unsafe {
        GetSecurityInfo(
            HANDLE(directory.as_raw_handle()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut dacl),
            None,
            Some(&mut descriptor),
        )
    };
    let _allocation = LocalAllocation(descriptor.0);
    ensure!(
        result.0 == 0
            && !owner.0.is_null()
            && !dacl.is_null()
            && unsafe { IsValidAcl(dacl) }.as_bool(),
        "the source quarantine has no provably private Windows ACL"
    );
    let mut control = 0u16;
    let mut revision = 0;
    unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }
        .context("could not verify quarantine inheritance protection")?;
    ensure!(
        revision == 1 && control & SE_DACL_PROTECTED.0 != 0,
        "the source quarantine ACL is not protected from parent inheritance"
    );
    ensure!(
        sid_string(owner)? == user,
        "the source quarantine is not owned by this user"
    );
    let ace_count = unsafe { (*dacl).AceCount } as u32;
    ensure!(
        ace_count <= 1024,
        "the source quarantine ACL is too complex to verify"
    );
    for index in 0..ace_count {
        let mut pointer = std::ptr::null_mut();
        unsafe { GetAce(dacl, index, &mut pointer) }
            .context("could not verify a source quarantine access rule")?;
        ensure!(
            !pointer.is_null(),
            "the source quarantine contains an empty access rule"
        );
        let header = unsafe { std::ptr::read_unaligned(pointer.cast::<ACE_HEADER>()) };
        // ACE_HEADER types 0/1 are the documented ordinary allow/deny ACEs.
        // Conditional, object and callback grants are conservatively rejected.
        if header.AceType == 1 {
            continue;
        }
        ensure!(
            header.AceType == 0 && header.AceSize as usize >= size_of::<ACCESS_ALLOWED_ACE>(),
            "the source quarantine has an unsupported access rule"
        );
        let ace = pointer.cast::<ACCESS_ALLOWED_ACE>();
        let mask = unsafe { std::ptr::read_unaligned(std::ptr::addr_of!((*ace).Mask)) };
        if mask == 0 {
            continue;
        }
        let sid = PSID(
            unsafe { std::ptr::addr_of!((*ace).SidStart) }
                .cast_mut()
                .cast(),
        );
        let sid_offset = (sid.0 as usize)
            .checked_sub(pointer as usize)
            .context("invalid source quarantine access rule")?;
        ensure!(
            sid_offset + 8 <= header.AceSize as usize,
            "the access rule SID is truncated"
        );
        let sub_authorities = unsafe { *sid.0.cast::<u8>().add(1) } as usize;
        ensure!(
            sub_authorities <= 15
                && sid_offset + 8 + sub_authorities * 4 <= header.AceSize as usize
                && unsafe { IsValidSid(sid) }.as_bool(),
            "the source quarantine access rule has no bounded valid SID"
        );
        let granted = sid_string(sid)?;
        ensure!(
            granted == user || granted == "S-1-5-18" || granted == "S-1-5-32-544",
            "the source quarantine grants access to another user or group"
        );
    }
    Ok(())
}

/// Protect only a freshly, exclusively created empty quarantine child.
///
/// The caller must own the creation result, call this before adding a journal
/// or detaching any source, and abandon the operation if it fails. Neither the
/// source nor any existing parent directory is authority for this mutation.
/// ReOpenFile upgrades the exact directory object; there is no path fallback.
pub(super) fn protect_new_private_directory(directory: &Dir) -> Result<()> {
    #[cfg(test)]
    super::require_hosted_native_scope(directory)?;
    let expected = identity(&directory.dir_metadata()?)?;
    let (actual, information) = native_identity(directory)?;
    ensure!(
        actual == expected && information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0,
        "the new quarantine directory has no stable identity"
    );
    require_empty_directory(directory)?;
    let user = process_user_sid()?;
    let handle = unsafe {
        ReOpenFile(
            HANDLE(directory.as_raw_handle()),
            FILE_READ_ATTRIBUTES.0 | READ_CONTROL.0 | WRITE_DAC.0,
            FILE_SHARE_READ,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
    }
    .context("could not safely reopen the new quarantine for private access rules")?;
    let mut exact = unsafe { File::from_raw_handle(handle.0) };
    let prior_owner = owner_sid(&exact)?;
    ensure!(
        native_identity(&exact)?.0 == expected
            && (prior_owner == user || prior_owner == "S-1-5-32-544"),
        "the new quarantine is not the exact directory owned by this user"
    );
    let change_owner = prior_owner != user;
    if change_owner {
        let handle = unsafe {
            ReOpenFile(
                HANDLE(exact.as_raw_handle()),
                FILE_READ_ATTRIBUTES.0 | READ_CONTROL.0 | WRITE_DAC.0 | WRITE_OWNER.0,
                FILE_SHARE_READ,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            )
        }
        .context("the fresh administrator-owned quarantine cannot grant current-user ownership")?;
        let upgraded = unsafe { File::from_raw_handle(handle.0) };
        ensure!(
            native_identity(&upgraded)?.0 == expected && owner_sid(&upgraded)? == prior_owner,
            "the new quarantine changed while acquiring owner access"
        );
        exact = upgraded;
    }

    // No inheritable grants. The caller's exclusive empty-creation contract
    // ensures there is no existing child for security propagation. The
    // user-owned case preserves its owner with minimum rights. A fresh object
    // created under an elevated token may instead default to Administrators;
    // only that case requests already-granted WRITE_OWNER and assigns User.
    // No token privilege or elevation changes are requested.
    let descriptor_text = format!("O:{user}D:P(A;;FA;;;{user})(A;;FA;;;SY)");
    let mut descriptor_wide: Vec<u16> = descriptor_text.encode_utf16().collect();
    descriptor_wide.push(0);
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(descriptor_wide.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .context("could not prepare private quarantine access rules")?;
    let _allocation = LocalAllocation(descriptor.0);
    let mut present = BOOL::default();
    let mut defaulted = BOOL::default();
    let mut dacl = std::ptr::null_mut();
    unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) }
        .context("could not extract private quarantine access rules")?;
    ensure!(
        present.as_bool() && !dacl.is_null() && unsafe { IsValidAcl(dacl) }.as_bool(),
        "private quarantine access rules are not a valid non-null DACL"
    );
    let mut desired_owner = PSID::default();
    let mut owner_defaulted = BOOL::default();
    unsafe { GetSecurityDescriptorOwner(descriptor, &mut desired_owner, &mut owner_defaulted) }
        .context("could not extract the intended quarantine owner")?;
    ensure!(
        sid_string(desired_owner)? == user,
        "the intended quarantine owner is not this user"
    );
    require_empty_directory(directory)?;
    ensure!(
        native_identity(&exact)?.0 == expected && native_identity(directory)?.0 == expected,
        "the new quarantine changed before protection"
    );
    let mut security_information = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
    if change_owner {
        security_information |= OWNER_SECURITY_INFORMATION;
    }
    let result = unsafe {
        SetSecurityInfo(
            HANDLE(exact.as_raw_handle()),
            SE_FILE_OBJECT,
            security_information,
            change_owner.then_some(desired_owner),
            None,
            Some(dacl.cast_const()),
            None,
        )
    };
    ensure!(
        result.0 == 0,
        "could not protect the new quarantine DACL (Windows error {})",
        result.0
    );
    ensure!(
        native_identity(&exact)?.0 == expected && native_identity(directory)?.0 == expected,
        "the new quarantine changed while its access rules were protected"
    );
    verify_private_directory(directory)?;
    require_empty_directory(directory)?;
    Ok(())
}

fn require_empty_directory(directory: &Dir) -> Result<()> {
    if let Some(entry) = directory.entries()?.next() {
        entry?;
        bail!("only a newly created empty quarantine directory can be protected")
    }
    Ok(())
}

struct Apartment;

impl Apartment {
    fn new() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }
            .ok()
            .context("could not initialize the dedicated recycling apartment")?;
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct AdvisorySession<'a> {
    transfer: &'a ITransferSource,
    cookie: u32,
}

impl Drop for AdvisorySession<'_> {
    fn drop(&mut self) {
        let _ = unsafe { self.transfer.Unadvise(self.cookie) };
    }
}

// The copy-engine contract delegates user decisions and failures to this sink.
// We never display a prompt, approve an overwrite, grant elevation, suggest an
// alternate path, or turn an error into an ignored/success result.
#[implement(ITransferAdviseSink)]
struct SilentTransferSink {
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
}

#[allow(non_snake_case)]
impl ITransferAdviseSink_Impl for SilentTransferSink_Impl {
    fn UpdateProgress(
        &self,
        _: u64,
        _: u64,
        _: i32,
        _: i32,
        _: i32,
        _: i32,
    ) -> windows::core::Result<()> {
        self.check_deadline()
    }

    fn UpdateTransferState(&self, _: u32) -> windows::core::Result<()> {
        self.check_deadline()
    }

    fn ConfirmOverwrite(
        &self,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        Err(WindowsError::from_hresult(COPYENGINE_E_USER_CANCELLED))
    }

    fn ConfirmEncryptionLoss(&self, _: Ref<IShellItem>) -> windows::core::Result<()> {
        Err(WindowsError::from_hresult(COPYENGINE_E_USER_CANCELLED))
    }

    fn FileFailure(
        &self,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        error: HRESULT,
        _: PWSTR,
        _: u32,
    ) -> windows::core::Result<()> {
        Err(failure(error))
    }

    fn SubStreamFailure(
        &self,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        error: HRESULT,
    ) -> windows::core::Result<()> {
        Err(failure(error))
    }

    fn PropertyFailure(
        &self,
        _: Ref<IShellItem>,
        _: *const PROPERTYKEY,
        error: HRESULT,
    ) -> windows::core::Result<()> {
        Err(failure(error))
    }
}

impl SilentTransferSink_Impl {
    fn check_deadline(&self) -> windows::core::Result<()> {
        if self.cancelled.load(Ordering::Acquire) || Instant::now() >= self.deadline {
            Err(WindowsError::from_hresult(COPYENGINE_E_USER_CANCELLED))
        } else {
            Ok(())
        }
    }
}

fn failure(error: HRESULT) -> WindowsError {
    WindowsError::from_hresult(if error.is_err() {
        error
    } else {
        COPYENGINE_E_USER_CANCELLED
    })
}

#[cfg(test)]
fn require_hosted_guards() -> Result<()> {
    ensure!(
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && std::env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted")
            && std::env::var("DESK_SWITCH_NATIVE_CLIPBOARD_TEST").as_deref() == Ok("1"),
        "native recycling fixtures require an explicitly authorized disposable hosted runner"
    );
    Ok(())
}

#[cfg(test)]
pub(super) fn require_test_quarantine_scope(directory: &Dir, fixture: &Path) -> Result<()> {
    require_hosted_guards()?;
    let fixture = fixture.canonicalize()?;
    let fixture = ordinary_dos_path(&fixture.as_os_str().encode_wide().collect::<Vec<_>>())?;
    let path = handle_path(directory)?;
    let relative = path
        .strip_prefix(&fixture)
        .context("the native quarantine handle is outside the authorized fresh fixture")?;
    ensure!(
        relative.components().next().is_some()
            && relative
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "native tests may mutate only a child of their fresh fixture"
    );
    let fixture_pins = pin_directory_path(&fixture)?;
    let directory_pins = pin_directory_path(&path)?;
    let actual = identity(&directory.dir_metadata()?)?;
    ensure!(
        directory_pins.identity() == actual
            && native_identity(directory)?.0 == actual
            && fixture_pins.identity().0 == actual.0,
        "the scoped native quarantine handle changed identity or volume"
    );
    Ok(())
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::OpenOptions;
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture {
        path: PathBuf,
        directory: Dir,
        token: String,
    }

    fn new_fixture(kind: &str) -> Result<Fixture> {
        // These checks deliberately precede RUNNER_TEMP resolution, directory
        // creation and every native API used by either fixture.
        require_hosted_guards()?;
        let runner =
            PathBuf::from(std::env::var_os("RUNNER_TEMP").context("RUNNER_TEMP is missing")?);
        let runner = runner.canonicalize()?;
        let runner = ordinary_dos_path(&runner.as_os_str().encode_wide().collect::<Vec<_>>())?;
        let runner_directory = open_absolute_dir(&runner, false)?;
        let value = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
            ^ ((std::process::id() as u128) << 64);
        let token = format!("{value:032x}");
        let name = format!("desk-switch-{kind}-{}-{token}", std::process::id());
        runner_directory.create_dir(&name)?;
        let directory = runner_directory.open_dir_nofollow(&name)?;
        let path = runner.join(name);
        ensure!(
            native_identity(&directory)?.0 == identity(&directory.dir_metadata()?)?,
            "the fresh hosted fixture changed identity"
        );
        Ok(Fixture {
            path,
            directory,
            token,
        })
    }

    #[derive(Debug, Eq, PartialEq)]
    struct AclSnapshot {
        owner: String,
        control: u16,
        bytes: Vec<u8>,
    }

    fn acl_snapshot(directory: &Dir, fixture: &Path) -> Result<AclSnapshot> {
        require_test_quarantine_scope(directory, fixture)?;
        let mut owner = PSID::default();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let result = unsafe {
            GetSecurityInfo(
                HANDLE(directory.as_raw_handle()),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                Some(&mut dacl),
                None,
                Some(&mut descriptor),
            )
        };
        let _allocation = LocalAllocation(descriptor.0);
        ensure!(
            result.0 == 0 && !dacl.is_null() && unsafe { IsValidAcl(dacl) }.as_bool(),
            "the fixture ACL could not be captured"
        );
        let size = unsafe { (*dacl).AclSize } as usize;
        ensure!(
            (size_of::<ACL>()..=65535).contains(&size),
            "invalid fixture ACL size"
        );
        let bytes = unsafe { std::slice::from_raw_parts(dacl.cast::<u8>(), size) }.to_vec();
        let mut control = 0u16;
        let mut revision = 0;
        unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }?;
        ensure!(revision == 1, "unsupported fixture security revision");
        Ok(AclSnapshot {
            owner: sid_string(owner)?,
            control,
            bytes,
        })
    }

    fn make_permissive_fixture_parent(directory: &Dir, fixture: &Path) -> Result<()> {
        require_test_quarantine_scope(directory, fixture)?;
        require_empty_directory(directory)?;
        let expected = native_identity(directory)?.0;
        let user = process_user_sid()?;
        let handle = unsafe {
            ReOpenFile(
                HANDLE(directory.as_raw_handle()),
                FILE_READ_ATTRIBUTES.0 | READ_CONTROL.0 | WRITE_DAC.0,
                FILE_SHARE_READ,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            )
        }?;
        let exact = unsafe { File::from_raw_handle(handle.0) };
        let owner = owner_sid(&exact)?;
        ensure!(
            native_identity(&exact)?.0 == expected && (owner == user || owner == "S-1-5-32-544"),
            "unowned ACL fixture parent"
        );
        let text = format!("D:P(A;OICI;FA;;;{user})(A;OICI;FA;;;SY)(A;OICI;FA;;;WD)");
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(wide.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }?;
        let _allocation = LocalAllocation(descriptor.0);
        let mut present = BOOL::default();
        let mut defaulted = BOOL::default();
        let mut dacl = std::ptr::null_mut();
        unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) }?;
        ensure!(present.as_bool() && !dacl.is_null(), "missing fixture DACL");
        require_empty_directory(directory)?;
        let result = unsafe {
            SetSecurityInfo(
                HANDLE(exact.as_raw_handle()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(dacl.cast_const()),
                None,
            )
        };
        ensure!(
            result.0 == 0 && native_identity(&exact)?.0 == expected,
            "could not initialize fresh fixture parent ACL"
        );
        Ok(())
    }

    fn write_proof(name: &str, proof: &serde_json::Value) -> Result<()> {
        require_hosted_guards()?;
        let runner =
            PathBuf::from(std::env::var_os("RUNNER_TEMP").context("RUNNER_TEMP is missing")?)
                .canonicalize()?;
        let runner = ordinary_dos_path(&runner.as_os_str().encode_wide().collect::<Vec<_>>())?;
        let proof_path = PathBuf::from(
            std::env::var_os("DESK_SWITCH_NATIVE_TRASH_PROOF_DIR")
                .context("native Trash proof directory is missing")?,
        );
        let parent = proof_path
            .parent()
            .context("proof directory has no parent")?
            .canonicalize()?;
        let parent = ordinary_dos_path(&parent.as_os_str().encode_wide().collect::<Vec<_>>())?;
        let leaf = proof_path
            .file_name()
            .and_then(OsStr::to_str)
            .context("invalid native proof directory")?;
        ensure!(
            parent == runner
                && leaf.starts_with("desk-switch-")
                && leaf
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
            "native proof directory must be its own direct desk-switch-* child of RUNNER_TEMP"
        );
        let runner_directory = open_absolute_dir(&runner, false)?;
        match runner_directory.create_dir(leaf) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let proof_directory = runner_directory.open_dir_nofollow(leaf)?;
        identity(&proof_directory.dir_metadata()?)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = proof_directory.open_with(name, &options)?;
        file.write_all(&serde_json::to_vec_pretty(proof)?)?;
        file.sync_all()?;
        println!(
            "DESK_SWITCH_NATIVE_TRASH_PROOF {}",
            serde_json::to_string(proof)?
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires an explicitly authorized disposable hosted Windows runner"]
    fn hosted_windows_quarantine_acl_fixture() -> Result<()> {
        require_hosted_guards()?;
        let fixture = new_fixture("native-trash-acl")?;
        let proof = super::super::with_hosted_native_trash(&fixture.path, || -> Result<_> {
            fixture.directory.create_dir("permissive-parent")?;
            let parent = fixture.directory.open_dir_nofollow("permissive-parent")?;
            make_permissive_fixture_parent(&parent, &fixture.path)?;
            let before = acl_snapshot(&parent, &fixture.path)?;
            parent.create_dir("quarantine")?;
            let child = parent.open_dir_nofollow("quarantine")?;
            require_test_quarantine_scope(&child, &fixture.path)?;
            let child_identity = native_identity(&child)?.0;
            ensure!(
                verify_private_directory(&child).is_err(),
                "permissive inheritance was not observed"
            );
            super::super::protect_new_private_dir(&child)?;
            verify_private_directory(&child)?;
            let after = acl_snapshot(&parent, &fixture.path)?;
            ensure!(
                before == after,
                "protecting the new child changed its parent ACL"
            );
            ensure!(
                native_identity(&child)?.0 == child_identity,
                "the child changed identity"
            );
            Ok(serde_json::json!({
                "schema": 1, "kind": "windows_quarantine_acl", "status": "passed",
                "fixture": fixture.path, "parent_acl_unchanged": true,
                "parent_acl_sha256_before": format!("{:x}", Sha256::digest(&before.bytes)),
                "parent_acl_sha256_after": format!("{:x}", Sha256::digest(&after.bytes)),
                "child_private": true, "child_acl_protected": true,
                "child_identity": child_identity,
            }))
        })??;
        write_proof("acl.json", &proof)
    }

    #[test]
    #[ignore = "requires an explicitly authorized disposable hosted Windows runner"]
    fn hosted_windows_recycle_item_fixture() -> Result<()> {
        require_hosted_guards()?;
        let fixture = new_fixture("native-trash-recycle")?;
        let proof = super::super::with_hosted_native_trash(&fixture.path, || -> Result<_> {
            fixture.directory.create_dir("quarantine")?;
            let quarantine = fixture.directory.open_dir_nofollow("quarantine")?;
            require_test_quarantine_scope(&quarantine, &fixture.path)?;
            super::super::protect_new_private_dir(&quarantine)?;
            let bytes = b"DeskSwitch hosted-only recoverable recycling fixture\r\n\0\xff";
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            let mut source = fixture.directory.open_with("original.bin", &options)?;
            source.write_all(bytes)?;
            source.sync_all()?;
            let expected = identity(&source.metadata()?)?;
            drop(source);
            let original = fixture.path.join("original.bin");
            fixture
                .directory
                .rename("original.bin", &quarantine, PAYLOAD)?;
            ensure!(
                identity(&quarantine.symlink_metadata(PAYLOAD)?)? == expected,
                "the fixture detach changed identity"
            );
            let anchor = volume_anchor(&handle_path(&quarantine)?, expected.0)?;
            let bin_existed_before = existing_recycle_parent(&anchor)?.is_some();
            drop(anchor);
            let plan = super::super::prepare(&quarantine, &original, expected, &fixture.token)?;
            let receipt = super::super::commit(&quarantine, &plan)?;
            ensure_payload_absent(&quarantine)?;
            let bin_path = receipt
                .item
                .parent()
                .context("the receipt has no bin parent")?;
            let bin_pins = pin_directory_path(bin_path)?;
            let bin = open_absolute_dir(bin_path, false)?;
            ensure!(
                identity(&bin.dir_metadata()?)? == bin_pins.identity(),
                "the receipt parent changed"
            );
            let mut read_options = OpenOptions::new();
            read_options.read(true).follow(FollowSymlinks::No);
            let mut recycled = bin.open_with(
                receipt
                    .item
                    .file_name()
                    .context("the receipt has no item name")?,
                &read_options,
            )?;
            ensure!(
                identity(&recycled.metadata()?)? == expected,
                "the actual receipt file identity differs"
            );
            let mut actual = Vec::new();
            recycled.read_to_end(&mut actual)?;
            ensure!(
                actual.as_slice() == &bytes[..],
                "the actual recycled fixture bytes differ"
            );
            ensure!(
                identity(&recycled.metadata()?)? == expected,
                "the receipt changed while reading bytes"
            );
            let recovered = super::super::inspect(&quarantine, &plan)?;
            ensure!(
                matches!(recovered, TrashStatus::Committed(ref found) if found == &receipt),
                "the durable plan did not recover the exact native recycling receipt"
            );
            Ok(serde_json::json!({
                "schema": 1, "kind": "windows_recycle_item", "status": "passed",
                "fixture": fixture.path, "source_identity": expected,
                "source_absent": true, "recycled_item": receipt.item,
                "recycled_identity": receipt.item_identity,
                "byte_count": bytes.len(), "bytes_verified": true,
                "expected_sha256": format!("{:x}", Sha256::digest(bytes)),
                "recycled_sha256": format!("{:x}", Sha256::digest(&actual)),
                "volume_anchor": plan.target,
                "volume_anchor_identity": plan.target_parent_identity,
                "recovery_inspect": "Committed", "bin_existed_before": bin_existed_before,
                "native_bin_bootstrap_observed": !bin_existed_before,
            }))
        })??;
        // The recycled item is deliberately retained. There is no purge,
        // empty-bin operation or source cleanup in this fixture.
        write_proof("recycle.json", &proof)
    }
}
