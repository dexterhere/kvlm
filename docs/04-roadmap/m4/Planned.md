# M4 plan: expanded integrations and public beta

Date: 2026-10-27. Status: planned. Some M4 items cannot be completed or verified by software work
alone; they are listed separately so they are never reported as done.

## What software work can deliver

1. **LM Studio adapter**: attached, externally owned; `GET /v1/models` and LM Studio's model listing
   endpoint, loopback only, read-only plus the OpenAI-compatible completion stream for benchmarks.
2. **vLLM adapter**: attached; `GET /v1/models`, `GET /health`, Prometheus `GET /metrics` scraping with a
   bounded parser for known vLLM metric names (running/waiting requests, KV cache usage, token counters).
3. **Keyboard and accessibility polish**: `Ctrl+1..4` view switching (exists), `Ctrl+K` quick search focus,
   consistent accessible labels and descriptions, focus order, no information by colour alone, minimum size
   360 px, text scaling, reduced motion. Automated checks where possible (widget tree label audit in smoke
   tests); real screen-reader and GNOME-session passes are manual.
4. **Packaging**: AppStream metainfo completeness (screenshots placeholders excluded, OARS, releases entry),
   desktop file, icon, Meson install and test, a native build recipe in the documentation, update check that is
   **opt-in** and makes one request only when asked (no telemetry, no background polling), release checklist.

## What needs a person or hardware

| Item | Why it cannot be done by the build |
| --- | --- |
| Validation on a physical NVIDIA laptop/desktop, an AMD APU, a CPU-only AVX2 machine | Only the reference machine (RTX 4050, Hyprland) is available here; AMD and CPU-only machines are not |
| GNOME-session validation | No GNOME Shell session is installed on the reference machine |
| Application namespace confirmation | Decided: `io.github.dexterhere.Kvlm` (ADR 0019). Flathub registration is still to do |
| Public security and community contacts | Requires real addresses the owner controls; placeholders are not invented |
| Public release/tag and store submissions | Outward-facing, owner decision |

For each, the deliverable is a checklist with exact commands and the expected evidence format so the owner can run
it, plus the code paths that make the check possible (for example a `--report-hardware` mode that prints a
redacted capability report). No hardware matrix is claimed as validated unless it was run.

## Local execution is not proven by loopback

An adapter that talks to `127.0.0.1` may still reach a runtime that proxies requests elsewhere, and an unauthenticated
loopback port trusts every local user. The execution-time local-only guard of the quick benchmark therefore applies to
every adapter: a benchmark runs only when the adapter supplies affirmative evidence (the model is listed as a local
file/weights and no remote routing marker is present). For LM Studio and vLLM that evidence is often unavailable; the
interface then says "local execution cannot be proven for this runtime" and requires an explicit confirmation instead of
implying it. Managed llama.cpp (own process, own file) is the provable case. Capability contracts (M2 phase 2a) cover
discovery, loaded models and streaming inference for these adapters; vLLM streaming uses the OpenAI-compatible stream.

## Security and performance

Adapters use loopback-only clients with no proxy or redirects, response caps, request deadlines, and parse only known
fields. Metrics scraping is rate limited (at most one scrape in flight, default every 5 s while a card is visible, never
in the background when hidden). No credentials are stored for these adapters.

## Phases

| Phase | Branch | Contents |
| --- | --- | --- |
| 4z | `chore/m4-app-id-decision` | decision record for the application ID and contacts with options; the ID lives in one constant and one rename script so a change is mechanical; **runs first** because packaging depends on it |
| 4a | `feat/m4-lmstudio-adapter` | LM Studio adapter, tests with a local server, Runtimes card |
| 4b | `feat/m4-vllm-adapter` | vLLM adapter and metrics parser, tests, Runtimes card |
| 4c | `feat/m4-a11y-keyboard` | quick search, shortcuts, label audit, minimum size, docs |
| 4d | `feat/m4-packaging` | AppStream/desktop/Meson completeness, opt-in update check, release checklist |
| 4e | `chore/m4-beta-readiness` | hardware validation procedures, capability report, beta status document; M4 is reported *implemented, validation pending* until the owner supplies reviewed evidence for each hardware matrix and the GNOME/accessibility passes |
