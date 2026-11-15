//! Validated, immutable configuration snapshots, replaced by the controller.
use anyhow::{bail, Context};
use serde::Deserialize;
use std::{
    fs,
    path::Path,
    sync::{Arc, OnceLock, RwLock},
    time::Duration,
};

pub(crate) const DEFAULT_PATH: &str = "/etc/backlightd/config.toml";
static CURRENT: OnceLock<RwLock<Arc<Config>>> = OnceLock::new();

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Config {
    pub brightness: Calibration,
    pub devices: Vec<Device>,
    pub automatic: Automatic,
    pub location: Location,
    pub manual: Manual,
    pub logging: Logging,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Calibration {
    pub minimum_percent: u8,
    pub maximum_percent: u8,
    pub exponent: f64,
}
impl Default for Calibration {
    fn default() -> Self {
        Self {
            minimum_percent: 0,
            maximum_percent: 100,
            exponent: 2.0,
        }
    }
}
impl Calibration {
    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.minimum_percent < self.maximum_percent && self.maximum_percent <= 100,
            "Calibration requires 0 <= minimum_percent < maximum_percent <= 100"
        );
        anyhow::ensure!(
            self.exponent.is_finite() && (0.1..=10.0).contains(&self.exponent),
            "Calibration exponent must be finite and between 0.1 and 10"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Device {
    pub id: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    pub minimum_percent: Option<u8>,
    pub maximum_percent: Option<u8>,
    pub exponent: Option<f64>,
}
fn enabled_by_default() -> bool {
    true
}
impl Device {
    pub fn calibration(&self, base: &Calibration) -> Calibration {
        Calibration {
            minimum_percent: self.minimum_percent.unwrap_or(base.minimum_percent),
            maximum_percent: self.maximum_percent.unwrap_or(base.maximum_percent),
            exponent: self.exponent.unwrap_or(base.exponent),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Schedule {
    Solar,
    Clock,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Automatic {
    pub schedule: Schedule,
    pub minimum_percent: u8,
    pub maximum_percent: u8,
    pub transition_duration: String,
    pub morning: String,
    pub evening: String,
}
impl Default for Automatic {
    fn default() -> Self {
        Self {
            schedule: Schedule::Solar,
            minimum_percent: 5,
            maximum_percent: 100,
            transition_duration: "4h".into(),
            morning: "06:00".into(),
            evening: "18:00".into(),
        }
    }
}
pub(crate) fn clock(value: &str) -> anyhow::Result<chrono::NaiveTime> {
    anyhow::ensure!(
        value.len() == 5 && value.as_bytes()[2] == b':',
        "Clock time must be HH:MM"
    );
    chrono::NaiveTime::parse_from_str(value, "%H:%M").context("Invalid clock time")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocationSource {
    None,
    Manual,
    Ip,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Location {
    pub source: LocationSource,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}
impl Default for Location {
    fn default() -> Self {
        Self {
            source: LocationSource::None,
            latitude: None,
            longitude: None,
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Manual {
    pub override_duration: String,
}
impl Default for Manual {
    fn default() -> Self {
        Self {
            override_duration: "12h".into(),
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Logging {
    pub level: String,
}
impl Default for Logging {
    fn default() -> Self {
        Self {
            level: "info".into(),
        }
    }
}

pub(crate) fn duration(value: &str) -> anyhow::Result<Duration> {
    let end = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let number: u64 = value[..end]
        .parse()
        .context("Duration must have a positive number and s, m, h, or d suffix")?;
    let multiplier = match &value[end..] {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => bail!("Unknown duration unit in {value:?}; use s, m, h, or d"),
    };
    let seconds = number
        .checked_mul(multiplier)
        .context("Duration overflow")?;
    anyhow::ensure!(
        (1..=365 * 86400).contains(&seconds),
        "Duration must be between 1s and 365d"
    );
    Ok(Duration::from_secs(seconds))
}
impl Config {
    pub fn manual_duration(&self) -> Option<Duration> {
        if self.manual.override_duration == "forever" {
            None
        } else {
            Some(duration(&self.manual.override_duration).expect("validated duration"))
        }
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        use chrono::Timelike;
        self.brightness.validate().context("brightness")?;
        let mut ids = std::collections::HashSet::new();
        for device in &self.devices {
            anyhow::ensure!(
                !device.id.is_empty() && ids.insert(&device.id),
                "Device IDs must be nonempty and unique"
            );
            device
                .calibration(&self.brightness)
                .validate()
                .with_context(|| format!("device {}", device.id))?;
        }
        let auto = &self.automatic;
        anyhow::ensure!(
            auto.minimum_percent >= 1
                && auto.minimum_percent <= auto.maximum_percent
                && auto.maximum_percent <= 100,
            "automatic brightness must satisfy 1 <= minimum_percent <= maximum_percent <= 100"
        );
        let morning = clock(&auto.morning)
            .context("automatic.morning")?
            .num_seconds_from_midnight();
        let evening = clock(&auto.evening)
            .context("automatic.evening")?
            .num_seconds_from_midnight();
        let gap = (evening + 86400 - morning) % 86400;
        let transition =
            duration(&auto.transition_duration).context("automatic.transition_duration")?;
        anyhow::ensure!(gap > 0 && transition.as_secs() <= u64::from(gap.min(86400 - gap)), "Clock schedule transitions overlap; shorten automatic.transition_duration or separate morning/evening");

        if self.manual.override_duration != "forever" {
            duration(&self.manual.override_duration).context("manual.override_duration")?;
        }
        self.logging
            .level
            .parse::<log::LevelFilter>()
            .context("Invalid logging.level")?;
        match (self.location.latitude, self.location.longitude) {
            (Some(lat), Some(lon)) => anyhow::ensure!(lat.is_finite() && lon.is_finite() && (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon), "Invalid location coordinates"),
            (None, None) if self.location.source != LocationSource::Manual => {},
            _ => bail!("location.latitude and location.longitude must both be provided for manual location"),
        }
        Ok(())
    }
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let config: Self = toml::from_str(text).context("Invalid TOML configuration")?;
        config.validate()?;
        Ok(config)
    }
}

pub(crate) fn load(path: &Path, required: bool) -> anyhow::Result<Arc<Config>> {
    let mut config = match fs::read_to_string(path) {
        Ok(text) => Config::parse(&text).with_context(|| path.display().to_string())?,
        Err(err) if !required && err.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(err) => return Err(err).with_context(|| format!("Unable to read {}", path.display())),
    };
    // Legacy environment settings override the corresponding file settings.
    if let Ok(value) = std::env::var("BACKLIGHTD_ENABLE_LOCATION_API") {
        if config.location.source != LocationSource::Manual {
            config.location.source = if ["1", "y", "yes"].contains(&value.as_str()) {
                LocationSource::Ip
            } else {
                LocationSource::None
            };
        }
    }
    if let Ok(value) = std::env::var("BACKLIGHTD_LOCATION") {
        let (lat, lon) = value
            .split_once(',')
            .context("BACKLIGHTD_LOCATION requires latitude,longitude")?;
        config.location = Location {
            source: LocationSource::Manual,
            latitude: Some(lat.trim().parse()?),
            longitude: Some(lon.trim().parse()?),
        };
    }
    config.validate()?;
    Ok(Arc::new(config))
}
pub(crate) fn current() -> Arc<Config> {
    CURRENT
        .get_or_init(|| RwLock::new(Arc::new(Config::default())))
        .read()
        .unwrap()
        .clone()
}
pub(crate) fn publish(config: Arc<Config>) {
    if std::env::var_os("BACKLIGHTD_LOG_LEVEL").is_none() {
        log::set_max_level(config.logging.level.parse().expect("validated log level"));
    }
    *CURRENT
        .get_or_init(|| RwLock::new(Arc::new(Config::default())))
        .write()
        .unwrap() = config;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_calibration_and_duplicate_device_rules() {
        for text in [
            "[brightness]\nexponent = nan",
            "[brightness]\nexponent = 0",
            "[brightness]\nminimum_percent = 90\nmaximum_percent = 80",
            "[[devices]]\nid = ''",
            "[[devices]]\nid = 'a'\n[[devices]]\nid = 'a'",
            "[[devices]]\nid = 'a'\nexponent = inf",
        ] {
            assert!(Config::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn schedule_validation_and_safe_defaults() {
        let defaults = Config::default();
        assert_eq!(defaults.location.source, LocationSource::None);
        assert_eq!(defaults.automatic.minimum_percent, 5);
        assert_eq!(defaults.brightness.minimum_percent, 0);
        assert_eq!(defaults.brightness.exponent, 2.0);
        assert!(Config::parse("[automatic]\nmorning = '22:00'\nevening = '10:00'").is_ok());
        for text in [
            "[automatic]\nminimum_percent = 0",
            "[automatic]\nmaximum_percent = 101",
            "[automatic]\nmorning = '6:00'",
            "[automatic]\nevening = '06:00'",
            "[automatic]\ntransition_duration = '13h'",
        ] {
            assert!(Config::parse(text).is_err(), "{text}");
        }
        let example = Config::parse(include_str!("../../etc/backlightd/config.toml")).unwrap();
        assert_eq!(
            example.automatic.minimum_percent,
            defaults.automatic.minimum_percent
        );
        assert_eq!(example.brightness.exponent, defaults.brightness.exponent);
    }

    #[test]
    fn defaults_partial_files_and_validation() {
        assert_eq!(
            Config::parse("").unwrap().manual_duration(),
            Some(Duration::from_secs(43200))
        );
        assert!(Config::parse("[manual]\noverride_duration = 'forever'")
            .unwrap()
            .manual_duration()
            .is_none());
        for text in [
            "typo = 1",
            "[manual]\nunknown = 2",
            "[manual]\noverride_duration = '0h'",
            "[logging]\nlevel = 'typo'",
            "[location]\nsource = 'manual'",
            "[location]\nsource = 'manual'\nlatitude = 91\nlongitude = 0",
        ] {
            assert!(Config::parse(text).is_err(), "{text}");
        }
        assert!(duration("18446744073709551615d").is_err());
    }
}
