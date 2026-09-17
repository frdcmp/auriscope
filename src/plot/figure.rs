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
/// Room for the time labels along the bottom, and the unit under them.
const BOTTOM: i64 = 42;
const PAD: i64 = 10;
const LANE_GAP: i64 = 8;

/// How a waveform's height is scaled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WaveScale {
    /// Amplitude as it is: full scale fills the lane. The familiar shape, and
    /// the one that shows asymmetry and DC.
    Linear,
    /// Decibels from full scale down to `floor`. Compresses the loud end to
    /// show the quiet one, which is where a noise floor, a room tone and the
    /// gap between two takes live.
    Db { floor: f32 },
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
    },
    /// A waveform, one [`Bin`] per pixel column.
    Waveform {
        title: String,
        bins: Vec<Bin>,
        height: i64,
        scale: WaveScale,
    },
}

impl Lane {
    fn height(&self) -> i64 {
        match self {
            Lane::Spectrogram { image, .. } => image.height() as i64,
            Lane::Waveform { height, .. } => *height,
        }
    }

    fn title(&self) -> &str {
        match self {
            Lane::Spectrogram { title, .. } | Lane::Waveform { title, .. } => title,
        }
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
    /// The span of the file the lanes cover, which is what the time axis is
    /// labelled in.
    pub start_secs: f64,
    pub end_secs: f64,
    pub lanes: Vec<Lane>,
    pub colorbar: Option<ColorBar>,
    pub theme: Theme,
}

impl Figure {
    /// Total size of the finished image.
    pub fn size(&self) -> (i64, i64) {
        let top = self.top_margin();
        let right = if self.colorbar.is_some() { BAR } else { PAD };
        let content: i64 = self.lanes.iter().map(Lane::height).sum::<i64>()
            + LANE_GAP * (self.lanes.len().max(1) as i64 - 1);
        (LEFT + self.width + right, top + content + BOTTOM)
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

        // Where the ticks go is decided once: every lane shares the time axis,
        // and a gridline that did not line up between lanes would be worse
        // than none at all.
        let span = self.end_secs - self.start_secs;
        let step = nice_step(span, (self.width / 110).clamp(2, 12) as usize);
        let ticks = linear_ticks(
            self.start_secs,
            self.end_secs,
            (self.width / 110).clamp(2, 12) as usize,
        );
        let clock = self.end_secs.abs() >= 60.0;
        let x_of = |secs: f64| -> i64 {
            if span <= 0.0 {
                return LEFT;
            }
            LEFT + (((secs - self.start_secs) / span) * self.width as f64).round() as i64
        };

        let mut top = self.top_margin();
        for lane in &self.lanes {
            let rect = Px::new(LEFT, top, self.width, lane.height());
            match lane {
                Lane::Spectrogram {
                    image,
                    min_hz,
                    max_hz,
                    log,
                    ..
                } => {
                    blit(&mut img, image, rect.x, rect.y);
                    let freq = Freq {
                        min_hz: *min_hz,
                        max_hz: *max_hz,
                        log: *log,
                    };
                    self.frequency_axis(&mut img, text, rect, freq, &label);
                }
                Lane::Waveform { bins, scale, .. } => {
                    fill(&mut img, rect, t.panel);
                    self.waveform(&mut img, text, rect, bins, *scale, &label);
                }
            }
            // Gridlines over the content rather than under it, faint enough
            // to read a spectrogram through.
            for &tick in &ticks {
                tint(
                    &mut img,
                    Px::new(x_of(tick), rect.y, 1, rect.h),
                    t.foreground,
                    48,
                );
            }
            self.border(&mut img, rect);
            if !lane.title().is_empty() {
                // Inside the lane, top left: outside it there is no room, and
                // a channel name belongs to the picture it names.
                let name = lane.title();
                let (tw, th) = text.measure(name, &small);
                tint(
                    &mut img,
                    Px::new(rect.x + 4, rect.y + 4, tw as i64 + 8, th as i64 + 2),
                    Color32::BLACK,
                    150,
                );
                text.draw(
                    &mut img,
                    rect.x as f32 + 8.0,
                    rect.y as f32 + 5.0,
                    name,
                    &small,
                    t.foreground,
                );
            }
            top = rect.bottom() + LANE_GAP;
        }

        // The time axis, once, under the last lane.
        let axis_y = top - LANE_GAP;
        for &tick in &ticks {
            let x = x_of(tick);
            vline(&mut img, x, axis_y, 4, t.axis);
            let s = format_time(tick, step, clock);
            let (tw, _) = text.measure(&s, &label);
            text.draw(
                &mut img,
                x as f32 - tw / 2.0,
                axis_y as f32 + 6.0,
                &s,
                &label,
                t.dim,
            );
        }
        let unit = if clock { "m:ss" } else { "seconds" };
        let (uw, _) = text.measure(unit, &label);
        text.draw(
            &mut img,
            (LEFT + self.width) as f32 - uw,
            axis_y as f32 + 6.0 + 13.0,
            unit,
            &label,
            t.dim,
        );

        if let Some(bar) = &self.colorbar {
            self.color_bar(&mut img, text, bar, &label, &small);
        }
        img
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
        let Freq {
            min_hz,
            max_hz,
            log,
        } = freq;
        if max_hz <= min_hz {
            return;
        }
        let t = &self.theme;
        let pos = |hz: f64| -> f64 {
            if log {
                (hz.ln() - min_hz.ln()) / (max_hz.ln() - min_hz.ln())
            } else {
                (hz - min_hz) / (max_hz - min_hz)
            }
        };
        let ticks = if log {
            log_ticks(min_hz, max_hz)
        } else {
            linear_ticks(min_hz, max_hz, (r.h / 60).clamp(2, 10) as usize)
        };
        for hz in ticks {
            let y = r.bottom() - (pos(hz) * r.h as f64).round() as i64;
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

    /// A waveform lane: peak span in one colour with the RMS drawn inside it
    /// in another, symmetrical about the centre line.
    fn waveform(
        &self,
        img: &mut ColorImage,
        text: &mut Text,
        r: Px,
        bins: &[Bin],
        scale: WaveScale,
        font: &FontId,
    ) {
        let t = &self.theme;
        let half = (r.h / 2).max(1);
        let mid = r.y + half;

        // Level gridlines, and on a dB scale the numbers that go with them.
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
                    let frac = 1.0 - (db / floor as f64);
                    let dy = (frac * half as f64).round() as i64;
                    for y in [mid - dy, mid + dy] {
                        tint(img, Px::new(r.x, y, r.w, 1), t.foreground, 30);
                    }
                    if i % every == 0 {
                        let s = format!("{db:.0}");
                        let (tw, th) = text.measure(&s, font);
                        text.draw(
                            img,
                            r.x as f32 - 7.0 - tw,
                            (mid - dy) as f32 - th / 2.0,
                            &s,
                            font,
                            t.dim,
                        );
                    }
                    db -= step;
                    i += 1;
                }
            }
            WaveScale::Linear => {
                for f in [0.5, 1.0] {
                    let dy = (f * half as f64).round() as i64;
                    for y in [mid - dy, mid + dy] {
                        tint(img, Px::new(r.x, y, r.w, 1), t.foreground, 30);
                    }
                }
            }
        }

        // The centre line goes down first so a silent stretch still draws as a
        // line rather than as nothing at all.
        tint(img, Px::new(r.x, mid, r.w, 1), t.foreground, 70);

        let height = |v: f32| -> i64 {
            let h = match scale {
                WaveScale::Linear => v.abs().min(1.0) as f64,
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
            fill(img, Px::new(x, mid - peak, 1, peak * 2 + 1), t.wave);
            fill(img, Px::new(x, mid - rms, 1, rms * 2 + 1), t.wave_rms);
        }
    }

    /// The colour bar down the right: the same lookup table the spectrogram
    /// was coloured with, against the decibels it was mapped from.
    fn color_bar(
        &self,
        img: &mut ColorImage,
        text: &mut Text,
        bar: &ColorBar,
        font: &FontId,
        small: &FontId,
    ) {
        if bar.lut.is_empty() {
            return;
        }
        let t = &self.theme;
        let top = self.top_margin();
        let bottom = self.size().1 - BOTTOM - LANE_GAP;
        let h = (bottom - top).max(1);
        let x = LEFT + self.width + 12;
        let w = 12;
        for i in 0..h {
            let frac = 1.0 - i as f64 / (h - 1).max(1) as f64;
            let c = bar.lut
                [((frac * (bar.lut.len() - 1) as f64).round() as usize).min(bar.lut.len() - 1)];
            fill(img, Px::new(x, top + i, w, 1), c);
        }
        self.border(img, Px::new(x, top, w, h));

        let span = (bar.db_max - bar.db_min) as f64;
        let step = nice_step(span, (h / 45).clamp(2, 8) as usize);
        for db in linear_ticks(
            bar.db_min as f64,
            bar.db_max as f64,
            (h / 45).clamp(2, 8) as usize,
        ) {
            let frac = (db - bar.db_min as f64) / span.max(1e-9);
            let y = bottom - (frac * h as f64).round() as i64;
            hline(img, x + w, y.clamp(top, bottom), 4, t.axis);
            let s = format!("{db:.0}");
            let (_, th) = text.measure(&s, font);
            text.draw(
                img,
                (x + w + 7) as f32,
                y.clamp(top, bottom) as f32 - th / 2.0,
                &s,
                font,
                t.dim,
            );
            let _ = step;
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
        let f = figure(vec![
            spec_lane(80),
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
                height: 60,
                scale: WaveScale::Db { floor: -90.0 },
            },
        ]);
        let (w, h) = f.size();
        let img = f.render(&mut Text::new());
        assert_eq!(img.size, [w as usize, h as usize]);

        // The spectrogram's own pixels, blitted whole.
        let at = |x: i64, y: i64| img.pixels[(y * w + x) as usize];
        assert_eq!(
            at(LEFT + 100, f.top_margin() + 40),
            Color32::from_rgb(1, 2, 3)
        );
        // Something was drawn in the waveform lane and in the left margin.
        let wave_top = f.top_margin() + 80 + LANE_GAP;
        assert!(at(LEFT + 100, wave_top + 30) != f.theme.panel);
        // The frequency labels are right-aligned against the lane, so they
        // land at the inner end of the left margin rather than at its edge.
        assert!(
            (0..LEFT - 6).any(|x| (0..h).any(|y| at(x, y) != f.theme.background)),
            "the axis labels go in the left margin"
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
