//! Repository-bound transfers shared by executable and package installers.
use crate::{
    Config,
    github::{Asset, Release, agent, asset_api_url, asset_digest, network_error, read_text},
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
    let response = initial_asset_response(
        &release.executable,
        config,
        cancel,
        |url, api| {
            let request = agent.get(url);
            if api {
                request.header("Accept", "application/octet-stream").call()
            } else {
                request.call()
            }
        },
        retry_pause,
    )?;
    write_verified(
        release,
        config,
        path,
        cancel,
        &mut progress,
        &expected,
        response.into_body().into_reader(),
    )?;
    Ok(expected)
}

fn transient_request(error: &ureq::Error) -> bool {
    matches!(
        error,
        ureq::Error::StatusCode(502 | 503 | 504) | ureq::Error::Timeout(_)
    ) || matches!(error, ureq::Error::Io(error) if error.kind() == std::io::ErrorKind::TimedOut)
}

/// Only acquisition of response headers can retry. Once a body is accepted,
/// reads, length and SHA-256 errors cannot select a different download route.
fn initial_asset_response<T>(
    asset: &Asset,
    config: &Config,
    cancel: &AtomicBool,
    mut request: impl FnMut(&str, bool) -> Result<T, ureq::Error>,
    mut pause: impl FnMut(&AtomicBool, Duration) -> Result<(), String>,
) -> Result<T, String> {
    let api = asset_api_url(asset, config)?;
    for attempt in 0..2 {
        check_cancel(cancel)?;
        match request(&asset.browser_download_url, false) {
            Ok(response) => return Ok(response),
            Err(error) if transient_request(&error) => {
                if attempt == 0 {
                    pause(cancel, Duration::from_millis(250))?;
                } else if let Some(api) = api {
                    pause(cancel, Duration::from_millis(500))?;
                    check_cancel(cancel)?;
                    return request(api, true).map_err(network_error);
                } else {
                    return Err(network_error(error));
                }
            }
            Err(error) => return Err(network_error(error)),
        }
    }
    unreachable!()
}

fn retry_pause(cancel: &AtomicBool, duration: Duration) -> Result<(), String> {
    let until = Instant::now() + duration;
    loop {
        check_cancel(cancel)?;
        let remaining = until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        std::thread::sleep(remaining.min(Duration::from_millis(25)));
    }
}

fn write_verified(
    release: &Release,
    config: &Config,
    path: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
    expected: &str,
    mut reader: impl Read,
) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
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
    Ok(())
}

pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Update download cancelled.".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{collections::VecDeque, io::Cursor, path::PathBuf};

    fn fixture() -> (Config, Release, Vec<u8>, String) {
        let mut config = Config::new("owner", "app", "1.0.0", "App").unwrap();
        config.allow_asset_digest = true;
        let bytes = vec![b'x'; 2048];
        let expected = format!("{:x}", Sha256::digest(&bytes));
        let name = config.executable_name(&"1.1.0".parse().unwrap());
        let json = serde_json::json!({"tag_name":"v1.1.0","html_url":"https://github.com/owner/app/releases/tag/v1.1.0","draft":false,"prerelease":false,"assets":[{
            "name":name,"size":bytes.len(),"digest":format!("sha256:{expected}"),"browser_download_url":format!("https://github.com/owner/app/releases/download/v1.1.0/{name}"),
            "id":123,"url":"https://api.github.com/repos/owner/app/releases/assets/123"}]});
        let release = crate::parse_release(&json.to_string(), &config)
            .unwrap()
            .unwrap();
        (config, release, bytes, expected)
    }

    struct Folder(PathBuf);
    impl Folder {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "updater-transfer-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn transient_headers_retry_then_use_only_the_official_api_and_verify_the_complete_body() {
        let (config, release, bytes, expected) = fixture();
        let cancel = AtomicBool::new(false);
        let mut responses = VecDeque::from([
            Err(ureq::Error::StatusCode(504)),
            Err(ureq::Error::Timeout(ureq::Timeout::RecvResponse)),
            Ok(Cursor::new(bytes.clone())),
        ]);
        let mut calls = Vec::new();
        let body = initial_asset_response(
            &release.executable,
            &config,
            &cancel,
            |url, api| {
                calls.push((url.to_owned(), api));
                responses.pop_front().unwrap()
            },
            |_, _| Ok(()),
        )
        .unwrap();
        assert_eq!(
            calls,
            vec![
                (release.executable.browser_download_url.clone(), false),
                (release.executable.browser_download_url.clone(), false),
                (release.executable.url.clone().unwrap(), true)
            ]
        );
        let folder = Folder::new();
        let path = folder.0.join("ready");
        let mut finished = None;
        write_verified(
            &release,
            &config,
            &path,
            &cancel,
            |received, total| finished = Some((received, total)),
            &expected,
            body,
        )
        .unwrap();
        assert_eq!(fs::read(path).unwrap(), bytes);
        assert_eq!(finished, Some((2048, 2048)));
    }

    #[test]
    fn primary_success_and_nontransient_errors_never_use_another_route() {
        let (config, release, _, _) = fixture();
        let cancel = AtomicBool::new(false);
        for error in [
            ureq::Error::StatusCode(403),
            ureq::Error::StatusCode(404),
            ureq::Error::StatusCode(429),
            ureq::Error::StatusCode(500),
            ureq::Error::Tls("certificate rejected"),
        ] {
            let mut calls = 0;
            let mut error = Some(error);
            let result: Result<(), String> = initial_asset_response(
                &release.executable,
                &config,
                &cancel,
                |_, api| {
                    assert!(!api);
                    calls += 1;
                    Err(error.take().unwrap())
                },
                |_, _| panic!("Nontransient failure was retried"),
            );
            assert!(result.is_err());
            assert_eq!(calls, 1);
        }
        let mut calls = 0;
        assert_eq!(
            initial_asset_response(
                &release.executable,
                &config,
                &cancel,
                |_, api| {
                    assert!(!api);
                    calls += 1;
                    Ok(17)
                },
                |_, _| panic!("Successful request was retried")
            )
            .unwrap(),
            17
        );
        assert_eq!(calls, 1);
        assert!(transient_request(&ureq::Error::Io(std::io::Error::from(
            std::io::ErrorKind::TimedOut
        ))));
    }

    #[test]
    fn retry_budget_is_finite_and_a_terminal_retry_never_falls_back() {
        let (config, release, _, _) = fixture();
        for statuses in [vec![502, 503, 504], vec![504, 404]] {
            let expected = statuses.len();
            let mut statuses = VecDeque::from(statuses);
            let mut calls = 0;
            let result: Result<(), String> = initial_asset_response(
                &release.executable,
                &config,
                &AtomicBool::new(false),
                |_, _| {
                    calls += 1;
                    Err(ureq::Error::StatusCode(
                        statuses.pop_front().expect("Retry budget was exceeded"),
                    ))
                },
                |_, _| Ok(()),
            );
            assert!(result.is_err());
            assert_eq!(calls, expected);
        }
        let mut legacy = release.executable.clone();
        legacy.id = None;
        legacy.url = None;
        let mut calls = 0;
        let result: Result<(), String> = initial_asset_response(
            &legacy,
            &config,
            &AtomicBool::new(false),
            |_, api| {
                assert!(!api);
                calls += 1;
                Err(ureq::Error::StatusCode(504))
            },
            |_, _| Ok(()),
        );
        assert!(result.is_err());
        assert_eq!(calls, 2);
    }

    #[test]
    fn cancellation_during_backoff_prevents_every_later_request() {
        let (config, release, _, _) = fixture();
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let result: Result<(), String> = initial_asset_response(
            &release.executable,
            &config,
            &cancel,
            |_, _| {
                calls += 1;
                Err(ureq::Error::StatusCode(504))
            },
            |cancel, _| {
                cancel.store(true, Ordering::Release);
                Ok(())
            },
        );
        assert!(result.unwrap_err().contains("cancelled"));
        assert_eq!(calls, 1);
    }

    #[test]
    fn invalid_api_metadata_is_rejected_before_any_request() {
        let (config, mut release, _, _) = fixture();
        release.executable.url =
            Some("https://api.github.com/repos/attacker/app/releases/assets/123".into());
        let result: Result<(), String> = initial_asset_response(
            &release.executable,
            &config,
            &AtomicBool::new(false),
            |_, _| panic!("Foreign API address reached transport"),
            |_, _| Ok(()),
        );
        assert!(result.is_err());
    }

    #[test]
    fn accepted_body_failures_never_retry_or_reuse_partial_files() {
        let (config, release, bytes, expected) = fixture();
        let cancel = AtomicBool::new(false);
        for (failures, damaged) in [
            (0, vec![b'y'; bytes.len()]),
            (2, vec![b'y'; bytes.len()]),
            (2, vec![b'x'; bytes.len() + 1]),
            (2, vec![b'x'; bytes.len() - 1]),
        ] {
            let folder = Folder::new();
            let path = folder.0.join("partial");
            let mut calls = 0;
            let body = initial_asset_response(
                &release.executable,
                &config,
                &cancel,
                |_, _| {
                    calls += 1;
                    if calls <= failures {
                        Err(ureq::Error::StatusCode(504))
                    } else {
                        Ok(Cursor::new(damaged.clone()))
                    }
                },
                |_, _| Ok(()),
            )
            .unwrap();
            assert!(
                write_verified(
                    &release,
                    &config,
                    &path,
                    &cancel,
                    |_, _| {},
                    &expected,
                    body
                )
                .is_err()
            );
            assert_eq!(calls, failures + 1);
            let before = fs::read(&path).unwrap();
            assert!(
                write_verified(
                    &release,
                    &config,
                    &path,
                    &cancel,
                    |_, _| {},
                    &expected,
                    Cursor::new(bytes.clone())
                )
                .is_err()
            );
            assert_eq!(fs::read(path).unwrap(), before);
        }
        struct FailedRead;
        impl Read for FailedRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::TimedOut.into())
            }
        }
        let folder = Folder::new();
        let path = folder.0.join("partial");
        assert!(
            write_verified(
                &release,
                &config,
                &path,
                &cancel,
                |_, _| {},
                &expected,
                Cursor::new(vec![b'x'; 128]).chain(FailedRead)
            )
            .is_err()
        );
        assert_eq!(fs::metadata(path).unwrap().len(), 128);
    }
}
