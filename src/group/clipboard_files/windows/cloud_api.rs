//! Runtime-loaded Cloud Files API bindings.
//!
//! Referencing Cloud Files types does not require importing `CldApi.dll` at
//! process startup. All entry points are resolved from the Windows system
//! directory, so an unsupported host can reject this provider without preventing
//! the rest of the application from starting.

use anyhow::{Context, Result, anyhow, ensure};
use std::{
    ffi::{CStr, c_void},
    ptr,
    sync::Arc,
};
use windows::{
    Win32::{
        Foundation::HANDLE,
        Storage::CloudFilters::{
            CF_CALLBACK_REGISTRATION, CF_CONNECT_FLAGS, CF_CONNECTION_KEY, CF_CREATE_FLAGS,
            CF_HYDRATE_FLAGS, CF_OPERATION_INFO, CF_OPERATION_PARAMETERS,
            CF_PLACEHOLDER_CREATE_INFO, CF_PLATFORM_INFO, CF_REGISTER_FLAGS, CF_SYNC_POLICIES,
            CF_SYNC_REGISTRATION,
        },
        System::IO::OVERLAPPED,
    },
    core::{HRESULT, PCWSTR, w},
};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HMODULE},
    System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
};

// SAFETY: The erased export signature is only the source of a transmute into
// one of the concrete aliases below. The pointer always comes from
// GetProcAddress on the owned CldApi.dll loader reference, so it stays valid
// for as long as that reference is alive.
type Procedure = unsafe extern "system" fn() -> isize;
// SAFETY: This alias states the documented CfGetPlatformInfo signature. The
// native call only fills the caller-provided CF_PLATFORM_INFO and returns an
// HRESULT, so the caller must pass a live, writable structure.
type GetPlatformInfo = unsafe extern "system" fn(*mut CF_PLATFORM_INFO) -> HRESULT;
// SAFETY: This alias states the documented CfRegisterSyncRoot signature. The
// native call reads the registration and policy structures, and every pointer
// they embed, only for the duration of the call.
type RegisterSyncRoot = unsafe extern "system" fn(
    PCWSTR,
    *const CF_SYNC_REGISTRATION,
    *const CF_SYNC_POLICIES,
    CF_REGISTER_FLAGS,
) -> HRESULT;
// SAFETY: This alias states the documented CfUnregisterSyncRoot signature. Its
// only argument is a NUL-terminated path that the native call reads during the
// call and never retains.
type UnregisterSyncRoot = unsafe extern "system" fn(PCWSTR) -> HRESULT;
// SAFETY: This alias states the documented CfConnectSyncRoot signature. The
// callback table, the callback context and the connection key must stay valid
// for the whole call and for as long as the connection exists.
type ConnectSyncRoot = unsafe extern "system" fn(
    PCWSTR,
    *const CF_CALLBACK_REGISTRATION,
    *const c_void,
    CF_CONNECT_FLAGS,
    *mut CF_CONNECTION_KEY,
) -> HRESULT;
// SAFETY: This alias states the documented CfDisconnectSyncRoot signature,
// whose only argument is the connection key that the matching connect call
// returned.
type DisconnectSyncRoot = unsafe extern "system" fn(CF_CONNECTION_KEY) -> HRESULT;
// SAFETY: This alias states the documented CfCreatePlaceholders signature. The
// batch array, its entry count and the processed counter must all stay valid
// and writable for the whole call.
type CreatePlaceholders = unsafe extern "system" fn(
    PCWSTR,
    *mut CF_PLACEHOLDER_CREATE_INFO,
    u32,
    CF_CREATE_FLAGS,
    *mut u32,
) -> HRESULT;
// SAFETY: This alias states the documented CfExecute signature. The operation
// info and parameters must describe the selected operation and every pointer
// they embed must stay valid for the whole call.
type Execute = unsafe extern "system" fn(
    // The native call reads both structures for the whole call.
    *const CF_OPERATION_INFO,
    *mut CF_OPERATION_PARAMETERS,
) -> HRESULT;
// SAFETY: This alias states the documented CfReportProviderProgress signature.
// Every argument is a plain value that the native call only reads; no pointer
// is passed or retained.
type ReportProviderProgress = unsafe extern "system" fn(
    // The native call reads the connection key and the three progress values.
    CF_CONNECTION_KEY,
    i64,
    i64,
    i64,
) -> HRESULT;
// SAFETY: This alias states the documented CfHydratePlaceholder signature. The
// file handle must be valid for hydration and the optional overlapped
// structure must stay valid until an asynchronous hydration has finished.
type HydratePlaceholder = unsafe extern "system" fn(
    // The handle and the overlapped structure stay valid for the whole call.
    HANDLE,
    i64,
    i64,
    CF_HYDRATE_FLAGS,
    *mut OVERLAPPED,
) -> HRESULT;

struct Module(HMODULE);

// A module handle has no thread affinity. This owner holds one loader reference,
// never changes the handle, and releases it exactly once. An Arc<Api> keeps that
// reference alive while its immutable entry points are called on any thread.
// SAFETY: The owned loader reference has no thread affinity and the handle is
// never mutated, so the owner can be moved to another thread; the module stays
// loaded until the owner's Drop releases that one reference.
unsafe impl Send for Module {}
// SAFETY: The owner only holds an immutable loader reference whose exported
// entry points may be called concurrently, so sharing it across threads cannot
// introduce a data race.
unsafe impl Sync for Module {}

impl Module {
    fn load() -> Result<Self> {
        // Restrict DLL lookup to System32; never load a similarly named library
        // from the working directory or an application-controlled search path.
        // SAFETY: The library name is a literal and the lookup is restricted to
        // System32, so the call can only load the genuine system library; the
        // returned handle owns exactly one loader reference that the Module
        // owner releases in its Drop.
        let handle = unsafe {
            LoadLibraryExW(
                w!("CldApi.dll").as_ptr(),
                ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error()).context(
                "Windows Cloud Files is unavailable: could not load CldApi.dll from System32",
            );
        }
        Ok(Self(handle))
    }

    fn optional(&self, name: &CStr) -> Option<Procedure> {
        // The returned pointer is used only while this module's loader reference
        // is alive. Each caller supplies the exact documented export name.
        // SAFETY: `name` is a NUL-terminated export name that outlives the
        // call, and the returned address stays valid while this module's
        // loader reference is alive because the module is never unloaded early.
        unsafe { GetProcAddress(self.0, name.as_ptr().cast()) }
    }

    fn required(&self, name: &CStr) -> Result<Procedure> {
        self.optional(name).ok_or_else(|| {
            anyhow!(
                "Windows Cloud Files is unavailable: CldApi.dll does not export {}",
                name.to_string_lossy()
            )
        })
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        // There is no recovery action for a loader-reference release failure.
        // Connections and outstanding operations must have ended before the last
        // Arc<Api> is dropped; that obligation is documented on connect below.
        // SAFETY: The handle owns exactly the one loader reference taken by
        // LoadLibraryExW and this Drop runs exactly once, so the reference is
        // released once and no other path frees it.
        unsafe { FreeLibrary(self.0) };
    }
}

/// Immutable entry points backed by one owned system-library reference.
///
/// A connected provider must retain an `Arc<Api>` until disconnection and all of
/// its outstanding operations have finished. This wrapper does not register,
/// connect, install, start, or enable any service merely by being constructed.
pub struct Api {
    get_platform_info: GetPlatformInfo,
    register_sync_root: RegisterSyncRoot,
    unregister_sync_root: UnregisterSyncRoot,
    connect_sync_root: ConnectSyncRoot,
    disconnect_sync_root: DisconnectSyncRoot,
    create_placeholders: CreatePlaceholders,
    execute: Execute,
    report_provider_progress: ReportProviderProgress,
    hydrate_placeholder: Option<HydratePlaceholder>,
    _module: Module,
}

impl Api {
    /// Load the supported system exports and perform a read-only platform probe.
    ///
    /// Registration and file-system support are checked by the later operations;
    /// no driver or service configuration is changed when this probe fails.
    pub fn new() -> Result<Arc<Self>> {
        let module = Module::load()?;

        // Every conversion below is between function pointers with the same
        // representation. Its concrete signature matches cfapi.h and the
        // windows 0.62 CloudFilters bindings for that exact export name.
        macro_rules! required {
            ($name:expr, $signature:ty) => {{
                let procedure = module.required($name)?;
                // SAFETY: The export was resolved from the owned CldApi.dll
                // module, and the target alias at this call site declares
                // exactly the documented cfapi.h signature of that export, so
                // the transmute yields a pointer with the native ABI and
                // calling convention.
                unsafe { std::mem::transmute::<Procedure, $signature>(procedure) }
            }};
        }
        macro_rules! optional {
            ($name:expr, $signature:ty) => {
                // SAFETY: The export was resolved from the owned CldApi.dll
                // module, and the target alias at this call site declares
                // exactly the documented cfapi.h signature of that export, so
                // the transmute yields a pointer with the native ABI and
                // calling convention.
                module.optional($name).map(|procedure| unsafe {
                    std::mem::transmute::<Procedure, $signature>(procedure)
                })
            };
        }

        let api = Arc::new(Self {
            get_platform_info: required!(c"CfGetPlatformInfo", GetPlatformInfo),
            register_sync_root: required!(c"CfRegisterSyncRoot", RegisterSyncRoot),
            unregister_sync_root: required!(c"CfUnregisterSyncRoot", UnregisterSyncRoot),
            connect_sync_root: required!(c"CfConnectSyncRoot", ConnectSyncRoot),
            disconnect_sync_root: required!(c"CfDisconnectSyncRoot", DisconnectSyncRoot),
            create_placeholders: required!(c"CfCreatePlaceholders", CreatePlaceholders),
            execute: required!(c"CfExecute", Execute),
            report_provider_progress: required!(
                c"CfReportProviderProgress",
                ReportProviderProgress
            ),
            hydrate_placeholder: optional!(c"CfHydratePlaceholder", HydratePlaceholder),
            _module: module,
        });
        api.get_platform_info()
            .context("The Windows Cloud Files platform is unavailable")?;
        Ok(api)
    }

    pub fn get_platform_info(&self) -> Result<CF_PLATFORM_INFO> {
        let mut info = CF_PLATFORM_INFO::default();
        // SAFETY: `info` is a live, writable CF_PLATFORM_INFO of the exact
        // native size, and the native call only fills that structure and
        // returns an HRESULT.
        let status = unsafe { (self.get_platform_info)(&mut info) };
        check(status, "CfGetPlatformInfo")?;
        Ok(info)
    }

    /// # Safety
    /// `path` must be a valid NUL-terminated path. All pointers embedded in the
    /// registration and policy structures must be valid for the native call.
    // SAFETY: The caller guarantees that `path` is a NUL-terminated path and
    // that the registration and policy structures, including every pointer
    // they embed, stay valid for the whole native call.
    pub unsafe fn register_sync_root(
        &self,
        path: PCWSTR,
        registration: &CF_SYNC_REGISTRATION,
        policies: &CF_SYNC_POLICIES,
        flags: CF_REGISTER_FLAGS,
    ) -> Result<()> {
        // SAFETY: The caller guarantees that `path` is NUL-terminated and that
        // the registration and policy structures and every pointer they embed
        // stay valid for the whole call.
        let status = unsafe { (self.register_sync_root)(path, registration, policies, flags) };
        check(status, "CfRegisterSyncRoot")
    }

    /// # Safety
    /// `path` must be a valid NUL-terminated path of a sync root owned by the
    /// caller, with connections and outstanding operations already finished.
    // SAFETY: The caller guarantees that `path` is a NUL-terminated path of a
    // sync root it owns and that no connection or outstanding operation
    // remains on that root.
    pub unsafe fn unregister_sync_root(&self, path: PCWSTR) -> Result<()> {
        // SAFETY: The caller guarantees that `path` is a valid NUL-terminated
        // path of the sync root it owns and that no connection or outstanding
        // operation remains on it.
        let status = unsafe { (self.unregister_sync_root)(path) };
        check(status, "CfUnregisterSyncRoot")
    }

    /// # Safety
    /// `path` must be a valid NUL-terminated registered sync-root path.
    /// `callback_table` must be valid and terminated by
    /// `CF_CALLBACK_REGISTRATION_END`. Callback code, its context, and an
    /// `Arc<Api>` must remain alive until successful disconnection completes and
    /// outstanding operations have finished. Callbacks may arrive before this
    /// call returns and during `disconnect_sync_root`; context must support that
    /// concurrency and callbacks must not unwind across their native boundary.
    // SAFETY: The caller guarantees that the callback table is terminated by
    // CF_CALLBACK_REGISTRATION_END and that the callback context, its code and
    // an Arc<Api> stay alive until disconnection and every outstanding
    // operation have finished.
    pub unsafe fn connect_sync_root(
        &self,
        path: PCWSTR,
        callback_table: *const CF_CALLBACK_REGISTRATION,
        callback_context: *const c_void,
        flags: CF_CONNECT_FLAGS,
    ) -> Result<CF_CONNECTION_KEY> {
        let mut key = CF_CONNECTION_KEY::default();
        // SAFETY: The caller guarantees that the callback table is terminated
        // and that the callback context stays alive for the whole call; `key`
        // is a live local that the native call fills with the new connection
        // key.
        let status = unsafe {
            (self.connect_sync_root)(path, callback_table, callback_context, flags, &mut key)
        };
        check(status, "CfConnectSyncRoot")?;
        Ok(key)
    }

    /// The callback context must remain alive throughout this call, including
    /// callbacks delivered while Windows is disconnecting the connection.
    pub fn disconnect_sync_root(&self, key: CF_CONNECTION_KEY) -> Result<()> {
        // SAFETY: The key was returned by a successful connect_sync_root call on
        // this Api, and the caller keeps the callback context alive until the
        // disconnection and the callbacks delivered during it have completed.
        let status = unsafe { (self.disconnect_sync_root)(key) };
        check(status, "CfDisconnectSyncRoot")
    }

    /// Create a batch and return its processed-entry count.
    ///
    /// Entries remain available to the caller, including their individual
    /// HRESULT values, when a partially successful batch returns an error.
    ///
    /// # Safety
    /// `base_directory` and all paths and identity buffers referenced by entries
    /// must be valid for this call. The base directory must belong to a sync root
    /// owned by the caller; entry paths must already have been validated.
    // SAFETY: The caller guarantees that `base_directory` and every path and
    // identity buffer referenced by the entries stay valid for this call.
    pub unsafe fn create_placeholders(
        &self,
        base_directory: PCWSTR,
        placeholders: &mut [CF_PLACEHOLDER_CREATE_INFO],
        flags: CF_CREATE_FLAGS,
    ) -> Result<u32> {
        let count = u32::try_from(placeholders.len())
            .context("Too many Windows Cloud Files placeholders in one batch")?;
        let mut processed = 0;
        // SAFETY: `placeholders` is a live slice of exactly `count` entries whose
        // embedded paths and identity buffers the caller guarantees to be valid,
        // and `processed` is a live u32 that the native call may write to.
        let status = unsafe {
            (self.create_placeholders)(
                base_directory,
                placeholders.as_mut_ptr(),
                count,
                flags,
                &mut processed,
            )
        };
        check(status, "CfCreatePlaceholders").with_context(|| {
            format!("Cloud Files processed {processed} of {count} placeholder entries")
        })?;
        ensure!(
            processed == count,
            "CfCreatePlaceholders processed {processed} of {count} placeholder entries"
        );
        for (index, placeholder) in placeholders.iter().enumerate() {
            check(placeholder.Result, "CfCreatePlaceholders entry")
                .with_context(|| format!("Cloud Files placeholder entry {index} failed"))?;
        }
        Ok(processed)
    }

    /// # Safety
    /// The selected operation, structure sizes, union arm, and all embedded
    /// pointers must match the native operation contract. Referenced data must
    /// remain valid for the duration required by that operation.
    // SAFETY: The caller guarantees that the operation info, the parameters and
    // every pointer they embed match the selected native operation and stay
    // valid for the whole call.
    pub unsafe fn execute(
        &self,
        info: &CF_OPERATION_INFO,
        parameters: &mut CF_OPERATION_PARAMETERS,
    ) -> Result<()> {
        // SAFETY: The caller has guaranteed that `info` and `parameters` match
        // the selected native operation and that every embedded pointer stays
        // valid for the duration the operation requires.
        let status = unsafe { (self.execute)(info, parameters) };
        check(status, "CfExecute")
    }

    pub fn report_provider_progress(
        &self,
        connection_key: CF_CONNECTION_KEY,
        transfer_key: i64,
        total: i64,
        completed: i64,
    ) -> Result<()> {
        // SAFETY: The native call only reads the connection key and the
        // three progress values; no pointer is passed or retained.
        let status = unsafe {
            (self.report_provider_progress)(connection_key, transfer_key, total, completed)
        };
        check(status, "CfReportProviderProgress")
    }

    /// # Safety
    /// `handle` must be valid for hydration. If an overlapped pointer is passed,
    /// its storage, event, handle, and an `Arc<Api>` must remain alive until the
    /// asynchronous operation has finished, including when it reports pending.
    // SAFETY: The caller guarantees that the handle is valid for hydration and
    // that the optional overlapped storage, its event and an Arc<Api> stay
    // alive until an asynchronous hydration has finished.
    pub unsafe fn hydrate_placeholder(
        &self,
        handle: HANDLE,
        offset: i64,
        length: i64,
        flags: CF_HYDRATE_FLAGS,
        overlapped: Option<*mut OVERLAPPED>,
    ) -> Result<()> {
        let function = self
            .hydrate_placeholder
            .ok_or_else(|| anyhow!("Windows Cloud Files does not provide CfHydratePlaceholder"))?;
        // SAFETY: The caller guarantees that the handle is valid for hydration
        // and that the optional overlapped storage and the Api stay alive until
        // an asynchronous hydration has finished.
        let status = unsafe {
            function(
                handle,
                offset,
                length,
                flags,
                overlapped.unwrap_or(ptr::null_mut()),
            )
        };
        check(status, "CfHydratePlaceholder")
    }
}

fn check(status: HRESULT, operation: &str) -> Result<()> {
    status
        .ok()
        .with_context(|| format!("{operation} failed (HRESULT 0x{:08X})", status.0 as u32))
}
