# VisualFingerprint

Give each receiver a copy of a file with a hidden code that only you can read.
If a copy leaks, the tool tells you whose copy it was.

- Works on **images, video, and audio**.
- The mark is invisible (or very hard to notice). It survives re-encoding, compression, resizing, cropping, trimming, and more.
- Each receiver gets a **collusion-resistant code** (a Tardos code). If several receivers blend their copies, the tool can still name at least one of them.
- Use it from the **command line** or the **desktop app**.

> **Read this first.** No watermark is unbreakable. A skilled attacker with time can weaken or remove it.
> This tool makes a leak *traceable in common cases*. A score is **statistical evidence, not proof**.

---

## How it works (short)

1. **Register** your original file. The tool keeps a private copy. Tracing needs it.
2. **Make copies.** Each receiver gets a different code. The tool hides the code in the file.
3. **Trace a leak.** You give the tool the leaked file. It lines the leak up with your original,
   reads the hidden symbols, and scores every receiver. A receiver above the threshold is accused.

Details:

| Part | What the tool does |
|---|---|
| Code | A Tardos code. Each receiver gets a random bit string. Each bit is one *symbol*. |
| Image / video | Adds a keyed pattern to mid-frequency DCT coefficients of the luma. Textured areas get more mark, flat and dark areas get less. |
| Audio | Scales each time-frequency cell of the STFT by a small keyed amount (about 5%, 300 Hz to 8 kHz). The mark follows the signal. |
| Alignment | Finds the crop, scale, rotation, time offset, and frame rate of the leak before it reads the mark. |
| Detection | Compares the leak with your original (this is called *informed* detection). That makes it much stronger than reading a mark with no reference. |
| Accusation | Scores every receiver. The threshold uses the exact weight of the symbols that were read, so it adapts to short or damaged leaks. |

The mark is built from your secret key. The database stores no secrets.

---

## Install

You need:

- Python 3.11 or newer
- [FFmpeg](https://ffmpeg.org/) and ffprobe on your `PATH`

```bash
python -m venv .venv
.venv\Scripts\activate
pip install -e ".[gui,dev]"
```

Leave out `gui` if you do not want the desktop app. Leave out `dev` if you do not want the tests.

---

## Quick start (command line)

```bash
vfp init
vfp receivers add alice bob carol
vfp asset add "my-film.mp4" --name film
vfp mark film --all --out marked
```

This writes one marked copy per receiver into `marked/`, for example `film__alice.mp4`.

When a copy leaks:

```bash
vfp trace "leaked.mp4"
```

Example result:

```
asset: film (video)   accusation threshold: 243.5

rank  receiver                          score
   1  carol                            1167.0  <-- above threshold
   2  bob                               -15.4

LEAKER: carol
```

### Commands

| Command | What it does |
|---|---|
| `vfp init` | Make a workspace and a secret key. |
| `vfp receivers add NAME…` / `import FILE` / `list` | Manage receivers. Import reads one `name[,note]` per line. |
| `vfp asset plan FILE` | Show how much a file can carry. Changes nothing. |
| `vfp asset add FILE` | Register an original file. |
| `vfp asset list` | List registered files and the copies issued. |
| `vfp mark ASSET --to NAME… \| --all --out DIR` | Make marked copies. |
| `vfp trace LEAKED [--asset X] [--json] [--top N]` | Find who leaked a file. Without `--asset`, it tries every file of the same type. |
| `vfp gui` | Open the desktop app. |

`vfp asset add` options:

| Option | Meaning | Default |
|---|---|---|
| `--max-receivers` | Most receivers you plan for. Sets the accusation threshold. | 1000 |
| `--colluders` | Colluder group size the code is designed for. | 3 |
| `--eps` | Chance of accusing an innocent receiver. | 0.001 |
| `--strength` | Mark strength. Higher = more robust, easier to notice. (Audio: the relative size, 0.05.) | 3.0 |
| `--slots` | Symbols per image, or per segment for audio and video. | automatic: short files pack more symbols |
| `--crf` | Video only: x264 quality of marked copies. Higher = smaller file, lower fidelity. | 16 |

The workspace folder is `--home`, `$VFP_HOME`, or `./.vfp`.

---

## Desktop app

```bash
vfp gui
```

On Windows you can also double-click `start-gui.bat`. It checks the setup and opens the app without a console window.
Give it a workspace folder to open a specific workspace: `start-gui.bat "D:\my-workspace"`.

Four tabs:

- **Receivers.** Add or import people.
- **Make copies.** Add a file, tick the receivers, choose an output folder, and make the copies.
  The *Add file* dialog shows what the file can carry before you register it.
- **Investigate a leak.** Choose a leaked file or drop it on the tab. You see a verdict, a ranked list with score bars, and warnings.
  *Save report…* writes a text report with the SHA-256 of the leaked file.
- **Workspace.** Shows where the workspace and the secret key are.

---

## Keep these safe

| Item | Why |
|---|---|
| `master.key` in the workspace | Every hidden code comes from it. **Without it you cannot trace a leak.** Back it up. Never share it. |
| `originals/` in the workspace | Private copies of your unmarked files. Tracing needs them. `asset add` makes the copy, so you can delete or move your own file afterwards. To move the workspace, move the whole folder (key, database, and `originals/`). |
| `vfp.db` | The list of receivers and the copies you issued. |

Never give a receiver the unmarked original. If two receivers get the same file, you cannot tell them apart.

---

## What the mark survives

These results come from the test files in `test-media/`. The attacker's file was traced to the right receiver, above the 0.1% false-accusation threshold, unless the table says otherwise.

### Images (photos, about 1 megapixel and larger)

JPEG down to quality 30, scaling to 50%, a 70% crop, a 2° rotation, blur, added noise, and brightness/contrast changes.
Scaling to 60% plus JPEG quality 60 also passed.
The mark costs about 44–49 dB PSNR (SSIM about 0.988).

**Weaker:** flat graphics and screenshots (little texture to hide in), and very small images.
An image under about 100 pixels wide cannot carry enough symbols to accuse anyone.

### Video

x264 (CRF 23 to 34), x265, VP9, scaling to 480p and to half size, a 90% crop, a lower frame rate,
blur, noise, brightness/contrast changes, trimming the start, and a rotated, shrunk screen-recording simulation.
This held for a 720p clip, a 240×240 clip, and a 4K clip.

**Weaker:** the time offset of heavily edited, near-static footage.
A 4K file is marked at about 720p resolution and scaled up, so a large downscale still meets a coarse mark.

### Audio

MP3 at 192k, 128k, and 64k, AAC at 96k and 48k mono, Opus at 64k, resampling to 22 kHz or 16 kHz,
volume changes, a 6 kHz lowpass, compressor plus EQ, mild echo, and trimming.
The mark has an SNR of about 33 dB.

**Fails:** a speed or pitch change (a +3% speed change broke the alignment). On a short clip, heavy echo also failed.

### Collusion

Colluders blended their copies, then saved them as ordinary files. The tool accused at least one colluder and never an innocent receiver.

| Media | Strategies | Group sizes | Result |
|---|---|---|---|
| Song, 173 s | average, random splicing, min-abs | 2, 3, 4, 6 | 54 of 54 trials caught |
| Video, 20 s | average, chunk splicing, frame mixing | 2, 3 | 18 of 18 caught |
| Photo, 1600 symbols (v1.1 default) | average, min, max, median, pixel mix, tile mix | 3, 4 | 60 of 60 caught |
| Audio, 10 s (v1.1 default, 208 symbols/s) | average, random splicing, min-abs | 2, 3, 4 | 72 of 72 caught |

**More symbols means stronger tracing.** Short or small files carry few symbols, so v1.1 packs more
symbols into them. For comparison, before v1.1 the same 10 s audio clip (384 symbols) lost groups of 3
in 20–40% of trials, and a photo with 256 symbols lost almost every group of 3.
`vfp asset plan FILE` shows the code length and the colluder group size the theory guarantees.
The theory is cautious. Real files did better than it promises.

### How v1.1 makes short clips stronger

Two changes, both measured on the test files:

1. **Exact accusation threshold.** The threshold now follows the real weight of the symbols that were read.
   When a leak is short, damaged, or partly lost, the threshold drops with it. The chance of accusing an
   innocent receiver stays at or below the level you chose (checked by simulation).
   On the 10 s clip this alone raised success against groups of 3 from 43% to about 59%.
2. **Symbol density that fits the clip.** The tool asks the theory how many symbols the plan needs
   (receivers, colluder group size, false-accusation rate). It packs that many symbols into the file,
   up to a limit set by measurement. Long files keep the default and stay strong per symbol.

| Media | Default | Most the tool packs | Why that limit |
|---|---|---|---|
| Audio | 32 symbols per 1 s segment | 256 | bit accuracy is still about 0.78 on harsh attacks |
| Video | 64 symbols per 0.5 s segment | 256 | all 4 harsh attacks still traced; at 512 one failed |
| Image | up to what fits | 12 blocks per symbol | 1600 symbols on a 1.2 MP photo passed every attack |

The price: each symbol gets a smaller share of the file, so per-symbol accuracy falls
(for audio, from 0.94 at 32 per segment to 0.78 at 256, over three harsh attacks).
More symbols still win, because they add up faster than the accuracy falls.
Set `--slots N` to choose the density yourself.

---

## Limits you should know

- **Not blind.** Tracing needs your original. You cannot trace a file that you never registered.
- **Not unbreakable.** I did not test an attacker who knows the method and adds heavy filtering before blending.
  I did not test AI "regeneration" or heavy learned denoisers.
- **Speed and pitch changes** are not handled for audio.
- **Screen recordings** work for mild geometry only. Heavy crop, rotation, or perspective is hard.
- **One extra re-encode.** A marked video is encoded once more with x264. The default (CRF 16) can make the file much larger than a low-bitrate source.
  Raise `--crf` for smaller files.
- **Video sound** is copied unchanged, so a video's sound track carries no mark yet.
- **Images:** copies are re-encoded. EXIF data is dropped. WebP images are saved as PNG.
- **Audio:** mono and stereo only. The whole file is held in memory.
- **Quality is measured, not heard.** I measured PSNR, SSIM, VMAF, and SNR. Check the marked files by eye and ear.
  The mark costs about 1 VMAF point at 720p and about 3.6 at 4K at the default strength.
- **Speed:** video marking runs at about 50 frames per second at 720p and about 10 at 4K.
- **Evidence, not proof.** Keep the leaked file, the report, and your originals. Treat the result as a strong lead for further action.

You are responsible for using this tool lawfully. Rules on marking media and on tracing people differ by place.
Tell receivers that their copies are marked where the law or a contract requires it.

---

## Test it yourself

```bash
pytest
```

The tests cover the codes, all three media types, the command line, the GUI (headless), and collusion.

Evaluation scripts run on any file:

```bash
python -m vfp.eval.image_eval  photo.jpg
python -m vfp.eval.video_eval  clip.mp4  --seconds 8
python -m vfp.eval.audio_eval  song.mp3  --seconds 30
python -m vfp.eval.collusion_eval {image|audio|video} FILE --receivers 12 --trials 6 --sizes 2,3
```

---

## Project layout

```
src/vfp/
  codes/          Tardos code, key derivation
  image/ video/ audio/   embed, detect, and alignment for each media type
  perceptual.py   how much mark each block can hide
  workspace.py    database and secret key
  pipeline.py     register, mark, trace (the CLI and the GUI both call this)
  cli.py          command line
  gui/            desktop app (PySide6)
  eval/           attack suites, quality metrics, collusion trials
tests/
test-media/       sample files used in the evaluations (not in git: put your own media here)
samples/          marked files for A/B checks (not in git: made locally)
```

The folders `test-media/`, `samples/`, and any workspace (`.vfp/`) are in `.gitignore`.
This keeps large media and secret keys out of the repository.
