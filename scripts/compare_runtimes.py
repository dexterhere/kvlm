# SPDX-License-Identifier: GPL-3.0-or-later
"""Time one prompt against local runtimes through their OpenAI-compatible streaming endpoint.

    python3 scripts/compare_runtimes.py --out DIR \
        --target vllm=8000:qwen2.5-1.5b --target ollama=11434:qwen2.5-coder:7b

Each target is NAME=PORT:MODEL. Requests go to 127.0.0.1 only. The same prompt, token cap and
temperature are used everywhere. Per target it records one cold-start request, then --runs timed
requests: time to first token, decode speed (tokens after the first, divided by the time they took),
total time and the peak GPU memory used on the machine while the target ran. Runtimes keep their own
tokenizers, so token counts are each runtime's own report, not a shared unit.

Output is DIR/results.json and DIR/results.md. The machine block holds only the CPU and GPU model,
core count, RAM and VRAM size. Before writing, the output is checked for the user name, the home
directory, the host name and the machine id, and the script refuses to write if any appears.
"""

import argparse
import json
import os
from pathlib import Path
import platform
import socket
import statistics
import subprocess
import threading
import time
import urllib.error
import urllib.request

PROMPT = (
    "Explain in about 120 words how the key-value cache affects memory use when a language "
    "model runs with a long context."
)


def post_stream(port, model, max_tokens):
    body = json.dumps(
        {
            "model": model,
            "messages": [{"role": "user", "content": PROMPT}],
            "max_tokens": max_tokens,
            "temperature": 0,
            "stream": True,
            "stream_options": {"include_usage": True},
        }
    ).encode()
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/v1/chat/completions",
        body,
        {"Content-Type": "application/json"},
    )
    start = time.perf_counter()
    first = None
    chunks = 0
    usage = None
    with urllib.request.urlopen(request, timeout=300) as response:
        for raw in response:
            line = raw.decode().strip()
            if not line.startswith("data:") or line == "data: [DONE]":
                continue
            event = json.loads(line[5:])
            if event.get("usage"):
                usage = event["usage"].get("completion_tokens")
            choices = event.get("choices") or []
            if choices and choices[0].get("delta", {}).get("content"):
                chunks += 1
                if first is None:
                    first = time.perf_counter()
    end = time.perf_counter()
    if first is None:
        raise RuntimeError("no tokens came back")
    tokens = usage or chunks
    decode = (end - first) or 1e-9
    return {
        "ttft_s": first - start,
        "total_s": end - start,
        "tokens": tokens,
        "decode_tps": (tokens - 1) / decode if tokens > 1 else None,
    }


def gpu_used_mib():
    try:
        out = subprocess.run(
            ["nvidia-smi", "--query-gpu=memory.used", "--format=csv,noheader,nounits"],
            capture_output=True, text=True, timeout=5, check=True,
        ).stdout.split()
        return int(out[0])
    except (OSError, subprocess.SubprocessError, ValueError, IndexError):
        return None


class PeakGpu(threading.Thread):
    def __init__(self):
        super().__init__(daemon=True)
        self.peak = gpu_used_mib()
        self.stop = threading.Event()

    def run(self):
        while not self.stop.wait(0.25):
            used = gpu_used_mib()
            if used is not None and (self.peak is None or used > self.peak):
                self.peak = used


def machine():
    info = {"os": platform.system(), "cores": os.cpu_count()}
    for line in Path("/proc/cpuinfo").read_text().splitlines():
        if line.startswith("model name"):
            info["cpu"] = line.split(":", 1)[1].strip()
            break
    for line in Path("/proc/meminfo").read_text().splitlines():
        if line.startswith("MemTotal"):
            info["ram_gib"] = round(int(line.split()[1]) / 1024 / 1024, 1)
            break
    try:
        name, total = subprocess.run(
            ["nvidia-smi", "--query-gpu=name,memory.total", "--format=csv,noheader,nounits"],
            capture_output=True, text=True, timeout=5, check=True,
        ).stdout.strip().split(", ")
        info["gpu"], info["vram_mib"] = name, int(total)
    except (OSError, subprocess.SubprocessError, ValueError):
        info["gpu"] = "none detected"
    return info


def summarize(runs, key):
    values = [r[key] for r in runs if r[key] is not None]
    if not values:
        return None
    return {"median": statistics.median(values), "min": min(values), "max": max(values)}


def bench(name, port, model, runs, max_tokens):
    cold = post_stream(port, model, max_tokens)
    peak = PeakGpu()
    peak.start()
    timed = [post_stream(port, model, max_tokens) for _ in range(runs)]
    peak.stop.set()
    peak.join()
    return {
        "target": name, "model": model, "max_tokens": max_tokens, "runs": runs,
        "cold_ttft_s": cold["ttft_s"],
        "ttft_s": summarize(timed, "ttft_s"),
        "decode_tps": summarize(timed, "decode_tps"),
        "total_s": summarize(timed, "total_s"),
        "tokens": [r["tokens"] for r in timed],
        "peak_gpu_used_mib": peak.peak,
    }


def markdown(results, info):
    lines = [
        f"Machine: {info.get('cpu')} ({info['cores']} threads), {info.get('ram_gib')} GiB RAM, "
        f"{info['gpu']}" + (f" {info['vram_mib']} MiB" if "vram_mib" in info else ""),
        "",
        "| Runtime | Model | Cold first token (s) | First token, median (s) | Decode, median (tok/s) | Peak GPU used, whole machine (MiB) |",
        "| --- | --- | ---: | ---: | ---: | ---: |",
    ]
    for r in results:
        d, t = r["decode_tps"], r["ttft_s"]
        lines.append(
            f"| {r['target']} | `{r['model']}` | {r['cold_ttft_s']:.2f} | "
            f"{t['median']:.2f} | {d['median'] if d else float('nan'):.1f} | "
            f"{r['peak_gpu_used_mib'] if r['peak_gpu_used_mib'] is not None else 'unknown'} |"
        )
    return "\n".join(lines) + "\n"


def refuse_if_private(text):
    needles = {os.environ.get("USER", ""), str(Path.home()), socket.gethostname()}
    try:
        needles.add(Path("/etc/machine-id").read_text().strip())
    except OSError:
        pass
    for needle in needles:
        if len(needle) >= 3 and needle in text:
            raise SystemExit("refusing to write: the output contains a private identifier")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--target", action="append", required=True, metavar="NAME=PORT:MODEL")
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--max-tokens", type=int, default=160)
    args = parser.parse_args()

    results = []
    for spec in args.target:
        name, rest = spec.split("=", 1)
        port, model = rest.split(":", 1)
        try:
            results.append(bench(name, int(port), model, args.runs, args.max_tokens))
        except (urllib.error.URLError, RuntimeError, OSError) as error:
            print(f"{name}: skipped ({error})")
    info = machine()
    document = {"machine": info, "prompt": PROMPT, "results": results}
    text = json.dumps(document, indent=2)
    md = markdown(results, info)
    refuse_if_private(text + md)
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "results.json").write_text(text + "\n")
    (args.out / "results.md").write_text(md)
    print(md)


if __name__ == "__main__":
    main()
