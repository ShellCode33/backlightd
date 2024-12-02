use std::{fs, path::PathBuf};

use anyhow::bail;

use crate::monitors::BacklightDevice;

pub(crate) const ACPI_DEVICES_PATH: &str = "/sys/class/backlight";

pub(crate) struct BacklightAcpiDevice {
    path: PathBuf,
    max_brightness_raw: u16,
    current_brightness_raw: u16,
    current_brightness_percent: u8,
}

impl BacklightAcpiDevice {
    pub(crate) fn new(path: PathBuf) -> anyhow::Result<Self> {
        let max_brightness_path = path.join("max_brightness");
        let current_brightness_path = path.join("brightness");

        let max_brightness_raw = match fs::read_to_string(&max_brightness_path) {
            Ok(max_brightness) => max_brightness.parse::<u16>()?,
            Err(err) => {
                bail!("{}: {err}", max_brightness_path.display());
            }
        };

        let current_brightness_raw = match fs::read_to_string(&current_brightness_path) {
            Ok(current_brightness) => current_brightness.parse::<u16>()?,
            Err(err) => {
                bail!("{}: {err}", current_brightness_path.display());
            }
        };

        Ok(Self {
            path,
            max_brightness_raw,
            current_brightness_raw,
            current_brightness_percent: (current_brightness_raw * 100 / max_brightness_raw) as u8,
        })
    }
}

impl BacklightDevice for BacklightAcpiDevice {
    fn set_brightness(&mut self, percent: u8) -> anyhow::Result<()> {
        assert!(percent <= 100);

        let current_brightness_path = self.path.join("brightness");
        let new_brightness = (percent as f64 / 100. * self.max_brightness_raw as f64) as u16;

        if let Err(err) = fs::write(&current_brightness_path, new_brightness.to_string()) {
            bail!("{}: {err}", current_brightness_path.display());
        }

        self.current_brightness_raw = new_brightness;
        self.current_brightness_percent = percent;
        Ok(())
    }

    fn get_brightness(&self) -> u8 {
        self.current_brightness_percent
    }

    fn name(&self) -> String {
        // It's ok to unwrap here, if there is no filename it means the developer did something wrong.
        self.path.file_name().unwrap().to_string_lossy().to_string()
    }

    fn turn_off(&mut self) -> anyhow::Result<()> {
        todo!()
    }

    fn turn_on(&mut self) -> anyhow::Result<()> {
        todo!()
    }
}
