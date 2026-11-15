# Configuration and scheduling internals

## Loading and application

The daemon reads `/etc/backlightd/config.toml` unless `--config PATH` is supplied.
An absent default file uses defaults; an absent explicit file is an error.
`--check-config` requires a file and validates it without creating a socket or
accessing monitors. TOML structures reject unknown fields, and validation checks
ranges, durations, coordinate pairs, clock schedules, and unique device IDs.

Parsing and validation precede device access. Runtime configuration is an
immutable `Arc<Config>` snapshot behind a short-lived read/write lock. Main-thread
SIGHUP handling loads and validates a candidate, then queues it for the controller.
Only the controller publishes it and schedules discovery/adjustment. Invalid
reloads retain the active snapshot. Valid configuration can still encounter a
separate hardware error during application; do not confuse that with parse failure.

Reload does not extend an existing manual override. Its new duration is measured
from the original manual adjustment, using boottime. `forever` bypasses expiry
while retaining periodic discovery. In manual mode, calibration reload preserves
current hardware settings until another brightness command; auto reapplies its
current target. Runtime mode/override state is not persisted across restarts.

Legacy environment configuration remains compatible. `BACKLIGHTD_LOCATION`
overrides file coordinates/source. `BACKLIGHTD_ENABLE_LOCATION_API` changes the
source to IP or none unless manual location was selected. Its truthy values are
`1`, `y`, and `yes`; other strings disable it. `BACKLIGHTD_LOG_LEVEL` overrides file
logging through the logger's filter syntax. New configuration belongs in TOML.

## Schedule calculation

Defaults are logical 5–100%, four-hour ramps, and fallback anchors 06:00/18:00
in the machine's local time zone. IP lookup is opt-in, so a fresh installation
without coordinates follows the clock schedule even if `schedule="solar"`.

Solar calculation uses the upstream `sunrise` crate and neighboring dates. Missing
polar events, invalid ordering, or transitions that cannot fit between solar
anchors select the clock fallback rather than compressing solar ramps. Evening
transitions can continue across midnight. The result includes a deadline for the
next integer target change or relevant schedule boundary, not a fixed poll rate.

Clock anchors can cross midnight. Nonexistent local times move to the first valid
minute, including skipped dates; ambiguous times select the earlier instant by
comparison. DST can shorten a clock ramp if the next anchor arrives sooner in
real elapsed time. Equal automatic min/max form a constant schedule. Configuration
rejects overlapping nominal clock ramps. Date boundaries trigger recalculation.

## Location and persistence

Manual coordinates are finite and restricted to geographic latitude/longitude
ranges. IP lookup contacts `ip-api.com` over HTTP with a two-second global budget.
Successful coordinates are cached in memory and in
`/var/cache/backlightd/last_known_location.toml` for twelve hours. A failed refresh
keeps valid stale coordinates and backs off for five minutes. A cache write error
must not discard a successful lookup. With no usable location, use the clock
schedule; malformed explicit configuration remains an error.

Hardware brightness cache and location persistence have different purposes.
Neither permits skipping the 60-second discovery safeguard or assuming a monitor
cannot be changed through its own controls.
