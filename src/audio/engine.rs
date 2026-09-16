//! Playback engine.
//!
//! Three rules, enforced here:
//!
//! 1. The audio callback never blocks: no locks, no allocation, no I/O.
//! 2. Nothing waits on the callback: the analysis tap drops on full.
//! 3. Seeks are generation-stamped: every chunk in the ring carries the
//!    generation it was produced under, and the callback discards stale ones.
//!
//! The feeder thread reads from the in-memory PCM cache, resamples when the
//! device rate differs from the file rate, maps channels, and pushes fixed-
//! size chunks into a lock-free SPSC ring. The callback pops chunks, applies
//! gain and pan, converts to the device sample format, and publishes the
//! playhead through an atomic.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample, SupportedStreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};
use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

use super::DecodedAudio;

/// Device frames per ring chunk.
pub const CHUNK_FRAMES: usize = 256;
/// Widest output we will drive.
pub const MAX_OUT_CHANNELS: usize = 16;
/// Ring depth in chunks. 64 x 256 frames is about a third of a second at 48 kHz.
const RING_CHUNKS: usize = 64;
/// Mono tap for the live spectrum, in device frames.
const TAP_CAPACITY: usize = 1 << 16;

struct Chunk {
    generation: u32,
    /// File frame index the first frame of this chunk corresponds to.
    src_pos: u64,
    frames: u16,
    /// Interleaved, `frames * out_channels` valid samples.
    data: [f32; CHUNK_FRAMES * MAX_OUT_CHANNELS],
}

/// State shared between UI, feeder and callback. Atomics only.
pub struct Shared {
    pub playing: AtomicBool,
    /// Current playback position in file frames, written by the callback.
    pub playhead: AtomicU64,
    pub generation: AtomicU32,
    gain_bits: AtomicU32,
    pan_bits: AtomicU32,
    /// Bit `n` set = file channel `n` muted.
    pub mute_mask: AtomicU32,
    pub loop_enabled: AtomicBool,
    pub loop_start: AtomicU64,
    pub loop_end: AtomicU64,
    /// Feeder reached end of file; callback stops when the ring drains.
    pub ended: AtomicBool,
    pub underruns: AtomicU32,
    quit: AtomicBool,
    total_frames: u64,
}

impl Shared {
    fn new(total_frames: u64) -> Self {
        Self {
            playing: AtomicBool::new(false),
            playhead: AtomicU64::new(0),
            generation: AtomicU32::new(1),
            gain_bits: AtomicU32::new(1.0f32.to_bits()),
            pan_bits: AtomicU32::new(0.0f32.to_bits()),
            mute_mask: AtomicU32::new(0),
            loop_enabled: AtomicBool::new(false),
            loop_start: AtomicU64::new(0),
            loop_end: AtomicU64::new(0),
            ended: AtomicBool::new(false),
            underruns: AtomicU32::new(0),
            quit: AtomicBool::new(false),
            total_frames,
        }
    }

    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain_bits.load(Ordering::Relaxed))
    }

    pub fn set_gain(&self, g: f32) {
        self.gain_bits
            .store(g.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// -1 = hard left, 0 = centre, 1 = hard right.
    pub fn pan(&self) -> f32 {
        f32::from_bits(self.pan_bits.load(Ordering::Relaxed))
    }

    pub fn set_pan(&self, p: f32) {
        self.pan_bits
            .store(p.clamp(-1.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn set_loop(&self, region: Option<(u64, u64)>) {
        match region {
            Some((a, b)) if b > a => {
                self.loop_start.store(a, Ordering::Relaxed);
                self.loop_end.store(b, Ordering::Relaxed);
                self.loop_enabled.store(true, Ordering::Relaxed);
            }
            _ => self.loop_enabled.store(false, Ordering::Relaxed),
        }
    }

    fn loop_region(&self) -> Option<(u64, u64)> {
        if self.loop_enabled.load(Ordering::Relaxed) {
            let a = self.loop_start.load(Ordering::Relaxed);
            let b = self.loop_end.load(Ordering::Relaxed);
            (b > a).then_some((a, b))
        } else {
            None
        }
    }
}

enum Cmd {
    Seek(u64),
    Quit,
}

pub struct Engine {
    _stream: cpal::Stream,
    pub shared: Arc<Shared>,
    cmd_tx: mpsc::Sender<Cmd>,
    feeder: Option<JoinHandle<()>>,
    tap: Consumer<f32>,
    pub device_name: String,
    pub device_rate: u32,
    pub device_channels: usize,
    pub sample_format: SampleFormat,
    pub resampling: bool,
}

impl Engine {
    /// Open the default output device for `audio` and start the feeder, paused.
    pub fn new(audio: Arc<DecodedAudio>) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("no default output device"))?;
        let device_name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "unknown device".into());

        let file_rate = audio.sample_rate();
        let default_cfg = device
            .default_output_config()
            .context("default output config")?;
        let cfg = pick_config(&device, &default_cfg, file_rate);
        let device_rate = cfg.sample_rate();
        let device_channels = cfg.channels() as usize;
        if device_channels == 0 || device_channels > MAX_OUT_CHANNELS {
            bail!("unsupported output channel count {device_channels}");
        }
        let sample_format = cfg.sample_format();
        let mut stream_cfg = cfg.config();
        stream_cfg.buffer_size = BufferSize::Default;

        let shared = Arc::new(Shared::new(audio.frames() as u64));
        let (prod, cons) = RingBuffer::<Chunk>::new(RING_CHUNKS);
        let (tap_prod, tap_cons) = RingBuffer::<f32>::new(TAP_CAPACITY);

        let renderer = Renderer {
            cons,
            pending: None,
            offset: 0,
            shared: shared.clone(),
            out_channels: device_channels,
            file_rate: file_rate as u64,
            device_rate: device_rate as u64,
            tap: tap_prod,
            scratch: Vec::new(),
        };

        let err_cb = |e: cpal::Error| log::error!("audio stream error: {e}");
        let stream = match sample_format {
            SampleFormat::F32 => build::<f32>(&device, &stream_cfg, renderer, err_cb)?,
            SampleFormat::I16 => build::<i16>(&device, &stream_cfg, renderer, err_cb)?,
            SampleFormat::I32 => build::<i32>(&device, &stream_cfg, renderer, err_cb)?,
            SampleFormat::U16 => build::<u16>(&device, &stream_cfg, renderer, err_cb)?,
            SampleFormat::F64 => build::<f64>(&device, &stream_cfg, renderer, err_cb)?,
            other => bail!("unsupported device sample format {other:?}"),
        };
        stream.play().context("start stream")?;

        let (cmd_tx, cmd_rx) = mpsc::channel();
        let resampling = device_rate != file_rate;
        let feeder = Feeder::new(audio, prod, shared.clone(), device_rate, device_channels)?;
        let handle = thread::Builder::new()
            .name("auriscope-feeder".into())
            .spawn(move || feeder.run(cmd_rx))
            .context("spawn feeder")?;

        Ok(Self {
            _stream: stream,
            shared,
            cmd_tx,
            feeder: Some(handle),
            tap: tap_cons,
            device_name,
            device_rate,
            device_channels,
            sample_format,
            resampling,
        })
    }

    pub fn play(&self) {
        self.shared.playing.store(true, Ordering::Release);
    }

    pub fn pause(&self) {
        self.shared.playing.store(false, Ordering::Release);
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Acquire)
    }

    pub fn playhead(&self) -> u64 {
        self.shared.playhead.load(Ordering::Acquire)
    }

    pub fn seek(&self, frame: u64) {
        let frame = frame.min(self.shared.total_frames);
        // Make the UI follow immediately; the feeder republishes on arrival.
        self.shared.playhead.store(frame, Ordering::Release);
        self.cmd_tx.send(Cmd::Seek(frame)).ok();
    }

    /// Drain the live-spectrum tap into `f`. Mono, device rate.
    pub fn drain_tap(&mut self, mut f: impl FnMut(&[f32])) {
        let mut buf = [0.0f32; 4096];
        loop {
            let n = self.tap.slots().min(buf.len());
            if n == 0 {
                break;
            }
            let (filled, _) = self.tap.pop_partial_slice(&mut buf[..n]);
            let len = filled.len();
            if len == 0 {
                break;
            }
            f(&buf[..len]);
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::Release);
        self.shared.playing.store(false, Ordering::Release);
        self.cmd_tx.send(Cmd::Quit).ok();
        if let Some(h) = self.feeder.take() {
            h.join().ok();
        }
    }
}

fn pick_config(
    device: &cpal::Device,
    default_cfg: &SupportedStreamConfig,
    file_rate: u32,
) -> SupportedStreamConfig {
    // Prefer running the device at the file's rate so no resampling happens,
    // and prefer f32 so no conversion happens. Fall back to the default.
    let want_channels = default_cfg.channels();
    let Ok(ranges) = device.supported_output_configs() else {
        return *default_cfg;
    };
    let ranges: Vec<_> = ranges.collect();
    let preference = [
        |r: &cpal::SupportedStreamConfigRange, ch: u16, rate: u32| {
            r.channels() == ch && r.sample_format() == SampleFormat::F32 && r.contains_rate(rate)
        },
        |r: &cpal::SupportedStreamConfigRange, ch: u16, rate: u32| {
            r.channels() == ch && r.contains_rate(rate)
        },
    ];
    for pref in preference {
        if let Some(r) = ranges.iter().find(|r| pref(r, want_channels, file_rate)) {
            return (*r).with_sample_rate(file_rate);
        }
    }
    *default_cfg
}

fn build<T>(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    mut renderer: Renderer,
    err_cb: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let stream = device
        .build_output_stream::<T, _, _>(
            *cfg,
            move |out: &mut [T], _info| renderer.render_into(out),
            err_cb,
            None,
        )
        .context("build output stream")?;
    Ok(stream)
}

/// The realtime side. Lives inside the cpal callback.
struct Renderer {
    cons: Consumer<Chunk>,
    pending: Option<Chunk>,
    offset: usize,
    shared: Arc<Shared>,
    out_channels: usize,
    file_rate: u64,
    device_rate: u64,
    tap: Producer<f32>,
    /// Only used for non-f32 devices. Grows once on the first callback.
    scratch: Vec<f32>,
}

impl Renderer {
    fn render_into<T: SizedSample + FromSample<f32>>(&mut self, out: &mut [T]) {
        // Render into the f32 scratch buffer, then convert. The buffer grows
        // once, on the first callback, and never again: the documented
        // exception to the no-allocation rule.
        if self.scratch.len() < out.len() {
            self.scratch.resize(out.len(), 0.0);
        }
        let n = out.len();
        let scratch = &mut self.scratch[..n];
        Self::render(
            &mut self.cons,
            &mut self.pending,
            &mut self.offset,
            &self.shared,
            self.out_channels,
            self.file_rate,
            self.device_rate,
            &mut self.tap,
            scratch,
        );
        for (o, s) in out.iter_mut().zip(scratch.iter()) {
            *o = T::from_sample(*s);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render(
        cons: &mut Consumer<Chunk>,
        pending: &mut Option<Chunk>,
        offset: &mut usize,
        shared: &Shared,
        dch: usize,
        file_rate: u64,
        device_rate: u64,
        tap: &mut Producer<f32>,
        out: &mut [f32],
    ) {
        let frames = out.len() / dch;
        if !shared.playing.load(Ordering::Acquire) {
            out.fill(0.0);
            return;
        }
        let generation = shared.generation.load(Ordering::Acquire);
        let gain = shared.gain();
        let pan = shared.pan();
        let (gl, gr) = if dch == 2 {
            let a = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
            (
                a.cos() * std::f32::consts::SQRT_2,
                a.sin() * std::f32::consts::SQRT_2,
            )
        } else {
            (1.0, 1.0)
        };

        let mut i = 0;
        while i < frames {
            if pending.is_none() {
                match cons.pop() {
                    Ok(c) => {
                        if c.generation != generation {
                            continue;
                        }
                        *pending = Some(c);
                        *offset = 0;
                    }
                    Err(_) => break,
                }
            }
            let c = pending.as_ref().unwrap();
            if c.generation != generation {
                *pending = None;
                continue;
            }
            let avail = c.frames as usize - *offset;
            let n = avail.min(frames - i);
            let src = &c.data[*offset * dch..(*offset + n) * dch];
            let dst = &mut out[i * dch..(i + n) * dch];
            if dch == 2 {
                let (dst2, _) = dst.as_chunks_mut::<2>();
                let (src2, _) = src.as_chunks::<2>();
                for (d, s) in dst2.iter_mut().zip(src2) {
                    d[0] = s[0] * gain * gl;
                    d[1] = s[1] * gain * gr;
                }
            } else {
                for (d, s) in dst.iter_mut().zip(src) {
                    *d = *s * gain;
                }
            }
            *offset += n;
            i += n;
            let pos = c.src_pos + (*offset as u64 * file_rate) / device_rate.max(1);
            shared.playhead.store(pos, Ordering::Release);
            if *offset >= c.frames as usize {
                *pending = None;
            }
        }

        if i < frames {
            out[i * dch..].fill(0.0);
            if shared.ended.load(Ordering::Acquire) {
                shared.playing.store(false, Ordering::Release);
                shared
                    .playhead
                    .store(shared.total_frames, Ordering::Release);
            } else {
                shared.underruns.fetch_add(1, Ordering::Relaxed);
            }
        }

        // Tap: mono mix, dropped on full.
        let mut mono = [0.0f32; 1024];
        let scale = 1.0 / dch as f32;
        let mut f = 0;
        while f < i {
            let n = (i - f).min(mono.len());
            for (k, m) in mono[..n].iter_mut().enumerate() {
                let frame = &out[(f + k) * dch..(f + k + 1) * dch];
                *m = frame.iter().sum::<f32>() * scale;
            }
            let _ = tap.push_partial_slice(&mono[..n]);
            f += n;
        }
    }
}

/// The non-realtime producer.
struct Feeder {
    audio: Arc<DecodedAudio>,
    prod: Producer<Chunk>,
    shared: Arc<Shared>,
    pos: u64,
    total: u64,
    nch: usize,
    dch: usize,
    resampler: Option<Async<f32>>,
    in_bufs: Vec<Vec<f32>>,
    out_bufs: Vec<Vec<f32>>,
    /// Resampler group delay in file frames.
    delay: u64,
}

impl Feeder {
    fn new(
        audio: Arc<DecodedAudio>,
        prod: Producer<Chunk>,
        shared: Arc<Shared>,
        device_rate: u32,
        dch: usize,
    ) -> Result<Self> {
        let nch = audio.channels.len();
        let file_rate = audio.sample_rate();
        let total = audio.frames() as u64;
        let (resampler, in_len, delay) = if device_rate != file_rate {
            let ratio = device_rate as f64 / file_rate as f64;
            let params = SincInterpolationParameters {
                sinc_len: 128,
                f_cutoff: None,
                oversampling_factor: 256,
                interpolation: SincInterpolationType::Cubic,
                window: WindowFunction::BlackmanHarris2,
            };
            let r =
                Async::<f32>::new_sinc(ratio, 1.1, &params, CHUNK_FRAMES, nch, FixedAsync::Output)
                    .map_err(|e| anyhow!("resampler: {e}"))?;
            let in_len = r.input_frames_max();
            let delay = (r.output_delay() as u64 * file_rate as u64) / device_rate as u64;
            (Some(r), in_len, delay)
        } else {
            (None, CHUNK_FRAMES, 0)
        };
        Ok(Self {
            audio,
            prod,
            shared,
            pos: 0,
            total,
            nch,
            dch,
            resampler,
            in_bufs: vec![vec![0.0; in_len]; nch],
            out_bufs: vec![vec![0.0; CHUNK_FRAMES]; nch],
            delay,
        })
    }

    fn run(mut self, rx: mpsc::Receiver<Cmd>) {
        loop {
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    Cmd::Seek(p) => self.seek(p),
                    Cmd::Quit => return,
                }
            }
            if self.shared.quit.load(Ordering::Acquire) {
                return;
            }
            if self.prod.slots() == 0 || self.shared.ended.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            self.produce_chunk();
        }
    }

    fn seek(&mut self, frame: u64) {
        self.pos = frame.min(self.total);
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        if let Some(r) = &mut self.resampler {
            r.reset();
        }
        self.shared.ended.store(false, Ordering::Release);
        self.shared.playhead.store(self.pos, Ordering::Release);
    }

    /// Read `n` frames starting at `self.pos` into `in_bufs`, zero past the
    /// end of the file, honouring the mute mask. Returns nothing; caller
    /// advances `pos`.
    fn read_input(&mut self, n: usize) {
        let mute = self.shared.mute_mask.load(Ordering::Relaxed);
        let start = self.pos as usize;
        for (ch, buf) in self.in_bufs.iter_mut().enumerate() {
            let src = &self.audio.channels[ch];
            let muted = mute & (1 << ch.min(31)) != 0;
            for (k, slot) in buf[..n].iter_mut().enumerate() {
                let idx = start + k;
                *slot = if !muted && idx < src.len() {
                    src[idx]
                } else {
                    0.0
                };
            }
        }
    }

    fn produce_chunk(&mut self) {
        let loop_region = self.shared.loop_region();
        if let Some((ls, le)) = loop_region
            && self.pos >= le
        {
            self.pos = ls;
        }
        if self.pos >= self.total {
            match loop_region {
                Some((ls, _)) => self.pos = ls,
                None => {
                    self.shared.ended.store(true, Ordering::Release);
                    return;
                }
            }
        }
        let boundary = loop_region.map_or(self.total, |(_, le)| le.min(self.total));
        let generation = self.shared.generation.load(Ordering::Acquire);
        let src_pos = self.pos.saturating_sub(self.delay);

        let (frames, advance) = match &mut self.resampler {
            None => {
                let n = ((boundary - self.pos) as usize).min(CHUNK_FRAMES);
                self.read_input(n);
                (n, n as u64)
            }
            Some(_) => {
                let need = self.resampler.as_ref().unwrap().input_frames_next();
                self.read_input(need);
                let r = self.resampler.as_mut().unwrap();
                let input = SequentialSliceOfVecs::new(&self.in_bufs, self.nch, need)
                    .expect("input adapter");
                let mut output =
                    SequentialSliceOfVecs::new_mut(&mut self.out_bufs, self.nch, CHUNK_FRAMES)
                        .expect("output adapter");
                match r.process_into_buffer(&input, &mut output, None) {
                    Ok((used, written)) => (written, used as u64),
                    Err(e) => {
                        log::error!("resampler: {e}");
                        (0, need as u64)
                    }
                }
            }
        };

        let mut chunk = Chunk {
            generation,
            src_pos,
            frames: frames as u16,
            data: [0.0; CHUNK_FRAMES * MAX_OUT_CHANNELS],
        };
        let planar: &[Vec<f32>] = if self.resampler.is_some() {
            &self.out_bufs
        } else {
            &self.in_bufs
        };
        map_channels(planar, frames, self.dch, &mut chunk.data);
        self.pos += advance;
        // Slot availability was checked by the caller; a failure here only
        // means the callback raced us, and the chunk is simply retried.
        if let Err(rtrb::PushError::Full(_)) = self.prod.push(chunk) {
            self.pos -= advance;
        }
    }
}

/// Planar file channels -> interleaved device channels.
fn map_channels(src: &[Vec<f32>], frames: usize, dch: usize, dst: &mut [f32]) {
    let nch = src.len();
    if nch == 0 {
        return;
    }
    for f in 0..frames {
        let out = &mut dst[f * dch..(f + 1) * dch];
        if nch == dch {
            for (c, o) in out.iter_mut().enumerate() {
                *o = src[c][f];
            }
        } else if nch == 1 {
            out.fill(src[0][f]);
        } else if dch == 1 {
            out[0] = src.iter().map(|ch| ch[f]).sum::<f32>() / nch as f32;
        } else {
            for (c, o) in out.iter_mut().enumerate() {
                *o = if c < nch { src[c][f] } else { 0.0 };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_mapping_cases() {
        let l = vec![1.0, 2.0];
        let r = vec![10.0, 20.0];
        let mut out = vec![0.0; 8];

        map_channels(&[l.clone(), r.clone()], 2, 2, &mut out);
        assert_eq!(&out[..4], &[1.0, 10.0, 2.0, 20.0]);

        out.fill(0.0);
        map_channels(std::slice::from_ref(&l), 2, 2, &mut out);
        assert_eq!(&out[..4], &[1.0, 1.0, 2.0, 2.0]);

        out.fill(0.0);
        map_channels(&[l.clone(), r.clone()], 2, 1, &mut out);
        assert_eq!(&out[..2], &[5.5, 11.0]);

        out.fill(0.0);
        map_channels(&[l, r], 2, 4, &mut out);
        assert_eq!(&out[..8], &[1.0, 10.0, 0.0, 0.0, 2.0, 20.0, 0.0, 0.0]);
    }

    #[test]
    fn feeder_resamples_to_device_rate_and_tracks_position() {
        use crate::audio::decoder::FileInfo;
        let file_rate = 44_100u32;
        let device_rate = 48_000u32;
        let n = file_rate as usize; // one second
        let tone: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / file_rate as f32).sin())
            .collect();
        let audio = Arc::new(DecodedAudio {
            info: FileInfo {
                path: "test.wav".into(),
                container: String::new(),
                codec: String::new(),
                sample_rate: file_rate,
                channels: 1,
                bits_per_sample: None,
                frames: n,
                tags: Vec::new(),
            },
            channels: vec![tone],
        });
        let shared = Arc::new(Shared::new(n as u64));
        let (prod, mut cons) = RingBuffer::<Chunk>::new(4);
        let mut feeder = Feeder::new(audio, prod, shared.clone(), device_rate, 2).unwrap();
        assert!(feeder.resampler.is_some());

        let mut out_mono = Vec::new();
        let mut last_src = 0u64;
        while !shared.ended.load(Ordering::Relaxed) {
            while feeder.prod.slots() > 0 && !shared.ended.load(Ordering::Relaxed) {
                feeder.produce_chunk();
            }
            while let Ok(c) = cons.pop() {
                assert!(c.src_pos >= last_src, "src_pos must not go backwards");
                last_src = c.src_pos;
                for f in 0..c.frames as usize {
                    // Mono file -> both device channels carry the same value.
                    assert_eq!(c.data[f * 2], c.data[f * 2 + 1]);
                    out_mono.push(c.data[f * 2]);
                }
            }
        }
        // One second of input becomes about one second at the device rate.
        let expect = device_rate as f32;
        assert!(
            (out_mono.len() as f32 - expect).abs() < expect * 0.02,
            "got {} output frames, expected ~{expect}",
            out_mono.len()
        );
        // The tone survives resampling: count zero crossings in the middle.
        let mid = &out_mono[device_rate as usize / 4..device_rate as usize * 3 / 4];
        let crossings = mid
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        let hz = crossings as f32 / 2.0 / 0.5;
        assert!((hz - 440.0).abs() < 5.0, "resampled tone measured {hz} Hz");
        let peak = mid.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 1.0).abs() < 0.05, "peak {peak}");
    }

    #[test]
    fn pan_law_is_constant_power_at_centre() {
        let a = (0.0f32 + 1.0) * std::f32::consts::FRAC_PI_4;
        let gl = a.cos() * std::f32::consts::SQRT_2;
        let gr = a.sin() * std::f32::consts::SQRT_2;
        assert!((gl - 1.0).abs() < 1e-6);
        assert!((gr - 1.0).abs() < 1e-6);
    }
}
