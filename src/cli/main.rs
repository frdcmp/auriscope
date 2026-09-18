//! Auriscope without the window: what the views show, as a PNG and as JSON,
//! for a script or a machine reader rather than a pair of eyes at a screen.
//!
//! Everything it does, the window does too. The difference is that nothing
//! here opens one, so it runs over a folder, in a pipeline, or on a box with
//! no display — and the picture it writes carries its own axes, because a
//! spectrogram with no numbers around it says that something happened without
//! saying when or at what frequency.

mod args;
mod spec;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use egui::ColorImage;
use serde_json::{Value, json};

use args::Args;
use auriscope::analysis::{
    ColorMap, DEFAULT_CUSTOM, DetailTile, Envelope, SPEECH_BAND, Spectrogram, StftParams,
    ViewParams, WaveformPyramid, WindowKind, band_limit, clicks, compute_stats, floor_steps,
    high_pass, render_view, truncation, zero_runs,
};
use auriscope::audio::{DecodedAudio, decode_file};
use auriscope::plot::figure::{ColorBar, Overlay, WaveScale};
use auriscope::plot::{Figure, Lane, Text, Theme, write_png};
use auriscope::report;

const HELP: &str = "\
Auriscope CLI — audio analysis without a window

Usage:
  auriscope-cli analyze PATH [options]
  auriscope-cli qc      PATH --spec SPEC.toml [options]
  auriscope-cli render  FILE [options]

PATH is a file, or a folder: a folder is walked to any depth and every audio
file under it is measured, one JSON object per line.

Commands:
  analyze   write what the sidebar knows about the file, as JSON
  qc        check files against a delivery spec; exit 1 if any fail
  render    write the waveform and spectrogram as an annotated PNG

Common options:
  -o, --output PATH    where to write (defaults to stdout)
      --quiet          no progress on stderr
  -h, --help           this text
  -V, --version        print the version and exit

analyze options:
      --start SECS     measure only from here (default the start of the file)
      --end SECS       measure only up to here (default the end)
      --structure      where the speech is: lead, tail, pauses, breaths
      --segments       every stretch of the file, with its level
      --defects        digital silence, clicks, seams, truncation
      --spectral       centroid, rolloff, flatness, ceiling, hum, bands
      --all            all of the above
      --timeline       loudness and level against time, one entry per 100 ms
      --compact        one line of JSON instead of indented

      --speech-db DB   how far over the floor is speech (default 10)
      --min-speech-ms  shorter runs are not speech (default 100)
      --min-gap-ms     shorter dips are not pauses (default 80)
      --zero-run-ms MS shortest run of zeros worth reporting (default 1)
      --click-db DB    step over the local RMS that counts (default 32)
      --spectral-window N  FFT size for --spectral (default 8192)

qc options:
      --spec PATH      the delivery spec, as TOML. Required
      --render-failures DIR   a picture of each failure, zoomed on the finding
      --fail-only      leave passing files out of the output

folder options:
      --csv            a flat table instead of one JSON object per line
      --jobs N         files at once (default: as many as there are cores)

render options:
      --start SECS     where the picture begins (default 0)
      --end SECS       where it ends (default the end of the file)
      --channel N      one channel only, counting from 0 (default all)
      --width PX       width of the plotted area (default 1600)
      --height PX      height of each spectrogram (default 340)
      --wave-height PX height of each waveform (default 90, 0 for none)
      --no-waveform    spectrogram only
      --no-spectrogram waveform only
      --no-merge       waveform above the spectrogram instead of over it
      --merge-opacity F        how strongly the waveform draws (default 1)
      --merge-spec-opacity F   how much the spectrogram shows (default 0.55)
      --spectrum       add a level-against-frequency pane for the whole span
      --spectrum-height PX     how tall it is (default 150)
      --wave-scale S   linear or db (default linear, with a dB ruler)
      --wave-zoom F    vertical zoom for the linear scale, 1 to 4096
      --wave-db FLOOR  bottom of the dB waveform (default -90)
      --wave-color C   #rrggbb or r,g,b
      --window N       FFT size: 256 512 1024 2048 4096 8192 16384 (default 2048)
      --overlap PCT    window overlap, per cent or as a fraction (default 75)
      --window-fn NAME hann hamming blackman blackman-harris rectangular
      --reassign       sharpen lines and clicks by reassignment (slower)
      --db MIN:MAX     decibel range of the colour map (default -115:-9)
      --min-hz HZ      bottom of the frequency axis (default 0 linear, 20 log)
      --max-hz HZ      top of it (default Nyquist)
      --log            logarithmic frequency axis instead of linear
      --colormap NAME  amber ember magma inferno viridis plasma turbo grey
      --contrast F     gamma on the level before it is coloured (default 0.92)
      --no-axes        the bare spectrogram, with no margins or labels
      --json PATH      also write the analyze report, with how this was framed

Examples:
  auriscope-cli analyze take.wav | jq .loudness
  auriscope-cli analyze take.wav --start 0 --end 0.2 | jq .channels[0]
  auriscope-cli analyze takes/ --all --csv -o takes.csv
  auriscope-cli qc takes/ --spec spec.toml --csv -o qc.csv --render-failures bad/
  auriscope-cli render take.wav -o take.png
  auriscope-cli render take.wav --start 3.1 --end 3.7 --window 1024 -o click.png
  auriscope-cli render take.wav --wave-zoom 512 -o floor.png
  auriscope-cli render take.wav --no-merge --log --spectrum -o panes.png
";

/// The view a render opens on, which is the one the window is usually left
/// sitting on: the waveform drawn over the spectrogram as a single pane, a
/// linear frequency axis, and a colour range wide enough at the quiet end to
/// show a room tone without the loud end burning out.
///
/// These are only starting points. Every one of them has a flag.
const DEFAULT_COLORMAP: ColorMap = ColorMap::Inferno;
const DEFAULT_CONTRAST: f32 = 0.92;
const DEFAULT_DB_FLOOR: f32 = -115.0;
const DEFAULT_DB_CEILING: f32 = -9.0;
/// Fully drawn: the waveform is the thing being read, and the spectrogram
/// behind it is dimmed instead to make room.
const DEFAULT_MERGE_WAVE: f32 = 1.0;
const DEFAULT_MERGE_SPEC: f32 = 0.55;
/// Amplitude with a decibel ruler beside it, as the window draws it, rather
/// than an envelope reshaped into decibels.
const DEFAULT_WAVE_SCALE: &str = "linear";

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("auriscope-cli: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<()> {
    let args = Args::parse(std::env::args().skip(1))?;
    if args.has("help") {
        print!("{HELP}");
        return Ok(());
    }
    if args.has("version") {
        let git = env!("AURISCOPE_GIT_DESCRIBE");
        match git.is_empty() || git == format!("v{}", report::VERSION) {
            true => println!("{}", report::VERSION),
            false => println!("{} (dev {git})", report::VERSION),
        }
        return Ok(());
    }
    if args.positional.is_empty() {
        print!("{HELP}");
        return Ok(());
    }
    match args.positional[0].as_str() {
        "analyze" | "analyse" => analyze(&args),
        "qc" => qc(&args),
        "render" => render(&args),
        other => bail!("unknown command {other:?}; try --help"),
    }
}

/// Extensions worth opening. Anything else in a folder — a sidecar, a
/// spreadsheet, a stray PDF — is passed over in silence rather than reported as
/// a file that would not decode.
const AUDIO_EXTENSIONS: &[&str] = &[
    "wav", "wave", "bwf", "rf64", "aif", "aiff", "aifc", "caf", "flac", "alac", "ape", "mp3",
    "m4a", "mp4", "aac", "ogg", "oga", "opus", "mka", "mkv", "w64",
];

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Every audio file under `root`, however deep, in a stable order.
///
/// Sorted rather than left in whatever order the filesystem hands them back:
/// two runs over the same tree should produce the same rows in the same
/// places, or comparing one run to the next is needless work.
fn walk(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
            let path = entry.path();
            // Hidden directories are skipped: a tree of takes is not improved
            // by the contents of its .git or .venv.
            let hidden = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'));
            match entry.file_type() {
                Ok(t) if t.is_dir() && !hidden => stack.push(path),
                Ok(t) if t.is_file() && is_audio(&path) => out.push(path),
                _ => {}
            }
        }
    }
    out.sort();
    Ok(out)
}

/// What one file produced, or why it could not be read.
struct Row {
    path: PathBuf,
    result: Result<Value>,
}

/// Run `each` over every file, on `jobs` threads, keeping the input order.
///
/// A file that fails to decode does not stop the run: in a folder of seven
/// thousand takes, one that is truncated or is not really audio is a row that
/// says so, not the end of the job.
fn each_file(
    files: &[PathBuf],
    jobs: usize,
    note: &(dyn Fn(&str) + Sync),
    each: &(dyn Fn(&Path) -> Result<Value> + Sync),
) -> Vec<Row> {
    let done = std::sync::atomic::AtomicUsize::new(0);
    let total = files.len();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let rows: Vec<std::sync::Mutex<Option<Row>>> =
        files.iter().map(|_| std::sync::Mutex::new(None)).collect();

    std::thread::scope(|scope| {
        for _ in 0..jobs.max(1) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(path) = files.get(i) else { break };
                    let result = each(path);
                    *rows[i].lock().unwrap() = Some(Row {
                        path: path.clone(),
                        result,
                    });
                    let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    // About twenty updates over the whole run, whatever its
                    // size: a line every twenty-five files is two hundred
                    // lines of scrollback for a folder of five thousand.
                    let every = (total / 20).max(1);
                    if n.is_multiple_of(every) || n == total {
                        note(&format!("{n}/{total}"));
                    }
                }
            });
        }
    });
    rows.into_iter()
        .filter_map(|r| r.into_inner().unwrap())
        .collect()
}

/// How many files to work on at once.
fn jobs(args: &Args) -> Result<usize> {
    Ok(args
        .get::<usize>("jobs")?
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
        .max(1))
}

/// The file to work on: the one positional argument that is not the command.
fn source(args: &Args) -> Result<PathBuf> {
    match args.positional.len() {
        0 | 1 => bail!("no file or folder given"),
        2 => Ok(PathBuf::from(&args.positional[1])),
        _ => bail!(
            "one path at a time, got {}",
            args.positional[1..].join(", ")
        ),
    }
}

/// The files a command was pointed at: one, or every audio file under a folder.
fn targets(path: &Path) -> Result<(Vec<PathBuf>, bool)> {
    if path.is_dir() {
        let files = walk(path)?;
        if files.is_empty() {
            bail!("no audio files under {}", path.display());
        }
        Ok((files, true))
    } else {
        Ok((vec![path.to_path_buf()], false))
    }
}

/// Decode, and measure while everything is in memory. Loudness that will not
/// compute is a warning and a null block, not a failure: the rest of the
/// report is still worth having.
fn open(path: &Path, note: &dyn Fn(&str)) -> Result<DecodedAudio> {
    note("decoding");
    let audio = decode_file(path, &|_| {})?;
    let audio =
        std::sync::Arc::try_unwrap(audio).map_err(|_| anyhow!("decoded file is still shared"))?;
    if audio.sample_rate() == 0 || audio.channels.is_empty() {
        bail!("{} has no audio in it", path.display());
    }
    Ok(audio)
}

/// Measure `audio` over `region`, or over all of it when there is none.
fn measure(
    audio: &DecodedAudio,
    region: Option<&Region>,
    note: &dyn Fn(&str),
) -> Option<auriscope::analysis::FileStats> {
    note("measuring");
    let planes: Vec<&[f32]> = match region {
        Some(r) => audio
            .channels
            .iter()
            .map(|c| &c[r.start_frame.min(c.len())..r.end_frame.min(c.len())])
            .collect(),
        None => audio.channels.iter().map(Vec::as_slice).collect(),
    };
    match compute_stats(&planes, audio.sample_rate()) {
        Ok(s) => Some(s),
        Err(e) => {
            log::warn!("loudness: {e}");
            None
        }
    }
}

/// A span of a file to measure or to draw, resolved against its length.
#[derive(Debug, Clone, Copy)]
struct Region {
    start_secs: f64,
    end_secs: f64,
    start_frame: usize,
    end_frame: usize,
}

impl Region {
    /// `--start`/`--end` against a file of `duration` seconds, or `None` when
    /// neither was given and the whole file is meant.
    fn resolve(args: &Args, duration: f64, sample_rate: u32) -> Result<Option<Self>> {
        let start = args.get::<f64>("start")?;
        let end = args.get::<f64>("end")?;
        if start.is_none() && end.is_none() {
            return Ok(None);
        }
        let start_secs = start.unwrap_or(0.0).max(0.0);
        let end_secs = end.unwrap_or(duration).min(duration);
        if end_secs <= start_secs {
            bail!(
                "--end ({end_secs}) must be after --start ({start_secs}); \
                 the file is {duration:.3} s long"
            );
        }
        let frame = |t: f64| (t * sample_rate as f64).round().max(0.0) as usize;
        Ok(Some(Self {
            start_secs,
            end_secs,
            start_frame: frame(start_secs),
            end_frame: frame(end_secs),
        }))
    }

    fn json(&self) -> Value {
        json!({
            "start_secs": self.start_secs,
            "end_secs": self.end_secs,
            "duration_secs": self.end_secs - self.start_secs,
            "start_frames": self.start_frame,
            "end_frames": self.end_frame,
        })
    }
}

fn analyze(args: &Args) -> Result<()> {
    let path = source(args)?;
    let note = notifier(args);
    let (files, folder) = targets(&path)?;
    if folder {
        note(&format!("{} files under {}", files.len(), path.display()));
        let quiet = |_: &str| {};
        let rows = each_file(&files, jobs(args)?, &note, &|p| {
            analyze_one(args, p, &quiet)
        });
        return emit_rows(args, rows, None);
    }
    let report = analyze_one(args, &path, &note)?;
    emit(args, &report)
}

/// One file's report, which is the same whether it was asked for on its own or
/// as one of several thousand.
fn analyze_one(args: &Args, path: &Path, note: &dyn Fn(&str)) -> Result<Value> {
    analyze_with(args, path, note, None)
}

/// As [`analyze_one`], with thresholds a spec has an opinion about. A spec is
/// meant to be the whole agreement, so when it names a threshold that is the
/// one used, whatever the command line says.
fn analyze_with(
    args: &Args,
    path: &Path,
    note: &dyn Fn(&str),
    click_db: Option<f32>,
) -> Result<Value> {
    let audio = open(path, note)?;
    let region = Region::resolve(args, audio.info.duration_secs(), audio.sample_rate())?;
    let stats = measure(&audio, region.as_ref(), note);
    let mut report = report::file_report(&audio.info, stats.as_ref());
    if let (Some(obj), Some(region)) = (report.as_object_mut(), region) {
        // Only when one was asked for: its absence is what says the numbers
        // are the whole file's.
        obj.insert("region".into(), region.json());
    }
    if args.has("timeline")
        && let (Some(obj), Some(stats)) = (report.as_object_mut(), stats.as_ref())
    {
        obj.insert("timeline".into(), report::timeline_json(stats));
    }
    let deep = Deep::run(args, &audio, region.as_ref(), note, click_db)?;
    if let Some(obj) = report.as_object_mut() {
        deep.insert_into(obj, args);
    }
    Ok(report)
}

/// The passes beyond the one-walk statistics: structure, defects and spectrum.
///
/// They share their working — the envelopes, and the segmentation that tells
/// the other two where the words are — so they are computed together and
/// emitted separately. Asking for any of them costs the envelope pass once.
#[derive(Default)]
struct Deep {
    segmentation: Option<auriscope::analysis::Segmentation>,
    defects: Option<Value>,
    spectral: Option<Value>,
    sample_rate: u32,
}

impl Deep {
    fn wanted(args: &Args) -> (bool, bool, bool) {
        // A spec asks about structure and defects, so `qc` needs both whether
        // or not they were asked for by name: a rule checked against a
        // measurement nobody took reports nothing and reads as a pass.
        let all = args.has("all") || args.positional.first().is_some_and(|c| c == "qc");
        (
            all || args.has("segments") || args.has("structure"),
            all || args.has("defects"),
            args.has("all") || args.has("spectral"),
        )
    }

    fn run(
        args: &Args,
        audio: &DecodedAudio,
        region: Option<&Region>,
        note: &dyn Fn(&str),
        click_db: Option<f32>,
    ) -> Result<Self> {
        let (want_structure, want_defects, want_spectral) = Self::wanted(args);
        if !(want_structure || want_defects || want_spectral) {
            return Ok(Self::default());
        }
        let sr = audio.sample_rate();
        // One channel decides the structure of a file: a mono mix, or the
        // chosen channel. Speech starts and stops at the same moment in every
        // channel of a take, and two answers would only have to be reconciled.
        let channel = args.get::<usize>("channel")?.unwrap_or(0);
        if channel >= audio.channels.len() {
            bail!(
                "--channel {channel}: the file has {} (0 to {})",
                audio.channels.len(),
                audio.channels.len() - 1
            );
        }
        let whole = &audio.channels[channel];
        let samples: &[f32] = match region {
            Some(r) => &whole[r.start_frame.min(whole.len())..r.end_frame.min(whole.len())],
            None => whole,
        };

        note("envelopes");
        let decision = Envelope::compute(
            &band_limit(samples, sr, SPEECH_BAND.0, SPEECH_BAND.1),
            sr,
            FRAME_MS,
            HOP_MS,
        );
        let full = Envelope::compute(samples, sr, FRAME_MS, HOP_MS);
        let above_80 = Envelope::compute(&high_pass(samples, sr, 80.0), sr, FRAME_MS, HOP_MS);

        let params = segment_params(args)?;
        let seg = auriscope::analysis::segment(&decision, &full, &above_80, samples, params);

        let mut out = Self {
            sample_rate: sr,
            ..Default::default()
        };
        if want_defects {
            note("defects");
            let min_zero = (sr as f64 * args.get::<f64>("zero-run-ms")?.unwrap_or(1.0) / 1000.0)
                .round()
                .max(1.0) as usize;
            let click_db = match click_db {
                Some(v) => v,
                None => args.get::<f32>("click-db")?.unwrap_or(32.0),
            };
            let zero = zero_runs(samples, min_zero);
            let hits = clicks(samples, sr, click_db);
            let seams = floor_steps(&full, &seg.speech_frames, 50, 6.0);
            let trunc = truncation(samples, sr, seg.floor_db, 5, 20.0);
            let hop = decision.hop_len;
            let speech = seg.speech_frames.clone();
            let speech_at = move |secs: f64| -> bool {
                let i = (secs * sr as f64) as usize / hop.max(1);
                speech.get(i).copied().unwrap_or(false)
            };
            out.defects = Some(report::defects_json(
                &zero, &hits, &seams, trunc, &speech_at, sr,
            ));
        }
        if want_spectral {
            note("spectrum");
            let window = args.get::<usize>("spectral-window")?.unwrap_or(8192);
            if !StftParams::SIZES.contains(&window) {
                bail!(
                    "--spectral-window {window}: one of {}",
                    StftParams::SIZES
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
                        .join(" ")
                );
            }
            let params = StftParams {
                window_size: window,
                ..Default::default()
            };
            let spec = Spectrogram::compute(samples, sr, params, &|_| {}, &|| false)
                .context("computing the spectrum")?;
            // The hum search prefers the quiet, so hand it the silence: a
            // 50 Hz line is easiest to see when nothing is talking over it.
            let hop = params.hop();
            let quiet: Vec<bool> = (0..spec.columns())
                .map(|c| {
                    let secs = (c * hop) as f64 / sr as f64;
                    let i = (secs * sr as f64) as usize / decision.hop_len.max(1);
                    !seg.speech_frames.get(i).copied().unwrap_or(false)
                })
                .collect();
            out.spectral = Some(report::spectral_json(
                &auriscope::analysis::spectral::analyse(&spec, &quiet),
            ));
        }
        if want_structure {
            out.segmentation = Some(seg);
        }
        Ok(out)
    }

    fn insert_into(self, obj: &mut serde_json::Map<String, Value>, args: &Args) {
        let (want_structure, ..) = Self::wanted(args);
        if let Some(seg) = &self.segmentation {
            if want_structure {
                obj.insert(
                    "structure".into(),
                    report::structure_json(seg, self.sample_rate),
                );
            }
            // The full list is long on a file with many words, so it waits to
            // be asked for by name even when --all is on.
            if args.has("segments") || args.has("all") {
                obj.insert("segments".into(), report::segments_json(seg));
            }
        }
        if let Some(v) = self.defects {
            obj.insert("defects".into(), v);
        }
        if let Some(v) = self.spectral {
            obj.insert("spectral".into(), v);
        }
    }
}

/// Frame and hop for every structural measurement. Twenty milliseconds holds a
/// couple of cycles of a low voice; a five millisecond hop puts an onset within
/// five milliseconds of where it really is.
const FRAME_MS: usize = 20;
const HOP_MS: usize = 5;

fn segment_params(args: &Args) -> Result<auriscope::analysis::SegmentParams> {
    let mut p = auriscope::analysis::SegmentParams::default();
    if let Some(v) = args.get::<f32>("speech-db")? {
        p.speech_db = v;
    }
    if let Some(v) = args.get::<usize>("min-speech-ms")? {
        p.min_speech_ms = v;
    }
    if let Some(v) = args.get::<usize>("min-gap-ms")? {
        p.min_gap_ms = v;
    }
    Ok(p)
}

/// Check files against a delivery spec.
///
/// Exit 1 when something failed, 2 when the tool could not do its job. A
/// script can then tell "the audio is wrong" from "the run is broken", which
/// one exit code cannot.
fn qc(args: &Args) -> Result<()> {
    let path = source(args)?;
    let note = notifier(args);
    let spec_path = args
        .str("spec")
        .ok_or_else(|| anyhow!("qc needs a spec: --spec path/to/spec.toml"))?;
    let spec = spec::Spec::load(Path::new(spec_path))?;
    let (files, folder) = targets(&path)?;
    note(&format!(
        "{} file{} against {}",
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        if spec.name.is_empty() {
            spec_path.to_owned()
        } else {
            spec.name.clone()
        }
    ));

    // A spec asks about structure and defects, so the passes that measure them
    // are on whether or not the flags said so. Checking a rule against a
    // measurement that was never taken would report nothing and look like a
    // pass.
    let quiet = |_: &str| {};
    let click_db = spec.defects.click_db;
    let rows = each_file(&files, jobs(args)?, &note, &|p| {
        let mut report = analyze_with(args, p, &quiet, click_db)?;
        let findings = spec.check(&report);
        if let Some(obj) = report.as_object_mut() {
            obj.insert(
                "qc".into(),
                json!({
                    "spec": spec.name,
                    "pass": !findings.iter().any(|f| f.severity == spec::Severity::Fail),
                    "counts": spec::tally(&findings),
                    "findings": findings.iter().map(|f| f.json()).collect::<Vec<_>>(),
                }),
            );
        }
        Ok(report)
    });

    let failed = rows
        .iter()
        .filter(|r| {
            r.result
                .as_ref()
                .map(|v| v["qc"]["pass"] == json!(false))
                .unwrap_or(true)
        })
        .count();

    if let Some(dir) = args.str("render-failures") {
        render_failures(&rows, Path::new(dir), &note)?;
    }
    let rows = match args.has("fail-only") {
        true => rows
            .into_iter()
            .filter(|r| {
                r.result
                    .as_ref()
                    .map(|v| v["qc"]["pass"] == json!(false))
                    .unwrap_or(true)
            })
            .collect(),
        false => rows,
    };
    let single = (!folder).then_some(());
    emit_rows(args, rows, single.map(|_| "qc"))?;
    note(&format!("{failed} of {} failed", files.len()));
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// A picture of each failing file, zoomed on the first thing wrong with it.
///
/// A picture is often what a client asks for as evidence that something was
/// found and fixed; this produces it in the same run that found the problem,
/// framed on the finding rather than on the whole file.
fn render_failures(rows: &[Row], dir: &Path, note: &dyn Fn(&str)) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut made = 0usize;
    for row in rows {
        let Ok(report) = &row.result else { continue };
        if report["qc"]["pass"] != json!(false) {
            continue;
        }
        let stem = row
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let out = dir.join(format!("{stem}.png"));
        // Around the first place something went wrong, with a second either
        // side; failing that, the whole file.
        let at = report["qc"]["findings"]
            .as_array()
            .and_then(|f| f.first())
            .and_then(|f| f["at_secs"].as_array())
            .and_then(|a| a.first())
            .and_then(Value::as_f64);
        let mut argv = vec![
            "render".to_owned(),
            row.path.display().to_string(),
            "-o".into(),
            out.display().to_string(),
            "--quiet".into(),
        ];
        if let Some(at) = at {
            argv.push("--start".into());
            argv.push(format!("{:.3}", (at - 1.0).max(0.0)));
            argv.push("--end".into());
            argv.push(format!("{:.3}", at + 1.0));
        }
        // Through the same option parser the command line uses, so a render
        // made here is one the reader could have made themselves.
        let sub = Args::parse(argv)?;
        if let Err(e) = render(&sub) {
            log::warn!("rendering {}: {e:#}", row.path.display());
            continue;
        }
        made += 1;
    }
    note(&format!(
        "wrote {made} picture{} to {}",
        if made == 1 { "" } else { "s" },
        dir.display()
    ));
    Ok(())
}

fn render(args: &Args) -> Result<()> {
    let path = source(args)?;
    let note = notifier(args);

    // Every option is read before the file is: a typo in one of them should
    // cost nothing, and decoding a long file first would cost a minute.
    let width = args.get::<i64>("width")?.unwrap_or(1600).clamp(64, 20_000);
    let height = args.get::<i64>("height")?.unwrap_or(340).clamp(32, 8_000);
    let wave_height = match args.has("no-waveform") {
        true => 0,
        false => args
            .get::<i64>("wave-height")?
            .unwrap_or(90)
            .clamp(0, 8_000),
    };
    let params = stft_params(args)?;
    if args.has("log") && args.has("linear") {
        bail!("--log and --linear ask for different axes; pick one");
    }
    let log = args.has("log");
    let (db_min, db_max) = db_range(args)?;
    let contrast = args
        .get::<f32>("contrast")?
        .unwrap_or(DEFAULT_CONTRAST)
        .clamp(0.25, 4.0);
    let colormap = colormap(args)?;
    let scale = wave_scale(args)?;
    let wave_color = wave_color(args)?;
    let show_spectrogram = !args.has("no-spectrogram");
    // Merging only means something when there are two panes to merge.
    let merge = !args.has("no-merge") && wave_height > 0 && show_spectrogram;
    // Over a spectrogram, amplitude rather than decibels unless the scale was
    // asked for by name: a dB envelope is tall nearly everywhere, and drawn on
    // top it would blot out the picture it is supposed to annotate.
    let overlay_scale = match (merge, args.str("wave-scale")) {
        (true, Some(s)) if matches("db", s) || matches("dbfs", s) => scale,
        (true, _) => WaveScale::Linear {
            zoom: args
                .get::<f32>("wave-zoom")?
                .unwrap_or(1.0)
                .clamp(1.0, 4096.0),
        },
        _ => scale,
    };
    let merge_opacity = args
        .get::<f32>("merge-opacity")?
        .unwrap_or(DEFAULT_MERGE_WAVE)
        .clamp(0.05, 1.0);
    let merge_spec_opacity = args
        .get::<f32>("merge-spec-opacity")?
        .unwrap_or(DEFAULT_MERGE_SPEC)
        .clamp(0.05, 1.0);
    let spectrum_height = match args.has("spectrum") {
        true => args
            .get::<i64>("spectrum-height")?
            .unwrap_or(150)
            .clamp(40, 8_000),
        false => 0,
    };
    if !show_spectrogram && wave_height == 0 && spectrum_height == 0 {
        bail!("nothing left to draw: --no-spectrogram with no waveform and no spectrum");
    }
    let want_start = args.get::<f64>("start")?.unwrap_or(0.0).max(0.0);
    let want_end = args.get::<f64>("end")?;
    let want_channel = args.get::<usize>("channel")?;

    let audio = open(&path, &note)?;
    // The picture is of a span; the report beside it still describes the file,
    // the way the window's capture does. `analyze --start --end` is the way to
    // measure a span.
    let stats = measure(&audio, None, &note);
    let sr = audio.sample_rate();
    let nyquist = sr as f32 / 2.0;
    let duration = audio.info.duration_secs();

    let start = want_start;
    let end = want_end.unwrap_or(duration).min(duration);
    if end <= start {
        bail!("--end ({end}) must be after --start ({start}); the file is {duration:.3} s long");
    }

    // The same clamping `render_view` does, done here as well so the axis
    // beside the picture is labelled with the range the picture was drawn
    // over rather than the one that was asked for.
    let min_hz = args
        .get::<f32>("min-hz")?
        .unwrap_or(if log { 20.0 } else { 0.0 })
        .max(if log { 10.0 } else { 0.0 });
    let max_hz = args
        .get::<f32>("max-hz")?
        .unwrap_or(nyquist)
        .min(nyquist)
        .max(min_hz + 1.0);

    let channels: Vec<usize> = match want_channel {
        Some(ch) if ch < audio.channels.len() => vec![ch],
        Some(ch) => bail!(
            "--channel {ch}: the file has {} ({} to {})",
            audio.channels.len(),
            0,
            audio.channels.len() - 1
        ),
        None => (0..audio.channels.len()).collect(),
    };

    let start_frame = start * sr as f64;
    let end_frame = end * sr as f64;
    let lut = colormap.lut_with(contrast, &DEFAULT_CUSTOM);
    let pyramid = (wave_height > 0).then(|| {
        note("waveform");
        WaveformPyramid::build(&audio.channels)
    });

    let nch = audio.channels.len();
    let view = ViewTemplate {
        min_hz,
        max_hz,
        log,
        db_min,
        db_max,
    };
    // The units live in the lane titles. In the margins they would have to
    // share a line with the topmost tick label, and two bits of text on top of
    // each other are worse than none.
    let axis = if log { "Hz, log" } else { "Hz, linear" };
    let mut lanes = Vec::new();
    let mut spectra: Vec<(String, Spectrum)> = Vec::new();
    for (i, &ch) in channels.iter().enumerate() {
        let samples = &audio.channels[ch];
        let name = report::channel_name(ch, nch);
        let bins = pyramid
            .as_ref()
            .map(|p| p.query(ch, samples, start_frame, end_frame, width as usize));

        let drawn = if show_spectrogram || spectrum_height > 0 {
            note(&format!("spectrogram {}/{}", i + 1, channels.len()));
            Some(spectrogram_image(
                samples,
                sr,
                params,
                start_frame,
                end_frame,
                width as usize,
                height as usize,
                &view,
                &lut,
                spectrum_height > 0,
            )?)
        } else {
            None
        };

        // Merged: one lane, the waveform drawn into the spectrogram. Separate:
        // the waveform above its spectrogram, each with its own scale.
        let overlay = match merge {
            true => bins.clone().map(|bins| Overlay {
                bins,
                scale: overlay_scale,
                opacity: merge_opacity,
                color: wave_color,
            }),
            false => None,
        };
        if let (Some(bins), false) = (&bins, merge) {
            lanes.push(Lane::Waveform {
                title: format!("{name} · {}", scale_name(scale)),
                bins: bins.clone(),
                height: wave_height,
                scale,
                color: wave_color,
            });
        }
        if let (true, Some(drawn)) = (show_spectrogram, &drawn) {
            let title = match (bins.is_some(), merge) {
                (true, false) => axis.to_owned(),
                _ => format!("{name} · {axis}"),
            };
            lanes.push(Lane::Spectrogram {
                title,
                image: drawn.image.clone(),
                min_hz: min_hz as f64,
                max_hz: max_hz as f64,
                log,
                overlay,
                opacity: if merge { merge_spec_opacity } else { 1.0 },
            });
        }
        if let Some(s) = drawn.as_ref().and_then(|d| d.spectrum.as_ref()) {
            spectra.push((name.clone(), s.clone()));
        }
    }

    // The spectrum lanes go last, under the shared time axis: their own axis
    // is frequency, so they cannot sit among the lanes that run along time.
    for (name, s) in &spectra {
        lanes.push(Lane::Spectrum {
            title: format!("{name} · spectrum, {}", if log { "log" } else { "linear" }),
            average: s.average.clone(),
            peak: s.peak.clone(),
            hz_per_bin: s.hz_per_bin,
            min_hz: min_hz as f64,
            max_hz: max_hz as f64,
            log,
            db_min,
            db_max,
            height: spectrum_height,
        });
    }

    let out = output_path(args, &path);
    let plot = if args.has("no-axes") {
        stack(&lanes)
    } else {
        Figure {
            width,
            title: audio.info.file_name(),
            subtitle: subtitle(
                &audio, &params, colormap, db_min, db_max, log, min_hz, max_hz,
            ),
            start_secs: start,
            end_secs: end,
            lanes,
            // The bar reads the spectrogram's colours; with no spectrogram
            // on the plot it would be explaining nothing.
            colorbar: show_spectrogram.then_some(ColorBar {
                lut,
                db_min,
                db_max,
            }),
            theme: Theme::default(),
        }
        .render(&mut Text::new())
    };
    write_png(&out, &plot)?;
    note(&format!("wrote {}", out.display()));

    if let Some(sidecar) = args.str("json") {
        let mut value = report::file_report(&audio.info, stats.as_ref());
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "view".into(),
                view_json(
                    start, end, sr, &channels, width, height, min_hz, max_hz, log, &out,
                ),
            );
            obj.insert(
                "analysis".into(),
                analysis_json(&params, db_min, db_max, contrast, colormap, merge, scale),
            );
        }
        let path = PathBuf::from(sidecar);
        write_json(&path, &value, args.has("compact"))?;
        note(&format!("wrote {}", path.display()));
    }
    Ok(())
}

/// The frequency and level mapping a view is drawn with, which is the same for
/// every channel on one plot.
struct ViewTemplate {
    min_hz: f32,
    max_hz: f32,
    log: bool,
    db_min: f32,
    db_max: f32,
}

/// The mean and the maximum level of each FFT bin over the drawn span.
///
/// The still-picture answer to the window's realtime spectrum, which has no
/// meaning without playback. It is read back off the analysis that drew the
/// spectrogram rather than transformed again, so the two always agree.
#[derive(Clone)]
struct Spectrum {
    average: Vec<f32>,
    peak: Vec<f32>,
    hz_per_bin: f64,
}

/// What one channel contributed to the plot.
struct Drawn {
    image: egui::ColorImage,
    spectrum: Option<Spectrum>,
}

/// One channel's spectrogram, rendered into the viewport.
///
/// Only the visible span is analysed, with a window's worth of padding on each
/// side: a spectrogram of the whole file would be thrown away for a picture of
/// half a second of it, and without the padding the first and last window of
/// the view would fade into the zeros beyond the slice and put a dark band
/// down each edge.
///
/// A detail tile over the same slice gives one analysis column per pixel
/// column, so a zoomed picture is as sharp as the window allows rather than as
/// sharp as the hop happens to be.
#[allow(clippy::too_many_arguments)]
fn spectrogram_image(
    samples: &[f32],
    sample_rate: u32,
    params: StftParams,
    start_frame: f64,
    end_frame: f64,
    width: usize,
    height: usize,
    view: &ViewTemplate,
    lut: &[egui::Color32],
    want_spectrum: bool,
) -> Result<Drawn> {
    let pad = params.window_size as f64;
    let slice_start = (start_frame - pad).floor().max(0.0) as usize;
    let slice_end = ((end_frame + pad).ceil().max(0.0) as usize).min(samples.len());
    if slice_start >= slice_end {
        return Ok(Drawn {
            image: ColorImage::filled([width, height], lut[0]),
            spectrum: None,
        });
    }
    let slice = &samples[slice_start..slice_end];
    let spec = Spectrogram::compute(slice, sample_rate, params, &|_| {}, &|| false)
        .context("computing the spectrogram")?;
    let (from, to) = (
        start_frame - slice_start as f64,
        end_frame - slice_start as f64,
    );
    let tile = DetailTile::compute(slice, sample_rate, params, from, to, width, &|| false);
    let params_view = ViewParams {
        start_frame: from,
        end_frame: to,
        min_hz: view.min_hz,
        max_hz: view.max_hz,
        log_frequency: view.log,
        db_min: view.db_min,
        db_max: view.db_max,
    };
    let image = render_view(&spec, tile.as_ref(), &params_view, width, height, lut);
    let spectrum = want_spectrum.then(|| match tile.as_ref() {
        // The tile covers exactly the drawn span, one column per pixel, so it
        // is both the cheapest and the most faithful thing to average.
        Some(t) => fold_spectrum(t.bins, t.columns, sample_rate, params, |c, b| t.db_at(c, b)),
        None => fold_spectrum(spec.bins, spec.columns(), sample_rate, params, |c, b| {
            spec.db_at(c, b)
        }),
    });
    Ok(Drawn { image, spectrum })
}

/// Walk every column once, keeping a running sum and maximum per bin.
fn fold_spectrum(
    bins: usize,
    columns: usize,
    sample_rate: u32,
    params: StftParams,
    db_at: impl Fn(usize, usize) -> f32,
) -> Spectrum {
    let mut sum = vec![0f64; bins];
    let mut peak = vec![f32::NEG_INFINITY; bins];
    for c in 0..columns {
        for b in 0..bins {
            let db = db_at(c, b);
            sum[b] += db as f64;
            peak[b] = peak[b].max(db);
        }
    }
    let n = columns.max(1) as f64;
    Spectrum {
        average: sum.iter().map(|s| (s / n) as f32).collect(),
        peak,
        hz_per_bin: sample_rate as f64 / params.window_size as f64,
    }
}

/// `--no-axes`: the spectrograms alone, stacked, with nothing added.
fn stack(lanes: &[Lane]) -> ColorImage {
    let images: Vec<&ColorImage> = lanes
        .iter()
        .filter_map(|l| match l {
            Lane::Spectrogram { image, .. } => Some(image),
            _ => None,
        })
        .collect();
    let w = images.iter().map(|i| i.width()).max().unwrap_or(1);
    let h: usize = images.iter().map(|i| i.height()).sum::<usize>().max(1);
    let mut out = ColorImage::filled([w, h], egui::Color32::BLACK);
    let mut y = 0i64;
    for img in images {
        auriscope::plot::blit(&mut out, img, 0, y);
        y += img.height() as i64;
    }
    out
}

/// The line under the file name: what the file is, and how this picture of it
/// was made. Enough to draw the same one again.
#[allow(clippy::too_many_arguments)]
fn subtitle(
    audio: &DecodedAudio,
    params: &StftParams,
    colormap: ColorMap,
    db_min: f32,
    db_max: f32,
    log: bool,
    min_hz: f32,
    max_hz: f32,
) -> String {
    let info = &audio.info;
    let bits = info
        .bits_per_sample
        .map(|b| format!(" · {b}-bit"))
        .unwrap_or_default();
    format!(
        "{:.3} kHz · {} ch{bits} · {:.3} s · STFT {} {} {} (hop {}){} · {db_min:.0}…{db_max:.0} dB · \
         {} {}–{} · {}",
        info.sample_rate as f64 / 1000.0,
        info.channels,
        info.duration_secs(),
        params.window_size,
        params.window.name(),
        params.overlap_label(),
        params.hop(),
        if params.reassign { " reassigned" } else { "" },
        if log { "log" } else { "linear" },
        auriscope::plot::format_hz(min_hz as f64),
        auriscope::plot::format_hz(max_hz as f64),
        colormap.name(),
    )
}

/// How the picture was framed, in the units a reader of the JSON would want:
/// seconds and hertz, plus what one pixel is worth, so a feature spotted in
/// the image can be turned back into a time in the file.
#[allow(clippy::too_many_arguments)]
fn view_json(
    start: f64,
    end: f64,
    sample_rate: u32,
    channels: &[usize],
    width: i64,
    height: i64,
    min_hz: f32,
    max_hz: f32,
    log: bool,
    image: &Path,
) -> Value {
    json!({
        "image": image.file_name().map(|n| n.to_string_lossy().into_owned()),
        "start_secs": start,
        "end_secs": end,
        "start_frames": (start * sample_rate as f64).round() as i64,
        "end_frames": (end * sample_rate as f64).round() as i64,
        "channels": channels,
        "width_px": width,
        "spectrogram_height_px": height,
        "secs_per_pixel": (end - start) / width.max(1) as f64,
        "min_hz": report::rounded(min_hz, 3),
        "max_hz": report::rounded(max_hz, 3),
        "log_frequency": log,
    })
}

fn analysis_json(
    params: &StftParams,
    db_min: f32,
    db_max: f32,
    contrast: f32,
    colormap: ColorMap,
    merged: bool,
    scale: WaveScale,
) -> Value {
    json!({
        "merged": merged,
        "waveform_scale": scale_name(scale),
        "window_size": params.window_size,
        "window": params.window.name(),
        "overlap": format!("{}/{}", params.overlap_num, params.overlap_den),
        "hop": params.hop(),
        "reassigned": params.reassign,
        "db_floor": report::rounded(db_min, 1),
        "db_ceiling": report::rounded(db_max, 1),
        "contrast": report::rounded(contrast, 2),
        "colormap": colormap.name(),
    })
}

fn stft_params(args: &Args) -> Result<StftParams> {
    let mut params = StftParams::default();
    if let Some(n) = args.get::<usize>("window")? {
        if !StftParams::SIZES.contains(&n) {
            bail!(
                "--window {n}: one of {}",
                StftParams::SIZES
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        params.window_size = n;
    }
    if let Some(spec) = args.str("overlap") {
        let (num, den) = parse_overlap(spec)?;
        params.overlap_num = num;
        params.overlap_den = den;
    }
    if let Some(name) = args.str("window-fn") {
        params.window = WindowKind::ALL
            .into_iter()
            .find(|w| matches(w.name(), name))
            .ok_or_else(|| {
                anyhow!(
                    "--window-fn {name:?}: one of {}",
                    WindowKind::ALL
                        .iter()
                        .map(|w| w.name().to_lowercase())
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            })?;
    }
    params.reassign = args.has("reassign");
    Ok(params)
}

/// `75`, `75%` or `3/4`, all meaning the same overlap. Per cent goes to
/// sixteenths, which is fine enough for every value anyone types and keeps the
/// hop a whole number of samples.
fn parse_overlap(spec: &str) -> Result<(u8, u8)> {
    let spec = spec.trim().trim_end_matches('%');
    if let Some((n, d)) = spec.split_once('/') {
        let num: u8 = n
            .trim()
            .parse()
            .map_err(|_| anyhow!("--overlap {spec:?}"))?;
        let den: u8 = d
            .trim()
            .parse()
            .map_err(|_| anyhow!("--overlap {spec:?}"))?;
        if den == 0 || num >= den {
            bail!("--overlap {spec:?}: must be less than one whole window");
        }
        return Ok((num, den));
    }
    let pct: f64 = spec
        .parse()
        .map_err(|_| anyhow!("--overlap {spec:?}: a percentage or a fraction like 3/4"))?;
    if !(0.0..100.0).contains(&pct) {
        bail!("--overlap {pct}: between 0 and 100 per cent");
    }
    Ok(((pct * 16.0 / 100.0).round() as u8, 16))
}

fn db_range(args: &Args) -> Result<(f32, f32)> {
    let Some(spec) = args.str("db") else {
        return Ok((DEFAULT_DB_FLOOR, DEFAULT_DB_CEILING));
    };
    let (lo, hi) = spec
        .split_once(':')
        .ok_or_else(|| anyhow!("--db {spec:?}: two numbers separated by a colon, like -90:0"))?;
    let parse = |s: &str| -> Result<f32> {
        s.trim()
            .parse::<f32>()
            .map_err(|_| anyhow!("--db {spec:?}: {s:?} is not a number"))
    };
    let (lo, hi) = (parse(lo)?, parse(hi)?);
    if hi <= lo {
        bail!("--db {spec:?}: the ceiling must be above the floor");
    }
    Ok((lo, hi))
}

fn colormap(args: &Args) -> Result<ColorMap> {
    let Some(name) = args.str("colormap") else {
        return Ok(DEFAULT_COLORMAP);
    };
    ColorMap::ALL
        .into_iter()
        .filter(|c| *c != ColorMap::Custom)
        .find(|c| matches(c.name(), name))
        .ok_or_else(|| {
            anyhow!(
                "--colormap {name:?}: one of {}",
                ColorMap::ALL
                    .iter()
                    .filter(|c| **c != ColorMap::Custom)
                    .map(|c| c.name().to_lowercase())
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        })
}

fn scale_name(scale: WaveScale) -> String {
    match scale {
        WaveScale::Db { floor } => format!("dBFS to {floor:.0}"),
        WaveScale::Linear { zoom } if zoom > 1.0 => format!("amplitude x{zoom:.0}"),
        WaveScale::Linear { .. } => "amplitude".into(),
    }
}

/// How the waveform lane is scaled.
///
/// Decibels by default, which is not what the window does: on screen a
/// vertical zoom and a scroll wheel find a noise floor in a second, and a still
/// picture has neither. `--wave-scale linear` puts the window's behaviour back,
/// with `--wave-zoom` standing in for the wheel.
fn wave_scale(args: &Args) -> Result<WaveScale> {
    let zoom = args
        .get::<f32>("wave-zoom")?
        .unwrap_or(1.0)
        .clamp(1.0, 4096.0);
    let floor = args.get::<f32>("wave-db")?.unwrap_or(-90.0);
    if floor >= 0.0 {
        bail!("--wave-db {floor}: a floor below full scale, like -90");
    }
    match args.str("wave-scale").unwrap_or(DEFAULT_WAVE_SCALE) {
        s if matches("db", s) || matches("dbfs", s) => Ok(WaveScale::Db { floor }),
        s if matches("linear", s) => Ok(WaveScale::Linear { zoom }),
        other => bail!("--wave-scale {other:?}: db or linear"),
    }
}

/// The waveform's colour, as `#rrggbb` or `r,g,b`.
fn wave_color(args: &Args) -> Result<egui::Color32> {
    let Some(spec) = args.str("wave-color") else {
        return Ok(Theme::default().wave);
    };
    let bad = || anyhow!("--wave-color {spec:?}: #rrggbb or r,g,b");
    if let Some(hex) = spec.strip_prefix('#') {
        if hex.len() != 6 {
            return Err(bad());
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| bad());
        return Ok(egui::Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?));
    }
    let parts: Vec<&str> = spec.split(',').collect();
    if parts.len() != 3 {
        return Err(bad());
    }
    let byte = |s: &str| s.trim().parse::<u8>().map_err(|_| bad());
    Ok(egui::Color32::from_rgb(
        byte(parts[0])?,
        byte(parts[1])?,
        byte(parts[2])?,
    ))
}

/// Names are matched the way someone would type them: case does not count,
/// and neither does the hyphen in "blackman-harris".
fn matches(name: &str, typed: &str) -> bool {
    let norm = |s: &str| s.to_lowercase().replace(['-', '_', ' '], "");
    norm(name) == norm(typed)
}

/// Where a render goes when nothing said: the file's own name with `.png` on
/// it, in the current directory rather than beside the source, so a run over
/// someone else's folder writes nothing into it.
fn output_path(args: &Args, source: &Path) -> PathBuf {
    if let Some(p) = args.str("output") {
        return PathBuf::from(p);
    }
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "auriscope".into());
    PathBuf::from(format!("{stem}.png"))
}

/// Progress on stderr, so it stays out of the JSON on stdout.
fn notifier(args: &Args) -> impl Fn(&str) + use<> {
    let quiet = args.has("quiet");
    move |s: &str| {
        if !quiet {
            eprintln!("auriscope-cli: {s}");
        }
    }
}

/// Write one row per file: JSON Lines by default, a flat table with `--csv`.
///
/// JSON Lines rather than one big array, because a run over seven thousand
/// takes should be readable as it goes and greppable afterwards, and because a
/// reader does not have to hold the whole thing to process one row.
fn emit_rows(args: &Args, rows: Vec<Row>, single: Option<&str>) -> Result<()> {
    // A lone file asked for by name still gets the readable form.
    if let Some(_key) = single
        && rows.len() == 1
    {
        let row = rows.into_iter().next().expect("one row");
        return match row.result {
            Ok(v) => emit(args, &v),
            Err(e) => Err(e),
        };
    }

    let mut text = String::new();
    if args.has("csv") {
        let headers: Vec<&str> = spec::COLUMNS.iter().map(|(h, _)| *h).collect();
        let qc = rows.iter().any(|r| {
            r.result
                .as_ref()
                .map(|v| !v["qc"].is_null())
                .unwrap_or(false)
        });
        text.push_str(&headers.join(","));
        if qc {
            text.push_str(",qc_pass,qc_fails,qc_rules");
        }
        text.push_str(
            ",error
",
        );
        for row in &rows {
            match &row.result {
                Ok(v) => {
                    let cells: Vec<String> = spec::COLUMNS
                        .iter()
                        .map(|(_, path)| spec::cell(spec::dig(v, path)))
                        .collect();
                    text.push_str(&cells.join(","));
                    if qc {
                        let rules: Vec<String> = v["qc"]["findings"]
                            .as_array()
                            .map(|f| {
                                f.iter()
                                    .filter(|f| f["severity"] == "fail")
                                    .filter_map(|f| f["rule"].as_str().map(str::to_owned))
                                    .collect()
                            })
                            .unwrap_or_default();
                        text.push_str(&format!(
                            ",{},{},{}",
                            spec::cell(v["qc"]["pass"].as_bool().map(Value::from).as_ref()),
                            rules.len(),
                            spec::cell(Some(&Value::from(rules.join(" "))))
                        ));
                    }
                    text.push_str(",\n");
                }
                Err(e) => {
                    // A file that would not open still gets a row, with its
                    // name and the reason. A silently shorter table is worse
                    // than a table with a hole in it.
                    let mut cells = vec![String::new(); spec::COLUMNS.len()];
                    cells[0] = spec::cell(Some(&Value::from(
                        row.path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    )));
                    cells[1] = spec::cell(Some(&Value::from(row.path.display().to_string())));
                    text.push_str(&cells.join(","));
                    if qc {
                        text.push_str(",,,");
                    }
                    text.push(',');
                    text.push_str(&spec::cell(Some(&Value::from(format!("{e:#}")))));
                    text.push('\n');
                }
            }
        }
    } else {
        for row in &rows {
            let line = match &row.result {
                Ok(v) => serde_json::to_string(v)?,
                Err(e) => serde_json::to_string(&json!({
                    "file": { "name": row.path.file_name().map(|n| n.to_string_lossy()),
                              "path": row.path.display().to_string() },
                    "error": format!("{e:#}"),
                }))?,
            };
            text.push_str(&line);
            text.push('\n');
        }
    }

    match args.str("output") {
        Some(path) => std::fs::write(path, text).with_context(|| format!("writing {path}")),
        None => to_stdout(&text),
    }
}

/// Write to stdout, and treat a closed pipe as the end of the job rather than
/// as a failure.
///
/// `… | head -3` closes the pipe after three lines, and the default behaviour
/// then is a panic about a broken pipe — which is no way to answer someone who
/// only wanted to see the first few rows of a long run.
fn to_stdout(text: &str) -> Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => other.context("writing to stdout"),
    }
}

fn emit(args: &Args, value: &Value) -> Result<()> {
    match args.str("output") {
        Some(path) => write_json(Path::new(path), value, args.has("compact")),
        None => {
            let text = match args.has("compact") {
                true => serde_json::to_string(value)?,
                false => serde_json::to_string_pretty(value)?,
            };
            to_stdout(&format!("{text}\n"))
        }
    }
}

fn write_json(path: &Path, value: &Value, compact: bool) -> Result<()> {
    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let w = std::io::BufWriter::new(file);
    match compact {
        true => serde_json::to_writer(w, value),
        false => serde_json::to_writer_pretty(w, value),
    }
    .with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Args {
        Args::parse(list.iter().map(|s| (*s).to_owned())).unwrap()
    }

    #[test]
    fn overlap_is_written_however_it_is_said() {
        assert_eq!(parse_overlap("75").unwrap(), (12, 16));
        assert_eq!(parse_overlap("75%").unwrap(), (12, 16));
        assert_eq!(parse_overlap("3/4").unwrap(), (3, 4));
        assert_eq!(parse_overlap("0").unwrap(), (0, 16));
        // And the hop that comes out of it is the one that was meant.
        let p = StftParams {
            window_size: 2048,
            overlap_num: 12,
            overlap_den: 16,
            ..Default::default()
        };
        assert_eq!(p.hop(), 512);
        assert!(parse_overlap("100").is_err());
        assert!(parse_overlap("5/4").is_err());
        assert!(parse_overlap("half").is_err());
    }

    #[test]
    fn db_range_wants_two_numbers_the_right_way_round() {
        assert_eq!(
            db_range(&args(&["--db", "-60:-10"])).unwrap(),
            (-60.0, -10.0)
        );
        assert_eq!(
            db_range(&args(&[])).unwrap(),
            (DEFAULT_DB_FLOOR, DEFAULT_DB_CEILING)
        );
        assert!(db_range(&args(&["--db", "0:-90"])).is_err());
        assert!(db_range(&args(&["--db", "-90"])).is_err());
    }

    #[test]
    fn names_are_matched_the_way_they_are_typed() {
        assert!(matches("Blackman-Harris", "blackman harris"));
        assert!(matches("Viridis", "VIRIDIS"));
        assert!(!matches("Hann", "hamming"));
        assert_eq!(
            stft_params(&args(&["--window-fn", "blackmanharris"]))
                .unwrap()
                .window,
            WindowKind::BlackmanHarris
        );
        assert_eq!(
            colormap(&args(&["--colormap", "Turbo"])).unwrap(),
            ColorMap::Turbo
        );
        // Custom has no stops to give on a command line, so it is not offered.
        assert!(colormap(&args(&["--colormap", "custom"])).is_err());
    }

    #[test]
    fn a_window_size_off_the_list_says_what_the_list_is() {
        let err = stft_params(&args(&["--window", "3000"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("16384"), "{err}");
    }

    #[test]
    fn the_default_output_is_named_after_the_file_and_lands_here() {
        assert_eq!(
            output_path(&args(&[]), Path::new("/takes/scene 4.wav")),
            PathBuf::from("scene 4.png")
        );
        assert_eq!(
            output_path(
                &args(&["-o", "/tmp/x.png"]),
                Path::new("/takes/scene 4.wav")
            ),
            PathBuf::from("/tmp/x.png")
        );
    }

    #[test]
    fn one_file_at_a_time() {
        assert!(source(&args(&["render"])).is_err());
        assert!(source(&args(&["render", "a.wav", "b.wav"])).is_err());
        assert_eq!(
            source(&args(&["render", "a.wav"])).unwrap(),
            PathBuf::from("a.wav")
        );
    }
}
