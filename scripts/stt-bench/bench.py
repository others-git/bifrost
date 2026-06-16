#!/usr/bin/env python3
"""
Local STT benchmark harness for Bifrost's server-side transcription path.

Fires a directory of audio clips at an OpenAI-compatible
`/v1/audio/transcriptions` endpoint (Speaches/faster-whisper today) and reports
**speed**, **load** (throughput under concurrency), and **accuracy** (WER +
exact match vs. ground truth). The point is to compare STT models — e.g.
base.en vs tiny.en — on the *real* deployment hardware without rebuilding the
kiosk app.

Why a script and not a Rust test: this benchmarks an external service + model
weights, not Bifrost code; it belongs out of the cargo build so it can be run
ad hoc against whatever endpoint/model you're evaluating.

Stdlib only (urllib) — no pip install needed.

Usage:
    # Baseline against the configured Speaches model:
    python3 bench.py

    # A/B a different model (pull it on Speaches first — see README):
    python3 bench.py --model Systran/faster-whisper-tiny.en

    # Stable latency: warm up, then repeat each clip:
    python3 bench.py --warmup 1 --repeat 3

    # Load test: 4 concurrent requests, each clip repeated 5x:
    python3 bench.py --concurrency 4 --repeat 5

Clips + ground truth live in ./clips/ with a manifest.tsv:
    <relative-wav-path>\t<expected transcript>
Clips with no manifest line are still transcribed (speed/load) but skipped for
accuracy. See README.md.
"""

import argparse
import json
import mimetypes
import os
import statistics
import sys
import time
import urllib.error
import urllib.request
import uuid
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field

DEFAULT_ENDPOINT = "http://192.168.1.197:11435/v1"
DEFAULT_MODEL = "Systran/faster-whisper-base.en"
HERE = os.path.dirname(os.path.abspath(__file__))


# ── text normalization + WER ────────────────────────────────────────────────
def normalize(text: str) -> str:
    """Lowercase, drop punctuation, collapse whitespace — mirrors how Bifrost's
    voice grammar normalizes a transcript, so WER reflects what actually feeds
    command parsing (a trailing '.' or capitalization shouldn't count as error)."""
    out = []
    for ch in text.lower():
        out.append(ch if (ch.isalnum() or ch.isspace()) else " ")
    return " ".join("".join(out).split())


def word_error_rate(ref: str, hyp: str) -> float:
    """Levenshtein edit distance over words / reference length. 0.0 = perfect."""
    r = normalize(ref).split()
    h = normalize(hyp).split()
    if not r:
        return 0.0 if not h else 1.0
    # classic DP edit distance
    prev = list(range(len(h) + 1))
    for i, rw in enumerate(r, 1):
        cur = [i] + [0] * len(h)
        for j, hw in enumerate(h, 1):
            cost = 0 if rw == hw else 1
            cur[j] = min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + cost)
        prev = cur
    return prev[len(h)] / len(r)


# ── HTTP: multipart POST to /audio/transcriptions ───────────────────────────
def transcribe(endpoint: str, model: str, api_key: str, wav_path: str, timeout: float):
    """POST one clip; return (transcript, latency_seconds). Raises on HTTP error."""
    boundary = "----stt-bench-" + uuid.uuid4().hex
    with open(wav_path, "rb") as f:
        audio = f.read()
    ctype = mimetypes.guess_type(wav_path)[0] or "audio/wav"
    filename = os.path.basename(wav_path)

    parts = []
    parts.append(f"--{boundary}\r\n".encode())
    parts.append(b'Content-Disposition: form-data; name="model"\r\n\r\n')
    parts.append(model.encode() + b"\r\n")
    parts.append(f"--{boundary}\r\n".encode())
    parts.append(
        f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'.encode()
    )
    parts.append(f"Content-Type: {ctype}\r\n\r\n".encode())
    parts.append(audio + b"\r\n")
    parts.append(f"--{boundary}--\r\n".encode())
    body = b"".join(parts)

    req = urllib.request.Request(
        endpoint.rstrip("/") + "/audio/transcriptions",
        data=body,
        method="POST",
    )
    req.add_header("Content-Type", f"multipart/form-data; boundary={boundary}")
    if api_key:
        req.add_header("Authorization", f"Bearer {api_key}")

    t0 = time.perf_counter()
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        raw = resp.read().decode("utf-8", "replace")
    dt = time.perf_counter() - t0
    try:
        text = json.loads(raw).get("text", "").strip()
    except json.JSONDecodeError:
        text = raw.strip()  # response_format=text
    return text, dt


# ── manifest + clip discovery ───────────────────────────────────────────────
@dataclass
class Clip:
    path: str
    name: str
    expected: str | None = None


def load_clips(clips_dir: str) -> list[Clip]:
    manifest = os.path.join(clips_dir, "manifest.tsv")
    expected: dict[str, str] = {}
    if os.path.exists(manifest):
        with open(manifest, encoding="utf-8") as f:
            for line in f:
                line = line.rstrip("\n")
                if not line or line.startswith("#"):
                    continue
                rel, _, exp = line.partition("\t")
                expected[rel.strip()] = exp.strip()
    clips = []
    for root, _, files in os.walk(clips_dir):
        for fn in sorted(files):
            if not fn.lower().endswith((".wav", ".mp3", ".m4a", ".flac", ".ogg")):
                continue
            full = os.path.join(root, fn)
            rel = os.path.relpath(full, clips_dir)
            # Ground truth precedence: manifest.tsv, then a sibling <clip>.txt
            # (the format the server's BIFROST_LISTEN_DUMP_DIR capture writes —
            # edit each .txt to what you actually said, then re-run).
            exp = expected.get(rel)
            if exp is None:
                sidecar = os.path.splitext(full)[0] + ".txt"
                if os.path.exists(sidecar):
                    with open(sidecar, encoding="utf-8") as sf:
                        exp = sf.read().strip()
            clips.append(Clip(path=full, name=rel, expected=exp))
    return clips


# ── run ──────────────────────────────────────────────────────────────────────
@dataclass
class Result:
    clip: str
    latency: float
    transcript: str = ""
    expected: str | None = None
    error: str | None = None
    wer: float | None = None
    exact: bool | None = None


def run(args) -> list[Result]:
    clips = load_clips(args.clips)
    if not clips:
        sys.exit(f"no audio clips found under {args.clips}")

    # Build the work list: each clip repeated `repeat` times.
    work = [c for c in clips for _ in range(args.repeat)]

    def do(clip: Clip) -> Result:
        try:
            text, dt = transcribe(
                args.endpoint, args.model, args.api_key, clip.path, args.timeout
            )
        except urllib.error.HTTPError as e:
            return Result(clip.name, 0.0, error=f"HTTP {e.code}: {e.read()[:200]!r}")
        except Exception as e:  # noqa: BLE001 — surface any transport failure
            return Result(clip.name, 0.0, error=str(e))
        r = Result(clip.name, dt, transcript=text, expected=clip.expected)
        if clip.expected is not None:
            r.wer = word_error_rate(clip.expected, text)
            r.exact = normalize(clip.expected) == normalize(text)
        return r

    # Warmup (loads the model into the server's memory; not counted).
    for _ in range(args.warmup):
        try:
            transcribe(args.endpoint, args.model, args.api_key, clips[0].path, args.timeout)
        except Exception as e:  # noqa: BLE001
            sys.exit(f"warmup failed (endpoint/model reachable?): {e}")

    if args.concurrency > 1:
        with ThreadPoolExecutor(max_workers=args.concurrency) as ex:
            results = list(ex.map(do, work))
    else:
        results = [do(c) for c in work]
    return results


def report(args, results: list[Result], wall: float):
    ok = [r for r in results if r.error is None]
    errs = [r for r in results if r.error is not None]

    print(f"\nendpoint : {args.endpoint}")
    print(f"model    : {args.model}")
    print(f"clips    : {len(set(r.clip for r in results))}  "
          f"requests: {len(results)}  concurrency: {args.concurrency}  "
          f"repeat: {args.repeat}  warmup: {args.warmup}")
    print("-" * 78)
    print(f"{'clip':<28}{'lat(s)':>8}  {'WER':>6}  {'exact':>6}  transcript")
    seen = set()
    for r in results:
        if r.clip in seen:
            continue  # show each clip once (first occurrence) to keep it readable
        seen.add(r.clip)
        if r.error:
            print(f"{r.clip:<28}{'ERR':>8}  {'':>6}  {'':>6}  {r.error}")
            continue
        wer = f"{r.wer*100:.0f}%" if r.wer is not None else "-"
        exact = ("yes" if r.exact else "no") if r.exact is not None else "-"
        tx = (r.transcript[:60] + "…") if len(r.transcript) > 61 else r.transcript
        print(f"{r.clip:<28}{r.latency:>8.2f}  {wer:>6}  {exact:>6}  {tx}")
    print("-" * 78)

    if ok:
        lats = sorted(r.latency for r in ok)
        def pct(p):
            return lats[min(len(lats) - 1, int(p / 100 * len(lats)))]
        print(f"latency  : mean {statistics.mean(lats):.2f}s  "
              f"median {statistics.median(lats):.2f}s  "
              f"p90 {pct(90):.2f}s  p95 {pct(95):.2f}s  max {max(lats):.2f}s")
        print(f"throughput: {len(ok)/wall:.2f} req/s over {wall:.1f}s wall "
              f"(concurrency {args.concurrency})")
        scored = [r for r in ok if r.wer is not None]
        if scored:
            mean_wer = statistics.mean(r.wer for r in scored)
            exact_n = sum(1 for r in scored if r.exact)
            print(f"accuracy : mean WER {mean_wer*100:.1f}%  "
                  f"exact-match {exact_n}/{len(scored)}  "
                  f"({len(ok)-len(scored)} clips had no ground truth)")
        else:
            print("accuracy : no ground-truth manifest entries — speed/load only")
    if errs:
        print(f"errors   : {len(errs)}/{len(results)} requests failed")


def main():
    p = argparse.ArgumentParser(description="STT benchmark harness")
    p.add_argument("--endpoint", default=os.environ.get("STT_ENDPOINT", DEFAULT_ENDPOINT))
    p.add_argument("--model", default=os.environ.get("STT_MODEL", DEFAULT_MODEL))
    p.add_argument("--api-key", default=os.environ.get("STT_API_KEY", ""))
    p.add_argument("--clips", default=os.path.join(HERE, "clips"))
    p.add_argument("--warmup", type=int, default=1, help="uncounted priming requests")
    p.add_argument("--repeat", type=int, default=1, help="times to run each clip")
    p.add_argument("--concurrency", type=int, default=1, help="parallel requests (load)")
    p.add_argument("--timeout", type=float, default=60.0)
    args = p.parse_args()

    t0 = time.perf_counter()
    results = run(args)
    wall = time.perf_counter() - t0
    report(args, results, wall)


if __name__ == "__main__":
    main()
