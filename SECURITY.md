# Security policy

## Supported versions

The project is pre-release and has no supported stable release yet. Security fixes
target the latest development branch. Do not use the scaffold as a production
inference manager or expose endpoints to untrusted networks.

## Reporting a vulnerability

Do not post secrets, working exploits, or sensitive system information in public
issues. Use the host's private vulnerability-reporting facility **if maintainers
have enabled it**, or an explicitly published private maintainer contact.

**Publication prerequisite:** no public repository or private security contact is
configured in this checkout. Maintainers must enable private reporting and publish
a verified contact here before inviting public vulnerability submissions. An
unconfigured reporting route must not be advertised as available. No response-time
or bounty promise is made.

Include the affected revision, environment, impact, reproduction steps, and any
suggested mitigation. Redact access tokens, prompts, machine identifiers, and paths.
Coordinate public disclosure with maintainers after a fix is available.

## Implementation boundaries

- Core operations run without root. Do not modify system service files.
- Attached runtimes remain externally owned: do not signal their processes, rewrite
  their launch configuration, or directly delete their model files.
- Managed runtimes bind to loopback by default. LAN access needs explicit user action.
- Never execute untrusted model code, Python pickle, or `trust_remote_code=True`.
- Verify downloaded runtime/model artifacts; do not trust filenames as integrity evidence.
- Store gated-model credentials in Secret Service, never SQLite, logs, or plaintext config.
- Do not add analytics or automatic crash-report uploads. Catalog queries and
  user-requested downloads are the intended outbound traffic.
- Block cloud-routed models when local-only mode is enabled.
- Validate paths and arguments at trust boundaries; avoid shell interpolation for launches.

These are requirements for upcoming features. Their presence in this policy does
not mean the corresponding runtime, downloader, or credential implementation exists.
See the [master specification](docs/01-product/gnome_local_ai_workstation_master_specification.md).
