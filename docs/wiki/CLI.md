# Command-line reference

## Brightness and mode

```sh
backlightctl                 # Group brightness and current mode
backlightctl -b 60%          # Set all enabled devices to a logical 60%
backlightctl -b +5%          # Increase each target by 5 percentage points
backlightctl -b -5%          # Decrease each target by 5 percentage points
backlightctl --auto          # Resume automatic control immediately
```

`-b` is short for `--brightness`. Values require a `%` suffix. Absolute values
range from 0 to 100; relative magnitudes range from 0 to 100. Relative increases
stop at 100, and decreases stop at 1 to avoid accidentally blacking out the screen.
An explicit 0% remains allowed, but calibration with a positive hardware minimum
means logical zero will not be dark. Use power commands to turn a device off.

A brightness command selects manual mode and resets its override timer. The
configured default is twelve hours, including time suspended; `"forever"` disables
automatic expiry. `--auto` returns to the schedule regardless of that timer.
There is no per-device write selector in the CLI; device-specific behavior is
configured in TOML. `--auto` and `--brightness` cannot be combined.

## Power

```sh
backlightctl --turn-off
backlightctl --turn-on
```

These affect enabled devices, independently of stored brightness targets. Sysfs
uses `bl_power`; external monitors use DDC/CI power commands. They do not invoke
compositor DPMS, and driver support varies. Power commands do not establish a
persistent power policy or change auto/manual mode. In auto mode, scheduled
brightness writes continue; whether those wake a powered-off external monitor is
hardware-dependent.

Keep a way to send `--turn-on` when testing, such as SSH or another visible screen.
`--turn-on` and `--turn-off` cannot be combined.

## Status and device details

```sh
backlightctl --json
backlightctl --devices
backlightctl --devices --json
backlightctl --refresh --devices --json
```

Ordinary JSON has this shape:

```json
{"brightness_percent":60,"mode":"manual"}
```

The percentage is the integer average of representable logical brightness across
enabled devices, using cached values. It is not a fresh hardware measurement.
An empty managed set reports 0. See [[Brightness-and-calibration]] for the
meaning of requested, hardware, readback, and actual values.

`--devices` lists discovered devices, including disabled ones, with configuration
IDs and detailed readings. With `--json`, it prints an array. It cannot be combined
with brightness, auto, power, or hotplug-notification commands; `--refresh` is allowed.

## Discovery and integration flags

`--refresh` (`-r`) performs full discovery and waits for completion. It can precede
a relative brightness command when an external control change should be reread
first. Discovery is also automatic; regular status does not itself discover devices.

`--notify-hotplug` is for the installed udev rule. It queues a debounced notification,
returns without waiting for discovery, and prints no status. Do not use it as a
replacement for a synchronous `--refresh` in scripts needing the result.

`--unix-socket-path PATH` (`-u`) selects another daemon socket. The default is
`/run/backlightd.sock`. `--help` and `--version` display client information.

## Daemon arguments

```sh
backlightd --help
backlightd --check-config
backlightd --config ./my-config.toml --check-config
```

The daemon also accepts `--unix-socket-path PATH`. Run validation freely; starting
a second daemon for diagnostics is unnecessary and can change brightness if a
different socket allows it to run. Errors are written to stderr and cause nonzero
exit status. A command may partially succeed across monitors and still report an
error when a device remains unavailable.
