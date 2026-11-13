use std::{sync::mpsc::Sender, time::Duration};

use anyhow::bail;
use backlight_ipc::{BacklightCommand, BacklightInfo, BacklightMode};
use boot_time::Instant;

use crate::{auto, monitors, wake::Receiver};

const MANUAL_DURATION: Duration = Duration::from_secs(12 * 60 * 60);
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const RETRY_DELAY: Duration = Duration::from_secs(60);

pub(crate) struct Request {
    pub command: BacklightCommand,
    pub received_at: Instant,
    pub reply: Sender<anyhow::Result<BacklightInfo>>,
}

trait Devices {
    fn execute(&mut self, command: BacklightCommand) -> anyhow::Result<()>;
    fn refresh(&mut self, reason: &str) -> anyhow::Result<()>;
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
            BacklightCommand::TurnOffMonitors => monitors::turn_off(),
            BacklightCommand::TurnOnMonitors => monitors::turn_on(),
            _ => bail!("Unsupported device command"),
        }
    }
    fn refresh(&mut self, reason: &str) -> anyhow::Result<()> {
        monitors::refresh_monitors_list(reason);
        Ok(())
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
    last_refresh_started: Option<Instant>,
    last_refresh_error: Option<String>,
}

impl<D: Devices> Controller<D> {
    fn new(devices: D) -> Self {
        Self {
            devices,
            mode: BacklightMode::Auto,
            deadline: Instant::now(),
            refresh_deadline: Instant::now() + REFRESH_INTERVAL,
            last_refresh_started: None,
            last_refresh_error: None,
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

    fn refresh(&mut self, reason: &str) -> anyhow::Result<()> {
        let started = std::time::Instant::now();
        self.last_refresh_started = Some(Instant::now());
        let result = self.devices.refresh(reason);
        let discovery_elapsed = started.elapsed();
        self.refresh_deadline = Instant::now() + REFRESH_INTERVAL;
        let adjustment_started = std::time::Instant::now();
        let result = result.and_then(|()| {
            if self.mode == BacklightMode::Auto {
                self.adjust()
            } else {
                Ok(())
            }
        });
        self.last_refresh_error = result.as_ref().err().map(ToString::to_string);
        log::debug!("Refresh finished: reason={reason} mode={} success={} discovery_ms={} adjustment_ms={} total_ms={}", self.mode, result.is_ok(), discovery_elapsed.as_millis(), adjustment_started.elapsed().as_millis(), started.elapsed().as_millis());
        result
    }

    fn service_deadlines(&mut self) {
        if Instant::now() >= self.refresh_deadline {
            if let Err(err) = self.refresh("scheduled") {
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

    #[cfg(test)]
    fn command(&mut self, command: BacklightCommand) -> anyhow::Result<BacklightInfo> {
        self.command_at(command, Instant::now())
    }

    fn command_at(
        &mut self,
        command: BacklightCommand,
        received_at: Instant,
    ) -> anyhow::Result<BacklightInfo> {
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
            BacklightCommand::Refresh => {
                // Only skip requests received before the last scan started.
                // An event during that scan still requires one follow-up scan.
                if self
                    .last_refresh_started
                    .is_some_and(|started| received_at <= started)
                {
                    log::debug!("Refresh coalesced: covered by a later discovery");
                    if let Some(err) = &self.last_refresh_error {
                        bail!("Refresh failed: {err}");
                    }
                } else {
                    self.refresh("requested")?;
                }
            }
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

pub(crate) fn run_controller(receiver: Receiver<Request>) -> anyhow::Result<()> {
    let mut controller = Controller::new(Monitors);
    if let Err(err) = controller.refresh("startup") {
        log::error!("Initial monitor refresh failed: {err}");
    }
    loop {
        let deadline = controller.deadline.min(controller.refresh_deadline);
        match receiver.recv_until(deadline) {
            Ok(Some(request)) => {
                let _ = request
                    .reply
                    .send(controller.command_at(request.command, request.received_at));
            }
            Ok(None) => controller.service_deadlines(),
            Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => return Ok(()),
            Err(err) => return Err(err.into()),
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
        fn refresh(&mut self, _reason: &str) -> anyhow::Result<()> {
            self.execute(BacklightCommand::Refresh)
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
    fn queued_refreshes_are_coalesced_without_losing_later_events() {
        let mut c = controller();
        let before_scan = Instant::now();
        c.refresh("test").unwrap();
        c.command_at(BacklightCommand::Refresh, before_scan)
            .unwrap();
        assert_eq!(c.devices.refreshes, 1);
        // A change received after the scan began must not be discarded.
        let during_scan = c.last_refresh_started.unwrap() + Duration::from_nanos(1);
        c.command_at(BacklightCommand::Refresh, during_scan)
            .unwrap();
        assert_eq!(c.devices.refreshes, 2);
        c.command_at(BacklightCommand::Refresh, during_scan)
            .unwrap();
        assert_eq!(c.devices.refreshes, 2);
    }

    #[test]
    fn scheduled_refresh_covers_an_already_queued_request() {
        let mut c = controller();
        c.refresh_deadline = Instant::now();
        c.command(BacklightCommand::Refresh).unwrap();
        assert_eq!(c.devices.refreshes, 1);
    }

    #[test]
    fn coalesced_requests_preserve_refresh_errors() {
        let mut c = controller();
        c.devices.fail = true;
        let received_at = Instant::now();
        assert!(c.refresh("test").is_err());
        assert!(c
            .command_at(BacklightCommand::Refresh, received_at)
            .is_err());
    }

    #[test]
    fn overdue_refresh_and_manual_override_are_serviced_together() {
        let mut c = controller();
        c.command(BacklightCommand::SetBrightness(20)).unwrap();
        c.refresh_deadline = Instant::now();
        c.deadline = Instant::now();
        c.service_deadlines();
        assert_eq!(c.devices.refreshes, 1);
        assert_eq!(c.mode, BacklightMode::Auto);
        assert_eq!(c.devices.hardware_brightness, 50);
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
