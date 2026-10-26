# SPDX-License-Identifier: GPL-3.0-or-later
"""Enforce PR promotion order and author-only commit messages."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys

PROTECTED = {"develop", "staging", "main"}


def message_error(message: str) -> str | None:
    if re.search(r"^\s*co-authored-by\s*:", message, re.IGNORECASE | re.MULTILINE):
        return "Co-authored-by trailers are not permitted."
    return None


def route_error(base: str, head: str, same_repository: bool) -> str | None:
    if base == "develop" and head not in PROTECTED:
        return None
    if same_repository and (base, head) in {("staging", "develop"), ("main", "staging")}:
        return None
    return "Use feature branches → develop → staging → main; promotion branches must belong to this repository."


def check_event(event: dict) -> list[str]:
    errors = []
    if "pull_request" in event:
        pr = event["pull_request"]
        base, head = pr["base"], pr["head"]
        error = route_error(base["ref"], head["ref"], base["repo"]["full_name"] == head["repo"]["full_name"])
        if error:
            errors.append(error)
        revision = f'{base["sha"]}..{head["sha"]}'
    else:
        before, after = event["before"], event["after"]
        if after == "0" * 40:
            return ["Protected integration branches must not be deleted."]
        revision = after if before == "0" * 40 else f"{before}..{after}"
    messages = subprocess.check_output(["git", "log", "--format=%B", revision], text=True)
    if error := message_error(messages):
        errors.append(error)
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--message", type=Path, help="Validate a local commit-msg hook input")
    parser.add_argument("--event", type=Path, default=os.environ.get("GITHUB_EVENT_PATH"))
    args = parser.parse_args()
    try:
        if args.message:
            branch = subprocess.check_output(["git", "branch", "--show-current"], text=True).strip()
            errors = ["Create a feature branch from develop before committing."] if branch in PROTECTED else []
            if error := message_error(args.message.read_text()):
                errors.append(error)
        elif args.event:
            errors = check_event(json.loads(args.event.read_text()))
        else:
            parser.error("provide --message or --event (or GITHUB_EVENT_PATH)")
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"Git policy validation failed: {error}", file=sys.stderr)
        return 1
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print("Git branch route and commit messages satisfy project policy.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
