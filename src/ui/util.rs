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
    // the desktop portal answers too: it opens the folder with the file
    // selected rather than just the folder.
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

/// A file's name for a one-line row, falling back to the whole path for the
/// odd path that has no final component.
pub fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

// ---- readouts --------------------------------------------------------------

pub fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// UTC date and time without pulling in a calendar crate.
pub fn fmt_system_time(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60
    )
}

/// Symphonia's long codec names, trimmed to fit a sidebar row.
pub fn short_codec(name: &str) -> String {
    name.replace("Little-Endian", "LE")
        .replace("Big-Endian", "BE")
        .replace(" Interleaved", "")
        .replace(" Planar", "")
        .replace("Signed ", "s")
        .replace("Unsigned ", "u")
        .replace("Floating Point ", "f")
        .replace("-bit", "")
}

pub fn db_str(v: f32) -> String {
    if v.is_finite() {
        format!("{v:+.1} dB")
    } else {
        "—".into()
    }
}

pub fn lufs_str(v: f32) -> String {
    if v.is_finite() {
        format!("{v:+.1} LUFS")
    } else {
        "—".into()
    }
}
/// Readouts for the STFT resolution line: enough digits to tell two settings
/// apart, no more.
pub fn fmt_hz_unit(hz: f64) -> String {
    if hz >= 100.0 {
        format!("{hz:.0} Hz")
    } else {
        format!("{hz:.1} Hz")
    }
}

pub fn fmt_ms(ms: f64) -> String {
    if ms >= 100.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{ms:.1} ms")
    }
}

/// A frequency at full precision, for a hover readout that has the room.
pub fn fmt_hz_full(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.2} kHz", hz / 1000.0)
    } else {
        format!("{hz:.1} Hz")
    }
}
#[cfg(test)]
mod tests {
    // The test below is about Unix paths, so on Windows it is compiled out
    // and this import would be unused, which `-D warnings` rejects. A test
    // that is not gated will fail to build here until the cfg comes off.
    #[cfg(not(windows))]
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
