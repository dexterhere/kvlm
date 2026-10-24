# Git workflow

`origin` is `https://github.com/dexterhere/kvlm.git`. The existing foundation was
published as the initial baseline on 2026-10-24. All subsequent work follows:

```text
feature/* → develop → staging → main
```

- `develop`: integration of tested feature work. Branch features from here and
  target feature pull requests here. Community fork branches are welcome.
- `staging`: release-candidate validation. Its pull requests come only from this
  repository's `develop`; fixes go through a feature branch and develop first.
- `main`: stable validated history. Its pull requests come only from this
  repository's `staging` after the acceptance evidence is reviewed.

Never skip a stage, force-push shared branches, or commit directly to these three
branches. The initial foundation bootstrap is the sole direct-push exception.
Do not label a milestone complete just because it has been merged.

## Enforcement

CI's `git-policy` check rejects incorrect pull-request routes and co-author trailers
across all proposed commits. `foundation` runs build, lint, tests, and packaging checks.
Both checks are intended to be required by server-side protection on `develop`,
`staging`, and `main`, with force pushes/deletions forbidden and pull requests required.
Server settings must be inspected separately; local files cannot enforce them.

Enable this checkout's local commit-message hook:

```sh
git config core.hooksPath .githooks
```

It rejects commits directly on integration branches and co-author trailers. Hooks
are a convenience, not a substitute for the required CI/server rules. Use the
configured Git identity; no generated attribution or sign-off is added automatically.

For example, start a new feature from an up-to-date develop branch:

```sh
git fetch origin
git switch develop
git pull --ff-only origin develop
git switch -c feature/my-change
```

Use merge commits for promotions so ancestry is preserved; do not squash or rebase
`develop → staging` or `staging → main`. Open a new feature PR for later fixes.

## Small, frequent checkpoints

Commit coherent units as they become reviewable: documentation organization,
workspace foundation, UI shell, build integration, or a focused bug fix. Avoid a
single end-of-task dump and avoid commits that only exist to inflate commit count.

Use an imperative subject with a useful prefix:

```text
docs: organize product and contributor guidance
build: initialize four-crate Rust workspace
feat: add GNOME application shell
test: validate repository metadata
```

Explain non-obvious intent or validation in the body when useful. Stage specific
paths and inspect the staged diff before committing. Do not rewrite a contributor's
history or discard unrelated working-tree changes without agreement.

## Authorship

Use the real configured Git author. Never invent an identity. Do not add
`Co-authored-by` trailers, generated assistant attribution, or tool signatures.
This does not remove legal copyright notices or authorize inaccurate authorship.
No sign-off trailer, CLA, or copyright assignment is required by this project.

## Before review

Run the [development checks](getting-started.md). Keep credentials, personal paths,
generated files, model weights, and database contents out of commits. Update docs
and tests with behavior changes. A pull request should explain the resulting
behavior and any meaningful validation limits.

## Before community publication

- Publish the intended commits to the chosen repository and add its URL to
  distribution metadata, including AppStream.
- Configure a private vulnerability-reporting route and conduct contact.
- Set branch protections and required CI checks on the hosting service.
- Keep the current application ID provisional until the namespace is confirmed.
- Validate release licensing, AppStream content, and distribution packaging.

These are host configuration tasks, not settings that local files can enforce.
