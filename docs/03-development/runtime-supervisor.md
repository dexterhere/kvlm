# Managed runtime supervisor

Status: implemented (M2 phase 2c), not yet wired into the interface (phase 2f).

`workstation-runtimes::supervisor` runs the one managed `llama-server` this application owns. Attached
runtimes (for example Ollama) have no path into it: the supervisor only ever signals a child it spawned.

## Lifecycle

`Stopped → Starting → Ready → Stopping → Stopped`, or `Failed` with the exit code and the last 50 log lines.
`Busy` is derived later from the runtime's own counters (phase 2d); the supervisor never invents it.

- **Pre-flight, nothing spawned on failure.** Profile valid; model is a readable regular file (the resolved path
  is what the child receives); the build directory and `llama-server` are real, non-link entries inside the install
  root; the loopback port is free. A conflict is reported (`PortInUse`), never fixed by killing the holder.
- **Spawn.** Argument vector from `LaunchProfile::arguments` (no shell), cleared environment with a short allow-list
  (so `LLAMA_ARG_*` overrides and Hugging Face tokens never reach the child), `LD_LIBRARY_PATH` set to the build
  directory, its own process group, piped output, `kill_on_drop`.
- **Ready means identified.** Every launch gets a random `--alias`. `Ready` needs `/health` = 200, `/v1/models` listing
  that alias, and the child alive. A different listener on the port can never look Ready. Polling is 500 ms while
  starting and 5 s once ready, one probe in flight, 2 s per probe. Six consecutive failures while Ready stop the
  process (`Unresponsive`). The start timeout (default 180 s) kills the group (`StartTimeout`).
- **Stop.** SIGTERM to the child's process group, 5 s grace, SIGKILL, reap. The group is signalled only while the
  child is un-reaped, so a recycled pid is never signalled. `Stopping` always ends in `Stopped`, or `Failed(StuckOnStop)`.
- **Recovery.** An unexpected exit is `Failed(Crashed)`; with `auto_restart` it is retried at most three times in ten
  minutes with 1 s, 2 s, 4 s backoff. Default is off.
- **Dropping the handle** stops the child; `shutdown()` waits for it.

## Bounded everywhere

- Logs: 2,000 lines of at most 4 KiB, control characters replaced, pipes always drained so a flooding child
  cannot block. Reader tasks end with the child (1 s drain, then abort).
- One actor task, one probe task, two reader tasks per child; the command channel holds 8 messages.

## Crash containment

`runtimes/managed.json` (0600) records pid, `/proc/<pid>/stat` start time, executable and launch alias while a child
runs. `record::reclaim` at the next application start terminates a recorded process **only** if the pid is alive, its
start time matches, `/proc/<pid>/exe` is the recorded path inside the install root, and it leads its own process
group. Anything else is left alone and reported. The small window between that check and the signal is the same one
every pid-file scheme has; the start-time and executable checks make a wrong hit require a pid reuse by the same
binary at the same tick.

## Tests

`tests/supervisor_tests.rs` drives a fake `llama-server` (`tests/support/fake_llama_server.rs`, a test-only binary
whose behaviour is picked by the model file's first line): ready/stop, startup failure with exit code, port conflict,
bad model, link refusals, double start, never-ready timeout, wrong alias, crash after ready, auto restart, restart,
SIGTERM-ignoring child killed with its group, hostile output, flooding output, environment allow-list, handle drop,
and orphan reclaim including pid reuse and wrong executable.
