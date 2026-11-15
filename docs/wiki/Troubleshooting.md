# Troubleshooting

## Start with logs and validation

```sh
systemctl status backlightd
journalctl -u backlightd -b
backlightd --check-config
backlightctl --devices
```

The installed service is a system service, so `journalctl --user` is not its log.
For more detail, set this in the config and reload:

```toml
[logging]
level = "debug"
```

```sh
backlightd --check-config
sudo systemctl kill --signal=HUP backlightd
```

Debug logs identify discovery reasons, timings, initialization failures, inheritance
writes, recovery results, and received termination signals. A successful write
means the operation succeeded at the interface; it is not verified luminance.

## A screen is missing

1. Enable DDC/CI in an external monitor's on-screen menu.
2. Check the cable/adapter/dock and the driver's exposed `/dev/i2c-*` devices.
3. Try `backlightctl --refresh --devices` and inspect initialization errors.
4. For a laptop, check whether `/sys/class/backlight` contains a device.

A disconnected or powered-off monitor may remain visible to enumeration while
its DDC probe fails. This can produce a partial-discovery error without breaking
the remaining screens. A transient failed probe is retried, but persistent errors
remain visible. A successful video connection alone does not establish DDC support.

The service needs writable sysfs attributes for laptop brightness and `bl_power`.
The supplied service allows sysfs writes; custom hardening overrides can block them.

## Brightness seems wrong or stale

Read [[Brightness-and-calibration]] and compare requested, representable, raw,
and readback fields. Coarse hardware steps, calibration, and averaging can all
explain a difference. Device details do not read hardware on every request.
Changes made through a monitor's buttons become visible on discovery; periodic
full discovery currently runs every 60 seconds. Use `--refresh` for a synchronous
reread before a relative adjustment.

Equal percentages do not guarantee equal perceived brightness. Avoid assuming a
curve is universal or interpreting cached control settings as emitted light.

## Automatic mode uses unexpected times

Check local system time/time zone, `[automatic]` anchors, and location source.
Solar scheduling without a location uses clock fallback. Polar conditions or
short solar intervals that cannot fit the configured ramps also use fallback.
An enabled IP service may locate a VPN exit instead of your physical location;
manual coordinates avoid that ambiguity.

IP lookup has a two-second timeout and five-minute retry backoff. Valid stale
coordinates can continue to be used when the network fails; the location cache is
`/var/cache/backlightd/last_known_location.toml`. Selecting `source = "none"` or
`schedule = "clock"` does not require deleting the cache.

## Reload did not change behavior

Check for a validation error in the journal and legacy environment overrides with
`systemctl cat backlightd`. Validate the same file the running daemon was started
with. A custom `--config` path is not changed by editing the default file.

Calibration reload in manual mode leaves the current hardware setting unchanged
until the next brightness command. Changing manual override duration counts from
the original adjustment and may immediately expire it. Do not use service restart
to test timeout preservation: restart starts auto mode again.

## Startup, shutdown, and hotplug

An existing `.lock` file is normal. Do not delete it to bypass ownership checks.
A second daemon is refused; stale sockets are recovered at startup. If binding
fails, inspect the path and permissions rather than removing arbitrary files.

SIGTERM/SIGINT wait for current device work, then release the socket. A blocked
hardware call can delay this until systemd's stop timeout. During suspend/resume,
DRM may emit hotplug events too; one follow-up scan after the resume scan can be
expected when events arrived during discovery.

For repeated screen blanking, compare daemon logs with compositor/output-manager
logs. This alone does not identify the cause. An explicitly coordinated test with
the daemon stopped can help separate DDC activity from display reconfiguration;
record what changes rather than assuming either component is responsible.
