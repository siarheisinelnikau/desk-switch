use anyhow::{Context, Result, ensure};
use std::path::Path;

pub fn sync(enabled: bool, executable: &Path, state: &Path) -> Result<()> {
    ensure!(
        executable.is_absolute() && state.is_absolute(),
        "autostart paths must be absolute"
    );
    native::sync(enabled, executable, state).context("настройка автозапуска")
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    fn xml(value: &Path) -> Result<String> {
        let value = value.to_str().context("The autostart path is not UTF-8")?;
        ensure!(
            !value.chars().any(char::is_control),
            "Invalid autostart path"
        );
        Ok(value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;"))
    }
    pub fn sync(enabled: bool, executable: &Path, state: &Path) -> Result<()> {
        let path = dirs::home_dir()
            .context("The home directory is unavailable")?
            .join("Library/LaunchAgents/com.deskswitch.Desktop.plist");
        if !enabled {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        } else {
            let document = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>com.deskswitch.Desktop</string><key>ProgramArguments</key><array><string>{}</string><string>gui</string><string>--tray</string><string>--state-dir</string><string>{}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><false/></dict></plist>\n",
                xml(executable)?,
                xml(state)?
            );
            if std::fs::read_to_string(&path).ok().as_deref() != Some(&document) {
                crate::config::write_atomic(&path, &document)?;
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod native {
    use super::*;
    use std::{fs, path::PathBuf};

    fn quote(value: &str) -> Result<String> {
        ensure!(
            !value.contains(['\n', '\r', '\0']),
            "invalid autostart path"
        );
        let mut result = String::from("\"");
        for character in value.chars() {
            // Desktop Entry string unescaping happens before Exec argument parsing.
            match character {
                '\\' => result.push_str("\\\\\\\\"),
                '"' | '`' | '$' => {
                    result.push_str("\\\\");
                    result.push(character);
                }
                '%' => result.push_str("%%"),
                _ => result.push(character),
            }
        }
        result.push('"');
        Ok(result)
    }

    fn document(executable: &Path, state: &Path) -> Result<String> {
        Ok(format!(
            "[Desktop Entry]\nType=Application\nName=Desk Switch\nComment=Share input and audio with a paired PC\nExec={} gui --tray --state-dir {}\nTryExec={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
            quote(
                executable
                    .to_str()
                    .context("autostart executable path is not UTF-8")?
            )?,
            quote(
                state
                    .to_str()
                    .context("autostart settings path is not UTF-8")?
            )?,
            executable
                .to_str()
                .context("invalid executable path")?
                .replace('\\', "\\\\")
        ))
    }

    fn sync_in(config_home: &Path, enabled: bool, executable: &Path, state: &Path) -> Result<()> {
        let path = config_home.join("autostart/com.deskswitch.Desktop.desktop");
        if !enabled {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        } else {
            let document = document(executable, state)?;
            if fs::read_to_string(&path).ok().as_deref() != Some(&document) {
                crate::config::write_atomic(&path, &document)?;
            }
        }
        Ok(())
    }

    pub fn sync(enabled: bool, executable: &Path, state: &Path) -> Result<()> {
        let launcher = std::env::var_os("DESK_SWITCH_PACKAGED_LAUNCHER").map(PathBuf::from);
        if let Some(path) = &launcher {
            ensure!(
                path.is_absolute() && path.is_file(),
                "packaged launcher is unavailable"
            );
        }
        let executable = launcher.as_deref().unwrap_or(executable);
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .context("cannot locate the user's autostart directory")?;
        sync_in(&config_home, enabled, executable, state)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn isolated_entry_retains_profile_and_can_be_disabled() {
            let root =
                std::env::temp_dir().join(format!("desk-switch-autostart-{}", std::process::id()));
            fs::create_dir_all(&root).unwrap();
            let executable = root.join("Desk Switch 100%.bin");
            let state = root.join("profile with spaces");
            sync_in(&root, true, &executable, &state).unwrap();
            let entry =
                fs::read_to_string(root.join("autostart/com.deskswitch.Desktop.desktop")).unwrap();
            assert!(entry.contains("100%%.bin\" gui --tray --state-dir \""));
            assert!(entry.contains("profile with spaces\""));
            assert!(entry.contains("TryExec="));
            sync_in(&root, false, &executable, &state).unwrap();
            assert!(
                !root
                    .join("autostart/com.deskswitch.Desktop.desktop")
                    .exists()
            );
            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{io, ptr};
    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::*,
    };

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    pub fn sync(enabled: bool, executable: &Path, state: &Path) -> Result<()> {
        let mut handle = ptr::null_mut();
        let code = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(r"Software\Microsoft\Windows\CurrentVersion\Run").as_ptr(),
                0,
                ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                ptr::null(),
                &mut handle,
                ptr::null_mut(),
            )
        };
        ensure!(
            code == ERROR_SUCCESS,
            "{}",
            io::Error::from_raw_os_error(code as i32)
        );
        let key = Key(handle);
        let name = wide("Desk Switch");
        if !enabled {
            let code = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
            ensure!(
                code == ERROR_SUCCESS || code == ERROR_FILE_NOT_FOUND,
                "{}",
                io::Error::from_raw_os_error(code as i32)
            );
            return Ok(());
        }
        let executable = executable.to_str().context("invalid executable path")?;
        let state = state.to_str().context("invalid profile path")?;
        ensure!(
            !executable.contains(['"', '\r', '\n', '\0'])
                && !state.contains(['"', '\r', '\n', '\0']),
            "invalid autostart path"
        );
        let trailing = state.chars().rev().take_while(|ch| *ch == '\\').count();
        let command = format!(
            "\"{executable}\" gui --tray --state-dir \"{state}{}\"",
            "\\".repeat(trailing)
        );
        ensure!(
            command.encode_utf16().count() < 260,
            "путь для автозапуска слишком длинный; переместите приложение в короткий путь"
        );
        let value = wide(&command);
        let mut old = vec![0u16; 1024];
        let mut bytes = (old.len() * 2) as u32;
        let code = unsafe {
            RegGetValueW(
                key.0,
                ptr::null(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                ptr::null_mut(),
                old.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if code == ERROR_SUCCESS && old.get(..bytes as usize / 2) == Some(value.as_slice()) {
            return Ok(());
        }
        let code = unsafe {
            RegSetValueExW(
                key.0,
                name.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        ensure!(
            code == ERROR_SUCCESS,
            "{}",
            io::Error::from_raw_os_error(code as i32)
        );
        Ok(())
    }
}
