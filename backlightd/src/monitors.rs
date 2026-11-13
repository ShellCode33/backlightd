use std::{fs, sync::Mutex, time::Instant};

use crate::acpi::{BacklightAcpiDevice, ACPI_DEVICES_PATH};
use crate::ddc::BacklightDdcDevice;

/// Holds the current backlight state of all monitors.
///
/// Querying/writing to backlight devices is expensive, we try to cache as many things as possible
/// in order to avoid unnecessary I/O.
struct MonitorState {
    devices: Vec<Box<dyn BacklightDevice + Send>>,
    manual_target: Option<u8>,
    pending_inheritance: Vec<(String, u8)>,
}

static MONITORS: Mutex<MonitorState> = Mutex::new(MonitorState {
    devices: Vec::new(),
    manual_target: None,
    pending_inheritance: Vec::new(),
});

pub(crate) trait BacklightDevice {
    fn identity(&self) -> String;
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
    failures += replace_devices(&mut monitors, new_monitors, reason);
    log::debug!(
        "Discovery finished: reason={reason} monitors={} failed={failures} elapsed_ms={}",
        monitors.devices.len(),
        started.elapsed().as_millis()
    );
    anyhow::ensure!(
        failures == 0,
        "Discovery incomplete: {failures} probe(s) failed"
    );
    Ok(())
}

pub(crate) fn set_manual_target(target: Option<u8>) {
    let mut state = MONITORS.lock().unwrap();
    state.manual_target = target;
    if target.is_none() {
        state.pending_inheritance.clear();
    }
}

fn replace_devices(
    state: &mut MonitorState,
    mut discovered: Vec<Box<dyn BacklightDevice + Send>>,
    reason: &str,
) -> usize {
    let known: Vec<_> = state
        .devices
        .iter()
        .map(|device| device.identity())
        .collect();
    let mut pending = Vec::new();
    for device in &mut discovered {
        let id = device.identity();
        // Retry the original absolute target, never apply a relative step twice.
        let target = state
            .pending_inheritance
            .iter()
            .find(|(failed_id, _)| failed_id == &id)
            .map(|(_, target)| *target)
            .or_else(|| {
                (reason == "resume" || !known.contains(&id))
                    .then_some(state.manual_target)
                    .flatten()
            });
        if let Some(target) = target {
            let write_started = Instant::now();
            let result = device.set_brightness(target);
            log::debug!(
                "Brightness target inheritance: monitor={} id={id:?} reason={reason} target_percent={target} success={} elapsed_ms={}",
                device.name(), result.is_ok(), write_started.elapsed().as_millis()
            );
            if let Err(err) = result {
                log::error!(
                    "Unable to inherit brightness target: monitor={} id={id:?} reason={reason} target_percent={target}: {err}",
                    device.name()
                );
                pending.push((id, target));
            }
        }
    }
    let failures = pending.len();
    state.pending_inheritance = pending;
    state.devices = discovered;
    failures
}

fn refresh_after_error(reason: &str, failed_ids: Vec<String>) {
    let mut state = MONITORS.lock().unwrap();
    if let Some(target) = state.manual_target {
        state
            .pending_inheritance
            .extend(failed_ids.into_iter().map(|id| (id, target)));
    }
    drop(state);
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

struct BrightnessFailure {
    error: anyhow::Error,
    ids: Vec<String>,
}

fn write_brightness(
    state: &mut MonitorState,
    target: impl Fn(u8) -> u8,
) -> Result<(), BrightnessFailure> {
    let mut failure = None;
    let mut failed_targets = Vec::new();
    for monitor in &mut state.devices {
        let percent = target(monitor.get_brightness());
        let id = monitor.identity();
        state
            .pending_inheritance
            .retain(|(pending_id, _)| pending_id != &id);
        let started = Instant::now();
        let result = monitor.set_brightness(percent);
        log::debug!(
            "Brightness write: monitor={} target_percent={percent} success={} elapsed_ms={}",
            monitor.name(),
            result.is_ok(),
            started.elapsed().as_millis()
        );
        if let Err(err) = result {
            log::error!("Unable to set brightness of {}: {err}", monitor.name());
            failed_targets.push((id, percent));
            failure = Some(err);
        }
    }
    if let Some(error) = failure {
        let ids = failed_targets.iter().map(|(id, _)| id.clone()).collect();
        state.pending_inheritance.extend(failed_targets);
        Err(BrightnessFailure { error, ids })
    } else {
        Ok(())
    }
}

fn brightness_recovery_result(
    state: &MonitorState,
    failure: BrightnessFailure,
    discovery: anyhow::Result<()>,
) -> anyhow::Result<()> {
    if let Err(err) = discovery {
        return Err(failure
            .error
            .context(format!("Brightness recovery failed: {err}")));
    }
    if failure
        .ids
        .iter()
        .any(|id| !state.devices.iter().any(|device| device.identity() == *id))
    {
        return Err(failure
            .error
            .context("Brightness recovery could not find a failed monitor"));
    }
    Ok(())
}

fn change_brightness(target: impl Fn(u8) -> u8, reason: &str) -> anyhow::Result<()> {
    let result = write_brightness(&mut MONITORS.lock().unwrap(), target);
    if let Err(failure) = result {
        log::info!("Trying to refresh monitors list to fix the error...");
        let discovery = refresh_monitors_list(reason);
        brightness_recovery_result(&MONITORS.lock().unwrap(), failure, discovery)?;
        log::info!("Brightness write recovered after discovery");
    }
    Ok(())
}

pub(crate) fn set_brightness_percent(percent: u8) -> anyhow::Result<()> {
    change_brightness(|_| percent, "set_brightness_error")?;
    log::info!("Brightness of all monitors has been set to {percent}%");
    Ok(())
}

pub(crate) fn increase_brightness_percent(percent: u8) -> anyhow::Result<()> {
    change_brightness(
        |current| current.saturating_add(percent).min(100),
        "increase_brightness_error",
    )?;
    log::info!("Brightness of all monitors increased by {percent} percentage points");
    Ok(())
}

pub(crate) fn decrease_brightness_percent(percent: u8) -> anyhow::Result<()> {
    // Don't allow a relative decrease to black out the display.
    change_brightness(
        |current| current.saturating_sub(percent).max(1),
        "decrease_brightness_error",
    )?;
    log::info!("Brightness of all monitors decreased by {percent} percentage points");
    Ok(())
}

pub(crate) fn turn_off() -> anyhow::Result<()> {
    let mut last_error = None;
    let mut failed_ids = Vec::new();

    for monitor in MONITORS.lock().unwrap().devices.iter_mut() {
        if let Err(err) = monitor.turn_off() {
            log::error!("Unable to turn OFF monitor: {err}");
            failed_ids.push(monitor.identity());
            last_error = Some(err);
        }
    }

    if last_error.is_some() {
        log::info!("Trying to refresh monitors list to fix the error and retry...");
        refresh_after_error("turn_off_error", failed_ids);

        last_error = None;
        for monitor in MONITORS.lock().unwrap().devices.iter_mut() {
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
    let mut failed_ids = Vec::new();

    for monitor in MONITORS.lock().unwrap().devices.iter_mut() {
        if let Err(err) = monitor.turn_on() {
            log::error!("Unable to turn ON monitor: {err}");
            failed_ids.push(monitor.identity());
            last_error = Some(err);
        }
    }

    if last_error.is_some() {
        log::info!("Trying to refresh monitors list to fix the error and retry...");
        refresh_after_error("turn_on_error", failed_ids);

        last_error = None;
        for monitor in MONITORS.lock().unwrap().devices.iter_mut() {
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
    let state = MONITORS.lock().unwrap();
    let monitors = &state.devices;

    if monitors.is_empty() {
        return 0;
    }

    let mut sum: usize = 0;

    for monitor in monitors {
        sum += monitor.get_brightness() as usize;
    }

    (sum / monitors.len()) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeDevice {
        id: &'static str,
        brightness: u8,
        fail: bool,
    }
    impl BacklightDevice for FakeDevice {
        fn identity(&self) -> String {
            self.id.into()
        }
        fn name(&self) -> String {
            "same model".into()
        }
        fn set_brightness(&mut self, percent: u8) -> anyhow::Result<()> {
            if self.fail {
                anyhow::bail!("device not ready");
            }
            self.brightness = percent;
            Ok(())
        }
        fn get_brightness(&self) -> u8 {
            self.brightness
        }
        fn turn_off(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
        fn turn_on(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
    }
    fn device(id: &'static str, brightness: u8, fail: bool) -> Box<dyn BacklightDevice + Send> {
        Box::new(FakeDevice {
            id,
            brightness,
            fail,
        })
    }

    #[test]
    fn brightness_recovery_retries_absolute_targets_without_repeating_relative_steps() {
        type Adjustment = fn(u8) -> u8;
        let operations: [(Adjustment, u8, u8); 3] = [
            (|_| 60, 60, 60),
            (|current| current + 5, 95, 45),
            (|current| current - 5, 85, 35),
        ];
        for (operation, a_target, b_target) in operations {
            let mut state = MonitorState {
                devices: vec![device("a", 90, false), device("b", 40, true)],
                manual_target: Some(60),
                pending_inheritance: vec![],
            };
            let failure = write_brightness(&mut state, operation).err().unwrap();
            assert_eq!(state.devices[0].get_brightness(), a_target);
            assert_eq!(state.pending_inheritance, vec![("b".into(), b_target)]);
            // b may have accepted the write before reporting an error. Retry the
            // intended absolute target, irrespective of its new hardware reading.
            let failures = replace_devices(
                &mut state,
                vec![device("a", a_target, false), device("b", b_target, false)],
                "test",
            );
            assert_eq!(failures, 0);
            assert!(brightness_recovery_result(&state, failure, Ok(())).is_ok());
            assert_eq!(state.devices[0].get_brightness(), a_target);
            assert_eq!(state.devices[1].get_brightness(), b_target);
        }
    }

    #[test]
    fn brightness_recovery_reports_persistent_failure_or_missing_device() {
        for missing in [false, true] {
            let mut state = MonitorState {
                devices: vec![device("a", 90, false), device("b", 40, true)],
                manual_target: Some(60),
                pending_inheritance: vec![],
            };
            let failure = write_brightness(&mut state, |_| 60).err().unwrap();
            let mut discovered = vec![device("a", 60, false)];
            if !missing {
                discovered.push(device("b", 40, true));
            }
            let failures = replace_devices(&mut state, discovered, "test");
            let discovery = if failures == 0 {
                Ok(())
            } else {
                Err(anyhow::anyhow!("inheritance failed"))
            };
            let err = brightness_recovery_result(&state, failure, discovery).unwrap_err();
            assert!(format!("{err:#}").contains("device not ready"));
            if missing {
                assert!(err.to_string().contains("could not find"));
            } else {
                assert_eq!(state.pending_inheritance, vec![("b".into(), 60)]);
            }
        }
    }

    #[test]
    fn automatic_write_recovery_works_without_manual_inheritance() {
        let mut state = MonitorState {
            devices: vec![device("a", 100, true)],
            manual_target: None,
            pending_inheritance: vec![],
        };
        let failure = write_brightness(&mut state, |_| 50).err().unwrap();
        assert_eq!(
            replace_devices(&mut state, vec![device("a", 100, false)], "test"),
            0
        );
        assert!(brightness_recovery_result(&state, failure, Ok(())).is_ok());
        assert_eq!(state.devices[0].get_brightness(), 50);
    }

    #[test]
    fn successful_new_write_clears_an_older_pending_target() {
        let mut state = MonitorState {
            devices: vec![device("a", 100, false)],
            manual_target: Some(60),
            pending_inheritance: vec![("a".into(), 95)],
        };
        assert!(write_brightness(&mut state, |_| 60).is_ok());
        assert!(state.pending_inheritance.is_empty());
        replace_devices(&mut state, vec![device("a", 60, false)], "test");
        assert_eq!(state.devices[0].get_brightness(), 60);
    }

    #[test]
    fn inheritance_only_changes_new_or_recovered_identities() {
        let mut state = MonitorState {
            devices: vec![device("a", 95, false)],
            manual_target: Some(95),
            pending_inheritance: vec![],
        };
        assert_eq!(
            replace_devices(
                &mut state,
                vec![device("a", 80, false), device("b", 100, false)],
                "test"
            ),
            0
        );
        assert_eq!(state.devices[0].get_brightness(), 80);
        assert_eq!(state.devices[1].get_brightness(), 95);
        // Removing and reintroducing b must inherit again, despite the same model name.
        replace_devices(&mut state, vec![device("a", 80, false)], "test");
        replace_devices(
            &mut state,
            vec![device("a", 80, false), device("b", 100, false)],
            "test",
        );
        assert_eq!(state.devices[1].get_brightness(), 95);
    }

    #[test]
    fn failed_inheritance_is_retried_and_reported() {
        let mut state = MonitorState {
            devices: vec![],
            manual_target: Some(95),
            pending_inheritance: vec![],
        };
        assert_eq!(
            replace_devices(&mut state, vec![device("a", 100, true)], "test"),
            1
        );
        assert_eq!(state.pending_inheritance, vec![("a".into(), 95)]);
        assert_eq!(
            replace_devices(&mut state, vec![device("a", 100, false)], "test"),
            0
        );
        assert_eq!(state.devices[0].get_brightness(), 95);
        assert!(state.pending_inheritance.is_empty());
        // A failed operation on an already listed monitor also marks it for inheritance.
        state.pending_inheritance.push(("a".into(), 95));
        replace_devices(&mut state, vec![device("a", 100, false)], "test");
        assert_eq!(state.devices[0].get_brightness(), 95);
    }

    #[test]
    fn resume_reconciles_known_monitors_but_auto_has_no_manual_target() {
        let mut state = MonitorState {
            devices: vec![device("a", 95, false)],
            manual_target: Some(95),
            pending_inheritance: vec![],
        };
        replace_devices(&mut state, vec![device("a", 100, false)], "resume");
        assert_eq!(state.devices[0].get_brightness(), 95);
        state.manual_target = None;
        replace_devices(&mut state, vec![device("a", 100, false)], "resume");
        assert_eq!(state.devices[0].get_brightness(), 100);
    }

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
