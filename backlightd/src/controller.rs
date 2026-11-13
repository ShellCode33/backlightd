use std::{
    sync::mpsc::{Receiver, RecvTimeoutError, Sender},
    time::Duration,
};

use anyhow::bail;
use backlight_ipc::{BacklightCommand, BacklightInfo, BacklightMode};
use boot_time::Instant;

use crate::{auto, monitors};

const MANUAL_DURATION: Duration = Duration::from_secs(12 * 60 * 60);
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const RETRY_DELAY: Duration = Duration::from_secs(60);

pub(crate) struct Request {
    pub command: BacklightCommand,
    pub reply: Sender<anyhow::Result<BacklightInfo>>,
}

trait Devices {
    fn execute(&mut self, command: BacklightCommand) -> anyhow::Result<()>;
    fn brightness(&self) -> u8;
    fn automatic(&mut self) -> anyhow::Result<(u8, Duration)>;
}

struct Monitors;
impl Devices for Monitors {
    fn execute(&mut self, command: BacklightCommand) -> anyhow::Result<()> {
        match command {
            BacklightCommand::SetBrightness(p) => monitors::set_brightness_percent(p),
            BacklightCommand::IncreaseBrightness(p) => monitors::increase_brightness_percent(p),
            BacklightCommand::DecreaseBrightness(p) => monitors::decrease_brightness_percent(p),
            BacklightCommand::Refresh => {
                monitors::refresh_monitors_list();
                Ok(())
            }
            BacklightCommand::TurnOffMonitors => monitors::turn_off(),
            BacklightCommand::TurnOnMonitors => monitors::turn_on(),
            _ => bail!("Unsupported device command"),
        }
    }
    fn brightness(&self) -> u8 {
        monitors::get_average_brightness()
    }
    fn automatic(&mut self) -> anyhow::Result<(u8, Duration)> {
        auto::next_adjustment()
    }
}

struct Controller<D> {
    devices: D,
    mode: BacklightMode,
    deadline: Instant,
    refresh_deadline: Instant,
}

impl<D: Devices> Controller<D> {
    fn new(devices: D) -> Self {
        Self {
            devices,
            mode: BacklightMode::Auto,
            deadline: Instant::now(),
            refresh_deadline: Instant::now() + REFRESH_INTERVAL,
        }
    }

    fn adjust(&mut self) -> anyhow::Result<()> {
        // A failed calculation or write gets a bounded retry rather than a busy loop.
        self.deadline = Instant::now() + RETRY_DELAY;
        let (brightness, delay) = self.devices.automatic()?;
        self.devices
            .execute(BacklightCommand::SetBrightness(brightness))?;
        self.deadline = Instant::now() + delay;
        Ok(())
    }

    fn expire(&mut self) -> anyhow::Result<()> {
        self.mode = BacklightMode::Auto;
        self.adjust()
    }

    fn refresh(&mut self) -> anyhow::Result<()> {
        let result = self.devices.execute(BacklightCommand::Refresh);
        self.refresh_deadline = Instant::now() + REFRESH_INTERVAL;
        result?;
        if self.mode == BacklightMode::Auto {
            self.adjust()?;
        }
        Ok(())
    }

    fn service_deadlines(&mut self) {
        if Instant::now() >= self.refresh_deadline {
            if let Err(err) = self.refresh() {
                log::error!("Monitor refresh failed: {err}");
            }
        }
        // Check even when requests keep arriving, so neither deadline starves.
        if Instant::now() >= self.deadline {
            if let Err(err) = self.expire() {
                log::error!("Automatic adjustment failed: {err}");
            }
        }
    }

    fn command(&mut self, command: BacklightCommand) -> anyhow::Result<BacklightInfo> {
        self.service_deadlines();
        match command {
            BacklightCommand::SetBrightness(p)
            | BacklightCommand::IncreaseBrightness(p)
            | BacklightCommand::DecreaseBrightness(p) => {
                if p > 100 {
                    bail!("Brightness percentage must not exceed 100");
                }
                self.mode = BacklightMode::Manual;
                self.deadline = Instant::now() + MANUAL_DURATION;
                self.devices.execute(command)?;
            }
            BacklightCommand::SetMode(mode) => {
                self.mode = mode;
                if mode == BacklightMode::Auto {
                    self.adjust()?;
                } else {
                    self.deadline = Instant::now() + MANUAL_DURATION;
                }
            }
            BacklightCommand::GetInfo => {}
            BacklightCommand::Refresh => self.refresh()?,
            BacklightCommand::TurnOnMonitors | BacklightCommand::TurnOffMonitors => {
                self.devices.execute(command)?
            }
            _ => bail!("Unexpected response or shutdown command"),
        }
        Ok(BacklightInfo {
            brightness_percent: self.devices.brightness(),
            mode: self.mode,
        })
    }
}

pub(crate) fn run_controller(receiver: Receiver<Request>) {
    let mut controller = Controller::new(Monitors);
    if let Err(err) = controller.refresh() {
        log::error!("Initial monitor refresh failed: {err}");
    }
    loop {
        let delay = controller
            .deadline
            .min(controller.refresh_deadline)
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        match receiver.recv_timeout(delay) {
            Ok(request) => {
                let _ = request.reply.send(controller.command(request.command));
            }
            Err(RecvTimeoutError::Timeout) => {
                controller.service_deadlines();
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        brightness: u8,
        hardware_brightness: u8,
        refreshes: usize,
        fail: bool,
        writes: Vec<u8>,
    }
    impl Devices for Fake {
        fn execute(&mut self, command: BacklightCommand) -> anyhow::Result<()> {
            if self.fail {
                bail!("device failure");
            }
            match command {
                BacklightCommand::Refresh => {
                    self.brightness = self.hardware_brightness;
                    self.refreshes += 1;
                }
                BacklightCommand::SetBrightness(p) => {
                    self.brightness = p;
                    self.hardware_brightness = p;
                    self.writes.push(p);
                }
                BacklightCommand::IncreaseBrightness(p) => {
                    self.brightness = self.brightness.saturating_add(p).min(100);
                    self.hardware_brightness = self.brightness;
                }
                BacklightCommand::DecreaseBrightness(p) => {
                    self.brightness = self.brightness.saturating_sub(p).max(1);
                    self.hardware_brightness = self.brightness;
                }
                _ => {}
            }
            Ok(())
        }
        fn brightness(&self) -> u8 {
            self.brightness
        }
        fn automatic(&mut self) -> anyhow::Result<(u8, Duration)> {
            Ok((50, Duration::from_secs(60)))
        }
    }
    fn controller() -> Controller<Fake> {
        let mut c = Controller::new(Fake {
            brightness: 50,
            hardware_brightness: 50,
            refreshes: 0,
            fail: false,
            writes: vec![],
        });
        c.adjust().unwrap();
        c
    }
    #[test]
    fn explicit_refresh_precedes_relative_adjustment_in_manual_mode() {
        let mut c = controller();
        c.command(BacklightCommand::SetBrightness(20)).unwrap();
        let manual_deadline = c.deadline;
        // An external change becomes visible when discovery rebuilds the cache.
        c.devices.hardware_brightness = 40;
        c.command(BacklightCommand::Refresh).unwrap();
        let info = c.command(BacklightCommand::DecreaseBrightness(5)).unwrap();
        assert_eq!(
            (info.brightness_percent, info.mode),
            (35, BacklightMode::Manual)
        );
        assert_eq!(c.devices.refreshes, 1);
        assert!(c.deadline >= manual_deadline);
    }

    #[test]
    fn periodic_refresh_precedes_queued_commands_without_expiring_manual_mode() {
        let mut c = controller();
        c.command(BacklightCommand::SetBrightness(20)).unwrap();
        let manual_deadline = c.deadline;
        c.devices.hardware_brightness = 60;
        c.refresh_deadline = Instant::now();
        // An arriving status request must service the refresh, not postpone it.
        let info = c.command(BacklightCommand::GetInfo).unwrap();
        assert_eq!(
            (info.brightness_percent, info.mode),
            (60, BacklightMode::Manual)
        );
        assert_eq!(c.deadline, manual_deadline);
        let info = c.command(BacklightCommand::IncreaseBrightness(5)).unwrap();
        assert_eq!(info.brightness_percent, 65);
        assert_eq!(c.devices.refreshes, 1);
        assert!(c.refresh_deadline > Instant::now());
    }

    #[test]
    fn periodic_refresh_applies_auto_target_to_discovered_devices() {
        let mut c = controller();
        c.devices.hardware_brightness = 5;
        c.refresh_deadline = Instant::now();
        c.service_deadlines();
        assert_eq!(c.devices.refreshes, 1);
        assert_eq!(c.devices.hardware_brightness, 50);
        assert_eq!(c.mode, BacklightMode::Auto);
    }

    #[test]
    fn unchanged_auto_target_completes() {
        let mut c = controller();
        let info = c
            .command(BacklightCommand::SetMode(BacklightMode::Auto))
            .unwrap();
        assert_eq!(info.brightness_percent, 50);
        assert_eq!(info.mode, BacklightMode::Auto);
    }
    #[test]
    fn manual_auto_and_status_are_ordered() {
        let mut c = controller();
        c.command(BacklightCommand::SetBrightness(20)).unwrap();
        let info = c.command(BacklightCommand::GetInfo).unwrap();
        assert_eq!(
            (info.brightness_percent, info.mode),
            (20, BacklightMode::Manual)
        );
        c.command(BacklightCommand::SetMode(BacklightMode::Auto))
            .unwrap();
        c.command(BacklightCommand::SetBrightness(30)).unwrap();
        assert_eq!(c.devices.writes, vec![50, 20, 50, 30]);
    }
    #[test]
    fn expiration_applies_auto_before_status() {
        let mut c = controller();
        c.command(BacklightCommand::SetBrightness(20)).unwrap();
        c.deadline = Instant::now();
        let info = c.command(BacklightCommand::GetInfo).unwrap();
        assert_eq!(
            (info.brightness_percent, info.mode),
            (50, BacklightMode::Auto)
        );
    }
    #[test]
    fn failure_returns_error_and_preserves_responsive_status() {
        let mut c = controller();
        c.devices.fail = true;
        assert!(c.command(BacklightCommand::SetBrightness(20)).is_err());
        let info = c.command(BacklightCommand::GetInfo).unwrap();
        assert_eq!(
            (info.brightness_percent, info.mode),
            (50, BacklightMode::Manual)
        );
        assert!(c
            .command(BacklightCommand::SetMode(BacklightMode::Auto))
            .is_err());
        assert_eq!(
            c.command(BacklightCommand::GetInfo).unwrap().mode,
            BacklightMode::Auto
        );
    }
}
