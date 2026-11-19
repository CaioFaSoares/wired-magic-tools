#!/usr/bin/env python3
"""Package prebuilt React assets. Never invoked by a default Rust/server build.

Requires Python 3.11+. No network, extraction, installation, or activation.
Only packages a bounded snapshot of allowlisted static assets from --dist.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import tarfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
MAX_FILES = 1024
MAX_FILE_BYTES = 8 * 1024 * 1024
MAX_TOTAL_BYTES = 32 * 1024 * 1024
MAX_MANIFEST_BYTES = 512 * 1024
EXTENSIONS = {"js", "mjs", "css", "svg", "png", "jpg", "jpeg", "gif", "webp", "avif", "ico", "woff", "woff2", "ttf", "otf"}


def safe_name(name: str) -> bool:
    parts = name.split("/")
    return (0 < len(name) <= 240 and len(parts) <= 12
            and all(part and not part.startswith(".")
                    and all(c.isascii() and (c.isalnum() or c in "._-") for c in part)
                    for part in parts))


def collect_assets(dist: Path, version: str) -> tuple[dict[str, bytes], bytes]:
    if dist.is_symlink() or not dist.is_dir():
        raise ValueError("--dist must be a real directory, not a symlink")
    files: dict[str, bytes] = {}
    total = 0
    for directory, dirs, names in os.walk(dist, followlinks=False):
        for dirname in dirs:
            path = Path(directory) / dirname
            if path.is_symlink():
                raise ValueError(f"symlink directory rejected: {path.name}")
        for filename in sorted(names):
            path = Path(directory) / filename
            name = path.relative_to(dist).as_posix()
            if not safe_name(name) or (name != "index.html" and name.rsplit(".", 1)[-1] not in EXTENSIONS):
                raise ValueError(f"unsupported asset path/type: {name}")
            metadata = path.lstat()
            if not stat.S_ISREG(metadata.st_mode):
                raise ValueError(f"asset must be a regular file: {name}")
            if metadata.st_size > MAX_FILE_BYTES:
                raise ValueError(f"asset exceeds size limit: {name}")
            # Bounded read also handles a file growing after stat.
            with path.open("rb") as handle:
                contents = handle.read(MAX_FILE_BYTES + 1)
            if len(contents) > MAX_FILE_BYTES:
                raise ValueError(f"asset grew beyond size limit: {name}")
            total += len(contents)
            if total > MAX_TOTAL_BYTES or len(files) >= MAX_FILES:
                raise ValueError("panel package exceeds total byte/file limit")
            files[name] = contents
    if "index.html" not in files:
        raise ValueError("index.html is required")
    manifest = json.dumps({"schema": 1, "id": "control_panel", "version": version,
                           "files": {name: hashlib.sha256(contents).hexdigest()
                                     for name, contents in sorted(files.items())}},
                          sort_keys=True, separators=(",", ":")).encode() + b"\n"
    if len(manifest) > MAX_MANIFEST_BYTES:
        raise ValueError("bundle manifest exceeds size limit")
    return files, manifest


def package(dist: Path, output: Path, version: str, extension_manifest: bytes) -> tuple[str, str]:
    files, manifest = collect_assets(dist, version)
    checksum_path = output.with_name(output.name + ".sha256")
    if output.exists() or output.is_symlink() or checksum_path.exists() or checksum_path.is_symlink():
        raise FileExistsError("refusing to overwrite package or checksum")
    output.parent.mkdir(parents=True, exist_ok=True)
    entries = {"control_panel/extension.toml": extension_manifest,
               "control_panel/www/bundle.json": manifest}
    entries.update({"control_panel/www/" + name: contents for name, contents in files.items()})
    with output.open("xb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
        with tarfile.open(mode="w", fileobj=compressed, format=tarfile.USTAR_FORMAT) as archive:
            for name, contents in sorted(entries.items()):
                item = tarfile.TarInfo(name)
                item.size = len(contents)
                item.mode = 0o644
                item.mtime = 0
                item.uid = item.gid = 0
                item.uname = item.gname = ""
                archive.addfile(item, io.BytesIO(contents))
    checksum = hashlib.sha256(output.read_bytes()).hexdigest()
    with checksum_path.open("x", encoding="utf-8") as handle:
        handle.write(f"{checksum}  {output.name}\n")
    return checksum, hashlib.sha256(manifest).hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist", type=Path, default=ROOT / "web/dist")
    parser.add_argument("--output", type=Path, required=True, help="new .tar.gz path (existing output is never overwritten)")
    args = parser.parse_args()
    with (ROOT / "Cargo.toml").open("rb") as handle:
        version = tomllib.load(handle)["workspace"]["package"]["version"]
    extension_manifest = (ROOT / "extensions/public/control_panel/extension.toml").read_bytes()
    if tomllib.loads(extension_manifest.decode())["version"] != version:
        parser.error("panel manifest version must match workspace package version")
    try:
        checksum, digest = package(args.dist, args.output, version, extension_manifest)
    except (ValueError, OSError) as error:
        parser.exit(1, f"error: {error}\n")
    print(f"Package SHA-256: {checksum}\nRuntime digest: {digest}\nNot installed or activated.")


if __name__ == "__main__":
    main()
