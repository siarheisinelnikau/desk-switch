//! Actual Shell discovery proof on a disposable GitHub-hosted Windows desktop.
//! This test does not perform a paste or certify file transfer/source cleanup.

use super::*;
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    os::windows::fs::MetadataExt,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use windows::Win32::{
    Foundation::VARIANT_BOOL,
    System::Com::{
        COINIT_APARTMENTTHREADED, CoAllowSetForegroundWindow, CoInitializeEx, CoUninitialize,
    },
    UI::Shell::{IWebBrowser2, ShellBrowserWindow},
};
use windows::core::BSTR;
use windows_sys::Win32::{
    Foundation::CloseHandle,
    System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    },
    UI::WindowsAndMessaging::{
        IsWindow, PostMessageW, SW_RESTORE, SetForegroundWindow, ShowWindowAsync, WM_CLOSE,
    },
};

const LIMIT: Duration = Duration::from_secs(30);
const MARKER: &str = ".desk-switch-explorer-ci-owner";

struct ExplorerApartment;
impl ExplorerApartment {
    fn enter() -> Option<Self> {
        // SAFETY: This proof worker thread has not initialized COM before, so
        // this call starts its single-threaded apartment; every successful
        // CoInitializeEx must be balanced by one CoUninitialize on the same
        // thread, and the returned guard is dropped there.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .ok()?;
        Some(Self)
    }
}
impl Drop for ExplorerApartment {
    fn drop(&mut self) {
        // SAFETY: This guard exists only after a successful CoInitializeEx on
        // this thread, and it is dropped exactly once here, so the apartment
        // initialization count stays balanced.
        unsafe { CoUninitialize() };
    }
}

fn require_hosted_desktop() {
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(
        std::env::var("RUNNER_ENVIRONMENT").as_deref(),
        Ok("github-hosted"),
        "Explorer capture tests must never run on a user or self-hosted desktop"
    );
    assert_eq!(
        std::env::var("DESK_SWITCH_NATIVE_CLIPBOARD_TEST").as_deref(),
        Ok("1"),
        "Explorer capture tests require the disposable native-CI opt-in"
    );
}

#[derive(Clone)]
struct Fixture {
    root: PathBuf,
    folder: PathBuf,
    token: String,
}
impl Fixture {
    fn create() -> Result<Self> {
        let temporary = PathBuf::from(
            std::env::var_os("RUNNER_TEMP")
                .context("The hosted runner temporary directory is missing")?,
        )
        .canonicalize()?;
        ensure!(
            temporary.is_absolute() && temporary.is_dir(),
            "Invalid hosted temporary directory"
        );
        let token = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        );
        let root = temporary.join(format!("desk-switch-explorer-ci-{token}"));
        fs::create_dir(&root)?;
        let root = root.canonicalize()?;
        ensure!(
            root.parent() == Some(temporary.as_path()),
            "The Explorer fixture escaped RUNNER_TEMP"
        );
        fs::write(root.join(MARKER), token.as_bytes())?;
        let folder = root.join("destination");
        fs::create_dir(&folder)?;
        let fixture = Self {
            root,
            folder,
            token,
        };
        fixture.validate()?;
        Ok(fixture)
    }
    fn validate(&self) -> Result<()> {
        let temporary =
            PathBuf::from(std::env::var_os("RUNNER_TEMP").context("Missing RUNNER_TEMP")?)
                .canonicalize()?;
        ensure!(
            self.root.is_absolute()
                && self.root.parent() == Some(temporary.as_path())
                && self.root.canonicalize()? == self.root
                && self.root.file_name().is_some_and(|name| name
                    .to_string_lossy()
                    .starts_with("desk-switch-explorer-ci-")),
            "The Explorer fixture was redirected"
        );
        for path in [&self.root, &self.folder] {
            let metadata = fs::symlink_metadata(path)?;
            ensure!(
                metadata.is_dir() && metadata.file_attributes() & 0x400 == 0,
                "The Explorer fixture contains a redirected directory"
            );
        }
        ensure!(
            self.folder.parent() == Some(self.root.as_path())
                && fs::read(self.root.join(MARKER))? == self.token.as_bytes(),
            "The Explorer fixture ownership changed"
        );
        Ok(())
    }
}

#[derive(Clone)]
struct Window {
    hwnd: usize,
    process: u32,
    folder: PathBuf,
}
#[derive(Default)]
struct Control {
    thread: u32,
    owned: Option<Window>,
}
struct OwnedBrowser {
    browser: IWebBrowser2,
    fixture: Fixture,
    known: Option<Window>,
}
impl OwnedBrowser {
    fn close(&mut self) -> Result<()> {
        let Some(window) = self.known.clone() else {
            return Ok(());
        };
        self.fixture.validate()?;
        ensure!(
            window.folder == self.fixture.folder && window_matches(&window),
            "The test Explorer window changed identity"
        );
        ensure!(
            folder_of(&self.browser).is_some_and(|path| same_path(&path, &self.fixture.folder)),
            "The test Explorer window no longer displays its owned folder"
        );
        // SAFETY: `self.browser` is the live `IWebBrowser2` pointer owned by
        // this struct; it was created by `CoCreateInstance` in the apartment
        // this thread entered and is only used from that thread, so the vtable
        // call targets a live object. `Quit` asks that window to close and
        // transfers no ownership.
        unsafe { self.browser.Quit() }.context("Could not close the owned Explorer window")?;
        let deadline = Instant::now() + Duration::from_secs(3);
        while window_matches(&window) {
            ensure!(
                Instant::now() < deadline,
                "The owned Explorer window did not close"
            );
            thread::sleep(Duration::from_millis(20));
        }
        self.known = None;
        Ok(())
    }
}
impl Drop for OwnedBrowser {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_lowercase()
        == right
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_lowercase()
}
fn window_matches(window: &Window) -> bool {
    let handle = window.hwnd as HWND;
    let mut process = 0;
    // SAFETY: `handle` is the Explorer window handle recorded from the owned
    // browser, and `IsWindow` only tests that the handle still refers to a
    // window; it neither closes nor transfers the handle.
    (unsafe { IsWindow(handle) }) != 0
        // SAFETY: `process` is a live `u32` local that outlives the call and
        // receives the owning process id; `handle` is a borrowed window handle
        // that is not closed here.
        && unsafe { GetWindowThreadProcessId(handle, &mut process) } != 0
        && process == window.process
}
fn explorer_process(process: u32) -> Result<()> {
    // SAFETY: `process` is the id of the Explorer process that owns the
    // browser window this test created, and the call requests only
    // `PROCESS_QUERY_LIMITED_INFORMATION`; a non-null handle is owned by this
    // function and closed exactly once by `CloseHandle` below, while a null
    // handle takes the early return that owns nothing.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process) };
    ensure!(
        !handle.is_null(),
        "Could not inspect the owned Explorer process"
    );
    let mut name = vec![0u16; MAX_PATH];
    let mut count = name.len() as u32;
    // SAFETY: `name` holds exactly `count` UTF-16 units, which is the capacity
    // the call is told it may write, and `handle` is the live process handle
    // opened above; the call writes only into `name` and only reads `handle`.
    let result = unsafe { QueryFullProcessImageNameW(handle, 0, name.as_mut_ptr(), &mut count) };
    // SAFETY: `handle` is the unique process handle returned by `OpenProcess`
    // above; it is not stored anywhere else and is closed exactly once here.
    unsafe { CloseHandle(handle) };
    ensure!(
        result != 0,
        "Could not resolve the owned Explorer executable"
    );
    let path = PathBuf::from(OsString::from_wide(&name[..count as usize]));
    ensure!(
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("explorer.exe")),
        "ShellBrowserWindow was not hosted by Explorer.exe"
    );
    Ok(())
}
fn folder_of(browser: &IWebBrowser2) -> Option<PathBuf> {
    let service: IServiceProvider = browser.cast().ok()?;
    // SAFETY: `service` is the live IServiceProvider interface pointer of the
    // owned Explorer browser, so the query targets a valid object; the returned
    // IShellBrowser pointer is owned by the `browser` binding and is released
    // when that binding goes out of scope.
    let browser: IShellBrowser = unsafe { service.QueryService(&SID_STopLevelBrowser) }.ok()?;
    // SAFETY: `browser` is a live IShellBrowser pointer owned by this scope, so
    // the call targets a valid object; the returned IShellView pointer is owned
    // by the `view` binding and is released at the end of this scope.
    let view = unsafe { browser.QueryActiveShellView() }.ok()?;
    let view: IFolderView = view.cast().ok()?;
    // SAFETY: `view` is the live IFolderView pointer of the Explorer's active
    // shell view, so the call targets a valid object; the returned
    // IPersistFolder2 pointer is owned by the `folder` binding.
    let folder: IPersistFolder2 = unsafe { view.GetFolder() }.ok()?;
    // SAFETY: `folder` is a live IPersistFolder2 pointer, and GetCurFolder
    // transfers an ITEMIDLIST allocated with CoTaskMemAlloc to this code; the
    // `Pidl` guard frees it exactly once with the matching allocator.
    let item = Pidl(unsafe { folder.GetCurFolder() }.ok()?);
    if item.0.is_null() {
        return None;
    }
    let mut buffer = vec![0u16; MAX_PATH];
    // SAFETY: `buffer` holds MAX_PATH UTF-16 units and the call is told that
    // capacity, so it writes at most that many units into it; `item.0` is the
    // ITEMIDLIST owned by the `item` guard, which stays alive for the call and
    // is only read here, never freed.
    if !unsafe { SHGetPathFromIDListEx(item.0, &mut buffer, GPFIDL_DEFAULT) }.as_bool() {
        return None;
    }
    let end = buffer.iter().position(|unit| *unit == 0)?;
    Some(PathBuf::from(OsString::from_wide(&buffer[..end])))
}

fn run_capture(fixture: Fixture, control: Arc<Mutex<Control>>) -> Result<serde_json::Value> {
    // ShellBrowserWindow is an STA shell object; an MTA proxy can be created
    // but fails navigation with RPC_X_NULL_REF_POINTER on a hosted runner.
    let _apartment =
        ExplorerApartment::enter().context("The hosted Explorer COM apartment is unavailable")?;
    let _cancel =
        Cancellation::enter().context("The hosted Explorer COM calls cannot be bounded")?;
    // SAFETY: The call takes no pointer arguments and returns the id of the
    // calling thread, which is this worker thread that owns the Explorer COM
    // apartment; the value is only used to target `CoCancelCall` at that
    // apartment.
    control.lock().unwrap().thread = unsafe { GetCurrentThreadId() };
    fixture.validate()?;
    let browser: IWebBrowser2 =
        // SAFETY: `ExplorerApartment::enter` above put this thread into a
        // single-threaded apartment before this call, so the returned
        // ShellBrowserWindow interface pointer is bound to that apartment; it
        // is owned by the `browser` binding, used only from this thread, and
        // released before the apartment guard drops.
        unsafe { CoCreateInstance(&ShellBrowserWindow, None, CLSCTX_LOCAL_SERVER) }.context(
            "The hosted runner has no available interactive Explorer ShellBrowserWindow",
        )?;
    let mut owned = OwnedBrowser {
        browser,
        fixture: fixture.clone(),
        known: None,
    };
    // Explorer's Shell parser does not accept the verbatim `\\?\` prefix that
    // canonicalize() returns. Navigate with the ordinary DOS spelling; the
    // captured folder is normalized before comparison.
    let folder_text = fixture.folder.to_string_lossy();
    let folder_text = folder_text.strip_prefix(r"\\?\").unwrap_or(&folder_text);
    let url = format!("file:///{}", folder_text.replace('\\', "/"));
    let target = BSTR::from(url.as_str());
    // SAFETY: `owned.browser` is the live IWebBrowser2 pointer owned by the
    // `owned` binding, created in this thread's apartment; the call only shows
    // the window and transfers no ownership.
    let _ = unsafe { owned.browser.SetVisible(VARIANT_BOOL(-1)) };
    // SAFETY: `owned.browser` is the live IWebBrowser2 pointer owned by the
    // `owned` binding and used from the apartment that created it; `target` is a
    // live BSTR that owns the NUL-terminated URL and outlives the call, and the
    // remaining arguments are None, so no other pointer is dereferenced.
    unsafe { owned.browser.Navigate(&target, None, None, None, None) }
        .context("Explorer could not navigate to the private test folder")?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        fixture.validate()?;
        if let Some(path) = folder_of(&owned.browser)
            && same_path(&path, &fixture.folder)
        {
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "Explorer did not open the private test folder; no desktop/session changes were attempted"
        );
        thread::sleep(Duration::from_millis(25));
    }
    // SAFETY: `owned.browser` is the live IWebBrowser2 pointer owned by the
    // `owned` binding; the call only reads the Explorer window handle, which
    // stays owned by Explorer and is not closed by this test.
    let hwnd = unsafe { owned.browser.HWND() }
        .context("Explorer has no window")?
        .0 as usize;
    ensure!(hwnd != 0, "Explorer did not create a window");
    let mut process = 0;
    ensure!(
        // SAFETY: `process` is a live `u32` local that outlives the call and receives
        // the owning process id; `hwnd` is the Explorer window handle the owned
        // browser reported, carried unchanged through `usize`, so the cast back to
        // the Win32 handle type recovers the same handle value.
        unsafe { GetWindowThreadProcessId(hwnd as HWND, &mut process) } != 0 && process != 0,
        "Explorer window has no process identity"
    );
    explorer_process(process)?;
    let window = Window {
        hwnd,
        process,
        folder: fixture.folder.clone(),
    };
    owned.known = Some(window.clone());
    control.lock().unwrap().owned = Some(window);
    // SAFETY: `&owned.browser` is a live IWebBrowser2 interface pointer owned by
    // the `owned` binding and valid for the duration of the call; the reserved
    // argument is None, so the call only grants foreground rights to that
    // window and takes no ownership.
    let _ = unsafe { CoAllowSetForegroundWindow(&owned.browser, None) };
    // SAFETY: `owned.browser` is the live IWebBrowser2 pointer owned by the
    // `owned` binding, created in this thread's apartment; the call only shows
    // the window and transfers no ownership.
    unsafe { owned.browser.SetVisible(VARIANT_BOOL(-1)) }
        .context("Could not display the owned Explorer window")?;
    // SAFETY: `hwnd` is the Explorer window handle the owned browser reported,
    // carried unchanged through `usize`; the call only requests an asynchronous
    // restore of that window and neither closes nor transfers the handle.
    unsafe { ShowWindowAsync(hwnd as HWND, SW_RESTORE) };
    // SAFETY: `hwnd` is the Explorer window handle the owned browser reported,
    // carried unchanged through `usize`; the call only asks that window to take
    // the foreground and neither closes nor transfers the handle.
    let _ = unsafe { SetForegroundWindow(hwnd as HWND) };
    let foreground_deadline = Instant::now() + Duration::from_secs(3);
    while foreground_for(process) != Some(hwnd) {
        ensure!(
            Instant::now() < foreground_deadline,
            "The hosted runner denied foreground access to the owned Explorer window; interactive capture was not proven"
        );
        thread::sleep(Duration::from_millis(25));
    }
    let capture_deadline = Instant::now() + Duration::from_secs(3);
    let captured = loop {
        if let Some(folder) = capture(process) {
            break folder;
        }
        ensure!(
            Instant::now() < capture_deadline,
            "The production Explorer destination helper did not resolve the foreground folder"
        );
        thread::sleep(Duration::from_millis(25));
    };
    ensure!(
        same_path(&captured, &fixture.folder) && foreground_for(process) == Some(hwnd),
        "The production helper captured a different folder or foreground process"
    );
    owned.close()?;
    control.lock().unwrap().owned = None;
    Ok(serde_json::json!({
        "test_context": "shell_capture_only",
        "fixture": fixture.root,
        "captured_folder": captured,
        "foreground_pid": process,
        "foreground_hwnd": hwnd,
        "matched_destination": true,
        "foreground_verified": true,
        "owned_window_closed": true,
        "clipboard_or_paste_performed": false,
        "source_cleanup_verified": false
    }))
}

fn proof_path(fixture: &Fixture) -> Result<PathBuf> {
    let configured = std::env::var_os("DESK_SWITCH_NATIVE_EXPLORER_PROOF")
        .map(PathBuf::from)
        .unwrap_or_else(|| fixture.root.join("explorer-proof.json"));
    let path = if configured.is_absolute() {
        configured
    } else {
        std::env::current_dir()?.join(configured)
    };
    ensure!(
        !path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir)),
        "The Explorer proof path contains parent traversal"
    );
    let parent = path
        .parent()
        .context("Missing Explorer proof directory")?
        .canonicalize()?;
    let metadata = fs::symlink_metadata(&parent)?;
    ensure!(
        metadata.is_dir() && metadata.file_attributes() & 0x400 == 0,
        "The Explorer proof directory was redirected"
    );
    let temporary = PathBuf::from(std::env::var_os("RUNNER_TEMP").context("Missing RUNNER_TEMP")?)
        .canonicalize()?;
    let wrapper_root = parent.parent() == Some(temporary.as_path())
        && parent.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .strip_prefix("desk-switch-native-proof-")
                .is_some_and(|suffix| {
                    suffix.len() == 32
                        && suffix
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
        });
    let target = std::env::current_dir()?.canonicalize()?.join("target");
    ensure!(
        parent.starts_with(&fixture.root) || parent.starts_with(target) || wrapper_root,
        "The Explorer proof path escaped its CI artifact directories"
    );
    Ok(parent.join(
        path.file_name()
            .context("Missing Explorer proof filename")?,
    ))
}

#[test]
#[ignore = "opens only an owned Explorer folder on an explicitly opted-in fresh GitHub-hosted Windows runner"]
fn isolated_ci_explorer_destination_capture() -> Result<()> {
    // This must precede every API, filesystem action, COM call and UI setup.
    require_hosted_desktop();
    let fixture = Fixture::create()?;
    let control = Arc::new(Mutex::new(Control::default()));
    let running = control.clone();
    let worker_fixture = fixture.clone();
    let (reply, result) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("Desk Switch hosted Explorer proof".into())
        .spawn(move || {
            let outcome = run_capture(worker_fixture, running);
            let _ = reply.try_send(outcome);
        })?;
    let proof = match result.recv_timeout(LIMIT) {
        Ok(result) => result?,
        Err(_) => {
            let active = control.lock().unwrap();
            if active.thread != 0 {
                // SAFETY: `active.thread` is the id of the worker thread that entered the
                // Explorer COM apartment and issued the pending Shell calls, recorded there
                // with `GetCurrentThreadId`, and the `Cancellation` guard enabled call
                // cancellation for that apartment; a zero timeout only requests
                // cancellation without waiting for the server.
                let _ = unsafe { CoCancelCall(active.thread, 0) };
            }
            // The only window eligible for timeout cleanup was created by our
            // own COM factory, verified at the owned folder before being shown.
            // Never terminate Explorer or enumerate-and-close unrelated windows.
            if let Some(window) = &active.owned
                && fixture.validate().is_ok()
                && window.folder == fixture.folder
                && window_matches(window)
            {
                // SAFETY: `window.hwnd` is the Explorer window handle that this test's own
                // COM factory created and that `window_matches` just confirmed still belongs
                // to the owned Explorer process; the call only posts a close request to that
                // window and neither closes nor transfers the handle.
                let _ = unsafe { PostMessageW(window.hwnd as HWND, WM_CLOSE, 0, 0) };
            }
            bail!(
                "The hosted Explorer capture proof exceeded 30 seconds; only its owned window was asked to close"
            )
        }
    };
    fixture.validate()?;
    let path = proof_path(&fixture)?;
    fs::write(&path, serde_json::to_vec_pretty(&proof)?)?;
    eprintln!("Explorer destination proof: {}", path.display());
    Ok(())
}
