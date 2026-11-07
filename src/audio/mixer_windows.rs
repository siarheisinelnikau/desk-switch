use super::{Backend, Session, State};
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;
use windows::{
    Win32::{
        Media::Audio::*,
        System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree},
    },
    core::Interface,
};

pub(super) struct Native {
    enumerator: IMMDeviceEnumerator,
    volumes: BTreeMap<String, ISimpleAudioVolume>,
    _com: super::super::output::Com,
}
impl Native {
    pub fn open() -> Result<Self> {
        let com = super::super::output::Com::open()?;
        let enumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }?;
        Ok(Self {
            _com: com,
            enumerator,
            volumes: BTreeMap::new(),
        })
    }
}
unsafe fn take_string(value: windows::core::PWSTR) -> Result<String> {
    let text = unsafe { value.to_string() };
    unsafe { CoTaskMemFree(Some(value.0.cast())) };
    Ok(text?)
}
impl Backend for Native {
    fn sessions(&mut self) -> Result<Vec<Session>> {
        self.volumes.clear();
        let mut sessions = Vec::new();
        unsafe {
            let devices = self
                .enumerator
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            for index in 0..devices.GetCount()? {
                let opened = (|| -> Result<_> {
                    let device = devices.Item(index)?;
                    let endpoint = take_string(device.GetId()?)?;
                    let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
                    Ok((endpoint, manager.GetSessionEnumerator()?))
                })();
                let Ok((endpoint, list)) = opened else {
                    continue;
                };
                let count = list.GetCount()?;
                ensure!(
                    (0..=1024).contains(&count),
                    "too many Windows audio sessions"
                );
                for index in 0..count {
                    let control = match list.GetSession(index) {
                        Ok(control) => control,
                        Err(_) => continue,
                    };
                    let read = (|| -> Result<Option<_>> {
                        if control.GetState()? == AudioSessionStateExpired {
                            return Ok(None);
                        }
                        let control2: IAudioSessionControl2 = control.cast()?;
                        let ours = control2.GetProcessId()? == std::process::id()
                            || control.GetGroupingParam()? == super::super::SESSION_GROUP;
                        let id = format!(
                            "{endpoint}:{}",
                            take_string(control2.GetSessionInstanceIdentifier()?)?
                        );
                        let group = format!(
                            "{endpoint}:{}",
                            take_string(control2.GetSessionIdentifier()?)?
                        );
                        let volume: ISimpleAudioVolume = control.cast()?;
                        let state = State {
                            channels: vec![f64::from(volume.GetMasterVolume()?)],
                            muted: volume.GetMute()?.as_bool(),
                        };
                        ensure!(
                            state.valid() && id.len() <= 4096 && group.len() <= 4096,
                            "invalid Windows session volume"
                        );
                        Ok(Some((
                            Session {
                                id,
                                group,
                                state,
                                ours,
                            },
                            volume,
                        )))
                    })();
                    // Closing an app or unplugging an output can invalidate a
                    // session between enumeration and its property queries.
                    if let Ok(Some((session, volume))) = read {
                        self.volumes.insert(session.id.clone(), volume);
                        sessions.push(session);
                    }
                }
            }
        }
        Ok(sessions)
    }
    fn apply(&mut self, id: &str, state: &State) -> Result<()> {
        ensure!(
            state.channels.len() == 1 && state.channels[0] <= 1.0,
            "invalid Windows session volume"
        );
        let volume = self
            .volumes
            .get(id)
            .context("Windows audio session disappeared")?;
        let result = unsafe {
            volume
                .SetMasterVolume(state.channels[0] as f32, std::ptr::null())
                .and_then(|()| volume.SetMute(state.muted, std::ptr::null()))
        };
        match result {
            Ok(()) => Ok(()),
            Err(e) if e.code() == AUDCLNT_E_DEVICE_INVALIDATED => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
