#!/bin/bash
set -euo pipefail
out=${1:-dist}
# Branch builds validate workspace versions without requiring a release tag.
if [[ -n ${RELEASE_TAG:-} ]]; then
    version=$(python3 packaging/version.py "$RELEASE_TAG")
else
    version=$(python3 packaging/version.py)
fi
mkdir -p "$out"
git archive --format=tar --prefix="backlightd-$version/" "${GITHUB_SHA:-HEAD}" | gzip -n > "$out/backlightd-$version.tar.gz"
if [[ -n ${GITHUB_OUTPUT:-} ]]; then
    {
        echo "version=$version"
        echo "epoch=$(git log -1 --format=%ct "${GITHUB_SHA:-HEAD}")"
        if [[ $version == *-* ]]; then echo 'prerelease=true'; else echo 'prerelease=false'; fi
    } >> "$GITHUB_OUTPUT"
fi
