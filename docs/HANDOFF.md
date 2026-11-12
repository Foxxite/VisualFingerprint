# VisualFingerprint: handoff

Written 2026-11-12. This document lets a new developer (or a new AI session) continue the work without
the earlier conversation. Read it first, then `README.md` (the user guide).

---

## 1. What this project is

A tool that gives each receiver a copy of a media file with a **hidden, receiver-specific code**.
If a copy leaks, the tool finds out whose copy it was.

- Media: images, video, audio.
- The mark must survive re-encoding, compression, resizing, cropping, trimming, and as far as possible screen recording.
- It must not visibly lower quality.
- It must resist **collusion** (several receivers blend their copies).
- Run mode: small, offline, up to about 1,000 receivers. Command line and desktop app.

The user asked for robustness "also for short clips". That drove v1.1 (section 7).

## 2. Status at a glance

| Item | State |
|---|---|
| Version | 0.1.1 (`pyproject.toml`, `src/vfp/__init__.py`) |
| Branch | `main` |
| Tags | `v1` (`d1dd075`), `v1.1` (`b56b7c7`). Both are on the remote. |
| Commits after `v1.1` | `ad4082c` launcher, `c28c9e4` storage fix, `e17e5a5` CI secret scan (made by the owner) |
| Remote | `origin` = `https://github.com/Foxxite/VisualFingerprint.git`. `main` is in sync. |
| Tests | 50, all pass (about 40 s). Run `pytest` from the repo root. |
| Size | about 2,800 lines of source in `src/vfp/` |
| Working tree | clean at the time of writing, except this document, `docs/experiments/`, and a one-line README edit (not committed) |

**Not verified:** the GUI was only run headless (offscreen Qt). A person has not used it on screen yet.
Image quality and sound quality were measured (PSNR, SSIM, VMAF, SNR), not judged by eye or ear.

## 3. Set up and run

Needs Python 3.11+ (developed on 3.14.6) and **FFmpeg and ffprobe on `PATH`**.

```bash
python -m venv .venv
.venv\Scripts\pip install -e ".[gui,dev]"
.venv\Scripts\python -m pytest -q          # 50 tests
start-gui.bat                              # desktop app (Windows)
```

Command-line walk-through:

```bash
vfp init                 # asks for a passphrase (encrypted). --plain skips it
vfp receivers add alice bob carol
vfp asset plan  "film.mp4"                 # shows symbols, density, guaranteed group size
vfp asset add   "film.mp4" --name film
vfp mark film --all --out marked
vfp trace "leaked.mp4"                     # add --json for machine output
```

Library versions in use: PySide6 6.12.0, numpy 2.5.4, scipy 1.18.1, opencv 5.0.0, FFmpeg 7.1 (full build, has libvmaf).

## 4. How it works

### Data flow

```
REGISTER   original file --copy--> workspace/originals/<id>_<name>
                      \--> assets table: kind, params, m (code length), max_receivers, eps, sha256

MARK       receiver -> user_index (issues table) -> Tardos codeword (m bits)
           bits -> embedder -> marked copy (+ one extra encode: x264 / mp3 / jpg / png)

TRACE      leaked file + original
           -> align (time offset, scale, crop, rotation)          [register / sync / align]
           -> read one soft value (z) per code symbol              [detect_*]
           -> bits + confidence = clip(|z|/3, 0, 1)
           -> Tardos score for every issued receiver
           -> threshold = sqrt(sum conf^2) * isf(eps / max_receivers)   [exact threshold]
           -> verdict: LEAKER / POSSIBLE COLLUSION / INCONCLUSIVE
```

### Code map (`src/vfp/`)

| Module | Role |
|---|---|
| `codes/tardos.py` | Tardos code (symmetric Škorić score). Codewords come from `(seed, user_index)`, so no table is stored. `required_length()`, `accusation_threshold()`. |
| `codes/keys.py` | Master key file, HMAC seed derivation. |
| `workspace.py` | SQLite database + master key. Tables: `receivers`, `assets`, `issues`. `original_path()` finds the stored copy by name inside `originals/`. |
| `pipeline.py` | `plan_file`, `add_asset`, `mark_for_receiver`, `trace_leak`, `report_text`. **The CLI and GUI both call only this layer.** |
| `perceptual.py` | `block_mask()`: how much mark each 8x8 block can hide (texture and brightness). |
| `image/` | `plane.py` (embed and detect on one luma plane, shared with video), `embed.py`, `detect.py`, `register.py` (ORB + RANSAC + ECC alignment), `common.py` (layout, 16 DCT positions, `usable_slots`). |
| `video/` | `ffio.py` (FFmpeg pipes), `embed.py`, `detect.py`, `sync.py` (thumbnail time offsets), `common.py` (params, working size, `auto_nslots`). |
| `audio/` | `ffio.py`, `embed.py`, `detect.py`, `align.py` (cross-correlation offset), `common.py` (params, layout, `auto_nslots`). |
| `cli.py` | `vfp init / receivers / asset / mark / trace / gui`. |
| `gui/` | PySide6 app: `app.py` (window, start page), `receivers.py`, `mark.py` (add-file dialog, marking), `investigate.py` (trace view, score bars), `common.py` (`Context`, `Worker` thread). |
| `eval/` | Attack suites and metrics: `image_eval`, `video_eval`, `audio_eval`, `collusion_eval`, `attacks`, `metrics`. |

### Embedding methods

- **Image and video:** add a keyed ±pattern to 16 low and mid DCT coefficients (u+v from 2 to 5, u and v up to 4) of each 8x8 luma block. Block strength = base strength × `block_mask`. Each block belongs to one symbol slot by a keyed shuffle.
  - **Video:** time is cut into 0.5 s segments. Each segment carries K symbols. The pattern is **constant inside a segment** (an encoder keeps it cheaply) and changes between segments. Frames above about 720p are marked at about 1280x720 and scaled up (`canon_size`).
- **Audio:** the STFT (2048 samples, 50% overlap) is scaled per cell by `1 + alpha * symbol * pattern`, from 300 Hz to 8 kHz. Segments are 1 s long and carry K symbols each.
- **Detection is informed:** it compares the leak with the original. This is the main source of robustness.

## 5. Key decisions and why

| Decision | Reason |
|---|---|
| Informed (non-blind) detection | The owner always has the original. It gives alignment and a large gain. Cost: you cannot trace a file you never registered. |
| Tardos code with one symbol per slot | Collusion resistance. The code length `m` is the main lever for tracing groups. |
| Private copy of every original inside the workspace | Tracing needs it. Deleting or moving the user's own file must not break anything. |
| Seeds derived from `master.key`; no secret in the database | The database can be backed up and shared without leaking the code. |
| Video marked at about 720p working size, strength scaled by `sqrt(cw/W)` | A 4K source scaled down to 480p lost the mark otherwise. A coarser mark is more visible, so strength is reduced. |
| Time offset confirmed by the watermark, scored against a **control pattern** | Thumbnails alone picked wrong offsets on near-static footage. A wrong offset inflates the raw energy, so the energy with a non-existent segment is subtracted. A "prefer zero shift" prior applies only to offset 0. |
| Audio alignment: whiten, then lowpass to 6 kHz before correlating | Steady low notes made many equal peaks. The top octave is removed by codecs, so it cannot match. |
| Audio: per-frame, per-96-bin gain correction | Compressor and EQ attacks otherwise destroyed detection. Group size is a trade-off (32 absorbs too much mark). |
| Exact accusation threshold (v1.1) | The old threshold assumed every symbol was fully reliable. See section 7. |
| Density adapts to clip length (v1.1) | Short clips had too few symbols. See section 7. |
| `Workspace.original_path()` resolves by file name inside `originals/` | Old workspaces stored relative or absolute paths that broke when the working folder or the workspace moved. |

## 6. Defaults (where they live)

| Setting | Value | File |
|---|---|---|
| Image strength | 3.0 | `image/common.py` (`ImageParams`) |
| Image symbols | as many as fit, at least 12 blocks each; plan asks for `max(1024, target)` | `image/common.py` (`MIN_BLOCKS_PER_SLOT`), `pipeline.plan_file` |
| Video | segment 0.5 s, K = 64 default, cap 256, strength 3.0, CRF 16, preset medium | `video/common.py` |
| Video sync | 60-frame head, every 2nd frame sampled, up to 5 offset candidates (top 4 plus offset 0) | `video/detect.py`, `video/sync.py` |
| Audio | segment 1 s, K = 32 default, cap 256, alpha 0.05, band 300–8000 Hz, gain group 96 bins | `audio/common.py`, `audio/detect.py` |
| Plan inputs | 1000 receivers, group of 3, false-accusation rate 0.001 | `pipeline.add_asset`, `cli.py` |
| Target code length | `ceil(2 pi^2 c^2 ln(n / eps))` = 2,455 for the defaults | `codes/tardos.py` |
| Confidence per symbol | `clip(abs(z) / 3, 0, 1)` | detectors |
| Threshold floor | 25 (about 25 reliable symbols of evidence) | `codes/tardos.py` (`MIN_EVIDENCE`) |

## 7. v1.1: what changed and the evidence

Research (see section 11) suggested we were losing information on short clips. Two experiments on a 10 s audio clip
(384 symbols at the old default) drove the changes. The scripts are in `docs/experiments/` (section 10).

1. **Exact threshold.** Old: `sqrt(m) * isf(...)`. New: `sqrt(sum of w^2) * isf(...)`, where `w` is the confidence of each symbol.
   Against groups of 3: success 43% -> about 59% (108 trials). No false accusations. A simulation of innocent receivers confirms the error rate holds (it is slightly conservative).
2. **Density that fits the clip.** `plan_file` asks `required_length()` how many symbols the plan needs, then `auto_nslots()` packs them, capped by measurement.

| Audio symbols per second | Symbols (10 s) | Bit accuracy, 3 harsh attacks | Groups of 3 caught |
|---|---|---|---|
| 32 | 384 | 0.94 | 58% |
| 64 | 768 | 0.89 | 100% |
| 128 | 1536 | 0.83 | 100% |
| 256 | 3072 | 0.78 | 100% |

Video (5 s, 720p): all four harsh single-leaker attacks were traced at K up to 256 per segment. At 512, one failed. Hence the cap of 256.
At v1.1 defaults: 10 s audio, groups of 2, 3, 4: **72 of 72** trials caught. Photo (1,600 symbols), groups of 3 and 4: **60 of 60**. No false accusations anywhere.

**Negative result worth remembering:** weighting symbols by the size of `z` (soft scoring) gave **no** gain. Our `z` values are poorly calibrated (median 71, 90th percentile about 487 on audio), so they saturate. An early 73% figure was noise.

## 8. What was measured (summary)

Full tables are in `README.md`. Short version:

- **Images:** JPEG q30, scale 50%, crop 70%, rotate 2°, blur, noise, brightness/contrast: traced. PSNR 44–49 dB. Weak on flat graphics and tiny images.
- **Video (720p, 240p, 4K):** x264 CRF 23–34, x265, VP9, scale to 480p, crop, 15 fps, blur, noise, trim, mild screen-recording simulation: traced. VMAF cost about 1 point at 720p, about 3.6 at 4K.
- **Audio:** MP3 192k to 64k, AAC 96k and 48k mono, Opus 64k, resample to 22 kHz and 16 kHz, volume, lowpass, compressor + EQ, mild echo, trim: traced. SNR about 33 dB. **Fails on speed or pitch change.**
- **Collusion:** averaging, min, max, median, pixel/tile mixing, random splicing, frame mixing: no false accusations in any trial.

**Sample sizes are small.** Collusion cells have 4–8 trials (up to 54 for a few). Most used 8–12 receivers. Treat the numbers as clear signals, not tight statistics. Randomness note: the master key is random per workspace, so scores vary between runs.

## 9. Known limits and open problems

1. **Speed and pitch changes** (audio) break alignment. Detection finds no evidence and says so (low alignment score).
2. **Heavy screen recording** (strong crop, rotation, perspective) is not handled. Only mild cases.
3. **Time offset of near-static, heavily edited video** is the weakest point. There is a prior toward offset 0.
4. **Video audio track is copied unchanged.** A video's sound carries no mark.
5. **Not blind.** The original is required.
6. **Regeneration attacks** (diffusion-based re-drawing, learned denoisers) were **not tested**. The literature says pixel-level invisible marks, ours included, can be removed this way. See section 11.
7. **Attackers who know the method** (targeted filtering, estimating the mark from many copies) were not tested.
8. **Quality is measured, not judged by a person.** `samples/` (not in git) held a marked clip, a control, and a marked song for A/B checks.
9. **Denser symbols lower per-symbol accuracy** (audio 0.94 -> 0.78 at the cap; video about 0.71 -> 0.64). Total evidence still grows, but harsh single-leaker attacks have less margin.
10. **Marked video files can be much larger** than a low-bitrate source (CRF 16 turned 1.8 MB into 7.4 MB). `--crf` controls this.
11. **Whole file in memory for audio.** Fine for songs, not for hours.
12. **Images lose EXIF** and WebP is saved as PNG.
13. The `guaranteed colluder group` shown by `asset plan` is the cautious theory. Real files did better.
14. **No licence file** in the repository. The owner must choose one.

## 10. Next steps (suggested order)

Based on the research in section 11. None of these is started.

1. **Calibrate detector output into log-likelihood ratios**, then use soft Tardos scoring (Furon and others). Expected to need real calibration data per media type.
2. **q-ary symbols** (3 or 4 patterns per slot) for a shorter code. Reported savings 35% (q=3), 80% (q=10) in the restricted-digit model. Try audio first.
3. **Informed embedding and better masking:** adapt strength per block or cell until a target robustness is reached.
4. **Re-run the density experiments for video and images** with more receivers and trials; add a real colluder-count sweep.
5. **Threat tests:** a learned denoiser or regeneration pass; many-copy averaging; a known-method attacker.
6. **Mark the video's audio track** (`audio/` already works; remux needed in `video/ffio.py` `Encoder`).
7. **Optional learned ID layer** (Video Seal or AudioSeal) for harsh geometry or screen recording. Payload is small (AudioSeal 16 bits), so it complements, not replaces, the Tardos layer. Check the licence text and weights licence first.
8. **Smaller items:** percent progress bar for video in the GUI; chunked audio processing for long files; a hash check of the stored original at trace time (the database already keeps its SHA-256).

Experiment scripts for reproducing the v1.1 evidence are in `docs/experiments/`. Run them **from the repo root**:

```bash
python docs/experiments/exp_soft.py   <workdir>              # soft vs hard scoring (10 s audio, about 5-8 min)
python docs/experiments/exp_slots.py  <workdir>              # audio symbols per second (about 10 min)
python docs/experiments/exp_vslots.py <workdir> [64,128,...] # video symbols per segment (about 12 min)
```

**Each script deletes `<workdir>` first. Give it an empty or throwaway folder.**
They read `test-media\...` (the owner's files, not in git) and set the number of symbols by editing the database row directly,
so they bypass `plan_file`. Re-run the evaluation scripts in `eval/` (see `README.md`) for regression numbers.

## 11. Research sources

- Škorić et al., "Tardos fingerprinting is better than we thought" (about 5x shorter codes): https://www.arxiv.org/abs/cs/0607131
- Laarhoven and de Weger, optimal symmetric Tardos schemes: https://research.tue.nl/nl/publications/optimal-symmetric-tardos-traitor-tracing-schemes-2/
- Laarhoven, nearest neighbor decoding (a summary says optimal length about ½π²c² ln n for the symmetric score; **not verified**): https://arxiv.org/pdf/1902.06196
- Meerwald and Furon, joint decoding and soft outputs: https://arxiv.org/pdf/1104.5616
- Miller, Doërr, Cox, informed embedding: https://unpaywall.org/10.1007%2F3-540-36617-2_2
- AudioSeal (16-bit message, MIT): https://pypi.org/project/audioseal/0.1.3
- Video Seal (payload size and licence text **not confirmed**): https://arxiv.org/pdf/2412.09492
- Zhao et al., regeneration attacks (NeurIPS 2024): https://proceedings.neurips.cc/paper_files/paper/2024/hash/10272bfd0371ef960ec557ed6c866058-Abstract.html
- A/B watermarking standard, ETSI TS 104 002 (DASH-IF): https://dashif.org/news/watermarking-final

I read search summaries and abstracts, not full papers.

## 12. Gotchas and lessons learned

**Tests**
- Run `pytest` from the repo root. Tests import each other as `tests.test_audio`, so `tests/__init__.py` must stay.
- **The master key is random per workspace**, so scores vary between runs. A test that sits near the accusation threshold is flaky. Give tests a wide margin (longer file, milder attack). Run the suite several times after changing detection.
- GUI tests run headless (`QT_QPA_PLATFORM=offscreen`). For readable screenshots set `QT_QPA_FONTDIR=C:\Windows\Fonts`; otherwise text renders as boxes.
- PowerShell's `Start-Process -Wait` also waits for the app the launcher starts. Test the launcher with `WaitForExit(timeout)` on the launcher process only.

**Windows**
- Use `cv2.imdecode(np.fromfile(...))` for images. `cv2.imread` fails on non-ASCII paths (`pipeline.read_image` does this).
- FFmpeg on this machine lives under another user's folder (`C:\Users\dimit\...\ffmpeg-7.1-full_build\bin`). It is on `PATH`, but a fresh machine needs its own install.
- `start-gui.bat` uses `pythonw` (no console). Errors in the app are then invisible; run `vfp gui` in a terminal to see them.
- `ffmpeg` writes the container by file extension: an MP3 stream written to `.m4a` fails (this bit an experiment script once).

**Code**
- Detectors return `z` per symbol, but `z` is **not calibrated** (section 7). Do not build on its absolute size.
- `Workspace` connections must be closed (use `with`). Workers in the GUI open their own handle.
- Segment layouts depend on `(seed, segment, slots)`. **Changing the layout, the pattern, or the density breaks tracing of copies made earlier** (the params are stored per asset, so old assets keep working, but any change to the algorithm itself does not).
- Video capacity uses `+1` segment as slack for frame-count estimates. Slack symbols get zero weight in scoring.
- The eval scripts contain a fixed dummy key (`eval-master-key-...`). It is for evaluation only. It is not a real secret.

**Git**
- Line-ending warnings (`LF will be replaced by CRLF`) are harmless on this machine.
- Owner's machine note: the first `git commit` of the day can fail with "gpg failed to sign the data". Retry the same commit once; do not change signing settings.

## 13. Security and privacy

Encryption has two server modes (Protected, Zero-knowledge) and an encrypted local workspace. Read `docs/SECURITY.md` first.
Open items: `rekey` (Protected to Zero-knowledge), a cache size limit, rollback detection, built-in TLS, and a hand test of the app in Zero-knowledge mode.


- **`master.key` is the secret.** It is created by `vfp init` in the workspace folder. Without it, no leak can be traced. Back it up. Never share it or commit it.
- `.gitignore` excludes `master.key`, `*.key`, `vfp.db`, `.vfp/`, `demo-ws/`, `demo-out/`, `samples/`, `test-media/`, `.venv/`. The index holds no key files (checked at v1 and again today).
- **`demo-ws/` exists locally and holds a real test key.** It is ignored by git. Delete it if it is no longer needed.
- `test-media/` holds the owner's personal files (photos, music, video). They are not in git. Keep it that way.
- The CI workflow `.github/workflows/scan-secrets.yml` (made by the owner) runs TruffleHog on every push and pull request. It uses `trufflesecurity/trufflehog@main`, a moving reference. Pin it to a release tag for a stable build.
- A score is **statistical evidence, not proof.** The report (`report_text`) includes the SHA-256 of the leaked file. Keep the leaked file, the report, and the originals.
- Tell receivers that copies are marked where the law or a contract requires it.

## 14. Git history

```
e17e5a5  Scan for secrets                                    (owner; adds CI workflow)
c28c9e4  Fix workspace paths so originals survive moving ... (storage fix + connection cleanup)
ad4082c  Add start-gui.bat launcher for Windows
b56b7c7  v1.1: stronger tracing for short clips              (tag v1.1)
d1dd075  v1: per-receiver invisible watermarking ...         (tag v1)
```

`v1.1` does **not** include the launcher or the storage fix. If you make a release, tag a new commit (for example `v1.1.1`).

## 15. Questions for the owner

- Which licence should the repository carry?
- Who are the real users (a studio, a small team)? This decides how much work the GUI polish and the learned ID layer deserve.
- Is a person's view of the marked files (eyes and ears) planned? The strength defaults were chosen from numbers, not from a viewing test.
