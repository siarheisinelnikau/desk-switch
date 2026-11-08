//! Native virtual-file contracts. Metadata calls must never request contents.
pub(crate) use crate::group::file_transfer::protocol::NativeConsumer;
use crate::group::file_transfer::protocol::{Description, Operation};
use anyhow::Result;
use std::{
    fs::File,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CompletionSupport {
    /// The native consumer provides data reads, but no destination commit receipt.
    Unavailable,
    /// The consumer will report its actual operation result separately.
    Reported,
}

#[derive(Clone, Debug)]
pub(crate) struct PasteIntent {
    pub operation: Operation,
    pub completion: CompletionSupport,
    pub destination: Option<String>,
    pub consumer: Option<NativeConsumer>,
}

// Platform consumers report only the outcomes they can actually observe:
// committed Move entries, cancellation, or a terminal failure.
#[derive(Clone, Debug)]
pub(crate) enum NativeResult {
    /// Every declared entry in one explicitly cut native batch was committed.
    MoveCommitted {
        consumer: NativeConsumer,
        entries: Vec<usize>,
    },
    Cancelled,
    Failed(String),
}

/// Implemented by the transfer service. Calling begin is an actual local paste
/// or file-content demand; merely enumerating descriptors must not call it.
pub(crate) trait DemandProvider: Send + Sync {
    /// Pins metadata authority without preparing, reading or transferring bytes.
    /// A lease opened before retirement remains usable by that native handle.
    fn lease(&self) -> Result<Arc<dyn DemandProvider>>;
    fn begin(&self, intent: PasteIntent) -> Result<Arc<dyn PasteSession>>;
}

/// Blocking methods are for a bounded native I/O worker, never the GUI thread.
/// Returned files are read-only handles opened through verified pinned storage.
pub(crate) trait PasteSession: Send + Sync {
    /// Stable identity for diagnostics and platform tests. Production paths do
    /// not currently branch on it, so the trait keeps it available without
    /// forcing every consumer to display it.
    #[allow(dead_code)]
    fn id(&self) -> &str;
    fn open_verified(&self, index: usize) -> Result<File>;
    fn wait_verified(&self) -> Result<()>;
    fn wait_unpaused(&self) -> Result<()>;
    fn report_read(&self, index: usize, offset: u64, count: usize) -> Result<()>;
    fn is_terminal(&self) -> bool;
    /// A cancelled native Move may retry only before any cleanup receipt was
    /// accepted. A committed/recovery journal requires a new explicit Cut.
    fn can_retry_move(&self) -> bool;
    fn native_complete(&self, result: NativeResult) -> Result<()>;
    #[cfg_attr(not(windows), allow(dead_code))]
    fn open_verified_cancellable(&self, index: usize, cancel: &AtomicBool) -> Result<File> {
        anyhow::ensure!(
            !cancel.load(Ordering::Acquire),
            "The native file request was cancelled"
        );
        let file = self.open_verified(index)?;
        anyhow::ensure!(
            !cancel.load(Ordering::Acquire),
            "The native file request was cancelled"
        );
        Ok(file)
    }
    #[cfg_attr(not(windows), allow(dead_code))]
    fn wait_verified_cancellable(&self, cancel: &AtomicBool) -> Result<()> {
        anyhow::ensure!(
            !cancel.load(Ordering::Acquire),
            "The native file request was cancelled"
        );
        self.wait_verified()?;
        anyhow::ensure!(
            !cancel.load(Ordering::Acquire),
            "The native file request was cancelled"
        );
        Ok(())
    }
    #[cfg_attr(not(windows), allow(dead_code))]
    fn wait_unpaused_cancellable(&self, cancel: &AtomicBool) -> Result<()> {
        anyhow::ensure!(
            !cancel.load(Ordering::Acquire),
            "The native file request was cancelled"
        );
        self.wait_unpaused()?;
        anyhow::ensure!(
            !cancel.load(Ordering::Acquire),
            "The native file request was cancelled"
        );
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct RemoteOffer {
    pub id: String,
    pub description: Description,
    pub operation: Operation,
    pub provider: Arc<dyn DemandProvider>,
}

/// Ownership must be checked natively: delayed rendering may change a serial
/// without a new user copy. Dropping a binding must not cancel started pastes.
pub(crate) trait PublishedOffer: Send {
    fn is_current(&self) -> Result<bool>;
    fn sequence(&self) -> Option<u64>;
    fn local_paths(&self) -> Vec<PathBuf>;
}
