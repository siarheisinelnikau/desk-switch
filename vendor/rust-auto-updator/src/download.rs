//! Repository-bound transfers shared by executable and package installers.
use crate::{
    Config,
    github::{Release, agent, asset_digest, network_error, read_text},
    verify::{checksum, hash, single_asset_checksum},
};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub(crate) fn download(
    release: &Release,
    config: &Config,
    path: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<String, String> {
    config.validate()?;
    check_cancel(cancel)?;
    if release.source != config.repository_url()
        || release
            .version
            .cmp_precedence(&config.current_version)
            .is_le()
        || !config
            .executable_names(&release.version)
            .contains(&release.executable.name)
        || release
            .checksum
            .as_ref()
            .is_some_and(|asset| asset.name != config.checksum_filename(&release.executable.name))
        || (release.checksum.is_none() && !config.allow_asset_digest)
    {
        return Err("The release belongs to a different updater configuration.".into());
    }
    let agent = agent(config);
    let digest = asset_digest(&release.executable)?;
    let expected = if let Some(asset) = &release.checksum {
        let text = read_text(&agent, &asset.browser_download_url, 128 * 1024)?;
        if config.checksum_name.contains("{executable}") {
            single_asset_checksum(&text, &release.executable.name)?
        } else {
            checksum(&text, &release.executable.name)?
        }
    } else {
        digest
            .clone()
            .ok_or("The release has no SHA-256 verification source.")?
    };
    check_cancel(cancel)?;
    if let Some(digest) = digest
        && digest != expected
    {
        return Err("GitHub's digest and checksum file do not agree.".into());
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let mut response = agent
        .get(&release.executable.browser_download_url)
        .call()
        .map_err(network_error)?;
    let mut reader = response.body_mut().as_reader();
    let mut buffer = [0u8; 64 * 1024];
    let mut received = 0;
    let mut notified = Instant::now();
    loop {
        check_cancel(cancel)?;
        let n = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        received += n as u64;
        if received > release.executable.size || received > config.max_executable_size {
            return Err("The download exceeded the expected size.".into());
        }
        file.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        if notified.elapsed() > Duration::from_millis(100) {
            progress(received, release.executable.size);
            notified = Instant::now();
        }
    }
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);

    if fs::metadata(path).map_err(|e| e.to_string())?.len() != release.executable.size
        || hash(path)? != expected
    {
        return Err("The download failed SHA-256 verification. Nothing was installed.".into());
    }
    check_cancel(cancel)?;
    progress(received, received);
    Ok(expected)
}

pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Update download cancelled.".into())
    } else {
        Ok(())
    }
}
