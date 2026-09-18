//! Where the speech is, and therefore where the silence is.
//!
//! This is energy against a threshold, not a voice activity model, and the
//! difference is deliberate. A model trained on speech is better at deciding
//! *whether* a sound is a voice; a threshold against the file's own floor is
//! better at saying *what a meter would read*, which is what a delivery spec is
//! written in and what a reviewer with a level meter is doing. Where the two
//! disagree, the disagreement is worth seeing — and it cannot be seen if both
//! instruments are the same instrument.
//!
//! The decisions run on a band-limited copy ([`SPEECH_BAND`]), because rumble
//! is most of the floor on some voices and a broadband threshold would find
//! speech in an empty room. The levels reported are full-band, because that is
//! what a meter shows.
//!
//! **A known bias, stated rather than hidden.** Energy cannot tell a loud
//! breath from a soft word. A breath more than [`Params::speech_db`] above the
//! floor is classified as speech, which splits the pause it sits in into two
//! shorter ones — so pause durations here are, if anything, shorter than a
//! person would call them, never longer. Breaths quiet enough to stay under
//! that threshold are found and marked ([`Pause::event_inside`]). Anyone using
//! these numbers to fail a file for an over-long pause should know which way
//! the error runs.

use super::envelope::Envelope;

/// What a stretch of a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Speech,
    Silence,
}

/// A run of frames of one kind.
#[derive(Debug, Clone, Copy)]
pub struct Segment {
    pub kind: Kind,
    pub start_secs: f64,
    pub end_secs: f64,
    /// Full-band RMS over the segment, in dB.
    pub rms_db: f32,
    /// The same above 80 Hz, where the rumble is not counted. Which of the two
    /// a delivery spec means is often an open question, so both are here.
    pub rms_above_80hz_db: f32,
}

impl Segment {
    pub fn duration_secs(&self) -> f64 {
        self.end_secs - self.start_secs
    }
}

/// A gap between two stretches of speech.
#[derive(Debug, Clone, Copy)]
pub struct Pause {
    pub start_secs: f64,
    pub end_secs: f64,
    /// Whether something above the floor and at least [`EVENT_MS`] long sits
    /// inside it — a breath, usually. Specs commonly allow a longer pause when
    /// one does, so this decides which limit applies.
    pub event_inside: bool,
}

impl Pause {
    pub fn duration_secs(&self) -> f64 {
        self.end_secs - self.start_secs
    }
}

/// How the file is laid out, in the terms a delivery spec is written in.
#[derive(Debug, Clone, Default)]
pub struct Structure {
    /// Start of file to the first speech, and the last speech to the end.
    pub leading_silence_secs: f64,
    pub trailing_silence_secs: f64,
    /// The same two measured to the first and last **non-zero sample** instead.
    /// A file padded with digital silence reads differently by the two rules,
    /// and which one a client means is worth not guessing.
    pub leading_nonzero_secs: f64,
    pub trailing_nonzero_secs: f64,
    pub speech_secs: f64,
    pub speech_ratio: f32,
    pub pauses: Vec<Pause>,
    /// Median level under the speech, against the level of the silence beside
    /// it. Large means the floor you hear during a word is not the floor you
    /// hear between words — which a listener hears as the room switching on and
    /// off, and which is a property of how the file was made rather than a
    /// defect in it.
    pub boundary_step_db: f32,
}

/// How long a non-speech sound has to be before it counts as an event — a
/// breath rather than a click.
pub const EVENT_MS: usize = 60;
/// Speech shorter than this is not speech.
pub const MIN_SPEECH_MS: usize = 100;
/// A dip shorter than this inside speech is phonetic, not a pause.
pub const MIN_GAP_MS: usize = 80;

/// How the segmenter decides.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// How far above the floor a frame has to be to be speech.
    pub speech_db: f32,
    /// How far above it something merely has to be to be *an event* — a
    /// breath, a rustle, anything that is not the room. Below `speech_db`, so
    /// there is a band in which a sound is present without being a word.
    pub event_db: f32,
    pub min_speech_ms: usize,
    pub min_gap_ms: usize,
    pub event_ms: usize,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            speech_db: 10.0,
            event_db: 6.0,
            min_speech_ms: MIN_SPEECH_MS,
            min_gap_ms: MIN_GAP_MS,
            event_ms: EVENT_MS,
        }
    }
}

/// Everything the segmenter worked out, together.
pub struct Segmentation {
    pub segments: Vec<Segment>,
    pub structure: Structure,
    /// One flag per envelope frame: was this frame speech. Other detectors take
    /// it to stay out of the way of words.
    pub speech_frames: Vec<bool>,
    /// The floor the threshold was set from, in dB, on the band-limited copy.
    pub floor_db: f32,
    pub threshold_db: f32,
}

/// Split a file into speech and silence.
///
/// `decision` is the band-limited envelope the threshold is applied to;
/// `full` and `above_80` are the envelopes the reported levels come from. All
/// three must share a hop, which they do when they come from the same call
/// site.
pub fn segment(
    decision: &Envelope,
    full: &Envelope,
    above_80: &Envelope,
    samples: &[f32],
    params: Params,
) -> Segmentation {
    let floor_db = decision.floor_db();
    let threshold_db = floor_db + params.speech_db;
    let n = decision.len();
    let hop_ms = decision.hop_ms().max(1);
    let min_speech = (params.min_speech_ms / hop_ms).max(1);
    let min_gap = (params.min_gap_ms / hop_ms).max(1);

    let mut speech: Vec<bool> = (0..n)
        .map(|i| decision.rms_db[i].is_finite() && decision.rms_db[i] >= threshold_db)
        .collect();
    close_gaps(&mut speech, min_gap, true);
    drop_runs(&mut speech, min_speech, true);

    let sr = decision.sample_rate as f64;
    let duration = samples.len() as f64 / sr;
    let level = |env: &Envelope, a: usize, b: usize| -> f32 {
        let v: Vec<f32> = (a..b)
            .filter_map(|i| env.rms_db.get(i).copied())
            .filter(|d| d.is_finite())
            .collect();
        if v.is_empty() {
            return f32::NEG_INFINITY;
        }
        // Mean of the decibel values: what a meter's needle settles on, and
        // less swayed by one loud frame than averaging the power would be.
        v.iter().sum::<f32>() / v.len() as f32
    };

    let mut segments = Vec::new();
    for (start, end, is_speech) in runs(&speech) {
        let start_secs = (start * decision.hop_len) as f64 / sr;
        let end_secs = ((end * decision.hop_len) as f64 / sr).min(duration);
        segments.push(Segment {
            kind: if is_speech {
                Kind::Speech
            } else {
                Kind::Silence
            },
            start_secs,
            end_secs,
            rms_db: level(full, start, end),
            rms_above_80hz_db: level(above_80, start, end),
        });
    }

    let first = segments.iter().find(|s| s.kind == Kind::Speech);
    let last = segments.iter().rev().find(|s| s.kind == Kind::Speech);
    let mut structure = Structure {
        leading_silence_secs: first.map_or(duration, |s| s.start_secs),
        trailing_silence_secs: last.map_or(duration, |s| duration - s.end_secs),
        ..Default::default()
    };

    let nonzero = samples.iter().position(|v| *v != 0.0);
    structure.leading_nonzero_secs = nonzero.map_or(duration, |i| i as f64 / sr);
    structure.trailing_nonzero_secs = samples
        .iter()
        .rposition(|v| *v != 0.0)
        .map_or(duration, |i| duration - (i + 1) as f64 / sr);

    structure.speech_secs = segments
        .iter()
        .filter(|s| s.kind == Kind::Speech)
        .map(Segment::duration_secs)
        .sum();
    structure.speech_ratio = if duration > 0.0 {
        (structure.speech_secs / duration) as f32
    } else {
        0.0
    };

    // Pauses are the silences *between* words. The lead and the tail are not
    // pauses: they are measured against a different rule and a different limit.
    let event_frames = (params.event_ms / hop_ms).max(1);
    let speech_indices: Vec<usize> = segments
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == Kind::Speech)
        .map(|(i, _)| i)
        .collect();
    if let (Some(&a), Some(&b)) = (speech_indices.first(), speech_indices.last()) {
        for (i, seg) in segments.iter().enumerate() {
            if seg.kind != Kind::Silence || i < a || i > b {
                continue;
            }
            let (f0, f1) = (
                decision.frame_at(seg.start_secs),
                decision.frame_at(seg.end_secs),
            );
            structure.pauses.push(Pause {
                start_secs: seg.start_secs,
                end_secs: seg.end_secs,
                event_inside: longest_above(decision, f0, f1, floor_db + params.event_db)
                    >= event_frames,
            });
        }
    }

    structure.boundary_step_db = boundary_step(full, &speech);

    Segmentation {
        segments,
        structure,
        speech_frames: speech,
        floor_db,
        threshold_db,
    }
}

/// The quiet moments inside speech against the silence beside it.
///
/// The floor under a word cannot be measured directly — the word is on top of
/// it — so this takes the fifth percentile of the frames inside speech, which
/// is the gaps between phonemes, and compares it to the median of the silence.
/// A broadband estimate of something usually done per frequency band, and
/// reported as an estimate.
fn boundary_step(full: &Envelope, speech: &[bool]) -> f32 {
    let pick = |want: bool| -> Vec<f32> {
        let mut v: Vec<f32> = speech
            .iter()
            .enumerate()
            .filter(|(_, s)| **s == want)
            .filter_map(|(i, _)| full.rms_db.get(i).copied())
            .filter(|d| d.is_finite())
            .collect();
        v.sort_by(f32::total_cmp);
        v
    };
    let inside = pick(true);
    let outside = pick(false);
    if inside.is_empty() || outside.is_empty() {
        return 0.0;
    }
    let at = |v: &[f32], f: f64| v[(((v.len() - 1) as f64) * f).round() as usize];
    at(&inside, 0.05) - at(&outside, 0.50)
}

/// The longest run of frames above `db` between `a` and `b`.
fn longest_above(env: &Envelope, a: usize, b: usize, db: f32) -> usize {
    let mut best = 0;
    let mut run = 0;
    for i in a..b.min(env.len()) {
        if env.rms_db[i].is_finite() && env.rms_db[i] >= db {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

/// Fill runs of `!want` shorter than `min` — a dip inside a word.
fn close_gaps(flags: &mut [bool], min: usize, want: bool) {
    for (start, end, value) in runs(flags) {
        if value != want && end - start < min && start > 0 && end < flags.len() {
            for f in &mut flags[start..end] {
                *f = want;
            }
        }
    }
}

/// Clear runs of `want` shorter than `min` — a blip that is not a word.
fn drop_runs(flags: &mut [bool], min: usize, want: bool) {
    for (start, end, value) in runs(flags) {
        if value == want && end - start < min {
            for f in &mut flags[start..end] {
                *f = !want;
            }
        }
    }
}

/// `(start, end, value)` for each run of equal flags.
fn runs(flags: &[bool]) -> Vec<(usize, usize, bool)> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < flags.len() {
        let value = flags[start];
        let mut end = start + 1;
        while end < flags.len() && flags[end] == value {
            end += 1;
        }
        out.push((start, end, value));
        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::envelope::{SPEECH_BAND, band_limit, high_pass};
    use super::*;

    /// A file shaped like a take: lead, two words with a pause, tail.
    fn take(sr: u32) -> Vec<f32> {
        let floor = 10f32.powf(-60.0 / 20.0);
        let loud = 10f32.powf(-12.0 / 20.0);
        let n = sr as usize * 3;
        let mut x: Vec<f32> = (0..n)
            .map(|i| floor * (std::f32::consts::TAU * 1000.0 * i as f32 / sr as f32).sin())
            .collect();
        let mut word = |from: f64, to: f64| {
            let (a, b) = ((from * sr as f64) as usize, (to * sr as f64) as usize);
            for (i, v) in x.iter_mut().enumerate().take(b).skip(a) {
                *v = loud * (std::f32::consts::TAU * 900.0 * i as f32 / sr as f32).sin();
            }
        };
        word(0.5, 1.0);
        word(1.9, 2.6);
        x
    }

    fn segmentation(x: &[f32], sr: u32) -> Segmentation {
        let decision =
            Envelope::compute(&band_limit(x, sr, SPEECH_BAND.0, SPEECH_BAND.1), sr, 20, 5);
        let full = Envelope::compute(x, sr, 20, 5);
        let above = Envelope::compute(&high_pass(x, sr, 80.0), sr, 20, 5);
        segment(&decision, &full, &above, x, Params::default())
    }

    #[test]
    fn a_take_splits_into_lead_words_pause_and_tail() {
        let sr = 48_000;
        let s = segmentation(&take(sr), sr);
        let speech: Vec<&Segment> = s
            .segments
            .iter()
            .filter(|x| x.kind == Kind::Speech)
            .collect();
        assert_eq!(speech.len(), 2, "two words");
        assert!(
            (speech[0].start_secs - 0.5).abs() < 0.03,
            "{}",
            speech[0].start_secs
        );
        assert!((speech[0].end_secs - 1.0).abs() < 0.03);
        assert!((speech[1].start_secs - 1.9).abs() < 0.03);

        let st = &s.structure;
        assert!((st.leading_silence_secs - 0.5).abs() < 0.03);
        assert!((st.trailing_silence_secs - 0.4).abs() < 0.03);
        assert_eq!(st.pauses.len(), 1, "one gap between the two words");
        assert!((st.pauses[0].duration_secs() - 0.9).abs() < 0.05);
        assert!(
            !st.pauses[0].event_inside,
            "the pause is plain room tone, nothing in it"
        );
        assert!((st.speech_ratio - 0.4).abs() < 0.05);
    }

    /// The lead and the tail are not pauses. Counting them would fail a file
    /// for its own opening silence, which the spec measures by another rule.
    #[test]
    fn the_lead_and_tail_are_not_counted_as_pauses() {
        let sr = 48_000;
        let s = segmentation(&take(sr), sr);
        for p in &s.structure.pauses {
            assert!(
                p.start_secs > 0.4 && p.end_secs < 2.7,
                "{p:?} is a lead or tail"
            );
        }
    }

    /// Something in a pause changes which limit applies to it, so it has to be
    /// noticed.
    #[test]
    fn an_event_inside_a_pause_is_noticed() {
        let sr = 48_000;
        let mut x = take(sr);
        // A quiet 100 ms puff in the middle of the pause. The floor reads
        // about -63 dB in the decision band, so this sits ~7 dB over it:
        // above the event threshold, below the one for speech.
        let breath = 10f32.powf(-52.5 / 20.0);
        let (a, b) = ((1.3 * sr as f64) as usize, (1.4 * sr as f64) as usize);
        for (i, v) in x.iter_mut().enumerate().take(b).skip(a) {
            *v = breath * (std::f32::consts::TAU * 2000.0 * i as f32 / sr as f32).sin();
        }
        let s = segmentation(&x, sr);
        assert_eq!(s.structure.pauses.len(), 1);
        assert!(
            s.structure.pauses[0].event_inside,
            "a 100 ms breath in the pause should be seen"
        );
    }

    /// Digital padding and a measured lead are different lengths, and a spec
    /// may mean either.
    #[test]
    fn padding_is_reported_apart_from_silence() {
        let sr = 48_000;
        let mut x = take(sr);
        for v in x.iter_mut().take(sr as usize / 10) {
            *v = 0.0; // 100 ms of true digital silence at the head
        }
        let st = segmentation(&x, sr).structure;
        assert!((st.leading_nonzero_secs - 0.1).abs() < 0.01);
        assert!(
            (st.leading_silence_secs - 0.5).abs() < 0.03,
            "the lead is still half a second of not-speech"
        );
    }

    #[test]
    fn a_file_with_no_speech_is_all_silence() {
        let sr = 8000;
        let quiet: Vec<f32> = (0..sr as usize).map(|i| 1e-4 * (i as f32).sin()).collect();
        let s = segmentation(&quiet, sr);
        assert!(s.segments.iter().all(|x| x.kind == Kind::Silence));
        assert_eq!(s.structure.speech_secs, 0.0);
        assert!(s.structure.pauses.is_empty());
    }
}
