# Auriscope

An audio player and analyser for Linux and Windows. Play a file, and *see* it —
waveform, spectrogram and realtime spectrum, in the spirit of Sound Forge and
iZotope RX.

> An auriscope is the instrument a doctor uses to look inside the ear.
> This one is for looking inside the audio.

**Status:** M0–M4 implemented and working on Linux; Windows builds in CI but
is untested on real hardware. Packaging (M5) has not started. See
[Roadmap](#roadmap) for what each milestone contains and what is still open.

---

## What it is

A fast, native desktop application that opens an audio file and gives you three
synchronised views of it:

- **Waveform** — peak/RMS envelope, zoomable from the whole file down to
  individual samples, with per-channel display.
- **Spectrogram** — STFT heatmap of the *whole file*, available the moment the
  file is open, with configurable window size, overlap, colour map and a
  linear or logarithmic frequency axis. The view you actually diagnose
  problems in.
- **Spectrum** — realtime FFT of what is currently playing, with adjustable
  averaging.

Plus the measurements a deliverable check needs, computed once on open:

- **Loudness** — integrated, short-term and momentary LUFS and true peak
  (EBU R128 / ITU-R BS.1770), via `ebur128`.
- **Clip and DC-offset detection** — flagged on the waveform, counted in the
  metadata panel.
- **Phase correlation** meter for stereo material.

And the things that make it usable as a daily player: transport controls,
sample-accurate seeking by clicking the waveform, loop regions, gain and pan,
open by drag-and-drop or command-line argument, and a clear readout of what
the file actually *is* (rate, depth, channels, codec, duration).

It is a **player and analyser, not an editor**. It does not modify your files.
Destructive editing is explicitly out of scope — that keeps the architecture
simple and the tool trustworthy.

## Stack

Rust throughout. The choices below are deliberate; the reasoning matters more
than the versions.

| Concern | Crate | Why |
|---|---|---|
| GUI | `eframe` / `egui` 0.36 | Immediate-mode, which suits a UI that redraws continuously anyway. One static binary per platform with no GTK/Qt/webview dependency chain — the single biggest saving on Windows packaging. |
| Rendering | `wgpu` (via `eframe`) | The spectrogram is a GPU texture, not a CPU-blitted image. Vulkan on Linux, DX12 on Windows, same code. Log-frequency display is a texture-sampling problem, not a recompute. |
| Decoding | `symphonia` 0.6 | Pure Rust. WAV, AIFF, CAF, FLAC, ALAC, APE, MP3, AAC, OGG/Vorbis with no FFmpeg to link, licence-audit or ship. Lossy codecs are behind feature flags and must be enabled explicitly (see `Cargo.toml`). |
| Output | `cpal` 0.18 | PipeWire/ALSA on Linux, WASAPI on Windows, one API. |
| FFT | `realfft` 3.5 | Wraps `rustfft` but exploits real-valued input — roughly twice the throughput, which matters when spectrogram tiles are computed for a two-hour file. |
| Resampling | `rubato` 5.0 | High-quality async resampling for files whose rate does not match the output device. |
| Loudness | `ebur128` | Reference R128 implementation; runs in the same pass as the peak pyramid. |
| Thread plumbing | `rtrb` 0.4 | Lock-free SPSC ring buffers. See below. |
| Colour maps | `colorous` | Viridis, magma, inferno — perceptually uniform and colourblind-safe. |
| File dialogs | `rfd` | Native open dialogs on both platforms. |

GUI choice, stated plainly: **egui over Tauri.** Tauri would mean drawing a
spectrogram into a canvas from JavaScript and moving audio frames across the
webview boundary sixty times a second. For a tool whose entire job is
high-rate custom drawing, that boundary is the wrong place to be.

## Architecture

### The rules

1. **The audio callback never blocks.** No locks, no allocation, no file I/O,
   no logging on that thread. An underrun is an audible click, and clicks in a
   tool people use to *hunt* for clicks are unacceptable.
2. **Nothing waits on the audio callback either.** If the analysis tap falls
   behind, the callback drops frames into the tap. It never waits for space.
3. **Seeks are generation-stamped.** Every seek bumps an atomic generation
   counter. Frames in the playback ring carry the generation they were decoded
   under; the callback discards any frame whose generation is stale. This is
   what makes a seek flush correct without a lock.

### Threads

```
         commands (play / pause / seek / gain)
  UI ─────────────────────────────────────────────────▶ Decoder, Audio
                                                          (mpsc + atomics)

  ┌─────────────┐   decoded frames    ┌──────────────┐   device frames
  │   Decoder   │ ──────rtrb───────▶  │    Audio     │ ──────────────▶  cpal
  │   thread    │  (generation-tagged)│   callback   │
  │ (symphonia) │                     │ (realtime)   │
  └──────┬──────┘                     └──────┬───────┘
         │                                   │ tap (copy out, drop on full)
         │ on open: full pass                ▼
         │  · peak/RMS pyramid        ┌──────────────┐
         │  · STFT tiles              │  Live FFT    │
         │  · LUFS / true peak        │   thread     │
         │  · clip / DC / correlation │  (realfft)   │
         ▼                            └──────┬───────┘
  ┌───────────────────────────────────────────▼───────┐
  │                    UI thread                       │
  │                 (egui, ~60 fps)                    │
  └────────────────────────────────────────────────────┘
```

- **Decoder** does two jobs. On open it runs a **full background pass** over
  the file that builds everything the views need: the multi-resolution
  peak/RMS pyramid, the spectrogram tiles, the loudness measurements and the
  clip/DC/correlation statistics. It reports progress so the UI can draw
  partial results as they arrive. During playback it decodes ahead of the
  playhead, resamples if needed, and pushes generation-tagged frames into the
  playback ring.
- **Audio callback** does nothing but copy from the ring, drop stale
  generations, apply gain, convert to the device sample format, and hand
  frames to the device. It publishes its playhead position through an atomic
  and copies a tap of each buffer into the live-FFT ring.
- **Live FFT** consumes the tap and produces the realtime spectrum only. It is
  *not* the source of the spectrogram — that would mean the spectrogram fills
  in as you listen, which is useless for diagnosis.
- **UI** owns no audio state. It reads atomics, the analysis results and the
  live-FFT ring, and draws. If it stutters, the audio does not.

### Two analysis paths, not one

The whole-file **spectrogram** is an offline STFT computed during the open
pass and cached as tiles at two or three hop sizes, so zooming selects a tile
level rather than recomputing. Window size, overlap and colour map changes
trigger a recompute of the visible region first, then the rest in the
background. The **realtime spectrum** is a separate, much cheaper FFT over
whatever just went to the device. Keeping these apart is what lets the
spectrogram be instant and the live view be honest.

### Memory model

The open pass needs every sample once, and playback needs random access for
seeking. Default: **decoded PCM is cached in RAM as f32** up to a configurable
cap (default 2 GB, roughly 90 minutes of 48 kHz stereo). Above the cap only the
pyramid, tiles and measurements are kept, and playback streams by re-decoding
from disk. A two-hour 96 kHz stereo file is about 5.5 GB as f32, so the
fallback is not theoretical. This default is cheap to change later because
nothing outside the decoder sees the difference.

### Seeking is format-dependent

WAV, AIFF and FLAC seek exactly. MP3 and AAC seek to the nearest packet and
then decode-and-discard to the target sample, honouring encoder delay and
padding from gapless metadata where present. The UI promises sample accuracy;
the decoder delivers it, but the cost differs by codec and the first seek in a
lossy file may need a moment.

### Waveform pyramid

Precompute min/max/RMS at successive decimation levels so that drawing a
two-hour file zoomed all the way out is a read of a few thousand precomputed
values, not a scan of hundreds of millions of samples. Clip and DC detection
ride along in the same pass since every sample is already being touched.

## Roadmap

**M0 — it builds.** ✅ Cargo skeleton, pinned stable toolchain, dependency
feature flags, dev-profile optimisation for dependencies, CI building and
testing on Linux and Windows.

**M1 — it plays.** ✅ Any Symphonia-decodable file, output through cpal,
transport controls, rate mismatch handled by Rubato, channel mismatch handled
by a mapping step, generation-stamped seeking, mute/solo, gain and pan.

**M2 — it draws.** ✅ Waveform with the peak/RMS pyramid, clip highlighting,
click-to-seek, drag-to-select, wheel pan, pinch/ctrl-wheel zoom, playhead
following, drag-and-drop and CLI open.

**M3 — it analyses.** ✅ Offline per-channel spectrogram with max-pooled
zoom levels, linear and log frequency axes, hover readout of time/frequency/
level, realtime spectrum with averaging and peak hold, window size, overlap
and window-function controls, dB floor/ceiling, perceptual colour maps.

**M4 — it measures.** ✅ Integrated/short-term/momentary LUFS, loudness
range, sample and true peak, clipped sample and run counts, DC offset, RMS,
stereo correlation, metadata and tag panel, loop region, channel solo/mute,
keyboard navigation, settings persisted across runs. *Not yet:* A/B markers.

**M5 — it ships.** ⬜ Flathub as `io.github.frdcmp.Auriscope`, AUR, and an
MSI or portable `.exe` for Windows via `cargo-dist`.

### Known gaps

- **Large files.** The streaming fallback above the PCM cache cap is not
  implemented: every file is decoded fully into RAM. The 8-bit spectrogram
  cache also lives in RAM, roughly 1 byte per STFT cell per channel.
- **Spectrogram on the GPU.** The spectrogram is uploaded as a texture, but
  the log-frequency mapping and max-pooling for the visible region are done
  on the CPU into a viewport-sized image whenever the view changes. A
  shader doing that sampling is the intended end state.
- **Live spectrum thread.** The realtime FFT runs on the UI thread from the
  tap ring, not on its own thread. It is a 4096-point real FFT per frame and
  has not been a problem; it will move if it ever is.
- **Untested on Windows.** It compiles there in CI. Nobody has heard it.

## Building

Requires a stable Rust toolchain; `rust-toolchain.toml` pins it.

```sh
cargo run
```

Debug builds are usable because `Cargo.toml` compiles *dependencies* at
`opt-level = 3` while keeping your own code at debug settings. FFT and
decimation live in dependencies or hot loops small enough not to matter. Use
`--release` for benchmarking and for anything you intend to ship.

**Linux** additionally needs ALSA development headers, which PipeWire systems
still use for the cpal backend, and GTK 3 headers for the native file dialog:

```sh
# Arch
sudo pacman -S alsa-lib gtk3
# Debian/Ubuntu
sudo apt install libasound2-dev libgtk-3-dev
```

Open a file from the command line, by drag-and-drop, or with Ctrl+O:

```sh
cargo run -- path/to/file.flac
```

**Windows** needs no extra system dependencies; WASAPI and DX12 are part of
the OS.

## Testing

No audio files are committed. Tests synthesise their signals:

- A sine at a known frequency must land in the expected FFT bin, at the
  expected magnitude, for every supported window.
- A synthetic ramp validates min/max/RMS at every pyramid level.
- A generated WAV round-trips through Symphonia sample-exact.
- A known-loudness reference signal (the R128 test vectors are public) must
  measure within tolerance.

This forces the analysis code to live in a library crate the tests can call,
which is also what keeps the UI thin. The resampling feeder is tested the
same way, with no audio device involved.

Two development tools exercise the parts tests cannot reach:

```sh
# Decode, play through the real device for N seconds, seek mid-play, report
# playhead progress and underruns. No window.
cargo run --example play -- file.wav 3 1.5

# Render one frame after the file is analysed, write it as PPM, exit.
AURISCOPE_SCREENSHOT=shot.ppm cargo run -- file.wav
```

## Packaging notes

- **Flathub** requires an app ID under a domain you control. `auriscope.com`
  is not ours, so the ID is `io.github.frdcmp.Auriscope`. Needs a metainfo
  file, a desktop file, and offline cargo sources generated with
  `flatpak-cargo-generator`.
- **Windows** needs `#![windows_subsystem = "windows"]` to hide the console,
  an icon resource, and optional file associations for double-click open.
- **CI** builds and tests on `ubuntu-latest` and `windows-latest` on every
  push, so the platform you are not sitting at cannot rot silently.

## Licence

GPL-3.0-or-later. Anyone may use it; anyone who changes and distributes it
must publish their changes under the same terms. See `LICENSE`.

## Open decisions

- **PCM cache cap** — 2 GB default described above; the cap and the
  streaming fallback are not implemented yet, see Known gaps.
- **Plugin hosting** — whether to ever load CLAP/VST3 for analysis plugins.
  Probably not; it conflicts with "player, not editor".
- **File formats beyond Symphonia's set** — Opus and WavPack would need
  additional crates.
