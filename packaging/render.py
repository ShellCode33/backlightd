#!/usr/bin/env python3
"""Render native package templates from validated release metadata."""
import os
import pathlib
import re
import sys
from version import ROOT, version

source, destination = map(pathlib.Path, sys.argv[1:])
upstream = version()
values = {
    "VERSION": upstream,
    "DEB_VERSION": upstream.replace("-", "~", 1) + "-1+" + os.environ["BUILD_FLAVOR"],
    "RPM_VERSION": upstream.replace("-", "~", 1),
    "ARCH_VERSION": upstream.replace("-", "_"),
    "DEB_ARCH": os.environ.get("DEB_ARCH", ""),
    "SHLIB_DEPENDS": os.environ.get("SHLIB_DEPENDS", ""),
    "SOURCE_SHA256": os.environ.get("SOURCE_SHA256", ""),
    "UDEV_REQUIRES": "udev" if os.environ["BUILD_FLAVOR"].startswith("opensuse") else "systemd-udev",
}
for key, filename in (("POSTINSTALL", "postinstall"), ("PREREMOVE", "preremove"), ("POSTREMOVE", "postremove")):
    # Scriptlets and Arch functions supply their own shell/context.
    values[key] = "\n".join((ROOT / "packaging/scripts" / filename).read_text().splitlines()[1:])
text = source.read_text()
for key, value in values.items():
    text = text.replace("@" + key + "@", value)
if re.search(r"@[A-Z_]+@", text):
    sys.exit(f"Unresolved template token in {source}")
destination.write_text(text)
