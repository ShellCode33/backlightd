#!/bin/bash
# Entry point after extracting a source archive inside an isolated container.
set -euo pipefail
bootstrap=${1:?missing distro family}
format=${2:?missing package format}
archive=${3:?missing source archive}
packaging/bootstrap.sh "$bootstrap"
export PATH="/opt/rust/bin:$PATH"
export CARGO_HOME=/build/cargo
# Signal tests fork subprocesses; avoid transient inherited flock descriptors in
# unrelated parallel tests. The socket race test still creates concurrent workers.
export RUST_TEST_THREADS=1
mkdir -p "$CARGO_HOME" /output
if [[ $format == arch ]]; then
    useradd --create-home builder
    chown -R builder:builder /build /output
    runuser -u builder -- env PATH="$PATH" CARGO_HOME="$CARGO_HOME" \
        BUILD_FLAVOR="$BUILD_FLAVOR" SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" RUST_TEST_THREADS=1 \
        bash packaging/build.sh arch /output "$archive"
else
    packaging/build.sh "$format" /output "$archive"
fi
case "$format" in
    deb) packages=(/output/*.deb) ;;
    rpm) packages=(/output/*.rpm) ;;
    arch) packages=(/output/*.pkg.tar.zst) ;;
esac
for package in "${packages[@]}"; do packaging/verify.sh "$format" "$package"; done
