//! Delivery specs, and checking a file against one.
//!
//! A spec is the numbers a client agreed to, written where a person can read
//! and edit them. Everything it holds is optional: a rule that is not in the
//! file is not checked, so a spec can start as two thresholds and grow.
//!
//! Findings carry three severities, and the middle one earns its keep. `fail`
//! is a defect. `warn` is close to the line. **`note` is for a measurement that
//! is worth recording and is not the file's fault** — a floor step at speech
//! boundaries on a voice whose room is louder than the bed under it, say. Left
//! as a failure it would condemn every file of that speaker and bury the real
//! defects; left out altogether the sheet would not show the thing the client
//! is going to ask about.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

/// Which reading of a silence level a spec means.
///
/// Rumble sits below most of what a listener notices, so the two readings can
/// differ by several decibels on the same file; which one a client uses is
/// often not written down anywhere. Both are always measured. This says which
/// one the limit applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Reading {
    #[default]
    FullBand,
    #[serde(rename = "above_80hz")]
    Above80Hz,
}

impl Reading {
    fn key(self) -> &'static str {
        match self {
            Reading::FullBand => "full_band",
            Reading::Above80Hz => "above_80hz",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// What to call this spec in a report.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub format: Format,
    #[serde(default)]
    pub levels: Levels,
    #[serde(default)]
    pub structure: Structure,
    #[serde(default)]
    pub defects: Defects,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Format {
    pub sample_rate_hz: Option<u32>,
    pub channels: Option<usize>,
    pub bits_per_sample: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Levels {
    /// The band a silent stretch has to sit in.
    pub silence_max_dbfs: Option<f32>,
    pub silence_min_dbfs: Option<f32>,
    /// Which measurement of it the two above apply to.
    #[serde(default)]
    pub reading: Reading,
    pub true_peak_max_dbtp: Option<f32>,
    pub integrated_lufs: Option<f32>,
    pub integrated_tolerance_lu: Option<f32>,
    /// Above this, the floor under the speech is audibly not the floor between
    /// the words. Reported as a note rather than a failure.
    pub boundary_step_max_db: Option<f32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Structure {
    pub lead_min_ms: Option<f64>,
    pub lead_max_ms: Option<f64>,
    pub tail_min_ms: Option<f64>,
    pub tail_max_ms: Option<f64>,
    pub pause_max_ms: Option<f64>,
    /// The longer limit that applies when a breath sits inside the pause.
    pub pause_max_with_event_ms: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Defects {
    /// Runs of digital silence longer than this fail. Source material often
    /// carries legitimate padding, so a spec for finished files and a spec for
    /// source files will not agree here.
    pub max_zero_run_ms: Option<f64>,
    pub click_db: Option<f32>,
    pub max_clicks_outside_speech: Option<usize>,
    pub max_seams: Option<usize>,
    pub allow_clipping: Option<bool>,
    pub max_dc_offset: Option<f32>,
    pub allow_truncation: Option<bool>,
}

/// How bad a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Fail,
    Warn,
    Note,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Fail => "fail",
            Severity::Warn => "warn",
            Severity::Note => "note",
        }
    }
}

/// One rule, and what the file did about it.
#[derive(Debug, Clone)]
pub struct Finding {
    pub rule: &'static str,
    pub severity: Severity,
    pub measured: Value,
    pub limit: Value,
    /// Where in the file, when the rule is about somewhere in particular.
    pub at_secs: Vec<f64>,
    pub detail: Option<String>,
}

impl Finding {
    fn new(rule: &'static str, severity: Severity, measured: Value, limit: Value) -> Self {
        Self {
            rule,
            severity,
            measured,
            limit,
            at_secs: Vec::new(),
            detail: None,
        }
    }

    fn at(mut self, secs: Vec<f64>) -> Self {
        self.at_secs = secs;
        self
    }

    fn saying(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn json(&self) -> Value {
        json!({
            "rule": self.rule,
            "severity": self.severity.as_str(),
            "measured": self.measured,
            "limit": self.limit,
            "at_secs": self.at_secs,
            "detail": self.detail,
        })
    }
}

impl Spec {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Check a report against this spec.
    ///
    /// The report is whatever `analyze` produced. Rules whose measurements are
    /// missing — because the pass that produces them was not run — are skipped
    /// rather than failed, so a spec and a set of flags that do not line up
    /// under-reports instead of inventing defects.
    pub fn check(&self, report: &Value) -> Vec<Finding> {
        let mut out = Vec::new();
        self.check_format(report, &mut out);
        self.check_levels(report, &mut out);
        self.check_structure(report, &mut out);
        self.check_defects(report, &mut out);
        // Worst first: a reader who stops after the first line should have read
        // the most important one.
        out.sort_by_key(|f| match f.severity {
            Severity::Fail => 0,
            Severity::Warn => 1,
            Severity::Note => 2,
        });
        out
    }

    fn check_format(&self, r: &Value, out: &mut Vec<Finding>) {
        let f = &r["file"];
        if let (Some(want), Some(got)) = (self.format.sample_rate_hz, f["sample_rate_hz"].as_u64())
            && got != want as u64
        {
            out.push(Finding::new(
                "sample_rate",
                Severity::Fail,
                json!(got),
                json!(want),
            ));
        }
        if let (Some(want), Some(got)) = (self.format.channels, f["channels"].as_u64())
            && got != want as u64
        {
            out.push(Finding::new(
                "channels",
                Severity::Fail,
                json!(got),
                json!(want),
            ));
        }
        if let (Some(want), Some(got)) =
            (self.format.bits_per_sample, f["bits_per_sample"].as_u64())
            && got != want as u64
        {
            out.push(Finding::new(
                "bit_depth",
                Severity::Fail,
                json!(got),
                json!(want),
            ));
        }
    }

    fn check_levels(&self, r: &Value, out: &mut Vec<Finding>) {
        let l = &self.levels;
        if let (Some(limit), Some(tp)) = (
            l.true_peak_max_dbtp,
            r["channels"].as_array().and_then(|c| {
                c.iter()
                    .filter_map(|c| c["true_peak_dbtp"].as_f64())
                    .reduce(f64::max)
            }),
        ) && tp > limit as f64
        {
            out.push(Finding::new(
                "true_peak",
                Severity::Fail,
                json!(tp),
                json!(limit),
            ));
        }
        if let (Some(target), Some(got)) =
            (l.integrated_lufs, r["loudness"]["integrated_lufs"].as_f64())
        {
            let tol = l.integrated_tolerance_lu.unwrap_or(1.0) as f64;
            if (got - target as f64).abs() > tol {
                out.push(
                    Finding::new(
                        "integrated_loudness",
                        Severity::Fail,
                        json!(got),
                        json!(target),
                    )
                    .saying(format!("tolerance ±{tol} LU")),
                );
            }
        }

        // Silence level, over the silent stretches the segmentation found.
        let key = l.reading.key();
        let field = match l.reading {
            Reading::FullBand => "rms_dbfs",
            Reading::Above80Hz => "rms_above_80hz_dbfs",
        };
        if l.silence_max_dbfs.is_some() || l.silence_min_dbfs.is_some() {
            let silences: Vec<(f64, f64)> = r["segments"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|s| s["kind"] == "silence")
                        .filter_map(|s| Some((s["start_secs"].as_f64()?, s[field].as_f64()?)))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(max) = l.silence_max_dbfs {
                let over: Vec<(f64, f64)> = silences
                    .iter()
                    .copied()
                    .filter(|(_, db)| *db > max as f64)
                    .collect();
                if !over.is_empty() {
                    let worst = over.iter().map(|(_, d)| *d).fold(f64::MIN, f64::max);
                    out.push(
                        Finding::new("silence_too_loud", Severity::Fail, json!(worst), json!(max))
                            .at(over.iter().map(|(t, _)| *t).collect())
                            .saying(format!("{key} reading, {} of them", over.len())),
                    );
                }
            }
            if let Some(min) = l.silence_min_dbfs {
                let under: Vec<(f64, f64)> = silences
                    .iter()
                    .copied()
                    .filter(|(_, db)| db.is_finite() && *db < min as f64)
                    .collect();
                if !under.is_empty() {
                    let worst = under.iter().map(|(_, d)| *d).fold(f64::MAX, f64::min);
                    out.push(
                        // Below the band is not a defect in the audio, it is a
                        // question about which tier the job is being held to.
                        Finding::new(
                            "silence_below_band",
                            Severity::Warn,
                            json!(worst),
                            json!(min),
                        )
                        .at(under.iter().map(|(t, _)| *t).collect())
                        .saying(format!("{key} reading, {} of them", under.len())),
                    );
                }
            }
        }

        if let (Some(limit), Some(step)) = (
            l.boundary_step_max_db,
            r["structure"]["boundary_step_db"].as_f64(),
        ) && step > limit as f64
        {
            out.push(
                Finding::new("boundary_step", Severity::Note, json!(step), json!(limit)).saying(
                    "the floor under the speech is louder than the silence beside it; \
                     a property of the recording, not an edit",
                ),
            );
        }
    }

    fn check_structure(&self, r: &Value, out: &mut Vec<Finding>) {
        let s = &self.structure;
        let st = &r["structure"];
        let mut edge = |rule: &'static str, key: &str, min: Option<f64>, max: Option<f64>| {
            let Some(secs) = st[key].as_f64() else { return };
            let ms = secs * 1000.0;
            let out_of = match (min, max) {
                (Some(a), Some(b)) => ms < a || ms > b,
                (Some(a), None) => ms < a,
                (None, Some(b)) => ms > b,
                (None, None) => return,
            };
            if out_of {
                out.push(
                    Finding::new(rule, Severity::Fail, json!(ms), json!([min, max]))
                        .saying("milliseconds"),
                );
            }
        };
        edge(
            "leading_silence",
            "leading_silence_secs",
            s.lead_min_ms,
            s.lead_max_ms,
        );
        edge(
            "trailing_silence",
            "trailing_silence_secs",
            s.tail_min_ms,
            s.tail_max_ms,
        );

        if s.pause_max_ms.is_some() || s.pause_max_with_event_ms.is_some() {
            let plain = s.pause_max_ms.unwrap_or(f64::MAX);
            let with_event = s.pause_max_with_event_ms.unwrap_or(plain);
            let over: Vec<(f64, f64, f64)> = st["pauses"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|p| {
                            let ms = p["duration_secs"].as_f64()? * 1000.0;
                            let limit = if p["event_inside"].as_bool().unwrap_or(false) {
                                with_event
                            } else {
                                plain
                            };
                            (ms > limit).then_some((p["start_secs"].as_f64()?, ms, limit))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !over.is_empty() {
                let worst = over.iter().map(|(_, ms, _)| *ms).fold(f64::MIN, f64::max);
                let limit = over.iter().map(|(_, _, l)| *l).fold(f64::MAX, f64::min);
                out.push(
                    Finding::new("pause_too_long", Severity::Fail, json!(worst), json!(limit))
                        .at(over.iter().map(|(t, _, _)| *t).collect())
                        .saying(format!("milliseconds, {} over the limit", over.len())),
                );
            }
        }
    }

    fn check_defects(&self, r: &Value, out: &mut Vec<Finding>) {
        let d = &self.defects;
        let found = &r["defects"];

        if let Some(max_ms) = d.max_zero_run_ms
            && let Some(runs) = found["digital_silence"]["runs"].as_array()
        {
            let over: Vec<&Value> = runs
                .iter()
                .filter(|z| z["duration_secs"].as_f64().unwrap_or(0.0) * 1000.0 > max_ms)
                .collect();
            if !over.is_empty() {
                let worst = over
                    .iter()
                    .filter_map(|z| z["duration_secs"].as_f64())
                    .fold(0.0, f64::max)
                    * 1000.0;
                out.push(
                    Finding::new(
                        "digital_silence",
                        Severity::Fail,
                        json!(worst),
                        json!(max_ms),
                    )
                    .at(over
                        .iter()
                        .filter_map(|z| z["start_secs"].as_f64())
                        .collect())
                    .saying(format!("milliseconds, {} runs over", over.len())),
                );
            }
        }

        if let Some(max) = d.max_clicks_outside_speech
            && let Some(clicks) = found["clicks"].as_array()
        {
            let outside: Vec<&Value> = clicks
                .iter()
                .filter(|c| !c["in_speech"].as_bool().unwrap_or(false))
                .collect();
            if outside.len() > max {
                out.push(
                    Finding::new("clicks", Severity::Fail, json!(outside.len()), json!(max))
                        .at(outside.iter().filter_map(|c| c["secs"].as_f64()).collect())
                        .saying("outside speech"),
                );
            }
        }

        if let Some(max) = d.max_seams
            && let Some(seams) = found["seams"].as_array()
            && seams.len() > max
        {
            out.push(
                Finding::new("seams", Severity::Fail, json!(seams.len()), json!(max))
                    .at(seams.iter().filter_map(|s| s["secs"].as_f64()).collect())
                    .saying("steps in the noise floor outside speech"),
            );
        }

        if d.allow_clipping == Some(false)
            && let Some(channels) = r["channels"].as_array()
        {
            let clipped: u64 = channels
                .iter()
                .filter_map(|c| c["clipped_samples"].as_u64())
                .sum();
            if clipped > 0 {
                out.push(Finding::new(
                    "clipping",
                    Severity::Fail,
                    json!(clipped),
                    json!(0),
                ));
            }
        }

        if let (Some(limit), Some(channels)) = (d.max_dc_offset, r["channels"].as_array()) {
            let worst = channels
                .iter()
                .filter_map(|c| c["dc_offset"].as_f64())
                .map(f64::abs)
                .fold(0.0, f64::max);
            if worst > limit as f64 {
                out.push(Finding::new(
                    "dc_offset",
                    Severity::Fail,
                    json!(worst),
                    json!(limit),
                ));
            }
        }

        if d.allow_truncation == Some(false) {
            let t = &found["truncation"];
            let head = t["head"].as_bool().unwrap_or(false);
            let tail = t["tail"].as_bool().unwrap_or(false);
            if head || tail {
                out.push(
                    Finding::new(
                        "truncated",
                        Severity::Fail,
                        json!([head, tail]),
                        json!(false),
                    )
                    .saying("signal is still present at the very first or last sample"),
                );
            }
        }
    }
}

/// The scalar measurements worth a column in a table, as `(header, path)`.
///
/// A fixed list rather than a flattening of whatever the report happens to
/// hold: a table whose columns change between files is not a table. Paths are
/// dotted, and `[]` takes the first element of an array.
pub const COLUMNS: &[(&str, &str)] = &[
    ("file", "file.name"),
    ("path", "file.path"),
    ("duration_secs", "file.duration_secs"),
    ("sample_rate_hz", "file.sample_rate_hz"),
    ("channels", "file.channels"),
    ("bits_per_sample", "file.bits_per_sample"),
    ("integrated_lufs", "loudness.integrated_lufs"),
    ("range_lu", "loudness.range_lu"),
    ("true_peak_dbtp", "channels[].true_peak_dbtp"),
    ("sample_peak_dbfs", "channels[].sample_peak_dbfs"),
    ("rms_dbfs", "channels[].rms_dbfs"),
    ("l10_dbfs", "channels[].l10_dbfs"),
    ("l50_dbfs", "channels[].l50_dbfs"),
    ("l90_dbfs", "channels[].l90_dbfs"),
    ("noise_floor_dbfs", "channels[].noise_floor_dbfs"),
    ("dc_offset", "channels[].dc_offset"),
    ("clipped_samples", "channels[].clipped_samples"),
    ("leading_silence_secs", "structure.leading_silence_secs"),
    ("trailing_silence_secs", "structure.trailing_silence_secs"),
    ("leading_nonzero_secs", "structure.leading_nonzero_secs"),
    ("trailing_nonzero_secs", "structure.trailing_nonzero_secs"),
    ("longest_pause_secs", "structure.longest_pause_secs"),
    ("speech_ratio", "structure.speech_ratio"),
    ("boundary_step_db", "structure.boundary_step_db"),
    ("digital_silence_runs", "defects.digital_silence.count"),
    ("digital_silence_secs", "defects.digital_silence.total_secs"),
    ("truncated_head", "defects.truncation.head"),
    ("truncated_tail", "defects.truncation.tail"),
    ("cutoff_hz", "spectral.cutoff_hz"),
    ("transcode_suspect", "spectral.transcode_suspect"),
];

/// Follow a dotted path into a report.
pub fn dig<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut at = value;
    for part in path.split('.') {
        at = match part.strip_suffix("[]") {
            Some(key) => at.get(key)?.as_array()?.first()?,
            None => at.get(part)?,
        };
    }
    (!at.is_null()).then_some(at)
}

/// A value as a CSV cell: numbers plain, strings unquoted unless they have to
/// be, and nothing at all for a measurement that was not taken.
pub fn cell(value: Option<&Value>) -> String {
    let Some(v) = value else {
        return String::new();
    };
    let text = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if text.contains([',', '"', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

/// Counts of each severity, for a run's summary line.
pub fn tally(findings: &[Finding]) -> BTreeMap<&'static str, usize> {
    let mut out = BTreeMap::new();
    for f in findings {
        *out.entry(f.severity.as_str()).or_insert(0) += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(text: &str) -> Spec {
        toml::from_str(text).expect("spec parses")
    }

    #[test]
    fn a_spec_checks_only_what_it_mentions() {
        let s = spec("[format]\nsample_rate_hz = 48000\n");
        let report = json!({ "file": { "sample_rate_hz": 44100, "channels": 2 } });
        let found = s.check(&report);
        assert_eq!(
            found.len(),
            1,
            "channels were not mentioned, so not checked"
        );
        assert_eq!(found[0].rule, "sample_rate");
        assert_eq!(found[0].severity, Severity::Fail);
    }

    /// A measurement that was never taken is not a failure. A spec asking about
    /// pauses and a run that never segmented should under-report, not invent.
    #[test]
    fn missing_measurements_are_skipped_not_failed() {
        let s = spec("[structure]\nlead_min_ms = 180\nlead_max_ms = 300\npause_max_ms = 700\n");
        let report = json!({ "file": {} });
        assert!(s.check(&report).is_empty());
    }

    #[test]
    fn lead_and_tail_are_checked_against_a_range() {
        let s = spec("[structure]\nlead_min_ms = 180\nlead_max_ms = 300\n");
        let inside = json!({ "file": {}, "structure": { "leading_silence_secs": 0.2 } });
        assert!(s.check(&inside).is_empty());

        let short = json!({ "file": {}, "structure": { "leading_silence_secs": 0.1 } });
        let found = s.check(&short);
        assert_eq!(found[0].rule, "leading_silence");
        assert_eq!(found[0].measured, json!(100.0));
    }

    /// The longer limit applies only to a pause with something in it.
    #[test]
    fn a_breath_buys_a_longer_pause() {
        let s = spec("[structure]\npause_max_ms = 700\npause_max_with_event_ms = 1000\n");
        let report = |event: bool| {
            json!({ "file": {}, "structure": { "pauses": [
                { "start_secs": 1.0, "duration_secs": 0.85, "event_inside": event }
            ]}})
        };
        assert_eq!(
            s.check(&report(false)).len(),
            1,
            "850 ms with nothing in it is over"
        );
        assert!(
            s.check(&report(true)).is_empty(),
            "with a breath it is allowed"
        );
    }

    /// Which reading the limit applies to is the spec's choice, and the two
    /// can disagree by several decibels on the same file.
    #[test]
    fn the_silence_reading_is_the_specs_to_choose() {
        let report = json!({ "file": {}, "segments": [
            { "kind": "silence", "start_secs": 0.0, "rms_dbfs": -58.0, "rms_above_80hz_dbfs": -64.0 }
        ]});
        let full = spec("[levels]\nsilence_max_dbfs = -60.0\nreading = \"full_band\"\n");
        assert_eq!(full.check(&report).len(), 1, "-58 full band is over -60");
        let hp = spec("[levels]\nsilence_max_dbfs = -60.0\nreading = \"above_80hz\"\n");
        assert!(hp.check(&report).is_empty(), "-64 above 80 Hz is under it");
    }

    /// The expected-but-worth-knowing kind, which must not fail a file.
    #[test]
    fn a_boundary_step_is_a_note_not_a_failure() {
        let s = spec("[levels]\nboundary_step_max_db = 6.0\n");
        let report = json!({ "file": {}, "structure": { "boundary_step_db": 11.2 } });
        let found = s.check(&report);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].severity, Severity::Note);
    }

    #[test]
    fn findings_come_worst_first() {
        let s = spec("[levels]\nboundary_step_max_db = 6.0\n[format]\nsample_rate_hz = 48000\n");
        let report = json!({
            "file": { "sample_rate_hz": 44100 },
            "structure": { "boundary_step_db": 11.2 }
        });
        let found = s.check(&report);
        assert_eq!(found[0].severity, Severity::Fail);
        assert_eq!(found[1].severity, Severity::Note);
    }

    #[test]
    fn a_typo_in_a_spec_is_an_error_not_a_silent_default() {
        assert!(toml::from_str::<Spec>("[levels]\nsilence_max_dbfs_typo = -60.0\n").is_err());
    }

    #[test]
    fn dotted_paths_reach_into_the_report() {
        let r = json!({ "file": { "name": "a.wav" }, "channels": [{ "rms_dbfs": -20.0 }] });
        assert_eq!(dig(&r, "file.name").unwrap(), &json!("a.wav"));
        assert_eq!(dig(&r, "channels[].rms_dbfs").unwrap(), &json!(-20.0));
        assert!(dig(&r, "file.missing").is_none());
        assert!(dig(&r, "nothing.at.all").is_none());
    }

    #[test]
    fn cells_are_escaped_only_when_they_have_to_be() {
        assert_eq!(cell(Some(&json!(-23.4))), "-23.4");
        assert_eq!(cell(Some(&json!("plain"))), "plain");
        assert_eq!(cell(Some(&json!("a,b"))), "\"a,b\"");
        assert_eq!(cell(None), "");
    }
}
