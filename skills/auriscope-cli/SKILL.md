---
name: auriscope-cli
description: Measure and look at audio files with auriscope-cli — loudness (LUFS), true peak, clipping, DC offset, WAV/Broadcast-Wave headers and cue markers as JSON, plus annotated spectrogram and waveform PNGs that you can actually read, with time, frequency and decibel axes drawn on. Use this whenever a question touches an audio file's levels, loudness, noise floor, silence, breaths, pauses, clicks, spikes, edits, gaps, hum, crosstalk or codec artefacts; whenever you need to *see* what is inside a .wav/.mp3/.flac/.aiff; and for batch QC of a whole folder tree against a delivery spec — including writing or editing that spec, which is a TOML file this skill documents. Reach for it even when the user only says "check these takes", "what's wrong with this recording", "is this clipped", "how loud is this", or points at a folder of audio.
---

# Auriscope CLI

`auriscope-cli` is the headless half of Auriscope: the same decoding, STFT and EBU R128
pass as the GUI, with nothing on screen. Three commands.

```bash
auriscope-cli analyze PATH [options]              # what it is and what it measures, as JSON
auriscope-cli qc      PATH --spec SPEC.toml [..]  # check it against a delivery spec
auriscope-cli render  FILE [options]              # an annotated PNG of waveform + spectrogram
```

`analyze` and `qc` take **a file or a folder**; a folder is walked to any depth and every
audio file under it measured, in parallel, one JSON object per line — thousands of takes in
seconds. `render` draws one file.

Check it is there with `auriscope-cli --version`. If it is not:

```bash
curl -fsSL https://raw.githubusercontent.com/frdcmp/auriscope/main/install.sh | bash   # Linux
irm https://raw.githubusercontent.com/frdcmp/auriscope/main/install.ps1 | iex          # Windows
```

or `cargo build --release` in a checkout, which leaves it at
`target/release/auriscope-cli`.

This file is the working guide, and **three references travel with it**, in the
`reference/` directory beside this file. Read them when something below does not cover
what you need:

| File | |
| :--- | :--- |
| `reference/CLI.md` | every option, the full JSON shape, exit codes |
| `reference/QC_SPECS.md` | **how to write a QC spec** — every key, and how to choose its number |
| `reference/starter.toml` | a commented spec to copy |

`auriscope-cli --help` is the other source of truth, and it is always the installed
version's, which a bundled document cannot promise.

## Numbers first, picture second

This is the habit that makes the tool useful rather than misleading.

`analyze` is exact and cheap — milliseconds for a short take, under half a second for two
minutes of audio. `render` is for your eyes, and your eyes are good at *where* and *what
kind*, and bad at *how much*. So:

- **Never state a level, a duration or a time by looking at the picture.** If you catch
  yourself writing "the tail is about 200 ms" from an image, stop and measure it — render
  a zoom of that span and read the axis, or get the number from `analyze`.
- Use the picture to decide **where to look**, then go back for numbers.
- When you report a finding, say which one it came from. "−4.0 dBTP (measured)" and
  "an edit seam around 0.54 s (seen, then confirmed by zooming)" are different claims.

## analyze

Writes one JSON object on stdout. Progress goes to stderr, so stdout is always clean
JSON — pipe it to `jq` or parse it directly. `--quiet` silences the progress,
`--compact` writes one line, `-o FILE` writes to a file.

Blocks in the output:

| Block | What is in it |
| :--- | :--- |
| `file` | name, path, container, codec, sample rate, channels, bit depth, frames, `duration_secs`, size, tags |
| `loudness` | `integrated_lufs`, `range_lu`, `max_momentary_lufs`, `max_short_term_lufs`, `correlation`, `headroom_db`, `crest_factor_db` |
| `channels[]` | per channel: `sample_peak_dbfs`, `true_peak_dbtp`, `rms_dbfs`, `dc_offset`, `clipped_samples`, `clipped_runs`, `l10/l50/l90_dbfs`, `noise_floor_dbfs` |
| `wave` | RIFF chunk list, `fmt` details — `null` for non-WAV files |
| `broadcast_wave` | the `bext` chunk: originator, date, time reference, coding history, its own loudness fields |
| `markers[]` | cue points, in both frames and seconds |

A measurement that has no finite value is `null`, not a number — a silent file
integrates to −∞ LUFS, and `max_short_term_lufs` is `null` on a file shorter than the 3 s
its window needs. Treat `null` as "not measurable", never as zero.

**`--start SECS --end SECS` measures just that span**, and it is the sharpest tool here.
Rather than reading a level off a picture, cut to the part you are asking about:

```bash
auriscope-cli analyze take.wav --start 0 --end 0.2 --quiet | jq .channels[0].rms_dbfs
```

A `region` block then says which span was measured; without those flags there is no such
block and the numbers are the whole file's.

Per channel you also get **`l10_dbfs` / `l50_dbfs` / `l90_dbfs`** — the levels exceeded 10,
50 and 90 % of the time — and **`noise_floor_dbfs`**, the quietest tenth of 10 ms frames
with digital-zero frames excluded. For "is the background too loud", `noise_floor_dbfs` over
the silent span is the number, not an impression of the picture. `--timeline` adds the same
against time, one entry per 100 ms, when you need to know *where* rather than *whether*.

```bash
auriscope-cli analyze take.wav --quiet | jq '{lufs: .loudness.integrated_lufs, tp: .channels[0].true_peak_dbtp, clipped: .channels[0].clipped_samples}'
```

### The passes that are off by default

Everything above comes from one walk through the samples. Four more are asked for by name,
and `--all` turns on all four. Reach for them when the question is about *content* rather
than level.

| Flag | What it adds |
| :--- | :--- |
| `--structure` | `leading_silence_secs`, `trailing_silence_secs`, `pauses[]` with `event_inside`, `speech_ratio`, `boundary_step_db` |
| `--segments` | every stretch as `silence` / `speech` / `event`, each with `rms_dbfs` **and** `rms_above_80hz_dbfs` |
| `--defects` | `digital_silence.runs[]`, `clicks[]` (with `in_speech`), `seams[]` (a floor step = a cut with no crossfade), `truncation` |
| `--spectral` | centroid, rolloff, flatness, `cutoff_hz`, `transcode_suspect`, `hum` and harmonics, third-octave bands |

Thresholds: `--speech-db` (10), `--min-speech-ms` (100), `--min-gap-ms` (80),
`--zero-run-ms` (1), `--click-db` (32), `--spectral-window` (8192). `--channel N` picks
which channel decides structure (default 0).

Three things to be honest about when you report from these:

- Segmentation is **energy-based, not a voice detector**, and a breath louder than
  `--speech-db` over the floor reads as speech — which splits the pause it sits in. So
  pauses are never over-reported, and `event_inside` marks a pause with a breath in it.
- `clicks` and `seams` are heuristics validated on synthetic signals and a few real files.
  An unexpected rate is a question about the threshold before it is a question about the
  audio — look at three by eye with `render` before quoting it.
- `cutoff_hz` looks for a *cliff*, not a rolloff; `transcode_suspect` means a wall where a
  microphone would not put one.

### A whole folder

```bash
auriscope-cli analyze takes/ --all --csv -o takes.csv     # flat table, joinable on `file`
auriscope-cli analyze takes/ --all --quiet | jq -r 'select(.defects.seams|length>0) | .file.path'
```

JSON Lines by default, `--csv` for a table (`--jsonl` is the explicit opposite), `--jobs N`
to cap the parallelism. A file that will not decode still gets a row with the reason in
`error`, so the table never silently gets shorter. Columns needing a pass that did not run
are empty, not zero.

## qc — checking against a delivery spec

```bash
auriscope-cli qc takes/ --spec specs/mine.toml --csv -o qc.csv --fail-only
auriscope-cli qc takes/ --spec specs/mine.toml --render-failures bad/
```

Exit **0** clean, **1** something failed, **2** the tool broke. Each report gains a `qc`
block: `pass`, `counts`, and `findings[]` worst-first, each with `rule`, `severity`,
`measured`, `limit` and `at_secs` — the times to point a render at. `--csv` adds
`qc_pass`, `qc_fails` and `qc_rules`. `--render-failures DIR` writes a PNG per failing
file, zoomed on the first finding, which is the picture to attach to a reply.

A spec is TOML with four optional tables — `[format]`, `[levels]`, `[structure]`,
`[defects]`. A rule left out is not checked; a misspelled key is an error, not a silent
pass; a measurement that was not taken is skipped rather than failed. Severities are
`fail` (fails the file), `warn` and `note` (recorded, do not fail).

**Before writing or editing a spec, read `reference/QC_SPECS.md`** — every key, what it
reads, and how to derive a threshold from material the client has already accepted instead
of inventing one. Start from `reference/starter.toml` rather than writing from memory.

```bash
# What is failing across a delivery, by rule
auriscope-cli qc v2/ --spec specs/mine.toml --quiet --fail-only |
  jq -r '.qc.findings[] | select(.severity=="fail") | .rule' | sort | uniq -c | sort -rn
```

## render

```bash
auriscope-cli render take.wav -o take.png                       # whole file
auriscope-cli render take.wav --start 3.4 --end 3.7 -o zoom.png # a zoom
```

Then **read the PNG back** with your image-reading tool. That is the whole point: the
plot carries its own axes, so you can locate what you see.

Each channel gets two lanes: a waveform above, a spectrogram below. Time runs left to
right with gridlines that line up across every lane, frequency runs bottom to top (linear
by default, `--log` for logarithmic), and the colour bar down the right says which colour
is which decibel level.

By default the waveform is drawn **over** the spectrogram as one pane, the frequency axis
is **linear**, the colour map is Inferno over `-115:-9` dB, and the waveform is amplitude
with a decibel ruler inside the lane on the right. At the default `--wave-zoom 1` the lane
is exactly full scale, so `0` sits on the top edge. `--no-merge` splits the panes apart,
`--log` gives a logarithmic frequency axis.

That default is tuned for looking at speech. Two things to reach for when it is not enough:
`--wave-scale db` reshapes the envelope into decibels, which makes a noise floor and the
gap between two takes visible where a linear waveform draws a flat line; `--log` spreads
the bottom two octaves out, which is where hum and rumble live.

Options worth knowing:

| Option | |
| :--- | :--- |
| `--start` `--end` | seconds. Only the visible span is analysed, so zooming into a long file is fast |
| `--window N` | FFT size — see below. 256 … 16384, default 2048 |
| `--reassign` | sharpens tones to hairlines and clicks to single columns. Slower. Worth it when hunting clicks |
| `--db MIN:MAX` | colour range, default `-115:-9`. Narrow it (`--db -75:-30`) to bring a noise floor into view |
| `--channel N` | one channel, counting from 0; the default stacks them all |
| `--width` `--height` `--wave-height` | pixels. `--no-waveform` drops the waveform lane |
| `--linear` `--min-hz` `--max-hz` | the frequency axis |
| `--no-merge` | waveform in its own lane above the spectrogram instead of over it |
| `--log` | logarithmic frequency axis (the default is linear) |
| `--spectrum` | add a level-against-frequency pane for the whole span: the still-picture spectrum |
| `--wave-scale linear --wave-zoom N` | the window's vertical zoom, 1 to 4096. Lifts a noise floor into view |
| `--wave-db FLOOR` | bottom of the dB waveform, separate from `--db` |
| `--no-spectrogram` `--wave-color C` | waveform only; `#rrggbb` or `r,g,b` |
| `--json PATH` | writes the analyze report plus how the picture was framed |
| `--quiet` | no progress on stderr |

Two of those are worth reaching for deliberately rather than as decoration. **A zoomed
linear waveform** (`--wave-scale linear --wave-zoom 256`) is the clearest way to see where
a background has been edited out: the loud parts saturate and the quiet stretches read as
gaps. **The spectrum pane** answers "what is the level at this frequency, over this
stretch" — a noise floor's shape, a codec's ceiling, a hum's partials — which a
spectrogram shows only as a colour you have to judge by eye.

`--json` is what lets you convert a pixel back into a time: its `view` block carries
`start_secs`, `end_secs`, `width_px` and `secs_per_pixel`. Prefer re-rendering a zoom over
doing that arithmetic, though — the axis is more reliable than your pixel estimate.

## Choosing the window size

This is the one setting that changes what you are able to see, and the default is not
always right. The window is a trade: a long window separates frequencies, a short one
separates events in time. You cannot have both at once.

- **256–512** — clicks, edit seams, hard cuts, plosives. Short events stop smearing.
- **2048** (default) — speech, general looking around.
- **8192–16384** — hum and its harmonics, pitch, tonal noise, anything where you need to
  tell 50 Hz from 60 Hz or resolve closely spaced partials.

If something looks like a vertical smear and you want to know exactly when it happened,
shorten the window. If lines look thick and you want to know exactly what frequency they
are, lengthen it.

## The working loop

1. `analyze` the file. Numbers first — they may answer the question outright.
2. `render` the whole file and look at it.
3. Zoom on anything suspicious: `--start`/`--end` around it, `--window 512`,
   and `--reassign` if you are chasing a click.
4. Read the time off the axis in the zoom, not off the wide view.

## What things look like

Verified signatures, so you know what you are looking at:

- **Digital silence** (a gap cut in, or a mute) — a black band spanning the full height,
  with the neighbouring room tone visible as a coloured wash. Unmistakable.
- **A hard cut with no crossfade** — a vertical broadband line at the join, often at both
  ends of an inserted region, and frequently a step in the noise floor's colour across the
  boundary. The noise floor changing brightness at a vertical line *is* an edit.
- **A click or spike** — a narrow vertical line crossing many octaves. `--reassign`
  collapses it to roughly one column, which is how you time it.
- **A codec bandwidth limit** — a horizontal ceiling with black above it: about 11 kHz for
  a 64 kbps MP3, ~15–16 kHz for 128 kbps. If a "WAV" shows one, it was transcoded.
- **Mains hum** — steady horizontal lines at 50 or 60 Hz and whole multiples of it. Needs
  a long window to separate from its neighbours.
- **A tone** — one steady horizontal line. **A sweep** — a diagonal one.
- **Room tone / noise floor** — a uniform wash at the bottom of the lane. Its colour
  against the colour bar gives you a rough level; `analyze`'s `rms_dbfs`, or a render of a
  silent stretch, gives you the real one.
- **Clipping** — do not judge this by eye. `analyze` counts `clipped_samples` and
  `clipped_runs` exactly.

## Batch work

Hand the folder over rather than looping — it walks nested trees, runs in parallel, and
keeps stdout clean JSON:

```bash
auriscope-cli analyze takes/ --quiet |
  jq -s 'map({name: .file.name, lufs: .loudness.integrated_lufs, tp: .channels[0].true_peak_dbtp, clipped: .channels[0].clipped_samples})'
```

With a spec in hand, `qc` is the same sweep with the judging done for you, and its
`--fail-only` output is the short list worth looking at.

Sweep with numbers first, sort by whatever the spec cares about, then render only the
handful of files that look wrong. Rendering fifty files you have no reason to doubt wastes
your context and the user's time.

## What it will not tell you

Be straight with the user about these rather than guessing:

- **Crosstalk, or who is speaking.** There is no diarisation here. A second voice may be
  visible as overlapping harmonics, but identifying speakers needs a different tool.
- **Mouth noises, smacks, breaths.** Sometimes visible as short transients in the 2–6 kHz
  region, but you cannot reliably separate a lip smack from a consonant by looking.
- **Whether something sounds bad.** The tool measures and draws. Perceptual judgement is
  the user's.

It also never writes to the audio: `analyze` and `render` only read.
