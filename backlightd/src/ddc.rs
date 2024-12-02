/// The bible for DDC:
/// https://milek7.pl/ddcbacklight/mccs.pdf
use std::error::Error;

use anyhow::bail;
use ddc_hi::{Ddc, Display, FeatureCode};

use crate::monitors::BacklightDevice;

const VCP_FEATURE_BRIGHTNESS: FeatureCode = 0x10;
const VCP_FEATURE_POWER: FeatureCode = 0xD6;
const VCP_VALUE_POWER_ON: u16 = 0x1;
const VCP_VALUE_POWER_OFF: u16 = 0x4;

pub(crate) struct BacklightDdcDevice {
    display: Display,
    max_brightness_raw: u16,
    current_brightness_raw: u16,
    current_brightness_percent: u8,
}

impl BacklightDdcDevice {
    pub(crate) fn new(mut ddc_device: ddc_hi::Display) -> Result<Self, Box<dyn Error>> {
        let brightness = ddc_device.handle.get_vcp_feature(VCP_FEATURE_BRIGHTNESS)?;

        Ok(Self {
            display: ddc_device,
            max_brightness_raw: brightness.maximum(),
            current_brightness_raw: brightness.value(),
            current_brightness_percent: (brightness.value() * 100 / brightness.maximum()) as u8,
        })
    }
}

impl BacklightDevice for BacklightDdcDevice {
    fn set_brightness(&mut self, percent: u8) -> anyhow::Result<()> {
        assert!(percent <= 100);

        let new_brightness = (percent as f64 / 100. * self.max_brightness_raw as f64) as u16;

        if let Err(err) = self
            .display
            .handle
            .set_vcp_feature(VCP_FEATURE_BRIGHTNESS, new_brightness)
        {
            bail!("{}: {err}", self.name());
        }

        self.current_brightness_raw = new_brightness;
        self.current_brightness_percent = percent;
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
