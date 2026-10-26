# Managed runtime in the interface

Status: implemented (M2 phase 2f). Live behaviour with a real `llama-server` is exercised in phase 2g.

The Runtimes page gains a "llama.cpp (Managed)" card beside the attached Ollama readings. Attached runtimes keep
their own wording and are never started, stopped or modified here.

## What the card does

- **Server row:** lifecycle state (`Stopped`, `Starting`, `Ready`, `Busy`, `Stopping`, `Failed`), the loopback endpoint
  while a process is live, the context size the server reports, restarts since the last start, and the failure reason
  with the exit code. `Busy` is derived from the server's own slot or request counters, never invented. Stop, Restart
  and Logs live here; the log window is read-only, selectable, monospace and refreshed once a second only while open.
- **Installed builds:** the builds this application installed, one marked for use, with Remove (refused while a process
  is live). Find lists the newest official release for this computer only when pressed; Install downloads, checks
  SHA-256, extracts and publishes under the application data directory, with progress and Cancel. One install at a time.
- **Launch profiles:** saved profiles with Start, Edit and Delete, and a New profile editor. The editor has typed
  fields only (model file, port, context length, GPU layers, K/V cache types, flash attention, micro-batch, threads,
  parallel slots, memory mapping, memory lock). There is no free-text argument field, and `LaunchProfile::validate` has
  the final say before anything is saved or started.

## Wiring

`CoreCommand::Managed(ManagedCommand)` and `UiEvent::Managed(ManagedEvent)` carry every request with an id; answers to
older snapshot requests are dropped. Status flows through a watch channel, so a burst of changes reaches the interface
as the latest value only; install progress is coalesced the same way. The backend is a trait (`ManagedBackend`) so the
bridge and card are tested with an in-memory fake: no process, filesystem or network.

The supervisor is created on the first managed request (the Runtimes page asks for the saved state when it opens). That
also checks for a `llama-server` left behind by a crashed run and stops it only when it can prove the process is ours.
Nothing is downloaded at start-up. On a normal quit, SIGINT, SIGTERM or SIGHUP the application stops the managed server
before exiting (bounded by the stop grace period); after a kill, the next start reclaims it.

## Limits

Profiles start with automatic restart on (at most three in ten minutes). The card shows one managed server. Logs show the
newest 500 lines of the 2,000-line ring buffer. A second application instance sees "Another instance of the application
manages the runtime" and cannot start a server.
