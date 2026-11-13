use std::{fs, path::PathBuf};

use anyhow::bail;

use crate::monitors::{percent_to_raw, raw_to_percent, BacklightDevice};

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
            Ok(max_brightness) => max_brightness.trim().parse::<u16>()?,
            Err(err) => {
                bail!("{}: {err}", max_brightness_path.display());
            }
        };

        let current_brightness_raw = match fs::read_to_string(&current_brightness_path) {
            Ok(current_brightness) => current_brightness.trim().parse::<u16>()?,
            Err(err) => {
                bail!("{}: {err}", current_brightness_path.display());
            }
        };

        Ok(Self {
            path,
            max_brightness_raw,
            current_brightness_raw,
            current_brightness_percent: raw_to_percent(current_brightness_raw, max_brightness_raw)?,
        })
    }
}

impl BacklightDevice for BacklightAcpiDevice {
    fn identity(&self) -> String {
        format!("acpi:{}", self.path.display())
    }

    fn set_brightness(&mut self, percent: u8) -> anyhow::Result<()> {
        let current_brightness_path = self.path.join("brightness");
        let new_brightness = percent_to_raw(percent, self.max_brightness_raw)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sysfs_parsing_and_brightness_writes() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("backlightd-test-{}-{unique}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let fixture = Fixture(path);
        let path = &fixture.0;
        fs::write(path.join("max_brightness"), "65535\n").unwrap();
        fs::write(path.join("brightness"), "32768\n").unwrap();
        let mut device = BacklightAcpiDevice::new(path.clone()).unwrap();
        assert_eq!(device.get_brightness(), 50);
        device.set_brightness(100).unwrap();
        assert_eq!(
            fs::read_to_string(path.join("brightness")).unwrap(),
            "65535"
        );
        assert!(device.set_brightness(101).is_err());
        assert_eq!(device.get_brightness(), 100);
        assert_eq!(
            fs::read_to_string(path.join("brightness")).unwrap(),
            "65535"
        );

        fs::write(path.join("max_brightness"), "3\n").unwrap();
        fs::write(path.join("brightness"), "1\n").unwrap();
        let mut device = BacklightAcpiDevice::new(path.clone()).unwrap();
        device.set_brightness(1).unwrap();
        assert_eq!(fs::read_to_string(path.join("brightness")).unwrap(), "1");
        fs::write(path.join("brightness"), "4\n").unwrap();
        assert!(BacklightAcpiDevice::new(path.clone()).is_err());
        fs::write(path.join("max_brightness"), "0\n").unwrap();
        fs::write(path.join("brightness"), "0\n").unwrap();
        assert!(BacklightAcpiDevice::new(path.clone()).is_err());
    }
}
