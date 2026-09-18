# `auriscope-cli` reference

The headless half of Auriscope: the same decoding, STFT and EBU R128 pass as the window,
with nothing on screen. Written to be driven by a script or by a language model as much as
by a person, so everything it knows comes out as JSON, and every picture it draws carries
the axes needed to say when and at what frequency something happened.

This is the complete reference. For the habits that make the tool useful rather than
misleading — when to trust a number over a picture, how to choose a window size, what an
edit seam looks like — see [Working with it](#working-with-it) at the end.

- [Commands](#commands)
- [`analyze`](#analyze)
- [Folders](#folders)
- [`qc`](#qc)
- [`render`](#render)
- [Options](#options)
- [Exit codes and streams](#exit-codes-and-streams)
- [Working with it](#working-with-it)

---

## Commands

```
auriscope-cli analyze PATH [options]             # what it is and what it measures, as JSON
auriscope-cli qc      PATH --spec SPEC.toml [..] # check it against a delivery spec
auriscope-cli render  FILE [options]             # an annotated PNG of waveform and spectrogram
auriscope-cli --help
auriscope-cli --version
```

`analyze` and `qc` take a file **or a folder**, walked to any depth — see
[Folders](#folders). `render` draws one file. Anything Symphonia decodes works: WAV, AIFF,
CAF, FLAC, ALAC, APE, MP3, AAC, OGG/Vorbis, Matroska. No command ever writes to the audio.

---

## `analyze`

One JSON object on stdout.

```bash
auriscope-cli analyze take.wav --quiet | jq .loudness
```

### Shape

```jsonc
{
  "auriscope": { "version": "0.1.3", "analysed_unix": 1789642923, "level_frame_ms": 10 },

  // Only when --start/--end were given. Its absence means the numbers below
  // are the whole file's.
  "region": { "start_secs": 0.0, "end_secs": 0.2, "duration_secs": 0.2,
              "start_frames": 0, "end_frames": 9600 },

  "file": {
    "name": "take.wav",
    "path": "/takes/take.wav",
    "container": "Waveform Audio File Format",
    "codec": "PCM Signed 32-bit Little-Endian Interleaved",
    "sample_rate_hz": 48000,
    "channels": 1,
    "bits_per_sample": 32,
    "frames": 38883,
    "duration_secs": 0.810063,
    "size_bytes": 155576,
    "bitrate_kbps": 1536.1,
    "modified_unix": 1789526375,
    "tags": [{ "name": "...", "value": "..." }]
  },

  "loudness": {
    "integrated_lufs": -23.34,       // EBU R128, whole file
    "range_lu": 0.0,
    "max_momentary_lufs": -21.59,    // 400 ms window
    "max_short_term_lufs": null,     // 3 s window — null on a file shorter than that
    "correlation": null,             // channels 0 and 1, null if mono
    "headroom_db": 8.59,             // to 0 dBTP
    "crest_factor_db": 15.31,        // peak minus RMS
    "plr_db": 14.75                  // true peak minus integrated loudness
  },

  "channels": [
    {
      "index": 0,
      "name": "Mono",                // "Left"/"Right" for stereo, "Channel N" above
      "sample_peak_dbfs": -8.64,
      "true_peak_dbtp": -8.59,       // oversampled per EBU R128: catches inter-sample peaks
      "rms_dbfs": -23.95,
      "dc_offset": -0.000004,        // linear, not dB: 0 is clean
      "clipped_samples": 0,
      "clipped_runs": 0,             // consecutive runs at or above 0.999
      "l10_dbfs": -15.43,            // level exceeded 10% of the time: the loud end
      "l50_dbfs": -31.56,
      "l90_dbfs": -68.38,            // exceeded 90%: the bed the signal rests on
      "noise_floor_dbfs": -68.38,    // quietest tenth, digital-zero frames excluded
      "level_frames": 311,           // 10 ms frames that went into those four
      "zero_frames": 0               // and how many were pure digital silence
    }
  ],

  "wave": { "rf64": false, "riff_size": 155568, "fmt": { … }, "chunks": [ … ], "info": [ … ] },
  "broadcast_wave": { "originator": "…", "origination_date": "…", "time_reference_samples": 0, … },
  "markers": [{ "id": 1, "position_frames": 48000, "position_secs": 1.0, "label": "…" }]
}
```

`wave` and `broadcast_wave` are `null` for anything that is not a RIFF/WAVE file, and
`broadcast_wave` is `null` for a WAV with no `bext` chunk.

**`null` means "no finite value", never zero.** A silent file integrates to −∞ LUFS and
comes out as `null`. Code that treats it as 0 will report silence as full scale.

Keys are sorted alphabetically, which is `serde_json`'s doing. Do not rely on order.

### A region instead of the whole file

`--start SECS` and `--end SECS` measure only that span. Every measurement block then
describes the region, `file` still describes the file, and a `region` block says which span
was taken. Without them there is no `region` key at all, which is how a reader tells the
two apart.

```bash
# What level is the leading silence, on its own?
auriscope-cli analyze take.wav --start 0 --end 0.2 --quiet | jq .channels[0].rms_dbfs
```

This is the cheapest way to answer a question about part of a file, and it is exact — no
reading levels off a picture.

### Levels and percentiles

`l10` / `l50` / `l90` are the levels exceeded 10, 50 and 90 % of the time, over 10 ms
frames (`auriscope.level_frame_ms`). Three numbers say what one average cannot: `l10` is
where the loud parts sit, `l90` the bed underneath them, and the gap between them is how
dynamic the file is.

`noise_floor_dbfs` is the quietest tenth of those frames. **Frames of pure digital silence
are excluded from all four**, so a file padded with zeros still reports the floor of the
audio in it rather than −∞.

### Loudness against time

`--timeline` adds a block with one entry per 100 ms:

```jsonc
"timeline": {
  "block_secs": 0.1,
  "blocks": 32,
  "momentary_lufs":  [null, null, null, -28.47, …],  // 400 ms window
  "short_term_lufs": [null, … 29 of them …, -27.4],  // 3 s window
  "channels": [{ "index": 0, "name": "Mono", "rms_dbfs": [ … ], "peak_dbfs": [ … ] }]
}
```

The leading `null`s are deliberate. The meter answers before its window has filled, but it
divides by the whole window regardless, so those opening blocks read progressively too
quiet — left in, every file would appear to fade in for its first three seconds. They are
withheld instead, and for the same reason `max_short_term_lufs` is `null` on a file shorter
than 3 s.

### The passes that are off by default

Everything above comes from one walk through the samples. Four more passes cost more time
and are asked for by name; `--all` turns on all four. They share their working, so asking
for two costs barely more than one.

#### `--structure` — where the speech is

```jsonc
"structure": {
  "leading_silence_secs": 0.214,     // to the first speech
  "trailing_silence_secs": 0.238,
  "leading_nonzero_secs": 0.0,       // to the first non-zero sample, which is not the same
  "trailing_nonzero_secs": 0.0,
  "speech_secs": 2.94, "speech_ratio": 0.8413,
  "longest_pause_secs": 0.42,
  "pauses": [{ "start_secs": 1.21, "end_secs": 1.63, "duration_secs": 0.42,
               "event_inside": true }],   // true when a breath sits in it
  "boundary_step_db": 11.2,          // floor under the speech vs the silence beside it
  "floor_dbfs": -66.4,
  "speech_threshold_dbfs": -56.4
}
```

Segmentation is **energy-based, not a voice detector**: a 300–8000 Hz copy of the signal,
a floor taken as the quiet tenth of its frames, and speech wherever the level sits
`--speech-db` above that floor for at least `--min-speech-ms`. That independence is the
point — it will not agree with a pipeline's own VAD by construction, so when the two agree
the answer is worth something.

Its known bias: **a breath louder than `--speech-db` over the floor reads as speech**, which
splits the pause it sits in into two shorter ones. Pauses are therefore never over-reported,
and `event_inside` is how a long pause with a breath in it is told from an empty one.

#### `--segments` — every stretch, with its level

```jsonc
"segments": [
  { "kind": "silence", "start_secs": 0.0, "end_secs": 0.214, "duration_secs": 0.214,
    "rms_dbfs": -63.1, "rms_above_80hz_dbfs": -67.8 },
  { "kind": "speech",  … },
  { "kind": "event",   … }     // short and quiet: a breath, a lip noise
]
```

Two readings of every stretch, because a limit written for one is several decibels wrong
against the other on any voice with rumble under it.

#### `--defects` — things that should not be there

```jsonc
"defects": {
  "digital_silence": { "count": 1, "total_secs": 0.02,
                       "runs": [{ "start_secs": 1.4, "end_secs": 1.42, "duration_secs": 0.02 }] },
  "clicks": [{ "secs": 3.417, "ratio_db": 38.2, "in_speech": false }],
  "seams":  [{ "secs": 2.006, "floor_step_db": 9.4 }],
  "truncation": { "head": false, "tail": true, "head_dbfs": -71.0, "tail_dbfs": -34.2 }
}
```

A **click** is a step more than `--click-db` above the local 20 ms RMS, measured with a
±1 ms guard around the spike itself — without it, a click loud enough to matter inflates
the level it is compared against and hides. A **seam** is a step in the noise floor away
from speech: what a cut with no crossfade leaves behind. **Truncation** is signal still
present at the very first or last sample.

Both detectors are heuristics, and validated so far on synthetic signals and a handful of
real files. Treat an unexpected rate as a question about the threshold before it is a
question about the audio.

#### `--spectral` — the shape of the sound

```jsonc
"spectral": {
  "window_size": 8192, "frames": 96,
  "centroid_hz":   { "median": 1180.4, "p10": 790.2, "p90": 2310.6 },
  "rolloff85_hz":  { … }, "rolloff95_hz": { … },
  "flatness":      { "median": 0.0021, … },   // 0 tonal, 1 noise-like
  "cutoff_hz": 15750.0,          // steepest fall on a 1/12-octave grid
  "transcode_suspect": true,     // a wall where a microphone would not put one
  "hum": { "hz": 50.0, "level_db": -62.1, "prominence_db": 14.0 },
  "hum_harmonics": [ … ],
  "bands_third_octave": [{ "centre_hz": 25.0, "level_db": -78.2 }, …]
}
```

`cutoff_hz` looks for a **cliff**, not a level crossing: speech rolls off naturally, and
anything that measures a drop below a peak calls that rolloff a codec ceiling. A fall of
15 dB or more between adjacent twelfth-octaves is a wall, and a wall is what
`transcode_suspect` reports — a "WAV" that was an MP3 at some point in its life.

### Options

`--compact` writes one line instead of indented. `-o PATH` writes to a file instead of
stdout. `--quiet` silences the progress on stderr.

The thresholds the passes above use are settable, and a spec that sets them is better than
a habit that remembers to: `--speech-db` (default 10), `--min-speech-ms` (100),
`--min-gap-ms` (80), `--zero-run-ms` (1), `--click-db` (32), `--spectral-window` (8192).

---

## Folders

Give `analyze` or `qc` a folder instead of a file and it walks it **to any depth**, measures
every audio file under it, and writes one result per file. Hidden directories (`.git`,
`.venv`) are skipped, and the files are sorted, so two runs over the same tree line up line
for line.

```bash
auriscope-cli analyze takes/ --all --csv -o takes.csv
auriscope-cli analyze takes/ --all --quiet | jq -r 'select(.loudness.integrated_lufs > -20) | .file.path'
```

Files are processed in parallel, `--jobs N` at a time, defaulting to one per core. Around
four thousand short takes measure in a few seconds.

**JSON Lines by default**: one complete object per line, so a reader can process the first
result before the last file is open, and `grep` still works. `--csv` writes a flat table
instead — a fixed set of scalar columns, chosen rather than flattened, because a table
whose columns change between files is not a table. The first column is the bare filename,
which is what makes it joinable against a delivery inventory.

Columns that need a pass that did not run are empty, not zero. **A file that will not
decode still gets a row**, with its name and the reason in the `error` column: a silently
shorter table is worse than a table with a hole in it.

A single file asked for by name still gets the readable indented object, not a table of
one.

---

## `qc`

Measure files, check them against a delivery spec, and exit non-zero if any failed.

```bash
auriscope-cli qc take.wav --spec specs/mine.toml
auriscope-cli qc takes/  --spec specs/mine.toml --csv -o qc.csv --fail-only
auriscope-cli qc takes/  --spec specs/mine.toml --render-failures bad/
```

A spec is TOML — the numbers a client agreed to, in a file that can be read, edited and
argued with. **[Writing a QC spec](QC_SPECS.md) is the reference for authoring one**, and a
commented template to copy ships as `specs/starter.toml`. In outline: four optional tables
(`[format]`, `[levels]`, `[structure]`, `[defects]`), a rule left out is a rule not checked,
a misspelled key is an error rather than a silent skip, and findings carry one of three
severities — `fail` fails the file, `warn` and `note` are recorded and do not.

The report gains a `qc` block:

```jsonc
"qc": {
  "spec": "Delivery v1",
  "pass": false,
  "counts": { "fail": 1, "note": 1 },
  "findings": [{ "rule": "pause_too_long", "severity": "fail",
                 "measured": 1240.5, "limit": 700.0,
                 "at_secs": [3.41, 7.88], "detail": "milliseconds, 2 over the limit" }]
}
```

Findings come worst first. `--csv` adds `qc_pass`, `qc_fails` and `qc_rules` — the failing
rule names, space separated — to the usual columns.

`qc` runs the structure and defect passes whether or not they were asked for, because a
rule checked against a measurement nobody took reports nothing and reads as a pass. The
spectral pass is not among them: nothing in a spec needs it, so add `--spectral` if you
want those columns too.

`--render-failures DIR` writes a PNG per failing file, zoomed to a second either side of
the first thing wrong with it, through the same option parser a hand-written `render` would
use. It is the picture to attach to the reply.

```bash
# What is failing, across a whole delivery
auriscope-cli qc v2/ --spec specs/mine.toml --quiet --fail-only |
  jq -r '.qc.findings[] | select(.severity=="fail") | .rule' | sort | uniq -c | sort -rn
```

---

## `render`

Writes a PNG. With `--json PATH`, also writes the `analyze` report with two extra blocks
saying how the picture was framed.

```bash
auriscope-cli render take.wav -o take.png
auriscope-cli render take.wav --start 3.44 --end 3.62 --window 1024 -o click.png
```

### What a render opens on

The defaults are the view the window is usually left sitting on: the waveform over the
spectrogram as one pane, a **linear** frequency axis, Inferno, and a `-115:-9` dB colour
range — wide enough at the quiet end to show a room tone without the loud end burning out.
Every one of them has a flag: `--no-merge`, `--log`, `--colormap`, `--db`.

### What is drawn

One channel per pair of lanes — a waveform above, a spectrogram below — stacked down the
image, all sharing one time axis whose gridlines line up across every lane.

- **Time** runs left to right, labelled in seconds, or `m:ss` once the span passes a
  minute. The unit is written under the last tick.
- **Frequency** runs bottom to top, linear by default. The lane title says which
  (`Hz, linear` or `Hz, log`).
- **Level** is the colour, with the bar down the right mapping colour to decibels.
- **The waveform is drawn over the spectrogram by default**, as one pane, which is the
  window's merged view. `--no-merge` puts it in a lane of its own above.
- **The waveform is amplitude with a decibel ruler**, peak span in one colour with RMS
  inside it, the way the window draws it. At the default `--wave-zoom 1` the lane is
  exactly full scale: **0 dBFS is the top edge**, labelled `0`, with no dead air above it.
- In a merged pane that ruler sits **inside the lane on the right**, because the frequency
  axis owns the left margin and two rulers in one margin land on top of each other.

### The other two panes

Merging is the default; `--no-merge` splits the two apart. The overlay stays on the
**linear** scale unless `--wave-scale db` says otherwise: a decibel envelope is tall nearly
everywhere, and drawn on top it would blot out the picture it is meant to annotate.
`--merge-opacity` and `--merge-spec-opacity` balance the two — the defaults draw the
waveform fully and dim the spectrogram to make room for it, rather than the other way
round.

`--spectrum` adds a pane whose horizontal axis is **frequency**, not time: the mean level
of every FFT bin across the drawn span as a filled trace, with the loudest each bin ever
reached as a line above it. It is the still-picture answer to the window's realtime
spectrum, which has no meaning with nothing playing. It is read back off the same analysis
that drew the spectrogram, so the two always agree — and its resolution at the low end
follows `--window`: at 2048 and 48 kHz the bins are 23 Hz apart, so everything below about
100 Hz is four bins wide and looks like a staircase. Lengthen the window to smooth it.

Because its axis is frequency, the spectrum pane sits *below* the shared time axis with an
axis of its own, rather than among the lanes that run along time.

### Vertical zoom

`--wave-zoom` is the window's vertical zoom, and it does the same thing: multiply the
amplitude and clip at the edge of the lane, from 1 to 4096 (about 72 dB). It applies to the
linear scale only — the decibel scale already shows the whole range, which is why it is the
default here. The decibel rulings on a zoomed linear lane move with the zoom, so the labels
keep saying what the levels are rather than just getting bigger.

Only the visible span is analysed, padded by one window on each side so the edges of a
zoom are not darkened by the analysis running off the end of the slice. Within the span
the transform is recomputed at one analysis column per pixel column, so a zoom is as sharp
as the window length allows rather than as sharp as the default hop happens to be.

### The sidecar

`--json` adds to the report:

```jsonc
"view": {
  "image": "click.png",
  "start_secs": 0.44, "end_secs": 0.62,
  "start_frames": 21120, "end_frames": 29760,
  "channels": [0],
  "width_px": 1600, "spectrogram_height_px": 340,
  "secs_per_pixel": 0.0001125,     // x_pixel * this + start_secs = time in the file
  "min_hz": 20.0, "max_hz": 24000.0, "log_frequency": true
},
"analysis": {
  "window_size": 1024, "window": "Hann", "overlap": "3/4", "hop": 256,
  "reassigned": false, "db_floor": -90.0, "db_ceiling": 0.0,
  "contrast": 1.0, "colormap": "Amber"
}
```

Between them, those two blocks are enough to draw the same picture again, and to turn a
position in the image back into a position in the file.

The sidecar's `loudness` and `channels` still describe **the whole file**, not the span
drawn — the picture is a view of a file, and the report beside it says what the file is, as
the window's capture does. To measure the span, use `analyze --start --end`.

---

## Options

Common:

| Option | Default | |
| :--- | :--- | :--- |
| `-o, --output PATH` | stdout for `analyze`, `<stem>.png` in the working directory for `render` | |
| `--quiet` | off | no progress on stderr |
| `-h, --help` `-V, --version` | | |

`analyze`:

| Option | Default | |
| :--- | :--- | :--- |
| `--start SECS` `--end SECS` | whole file | measure only this span; adds a `region` block |
| `--structure` | off | lead, tail, pauses, breaths, speech ratio |
| `--segments` | off | every stretch of the file with its level, two readings |
| `--defects` | off | digital silence, clicks, seams, truncation |
| `--spectral` | off | centroid, rolloff, flatness, codec ceiling, hum, bands |
| `--all` | off | all four of the above |
| `--timeline` | off | loudness and level against time, one entry per 100 ms |
| `--compact` | off | one line of JSON |
| `--channel N` | 0 | which channel decides structure and defects |
| `--speech-db DB` | 10 | how far over the floor counts as speech |
| `--min-speech-ms MS` | 100 | shorter runs are not speech |
| `--min-gap-ms MS` | 80 | shorter dips are not pauses |
| `--zero-run-ms MS` | 1 | shortest run of zeros worth reporting |
| `--click-db DB` | 32 | step over the local 20 ms RMS that counts as a click |
| `--spectral-window N` | 8192 | FFT size for `--spectral` |

`qc`:

| Option | Default | |
| :--- | :--- | :--- |
| `--spec PATH` | **required** | the delivery spec, as TOML — see [Writing a QC spec](QC_SPECS.md) |
| `--fail-only` | off | leave passing files out of the output |
| `--render-failures DIR` | off | a PNG per failing file, zoomed on the first finding |

A folder, for either of them:

| Option | Default | |
| :--- | :--- | :--- |
| `--csv` | off | a flat table instead of one JSON object per line |
| `--jsonl` | default | one JSON object per line; the explicit opposite of `--csv` |
| `--jobs N` | one per core | how many files at a time |

`render`:

| Option | Default | |
| :--- | :--- | :--- |
| `--start SECS` `--end SECS` | whole file | the span to draw |
| `--channel N` | all | one channel, counting from 0 |
| `--width PX` | 1600 | width of the plotted area, not of the image |
| `--height PX` | 340 | each spectrogram lane |
| `--wave-height PX` | 90 | each waveform lane; `0` drops them |
| `--no-waveform` | off | same as `--wave-height 0` |
| `--no-spectrogram` | off | waveform and/or spectrum only |
| `--no-merge` | off | waveform *above* the spectrogram instead of over it |
| `--merge-opacity F` | 1.0 | how strongly the overlaid waveform is drawn |
| `--merge-spec-opacity F` | 0.55 | how much of the spectrogram shows through it |
| `--spectrum` | off | add a level-against-frequency pane for the whole span |
| `--spectrum-height PX` | 150 | how tall it is |
| `--wave-scale linear\|db` | `linear` | amplitude with a dB ruler, or an envelope reshaped into dB |
| `--wave-zoom F` | 1 | vertical zoom for the linear scale, 1 to 4096 |
| `--wave-db FLOOR` | −90 | bottom of the dB waveform, independent of `--db` |
| `--wave-color C` | blue | `#rrggbb` or `r,g,b` |
| `--window N` | 2048 | 256, 512, 1024, 2048, 4096, 8192, 16384 |
| `--overlap PCT` | 75 | `75`, `75%` or `3/4` |
| `--window-fn NAME` | `hann` | `hann` `hamming` `blackman` `blackman-harris` `rectangular` |
| `--reassign` | off | time–frequency reassignment: thin lines, single-column clicks. Slower |
| `--db MIN:MAX` | `-115:-9` | decibels mapped across the colour map |
| `--contrast F` | 0.92 | gamma on the level before colouring, 0.25 to 4 |
| `--colormap NAME` | `inferno` | `amber` `ember` `magma` `inferno` `viridis` `plasma` `turbo` `grey` |
| `--min-hz HZ` | 0 linear, 20 log | bottom of the frequency axis |
| `--max-hz HZ` | Nyquist | top of it |
| `--log` | off | logarithmic frequency axis instead of linear |
| `--linear` `--merge` | default | the explicit opposites of `--log` and `--no-merge` |
| `--no-axes` | off | the bare spectrogram at exactly `--width` × `--height`, no margins |
| `--json PATH` | | the report, with the framing |

Names ignore case and punctuation, so `blackmanharris`, `blackman-harris` and
`Blackman Harris` are the same thing. A value that will not parse names itself:

```
$ auriscope-cli render take.wav --window 3000
auriscope-cli: --window 3000: one of 256 512 1024 2048 4096 8192 16384
```

---

## Exit codes and streams

- **stdout** carries only the JSON that was asked for. Nothing else is ever written there.
- **stderr** carries progress and errors, prefixed `auriscope-cli:`. `--quiet` removes the
  progress but never the errors.
- **Exit 0** on success, **1** when `qc` failed a file, **2** when the tool could not do its
  job. A script can tell "the audio is wrong" from "the run is broken", which one code
  cannot.

Inside a folder run, a file that will not decode is reported in its own row and does not
stop the others or corrupt the output:

```bash
auriscope-cli analyze takes/ --quiet |
  jq -s 'map({name: .file.name, lufs: .loudness.integrated_lufs, tp: .channels[0].true_peak_dbtp})'
```

---

## Working with it

### Numbers first, picture second

`analyze` is exact and cheap — a few milliseconds for a short take, under half a second
for two minutes, most of it spent decoding. A render is for the eye, and eyes are good at
*where* and *what kind* and bad at *how much*.

Never state a level, a duration or a time by looking at a picture. If a tail looks like
"about 200 ms", render that span and read the axis, or measure it. Use the picture to
decide where to look, then go back for numbers — and when reporting, say which of the two
a claim came from.

### Choosing the window

The one setting that changes what is visible at all. A long window separates frequencies,
a short one separates events in time, and no window does both.

| | |
| :--- | :--- |
| **256–512** | clicks, edit seams, hard cuts, plosives — short events stop smearing |
| **2048** | speech, and looking around in general |
| **8192–16384** | hum and its harmonics, pitch, closely spaced partials |

A vertical smear whose timing you need means shorten the window. A thick line whose
frequency you need means lengthen it.

### What things look like

- **Digital silence** — a black band spanning the full height, with the neighbouring room
  tone visible as a coloured wash on either side.
- **A hard cut with no crossfade** — a vertical broadband line at the join, often at both
  ends of an inserted region, frequently with a step in the noise floor's colour across the
  boundary. A noise floor that changes brightness at a vertical line *is* an edit.
- **A click or spike** — a narrow vertical line crossing many octaves. `--reassign`
  collapses it to about one column, which is how to time it.
- **A codec bandwidth limit** — a horizontal ceiling with black above: roughly 11 kHz for
  64 kbps MP3, 15–16 kHz for 128 kbps. A "WAV" showing one was transcoded.
- **Mains hum** — steady horizontal lines at 50 or 60 Hz and whole multiples.
- **Room tone** — a uniform wash at the bottom of the lane. Its colour against the bar
  gives a rough level; `rms_dbfs` over a silent stretch gives the real one.
- **Clipping** — not a judgement to make by eye. `clipped_samples` and `clipped_runs`
  count it exactly.

### What it will not tell you

No diarisation, so **crosstalk and speaker identity** are out of reach — a second voice may
show as overlapping harmonics, but naming who is talking needs a different tool. **Mouth
noises** are sometimes visible as short transients around 2–6 kHz, but a lip smack and a
consonant are not reliably separable by looking. And nothing here judges whether something
*sounds* bad; it measures and it draws.
