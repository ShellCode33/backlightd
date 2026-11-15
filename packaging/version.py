#!/usr/bin/env python3
"""Validate release versions without requiring third-party Python modules."""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent


def version():
    versions = []
    for name in ("backlightd", "backlightctl", "backlight_ipc"):
        text = (ROOT / name / "Cargo.toml").read_text()
        match = re.search(r'^version\s*=\s*"([^"]+)"', text, re.MULTILINE)
        if not match:
            raise ValueError(f"No package version in {name}/Cargo.toml")
        versions.append(match[1])
    if len(set(versions)) != 1:
        raise ValueError(f"Workspace package versions disagree: {versions}")
    result = versions[0]
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?", result):
        raise ValueError(f"Unsupported release version: {result}")
    return result


if __name__ == "__main__":
    try:
        result = version()
        if len(sys.argv) > 1 and sys.argv[1] != "v" + result:
            raise ValueError(f"Tag must be v{result}, got {sys.argv[1]!r}")
        print(result)
    except ValueError as error:
        sys.exit(str(error))
