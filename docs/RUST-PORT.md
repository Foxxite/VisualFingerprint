# Rust port: status and guide

Branch `rust-port`. The goal is one engine that runs on Windows, macOS, Linux and Android.
The Python code in `src/vfp/` stays the **reference**. The Rust code must give the same results.

## Status

| Part | State |
|---|---|
| Image: mark, align, trace | done, matches Python |
| Audio: mark, align, trace | done, matches Python |
| Video: mark, align, trace | done, matches Python (scaled video: scores 2-6% lower) |
| Workspace (SQLite, `master.key`) | done, same files as Python. Either program opens the other's workspace. |
| `vfp` command line | done: `init`, `receivers`, `asset`, `mark`, `trace` (no `gui`) |
| Backend trait, server, apps | not started |

## Build and run

You need Rust 1.82 or newer. For audio and video you also need `ffmpeg` and `ffprobe` on `PATH`
(libx264, libmp3lame and libopus enabled).

```bash
cargo build --release -p vfp-cli          # makes target/release/vfp
target/release/vfp init
target/release/vfp receivers add alice bob carol
target/release/vfp asset add "film.mp4" --name film
target/release/vfp mark film --all --out marked
target/release/vfp trace "leaked.mp4"
```

The commands and the output match the Python `vfp` command.

## Layout

```
crates/vfp-core/        the engine (library)
  src/rng.rs            NumPy-exact random numbers (SeedSequence + PCG64)
  src/keys.rs tardos.rs key derivation, Tardos code, scores, thresholds
  src/image/            DCT, layout, embed, detect, alignment (features, RANSAC, ECC), warp
  src/audio/            STFT, offset search, embed, detect, FFmpeg pipes
  src/video/            per-segment marks, time-offset search, embed, detect, FFmpeg pipes
  src/workspace.rs      database and key
  src/pipeline.rs       plan, add_asset, mark_for_receiver, trace_leak, report_text
  examples/collusion.rs collusion experiment (Rust version of vfp.eval.collusion_eval)
  tests/                see below
crates/vfp-cli/         the `vfp` command
tools/dump_golden.py    writes tests/golden/golden.json from the Python code
```

## Tests

```bash
cargo test                      # all tests (the real-photo test needs test-media/ and takes 2 minutes)
python tools/dump_golden.py     # only when the Python reference changes
```

| Test file | What it checks |
|---|---|
| `golden.rs`, `golden_audio.rs`, `golden_video.rs` | Rust values equal Python values (random streams, codes, layouts, DCT, STFT, filters, offsets, resizers) |
| `register.rs` | alignment on a made-up picture: crop, scale, rotation, then mark and trace |
| `pipeline.rs` | workspace and image pipeline |
| `audio_pipeline.rs`, `video_pipeline.rs` | mark, re-encode with FFmpeg, trim, trace. They skip when FFmpeg is missing. |
| `real_photos.rs` | real photos from `test-media/` (skipped when the folder is missing). Set `VFP_TEST_MEDIA` to use another folder. |

`VFP_TRACE=1` prints timing and candidate offsets while tracing.

## Compatibility rules

- The random streams must stay bit-exact with NumPy (`rng.rs`). If they change, old marked files cannot be traced.
- Do not change a layout, a pattern, or a density without a new workspace version (the same rule as in `HANDOFF.md`).
- A Python workspace and a Rust workspace are the same thing. Tested both ways with real photo and song files.

## Measured (this Windows PC, release build)

**Images** (1.2 MP photo and two 12 MP photos). Every attack traced: JPEG q60 and q30, scale 50%, crop 70%,
rotate 2 degrees, scale 60% + JPEG, crop + scale. Scores are within about 0.03 bit accuracy of Python.
A 12 MP photo needs 6-8 s to align (Python on a 1.2 MP photo: 0.5 s).

**Audio** (173 s song). Marking takes about 1.9 s per copy. 11 of 12 attacks traced, with scores equal to Python's to one decimal.
Speed change (+3%) fails in both programs, as expected.

**Video** (1276x720, 19.7 s). Marking about 98 frames per second. 11 of 11 attacks traced.
Nine scores equal Python's; the two scale-down attacks are 2-6% lower. 4K marking about 14 frames per second.
The 240x240 clip and an 8 s 4K excerpt also trace (untouched, CRF 30, scale, trim).

**Collusion** (`cargo run --release --example collusion -- ...`). No false accusation in any trial.

| Media | Strategies | Group sizes | Result |
|---|---|---|---|
| Photo, 1600 symbols | average, min, max, median, pixel mix, tile mix | 3, 4 | 72 of 72 |
| Song, 173 s | average, interleave, min-abs | 2, 3, 4, 6 | 48 of 48 |
| Video, 20 s | average, interleave, frame mix | 2, 3 | 18 of 18 |

## Known gaps

- `ffmpeg` and `ffprobe` are needed for audio and video. A phone has neither. Android needs another decoder and encoder.
- Pure-Rust alignment is not bit-exact with OpenCV. It is slightly weaker on strong downscales.
- A fake test clip (`testsrc2` with grain) picks a wrong time offset after a scale-down plus trim. Python is only just right on it. Real footage was fine.
- The video's sound track is copied unchanged (as in Python).
- Audio output formats: wav, flac, mp3, m4a, ogg. Audio is held in memory.
- Not built or tested yet on macOS, Linux or Android. The CI workflow `rust.yml` will show the first result.
- No GUI yet.

## Next steps

1. `Backend` trait with `LocalBackend` and `RemoteBackend`, then `vfp-server` (Docker). See the plan.
2. Tauri desktop app.
3. Android: images first; audio and video need a decoder that does not use the `ffmpeg` program.
4. Choose a licence (x264 and FFmpeg builds may decide it).
