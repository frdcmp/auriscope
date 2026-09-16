# Auriscope

An audio player and analyser for Linux and Windows. Play a file, and *see* it —
waveform, spectrogram and realtime spectrum, in the spirit of Sound Forge and
iZotope RX.

> An auriscope is the instrument a doctor uses to look inside the ear.
> This one is for looking inside the audio.

**Status:** day zero. Nothing is implemented yet; this document is the plan.

---

## What it is

A fast, native desktop application that opens an audio file and gives you three
synchronised views of it:

- **Waveform** — peak/RMS envelope, zoomable from the whole file down to
  individual samples, with per-channel display.
- **Spectrogram** — STFT heatmap with configurable window size, overlap and
  colour map. The view you actually diagnose problems in.
- **Spectrum** — realtime FFT of the playing region, with adjustable averaging.

Plus the things that make it usable as a daily player: transport controls,
sample-accurate seeking by clicking the waveform, loop regions, gain and pan,
and a clear readout of what the file actually *is* (rate, depth, channels,
codec, duration).

It is a **player and analyser, not an editor**. It does not modify your files.
Destructive editing is explicitly out of scope — that keeps the architecture
simple and the tool trustworthy.

## Stack

Rust throughout. The choices below are deliberate; the reasoning matters more
than the versions.

| Concern | Crate | Why |
|---|---|---|
| GUI | `eframe` / `egui` 0.36 | Immediate-mode, which suits a UI that redraws continuously anyway. One static binary per platform with no GTK/Qt/webview dependency chain — the single biggest saving on Windows packaging. |
| Rendering | `wgpu` (via `eframe`) | The spectrogram is a GPU texture, not a CPU-blitted image. Vulkan on Linux, DX12 on Windows, same code. |
| Decoding | `symphonia` 0.6 | Pure Rust. WAV, FLAC, MP3, AAC, ALAC, OGG/Vorbis with no FFmpeg to link, licence-audit or ship. |
| Output | `cpal` 0.18 | PipeWire/ALSA on Linux, WASAPI on Windows, one API. |
| FFT | `realfft` 3.5 | Wraps `rustfft` but exploits real-valued input — roughly twice the throughput, which matters when the spectrogram is recomputed on every zoom. |
| Resampling | `rubato` 5.0 | High-quality async resampling for files whose rate does not match the output device. |
| Thread plumbing | `rtrb` 0.4 | Lock-free SPSC ring buffers. See below. |

GUI choice, stated plainly: **egui over Tauri.** Tauri would mean drawing a
spectrogram into a canvas from JavaScript and moving audio frames across the
webview boundary sixty times a second. For a tool whose entire job is
high-rate custom drawing, that boundary is the wrong place to be.

## Architecture

The one rule that shapes everything: **the audio callback never blocks.** No
locks, no allocation, no file I/O, no logging on that thread. An underrun is
an audible click, and clicks in a tool people use to *hunt* for clicks are
unacceptable.

That forces a four-thread design connected by lock-free ring buffers:

```
  ┌─────────────┐   decoded frames    ┌──────────────┐   device frames
  │   Decoder   │ ──────rtrb───────▶  │    Audio     │ ──────────────▶  cpal
  │   thread    │                     │   callback   │
  │ (symphonia) │                     │ (realtime)   │
  └─────────────┘                     └──────┬───────┘
         │                                   │ tap (copy out)
         │ peak/RMS pyramid                  ▼
         │                            ┌──────────────┐
         │                            │   Analysis   │
         │                            │    thread    │
         │                            │ (STFT/realfft)│
         └──────────────┬─────────────└──────┬───────┘
                        ▼                    ▼
                  ┌───────────────────────────────┐
                  │           UI thread           │
                  │      (egui, ~60 fps)          │
                  └───────────────────────────────┘
```

- **Decoder** demuxes and decodes ahead of playback, resamples if needed, and
  pushes into the playback ring. It also builds the multi-resolution
  peak/RMS pyramid used to draw the waveform at any zoom without re-reading
  the file.
- **Audio callback** does nothing but copy from the ring, apply gain, and hand
  frames to the device. It publishes its playhead position through an atomic.
- **Analysis** consumes a tap of the same frames, runs the windowed FFT, and
  writes magnitude columns into a GPU-uploadable ring.
- **UI** owns no audio state. It reads atomics and the analysis ring, and
  draws. If it stutters, the audio does not.

The waveform pyramid is the other thing worth getting right early: precompute
min/max/RMS at successive decimation levels so that drawing a two-hour file
zoomed all the way out is a read of a few thousand precomputed values, not a
scan of hundreds of millions of samples.

## Roadmap

**M1 — it plays.** Open a WAV, decode with Symphonia, output through cpal,
transport controls, correct handling of rate/channel mismatch. No views yet.

**M2 — it draws.** Waveform with the peak pyramid, click-to-seek, zoom and
scroll, playhead tracking.

**M3 — it analyses.** Spectrogram on the GPU, realtime spectrum, window and
overlap controls, dB scaling, colour maps.

**M4 — it is a tool.** Loop regions, A/B markers, channel solo, metadata
panel, keyboard-driven navigation, session persistence.

**M5 — it ships.** Flathub (`com.auriscope.Auriscope`), AUR, and an MSI or
portable `.exe` for Windows via `cargo-dist`.

## Building

Requires a recent stable Rust toolchain.

```sh
cargo run --release
```

`--release` is not optional in practice — FFT and waveform decimation in a
debug build are slow enough to be misleading.

**Linux** additionally needs ALSA development headers, which PipeWire systems
still use for the cpal backend:

```sh
# Arch
sudo pacman -S alsa-lib
# Debian/Ubuntu
sudo apt install libasound2-dev
```

**Windows** needs no extra system dependencies; WASAPI and DX12 are part of
the OS.

## Open decisions

- **Licence** — not yet chosen. Flathub distribution and the eventual
  open-sourcing of this repo both depend on it. GPL-3.0 or MPL-2.0 are the
  likely candidates.
- **Plugin hosting** — whether to ever load CLAP/VST3 for analysis plugins.
  Probably not; it conflicts with "player, not editor".
- **File formats beyond Symphonia's set** — Opus and WavPack would need
  additional crates.
