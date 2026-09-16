//! Stamps the build with `git describe`, so a development build can say so.
//!
//! Outside a git checkout — a release tarball, or Flathub's vendored build —
//! this is simply empty and the app reports its plain Cargo version.

use std::path::Path;
use std::process::Command;

fn main() {
    let describe = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty", "--abbrev=7"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=AURISCOPE_GIT_DESCRIBE={describe}");

    // Rebuild when HEAD moves. Only when these exist: naming a missing path
    // makes cargo rerun the script on every single build.
    for p in [".git/HEAD", ".git/refs"] {
        if Path::new(p).exists() {
            println!("cargo:rerun-if-changed={p}");
        }
    }
}
