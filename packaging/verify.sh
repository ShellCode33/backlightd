#!/bin/bash
# Run only in the disposable target container. Never starts a daemon.
set -euo pipefail
family=${1:?missing package format}
package=${2:?missing package file}
case "$family" in
    deb) dpkg-deb --info "$package"; DEBIAN_FRONTEND=noninteractive apt-get install -y "$package" ;;
    rpm)
        rpm -qp --requires "$package"
        if command -v dnf >/dev/null; then dnf install -y "$package"
        else zypper --non-interactive --no-gpg-checks install "$package"; fi
        ;;
    arch) pacman -U --noconfirm "$package" ;;
esac
/usr/bin/backlightd --check-config
/usr/bin/backlightctl --help >/dev/null
test -f /usr/lib/systemd/system/backlightd.service
test -f /usr/lib/udev/rules.d/95-backlightd.rules
grep -q 'ExecStart=/usr/bin/backlightd$' /usr/lib/systemd/system/backlightd.service
grep -q '/usr/bin/backlightctl --notify-hotplug' /usr/lib/udev/rules.d/95-backlightd.rules
# Reinstall the same package after a local edit to check native config preservation.
printf '\n# package-reinstall-preservation-test\n' >> /etc/backlightd/config.toml
case "$family" in
    deb) dpkg -i "$package" ;;
    rpm)
        if command -v dnf >/dev/null; then dnf reinstall -y "$package"
        else rpm -Uvh --replacepkgs "$package"; fi
        ;;
    arch) pacman -U --noconfirm "$package" ;;
esac
grep -q 'package-reinstall-preservation-test' /etc/backlightd/config.toml
case "$family" in
    deb) dpkg --purge backlightd ;;
    rpm) rpm -e backlightd ;;
    arch) pacman -R --noconfirm backlightd ;;
esac
test ! -e /usr/bin/backlightd
