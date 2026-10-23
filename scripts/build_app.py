# SPDX-License-Identifier: GPL-3.0-or-later
"""Build the Cargo application and copy its executable to Meson's target path."""

import argparse
from pathlib import Path
import shutil
import subprocess


def build(cargo: str, root: Path, target: Path, output: Path, profile: str) -> None:
    command = [
        cargo, "build", "--locked", "--package", "workstation-app",
        "--manifest-path", str(root / "Cargo.toml"), "--target-dir", str(target),
    ]
    if profile == "release":
        command.append("--release")
    subprocess.run(command, cwd=root, check=True)
    shutil.copy2(target / profile / "local-ai-workstation", output)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cargo")
    parser.add_argument("root", type=Path)
    parser.add_argument("target", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("profile", choices=["debug", "release"])
    args = parser.parse_args()
    build(args.cargo, args.root, args.target, args.output, args.profile)
