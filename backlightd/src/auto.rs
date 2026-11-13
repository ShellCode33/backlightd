use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Local, NaiveTime};
use sunrise::{Coordinates, SolarDay, SolarEvent};

use crate::location::find_location;

const BRIGHTNESS_TRANSITION_DURATION: Duration = Duration::from_secs(4 * 60 * 60);
const FALLBACK_BRIGHTNESS_UP_BEGIN: Option<NaiveTime> = NaiveTime::from_hms_opt(6, 0, 0);
const FALLBACK_BRIGHTNESS_DOWN_BEGIN: Option<NaiveTime> = NaiveTime::from_hms_opt(18, 0, 0);

// Calculate both the target and the next time it can change. No hardware polling.
pub(crate) fn next_adjustment() -> anyhow::Result<(u8, Duration)> {
    match find_location()? {
        Some((latitude, longitude)) => {
            get_brightness_based_on_location(latitude, longitude, Local::now())
        }
        None => Ok(get_brightness_based_on_time(Local::now())),
    }
}

fn get_brightness_based_on_location(
    latitude: f64,
    longitude: f64,
    now: DateTime<Local>,
) -> anyhow::Result<(u8, Duration)> {
    let coordinates =
        Coordinates::new(latitude, longitude).context("Invalid location coordinates")?;
    let day = SolarDay::new(coordinates, now.date_naive());
    let (Some(sunrise), Some(sunset)) = (
        day.event_time(SolarEvent::Sunrise),
        day.event_time(SolarEvent::Sunset),
    ) else {
        // Sunrise/sunset do not occur on every date at polar latitudes.
        return Ok(get_brightness_based_on_time(now));
    };
    let sunrise_datetime = sunrise.with_timezone(&Local);
    let sunset_datetime = sunset.with_timezone(&Local);

    Ok(scheduled_brightness(
        now,
        sunrise_datetime,
        sunrise_datetime + BRIGHTNESS_TRANSITION_DURATION,
        sunset_datetime,
        sunset_datetime + BRIGHTNESS_TRANSITION_DURATION,
    ))
}

fn get_brightness_based_on_time(now: DateTime<Local>) -> (u8, Duration) {
    scheduled_brightness(
        now,
        now.with_time(FALLBACK_BRIGHTNESS_UP_BEGIN.unwrap())
            .unwrap(),
        now.with_time(FALLBACK_BRIGHTNESS_UP_BEGIN.unwrap())
            .unwrap()
            + BRIGHTNESS_TRANSITION_DURATION,
        now.with_time(FALLBACK_BRIGHTNESS_DOWN_BEGIN.unwrap())
            .unwrap(),
        now.with_time(FALLBACK_BRIGHTNESS_DOWN_BEGIN.unwrap())
            .unwrap()
            + BRIGHTNESS_TRANSITION_DURATION,
    )
}

fn scheduled_brightness(
    now: DateTime<Local>,
    up_begin: DateTime<Local>,
    up_end: DateTime<Local>,
    down_begin: DateTime<Local>,
    down_end: DateTime<Local>,
) -> (u8, Duration) {
    let brightness = compute_brightness_percentage(now, up_begin, up_end, down_begin, down_end);
    // Recalculate the solar schedule at the next local date boundary as well.
    let midnight = now
        .date_naive()
        .succ_opt()
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(Local)
        .earliest()
        .unwrap_or(now + chrono::Duration::hours(1));
    let next = if now < up_begin {
        up_begin
            + chrono::Duration::seconds(
                ((up_end - up_begin).num_seconds() as f64 / 198.).ceil() as i64
            )
    } else if now < up_end {
        up_begin
            + chrono::Duration::seconds(
                (((brightness as f64 - 0.5) / 99.) * (up_end - up_begin).num_seconds() as f64)
                    .ceil() as i64,
            )
    } else if now < down_begin {
        down_begin
            + chrono::Duration::seconds(
                ((down_end - down_begin).num_seconds() as f64 / 198.).floor() as i64 + 1,
            )
    } else if now < down_end && brightness > 1 {
        down_begin
            + chrono::Duration::seconds(
                (((100.5 - brightness as f64) / 99.) * (down_end - down_begin).num_seconds() as f64)
                    .floor() as i64
                    + 1,
            )
    } else {
        midnight
    };
    (
        brightness,
        (next.min(midnight) - now)
            .to_std()
            .unwrap_or(Duration::from_secs(1))
            .max(Duration::from_secs(1)),
    )
}

fn compute_brightness_percentage(
    now: DateTime<Local>,
    brightness_up_begin: DateTime<Local>,
    brightness_up_end: DateTime<Local>,
    brightness_down_begin: DateTime<Local>,
    brightness_down_end: DateTime<Local>,
) -> u8 {
    assert!(brightness_up_begin < brightness_up_end);
    assert!(brightness_up_end < brightness_down_begin);
    assert!(brightness_down_begin < brightness_down_end);

    if now < brightness_up_begin || now > brightness_down_end {
        1
    } else if now > brightness_up_end && now < brightness_down_begin {
        100
    } else if now >= brightness_up_begin && now <= brightness_up_end {
        let duration = (brightness_up_end - brightness_up_begin).num_seconds() as f64;
        let elapsed = (now - brightness_up_begin).num_seconds() as f64;
        ((elapsed / duration * 99.) + 1.).round() as u8
    } else if now >= brightness_down_begin && now <= brightness_down_end {
        let duration = (brightness_down_end - brightness_down_begin).num_seconds() as f64;
        let elapsed = (now - brightness_down_begin).num_seconds() as f64;
        ((1. - elapsed / duration) * 99. + 1.).round() as u8
    } else {
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    use chrono::Timelike;

    use super::*;

    #[test]
    fn upstream_solar_events_produce_a_schedule() {
        let now = DateTime::parse_from_rfc3339("2026-03-20T12:00:00+01:00")
            .unwrap()
            .with_timezone(&Local);
        let (brightness, delay) = get_brightness_based_on_location(48.8566, 2.3522, now).unwrap();
        assert!((1..=100).contains(&brightness));
        assert!(delay > Duration::ZERO);
    }

    #[test]
    fn missing_polar_events_use_clock_schedule() {
        for date in ["2026-06-21T12:00:00+02:00", "2026-12-21T12:00:00+01:00"] {
            let now = DateTime::parse_from_rfc3339(date)
                .unwrap()
                .with_timezone(&Local);
            assert_eq!(
                get_brightness_based_on_location(89., 0., now).unwrap(),
                get_brightness_based_on_time(now)
            );
        }
    }

    #[test]
    fn invalid_coordinates_return_errors() {
        for (latitude, longitude) in [(91., 0.), (0., 181.), (f64::NAN, 0.), (0., f64::INFINITY)] {
            assert!(get_brightness_based_on_location(latitude, longitude, Local::now()).is_err());
        }
    }

    #[test]
    fn schedule_wakes_at_next_integer_change_or_transition() {
        let day = Local::now();
        let at = |hour| {
            day.with_time(NaiveTime::from_hms_opt(hour, 0, 0).unwrap())
                .unwrap()
        };
        let schedule = |now| scheduled_brightness(now, at(6), at(10), at(18), at(22));
        for now in [at(5), at(6), at(8), at(12), at(18), at(20)] {
            let (brightness, delay) = schedule(now);
            let next = now + delay;
            assert!(delay > Duration::ZERO);
            assert_ne!(schedule(next).0, brightness);
            assert_eq!(schedule(next - Duration::from_secs(1)).0, brightness);
        }
    }

    #[test]
    fn test_fallback() {
        let now = Local::now();
        let brightness_up_begin = now
            .with_time(FALLBACK_BRIGHTNESS_UP_BEGIN.unwrap())
            .unwrap();
        let brightness_up_end = brightness_up_begin + BRIGHTNESS_TRANSITION_DURATION;
        let brightness_down_begin = now
            .with_time(FALLBACK_BRIGHTNESS_DOWN_BEGIN.unwrap())
            .unwrap();
        let brightness_down_end = brightness_down_begin + BRIGHTNESS_TRANSITION_DURATION;

        for i in 0..=brightness_up_begin.hour() {
            assert_eq!(
                compute_brightness_percentage(
                    now.with_time(NaiveTime::from_hms_opt(i, 0, 0).unwrap())
                        .unwrap(),
                    brightness_up_begin,
                    brightness_up_end,
                    brightness_down_begin,
                    brightness_down_end
                ),
                1
            );
        }

        for i in brightness_up_end.hour()..=brightness_down_begin.hour() {
            assert_eq!(
                compute_brightness_percentage(
                    now.with_time(NaiveTime::from_hms_opt(i, 0, 0).unwrap())
                        .unwrap(),
                    brightness_up_begin,
                    brightness_up_end,
                    brightness_down_begin,
                    brightness_down_end
                ),
                100
            );
        }

        for i in brightness_down_end.hour()..=23 {
            assert_eq!(
                compute_brightness_percentage(
                    now.with_time(NaiveTime::from_hms_opt(i, 0, 0).unwrap())
                        .unwrap(),
                    brightness_up_begin,
                    brightness_up_end,
                    brightness_down_begin,
                    brightness_down_end
                ),
                1
            );
        }
    }

    #[test]
    fn test_exact_transition_points() {
        let now = Local::now();
        let brightness_up_begin = now
            .with_time(NaiveTime::from_hms_opt(6, 7, 8).unwrap())
            .unwrap();
        let brightness_up_end = now
            .with_time(NaiveTime::from_hms_opt(7, 8, 9).unwrap())
            .unwrap();
        let brightness_down_begin = now
            .with_time(NaiveTime::from_hms_opt(19, 18, 17).unwrap())
            .unwrap();
        let brightness_down_end = now
            .with_time(NaiveTime::from_hms_opt(20, 19, 18).unwrap())
            .unwrap();

        assert_eq!(
            compute_brightness_percentage(
                brightness_up_begin,
                brightness_up_begin,
                brightness_up_end,
                brightness_down_begin,
                brightness_down_end
            ),
            1
        );
        assert_eq!(
            compute_brightness_percentage(
                brightness_down_end,
                brightness_up_begin,
                brightness_up_end,
                brightness_down_begin,
                brightness_down_end
            ),
            1
        );
        assert_eq!(
            compute_brightness_percentage(
                brightness_up_end,
                brightness_up_begin,
                brightness_up_end,
                brightness_down_begin,
                brightness_down_end
            ),
            100
        );
        assert_eq!(
            compute_brightness_percentage(
                brightness_down_begin,
                brightness_up_begin,
                brightness_up_end,
                brightness_down_begin,
                brightness_down_end
            ),
            100
        );
    }
}
