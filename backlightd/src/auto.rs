//! Solar and local-clock schedules with bounded ramps and explicit next deadlines.
//!
//! Polar/overlapping solar cycles use clock fallback. Local anchors resolve DST
//! gaps and repetitions explicitly; scheduling does not poll hardware.

use std::time::Duration;

use crate::config::{Automatic, Schedule};
use anyhow::Context;
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use sunrise::{Coordinates, SolarDay, SolarEvent};

use crate::location::find_location;

// Return the next representable logical percentage transition, without polling.
pub(crate) fn next_adjustment() -> anyhow::Result<(u8, Duration)> {
    let config = crate::config::current();
    let now = Local::now();
    if config.automatic.schedule == Schedule::Solar {
        if let Some((latitude, longitude)) = find_location()? {
            return location_schedule(latitude, longitude, now, &config.automatic);
        }
    }
    Ok(clock_schedule(now, &config.automatic))
}

fn location_schedule(
    latitude: f64,
    longitude: f64,
    now: DateTime<Local>,
    settings: &Automatic,
) -> anyhow::Result<(u8, Duration)> {
    let coordinates =
        Coordinates::new(latitude, longitude).context("Invalid location coordinates")?;
    let date = now.date_naive();
    let mut days = Vec::new();
    for date in [date.pred_opt(), Some(date), date.succ_opt()]
        .into_iter()
        .flatten()
    {
        let day = SolarDay::new(coordinates, date);
        let (Some(sunrise), Some(sunset)) = (
            day.event_time(SolarEvent::Sunrise),
            day.event_time(SolarEvent::Sunset),
        ) else {
            // A complete neighboring cycle is needed to check for overlap.
            return Ok(clock_schedule(now, settings));
        };
        if sunrise >= sunset {
            return Ok(clock_schedule(now, settings));
        }
        days.push((sunrise.with_timezone(&Local), sunset.with_timezone(&Local)));
    }
    Ok(solar_schedule_with_settings(now, &days, settings)
        .unwrap_or_else(|| clock_schedule(now, settings)))
}

fn solar_schedule_with_settings(
    now: DateTime<Local>,
    days: &[(DateTime<Local>, DateTime<Local>)],
    settings: &Automatic,
) -> Option<(u8, Duration)> {
    let duration =
        crate::config::duration(&settings.transition_duration).expect("validated duration");
    // Keep the configured ramps. If the surrounding solar cycles
    // cannot fit them, let the caller use the regular clock-based schedule.
    if days
        .iter()
        .any(|(sunrise, sunset)| *sunrise + duration > *sunset)
        || days.windows(2).any(|pair| pair[0].1 + duration > pair[1].0)
    {
        return None;
    }
    // Before today's sunrise, yesterday's sunset ramp can still be active.
    let index = days
        .iter()
        .rposition(|(sunrise, _)| *sunrise <= now)
        .unwrap_or(0);
    let &(sunrise, sunset) = days.get(index)?;
    let next_sunrise = days.get(index + 1).map(|(sunrise, _)| *sunrise);
    let up_end = sunrise + duration;
    let down_end = sunset + duration;
    let (brightness, mut delay) =
        scheduled_with_settings(now, sunrise, up_end, sunset, down_end, settings);
    if let Some(next) = next_sunrise {
        if let Ok(until_sunrise) = (next - now).to_std() {
            delay = delay.min(until_sunrise);
        }
    }
    Some((brightness, delay))
}

// Choose the earliest occurrence of a repeated local time. Move a nonexistent
// local time forward to the first valid minute (also covers skipped dates).
fn local_time<T: TimeZone>(zone: &T, mut time: NaiveDateTime) -> Option<DateTime<T>> {
    for _ in 0..=48 * 60 {
        match zone.from_local_datetime(&time) {
            chrono::LocalResult::Single(at) => return Some(at),
            chrono::LocalResult::Ambiguous(a, b) => return Some(a.min(b)),
            chrono::LocalResult::None => {}
        }
        time = time.checked_add_signed(chrono::Duration::minutes(1))?;
    }
    None
}

fn clock_schedule(now: DateTime<Local>, settings: &Automatic) -> (u8, Duration) {
    let duration =
        crate::config::duration(&settings.transition_duration).expect("validated duration");
    let morning = crate::config::clock(&settings.morning).expect("validated time");
    let evening = crate::config::clock(&settings.evening).expect("validated time");
    let mut phases = Vec::new();
    for offset in -2..=2 {
        if let Some(date) = now
            .date_naive()
            .checked_add_signed(chrono::Duration::days(offset))
        {
            for (time, rising) in [(morning, true), (evening, false)] {
                if let Some(at) = local_time(&Local, date.and_time(time)) {
                    phases.push((at, rising));
                }
            }
        }
    }
    phases.sort_by_key(|(at, _)| *at);
    phases.dedup_by_key(|(at, _)| *at);
    if let Some(index) = phases.iter().rposition(|(at, _)| *at <= now) {
        let (start, rising) = phases[index];
        if let Some((next, _)) = phases.get(index + 1) {
            // DST can shorten the real interval between local-clock anchors.
            let end = (start + duration).min(*next);
            return ramp(now, start, end, *next, rising, settings);
        }
    }
    (settings.minimum_percent, Duration::from_secs(60 * 60))
}

fn next_midnight(now: DateTime<Local>) -> DateTime<Local> {
    now.date_naive()
        .succ_opt()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .and_then(|time| local_time(&Local, time))
        .filter(|next| *next > now)
        .unwrap_or(now + chrono::Duration::hours(1))
}

fn ramp(
    now: DateTime<Local>,
    start: DateTime<Local>,
    end: DateTime<Local>,
    next_phase: DateTime<Local>,
    rising: bool,
    settings: &Automatic,
) -> (u8, Duration) {
    let range = f64::from(settings.maximum_percent - settings.minimum_percent);
    let span = (end - start).num_seconds().max(1) as f64;
    let progress = ((now - start).num_seconds() as f64 / span).clamp(0., 1.);
    let value = (f64::from(settings.minimum_percent)
        + range * if rising { progress } else { 1. - progress })
    .round() as u8;
    let next = if range == 0. || now >= end {
        next_phase
    } else if rising {
        let seconds =
            ((f64::from(value - settings.minimum_percent) + 0.5) / range * span).ceil() as i64;
        start + chrono::Duration::seconds(seconds)
    } else {
        let seconds =
            ((f64::from(settings.maximum_percent - value) + 0.5) / range * span).floor() as i64 + 1;
        start + chrono::Duration::seconds(seconds)
    };
    (
        value,
        (next.min(next_midnight(now)) - now)
            .to_std()
            .unwrap_or(Duration::from_secs(1))
            .max(Duration::from_secs(1)),
    )
}

fn scheduled_with_settings(
    now: DateTime<Local>,
    up_begin: DateTime<Local>,
    up_end: DateTime<Local>,
    down_begin: DateTime<Local>,
    down_end: DateTime<Local>,
    settings: &Automatic,
) -> (u8, Duration) {
    if now < up_end {
        ramp(now, up_begin, up_end, down_begin, true, settings)
    } else {
        ramp(
            now,
            down_begin,
            down_end,
            next_midnight(now),
            false,
            settings,
        )
    }
}

#[cfg(test)]
fn legacy() -> Automatic {
    Automatic {
        minimum_percent: 1,
        ..Automatic::default()
    }
}
#[cfg(test)]
fn get_brightness_based_on_location(
    lat: f64,
    lon: f64,
    now: DateTime<Local>,
) -> anyhow::Result<(u8, Duration)> {
    location_schedule(lat, lon, now, &legacy())
}
#[cfg(test)]
fn get_brightness_based_on_time(now: DateTime<Local>) -> (u8, Duration) {
    clock_schedule(now, &legacy())
}
#[cfg(test)]
fn solar_schedule(
    now: DateTime<Local>,
    days: &[(DateTime<Local>, DateTime<Local>)],
) -> Option<(u8, Duration)> {
    solar_schedule_with_settings(now, days, &legacy())
}
#[cfg(test)]
fn scheduled_brightness(
    now: DateTime<Local>,
    a: DateTime<Local>,
    b: DateTime<Local>,
    c: DateTime<Local>,
    d: DateTime<Local>,
) -> (u8, Duration) {
    scheduled_with_settings(now, a, b, c, d, &legacy())
}
#[cfg(test)]
fn compute_brightness_percentage(
    now: DateTime<Local>,
    a: DateTime<Local>,
    b: DateTime<Local>,
    c: DateTime<Local>,
    d: DateTime<Local>,
) -> u8 {
    scheduled_brightness(now, a, b, c, d).0
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveTime, Timelike};
    const BRIGHTNESS_TRANSITION_DURATION: Duration = Duration::from_secs(4 * 3600);
    const FALLBACK_BRIGHTNESS_UP_BEGIN: Option<NaiveTime> = NaiveTime::from_hms_opt(6, 0, 0);
    const FALLBACK_BRIGHTNESS_DOWN_BEGIN: Option<NaiveTime> = NaiveTime::from_hms_opt(18, 0, 0);

    use super::*;

    fn at(day: u32, hour: u32) -> DateTime<Local> {
        use chrono::TimeZone;
        Local
            .with_ymd_and_hms(2026, 12, day, hour, 0, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn configured_limits_overnight_and_constant_schedules() {
        let settings = Automatic {
            minimum_percent: 20,
            maximum_percent: 80,
            morning: "22:00".into(),
            evening: "10:00".into(),
            transition_duration: "2h".into(),
            ..Automatic::default()
        };
        for (hour, expected) in [
            (21, 20),
            (22, 20),
            (23, 50),
            (0, 80),
            (9, 80),
            (10, 80),
            (11, 50),
            (12, 20),
        ] {
            assert_eq!(clock_schedule(at(21, hour), &settings).0, expected);
        }
        let constant = Automatic {
            minimum_percent: 42,
            maximum_percent: 42,
            ..Automatic::default()
        };
        for hour in 0..24 {
            assert_eq!(clock_schedule(at(21, hour), &constant).0, 42);
        }
    }

    #[test]
    fn daylight_saving_and_skipped_dates_resolve_without_panics() {
        let zone = std::env::var("TZ").unwrap_or_default();
        let cases = match zone.as_str() {
            "America/New_York" => vec![
                ("2026-03-08 02:30", "2026-03-08 03:00"),
                ("2026-11-01 01:30", "2026-11-01 01:30"),
            ],
            "Europe/Berlin" => vec![
                ("2026-03-29 02:30", "2026-03-29 03:00"),
                ("2026-10-25 02:30", "2026-10-25 02:30"),
            ],
            "Pacific/Apia" => vec![("2011-12-30 06:00", "2011-12-31 00:00")],
            _ => vec![],
        };
        for (input, expected) in cases {
            let naive = NaiveDateTime::parse_from_str(input, "%Y-%m-%d %H:%M").unwrap();
            let resolved = local_time(&Local, naive).unwrap();
            assert_eq!(resolved.format("%Y-%m-%d %H:%M").to_string(), expected);
            assert!(clock_schedule(resolved, &Automatic::default()).1 > Duration::ZERO);
            if let chrono::LocalResult::Ambiguous(a, b) = Local.from_local_datetime(&naive) {
                assert_eq!(resolved, a.min(b));
            }
        }
    }

    #[test]
    fn short_days_use_clock_fallback() {
        let days = [(at(20, 11), at(20, 13)), (at(21, 11), at(21, 13))];
        for now in [at(20, 11), at(20, 12), at(20, 13), at(20, 15), at(20, 17)] {
            assert!(solar_schedule(now, &days).is_none());
        }
    }

    #[test]
    fn sunset_ramp_continues_across_midnight() {
        let days = [(at(20, 6), at(20, 23)), (at(21, 6), at(21, 23))];
        for (now, expected) in [
            (at(20, 23), 100),
            (at(21, 0), 75),
            (at(21, 1), 51),
            (at(21, 2), 26),
            (at(21, 3), 1),
        ] {
            assert_eq!(solar_schedule(now, &days).unwrap().0, expected);
        }
        let before = at(21, 0) - Duration::from_secs(1);
        let (_, delay) = solar_schedule(before, &days).unwrap();
        assert_eq!(before + delay, at(21, 0));
        let (_, delay) = solar_schedule(at(21, 0), &days).unwrap();
        assert!(delay > Duration::ZERO);
        assert_eq!(solar_schedule(at(21, 0) + delay, &days).unwrap().0, 74);
    }

    #[test]
    fn location_schedule_keeps_previous_evening_brightness_after_midnight() {
        let now = at(21, 0);
        // Choose a longitude with sunset near 23:00 in the test's local zone.
        let offset_hours = f64::from(now.offset().local_minus_utc()) / 3600.;
        let longitude = ((offset_hours - 5.) * 15. + 180.).rem_euclid(360.) - 180.;
        let (brightness, delay) = get_brightness_based_on_location(0., longitude, now).unwrap();
        assert!(
            (70..=80).contains(&brightness),
            "midnight brightness: {brightness}"
        );
        assert!(delay > Duration::ZERO);
    }

    #[test]
    fn short_nights_use_clock_fallback() {
        let days = [(at(20, 1), at(20, 23)), (at(21, 1), at(21, 23))];
        for now in [at(20, 12), at(21, 0), at(21, 1), at(21, 2)] {
            assert!(solar_schedule(now, &days).is_none());
        }
    }

    #[test]
    fn real_short_winter_day_uses_clock_fallback() {
        // At this latitude, the winter solstice has less than four hours of daylight.
        let now = at(21, 12);
        let coordinates = Coordinates::new(66., 0.).unwrap();
        let day = SolarDay::new(coordinates, now.date_naive());
        let sunrise = day.event_time(SolarEvent::Sunrise).unwrap();
        let sunset = day.event_time(SolarEvent::Sunset).unwrap();
        assert!(sunset - sunrise < chrono::Duration::hours(4));
        assert_eq!(
            get_brightness_based_on_location(66., 0., now).unwrap(),
            get_brightness_based_on_time(now)
        );
    }

    #[test]
    fn real_short_summer_night_uses_clock_fallback() {
        let now = at(21, 12);
        let coordinates = Coordinates::new(-65., 0.).unwrap();
        let today = SolarDay::new(coordinates, now.date_naive());
        let tomorrow = SolarDay::new(coordinates, now.date_naive().succ_opt().unwrap());
        let sunset = today.event_time(SolarEvent::Sunset).unwrap();
        let sunrise = tomorrow.event_time(SolarEvent::Sunrise).unwrap();
        assert!(sunrise - sunset < chrono::Duration::hours(4));
        assert_eq!(
            get_brightness_based_on_location(-65., 0., now).unwrap(),
            get_brightness_based_on_time(now)
        );
    }

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
            assert!(get_brightness_based_on_location(latitude, longitude, at(21, 12)).is_err());
        }
    }

    #[test]
    fn schedule_wakes_at_next_integer_change_or_transition() {
        let day = at(21, 12);
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
        let now = at(21, 12);
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
        let now = at(21, 12);
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
