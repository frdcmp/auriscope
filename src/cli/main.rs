//! Auriscope without the window: what the views show, as a PNG and as JSON,
//! for a script or a machine reader rather than a pair of eyes at a screen.
//!
//! Everything it does, the window does too. The difference is that nothing
//! here opens one, so it runs over a folder, in a pipeline, or on a box with
//! no display — and the picture it writes carries its own axes, because a
//! spectrogram with no numbers around it says that something happened without
//! saying when or at what frequency.

mod args;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use egui::ColorImage;
use serde_json::{Value, json};

use args::Args;
use auriscope::analysis::{
    ColorMap, DEFAULT_CUSTOM, DetailTile, Spectrogram, StftParams, ViewParams, WaveformPyramid,
    WindowKind, compute_stats, render_view,
};
use auriscope::audio::{DecodedAudio, decode_file};
use auriscope::plot::figure::{ColorBar, WaveScale};
use auriscope::plot::{Figure, Lane, Text, Theme, write_png};
use auriscope::report;

const HELP: &str = "\
Auriscope CLI — audio analysis without a window

Usage:
  auriscope-cli analyze FILE [options]
  auriscope-cli render  FILE [options]

Commands:
  analyze   write what the sidebar knows about the file, as JSON
  render    write the waveform and spectrogram as an annotated PNG

Common options:
  -o, --output PATH    where to write (analyze defaults to stdout)
      --quiet          no progress on stderr
  -h, --help           this text
  -V, --version        print the version and exit

analyze options:
      --compact        one line of JSON instead of indented

render options:
      --start SECS     where the picture begins (default 0)
      --end SECS       where it ends (default the end of the file)
      --channel N      one channel only, counting from 0 (default all)
      --width PX       width of the plotted area (default 1600)
      --height PX      height of each spectrogram (default 340)
      --wave-height PX height of each waveform (default 90, 0 for none)
      --no-waveform    spectrogram only
      --wave-scale S   db or linear (default db: shows the noise floor)
      --window N       FFT size: 256 512 1024 2048 4096 8192 16384 (default 2048)
      --overlap PCT    window overlap, per cent or as a fraction (default 75)
      --window-fn NAME hann hamming blackman blackman-harris rectangular
      --reassign       sharpen lines and clicks by reassignment (slower)
      --db MIN:MAX     decibel range of the colour map (default -90:0)
      --min-hz HZ      bottom of the frequency axis (default 20 log, 0 linear)
      --max-hz HZ      top of it (default Nyquist)
      --linear         linear frequency axis instead of logarithmic
      --colormap NAME  amber ember magma inferno viridis plasma turbo grey
      --contrast F     gamma on the level before it is coloured (default 1)
      --no-axes        the bare spectrogram, with no margins or labels
      --json PATH      also write the analyze report, with how this was framed

Examples:
  auriscope-cli analyze take.wav | jq .loudness
  auriscope-cli render take.wav -o take.png
  auriscope-cli render take.wav --start 3.1 --end 3.7 --window 1024 -o click.png
";

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
        "render" => render(&args),
        other => bail!("unknown command {other:?}; try --help"),
    }
}

/// The file to work on: the one positional argument that is not the command.
fn source(args: &Args) -> Result<PathBuf> {
    match args.positional.len() {
        0 | 1 => bail!("no file given"),
        2 => Ok(PathBuf::from(&args.positional[1])),
        _ => bail!(
            "one file at a time, got {}",
            args.positional[1..].join(", ")
        ),
    }
}

/// Decode, and measure while everything is in memory. Loudness that will not
/// compute is a warning and a null block, not a failure: the rest of the
/// report is still worth having.
fn open(
    path: &Path,
    note: &dyn Fn(&str),
) -> Result<(DecodedAudio, Option<auriscope::analysis::FileStats>)> {
    note("decoding");
    let audio = decode_file(path, &|_| {})?;
    let audio =
        std::sync::Arc::try_unwrap(audio).map_err(|_| anyhow!("decoded file is still shared"))?;
    if audio.sample_rate() == 0 || audio.channels.is_empty() {
        bail!("{} has no audio in it", path.display());
    }
    note("measuring");
    let stats = match compute_stats(&audio.channels, audio.sample_rate()) {
        Ok(s) => Some(s),
        Err(e) => {
            log::warn!("loudness: {e}");
            None
        }
    };
    Ok((audio, stats))
}

fn analyze(args: &Args) -> Result<()> {
    let path = source(args)?;
    let note = notifier(args);
    let (audio, stats) = open(&path, &note)?;
    let report = report::file_report(&audio.info, stats.as_ref());
    emit(args, &report)
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
    let log = !args.has("linear");
    let (db_min, db_max) = db_range(args)?;
    let contrast = args.get::<f32>("contrast")?.unwrap_or(1.0).clamp(0.25, 4.0);
    let colormap = colormap(args)?;
    let scale = wave_scale(args, db_min)?;
    let want_start = args.get::<f64>("start")?.unwrap_or(0.0).max(0.0);
    let want_end = args.get::<f64>("end")?;
    let want_channel = args.get::<usize>("channel")?;

    let (audio, stats) = open(&path, &note)?;
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
    let mut lanes = Vec::new();
    for (i, &ch) in channels.iter().enumerate() {
        note(&format!("spectrogram {}/{}", i + 1, channels.len()));
        let samples = &audio.channels[ch];
        let name = report::channel_name(ch, nch);
        let image = spectrogram_image(
            samples,
            sr,
            params,
            start_frame,
            end_frame,
            width as usize,
            height as usize,
            &ViewTemplate {
                min_hz,
                max_hz,
                log,
                db_min,
                db_max,
            },
            &lut,
        )?;
        // The units live in the lane titles. In the margins they would have
        // to share a line with the topmost tick label, and two bits of text
        // on top of each other are worse than none.
        let axis = if log { "Hz, log" } else { "Hz, linear" };
        if let Some(p) = &pyramid {
            lanes.push(Lane::Waveform {
                title: format!("{name} · {}", scale_name(scale)),
                bins: p.query(ch, samples, start_frame, end_frame, width as usize),
                height: wave_height,
                scale,
            });
            lanes.push(Lane::Spectrogram {
                title: axis.to_owned(),
                image,
                min_hz: min_hz as f64,
                max_hz: max_hz as f64,
                log,
            });
        } else {
            lanes.push(Lane::Spectrogram {
                title: format!("{name} · {axis}"),
                image,
                min_hz: min_hz as f64,
                max_hz: max_hz as f64,
                log,
            });
        }
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
            colorbar: Some(ColorBar {
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
                analysis_json(&params, db_min, db_max, contrast, colormap),
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
) -> Result<ColorImage> {
    let pad = params.window_size as f64;
    let slice_start = (start_frame - pad).floor().max(0.0) as usize;
    let slice_end = ((end_frame + pad).ceil().max(0.0) as usize).min(samples.len());
    if slice_start >= slice_end {
        return Ok(ColorImage::filled([width, height], lut[0]));
    }
    let slice = &samples[slice_start..slice_end];
    let spec = Spectrogram::compute(slice, sample_rate, params, &|_| {}, &|| false)
        .context("computing the spectrogram")?;
    let (from, to) = (
        start_frame - slice_start as f64,
        end_frame - slice_start as f64,
    );
    let tile = DetailTile::compute(slice, sample_rate, params, from, to, width, &|| false);
    let view = ViewParams {
        start_frame: from,
        end_frame: to,
        min_hz: view.min_hz,
        max_hz: view.max_hz,
        log_frequency: view.log,
        db_min: view.db_min,
        db_max: view.db_max,
    };
    Ok(render_view(&spec, tile.as_ref(), &view, width, height, lut))
}

/// `--no-axes`: the spectrograms alone, stacked, with nothing added.
fn stack(lanes: &[Lane]) -> ColorImage {
    let images: Vec<&ColorImage> = lanes
        .iter()
        .filter_map(|l| match l {
            Lane::Spectrogram { image, .. } => Some(image),
            Lane::Waveform { .. } => None,
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
) -> Value {
    json!({
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
        return Ok((-90.0, 0.0));
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
        return Ok(ColorMap::Amber);
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

fn scale_name(scale: WaveScale) -> &'static str {
    match scale {
        WaveScale::Db { .. } => "dBFS",
        WaveScale::Linear => "amplitude",
    }
}

fn wave_scale(args: &Args, db_min: f32) -> Result<WaveScale> {
    match args.str("wave-scale").unwrap_or("db") {
        s if matches("db", s) || matches("dbfs", s) => Ok(WaveScale::Db { floor: db_min }),
        s if matches("linear", s) => Ok(WaveScale::Linear),
        other => bail!("--wave-scale {other:?}: db or linear"),
    }
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

fn emit(args: &Args, value: &Value) -> Result<()> {
    match args.str("output") {
        Some(path) => write_json(Path::new(path), value, args.has("compact")),
        None => {
            let text = match args.has("compact") {
                true => serde_json::to_string(value)?,
                false => serde_json::to_string_pretty(value)?,
            };
            println!("{text}");
            Ok(())
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
        assert_eq!(db_range(&args(&[])).unwrap(), (-90.0, 0.0));
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
