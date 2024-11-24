use core::f64;
use std::{
    env::{self, VarError},
    fs,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context};
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

impl LastKnownLocation {
    const fn default() -> Self {
        Self {
            latitude: 0.,
            longitude: 0.,
            timestamp: 0.,
        }
    }
}

static LAST_LOCATION_REFRESH: Mutex<LastKnownLocation> = Mutex::new(LastKnownLocation::default());
const IP_BASED_LOCATION_REFRESH_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);
const LOCATION_CACHE_FILE: &str = "/var/cache/backlightd/last_known_location.toml";

fn find_location_from_config() -> anyhow::Result<(f64, f64)> {
    let location_str = match env::var("BACKLIGHTD_LOCATION") {
        Ok(loc) => loc,
        Err(err) => match err {
            VarError::NotPresent => return Err(anyhow::Error::new(VarError::NotPresent)),
            VarError::NotUnicode(str) => {
                bail!("Invalid environment variable value: {str:?}");
            }
        },
    };

    let (lat, long) = location_str.split_once(',').with_context(|| {
        "Invalid BACKLIGHTD_LOCATION, expected a comma between latitude and longitude"
    })?;

    let lat = lat
        .trim()
        .parse()
        .with_context(|| "Unable to parse latitude to float")?;

    let long = long
        .trim()
        .parse()
        .with_context(|| "Unable to parse longitude to float")?;

    Ok((lat, long))
}

fn find_ip_location() -> anyhow::Result<(f64, f64)> {
    let mut last_known_location = LAST_LOCATION_REFRESH
        .lock()
        .expect("Unable to acquire LAST_LOCATION_REFRESH mutex");

    if last_known_location.timestamp == 0. {
        // On first run (timestamp == 0), try to read from the cache file
        match fs::read_to_string(LOCATION_CACHE_FILE) {
            Ok(cache_str) => match toml::from_str::<LastKnownLocation>(&cache_str) {
                Ok(cache) => {
                    last_known_location.latitude = cache.latitude;
                    last_known_location.longitude = cache.longitude;
                    last_known_location.timestamp = cache.timestamp;
                }
                Err(err) => warn!("Invalid cache file detected: {err}"),
            },
            Err(err) => match err.kind() {
                std::io::ErrorKind::NotFound => {}
                _ => {
                    error!("Unable to read from location cache file: {err}");
                }
            },
        };
    }

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();

    if now - last_known_location.timestamp > IP_BASED_LOCATION_REFRESH_INTERVAL.as_secs_f64() {
        debug!("Trying to get location from public API...");
        let resp: IpApiResponse =
            ureq::get("http://ip-api.com/json/?fields=status,message,country,city,lat,lon")
                .call()?
                .into_json()?;

        if resp.status != "success" {
            bail!("Unable to find location by IP: {:?}", resp.message);
        }

        info!(
            "Found your location using your IP: {}/{} [{}, {}]",
            resp.country, resp.city, resp.lat, resp.lon
        );

        last_known_location.timestamp = now;
        last_known_location.latitude = resp.lat;
        last_known_location.longitude = resp.lon;

        let last_known_location_as_toml = toml::to_string(&*last_known_location)
            .expect("unable to convert LastKnownLocation to toml");
        let cache_location = PathBuf::from(LOCATION_CACHE_FILE);
        fs::create_dir_all(cache_location.parent().unwrap())?;
        if let Err(err) = fs::write(LOCATION_CACHE_FILE, last_known_location_as_toml) {
            error!("Failed to write to location cache: {err}");
        }
    }

    Ok((last_known_location.latitude, last_known_location.longitude))
}

pub(crate) fn find_location() -> anyhow::Result<Option<(f64, f64)>> {
    match find_location_from_config() {
        Ok(location) => {
            info!("Using location from configuration: {location:?}");
            return Ok(Some(location));
        }
        Err(err) => match err.downcast_ref::<VarError>() {
            Some(VarError::NotPresent) => {}
            _ => bail!("{err}"),
        },
    }

    let allow_api_call = match env::var("BACKLIGHTD_ENABLE_LOCATION_API") {
        Ok(value) => ["1", "y", "yes"].contains(&value.as_ref()),
        Err(err) => match err {
            VarError::NotPresent => true,
            VarError::NotUnicode(str) => {
                bail!("Invalid environment variable value: {str:?}");
            }
        },
    };

    Ok(if allow_api_call {
        match find_ip_location() {
            Ok(location) => Some(location),
            Err(err) => {
                warn!("Failed to get location using public API: {err}");
                info!("Fallback to clock based brightness adjustement");
                None
            }
        }
    } else {
        warn!("Unable to find your location, you might want to configure backlightd or to allow the usage of the public API");
        info!("Fallback to clock based brightness adjustement");
        None
    })
}
