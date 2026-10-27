# Beta status

Status of the M0–M4 work as of this document. Terms are exact:

- **Implemented**: the code exists, has automated tests that need no GPU, model or network, and passed CI.
- **Validated**: a person has run it on the stated real hardware or session and recorded the result.

Nothing below is marked validated unless a record says so.

## Implemented, validation pending

| Area | Implemented | Not yet validated |
| --- | --- | --- |
| Hardware discovery, memory fit | CPU, RAM, NVIDIA (NVML), AMD (sysfs) probes; fit math | AMD discrete/APU, CPU-only AVX2 machine, a second NVIDIA generation |
| Ollama (attached) | Read-only discovery, inspection, quick benchmark | Other Ollama versions |
| Hugging Face catalog, downloads | Search, pinned revisions, resumable verified downloads, keyring token | Gated model with a real token |
| Managed llama.cpp | Verified install, profiles, supervisor, developer exports | GPU builds serving a model; editor imports and `systemd --user` unit install on a real desktop |
| Benchmark Lab | Cold/warm protocol, diagnostics, A/B and controlled follow-up | A real chat model (only a toy model was run); GPU runs |
| LM Studio, vLLM (attached) | Read-only discovery and vLLM metrics on loopback | Live servers (neither was installed) |
| Keyboard, accessibility | Shortcuts, named controls, 360 px minimum width, automated audit | Screen reader, GNOME session, large text, reduced motion |
| Packaging | Meson install, desktop file, metainfo, icon, schema, opt-in update check | Install on a clean machine, store submission |

## Needs the owner

- Application ID and namespace ([ADR 0019](../05-decisions/0019-application-id.md)); `scripts/rename_app_id.py`
  applies the decision, and a saved token under the old ID is not carried over.
- Real security and conduct contact addresses.
- Version, tag, release notes and any store or Flathub submission
  ([release checklist](release-checklist.md)).
- The machines and sessions in [hardware validation procedures](hardware-validation-procedures.md).

## Known limits

- Attached runtimes (Ollama, LM Studio, vLLM) are never benchmarked in the Lab or restarted: local execution of
  an externally owned process cannot be proven.
- The Lab does not read the offloaded layer count from llama-server, so the related diagnostic reports
  "not evaluated".
- vLLM metrics are read on demand, never polled.
- GTK smoke tests are opt-in (`-- --ignored`) and need a graphical session; on a tiling window manager some
  width assertions can fail because the compositor resizes the test window.
