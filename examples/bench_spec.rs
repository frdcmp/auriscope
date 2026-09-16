//! Timing harness for the spectrogram render path.
//!
//!     cargo run --release --example bench_spec -- file.wav [window] [overlap_den]

use std::time::Instant;

use auriscope::analysis::{ColorMap, DetailTile, Spectrogram, StftParams, ViewParams, render_view};
use auriscope::audio::decode_file;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: bench_spec FILE [WINDOW] [OVERLAP_DEN]");
    let window: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(2048);
    let den: u8 = args.next().and_then(|s| s.parse().ok()).unwrap_or(4);
    let audio = decode_file(std::path::Path::new(&path), &|_| {})?;
    let sr = audio.sample_rate();
    let samples = &audio.channels[0];
    let frames = samples.len() as f64;
    println!(
        "{} frames @ {sr} Hz, {} ch",
        samples.len(),
        audio.channels.len()
    );

    for reassign in [false, true] {
        let params = StftParams {
            window_size: window,
            overlap_num: den - 1,
            overlap_den: den,
            reassign,
            ..Default::default()
        };
        let t = Instant::now();
        let spec = Spectrogram::compute(samples, sr, params, &|_| {}, &|| false).unwrap();
        println!(
            "[reassign={reassign}] whole-file spectrogram: {} cols in {:.0} ms",
            spec.columns(),
            t.elapsed().as_secs_f64() * 1e3
        );
        let lut = ColorMap::Amber.lut(1.0);
        let (w, h) = (1774usize, 900usize);
        for zoom in [1.0, 8.0, 64.0, 512.0] {
            let len = frames / zoom;
            let start = frames * 0.4;
            let view = ViewParams {
                start_frame: start,
                end_frame: start + len,
                min_hz: 20.0,
                max_hz: sr as f32 / 2.0,
                log_frequency: true,
                db_min: -90.0,
                db_max: 0.0,
            };
            let t = Instant::now();
            let _ = render_view(&spec, None, &view, w, h, &lut);
            let base_ms = t.elapsed().as_secs_f64() * 1e3;

            let fpp = len / w as f64;
            let mut tile_ms = 0.0;
            let mut tiled_ms = 0.0;
            let mut cols = 0;
            if fpp < params.hop() as f64 {
                let pad = len * 0.2;
                let a = (start - pad).max(0.0);
                let b = (start + len + pad).min(frames);
                cols = (((b - a) / fpp).ceil() as usize).clamp(1, DetailTile::MAX_COLUMNS);
                let t = Instant::now();
                let tile = DetailTile::compute(samples, sr, params, a, b, cols, &|| false).unwrap();
                tile_ms = t.elapsed().as_secs_f64() * 1e3;
                let t = Instant::now();
                let _ = render_view(&spec, Some(&tile), &view, w, h, &lut);
                tiled_ms = t.elapsed().as_secs_f64() * 1e3;
            }
            println!(
                "  zoom {zoom:>5}x  {fpp:8.1} frames/px  render(base) {base_ms:6.1} ms  \
                 tile({cols} cols) {tile_ms:7.1} ms  render(tile) {tiled_ms:6.1} ms"
            );
        }
    }
    // Parallel tile against a single-threaded one, byte for byte.
    let params = StftParams {
        window_size: window,
        overlap_num: den - 1,
        overlap_den: den,
        reassign: true,
        ..Default::default()
    };
    let (a, b) = (frames * 0.68, frames * 0.75);
    let cols = 4000.min(DetailTile::MAX_COLUMNS);
    let par = DetailTile::compute(samples, sr, params, a, b, cols, &|| false).unwrap();
    // SAFETY: single-threaded example; nothing else reads the environment.
    unsafe { std::env::set_var("AURISCOPE_THREADS", "1") };
    let seq = DetailTile::compute(samples, sr, params, a, b, cols, &|| false).unwrap();
    let cells = || (0..cols).flat_map(|c| (0..par.bins).map(move |b| (c, b)));
    let diff = cells()
        .filter(|&(c, b)| par.db_at(c, b) != seq.db_at(c, b))
        .count();
    let mean = |t: &DetailTile| {
        cells().map(|(c, b)| t.db_at(c, b) as f64).sum::<f64>() / (cols * par.bins) as f64
    };
    println!(
        "tile parallel vs sequential: {diff} of {} cells differ; mean level {:.2} dB vs {:.2} dB",
        cols * par.bins,
        mean(&par),
        mean(&seq),
    );
    Ok(())
}
