//! Optional cursor-region PNGs inside the existing authenticated Clipboard frame.
use anyhow::{Context, Result, ensure};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::watch,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};

pub const MIME: &str = "application/x-desk-switch-screen-options+json";
pub const MAX_PNG: usize = 40 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    pub enabled: bool,
    pub fps: u16,
    pub width: u16,
    pub height: u16,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            enabled: false,
            fps: 10,
            width: 384,
            height: 256,
        }
    }
}
impl Options {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=30).contains(&self.fps),
            "Choose a preview frame rate from 1 to 30 FPS"
        );
        ensure!(
            (128..=640).contains(&self.width) && (96..=480).contains(&self.height),
            "Invalid preview region size"
        );
        Ok(())
    }
    pub fn packet(self, source: &str) -> Result<Value> {
        self.validate()?;
        packet(source, MIME, &serde_json::to_vec(&self)?)
    }
}

#[derive(Clone, Debug)]
pub enum Frame {
    Png(Arc<Vec<u8>>),
    Unavailable(String),
}
impl Frame {
    pub fn packet(&self, source: &str) -> Result<Value> {
        match self {
            Self::Png(bytes) => {
                ensure!(bytes.len() <= MAX_PNG, "Cursor preview is too large");
                packet(source, "image/png", bytes)
            }
            Self::Unavailable(reason) => Ok(json!({"Error": format!("Cursor preview: {reason}")})),
        }
    }
}
fn packet(source: &str, mime: &str, bytes: &[u8]) -> Result<Value> {
    crate::config::key(source)?;
    Ok(json!({"Clipboard": { "source": source, "mime": mime,
        "bytes": base64::engine::general_purpose::STANDARD.encode(bytes),
        "digest": crate::config::hex(&Sha256::digest(bytes)) }}))
}

/// Other Clipboard records remain owned by the existing text adapter.
pub fn parse(value: &Value, peer: &str) -> Result<Option<Options>> {
    let Some(clipboard) = value.get("Clipboard") else {
        return Ok(None);
    };
    if clipboard.get("mime").and_then(Value::as_str) != Some(MIME) {
        return Ok(None);
    }
    ensure!(
        clipboard.get("source").and_then(Value::as_str) == Some(peer),
        "Invalid preview source"
    );
    let encoded = clipboard
        .get("bytes")
        .and_then(Value::as_str)
        .context("Missing preview options")?;
    ensure!(encoded.len() <= 1024, "Preview options are too large");
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)?;
    let digest = crate::config::hex(&Sha256::digest(bytes.as_slice()));
    ensure!(
        clipboard.get("digest").and_then(Value::as_str) == Some(digest.as_str()),
        "Preview options checksum mismatch"
    );
    let options: Options = serde_json::from_slice(&bytes)?;
    options.validate()?;
    Ok(Some(options))
}

pub fn available() -> bool {
    crate::platform::screen::available()
}

// All viewers use one region size and share one capture/encode budget.
fn capture_shared() -> Result<Option<Arc<Vec<u8>>>> {
    use std::sync::{Mutex, OnceLock, Weak};
    use std::time::Instant;
    #[derive(Default)]
    struct Shared {
        device: crate::platform::screen::Capture,
        last: Option<Instant>,
        cached: Weak<Vec<u8>>,
    }
    static SHARED: OnceLock<Mutex<Shared>> = OnceLock::new();
    let Ok(mut state) = SHARED.get_or_init(Default::default).try_lock() else {
        return Ok(None);
    };
    if state
        .last
        .is_some_and(|time| time.elapsed() < Duration::from_micros(33_334))
    {
        return Ok(state.cached.upgrade());
    }
    state.last = Some(Instant::now());
    let frame = Arc::new(state.device.frame(Options::default())?);
    state.cached = Arc::downgrade(&frame);
    Ok(Some(frame))
}

#[derive(Clone, Copy)]
pub(super) enum Provider {
    Native,
    #[cfg(test)]
    Synthetic,
}
impl Provider {
    pub(super) fn available(self) -> bool {
        match self {
            Self::Native => available(),
            #[cfg(test)]
            Self::Synthetic => true,
        }
    }

    fn frame(self) -> Result<Option<Arc<Vec<u8>>>> {
        match self {
            Self::Native => capture_shared(),
            #[cfg(test)]
            Self::Synthetic => {
                use image::ImageEncoder;
                let mut bytes = Vec::new();
                image::codecs::png::PngEncoder::new(&mut bytes).write_image(
                    &[31, 92, 177],
                    1,
                    1,
                    image::ExtendedColorType::Rgb8,
                )?;
                Ok(Some(Arc::new(bytes)))
            }
        }
    }
}

/// One pending frame, no timer or captures while disabled. Consumers may drop
/// intermediate frames; stopping never flushes a captured frame after revocation.
pub(super) fn start(
    provider: Provider,
    mut options: watch::Receiver<Options>,
    mut allowed: watch::Receiver<bool>,
    mut stop: watch::Receiver<bool>,
) -> (watch::Receiver<Option<Frame>>, JoinHandle<()>) {
    let (send, frames) = watch::channel(None);
    let task = tokio::spawn(async move {
        let mut last_digest = None;
        loop {
            if *stop.borrow() {
                return;
            }
            let current = *options.borrow_and_update();
            let permitted = *allowed.borrow_and_update();
            if !current.enabled || !permitted {
                send.send_replace(None);
                last_digest = None;
                tokio::select! {
                    value = options.changed() => if value.is_err() { return },
                    value = allowed.changed() => if value.is_err() { return },
                    _ = stop.changed() => {},
                }
                continue;
            }
            if !provider.available() {
                send.send_replace(Some(Frame::Unavailable(
                    "Cursor preview is unavailable in this desktop session".into(),
                )));
                tokio::select! {
                    value = options.changed() => if value.is_err() { return },
                    value = allowed.changed() => if value.is_err() { return },
                    _ = stop.changed() => {},
                }
                continue;
            }
            let mut timer = interval(Duration::from_secs_f64(1.0 / f64::from(current.fps)));
            timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = stop.changed() => break,
                    _ = options.changed() => break,
                    _ = allowed.changed() => break,
                    _ = timer.tick() => {},
                }
                if *stop.borrow() || !*allowed.borrow() || *options.borrow() != current {
                    break;
                }
                let result = tokio::task::spawn_blocking(move || provider.frame()).await;
                if *stop.borrow() || !*allowed.borrow() || *options.borrow() != current {
                    break;
                }
                match result {
                    Ok(Ok(Some(bytes))) => {
                        let digest = Sha256::digest(bytes.as_slice());
                        if last_digest.as_ref() != Some(&digest) {
                            last_digest = Some(digest);
                            send.send_replace(Some(Frame::Png(bytes)));
                        }
                    }
                    Ok(Ok(None)) => {}
                    _ => {
                        send.send_replace(Some(Frame::Unavailable(
                            "Cursor preview could not capture this desktop".into(),
                        )));
                        // An error waits for an explicit options/permission change.
                        tokio::select! { _ = stop.changed() => {}, _ = options.changed() => {}, _ = allowed.changed() => {} }
                        break;
                    }
                }
            }
        }
    });
    (frames, task)
}

pub(super) fn parse_frame(value: &Value, peer: &str) -> Result<Option<Frame>> {
    if let Some(reason) = value.get("Error").and_then(Value::as_str)
        && let Some(reason) = reason.strip_prefix("Cursor preview: ")
    {
        return Ok(Some(Frame::Unavailable(reason.chars().take(160).collect())));
    }
    let Some(record) = value.get("Clipboard") else {
        return Ok(None);
    };
    if record.get("mime").and_then(Value::as_str) != Some("image/png") {
        return Ok(None);
    }
    ensure!(
        record.get("source").and_then(Value::as_str) == Some(peer),
        "Invalid preview frame source"
    );
    let encoded = record
        .get("bytes")
        .and_then(Value::as_str)
        .context("Missing preview frame")?;
    ensure!(
        encoded.len() <= MAX_PNG.div_ceil(3) * 4,
        "Preview frame is too large"
    );
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)?;
    ensure!(
        bytes.len() <= MAX_PNG
            && bytes.len() >= 33
            && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
            && bytes[8..12] == 13u32.to_be_bytes()
            && &bytes[12..16] == b"IHDR",
        "Invalid preview PNG"
    );
    let width = u32::from_be_bytes(bytes[16..20].try_into()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into()?);
    ensure!(
        (1..=640).contains(&width) && (1..=480).contains(&height),
        "Invalid preview frame dimensions"
    );
    let digest = crate::config::hex(&Sha256::digest(&bytes));
    ensure!(
        record.get("digest").and_then(Value::as_str) == Some(digest.as_str()),
        "Preview frame checksum mismatch"
    );
    Ok(Some(Frame::Png(Arc::new(bytes))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requests_use_original_clipboard_container_and_require_source_digest_and_bounds() {
        let key = "01".repeat(32);
        let options = Options {
            enabled: true,
            fps: 24,
            ..Default::default()
        };
        let value = options.packet(&key).unwrap();
        assert_eq!(parse(&value, &key).unwrap(), Some(options));
        assert!(parse(&value, &"02".repeat(32)).is_err());
        let mut damaged = value.clone();
        damaged["Clipboard"]["digest"] = json!("00".repeat(32));
        assert!(parse(&damaged, &key).is_err());
        assert!(Options { fps: 31, ..options }.packet(&key).is_err());
        assert!(
            Options {
                width: 4096,
                ..options
            }
            .packet(&key)
            .is_err()
        );
        assert!(parse(&json!("Ping"), &key).unwrap().is_none());
    }
    #[test]
    fn maximum_preview_stays_inside_the_existing_encrypted_record_limit() {
        let value = Frame::Png(Arc::new(vec![255; MAX_PNG]))
            .packet(&"01".repeat(32))
            .unwrap();
        assert!(serde_json::to_vec(&value).unwrap().len() + 16 < super::super::control::LIMIT);
    }
    #[test]
    fn relay_checks_frame_identity_dimensions_and_checksum() {
        let key = "01".repeat(32);
        let frame = Frame::Png(Provider::Synthetic.frame().unwrap().unwrap());
        let record = frame.packet(&key).unwrap();
        assert!(matches!(
            parse_frame(&record, &key).unwrap(),
            Some(Frame::Png(_))
        ));
        assert!(parse_frame(&record, &"02".repeat(32)).is_err());
        let mut bad = record;
        bad["Clipboard"]["digest"] = json!("00".repeat(32));
        assert!(parse_frame(&bad, &key).is_err());
        assert!(matches!(
            parse_frame(&Frame::Unavailable("Paused".into()).packet(&key).unwrap(), &key).unwrap(),
            Some(Frame::Unavailable(reason)) if reason == "Paused"
        ));
    }
}
