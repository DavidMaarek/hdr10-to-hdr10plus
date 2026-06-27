# mkvhdr10plus — HDR10 → HDR10+ writer

Generates HDR10+ dynamic metadata (Profile B) from an HDR10 source and injects
it into the original HEVC stream **without re-encoding**. Implements
[`HDR10plus_writer_spec.md`](./HDR10plus_writer_spec.md).

## Pipeline

```
HDR10 MKV
  └─ decode frames (libav) ─► per-frame RGB-linear measurement ─► scene grouping ─► metadata.json
                                                                          │
flux HEVC (ffmpeg -c copy) ─► hdr10plus_tool inject -j metadata.json ─► injected.hevc
                                                                          │
                                              mkvmerge (remux audio/subs) ─► output HDR10+ MKV
```

No re-encode: image quality is untouched; only HDR10+ SEI are added.

## Requirements

Decoding/measurement use the FFmpeg libraries (linked via `ffmpeg-next`). The
end-to-end chain additionally needs these CLI tools on `PATH`:

- `ffmpeg` (HEVC bitstream extraction, and remux fallback) — **required**
- `hdr10plus_tool` (SEI injection / verification) — **required**
- `mkvmerge` (remux) — **strongly recommended**. If absent, the remux falls
  back to `ffmpeg`, but `ffmpeg -c copy` cannot reliably retime a raw HEVC
  stream with frame reordering (B-frames) and often fails with
  "unknown timestamp". Keep `mkvmerge` on `PATH` for dependable muxing.

`--json-only` needs none of them.

## Usage

```bash
# Full end-to-end conversion (writes <input>.HDR10plus.mkv next to the source)
mkvhdr10plus input.mkv

# Choose the output path and verify the injected metadata afterwards
mkvhdr10plus input.mkv -o out.HDR10plus.mkv --verify

# Only generate the metadata JSON (no external tools required)
mkvhdr10plus input.mkv --json-only --json-out metadata.json

# Faster analysis on 4K (half-res, every 2nd frame)
mkvhdr10plus input.mkv --downscale 2 --sample-rate 2
```

### Key options

| Flag | Default | Meaning |
|---|---|---|
| `-o, --output` | `<stem>.HDR10plus.mkv` | Output MKV path |
| `--json-only` | off | Emit JSON only, skip extract/inject/mux |
| `--json-out <path>` | — | Keep the generated metadata JSON at this path |
| `--target-nits <n>` | `1000` | `TargetedSystemDisplayMaximumLuminance` (Profile B requires non-zero) |
| `--downscale <1\|2\|4>` | `1` | Downscale before measurement |
| `--sample-rate <n>` | `1` | Analyze every Nth frame |
| `--scene-threshold <f>` | `0.35` | Scene-cut sensitivity (chi-squared histogram distance) |
| `--min-scene-length <n>` | `24` | Minimum frames between cuts |
| `--keep-temp` | off | Keep intermediate HEVC/JSON files |
| `--verify` | off | Re-extract injected metadata and report frame count |

## Calibration notes (open items, spec §8)

- Percentile interpolation uses **nearest-rank** over an integer histogram at
  ×100000 resolution. Switching to linear interpolation may shift values by a
  few percent.
- Chroma upsampling is **bilinear** with approximate center siting.
- Measurement currently uses the full active frame (single window,
  `NumberOfWindows = 1`); no black-bar crop is applied.

These are the points to calibrate against a frame-exact reference extraction
(`hdr10plus_tool extract`) per spec §8.
