//! macOS has no public per-application output-volume mixer. The current audio
//! model uses the endpoint's real system volume plus each received stream's
//! relative gain; it never needs to change other applications' session gains.
//! This backend is only the legacy Windows/Linux gain-recovery journal reader.
use super::{Backend, Session, State};
use anyhow::Result;
pub(super) struct Native;
impl Native {
    pub fn open() -> Result<Self> {
        Ok(Self)
    }
}
impl Backend for Native {
    fn sessions(&mut self) -> Result<Vec<Session>> {
        Ok(Vec::new())
    }
    fn apply(&mut self, _id: &str, _state: &State) -> Result<()> {
        anyhow::bail!(
            "macOS does not expose per-application audio gains; use system and relative stream volume"
        )
    }
}
