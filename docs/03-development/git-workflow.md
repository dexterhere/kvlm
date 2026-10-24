# Git workflow

Use `develop` for feature integration, `staging` for release validation, and `main`
for stable releases. Promote changes by reviewed PRs in that order:
`codex/<feature>` → `develop` → `staging` → `main`.
Create feature branches from `origin/develop`; do not merge pending feature branches
into one another. While the current instruction is PR-only, leave every PR open
for review and do not merge or enable auto-merge.
`origin` is configured as `https://github.com/dexterhere/kvlm.git`. Adding the remote
does not publish commits or establish an upstream tracking branch. Push changes
only when publication is requested.

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
