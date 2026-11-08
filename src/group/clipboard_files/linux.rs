//! Native file copies on GNOME Wayland and X11. URI data is never interpreted
//! as a shell command, and plain text is never treated as a file selection.
use super::Selection;
use anyhow::{Context, Result, bail, ensure};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
    time::Duration,
};

const LIMIT: usize = 1024 * 1024;
const ROOTS: usize = 10_000;
const URI_MIME: &str = "text/uri-list";
const GNOME_MIME: &str = "x-special/gnome-copied-files";
const COMPANION: &str = "io.github.siarheisinelnikau.DeskSwitch.Clipboard";
const COMPANION_PATH: &str = "/io/github/siarheisinelnikau/DeskSwitch/Clipboard";

thread_local! {
    // The shared-clipboard worker is the sole caller. Keeping this native owner
    // alive is required on X11: dropping a temporary clipboard loses its files.
    static NATIVE: RefCell<Option<Native>> = const { RefCell::new(None) };
}

pub(super) fn read() -> Result<Option<Selection>> {
    NATIVE.with(|native| {
        let mut native = native.borrow_mut();
        if native.is_none() {
            *native = Some(Native::new()?);
        }
        native.as_mut().unwrap().read()
    })
}

/// Last revision observed by `read`, including text-only or empty selections.
/// This is deliberately a cache lookup: it neither opens nor reads a clipboard.
pub(super) fn sequence() -> Option<u64> {
    NATIVE.with(|native| {
        native.borrow().as_ref().and_then(|native| match native {
            Native::Gnome(native) => (native.serial != 0).then_some(u64::from(native.serial)),
            Native::X11(native) => (native.sequence != 0).then_some(native.sequence),
        })
    })
}

pub(super) fn write_if_current(paths: &[PathBuf], expected: u64) -> Result<bool> {
    let bytes = encode_paths(paths)?;
    NATIVE.with(|native| {
        let mut native = native.borrow_mut();
        let Some(native) = native.as_mut() else {
            // No revision has been observed on this worker yet.
            return Ok(false);
        };
        match native {
            Native::Gnome(native) => native.write_if_current(paths, bytes, expected),
            Native::X11(native) => native.write_if_current(paths, expected),
        }
    })
}

fn wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

enum Native {
    Gnome(Gnome),
    X11(X11),
}
impl Native {
    fn new() -> Result<Self> {
        if wayland() {
            // GNOME does not offer a general background data-control protocol.
            // Never silently use an Xwayland clipboard with different content.
            Gnome::new().map(Self::Gnome).context(
                "Enable or reload Desk Switch Tray in GNOME Extensions to share files on Wayland",
            )
        } else {
            X11::new().map(Self::X11)
        }
    }
    fn read(&mut self) -> Result<Option<Selection>> {
        match self {
            Self::Gnome(native) => native.read(),
            Self::X11(native) => native.read(),
        }
    }
}

struct Gnome {
    connection: zbus::blocking::Connection,
    serial: u32,
    paths: Option<Vec<PathBuf>>,
    refresh: bool,
}
impl Gnome {
    fn new() -> Result<Self> {
        let connection = zbus::blocking::connection::Builder::session()?
            .method_timeout(Duration::from_secs(2))
            .build()?;
        Ok(Self {
            connection,
            serial: 0,
            paths: None,
            refresh: true,
        })
    }
    fn read(&mut self) -> Result<Option<Selection>> {
        let proxy =
            zbus::blocking::Proxy::new(&self.connection, COMPANION, COMPANION_PATH, COMPANION)?;
        let (serial, mime, bytes): (u32, String, Vec<u8>) = match proxy
            .call("ReadFiles", &(if self.refresh { 0 } else { self.serial },))
        {
            Ok(value) => value,
            Err(error) => {
                self.serial = 0;
                self.paths = None;
                self.refresh = true;
                return Err(error).context(
                        "Enable or reload Desk Switch Tray in GNOME Extensions to share files on Wayland",
                    );
            }
        };
        ensure!(
            serial != 0,
            "The clipboard companion returned an invalid revision"
        );
        if self.refresh || serial != self.serial || !mime.is_empty() {
            self.paths = if mime.is_empty() {
                None
            } else {
                parse_selection(&mime, &bytes)?
            };
            self.serial = serial;
            self.refresh = false;
        }
        Ok(self.paths.clone().map(|paths| Selection {
            sequence: u64::from(serial),
            paths,
        }))
    }
    fn write_if_current(
        &mut self,
        paths: &[PathBuf],
        bytes: Vec<u8>,
        expected: u64,
    ) -> Result<bool> {
        let Ok(expected) = u32::try_from(expected) else {
            return Ok(false);
        };
        if expected == 0 {
            return Ok(false);
        }
        let proxy =
            zbus::blocking::Proxy::new(&self.connection, COMPANION, COMPANION_PATH, COMPANION)?;
        let (accepted, serial): (bool, u32) = proxy
            .call("WriteFilesIfCurrent", &(expected, bytes))
            .context(
                "Enable or reload Desk Switch Tray in GNOME Extensions to share files on Wayland",
            )?;
        ensure!(
            serial != 0,
            "The clipboard companion returned an invalid revision"
        );
        self.serial = serial;
        self.paths = accepted.then(|| paths.to_vec());
        // A rejection reveals only the revision. Refetch the newer selection's
        // MIME and paths instead of treating it as an empty clipboard.
        self.refresh = !accepted;
        Ok(accepted)
    }
}

fn validate_path(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute()
            && !path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
            && path
                .to_str()
                .is_some_and(|text| !text.chars().any(char::is_control)),
        "File clipboard contains an unsupported local path"
    );
    Ok(())
}

fn decode_uri(uri: &str) -> Result<PathBuf> {
    let remainder = uri
        .strip_prefix("file://")
        .context("File clipboard contains a non-local URL")?;
    let encoded = if remainder.starts_with('/') {
        remainder
    } else {
        remainder
            .strip_prefix("localhost")
            .filter(|path| path.starts_with('/'))
            .context("File clipboard contains a non-local URL")?
    };
    ensure!(
        !encoded
            .chars()
            .any(|value| matches!(value, '?' | '#' | '\\'))
            && !encoded.bytes().any(|byte| byte.is_ascii_control()),
        "File clipboard contains an invalid file URL"
    );
    let mut decoded = Vec::with_capacity(encoded.len());
    let bytes = encoded.as_bytes();
    let mut position = 0;
    while position < bytes.len() {
        if bytes[position] == b'%' {
            let pair = bytes
                .get(position + 1..position + 3)
                .context("File clipboard contains an invalid URL escape")?;
            let hex = std::str::from_utf8(pair)?;
            let value = u8::from_str_radix(hex, 16)
                .context("File clipboard contains an invalid URL escape")?;
            decoded.push(value);
            position += 3;
        } else {
            decoded.push(bytes[position]);
            position += 1;
        }
    }
    let path = PathBuf::from(
        String::from_utf8(decoded).context("File clipboard contains a non-UTF-8 local path")?,
    );
    validate_path(&path)?;
    Ok(path)
}

fn parse_selection(mime: &str, bytes: &[u8]) -> Result<Option<Vec<PathBuf>>> {
    ensure!(
        bytes.len() <= LIMIT,
        "File clipboard exceeds the 1 MiB metadata limit"
    );
    let text = std::str::from_utf8(bytes).context("File clipboard is not valid UTF-8")?;
    let mut lines = text.lines();
    if mime == GNOME_MIME {
        match lines.next() {
            Some("copy") => {}
            // A cut operation never authorizes moving or deleting the originals.
            Some("cut") => return Ok(Some(Vec::new())),
            _ => bail!("File clipboard does not contain a copy operation"),
        }
    } else {
        ensure!(mime == URI_MIME, "Unsupported file clipboard format");
    }
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    for line in lines {
        if line.is_empty() || (mime == URI_MIME && line.starts_with('#')) {
            continue;
        }
        ensure!(
            paths.len() < ROOTS,
            "File clipboard exceeds 10000 selected items"
        );
        let path = decode_uri(line)?;
        ensure!(
            seen.insert(path.clone()),
            "File clipboard contains a duplicate path"
        );
        paths.push(path);
    }
    ensure!(!paths.is_empty(), "File clipboard contains no local files");
    Ok(Some(paths))
}

fn encode_paths(paths: &[PathBuf]) -> Result<Vec<u8>> {
    ensure!(
        !paths.is_empty() && paths.len() <= ROOTS,
        "File clipboard must contain between 1 and 10000 items"
    );
    let mut output = Vec::new();
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for (index, path) in paths.iter().enumerate() {
        validate_path(path)?;
        if index != 0 {
            output.extend_from_slice(b"\r\n");
        }
        output.extend_from_slice(b"file://");
        for &byte in path.to_str().unwrap().as_bytes() {
            if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
                output.push(byte);
            } else {
                output.extend_from_slice(&[
                    b'%',
                    HEX[(byte >> 4) as usize],
                    HEX[(byte & 15) as usize],
                ]);
            }
            ensure!(
                output.len() <= LIMIT,
                "File clipboard exceeds the 1 MiB metadata limit"
            );
        }
    }
    Ok(output)
}

#[path = "linux_x11.rs"]
mod x11;
use x11::X11;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn uri_lists_round_trip_spaces_unicode_delimiters_and_folders() {
        let paths = vec![
            PathBuf::from("/tmp/Shared folder/Привет #?.txt"),
            PathBuf::from("/tmp/a%20b"),
        ];
        let encoded = encode_paths(&paths).unwrap();
        assert_eq!(parse_selection(URI_MIME, &encoded).unwrap(), Some(paths));
        assert_eq!(
            decode_uri("file://localhost/tmp/a%20b").unwrap(),
            PathBuf::from("/tmp/a b")
        );
        assert_eq!(
            parse_selection(URI_MIME, b"# file copy\r\nfile:///tmp/a\r\n").unwrap(),
            Some(vec![PathBuf::from("/tmp/a")])
        );
    }

    #[test]
    fn no_remote_urls_paths_text_or_cut_operations_are_exported() {
        for value in [
            "https://example.invalid/a",
            "file://server/tmp/a",
            "file://localhost.evil/tmp/a",
            "file:///tmp/%00a",
            "file:///tmp/%0Aa",
            "file:///tmp/../secret",
            "file:///tmp/%2E%2E/secret",
            "file:///tmp/a?query",
            "file:///tmp/a#fragment",
            "file:///tmp/a%GG",
            "file:///tmp/a%",
            "file:///tmp/%FF",
        ] {
            assert!(decode_uri(value).is_err(), "{value}");
        }
        assert!(parse_selection("text/plain", b"/tmp/a").is_err());
        assert!(
            parse_selection(
                GNOME_MIME,
                b"copy\nfile:///tmp/a\nhttps://example.invalid/private"
            )
            .is_err()
        );
        assert_eq!(
            parse_selection(GNOME_MIME, b"cut\nfile:///tmp/a").unwrap(),
            Some(Vec::new())
        );
        assert_eq!(
            parse_selection(GNOME_MIME, b"copy\nfile:///tmp/a").unwrap(),
            Some(vec![PathBuf::from("/tmp/a")])
        );
        assert!(parse_selection(URI_MIME, b"file:///tmp/a\nfile:///tmp/a").is_err());
    }

    #[test]
    fn file_metadata_is_bounded_before_decoding_or_writing() {
        assert!(parse_selection(URI_MIME, &vec![b'x'; LIMIT + 1]).is_err());
        assert!(parse_selection(URI_MIME, b"\n# no files\n").is_err());
        assert!(encode_paths(&[]).is_err());
        assert!(encode_paths(&vec![PathBuf::from("/tmp/a"); ROOTS + 1]).is_err());
        assert!(encode_paths(&[PathBuf::from("/".to_owned() + &"a".repeat(LIMIT))]).is_err());
    }

    #[test]
    fn native_file_formats_use_a_private_x11_display_only() {
        use std::process::Command;
        const MARKER: &str = "DESK_SWITCH_PRIVATE_FILE_CLIPBOARD_X11";
        const PARENT: &str = "DESK_SWITCH_FILE_CLIPBOARD_PARENT_DISPLAY";
        if std::env::var(MARKER).as_deref() != Ok("1") {
            if !Path::new("/usr/bin/xvfb-run").is_file() {
                eprintln!("Private X11 file clipboard test skipped: xvfb-run is unavailable");
                return;
            }
            let output = Command::new("/usr/bin/xvfb-run")
                .args(["-a", "-s", "-screen 0 800x600x24"])
                .arg(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "group::clipboard_files::linux::tests::native_file_formats_use_a_private_x11_display_only",
                    "--nocapture",
                ])
                .env(MARKER, "1")
                .env(PARENT, std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".into()))
                .env("XDG_SESSION_TYPE", "x11")
                .env_remove("WAYLAND_DISPLAY")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "Private X11 clipboard test failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let display = std::env::var("DISPLAY").expect("Private Xvfb display is required");
        let parent = std::env::var(PARENT).expect("Parent display isolation marker is required");
        assert_ne!(
            display, parent,
            "The test must not touch the user's display"
        );
        assert!(!wayland());
        let root = std::env::temp_dir().join(format!(
            "desk-switch-file-clipboard-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir(&root).unwrap();
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _fixture = Fixture(root.clone());
        let file = root.join("Copied #1.txt");
        let folder = root.join("Copied folder");
        std::fs::write(&file, b"Synthetic file clipboard fixture").unwrap();
        std::fs::create_dir(&folder).unwrap();
        let expected = vec![file, folder];
        let mut text_only = arboard::Clipboard::new().unwrap();
        text_only
            .set_text(format!("file://{}", expected[0].display()))
            .unwrap();
        let mut native = X11::new().unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match native.read() {
                Ok(None) => break,
                Ok(Some(_)) => panic!("A plain-text path must never become an exported file"),
                Err(error) if Instant::now() < until => {
                    eprintln!("Waiting for the private selection owner: {error}");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("Private file clipboard failed: {error:#}"),
            }
        }
        let text_revision = native.sequence;
        // A user may explicitly recopy identical text while a file download is
        // pending. Its native revision must invalidate that paste reservation.
        text_only
            .set_text(format!("file://{}", expected[0].display()))
            .unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            if matches!(native.read(), Ok(None)) && native.sequence != text_revision {
                break;
            }
            assert!(
                Instant::now() < until,
                "An identical text copy did not advance its native revision"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !native.write_if_current(&expected, text_revision).unwrap(),
            "A newer identical text copy must invalidate the old reservation"
        );
        assert!(native.write_if_current(&expected, native.sequence).unwrap());
        let until = Instant::now() + Duration::from_secs(3);
        let actual = loop {
            if let Ok(Some(selection)) = native.read() {
                break selection;
            }
            assert!(
                Instant::now() < until,
                "Private native file selection was not received"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(actual.paths, expected);
        let repeated = native.read().unwrap().unwrap();
        assert_eq!(repeated.sequence, actual.sequence);
        assert_eq!(repeated.paths, expected);
        assert_eq!(
            text_only.get().file_list().unwrap(),
            expected,
            "A different native client must be able to paste the owned file selection"
        );
        // A different native owner copies the same files twice using exactly
        // the same acquisition timestamp. The reservation must still expire.
        let mut other_owner = X11::new().unwrap();
        other_owner.write(&expected).unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        let reserved = loop {
            if let Ok(Some(selection)) = native.read() {
                break selection.sequence;
            }
            assert!(
                Instant::now() < until,
                "The other private selection owner was not observed"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        other_owner.recopy_same_timestamp().unwrap();
        assert!(
            !native
                .write_if_current(&[expected[0].clone()], reserved)
                .unwrap(),
            "Same-owner, same-timestamp recopy must be detected inside the server grab"
        );
        assert_eq!(
            text_only.get().file_list().unwrap(),
            expected,
            "A rejected conditional write must preserve the newer owner's complete selection"
        );
        other_owner.recopy_same_timestamp().unwrap();
        // Metadata beyond an inline X property exercises the owner's bounded
        // outgoing INCR protocol on a separate, genuine native requestor.
        let mut many = Vec::new();
        for index in 0..400 {
            let path = root.join(format!("{:04}-{}.txt", index, "f".repeat(180)));
            std::fs::write(&path, []).unwrap();
            many.push(path);
        }
        assert!(encode_paths(&many).unwrap().len() > 48 * 1024);
        native.write(&many).unwrap();
        assert_eq!(
            text_only.get().file_list().unwrap(),
            many,
            "The dedicated owner must complete native INCR file-list requests"
        );
        native.rejects_old_selection_request().unwrap();
        native
            .timed_out_publication_keeps_selection(&[expected[0].clone()])
            .unwrap();
        assert_eq!(
            native.read().unwrap().unwrap().paths,
            many,
            "A publication queued past its caller's five-second deadline must remain rejected"
        );
        assert_eq!(
            text_only.get().file_list().unwrap(),
            many,
            "A timed-out caller must not take native ownership later"
        );
        assert_eq!(
            std::fs::read(&expected[0]).unwrap(),
            b"Synthetic file clipboard fixture"
        );
        assert!(expected[1].is_dir());
    }
}
