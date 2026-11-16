//! Serialized commands, mode/override state, configuration application, and deadlines.
//!
//! EDID enumeration runs in a worker; VCP operations remain serialized. Boottime deadlines include
//! suspend; manual intent is recorded before hardware I/O and survives recovery.

use std::{sync::mpsc::Sender, time::Duration};

use anyhow::bail;
use backlight_ipc::{BacklightCommand, BacklightInfo, BacklightMode};
use boot_time::Instant;

use crate::{auto, monitors, wake::Receiver};

const REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const HOTPLUG_SETTLE: Duration = Duration::from_millis(250);
const HOTPLUG_MAX_DELAY: Duration = Duration::from_secs(1);
const RETRY_DELAY: Duration = Duration::from_secs(60);

pub(crate) struct Request {
    pub command: BacklightCommand,
    pub reload: Option<std::sync::Arc<crate::config::Config>>,
    pub received_at: Instant,
    pub reply: Sender<anyhow::Result<BacklightInfo>>,
}

trait Devices {
    fn configure(&mut self, _config: std::sync::Arc<crate::config::Config>) {}
    fn execute(&mut self, command: BacklightCommand) -> anyhow::Result<()>;
    fn refresh(&mut self, reason: &str) -> anyhow::Result<()>;
    fn start_refresh(&mut self) -> anyhow::Result<bool> {
        Ok(false)
    }
    fn refresh_fd(&self) -> Option<std::os::fd::RawFd> {
        None
    }
    fn refresh_ready(&self) -> bool {
        false
    }
    fn finish_refresh(&mut self, reason: &str) -> anyhow::Result<()> {
        self.refresh(reason)
    }
    fn set_manual_target(&mut self, target: Option<u8>);
    fn brightness(&self) -> u8;
    fn target_brightness(&self) -> u8 {
        self.brightness()
    }
    fn device_info(&self, _index: usize) -> Option<backlight_ipc::DeviceInfo> {
        None
    }
    fn automatic(&mut self) -> anyhow::Result<(u8, Duration)>;
}

#[derive(Default)]
struct Monitors {
    enumeration: Option<crate::discovery::Enumeration<Vec<ddc_hi::Display>>>,
}
impl Devices for Monitors {
    fn start_refresh(&mut self) -> anyhow::Result<bool> {
        self.enumeration = Some(crate::discovery::Enumeration::spawn(
            monitors::enumerate_displays,
        )?);
        Ok(true)
    }
    fn refresh_fd(&self) -> Option<std::os::fd::RawFd> {
        self.enumeration.as_ref().map(|worker| worker.fd())
    }
    fn refresh_ready(&self) -> bool {
        self.enumeration
            .as_ref()
            .is_some_and(|worker| worker.ready())
    }
    fn finish_refresh(&mut self, reason: &str) -> anyhow::Result<()> {
        let displays = self
            .enumeration
            .take()
            .expect("active enumeration")
            .finish()?;
        monitors::refresh_with_displays(reason, displays)
    }

    fn device_info(&self, index: usize) -> Option<backlight_ipc::DeviceInfo> {
        monitors::device_info(index)
    }
    fn configure(&mut self, config: std::sync::Arc<crate::config::Config>) {
        crate::config::publish(config);
    }
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
        monitors::refresh_monitors_list(reason)
    }
    fn set_manual_target(&mut self, target: Option<u8>) {
        monitors::set_manual_target(target);
    }
    fn brightness(&self) -> u8 {
        monitors::get_average_brightness()
    }
    fn automatic(&mut self) -> anyhow::Result<(u8, Duration)> {
        auto::next_adjustment()
    }
}

struct PendingRefresh {
    deadline: Instant,
    limit: Instant,
}

struct Controller<D> {
    devices: D,
    mode: BacklightMode,
    deadline: Instant,
    refresh_deadline: Instant,
    pending_refresh: Option<PendingRefresh>,
    last_refresh_started: Option<Instant>,
    last_refresh_completed_started: Option<Instant>,
    last_refresh_error: Option<String>,
    last_awake_sample: std::time::Instant,
    last_boot_sample: Instant,
    config: std::sync::Arc<crate::config::Config>,
    manual_started: Option<Instant>,
    manual_forever: bool,
    active_refresh: Option<(&'static str, std::time::Instant)>,
    resume_pending: bool,
}

impl<D: Devices> Controller<D> {
    fn new(devices: D) -> Self {
        let last_awake_sample = std::time::Instant::now();
        let last_boot_sample = Instant::now();
        Self {
            devices,
            mode: BacklightMode::Auto,
            deadline: Instant::now(),
            refresh_deadline: Instant::now() + REFRESH_INTERVAL,
            pending_refresh: None,
            last_refresh_started: None,
            last_refresh_completed_started: None,
            last_refresh_error: None,
            last_awake_sample,
            last_boot_sample,
            config: crate::config::current(),
            manual_started: None,
            manual_forever: false,
            active_refresh: None,
            resume_pending: false,
        }
    }

    fn begin_manual(&mut self) {
        self.manual_started = Some(Instant::now());
        self.update_manual_deadline();
    }
    fn update_manual_deadline(&mut self) {
        self.manual_forever = self.config.manual_duration().is_none();
        if let (Some(started), Some(duration)) =
            (self.manual_started, self.config.manual_duration())
        {
            self.deadline = started + duration;
        }
    }
    fn reload(&mut self, config: std::sync::Arc<crate::config::Config>) {
        self.config = config.clone();
        self.devices.configure(config);
        self.refresh_deadline = Instant::now();
        if self.mode == BacklightMode::Manual {
            self.update_manual_deadline();
        } else {
            self.deadline = Instant::now();
        }
        self.service_deadlines();
        log::info!("Configuration reloaded");
    }

    fn adjust(&mut self) -> anyhow::Result<()> {
        self.devices.set_manual_target(None);
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

    fn complete_refresh(
        &mut self,
        reason: &str,
        started: std::time::Instant,
        result: anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.last_refresh_completed_started = self.last_refresh_started;
        let discovery_elapsed = started.elapsed();
        self.refresh_deadline = Instant::now() + REFRESH_INTERVAL;
        let adjustment_started = std::time::Instant::now();
        let adjustment = if self.mode == BacklightMode::Auto {
            self.adjust()
        } else {
            Ok(())
        };
        let result = result.and(adjustment);
        self.last_refresh_error = result.as_ref().err().map(ToString::to_string);
        log::debug!("Refresh finished: reason={reason} mode={} success={} discovery_ms={} adjustment_ms={} total_ms={}", self.mode, result.is_ok(), discovery_elapsed.as_millis(), adjustment_started.elapsed().as_millis(), started.elapsed().as_millis());
        result
    }

    fn finish_background_refresh(&mut self) -> anyhow::Result<()> {
        let (reason, started) = self.active_refresh.take().expect("active refresh");
        // Read brightness now, on the controller, rather than merging an old worker
        // snapshot over commands or configuration changes received during enumeration.
        let result = self.devices.finish_refresh(reason);
        self.complete_refresh(reason, started, result)
    }

    fn refresh(&mut self, reason: &str) -> anyhow::Result<()> {
        if self.active_refresh.is_some() {
            if let Err(err) = self.finish_background_refresh() {
                log::error!("Background refresh failed: {err}");
            }
        }
        let started = std::time::Instant::now();
        self.last_refresh_started = Some(Instant::now());
        self.pending_refresh = None;
        let result = self.devices.refresh(reason);
        self.complete_refresh(reason, started, result)
    }

    fn start_background_refresh(&mut self, reason: &'static str) -> anyhow::Result<()> {
        let started = std::time::Instant::now();
        self.last_refresh_started = Some(Instant::now());
        self.pending_refresh = None;
        self.resume_pending = false;
        log::debug!("Background enumeration started: reason={reason}");
        match self.devices.start_refresh() {
            Ok(true) => {
                self.active_refresh = Some((reason, started));
                Ok(())
            }
            Ok(false) => {
                let result = self.devices.refresh(reason);
                self.complete_refresh(reason, started, result)
            }
            Err(err) => self.complete_refresh(reason, started, Err(err)),
        }
    }

    fn service_deadlines(&mut self) {
        // BOOTTIME advances through suspend; std::Instant (MONOTONIC on Linux)
        // does not. Bracket its sample to avoid counting scheduling delays as sleep.
        let boot_before = Instant::now();
        let awake = std::time::Instant::now();
        let boot_after = Instant::now();
        let resumed = boot_before.duration_since(self.last_boot_sample)
            > awake.duration_since(self.last_awake_sample) + Duration::from_millis(100);
        self.last_boot_sample = boot_after;
        self.last_awake_sample = awake;
        self.service_deadlines_after_resume(resumed);
    }

    fn service_deadlines_after_resume(&mut self, resumed: bool) {
        self.resume_pending |= resumed;
        if !self.manual_forever
            && Instant::now() >= self.deadline
            && self.mode == BacklightMode::Manual
        {
            self.mode = BacklightMode::Auto;
            self.devices.set_manual_target(None);
        }
        if self.active_refresh.is_some() && self.devices.refresh_ready() {
            if let Err(err) = self.finish_background_refresh() {
                log::error!("Background refresh failed: {err}");
            }
        }
        let hotplug_due = self
            .pending_refresh
            .as_ref()
            .is_some_and(|pending| Instant::now() >= pending.deadline);
        if self.active_refresh.is_none()
            && (self.resume_pending || Instant::now() >= self.refresh_deadline || hotplug_due)
        {
            let reason = if self.resume_pending {
                "resume"
            } else if hotplug_due {
                "hotplug"
            } else {
                "scheduled"
            };
            if let Err(err) = self.start_background_refresh(reason) {
                log::error!("Monitor refresh failed: {err}");
            }
        }
        // Check even when requests keep arriving, so neither deadline starves.
        if !(self.mode == BacklightMode::Manual && self.manual_forever)
            && Instant::now() >= self.deadline
        {
            if let Err(err) = self.expire() {
                log::error!("Automatic adjustment failed: {err}");
            }
        }
    }

    fn notify_hotplug(&mut self, received_at: Instant, now: Instant) {
        if self
            .last_refresh_started
            .is_some_and(|started| received_at <= started)
        {
            return;
        }
        let pending = self.pending_refresh.get_or_insert(PendingRefresh {
            deadline: now + HOTPLUG_SETTLE,
            limit: now + HOTPLUG_MAX_DELAY,
        });
        pending.deadline = (now + HOTPLUG_SETTLE).min(pending.limit);
    }

    fn next_deadline(&self) -> Instant {
        if self.active_refresh.is_some() {
            // Completion wakes poll directly; do not spin on overdue scan deadlines.
            return if self.mode == BacklightMode::Manual && self.manual_forever {
                Instant::now() + Duration::from_secs(86400)
            } else {
                self.deadline
            };
        }
        if self.resume_pending {
            return Instant::now();
        }
        let deadline = if self.mode == BacklightMode::Manual && self.manual_forever {
            self.refresh_deadline
        } else {
            self.deadline.min(self.refresh_deadline)
        };
        self.pending_refresh
            .as_ref()
            .map_or(deadline, |pending| deadline.min(pending.deadline))
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
        // Include a queued notification before checking its settling deadline.
        if matches!(command, BacklightCommand::NotifyHotplug) {
            self.notify_hotplug(received_at, Instant::now());
        }
        self.service_deadlines();
        let device_index = if let BacklightCommand::GetDeviceInfo(index) = &command {
            Some(*index)
        } else {
            None
        };
        match command {
            BacklightCommand::SetBrightness(p)
            | BacklightCommand::IncreaseBrightness(p)
            | BacklightCommand::DecreaseBrightness(p) => {
                if p > 100 {
                    bail!("Brightness percentage must not exceed 100");
                }
                let target = match &command {
                    BacklightCommand::SetBrightness(p) => *p,
                    BacklightCommand::IncreaseBrightness(p) => {
                        self.devices.target_brightness().saturating_add(*p).min(100)
                    }
                    BacklightCommand::DecreaseBrightness(p) => {
                        self.devices.target_brightness().saturating_sub(*p).max(1)
                    }
                    _ => unreachable!(),
                };
                self.mode = BacklightMode::Manual;
                self.begin_manual();
                // Record intent before I/O so recovery scans inherit it even if
                // this operation only succeeds on some of the monitors.
                self.devices.set_manual_target(Some(target));
                self.devices.execute(command)?;
            }
            BacklightCommand::SetMode(mode) => {
                self.mode = mode;
                if mode == BacklightMode::Auto {
                    self.adjust()?;
                } else {
                    self.devices
                        .set_manual_target(Some(self.devices.brightness()));
                    self.begin_manual();
                }
            }
            BacklightCommand::GetInfo
            | BacklightCommand::GetDeviceInfo(_)
            | BacklightCommand::NotifyHotplug => {}
            BacklightCommand::Refresh => {
                if self.active_refresh.is_some() {
                    self.finish_background_refresh()?;
                }
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
        let device = device_index.and_then(|index| self.devices.device_info(index));
        Ok(BacklightInfo {
            device,
            brightness_percent: self.devices.brightness(),
            mode: self.mode,
        })
    }
}

pub(crate) fn run_controller(
    receiver: Receiver<Request>,
    stopping: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<()> {
    run_with_devices(receiver, stopping, Monitors::default())
}

fn run_with_devices<D: Devices>(
    receiver: Receiver<Request>,
    stopping: std::sync::Arc<std::sync::atomic::AtomicBool>,
    devices: D,
) -> anyhow::Result<()> {
    if stopping.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(());
    }
    let mut controller = Controller::new(devices);
    if let Err(err) = controller.refresh("startup") {
        log::error!("Initial monitor refresh failed: {err}");
    }
    let mut refresh_waiters: Vec<Request> = Vec::new();
    loop {
        if stopping.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }
        controller.service_deadlines();
        refresh_waiters.retain(|request| {
            if !controller
                .last_refresh_completed_started
                .is_some_and(|started| request.received_at <= started)
            {
                return true;
            }
            let result = match &controller.last_refresh_error {
                Some(err) => Err(anyhow::anyhow!("Refresh failed: {err}")),
                None => Ok(BacklightInfo {
                    brightness_percent: controller.devices.brightness(),
                    mode: controller.mode,
                    device: None,
                }),
            };
            let _ = request.reply.send(result);
            false
        });
        if !refresh_waiters.is_empty() && controller.active_refresh.is_none() {
            if let Err(err) = controller.start_background_refresh("requested") {
                log::error!("Requested refresh failed: {err}");
            }
            // Synchronous test adapters and spawn failures complete immediately.
            if controller.active_refresh.is_none() {
                continue;
            }
        }
        let deadline = controller.next_deadline();
        let request = receiver.recv_until_notified(deadline, controller.devices.refresh_fd());
        if stopping.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }
        match request {
            Ok(Some(request)) => {
                if matches!(request.command, BacklightCommand::Refresh) {
                    if refresh_waiters.len() >= crate::MAX_CLIENTS {
                        let _ = request
                            .reply
                            .send(Err(anyhow::anyhow!("Too many pending refresh requests")));
                    } else {
                        refresh_waiters.push(request);
                    }
                    continue;
                }
                if let Some(config) = request.reload {
                    controller.reload(config);
                    continue;
                }
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
        partial_refresh: bool,
        manual_target: Option<u8>,
        discovered_new: bool,
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
        fn refresh(&mut self, reason: &str) -> anyhow::Result<()> {
            self.execute(BacklightCommand::Refresh)?;
            if self.discovered_new || reason == "resume" {
                if let Some(target) = self.manual_target {
                    self.execute(BacklightCommand::SetBrightness(target))?;
                }
                self.discovered_new = false;
            }
            if self.partial_refresh {
                bail!("Discovery incomplete: 1 probe failed");
            }
            Ok(())
        }
        fn set_manual_target(&mut self, target: Option<u8>) {
            self.manual_target = target;
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
            partial_refresh: false,
            manual_target: None,
            discovered_new: false,
            writes: vec![],
        });
        c.adjust().unwrap();
        c
    }
    struct BackgroundFake {
        inner: Fake,
        gates: std::collections::VecDeque<std::sync::mpsc::Receiver<()>>,
        started: std::sync::mpsc::Sender<()>,
        worker: Option<crate::discovery::Enumeration<()>>,
    }
    impl Devices for BackgroundFake {
        fn execute(&mut self, command: BacklightCommand) -> anyhow::Result<()> {
            self.inner.execute(command)
        }
        fn refresh(&mut self, reason: &str) -> anyhow::Result<()> {
            self.inner.refresh(reason)
        }
        fn set_manual_target(&mut self, target: Option<u8>) {
            self.inner.set_manual_target(target);
        }
        fn brightness(&self) -> u8 {
            self.inner.brightness()
        }
        fn automatic(&mut self) -> anyhow::Result<(u8, Duration)> {
            self.inner.automatic()
        }
        fn start_refresh(&mut self) -> anyhow::Result<bool> {
            let gate = self.gates.pop_front();
            self.worker = Some(crate::discovery::Enumeration::spawn(move || {
                if let Some(gate) = gate {
                    gate.recv_timeout(Duration::from_secs(5)).unwrap();
                }
            })?);
            self.started.send(()).unwrap();
            Ok(true)
        }
        fn refresh_fd(&self) -> Option<std::os::fd::RawFd> {
            self.worker.as_ref().map(|worker| worker.fd())
        }
        fn refresh_ready(&self) -> bool {
            self.worker.as_ref().is_some_and(|worker| worker.ready())
        }
        fn finish_refresh(&mut self, reason: &str) -> anyhow::Result<()> {
            self.worker.take().unwrap().finish()?;
            self.inner.refresh(reason)
        }
    }

    #[test]
    fn background_and_explicit_discovery_do_not_block_commands_or_overwrite_targets() {
        let (sender, receiver) = crate::wake::channel().unwrap();
        let stopping = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (first, first_gate) = std::sync::mpsc::channel();
        let (second, second_gate) = std::sync::mpsc::channel();
        let (started, starts) = std::sync::mpsc::channel();
        let devices = BackgroundFake {
            inner: controller().devices,
            gates: [first_gate, second_gate].into(),
            started,
            worker: None,
        };
        let stop = stopping.clone();
        let runner = std::thread::spawn(move || run_with_devices(receiver, stop, devices));
        let request = |command| {
            let (reply, response) = std::sync::mpsc::channel();
            sender
                .send(Request {
                    command,
                    reload: None,
                    received_at: Instant::now(),
                    reply,
                })
                .unwrap();
            response
        };
        let response = |reply: std::sync::mpsc::Receiver<anyhow::Result<BacklightInfo>>| {
            reply.recv_timeout(Duration::from_secs(2)).unwrap().unwrap()
        };
        // Ensure startup has completed before sending a new hotplug event.
        response(request(BacklightCommand::GetInfo));
        response(request(BacklightCommand::NotifyHotplug));
        starts.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            response(request(BacklightCommand::SetBrightness(70))).brightness_percent,
            70
        );
        let refresh = request(BacklightCommand::Refresh);
        assert_eq!(
            response(request(BacklightCommand::IncreaseBrightness(5))).brightness_percent,
            75
        );
        assert!(matches!(
            refresh.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        first.send(()).unwrap();
        // A request received during enumeration needs a scan started afterwards.
        starts.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            response(request(BacklightCommand::SetBrightness(80))).brightness_percent,
            80
        );
        assert!(matches!(
            refresh.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        second.send(()).unwrap();
        // Completion must wake the controller even without another client request.
        let refreshed = response(refresh);
        assert_eq!(
            (refreshed.brightness_percent, refreshed.mode),
            (80, BacklightMode::Manual)
        );
        stopping.store(true, std::sync::atomic::Ordering::Release);
        drop(sender);
        runner.join().unwrap().unwrap();
    }

    #[test]
    fn pending_hotplug_and_resume_survive_background_discovery_and_manual_expiry() {
        let (release, gate) = std::sync::mpsc::channel();
        let (started, _starts) = std::sync::mpsc::channel();
        let mut c = Controller::new(BackgroundFake {
            inner: controller().devices,
            gates: [gate].into(),
            started,
            worker: None,
        });
        c.command(BacklightCommand::SetBrightness(75)).unwrap();
        c.start_background_refresh("scheduled").unwrap();
        let event = c.last_refresh_started.unwrap() + Duration::from_millis(1);
        c.notify_hotplug(event, Instant::now());
        c.pending_refresh.as_mut().unwrap().deadline = Instant::now();
        c.deadline = Instant::now();
        c.service_deadlines_after_resume(true);
        assert_eq!(c.mode, BacklightMode::Auto);
        assert_eq!(c.devices.inner.manual_target, None);
        assert!(c.resume_pending);
        assert!(c.pending_refresh.is_some());
        assert!(c.next_deadline() > Instant::now());
        release.send(()).unwrap();
        c.finish_background_refresh().unwrap();
        assert_eq!(c.devices.inner.brightness, 50);
        c.service_deadlines_after_resume(false);
        assert_eq!(c.active_refresh.as_ref().unwrap().0, "resume");
        assert!(!c.resume_pending);
        assert!(c.pending_refresh.is_none());
        c.finish_background_refresh().unwrap();
    }

    #[test]
    fn reload_preserves_manual_start_and_supports_forever() {
        let mut c = controller();
        c.command(BacklightCommand::SetBrightness(85)).unwrap();
        let started = c.manual_started.unwrap();
        let deadline = c.deadline;
        c.reload(std::sync::Arc::new(
            crate::config::Config::parse("[manual]\noverride_duration = 'forever'").unwrap(),
        ));
        c.deadline = Instant::now();
        c.service_deadlines();
        assert_eq!(c.mode, BacklightMode::Manual);
        assert_eq!(c.next_deadline(), c.refresh_deadline);
        c.reload(std::sync::Arc::new(crate::config::Config::default()));
        assert_eq!(c.manual_started, Some(started));
        assert_eq!(c.deadline, deadline);
        assert_eq!(c.devices.hardware_brightness, 85);
    }

    #[test]
    fn recovered_monitor_inherits_absolute_and_relative_manual_targets() {
        let mut c = controller();
        for (command, expected) in [
            (BacklightCommand::SetBrightness(95), 95),
            (BacklightCommand::DecreaseBrightness(5), 90),
            (BacklightCommand::IncreaseBrightness(5), 95),
        ] {
            c.command(command).unwrap();
            let deadline = c.deadline;
            c.devices.hardware_brightness = 100;
            c.devices.discovered_new = true;
            let info = c.command(BacklightCommand::Refresh).unwrap();
            assert_eq!(
                (info.brightness_percent, info.mode),
                (expected, BacklightMode::Manual)
            );
            assert_eq!(c.deadline, deadline);
        }
    }

    #[test]
    fn failed_manual_request_retains_intent_for_recovery() {
        let mut c = controller();
        c.devices.fail = true;
        assert!(c.command(BacklightCommand::SetBrightness(95)).is_err());
        assert_eq!(c.devices.manual_target, Some(95));
        c.devices.fail = false;
        c.devices.discovered_new = true;
        c.devices.hardware_brightness = 100;
        c.command(BacklightCommand::Refresh).unwrap();
        assert_eq!(c.devices.hardware_brightness, 95);
    }

    #[test]
    fn resume_reapplies_manual_target_without_extending_override() {
        let mut c = controller();
        c.command(BacklightCommand::SetBrightness(95)).unwrap();
        let deadline = c.deadline;
        c.devices.hardware_brightness = 100;
        c.service_deadlines_after_resume(true);
        assert_eq!(c.devices.hardware_brightness, 95);
        assert_eq!(c.deadline, deadline);
        c.deadline = Instant::now();
        c.devices.hardware_brightness = 100;
        c.service_deadlines_after_resume(true);
        assert_eq!(c.devices.hardware_brightness, 50);
        assert_eq!(c.devices.manual_target, None);
        assert_eq!(c.mode, BacklightMode::Auto);
    }

    #[test]
    fn auto_mode_clears_manual_target_before_discovery() {
        let mut c = controller();
        c.command(BacklightCommand::SetBrightness(95)).unwrap();
        c.command(BacklightCommand::SetMode(BacklightMode::Auto))
            .unwrap();
        assert_eq!(c.devices.manual_target, None);
        c.devices.discovered_new = true;
        c.devices.hardware_brightness = 100;
        c.command(BacklightCommand::Refresh).unwrap();
        assert_eq!(c.devices.hardware_brightness, 50);
    }

    #[test]
    fn partial_refresh_reports_failure_but_adjusts_valid_monitors() {
        let mut c = controller();
        c.devices.partial_refresh = true;
        c.devices.hardware_brightness = 10;
        let received_at = Instant::now();
        assert!(c.refresh("test").is_err());
        assert_eq!(c.devices.hardware_brightness, 50);
        assert!(c
            .last_refresh_error
            .as_ref()
            .unwrap()
            .contains("incomplete"));
        assert!(c
            .command_at(BacklightCommand::Refresh, received_at)
            .is_err());
        assert_eq!(c.devices.refreshes, 1);
    }

    #[test]
    fn hotplug_burst_waits_for_settling_and_cannot_starve_discovery() {
        let mut c = controller();
        let now = Instant::now();
        for millis in [0, 100, 200, 400, 600, 800, 1000] {
            let event = now + Duration::from_millis(millis);
            c.notify_hotplug(event, event);
            assert_eq!(
                c.next_deadline(),
                (event + HOTPLUG_SETTLE).min(now + HOTPLUG_MAX_DELAY)
            );
        }
        assert_eq!(c.devices.refreshes, 0);
        c.pending_refresh.as_mut().unwrap().deadline = Instant::now();
        c.service_deadlines();
        assert_eq!(c.devices.refreshes, 1);
        assert!(c.pending_refresh.is_none());
    }

    #[test]
    fn events_during_discovery_schedule_a_settled_follow_up() {
        let mut c = controller();
        let before = Instant::now();
        c.refresh("resume").unwrap();
        c.notify_hotplug(before, Instant::now());
        assert!(c.pending_refresh.is_none());
        // These represent notifications queued after discovery had begun.
        let during = c.last_refresh_started.unwrap() + Duration::from_nanos(1);
        for _ in 0..4 {
            c.command_at(BacklightCommand::NotifyHotplug, during)
                .unwrap();
        }
        assert_eq!(c.devices.refreshes, 1);
        assert!(c.pending_refresh.is_some());
        c.pending_refresh.as_mut().unwrap().deadline = Instant::now();
        c.service_deadlines();
        assert_eq!(c.devices.refreshes, 2);
        // A later event is still honored, even during the follow-up scan.
        let later = c.last_refresh_started.unwrap() + Duration::from_nanos(1);
        c.command_at(BacklightCommand::NotifyHotplug, later)
            .unwrap();
        assert!(c.pending_refresh.is_some());
    }

    #[test]
    fn explicit_refresh_covers_pending_hotplug_and_still_waits() {
        let mut c = controller();
        c.command(BacklightCommand::SetBrightness(85)).unwrap();
        let deadline = c.deadline;
        c.command(BacklightCommand::NotifyHotplug).unwrap();
        c.devices.hardware_brightness = 70;
        let info = c.command(BacklightCommand::Refresh).unwrap();
        assert_eq!(info.brightness_percent, 70);
        assert_eq!(c.devices.refreshes, 1);
        assert!(c.pending_refresh.is_none());
        assert_eq!(c.deadline, deadline);
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
