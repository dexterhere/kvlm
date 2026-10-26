# Benchmark Lab page

The Lab page runs the fixed benchmark protocol (see `benchmark-lab-protocol.md`) against the
managed llama.cpp server, stores the experiment, shows the diagnostic findings
(`benchmark-lab-diagnostics.md`) and compares two stored experiments
(`benchmark-lab-compare.md`).

## What it does

- **Check** resolves the request against the running managed server and describes in plain
  language what Run would do. Nothing is started by Check.
- **Run** is enabled only after a successful Check of the same request. Changing the number of
  passes or the cold-load option withdraws the Check. One Check covers one Run.
- **Cancel** raises the run's cancellation flag; a cancelled run is stored as cancelled with the
  passes it finished.
- Only one run is in flight. A second `Run` command is refused by the worker, not queued.
- **Stored experiments** lists the newest 100. View loads one with its passes and findings;
  Delete asks twice; two picked rows can be compared, the older one being the baseline.
- A triggered finding that names a controlled change offers **Check a follow-up**. The follow-up
  needs its own Check and Run like any other run.

## What it restarts

The lab only measures a server this application started and still owns. It never starts a
server by itself: with nothing running, Check says to start one on the Runtimes page.

- A *cold load* restarts the running server once, timed until it reports ready. The restarted
  process has a new process group, port and alias; the lab follows it.
- A *follow-up* stops the user's server, starts the same build with exactly one profile setting
  changed, runs the protocol, then starts the user's original profile again. The restore runs
  after failure and cancellation too; if it fails, the result carries a notice saying so.
- A follow-up is refused unless the running profile's requested settings equal the baseline's,
  the baseline completed, and the change is valid for the profile (for example half the
  context must still hold the baseline's prompt and answer).

An attached Ollama is never restarted or modified; the lab does not run against it yet.

## Safety rules

- A Check is bound to the server it looked at (profile, build and generation). If the user
  stops or replaces that server before pressing Run, Run is refused and asks for a new Check.
- Every restart the lab causes is guarded: under the runtime operations lock it re-checks that
  the server is still the expected generation and that the run was not cancelled. Restoring
  the user's profile after a follow-up only happens if the last start was still the
  follow-up's own profile; if the user started something else meanwhile, it is left alone.
- A panic in a run is contained: the user's profile is still restored, the run slot is freed
  and the page gets an error result.
- Closing the window cancels a run in progress.
- Experiments a previous process left running are marked interrupted once, before the first
  list or run.
- The model file is checked to be a regular file before it is opened (a FIFO would block).
  The digest describes the file at the profile's path when the run starts; replacing the file
  or retargeting a symlink under a running server is not detected.

## What it records

The experiment record holds the requested settings from the profile and, for a managed server,
the settings read from the owned process's own command line. The model digest is the SHA-256 of
the model file, computed on first use and cached by path, size and modification time. Findings
are evaluated from the stored passes and saved with the experiment.

Layers offloaded to the GPU are not read from the server yet, so DIAG-01 reports what it could
not check instead of guessing. Nothing is sent over the network; the page performs no
background work until the user presses Check or Run.

## Threading

The page uses its own command and event channels (`lab::start`). GTK objects stay on the main
thread; hashing, hardware detection and store access run on blocking threads; progress ticks
are coalesced and dropped when the channel is full, results are never dropped.
