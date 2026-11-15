# Brightness representation and calibration

## Four distinct values

| Value | Meaning |
|---|---|
| Requested logical percentage | The last successfully accepted user/automatic target for a device |
| Representable logical percentage | The inverse mapping of its stored raw control value |
| Hardware control value | The last read or successfully written raw brightness setting |
| Readback / actual reading | Evidence from the last discovery read, not a physical luminance measurement |

Ordinary status averages retained logical targets across enabled devices, using
integer division. A device without a retained target uses its inverse-mapped
hardware value. Status requests do not perform fresh hardware I/O.
`GetDeviceInfo(index)` exposes the other values individually. A successful brightness write
clears its control-readback evidence; a computed representable value must not be
presented as proof that hardware applied the write.

The sysfs backend separately reports the optional `actual_brightness` attribute.
This is a driver reading, which may differ from requested `brightness`. It is not
a measurement in nits. External monitors do not expose this sysfs attribute.

## Mapping

For logical percentage `p`, configured hardware bounds `a` and `b` (in percent),
exponent `e`, and hardware maximum `M`:

```text
hardware_percent = a + (b - a) * (p / 100)^e
raw = round(M * hardware_percent / 100)
```

A positive mapped value is kept at least one raw step. Defaults are `a=0`, `b=100`,
`e=2`: a power curve allocating finer control to the low end. Set `e=1` for a
linear percentage of the device's control range. These are preference curves,
not a universal perceptual model or automatic cross-monitor luminance calibration.

The inverse normalizes the raw value into `[a,b]`, clamps it to `[0,1]`, takes the
`1/e` power, and rounds to an integer percentage. Outside configured bounds,
logical reporting clamps to 0 or 100. With a nonzero hardware minimum, logical
zero does not black out the screen; power commands remain separate operations.

Global calibration applies to manual and automatic commands. Per-device settings
override selected global values. Automatic min/max are logical schedule targets,
so they are transformed by the same calibration afterward.

## Precision and intent

Raw values and device-status fields use `u32`; percentage arithmetic uses `u64`
intermediates. Sysfs values are parsed without a 16-bit restriction. DDC retains
`u16` storage and checked narrowing at the protocol boundary. Calibration uses
`f64` for its curve and rounds only at the raw hardware step.

A panel with maximum 3 cannot represent every integer percentage. A 1% request
can produce raw 1, represented as 58% with the default curve; ordinary status
still reports the requested 1%. Retain requested intent separately: otherwise
repeated small relative adjustments can stall on the same hardware step.

On rediscovery, requested intent is restored only if both the observed raw value
and calibration are unchanged. External raw changes reset it. Failed operations
retain absolute retry targets independently of this successful-write cache.

## Device identity and exclusion

DDC configuration IDs use a deterministic FNV-1a fingerprint of EDID bytes, separate
from discovery identity, which includes backend, bus identity, and EDID. This hash
is for stable matching, not cryptographic authentication. Identical EDIDs share a
configuration rule. Without EDID, the fallback can depend on bus enumeration.
Sysfs device IDs retain their legacy `acpi:` prefix for configuration compatibility,
even though the implementation is `SysfsBacklightDevice` in `sysfs.rs`.

Disabled devices remain discoverable and listed. They are excluded from brightness
and power writes, inheritance, and the group average. Removing a device override
restores global defaults; it does not exclude the device.
