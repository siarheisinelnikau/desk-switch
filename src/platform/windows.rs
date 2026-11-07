use super::win_input::{self, Hooks};
use crate::{
    config::{Edge, Identity, Key},
    gate::Gate,
    video::{Inputs, VideoWorker},
};
use anyhow::Result;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::mpsc;
use windows_sys::Win32::{
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT},
};

struct WindowsCursor(u32, bool);

impl super::host::Cursor for WindowsCursor {
    async fn request_toggle(&self) -> Result<()> {
        anyhow::ensure!(
            unsafe { PostThreadMessageW(self.0, win_input::REQUEST_TOGGLE, 0, 0) } != 0,
            "Windows input thread is unavailable"
        );
        Ok(())
    }
    async fn center(&self) -> Result<()> {
        if self.1 { win_input::center() } else { Ok(()) }
    }
    async fn enter(&self, edge: &Edge, position: u16) -> Result<()> {
        if self.1 {
            win_input::enter(edge, position)
        } else {
            Ok(())
        }
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
