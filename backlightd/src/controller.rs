use std::{
    sync::mpsc::{Receiver, RecvTimeoutError, Sender},
    time::Duration,
};

use anyhow::bail;
use backlight_ipc::{BacklightCommand, BacklightInfo, BacklightMode};
use boot_time::Instant;

use crate::{auto, monitors};

const MANUAL_DURATION: Duration = Duration::from_secs(12 * 60 * 60);
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
}

impl<D: Devices> Controller<D> {
    fn new(devices: D) -> Self {
        Self {
            devices,
            mode: BacklightMode::Auto,
            deadline: Instant::now(),
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

    fn command(&mut self, command: BacklightCommand) -> anyhow::Result<BacklightInfo> {
        // A command arriving at the deadline must observe the expired manual mode too.
        if Instant::now() >= self.deadline {
            if let Err(err) = self.expire() {
                log::error!("Automatic adjustment failed: {err}");
            }
        }
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
                self.devices.execute(command)?;
                if self.mode == BacklightMode::Auto {
                    self.adjust()?;
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

pub(crate) fn run_controller(receiver: Receiver<Request>) {
    let mut controller = Controller::new(Monitors);
    if let Err(err) = controller.adjust() {
        log::error!("Initial automatic adjustment failed: {err}");
    }
    loop {
        let delay = controller
            .deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        match receiver.recv_timeout(delay) {
            Ok(request) => {
                let _ = request.reply.send(controller.command(request.command));
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Err(err) = controller.expire() {
                    log::error!("Automatic adjustment failed: {err}");
                }
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
        fail: bool,
        writes: Vec<u8>,
    }
    impl Devices for Fake {
        fn execute(&mut self, command: BacklightCommand) -> anyhow::Result<()> {
            if self.fail {
                bail!("device failure");
            }
            if let BacklightCommand::SetBrightness(p) = command {
                self.brightness = p;
                self.writes.push(p);
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
            fail: false,
            writes: vec![],
        });
        c.adjust().unwrap();
        c
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
