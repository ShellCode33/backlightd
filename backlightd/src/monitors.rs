use std::{fs, sync::Mutex, time::Instant};

use crate::acpi::{BacklightAcpiDevice, ACPI_DEVICES_PATH};
use crate::ddc::BacklightDdcDevice;

/// Holds the current backlight state of all monitors.
///
/// Querying/writing to backlight devices is expensive, we try to cache as many things as possible
/// in order to avoid unnecessary I/O.
static MONITORS: Mutex<Vec<Box<dyn BacklightDevice + Send>>> = Mutex::new(Vec::new());

pub(crate) trait BacklightDevice {
    fn name(&self) -> String;
    fn set_brightness(&mut self, percent: u8) -> anyhow::Result<()>;
    fn get_brightness(&self) -> u8;
    fn turn_off(&mut self) -> anyhow::Result<()>;
    fn turn_on(&mut self) -> anyhow::Result<()>;
}

pub(crate) fn refresh_monitors_list(reason: &str) -> anyhow::Result<()> {
    let mut failures = 0;
    let started = Instant::now();
    log::debug!("Discovery started: reason={reason}");
    // Called only by the controller, including recovery after device errors.
    let mut new_monitors: Vec<Box<dyn BacklightDevice + Send>> = Vec::new();

    let enumeration_started = Instant::now();
    let displays = ddc_hi::Display::enumerate();
    log::debug!(
        "Discovery stage=ddc_enumeration candidates={} elapsed_ms={}",
        displays.len(),
        enumeration_started.elapsed().as_millis()
    );
    for ddc_device in displays {
        let id = ddc_device.info.id.clone();
        let device_started = Instant::now();
        let result = BacklightDdcDevice::new(ddc_device);
        log::debug!(
            "Discovery stage=ddc_initialization id={id} success={} elapsed_ms={}",
            result.is_ok(),
            device_started.elapsed().as_millis()
        );
        match result {
            Ok(monitor) => new_monitors.push(Box::new(monitor)),
            Err(err) => {
                failures += 1;
                log::error!("Failed to retrieve DDC backlight monitor id={id}: {err}");
            }
        }
    }

    let acpi_started = Instant::now();
    match fs::read_dir(ACPI_DEVICES_PATH) {
        Ok(dir) => {
            for entry in dir {
                match entry {
                    Ok(file) => {
                        let path = file.path();
                        let device_started = Instant::now();
                        let result = BacklightAcpiDevice::new(path.clone());
                        log::debug!(
                            "Discovery stage=acpi_initialization path={} success={} elapsed_ms={}",
                            path.display(),
                            result.is_ok(),
                            device_started.elapsed().as_millis()
                        );
                        match result {
                            Ok(monitor) => new_monitors.push(Box::new(monitor)),
                            Err(err) => {
                                failures += 1;
                                log::error!("Failed to retrieve ACPI backlight monitor: {err}")
                            }
                        }
                    }
                    Err(err) => {
                        failures += 1;
                        log::error!("Unable to read entry from {ACPI_DEVICES_PATH}: {err}");
                    }
                }
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            failures += 1;
            log::error!("{ACPI_DEVICES_PATH}: {err}");
        }
    }

    log::debug!(
        "Discovery stage=acpi_scan elapsed_ms={}",
        acpi_started.elapsed().as_millis()
    );
    let mut monitors = MONITORS.lock().expect("Unable to acquire MONITORS mutex");
    monitors.clear();
    monitors.extend(new_monitors);
    log::debug!(
        "Discovery finished: reason={reason} monitors={} failed={failures} elapsed_ms={}",
        monitors.len(),
        started.elapsed().as_millis()
    );
    anyhow::ensure!(
        failures == 0,
        "Discovery incomplete: {failures} probe(s) failed"
    );
    Ok(())
}

fn refresh_after_error(reason: &str) {
    if let Err(err) = refresh_monitors_list(reason) {
        log::warn!("Recovery discovery incomplete: {err}");
    }
}

// Use widened integer arithmetic for device scales up to u16::MAX.
pub(crate) fn raw_to_percent(raw: u16, maximum: u16) -> anyhow::Result<u8> {
    anyhow::ensure!(maximum > 0, "Maximum brightness must be positive");
    anyhow::ensure!(raw <= maximum, "Brightness exceeds device maximum");
    Ok(((u32::from(raw) * 100 + u32::from(maximum) / 2) / u32::from(maximum)) as u8)
}

pub(crate) fn percent_to_raw(percent: u8, maximum: u16) -> anyhow::Result<u16> {
    anyhow::ensure!(percent <= 100, "Brightness percentage must not exceed 100");
    anyhow::ensure!(maximum > 0, "Maximum brightness must be positive");
    let raw = ((u32::from(percent) * u32::from(maximum) + 50) / 100) as u16;
    Ok(if percent > 0 { raw.max(1) } else { 0 })
}

pub(crate) fn set_brightness_percent(percent: u8) -> anyhow::Result<()> {
    let mut last_error = None;

    for monitor in MONITORS.lock().unwrap().iter_mut() {
        let write_started = Instant::now();
        let res = monitor.set_brightness(percent);
        log::debug!(
            "Brightness write: monitor={} target_percent={percent} success={} elapsed_ms={}",
            monitor.name(),
            res.is_ok(),
            write_started.elapsed().as_millis()
        );

        if let Err(err) = res {
            log::error!("Unable to set brightness of {}", monitor.name());
            last_error = Some(err);
        }
    }

    if let Some(err) = last_error {
        log::info!("Trying to refresh monitors list to fix the error...");
        refresh_after_error("set_brightness_error");
        Err(err)
    } else {
        log::info!("Brightness of all monitors has been set to {percent}%");
        Ok(())
    }
}

pub(crate) fn increase_brightness_percent(percent: u8) -> anyhow::Result<()> {
    let mut last_error = None;

    for monitor in MONITORS.lock().unwrap().iter_mut() {
        let new_brightness = monitor.get_brightness().saturating_add(percent).min(100);

        let write_started = Instant::now();
        let res = monitor.set_brightness(new_brightness);
        log::debug!(
            "Brightness write: monitor={} target_percent={new_brightness} success={} elapsed_ms={}",
            monitor.name(),
            res.is_ok(),
            write_started.elapsed().as_millis()
        );

        if let Err(err) = res {
            log::error!("Unable to set brightness of {}", monitor.name());
            last_error = Some(err);
        }
    }

    if let Some(err) = last_error {
        log::info!("Trying to refresh monitors list to fix the error...");
        refresh_after_error("increase_brightness_error");
        Err(err)
    } else {
        log::info!("Brightness of all monitors increased by {percent} percentage points");
        Ok(())
    }
}

pub(crate) fn decrease_brightness_percent(percent: u8) -> anyhow::Result<()> {
    let mut last_error = None;

    for monitor in MONITORS.lock().unwrap().iter_mut() {
        // Don't allow a relative decrease to black out the display.
        let new_brightness = monitor.get_brightness().saturating_sub(percent).max(1);
        let write_started = Instant::now();
        let res = monitor.set_brightness(new_brightness);
        log::debug!(
            "Brightness write: monitor={} target_percent={new_brightness} success={} elapsed_ms={}",
            monitor.name(),
            res.is_ok(),
            write_started.elapsed().as_millis()
        );

        if let Err(err) = res {
            log::error!("Unable to set brightness of {}: {err}", monitor.name());
            last_error = Some(err);
        }
    }

    if let Some(err) = last_error {
        log::info!("Trying to refresh monitors list to fix the error...");
        refresh_after_error("decrease_brightness_error");
        Err(err)
    } else {
        log::info!("Brightness of all monitors decreased by {percent} percentage points");
        Ok(())
    }
}

pub(crate) fn turn_off() -> anyhow::Result<()> {
    let mut last_error = None;

    for monitor in MONITORS.lock().unwrap().iter_mut() {
        if let Err(err) = monitor.turn_off() {
            log::error!("Unable to turn OFF monitor: {err}");
            last_error = Some(err);
        }
    }

    if last_error.is_some() {
        log::info!("Trying to refresh monitors list to fix the error and retry...");
        refresh_after_error("turn_off_error");

        last_error = None;
        for monitor in MONITORS.lock().unwrap().iter_mut() {
            if let Err(err) = monitor.turn_off() {
                log::error!("Unable to turn OFF monitor: {err}");
                last_error = Some(err);
            }
        }

        if let Some(err) = last_error {
            Err(err)
        } else {
            Ok(())
        }
    } else {
        Ok(())
    }
}

pub(crate) fn turn_on() -> anyhow::Result<()> {
    let mut last_error = None;

    for monitor in MONITORS.lock().unwrap().iter_mut() {
        if let Err(err) = monitor.turn_on() {
            log::error!("Unable to turn ON monitor: {err}");
            last_error = Some(err);
        }
    }

    if last_error.is_some() {
        log::info!("Trying to refresh monitors list to fix the error and retry...");
        refresh_after_error("turn_on_error");

        last_error = None;
        for monitor in MONITORS.lock().unwrap().iter_mut() {
            if let Err(err) = monitor.turn_on() {
                log::error!("Unable to turn ON monitor: {err}");
                last_error = Some(err);
            }
        }

        if let Some(err) = last_error {
            Err(err)
        } else {
            Ok(())
        }
    } else {
        Ok(())
    }
}

pub(crate) fn get_average_brightness() -> u8 {
    let monitors = MONITORS.lock().unwrap();

    if monitors.is_empty() {
        return 0;
    }

    let mut sum: usize = 0;

    for monitor in &*monitors {
        sum += monitor.get_brightness() as usize;
    }

    (sum / monitors.len()) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_scale_boundaries_and_invalid_values() {
        assert_eq!(raw_to_percent(u16::MAX, u16::MAX).unwrap(), 100);
        assert_eq!(raw_to_percent(0, u16::MAX).unwrap(), 0);
        assert_eq!(raw_to_percent(1, 3).unwrap(), 33);
        assert_eq!(raw_to_percent(2, 3).unwrap(), 67);
        assert!(raw_to_percent(0, 0).is_err());
        assert!(raw_to_percent(101, 100).is_err());
        assert!(percent_to_raw(101, 100).is_err());
        assert!(percent_to_raw(1, 0).is_err());
        assert_eq!(percent_to_raw(100, u16::MAX).unwrap(), u16::MAX);
        assert_eq!(percent_to_raw(50, 3).unwrap(), 2);
        assert_eq!(percent_to_raw(0, 3).unwrap(), 0);
        for maximum in [1, 3, 100, u16::MAX] {
            for percent in 1..=100 {
                let raw = percent_to_raw(percent, maximum).unwrap();
                assert!((1..=maximum).contains(&raw));
            }
        }
    }
}
