# SPDX-License-Identifier: GPL-3.0-or-later
"""Validate local Markdown links and the shared application identity, without network access."""

import configparser
from pathlib import Path
import re
import sys
import tomllib
from urllib.parse import unquote, urlsplit
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parents[1]
APP_ID = "org.gnome.LocalAIWorkstation"


def markdown_errors(root: Path) -> list[str]:
    errors = []
    files = list(root.glob("*.md"))
    for directory in ("docs", "crates", "tests", ".github"):
        files.extend((root / directory).rglob("*.md"))
    for source in sorted(files):
        for match in re.finditer(r"\[[^\]]*\]\(([^\s)]+)\)", source.read_text()):
            target = urlsplit(match.group(1))
            if target.scheme or target.netloc or not target.path:
                continue
            destination = source.parent / unquote(target.path)
            if not destination.exists():
                errors.append(f"{source.relative_to(root)}: missing link {target.path}")
    return errors


def identity_errors(root: Path) -> list[str]:
    errors = []
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    if len(manifest["workspace"]["members"]) != 4:
        errors.append("Cargo.toml: expected the four M0–M4 crates")
    if manifest["workspace"]["package"]["license"] != "GPL-3.0-or-later":
        errors.append("Cargo.toml: unexpected project license")

    desktop = configparser.ConfigParser(interpolation=None)
    desktop.read(root / "data" / f"{APP_ID}.desktop.in")
    if desktop["Desktop Entry"]["Exec"] != "local-ai-workstation":
        errors.append("Desktop launcher does not match the Cargo binary")
    if desktop["Desktop Entry"]["Icon"] != APP_ID:
        errors.append("Desktop launcher icon does not match the application ID")

    metadata = ET.parse(root / "data" / f"{APP_ID}.metainfo.xml.in").getroot()
    if metadata.findtext("id") != APP_ID:
        errors.append("AppStream component ID does not match")
    if metadata.findtext("launchable") != f"{APP_ID}.desktop":
        errors.append("AppStream launcher does not match")
    if metadata.findtext("project_license") != "GPL-3.0-or-later":
        errors.append("AppStream project license does not match")

    schema = ET.parse(root / "data" / f"{APP_ID}.gschema.xml").find("schema")
    if schema is None or schema.get("id") != APP_ID:
        errors.append("GSettings schema ID does not match")
    icon = root / "data/icons/hicolor/scalable/apps" / f"{APP_ID}.svg"
    if not icon.is_file():
        errors.append("Application icon is missing")
    if f'pub const APP_ID: &str = "{APP_ID}";' not in (
        root / "crates/workstation-core/src/lib.rs"
    ).read_text():
        errors.append("Rust application ID does not match desktop metadata")
    return errors


def main() -> int:
    try:
        errors = markdown_errors(ROOT) + identity_errors(ROOT)
    except (OSError, ValueError, KeyError, configparser.Error, ET.ParseError) as error:
        print(f"Repository validation failed: {error}", file=sys.stderr)
        return 1
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print("Local Markdown links and application metadata are consistent.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
