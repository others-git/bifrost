# STT benchmark harness

Compare speech-to-text models for Bifrost's server-side voice path
(`/api/voice/listen` → an OpenAI-compatible `/v1/audio/transcriptions`
endpoint, Speaches/faster-whisper today) on **speed**, **load**, and
**accuracy** — without rebuilding the kiosk app.

Stdlib Python 3 only (no `pip install`).

## Quick start

```bash
cd scripts/stt-bench

# Baseline against the configured model:
python3 bench.py

# Stable latency (prime the model, then repeat each clip):
python3 bench.py --warmup 1 --repeat 3

# Load test — N concurrent requests (the contention question on a 1-CPU box):
python3 bench.py --concurrency 4 --repeat 5
```

Defaults point at the dev Speaches box (`http://192.168.1.197:11435/v1`,
`Systran/faster-whisper-base.en`). Override with `--endpoint`, `--model`,
`--api-key` (or env `STT_ENDPOINT` / `STT_MODEL` / `STT_API_KEY`).

## Clips + ground truth

Put audio under `clips/` and ground truth in `clips/manifest.tsv`:

```
<relative-wav-path>\t<expected transcript>
```

- A clip **with** a manifest line is scored for accuracy (WER + exact match,
  using the same normalization Bifrost's grammar applies — lowercase, no
  punctuation).
- A clip **without** one is still transcribed for speed/load, just not scored.
- `jfk.wav` (stock whisper sample) ships as a general-English baseline.

### Harvesting real clips off the tablet (no app rebuild)

Run the dev server with `BIFROST_LISTEN_DUMP_DIR` set, then just talk to the
tablet — every command the kiosk uploads to `/api/voice/listen` is saved there
as `clip_<ms>.wav` plus a `clip_<ms>.txt` holding whisper's transcript:

```bash
BIFROST_LISTEN_DUMP_DIR=data/listen-clips DATABASE_URL=sqlite://data/bifrost.db ./target/debug/bifrost
```

Point the bench straight at the capture dir; **edit each `.txt` to what you
actually said** (it starts as whisper's guess, which would bias the comparison),
then re-run:

```bash
python3 bench.py --clips ../../data/listen-clips --warmup 1 --repeat 3
python3 bench.py --clips ../../data/listen-clips --model Systran/faster-whisper-tiny.en --warmup 1 --repeat 3
```

Ground truth precedence is `manifest.tsv` first, then a sibling `<clip>.txt`.

**For accuracy on your home, you need real clips** — synthetic TTS audio is
cleaner than a room mic and gives misleadingly good numbers, and small whisper
models specifically struggle with proper nouns (your room/device names). Record
on a phone (voice memo → export WAV, any rate — whisper resamples), drop them in
`clips/`, and add a manifest line with what you actually said, e.g.:

```
office-lights-on.wav	bifrost turn on the office lights
bedroom-cozy.wav	bifrost make the bedroom cozy
```

## A/B a different model

Pull the candidate on Speaches (the `/` in the id must be URL-encoded), then
point the bench at it:

```bash
# pull tiny.en (≈3× faster than base.en, weaker on proper nouns)
curl -X POST "http://192.168.1.197:11435/v1/models/Systran%2Ffaster-whisper-tiny.en"

python3 bench.py --model Systran/faster-whisper-tiny.en --warmup 1 --repeat 3
```

List installed models: `curl -s .../v1/models`. List pullable:
`curl -s ".../v1/registry?task=automatic-speech-recognition"`.

## Reading the output

- **latency**: mean / median / p90 / p95 / max per request.
- **throughput**: req/s over wall-clock at the given concurrency — watch this
  drop (and latency rise) as concurrency climbs on a CPU-only box; that's the
  queuing cost of racing extra inferences.
- **accuracy**: mean WER (word error rate, 0% = perfect) and exact-match count
  over the clips that have ground truth.
