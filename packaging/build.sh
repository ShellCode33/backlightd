#!/bin/bash
# Native package build in a disposable container with the target distro's libraries.
set -euo pipefail
family=${1:?usage: packaging/build.sh deb|rpm|arch OUTPUT_DIRECTORY SOURCE_ARCHIVE}
out=$(realpath -m "${2:?missing output directory}")
archive=$(realpath "${3:?missing source archive}")
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
version=$(python3 packaging/version.py)
: "${BUILD_FLAVOR:?set a distro identifier such as debian13 or fedora45}"
[[ $BUILD_FLAVOR =~ ^[a-z][a-z0-9]*$ ]] || { echo 'Invalid BUILD_FLAVOR' >&2; exit 1; }
mkdir -p "$out"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export SOURCE_SHA256
SOURCE_SHA256=$(sha256sum "$archive" | cut -d ' ' -f 1)

case "$family" in
    deb)
        cargo test --workspace --locked
        cargo build --workspace --release --locked
        stage="$work/stage"
        make install DESTDIR="$stage" PREFIX=/usr INSTALL_PREFIX=/usr/bin \
            SYSTEMD_UNIT_DIR=/usr/lib/systemd/system UDEV_RULES_DIR=/usr/lib/udev/rules.d
        install -m644 LICENSE "$stage/usr/share/doc/backlightd/copyright"
        mkdir -p "$work/debian" "$stage/DEBIAN"
        # dpkg-shlibdeps requires source metadata even when constructing a binary package.
        printf 'Source: backlightd\n\nPackage: backlightd\nArchitecture: any\nDescription: backlight control\n' > "$work/debian/control"
        export SHLIB_DEPENDS DEB_ARCH
        SHLIB_DEPENDS=$(cd "$work" && dpkg-shlibdeps -O \
            -e"$stage/usr/bin/backlightd" -e"$stage/usr/bin/backlightctl" | sed -n 's/^shlibs:Depends=//p')
        DEB_ARCH=$(dpkg --print-architecture)
        python3 packaging/render.py packaging/debian/control.in "$stage/DEBIAN/control"
        printf '/etc/backlightd/config.toml\n' > "$stage/DEBIAN/conffiles"
        install -m755 packaging/scripts/postinstall "$stage/DEBIAN/postinst"
        install -m755 packaging/scripts/preremove "$stage/DEBIAN/prerm"
        install -m755 packaging/scripts/postremove "$stage/DEBIAN/postrm"
        strip --strip-unneeded "$stage/usr/bin/backlightd" "$stage/usr/bin/backlightctl"
        dpkg-deb --root-owner-group --build "$stage" "$out/backlightd-${version}-${BUILD_FLAVOR}-${DEB_ARCH}.deb"
        ;;
    rpm)
        mkdir -p "$work/rpm"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
        cp "$archive" "$work/rpm/SOURCES/backlightd-$version.tar.gz"
        python3 packaging/render.py packaging/rpm/backlightd.spec.in "$work/rpm/SPECS/backlightd.spec"
        # Rust comes from the explicit CI toolchain; all native build requirements
        # are installed by bootstrap.sh rather than resolved by rpmbuild.
        rpmbuild -bb --nodeps --define "_topdir $work/rpm" \
            --define "_unitdir /usr/lib/systemd/system" \
            --define "dist .$BUILD_FLAVOR" "$work/rpm/SPECS/backlightd.spec"
        find "$work/rpm/RPMS" -name '*.rpm' -exec cp {} "$out/" \;
        ;;
    arch)
        [[ $(id -u) != 0 ]] || { echo 'makepkg must run as an unprivileged user' >&2; exit 1; }
        cp "$archive" "$work/backlightd-$version.tar.gz"
        python3 packaging/render.py packaging/arch/PKGBUILD.in "$work/PKGBUILD"
        python3 packaging/render.py packaging/arch/backlightd.install.in "$work/backlightd.install"
        (cd "$work" && makepkg --noconfirm --nodeps)
        cp "$work/"*.pkg.tar.zst "$out/"
        # Publish a source recipe that builds the same checksummed release archive.
        cp "$work/PKGBUILD" "$out/PKGBUILD"
        cp "$work/backlightd.install" "$out/backlightd.install"
        ;;
    *) echo "Unknown package format: $family" >&2; exit 1 ;;
esac
