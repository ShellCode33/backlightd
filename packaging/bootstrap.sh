#!/bin/sh
# Build dependencies inside a disposable distribution container, never the host.
set -eu
case "$1" in
    deb)
        apt-get update
        DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
            build-essential ca-certificates curl git libudev-dev pkg-config python3 \
            dpkg-dev fakeroot zstd tzdata
        ;;
    rpm)
        dnf install -y gcc make git curl ca-certificates pkgconf-pkg-config \
            systemd-devel rpm-build python3 tar gzip zstd findutils tzdata
        ;;
    suse)
        zypper --non-interactive install gcc make git curl ca-certificates pkg-config \
            systemd-devel rpm-build python3 tar gzip zstd findutils shadow util-linux timezone
        ;;
    arch)
        pacman -Syu --noconfirm --needed git curl ca-certificates python pkgconf \
            systemd-libs fakeroot tzdata
        ;;
    *) echo "Unknown package family: $1" >&2; exit 1 ;;
esac
