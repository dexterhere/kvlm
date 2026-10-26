# GNOME Local AI Workstation — Product & Technical Specification
**Document Version:** 1.0.0  
**Date:** October 2026  
**Target Platform:** Linux (GNOME Desktop Primary, GTK4/Libadwaita Compatible)  
**Core Language & Stack:** Rust (2024 Edition), GTK4, Libadwaita, Tokio, SQLite (`rusqlite`)  
**Document Purpose:** Definitive product blueprint, system architecture, data contracts, and agent-ready implementation specification from Milestone M0 through M4.

---

## 1. Executive Summary & Agent Directives

### 1.1 Project Identity
**GNOME Local AI Workstation** (`org.gnome.LocalAIWorkstation` / binary: `local-ai-workstation`) is a developer-first, open-source GNOME control plane for discovering, configuring, running, benchmarking, and optimizing local AI inference engines.

**What It Is:**
- A hardware-aware model fit analyzer, runtime lifecycle manager, empirical benchmark lab, and developer endpoint configurator.
- A hybrid orchestrator that safely attaches to existing background services (e.g., system `ollama.service`) or manages isolated application-owned inference processes (e.g., portable `llama-server` binaries).

**What It Is NOT (Strict Anti-Goals):**
- **NOT a chatbot or prompt studio:** Only minimal request testing exists to verify endpoints.
- **NOT a custom inference engine:** Never write custom tensor math; always orchestrate established engines (`Ollama`, `llama.cpp`, `LM Studio`, `vLLM`).
- **NOT an autonomous agent or IDE plugin:** It exports configurations *for* tools like Continue.dev, Zed, OpenCode, and Neovim, rather than replacing them.
- **NOT a privileged root daemon:** Never require `sudo` or root privileges for core operations, model downloads, or process management.

### 1.2 The Four Core Developer Questions Answered
Every view, struct, and background worker in this repository exists to answer four questions with empirical certainty:
1. **What AI models can my machine realistically run?** (Hardware probing across Discrete VRAM, Unified/GTT GPU memory, and System RAM + exact KV-cache & weight math).
2. **Which inference engine and quantization should I use for my workload?** (Artifact-level recommendations tailored to Coding, Reasoning, Low-Latency Chat, Embeddings, or Concurrent Serving).
3. **How can I configure and switch between runtimes without breaking my system?** (Strict separation of *Attached* vs. *Managed* runtimes with zero unapproved mutations).
4. **How can I measure and fix performance bottlenecks?** (Reproducible cold/warm benchmarks, PCIe spill detection, KV-cache quantization guidance, and A/B comparisons).

---

## 2. Consolidated GNOME User Experience & Navigation Architecture

Following the GNOME Human Interface Guidelines (HIG) and patterns established by modern GNOME Circle apps (*Resources*, *Boxes*), the application consolidates its functional areas into **four primary top-level views** inside an `AdwNavigationSplitView` (or `AdwViewStack` with `AdwViewSwitcherBar` on narrow windows) plus an on-demand First-Run Assistant.

```
+-----------------------------------------------------------------------------------+
| [Sidebar Toggle]  Local AI Workstation          [Active Runtime Pill] [Menu/Prefs]|
+----------------------+------------------------------------------------------------+
|                      |                                                            |
|  * Overview &        |  MAIN CONTENT PANE (Context-Dependent)                     |
|    Monitor           |                                                            |
|                      |  - Hardware Telemetry Cards (VRAM / GTT / RAM / Compute)   |
|  * Model Explorer    |  - Real-Time KV & Weight Allocation Bar                    |
|    & Catalog         |  - Active Inference Engine Status & Bottleneck Diagnostics |
|                      |  - Quick Endpoint Copy & Profile Exporter                  |
|  * Runtimes &        |                                                            |
|    Endpoints         |                                                            |
|                      |                                                            |
|  * Benchmark Lab     |                                                            |
|                      |                                                            |
+----------------------+------------------------------------------------------------+
| Status Bar: Connected: Ollama (Attached :11434) | GPU: RTX 4050 (1.8 / 6.0 GB)    |
+-----------------------------------------------------------------------------------+
```

### 2.1 View 1: Overview & Monitor (Dashboard + Real-Time Telemetry)
- **Hardware Summary Banner:** Displays detected CPU topology, discrete GPU(s), unified memory (AMD GTT / Intel UMA), system RAM, and active acceleration backends (`CUDA`, `ROCm`, `Vulkan`, `SYCL`, `AVX2/AVX-512`).
- **Memory Pool Visualizer:** Segmented level bar (`GtkLevelBar` / custom Cairo/GSK widget) showing:
  - Baseline OS/Other GPU usage.
  - Loaded Model Weights ($M_{\text{weights}}$).
  - Allocated KV Cache ($M_{\text{KV}}$).
  - Compute Overhead ($M_{\text{buffers}}$).
  - **Spill Alert Indicator:** Highlights in warning amber (`@warning_color`) when layers or KV cache spill over PCIe into System RAM.
- **Active Engines & Live Telemetry:** Token throughput ($t/s$), active concurrent slots, GPU utilization %, and thermal/power throttling flags.

### 2.2 View 2: Model Explorer & Catalog
- **Dual-Source Search:** Queries Hugging Face Hub API and Ollama Registry with offline SQLite caching.
- **Three-Tier Hierarchy Browser:** Normalizes results into `ModelFamily` $\rightarrow$ `ModelRelease` $\rightarrow$ `ModelArtifact` (specific `.gguf` or `.safetensors` files).
- **Hardware Fit Badges:** Every artifact displays two independent badges:
  1. *Static Fit Badge:* `Comfortable` (Green), `Constrained` (Amber), or `Not Recommended` (Red/Dimmed).
  2. *Empirical Badge:* `Benchmark Verified` (Blue checkmark if local SQLite holds a completed benchmark run for that exact artifact + hardware ID).
- **Storage Deduplicator & Downloader:** Resumable HTTP downloader into `~/.local/share/local-ai-workstation/models/` with automatic SHA256 cross-checking against `~/.ollama/models/blobs` and `~/.cache/huggingface/hub` to prevent duplicate multi-gigabyte files.

### 2.3 View 3: Runtimes & Endpoints (Runtime Manager + Developer Workspace)
- **Runtime Cards:** Lists `Ollama`, `llama.cpp (Managed)`, `LM Studio`, and `vLLM`.
- **Ownership Mode Badge:** Clearly marks each instance as `Attached (External Process)` or `Managed (Workstation Owned)`.
- **Configuration Inspector:**
  - For *Attached* runtimes: Read-only inspection of running models, ports, and environment variables, plus non-destructive API actions (load/unload model via `keep_alive`).
  - For *Managed* runtimes: Full editor for context length (`--ctx-size`), GPU layers (`--n-gpu-layers`), flash attention (`--flash-attn`), KV cache quantization (`-ctk`, `-ctv`), batch size, and loopback binding (`127.0.0.1`).
- **Developer Workspace & 1-Click Exporter:**
  - Interactive mini-tester (sends a single streaming `/v1/chat/completions` request and displays raw JSON + latency breakdown).
  - **Config Snippet Generator:** Exports ready-to-paste configurations for **Continue.dev**, **Zed**, **OpenCode / Claude Code**, **Neovim (`avante.nvim` / `codecompanion.nvim`)**, **systemd user services**, and standalone shell scripts.

### 2.4 View 4: Benchmark Lab
- **Reproducible Experiment Runner:** Select 1 or more `(Runtime Profile + Model Artifact + Context Size)` configurations and run standardized workloads (*Coding Generation*, *Long-Context Prompt Processing*, *Reasoning*, *Concurrency Stress*).
- **Warm vs. Cold Separation:** Explicitly measures cold model-load latency separately from warm token generation.
- **A/B Comparison Matrix & Optimization Advisor:** Side-by-side comparison table showing TTFT (Time to First Token), Prompt Processing ($t/s$), Generation ($t/s$), Peak VRAM, and Peak RAM. Includes automated diagnostic rules that recommend concrete flag adjustments.

---

## 3. System Architecture & Workspace Layout

### 3.1 High-Level Component Diagram

```
+-------------------------------------------------------------------------+
|                       workstation-app (GTK4 / Libadwaita)               |
|  Views | Widgets | GObject State Models | Async UI Event Dispatcher     |
+------------------------------------+------------------------------------+
                                     |
                    Tokio mpsc / oneshot Channels (Typed Commands & Events)
                                     |
+------------------------------------v------------------------------------+
|                         workstation-core (Pure Rust)                    |
|  +-----------------------+  +-----------------------+  +-------------+  |
|  | Hardware Profiler     |  | Fit & Recommendation  |  | Config &    |  |
|  | (NVML, DRM sysfs,     |  | Engine (llmfit-core   |  | Profile     |  |
|  |  Vulkan, CPU flags)   |  |  integration + math)  |  | Exporter    |  |
|  +-----------------------+  +-----------------------+  +-------------+  |
+-----+--------------------+--------------------+--------------------+----+
      |                    |                    |                    |
      v                    v                    v                    v
+------------+      +--------------+     +--------------+     +-----------+
|workstation-|      | workstation- |     | workstation- |     |workstation|
| runtimes   |      |   catalog    |     |    bench     |     |   -db     |
|            |      |              |     |              |     |           |
|- Ollama    |      |- HuggingFace |     |- Warm/Cold   |     |- SQLite   |
|- llama.cpp |      |- Ollama Hub  |     |  Runner      |     |  rusqlite |
|- LM Studio |      |- Blob Dedup  |     |- Telemetry   |     |- Schema   |
|- vLLM      |      |- Downloader  |     |  Sampler     |     |  Migrations
+------------+      +--------------+     +--------------+     +-----------+
```

### 3.2 Phased Crate Structure
To avoid over-engineering on day one while maintaining clean compilation boundaries, the Cargo workspace uses **four core crates** from M0 through M4:

```text
local-ai-workstation/
├── Cargo.toml                      # Workspace root with unified dependency versions
├── meson.build                     # GNOME standard build & desktop file integration
├── data/
│   ├── org.gnome.LocalAIWorkstation.desktop.in
│   ├── org.gnome.LocalAIWorkstation.metainfo.xml.in
│   ├── org.gnome.LocalAIWorkstation.gschema.xml
│   └── icons/
├── crates/
│   ├── workstation-core/           # Domain types, hardware probing, fit math, SQLite, exporters
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── hardware/           # nvml.rs, amd_sysfs.rs, cpu.rs, vulkan.rs
│   │       ├── fit/                # memory_math.rs, llmfit_bridge.rs, advisor.rs
│   │       ├── models/             # family.rs, artifact.rs, dedup.rs
│   │       ├── db/                 # schema.rs, queries.rs
│   │       └── export/             # continue_dev.rs, zed.rs, systemd.rs
│   │
│   ├── workstation-runtimes/       # RuntimeAdapter trait, process supervisor, HTTP clients
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── traits.rs           # RuntimeAdapter, LifecycleState, OwnershipMode
│   │       ├── supervisor.rs       # Tokio child process management, log ring-buffer
│   │       ├── ollama.rs           # Attached & Managed Ollama REST implementation
│   │       ├── llamacpp.rs         # Portable llama-server manager & /metrics parser
│   │       ├── lmstudio.rs         # OpenAI-compatible + lms CLI adapter (M4)
│   │       └── vllm.rs             # OpenAI-compatible + Prometheus metrics parser (M4)
│   │
│   ├── workstation-bench/          # Reproducible benchmark runner & resource sampler
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── runner.rs           # SSE token timing, TTFT, ITL, throughput calculation
│   │       ├── workloads.rs        # Deterministic bundled prompts & seed control
│   │       └── sampler.rs          # 100ms background VRAM/RAM/GPU% polling loop
│   │
│   └── workstation-app/            # GTK4 + Libadwaita GUI application
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           ├── application.rs      # AdwApplication subclass
│           ├── window.rs           # Main window & split navigation
│           ├── bridge.rs           # Tokio <-> glib::MainContext channel bridge
│           └── views/
│               ├── overview.rs
│               ├── catalog.rs
│               ├── runtimes.rs
│               └── bench_lab.rs
└── tests/
    ├── mock_ollama_server.rs
    └── fit_calculation_tests.rs
```

### 3.3 Threading & Async Concurrency Contract (CRITICAL FOR AGENTS)
GTK4 is strictly single-threaded; all widget mutations **must** happen on the GLib main thread. Blocking the GLib main thread for even 50ms causes frame drops in GNOME Shell.
1. **Global Tokio Runtime:** Initialized once in `main.rs` (`tokio::runtime::Builder::new_multi_thread().enable_all().build()`).
2. **Command Channel (`UI -> Core`):** The GTK UI sends `CoreCommand` enums over a `tokio::sync::mpsc::UnboundedSender<CoreCommand>`.
3. **Event Channel (`Core -> UI`):** Background workers send `UiEvent` enums over an `async_channel::Sender<UiEvent>` (or `glib::MainContext::default().spawn_local(...)`) which updates GObject properties and triggers GTK redraws cleanly.
4. **Blocking Calls:** Synchronous libraries (`rusqlite`, `nvml-wrapper`, `sysfs` file reads) **must** run inside `tokio::task::spawn_blocking`.

---

## 4. Hardware Intelligence & Memory Fit Engine (`workstation-core`)

### 4.1 Three-Pool Memory Architecture
Standard tools often fail on Linux laptops and APUs because they only check discrete VRAM and system RAM. Our hardware probe must classify memory into **three distinct pools**:

1. **`DedicatedVram`:** High-bandwidth memory physically on a discrete GPU (probed via `nvml-wrapper` for NVIDIA, or `/sys/class/drm/card*/device/mem_info_vram_total` for AMD/Intel).
2. **`DynamicSharedGpu` (GTT / UMA):** System memory dynamically mappable by an integrated GPU (probed via `/sys/class/drm/card*/device/mem_info_gtt_total` on AMD Ryzen APUs or Intel Arc shared memory).
3. **`SystemRam`:** Standard host memory used for CPU inference or layer offloading (probed via `/proc/meminfo` `MemAvailable`).

### 4.2 Core Hardware Data Structures

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComputeBackend {
    Cuda { compute_capability: (u32, u32), driver_version: String },
    Rocm { gfx_version: String },
    Vulkan { api_version: String, device_name: String },
    Sycl,
    CpuOnly { avx2: bool, avx512: bool, vnni: bool, amx: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuDeviceProfile {
    pub id: String,                        // PCI bus ID or UUID
    pub name: String,                      // e.g., "NVIDIA GeForce RTX 4050 Laptop GPU"
    pub vendor: GpuVendor,
    pub dedicated_vram_total_bytes: u64,
    pub dedicated_vram_free_bytes: u64,
    pub shared_gtt_total_bytes: u64,       // Crucial for AMD APUs
    pub shared_gtt_free_bytes: u64,
    pub estimated_bandwidth_gbps: Option<f32>,
    pub supported_backends: Vec<ComputeBackend>,
    pub unified_memory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareProfile {
    pub profile_hash: String,              // Deterministic hash of CPU + GPU IDs + RAM
    pub hostname: String,
    pub cpu_model: String,
    pub physical_cores: usize,
    pub logical_cores: usize,
    pub system_ram_total_bytes: u64,
    pub system_ram_available_bytes: u64,
    pub gpus: Vec<GpuDeviceProfile>,
    pub primary_backend: ComputeBackend,
    pub captured_at_unix_ms: i64,
}
```

### 4.3 Hardware Probing Implementation Rules
- **NVIDIA Probe (`nvml.rs`):** Dynamically load `libnvidia-ml.so.1` at runtime via `nvml-wrapper`. Never link hard against CUDA libraries at compile time so the binary runs cleanly on AMD/Intel machines.
- **AMD Probe (`amd_sysfs.rs`):** Read Linux DRM sysfs nodes directly (zero external binary dependencies required):
  - Vendor check: `/sys/class/drm/cardX/device/vendor` == `0x1002`
  - Total VRAM: `/sys/class/drm/cardX/device/mem_info_vram_total`
  - Used VRAM: `/sys/class/drm/cardX/device/mem_info_vram_used`
  - Total GTT (APU Dynamic Pool): `/sys/class/drm/cardX/device/mem_info_gtt_total`
  - Used GTT: `/sys/class/drm/cardX/device/mem_info_gtt_used`
  - GPU Busy %: `/sys/class/drm/cardX/device/gpu_busy_percent`
- **Intel / Fallback Vulkan Probe (`vulkan.rs`):** Query `VkPhysicalDeviceMemoryProperties` via `ash` (dynamic Vulkan loader) or parse `vulkaninfo --summary` if present to detect Intel Arc / integrated GPU heaps.

### 4.4 Mathematical Memory Fit Formula
**Implementation clarification (2026-10-24):** The initial core calculator supports one uniform full-attention sequence with supplied single-GGUF bytes and explicit K/V dimensions. GGML Q8_0 storage is 34 bytes per 32 elements, rather than the illustrative 1.0 byte coefficient below; Q4_0 is 18 bytes per 32. Context padding and separate K/V types are accounted for. The four-part result remains an estimate, and `llmfit` integration is deferred after API evaluation. See [decision 0004](../05-decisions/0004-static-memory-fit.md) for the supported scope, pinned upstream evidence, and unknown-input rules. The original target formula is retained below for traceability.

Given a `ModelArtifact`, a `RuntimeConfig` (context window $C$, batch size $B$, KV cache data type $Q_{\text{KV}}$), and a `HardwareProfile`, the total required memory is broken into four components:

$$M_{\text{required}} = M_{\text{weights}} + M_{\text{KV}} + M_{\text{buffers}} + M_{\text{overhead}}$$

1. **Model Weights ($M_{\text{weights}}$):**
   Exact `file_size_bytes` of the `.gguf` or `.safetensors` artifact from catalog metadata.

2. **KV Cache Memory ($M_{\text{KV}}$):**
   For standard Transformer architectures (including Grouped-Query Attention, GQA):
   $$M_{\text{KV}} = 2 \times L \times H_{\text{KV}} \times D_{\text{head}} \times C \times B_{\text{bytes}}(Q_{\text{KV}})$$
   Where:
   - $L$: Number of transformer layers (`block_count`).
   - $H_{\text{KV}}$: Number of Key-Value heads (`attention.head_count_kv`).
   - $D_{\text{head}}$: Dimension per head (`embedding_length / attention.head_count`).
   - $C$: Configured context length (e.g., `8192`).
   - $B_{\text{bytes}}(Q_{\text{KV}})$: Bytes per element (`2.0` for `FP16`, `1.0` for `Q8_0`, `0.5625` for `Q4_0`).
   *(Note: For Multi-head Latent Attention (MLA) or hybrid Mamba/Attention models, delegate lookup to `llmfit-core` architectural tables).*

3. **Compute & Activation Buffers ($M_{\text{buffers}}$):**
   Estimated scratchpad memory for prompt evaluation batch size $B_{\text{ubatch}}$:
   $$M_{\text{buffers}} \approx \max(256\text{ MiB}, \; 2 \times B_{\text{ubatch}} \times D_{\text{model}} \times 4\text{ bytes})$$

4. **Runtime Context Overhead ($M_{\text{overhead}}$):**
   - CUDA / ROCm driver context initialization: $350\text{ MiB}$
   - Vulkan context initialization: $150\text{ MiB}$
   - CPU-only runtime: $64\text{ MiB}$

### 4.5 Fit Classification Rules
**Initial implementation policy (2026-10-24):** Classification applies to the selected fully resident placement. CPU uses 70%/75% available-RAM limits; dedicated GPU uses 85%/96%; shared GPU uses the minimum of its GPU-mapping and host-RAM limits rather than adding the pools. Limits are inclusive, and exceeding the maximum means NotRecommended for that placement. Missing metadata/readings or unverified compatibility stays Unknown. Partial offload and automatic alternative plans are not implemented. These are conservative static policies, not calibrated throughput claims or benchmark evidence; see [decision 0004](../05-decisions/0004-static-memory-fit.md). The broader target below remains pending integration.

Let $V_{\text{avail}}$ be the usable GPU memory (`dedicated_vram_free_bytes`, or `shared_gtt_free_bytes` on unified memory systems) and $R_{\text{avail}}$ be `system_ram_available_bytes`:

- **`Comfortable`:**
  $$M_{\text{required}} \le 0.85 \times V_{\text{avail}}$$
  *(100% of weights and KV cache fit in fast GPU memory with at least 15% safety headroom).*

- **`Constrained`:**
  - **Case A (Tight GPU Fit):** $0.85 \times V_{\text{avail}} < M_{\text{required}} \le 0.96 \times V_{\text{avail}}$, OR fits if context $C$ is reduced to $4096$ or KV cache is quantized to `Q8_0`.
  - **Case B (Partial CPU Offload):** $M_{\text{required}} > V_{\text{avail}}$, but at least $50\%$ of layers fit in $V_{\text{avail}}$ and the remaining $M_{\text{spill}} \le 0.70 \times R_{\text{avail}}$.

- **`NotRecommended`:**
  $$M_{\text{required}} > (V_{\text{avail}} + 0.75 \times R_{\text{avail}})$$
  OR the model tensor format is incompatible with all available runtimes/hardware (e.g., an `AWQ` or `FP8` model on a GPU lacking native instructions).

- **`BenchmarkVerified` (Independent Boolean Flag):**
  `true` if and only if the local SQLite database contains a successful `benchmark_runs` record for `(artifact_id, hardware_profile_hash)`.

---

## 5. Hybrid Runtime Management (`workstation-runtimes`)

### 5.1 Ownership & Safety Invariants
1. **`OwnershipMode::Attached`:** The runtime process was started outside Local AI Workstation (e.g., `systemd` `ollama.service` or user terminal).
   - **Allowed:** Query health, list models, pull models (with user confirmation), load/unload models via official API (`keep_alive: 0`), run inference benchmarks.
   - **Strictly Forbidden:** Sending `SIGTERM`/`SIGKILL` to the PID, modifying `/etc/systemd/system/ollama.service`, or deleting model files directly from disk without using the runtime's API.
2. **`OwnershipMode::Managed`:** The runtime process was spawned directly by Local AI Workstation's process supervisor (or via a Workstation-owned `systemd --user` unit).
   - **Allowed:** Start, stop, restart, customize CLI flags (`--ctx-size`, `--n-gpu-layers`, `--port`), capture stdout/stderr logs, and clean up zombie processes on application exit.
3. **Portable Managed `llama.cpp` Strategy:** Instead of writing fragile distro-specific package installers, Local AI Workstation downloads official, pre-compiled `llama-server` release bundles (Vulkan or CUDA build matching the user's GPU) into `~/.local/share/local-ai-workstation/runtimes/llamacpp/<version>/`, verifies the SHA256 checksum, and runs it unprivileged.

### 5.2 Runtime Lifecycle State Machine

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeLifecycleState {
    NotInstalled,
    Detected { version: Option<String>, endpoint: String },
    Starting { pid: Option<u32> },
    Ready { version: String, endpoint: String, loaded_models: Vec<LoadedModelState> },
    Busy { active_requests: u32, loaded_models: Vec<LoadedModelState> },
    Stopping,
    Stopped,
    Failed { error_code: String, message: String, last_log_lines: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadedModelState {
    pub artifact_id: String,
    pub runtime_model_name: String,
    pub vram_allocated_bytes: u64,
    pub ram_allocated_bytes: u64,
    pub context_length: u32,
}
```

### 5.3 The `RuntimeAdapter` Trait Contract

```rust
use async_trait::async_trait;
use anyhow::Result;

#[async_trait]
pub trait RuntimeAdapter: Send + Sync {
    /// Unique identifier: "ollama", "llamacpp", "lmstudio", "vllm"
    fn engine_id(&self) -> &'static str;

    /// Ownership mode of this specific runtime instance
    fn ownership_mode(&self) -> OwnershipMode;

    /// Probe localhost ports, processes, and binaries to determine current state
    async fn probe_state(&self) -> Result<RuntimeLifecycleState>;

    /// List all locally cached/available models recognized by this runtime
    async fn list_local_models(&self) -> Result<Vec<LocalRuntimeModel>>;

    /// List models currently resident in VRAM/RAM
    async fn list_running_models(&self) -> Result<Vec<LoadedModelState>>;

    /// Start the runtime with a specific configuration profile (Managed mode only)
    async fn start_managed(&self, profile: &RuntimeLaunchProfile) -> Result<u32>;

    /// Gracefully stop the runtime (Managed mode only; returns Err if Attached)
    async fn stop_managed(&self) -> Result<()>;

    /// Explicitly load a model into memory and warm it up
    async fn load_model(&self, model_id: &str, config: &ModelLoadConfig) -> Result<()>;

    /// Unload a model from VRAM/RAM immediately
    async fn unload_model(&self, model_id: &str) -> Result<()>;

    /// Execute a streaming inference request and emit granular timing metrics
    async fn stream_inference(
        &self,
        request: InferenceRequest,
        token_tx: tokio::sync::mpsc::Sender<TokenStreamEvent>,
    ) -> Result<InferenceSummaryMetrics>;
}
```

### 5.4 Engine-Specific Wire Protocols

**Implementation note (2026-10-25):** Attached Ollama discovery and per-model inspection
are read-only. `POST /api/show` supplies model declarations, while fresh `/api/ps`
supplies reported loaded context/allocation. Defaults never become loaded settings,
missing routing data never proves local execution, and runtime digests do not verify
an Overview file. See [decision 0008](../05-decisions/0008-ollama-model-inspection.md).
Quick Benchmark now permits one explicit request only after the guard in
[decision 0009](../05-decisions/0009-quick-benchmark.md). General inference controls,
model mutations, verified fit, and benchmark persistence remain pending.

- **Ollama Adapter (`ollama.rs`):**
  - Default Endpoint: `http://127.0.0.1:11434`
  - Health & Version: `GET /api/version`
  - Installed Models: `GET /api/tags`
  - Currently Loaded Models & VRAM Split: `GET /api/ps` (returns exact `size` and `size_vram` bytes per loaded model—use this to calculate CPU spill percentage!).
  - Unload Model: `POST /api/generate` with `{"model": "<name>", "keep_alive": 0}`
  - Cloud-Model Guardrail: Inspect model manifests; if a model is a remote proxy/cloud relay, flag it with a `Cloud Routed` warning badge and block it when `local_only_mode = true`.
- **Managed `llama.cpp` Adapter (`llamacpp.rs`):**
  - Default Endpoint: `http://127.0.0.1:8080` (dynamically configurable per profile)
  - Health Check: `GET /health` (returns `200 OK` when ready, `503` while loading weights)
  - Slot & Context Inspection: `GET /slots`
  - Prometheus Telemetry: `GET /metrics` (started with `--metrics` flag)
  - Launch Flags Controlled by Workstation:
    ```bash
    llama-server \
      --model /path/to/artifact.gguf \
      --host 127.0.0.1 \
      --port 8080 \
      --ctx-size 8192 \
      --n-gpu-layers 99 \
      --ubatch-size 512 \
      --cache-type-k q8_0 \
      --cache-type-v q8_0 \
      --flash-attn \
      --metrics
    ```

---

## 6. Live Model Catalog & Storage Deduplication

### 6.1 Three-Tier Normalization Schema
Raw Hugging Face repositories and Ollama tags are normalized into three relational structs:
1. **`ModelFamily`:** High-level architecture and family (e.g., `Qwen2.5-Coder`, Publisher: `Qwen`, Modalities: `[Text, Code]`, Parameter Sizes: `[1.5B, 7B, 14B, 32B]`).
2. **`ModelRelease`:** Specific fine-tune and parameter count (e.g., `Qwen/Qwen2.5-Coder-7B-Instruct`, Base License: `Apache-2.0`, Max Native Context: `131072`, Architectural metadata: $L=28, H_{\text{KV}}=4, D_{\text{head}}=128$).
3. **`ModelArtifact`:** A concrete, downloadable weight bundle:
   - `artifact_id`: `"hf:bartowski/Qwen2.5-Coder-7B-Instruct-GGUF:Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf"`
   - `format`: `ArtifactFormat::Gguf | Safetensors | Exl2 | Awq`
   - `quantization`: `"Q4_K_M"` (4.85 bits per weight)
   - `file_size_bytes`: `4_683_073_536`
   - `sha256`: `"65a8..."`
   - `compatible_engines`: `["ollama", "llamacpp", "lmstudio"]`

### 6.2 GGUF Header Fast-Inspection (Zero Code Execution)
- **Security Rule:** Never run Python `pickle` or `trust_remote_code=True`.
- When inspecting local `.gguf` files or fetching remote Hugging Face files, read only the GGUF binary header (or query the Hugging Face `/api/models/{repo_id}?blobs=true` endpoint which exposes parsed `gguf` metadata directly in JSON) to extract exact layer counts, head counts, and context length without downloading the full weight file.

### 6.3 Local Storage Deduplication
Before downloading any artifact or importing a GGUF into Ollama:
1. Scan `~/.ollama/models/blobs/sha256-*` and `~/.cache/huggingface/hub/models--*/blobs/*`.
2. If a matching SHA256 digest already exists on disk:
   - For `llama-server`: Point `--model` directly to the existing blob path.
   - For `Ollama`: Generate a temporary `Modelfile` (`FROM /path/to/existing/blob`) and invoke `POST /api/create` referencing the local path.

---

## 7. Benchmark Lab & Diagnostic Engine (`workstation-bench`)

### 7.1 Standardized Benchmark Protocol
**Initial implementation (2026-10-25):** Quick Benchmark is one explicitly requested,
local-only digest-pinned text request with cancellation and typed partial outcomes.
Client first-response-text latency includes model loading and is not token-level
TTFT/ITL; server decode rate uses independent server counters. There is no warm-up,
forced cold load, resource sampling, repeated pass, persistence, or verified badge.
See [decision 0009](../05-decisions/0009-quick-benchmark.md). The target protocol below
remains pending full integration and real-hardware acceptance.

**Follow-up implementation (2026-10-26):** SQLite history preserves requested snapshots
and terminal outcomes ([0010](../05-decisions/0010-benchmark-history.md)). Bounded
100ms-target sampling now stores observed system RAM and per-device VRAM/GTT maxima
with coverage counts ([0011](../05-decisions/0011-benchmark-memory-sampling.md)).
These observations are system/device-wide, not process RSS or certified true peaks.
GPU utilization, repeated cold/warm protocols, empirical badges, and real-hardware
acceptance remain pending. Existing records are not rewritten.

Every benchmark run executes a deterministic pipeline to prevent noisy or misleading numbers:
1. **Pre-Flight Check:** Record baseline free VRAM, free RAM, GPU temperature, and CPU load.
2. **Cold Load Phase (Optional toggle):** If the model is unloaded, trigger `load_model` and record `cold_load_duration_ms` until the runtime reports `Ready`.
3. **Warm-Up Pass:** Send a short 32-token prompt (`seed = 42, temperature = 0.0`) to initialize GPU compute kernels. Discard timing from this pass.
4. **Measured Passes ($N = 3$ default):**
   - Stream responses via Server-Sent Events (SSE) or NDJSON.
   - Record high-resolution monotonic timestamps (`std::time::Instant`) for:
     - $t_{\text{send}}$: Request dispatched.
     - $t_{\text{first\_token}}$: First non-empty content token received.
     - $t_{i}$: Arrival timestamp of each subsequent token $i \in [2..K]$.
   - Merge client-side wall-clock timestamps with server-reported metrics (`prompt_eval_duration`, `eval_duration` from Ollama / `timings` object from `llama.cpp`).
5. **Parallel Resource Sampling:** A background Tokio task samples `nvml` / `sysfs` every `100ms` during the run to capture `peak_vram_bytes`, `peak_ram_bytes`, and `mean_gpu_utilization_pct`.

### 7.2 Captured Metrics Summary
- **TTFT (Time to First Token):** $t_{\text{first\_token}} - t_{\text{send}}$ (ms)
- **Prompt Processing Speed:** $\frac{\text{prompt\_tokens}}{\text{prompt\_eval\_duration\_sec}}$ ($t/s$)
- **Token Generation Speed (Decode):** $\frac{\text{generated\_tokens}}{\text{eval\_duration\_sec}}$ ($t/s$)
- **Inter-Token Latency (ITL):** Median and P95 of $(t_i - t_{i-1})$ (ms)
- **Memory Footprint:** Peak GPU VRAM (bytes), Peak Process RAM (bytes), and Offloaded Layer Ratio.

### 7.3 Automated Bottleneck Diagnostic Rules
After every benchmark or during live monitoring, the **Diagnostic Advisor** evaluates deterministic heuristics and surfaces actionable recommendations in the UI:

| Rule ID | Trigger Condition | Explanation Shown to Developer | Recommended Action |
|---|---|---|---|
| `DIAG-01` | `vram_used > 0` AND `ram_model_alloc > 256 MiB` (via Ollama `/api/ps` or `llama.cpp` log) | **PCIe Memory Spill Detected:** Model layers or KV cache exceeded VRAM capacity and spilled into system RAM, bottlenecking generation speed to system RAM bandwidth. | Reduce context window from $C$ to $C/2$, enable `Q8_0` KV cache quantization, or switch to a smaller quantization (e.g., `Q4_K_M`). |
| `DIAG-02` | `gpu_utilization_pct < 15%` AND `cpu_utilization_pct > 70%` when GPU is present | **CPU Fallback Active:** The runtime is executing inference on the CPU instead of your GPU. | Check that your runtime build supports your GPU backend (`CUDA`/`ROCm`/`Vulkan`) and set `n_gpu_layers = 99`. |
| `DIAG-03` | `TTFT > 3000ms` on warm run with prompt $> 2048$ tokens | **Slow Prompt Evaluation:** Flash Attention is disabled or micro-batch size (`ubatch`) is too small for long prompts. | Enable `--flash-attn` and test `--ubatch-size 512` in a Controlled Comparison. |
| `DIAG-04` | `ITL_p95 > 2.5 * ITL_median` | **Generation Jitter / Thermal or Swap Throttling:** High variance between tokens indicates system swap activity or laptop GPU thermal/power throttling. | Check AC power state (`upower`) or reduce context size to eliminate swap pressure. |

---

## 8. Local SQLite Storage Schema (`workstation-core::db`)

Database file location: `~/.local/share/local-ai-workstation/workstation.db` (configured with `PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;`).

```sql
-- 1. Hardware Snapshots
CREATE TABLE IF NOT EXISTS hardware_profiles (
    profile_hash TEXT PRIMARY KEY,
    hostname TEXT NOT NULL,
    cpu_model TEXT NOT NULL,
    system_ram_total_bytes INTEGER NOT NULL,
    gpus_json TEXT NOT NULL,               -- Serialized Vec<GpuDeviceProfile>
    primary_backend_json TEXT NOT NULL,    -- Serialized ComputeBackend
    created_at_unix_ms INTEGER NOT NULL
);

-- 2. Cached Catalog Artifacts (Offline-First Support)
CREATE TABLE IF NOT EXISTS catalog_artifacts (
    artifact_id TEXT PRIMARY KEY,
    family_name TEXT NOT NULL,
    release_repo TEXT NOT NULL,
    filename TEXT NOT NULL,
    format TEXT NOT NULL,                  -- 'gguf', 'safetensors', etc.
    quantization TEXT NOT NULL,
    file_size_bytes INTEGER NOT NULL,
    sha256 TEXT,
    block_count INTEGER,
    head_count_kv INTEGER,
    head_dim INTEGER,
    native_context_length INTEGER,
    license TEXT,
    local_path TEXT,                       -- Populated if downloaded or found via deduplicator
    updated_at_unix_ms INTEGER NOT NULL
);

-- 3. Reproducible Runtime Launch Profiles
CREATE TABLE IF NOT EXISTS runtime_profiles (
    profile_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    engine_id TEXT NOT NULL,               -- 'ollama', 'llamacpp', 'lmstudio', 'vllm'
    ownership_mode TEXT NOT NULL,          -- 'attached' | 'managed'
    artifact_id TEXT REFERENCES catalog_artifacts(artifact_id),
    bind_host TEXT NOT NULL DEFAULT '127.0.0.1',
    bind_port INTEGER NOT NULL,
    context_length INTEGER NOT NULL,
    n_gpu_layers INTEGER NOT NULL,
    kv_cache_type_k TEXT NOT NULL DEFAULT 'f16',
    kv_cache_type_v TEXT NOT NULL DEFAULT 'f16',
    flash_attention INTEGER NOT NULL DEFAULT 1,
    ubatch_size INTEGER NOT NULL DEFAULT 512,
    extra_args_json TEXT NOT NULL DEFAULT '[]',
    created_at_unix_ms INTEGER NOT NULL
);

-- 4. Empirical Benchmark Runs
CREATE TABLE IF NOT EXISTS benchmark_runs (
    run_id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL REFERENCES runtime_profiles(profile_id),
    artifact_id TEXT NOT NULL REFERENCES catalog_artifacts(artifact_id),
    hardware_profile_hash TEXT NOT NULL REFERENCES hardware_profiles(profile_hash),
    workload_id TEXT NOT NULL,             -- 'coding_7b_std', 'long_ctx_8k', etc.
    is_cold_start INTEGER NOT NULL,
    cold_load_ms REAL,
    ttft_ms REAL NOT NULL,
    prompt_tokens INTEGER NOT NULL,
    prompt_eval_tps REAL NOT NULL,
    generated_tokens INTEGER NOT NULL,
    generation_tps REAL NOT NULL,
    itl_median_ms REAL NOT NULL,
    itl_p95_ms REAL NOT NULL,
    peak_vram_bytes INTEGER NOT NULL,
    peak_ram_bytes INTEGER NOT NULL,
    offloaded_layers_pct REAL NOT NULL,
    diagnostics_json TEXT NOT NULL,        -- Triggered DIAG-* rules
    created_at_unix_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_bench_artifact_hw 
ON benchmark_runs(artifact_id, hardware_profile_hash);
```

---

## 9. Security, Packaging & GNOME Integration Constraints

### 9.1 Native vs. Flatpak Packaging Architecture
- **Primary Development & Power-User Target:** Native Linux package (`.deb`, `.rpm`, Arch `PKGBUILD`, Nix flake) and AppImage/tarball so the application has direct, unthrottled access to `/sys/class/drm`, `libnvidia-ml.so.1`, local ports, and user processes.
- **Flatpak Compatibility Design:**
  - The UI (`workstation-app`) communicates with runtimes over localhost HTTP (`--share=network`) and reads GPU info via `/dev/dri` (`--device=dri`).
  - For spawning managed `llama-server` processes or inspecting host processes from inside a Flatpak sandbox, implement an optional D-Bus bridge (`org.gnome.LocalAIWorkstation.HostAgent`) running as a rootless `systemd --user` unit on the host, rather than requesting blanket `org.freedesktop.Flatpak.Development` permissions that violate Flathub policy.

### 9.2 Network & Local-Only Privacy Guarantees
- **Loopback Binding Default:** Every managed runtime launched by the application binds strictly to `127.0.0.1`. Exposing an endpoint on `0.0.0.0` (LAN) requires toggling an explicit security switch with a warning banner.
- **Zero Telemetry:** The application sends zero usage analytics or crash reports. Outbound requests occur *only* when querying Hugging Face / Ollama catalogs or downloading user-requested model/runtime artifacts.
- **Credential Storage:** Hugging Face access tokens (for gated models like Llama 3 or Gemma) are stored exclusively via the freedesktop Secret Service API (`oo7` Rust crate / GNOME Keyring), never in plaintext SQLite or config files.

---

## 10. Versioned Delivery Roadmap & Milestone Acceptance Criteria

### Milestone M0: Feasibility, Foundation & Vertical Slice (Immediate Focus)
**Objective:** Prove the end-to-end Rust + GTK4 + Tokio architecture, hardware probing, Ollama discovery, static fit calculation, and single-run benchmark storage on a real Linux GNOME machine.

**M0 Implementation Checklist (Agent Execution Plan):**
1. [x] **Step 1 — Workspace & GTK4 Shell:** Initialize Cargo workspace with `workstation-core`, `workstation-runtimes`, `workstation-bench`, and `workstation-app`. Create `AdwApplicationWindow` with the 4-view navigation sidebar and Tokio-to-GLib async event bridge.
2. [x] **Step 2 — Hardware Profiler:** Implement `HardwareProfile::detect()` in `workstation-core` supporting NVIDIA (`nvml-wrapper`), AMD (`sysfs` VRAM + GTT), and Linux `/proc/meminfo` + `/proc/cpuinfo`. Render hardware cards in View 1 (*Overview*).
3. [x] **Step 3 — Ollama Attached Adapter:** Implement `OllamaAdapter` in `workstation-runtimes` probing `http://127.0.0.1:11434/api/version`, `/api/tags`, and `/api/ps`. Display connected status and installed models in View 3 (*Runtimes*).
4. [x] **Step 4 — Static Memory Fit Calculator:** Implement the 4-part memory formula ($M_{\text{weights}} + M_{\text{KV}} + M_{\text{buffers}} + M_{\text{overhead}}$) and `llmfit` evaluation in `workstation-core::fit`. Display `Comfortable` / `Constrained` / `Not Recommended` badges with memory breakdown tooltips.
5. [x] **Step 5 — Vertical Slice Benchmark & SQLite Persistence:** Implement a single-button "Run Quick Benchmark" on any installed Ollama model, stream tokens, compute TTFT and Decode $t/s$, sample VRAM via NVML/sysfs, save to `~/.local/share/local-ai-workstation/workstation.db`, and display the `Benchmark Verified` badge on restart.

**Implementation evidence (2026-10-26):** The five feature areas now have code and
reference checks, including Qwen2/CUDA conditional memory classes and an exact
manifest/hardware `Benchmark Verified` indicator restored in a separate process.
See [M0 acceptance](../03-development/m0-acceptance.md),
[decision 0012](../05-decisions/0012-qwen2-gpu-fit.md), and
[decision 0013](../05-decisions/0013-benchmark-verification.md). The measured Qwen
result is Constrained at 8K and Not recommended at 32K under current full-residence
budgets. The native session was Hyprland.

**M0 acceptance amendment (2026-10-26):** Explicit project instruction accepted the
measured GTK/CUDA reference results and their documented limits for M0 sign-off.
The accepted reference baseline uses Hyprland and preserves the actual 8K/32K fit
classifications. GNOME-session validation is a follow-up, not a claimed test result.
The original definition below is retained as the product target; this dated amendment
records the approved M0 acceptance scope. Earlier implementation decisions, including
[0004](../05-decisions/0004-static-memory-fit.md), remain in effect: the calculator for
supplied metadata is implemented; `llmfit-core` integration was evaluated and deferred.
M0 completion does not merge feature PRs or skip `develop → staging → main`.

**M0 Definition of Done:**
- `cargo test --workspace` passes all unit tests for memory fit calculations and mock Ollama HTTP responses.
- Launching `local-ai-workstation` on the reference machine (e.g., Linux + RTX 4050 6GB VRAM + 16GB RAM) accurately reports total/free VRAM, detects a running Ollama instance, explains why a 7B Q4_K_M model fits comfortably at 8K context but becomes constrained at 32K context, runs a live inference benchmark, and persists the result across app restarts.

---

### Milestone M1: Model Explorer, Hugging Face Catalog & Deduplicator
- Implement asynchronous Hugging Face Hub API search (`models?filter=gguf&sort=downloads`) with GGUF header metadata parsing.
- Implement the 3-tier normalized browser (`Family -> Release -> Artifact`) in View 2.
- Build the resumable artifact downloader and SHA256 blob deduplicator across `~/.ollama/models/blobs` and `~/.cache/huggingface`.
- Integrate `oo7` Secret Service for gated Hugging Face tokens.

### Milestone M2: Hybrid Runtime Manager & Managed `llama.cpp`
- Implement portable `llama-server` release downloading and process supervision (`Managed` mode).
- Implement full lifecycle state machine (`NotInstalled` $\rightarrow$ `Ready` $\rightarrow$ `Busy` $\rightarrow$ `Stopped` / `Failed`) with ring-buffered stdout/stderr log viewer dialog.
- Enforce strict `Attached` vs. `Managed` ownership guards.
- Add Developer Profile Exporter (1-click config generation for Continue.dev, Zed, OpenCode, and `systemd --user` service units).

### Milestone M3: Full Benchmark Lab & Bottleneck Diagnostics
- Build multi-configuration A/B experiment runner in View 4 (*Benchmark Lab*).
- Implement cold-start vs. warm-run separation and 100ms background telemetry sampling.
- Implement the automated Diagnostic Advisor (`DIAG-01` through `DIAG-04`) with guided 1-click "Run Controlled Comparison" experiments (e.g., comparing `f16` KV cache vs `q8_0` KV cache).

### Milestone M4: Expanded Integrations (LM Studio, vLLM) & Public Beta
- Add `LmStudioAdapter` and `VllmAdapter` (including Prometheus `/metrics` scraping for vLLM).
- Validate across the three mandatory hardware matrices:
  1. Discrete NVIDIA GPU Laptop/Desktop (e.g., RTX 4050 6GB VRAM).
  2. AMD Radeon / Ryzen APU Unified Memory System (`amdgpu` VRAM + GTT).
  3. CPU-Only x86_64 Linux System (`AVX2`).
- Complete GNOME HIG polish, keyboard navigation shortcuts (`Ctrl+1..4`, `Ctrl+K` quick search), AppStream metadata, and public beta packaging.
