<div align="center">
  <img src="assets/icon.png" width="96" alt="Auriscope logo" />

  # Auriscope

  ### An Audio Player & Analyser for Linux and Windows

  *Play a file, and **see** it — waveform, spectrogram and realtime spectrum, in the spirit of Sound Forge and iZotope RX.*

  [![License: GPL v3](https://img.shields.io/badge/License-GPLv3-b6377a.svg?style=flat-square)](LICENSE)
  [![Rust](https://img.shields.io/badge/Rust-1.98+-3a4150.svg?logo=rust&logoColor=white&style=flat-square)](https://www.rust-lang.org/)
  [![GUI](https://img.shields.io/badge/GUI-egui_0.36_%2F_wgpu-5b1878.svg?style=flat-square)](https://github.com/emilk/egui)
  [![Decoding](https://img.shields.io/badge/Decode-Symphonia_0.6-9c2a6f.svg?style=flat-square)](https://github.com/pdeljanov/Symphonia)
  [![Platform](https://img.shields.io/badge/Platform-Linux_%7C_Windows-2b2b35.svg?style=flat-square)](#-quick-start)
  ![Status](https://img.shields.io/badge/Status-M1--M4_complete-3fa46a.svg?style=flat-square)
</div>

> An auriscope is the instrument a doctor uses to look inside the ear.
> This one is for looking inside the audio.

---

<div align="center">
  <img src="assets/screenshot.png" width="100%" alt="Auriscope showing a log sweep and a 1 kHz tone, with a clipped region flagged in red" />
  <sub><i>A synthetic test file: log sweep on the left channel, 1 kHz tone plus impulses on the right, and a deliberately clipped burst at 0:05 flagged in red.</i></sub>
</div>

---

**Auriscope** opens an audio file and shows you what is actually in it. Three synchronised views — waveform, whole-file spectrogram and realtime spectrum — sit over a playback engine whose audio callback never blocks, never allocates and never takes a lock. It is a **player and analyser, not an editor**: it never modifies your files, which keeps the architecture simple and the tool trustworthy.

---

## ✨ Features

### 🌊 1. Waveform
Peak and RMS envelope drawn from a multi-resolution pyramid, so a two-hour file zoomed all the way out is a read of a few thousand precomputed values rather than a scan of hundreds of millions of samples. Per-channel display, a dBFS scale that follows the vertical zoom, clipped runs highlighted in red, zoom from the whole file down to individual samples.

### 🔥 2. Spectrogram
An STFT heatmap of the **whole file**, available the moment analysis finishes rather than filling in as you listen. Configurable window size, overlap and window function, linear or logarithmic frequency axis, seven colour maps led by an Amber palette in the restoration-suite tradition, a contrast curve, and adjustable dB floor and ceiling. Hovering reads out time, frequency, level and channel. This is the view you actually diagnose problems in.

**It sharpens as you zoom.** The whole-file pass is computed at one hop, so magnifying past a column per pixel would otherwise just enlarge blocks. Zooming in instead triggers a background re-transform of the visible range at whatever hop the current zoom deserves, and sampling interpolates rather than peak-picks wherever the view magnifies. Note the honest limit: hop controls how densely the transform is *sampled*, while true time resolution is set by the window length — to separate events closer together than one window, shorten the window.

**Reassignment, for the RX look.** The reason a restoration suite's spectrogram looks crisper than a textbook STFT is not a finer FFT; it is *time-frequency reassignment*. An ordinary spectrogram paints each bin's energy at the bin's nominal frequency and the frame's nominal time, so a steady tone smears across the window's main lobe and a click smears across every frame that overlaps it. Reassignment computes two extra transforms per frame — against the time-weighted window and the derivative window — which give, per bin, where in time the energy actually sits and what frequency it actually has, then paints it there instead. Tones collapse to hairlines, clicks to single columns. Switch it on in Settings; it costs three FFTs per frame instead of one. Palettes: *Amber* is the restoration-suite look, blue in the quiet half and orange above; *Ember* keeps the warmth without the blue, for use over a blue waveform; *Custom* takes three colours of your own for the quiet, medium and loud ends, over black, with a live preview strip; and the colorous maps (Magma, Inferno, Viridis, Plasma, Turbo) and Grey are there too. The waveform colour is pickable as well, with swatches, so the two views never have to share a hue. Cells more than 70 dB below the frame's peak are left where the plain STFT would put them, because down there the estimates are dominated by leakage and point nowhere useful. Everything else is stored with its *sub-bin position*, one extra byte per cell, and drawn there: rounding to whole bins would turn a gliding harmonic into a staircase one bin tall per step, which at high zoom is many pixels, while smearing it across neighbouring bins would triple the line's width. Recording the fraction keeps lines one bin thin and lets them glide continuously. Zooming keeps drawing the last high-resolution tile while its replacement is computed, and recomputation waits for the view to settle, so the picture never snaps back to the coarse level between wheel notches. The image itself is rendered on a worker thread, in parallel across pixel columns; until it lands, the previous image is drawn shifted and stretched to the new view, so the UI thread never waits on a render and a wheel notch costs a frame, not a redraw.

### 📈 3. Realtime Spectrum
A separate, much cheaper FFT over whatever just went to the output device, with adjustable averaging and a decaying peak-hold trace. Logarithmic frequency axis matching the spectrogram above it.

### 📊 4. Loudness & Delivery Checks
Everything a deliverable check needs, computed in one pass on open:
*   **Loudness:** integrated, short-term and momentary LUFS, loudness range, and true peak (EBU R128 / ITU-R BS.1770).
*   **Clipping:** clipped sample counts *and* run counts, so a single inter-sample kiss is distinguishable from a crushed passage.
*   **Per channel:** sample peak, true peak, RMS and DC offset.
*   **Stereo:** phase correlation, to catch an inverted or collapsing mix.

### ⚙️ 5. One Settings Dialog
A gear in the transport bar (or `Ctrl+,`) opens a modal holding every view control in one place: which panes to show at all — waveform, spectrogram, spectrum, each independently — plus the window size, overlap, window function, colour map, frequency scale and dB range for the spectrogram, the waveform's colour, the RMS overlay, strip height and vertical zoom for the waveform, and the FFT size and averaging for the spectrum. Hiding a pane gives its space to the others. The side panel keeps what you read rather than what you set.

**Merge, if you prefer one pane.** A toggle draws the waveform straight over the spectrogram, sharing a single strip instead of stacking two, with independent opacity for the waveform and for the spectrogram underneath it. The waveform keeps its blue, its dBFS scale moves to the right edge so the frequency axis keeps the left, the divider disappears, the vertical-zoom gesture still applies to the overlay, and clipped runs stay flagged in red on top of the heat map.

### 🎛️ 6. A Real Player
Transport controls, sample-accurate seeking by clicking the waveform, drag-to-select with loop regions, gain and pan, per-channel mute and solo, keyboard-driven navigation, and a clear readout of what the file actually *is* — rate, depth, channels, codec, duration and container tags. Open by double-click, drag-and-drop, `Ctrl+O` or a command-line argument.

### 🗂️ 7. A Side Panel Worth Reading

The right-hand panel is a stack of cards, one typeface, one type scale. **File**: name, folder, container, codec, rate, layout, depth, duration, frames, size on disk, bit rate, footprint in memory, modified time. **WAVE header**, read straight from the RIFF/RF64 structure rather than from the decoder: the raw `fmt ` fields, and every chunk with its offset and size. **Broadcast Wave**, when a `bext` chunk is present: description, originator, reference, origination date and time, the time reference as a timecode, UMID, and the v2 loudness fields and coding history. **Tags** from `LIST INFO` and from the container, **Markers** from `cue ` chunks with their labels. **Loudness**: the R128 set plus the distance to the −23, −16 and −14 LUFS targets, headroom to 0 dBTP and crest factor. **Levels**: per-channel peak, true peak and RMS as bars on a 60 dB scale, with clip and DC warnings and mute/solo. **Analysis**: what the spectrogram is actually computing — window, hop, resolution, reassignment, the detail tile in use, the visible range. **Cursor**: playhead, pointer, selection and loop.

Text is set in [JetBrains Mono Nerd Font](https://www.nerdfonts.com/), bundled: the proportional cut for labels, the mono cut for numbers so columns line up, and its icon glyphs for the card headers. Every size in the app comes from one type scale.

### 🔒 8. Read-Only by Design
Destructive editing is explicitly out of scope. Auriscope opens files and never writes to them. Your settings persist between runs; your audio does not change.

---

## 🏛️ Architecture

Three rules shape everything:

1. **The audio callback never blocks.** No locks, no allocation, no file I/O, no logging on that thread. An underrun is an audible click, and clicks in a tool people use to *hunt* for clicks are unacceptable.
2. **Nothing waits on the audio callback either.** If the analysis tap falls behind, the callback drops frames into it. It never waits for space.
3. **Seeks are generation-stamped.** Every seek bumps an atomic counter. Chunks in the playback ring carry the generation they were produced under, and the callback discards any stale one. That is what makes a seek flush correct without a lock.

```mermaid
flowchart TD
    FILE([Audio file]) --> DEC

    DEC[Decoder thread<br/>Symphonia] -->|generation-tagged chunks| RING[[rtrb ring<br/>lock-free SPSC]]
    RING --> RT[Audio callback<br/>realtime, never blocks]
    RT -->|device frames| OUT([cpal to PipeWire / WASAPI])

    RT -.->|tap, drops on full| TAP[[tap ring]]
    TAP --> LIVE[Live FFT<br/>realfft]

    DEC -->|open pass| PYR[Peak / RMS pyramid]
    DEC -->|open pass| STFT[Spectrogram tiles]
    DEC -->|open pass| LOUD[LUFS, true peak<br/>clip, DC, correlation]

    PYR --> UI
    STFT --> UI
    LOUD --> UI
    LIVE --> UI
    RT -.->|playhead atomic| UI[UI thread<br/>egui, ~60 fps]

    UI ==>|play / pause / seek| DEC
    UI ==>|gain, pan, mute| RT

    classDef store fill:#1b1e25,stroke:#3a4150,stroke-width:2px;
    classDef hot fill:#2a1420,stroke:#b6377a,stroke-width:2px;
    class PYR,STFT,LOUD store;
    class RT hot;
```

*   **Decoder** does two jobs. On open it runs a full background pass that builds everything the views need — the peak/RMS pyramid, the spectrogram tiles, the loudness measurements and the clip/DC/correlation statistics — reporting progress so the UI can draw partial results as they arrive. During playback it decodes ahead of the playhead, resamples if needed, maps channels, and pushes generation-tagged chunks into the playback ring.
*   **Audio callback** copies from the ring, drops stale generations, applies gain and pan, converts to the device sample format, and hands frames to the device. It publishes its playhead through an atomic and copies a tap of each buffer out for the live FFT.
*   **Live FFT** consumes that tap and produces the realtime spectrum only. It is *not* the source of the spectrogram.
*   **UI** owns no audio state. It reads atomics, the cached analysis results and the tap ring, and draws. If it stutters, the audio does not.

### 🔀 Two analysis paths, not one

The whole-file **spectrogram** is an offline STFT computed during the open pass and cached as 8-bit dB columns, max-pooled into coarser levels so that zooming out selects a level rather than recomputing. The **realtime spectrum** is a separate FFT over whatever just went to the device. Keeping these apart is what lets the spectrogram be instant and the live view be honest — a spectrogram fed from the playback tap would fill in only as you listen, which is useless for diagnosis.

### 💾 Memory model

The open pass needs every sample once, and playback needs random access for seeking, so decoded PCM is cached in RAM as `f32`. The intended design caps this (default 2 GB, roughly 90 minutes of 48 kHz stereo) and falls back to re-decoding from disk above the cap, keeping only the pyramid, tiles and measurements. A two-hour 96 kHz stereo file is about 5.5 GB as `f32`, so the fallback is not theoretical — but it is **not implemented yet**, see [Known gaps](#known-gaps).

### ⏱️ Seeking is format-dependent

WAV, AIFF and FLAC seek exactly. MP3 and AAC seek to the nearest packet and then decode-and-discard to the target sample, honouring encoder delay and padding from gapless metadata where present. The UI promises sample accuracy and the decoder delivers it, but the cost differs by codec.

---

## 🛠️ Tech Stack

Rust throughout. The choices are deliberate; the reasoning matters more than the versions.

| Concern | Crate | Why |
| :--- | :--- | :--- |
| **GUI** | `eframe` / `egui` 0.36 | Immediate-mode, which suits a UI that redraws continuously anyway. One static binary per platform with no GTK/Qt/webview dependency chain — the single biggest saving on Windows packaging. |
| **Rendering** | `wgpu` via `eframe` | The spectrogram is a GPU texture, not a CPU-blitted image. Vulkan on Linux, DX12 on Windows, same code. |
| **Decoding** | `symphonia` 0.6 | Pure Rust. WAV, AIFF, CAF, FLAC, ALAC, APE, MP3, AAC, OGG/Vorbis and Matroska with no FFmpeg to link, licence-audit or ship. Lossy codecs sit behind feature flags, enabled explicitly in `Cargo.toml`. |
| **Output** | `cpal` 0.18 | PipeWire/ALSA on Linux, WASAPI on Windows, one API. |
| **FFT** | `realfft` 3.5 | Wraps `rustfft` but exploits real-valued input — roughly twice the throughput, which matters when the whole file is transformed on open. |
| **Resampling** | `rubato` 5.0 | High-quality async sinc resampling for files whose rate does not match the output device. |
| **Loudness** | `ebur128` | Reference R128 implementation, run in the same pass as the pyramid. |
| **Thread plumbing** | `rtrb` 0.4 | Lock-free SPSC ring buffers, for both the playback ring and the analysis tap. |
| **Colour maps** | `colorous` + one of our own | Amber (black, navy, blue, orange, amber, white — the default), plus magma, inferno, viridis, plasma and turbo from `colorous`. A contrast gamma sits on top of whichever is chosen. |
| **File dialogs** | `rfd` | Native open dialogs on both platforms. |

> **GUI choice, stated plainly: egui over Tauri.** Tauri would mean drawing a spectrogram into a canvas from JavaScript and moving audio frames across the webview boundary sixty times a second. For a tool whose entire job is high-rate custom drawing, that boundary is the wrong place to be.

---

## 🗺️ Roadmap

| | Milestone | What it contains |
| :---: | :--- | :--- |
| ✅ | **M0 — it builds** | Cargo skeleton, pinned stable toolchain, dependency feature flags, dev-profile optimisation for dependencies, CI building and testing on Linux and Windows. |
| ✅ | **M1 — it plays** | Any Symphonia-decodable file through cpal, transport controls, rate mismatch via Rubato, channel mismatch via a mapping step, generation-stamped seeking, mute/solo, gain and pan. |
| ✅ | **M2 — it draws** | Waveform from the peak/RMS pyramid, clip highlighting, click-to-seek, drag-to-select, wheel pan, pinch and ctrl-wheel zoom, playhead following, drag-and-drop and CLI open. |
| ✅ | **M3 — it analyses** | Offline per-channel spectrogram with max-pooled zoom levels, linear and log frequency axes, hover readout, realtime spectrum with averaging and peak hold, window/overlap/function controls, dB floor and ceiling, perceptual colour maps. |
| ✅ | **M4 — it measures** | Integrated, short-term and momentary LUFS, loudness range, sample and true peak, clipped sample and run counts, DC offset, RMS, stereo correlation, metadata and tag panel, loop regions, keyboard navigation, persisted settings. *Not yet:* A/B markers. |
| ⬜ | **M5 — it ships** | Flathub as `io.github.frdcmp.Auriscope`, AUR, and an MSI or portable `.exe` for Windows via `cargo-dist`. |

### Known gaps

*   **Large files.** The streaming fallback above the PCM cache cap is not implemented: every file is decoded fully into RAM. The 8-bit spectrogram cache also lives in RAM, roughly one byte per STFT cell per channel.
*   **Spectrogram on the GPU.** The spectrogram is uploaded as a texture, but the log-frequency mapping and resampling for the visible region are done on the CPU into a viewport-sized image whenever the view changes. A shader doing that sampling is the intended end state. The on-demand detail tiles would stay as they are: they add data the GPU does not have, not just a faster way to draw it.
*   **Live spectrum thread.** The realtime FFT runs on the UI thread from the tap ring, not on its own thread. It is one 4096-point real FFT per frame and has not been a problem; it will move if it ever is.
*   **Untested on Windows.** It compiles there in CI. Nobody has heard it.

---

## 🚀 Quick Start

Requires a stable Rust toolchain — `rust-toolchain.toml` pins the exact version.

### 1. System dependencies

**Linux** needs ALSA development headers, which PipeWire systems still use for the cpal backend, plus GTK 3 for the native file dialog. **Windows** needs nothing extra; WASAPI and DX12 are part of the OS.

```bash
# Arch
sudo pacman -S alsa-lib gtk3
# Debian / Ubuntu
sudo apt install libasound2-dev libgtk-3-dev
```

### 2. Run it

```bash
cargo run -- path/to/file.flac
```

> Debug builds are usable because `Cargo.toml` compiles *dependencies* at `opt-level = 3` while keeping your own code at debug settings. Use `--release` for benchmarking and for anything you intend to ship — expect a few minutes, since the release profile uses fat LTO with a single codegen unit.

### 3. Install the binary

```bash
cargo build --release
install -Dm755 target/release/auriscope ~/.local/bin/auriscope
```

With `~/.local/bin` on your `PATH`, `auriscope file.wav` then works from anywhere.

### 4. Desktop integration (Linux)

Register a desktop entry so Auriscope appears in your launcher and in the "Open With" dialog. The filename **must** match the app ID the window sets, or Wayland will not associate the window with the entry.

```bash
cat > ~/.local/share/applications/io.github.frdcmp.Auriscope.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=Auriscope
GenericName=Audio Player and Analyser
Comment=Play a file and see it: waveform, spectrogram, spectrum
Exec=/home/YOU/.local/bin/auriscope %f
Icon=io.github.frdcmp.Auriscope
Terminal=false
Categories=AudioVideo;Audio;Player;
MimeType=audio/wav;audio/x-wav;audio/vnd.wave;audio/flac;audio/x-flac;audio/mpeg;audio/mp4;audio/aac;audio/ogg;audio/x-vorbis+ogg;audio/x-aiff;audio/x-caf;audio/x-ape;audio/x-m4a;audio/x-matroska;
EOF

# Icons, from the SVG in this repo
for s in 16 24 32 48 64 128 256 512; do
  rsvg-convert -w $s -h $s assets/io.github.frdcmp.Auriscope.svg \
    -o ~/.local/share/icons/hicolor/${s}x${s}/apps/io.github.frdcmp.Auriscope.png
done
install -Dm644 assets/io.github.frdcmp.Auriscope.svg \
  ~/.local/share/icons/hicolor/scalable/apps/io.github.frdcmp.Auriscope.svg

update-desktop-database ~/.local/share/applications
gtk-update-icon-cache -f -t ~/.local/share/icons/hicolor
```

Use an **absolute path** in `Exec` — the session that launches desktop files does not always inherit `~/.local/bin` on `PATH` — and `%f` rather than `%U`, since the app wants a plain path and not a URI. To make it the default handler for a type:

```bash
xdg-mime default io.github.frdcmp.Auriscope.desktop audio/x-wav
```

> WAV files resolve to more than one MIME type depending on the file. If one still opens elsewhere, check with `gio info -a standard::content-type yourfile.wav` and claim that exact string too.

### ⌨️ Keyboard

| Key | Action |
| :--- | :--- |
| `Space` | Play / pause |
| `←` `→` | Seek ∓5 s (hold `Shift` for 1 s) |
| `Home` `End` | Jump to start / end |
| Drag | Select. The highlight is transient; the range it made stays on the ruler |
| Click | Seek, and drop the highlight. The ruler range stays |
| Ruler handles | Drag either end of the range |
| Right-click | Clear highlight and range |
| `L` | Loop the ruler range |
| `F` / `Shift+F` | Zoom to the highlight or range / fit whole file |
| `+` `-` | Zoom in / out |
| Middle-drag | Grab and scroll the clip sideways |
| Drag divider | Rebalance waveform against spectrogram (double-click resets) |
| `Esc` | Clear the highlight; press again to clear the range and loop |
| `Ctrl+O` | Open a file |
| `Ctrl+,` | Open the settings dialog |

Click the waveform to seek, drag to select, scroll to zoom at the pointer, `Shift`-scroll to pan, and pinch or `Ctrl`-scroll to zoom. `Alt+Shift`-scroll scales the waveform vertically, from 0.1x to 4096x, which is about 72 dB of boost and enough to lift a noise floor to full height. The side panel carries the same control as a slider with a reset.

Selection works the way a DAW's does. Dragging highlights a region in the waveform and spectrogram and also sets a *range* on the time ruler, shown as a band with a handle at each end. The next click clears the highlight but leaves the range, so you can seek around inside it; the loop plays the range, the handles adjust it, and the ruler band turns green while looping. Right-click or a second `Esc` clears it.

---

## 🧪 Testing

No audio files are committed — the tests synthesise their own signals:

*   A sine at a known frequency must land in the **expected FFT bin at the expected magnitude**, for every supported window function.
*   A synthetic ramp validates **min/max/RMS at every pyramid level**.
*   A generated WAV **round-trips through Symphonia sample-exact**.
*   An EBU R128 reference tone must measure **−23.0 LUFS within tolerance**.
*   The resampling feeder must convert 44.1 → 48 kHz with the tone frequency and peak level preserved, with no audio device involved.

This forces the analysis code to live in a library crate the tests can call, which is also what keeps the UI thin.

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

Two development tools exercise what unit tests cannot reach:

```bash
# Decode, play through the real device for 3 s, seek to 1.5 s mid-play, and
# report playhead progress, tap throughput and underruns. No window.
cargo run --example play -- file.wav 3 1.5

# Open a file, wait for analysis, start playback, capture one frame to PPM
# and exit. This is how the screenshot above was made.
AURISCOPE_SCREENSHOT=shot.ppm cargo run -- file.wav

# Same, but framed on a time range in seconds, which exercises the
# high-resolution detail-tile path instead of the whole-file view.
AURISCOPE_SCREENSHOT=shot.ppm AURISCOPE_SCREENSHOT_ZOOM=1.00,1.06 \
  cargo run -- file.wav

# Open with the settings dialog up, or with a looped range on the ruler.
AURISCOPE_SCREENSHOT=shot.ppm AURISCOPE_SCREENSHOT_SETTINGS=1 cargo run -- file.wav
AURISCOPE_SCREENSHOT=shot.ppm AURISCOPE_SCREENSHOT_RANGE=0.4,0.9 cargo run -- file.wav
```

Time the spectrogram path on a real file — whole-file STFT, detail tiles and the viewport render at four zoom levels, with and without reassignment:

```bash
cargo run --release --example bench_spec -- file.wav 2048 4   # window size, overlap denominator
AURISCOPE_THREADS=1 cargo run --release --example bench_spec -- file.wav   # force single-threaded
```

`AURISCOPE_THREADS` caps the worker threads used by tile analysis and rendering anywhere in the app; the harness also checks that the parallel tile matches the single-threaded one cell for cell.

---

## 📦 Packaging notes

*   **Flathub** requires an app ID under a domain you control. `auriscope.com` is not ours, so the ID is `io.github.frdcmp.Auriscope`. Still needs a metainfo file and offline cargo sources generated with `flatpak-cargo-generator`.
*   **Windows** already sets `#![windows_subsystem = "windows"]` to hide the console; it still needs an icon resource and optional file associations for double-click open.
*   **CI** builds and tests on `ubuntu-latest` and `windows-latest` on every push, so the platform you are not sitting at cannot rot silently.

---

## ⚖️ Licence

**GPL-3.0-or-later.** Anyone may use it. Anyone who changes it and distributes it must publish their changes under the same terms. See [LICENSE](LICENSE).

The bundled JetBrains Mono Nerd Font (`assets/fonts/`) is © The JetBrains Mono Project Authors, licensed under the SIL Open Font License 1.1 (`assets/fonts/OFL.txt`); the Nerd Fonts glyph patch is MIT.

## 🤔 Open decisions

*   **PCM cache cap** — 2 GB default described above; the cap and the streaming fallback are not implemented yet.
*   **Plugin hosting** — whether to ever load CLAP/VST3 for analysis plugins. Probably not; it conflicts with "player, not editor".
*   **File formats beyond Symphonia's set** — Opus and WavPack would need additional crates.
