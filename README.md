# backlightd

This tool aims to manage the backlight of all your monitors (laptop + external ones) at the same time.

Some alternatives exist but they all have some drawbacks:

- brightnessctl : unable to manage external monitors unless you install an out-of-tree kernel module
- ddcutil : unable to manage laptops' builtin monitor + extremely slow
- clightd : written in C, prone to bugs and vulnerabilities (its daemon runs as root)

Features:

- Automatically adjust brightness based on sunrise/sunset at your location.
- Allows you to take over and manually set the desired brightness
- Caching mechanism which enables way more reactive external monitors brightness change than ddcutil would by default.

Roadmap:

- Change color temperature of monitors
- Support luminescence sensors
