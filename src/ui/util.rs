use std::path::Path;
use std::process::Command;

/// `m:ss.mmm`, or `h:mm:ss.mmm` past an hour.
pub fn fmt_time(secs: f64) -> String {
    let secs = secs.max(0.0);
    let h = (secs / 3600.0).floor() as u64;
    let m = ((secs % 3600.0) / 60.0).floor() as u64;
    let s = secs % 60.0;
    if h > 0 {
        format!("{h}:{m:02}:{s:06.3}")
    } else {
        format!("{m}:{s:06.3}")
    }
}

/// Short form for rulers: drops millis when the step is coarse.
pub fn fmt_time_step(secs: f64, step: f64) -> String {
    let secs = secs.max(0.0);
    let h = (secs / 3600.0).floor() as u64;
    let m = ((secs % 3600.0) / 60.0).floor() as u64;
    let s = secs % 60.0;
    let body = if step >= 1.0 {
        format!("{m}:{:02}", s.round() as u64)
    } else if step >= 0.01 {
        format!("{m}:{s:05.2}")
    } else {
        format!("{m}:{s:06.3}")
    };
    if h > 0 { format!("{h}:{body}") } else { body }
}

/// A "nice" tick step in seconds for a ruler covering `visible_secs` in
/// `width_px`, aiming for at least `min_px` between ticks.
pub fn nice_time_step(visible_secs: f64, width_px: f32, min_px: f32) -> f64 {
    const STEPS: [f64; 22] = [
        0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0,
        60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0,
    ];
    let min_secs = visible_secs * (min_px / width_px.max(1.0)) as f64;
    STEPS
        .iter()
        .copied()
        .find(|s| *s >= min_secs)
        .unwrap_or(3600.0)
}

pub fn fmt_hz(hz: f32) -> String {
    if hz >= 1000.0 {
        let k = hz / 1000.0;
        if k.fract().abs() < 0.05 {
            format!("{k:.0}k")
        } else {
            format!("{k:.1}k")
        }
    } else {
        format!("{hz:.0}")
    }
}

/// `fmt_time` right-aligned in a field as wide as the longest time the clip
/// can show, so a readout that updates every frame keeps its neighbours put
/// instead of shuffling them when the minute rolls over to two digits.
pub fn fmt_time_field(secs: f64, total_secs: f64) -> String {
    let w = fmt_time(total_secs.max(secs)).len();
    format!("{:>w$}", fmt_time(secs))
}

// ---- Showing a file to the desktop -----------------------------------------

/// Open the desktop's file manager on `path`, with the file itself selected
/// where the manager can do that.
///
/// Spawned on a thread of its own and forgotten: the D-Bus call below waits for
/// a reply, and the UI thread has a frame to draw. A failure is logged rather
/// than surfaced — there is nothing useful to tell someone beyond "this desktop
/// has no file manager", and the path is on screen to copy either way.
pub fn reveal(path: &Path) {
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        if let Err(e) = show_in_file_manager(&path) {
            log::warn!("could not show {} in a file manager: {e}", path.display());
        }
    });
}

#[cfg(windows)]
fn show_in_file_manager(path: &Path) -> std::io::Result<()> {
    // Explorer exits non-zero even when it did exactly what was asked, so the
    // status is not worth reading.
    Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn()?;
    Ok(())
}

#[cfg(not(windows))]
fn show_in_file_manager(path: &Path) -> std::io::Result<()> {
    // The freedesktop interface every major file manager implements, and what
    // the portal answers inside a Flatpak sandbox: it opens the folder with the
    // file selected rather than just the folder.
    let shown = Command::new("dbus-send")
        .args([
            "--session",
            "--type=method_call",
            "--dest=org.freedesktop.FileManager1",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
        ])
        .arg(format!("array:string:{}", file_uri(path)))
        .arg("string:")
        .status()
        .is_ok_and(|s| s.success());
    if shown {
        return Ok(());
    }
    // No such service, or no dbus-send: settle for the containing folder,
    // which every desktop opens.
    let dir = path.parent().unwrap_or(path);
    let status = Command::new("xdg-open").arg(dir).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("xdg-open: {status}")))
    }
}

/// A `file://` URI for a local path, percent-encoding everything outside the
/// unreserved set. Spaces and accents in a path are the common case, and
/// D-Bus wants a URI, not a path.
#[cfg(not(windows))]
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        match b {
            b'/' | b'-' | b'_' | b'.' | b'~' => uri.push(b as char),
            b if b.is_ascii_alphanumeric() => uri.push(b as char),
            b => uri.push_str(&format!("%{b:02X}")),
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn file_uri_encodes_what_a_path_can_hold() {
        assert_eq!(file_uri(Path::new("/tmp/a.wav")), "file:///tmp/a.wav");
        // Spaces, and anything non-ASCII, byte by byte in UTF-8.
        assert_eq!(
            file_uri(Path::new("/tmp/take 1/caffè.wav")),
            "file:///tmp/take%201/caff%C3%A8.wav"
        );
    }
}
