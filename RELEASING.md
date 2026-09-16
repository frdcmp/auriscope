# Releasing Auriscope

Everything downstream — the GitHub release archives, the AUR package and the
in-app "new version available" notice — keys off one thing: an annotated
`vX.Y.Z` tag on `main`. Do the steps in this order.

## 1. Bump the version

- `Cargo.toml`: `version = "X.Y.Z"`. Then `cargo build` so `Cargo.lock`
  picks up the new version of the `auriscope` package itself.
- `assets/io.github.frdcmp.Auriscope.metainfo.xml`: add a `<release>` at the
  top of `<releases>` with today's date and a short `<description>`. Software
  centres show this text, and AppStream linters expect the newest entry to
  match the version being tagged.
- `packaging/arch/auriscope/PKGBUILD`: `pkgver=X.Y.Z`, `pkgrel=1`.

Check it all still builds the way each package builds it:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo check --no-default-features --features portal   # portal file dialog
cargo check --no-default-features --features gtk      # AUR
```

Commit: `git commit -am "Release vX.Y.Z"`.

## 2. Tag and push

```bash
git tag -a vX.Y.Z -m "Auriscope X.Y.Z"
git push origin main vX.Y.Z
```

The tag push triggers `.github/workflows/release.yml`, which builds the Linux
tarball and the Windows zip, writes their `.sha256` files, and attaches all
four to a GitHub release with generated notes. Watch it:

```bash
gh run watch
gh release view vX.Y.Z
```

Edit the release notes on GitHub if the generated list needs a human summary.
Leave the release published, not a draft or pre-release: the in-app update
check reads `/releases/latest`, which skips both.

## 3. AUR

Needs an AUR account with an SSH key. In `packaging/arch/auriscope/`:

```bash
updpkgsums                                # fills sha256sums from the tag tarball
makepkg --printsrcinfo > .SRCINFO
makepkg -sf                               # build it once, for real
```

Commit `PKGBUILD` and `.SRCINFO` here, then push the same two files to
`ssh://aur@aur.archlinux.org/auriscope.git` (the AUR repo holds only those two
files at its root). `auriscope-git` needs no change per release.

## 4. Windows

The zip on the GitHub release is the Windows distribution. People who
downloaded it get the in-app notice on their next launch, at most one check a
day, pointing at the release page. A winget manifest, so `winget upgrade`
handles it too, is the planned next step — see the roadmap in the README.

## Checklist

```
[ ] Cargo.toml version, Cargo.lock rebuilt
[ ] metainfo <release> entry
[ ] PKGBUILD pkgver / pkgrel
[ ] fmt, clippy, test, both --no-default-features checks
[ ] commit, annotated tag, push main + tag
[ ] release.yml green, four assets on the release, not draft
[ ] AUR: updpkgsums, .SRCINFO, push
```

## Conventions

- Tags are `vX.Y.Z`, semver. The update check compares the numeric part
  against `CARGO_PKG_VERSION`, so a tag like `v0.2.0-rc1` is seen as older
  than `v0.2.0` and as newer than `v0.1.9`; publish pre-releases as GitHub
  pre-releases and `/releases/latest` will skip them entirely.
- Never re-point a published tag. If the release is wrong, fix forward with a
  patch version.
