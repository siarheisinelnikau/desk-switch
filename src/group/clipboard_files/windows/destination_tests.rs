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
    System::Com::CoAllowSetForegroundWindow,
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
    (unsafe { IsWindow(handle) }) != 0
        && unsafe { GetWindowThreadProcessId(handle, &mut process) } != 0
        && process == window.process
}
fn explorer_process(process: u32) -> Result<()> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process) };
    ensure!(
        !handle.is_null(),
        "Could not inspect the owned Explorer process"
    );
    let mut name = vec![0u16; MAX_PATH];
    let mut count = name.len() as u32;
    let result = unsafe { QueryFullProcessImageNameW(handle, 0, name.as_mut_ptr(), &mut count) };
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
    let browser: IShellBrowser = unsafe { service.QueryService(&SID_STopLevelBrowser) }.ok()?;
    let view = unsafe { browser.QueryActiveShellView() }.ok()?;
    let view: IFolderView = view.cast().ok()?;
    let folder: IPersistFolder2 = unsafe { view.GetFolder() }.ok()?;
    let item = Pidl(unsafe { folder.GetCurFolder() }.ok()?);
    if item.0.is_null() {
        return None;
    }
    let mut buffer = vec![0u16; MAX_PATH];
    if !unsafe { SHGetPathFromIDListEx(item.0, &mut buffer, GPFIDL_DEFAULT) }.as_bool() {
        return None;
    }
    let end = buffer.iter().position(|unit| *unit == 0)?;
    Some(PathBuf::from(OsString::from_wide(&buffer[..end])))
}

fn run_capture(fixture: Fixture, control: Arc<Mutex<Control>>) -> Result<serde_json::Value> {
    let _apartment =
        Apartment::enter().context("The hosted Explorer COM apartment is unavailable")?;
    let _cancel =
        Cancellation::enter().context("The hosted Explorer COM calls cannot be bounded")?;
    control.lock().unwrap().thread = unsafe { GetCurrentThreadId() };
    fixture.validate()?;
    let browser: IWebBrowser2 =
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
    let hwnd = unsafe { owned.browser.HWND() }
        .context("Explorer has no window")?
        .0 as usize;
    ensure!(hwnd != 0, "Explorer did not create a window");
    let mut process = 0;
    ensure!(
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
    let _ = unsafe { CoAllowSetForegroundWindow(&owned.browser, None) };
    unsafe { owned.browser.SetVisible(VARIANT_BOOL(-1)) }
        .context("Could not display the owned Explorer window")?;
    unsafe { ShowWindowAsync(hwnd as HWND, SW_RESTORE) };
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
