use super::{portal::Portal, receiver::Sink, uinput::NativeInput};
use crate::{
    config::Edge,
    edge::{Crossing, Entry, Routes},
    wire::{Input, Mode},
};
use anyhow::{Result, bail};
#[cfg(feature = "android-remote")]
use std::path::Path;

pub(crate) enum LinuxInput {
    Native(Box<NativeInput>),
    Portal(Box<Portal>),
}

impl LinuxInput {
    pub(crate) async fn open(edge: Option<Edge>, absolute_input: bool) -> Result<Self> {
        if edge.is_none()
            && !absolute_input
            && let Some(native) = Self::try_native().await
        {
            return Ok(native);
        }
        Ok(Self::Portal(Box::new(Portal::open(edge).await?)))
    }

    #[cfg(feature = "android-remote")]
    pub(crate) async fn open_phone(profile: &Path) -> Result<Self> {
        if let Some(native) = Self::try_native().await {
            return Ok(native);
        }
        // Fallback uses a separate restore token so it can coexist with a PC
        // connection. Native input doesn't create or consume either token.
        let permission_dir = profile.join("phone-input-permission");
        std::fs::create_dir_all(&permission_dir)?;
        Ok(Self::Portal(Box::new(
            Portal::open_at(None, &permission_dir).await?,
        )))
    }

    #[cfg(feature = "android-remote")]
    pub(crate) fn phone_ready(&self) -> bool {
        match self {
            Self::Native(sink) => sink.phone_ready(),
            Self::Portal(_) => true,
        }
    }

    #[cfg(feature = "android-remote")]
    pub(crate) async fn poll_phone(&mut self) -> Result<()> {
        match self {
            Self::Native(sink) => sink.poll_phone().await,
            Self::Portal(sink) => sink.poll().await,
        }
    }

    async fn try_native() -> Option<Self> {
        match NativeInput::open().await {
            Ok(input) => {
                println!("Using the Linux virtual keyboard and mouse.");
                Some(Self::Native(Box::new(input)))
            }
            Err(error) => {
                eprintln!("Native input is unavailable. Using Wayland input consent: {error:#}");
                None
            }
        }
    }

    pub(crate) async fn wait_for_active_session(&mut self) -> Result<()> {
        if let Self::Native(sink) = self {
            tokio::select! {
                result = sink.wait_for_active_session() => result?,
                result = crate::lifecycle::shutdown() => {
                    result?;
                    bail!("Input startup stopped");
                }
            }
        }
        Ok(())
    }

    pub(crate) fn start_local(&mut self) -> Result<()> {
        match self {
            Self::Native(sink) => sink.activate(None, Mode::Input),
            Self::Portal(sink) => sink.start_local(),
        }
    }

    pub(crate) fn local_event(&mut self, event: Input, detect_edge: bool) -> Result<Option<u16>> {
        match self {
            Self::Native(sink) => sink.event(event),
            Self::Portal(sink) => sink.local_event(event, detect_edge),
        }
    }

    pub(crate) fn enter_local(&mut self, position: u16) -> Result<()> {
        match self {
            Self::Native(_) => bail!("Screen edges require Wayland input access"),
            Self::Portal(sink) => sink.enter_local(position),
        }
    }

    pub(crate) fn cancel_dwell(&mut self) {
        if let Self::Portal(sink) = self {
            sink.cancel_dwell();
        }
    }

    pub(crate) fn refresh_pointer(&mut self) {
        if let Self::Portal(sink) = self {
            sink.refresh_pointer();
        }
    }

    pub(crate) async fn close(self) {
        if let Self::Portal(sink) = self {
            sink.close().await;
        }
    }
}

impl Sink for LinuxInput {
    async fn poll(&mut self) -> Result<()> {
        match self {
            Self::Native(sink) => sink.poll().await,
            Self::Portal(sink) => sink.poll().await,
        }
    }

    fn prepare(&self, edge: Option<u16>, mode: Mode) -> Result<()> {
        self.prepare_display(edge, None, mode)
    }

    fn activate(&mut self, edge: Option<u16>, mode: Mode) -> Result<()> {
        self.activate_display(edge, None, mode)
    }

    fn prepare_display(&self, edge: Option<u16>, entry: Option<Entry>, mode: Mode) -> Result<()> {
        match self {
            Self::Native(sink) => sink.prepare_display(edge, entry, mode),
            Self::Portal(sink) => sink.prepare_display(edge, entry, mode),
        }
    }

    fn activate_display(
        &mut self,
        edge: Option<u16>,
        entry: Option<Entry>,
        mode: Mode,
    ) -> Result<()> {
        match self {
            Self::Native(sink) => sink.activate_display(edge, entry, mode),
            Self::Portal(sink) => sink.activate_display(edge, entry, mode),
        }
    }

    fn event(&mut self, event: Input) -> Result<Option<u16>> {
        self.local_event(event, true)
    }

    fn release(&mut self) -> Result<()> {
        match self {
            Self::Native(sink) => sink.release(),
            Self::Portal(sink) => sink.release(),
        }
    }

    fn reposition(&mut self, entry: Entry) -> Result<()> {
        match self {
            Self::Native(sink) => sink.reposition(entry),
            Self::Portal(sink) => sink.reposition(entry),
        }
    }

    fn set_routes(&mut self, routes: Routes) -> Result<()> {
        match self {
            Self::Native(sink) => sink.set_routes(routes),
            Self::Portal(sink) => sink.set_routes(routes),
        }
    }

    fn set_generation(&mut self, generation: u64) {
        if let Self::Portal(sink) = self {
            sink.set_generation(generation);
        }
    }

    fn take_crossing(&mut self) -> Option<Crossing> {
        match self {
            Self::Native(_) => None,
            Self::Portal(sink) => sink.take_crossing(),
        }
    }

    fn display_pending(&self) -> bool {
        match self {
            Self::Native(_) => false,
            Self::Portal(sink) => sink.display_pending(),
        }
    }
}
