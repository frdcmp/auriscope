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
- [`render`](#render)
- [Options](#options)
- [Exit codes and streams](#exit-codes-and-streams)
- [Working with it](#working-with-it)

---

## Commands

```
auriscope-cli analyze FILE [options]   # what the file is and what it measures, as JSON
auriscope-cli render  FILE [options]   # an annotated PNG of the waveform and spectrogram
auriscope-cli --help
auriscope-cli --version
```

One file per invocation. Anything Symphonia decodes works: WAV, AIFF, CAF, FLAC, ALAC,
APE, MP3, AAC, OGG/Vorbis, Matroska. Neither command ever writes to the audio.

---

## `analyze`

One JSON object on stdout.

```bash
auriscope-cli analyze take.wav --quiet | jq .loudness
```

### Shape

```jsonc
{
  "auriscope": { "version": "0.1.3", "analysed_unix": 1789642923 },

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
    "modified_unix": 1789526375,
    "tags": [{ "name": "...", "value": "..." }]
  },

  "loudness": {
    "integrated_lufs": -23.34,       // EBU R128, whole file
    "range_lu": 0.0,
    "max_momentary_lufs": -21.59,    // 400 ms window
    "max_short_term_lufs": -30.34,   // 3 s window
    "correlation": null,             // channels 0 and 1, null if mono
    "headroom_db": 8.59,             // to 0 dBTP
    "crest_factor_db": 15.31         // peak minus RMS
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
      "clipped_runs": 0              // consecutive runs at or above 0.999
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

### Options

`--compact` writes one line instead of indented. `-o PATH` writes to a file instead of
stdout. `--quiet` silences the progress on stderr.

---

## `render`

Writes a PNG. With `--json PATH`, also writes the `analyze` report with two extra blocks
saying how the picture was framed.

```bash
auriscope-cli render take.wav -o take.png
auriscope-cli render take.wav --start 3.44 --end 3.62 --window 1024 -o click.png
```

### What is drawn

One channel per pair of lanes — a waveform above, a spectrogram below — stacked down the
image, all sharing one time axis whose gridlines line up across every lane.

- **Time** runs left to right, labelled in seconds, or `m:ss` once the span passes a
  minute. The unit is written under the last tick.
- **Frequency** runs bottom to top, logarithmic from 20 Hz by default. The lane title says
  which (`Hz, log` or `Hz, linear`).
- **Level** is the colour, with the bar down the right mapping colour to decibels.
- **The waveform lane is on a decibel scale by default** (lane title `· dBFS`), peak span
  in one colour with RMS drawn inside it. `--wave-scale linear` switches to amplitude.

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
| `--compact` | off | one line of JSON |

`render`:

| Option | Default | |
| :--- | :--- | :--- |
| `--start SECS` `--end SECS` | whole file | the span to draw |
| `--channel N` | all | one channel, counting from 0 |
| `--width PX` | 1600 | width of the plotted area, not of the image |
| `--height PX` | 340 | each spectrogram lane |
| `--wave-height PX` | 90 | each waveform lane; `0` drops them |
| `--no-waveform` | off | same as `--wave-height 0` |
| `--wave-scale db\|linear` | `db` | decibel or amplitude waveform |
| `--window N` | 2048 | 256, 512, 1024, 2048, 4096, 8192, 16384 |
| `--overlap PCT` | 75 | `75`, `75%` or `3/4` |
| `--window-fn NAME` | `hann` | `hann` `hamming` `blackman` `blackman-harris` `rectangular` |
| `--reassign` | off | time–frequency reassignment: thin lines, single-column clicks. Slower |
| `--db MIN:MAX` | `-90:0` | decibels mapped across the colour map |
| `--contrast F` | 1.0 | gamma on the level before colouring, 0.25 to 4 |
| `--colormap NAME` | `amber` | `amber` `ember` `magma` `inferno` `viridis` `plasma` `turbo` `grey` |
| `--min-hz HZ` | 20 log, 0 linear | bottom of the frequency axis |
| `--max-hz HZ` | Nyquist | top of it |
| `--linear` | off | linear frequency axis |
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
- **Exit 0** on success, **2** on any error.

So a folder sweep is a plain loop, and a failure inside it is visible without corrupting
the output:

```bash
for f in takes/*.wav; do
  auriscope-cli analyze "$f" --quiet --compact
done | jq -s 'map({name: .file.name, lufs: .loudness.integrated_lufs, tp: .channels[0].true_peak_dbtp})'
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
