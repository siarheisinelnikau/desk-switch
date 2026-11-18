//! Existing native input with per-phone proxies and one shared input owner.
use super::receiver::{self, Sink};
use crate::{
    mobile::{Backend, Text, hub::Source},
    transport::Pipe,
    wire::{Input, Mode},
};
use anyhow::{Result, ensure};
use tokio::sync::{mpsc, watch};

pub(crate) async fn run(
    mut pipe: Pipe,
    texts: mpsc::Receiver<Text>,
    stop: watch::Receiver<bool>,
    source: Source,
) -> Result<()> {
    let mut proxy = Proxy {
        source,
        texts,
        active: false,
    };
    let result = receiver::paired(&mut pipe, &mut proxy, stop).await;
    let released = proxy.source.release_wait().await;
    result.and(released)
}
struct Proxy {
    source: Source,
    texts: mpsc::Receiver<Text>,
    active: bool,
}
impl Sink for Proxy {
    async fn poll(&mut self) -> Result<()> {
        match self.texts.recv().await {
            Some(text) => self.source.text(text).await,
            None => std::future::pending().await,
        }
    }
    fn prepare(&self, edge: Option<u16>, mode: Mode) -> Result<()> {
        ensure!(
            edge.is_none() && mode == Mode::Input,
            "Phone remotes cannot change monitor inputs or screen edges"
        );
        Ok(())
    }
    fn activate(&mut self, _: Option<u16>, _: Mode) -> Result<()> {
        self.active = true;
        Ok(())
    }
    fn event(&mut self, event: Input) -> Result<Option<u16>> {
        ensure!(self.active && event.valid(), "Invalid phone input");
        self.source.input(event)?;
        Ok(None)
    }
    fn release(&mut self) -> Result<()> {
        self.active = false;
        self.source.release()
    }
}

pub(crate) enum Native {
    #[cfg(windows)]
    Windows(Box<super::win_receive::NativeSink>),
    #[cfg(target_os = "linux")]
    Linux {
        sink: super::linux_input::LinuxInput,
        writer: Option<crate::group::clipboard::RemoteText>,
    },
    #[cfg(test)]
    Proof(Fake),
    #[cfg(not(any(windows, target_os = "linux")))]
    Unsupported,
}
impl Native {
    pub(crate) fn ready(&self) -> bool {
        match self {
            #[cfg(target_os = "linux")]
            Self::Linux { sink, .. } => sink.phone_ready(),
            #[cfg(windows)]
            Self::Windows(_) => true,
            #[cfg(test)]
            Self::Proof(_) => true,
            #[cfg(not(any(windows, target_os = "linux")))]
            Self::Unsupported => false,
        }
    }

    pub(crate) async fn open(backend: Backend, _profile: &std::path::Path) -> Result<Self> {
        #[cfg(test)]
        if let Backend::Proof(proof) = &backend {
            return Ok(Self::Proof(Fake {
                proof: proof.clone(),
                active: true,
            }));
        }
        let _ = backend;
        #[cfg(windows)]
        let mut sink = super::win_receive::NativeSink::new(None)?;
        #[cfg(target_os = "linux")]
        let mut sink = super::linux_input::LinuxInput::open_phone(_profile).await?;
        #[cfg(any(windows, target_os = "linux"))]
        {
            sink.prepare(None, Mode::Input)?;
            sink.activate(None, Mode::Input)?;
        }
        #[cfg(windows)]
        {
            Ok(Self::Windows(Box::new(sink)))
        }
        #[cfg(target_os = "linux")]
        {
            Ok(Self::Linux { sink, writer: None })
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            anyhow::bail!("Phone remotes currently support Windows and Ubuntu computers")
        }
    }
    pub(crate) async fn poll(&mut self) -> Result<()> {
        match self {
            #[cfg(windows)]
            Self::Windows(sink) => sink.poll().await,
            #[cfg(target_os = "linux")]
            Self::Linux { sink, .. } => sink.poll_phone().await,
            #[cfg(test)]
            Self::Proof(sink) => sink.poll().await,
            #[cfg(not(any(windows, target_os = "linux")))]
            Self::Unsupported => std::future::pending().await,
        }
    }
    pub(crate) fn event(&mut self, event: Input) -> Result<()> {
        match self {
            #[cfg(windows)]
            Self::Windows(sink) => sink.event(event).map(|_| ()),
            #[cfg(target_os = "linux")]
            Self::Linux { sink, .. } => {
                if sink.phone_ready() {
                    sink.event(event)?;
                }
                Ok(())
            }
            #[cfg(test)]
            Self::Proof(sink) => sink.event(event).map(|_| ()),
            #[cfg(not(any(windows, target_os = "linux")))]
            Self::Unsupported => anyhow::bail!("Unsupported input destination"),
        }
    }
    pub(crate) fn release(&mut self) -> Result<()> {
        match self {
            #[cfg(windows)]
            Self::Windows(sink) => sink.release(),
            #[cfg(target_os = "linux")]
            Self::Linux { sink, .. } => sink.release(),
            #[cfg(test)]
            Self::Proof(sink) => sink.release(),
            #[cfg(not(any(windows, target_os = "linux")))]
            Self::Unsupported => Ok(()),
        }
    }
    pub(crate) async fn text(&mut self, value: String) -> Result<()> {
        match self {
            #[cfg(windows)]
            Self::Windows(_) => unicode(&value),
            #[cfg(target_os = "linux")]
            Self::Linux { sink, writer } => {
                ensure!(
                    sink.phone_ready(),
                    "The Ubuntu desktop is locked or inactive"
                );
                if writer.is_none() {
                    *writer = Some(crate::group::clipboard::RemoteText::start()?);
                }
                let mut result = writer.as_ref().unwrap().write(value)?;
                loop {
                    tokio::select! {
                        result = &mut result => { result?.map_err(anyhow::Error::msg)?; break; },
                        result = sink.poll_phone() => {
                            result?;
                            ensure!(sink.phone_ready(), "The Ubuntu desktop is locked or inactive");
                        },
                    }
                }
                ensure!(
                    sink.phone_ready(),
                    "The Ubuntu desktop is locked or inactive"
                );
                for (code, down) in [(29, true), (47, true), (47, false), (29, false)] {
                    sink.event(Input::Key { code, down })?;
                }
                Ok(())
            }
            #[cfg(test)]
            Self::Proof(sink) => {
                sink.proof.lock().unwrap().texts.push(value);
                Ok(())
            }
            #[cfg(not(any(windows, target_os = "linux")))]
            Self::Unsupported => {
                let _ = value;
                anyhow::bail!("Unsupported text destination")
            }
        }
    }
    pub(crate) async fn close(mut self) {
        let _ = self.release();
        #[cfg(target_os = "linux")]
        match self {
            Self::Linux { sink, .. } => sink.close().await,
            #[cfg(test)]
            Self::Proof(_) => {}
        }
    }
}

impl<S: Sink> Sink for &mut S {
    async fn poll(&mut self) -> Result<()> {
        (**self).poll().await
    }
    fn prepare(&self, edge: Option<u16>, mode: Mode) -> Result<()> {
        (**self).prepare(edge, mode)
    }
    fn activate(&mut self, edge: Option<u16>, mode: Mode) -> Result<()> {
        (**self).activate(edge, mode)
    }
    fn event(&mut self, event: Input) -> Result<Option<u16>> {
        (**self).event(event)
    }
    fn release(&mut self) -> Result<()> {
        (**self).release()
    }
    fn prepare_display(
        &self,
        edge: Option<u16>,
        entry: Option<crate::edge::Entry>,
        mode: Mode,
    ) -> Result<()> {
        (**self).prepare_display(edge, entry, mode)
    }
    fn activate_display(
        &mut self,
        edge: Option<u16>,
        entry: Option<crate::edge::Entry>,
        mode: Mode,
    ) -> Result<()> {
        (**self).activate_display(edge, entry, mode)
    }
    fn reposition(&mut self, entry: crate::edge::Entry) -> Result<()> {
        (**self).reposition(entry)
    }
    fn set_routes(&mut self, routes: crate::edge::Routes) -> Result<()> {
        (**self).set_routes(routes)
    }
    fn set_generation(&mut self, generation: u64) {
        (**self).set_generation(generation);
    }
    fn take_crossing(&mut self) -> Option<crate::edge::Crossing> {
        (**self).take_crossing()
    }
    fn display_pending(&self) -> bool {
        (**self).display_pending()
    }
}

#[cfg(windows)]
fn unicode(text: &str) -> Result<()> {
    use std::mem::size_of;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
    let mut inputs = Vec::with_capacity(text.len() * 2);
    for unit in text.encode_utf16() {
        for flags in [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP] {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wScan: unit,
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            });
        }
    }
    // SAFETY: the vector contains fully initialized keyboard INPUT records;
    // Windows only reads its live buffer during this bounded synchronous call.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    ensure!(
        sent as usize == inputs.len(),
        "Windows could not insert all remote text; no retry was sent"
    );
    Ok(())
}

#[cfg(test)]
pub(crate) struct Fake {
    proof: std::sync::Arc<std::sync::Mutex<crate::mobile::Proof>>,
    active: bool,
}
#[cfg(test)]
impl Sink for Fake {
    async fn poll(&mut self) -> Result<()> {
        std::future::pending().await
    }
    fn prepare(&self, _: Option<u16>, mode: Mode) -> Result<()> {
        ensure!(mode == Mode::Input, "No video in a phone session");
        Ok(())
    }
    fn activate(&mut self, _: Option<u16>, _: Mode) -> Result<()> {
        self.active = true;
        Ok(())
    }
    fn event(&mut self, event: Input) -> Result<Option<u16>> {
        ensure!(self.active, "No active input session");
        self.proof.lock().unwrap().inputs.push(event);
        Ok(None)
    }
    fn release(&mut self) -> Result<()> {
        self.active = false;
        self.proof.lock().unwrap().releases += 1;
        Ok(())
    }
}
