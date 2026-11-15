# Configuration reference

Edit `/etc/backlightd/config.toml`. The installed template contains comments;
reinstalling preserves your file. Omitted keys use defaults. Unknown keys, wrong
types, and invalid values are errors rather than silently ignored typos.

```sh
backlightd --check-config
sudo systemctl kill --signal=HUP backlightd
```

The validator requires a file. An absent default file during normal startup uses
built-in defaults; an explicitly supplied missing `--config PATH` is an error.
Invalid startup configuration fails before monitor access. Invalid reloads retain
the active settings. Validation checks settings, not monitor availability or driver
support. A valid reload can therefore still encounter a separate hardware error.

## `[automatic]`

| Key | Default | Meaning |
|---|---|---|
| `schedule` | `"solar"` | `"solar"` with clock fallback, or always `"clock"` |
| `minimum_percent` | `10` | Minimum automatic logical brightness, at least 1 |
| `maximum_percent` | `100` | Maximum automatic logical brightness, at most 100 |
| `transition_duration` | `"4h"` | Duration of each morning/evening ramp |
| `morning` | `"06:00"` | Morning clock anchor in local `HH:MM` time |
| `evening` | `"18:00"` | Evening clock anchor in local `HH:MM` time |

Minimum may equal maximum for constant auto brightness. Clock anchors must differ;
each ramp must fit in both intervals between anchors. The settings are logical
percentages, transformed by calibration. They do not restrict manual targets.
See [[Automatic-brightness]] for solar fallback, overnight schedules, and DST.

## `[brightness]`

| Key | Default | Meaning |
|---|---|---|
| `minimum_percent` | `0` | Hardware-control percentage corresponding to logical 0 |
| `maximum_percent` | `100` | Hardware-control percentage corresponding to logical 100 |
| `exponent` | `1.0` | Optional percentage curve; finite, from 0.1 through 10 |

The minimum must be strictly below the maximum, which cannot exceed 100. These
settings affect manual and automatic brightness. The default mapping is linear.
See [[Brightness-and-calibration]] before choosing a curve or narrowing the range.

## `[[devices]]`

Repeat this table for exact device IDs from `backlightctl --devices`:

```toml
[[devices]]
id = "COPY-THE-EXACT-ID-HERE"
enabled = false
```

| Key | Default | Meaning |
|---|---|---|
| `id` | Required | Exact device configuration ID; nonempty and unique |
| `enabled` | `true` | Whether brightness/power control and averaging include it |
| `minimum_percent` | Inherited | Override the global hardware-control minimum |
| `maximum_percent` | Inherited | Override the global hardware-control maximum |
| `exponent` | Inherited | Override the global curve |

Inherited and explicit values must form a valid calibration together. Rules may
refer to currently disconnected monitors. Disabling excludes writes and averaging,
but discovery still reads the monitor. Removing the rule enables global defaults
again; it does not exclude the device. Identical EDIDs share a rule.

## `[location]`

| Key | Default | Meaning |
|---|---|---|
| `source` | `"none"` | `"none"`, `"manual"`, or `"ip"` |
| `latitude` | Unset | Finite number from -90 through 90 |
| `longitude` | Unset | Finite number from -180 through 180 |

Manual location requires both coordinates. Supplying only one is invalid, even
if another source is selected. To use sunrise and sunset without a network lookup:

```toml
[location]
source = "manual"
latitude = 48.8566
longitude = 2.3522
```

`source = "ip"` explicitly enables the external ip-api.com HTTP service. It is
approximate, can be affected by VPNs, and uses a persistent location cache. A valid
stale location survives lookup failures. With no usable location, solar scheduling
falls back to clock times. `schedule = "clock"` needs no lookup.

## `[manual]`

`override_duration = "12h"` sets the timeout after a manual brightness adjustment.
Use `"forever"` to keep manual mode until `--auto` or a daemon restart. Duration
strings use a positive integer followed by `s`, `m`, `h`, or `d`; finite durations
must be between 1 second and 365 days. Automatic transition duration uses the same
units, subject to its tighter schedule-fit constraints.

Elapsed override time includes suspend. A reload measures the new duration from
the original adjustment; it does not restart the timer. A shorter duration may
therefore immediately return to auto. Restarting the daemon starts auto again.

## `[logging]`

`level = "info"` is the default. Supported values are `off`, `error`, `warn`,
`info`, `debug`, and `trace`. Change the file and reload; no service override is needed.

## Legacy environment overrides

Existing deployments may still set these variables:

- `BACKLIGHTD_LOCATION=latitude,longitude` overrides file coordinates and source.
- `BACKLIGHTD_ENABLE_LOCATION_API`: `1`, `y`, or `yes` selects IP; other strings
  select none, unless manual location is selected.
- `BACKLIGHTD_LOG_LEVEL` overrides file logging using the logger's filter syntax,
  for example `backlightd=debug`.

If a file setting seems ineffective, inspect legacy service overrides with
`systemctl cat backlightd`. New installations should use the TOML file.
