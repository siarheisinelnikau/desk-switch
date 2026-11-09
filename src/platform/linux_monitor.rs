use crate::{
    config::hex,
    video::{Monitor, input_codes},
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Display {
    id: String,
    connector: String,
    bus: Option<u32>,
    fingerprint: String,
}

fn displays() -> Result<Vec<Display>> {
    let mut displays = Vec::new();
    for entry in fs::read_dir("/sys/class/drm").context("reading Linux DRM connectors")? {
        let entry = entry?;
        let path = entry.path();
        let Ok(edid) = fs::read(path.join("edid")) else {
            continue;
        };
        if edid.len() < 128 || !edid.starts_with(&[0, 255, 255, 255, 255, 255, 255, 0]) {
            continue;
        }
        let connector = entry.file_name().to_string_lossy().into_owned();
        let id = format!("{connector}:{}", hex(&Sha256::digest(&edid)));
        let bus = fs::canonicalize(path.join("ddc")).ok().and_then(|path| {
            path.file_name()?
                .to_str()?
                .strip_prefix("i2c-")?
                .parse()
                .ok()
        });
        displays.push(Display {
            id,
            connector,
            bus,
            fingerprint: crate::workspace::hardware::display_key(&edid),
        });
    }
    displays.sort_by(|a, b| a.connector.cmp(&b.connector));
    Ok(displays)
}

fn selected_bus(displays: &[Display], id: &str) -> Result<u32> {
    let mut matches = displays.iter().filter(|display| display.id == id);
    let display = matches
        .next()
        .context("configured monitor identity is unavailable; run probe on its active input")?;
    ensure!(
        matches.next().is_none(),
        "configured monitor identity is ambiguous"
    );
    let bus = display
        .bus
        .context("this connector exposes no DRM DDC/I²C adapter")?;
    ensure!(
        displays
            .iter()
            .filter(|display| display.bus == Some(bus))
            .count()
            == 1,
        "multiple physical displays share the selected DDC bus"
    );
    Ok(bus)
}

fn ddcutil(bus: u32, args: &[&str]) -> Result<String> {
    use std::os::unix::process::CommandExt;
    let parent = std::process::id();
    let mut command = Command::new("ddcutil");
    // SAFETY: pre_exec runs between fork and exec in the child, where only
    // async-signal-safe operations are allowed. The closure sets the parent
    // death signal and reports failures through the exec pipe.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() as u32 != parent {
                libc::raise(libc::SIGTERM);
                return Err(std::io::Error::from_raw_os_error(libc::ECANCELED));
            }
            Ok(())
        });
    }
    let mut child = command
        .arg("--skip-ddc-checks")
        .arg("--bus")
        .arg(bus.to_string())
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting ddcutil; install it and check your I²C device access")
        .map_err(crate::video::not_issued)?;
    if args.contains(&"setvcp") {
        println!(
            "DDC write process {}: input {}; checks, verification and write retries disabled.",
            child.id(),
            args.last().unwrap_or(&"unknown")
        );
    }
    // Drain both pipes while the process runs, with fixed memory and a bounded child lifetime.
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let drain = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || -> std::io::Result<Vec<u8>> {
            let mut output = Vec::new();
            pipe.by_ref().take(32 * 1024).read_to_end(&mut output)?;
            std::io::copy(&mut pipe, &mut std::io::sink())?;
            Ok(output)
        })
    };
    let stdout = drain(Box::new(stdout));
    let stderr = drain(Box::new(stderr));
    let end = Instant::now() + Duration::from_secs(3);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < end => thread::sleep(Duration::from_millis(20)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                result.context("waiting for ddcutil")?;
                bail!("ddcutil timed out on /dev/i2c-{bus}");
            }
        }
    };
    let output = String::from_utf8_lossy(
        &stdout
            .join()
            .map_err(|_| anyhow!("ddcutil stdout reader failed"))??,
    )
    .into_owned();
    let error = String::from_utf8_lossy(
        &stderr
            .join()
            .map_err(|_| anyhow!("ddcutil stderr reader failed"))??,
    )
    .into_owned();
    ensure!(
        status.success(),
        "DDC/CI VCP 0x60 unavailable on /dev/i2c-{bus}: {} {}",
        output.trim(),
        error.trim()
    );
    Ok(output)
}

fn parse_input(output: &str) -> Result<u16> {
    for line in output.lines() {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        let hex = |value: &str| -> Result<u16> {
            Ok(u16::from_str_radix(
                value.strip_prefix('x').context("missing VCP hex prefix")?,
                16,
            )?)
        };
        match parts.as_slice() {
            ["VCP", "60", "SNC", value] => return hex(value),
            ["VCP", "60", "CNC" | "CND", _, _, hi, lo] => return Ok(hex(hi)? << 8 | hex(lo)?),
            ["VCP", "60", "C", value, _] => return Ok(value.parse()?),
            _ => {}
        }
    }
    bail!(
        "ddcutil did not report a readable VCP 0x60 input: {}",
        output.trim()
    )
}

fn read_input(bus: u32) -> Result<u16> {
    parse_input(&ddcutil(bus, &["--mccs=2.2", "--terse", "getvcp", "60"])?)
}

pub struct Ddc(pub String);

impl Monitor for Ddc {
    fn read(&mut self) -> Result<u16> {
        read_input(selected_bus(&displays()?, &self.0)?)
    }

    fn write(&mut self, input: u16) -> Result<()> {
        let bus = displays()
            .and_then(|displays| selected_bus(&displays, &self.0))
            .map_err(crate::video::not_issued)?;
        ddcutil(
            bus,
            &[
                "--noverify",
                "--mccs=2.2",
                "--maxtries=1,0,0",
                "setvcp",
                "60",
                &format!("0x{input:04x}"),
            ],
        )?;
        Ok(())
    }
}

pub fn probe(only: Option<&str>) -> Result<()> {
    let displays = displays()?;
    ensure!(
        !displays.is_empty(),
        "no Linux DRM connectors expose a monitor EDID"
    );
    if let Some(id) = only {
        probe_one(&displays, id)
    } else {
        for display in &displays {
            if let Err(error) = probe_one(&displays, &display.id) {
                println!("  {error:#}");
            }
        }
        Ok(())
    }
}

fn probe_one(displays: &[Display], id: &str) -> Result<()> {
    println!("Monitor ID: {id}");
    let bus = selected_bus(displays, id)?;
    println!("  Current VCP 0x60: {:#06x}", read_input(bus)?);
    let codes = advertised_inputs(bus);
    match codes {
        Ok(codes) if !codes.is_empty() => println!("  Advertised inputs: {codes:#06x?}"),
        _ => println!(
            "  Monitor did not advertise an input list; read each active input during setup."
        ),
    }
    Ok(())
}

fn advertised_inputs(bus: u32) -> Result<Vec<u16>> {
    ddcutil(bus, &["capabilities"]).map(|caps| {
        let raw = caps
            .lines()
            .any(|line| line.trim_start().starts_with("vcp("));
        if raw {
            input_codes(&caps)
        } else {
            // Some ddcutil versions print human-readable capabilities instead of the raw string.
            let mut lines = caps
                .lines()
                .skip_while(|line| !line.contains("Feature: 60"));
            lines.next();
            lines
                .take_while(|line| !line.contains("Feature:"))
                .filter_map(|line| {
                    let (value, _) = line.trim().split_once(':')?;
                    if value.len() != 2 {
                        return None;
                    }
                    u16::from_str_radix(value, 16).ok()
                })
                .collect()
        }
    })
}

pub fn discover() -> Result<Vec<super::MonitorInfo>> {
    let displays = displays()?;
    let native = super::linux_geometry::screens(
        &displays
            .iter()
            .map(|display| display.id.clone())
            .collect::<Vec<_>>(),
    );
    Ok(displays
        .iter()
        .map(|display| {
            let read = selected_bus(&displays, &display.id)
                .and_then(|bus| Ok((read_input(bus)?, advertised_inputs(bus).unwrap_or_default())));
            let (current, inputs, error) = match read {
                Ok((value, inputs)) => (Some(value), inputs, None),
                Err(error) => (None, Vec::new(), Some(format!("{error:#}"))),
            };
            super::MonitorInfo {
                id: display.id.clone(),
                name: display.connector.clone(),
                screen: display.connector.clone(),
                current,
                inputs,
                error,
                fingerprint: display.fingerprint.clone(),
                native: native.get(&display.id).cloned(),
            }
        })
        .collect())
}

pub fn list_displays() -> Result<Vec<super::MonitorInfo>> {
    let displays = displays()?;
    let native = super::linux_geometry::screens(
        &displays
            .iter()
            .map(|display| display.id.clone())
            .collect::<Vec<_>>(),
    );
    Ok(displays
        .into_iter()
        .map(|display| super::MonitorInfo {
            native: native.get(&display.id).cloned(),
            id: display.id,
            name: display.connector.clone(),
            screen: display.connector,
            current: None,
            inputs: Vec::new(),
            error: None,
            fingerprint: display.fingerprint,
        })
        .collect())
}

pub fn read_monitor(id: &str) -> Result<super::MonitorInfo> {
    let displays = displays()?;
    let display = displays
        .iter()
        .find(|d| d.id == id)
        .context("Выбранный монитор отключён")?;
    let current = crate::video::read_once(Ddc(id.into()))?;
    Ok(super::MonitorInfo {
        id: id.into(),
        name: display.connector.clone(),
        screen: display.connector.clone(),
        current: Some(current),
        inputs: Vec::new(),
        error: None,
        fingerprint: display.fingerprint.clone(),
        native: super::linux_geometry::screens(&[id.into()]).remove(id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_selects_the_matching_connector_and_refuses_a_shared_bus() {
        let mut fixture = vec![
            Display {
                id: "private".into(),
                connector: "card0-DP-1".into(),
                bus: Some(2),
                fingerprint: String::new(),
            },
            Display {
                id: "shared".into(),
                connector: "card0-HDMI-A-1".into(),
                bus: Some(5),
                fingerprint: String::new(),
            },
        ];
        assert_eq!(selected_bus(&fixture, "shared").unwrap(), 5);
        assert!(selected_bus(&fixture, "missing").is_err());
        fixture[0].bus = Some(5);
        assert!(selected_bus(&fixture, "shared").is_err());
    }

    #[test]
    fn parses_only_valid_vcp_input_reports() {
        assert_eq!(parse_input("VCP 60 SNC x11\n").unwrap(), 0x11);
        assert_eq!(parse_input("VCP 60 CNC x00 x00 x01 x11\n").unwrap(), 0x111);
        assert!(parse_input("VCP 60 ERR").is_err());
    }
}
