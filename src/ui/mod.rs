//! The window. Owns no audio state beyond what it needs to draw: it reads
//! atomics from the engine, cached analysis results, and the live tap.

mod capture;
pub mod fonts;
pub mod help;
pub mod icon;
mod panels;
mod spectrum;
pub mod update;
mod util;
mod views;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use eframe::egui;
use egui::TextureHandle;
use serde::{Deserialize, Serialize};

use auriscope::analysis::{
    ColorMap, CustomStops, DEFAULT_CUSTOM, DetailTile, FileStats, LiveSpectrum, Spectrogram,
    StftParams, WaveformPyramid, compute_stats,
};
use auriscope::audio::{DecodedAudio, Engine, decode_file};

/// Default waveform colour: the blue the app has always drawn.
pub const DEFAULT_WAVE_COLOR: [u8; 3] = [86, 156, 214];

/// How many files the history keeps. Long enough to cover a session's worth of
/// takes, short enough that the menu stays a menu.
pub const RECENT_MAX: usize = 12;

/// Everything that survives a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub colormap: ColorMap,
    /// Quiet, medium and loud colours for `ColorMap::Custom`.
    pub custom_stops: CustomStops,
    /// Waveform colour; its RMS overlay is a lighter tint of it.
    pub wave_color: [u8; 3],
    pub stft: StftParams,
    pub db_min: f32,
    pub db_max: f32,
    pub log_frequency: bool,
    pub min_hz: f32,
    pub gain_db: f32,
    pub pan: f32,
    pub spectrum_size: usize,
    pub spectrum_averaging: f32,
    pub waveform_fraction: f32,
    pub show_waveform: bool,
    pub show_spectrogram: bool,
    pub show_spectrum: bool,
    /// Draw the waveform over the spectrogram as one pane instead of two.
    pub merge_views: bool,
    /// Opacity of the overlaid waveform when merged.
    pub merge_opacity: f32,
    /// Opacity of the spectrogram underneath when merged.
    pub merge_spec_opacity: f32,
    /// Gamma on the colour map: 1.0 is linear in dB.
    pub spec_contrast: f32,
    /// Draw a dB scale on the waveform.
    pub show_db_scale: bool,
    /// Vertical (amplitude) zoom of the waveform strip; 1.0 fits ±1.
    pub wave_v_zoom: f32,
    pub follow_playhead: bool,
    pub show_rms: bool,
    pub last_file: Option<PathBuf>,
    /// Keep a history of the files opened, and reopen the newest of them at
    /// startup. Off means nothing about opened files is written to disk.
    pub remember_recent: bool,
    /// The history itself, newest first, at most [`RECENT_MAX`] long.
    pub recent_files: Vec<PathBuf>,
    /// Look for a newer release on GitHub at startup (`update-check` builds).
    pub check_updates: bool,
    /// When the last automatic check ran, in seconds since the Unix epoch.
    pub update_last_check: u64,
    /// A release the user chose not to be reminded of.
    pub update_skipped: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            colormap: ColorMap::Amber,
            custom_stops: DEFAULT_CUSTOM,
            wave_color: DEFAULT_WAVE_COLOR,
            stft: StftParams::default(),
            db_min: -90.0,
            db_max: 0.0,
            log_frequency: true,
            min_hz: 20.0,
            gain_db: 0.0,
            pan: 0.0,
            spectrum_size: 4096,
            spectrum_averaging: 0.6,
            waveform_fraction: 0.3,
            show_waveform: true,
            show_spectrogram: true,
            show_spectrum: true,
            merge_views: false,
            merge_opacity: 0.45,
            merge_spec_opacity: 1.0,
            spec_contrast: 1.0,
            show_db_scale: true,
            wave_v_zoom: 1.0,
            follow_playhead: true,
            show_rms: true,
            last_file: None,
            remember_recent: true,
            recent_files: Vec::new(),
            check_updates: true,
            update_last_check: 0,
            update_skipped: None,
        }
    }
}

impl Settings {
    /// Put `path` at the head of the history and make it the file to reopen
    /// next time. A no-op while the history is switched off, which is what
    /// keeps that setting a real one: nothing about the file is written.
    fn remember_file(&mut self, path: &Path) {
        if !self.remember_recent {
            return;
        }
        self.last_file = Some(path.to_path_buf());
        let recent = &mut self.recent_files;
        // Reopening a file moves it to the top rather than duplicating it.
        recent.retain(|p| p != path);
        recent.insert(0, path.to_path_buf());
        recent.truncate(RECENT_MAX);
    }

    /// Drop one file from the history.
    fn forget_file(&mut self, path: &Path) {
        self.recent_files.retain(|p| p != path);
        if self.last_file.as_deref() == Some(path) {
            self.last_file = None;
        }
    }

    /// Forget every file opened so far, including the one that would reopen
    /// at startup.
    fn clear_recent(&mut self) {
        self.recent_files.clear();
        self.last_file = None;
    }
}

/// Visible frame range, shared by the waveform and the spectrogram.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub start: f64,
    pub end: f64,
}

impl View {
    pub fn len(&self) -> f64 {
        (self.end - self.start).max(1.0)
    }

    pub fn clamp_to(&mut self, total: f64) {
        let len = self.len().min(total.max(1.0));
        if self.start < 0.0 {
            self.start = 0.0;
        }
        if self.start + len > total {
            self.start = (total - len).max(0.0);
        }
        self.end = self.start + len;
    }

    /// Zoom by `factor` (>1 zooms in) keeping frame `anchor` fixed on screen.
    pub fn zoom(&mut self, factor: f64, anchor: f64, total: f64, min_len: f64) {
        let len = (self.len() / factor).clamp(min_len, total.max(min_len));
        let t = ((anchor - self.start) / self.len()).clamp(0.0, 1.0);
        self.start = anchor - t * len;
        self.end = self.start + len;
        self.clamp_to(total);
    }
}

/// Identifies a detail-tile request. Quantised so that small pans do not
/// restart the computation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DetailKey {
    start: i64,
    end: i64,
    columns: usize,
    params: StftParams,
}

/// How long the view must hold still before a detail tile is recomputed.
const DETAIL_SETTLE: std::time::Duration = std::time::Duration::from_millis(80);

/// How long a note in the status bar stays up before it clears itself.
const NOTICE_LIFE: std::time::Duration = std::time::Duration::from_secs(6);

enum Msg {
    Stage(String),
    Progress(f32),
    Decoded(Arc<DecodedAudio>),
    Pyramid(Arc<WaveformPyramid>),
    Spectrogram {
        job: u64,
        channel: usize,
        spec: Arc<Spectrogram>,
    },
    Stats(FileStats),
    Error(String),
    Done,
}

struct Job {
    rx: mpsc::Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    stage: String,
    progress: f32,
}

pub struct App {
    pub settings: Settings,
    audio: Option<Arc<DecodedAudio>>,
    engine: Option<Engine>,
    pyramid: Option<Arc<WaveformPyramid>>,
    spectrograms: Vec<Option<Arc<Spectrogram>>>,
    spec_textures: Vec<Option<(TextureHandle, views::TexKey)>>,
    /// One in-flight render per channel; see `views::draw_spectrogram`.
    spec_render: Vec<Option<views::SpecRender>>,
    stats: Option<FileStats>,
    /// Per-channel high-resolution tiles for the current zoom, when the view
    /// is magnified past the whole-file spectrogram's hop.
    detail: Vec<Option<Arc<DetailTile>>>,
    detail_key: Option<DetailKey>,
    detail_cancel: Option<Arc<AtomicBool>>,
    detail_rx: Option<mpsc::Receiver<(usize, Arc<DetailTile>)>>,
    live: Option<LiveSpectrum>,
    /// Output meter, per device channel: the bar's level and the peak-hold
    /// above it, both linear amplitude. The engine publishes instantaneous
    /// peaks; the fall-off lives here, where the frame time is known.
    pub meter: [MeterChannel; 2],
    job: Option<Job>,
    /// Spectrogram job counter; results from older jobs are ignored.
    spec_job: u64,
    spec_job_progress: Option<(usize, f32)>,
    view: View,
    /// Transient highlight in the waveform and spectrogram, in frames. Goes
    /// away on the next click.
    selection: Option<(f64, f64)>,
    /// The range kept on the ruler, in frames and ordered. Set from a drag,
    /// adjustable by its handles, and what the loop plays.
    range: Option<(f64, f64)>,
    /// Which range handle a ruler drag is moving: 0 = start, 1 = end.
    range_drag: Option<usize>,
    loop_enabled: bool,
    mutes: Vec<bool>,
    solo: Option<usize>,
    /// The one channel picked out by clicking its name in Levels: the file is
    /// then shown and played the way a mono file of that channel would be —
    /// drawn full height, heard on its own out of both speakers rather than
    /// stuck in its own one. Mutually exclusive with `solo`, which hears a
    /// channel in place.
    isolated: Option<usize>,
    error: Option<String>,
    hover_info: String,
    /// Whether the settings dialog is open.
    settings_open: bool,
    /// Whether help mode is on: hovering a label explains it. Session-only —
    /// it is a way of reading the UI, not a preference worth remembering.
    help_mode: bool,
    /// Content height of the settings dialog last frame, so its scroll area
    /// can claim that much: inside a modal the reported available height is
    /// not the screen's.
    settings_content_h: f32,
    /// Dev hook: keep the settings dialog scrolled to the bottom.
    settings_scroll_end: bool,
    /// For debouncing detail-tile requests while the view is still moving.
    last_view: View,
    view_changed_at: Instant,
    /// True while the middle mouse button is dragging the view sideways.
    middle_panning: bool,
    /// Window title shown by our custom title bar.
    title: String,
    /// The app icon beside it.
    icon: TextureHandle,
    pub updater: update::Updater,
    last_tick: Instant,
    pending_open: Option<PathBuf>,
    /// A window capture on its way to disk, from the camera button in the
    /// transport bar. See [`capture`].
    capture: Option<capture::Capture>,
    /// What a capture keeps: the waveform, spectrogram and spectrum panels
    /// together, as they were laid out last frame.
    views_rect: egui::Rect,
    /// A line the status bar shows for a few seconds and then drops: what was
    /// saved, and when it was said.
    notice: Option<(String, Instant)>,
    /// Dev hook: `AURISCOPE_SCREENSHOT=out.png` writes one frame of the whole
    /// window after the file is fully analysed, then exits. Used to eyeball
    /// rendering headless. Unlike the camera button this keeps the chrome, and
    /// a `.ppm` path gets the raw format it has always written.
    screenshot: Option<(PathBuf, Option<Instant>)>,
}

/// Gap between a checkbox or radio button and its label.
const ICON_GAP: f32 = 7.0;

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        let mut settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
            .unwrap_or_default();
        // A stored zoom from a build whose range reached below 1x would leave
        // the strip with dead space above 0 dBFS.
        settings.wave_v_zoom = settings
            .wave_v_zoom
            .clamp(views::V_ZOOM_MIN, views::V_ZOOM_MAX);
        // set_visuals alone loses to egui's system-theme sync, which repaints
        // the app in the desktop's light theme. Setting the *preference* is
        // what actually sticks.
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        fonts::install(&cc.egui_ctx);
        // egui's default of 4 px sets a checkbox's box almost against its
        // label. A little more air reads better, everywhere at once.
        cc.egui_ctx
            .all_styles_mut(|s| s.spacing.icon_spacing = ICON_GAP);
        let pending_open = initial.or_else(|| settings.last_file.clone());
        let mut updater = update::Updater::default();
        if update::ENABLED
            && settings.check_updates
            && update::Updater::due(settings.update_last_check)
        {
            updater.start();
        }
        Self {
            settings,
            audio: None,
            engine: None,
            pyramid: None,
            spectrograms: Vec::new(),
            spec_textures: Vec::new(),
            spec_render: Vec::new(),
            stats: None,
            detail: Vec::new(),
            detail_key: None,
            detail_cancel: None,
            detail_rx: None,
            live: None,
            meter: Default::default(),
            job: None,
            spec_job: 0,
            spec_job_progress: None,
            view: View {
                start: 0.0,
                end: 1.0,
            },
            selection: None,
            range: None,
            range_drag: None,
            loop_enabled: false,
            mutes: Vec::new(),
            solo: None,
            isolated: None,
            error: None,
            hover_info: String::new(),
            settings_open: false,
            help_mode: false,
            settings_content_h: 0.0,
            settings_scroll_end: false,
            last_view: View {
                start: 0.0,
                end: 1.0,
            },
            view_changed_at: Instant::now(),
            middle_panning: false,
            title: "Auriscope".into(),
            icon: icon::title_texture(&cc.egui_ctx),
            updater,
            last_tick: Instant::now(),
            pending_open,
            capture: None,
            views_rect: egui::Rect::NOTHING,
            notice: None,
            screenshot: std::env::var_os("AURISCOPE_SCREENSHOT").map(|p| (PathBuf::from(p), None)),
        }
    }

    fn total_frames(&self) -> f64 {
        self.audio.as_ref().map_or(1.0, |a| a.frames() as f64)
    }

    fn sample_rate(&self) -> f64 {
        self.audio
            .as_ref()
            .map_or(48_000.0, |a| a.sample_rate() as f64)
    }

    fn duration_secs(&self) -> f64 {
        self.audio.as_ref().map_or(0.0, |a| a.info.duration_secs())
    }

    // ---- loading -------------------------------------------------------

    pub fn open(&mut self, path: &Path) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.engine = None;
        self.audio = None;
        self.pyramid = None;
        self.spectrograms.clear();
        self.spec_textures.clear();
        self.spec_render.clear();
        self.detail.clear();
        self.cancel_detail();
        self.stats = None;
        self.live = None;
        self.meter = Default::default();
        self.selection = None;
        self.range = None;
        self.range_drag = None;
        self.loop_enabled = false;
        self.error = None;
        self.spec_job += 1;
        self.spec_job_progress = None;

        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let path = path.to_path_buf();
        let stft = self.settings.stft;
        let job_id = self.spec_job;
        let c = cancel.clone();
        std::thread::Builder::new()
            .name("auriscope-loader".into())
            .spawn(move || loader(path, stft, job_id, tx, c))
            .ok();
        self.job = Some(Job {
            rx,
            cancel,
            stage: "Opening".into(),
            progress: 0.0,
        });
    }

    fn recompute_spectrograms(&mut self) {
        let Some(audio) = self.audio.clone() else {
            return;
        };
        self.spec_job += 1;
        let job_id = self.spec_job;
        let stft = self.settings.stft;
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
        let c = cancel.clone();
        std::thread::spawn(move || {
            spectrogram_pass(&audio, stft, job_id, &tx, &c);
            tx.send(Msg::Done).ok();
        });
        self.job = Some(Job {
            rx,
            cancel,
            stage: "Spectrogram".into(),
            progress: 0.0,
        });
    }

    fn cancel_detail(&mut self) {
        if let Some(c) = self.detail_cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.detail_rx = None;
        self.detail_key = None;
    }

    /// Ask for high-resolution tiles covering the visible range, if the zoom
    /// has gone past what the whole-file spectrogram can resolve on screen.
    ///
    /// `width_px` is the spectrogram's width in physical pixels.
    pub fn request_detail(&mut self, width_px: usize) {
        let Some(audio) = self.audio.clone() else {
            return;
        };
        if self.detail.len() != audio.channels.len() {
            self.detail = (0..audio.channels.len()).map(|_| None).collect();
        }
        let params = self.settings.stft;
        let view_len = self.view.len();
        let frames_per_px = view_len / width_px.max(1) as f64;
        // Nothing to gain until a pixel spans less than one base column.
        if frames_per_px >= params.hop() as f64 || width_px == 0 {
            if self.detail_key.is_some() {
                self.cancel_detail();
                self.detail.iter_mut().for_each(|d| *d = None);
            }
            return;
        }

        // Compute a little beyond the view so small pans stay covered.
        let total = self.total_frames();
        let pad = view_len * 0.2;
        let start = (self.view.start - pad).max(0.0);
        let end = (self.view.end + pad).min(total.max(1.0));
        if end <= start {
            return;
        }
        let want = ((end - start) / frames_per_px).ceil() as usize;
        let columns = want.clamp(1, DetailTile::MAX_COLUMNS);

        // Quantise so that dragging does not respawn the job every frame.
        let q = (view_len / 32.0).max(1.0);
        let key = DetailKey {
            start: (start / q).round() as i64,
            end: (end / q).round() as i64,
            columns,
            params,
        };
        if self.detail_key.as_ref() == Some(&key) {
            return;
        }
        // Let the view settle before recomputing. Each wheel notch would
        // otherwise start and cancel a job, and reassignment makes them
        // three times as expensive. The last tile keeps drawing meanwhile.
        if self.view_changed_at.elapsed() < DETAIL_SETTLE {
            return;
        }

        self.cancel_detail();
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let c = cancel.clone();
        let rate = audio.sample_rate();
        std::thread::Builder::new()
            .name("auriscope-detail".into())
            .spawn(move || {
                for (ch, samples) in audio.channels.iter().enumerate() {
                    if c.load(Ordering::Relaxed) {
                        return;
                    }
                    let tile =
                        DetailTile::compute(samples, rate, params, start, end, columns, &|| {
                            c.load(Ordering::Relaxed)
                        });
                    match tile {
                        Some(t) => {
                            if tx.send((ch, Arc::new(t))).is_err() {
                                return;
                            }
                        }
                        None => return,
                    }
                }
            })
            .ok();
        log::debug!(
            "detail tile requested: {columns} columns over frames {start:.0}..{end:.0} \
             ({:.2} frames/px, base hop {})",
            frames_per_px,
            params.hop()
        );
        self.detail_key = Some(key);
        self.detail_cancel = Some(cancel);
        self.detail_rx = Some(rx);
    }

    fn poll_detail(&mut self) {
        let Some(rx) = &self.detail_rx else {
            return;
        };
        let mut got = Vec::new();
        while let Ok(v) = rx.try_recv() {
            got.push(v);
        }
        for (ch, tile) in got {
            if ch < self.detail.len() {
                log::debug!(
                    "detail tile ready: ch{ch} {} columns, stride {:.1} frames",
                    tile.columns,
                    tile.stride
                );
                self.detail[ch] = Some(tile);
            }
        }
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.job else {
            return;
        };
        let mut finished = false;
        while let Ok(msg) = job.rx.try_recv() {
            match msg {
                Msg::Stage(s) => {
                    job.stage = s;
                    job.progress = 0.0;
                }
                Msg::Progress(p) => job.progress = p,
                Msg::Decoded(audio) => {
                    self.settings.remember_file(&audio.info.path);
                    self.title = format!("{} — Auriscope", audio.info.file_name());
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.title.clone()));
                    self.view = View {
                        start: 0.0,
                        end: audio.frames() as f64,
                    };
                    self.spectrograms = vec![None; audio.channels.len()];
                    self.detail = (0..audio.channels.len()).map(|_| None).collect();
                    self.spec_textures = (0..audio.channels.len()).map(|_| None).collect();
                    self.spec_render = (0..audio.channels.len()).map(|_| None).collect();
                    self.mutes = vec![false; audio.channels.len()];
                    self.solo = None;
                    self.isolated = None;
                    match Engine::new(audio.clone()) {
                        Ok(engine) => {
                            engine
                                .shared
                                .set_gain(auriscope::analysis::db_to_amp(self.settings.gain_db));
                            engine.shared.set_pan(self.settings.pan);
                            self.live = Some(LiveSpectrum::new(
                                self.settings.spectrum_size,
                                engine.device_rate,
                            ));
                            self.engine = Some(engine);
                        }
                        Err(e) => self.error = Some(format!("audio output: {e:#}")),
                    }
                    self.audio = Some(audio);
                }
                Msg::Pyramid(p) => self.pyramid = Some(p),
                Msg::Spectrogram {
                    job: id,
                    channel,
                    spec,
                } => {
                    if id == self.spec_job && channel < self.spectrograms.len() {
                        self.spectrograms[channel] = Some(spec);
                    }
                }
                Msg::Stats(s) => self.stats = Some(s),
                Msg::Error(e) => {
                    self.error = Some(e);
                    finished = true;
                }
                Msg::Done => finished = true,
            }
        }
        if finished {
            self.job = None;
        }
    }

    // ---- transport -------------------------------------------------------

    fn toggle_play(&mut self) {
        let Some(engine) = &self.engine else {
            return;
        };
        if engine.is_playing() {
            engine.pause();
        } else {
            if engine.playhead() >= self.total_frames() as u64 {
                let start = self.loop_region().map_or(0, |(a, _)| a);
                engine.seek(start);
            }
            engine.play();
        }
    }

    fn seek_frames(&mut self, frame: f64) {
        if let Some(engine) = &self.engine {
            engine.seek(frame.clamp(0.0, self.total_frames()) as u64);
        }
    }

    fn seek_relative(&mut self, secs: f64) {
        if let Some(engine) = &self.engine {
            let cur = engine.playhead() as f64;
            let target = cur + secs * self.sample_rate();
            self.seek_frames(target);
        }
    }

    fn loop_region(&self) -> Option<(u64, u64)> {
        if !self.loop_enabled {
            return None;
        }
        self.range
            .map(|(a, b)| (a as u64, b as u64))
            .filter(|(a, b)| b > a)
    }

    fn apply_loop(&self) {
        if let Some(engine) = &self.engine {
            engine.shared.set_loop(self.loop_region());
        }
    }

    /// The channels the views lay out, in order. Normally every one; while a
    /// channel is isolated, just that one, so it gets the whole height.
    pub fn visible_channels(&self, nch: usize) -> std::ops::Range<usize> {
        match self.isolated {
            Some(c) if c < nch => c..c + 1,
            _ => 0..nch,
        }
    }

    /// Whether a channel is silent right now: explicitly muted, or left out
    /// by a channel picked out elsewhere. The views dim what this returns
    /// true for, so the picture says the same thing as the sound. Isolating
    /// or soloing wins over a mute, the way a mixer's solo does.
    pub fn channel_muted(&self, ch: usize) -> bool {
        match self.isolated.or(self.solo) {
            Some(s) => s != ch,
            None => self.mutes.get(ch).copied().unwrap_or(false),
        }
    }

    /// Push the channel routing to the engine: what is silent, and whether
    /// one channel is being played on its own to every speaker.
    fn apply_channel_routing(&self) {
        let Some(engine) = &self.engine else {
            return;
        };
        let mut mask = 0u32;
        for i in 0..self.mutes.len() {
            if self.channel_muted(i) {
                mask |= 1 << i.min(31);
            }
        }
        engine.shared.mute_mask.store(mask, Ordering::Relaxed);
        engine.shared.set_mono_source(self.isolated);
    }

    fn zoom_to_fit(&mut self) {
        let total = self.total_frames();
        self.view = View {
            start: 0.0,
            end: total,
        };
    }

    fn zoom_to_selection(&mut self) {
        if let Some((a, b)) = self.selection.or(self.range)
            && (b - a).abs() > 2.0
        {
            self.view = View {
                start: a.min(b),
                end: a.max(b),
            };
            self.view.clamp_to(self.total_frames());
        }
    }

    // ---- input -----------------------------------------------------------

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let (space, home, end, left, right, l, f, esc, plus, minus, open, comma, shot, f1, shift) =
            ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::Space),
                    i.key_pressed(egui::Key::Home),
                    i.key_pressed(egui::Key::End),
                    i.key_pressed(egui::Key::ArrowLeft),
                    i.key_pressed(egui::Key::ArrowRight),
                    i.key_pressed(egui::Key::L),
                    i.key_pressed(egui::Key::F),
                    i.key_pressed(egui::Key::Escape),
                    // Bare, because egui takes the same keys under Ctrl for
                    // the interface scale. Without the guard one Ctrl+= both
                    // scaled the UI and zoomed the view.
                    !i.modifiers.command
                        && (i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals)),
                    !i.modifiers.command && i.key_pressed(egui::Key::Minus),
                    i.modifiers.command && i.key_pressed(egui::Key::O),
                    i.modifiers.command && i.key_pressed(egui::Key::Comma),
                    i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::S),
                    i.key_pressed(egui::Key::F1),
                    i.modifiers.shift,
                )
            });
        if open {
            self.pick_file();
        }
        if comma {
            self.settings_open = !self.settings_open;
        }
        // Before the early return below, so the dialog being up is no reason
        // not to take a picture; the capture leaves it out of the frame.
        if shot {
            self.save_screenshot();
        }
        // Before the early return below: help mode is as useful over the
        // settings dialog as over the panels, so F1 has to reach it there.
        if f1 {
            self.help_mode = !self.help_mode;
        }
        if esc && self.help_mode {
            self.help_mode = false;
            return;
        }
        // While the dialog is up it owns the keyboard, so the transport and
        // view shortcuts below stay out of its way.
        if self.settings_open {
            return;
        }
        if space {
            self.toggle_play();
        }
        if home {
            self.seek_frames(0.0);
        }
        if end {
            self.seek_frames(self.total_frames());
        }
        let step = if shift { 1.0 } else { 5.0 };
        if left {
            self.seek_relative(-step);
        }
        if right {
            self.seek_relative(step);
        }
        if l {
            self.loop_enabled = !self.loop_enabled && self.range.is_some();
            self.apply_loop();
        }
        if f {
            if (self.selection.is_some() || self.range.is_some()) && !shift {
                self.zoom_to_selection();
            } else {
                self.zoom_to_fit();
            }
        }
        if esc {
            // First Esc drops the highlight, the second the ruler range.
            if self.selection.is_some() {
                self.selection = None;
            } else {
                self.range = None;
                self.loop_enabled = false;
                self.apply_loop();
            }
        }
        let centre = self.view.start + self.view.len() / 2.0;
        if plus {
            self.view.zoom(2.0, centre, self.total_frames(), 64.0);
        }
        if minus {
            self.view.zoom(0.5, centre, self.total_frames(), 64.0);
        }
    }

    fn pick_file(&mut self) {
        let picked = rfd::FileDialog::new()
            .add_filter(
                "Audio",
                &[
                    "wav", "aif", "aiff", "caf", "flac", "alac", "ape", "mp3", "m4a", "aac", "mp4",
                    "ogg", "oga", "mkv", "mka", "webm",
                ],
            )
            .set_title("Open audio file")
            .pick_file();
        if let Some(p) = picked {
            self.open(&p);
        }
    }

    /// Ask where a capture of the views should go, and queue it.
    ///
    /// The dialog comes first so that nothing is left to do once the pixels
    /// arrive: a capture that had to stop and ask would be a capture of the
    /// window with a file dialog in front of it.
    fn save_screenshot(&mut self) {
        if self.capture.is_some() {
            return;
        }
        let source = self.audio.as_ref().map(|a| a.info.path.clone());
        let mut dialog = rfd::FileDialog::new()
            .add_filter("PNG image", &["png"])
            .set_title("Save views as PNG")
            .set_file_name(capture::default_name(source.as_deref()));
        if let Some(dir) = source.as_deref().and_then(Path::parent) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.save_file() {
            self.capture = Some(capture::Capture::Settling(capture::with_png_extension(
                path,
            )));
        }
    }

    /// The status bar's transient line, while it is still fresh. Stale ones
    /// clear themselves here, so nothing else has to keep time.
    fn notice(&mut self) -> Option<&str> {
        if self
            .notice
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > NOTICE_LIFE)
        {
            self.notice = None;
        }
        self.notice.as_ref().map(|(m, _)| m.as_str())
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if let Some(p) = dropped.into_iter().next() {
            self.open(&p);
        }
    }

    fn follow_playhead(&mut self) {
        let Some(engine) = &self.engine else {
            return;
        };
        if !self.settings.follow_playhead || !engine.is_playing() {
            return;
        }
        let ph = engine.playhead() as f64;
        let total = self.total_frames();
        if self.view.len() >= total {
            return;
        }
        if ph < self.view.start || ph >= self.view.end {
            let len = self.view.len();
            self.view.start = (ph - len * 0.1).max(0.0);
            self.view.end = self.view.start + len;
            self.view.clamp_to(total);
        }
    }

    /// Move a queued capture along by one frame: skip the frame that started
    /// it, ask for the next one, then write what comes back. Called last in
    /// the frame, so the grab covers everything drawn in it.
    fn capture_hook(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.capture.take() else {
            return;
        };
        match pending {
            capture::Capture::Settling(path) => {
                self.capture = Some(capture::Capture::Grabbing(path));
                ctx.request_repaint();
            }
            capture::Capture::Grabbing(path) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                self.capture = Some(capture::Capture::Waiting {
                    path,
                    views: self.views_rect,
                    asked: Instant::now(),
                });
                ctx.request_repaint();
            }
            capture::Capture::Waiting { path, views, asked } => {
                let shot = ctx.input(|i| {
                    i.events.iter().find_map(|e| match e {
                        egui::Event::Screenshot { image, .. } => Some(image.clone()),
                        _ => None,
                    })
                });
                let Some(img) = shot else {
                    if asked.elapsed() > capture::GRAB_TIMEOUT {
                        self.error = Some("screenshot: the window was never handed back".into());
                        return;
                    }
                    self.capture = Some(capture::Capture::Waiting { path, views, asked });
                    ctx.request_repaint();
                    return;
                };
                // The grab is of the whole window; only the views are kept.
                let views = capture::crop(&img, views, ctx.pixels_per_point());
                let json = capture::json_path(&path);
                let written = auriscope::plot::write_png(&path, &views)
                    .and_then(|()| capture::write_json(&json, &capture::metadata(self, &path)));
                match written {
                    Ok(()) => {
                        log::info!("saved {} and {}", path.display(), json.display());
                        self.notice = Some((
                            format!("Saved {} and {}", file_name(&path), file_name(&json)),
                            Instant::now(),
                        ));
                    }
                    Err(e) => self.error = Some(format!("screenshot: {e:#}")),
                }
            }
        }
    }

    fn screenshot_hook(&mut self, ctx: &egui::Context) {
        if self.screenshot.is_none() {
            return;
        }
        // Handle the reply first.
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            if let Some((path, _)) = &self.screenshot
                && let Err(e) = write_dev_shot(path, &img)
            {
                log::error!("screenshot: {e}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        let detail_ready = self.detail_key.is_none() || self.detail.iter().all(Option::is_some);
        let ready = self.job.is_none()
            && (self.audio.is_none() || self.spectrograms.iter().all(Option::is_some))
            && detail_ready;
        let armed = self.screenshot.as_ref().map(|(_, t)| *t).unwrap_or(None);
        match armed {
            // Start playing, so the captured frame shows a moving playhead
            // and a populated live spectrum rather than an idle window. No
            // seek: a cold seek costs one underrun and that would show in
            // the status bar of the capture.
            None if ready => {
                // An optional zoom range lets a capture exercise the
                // detail-tile path instead of the whole-file view.
                let zoom = std::env::var("AURISCOPE_SCREENSHOT_ZOOM")
                    .ok()
                    .and_then(|v| {
                        let (a, b) = v.split_once(',')?;
                        let a = a.trim().parse::<f64>().ok()?;
                        let b = b.trim().parse::<f64>().ok()?;
                        (b > a).then_some((a, b))
                    });
                if std::env::var_os("AURISCOPE_SCREENSHOT_SETTINGS").is_some() {
                    self.settings_open = true;
                    // `=end` scrolls the dialog to its last card.
                    self.settings_scroll_end =
                        std::env::var("AURISCOPE_SCREENSHOT_SETTINGS").is_ok_and(|v| v == "end");
                }
                // An optional looped range in seconds, to show the ruler band.
                if let Some((a, b)) =
                    std::env::var("AURISCOPE_SCREENSHOT_RANGE")
                        .ok()
                        .and_then(|v| {
                            let (a, b) = v.split_once(',')?;
                            Some((a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?))
                        })
                {
                    let sr = self.sample_rate();
                    self.range = Some((a.min(b) * sr, a.max(b) * sr));
                    // The highlight too, as if the drag had just ended.
                    self.selection = self.range;
                    self.loop_enabled = true;
                    self.apply_loop();
                }
                match zoom {
                    Some((a, b)) => {
                        let sr = self.sample_rate();
                        self.view = View {
                            start: a * sr,
                            end: b * sr,
                        };
                        self.view.clamp_to(self.total_frames());
                        self.settings.follow_playhead = false;
                    }
                    None => {
                        if let Some(e) = &self.engine {
                            e.play();
                        }
                    }
                }
                if let Some((_, t)) = &mut self.screenshot {
                    *t = Some(Instant::now());
                }
            }
            None => {}
            Some(t)
                if t.elapsed() > std::time::Duration::from_millis(1600)
                    && self.spec_render.iter().all(Option::is_none) =>
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                if let Some((_, slot)) = &mut self.screenshot {
                    *slot = Some(Instant::now() + std::time::Duration::from_secs(3600));
                }
            }
            Some(_) => {}
        }
        ctx.request_repaint();
    }

    fn tick_live(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.2);
        self.last_tick = now;
        // The meter falls whether or not anything is playing, so it is stepped
        // before the engine check rather than inside it.
        let peaks = match &self.engine {
            Some(e) => e.shared.take_out_peak(),
            None => (0.0, 0.0),
        };
        for (m, p) in self.meter.iter_mut().zip([peaks.0, peaks.1]) {
            m.push(p, dt);
        }
        if let (Some(engine), Some(live)) = (&mut self.engine, &mut self.live) {
            live.averaging = self.settings.spectrum_averaging;
            engine.drain_tap(|s| live.push(s));
            live.update(dt);
        }
    }
}

/// One channel of the output meter. Peak ballistics: instant rise, a steady
/// fall in dB per second, and a peak-hold that sits still before it drops.
#[derive(Default, Clone, Copy)]
pub struct MeterChannel {
    /// Bar level, linear amplitude.
    pub level: f32,
    /// Peak-hold marker, linear amplitude.
    pub hold: f32,
    /// Seconds the hold has been standing still.
    held_for: f32,
}

impl MeterChannel {
    /// Fall of the bar and of the hold marker, in dB per second, and how long
    /// the marker stands before it starts to fall.
    const FALL_DB: f32 = 26.0;
    const HOLD_FALL_DB: f32 = 12.0;
    const HOLD_SECS: f32 = 1.2;

    fn push(&mut self, peak: f32, dt: f32) {
        let fall = |v: f32, db: f32| v * 10f32.powf(-db * dt / 20.0);
        self.level = peak.max(fall(self.level, Self::FALL_DB));
        if peak >= self.hold {
            self.hold = peak;
            self.held_for = 0.0;
        } else {
            self.held_for += dt;
            if self.held_for > Self::HOLD_SECS {
                self.hold = fall(self.hold, Self::HOLD_FALL_DB).max(self.level);
            }
        }
    }
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        if let Some(p) = self.pending_open.take()
            && p.exists()
        {
            self.open(&p);
        }
        self.poll_job(ctx);
        self.poll_detail();
        if self.updater.poll() {
            self.settings.update_last_check = update::unix_now();
        }
        self.handle_drops(ctx);
        self.handle_shortcuts(ctx);
        help::set_enabled(ctx, self.help_mode);
        self.tick_live();
        self.follow_playhead();
        if self.view != self.last_view {
            self.last_view = self.view;
            self.view_changed_at = Instant::now();
        }

        panels::title_bar(self, ui);
        panels::top_bar(self, ui);
        panels::status_bar(self, ui);
        panels::side_panel(self, ui);
        // The views' own corner of the window, remembered for the capture: the
        // spectrum panel when it is shown, and the waveform and spectrogram
        // above it. Everything else — the bars, the sidebar, the dialog — is
        // chrome around the picture rather than part of it.
        let mut views_rect = egui::Rect::NOTHING;
        if self.settings.show_spectrum {
            views_rect = spectrum::bottom_panel(self, ui);
        }
        views_rect = views_rect.union(views::central(self, ui));
        self.views_rect = views_rect;
        panels::resize_borders(ctx);
        // Last, so the dialog sits above the resize overlay. Not while a
        // capture is in flight, though: the dialog and the scrim under it sit
        // over the very views the picture is of, and which frame the backend
        // hands back is its business rather than ours.
        if self.capture.is_none() {
            panels::settings_window(self, ctx);
        }

        self.capture_hook(ctx);
        self.screenshot_hook(ctx);

        let playing = self.engine.as_ref().is_some_and(Engine::is_playing);
        if playing || self.job.is_some() {
            ctx.request_repaint();
        } else if self.view_changed_at.elapsed() < DETAIL_SETTLE * 2 {
            // Come back promptly once the view has settled, so the detail
            // tile is requested without waiting for the next input event.
            ctx.request_repaint_after(std::time::Duration::from_millis(20));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

/// A path's last component, for saying what was written without saying where.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The dev hook's output: a PNG unless the path asks for the raw PPM the hook
/// has always written, which needs no decoder to eyeball.
fn write_dev_shot(path: &Path, img: &egui::ColorImage) -> anyhow::Result<()> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("ppm"))
    {
        write_ppm(path, img)?;
        return Ok(());
    }
    auriscope::plot::write_png(path, img)
}

fn write_ppm(path: &Path, img: &egui::ColorImage) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    write!(f, "P6\n{} {}\n255\n", img.width(), img.height())?;
    for px in &img.pixels {
        f.write_all(&[px.r(), px.g(), px.b()])?;
    }
    Ok(())
}

// ---- background work ---------------------------------------------------

fn loader(
    path: PathBuf,
    stft: StftParams,
    job_id: u64,
    tx: mpsc::Sender<Msg>,
    cancel: Arc<AtomicBool>,
) {
    tx.send(Msg::Stage("Decoding".into())).ok();
    let progress_tx = tx.clone();
    let audio = match decode_file(&path, &|p| {
        progress_tx.send(Msg::Progress(p)).ok();
    }) {
        Ok(a) => a,
        Err(e) => {
            tx.send(Msg::Error(format!("{}: {e:#}", path.display())))
                .ok();
            return;
        }
    };
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    tx.send(Msg::Decoded(audio.clone())).ok();

    tx.send(Msg::Stage("Waveform".into())).ok();
    let pyramid = WaveformPyramid::build(&audio.channels);
    tx.send(Msg::Pyramid(Arc::new(pyramid))).ok();

    spectrogram_pass(&audio, stft, job_id, &tx, &cancel);
    if cancel.load(Ordering::Relaxed) {
        return;
    }

    tx.send(Msg::Stage("Loudness".into())).ok();
    match compute_stats(&audio.channels, audio.sample_rate()) {
        Ok(s) => {
            tx.send(Msg::Stats(s)).ok();
        }
        Err(e) => log::warn!("loudness: {e}"),
    }
    tx.send(Msg::Done).ok();
}

fn spectrogram_pass(
    audio: &DecodedAudio,
    stft: StftParams,
    job_id: u64,
    tx: &mpsc::Sender<Msg>,
    cancel: &AtomicBool,
) {
    for (ch, samples) in audio.channels.iter().enumerate() {
        tx.send(Msg::Stage(format!(
            "Spectrogram {}/{}",
            ch + 1,
            audio.channels.len()
        )))
        .ok();
        let spec = Spectrogram::compute(
            samples,
            audio.sample_rate(),
            stft,
            &|p| {
                tx.send(Msg::Progress(p)).ok();
            },
            &|| cancel.load(Ordering::Relaxed),
        );
        match spec {
            Some(s) => {
                tx.send(Msg::Spectrogram {
                    job: job_id,
                    channel: ch,
                    spec: Arc::new(s),
                })
                .ok();
            }
            None => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened(paths: &[&str]) -> Settings {
        let mut s = Settings::default();
        for p in paths {
            s.remember_file(Path::new(p));
        }
        s
    }

    #[test]
    fn history_is_newest_first_and_holds_each_file_once() {
        // Reopening a file moves it up rather than adding a second row.
        let s = opened(&["/a.wav", "/b.wav", "/a.wav"]);
        assert_eq!(
            s.recent_files,
            vec![PathBuf::from("/a.wav"), PathBuf::from("/b.wav")]
        );
        assert_eq!(s.last_file, Some(PathBuf::from("/a.wav")));
    }

    #[test]
    fn history_drops_the_oldest_past_the_cap() {
        let names: Vec<String> = (0..RECENT_MAX + 3).map(|i| format!("/{i}.wav")).collect();
        let s = opened(&names.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(s.recent_files.len(), RECENT_MAX);
        assert_eq!(
            s.recent_files[0],
            PathBuf::from(format!("/{}.wav", RECENT_MAX + 2))
        );
        assert!(!s.recent_files.contains(&PathBuf::from("/0.wav")));
    }

    #[test]
    fn history_off_records_nothing_at_all() {
        // Including the file to reopen at startup: switching it off has to
        // mean the app forgets, not that it hides what it kept.
        let mut s = Settings {
            remember_recent: false,
            ..Settings::default()
        };
        s.remember_file(Path::new("/a.wav"));
        assert!(s.recent_files.is_empty());
        assert_eq!(s.last_file, None);
    }

    #[test]
    fn clearing_forgets_the_startup_file_too() {
        let mut s = opened(&["/a.wav", "/b.wav"]);
        s.forget_file(Path::new("/b.wav"));
        assert_eq!(s.recent_files, vec![PathBuf::from("/a.wav")]);
        // /b.wav was the newest, so it was also the one to reopen.
        assert_eq!(s.last_file, None);
        s.clear_recent();
        assert!(s.recent_files.is_empty());
        assert_eq!(s.last_file, None);
    }
}
