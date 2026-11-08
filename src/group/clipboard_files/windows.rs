//! Windows file selections use CF_HDROP, never text that happens to name a file.
//! Remote selections reference genuine Cloud Files placeholders. Publishing
//! metadata keeps the native clipboard lock short; filesystem reads hydrate it.
mod cloud;
mod cloud_api;
mod destination;

use crate::group::file_transfer::protocol::Operation;
use anyhow::{Context, Result, ensure};
use std::{
    cell::RefCell,
    ffi::{OsString, c_void},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    ptr,
    time::Duration,
};

const CF_HDROP: u32 = 15;
const MAX_ROOTS: usize = 10_000;
const MAX_METADATA: usize = 1024 * 1024;
const DROP_HEADER: usize = 20;
const GMEM_MOVEABLE: u32 = 0x0002;
const GMEM_ZEROINIT: u32 = 0x0040;
const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;
type Handle = *mut c_void;

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}
#[repr(C)]
struct Message {
    window: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    point: Point,
    private: u32,
}

#[link(name = "user32")]
// SAFETY: The declarations below match the documented `user32.dll` exports with
// the `WINAPI` (`extern "system"`) calling convention and the exact argument
// and return types those exports use, so the linker binds the real functions.
unsafe extern "system" {
    fn OpenClipboard(owner: Handle) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn IsClipboardFormatAvailable(format: u32) -> i32;
    fn GetClipboardData(format: u32) -> Handle;
    #[cfg(test)]
    fn GetClipboardOwner() -> Handle;
    fn SetClipboardData(format: u32, memory: Handle) -> Handle;
    fn GetClipboardSequenceNumber() -> u32;
    fn RegisterClipboardFormatW(name: *const u16) -> u32;
    fn CreateWindowExW(
        extended_style: u32,
        class_name: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Handle,
        menu: Handle,
        instance: Handle,
        parameter: Handle,
    ) -> Handle;
    fn DestroyWindow(window: Handle) -> i32;
    fn IsWindow(window: Handle) -> i32;
    fn GetMessageW(message: *mut Message, window: Handle, minimum: u32, maximum: u32) -> i32;
    fn DispatchMessageW(message: *const Message) -> isize;
    fn PostMessageW(window: Handle, message: u32, wparam: usize, lparam: isize) -> i32;
}
#[link(name = "kernel32")]
// SAFETY: The declarations below match the documented `kernel32.dll` exports
// with the `WINAPI` (`extern "system"`) calling convention and the exact
// argument and return types those exports use, so the linker binds the real
// functions.
unsafe extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
    fn GlobalFree(memory: Handle) -> Handle;
    fn GlobalLock(memory: Handle) -> *mut c_void;
    fn GlobalUnlock(memory: Handle) -> i32;
    fn GlobalSize(memory: Handle) -> usize;
    fn MultiByteToWideChar(
        code_page: u32,
        flags: u32,
        bytes: *const u8,
        length: i32,
        wide: *mut u16,
        capacity: i32,
    ) -> i32;
}

fn os_error(operation: &str) -> anyhow::Error {
    anyhow::Error::new(std::io::Error::last_os_error()).context(operation.to_owned())
}

struct Clipboard;
impl Clipboard {
    fn open(owner: Handle) -> Result<Self> {
        // A busy clipboard is a transient worker error. Do not sleep or spin
        // here; the sharing worker retains its pending write and retries later.
        ensure!(
            // SAFETY: `owner` is either a null HWND, which OpenClipboard accepts, or the
            // message-only window this process created; the call only borrows the handle
            // and the returned guard closes the clipboard exactly once.
            unsafe { OpenClipboard(owner) } != 0,
            "Windows clipboard is temporarily busy"
        );
        Ok(Self)
    }
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        // SAFETY: This guard exists only after a successful `OpenClipboard` on this
        // thread and Windows allows one open clipboard per thread, so the drop closes
        // exactly the lock this guard took, and only once.
        unsafe { CloseClipboard() };
    }
}

struct Locked(Handle);
impl Locked {
    fn new(memory: Handle) -> Result<(Self, *mut u8)> {
        // SAFETY: `memory` is a live moveable global-memory handle owned by the
        // caller, so `GlobalLock` returns a pointer to its locked block that stays
        // valid until the returned guard unlocks it in `Drop`.
        let data = unsafe { GlobalLock(memory) }.cast::<u8>();
        if data.is_null() {
            return Err(os_error("Could not lock Windows clipboard data"));
        }
        Ok((Self(memory), data))
    }
}
impl Drop for Locked {
    fn drop(&mut self) {
        // SAFETY: `Locked` holds the lock this thread took on `self.0` in
        // `Locked::new`, and that lock is released exactly once when this guard drops.
        unsafe { GlobalUnlock(self.0) };
    }
}

struct Allocation(Handle);
impl Allocation {
    fn new(bytes: &[u8]) -> Result<Self> {
        // SAFETY: The flags request a moveable, zero-initialized global block of
        // exactly `bytes.len()` bytes; a non-null result is owned by the `Allocation`
        // wrapper and freed exactly once in `Drop`.
        let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len()) };
        if memory.is_null() {
            return Err(os_error("Could not allocate Windows clipboard data"));
        }
        let allocated = Self(memory);
        let (_lock, data) = Locked::new(memory)?;
        // SAFETY: `data` points at the locked global block that was allocated with
        // exactly `bytes.len()` bytes and the source slice is separate Rust memory, so
        // the copy stays in bounds and does not overlap.
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len()) };
        Ok(allocated)
    }
    fn publish(mut self, format: u32) -> Result<()> {
        // SAFETY: The clipboard is open on this thread and `self.0` is a moveable
        // global block this wrapper owns; on success Windows takes that ownership, and
        // on failure the block is still freed by `Drop`.
        if unsafe { SetClipboardData(format, self.0) }.is_null() {
            return Err(os_error("Could not publish Windows clipboard files"));
        }
        // SetClipboardData owns the allocation after success. GlobalFree must
        // only run for an allocation that Windows has not accepted.
        self.0 = ptr::null_mut();
        Ok(())
    }
}
impl Drop for Allocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` is non-null only while this wrapper still owns the
            // block; `publish` clears it once Windows has taken ownership, so the block is
            // freed exactly once here.
            unsafe { GlobalFree(self.0) };
        }
    }
}

struct Owner(Handle);
impl Owner {
    fn create() -> Result<Self> {
        let (ready, result) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("Desk Switch file clipboard owner".into())
            .spawn(move || {
                let window = match Self::window() {
                    Ok(window) => window,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                if ready.send(Ok(window as usize)).is_err() {
                    // SAFETY: The window was created on this thread by `Self::window()` and the
                    // receiver is gone, so this is its only owner and it is destroyed once.
                    unsafe { DestroyWindow(window) };
                    return;
                }
                // Other applications can synchronously notify the clipboard
                // owner when replacing it. Keep this thread in a real message
                // loop even while the opt-in clipboard worker is asleep.
                // SAFETY: `Message` is a `repr(C)` struct of integers and a null-able
                // window handle, so the all-zero bit pattern is a valid value that
                // `GetMessageW` fills in.
                let mut message: Message = unsafe { std::mem::zeroed() };
                // SAFETY: `message` is a live, writable `Message` and `window` is the
                // message-only window this thread created, so the call fills the struct that
                // the loop dispatches below.
                while unsafe { GetMessageW(&mut message, window, 0, 0) } > 0 {
                    // SAFETY: `message` was filled by the `GetMessageW` call above and is not
                    // modified in between, so the pointer refers to the message being dispatched.
                    unsafe { DispatchMessageW(&message) };
                    // SAFETY: `window` is the message-only window created on this thread;
                    // `IsWindow` only inspects the handle value and never dereferences it.
                    if unsafe { IsWindow(window) } == 0 {
                        return;
                    }
                }
                // SAFETY: The window was created by `Self::window()` on this thread, which
                // owns it, so it is destroyed exactly once when the message loop ends.
                unsafe { DestroyWindow(window) };
            })?;
        let window = result
            .recv_timeout(Duration::from_secs(2))
            .context("Windows file clipboard owner did not start")??;
        Ok(Self(window as Handle))
    }

    fn window() -> Result<Handle> {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let title: Vec<u16> = "Desk Switch file clipboard\0".encode_utf16().collect();
        // A message-only window is never shown or rendered. EmptyClipboard
        // needs a real owner HWND; opening with NULL for a write would leave
        // the clipboard owner NULL and make SetClipboardData fail.
        // SAFETY: `class` and `title` are NUL-terminated UTF-16 buffers that outlive
        // the call, every other argument is a constant, and a non-null result is the
        // window this thread then owns.
        let window = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                -3isize as Handle, // HWND_MESSAGE
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if window.is_null() {
            return Err(os_error("Could not create the Windows clipboard owner"));
        }
        Ok(window)
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        // STATIC's default WM_CLOSE handler destroys the owned window on its
        // message thread; that thread then exits. Do not block a caller or the
        // GUI waiting for shutdown.
        // SAFETY: `self.0` is the message-only window created on the owner thread
        // and still alive here; the post only queues a WM_CLOSE for that thread, which
        // destroys the window.
        unsafe { PostMessageW(self.0, 0x0010, 0, 0) }; // WM_CLOSE
    }
}
thread_local! {
    // The adapter is called by the dedicated clipboard worker. Its separate
    // message-only owner stays responsive until that worker exits.
    static OWNER: RefCell<Option<Owner>> = const { RefCell::new(None) };
}

fn valid_path(path: &Path) -> Result<Vec<u16>> {
    let encoded = path.as_os_str().encode_wide().collect::<Vec<_>>();
    let text = String::from_utf16(&encoded).context("File clipboard path is not valid Unicode")?;
    // Canonical paths from the standard library use the verbatim drive form.
    // Normalize it to the ordinary DOS path the clipboard and Shell accept.
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let bytes = text.as_bytes();
    ensure!(
        bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/')
            && !text.chars().any(char::is_control),
        "File clipboard supports absolute local disk paths only"
    );
    for component in text[3..].split(['\\', '/']) {
        if component.is_empty() {
            continue;
        }
        ensure!(
            !matches!(component, "." | "..")
                && !component.ends_with([' ', '.'])
                && !component.chars().any(|c| "<>:\"|?*".contains(c)),
            "File clipboard path contains an unsafe component"
        );
        let stem = component
            .split('.')
            .next()
            .unwrap_or_default()
            .trim_end_matches(' ');
        let reserved = stem.eq_ignore_ascii_case("CON")
            || stem.eq_ignore_ascii_case("PRN")
            || stem.eq_ignore_ascii_case("AUX")
            || stem.eq_ignore_ascii_case("NUL")
            || stem.eq_ignore_ascii_case("CONIN$")
            || stem.eq_ignore_ascii_case("CONOUT$")
            || stem.get(..3).is_some_and(|prefix| {
                (prefix.eq_ignore_ascii_case("COM") || prefix.eq_ignore_ascii_case("LPT"))
                    && matches!(
                        &stem[3..],
                        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                    )
            });
        ensure!(!reserved, "File clipboard path names a Windows device");
    }
    let normalized = text.encode_utf16().collect::<Vec<_>>();
    ensure!(
        normalized.len() <= 32_767,
        "File clipboard path exceeds the Windows path limit"
    );
    Ok(normalized)
}

fn ansi_path(bytes: &[u8]) -> Result<PathBuf> {
    ensure!(
        bytes.len() <= MAX_METADATA,
        "File clipboard data is too large"
    );
    let length = i32::try_from(bytes.len())?;
    // A legacy ANSI CF_HDROP explicitly uses the user's active Windows code
    // page, not UTF-8. Keep this conversion at the Win32 boundary.
    const MB_ERR_INVALID_CHARS: u32 = 0x0008;
    // SAFETY: `bytes` is a live slice whose length was converted to `length`,
    // and the null destination with capacity 0 only asks for the required size.
    let needed = unsafe {
        MultiByteToWideChar(
            0,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            length,
            ptr::null_mut(),
            0,
        )
    };
    if needed <= 0 {
        return Err(os_error("Could not decode Windows file clipboard paths"));
    }
    let mut wide = vec![0; needed as usize];
    // SAFETY: `wide` holds exactly the `needed` UTF-16 units the previous call
    // reported and the call writes at most that many, while `bytes` stays the
    // live `length`-byte source.
    let written = unsafe {
        MultiByteToWideChar(
            0,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            length,
            wide.as_mut_ptr(),
            needed,
        )
    };
    ensure!(
        written == needed,
        "Windows file clipboard path decoding failed"
    );
    Ok(PathBuf::from(OsString::from_wide(&wide)))
}

fn decode_drop(bytes: &[u8]) -> Result<Vec<PathBuf>> {
    ensure!(
        (DROP_HEADER + 2..=MAX_METADATA).contains(&bytes.len()),
        "File clipboard data has an invalid size"
    );
    let offset = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    let wide = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) != 0;
    ensure!(
        offset >= DROP_HEADER && offset < bytes.len() && (!wide || offset.is_multiple_of(2)),
        "File clipboard data has an invalid path offset"
    );
    let payload = &bytes[offset..];
    let mut paths = Vec::new();
    let mut at = 0;
    if wide {
        let units = payload
            .as_chunks::<2>()
            .0
            .iter()
            .copied()
            .map(u16::from_le_bytes)
            .collect::<Vec<_>>();
        loop {
            let end = units[at..]
                .iter()
                .position(|value| *value == 0)
                .map(|relative| at + relative)
                .context("File clipboard paths are not terminated")?;
            if end == at {
                ensure!(at > 0, "File clipboard selection is empty");
                break;
            }
            ensure!(paths.len() < MAX_ROOTS, "Too many clipboard files");
            let path = PathBuf::from(OsString::from_wide(&units[at..end]));
            valid_path(&path)?;
            paths.push(path);
            at = end + 1;
            ensure!(
                at < units.len(),
                "File clipboard paths need a double terminator"
            );
        }
    } else {
        loop {
            let end = payload[at..]
                .iter()
                .position(|value| *value == 0)
                .map(|relative| at + relative)
                .context("File clipboard paths are not terminated")?;
            if end == at {
                ensure!(at > 0, "File clipboard selection is empty");
                break;
            }
            ensure!(paths.len() < MAX_ROOTS, "Too many clipboard files");
            let path = ansi_path(&payload[at..end])?;
            valid_path(&path)?;
            paths.push(path);
            at = end + 1;
            ensure!(
                at < payload.len(),
                "File clipboard paths need a double terminator"
            );
        }
    }
    ensure!(!paths.is_empty(), "File clipboard selection is empty");
    Ok(paths)
}

fn encode_drop(paths: &[PathBuf]) -> Result<Vec<u8>> {
    ensure!(
        !paths.is_empty() && paths.len() <= MAX_ROOTS,
        "File clipboard selection is empty or has too many files"
    );
    let mut bytes = vec![0; DROP_HEADER];
    bytes[..4].copy_from_slice(&(DROP_HEADER as u32).to_le_bytes());
    bytes[16..20].copy_from_slice(&1u32.to_le_bytes()); // DROPFILES.fWide
    for path in paths {
        let wide = valid_path(path)?;
        ensure!(
            bytes
                .len()
                .checked_add((wide.len() + 1) * 2 + 2)
                .is_some_and(|length| length <= MAX_METADATA),
            "File clipboard data exceeds the 1 MiB limit"
        );
        for unit in wide.into_iter().chain(std::iter::once(0)) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&0u16.to_le_bytes());
    Ok(bytes)
}

fn preferred_format() -> Result<u32> {
    let name = "Preferred DropEffect\0".encode_utf16().collect::<Vec<_>>();
    // SAFETY: `name` is a NUL-terminated UTF-16 buffer that outlives the call,
    // which only reads it and returns the registered format id.
    let format = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
    if format == 0 {
        return Err(os_error("Could not register Windows file clipboard format"));
    }
    Ok(format)
}

fn drop_operation(bytes: Option<&[u8]>) -> Result<Option<Operation>> {
    let Some(bytes) = bytes else {
        // The standard CF_HDROP format does not require a preferred effect.
        return Ok(Some(Operation::Copy));
    };
    ensure!(
        (4..=MAX_METADATA).contains(&bytes.len()),
        "Windows file clipboard drop effect is malformed"
    );
    let effect = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    Ok(match effect {
        DROPEFFECT_COPY => Some(Operation::Copy),
        DROPEFFECT_MOVE => Some(Operation::Move),
        _ => None,
    })
}

fn preferred_operation(_clipboard: &Clipboard) -> Result<Option<Operation>> {
    let format = preferred_format()?;
    // SAFETY: The clipboard is open on this thread for the whole call, so this
    // query sees the same format set that the reads below observe.
    if unsafe { IsClipboardFormatAvailable(format) } == 0 {
        return drop_operation(None);
    }
    // SAFETY: The clipboard is open on this thread for the whole call, so
    // `GetClipboardData` returns the moveable global block that holds this
    // format; it stays valid until the clipboard is closed and is locked before it
    // is read.
    let memory = unsafe { GetClipboardData(format) };
    if memory.is_null() {
        return Err(os_error(
            "Could not read Windows file clipboard drop effect",
        ));
    }
    // SAFETY: `memory` is the non-null moveable global block returned by
    // `GetClipboardData`, and `GlobalSize` only reports the size of that block.
    let length = unsafe { GlobalSize(memory) };
    ensure!(
        (4..=MAX_METADATA).contains(&length),
        "Windows file clipboard drop effect is malformed"
    );
    let (_lock, bytes) = Locked::new(memory)?;
    // SAFETY: `bytes` is the locked global block whose `GlobalSize` is `length`,
    // and the `_lock` guard keeps it locked and alive while the slice is read.
    drop_operation(Some(unsafe { std::slice::from_raw_parts(bytes, length) }))
}

#[cfg(test)]
fn preferred_copy(clipboard: &Clipboard) -> Result<bool> {
    Ok(preferred_operation(clipboard)? == Some(Operation::Copy))
}

fn sequence_matches(expected: Option<u64>, actual: u32) -> bool {
    expected.is_none_or(|expected| actual != 0 && expected == u64::from(actual))
}

pub(super) fn read() -> Result<Option<super::Selection>> {
    // SAFETY: The call only inspects the current clipboard format list and needs
    // no open clipboard; `CF_HDROP` is the standard file-drop format id.
    if unsafe { IsClipboardFormatAvailable(CF_HDROP) } == 0 {
        return Ok(None);
    }
    let clipboard = Clipboard::open(ptr::null_mut())?;
    read_locked(&clipboard, true)
}
fn read_locked(clipboard: &Clipboard, suppress_provider: bool) -> Result<Option<super::Selection>> {
    // Formats may change between IsClipboardFormatAvailable and OpenClipboard.
    // Both Preferred DropEffect and CF_HDROP must be read under this same lock.
    // SAFETY: The clipboard is open on this thread for the whole call, so this
    // query sees the same format set that the reads below observe.
    if unsafe { IsClipboardFormatAvailable(CF_HDROP) } == 0 {
        return Ok(None);
    }
    let Some(operation) = preferred_operation(clipboard)? else {
        // SAFETY: The call only reads the clipboard's serial number; it takes no
        // pointer and needs no open clipboard.
        let sequence = u64::from(unsafe { GetClipboardSequenceNumber() });
        ensure!(
            sequence != 0,
            "Windows file clipboard sequence is unavailable"
        );
        // A file format is present, but Link/unknown effects stay local. The empty
        // selection prevents ancillary filenames from leaking as plain text.
        return Ok(Some(super::Selection {
            sequence,
            paths: Vec::new(),
            operation: Operation::Copy,
        }));
    };
    // SAFETY: The clipboard is open on this thread for the whole call, so
    // `GetClipboardData` returns the moveable global block that holds the
    // CF_HDROP selection; it stays valid until the clipboard is closed and is
    // locked before it is read.
    let memory = unsafe { GetClipboardData(CF_HDROP) };
    if memory.is_null() {
        return Err(os_error("Could not read Windows clipboard files"));
    }
    // SAFETY: `memory` is the non-null moveable global block returned by
    // `GetClipboardData`, and `GlobalSize` only reports the size of that block.
    let length = unsafe { GlobalSize(memory) };
    ensure!(
        (DROP_HEADER + 2..=MAX_METADATA).contains(&length),
        "File clipboard data exceeds the 1 MiB limit or is malformed"
    );
    let (_lock, bytes) = Locked::new(memory)?;
    // SAFETY: `bytes` is the locked global block whose `GlobalSize` is `length`,
    // and the `_lock` guard keeps it locked and alive while the slice is decoded.
    let mut paths = decode_drop(unsafe { std::slice::from_raw_parts(bytes, length) })?;
    if suppress_provider && cloud::owns_paths(&paths) {
        paths.clear();
    }
    // SAFETY: The call only reads the clipboard's serial number; it takes no
    // pointer and needs no open clipboard.
    let sequence = u64::from(unsafe { GetClipboardSequenceNumber() });
    ensure!(
        sequence != 0,
        "Windows file clipboard sequence is unavailable"
    );
    Ok(Some(super::Selection {
        sequence,
        paths,
        operation,
    }))
}

pub(super) fn publish_offer_if_current(
    offer: super::provider::RemoteOffer,
    expected: u64,
) -> Result<Option<Box<dyn super::provider::PublishedOffer>>> {
    cloud::publish(offer, expected)
}
pub(super) fn is_provider_path(path: &Path) -> bool {
    cloud::owns_path(path)
}
pub(super) fn clear_cut_if_current(paths: &[PathBuf], expected: u64) -> Result<bool> {
    ensure!(
        expected != 0,
        "The original Cut clipboard generation is unavailable"
    );
    OWNER.with(|slot| {
        let mut owner = slot.borrow_mut();
        if owner.is_none() {
            *owner = Some(Owner::create()?);
        }
        let clipboard = Clipboard::open(owner.as_ref().unwrap().0)?;
        // SAFETY: The clipboard is open on this thread, so this serial read is
        // protected by the same native lock as the empty below.
        if !sequence_matches(Some(expected), unsafe { GetClipboardSequenceNumber() }) {
            return Ok(false);
        }
        let Some(selection) = read_locked(&clipboard, false)? else {
            return Ok(false);
        };
        if selection.sequence != expected
            || selection.operation != Operation::Move
            || selection.paths != paths
        {
            return Ok(false);
        }
        ensure!(
            // SAFETY: The clipboard is open on this thread for the whole call, and
            // `EmptyClipboard` only drops the formats the current owner published under
            // that lock.
            unsafe { EmptyClipboard() } != 0,
            "Could not clear the completed Cut selection"
        );
        Ok(true)
    })
}

#[cfg(test)]
pub(super) fn write(paths: &[PathBuf]) -> Result<()> {
    write_inner(paths, None).map(|_| ())
}

pub(super) fn write_if_current(paths: &[PathBuf], expected: u64) -> Result<bool> {
    write_inner(paths, Some(expected))
}

fn write_inner(paths: &[PathBuf], expected: Option<u64>) -> Result<bool> {
    publish_paths(paths, expected, Operation::Copy).map(|receipt| receipt.is_some())
}

#[derive(Clone, Copy)]
struct Receipt {
    sequence: u64,
}

fn publish_paths(
    paths: &[PathBuf],
    expected: Option<u64>,
    operation: Operation,
) -> Result<Option<Receipt>> {
    // Finish validation and allocation before replacing the current clipboard.
    let files = Allocation::new(&encode_drop(paths)?)?;
    let value = match operation {
        Operation::Copy => DROPEFFECT_COPY,
        Operation::Move => DROPEFFECT_MOVE,
    };
    let effect = Allocation::new(&value.to_le_bytes())?;
    let format = preferred_format()?;
    OWNER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(Owner::create()?);
        }
        let owner = slot.as_ref().unwrap().0;
        let clipboard = Clipboard::open(owner)?;
        // This check is protected by the same native lock as EmptyClipboard.
        // A newer local copy cannot be overwritten between comparison and
        // publication by another application.
        // SAFETY: The clipboard is open on this thread, so this serial read is
        // protected by the same native lock as the empty below.
        if !sequence_matches(expected, unsafe { GetClipboardSequenceNumber() }) {
            return Ok(None);
        }
        // SAFETY: The clipboard is open on this thread and the publication below
        // re-fills it, so no other application can observe or replace the contents in
        // between.
        if unsafe { EmptyClipboard() } == 0 {
            return Err(os_error("Could not prepare Windows file clipboard"));
        }
        effect.publish(format)?;
        files.publish(CF_HDROP)?;
        // SAFETY: The call only reads the clipboard's serial number; it takes no
        // pointer and needs no open clipboard.
        let sequence = u64::from(unsafe { GetClipboardSequenceNumber() });
        ensure!(
            sequence != 0,
            "Windows file clipboard sequence is unavailable"
        );
        drop(clipboard);
        // Record our serial while still protected, not a later local copy's
        // serial. The binding compares exact file metadata before reusing it.
        Ok(Some(Receipt { sequence }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::Cell,
        fs,
        time::{Instant, SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn native_drop_roundtrips_files_folders_and_non_ascii_names() {
        let paths = vec![
            PathBuf::from(r"C:\Users\user\Downloads\Desk Switch\photo.png"),
            PathBuf::from("D:\\Пример\\Folder with spaces\\📂"),
        ];
        let encoded = encode_drop(&paths).unwrap();
        assert_eq!(encoded[..4], 20u32.to_le_bytes());
        assert_eq!(encoded[16..20], 1u32.to_le_bytes());
        assert_eq!(decode_drop(&encoded).unwrap(), paths);
        assert_eq!(&encoded[encoded.len() - 4..], &[0; 4]);

        let verbatim = vec![PathBuf::from(r"\\?\C:\Users\user\safe.txt")];
        assert_eq!(
            decode_drop(&encode_drop(&verbatim).unwrap()).unwrap(),
            vec![PathBuf::from(r"C:\Users\user\safe.txt")]
        );
    }

    #[test]
    fn native_drop_rejects_network_device_relative_and_unsafe_paths() {
        for path in [
            r"\\server\share\private.txt",
            r"\\.\PhysicalDrive0",
            r"\??\C:\private.txt",
            r"C:relative.txt",
            r"\root-relative.txt",
            "relative.txt",
            r"C:\private.txt:secret",
            r"C:\safe\..\private.txt",
            r"C:\safe\CON.txt",
            r"C:\safe\COM1",
            r"C:\safe\LPT9.txt",
            r"C:\safe\trailing.",
            "C:\\safe\\line\nfeed",
        ] {
            assert!(encode_drop(&[path.into()]).is_err(), "accepted {path:?}");
        }
        // Similar ordinary filenames are not devices.
        assert!(encode_drop(&[r"C:\safe\COM10.txt".into()]).is_ok());
    }

    #[test]
    fn native_drop_rejects_truncation_bad_offsets_but_accepts_allocation_padding() {
        let valid = encode_drop(&[r"C:\safe\file.txt".into()]).unwrap();
        assert!(decode_drop(&valid[..valid.len() - 2]).is_err());
        assert!(decode_drop(&valid[..valid.len() - 1]).is_err());
        for offset in [0u32, 19, 21, valid.len() as u32, u32::MAX] {
            let mut bytes = valid.clone();
            bytes[..4].copy_from_slice(&offset.to_le_bytes());
            assert!(decode_drop(&bytes).is_err());
        }
        // GlobalSize may report more bytes than were requested by the owner.
        // Bytes after the final double NUL are padding, not additional paths.
        let mut padded = valid.clone();
        padded.extend_from_slice(&[1, 2, 3]);
        assert_eq!(
            decode_drop(&padded).unwrap(),
            [PathBuf::from(r"C:\safe\file.txt")]
        );
        assert!(decode_drop(&vec![0; MAX_METADATA + 1]).is_err());
    }

    #[test]
    fn native_drop_enforces_file_count_and_metadata_limits_before_native_calls() {
        assert!(encode_drop(&[]).is_err());
        let paths = vec![PathBuf::from(r"C:\safe\file.txt"); MAX_ROOTS + 1];
        assert!(encode_drop(&paths).is_err());
        let long = PathBuf::from(format!("C:\\{}", "a".repeat(30_000)));
        assert!(encode_drop(&vec![long; 20]).is_err());
    }

    #[test]
    fn explicit_copy_and_cut_are_distinct_and_links_stay_local() {
        assert_eq!(drop_operation(None).unwrap(), Some(Operation::Copy));
        assert_eq!(
            drop_operation(Some(&1u32.to_le_bytes())).unwrap(),
            Some(Operation::Copy)
        );
        assert_eq!(
            drop_operation(Some(&2u32.to_le_bytes())).unwrap(),
            Some(Operation::Move)
        );
        for effect in [0u32, 3, 4, 7, u32::MAX] {
            assert_eq!(drop_operation(Some(&effect.to_le_bytes())).unwrap(), None);
        }
        assert!(drop_operation(Some(&[1, 0, 0])).is_err());
    }

    #[test]
    fn conditional_publish_requires_the_exact_nonzero_sequence() {
        assert!(sequence_matches(None, 0));
        assert!(sequence_matches(None, 19));
        assert!(sequence_matches(Some(19), 19));
        assert!(!sequence_matches(Some(18), 19));
        assert!(!sequence_matches(Some(0), 0));
        assert!(!sequence_matches(Some(u64::from(u32::MAX) + 1), 1));
    }

    const CF_UNICODETEXT: u32 = 13;

    /// Only constructed after the ignored integration test has verified the
    /// disposable GitHub-hosted runner and the explicit opt-in marker.
    struct NativeFixture {
        directory: PathBuf,
        canonical: PathBuf,
        last_sequence: Cell<u64>,
    }
    impl NativeFixture {
        fn create() -> Self {
            assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
            assert_eq!(
                std::env::var("DESK_SWITCH_NATIVE_CLIPBOARD_TEST").as_deref(),
                Ok("1"),
                "Native clipboard tests require the dedicated CI-only opt-in"
            );
            assert_eq!(
                std::env::var("RUNNER_ENVIRONMENT").as_deref(),
                Ok("github-hosted"),
                "Refusing native clipboard access on a self-hosted/user desktop"
            );
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "desk-switch-native-clipboard-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&directory).unwrap();
            let canonical = directory.canonicalize().unwrap();
            Self {
                directory,
                canonical,
                last_sequence: Cell::new(0),
            }
        }
        fn remember(&self) -> u64 {
            // SAFETY: The call only reads the clipboard's serial number; it takes no
            // pointer and needs no open clipboard.
            let sequence = u64::from(unsafe { GetClipboardSequenceNumber() });
            assert_ne!(sequence, 0);
            self.last_sequence.set(sequence);
            sequence
        }
        fn publish(
            &self,
            paths: Option<&[PathBuf]>,
            effect: Option<u32>,
            text: Option<&str>,
        ) -> Result<u64> {
            // Eagerly allocate all fixture formats before EmptyClipboard. This
            // test never supplies delayed-rendering handles or source streams.
            let files = paths
                .map(|paths| Allocation::new(&encode_drop(paths)?))
                .transpose()?;
            let effect = effect
                .map(|value| Allocation::new(&value.to_le_bytes()))
                .transpose()?;
            let text = text
                .map(|value| {
                    let bytes = value
                        .encode_utf16()
                        .chain(std::iter::once(0))
                        .flat_map(u16::to_le_bytes)
                        .collect::<Vec<_>>();
                    Allocation::new(&bytes)
                })
                .transpose()?;
            let effect_format = preferred_format()?;
            OWNER.with(|slot| -> Result<()> {
                let mut slot = slot.borrow_mut();
                if slot.is_none() {
                    *slot = Some(Owner::create()?);
                }
                let owner = slot.as_ref().unwrap().0;
                ensure!(
                    !owner.is_null(),
                    "Fixture clipboard owner must be a real HWND"
                );
                let _clipboard = Clipboard::open(owner)?;
                ensure!(
                    // SAFETY: The fixture holds the clipboard open on this thread for the
                    // whole publication and re-fills every format it wants afterwards.
                    unsafe { EmptyClipboard() } != 0,
                    "Could not empty fixture clipboard"
                );
                if let Some(effect) = effect {
                    effect.publish(effect_format)?;
                }
                if let Some(files) = files {
                    files.publish(CF_HDROP)?;
                }
                if let Some(text) = text {
                    text.publish(CF_UNICODETEXT)?;
                }
                ensure!(
                    // SAFETY: The call only reads the current clipboard owner handle and
                    // compares it with the fixture's own window handle.
                    unsafe { GetClipboardOwner() } == owner,
                    "Fixture HWND did not own clipboard"
                );
                Ok(())
            })?;
            // Closing a text publication can add synthesized Windows formats
            // and advance its sequence. Acknowledge the completed publication
            // after the native guard has closed, like production write().
            Ok(self.remember())
        }
        fn assert_formats(&self, expect_files: bool, expect_text: bool) -> Result<()> {
            let clipboard = Clipboard::open(ptr::null_mut())?;
            ensure!(
                // SAFETY: The call only inspects the clipboard's format list and needs no
                // open clipboard.
                (unsafe { IsClipboardFormatAvailable(CF_HDROP) } != 0) == expect_files,
                "Unexpected CF_HDROP availability"
            );
            ensure!(
                // SAFETY: The call only inspects the clipboard's format list and needs no
                // open clipboard.
                (unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } != 0) == expect_text,
                "Unexpected text availability"
            );
            // SAFETY: The call only inspects the clipboard's format list, and
            // `preferred_format` returns a format id registered with Windows.
            let effect_present = unsafe { IsClipboardFormatAvailable(preferred_format()?) } != 0;
            ensure!(
                effect_present == expect_files,
                "Partial file clipboard formats were published"
            );
            if expect_files {
                ensure!(
                    preferred_copy(&clipboard)?,
                    "Published file selection did not request copy"
                );
            }
            ensure!(
                // SAFETY: The call only reads the clipboard's serial number; it takes no
                // pointer and needs no open clipboard.
                u64::from(unsafe { GetClipboardSequenceNumber() }) == self.last_sequence.get(),
                "Clipboard changed outside the fixture"
            );
            Ok(())
        }
        fn text(&self) -> Result<String> {
            let _clipboard = Clipboard::open(ptr::null_mut())?;
            // SAFETY: The fixture holds the clipboard open on this thread for the whole
            // call, so the returned moveable global block stays valid until it is locked
            // and read below.
            let memory = unsafe { GetClipboardData(CF_UNICODETEXT) };
            ensure!(!memory.is_null(), "Fixture text is missing");
            // SAFETY: `memory` is the non-null moveable global block returned by
            // `GetClipboardData`, and `GlobalSize` only reports the size of that block.
            let length = unsafe { GlobalSize(memory) };
            ensure!(
                (2..=4096).contains(&length),
                "Fixture text has an invalid size"
            );
            let (_lock, bytes) = Locked::new(memory)?;
            // SAFETY: `bytes` is the locked global block whose `GlobalSize` is `length`,
            // and the `_lock` guard keeps it locked and alive while the slice is decoded.
            let bytes = unsafe { std::slice::from_raw_parts(bytes, length) };
            let units = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .copied()
                .map(u16::from_le_bytes)
                .take_while(|value| *value != 0)
                .collect::<Vec<_>>();
            Ok(String::from_utf16(&units)?)
        }
    }
    impl Drop for NativeFixture {
        fn drop(&mut self) {
            // Do not empty data that a different process has since copied. On
            // this disposable runner, cleanup is limited to our owner HWND and
            // the exact last sequence successfully published by this fixture.
            let expected = self.last_sequence.get();
            if expected != 0 {
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    let cleaned =
                        OWNER
                            .try_with(|slot| {
                                let slot = slot.borrow();
                                let Some(owner) = slot.as_ref() else {
                                    return true;
                                };
                                // SAFETY: The call only reads the current clipboard owner handle and
                                // compares it with the fixture's own window handle.
                                if unsafe { GetClipboardOwner() } != owner.0
                                // SAFETY: The call only reads the clipboard's serial number, which confirms
                                // that the fixture still owns the publication it made.
                                || !sequence_matches(Some(expected), unsafe {
                                    GetClipboardSequenceNumber()
                                }) {
                                    return true;
                                }
                                let Ok(_clipboard) = Clipboard::open(owner.0) else {
                                    return false;
                                };
                                // SAFETY: The call only reads the current clipboard owner handle and
                                // compares it with the fixture's own window handle.
                                if unsafe { GetClipboardOwner() } == owner.0
                                // SAFETY: The call only reads the clipboard's serial number, which confirms
                                // that the fixture still owns the publication it made.
                                && sequence_matches(Some(expected), unsafe {
                                    GetClipboardSequenceNumber()
                                }) {
                                    // SAFETY: The fixture holds the clipboard open on this thread and only
                                    // empties it while it still owns the exact publication it made.
                                    unsafe { EmptyClipboard() };
                                }
                                true
                            })
                            .unwrap_or(true);
                    if cleaned || Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            // Only recursively remove the same unique fixture directory we
            // created under the canonical temporary directory.
            let allowed = self.directory.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("desk-switch-native-clipboard-")
            }) && self.directory.canonicalize().ok().as_ref()
                == Some(&self.canonical)
                && std::env::temp_dir().canonicalize().ok().as_deref() == self.canonical.parent();
            if allowed {
                let _ = fs::remove_dir_all(&self.directory);
            }
        }
    }

    fn native_retry<T>(mut operation: impl FnMut() -> Result<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match operation() {
                Ok(value) => return value,
                Err(error) if Instant::now() >= deadline => {
                    panic!("Native clipboard fixture failed: {error:#}")
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    #[test]
    #[ignore = "Requires disposable GitHub-hosted Windows CI and DESK_SWITCH_NATIVE_CLIPBOARD_TEST=1"]
    fn isolated_ci_native_file_clipboard_roundtrip() {
        // This constructor checks all CI markers before any clipboard API,
        // owner-window creation, or fixture-file operation can run.
        let fixture = NativeFixture::create();
        let file = fixture.directory.join("first file.txt");
        let folder = fixture.directory.join("Folder — пример");
        fs::write(&file, b"Only isolated CI fixture bytes").unwrap();
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("nested.txt"), b"Folder fixture").unwrap();
        let paths = vec![file, folder];

        // Real production write/read proves CF_HDROP + Preferred DropEffect
        // ownership and the GetClipboardData lock ordering, not only parsing.
        native_retry(|| write(&paths));
        let copied = fixture.remember();
        native_retry(|| fixture.assert_formats(true, false));
        let selected = native_retry(read).expect("Native file selection is missing");
        assert_eq!(selected.paths, paths);
        assert_eq!(selected.sequence, copied);

        // Cut preserves its explicit Move intent; links suppress file and text
        // fallback. Neither metadata read starts any file-content transfer.
        for effect in [2u32, 4] {
            let sequence =
                native_retry(|| fixture.publish(Some(&paths), Some(effect), Some("Stay local")));
            let selected = native_retry(read).expect("Cut/link must remain a file selection");
            if effect == DROPEFFECT_MOVE {
                assert_eq!(selected.paths, paths);
                assert_eq!(selected.operation, Operation::Move);
            } else {
                assert!(selected.paths.is_empty());
            }
            assert_eq!(selected.sequence, sequence);
        }

        let newer = native_retry(|| fixture.publish(None, None, Some("A newer local copy")));
        assert!(!native_retry(|| write_if_current(&paths, copied)));
        native_retry(|| fixture.assert_formats(false, true));
        assert_eq!(native_retry(|| fixture.text()), "A newer local copy");
        // SAFETY: The call only reads the clipboard's serial number; it takes no
        // pointer and needs no open clipboard.
        assert_eq!(u64::from(unsafe { GetClipboardSequenceNumber() }), newer);

        // Matching acknowledgements replace all formats together; no stale
        // text survives and the new read acknowledges the exact native serial.
        assert!(native_retry(|| write_if_current(&paths, newer)));
        let completed = fixture.remember();
        assert_ne!(completed, newer);
        native_retry(|| fixture.assert_formats(true, false));
        let selected = native_retry(read).expect("Completed file clipboard is missing");
        assert_eq!(selected.paths, paths);
        assert_eq!(selected.sequence, completed);
    }
}
