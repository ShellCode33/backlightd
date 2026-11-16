# Architecture and invariants

## Ownership and request flow

```text
CLI / udev notification
        -> Unix socket -> bounded client worker -> controller queue
                                                    |
                                           serialized device work
                                           /                    \
                                  sysfs backlight             DDC/CI
```

The main thread owns the listening socket, its lifetime-held ownership lock, and
signalfd. It waits for clients, signals, and controller completion using `poll`,
not periodic supervision polling. At most 16 client workers are retained. Socket
reads and writes have five-second timeouts; waiting for controller completion is
not covered by those socket timeouts.

Workers decode a stream with one persistent `BufReader`, timestamp requests with
boottime, enqueue them, and wait for replies. Successful mutating commands do not
send an acknowledgment frame: clients request status afterward. Errors send an
`ErrorResponse` and close the connection. `NotifyShutdown` closes that client
connection; it never shuts down the system daemon. `NotifyHotplug` enqueues a
notification without waiting for device discovery or returning status.

The controller owns ordering of commands, discovery, mode changes, and deadlines.
Linux bus/EDID enumeration runs in a single worker. Brightness commands use the
current devices while it runs. EDID uses I2C address 0x50, while DDC/CI uses 0x37;
kernel transfers serialize bus access and each descriptor keeps its own address.
Brightness/power VCP operations, initialization probes, and sysfs discovery stay
on the controller, avoiding competing DDC/CI request/reply sequences.

Worker completion wakes the existing poll loop through a socket endpoint closing,
including on panic. No scan-completion polling or new timer is needed. The worker
returns handles/EDID only, so brightness is probed on application using the latest
configuration and mode. Startup and error recovery remain synchronous. Short VCP
probes, retries, and blocked hardware I/O can still delay commands.

An explicit refresh queues a bounded reply waiter. Other clients keep operating
while it enumerates. Shutdown joins the enumeration worker before returning.

## Discovery and recovery

Full discovery runs at startup, every 60 seconds, after detected resume, on an
explicit refresh or hotplug notification, and during failed-operation recovery.
Periodic discovery is a deliberate safeguard for missed events and unusual
hardware. Removing it changes reliability, not merely performance.

Hotplug notifications use a 250 ms quiet window capped at one second. Events
received before a later scan started are covered by that scan. Events during a
scan remain eligible for a follow-up. Synchronous refreshes preserve the scan's
error result when coalesced. The udev rule filters DRM events using `HOTPLUG=1`.

Discovery builds a replacement device list and reports partial failures while
retaining usable devices. Auto mode adjusts those devices even after a partial
scan. DDC initialization retries a failed brightness probe up to three times,
waiting 200 ms between attempts.

Manual intent is recorded before I/O. Recovery saves the absolute target of each
failed brightness write, so a relative step is never applied twice. A recovered
write returns success only if discovery succeeds and all failed devices remain
present and accept their target. Missing devices remain errors. Power recovery
retries only failed identities, not every rediscovered display.

New and recovered devices inherit the manual target, or the current automatic
schedule when auto is active. Ordinary scans preserve externally changed values
on continuously present devices. Detected resume reconciles manual targets on
known devices too, because monitors can reset their settings during sleep.

## Clocks and shutdown

`boot_time::Instant` and `CLOCK_BOOTTIME` deadlines include suspend time. The
controller compares boottime progress with `std::time::Instant` progress to detect
suspend when it next wakes. Clock samples bracket the monotonic read to avoid
mistaking scheduling delays for suspend. This is not an independent immediate
resume notification; timer deadlines and queued commands wake the controller.

SIGTERM, SIGINT, and SIGHUP are blocked before workers start and received through
signalfd on the main thread. SIGHUP validates and queues configuration; termination
stops accepting clients, closes client connections, and requests controller exit.
The current controller operation finishes, queued commands are not executed after
shutdown is observed, and workers are joined before socket ownership is released.
A hardware call that never returns can still require systemd's stop timeout.

## Socket lifetime

A persistent `.lock` sidecar is held with nonblocking `flock`. Never unlink the
lock file during cleanup: another process may already hold an open descriptor to
that inode. Startup refuses live sockets, symlinks, and non-socket paths, and
recovers stale sockets under the lock. Nonblocking probes treat a full listener
backlog as active. Cleanup compares device/inode identity before removing a path.

The socket currently has mode `0777`; there is no application authentication or
per-user authorization layer. Group-restricted access is future work. Never
expose this interface as a network service without a separate security design.
