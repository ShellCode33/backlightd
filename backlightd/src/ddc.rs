//! External-monitor adapter using `ddc-hi` and DDC/CI VCP features.
//!
//! Brightness is VCP 0x10, power is 0xD6. Writes narrow to the 16-bit wire type
//! after validation; successful writes invalidate cached readback evidence.
use std::{error::Error, fmt::Display as ErrorDisplay, thread, time::Duration};

use anyhow::{bail, Context};
use ddc_hi::{Ddc, Display, FeatureCode};

use crate::monitors::{percent_to_raw, raw_to_percent, BacklightDevice, RawBrightness};

const VCP_FEATURE_BRIGHTNESS: FeatureCode = 0x10;
const VCP_FEATURE_POWER: FeatureCode = 0xD6;
const VCP_VALUE_POWER_ON: u16 = 0x1;
const VCP_VALUE_POWER_OFF: u16 = 0x4;

const INITIALIZATION_ATTEMPTS: usize = 3;
const INITIALIZATION_RETRY_DELAY: Duration = Duration::from_millis(200);

fn probe_with_retries<T, E: ErrorDisplay>(
    mut probe: impl FnMut() -> Result<T, E>,
    mut wait: impl FnMut(),
) -> Result<T, E> {
    for attempt in 1..INITIALIZATION_ATTEMPTS {
        match probe() {
            Ok(value) => return Ok(value),
            Err(err) => log::debug!("Brightness probe attempt={attempt} failed: {err}; retrying"),
        }
        wait();
    }
    probe()
}

pub(crate) struct BacklightDdcDevice {
    display: Display,
    max_brightness_raw: u16,
    current_brightness_raw: u16,
    current_brightness_percent: u8,
    readback_raw: Option<u16>,
    actual_raw: Option<u16>,
}

impl BacklightDdcDevice {
    pub(crate) fn new(mut ddc_device: ddc_hi::Display) -> Result<Self, Box<dyn Error>> {
        let brightness = probe_with_retries(
            || ddc_device.handle.get_vcp_feature(VCP_FEATURE_BRIGHTNESS),
            || thread::sleep(INITIALIZATION_RETRY_DELAY),
        )?;

        Ok(Self {
            readback_raw: Some(brightness.value()),
            actual_raw: None,
            display: ddc_device,
            max_brightness_raw: brightness.maximum(),
            current_brightness_raw: brightness.value(),
            current_brightness_percent: raw_to_percent(
                brightness.value().into(),
                brightness.maximum().into(),
            )?,
        })
    }
}

impl BacklightDevice for BacklightDdcDevice {
    fn identity(&self) -> String {
        // Include EDID so replacing a monitor on the same bus counts as a new device.
        format!(
            "ddc:{}:{}:{:?}",
            self.display.info.backend, self.display.info.id, self.display.info.edid_data
        )
    }

    fn config_id(&self) -> String {
        self.display
            .info
            .edid_data
            .as_deref()
            .filter(|edid| !edid.is_empty())
            .map(crate::calibration::edid_id)
            .unwrap_or_else(|| self.identity())
    }
    fn raw_brightness(&self) -> RawBrightness {
        RawBrightness {
            value: self.current_brightness_raw.into(),
            maximum: self.max_brightness_raw.into(),
            readback: self.readback_raw.map(u32::from),
            actual: self.actual_raw.map(u32::from),
        }
    }
    fn set_brightness(&mut self, percent: u8) -> anyhow::Result<()> {
        self.set_raw_brightness(percent_to_raw(percent, self.max_brightness_raw.into())?)
    }
    fn set_raw_brightness(&mut self, new_brightness: u32) -> anyhow::Result<()> {
        let new_brightness = u16::try_from(new_brightness)
            .context("DDC brightness exceeds the 16-bit protocol limit")?;
        anyhow::ensure!(
            new_brightness <= self.max_brightness_raw,
            "Brightness exceeds hardware maximum"
        );

        if let Err(err) = self
            .display
            .handle
            .set_vcp_feature(VCP_FEATURE_BRIGHTNESS, new_brightness)
        {
            bail!("{}: {err}", self.name());
        }

        self.current_brightness_raw = new_brightness;
        self.current_brightness_percent =
            raw_to_percent(new_brightness.into(), self.max_brightness_raw.into())?;
        self.readback_raw = None;
        self.actual_raw = None;
        Ok(())
    }

    fn get_brightness(&self) -> u8 {
        self.current_brightness_percent
    }

    fn name(&self) -> String {
        self.display
            .info
            .model_name
            .clone()
            .unwrap_or(String::from("Unknown"))
    }

    fn turn_off(&mut self) -> anyhow::Result<()> {
        if let Err(err) = self
            .display
            .handle
            .set_vcp_feature(VCP_FEATURE_POWER, VCP_VALUE_POWER_OFF)
        {
            bail!("{}: {err}", self.name());
        }

        Ok(())
    }

    fn turn_on(&mut self) -> anyhow::Result<()> {
        if let Err(err) = self
            .display
            .handle
            .set_vcp_feature(VCP_FEATURE_POWER, VCP_VALUE_POWER_ON)
        {
            bail!("{}: {err}", self.name());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_probe_error_recovers_without_repeating_successful_probe() {
        let mut attempts = 0;
        let mut waits = 0;
        let value = probe_with_retries(
            || {
                attempts += 1;
                if attempts == 1 {
                    Err("monitor not ready")
                } else {
                    Ok(95)
                }
            },
            || waits += 1,
        )
        .unwrap();
        assert_eq!((value, attempts, waits), (95, 2, 1));
        assert_eq!(
            probe_with_retries(
                || Ok::<_, &str>(100),
                || panic!("successful probe must not wait")
            )
            .unwrap(),
            100
        );
    }

    #[test]
    fn permanent_probe_failure_has_a_bounded_retry_count() {
        let mut attempts = 0;
        let mut waits = 0;
        let result: Result<(), _> = probe_with_retries(
            || {
                attempts += 1;
                Err("monitor unavailable")
            },
            || waits += 1,
        );
        assert_eq!(result.unwrap_err(), "monitor unavailable");
        assert_eq!((attempts, waits), (3, 2));
    }
}
