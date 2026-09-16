//! Update check: is there a newer release on GitHub?
//!
//! One GET to the releases API on a background thread, at most once a day,
//! and only in builds with the `update-check` feature. Flatpak and
//! distribution packages leave it out: their package manager is the update
//! channel, and Flathub does not allow apps to check on their own. Nothing is
//! downloaded here either way; the notice links to the release page.

use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Whether this build checks at all.
pub const ENABLED: bool = cfg!(feature = "update-check");

pub const REPO_URL: &str = "https://github.com/frdcmp/auriscope";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// `git describe` at build time, or empty outside a checkout. See `build.rs`.
pub const GIT_DESCRIBE: &str = env!("AURISCOPE_GIT_DESCRIBE");

/// True when this was built from a working tree that is not exactly the tag
/// matching `CURRENT`: a development build, ahead of or dirty against it.
pub fn is_dev_build() -> bool {
    !GIT_DESCRIBE.is_empty() && GIT_DESCRIBE != format!("v{CURRENT}")
}

/// What `--version` prints. The first whitespace-separated field is always the
/// plain version, because the installers parse it to decide about upgrades.
pub fn version_line() -> String {
    if is_dev_build() {
        format!("{CURRENT} (dev {GIT_DESCRIBE})")
    } else {
        CURRENT.to_string()
    }
}

/// Minimum gap between two automatic checks.
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// Bare version, `0.2.0`, without the tag's `v`.
    pub version: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Never checked in this run.
    Idle,
    Checking,
    UpToDate,
    Newer(Release),
    /// Offline, rate-limited, unparsable: shown only in the About card.
    Failed(String),
}

pub struct Updater {
    rx: Option<mpsc::Receiver<Result<Option<Release>, String>>>,
    pub status: Status,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            rx: None,
            status: Status::Idle,
        }
    }
}

impl Updater {
    /// Whether an automatic check is due given the persisted time of the
    /// last one, in seconds since the Unix epoch.
    pub fn due(last_check: u64) -> bool {
        // A clock set back into the past counts as due; better one extra
        // request than never checking again.
        unix_now().saturating_sub(last_check) >= INTERVAL.as_secs() || last_check > unix_now()
    }

    pub fn start(&mut self) {
        if !ENABLED || self.rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.status = Status::Checking;
        std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                tx.send(fetch_latest()).ok();
            })
            .ok();
    }

    /// Picks up the thread's answer. Returns true once, when a check has
    /// just finished, so the caller can record the time.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else {
            return false;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Err("check thread died".into()),
        };
        self.rx = None;
        self.status = match result {
            Ok(Some(r)) => Status::Newer(r),
            Ok(None) => Status::UpToDate,
            Err(e) => {
                log::warn!("update check: {e}");
                Status::Failed(e)
            }
        };
        true
    }

    /// The release to announce, unless the user skipped that version.
    pub fn notice(&self, skipped: Option<&str>) -> Option<&Release> {
        match &self.status {
            Status::Newer(r) if skipped != Some(r.version.as_str()) => Some(r),
            _ => None,
        }
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(feature = "update-check")]
fn fetch_latest() -> Result<Option<Release>, String> {
    const API: &str = "https://api.github.com/repos/frdcmp/auriscope/releases/latest";
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent(concat!("auriscope/", env!("CARGO_PKG_VERSION")))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut resp = agent
        .get(API)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let tag = json_string(&body, "tag_name").ok_or("no tag_name in response")?;
    Ok(newer_than_current(&tag))
}

#[cfg(not(feature = "update-check"))]
fn fetch_latest() -> Result<Option<Release>, String> {
    Err("update check not built in".into())
}

/// The release for `tag` if it is newer than the running version.
#[cfg(feature = "update-check")]
fn newer_than_current(tag: &str) -> Option<Release> {
    let version = tag.trim().trim_start_matches('v');
    let latest = semver::Version::parse(version).ok()?;
    let current = semver::Version::parse(CURRENT).ok()?;
    (latest > current).then(|| Release {
        version: latest.to_string(),
        url: format!("{REPO_URL}/releases/tag/{}", tag.trim()),
    })
}

/// The string value of the first `"key": "..."` in a JSON document. Enough
/// for one field of a fixed API response, without a JSON dependency; tag
/// names contain no escapes.
#[cfg(feature = "update-check")]
fn json_string(body: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let after_key = &body[body.find(&needle)? + needle.len()..];
    let after_colon = after_key.trim_start().strip_prefix(':')?.trim_start();
    let inner = after_colon.strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
}

#[cfg(all(test, feature = "update-check"))]
mod tests {
    use super::*;

    #[test]
    fn finds_tag_name() {
        let body = r#"{"url":"https://x","tag_name": "v0.2.0","name":"v0.2.0"}"#;
        assert_eq!(json_string(body, "tag_name").as_deref(), Some("v0.2.0"));
        assert_eq!(json_string(body, "missing"), None);
    }

    #[test]
    fn compares_against_current() {
        assert!(newer_than_current("v99.0.0").is_some());
        assert!(newer_than_current(&format!("v{CURRENT}")).is_none());
        assert!(newer_than_current("v0.0.1").is_none());
        assert!(newer_than_current("nightly").is_none());
        let r = newer_than_current("v99.1.2").unwrap();
        assert_eq!(r.version, "99.1.2");
        assert!(r.url.ends_with("/releases/tag/v99.1.2"));
    }
}
