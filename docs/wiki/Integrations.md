# Desktop integrations

## Shortcuts

Bind these commands using your desktop or compositor's shortcut configuration:

```sh
backlightctl -b +5%
backlightctl -b -5%
backlightctl --auto
```

The commands use the configured socket and calibration. A brightness adjustment
selects manual mode; auto resumes after the configured timeout unless set to
`"forever"`. Use `--refresh` before a relative command if you need to reread an
external brightness change first, accepting the cost of discovery.

## Waybar

Waybar can consume JSON directly; an optional `jq` transformation avoids a custom
wrapper script. This example requires Waybar's custom module support and `jq`:

```jsonc
"custom/backlight": {
    "exec": "backlightctl --json | jq -c '{text: (.brightness_percent | tostring), alt: .mode}'",
    "return-type": "json",
    "interval": 10,
    "format": "{text}% ({alt})",
    "tooltip-format": "Scroll to adjust brightness; click for automatic control",
    "exec-on-event": true,
    "on-click": "backlightctl --auto",
    "on-scroll-up": "backlightctl -b +5%",
    "on-scroll-down": "backlightctl -b -5%"
}
```

Add `custom/backlight` to the desired module list. The ten-second interval updates
the display while auto mode changes brightness. These queries read daemon state;
they do not trigger hardware reads or full discovery. The current CLI does not
provide a streaming status subscription. Waybar configuration/reload belongs to
your desktop session, independently of the daemon configuration.

## Scripts

```sh
backlightctl --json
backlightctl --devices --json
```

Check exit status and stderr. A failed command can have succeeded on some screens;
retrying a relative command blindly may adjust those screens again. Prefer an
absolute target when implementing script-level retries. The daemon already makes
bounded recovery attempts for failed writes.

The regular JSON shape is suitable for a group indicator; the device array is
better for diagnostics or displays that need to distinguish monitor settings.
