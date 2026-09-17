//! The annotated plot: margins, lanes, ticks and a colour bar around whatever
//! was rendered.
//!
//! A bare spectrogram is an unanchored picture. It shows that something
//! happened without saying when, at what frequency, or how loud, which is
//! exactly what a reader — a person glancing at a file, or a model asked what
//! is wrong with it — needs to know. Everything in this module exists to put
//! the numbers back around the picture.
//!
//! A [`Figure`] is passed content that has already been rendered: the
//! spectrogram lanes arrive as images from
//! [`render_view`](crate::analysis::render_view), the waveform lanes as the
//! [`Bin`]s a [`WaveformPyramid`](crate::analysis::WaveformPyramid) query
//! returns. This module decides where they go and what is written beside them.
//!
//! Lanes come in two kinds, and the difference decides the layout. Waveforms
//! and spectrograms run along *time*, so they share one axis drawn once under
//! the last of them. A [`Lane::Spectrum`] does not: its horizontal axis is
//! frequency, so it sits below that shared axis with an axis of its own.

use egui::{Color32, ColorImage, FontId};

use super::{
    Px, Text, Theme, blit, fill, format_hz, format_time, hline, linear_ticks, log_ticks, nice_step,
    tint, vline,
};
use crate::analysis::{Bin, amp_to_db};

/// Room for the frequency and level labels down the left.
const LEFT: i64 = 58;
/// Room for the colour bar and its labels down the right.
const BAR: i64 = 58;
/// Room for an axis's labels along the bottom, and the unit under them.
const BOTTOM: i64 = 42;
const PAD: i64 = 10;
const LANE_GAP: i64 = 8;

/// How a waveform's height is scaled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WaveScale {
    /// Amplitude as it is, times `zoom`, clipped at the edge of the lane. What
    /// the window draws: the familiar shape, the one that shows asymmetry and
    /// DC, and the one a vertical zoom lifts a noise floor out of.
    Linear { zoom: f32 },
    /// Decibels from full scale down to `floor`. Compresses the loud end to
    /// show the quiet one, which is where a noise floor, a room tone and the
    /// gap between two takes live. Nothing is clipped, so it needs no zoom.
    Db { floor: f32 },
}

/// A waveform drawn over a spectrogram rather than beside it.
pub struct Overlay {
    pub bins: Vec<Bin>,
    pub scale: WaveScale,
    /// How strongly the waveform is drawn over what is underneath, 0 to 1.
    pub opacity: f32,
    pub color: Color32,
}

/// One horizontal strip of the plot.
pub enum Lane {
    /// A rendered spectrogram, with the frequency range it was drawn over so
    /// the axis beside it can be labelled.
    Spectrogram {
        title: String,
        image: ColorImage,
        min_hz: f64,
        max_hz: f64,
        log: bool,
        /// A waveform drawn on top, for the merged view.
        overlay: Option<Overlay>,
        /// How strongly the spectrogram itself is drawn, 0 to 1. Below 1 it is
        /// faded towards the background so an overlay reads over it.
        opacity: f32,
    },
    /// A waveform, one [`Bin`] per pixel column.
    Waveform {
        title: String,
        bins: Vec<Bin>,
        height: i64,
        scale: WaveScale,
        color: Color32,
    },
    /// The level of each frequency across the whole span drawn above: the
    /// still-picture answer to the window's realtime spectrum. Horizontal axis
    /// is frequency, vertical is decibels.
    Spectrum {
        title: String,
        /// Mean level per FFT bin over the span, in dB.
        average: Vec<f32>,
        /// The loudest that bin reached anywhere in the span, in dB.
        peak: Vec<f32>,
        hz_per_bin: f64,
        min_hz: f64,
        max_hz: f64,
        log: bool,
        db_min: f32,
        db_max: f32,
        height: i64,
    },
}

impl Lane {
    fn height(&self) -> i64 {
        match self {
            Lane::Spectrogram { image, .. } => image.height() as i64,
            Lane::Waveform { height, .. } | Lane::Spectrum { height, .. } => *height,
        }
    }

    fn title(&self) -> &str {
        match self {
            Lane::Spectrogram { title, .. }
            | Lane::Waveform { title, .. }
            | Lane::Spectrum { title, .. } => title,
        }
    }

    /// Whether this lane runs along time and so shares the time axis.
    fn is_time(&self) -> bool {
        !matches!(self, Lane::Spectrum { .. })
    }
}

/// The frequency range a spectrogram lane was drawn over, and how: what the
/// axis beside it has to be labelled with.
#[derive(Debug, Clone, Copy)]
struct Freq {
    min_hz: f64,
    max_hz: f64,
    log: bool,
}

impl Freq {
    /// Where a frequency sits in the range, 0 at the bottom and 1 at the top.
    fn at(&self, hz: f64) -> f64 {
        if self.log {
            (hz.ln() - self.min_hz.ln()) / (self.max_hz.ln() - self.min_hz.ln())
        } else {
            (hz - self.min_hz) / (self.max_hz - self.min_hz)
        }
    }

    /// The frequency at `t` of the way up the range: the inverse of [`at`].
    fn hz_at(&self, t: f64) -> f64 {
        if self.log {
            (self.min_hz.ln() + (self.max_hz.ln() - self.min_hz.ln()) * t).exp()
        } else {
            self.min_hz + (self.max_hz - self.min_hz) * t
        }
    }

    fn ticks(&self, want: usize) -> Vec<f64> {
        if self.log {
            log_ticks(self.min_hz, self.max_hz)
        } else {
            linear_ticks(self.min_hz, self.max_hz, want)
        }
    }
}

/// The colour bar: which colours mean which levels.
pub struct ColorBar {
    pub lut: Vec<Color32>,
    pub db_min: f32,
    pub db_max: f32,
}

/// A plot waiting to be drawn.
pub struct Figure {
    /// Width of the content, not of the finished image: the margins are added
    /// around this.
    pub width: i64,
    pub title: String,
    pub subtitle: String,
    /// The span of the file the time lanes cover, which is what the time axis
    /// is labelled in.
    pub start_secs: f64,
    pub end_secs: f64,
    pub lanes: Vec<Lane>,
    pub colorbar: Option<ColorBar>,
    pub theme: Theme,
}

impl Figure {
    /// Total size of the finished image.
    pub fn size(&self) -> (i64, i64) {
        let right = if self.colorbar.is_some() { BAR } else { PAD };
        (LEFT + self.width + right, self.top_margin() + self.body())
    }

    /// Everything below the title: the lanes, and one axis strip per axis.
    fn body(&self) -> i64 {
        let time: Vec<&Lane> = self.lanes.iter().filter(|l| l.is_time()).collect();
        let mut h = 0;
        if !time.is_empty() {
            h += time.iter().map(|l| l.height()).sum::<i64>()
                + LANE_GAP * (time.len() as i64 - 1)
                + BOTTOM;
        }
        for lane in self.lanes.iter().filter(|l| !l.is_time()) {
            h += LANE_GAP + lane.height() + BOTTOM;
        }
        h
    }

    fn top_margin(&self) -> i64 {
        let mut top = PAD;
        if !self.title.is_empty() {
            top += 18;
        }
        if !self.subtitle.is_empty() {
            top += 15;
        }
        top + 6
    }

    /// Draw it.
    pub fn render(&self, text: &mut Text) -> ColorImage {
        let (w, h) = self.size();
        let t = &self.theme;
        let mut img = ColorImage::filled([w.max(1) as usize, h.max(1) as usize], t.background);

        let label = FontId::proportional(11.0);
        let title = FontId::proportional(13.0);
        let small = FontId::proportional(10.0);

        let mut y = PAD;
        if !self.title.is_empty() {
            text.draw(
                &mut img,
                LEFT as f32,
                y as f32,
                &self.title,
                &title,
                t.foreground,
            );
            y += 18;
        }
        if !self.subtitle.is_empty() {
            text.draw(
                &mut img,
                LEFT as f32,
                y as f32,
                &self.subtitle,
                &small,
                t.dim,
            );
        }

        // Where the time ticks go is decided once: every time lane shares the
        // axis, and a gridline that did not line up between lanes would be
        // worse than none at all.
        let span = self.end_secs - self.start_secs;
        let want = (self.width / 110).clamp(2, 12) as usize;
        let step = nice_step(span, want);
        let ticks = linear_ticks(self.start_secs, self.end_secs, want);
        let clock = self.end_secs.abs() >= 60.0;
        let x_of = |secs: f64| -> i64 {
            if span <= 0.0 {
                return LEFT;
            }
            LEFT + (((secs - self.start_secs) / span) * self.width as f64).round() as i64
        };

        // The time lanes, then their shared axis, then anything on another
        // axis with its own strip underneath it.
        let mut top = self.top_margin();
        let mut time_bottom = top;
        for lane in self.lanes.iter().filter(|l| l.is_time()) {
            let rect = Px::new(LEFT, top, self.width, lane.height());
            self.draw_lane(&mut img, text, rect, lane, &label, &small);
            for &tick in &ticks {
                // Gridlines over the content rather than under it, faint
                // enough to read a spectrogram through.
                tint(
                    &mut img,
                    Px::new(x_of(tick), rect.y, 1, rect.h),
                    t.foreground,
                    48,
                );
            }
            self.border(&mut img, rect);
            self.lane_title(&mut img, text, rect, lane.title(), &small);
            time_bottom = rect.bottom();
            top = rect.bottom() + LANE_GAP;
        }

        if time_bottom > self.top_margin() {
            let unit = if clock { "m:ss" } else { "seconds" };
            for &tick in &ticks {
                let x = x_of(tick);
                vline(&mut img, x, time_bottom, 4, t.axis);
                let s = format_time(tick, step, clock);
                let (tw, _) = text.measure(&s, &label);
                text.draw(
                    &mut img,
                    x as f32 - tw / 2.0,
                    time_bottom as f32 + 6.0,
                    &s,
                    &label,
                    t.dim,
                );
            }
            let (uw, _) = text.measure(unit, &label);
            text.draw(
                &mut img,
                (LEFT + self.width) as f32 - uw,
                time_bottom as f32 + 19.0,
                unit,
                &label,
                t.dim,
            );
            top = time_bottom + BOTTOM + LANE_GAP;
        }

        for lane in self.lanes.iter().filter(|l| !l.is_time()) {
            let rect = Px::new(LEFT, top, self.width, lane.height());
            self.draw_lane(&mut img, text, rect, lane, &label, &small);
            self.border(&mut img, rect);
            self.lane_title(&mut img, text, rect, lane.title(), &small);
            top = rect.bottom() + BOTTOM + LANE_GAP;
        }

        if let Some(bar) = &self.colorbar {
            // The bar explains the spectrogram, so it stands beside the time
            // lanes rather than running the whole height of the image.
            self.color_bar(&mut img, text, bar, time_bottom, &label, &small);
        }
        img
    }

    fn draw_lane(
        &self,
        img: &mut ColorImage,
        text: &mut Text,
        rect: Px,
        lane: &Lane,
        label: &FontId,
        small: &FontId,
    ) {
        let t = &self.theme;
        match lane {
            Lane::Spectrogram {
                image,
                min_hz,
                max_hz,
                log,
                overlay,
                opacity,
                ..
            } => {
                blit(img, image, rect.x, rect.y);
                // Fading towards the panel colour rather than towards black:
                // the quiet end of every colour map is already black, and
                // fading to it would leave the lane looking merely quieter.
                let faded = ((1.0 - opacity.clamp(0.0, 1.0)) * 255.0) as u8;
                if faded > 0 {
                    tint(img, rect, t.panel, faded);
                }
                let freq = Freq {
                    min_hz: *min_hz,
                    max_hz: *max_hz,
                    log: *log,
                };
                self.frequency_axis(img, text, rect, freq, label);
                if let Some(o) = overlay {
                    self.waveform(
                        img, text, rect, &o.bins, o.scale, o.color, true, o.opacity, label,
                    );
                }
            }
            Lane::Waveform {
                bins, scale, color, ..
            } => {
                fill(img, rect, t.panel);
                self.waveform(img, text, rect, bins, *scale, *color, false, 1.0, label);
            }
            Lane::Spectrum { .. } => self.spectrum(img, text, rect, lane, label, small),
        }
    }

    /// The lane's name, inside it at the top left: outside it there is no room,
    /// and a channel name belongs to the picture it names.
    fn lane_title(
        &self,
        img: &mut ColorImage,
        text: &mut Text,
        rect: Px,
        name: &str,
        font: &FontId,
    ) {
        if name.is_empty() {
            return;
        }
        let (tw, th) = text.measure(name, font);
        tint(
            img,
            Px::new(rect.x + 4, rect.y + 4, tw as i64 + 8, th as i64 + 2),
            Color32::BLACK,
            150,
        );
        text.draw(
            img,
            rect.x as f32 + 8.0,
            rect.y as f32 + 5.0,
            name,
            font,
            self.theme.foreground,
        );
    }

    fn border(&self, img: &mut ColorImage, r: Px) {
        let c = self.theme.axis;
        hline(img, r.x, r.y - 1, r.w, c);
        hline(img, r.x, r.bottom(), r.w, c);
        vline(img, r.x - 1, r.y - 1, r.h + 2, c);
        vline(img, r.right(), r.y - 1, r.h + 2, c);
    }

    /// Hertz down the left of a spectrogram lane. The mapping is the one
    /// [`render_view`](crate::analysis::render_view) drew with: the top row is
    /// `max_hz`, and a logarithmic axis is even in the log of the frequency.
    fn frequency_axis(
        &self,
        img: &mut ColorImage,
        text: &mut Text,
        r: Px,
        freq: Freq,
        font: &FontId,
    ) {
        if freq.max_hz <= freq.min_hz {
            return;
        }
        let t = &self.theme;
        for hz in freq.ticks((r.h / 60).clamp(2, 10) as usize) {
            let y = r.bottom() - (freq.at(hz) * r.h as f64).round() as i64;
            if y < r.y || y > r.bottom() {
                continue;
            }
            tint(img, Px::new(r.x, y, r.w, 1), t.foreground, 34);
            hline(img, r.x - 4, y, 4, t.axis);
            let s = format_hz(hz);
            let (tw, th) = text.measure(&s, font);
            text.draw(
                img,
                r.x as f32 - 7.0 - tw,
                y as f32 - th / 2.0,
                &s,
                font,
                t.dim,
            );
        }
    }

    /// A waveform lane: peak span in one colour with the RMS drawn inside it in
    /// a lighter one, symmetrical about the centre line.
    ///
    /// `over` is the merged view: the waveform drawn into a spectrogram's lane
    /// rather than into one of its own. It says where the thing is, not how
    /// strongly it is drawn — a fully opaque overlay is still an overlay, and
    /// still has to keep its labels out of the margin the frequency axis owns.
    #[allow(clippy::too_many_arguments)]
    fn waveform(
        &self,
        img: &mut ColorImage,
        text: &mut Text,
        r: Px,
        bins: &[Bin],
        scale: WaveScale,
        color: Color32,
        over: bool,
        opacity: f32,
        font: &FontId,
    ) {
        let t = &self.theme;
        let half = (r.h / 2).max(1);
        let mid = r.y + half;
        let alpha = (opacity.clamp(0.0, 1.0) * 255.0) as u8;
        // The RMS reads as a brighter core inside the peak span.
        let rms_color = Color32::from_rgb(
            color.r().saturating_add(60),
            color.g().saturating_add(50),
            color.b().saturating_add(40),
        );

        // Level gridlines, and the numbers that go with them. Over a
        // spectrogram the frequency axis owns the left margin, so the levels
        // are labelled inside the lane on the right instead — which is where
        // the window puts them in its merged view, and the only way to have
        // both rulers without one landing on top of the other.
        let ruling = |img: &mut ColorImage, text: &mut Text, dy: i64, db: f32| {
            for y in [mid - dy, mid + dy] {
                tint(
                    img,
                    Px::new(r.x, y, r.w, 1),
                    t.foreground,
                    if over { 22 } else { 30 },
                );
            }
            let s = format!("{db:.0}");
            let (tw, th) = text.measure(&s, font);
            let (x, y) = match over {
                true => (r.right() as f32 - tw - 5.0, (mid - dy) as f32 - th / 2.0),
                false => (r.x as f32 - 7.0 - tw, (mid - dy) as f32 - th / 2.0),
            };
            if over {
                tint(
                    img,
                    Px::new(x as i64 - 3, y as i64, tw as i64 + 6, th as i64),
                    Color32::BLACK,
                    140,
                );
            }
            text.draw(img, x, y, &s, font, t.dim);
        };
        {
            // At a zoom of 1 the lane is exactly full scale, so 0 dBFS is the
            // top edge. Labelled just inside it rather than centred on it,
            // which would put half the text outside the lane.
            let top_is_full_scale = matches!(scale, WaveScale::Linear { zoom } if zoom <= 1.0)
                || matches!(scale, WaveScale::Db { .. });
            if top_is_full_scale && half > 14 {
                let s = "0";
                let (tw, th) = text.measure(s, font);
                let (x, y) = match over {
                    true => (r.right() as f32 - tw - 5.0, (mid - half) as f32 + 1.0),
                    false => (r.x as f32 - 7.0 - tw, (mid - half) as f32 + 1.0),
                };
                if over {
                    tint(
                        img,
                        Px::new(x as i64 - 3, y as i64, tw as i64 + 6, th as i64),
                        Color32::BLACK,
                        140,
                    );
                }
                text.draw(img, x, y, s, font, t.dim);
            }
            match scale {
                WaveScale::Db { floor } => {
                    // Lines about every twelve pixels of half-height, and a
                    // label on as many of them as will fit without touching: a
                    // dB scale is symmetrical about the centre line, so the
                    // labels have half the lane to sit in, not all of it.
                    let step = nice_step(-floor as f64, (half / 12).clamp(2, 8) as usize);
                    let line_px = (step / -floor as f64) * half as f64;
                    let every = (14.0 / line_px.max(1.0)).ceil().max(1.0) as usize;
                    let mut db = -step;
                    let mut i = 1;
                    while db > floor as f64 {
                        let dy = ((1.0 - (db / floor as f64)) * half as f64).round() as i64;
                        if i % every == 0 {
                            ruling(img, text, dy, db as f32);
                        }
                        db -= step;
                        i += 1;
                    }
                }
                WaveScale::Linear { zoom } => {
                    // A linear lane is labelled in decibels too, the way the
                    // window labels it: the line for a level is wherever that
                    // amplitude lands once the zoom has been applied, which is
                    // what makes a vertical zoom readable rather than just big.
                    // Candidates from loud to quiet, each kept only if it
                    // clears the last one, so the labels never pile up however
                    // far the zoom is pushed.
                    let mut last = i64::MAX;
                    for step in 1..=30 {
                        let db = -3.0 * step as f32;
                        let amp = 10f32.powf(db / 20.0) * zoom.max(1e-6);
                        if amp > 1.0 {
                            continue;
                        }
                        let dy = (amp as f64 * half as f64).round() as i64;
                        if dy < 6 {
                            break;
                        }
                        if last - dy < 14 {
                            continue;
                        }
                        last = dy;
                        ruling(img, text, dy, db);
                    }
                }
            }
        }
        // The centre line goes down whether or not this is a lane of its own,
        // so a silent stretch reads as silence rather than as nothing at all.
        tint(
            img,
            Px::new(r.x, mid, r.w, 1),
            t.foreground,
            if over { 40 } else { 70 },
        );

        let height = |v: f32| -> i64 {
            let h = match scale {
                WaveScale::Linear { zoom } => (v.abs() * zoom).min(1.0) as f64,
                WaveScale::Db { floor } => {
                    let db = amp_to_db(v.abs());
                    if db <= floor {
                        0.0
                    } else {
                        (1.0 - db as f64 / floor as f64).clamp(0.0, 1.0)
                    }
                }
            };
            (h * half as f64).round() as i64
        };

        for (i, bin) in bins.iter().enumerate().take(r.w.max(0) as usize) {
            let x = r.x + i as i64;
            let peak = height(bin.min.abs().max(bin.max.abs()));
            let rms = height(bin.rms).min(peak);
            if over {
                // The RMS core is drawn over a spectrogram too: it is what
                // tells a loud passage from a peaky one, and leaving it out
                // made a merged waveform a flat silhouette.
                tint(img, Px::new(x, mid - peak, 1, peak * 2 + 1), color, alpha);
                tint(img, Px::new(x, mid - rms, 1, rms * 2 + 1), rms_color, alpha);
            } else {
                fill(img, Px::new(x, mid - peak, 1, peak * 2 + 1), color);
                fill(img, Px::new(x, mid - rms, 1, rms * 2 + 1), rms_color);
            }
        }
    }

    /// The spectrum lane: level against frequency over the whole span above.
    ///
    /// Each pixel column takes every bin that falls in it — the mean for the
    /// average trace, the largest for the peak. On a log axis the low end has
    /// several columns per bin and the high end several bins per column, so
    /// sampling one bin per column would drop most of the top octave.
    fn spectrum(
        &self,
        img: &mut ColorImage,
        text: &mut Text,
        r: Px,
        lane: &Lane,
        font: &FontId,
        small: &FontId,
    ) {
        let Lane::Spectrum {
            average,
            peak,
            hz_per_bin,
            min_hz,
            max_hz,
            log,
            db_min,
            db_max,
            ..
        } = lane
        else {
            return;
        };
        let t = &self.theme;
        fill(img, r, t.panel);
        if average.is_empty() || *hz_per_bin <= 0.0 || max_hz <= min_hz {
            return;
        }
        let freq = Freq {
            min_hz: *min_hz,
            max_hz: *max_hz,
            log: *log,
        };
        let span = (db_max - db_min).max(1e-6);
        let y_of = |db: f32| -> i64 {
            let frac = ((db - db_min) / span).clamp(0.0, 1.0) as f64;
            r.bottom() - (frac * r.h as f64).round() as i64
        };

        // Decibel gridlines and their labels down the left.
        for db in linear_ticks(
            *db_min as f64,
            *db_max as f64,
            (r.h / 34).clamp(2, 8) as usize,
        ) {
            let y = y_of(db as f32);
            tint(img, Px::new(r.x, y, r.w, 1), t.foreground, 30);
            hline(img, r.x - 4, y, 4, t.axis);
            let s = format!("{db:.0}");
            let (tw, th) = text.measure(&s, font);
            text.draw(
                img,
                r.x as f32 - 7.0 - tw,
                y as f32 - th / 2.0,
                &s,
                font,
                t.dim,
            );
        }

        // Frequency gridlines, and the axis under the lane.
        let ticks = freq.ticks((r.w / 90).clamp(2, 12) as usize);
        for hz in &ticks {
            let x = r.x + (freq.at(*hz) * r.w as f64).round() as i64;
            if x < r.x || x > r.right() {
                continue;
            }
            tint(img, Px::new(x, r.y, 1, r.h), t.foreground, 30);
            vline(img, x, r.bottom(), 4, t.axis);
            let s = format_hz(*hz);
            let (tw, _) = text.measure(&s, font);
            text.draw(
                img,
                x as f32 - tw / 2.0,
                r.bottom() as f32 + 6.0,
                &s,
                font,
                t.dim,
            );
        }
        let (uw, _) = text.measure("Hz", font);
        text.draw(
            img,
            r.right() as f32 - uw,
            r.bottom() as f32 + 19.0,
            "Hz",
            font,
            t.dim,
        );
        let _ = small;

        // The traces. Peak first, as a line; the average filled underneath it,
        // so the two never hide each other.
        let bins = average.len();
        let mut previous: Option<i64> = None;
        let bin_at = |hz: f64| (hz / hz_per_bin).round().clamp(0.0, (bins - 1) as f64) as usize;
        for px in 0..r.w {
            let lo = freq.hz_at(px as f64 / r.w as f64);
            let hi = freq.hz_at((px + 1) as f64 / r.w as f64);
            let (b0, b1) = (bin_at(lo), bin_at(hi).max(bin_at(lo) + 1).min(bins));
            if b0 >= b1 {
                continue;
            }
            let mean = average[b0..b1].iter().sum::<f32>() / (b1 - b0) as f32;
            let top = peak[b0..b1]
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max);
            let x = r.x + px;
            let y_mean = y_of(mean);
            let y_peak = y_of(top);
            fill(img, Px::new(x, y_mean, 1, r.bottom() - y_mean), t.spectrum);
            // Joined to where the last column left it, so the trace reads as
            // one line rather than as a row of dots.
            let (a, b) = match previous {
                Some(prev) if prev < y_peak => (prev, y_peak),
                Some(prev) => (y_peak, prev),
                None => (y_peak, y_peak),
            };
            fill(img, Px::new(x, a, 1, (b - a) + 1), t.spectrum_peak);
            previous = Some(y_peak);
        }
    }

    /// The colour bar down the right: the same lookup table the spectrogram was
    /// coloured with, against the decibels it was mapped from.
    fn color_bar(
        &self,
        img: &mut ColorImage,
        text: &mut Text,
        bar: &ColorBar,
        bottom: i64,
        font: &FontId,
        small: &FontId,
    ) {
        if bar.lut.is_empty() {
            return;
        }
        let t = &self.theme;
        let top = self.top_margin();
        let h = (bottom - top).max(1);
        let x = LEFT + self.width + 12;
        let w = 12;
        for i in 0..h {
            let frac = 1.0 - i as f64 / (h - 1).max(1) as f64;
            let idx = ((frac * (bar.lut.len() - 1) as f64).round() as usize).min(bar.lut.len() - 1);
            fill(img, Px::new(x, top + i, w, 1), bar.lut[idx]);
        }
        self.border(img, Px::new(x, top, w, h));

        let span = (bar.db_max - bar.db_min) as f64;
        let want = (h / 45).clamp(2, 8) as usize;
        for db in linear_ticks(bar.db_min as f64, bar.db_max as f64, want) {
            let frac = (db - bar.db_min as f64) / span.max(1e-9);
            let y = (bottom - (frac * h as f64).round() as i64).clamp(top, bottom);
            hline(img, x + w, y, 4, t.axis);
            let s = format!("{db:.0}");
            let (_, th) = text.measure(&s, font);
            text.draw(
                img,
                (x + w + 7) as f32,
                y as f32 - th / 2.0,
                &s,
                font,
                t.dim,
            );
        }
        text.draw(img, x as f32, top as f32 - 14.0, "dB", small, t.dim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_lane(h: usize) -> Lane {
        Lane::Spectrogram {
            title: "Mono".into(),
            image: ColorImage::filled([200, h], Color32::from_rgb(1, 2, 3)),
            min_hz: 20.0,
            max_hz: 20000.0,
            log: true,
            overlay: None,
            opacity: 1.0,
        }
    }

    fn wave_lane(h: i64) -> Lane {
        Lane::Waveform {
            title: "Mono".into(),
            bins: vec![
                Bin {
                    min: -0.5,
                    max: 0.5,
                    rms: 0.25
                };
                200
            ],
            height: h,
            scale: WaveScale::Db { floor: -90.0 },
            color: Color32::from_rgb(90, 165, 235),
        }
    }

    fn spectrum_lane(h: i64) -> Lane {
        Lane::Spectrum {
            title: "Spectrum".into(),
            average: (0..513).map(|i| -90.0 + i as f32 / 10.0).collect(),
            peak: (0..513).map(|i| -80.0 + i as f32 / 10.0).collect(),
            hz_per_bin: 48000.0 / 1024.0,
            min_hz: 20.0,
            max_hz: 20000.0,
            log: true,
            db_min: -90.0,
            db_max: 0.0,
            height: h,
        }
    }

    fn figure(lanes: Vec<Lane>) -> Figure {
        Figure {
            width: 200,
            title: "take 2.wav".into(),
            subtitle: "48 kHz · 1 ch".into(),
            start_secs: 0.0,
            end_secs: 3.0,
            lanes,
            colorbar: Some(ColorBar {
                lut: vec![Color32::BLACK, Color32::WHITE],
                db_min: -90.0,
                db_max: 0.0,
            }),
            theme: Theme::default(),
        }
    }

    /// The margins are added around the content, not taken out of it: a lane
    /// asked for at 200x120 is drawn at 200x120 whatever else is on the plot.
    #[test]
    fn content_keeps_its_size_and_the_margins_go_around_it() {
        let f = figure(vec![spec_lane(120)]);
        let (w, h) = f.size();
        assert_eq!(w, LEFT + 200 + BAR);
        assert_eq!(h, f.top_margin() + 120 + BOTTOM);

        let two = figure(vec![spec_lane(120), spec_lane(120)]);
        assert_eq!(two.size().1, two.top_margin() + 240 + LANE_GAP + BOTTOM);
    }

    /// A spectrum is on its own axis, so it costs a second axis strip rather
    /// than sharing the time one.
    #[test]
    fn a_spectrum_lane_brings_its_own_axis() {
        let f = figure(vec![spec_lane(120), spectrum_lane(90)]);
        assert_eq!(
            f.size().1,
            f.top_margin() + 120 + BOTTOM + LANE_GAP + 90 + BOTTOM
        );
        // And on its own it is the only thing there, with no time axis.
        let alone = figure(vec![spectrum_lane(90)]);
        assert_eq!(alone.size().1, alone.top_margin() + LANE_GAP + 90 + BOTTOM);
    }

    /// Without a colour bar the right margin is only padding.
    #[test]
    fn the_bar_is_what_widens_the_right_margin() {
        let mut f = figure(vec![spec_lane(60)]);
        f.colorbar = None;
        assert_eq!(f.size().0, LEFT + 200 + PAD);
    }

    /// The whole plot draws, at the size it said it would, with the lane's
    /// pixels where the layout puts them.
    #[test]
    fn it_draws_the_content_where_it_says_it_will() {
        let f = figure(vec![spec_lane(80), wave_lane(60)]);
        let (w, h) = f.size();
        let img = f.render(&mut Text::new());
        assert_eq!(img.size, [w as usize, h as usize]);

        let at = |x: i64, y: i64| img.pixels[(y * w + x) as usize];
        // The spectrogram's own pixels, blitted whole.
        assert_eq!(
            at(LEFT + 100, f.top_margin() + 40),
            Color32::from_rgb(1, 2, 3)
        );
        let wave_top = f.top_margin() + 80 + LANE_GAP;
        assert!(at(LEFT + 100, wave_top + 30) != f.theme.panel);
        // The frequency labels are right-aligned against the lane, so they
        // land at the inner end of the left margin rather than at its edge.
        assert!(
            (0..LEFT - 6).any(|x| (0..h).any(|y| at(x, y) != f.theme.background)),
            "the axis labels go in the left margin"
        );
    }

    /// A merged lane draws the waveform into the spectrogram's own rectangle
    /// rather than taking a strip of its own.
    #[test]
    fn merging_costs_no_extra_height() {
        let plain = figure(vec![spec_lane(120)]);
        let merged = figure(vec![Lane::Spectrogram {
            title: "Mono".into(),
            image: ColorImage::filled([200, 120], Color32::from_rgb(1, 2, 3)),
            min_hz: 20.0,
            max_hz: 20000.0,
            log: true,
            opacity: 1.0,
            overlay: Some(Overlay {
                bins: vec![
                    Bin {
                        min: -0.9,
                        max: 0.9,
                        rms: 0.5
                    };
                    200
                ],
                scale: WaveScale::Linear { zoom: 1.0 },
                opacity: 0.45,
                color: Color32::from_rgb(90, 165, 235),
            }),
        }]);
        assert_eq!(plain.size(), merged.size());

        // And the overlay actually marks the spectrogram's pixels.
        let (w, _) = merged.size();
        let img = merged.render(&mut Text::new());
        let mid = merged.top_margin() + 60;
        assert!(
            (0..200)
                .any(|x| img.pixels[(mid * w + LEFT + x) as usize] != Color32::from_rgb(1, 2, 3)),
            "the overlaid waveform should be visible over the spectrogram"
        );
    }

    /// Vertical zoom lifts a quiet signal up the lane; that is its whole job.
    #[test]
    fn vertical_zoom_lifts_a_quiet_signal() {
        let quiet = |zoom: f32| {
            let f = figure(vec![Lane::Waveform {
                title: String::new(),
                bins: vec![
                    Bin {
                        min: -0.002,
                        max: 0.002,
                        rms: 0.001
                    };
                    200
                ],
                height: 100,
                scale: WaveScale::Linear { zoom },
                color: Color32::from_rgb(90, 165, 235),
            }]);
            let (w, _) = f.size();
            let img = f.render(&mut Text::new());
            let mid = f.top_margin() + 50;
            // How tall the drawn envelope is at the middle column.
            (0..50)
                .filter(|dy| {
                    img.pixels[((mid - dy) * w + LEFT + 100) as usize]
                        == Color32::from_rgb(90, 165, 235)
                })
                .count()
        };
        assert!(
            quiet(200.0) > quiet(1.0),
            "a zoom of 200 should draw the signal taller than no zoom at all"
        );
    }

    /// A degenerate span must not divide by zero or draw a tick per pixel.
    #[test]
    fn an_empty_span_still_draws() {
        let mut f = figure(vec![spec_lane(40)]);
        f.end_secs = f.start_secs;
        let img = f.render(&mut Text::new());
        assert_eq!(img.size, [f.size().0 as usize, f.size().1 as usize]);
    }
}
