//! A small command line, parsed by hand.
//!
//! The options here are all `--name value`, `--name=value` or a bare
//! `--switch`, which is little enough grammar that a dependency to parse it
//! would weigh more than the code does. What a parser this size must not do is
//! guess: an unknown option, a missing value or a value that will not parse is
//! an error naming itself, never a silent default.

use std::collections::HashMap;

use anyhow::{Result, bail};

/// Options that take a value. Everything else spelled `--like-this` is a
/// switch, and this is the table that tells the two apart.
const VALUED: &[&str] = &[
    "output",
    "start",
    "end",
    "width",
    "height",
    "wave-height",
    "channel",
    "window",
    "overlap",
    "window-fn",
    "db",
    "min-hz",
    "max-hz",
    "colormap",
    "contrast",
    "wave-scale",
    "wave-zoom",
    "wave-db",
    "wave-color",
    "merge-opacity",
    "merge-spec-opacity",
    "spectrum-height",
    "zero-run-ms",
    "click-db",
    "speech-db",
    "min-speech-ms",
    "min-gap-ms",
    "spectral-window",
    "spec",
    "jobs",
    "render-failures",
    "json",
];

/// Switches, spelled out so a typo in one is caught rather than ignored.
const SWITCHES: &[&str] = &[
    "timeline",
    "structure",
    "segments",
    "defects",
    "spectral",
    "all",
    "csv",
    "jsonl",
    "fail-only",
    "merge",
    "no-merge",
    "spectrum",
    "no-spectrogram",
    "linear",
    "log",
    "reassign",
    "no-axes",
    "no-waveform",
    "compact",
    "quiet",
    "help",
    "version",
];

#[derive(Debug, Default)]
pub struct Args {
    pub positional: Vec<String>,
    values: HashMap<String, String>,
    switches: Vec<String>,
}

impl Args {
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self> {
        let mut out = Args::default();
        let mut it = args.into_iter().peekable();
        while let Some(arg) = it.next() {
            // A bare `--` hands the rest over as positional, which is the only
            // way to name a file that begins with a dash.
            if arg == "--" {
                out.positional.extend(it);
                break;
            }
            let Some(rest) = arg.strip_prefix("--").or_else(|| short(&arg)) else {
                out.positional.push(arg);
                continue;
            };
            let (name, inline) = match rest.split_once('=') {
                Some((n, v)) => (n.to_owned(), Some(v.to_owned())),
                None => (rest.to_owned(), None),
            };
            if VALUED.contains(&name.as_str()) {
                let value = match inline {
                    Some(v) => v,
                    // The next word, unless it is plainly another option:
                    // `--start --quiet` means a forgotten value, and saying so
                    // beats complaining that "--quiet" is not a number. A
                    // negative number is not another option, so `--db -90:0`
                    // still reads as one.
                    None => match it.next_if(|v| !is_option(v)) {
                        Some(v) => v,
                        None => bail!("--{name} needs a value"),
                    },
                };
                out.values.insert(name, value);
            } else if SWITCHES.contains(&name.as_str()) {
                if inline.is_some() {
                    bail!("--{name} takes no value");
                }
                out.switches.push(name);
            } else {
                bail!("unknown option --{name}");
            }
        }
        Ok(out)
    }

    pub fn has(&self, name: &str) -> bool {
        self.switches.iter().any(|s| s == name)
    }

    pub fn str(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// A value parsed into whatever the caller wants, or an error that says
    /// which option would not parse and what was in it.
    pub fn get<T>(&self, name: &str) -> Result<Option<T>>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        match self.values.get(name) {
            None => Ok(None),
            Some(v) => match v.parse::<T>() {
                Ok(p) => Ok(Some(p)),
                Err(e) => bail!("--{name}: {e} in {v:?}"),
            },
        }
    }
}

/// Whether a word is one of this program's options rather than a value. Only
/// names it knows count, so a file or a value that happens to start with
/// dashes is still taken as one.
fn is_option(word: &str) -> bool {
    if word == "--" {
        return true;
    }
    let Some(rest) = word.strip_prefix("--").or_else(|| short(word)) else {
        return false;
    };
    let name = rest.split_once('=').map_or(rest, |(n, _)| n);
    VALUED.contains(&name) || SWITCHES.contains(&name)
}

/// The two short options worth having: `-o` for output and `-h` for help.
fn short(arg: &str) -> Option<&str> {
    match arg {
        "-o" => Some("output"),
        "-h" => Some("help"),
        "-V" => Some("version"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args> {
        Args::parse(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn values_come_either_way_round() {
        let a = parse(&["render", "--start", "1.5", "--end=2", "f.wav"]).unwrap();
        assert_eq!(a.positional, vec!["render", "f.wav"]);
        assert_eq!(a.get::<f64>("start").unwrap(), Some(1.5));
        assert_eq!(a.get::<f64>("end").unwrap(), Some(2.0));
        assert_eq!(a.get::<f64>("min-hz").unwrap(), None);
    }

    #[test]
    fn switches_are_switches_and_shorts_are_expanded() {
        let a = parse(&["render", "-o", "out.png", "--reassign", "--linear"]).unwrap();
        assert_eq!(a.str("output"), Some("out.png"));
        assert!(a.has("reassign") && a.has("linear"));
        assert!(!a.has("log"));
    }

    /// Everything a parser this size can get wrong, it says out loud.
    #[test]
    fn mistakes_are_named() {
        let err = |a: &[&str]| parse(a).unwrap_err().to_string();
        assert!(err(&["--nonsense"]).contains("unknown option --nonsense"));
        assert!(err(&["--start"]).contains("--start needs a value"));
        // A forgotten value must not swallow the option after it.
        assert!(err(&["--start", "--quiet"]).contains("--start needs a value"));
        assert!(err(&["--output", "-o"]).contains("--output needs a value"));
        assert!(err(&["--reassign=yes"]).contains("takes no value"));
        let bad = parse(&["--width", "wide"]).unwrap();
        assert!(
            bad.get::<usize>("width")
                .unwrap_err()
                .to_string()
                .contains("\"wide\"")
        );
    }

    /// A negative number is a value, not an option: nothing this program
    /// accepts is spelled with one dash and a digit.
    #[test]
    fn negative_values_are_values() {
        let a = parse(&["render", "--db", "-90:0", "--start", "-1.5"]).unwrap();
        assert_eq!(a.str("db"), Some("-90:0"));
        assert_eq!(a.get::<f64>("start").unwrap(), Some(-1.5));
    }

    /// A file whose name starts with a dash is still a file.
    #[test]
    fn a_double_dash_ends_the_options() {
        let a = parse(&["analyze", "--", "--odd-name.wav"]).unwrap();
        assert_eq!(a.positional, vec!["analyze", "--odd-name.wav"]);
    }
}
