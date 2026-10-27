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

Read the memory figure with care: vLLM reserves a fixed share of VRAM at start (here 80%), so it shows the
reservation, not what the model needs. Eager mode (no CUDA graphs) was used by the owner to fit a 6 GiB card and
lowers speed.

## Not measured yet

| Runtime | Model | Blocker |
| --- | --- | --- |
| LM Studio | Qwen2.5-1.5B-Instruct and Qwen2.5-Coder 7B Q4_K_M | Its local server was off, and the 6 GiB GPU was occupied by vLLM |
| Ollama | Qwen2.5-Coder 7B Q4_K_M | The GPU was occupied by vLLM; a 7B Q4 model needs about 4.7 GiB |

The 1.5B and 7B models are not directly comparable, and vLLM cannot serve the 7B GGUF model yet (its GGUF support
is experimental), so a like-for-like three-way table is not possible on this machine. The planned comparison is
vLLM against LM Studio on the 1.5B model, and LM Studio against Ollama on the 7B Q4_K_M model.
