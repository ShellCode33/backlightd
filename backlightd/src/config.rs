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
    pub location: Location,
    pub manual: Manual,
    pub logging: Logging,
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
            source: LocationSource::Ip,
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
