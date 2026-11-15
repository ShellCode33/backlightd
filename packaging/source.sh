#!/bin/bash
set -euo pipefail
out=${1:-dist}
version=$(python3 packaging/version.py "${RELEASE_TAG:?set the release tag, e.g. v0.1.0}")
mkdir -p "$out"
git archive --format=tar --prefix="backlightd-$version/" "${GITHUB_SHA:-HEAD}" | gzip -n > "$out/backlightd-$version.tar.gz"
if [[ -n ${GITHUB_OUTPUT:-} ]]; then
    {
        echo "version=$version"
        echo "epoch=$(git log -1 --format=%ct "${GITHUB_SHA:-HEAD}")"
        if [[ $version == *-* ]]; then echo 'prerelease=true'; else echo 'prerelease=false'; fi
    } >> "$GITHUB_OUTPUT"
fi
