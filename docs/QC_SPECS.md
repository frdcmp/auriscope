# Writing a QC spec

A spec is the numbers a client agreed to, written in TOML where a person can read and edit
them. `auriscope-cli qc` measures a file and checks it against one:

```bash
auriscope-cli qc take.wav  --spec specs/mine.toml
auriscope-cli qc takes/    --spec specs/mine.toml --csv -o qc.csv
```

The measuring and the judging are deliberately separate. `analyze` takes every measurement
and says nothing about whether it is good; a spec holds all the opinions, in one file,
under version control, that a client can be shown and can argue with.

- [Three rules that govern everything](#three-rules-that-govern-everything)
- [The smallest useful spec](#the-smallest-useful-spec)
- [Every key](#every-key)
- [Severities](#severities)
- [What comes out](#what-comes-out)
- [Choosing the numbers](#choosing-the-numbers)
- [Traps](#traps)
- [A template to copy](#a-template-to-copy)

---

## Three rules that govern everything

**A rule that is not in the file is not checked.** Every key is optional. A spec can start
as two thresholds and grow. Nothing is checked by default, and nothing has a hidden limit —
if the file does not mention true peak, no file will ever fail on true peak.

**A measurement that was not taken is skipped, not failed.** If a spec asks about pauses
and the pass that finds pauses did not run, the rule reports nothing. A mismatch between a
spec and a run under-reports; it never invents a defect. (`qc` turns on the structure and
defect passes for you precisely so this cannot happen quietly.)

**A key it does not recognise is an error.** `silence_max_dbfs_typo = -60` fails the run
with a message naming the line, rather than being ignored and reading as a pass forever.
The same goes for a value of the wrong type.

---

## The smallest useful spec

```toml
name = "Delivery v1"

[format]
sample_rate_hz = 48000
channels = 1
```

```
$ auriscope-cli qc takes/ --spec specs/v1.toml
auriscope-cli: 312 files against Delivery v1
auriscope-cli: 4 of 312 failed
$ echo $?
1
```

`name` is what the report calls the spec. Leave it out and the path is used instead.

---

## Every key

Each table is optional, and so is every key inside it. "Reads" is the field of the
`analyze` report the rule is checked against — useful when a finding surprises you, because
you can go and look at the same number yourself.

### `[format]` — the container

| Key | Type | Rule | Reads |
| :--- | :--- | :--- | :--- |
| `sample_rate_hz` | integer | `sample_rate` | `file.sample_rate_hz` |
| `channels` | integer | `channels` | `file.channels` |
| `bits_per_sample` | integer | `bit_depth` | `file.bits_per_sample` |

Exact equality, all three, all `fail`. These are the cheapest rules there are and the ones
most worth having: a single 44.1 kHz file in a 48 kHz delivery is the kind of thing nobody
hears until the client does.

### `[levels]` — how loud

| Key | Type | Rule | Reads |
| :--- | :--- | :--- | :--- |
| `true_peak_max_dbtp` | dBTP | `true_peak` | loudest channel's `channels[].true_peak_dbtp` |
| `integrated_lufs` | LUFS | `integrated_loudness` | `loudness.integrated_lufs` |
| `integrated_tolerance_lu` | LU | — | the ± around the target. **Default 1.0** |
| `silence_max_dbfs` | dBFS | `silence_too_loud` | every `silence` segment |
| `silence_min_dbfs` | dBFS | `silence_below_band` (**warn**) | every `silence` segment |
| `reading` | `"full_band"` or `"above_80hz"` | — | which measurement the two above use |
| `boundary_step_max_db` | dB | `boundary_step` (**note**) | `structure.boundary_step_db` |

`silence_max_dbfs` and `silence_min_dbfs` are a band, not a pair of independent limits:
above the band the bed is audible, below it the file has been treated to a different tier's
standard. Both are checked over the silent stretches the segmentation found, and the
finding carries every offending time in `at_secs`, so a spectrogram can be pointed at it.

`reading` decides which of two measurements of those stretches the band applies to —
`segments[].rms_dbfs` or `segments[].rms_above_80hz_dbfs`. **The two differ by several
decibels on a voice recorded in a room with any rumble in it**, and which one a client
means is rarely written down anywhere. Both are always measured and both always appear in
the report; this only says which one the limit judges. Full band is the stricter of the two
and is the default.

`boundary_step_max_db` is the floor under the speech against the silence beside it. See
[Severities](#severities) for why it is a note.

### `[structure]` — where the speech is

All in milliseconds. The report holds seconds; the conversion is done for you.

| Key | Type | Rule | Reads |
| :--- | :--- | :--- | :--- |
| `lead_min_ms` / `lead_max_ms` | ms | `leading_silence` | `structure.leading_silence_secs` |
| `tail_min_ms` / `tail_max_ms` | ms | `trailing_silence` | `structure.trailing_silence_secs` |
| `pause_max_ms` | ms | `pause_too_long` | each of `structure.pauses[]` |
| `pause_max_with_event_ms` | ms | `pause_too_long` | the same, for a pause with a breath in it |

Either half of a min/max pair works alone: `lead_max_ms` with no `lead_min_ms` checks only
that the head is not too long.

`pause_max_with_event_ms` is the longer limit that applies to a pause with something inside
it — a breath, a lip noise, anything short and quiet that is not a word. A pause with
nothing in it gets `pause_max_ms`. Leave it out and every pause gets the plain limit.

### `[defects]` — things that should not be there

| Key | Type | Rule | Reads |
| :--- | :--- | :--- | :--- |
| `max_zero_run_ms` | ms | `digital_silence` | `defects.digital_silence.runs[]` |
| `click_db` | dB | — | **a detector setting**, see below |
| `max_clicks_outside_speech` | count | `clicks` | `defects.clicks[]` where `in_speech` is false |
| `max_seams` | count | `seams` | `defects.seams[]` |
| `allow_clipping` | bool | `clipping` | sum of `channels[].clipped_samples` |
| `max_dc_offset` | linear | `dc_offset` | worst `channels[].dc_offset`, absolute |
| `allow_truncation` | bool | `truncated` | `defects.truncation.head` / `.tail` |

`click_db` is the odd one out: it is not a limit, it **changes the measurement**. It is how
far a step has to rise above the surrounding 20 ms RMS to be called a click at all, and
setting it here means `qc` detects with the same sensitivity the spec was written for,
rather than whatever the default happens to be that release. `max_clicks_outside_speech`
is the rule; `click_db` decides what it is counting. Clicks inside speech are never
counted — a plosive is not a defect.

`allow_clipping = false` and `allow_truncation = false` are what turn those checks on.
`true`, or leaving the key out, means not checked. `max_dc_offset` is linear amplitude, not
decibels: `0.001` is a tenth of a per cent of full scale, and a clean file reads about
`0.00001`.

There is no `[spectral]` table. Centroid, rolloff, flatness, the codec ceiling and hum are
all measured and reported, but nothing in a spec can check them yet.

---

## Severities

| | |
| :--- | :--- |
| `fail` | a defect. The file fails, and the run exits 1 |
| `warn` | close to the line, or a question rather than a defect. Reported, does not fail |
| `note` | worth recording and **not the file's fault**. Reported, does not fail |

The middle and bottom earn their keep. Two rules use them today, both deliberately:

**`silence_below_band` is a `warn`.** A file quieter than `silence_min_dbfs` is either
over-corrected or correct for a stricter tier than the one being checked. That is a
question for the client, not a defect in the audio, and it should land on the sheet without
condemning the file.

**`boundary_step` is a `note`.** On a voice whose room is louder than the bed laid between
the words, the floor steps at every speech boundary in every file. It is a property of the
recording. Left as a failure it would fail that speaker's entire delivery and bury the real
defects underneath; left out of the spec altogether, the sheet would not show the thing the
client is going to ask about.

Severity is fixed per rule and cannot be set from the spec file. If a rule needs to fail in
your job and does not, that is a change to the tool, not to the TOML — say so rather than
working around it.

---

## What comes out

One file gets the report with a `qc` block added:

```jsonc
"qc": {
  "spec": "Delivery v1",
  "pass": false,                      // false if anything is `fail`; warns and notes pass
  "counts": { "fail": 1, "note": 1 },
  "findings": [
    {
      "rule": "pause_too_long",
      "severity": "fail",
      "measured": 1240.5,             // the worst one
      "limit": 700.0,
      "at_secs": [3.41, 7.88],        // every place it happened
      "detail": "milliseconds, 2 over the limit"
    }
  ]
}
```

Findings come **worst first**, so a reader who takes only the first line has the most
important one.

A folder gets one such object per line (JSON Lines), or with `--csv` a flat table: the
`analyze` columns plus `qc_pass`, `qc_fails`, `qc_rules` (the failing rule names, space
separated) and `error`. The first column is the bare filename, which is what makes the
table joinable against a delivery inventory.

`--fail-only` drops the passing files. `--render-failures DIR` writes a PNG per failing
file, zoomed to a second either side of the first thing wrong with it.

**Exit 0** when nothing failed, **1** when something did, **2** when the tool could not do
its job. A script can tell "the audio is wrong" from "the run is broken", which one code
cannot.

---

## Choosing the numbers

A threshold invented at a desk fails the wrong files. Measure a batch the client has
already accepted and put the limits outside what it does:

```bash
auriscope-cli analyze accepted/ --all --csv -o accepted.csv --quiet

col() { awk -F, -v n="$1" 'NR==1{for(i=1;i<=NF;i++) if($i==n) c=i; next} c&&$c!=""{print $c}' "$2"; }
col leading_silence_secs accepted.csv | sort -n |
  awk '{v[NR]=$1} END{printf "p05 %.0f ms   p95 %.0f ms\n", v[int(NR*0.05)+1]*1000, v[int(NR*0.95)]*1000}'
```

Then check the rate before trusting the rule. Run the finished spec back over that same
accepted set:

```bash
auriscope-cli qc accepted/ --spec specs/mine.toml --quiet --fail-only |
  jq -r '.qc.findings[] | select(.severity=="fail") | .rule' | sort | uniq -c | sort -rn
```

Anything failing more than a few per cent of files the client has already signed off is
telling you about your threshold, not about the audio. Look at three of them by eye with
`render` before changing anything.

---

## Traps

**Source files and finished files need different specs.** Recordings often carry real
padding of digital silence at each end; `max_zero_run_ms = 1` is right for a delivered file
and will fail every source file, correctly and uselessly.

**`seams` and `clicks` are heuristics.** They look for a step in the noise floor and a step
above the local RMS. Both are new, and a high failure rate is as likely to mean the
threshold is wrong as that the audio is. Calibrate them against known-good material before
quoting a number to anyone.

**`qc` runs the structure and defect passes automatically; it does not run the spectral
one.** Nothing in a spec needs it — but if you want `cutoff_hz` and `transcode_suspect` in
the same CSV, pass `--spectral` as well.

**Only one channel decides structure.** Lead, tail, pauses and defects are measured on
channel 0 unless `--channel N` says otherwise. Format, true peak, clipping and DC offset
read every channel and report the worst.

**`--start` and `--end` apply to `qc` too.** A spec checked against a region is checked
against that region only, and the durations in `[structure]` will be measured from the
region's edges, not the file's.

---

## A template to copy

A commented copy of this template ships beside this document as `starter.toml` — in an
Auriscope checkout that is `specs/starter.toml`, and in the bundled skill it sits next to
this file in `reference/`. Start from it rather than from memory.

One habit is worth copying from any spec that has survived a real delivery: **leave the
open questions in the comments rather than settling them quietly.** Where a client's own
method is undocumented, or two of their checklists disagree, write both numbers down and
say which one is in force. A value nobody remembers choosing is worse than a question in a
comment, and the comment is what makes the spec something a client can be shown.

```toml
name = "My Delivery"

[format]
sample_rate_hz = 48000
channels = 1
bits_per_sample = 24

[levels]
true_peak_max_dbtp = -1.0
integrated_lufs = -23.0
integrated_tolerance_lu = 1.0
silence_max_dbfs = -60.0
silence_min_dbfs = -65.0
reading = "full_band"          # or "above_80hz"
boundary_step_max_db = 6.0     # reported as a note, never fails

[structure]
lead_min_ms = 180.0
lead_max_ms = 300.0
tail_min_ms = 180.0
tail_max_ms = 300.0
pause_max_ms = 700.0
pause_max_with_event_ms = 1000.0

[defects]
max_zero_run_ms = 1.0          # finished files only
click_db = 32.0
max_clicks_outside_speech = 0
max_seams = 0
allow_clipping = false
allow_truncation = false
max_dc_offset = 0.001
```
