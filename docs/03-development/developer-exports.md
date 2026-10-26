# Developer exports

Status: implemented (M2 phase 2e), not yet shown in the interface (phase 2f).

`workstation-core::export` turns a validated launch profile into text for Continue.dev (`config.yaml` model entry),
Zed (`settings.json` fragment), OpenCode (`opencode.json` provider fragment) and a `systemd --user` unit. Every
function validates the profile first and returns text only; the application never edits an existing editor config,
never writes into the unit directory by itself, and never enables or starts a unit. A unit the user starts is
externally owned, not Managed.

- Endpoints always use `127.0.0.1` and the profile port. The model name is the profile name.
- JSON is built with `serde_json`. YAML scalars are JSON double-quoted strings, which are valid YAML.
- Unit words are double-quoted with `\`, `"`, `%` and `$` escaped. Control characters are rejected, and the
  `llama-server` path must be absolute, without `..`, quotes, backslashes, `:`, `;`, `$` or `%` (systemd refuses those in an
  executable path). The unit uses a fixed launch alias because no supervisor picks a random one.
- Tests parse the JSON, check the escaping, and run `systemd-analyze --user verify` on a unit when it is installed.
