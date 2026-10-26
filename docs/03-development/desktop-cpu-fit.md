# Desktop CPU fit scenario

The Overview contains an optional, local CPU memory-estimate workflow. It inspects a
user-selected GGUF and evaluates an explicitly supplied scenario. It does not load the
model or connect to an inference runtime.

## Choose a file and scenario

Use the GTK file chooser to select a local GGUF file. The `*.gguf` filter is a picker
convenience; the bounded regular-file inspector still validates the selected file.
It reads the GGUF header and tensor directory without reading tensor payloads. A
non-local URI is rejected and clears the previous selection. Cancelling the chooser
leaves the current selection and result unchanged. The selected path is passed only
to the local worker; it is not logged or sent over a network.

The visible context and prompt-microbatch fields start at 4096 and 512 tokens. These
are editable scenario presets, not values read from a running runtime. Choose the key
and value cache types independently (`F16`, `Q8_0`, or `Q4_0`). The workflow fixes
sequence count at one and labels it as a scenario assumption. The “Assume uniform full
attention” switch starts off. Turning it on records an explicit layout assumption;
it does not verify the model architecture or runtime compatibility.

The inspector only preserves typed GGUF declarations. Missing fields remain unknown;
training context is never substituted for the requested scenario context. Any layout
warning prevents the adapter from promoting a transformer shape. An empty warning
list does not prove that a model uses dense full attention.

## Worker, memory reading, and stale results

Selecting a file or changing any scenario control clears the prior metadata and
assessment immediately. A hardware reading or failure also invalidates and clears the
prior result. If an estimate is in flight, the controls remain editable while another
estimate is disabled. Its request ID is invalidated by changes; a stale reply releases
the busy indicator but is not rendered. Only the matching current reply may populate
the result.

The GTK thread sends a typed request to the core worker. File inspection and the fresh
CPU `MemAvailable` read run on a blocking worker (`spawn_blocking`). Each estimate
reads RAM again rather than using the Overview’s cached snapshot. A failed or missing
reading remains unknown; a zero-byte reading remains a real reading. If the command
queue is full, the UI asks the user to wait for the hardware refresh and retry. If the
worker channel is closed, it reports that the application must be reopened. Both
conditions release the pending request and permit retry.

## Reading the result

The view shows the declared architecture and relevant metadata, recognized layout
warnings, the adapter status, the fit assessment reason, any available four-part
memory breakdown, and CPU RAM budget limits. Architecture text is rendered as plain
text. Runtime compatibility remains unverified. The adapter does not produce
`Comfortable` or `Constrained`; `Not recommended` can only mean the estimate exceeded
the supplied CPU budget for this scenario. It is not a conclusion about other devices
or placements.

This workflow does not run inference, make network requests, download or execute model
code, alter runtime state, predict throughput, or provide benchmark proof. The CPU
budget is a current RAM reading from the worker, not a test of runtime allocation.
The file-size component is an estimate and is not an authenticity check. No GPU fit,
attached or managed runtime configuration, or automatic recommendation is implied.

## Optional GTK widget-flow check

The optional ignored GTK widget-flow test uses a private path-selection hook rather
than operating the native file chooser. It uses a synthetic GGUF fixture with the real
worker/RAM path and exercises input invalidation, stale replies, error display, retry,
and narrow-window layout. It is a widget-flow check, not file-chooser acceptance.

Run it only after configuring a local GTK4 Broadway display. The Broadway daemon must
be bound to loopback (`127.0.0.1`) on the chosen port (for example, `8086`), and
`XDG_RUNTIME_DIR` must point to the same runtime directory used by that daemon. With
the display exposed as `:7`, run:

```sh
GDK_BACKEND=broadway BROADWAY_DISPLAY=:7 cargo test -p workstation-app desktop_fit_widget_flow -- --ignored --test-threads=1
```

The rendered surface was viewed through an isolated local Broadway display. The
available browser controls did not exercise that rendered GTK surface, so no
interactive file-chooser test is claimed. The native
Wayland/X11 experience, real GGUFs, GPU behavior, accessibility, and supported-minimum
GTK versions still need manual acceptance. This workflow does not complete M0.

## Validation record

On 2026-10-24, formatting, workspace Clippy with warnings denied, and the normal
workspace suite passed: 65 Rust tests, with the graphical test ignored by default.
The 17 app tests cover request generations, validation, partial/zero/missing RAM,
fresh reads, preserving fit jobs during refresh coalescing, worker errors, and
shutdown. The repository validator and 9 Python tests also passed.

The opt-in GTK test passed separately on an isolated GTK4 Broadway display. It
verified unconfirmed and explicitly assumed layouts, the synthetic two-layer
393,216-byte F16 KV calculation, warning display, immediate clearing, invalid input,
stale reply rejection, retry, malformed-file handling, hardware-failure clearing,
and an allocated width at most 360 pixels. This is a synthetic widget/worker test,
not a real model loading or native file-chooser acceptance test.

The isolated session emitted portal backend warnings because its GTK3 portal could
not use the GTK4 test display. The widget test did not invoke that external chooser.
No application panic or GTK critical error occurred in the successful test.

Meson 1.3.2 setup/build, all five checks, and a staged `/usr` installation passed.
Its two existing `configure_file(copy:)` deprecation warnings remain. The staged
installation did not update host desktop caches or install into the host prefix.
