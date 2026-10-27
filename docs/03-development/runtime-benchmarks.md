# Runtime benchmarks on the reference machine

One machine, one prompt, one day. These numbers describe this laptop and these settings, not the runtimes in
general. Machine details are in [test coverage](test-coverage.md). Method: `scripts/compare_runtimes.py` sends the
same chat prompt to each runtime's OpenAI-compatible streaming endpoint on `127.0.0.1`, temperature 0, 160 tokens,
one cold request and then five timed requests. Decode speed is the tokens after the first divided by the time they
took. Token counts are each runtime's own. The script refuses to write output containing the user name, home path,
host name or machine id; no paths or identifiers are in this page.

## Measured

| Runtime | Version | Model | Cold first token | First token (median) | Decode (median) | GPU memory used by whole machine |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| vLLM | 0.32.0 | Qwen2.5-1.5B-Instruct, bf16 safetensors, 4096 context, eager mode | 0.53 s | 0.03 s | 55.2 tok/s (range 55.1–55.3) | 4.7 GiB |

| LM Studio | server on 1234, GGUF runtime | Qwen2.5-Coder 7B Instruct Q4_K_M, all layers requested on GPU | 0.11 s (model preloaded) | 0.03 s | 36.8 tok/s | 4.8 GiB |
| Ollama | local service on 11434 | Qwen2.5-Coder 7B Q4_K_M | 6.03 s (includes loading the model) | 0.03 s | 38.1 tok/s | 4.6 GiB |

How to read this:

- **7B Q4_K_M, LM Studio against Ollama:** decode speed is within about 4% (36.8 against 38.1 tok/s), which is inside what one run on one day can tell apart. Treat them as equal on this machine. The cold figures are not comparable: LM Studio's model was loaded before the timing began, Ollama's was loaded by the first request.
- **vLLM's row above is a different model** (1.5B, bf16), so its 55 tok/s must not be compared with the 7B rows; see the same-model table below.
- GPU memory is the whole machine's use while the target ran, including the desktop, so it overstates the runtime itself by a few hundred MiB.
- vLLM ran in eager mode (no CUDA graphs) so that it fits a 6 GiB card; that lowers its speed.
- Raw per-run data: [benchmark-data](benchmark-data/).

## Same model on every runtime: Qwen2.5-1.5B-Instruct

A 16-bit GGUF file (`Qwen/Qwen2.5-1.5B-Instruct-GGUF`, `fp16`, pinned revision, SHA-256 checked against the
Hugging Face listing) was loaded by llama.cpp, Ollama and LM Studio, so all three run the same weights at the same
precision. vLLM ran the original safetensors in bf16, which is the same 16-bit precision but a different file and
engine. Context 4096 everywhere.

| Runtime | Cold first token | First token (median) | Decode (median) | GPU memory used by whole machine |
| --- | ---: | ---: | ---: | ---: |
| vLLM 0.32.0 (bf16 safetensors, eager mode) | 0.53 s | 0.03 s | **55.2 tok/s** | 4.7 GiB (reservation) |
| llama.cpp (CUDA 12 server build, standalone) | 0.12 s | 0.02 s | 53.1 tok/s | 3.2 GiB |
| Ollama | 5.50 s (includes loading) | 0.02 s | 53.2 tok/s | 3.2 GiB |
| LM Studio | 2.38 s (loaded on first request) | 0.03 s | 51.1 tok/s | 3.3 GiB |

All four are within about 8% of each other. vLLM is fastest, LM Studio slowest, and llama.cpp and Ollama are tied
(Ollama runs llama.cpp underneath). One run on one day cannot rank differences this small with confidence.

Caveats:

- The llama.cpp row is a llama.cpp CUDA server binary that happened to be on this machine (the one bundled with LM
  Studio), started by hand on loopback. It is **not** Kvlm's managed llama.cpp install: that download from GitHub
  ran at about 14 KB/s here and was abandoned, so the managed CUDA install is still untested. The llama.cpp
  numbers show the engine, not Kvlm's supervisor.
- The llama.cpp cold figure has no model-load time (the file was read before the first request was sent to a ready
  server); the Ollama and LM Studio cold figures include loading.
- Models created for the test (an Ollama model and an LM Studio link) were removed afterwards.

## Not measured

| Comparison | Why |
| --- | --- |
| vLLM on the 7B Q4_K_M model | vLLM's GGUF support is experimental; not attempted |
| Kvlm's own managed llama.cpp on GPU | The release download was too slow to finish; retry on a faster link |
| Anything through the Kvlm app UI | Measurements came from `scripts/compare_runtimes.py` |
