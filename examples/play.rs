//! Headless playback smoke test: decode a file, play `secs` seconds through
//! the default device, report playhead progress and underruns.
//!
//!     cargo run --example play -- file.wav [secs] [seek_secs]

use std::time::{Duration, Instant};

use auriscope::audio::{Engine, decode_file};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: play FILE [SECS] [SEEK_SECS]");
    let secs: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(3.0);
    let seek: Option<f64> = args.next().and_then(|s| s.parse().ok());

    let t0 = Instant::now();
    let audio = decode_file(std::path::Path::new(&path), &|_| {})?;
    let info = &audio.info;
    println!(
        "decoded {} in {:.0} ms: {} {} Hz {} ch {} frames ({:.2} s){}",
        info.file_name(),
        t0.elapsed().as_secs_f64() * 1000.0,
        info.codec,
        info.sample_rate,
        info.channels,
        info.frames,
        info.duration_secs(),
        info.bits_per_sample
            .map(|b| format!(" {b}-bit"))
            .unwrap_or_default()
    );

    let mut engine = Engine::new(audio.clone())?;
    println!(
        "device: {} @ {} Hz, {} ch, {:?}, resampling={}",
        engine.device_name,
        engine.device_rate,
        engine.device_channels,
        engine.sample_format,
        engine.resampling
    );
    if let Some(s) = seek {
        engine.seek((s * info.sample_rate as f64) as u64);
    }
    engine.play();
    let start = Instant::now();
    let mut last = 0u64;
    let mut tap_samples = 0usize;
    let mut tap_peak = 0.0f32;
    while start.elapsed() < Duration::from_secs_f64(secs) {
        std::thread::sleep(Duration::from_millis(250));
        let ph = engine.playhead();
        engine.drain_tap(|s| {
            tap_samples += s.len();
            for v in s {
                tap_peak = tap_peak.max(v.abs());
            }
        });
        println!(
            "  t={:.2}s playhead={:.3}s (+{} frames) playing={} underruns={}",
            start.elapsed().as_secs_f64(),
            ph as f64 / info.sample_rate as f64,
            ph.saturating_sub(last),
            engine.is_playing(),
            engine
                .shared
                .underruns
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        last = ph;
    }
    // Seek while playing: the callback must drop stale chunks.
    engine.seek(0);
    std::thread::sleep(Duration::from_millis(300));
    let after = engine.playhead() as f64 / info.sample_rate as f64;
    println!("after seek(0)+300ms: playhead={after:.3}s");
    engine.pause();
    println!("tap: {} mono samples, peak {:.3}", tap_samples, tap_peak);
    let elapsed = start.elapsed().as_secs_f64();
    let expected = seek.unwrap_or(0.0) + elapsed.min(secs);
    let _ = expected;
    Ok(())
}
