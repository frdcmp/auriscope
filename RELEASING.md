# Releasing Auriscope

Everything downstream — the GitHub release archives, the Flatpak, the AUR
package and the in-app "new version available" notice — keys off one thing: an
annotated `vX.Y.Z` tag on `main`. Do the steps in this order.

## 1. Bump the version

- `Cargo.toml`: `version = "X.Y.Z"`. Then `cargo build` so `Cargo.lock`
  picks up the new version of the `auriscope` package itself.
- `assets/io.github.frdcmp.Auriscope.metainfo.xml`: add a `<release>` at the
  top of `<releases>` with today's date and a short `<description>`. Flathub
  shows this text on the store page, and its linter expects the newest entry
  to match the version being tagged.
- `packaging/flatpak/cargo-sources.json`: regenerate whenever `Cargo.lock`
  changed since the last release (it did, because of the version bump):

  ```bash
  python3 packaging/flatpak/gen-cargo-sources.py Cargo.lock -o packaging/flatpak/cargo-sources.json
  ```

- `packaging/arch/auriscope/PKGBUILD`: `pkgver=X.Y.Z`, `pkgrel=1`.

Check it all still builds the way each package builds it:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo check --no-default-features --features portal   # Flatpak
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

## 3. Pin the Flatpak manifest to the tag

The tag's **tag object** hash, not the commit's, goes into the manifest:

```bash
git rev-parse vX.Y.Z          # tag object; this is what `commit:` wants
git rev-parse vX.Y.Z^{commit} # the commit it points at, for reference
```

Set `tag:` and `commit:` in `packaging/flatpak/io.github.frdcmp.Auriscope.yml`
and commit that as a follow-up (`Pin the Flatpak manifest to vX.Y.Z`). The
manifest in this repo is for local builds and as the source of truth; Flathub
builds from its own copy, next step.

Try it locally before opening the Flathub PR:

```bash
flatpak-builder --user --install --force-clean \
  ~/.cache/auriscope-flatpak/build packaging/flatpak/io.github.frdcmp.Auriscope.yml
flatpak run io.github.frdcmp.Auriscope
```

## 4. Flathub

Flathub keeps the manifest in its own repository, `flathub/io.github.frdcmp.Auriscope`
(after the first submission is merged; until then, the PR against
`flathub/flathub`). For each release:

1. In a checkout of that repository, copy over
   `packaging/flatpak/io.github.frdcmp.Auriscope.yml` and
   `packaging/flatpak/cargo-sources.json`.
2. Open a pull request. The Flathub bot builds it; when the test build passes,
   merge. Flathub publishes within a few hours.

Rules that bite: the manifest must build offline (hence `cargo-sources.json`),
the metainfo release entry must match the tag, and the app must not update or
check for updates itself — which is why Flathub builds with
`--no-default-features --features portal`.

## 5. AUR

Needs an AUR account with an SSH key. In `packaging/arch/auriscope/`:

```bash
updpkgsums                                # fills sha256sums from the tag tarball
makepkg --printsrcinfo > .SRCINFO
makepkg -sf                               # build it once, for real
```

Commit `PKGBUILD` and `.SRCINFO` here, then push the same two files to
`ssh://aur@aur.archlinux.org/auriscope.git` (the AUR repo holds only those two
files at its root). `auriscope-git` needs no change per release.

## 6. Windows

The zip on the GitHub release is the Windows distribution. People who
downloaded it get the in-app notice on their next launch, at most one check a
day, pointing at the release page. A winget manifest, so `winget upgrade`
handles it too, is the planned next step — see the roadmap in the README.

## Checklist

```
[ ] Cargo.toml version, Cargo.lock rebuilt
[ ] metainfo <release> entry
[ ] cargo-sources.json regenerated
[ ] PKGBUILD pkgver / pkgrel
[ ] fmt, clippy, test, both --no-default-features checks
[ ] commit, annotated tag, push main + tag
[ ] release.yml green, four assets on the release, not draft
[ ] manifest tag: + commit: (tag object hash), committed
[ ] Flathub PR opened and merged
[ ] AUR: updpkgsums, .SRCINFO, push
```

## Conventions

- Tags are `vX.Y.Z`, semver. The update check compares the numeric part
  against `CARGO_PKG_VERSION`, so a tag like `v0.2.0-rc1` is seen as older
  than `v0.2.0` and as newer than `v0.1.9`; publish pre-releases as GitHub
  pre-releases and `/releases/latest` will skip them entirely.
- Never re-point a published tag. If the release is wrong, fix forward with a
  patch version.
