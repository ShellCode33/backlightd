use core::panic;
use std::{
    sync::mpsc::{Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};

use anyhow::anyhow;
use backlight_ipc::BacklightMode;
use chrono::{DateTime, Datelike, Local, NaiveTime};
use sunrise::sunrise_sunset;

use crate::{location::find_location, monitors};

const AUTO_ADJUST_INTERVAL: Duration = Duration::from_secs(600);

const BRIGHTNESS_TRANSITION_DURATION: Duration = Duration::from_secs(4 * 60 * 60);
const FALLBACK_BRIGHTNESS_UP_BEGIN: Option<NaiveTime> = NaiveTime::from_hms_opt(6, 0, 0);
const FALLBACK_BRIGHTNESS_DOWN_BEGIN: Option<NaiveTime> = NaiveTime::from_hms_opt(18, 0, 0);

pub fn auto_adjust(auto_adjust_receiver: Receiver<BacklightMode>) -> ! {
    let mut current_mode = BacklightMode::Auto;
    let mut last_time_mode_was_changed = Instant::now();

    loop {
        if matches!(current_mode, BacklightMode::Auto) {
            let result = match find_location() {
                Ok(Some((latitude, longitude))) => monitors::set_brightness_percent(
                    get_brightness_based_on_location(latitude, longitude),
                ),
                Ok(None) => monitors::set_brightness_percent(get_brightness_based_on_time()),
                Err(err) => Err(anyhow!("find location function failed: {err}")),
            };

            if let Err(err) = result {
                eprintln!("Unable to set brightness: {err}");
            }
        }

        match auto_adjust_receiver.recv_timeout(AUTO_ADJUST_INTERVAL) {
            Ok(new_mode) => {
                if new_mode != current_mode {
                    println!("Set backlightd mode to {new_mode:?}");
                }
                last_time_mode_was_changed = Instant::now();
                current_mode = new_mode;
            }
            Err(err) => match err {
                RecvTimeoutError::Timeout => {} // fallthrough
                RecvTimeoutError::Disconnected => panic!("channel sender disconnected"),
            },
        }

        // Set back the mode to auto when the mode has not been changed for 12 hours,
        // so that the user doesn't have to manually set the auto mode after a manual adjustment.
        if Instant::now() - last_time_mode_was_changed > Duration::from_secs(12 * 60 * 60) {
            current_mode = BacklightMode::Auto;
        }
    }
}

fn get_brightness_based_on_location(latitude: f64, longitude: f64) -> u8 {
    let now = Local::now();
    let (sunrise_timestamp, sunset_timestamp) =
        sunrise_sunset(latitude, longitude, now.year(), now.month(), now.day());
    let sunrise_datetime: DateTime<Local> = DateTime::from_timestamp(sunrise_timestamp, 0)
        .expect("failed to create datetime from sunrise timestamp")
        .into();
    let sunset_datetime: DateTime<Local> = DateTime::from_timestamp(sunset_timestamp, 0)
        .expect("failed to create datetime from sunset timestamp")
        .into();

    compute_brightness_percentage(
        now.time(),
        sunrise_datetime.time(),
        sunrise_datetime.time() + BRIGHTNESS_TRANSITION_DURATION,
        sunset_datetime.time(),
        sunset_datetime.time() + BRIGHTNESS_TRANSITION_DURATION,
    )
}

fn get_brightness_based_on_time() -> u8 {
    compute_brightness_percentage(
        Local::now().time(),
        FALLBACK_BRIGHTNESS_UP_BEGIN.unwrap(),
        FALLBACK_BRIGHTNESS_UP_BEGIN.unwrap() + BRIGHTNESS_TRANSITION_DURATION,
        FALLBACK_BRIGHTNESS_DOWN_BEGIN.unwrap(),
        FALLBACK_BRIGHTNESS_DOWN_BEGIN.unwrap() + BRIGHTNESS_TRANSITION_DURATION,
    )
}

fn compute_brightness_percentage(
    now: NaiveTime,
    brightness_up_begin: NaiveTime,
    brightness_up_end: NaiveTime,
    brightness_down_begin: NaiveTime,
    brightness_down_end: NaiveTime,
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
    fn test_fallback() {
        let brightness_up_begin = FALLBACK_BRIGHTNESS_UP_BEGIN.unwrap();
        let brightness_up_end = brightness_up_begin + BRIGHTNESS_TRANSITION_DURATION;
        let brightness_down_begin = FALLBACK_BRIGHTNESS_DOWN_BEGIN.unwrap();
        let brightness_down_end = brightness_down_begin + BRIGHTNESS_TRANSITION_DURATION;

        for i in 0..=brightness_up_begin.hour() {
            assert_eq!(
                compute_brightness_percentage(
                    NaiveTime::from_hms_opt(i, 0, 0).unwrap(),
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
                    NaiveTime::from_hms_opt(i, 0, 0).unwrap(),
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
                    NaiveTime::from_hms_opt(i, 0, 0).unwrap(),
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
        let brightness_up_begin = NaiveTime::from_hms_opt(6, 7, 8).unwrap();
        let brightness_up_end = NaiveTime::from_hms_opt(7, 8, 9).unwrap();
        let brightness_down_begin = NaiveTime::from_hms_opt(19, 18, 17).unwrap();
        let brightness_down_end = NaiveTime::from_hms_opt(20, 19, 18).unwrap();

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
