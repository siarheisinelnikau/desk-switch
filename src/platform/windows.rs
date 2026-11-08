use super::win_input::{self, Hooks};
use crate::{
    config::{Edge, Identity, Key},
    edge::Entry,
    gate::Gate,
    video::{Inputs, VideoWorker},
};
use anyhow::Result;
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;
use windows_sys::Win32::{
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT},
};

struct WindowsCursor(u32, bool);

struct PendingEntry(Arc<Mutex<bool>>);
impl Drop for PendingEntry {
    fn drop(&mut self) {
        if let Ok(mut canceled) = self.0.lock() {
            *canceled = true;
        }
    }
}

impl super::host::Cursor for WindowsCursor {
    async fn request_toggle(&self) -> Result<()> {
        anyhow::ensure!(
            // SAFETY: the thread id was captured from the running input
            // thread, and PostThreadMessage copies its parameters into the
            // target queue instead of keeping the pointers.
            unsafe { PostThreadMessageW(self.0, win_input::REQUEST_TOGGLE, 0, 0) } != 0,
            "Windows input thread is unavailable"
        );
        Ok(())
    }
    async fn center(&self) -> Result<()> {
        if self.1 {
            tokio::task::spawn_blocking(win_input::center)
                .await
                .map_err(|error| anyhow::anyhow!("Windows cursor worker stopped: {error}"))?
        } else {
            Ok(())
        }
    }
    async fn enter(&self, edge: &Edge, position: u16) -> Result<()> {
        if self.1 {
            let edge = edge.clone();
            tokio::task::spawn_blocking(move || win_input::enter(&edge, position))
                .await
                .map_err(|error| anyhow::anyhow!("Windows cursor worker stopped: {error}"))?
        } else {
            Ok(())
        }
    }
    async fn enter_display(&self, activation: u64, entry: Entry) -> Result<()> {
        if self.1 {
            let routes = crate::lifecycle::routes();
            let canceled = Arc::new(Mutex::new(false));
            let _pending = PendingEntry(Arc::clone(&canceled));
            tokio::task::spawn_blocking(move || {
                win_input::enter_display_current(entry, activation, &routes, &canceled)
            })
            .await
            .map_err(|error| anyhow::anyhow!("Windows cursor worker stopped: {error}"))??;
            anyhow::ensure!(
                // SAFETY: the thread id was captured from the running input
                // thread, and PostThreadMessage copies its parameters into the
                // target queue instead of keeping the pointers.
                unsafe { PostThreadMessageW(self.0, win_input::RESET_ENTRY, 0, 0) } != 0,
                "Windows input thread is unavailable"
            );
        }
        Ok(())
    }
}

pub fn run(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    edge: Option<Edge>,
    sharing: crate::config::InputSharing,
    video: Option<(Inputs, VideoWorker)>,
) -> Result<()> {
    let gate = Arc::new(Gate::new());
    let (control_tx, controls) = mpsc::channel(8);
    let (input_tx, events) = mpsc::channel(256);
    let hooks = Hooks::install(
        Arc::clone(&gate),
        edge.clone(),
        sharing,
        control_tx,
        input_tx,
    )?;
    // SAFETY: GetCurrentThreadId takes no arguments and returns the id of the
    // calling thread, which is the thread that owns the hook message queue.
    let hook_thread = unsafe { GetCurrentThreadId() };
    let runtime = crate::runtime()?;
    let thread = std::thread::Builder::new()
        .name("Desk Switch coordinator".into())
        .spawn(move || {
            let result = runtime.block_on(super::host::connect(
                address,
                identity,
                peer,
                super::host::Coordinator {
                    monitor_required: crate::lifecycle::monitor_required,
                    gate,
                    edge,
                    video,
                    controls,
                    events,
                    cursor: WindowsCursor(hook_thread, sharing.mouse),
                },
            ));
            unsafe {
                // SAFETY: the thread id belongs to the hook thread that owns
                // this message queue, and PostThreadMessage copies its
                // parameters instead of keeping the pointers.
                PostThreadMessageW(hook_thread, WM_QUIT, 0, 0);
            }
            result
        })?;
    println!("F12 switches the shared monitor and input. F11 immediately restores Windows input.");
    hooks.run()?;
    thread
        .join()
        .map_err(|_| anyhow::anyhow!("Windows coordinator stopped unexpectedly"))?
}
