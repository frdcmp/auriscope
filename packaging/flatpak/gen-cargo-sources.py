#!/usr/bin/env python3
"""Turn Cargo.lock into a flatpak-builder sources list.

Flathub builds have no network, so every crate is listed as an archive source
with the checksum Cargo.lock already records, unpacked into a vendor directory
that a generated cargo config points at. This is the same layout the official
flatpak-cargo-generator produces, written with the standard library only so
it runs anywhere Python 3.11+ does.

    python3 gen-cargo-sources.py Cargo.lock -o cargo-sources.json

Git dependencies are not supported; Auriscope has none.
"""

import argparse
import json
import sys
import tomllib

CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
VENDOR = "cargo/vendor"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("lockfile")
    ap.add_argument("-o", "--output", default="cargo-sources.json")
    args = ap.parse_args()

    with open(args.lockfile, "rb") as f:
        lock = tomllib.load(f)

    sources = []
    for pkg in lock.get("package", []):
        source = pkg.get("source")
        if source is None:
            continue  # the workspace crate itself
        if source != CRATES_IO:
            print(f"unsupported source for {pkg['name']}: {source}", file=sys.stderr)
            return 1
        name, version, checksum = pkg["name"], pkg["version"], pkg["checksum"]
        dest = f"{VENDOR}/{name}-{version}"
        sources.append(
            {
                "type": "archive",
                "archive-type": "tar-gzip",
                "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
                "sha256": checksum,
                "dest": dest,
            }
        )
        sources.append(
            {
                "type": "inline",
                "contents": json.dumps({"package": checksum, "files": {}}),
                "dest": dest,
                "dest-filename": ".cargo-checksum.json",
            }
        )

    sources.append(
        {
            "type": "inline",
            "contents": (
                "[source.crates-io]\n"
                'replace-with = "vendored-sources"\n\n'
                "[source.vendored-sources]\n"
                f'directory = "{VENDOR}"\n'
            ),
            "dest": "cargo",
            "dest-filename": "config",
        }
    )

    with open(args.output, "w") as f:
        json.dump(sources, f, indent=2)
        f.write("\n")
    crates = (len(sources) - 1) // 2
    print(f"wrote {args.output}: {crates} crates")
    return 0


if __name__ == "__main__":
    sys.exit(main())
