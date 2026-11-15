#!/bin/bash
# No root, live service, hardware, or third-party Python modules required.
set -euo pipefail
cd "$(dirname "$0")/.."
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/forbidden"
# An accidental service activation must fail, even when run on a systemd host.
printf '#!/bin/sh\necho "Unexpected service operation" >&2\nexit 99\n' > "$work/forbidden/systemctl"
cp "$work/forbidden/systemctl" "$work/forbidden/udevadm"
chmod +x "$work/forbidden/"*
export PATH="$work/forbidden:$PATH"
stage="$work/stage"
make install DESTDIR="$stage" PREFIX=/usr INSTALL_PREFIX=/usr/bin \
    SYSTEMD_UNIT_DIR=/usr/lib/systemd/system UDEV_RULES_DIR=/usr/lib/udev/rules.d
cmp target/release/backlightd "$stage/usr/bin/backlightd"
cmp target/release/backlightctl "$stage/usr/bin/backlightctl"
grep -q 'ExecStart=/usr/bin/backlightd$' "$stage/usr/lib/systemd/system/backlightd.service"
grep -q '/usr/bin/backlightctl --notify-hotplug' "$stage/usr/lib/udev/rules.d/95-backlightd.rules"
if grep -RFq "$stage" "$stage/usr/lib"; then echo 'Staging path leaked into integration files' >&2; exit 1; fi
printf '\n# preserve custom configuration\n' >> "$stage/etc/backlightd/config.toml"
cp "$stage/etc/backlightd/config.toml" "$work/expected.toml"
make install DESTDIR="$stage" PREFIX=/usr INSTALL_PREFIX=/usr/bin \
    SYSTEMD_UNIT_DIR=/usr/lib/systemd/system UDEV_RULES_DIR=/usr/lib/udev/rules.d
cmp "$work/expected.toml" "$stage/etc/backlightd/config.toml"
target/release/backlightd --config "$stage/etc/backlightd/config.toml" --check-config
version=$(python3 packaging/version.py)
python3 packaging/version.py "v$version"
if python3 packaging/version.py v999.999.999; then echo 'Mismatching tag accepted' >&2; exit 1; fi
export BUILD_FLAVOR=debian13 DEB_ARCH=amd64 SHLIB_DEPENDS='libc6 (>= 2.36)' SOURCE_SHA256
SOURCE_SHA256=$(sha256sum Cargo.lock | cut -d ' ' -f1)
python3 packaging/render.py packaging/debian/control.in "$work/control"
python3 packaging/render.py packaging/rpm/backlightd.spec.in "$work/backlightd.spec"
python3 packaging/render.py packaging/arch/PKGBUILD.in "$work/PKGBUILD"
python3 packaging/render.py packaging/arch/backlightd.install.in "$work/backlightd.install"
bash -n "$work/PKGBUILD" "$work/backlightd.install"
for script in packaging/*.sh packaging/scripts/*; do bash -n "$script"; done
echo 'Staged installation and package templates passed.'
