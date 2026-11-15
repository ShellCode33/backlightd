# backlightd user guide

backlightd controls laptop backlights and external DDC/CI monitors together. Use
one CLI for desktop shortcuts, a status bar, and gradual automatic brightness.

- [[Installation]] — requirements, build, installation, and service commands.
- [[NixOS]] — Nix package and declarative system setup.
- [[CLI]] — brightness, power, JSON, refresh, and device listing.
- [[Configuration]] — every setting, validation, reload, and legacy overrides.
- [[Automatic-brightness]] — solar schedules, clock fallback, manual overrides.
- [[Brightness-and-calibration]] — limits, curves, monitor IDs, and accuracy.
- [[Integrations]] — shortcuts and a Waybar example without a wrapper script.
- [[Troubleshooting]] — missing screens, stale readings, logs, and recovery.

## Quick start

```sh
backlightctl -b 60%
backlightctl -b +5%
backlightctl --auto
backlightctl --devices
```

New installations use a linear brightness scale and automatic logical targets
between 10% and 100%. With no configured location, morning/evening ramps start at
06:00 and 18:00 local time and last four hours each. IP geolocation is disabled
by default. There is no ambient-light sensor integration.

Settings live in `/etc/backlightd/config.toml`, with comments. After editing:

```sh
backlightd --check-config
sudo systemctl kill --signal=HUP backlightd
```

The code-focused reference is generated with rustdoc; this wiki is the user manual.
