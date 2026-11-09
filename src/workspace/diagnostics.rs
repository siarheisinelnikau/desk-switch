use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const LINES: usize = 40;
pub const LINE_BYTES: usize = 320;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Diagnostics {
    pub platform: String,
    pub logs: Vec<String>,
}

impl Diagnostics {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            ["", "linux", "windows", "macos"].contains(&self.platform.as_str()),
            "invalid diagnostics platform"
        );
        ensure!(
            self.logs.len() <= LINES
                && self
                    .logs
                    .iter()
                    .all(|s| s.len() <= LINE_BYTES
                        && !s.chars().any(|c| c.is_control() && c != '\t')),
            "diagnostics exceeds its limit"
        );
        Ok(())
    }

    pub fn line(&mut self, service: &str, line: &str) {
        let mut line = format!("{service}: {line}");
        line.retain(|c| !c.is_control() || c == '\t');
        line.truncate(line.floor_char_boundary(LINE_BYTES));
        if self.logs.len() == LINES {
            self.logs.remove(0);
        }
        self.logs.push(line.clone());
        // The optional file sink keeps the last messages when the application
        // stops before the report can be copied.
        crate::diagnostics_log::append(service, &line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_utf8_log_tails_and_rejects_oversized_remote_reports() {
        let mut report = Diagnostics {
            platform: "linux".into(),
            ..Default::default()
        };
        for _ in 0..100 {
            report.line("Input", &"ж\n".repeat(400));
        }
        report.validate().unwrap();
        assert_eq!(report.logs.len(), LINES);
        assert!(serde_json::to_vec(&report).unwrap().len() < 16 * 1024);
        report.logs.push("extra".into());
        assert!(report.validate().is_err());
    }
}
