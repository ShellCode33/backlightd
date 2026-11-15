//! Logical brightness -> optional user curve -> device range -> hardware steps.
use crate::{
    config::{Calibration, Config},
    monitors::{BacklightDevice, RawBrightness},
};

pub(crate) fn configure(
    inner: Box<dyn BacklightDevice + Send>,
    config: &Config,
) -> Box<dyn BacklightDevice + Send> {
    let specific = config
        .devices
        .iter()
        .find(|device| device.id == inner.config_id());
    let mapping = specific.map_or_else(
        || config.brightness.clone(),
        |device| device.calibration(&config.brightness),
    );
    Box::new(Calibrated {
        inner,
        mapping,
        enabled: specific.is_none_or(|device| device.enabled),
        requested: None,
    })
}
struct Calibrated {
    inner: Box<dyn BacklightDevice + Send>,
    mapping: Calibration,
    enabled: bool,
    requested: Option<u8>,
}
impl Calibration {
    fn to_raw(&self, percent: u8, maximum: u16) -> anyhow::Result<u16> {
        anyhow::ensure!(
            percent <= 100 && maximum > 0,
            "Invalid brightness or hardware maximum"
        );
        let value = f64::from(self.minimum_percent)
            + f64::from(self.maximum_percent - self.minimum_percent)
                * (f64::from(percent) / 100.).powf(self.exponent);
        let raw = (value / 100. * f64::from(maximum)).round() as u16;
        Ok(if value > 0. { raw.max(1) } else { 0 })
    }
    fn logical_percent(&self, raw: RawBrightness) -> u8 {
        let value = f64::from(raw.value) / f64::from(raw.maximum) * 100.;
        let fraction = ((value - f64::from(self.minimum_percent))
            / f64::from(self.maximum_percent - self.minimum_percent))
        .clamp(0., 1.);
        (fraction.powf(1. / self.exponent) * 100.).round() as u8
    }
}
impl BacklightDevice for Calibrated {
    fn identity(&self) -> String {
        self.inner.identity()
    }
    fn config_id(&self) -> String {
        self.inner.config_id()
    }
    fn name(&self) -> String {
        self.inner.name()
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn calibration(&self) -> Option<Calibration> {
        Some(self.mapping.clone())
    }
    fn requested_percent(&self) -> Option<u8> {
        self.requested
    }
    fn restore_requested(&mut self, percent: u8) {
        self.requested = Some(percent);
    }
    fn raw_brightness(&self) -> RawBrightness {
        self.inner.raw_brightness()
    }
    fn get_brightness(&self) -> u8 {
        self.requested.unwrap_or_else(|| self.reported_brightness())
    }
    fn reported_brightness(&self) -> u8 {
        self.mapping.logical_percent(self.inner.raw_brightness())
    }
    fn set_brightness(&mut self, percent: u8) -> anyhow::Result<()> {
        let raw = self
            .mapping
            .to_raw(percent, self.inner.raw_brightness().maximum)?;
        self.inner.set_raw_brightness(raw)?;
        self.requested = Some(percent);
        Ok(())
    }
    fn turn_off(&mut self) -> anyhow::Result<()> {
        self.inner.turn_off()
    }
    fn turn_on(&mut self) -> anyhow::Result<()> {
        self.inner.turn_on()
    }
}

// Stable FNV-1a fingerprint, independent of bus enumeration and Rust's hash seed.
pub(crate) fn edid_id(edid: &[u8]) -> String {
    let hash = edid.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    format!("ddc:edid:{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Hardware(RawBrightness);
    impl BacklightDevice for Hardware {
        fn identity(&self) -> String {
            "test".into()
        }
        fn name(&self) -> String {
            "test".into()
        }
        fn get_brightness(&self) -> u8 {
            crate::monitors::raw_to_percent(self.0.value, self.0.maximum).unwrap()
        }
        fn raw_brightness(&self) -> RawBrightness {
            self.0
        }
        fn set_brightness(&mut self, p: u8) -> anyhow::Result<()> {
            self.set_raw_brightness(crate::monitors::percent_to_raw(p, self.0.maximum)?)
        }
        fn set_raw_brightness(&mut self, raw: u16) -> anyhow::Result<()> {
            self.0.value = raw;
            self.0.readback = None;
            Ok(())
        }
        fn turn_on(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
        fn turn_off(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn calibration_is_invertible_and_uses_full_hardware_precision() {
        let mapping = Calibration {
            minimum_percent: 20,
            maximum_percent: 80,
            exponent: 2.0,
        };
        assert_eq!(mapping.to_raw(0, 10000).unwrap(), 2000);
        assert_eq!(mapping.to_raw(50, 10000).unwrap(), 3500);
        assert_eq!(mapping.to_raw(100, 10000).unwrap(), 8000);
        for percent in 0..=100 {
            let raw = mapping.to_raw(percent, 65535).unwrap();
            assert_eq!(
                mapping.logical_percent(RawBrightness {
                    value: raw,
                    maximum: 65535,
                    readback: None,
                    actual: None
                }),
                percent
            );
        }
        assert!(mapping.to_raw(101, 100).is_err());
        assert!(mapping.to_raw(1, 0).is_err());
    }
    #[test]
    fn quantization_is_reported_without_losing_relative_adjustment_intent() {
        let inner = Box::new(Hardware(RawBrightness {
            value: 1,
            maximum: 3,
            readback: Some(1),
            actual: None,
        }));
        let mut device = configure(inner, &Config::default());
        device.set_brightness(1).unwrap();
        assert_eq!(device.requested_percent(), Some(1));
        assert_eq!(device.reported_brightness(), 33);
        assert_eq!(device.raw_brightness().readback, None);
        for _ in 0..10 {
            device.set_brightness(device.get_brightness() + 5).unwrap();
        }
        assert_eq!(device.requested_percent(), Some(51));
        assert_eq!(device.raw_brightness().value, 2);
        assert_eq!(device.reported_brightness(), 67);
    }
    #[test]
    fn device_overrides_inherit_global_values() {
        let config = Config::parse("[brightness]\nmaximum_percent = 80\nexponent = 2\n[[devices]]\nid = 'test'\nminimum_percent = 20\nenabled = false").unwrap();
        let device = configure(
            Box::new(Hardware(RawBrightness {
                value: 35,
                maximum: 100,
                readback: Some(35),
                actual: None,
            })),
            &config,
        );
        assert!(!device.enabled());
        assert_eq!(device.reported_brightness(), 50);
        assert_eq!(device.raw_brightness().readback, Some(35));
        assert_eq!(edid_id(b"a"), "ddc:edid:af63dc4c8601ec8c");
    }
}
