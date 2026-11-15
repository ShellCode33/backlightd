# Brightness and calibration

## Understand the numbers first

```sh
backlightctl --devices
backlightctl --devices --json
```

Device details distinguish:

| Field | Meaning |
|---|---|
| `requested_percent` | Retained logical target from a successful command, or null |
| `brightness_percent` | Logical percentage representable by the stored hardware value |
| `hardware_percent` | Raw control value as a rounded percentage of the hardware maximum |
| `hardware_raw`, `maximum_raw` | Device control value and maximum, using 32-bit integers |
| `readback_raw` | Control value read during discovery; null after an unverified write |
| `actual_raw` | Optional sysfs actual_brightness reading; null when unavailable/invalidated |

Ordinary group status averages representable logical brightness across enabled
monitors. None of these values measures emitted light in nits or guarantees equal
perceived brightness between monitors. Status uses cached values; it does not
perform a fresh read. To update device readings, use `backlightctl --refresh`.

A device with only three raw brightness steps cannot represent every percentage.
A requested 1% may become raw 1, reported as 33%. The daemon retains the requested
target so repeated small relative changes can eventually reach the next step.
External hardware changes found during discovery reset that retained intent.

## Limit a monitor's hardware range

Copy its exact ID from `--devices`, then add:

```toml
[[devices]]
id = "COPY-THE-EXACT-ID-HERE"
minimum_percent = 5
maximum_percent = 80
```

Logical 0% now maps to 5% of that monitor's control range, and logical 100% maps to
80%. With default linear mapping, logical 50% maps to 42.5%, rounded to a hardware
step. This applies to both manual commands and automatic targets.

The same fields under `[brightness]` apply globally. Device rules override only
the fields they specify. `[automatic]` limits select logical schedule endpoints;
they are not a second set of raw hardware bounds.

## Optional curves

```toml
[brightness]
exponent = 2.0
```

With default hardware bounds, logical 50% maps to 25% of the hardware range for
exponent 2.0. Exponents above one provide finer low-end control; below one brighten
the low end. Default 1.0 is linear. Valid values are finite and between 0.1 and 10.
This is a tuning preference, not a universal correction for human perception.

Start with the linear default, tune one screen at a time, and compare under your
usual lighting. Validate and reload after editing. In manual mode, reload leaves
the hardware setting unchanged; the next brightness command uses the new mapping.
Auto mode reapplies its current target.

## Exclude a screen

```toml
[[devices]]
id = "COPY-THE-EXACT-ID-HERE"
enabled = false
```

The screen remains discoverable/listed but receives no brightness/power writes
and does not affect the group average. Removing the rule restores global control.

## IDs and hardware limits

DDC IDs fingerprint EDID and normally survive I2C bus renumbering. Two screens
with identical EDIDs share a configuration ID. Without EDID, the fallback may be
bus-dependent. Sysfs IDs retain the historical `acpi:` prefix for compatibility;
this does not mean the backend assumes ACPI firmware.

Sysfs brightness supports values beyond 65,535. DDC retains its 16-bit protocol
limit. The driver/monitor remains responsible for supported steps and behavior.
Backlight power, control percentages, actual driver readings, and compositor DPMS
are separate mechanisms.
