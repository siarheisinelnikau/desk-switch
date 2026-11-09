//! Finder file selections and explicit publication of verified local copies.
use super::{
    Selection,
    provider::{PublishedOffer, RemoteOffer},
};
use crate::group::file_transfer::protocol::Operation;
use anyhow::{Context, Result, ensure};
use objc2::{
    ClassType, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::ProtocolObject,
};
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSString, NSURL};
use std::{
    cell::{Cell, RefCell},
    os::unix::ffi::OsStrExt,
    path::PathBuf,
};

const MAX_ROOTS: usize = 10_000;
const MAX_METADATA: usize = 1024 * 1024;

struct Cached {
    sequence: u64,
    paths: Option<Vec<PathBuf>>,
}
thread_local! {
    // Only owned Rust paths outlive the autorelease pool. The clipboard
    // worker can keep suppressing ancillary text without decoding URL items
    // again or retaining Objective-C objects on another thread.
    static CACHED: RefCell<Option<Cached>> = const { RefCell::new(None) };
    static LAST_SEQUENCE: Cell<Option<u64>> = const { Cell::new(None) };
}

fn pasteboard() -> Result<Retained<NSPasteboard>> {
    // Match arboard's nullable lookup when no Aqua pasteboard is available.
    let board: Option<Retained<NSPasteboard>> =
        // SAFETY: the message takes no arguments and returns a retained
        // pasteboard, which the optional keeps owned.
        unsafe { msg_send![NSPasteboard::class(), generalPasteboard] };
    board.context("The macOS system pasteboard is unavailable")
}
fn pasteboard_sequence(board: &NSPasteboard) -> Result<u64> {
    let sequence = u64::try_from(board.changeCount()).ok();
    LAST_SEQUENCE.with(|last| last.set(sequence));
    sequence.context("The macOS pasteboard change count is invalid")
}
pub(super) fn sequence() -> Option<u64> {
    LAST_SEQUENCE.with(Cell::get)
}
fn validate_paths(paths: &[PathBuf]) -> Result<()> {
    ensure!(
        !paths.is_empty() && paths.len() <= MAX_ROOTS,
        "Select between 1 and 10000 clipboard files or folders"
    );
    let mut bytes = 0usize;
    for path in paths {
        let raw = path.as_os_str().as_bytes();
        ensure!(
            path.is_absolute() && !raw.starts_with(b"//") && !raw.contains(&0),
            "The clipboard contains an invalid local file path"
        );
        bytes = bytes
            .checked_add(raw.len())
            .context("Clipboard file metadata overflow")?;
        ensure!(
            bytes <= MAX_METADATA,
            "Clipboard file metadata exceeds 1 MiB"
        );
    }
    Ok(())
}

fn file_path_from_uri(value: &str) -> Result<PathBuf> {
    // URL::parse normalizes an opaque `file:relative` into an absolute URL;
    // reject that spelling before normalization rather than granting access
    // to a file the source did not identify with an absolute file URI.
    let (scheme, rest) = value
        .split_once(':')
        .context("The clipboard file URI has no scheme")?;
    ensure!(
        scheme.eq_ignore_ascii_case("file") && rest.starts_with('/'),
        "The clipboard item is not an absolute file URI"
    );
    let url = reqwest::Url::parse(value).context("The clipboard file URI is invalid")?;
    ensure!(
        url.scheme() == "file"
            && url
                .host_str()
                .is_none_or(|host| host.is_empty() || host.eq_ignore_ascii_case("localhost"))
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Only local clipboard file URLs are supported"
    );
    let path = url
        .to_file_path()
        .map_err(|_| anyhow::anyhow!("The clipboard file URI has no local filesystem path"))?;
    validate_paths(std::slice::from_ref(&path))?;
    Ok(path)
}

pub(super) fn read() -> Result<Option<Selection>> {
    autoreleasepool(|_| {
        let board = pasteboard()?;
        let current = pasteboard_sequence(&board)?;
        if let Some(paths) = CACHED.with(|cache| {
            cache
                .borrow()
                .as_ref()
                .filter(|cached| cached.sequence == current)
                .map(|cached| cached.paths.clone())
        }) {
            return Ok(paths.map(|paths| Selection {
                sequence: current,
                paths,
                operation: Operation::Copy,
            }));
        }
        let mut paths = Vec::new();
        if let Some(items) = board.pasteboardItems() {
            ensure!(
                items.len() <= MAX_ROOTS,
                "The clipboard has more than 10000 items"
            );
            // SAFETY: the constant is a static pasteboard type identifier and no
            // memory is transferred.
            let file_type = unsafe { NSPasteboardTypeFileURL };
            let file_types = NSArray::from_slice(&[file_type]);
            let mut metadata = 0usize;
            for item in items.iter() {
                if item.availableTypeFromArray(&file_types).is_none() {
                    continue;
                }
                let uri = item
                    .stringForType(file_type)
                    .context("The clipboard file URL is not ready")?;
                metadata = metadata
                    .checked_add(uri.len())
                    .context("Clipboard file metadata overflow")?;
                ensure!(
                    metadata <= MAX_METADATA,
                    "Clipboard file metadata exceeds 1 MiB"
                );
                let mut path = file_path_from_uri(&uri.to_string())?;
                let url =
                    NSURL::URLWithString(&uri).context("macOS rejected the clipboard file URL")?;
                ensure!(url.isFileURL(), "The clipboard URL is not a local file");
                if url.isFileReferenceURL() {
                    // Finder may supply an identity-based file URL. Resolve
                    // that native representation rather than sending its
                    // opaque /.file/id spelling to another computer.
                    let resolved = url
                        .filePathURL()
                        .context("The clipboard file reference is unavailable")?;
                    let native = resolved
                        .path()
                        .context("The clipboard file has no filesystem path")?;
                    ensure!(
                        native.len() <= MAX_METADATA,
                        "Clipboard file metadata exceeds 1 MiB"
                    );
                    path = PathBuf::from(native.to_string());
                }
                paths.push(path);
            }
        }
        if !paths.is_empty() {
            validate_paths(&paths)?;
        }
        ensure!(
            pasteboard_sequence(&board)? == current,
            "The clipboard changed while its files were read; retry the copy"
        );
        let paths = (!paths.is_empty()).then_some(paths);
        CACHED.with(|cache| {
            *cache.borrow_mut() = Some(Cached {
                sequence: current,
                paths: paths.clone(),
            })
        });
        Ok(paths.map(|paths| Selection {
            sequence: current,
            paths,
            operation: Operation::Copy,
        }))
    })
}

pub(super) fn publish_offer_if_current(
    _offer: RemoteOffer,
    _expected: u64,
) -> Result<Option<Box<dyn PublishedOffer>>> {
    // AppKit file promises establish drag-and-drop support, not Finder Cmd+V.
    // Until native paste is verified, leave the local pasteboard untouched and
    // never replace demand-driven delivery with an eager transfer.
    anyhow::bail!("File clipboard paste is unavailable on macOS. Use Send files instead.")
}

pub(super) fn write_if_current(paths: &[PathBuf], expected: u64) -> Result<bool> {
    write_impl(paths, Some(expected))
}

fn write_impl(paths: &[PathBuf], expected: Option<u64>) -> Result<bool> {
    // The caller supplies complete, verified local staging paths. Validate
    // everything before clearing the existing pasteboard. NSURL objects
    // advertise standard file-copy items, not Finder's cut/move marker.
    validate_paths(paths)?;
    autoreleasepool(|_| {
        let objects = paths
            .iter()
            .map(|path| {
                let path = path
                    .to_str()
                    .context("A clipboard filename is not valid UTF-8")?;
                let url = NSURL::fileURLWithPath(&NSString::from_str(path));
                Ok(ProtocolObject::<dyn NSPasteboardWriting>::from_retained(
                    url,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let objects = NSArray::from_retained_slice(&objects);
        let board = pasteboard()?;
        // Build every URL first, then recheck immediately before ownership is
        // changed. NSPasteboard has no cross-process compare-and-set API.
        if let Some(expected) = expected
            && pasteboard_sequence(&board)? != expected
        {
            return Ok(false);
        }
        board.clearContents();
        ensure!(
            board.writeObjects(&objects),
            "macOS refused to publish the received clipboard files"
        );
        pasteboard_sequence(&board)?;
        // Another application may copy immediately after writeObjects. Read
        // the actual items next time rather than associating our paths with a
        // newer owner's change count. The worker deduplicates our own write.
        CACHED.with(|cache| *cache.borrow_mut() = None);
        Ok(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsupported_native_paste_never_requests_file_contents() {
        use crate::group::{
            clipboard_files::provider::{DemandProvider, PasteIntent, PasteSession},
            file_transfer::protocol::{Description, Descriptor, DescriptorKind},
        };
        use std::sync::Arc;
        struct UnexpectedDemand;
        impl DemandProvider for UnexpectedDemand {
            fn lease(&self) -> Result<Arc<dyn DemandProvider>> {
                panic!("An unsupported native paste must not acquire a lease")
            }
            fn begin(&self, _intent: PasteIntent) -> Result<Arc<dyn PasteSession>> {
                panic!("An unsupported native paste must not start a transfer")
            }
        }
        for operation in [Operation::Copy, Operation::Move] {
            let result = publish_offer_if_current(
                RemoteOffer {
                    id: "0".repeat(32),
                    description: Description {
                        name: "Untouched clipboard".into(),
                        roots: vec!["file.txt".into()],
                        entries: vec![Descriptor {
                            path: vec!["file.txt".into()],
                            kind: DescriptorKind::File { size: 0 },
                        }],
                        total: 0,
                    },
                    operation,
                    provider: Arc::new(UnexpectedDemand),
                },
                0,
            );
            assert!(result.is_err_and(|error| error.to_string().contains("unavailable on macOS")));
        }
    }
    #[test]
    fn file_uris_preserve_spaces_unicode_and_literal_delimiters() {
        assert_eq!(
            file_path_from_uri("file:///Users/me/My%20Files/%E2%9C%93%23%3F.txt").unwrap(),
            PathBuf::from("/Users/me/My Files/✓#?.txt")
        );
        assert_eq!(
            file_path_from_uri("file://localhost/Users/me/test.txt").unwrap(),
            PathBuf::from("/Users/me/test.txt")
        );
    }
    #[test]
    fn native_file_selection_rejects_remote_relative_and_text_paths() {
        for value in [
            "/Users/me/plain.txt",
            "https://example.com/file",
            "smb://server/share/file",
            "file:relative.txt",
            "file://other-host/share/file",
            "file:///Users/me/a%00b",
            "file:///Users/me/a?query",
            "file:///Users/me/a#fragment",
        ] {
            assert!(file_path_from_uri(value).is_err(), "Accepted {value}");
        }
        assert!(validate_paths(&[PathBuf::from("relative")]).is_err());
        assert!(validate_paths(&vec![PathBuf::from("/x"); MAX_ROOTS + 1]).is_err());
        assert!(
            validate_paths(&[PathBuf::from(format!("/{}", "x".repeat(MAX_METADATA)))]).is_err()
        );
    }
}
