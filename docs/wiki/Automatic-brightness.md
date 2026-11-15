# Automatic brightness

The daemon starts in auto mode. Brightness follows a time-based schedule; no
ambient-light sensor, physical luminance calibration, or room-light measurement
is involved. Defaults are logical 10–100%, transformed by global/per-device calibration.

## Clock schedule

```toml
[automatic]
schedule = "clock"
minimum_percent = 15
maximum_percent = 80
transition_duration = "2h"
morning = "07:00"
evening = "19:00"
```

This ramps up between 07:00 and 09:00, holds the maximum until 19:00, ramps down
until 21:00, and holds the minimum overnight. Times use the machine's local time
zone. A schedule with morning `"22:00"` and evening `"10:00"` is also valid.
Each ramp must fit between anchors; overlapping nominal clock ramps are rejected.

Repeated daylight-saving times use the earlier instant. Missing times move to
the first valid minute, including skipped dates. If DST shortens the real interval
before the next anchor, a clock ramp shortens to fit. This avoids invalid or
backward schedule windows.

## Solar schedule and high latitudes

With `schedule = "solar"` and a location, the morning ramp starts at sunrise and
the evening ramp at sunset. The configured transition duration remains unchanged.
If neighboring solar events are missing or cannot accommodate those ramps, the
clock schedule is used. This conservative fallback covers polar day/night and
very short days or nights. Evening ramps can continue across midnight.

The default location source is none, so a fresh installation uses clock fallback.
Choose manual coordinates or opt into IP lookup in [[Configuration]]. Morning and
evening clock settings remain important even when solar scheduling is selected.

## Manual overrides, hotplug, and sleep

```sh
backlightctl -b 55%
backlightctl --auto
```

A manual adjustment selects manual mode, records intent, and starts the configured
override timer. Each later manual brightness adjustment restarts it. Time asleep
counts. Default expiry is twelve hours; `"forever"` keeps manual mode until auto is
selected or the daemon restarts.

A newly discovered or recovered monitor inherits the manual target, or the current
automatic target in auto mode. Ordinary scans preserve external control changes
on continuously present monitors in manual mode. Detected resume reapplies manual
intent because monitors can reset brightness during sleep.

Discovery remains periodic every 60 seconds as a safeguard against missed events.
DRM hotplug notifications are debounced; events during a scan can require a second
scan. A scan takes roughly two seconds on the maintainer's tested setup, but this
is not a latency guarantee. Detection after resume occurs when the controller
wakes through an event or deadline; the daemon does not independently receive an
instantaneous resume notification.
