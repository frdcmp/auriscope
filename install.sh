#!/usr/bin/env bash
# Install or update Auriscope on Linux, for one user, no root.
#
#   curl -fsSL https://raw.githubusercontent.com/frdcmp/auriscope/main/install.sh | bash
#
# This downloads the latest GitHub release, checks its SHA-256 and installs it
# under ~/.local: the binary, a desktop entry, the icon and the AppStream
# metainfo. Run it again to update; it does nothing if you are already current.
#
# Options, after `bash -s --` when piping:
#
#   --source          build from source instead: the checkout this script sits
#                     in, or a clone of `main` in ~/.cache/auriscope-src
#   --git             bleeding edge: always fetch the newest `main` and build
#                     that, ignoring any checkout you are standing in
#   --version vX.Y.Z  a specific release instead of the latest
#   --force           reinstall even if that version is already installed
#   --skill           also install the Claude Code skill, even with no ~/.claude
#   --no-skill        never install it (the default is: only if ~/.claude exists)
#   --uninstall       remove everything this script installed
#
# On Windows, use install.ps1 instead:
#   irm https://raw.githubusercontent.com/frdcmp/auriscope/main/install.ps1 | iex
#
set -euo pipefail

REPO="frdcmp/auriscope"
APP_ID="io.github.frdcmp.Auriscope"
PREFIX="${AURISCOPE_PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin/auriscope"
# The headless analysis binary, installed beside the app. Older releases do
# not carry it, so everything below treats it as optional.
CLI_BIN="$PREFIX/bin/auriscope-cli"
SHARE="$PREFIX/share"
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/auriscope-src"
# The Claude Code skill: how a model is told what auriscope-cli can do. It
# lives outside $PREFIX because that is where Claude Code looks, so it is
# installed only when that directory already exists — finding it is the signal
# that the tool is wanted here — or when --skill asks outright.
SKILLS="${CLAUDE_SKILLS_DIR:-$HOME/.claude/skills}"
SKILL_DIR="$SKILLS/auriscope-cli"

MODE=release
VERSION=""
FORCE=0
# auto: install the skill if ~/.claude/skills is there. force: make it. no: never.
SKILL=auto
# --git: build the newest main rather than whatever checkout we are in.
FRESH=0
while [ $# -gt 0 ]; do
  case "$1" in
    --source) MODE=source ;;
    --git) MODE=source; FRESH=1 ;;
    --uninstall) MODE=uninstall ;;
    --version) VERSION="$2"; shift ;;
    --force) FORCE=1 ;;
    --skill) SKILL=force ;;
    --no-skill) SKILL=no ;;
    -h|--help) sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

say() { printf '\033[1m%s\033[0m\n' "$*"; }
# The version already installed, or empty.
#
# Builds before 0.1.1 have no --version flag: they take the argument as a file
# path and open the window instead of printing anything, so this must never
# wait for the process. `timeout` kills that case and the empty answer means
# "too old to ask, reinstall".
installed_version() {
  [ -x "$BIN" ] || return 0
  local probe="timeout -k 1 5"
  command -v timeout >/dev/null 2>&1 || probe=""
  # No display, no stdin, no window: belt and braces if timeout is missing.
  DISPLAY= WAYLAND_DISPLAY= $probe "$BIN" --version </dev/null 2>/dev/null | head -1 || true
}
# Just the number, for comparing against a tag.
bare_version() { installed_version | awk '{print $1}'; }
# A build from git rather than a release; the printed line carries a suffix.
is_dev_build() { case "$(installed_version)" in *\(dev*) return 0 ;; *) return 1 ;; esac; }
die() { echo "error: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required"; }

case "$(uname -s)" in
  Linux) ;;
  MINGW*|MSYS*|CYGWIN*)
    die "on Windows, run install.ps1 in PowerShell instead:
  irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex" ;;
  *) die "unsupported OS $(uname -s); Auriscope ships for Linux and Windows" ;;
esac

# ---- uninstall ------------------------------------------------------------

if [ "$MODE" = uninstall ]; then
  rm -f "$BIN" "$CLI_BIN" \
    "$SHARE/applications/$APP_ID.desktop" \
    "$SHARE/metainfo/$APP_ID.metainfo.xml" \
    "$SHARE/icons/hicolor/scalable/apps/$APP_ID.svg" \
    "$SHARE"/icons/hicolor/*/apps/"$APP_ID".png
  command -v update-desktop-database >/dev/null && update-desktop-database "$SHARE/applications" || true
  if [ -d "$SKILL_DIR" ]; then
    rm -rf "${SKILL_DIR:?}"
    say "Claude Code skill removed from $SKILL_DIR"
  fi
  say "Auriscope removed from $PREFIX"
  exit 0
fi

# ---- get the files ----------------------------------------------------------

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Where the binary, the desktop entry, the SVG and the metainfo end up before
# installing. Both modes fill these four, and `cli` when there is one.
binary=""; cli=""; desktop=""; svg=""; metainfo=""; skill=""

if [ "$MODE" = release ]; then
  need curl; need tar; need sha256sum
  [ "$(uname -m)" = x86_64 ] || die "no prebuilt binary for $(uname -m); try --source"
  if [ -z "$VERSION" ]; then
    # /releases/latest redirects to /releases/tag/vX.Y.Z; drafts and
    # pre-releases are skipped, the same way the app's own update check works.
    VERSION="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest")"
    VERSION="${VERSION##*/}"
    [ -n "$VERSION" ] || die "could not resolve the latest release"
  fi
  have="$(bare_version)"
  # A development build is never "current": installing a release over it is
  # exactly how you go back to a tested version.
  if [ "$FORCE" = 0 ] && [ -n "$have" ] && [ "v$have" = "$VERSION" ] && ! is_dev_build; then
    say "Auriscope $have is already installed and current."
    exit 0
  fi
  archive="auriscope-$VERSION-x86_64-linux.tar.gz"
  url="https://github.com/$REPO/releases/download/$VERSION/$archive"
  [ -n "$have" ] && say "Updating Auriscope $have to $VERSION" \
    || say "Downloading Auriscope $VERSION"
  curl -fsSL -o "$WORK/$archive" "$url"
  curl -fsSL -o "$WORK/$archive.sha256" "$url.sha256"
  (cd "$WORK" && sha256sum -c --quiet "$archive.sha256") || die "checksum mismatch"
  tar -C "$WORK" -xzf "$WORK/$archive"
  binary="$WORK/auriscope/auriscope"
  cli="$WORK/auriscope/auriscope-cli"
  desktop="$WORK/auriscope/$APP_ID.desktop"
  svg="$WORK/auriscope/$APP_ID.svg"
  metainfo="$WORK/auriscope/$APP_ID.metainfo.xml"
  # Releases before the skill existed simply do not carry this directory.
  skill="$WORK/auriscope/skills/auriscope-cli"
else
  need cargo
  # Inside a checkout? Build that, unless --git asked for the newest main.
  here="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || true)"
  if [ "$FRESH" = 0 ] && [ -n "$here" ] \
     && grep -q '^name = "auriscope"' "$here/Cargo.toml" 2>/dev/null; then
    src="$here"
  else
    need git
    if [ -d "$CACHE/.git" ]; then
      say "Fetching the newest main into $CACHE"
      # --depth 1 clones cannot fast-forward across a force-push; reset to
      # whatever origin/main is now, which is what "bleeding edge" means.
      git -C "$CACHE" fetch --depth 1 origin main
      git -C "$CACHE" reset --hard FETCH_HEAD
    else
      git clone --depth 1 "https://github.com/$REPO.git" "$CACHE"
    fi
    # `git describe` needs tags to name the build; a depth-1 clone has none.
    git -C "$CACHE" fetch --tags --depth 1 origin 2>/dev/null || true
    src="$CACHE"
  fi
  say "Building Auriscope from $src @ $(git -C "$src" rev-parse --short=7 HEAD 2>/dev/null || echo 'no git') (release profile, a few minutes)"
  if ! (cd "$src" && cargo build --release --locked); then
    cat >&2 <<'HINT'

The build failed. If it stopped on a missing system library, install:
  Arch:           sudo pacman -S alsa-lib libxkbcommon wayland gtk3
  Debian/Ubuntu:  sudo apt install libasound2-dev libxkbcommon-dev libwayland-dev libgtk-3-dev
and run this again.
HINT
    exit 1
  fi
  binary="$src/target/release/auriscope"
  cli="$src/target/release/auriscope-cli"
  desktop="$src/assets/$APP_ID.desktop"
  svg="$src/assets/$APP_ID.svg"
  metainfo="$src/assets/$APP_ID.metainfo.xml"
  skill="$src/skills/auriscope-cli"
fi

# ---- install ----------------------------------------------------------------

say "Installing to $PREFIX"
install -Dm755 "$binary" "$BIN"
# Only if this version has one: an archive from before it existed should
# install the app rather than fail over a missing file. An `if` and not
# `a && b`, which under `set -e` would end the script when the file is
# absent, while `|| true` would hide a real failure to install it.
if [ -n "$cli" ] && [ -f "$cli" ]; then
  install -Dm755 "$cli" "$CLI_BIN"
fi
# The launcher session does not always have ~/.local/bin on PATH, so the
# desktop entry names the binary by absolute path.
mkdir -p "$SHARE/applications"
sed "s|^Exec=.*|Exec=$BIN %f|" "$desktop" > "$SHARE/applications/$APP_ID.desktop"
install -Dm644 "$svg" "$SHARE/icons/hicolor/scalable/apps/$APP_ID.svg"
install -Dm644 "$metainfo" "$SHARE/metainfo/$APP_ID.metainfo.xml"
if command -v rsvg-convert >/dev/null 2>&1; then
  for s in 16 24 32 48 64 128 256 512; do
    mkdir -p "$SHARE/icons/hicolor/${s}x${s}/apps"
    rsvg-convert -w "$s" -h "$s" "$svg" -o "$SHARE/icons/hicolor/${s}x${s}/apps/$APP_ID.png"
  done
fi
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$SHARE/applications" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -f -t "$SHARE/icons/hicolor" 2>/dev/null || true

# The skill, if this version ships one and this machine wants it. Copied with
# -L so the references, which are symlinks into docs/ in the repository, land
# as real files: the installed skill has to stand on its own.
skill_installed=""
if [ "$SKILL" != no ] && [ -n "$skill" ] && [ -d "$skill" ] \
   && { [ "$SKILL" = force ] || [ -d "$SKILLS" ] || [ -d "$HOME/.claude" ]; }; then
  mkdir -p "$SKILL_DIR"
  # Replace rather than merge: a reference renamed upstream should not linger.
  rm -rf "${SKILL_DIR:?}/reference"
  cp -RL "$skill/." "$SKILL_DIR/"
  skill_installed="$SKILL_DIR"
fi

now="$(installed_version)"
say "Done: $BIN (${now:-${VERSION:-installed}})"
if [ -x "$CLI_BIN" ]; then
  say "Also: $CLI_BIN (analysis without a window)"
fi
if [ -n "$skill_installed" ]; then
  say "Also: $skill_installed (Claude Code skill; --no-skill to skip)"
fi
case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *) echo "note: $PREFIX/bin is not on your PATH; the launcher entry works regardless." ;;
esac
