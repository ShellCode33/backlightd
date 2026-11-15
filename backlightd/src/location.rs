use core::f64;
use std::{
    fs,
    path::Path,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context};
use boot_time::Instant;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct IpApiResponse {
    status: String,
    message: Option<String>,
    country: String,
    city: String,
    lat: f64,
    lon: f64,
}

#[derive(Serialize, Deserialize)]
struct LastKnownLocation {
    latitude: f64,
    longitude: f64,
    timestamp: f64,
}

#[derive(Default)]
struct LocationState {
    cached: Option<LastKnownLocation>,
    cache_loaded: bool,
    last_attempt: Option<Instant>,
}

static LAST_LOCATION_REFRESH: Mutex<LocationState> = Mutex::new(LocationState {
    cached: None,
    cache_loaded: false,
    last_attempt: None,
});
const IP_BASED_LOCATION_REFRESH_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);
const LOCATION_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const LOCATION_RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);
const LOCATION_CACHE_FILE: &str = "/var/cache/backlightd/last_known_location.toml";

fn validate_coordinates(latitude: f64, longitude: f64) -> anyhow::Result<(f64, f64)> {
    anyhow::ensure!(
        latitude.is_finite()
            && longitude.is_finite()
            && (-90. ..=90.).contains(&latitude)
            && (-180. ..=180.).contains(&longitude),
        "Invalid location coordinates"
    );
    Ok((latitude, longitude))
}

impl LocationState {
    fn locate(
        &mut self,
        now: f64,
        cache_path: &Path,
        fetch: impl FnOnce() -> anyhow::Result<(f64, f64)>,
    ) -> anyhow::Result<(f64, f64)> {
        if !self.cache_loaded {
            self.cache_loaded = true;
            let cached = (|| -> anyhow::Result<LastKnownLocation> {
                let cached: LastKnownLocation = toml::from_str(&fs::read_to_string(cache_path)?)?;
                validate_coordinates(cached.latitude, cached.longitude)?;
                anyhow::ensure!(
                    cached.timestamp.is_finite() && cached.timestamp > 0.,
                    "Invalid cache timestamp"
                );
                Ok(cached)
            })();
            match cached {
                Ok(cached) => self.cached = Some(cached),
                Err(err)
                    if err
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) => {}
                Err(err) => warn!("Unable to load location cache: {err}"),
            }
        }

        let fresh = self.cached.as_ref().is_some_and(|cache| {
            // A future timestamp must not suppress lookups after a clock correction.
            cache.timestamp <= now
                && now - cache.timestamp < IP_BASED_LOCATION_REFRESH_INTERVAL.as_secs_f64()
        });
        let retry_due = self
            .last_attempt
            .is_none_or(|last| last.elapsed() >= LOCATION_RETRY_INTERVAL);
        if !fresh && retry_due {
            self.last_attempt = Some(Instant::now());
            match fetch().and_then(|(lat, lon)| validate_coordinates(lat, lon)) {
                Ok((latitude, longitude)) => {
                    let cache = LastKnownLocation {
                        latitude,
                        longitude,
                        timestamp: now,
                    };
                    // Persistence is best effort: a successful lookup is usable
                    // even when the cache directory cannot be created or written.
                    let persist = (|| -> anyhow::Result<()> {
                        if let Some(parent) = cache_path.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        fs::write(cache_path, toml::to_string(&cache)?)?;
                        Ok(())
                    })();
                    if let Err(err) = persist {
                        error!("Failed to persist location cache: {err}");
                    }
                    self.cached = Some(cache);
                }
                Err(err) => {
                    if self.cached.is_none() {
                        return Err(err);
                    }
                    warn!("Location lookup failed; using last known location: {err}");
                }
            }
        }
        self.cached
            .as_ref()
            .map(|cache| (cache.latitude, cache.longitude))
            .context("Location unavailable; waiting before retrying lookup")
    }
}

fn fetch_ip_location() -> anyhow::Result<(f64, f64)> {
    debug!("Trying to get location from public API...");
    // The global budget covers DNS, connection setup, headers, and body reads.
    let resp: IpApiResponse =
        ureq::get("http://ip-api.com/json/?fields=status,message,country,city,lat,lon")
            .config()
            .timeout_global(Some(LOCATION_REQUEST_TIMEOUT))
            .build()
            .call()?
            .body_mut()
            .read_json()?;
    if resp.status != "success" {
        bail!("Unable to find location by IP: {:?}", resp.message);
    }
    validate_coordinates(resp.lat, resp.lon)?;
    info!(
        "Found your location using your IP: {}/{} [{}, {}]",
        resp.country, resp.city, resp.lat, resp.lon
    );
    Ok((resp.lat, resp.lon))
}

fn find_ip_location() -> anyhow::Result<(f64, f64)> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
    LAST_LOCATION_REFRESH
        .lock()
        .expect("Unable to acquire location mutex")
        .locate(now, Path::new(LOCATION_CACHE_FILE), fetch_ip_location)
}

pub(crate) fn find_location() -> anyhow::Result<Option<(f64, f64)>> {
    let settings = crate::config::current();
    match settings.location.source {
        crate::config::LocationSource::Manual => Ok(Some((
            settings.location.latitude.unwrap(),
            settings.location.longitude.unwrap(),
        ))),
        crate::config::LocationSource::None => Ok(None),
        crate::config::LocationSource::Ip => match find_ip_location() {
            Ok(location) => Ok(Some(location)),
            Err(err) => {
                warn!("IP location unavailable; using clock schedule: {err}");
                Ok(None)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "backlightd-location-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn cache(&self, latitude: f64, timestamp: f64) -> std::path::PathBuf {
            let path = self.0.join("location.toml");
            fs::write(
                &path,
                toml::to_string(&LastKnownLocation {
                    latitude,
                    longitude: 2.,
                    timestamp,
                })
                .unwrap(),
            )
            .unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn offline_lookup_keeps_stale_cache_and_backs_off() {
        let fixture = Fixture::new();
        let path = fixture.cache(48., 1.);
        let mut state = LocationState::default();
        assert_eq!(
            state.locate(86400., &path, || bail!("offline")).unwrap(),
            (48., 2.)
        );
        assert_eq!(state.cached.as_ref().unwrap().timestamp, 1.);
        assert_eq!(
            state
                .locate(86401., &path, || panic!("lookup retried too soon"))
                .unwrap(),
            (48., 2.)
        );
    }

    #[test]
    fn offline_without_cache_returns_error_and_backs_off() {
        let fixture = Fixture::new();
        let path = fixture.0.join("missing.toml");
        let mut state = LocationState::default();
        assert!(state.locate(86400., &path, || bail!("offline")).is_err());
        assert!(state
            .locate(86401., &path, || panic!("lookup retried too soon"))
            .is_err());
    }

    #[test]
    fn fresh_cache_skips_lookup_but_future_timestamp_does_not() {
        let fixture = Fixture::new();
        let path = fixture.cache(48., 86400.);
        assert_eq!(
            LocationState::default()
                .locate(86401., &path, || panic!("fresh cache ignored"))
                .unwrap(),
            (48., 2.)
        );
        let mut state = LocationState::default();
        assert_eq!(
            state.locate(80000., &path, || Ok((50., 3.))).unwrap(),
            (50., 3.)
        );
        let saved: LastKnownLocation = toml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            (saved.latitude, saved.longitude, saved.timestamp),
            (50., 3., 80000.)
        );
    }

    #[test]
    fn invalid_cache_never_becomes_an_offline_location() {
        let fixture = Fixture::new();
        for (latitude, timestamp) in [(91., 1.), (f64::NAN, 1.), (48., f64::NAN), (48., 0.)] {
            let path = fixture.cache(latitude, timestamp);
            assert!(LocationState::default()
                .locate(86400., &path, || bail!("offline"))
                .is_err());
        }
        let path = fixture.0.join("broken.toml");
        fs::write(&path, "invalid TOML").unwrap();
        assert!(LocationState::default()
            .locate(86400., &path, || bail!("offline"))
            .is_err());
    }

    #[test]
    fn invalid_lookup_does_not_replace_valid_stale_location() {
        let fixture = Fixture::new();
        let path = fixture.cache(48., 1.);
        let mut state = LocationState::default();
        assert_eq!(
            state.locate(86400., &path, || Ok((f64::NAN, 2.))).unwrap(),
            (48., 2.)
        );
        assert_eq!(state.cached.as_ref().unwrap().timestamp, 1.);
    }

    #[test]
    fn lookup_success_survives_directory_and_file_write_failures() {
        let fixture = Fixture::new();
        let blocker = fixture.0.join("blocker");
        fs::write(&blocker, "not a directory").unwrap();
        for path in [blocker.join("location.toml"), fixture.0.clone()] {
            let mut state = LocationState::default();
            assert_eq!(
                state.locate(86400., &path, || Ok((48., 2.))).unwrap(),
                (48., 2.)
            );
            assert_eq!(
                state
                    .locate(86401., &path, || panic!("in-memory success lost"))
                    .unwrap(),
                (48., 2.)
            );
        }
    }

    #[test]
    fn location_cache_remains_compatible_with_previous_toml_format() {
        let previous = "latitude = 48.8566\nlongitude = 2.3522\ntimestamp = 1730000000.0\n";
        let cache: LastKnownLocation = toml::from_str(previous).unwrap();
        assert_eq!(cache.latitude, 48.8566);
        assert_eq!(cache.longitude, 2.3522);
        assert_eq!(cache.timestamp, 1730000000.0);
        let restored: LastKnownLocation =
            toml::from_str(&toml::to_string(&cache).unwrap()).unwrap();
        assert_eq!(restored.latitude, cache.latitude);
        assert_eq!(restored.longitude, cache.longitude);
        assert_eq!(restored.timestamp, cache.timestamp);
    }
}
