//! Optional Explorer destination discovery through documented Shell interfaces.
//!
//! A foreground folder is only a destination hint. The native Move adapter must
//! still verify the real completed destination before authorizing source cleanup.
//! No clipboard, input, window activation, or Shell state is changed here.

#[cfg(test)]
#[path = "destination_tests.rs"]
mod hosted_tests;

use std::{
    ffi::OsString,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        System::{
            Com::{
                CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoCancelCall, CoCreateInstance,
                CoDisableCallCancellation, CoEnableCallCancellation, CoInitializeEx, CoTaskMemFree,
                CoUninitialize, IServiceProvider,
            },
            Variant::VARIANT,
        },
        UI::Shell::{
            Common::ITEMIDLIST, GPFIDL_DEFAULT, IFolderView, IPersistFolder2, IShellBrowser,
            IShellWindows, IWebBrowserApp, SHGetPathFromIDListEx, SID_STopLevelBrowser,
            ShellWindows,
        },
    },
    core::Interface,
};
use windows_sys::Win32::{
    Foundation::HWND,
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, IsChild, IsWindowVisible,
    },
};

const QUERY_LIMIT: Duration = Duration::from_millis(500);
const MAX_WINDOWS: i32 = 256;
const MAX_PATH: usize = 32_768;

struct Job {
    id: u64,
    process: u32,
    foreground: usize,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    reply: SyncSender<Option<PathBuf>>,
}
struct Worker {
    send: SyncSender<Job>,
    // The mutex is never held across a Shell call. It prevents a late timeout
    // from cancelling the next query after the preceding call has returned.
    active: Arc<Mutex<Option<(u64, u32)>>>,
    next: AtomicU64,
}

struct Apartment;
impl Apartment {
    fn enter() -> Option<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .ok()?;
        Some(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}
struct Cancellation;
impl Cancellation {
    fn enter() -> Option<Self> {
        unsafe { CoEnableCallCancellation(None) }.ok()?;
        Some(Self)
    }
}
impl Drop for Cancellation {
    fn drop(&mut self) {
        let _ = unsafe { CoDisableCallCancellation(None) };
    }
}
struct Pidl(*mut ITEMIDLIST);
impl Drop for Pidl {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.cast())) };
    }
}

impl Worker {
    fn start() -> Option<Self> {
        // At most one running and one queued query can exist, even if Explorer
        // stops answering COM. The caller always has its own bounded wait.
        let (send, receive) = mpsc::sync_channel::<Job>(1);
        let active = Arc::new(Mutex::new(None));
        let running = active.clone();
        thread::Builder::new()
            .name("Desk Switch Explorer destination".into())
            .spawn(move || {
                let Some(_apartment) = Apartment::enter() else {
                    return;
                };
                let thread_id = unsafe { GetCurrentThreadId() };
                while let Ok(job) = receive.recv() {
                    if !job.live() {
                        let _ = job.reply.try_send(None);
                        continue;
                    }
                    *running.lock().unwrap() = Some((job.id, thread_id));
                    let result = Cancellation::enter().and_then(|_cancellation| {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| discover(&job)))
                            .ok()
                            .flatten()
                    });
                    *running.lock().unwrap() = None;
                    let _ = job.reply.try_send(result.filter(|_| job.live()));
                }
            })
            .ok()?;
        Some(Self {
            send,
            active,
            next: AtomicU64::new(1),
        })
    }
}
impl Job {
    fn live(&self) -> bool {
        !self.cancelled.load(Ordering::Acquire) && Instant::now() < self.deadline
    }
    fn foreground_matches(&self) -> bool {
        self.live() && foreground_for(self.process) == Some(self.foreground)
    }
}

fn foreground_for(process: u32) -> Option<usize> {
    if process == 0 {
        return None;
    }
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return None;
    }
    let mut actual = 0;
    if unsafe { GetWindowThreadProcessId(window, &mut actual) } == 0 || actual != process {
        return None;
    }
    Some(window as usize)
}

/// Call before publishing the app's progress dialog, while Explorer still owns
/// the foreground. Unknown consumers and ambiguous views deliberately return
/// None; they must not be replaced with Downloads or any guessed destination.
pub(super) fn capture(process_id: u32) -> Option<PathBuf> {
    let foreground = foreground_for(process_id)?;
    static WORKER: OnceLock<Option<Worker>> = OnceLock::new();
    let worker = WORKER.get_or_init(Worker::start).as_ref()?;
    let id = worker.next.fetch_add(1, Ordering::Relaxed);
    let cancelled = Arc::new(AtomicBool::new(false));
    let (reply, result) = mpsc::sync_channel(1);
    worker
        .send
        .try_send(Job {
            id,
            process: process_id,
            foreground,
            deadline: Instant::now() + QUERY_LIMIT,
            cancelled: cancelled.clone(),
            reply,
        })
        .ok()?;
    match result.recv_timeout(QUERY_LIMIT) {
        Ok(path) if foreground_for(process_id) == Some(foreground) => path,
        Ok(_) => None,
        Err(_) => {
            cancelled.store(true, Ordering::Release);
            let running = worker.active.lock().unwrap();
            if let Some((active, thread_id)) = *running {
                if active == id {
                    // Zero means no server wait. Cancellation is best effort;
                    // a noncooperating server can occupy only this one worker.
                    let _ = unsafe { CoCancelCall(thread_id, 0) };
                }
            }
            None
        }
    }
}

fn discover(job: &Job) -> Option<PathBuf> {
    if !job.foreground_matches() {
        return None;
    }
    let windows: IShellWindows =
        unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER) }.ok()?;
    let count = unsafe { windows.Count() }.ok()?;
    if !(1..=MAX_WINDOWS).contains(&count) {
        return None;
    }
    let mut matches = Vec::new();
    for index in 0..count {
        if !job.foreground_matches() {
            return None;
        }
        let dispatch = unsafe { windows.Item(&VARIANT::from(index)) }.ok()?;
        let Ok(browser) = dispatch.cast::<IWebBrowserApp>() else {
            continue;
        };
        let Ok(window) = (unsafe { browser.HWND() }) else {
            continue;
        };
        if window.0 as usize != job.foreground {
            continue;
        }
        let service: IServiceProvider = dispatch.cast().ok()?;
        let browser: IShellBrowser = unsafe { service.QueryService(&SID_STopLevelBrowser) }.ok()?;
        let view = unsafe { browser.QueryActiveShellView() }.ok()?;
        let view_window = unsafe { view.GetWindow() }.ok()?.0;
        let frame = job.foreground as HWND;
        if view_window.is_null()
            || unsafe { IsWindowVisible(view_window) } == 0
            || (view_window != frame && unsafe { IsChild(frame, view_window) } == 0)
        {
            continue;
        }
        let mut view_process = 0;
        if unsafe { GetWindowThreadProcessId(view_window, &mut view_process) } == 0
            || view_process != job.process
        {
            return None;
        }
        let folder_view: IFolderView = view.cast().ok()?;
        let folder: IPersistFolder2 = unsafe { folder_view.GetFolder() }.ok()?;
        let item = Pidl(unsafe { folder.GetCurFolder() }.ok()?);
        if item.0.is_null() {
            return None;
        }
        let mut buffer = vec![0u16; MAX_PATH];
        if !unsafe { SHGetPathFromIDListEx(item.0, &mut buffer, GPFIDL_DEFAULT) }.as_bool() {
            return None;
        }
        let end = buffer.iter().position(|unit| *unit == 0)?;
        let path = PathBuf::from(OsString::from_wide(&buffer[..end]));
        if !path.is_absolute() || !std::fs::metadata(&path).ok()?.is_dir() {
            return None;
        }
        matches.push(path);
        // Multiple matching active tab views cannot safely identify a target.
        if matches.len() > 1 {
            return None;
        }
    }
    if !job.foreground_matches() || unsafe { windows.Count() }.ok()? != count {
        return None;
    }
    unique_path(matches)
}

fn unique_path(mut paths: Vec<PathBuf>) -> Option<PathBuf> {
    (paths.len() == 1).then(|| paths.pop()).flatten()
}

#[cfg(test)]
mod tests {
    use super::unique_path;
    use std::path::PathBuf;

    #[test]
    fn unknown_and_ambiguous_shell_views_never_select_a_destination() {
        assert_eq!(unique_path(Vec::new()), None);
        let folder = PathBuf::from(r"C:\destination");
        assert_eq!(unique_path(vec![folder.clone()]), Some(folder.clone()));
        assert_eq!(unique_path(vec![folder.clone(), folder]), None);
        assert_eq!(
            unique_path(vec![PathBuf::from(r"C:\one"), PathBuf::from(r"C:\two")]),
            None
        );
    }
}
