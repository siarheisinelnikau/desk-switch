use crate::video::Monitor;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Deserialize)]
struct Display {
    id: String,
    name: String,
    screen: String,
    fingerprint: String,
    builtin: bool,
}
fn displays() -> Result<Vec<Display>> {
    crate::macos_native::query("displays", &[])
}
fn identity(id: &str) -> Result<String> {
    let display=displays()?.into_iter().find(|display|display.id==id).context("The configured macOS display is unavailable; select it again after changing the display connection")?;
    ensure!(
        !display.builtin,
        "The built-in display has no switchable input"
    );
    Ok(display
        .id
        .splitn(3, ':')
        .nth(2)
        .context("Invalid macOS display identity")?
        .into())
}
#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    issued: bool,
    value: Option<u16>,
    error: Option<String>,
}
fn request(operation: &str, identity: String, input: Option<u16>) -> Result<Response> {
    let mut args = vec![identity];
    if let Some(input) = input {
        args.push(input.to_string());
    }
    crate::macos_native::query_command(
        operation,
        crate::macos_native::ddc_command(operation)?,
        &args,
    )
}
pub struct Ddc(pub String);
impl Monitor for Ddc {
    fn read(&mut self) -> Result<u16> {
        let response = request("read", identity(&self.0)?, None)?;
        if let Some(error) = response.error {
            anyhow::bail!(error);
        }
        response
            .value
            .filter(|input| *input != 0)
            .context("The monitor did not return a valid input")
    }
    fn write(&mut self, input: u16) -> Result<()> {
        let selected = identity(&self.0).map_err(crate::video::not_issued)?;
        // Exactly one selection; no pre-read, retry, LG alternate code or readback.
        let response = request("select", selected, Some(input))?;
        if let Some(error) = response.error {
            let error = anyhow::anyhow!(error);
            return Err(if response.issued {
                error
            } else {
                crate::video::not_issued(error)
            });
        }
        ensure!(
            response.issued,
            "The macOS helper did not issue the selection"
        );
        Ok(())
    }
}
pub fn list_displays() -> Result<Vec<super::MonitorInfo>> {
    Ok(displays()?
        .into_iter()
        .map(|display| super::MonitorInfo {
            id: display.id,
            name: display.name,
            screen: display.screen,
            current: None,
            inputs: Vec::new(),
            error: display
                .builtin
                .then(|| "The built-in display has no switchable input".into()),
            fingerprint: display.fingerprint,
        })
        .collect())
}
pub fn read_monitor(id: &str) -> Result<super::MonitorInfo> {
    let mut info = list_displays()?
        .into_iter()
        .find(|display| display.id == id)
        .context("The selected macOS display is unavailable")?;
    info.current = Some(Ddc(id.into()).read()?);
    Ok(info)
}
pub fn discover() -> Result<Vec<super::MonitorInfo>> {
    Ok(list_displays()?
        .into_iter()
        .map(|mut display| {
            match Ddc(display.id.clone()).read() {
                Ok(input) => display.current = Some(input),
                Err(error) => display.error = Some(error.to_string()),
            };
            display
        })
        .collect())
}
pub fn probe(only: Option<&str>) -> Result<()> {
    for display in list_displays()?
        .into_iter()
        .filter(|display| only.is_none_or(|id| id == display.id))
    {
        println!("Monitor ID: {} ({})", display.id, display.name);
        match Ddc(display.id).read() {
            Ok(input) => println!("  Current VCP 0x60: {input:#06x}"),
            Err(error) => println!("  {error:#}"),
        }
    }
    Ok(())
}
