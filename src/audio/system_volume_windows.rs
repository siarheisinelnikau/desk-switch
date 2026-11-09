use super::Backend;
use crate::workspace::AudioLevel;
use anyhow::{Result, ensure};
use windows::Win32::{Media::Audio::Endpoints::IAudioEndpointVolume, System::Com::CLSCTX_ALL};

pub(super) struct Native {
    volume: IAudioEndpointVolume,
    _com: super::super::output::Com,
}

impl Native {
    pub fn open(output: Option<&str>) -> Result<Self> {
        let com = super::super::output::Com::open()?;
        let device = super::super::output::output_device(output)?;
        // SAFETY: the activated interface is owned by this backend, which
        // stores it and releases it when it is dropped.
        let volume = unsafe { device.as_immdevice().Activate(CLSCTX_ALL, None)? };
        Ok(Self { volume, _com: com })
    }
}

impl Backend for Native {
    fn read(&mut self) -> Result<AudioLevel> {
        // SAFETY: the endpoint volume interface is owned by this backend and
        // is only used while it is alive.
        let volume = unsafe { self.volume.GetMasterVolumeLevelScalar()? };
        ensure!(
            volume.is_finite() && (0.0..=1.0).contains(&volume),
            "invalid Windows endpoint volume"
        );
        Ok(AudioLevel {
            volume: (volume * 100.0).round() as u8,
            // SAFETY: the endpoint volume interface is owned by this backend.
            muted: unsafe { self.volume.GetMute()?.as_bool() },
        })
    }

    fn set_volume(&mut self, volume: u8) -> Result<()> {
        // SAFETY: the endpoint volume interface is owned by this backend and
        // the null event pointer is allowed by the API.
        // SAFETY: the endpoint volume interface is owned by this backend and the
        // null event pointer is allowed by the API.
        unsafe {
            self.volume
                .SetMasterVolumeLevelScalar(f32::from(volume) / 100.0, std::ptr::null())?;
        }
        Ok(())
    }

    fn set_muted(&mut self, muted: bool) -> Result<()> {
        // SAFETY: the endpoint volume interface is owned by this backend and
        // the null event pointer is allowed by the API.
        // SAFETY: the endpoint volume interface is owned by this backend and the
        // null event pointer is allowed by the API.
        unsafe {
            self.volume.SetMute(muted, std::ptr::null())?;
        }
        Ok(())
    }
}
