# SPDX-License-Identifier: GPL-3.0-or-later
"""Rename the application ID everywhere it appears, mechanically and reviewably.

    python3 scripts/rename_app_id.py NEW_ID           # print the plan, change nothing
    python3 scripts/rename_app_id.py NEW_ID --apply   # rename files and rewrite text

The old ID is read from `APP_ID` in crates/workstation-core/src/lib.rs. Only files tracked by git are
touched, and only if they are valid UTF-8 text. Run `cargo test`, `python3 scripts/check_repository.py`
and the packaging checks afterwards.
"""

import argparse
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
LIB = "crates/workstation-core/src/lib.rs"
ID_PATTERN = re.compile(r"^[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_-]*){2,}$")
# The keyring attribute that finds the saved Hugging Face token is derived from the ID.
SECRET_FILE = "crates/workstation-runtimes/src/secrets.rs"
# The decision record names both IDs on purpose and is never rewritten.
KEEP = {"docs/05-decisions/0019-application-id.md"}


def current_id(root: Path) -> str:
    match = re.search(r'pub const APP_ID: &str = "([^"]+)";', (root / LIB).read_text())
    if match is None:
        raise ValueError(f"{LIB}: APP_ID not found")
    return match.group(1)


def validate(new_id: str) -> None:
    if not ID_PATTERN.fullmatch(new_id) or len(new_id) > 255:
        raise ValueError(
            "The ID must be reverse-DNS with at least three dot-separated parts, "
            "for example io.github.owner.AppName"
        )
    if new_id.startswith(("org.gnome.", "org.freedesktop.", "org.kde.")):
        raise ValueError(
            "IDs under org.gnome., org.freedesktop. and org.kde. imply membership in those projects"
        )


def tracked(root: Path) -> list[str]:
    out = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z"], check=True, capture_output=True
    ).stdout
    return [name for name in out.decode().split("\0") if name]


def plan(root: Path, old: str, new: str, files: list[str]) -> dict:
    renames, rewrites = [], []
    for name in files:
        if name in KEEP:
            continue
        path = root / name
        if old in path.name:
            renames.append((name, str(Path(name).with_name(path.name.replace(old, new)))))
        try:
            text = path.read_text()
        except (UnicodeDecodeError, OSError):
            continue
        if old in text:
            rewrites.append(name)
    return {"renames": renames, "rewrites": rewrites}


def apply(root: Path, old: str, new: str, changes: dict) -> None:
    for name in changes["rewrites"]:
        path = root / name
        path.write_text(path.read_text().replace(old, new))
    for source, target in changes["renames"]:
        subprocess.run(["git", "-C", str(root), "mv", source, target], check=True)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("new_id")
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args(argv)
    try:
        validate(args.new_id)
        old = current_id(ROOT)
        if old == args.new_id:
            raise ValueError("The application ID is already that value")
        changes = plan(ROOT, old, args.new_id, tracked(ROOT))
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"rename_app_id: {error}", file=sys.stderr)
        return 1
    print(f"{old} -> {args.new_id}")
    for source, target in changes["renames"]:
        print(f"  rename  {source} -> {target}")
    for name in changes["rewrites"]:
        print(f"  rewrite {name}")
    if SECRET_FILE in changes["rewrites"]:
        print(
            "  note: a Hugging Face token saved under the old ID will not be found; "
            "save it again after the rename"
        )
    if args.apply:
        apply(ROOT, old, args.new_id, changes)
        print("Applied. Review `git status`, then run the test and packaging checks.")
    else:
        print("Dry run. Add --apply to make these changes.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
