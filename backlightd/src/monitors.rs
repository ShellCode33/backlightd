use std::{
    fs::{self},
    sync::Mutex,
    thread,
    time::Duration,
};

use chrono::{DateTime, Local};

use crate::acpi::{BacklightAcpiDevice, ACPI_DEVICES_PATH};
use crate::ddc::BacklightDdcDevice;

/// Holds the current backlight state of all monitors.
///
/// Querying/writing to backlight devices is expensive, we try to cache as many things as possible
/// in order to avoid unnecessary I/O.
static MONITORS: Mutex<Vec<Box<dyn BacklightDevice + Send>>> = Mutex::new(Vec::new());

/// The last time the list of known monitors was refreshed.
static LAST_REFRESH: Mutex<Option<DateTime<Local>>> = Mutex::new(None);

/// The frequency at which the list of known monitors must be refreshed.
const MONITORS_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

pub(crate) trait BacklightDevice {
    fn name(&self) -> String;
    fn set_brightness(&mut self, percent: u8) -> anyhow::Result<()>;
    fn get_brightness(&self) -> u8;
}

pub(crate) fn auto_refresh_monitors_list() -> ! {
    loop {
        let must_refresh = {
            let last_refresh = LAST_REFRESH
                .lock()
                .expect("Unable to acquire LAST_REFRESH mutex");

            last_refresh.is_none()
                || last_refresh.is_some_and(|dt| {
                    (Local::now() - dt)
                        .to_std()
                        .unwrap_or(MONITORS_REFRESH_INTERVAL + Duration::from_secs(1))
                        > MONITORS_REFRESH_INTERVAL
                })
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
            Err(err) => eprintln!("Failed to retrieve DDC backlight monitor: {err}"),
        }
    }

    match fs::read_dir(ACPI_DEVICES_PATH) {
        Ok(dir) => {
            for entry in dir {
                match entry {
                    Ok(file) => match BacklightAcpiDevice::new(file.path()) {
                        Ok(monitor) => new_monitors.push(Box::new(monitor)),
                        Err(err) => println!("Failed to retrieve ACPI backlight monitor: {err}"),
                    },
                    Err(err) => {
                        eprintln!("Unable to read entry from {ACPI_DEVICES_PATH}: {err}");
                    }
                }
            }
        }
        Err(err) => {
            eprintln!("{ACPI_DEVICES_PATH}: {err}");
            // fallthrough
        }
    }

    let mut monitors = MONITORS.lock().expect("Unable to acquire MONITORS mutex");
    monitors.clear();
    monitors.extend(new_monitors);

    *LAST_REFRESH
        .lock()
        .expect("Unable to acquire LAST_REFRESH mutex") = Some(Local::now());
}

pub(crate) fn set_brightness_percent(percent: u8) -> anyhow::Result<()> {
    for monitor in MONITORS.lock().unwrap().iter_mut() {
        monitor.set_brightness(percent)?;
    }
    Ok(())
}

pub(crate) fn increase_brightness_percent(percent: u8) -> anyhow::Result<()> {
    for monitor in MONITORS.lock().unwrap().iter_mut() {
        let mut new_brightness = monitor.get_brightness() + percent;

        if new_brightness > 100 {
            new_brightness = 100;
        }

        monitor.set_brightness(new_brightness)?;
    }
    Ok(())
}

pub(crate) fn decrease_brightness_percent(percent: u8) -> anyhow::Result<()> {
    for monitor in MONITORS.lock().unwrap().iter_mut() {
        let mut new_brightness = monitor.get_brightness() as isize - percent as isize;

        // Don't allow setting the brightness to 0 to prevent the monitor from being completely black.
        if new_brightness < 1 {
            new_brightness = 1;
        }

        monitor.set_brightness(new_brightness as u8)?;
    }
    Ok(())
}
