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
- **vLLM's row is a different model** (1.5B, bf16), so its 55 tok/s must not be compared with the 7B rows. Its memory figure is vLLM's fixed 80% reservation, not what the model needs.
- GPU memory is the whole machine's use while the target ran, including the desktop, so it overstates the runtime itself by a few hundred MiB.
- vLLM ran in eager mode (no CUDA graphs) so that it fits a 6 GiB card; that lowers its speed.
- Raw per-run data: [benchmark-data](benchmark-data/).

## Not possible on this machine

| Comparison | Why |
| --- | --- |
| LM Studio on the 1.5B model | The only 1.5B copy here is a safetensors model; LM Studio on Linux reported "No LM Runtime found for model format torchSafetensors" |
| vLLM on the 7B Q4_K_M model | vLLM's GGUF support is experimental; not attempted |

So a three-way, same-model table does not exist yet. A matching 1.5B GGUF file would allow LM Studio and Ollama to join the vLLM row.
