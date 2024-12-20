use std::{
    fs::{self},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use crate::acpi::{BacklightAcpiDevice, ACPI_DEVICES_PATH};
use crate::ddc::BacklightDdcDevice;

/// Holds the current backlight state of all monitors.
///
/// Querying/writing to backlight devices is expensive, we try to cache as many things as possible
/// in order to avoid unnecessary I/O.
static MONITORS: Mutex<Vec<Box<dyn BacklightDevice + Send>>> = Mutex::new(Vec::new());

/// The last time the list of known monitors was refreshed.
static LAST_REFRESH: Mutex<Option<Instant>> = Mutex::new(None);

/// The frequency at which the list of known monitors must be refreshed.
const MONITORS_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

pub(crate) trait BacklightDevice {
    fn name(&self) -> String;
    fn set_brightness(&mut self, percent: u8) -> anyhow::Result<()>;
    fn get_brightness(&self) -> u8;
    fn turn_off(&mut self) -> anyhow::Result<()>;
    fn turn_on(&mut self) -> anyhow::Result<()>;
}

pub(crate) fn auto_refresh_monitors_list() -> ! {
    loop {
        let must_refresh = {
            let last_refresh = LAST_REFRESH
                .lock()
                .expect("Unable to acquire LAST_REFRESH mutex");

            last_refresh.is_none()
                || last_refresh.is_some_and(|dt| Instant::now() - dt > MONITORS_REFRESH_INTERVAL)
        };

        if must_refresh {
            refresh_monitors_list();
        }

        thread::sleep(Duration::from_secs(10));
    }
}

pub(crate) fn refresh_monitors_list() {
    // Don't modify the global MONITORS variable directly to avoid locking the mutex for too long.
    let mut new_monitors: Vec<Box<dyn BacklightDevice + Send>> = Vec::new();

    for ddc_device in ddc_hi::Display::enumerate() {
        match BacklightDdcDevice::new(ddc_device) {
            Ok(monitor) => new_monitors.push(Box::new(monitor)),
            Err(err) => log::error!("Failed to retrieve DDC backlight monitor: {err}"),
        }
    }

    match fs::read_dir(ACPI_DEVICES_PATH) {
        Ok(dir) => {
            for entry in dir {
                match entry {
                    Ok(file) => match BacklightAcpiDevice::new(file.path()) {
                        Ok(monitor) => new_monitors.push(Box::new(monitor)),
                        Err(err) => log::error!("Failed to retrieve ACPI backlight monitor: {err}"),
                    },
                    Err(err) => {
                        log::error!("Unable to read entry from {ACPI_DEVICES_PATH}: {err}");
                    }
                }
            }
        }
        Err(err) => {
            log::error!("{ACPI_DEVICES_PATH}: {err}");
            // fallthrough
        }
    }

    let mut monitors = MONITORS.lock().expect("Unable to acquire MONITORS mutex");
    monitors.clear();
    monitors.extend(new_monitors);

    *LAST_REFRESH
        .lock()
        .expect("Unable to acquire LAST_REFRESH mutex") = Some(Instant::now());
}

pub(crate) fn set_brightness_percent(percent: u8) -> anyhow::Result<()> {
    let mut last_error = None;

    for monitor in MONITORS.lock().unwrap().iter_mut() {
        let res = monitor.set_brightness(percent);

        if let Err(err) = res {
            log::error!("Unable to set brightness of {}", monitor.name());
            last_error = Some(err);
        }
    }

    if let Some(err) = last_error {
        log::info!("Trying to refresh monitors list to fix the error...");
        refresh_monitors_list();
        Err(err)
    } else {
        log::info!("Brightness of all monitors has been set to {percent}%");
        Ok(())
    }
}

pub(crate) fn increase_brightness_percent(percent: u8) -> anyhow::Result<()> {
    let mut last_error = None;

    for monitor in MONITORS.lock().unwrap().iter_mut() {
        let mut new_brightness = monitor.get_brightness() + percent;

        if new_brightness > 100 {
            new_brightness = 100;
        }

        let res = monitor.set_brightness(new_brightness);

        if let Err(err) = res {
            log::error!("Unable to set brightness of {}", monitor.name());
            last_error = Some(err);
        }
    }

    if let Some(err) = last_error {
        log::info!("Trying to refresh monitors list to fix the error...");
        refresh_monitors_list();
        Err(err)
    } else {
        log::info!("Brightness of all monitors has been set to {percent}%");
        Ok(())
    }
}

pub(crate) fn decrease_brightness_percent(percent: u8) -> anyhow::Result<()> {
    let mut last_error = None;

    for monitor in MONITORS.lock().unwrap().iter_mut() {
        let mut new_brightness = monitor.get_brightness() as i8 - percent as i8;

        // Don't allow setting the brightness to 0 to prevent the monitor from being completely black.
        if new_brightness < 1 {
            new_brightness = 1;
        }

        let res = monitor.set_brightness(new_brightness as u8);

        if let Err(err) = res {
            log::error!("Unable to set brightness of {}: {err}", monitor.name());
            last_error = Some(err);
        }
    }

    if let Some(err) = last_error {
        log::info!("Trying to refresh monitors list to fix the error...");
        refresh_monitors_list();
        Err(err)
    } else {
        log::info!("Brightness of all monitors has been set to {percent}%");
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
        refresh_monitors_list();

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
        refresh_monitors_list();

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
    let mut sum: usize = 0;

    for monitor in &*monitors {
        sum += monitor.get_brightness() as usize;
    }

    (sum / monitors.len()) as u8
}
