#!/usr/bin/env python3
"""Sync the local credseal package, or verify its recorded source hashes."""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import sys
import tomllib


ROOT = Path(__file__).resolve().parents[1]
DESTINATION = ROOT / "vendor" / "credseal"
RECORD = "SOURCE.json"
FILES = {"Cargo.toml", "Cargo.lock", "LICENSE", "NOTICE", "README.md", "AGENTS.md", ".gitignore", ".gitattributes"}
DIRECTORIES = {"src", "tests", "examples", "benches"}


def fail(message):
    raise SystemExit(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def ignored(path):
    return any(part in {".git", "target", "__pycache__"} or part.startswith(".env")
               or part.endswith((".key", ".toml.enc.lock")) for part in path.parts)


def paths_in(root):
    for directory, directories, files in os.walk(root, followlinks=False):
        directories[:] = [name for name in directories
                          if not ignored((Path(directory) / name).relative_to(root))]
        for name in directories + files:
            path = Path(directory) / name
            if not ignored(path.relative_to(root)):
                yield path


def allowed(name):
    path = PurePosixPath(name)
    return (not path.is_absolute() and ".." not in path.parts and name != RECORD
            and (name in FILES or len(path.parts) > 1 and path.parts[0] in DIRECTORIES)
            and not ignored(path))


def read_source(source):
    if source.is_symlink() or not source.is_dir():
        fail("The source must be a regular directory")
    try:
        manifest = tomllib.loads((source / "Cargo.toml").read_text())
    except (OSError, ValueError):
        fail("The source needs a valid Cargo.toml")
    if manifest.get("package", {}).get("name") != "credseal":
        fail("The source package must be named credseal")
    contents = {}
    for path in sorted(paths_in(source)):
        name = path.relative_to(source).as_posix()
        if not allowed(name):
            continue
        if path.is_symlink():
            fail(f"Source symbolic links are not supported: {name}")
        if path.is_file():
            contents[name] = path.read_bytes()
    for name in ["Cargo.toml", "LICENSE", "NOTICE", "src/lib.rs"]:
        if name not in contents:
            fail(f"The source is missing {name}")
    return manifest["package"]["version"], contents


def recorded_files():
    record = DESTINATION / RECORD
    if DESTINATION.is_symlink() or record.is_symlink():
        fail("Vendor symbolic links are not supported")
    if not record.exists():
        if DESTINATION.exists() and any(DESTINATION.iterdir()):
            fail("Refusing to replace a vendor directory without SOURCE.json")
        return {}
    try:
        value = json.loads(record.read_text())
        files = value["files"]
        if value["package"] != "credseal" or not isinstance(files, dict):
            raise ValueError
        for name, expected in files.items():
            if not allowed(name) or not isinstance(expected, str) or len(expected) != 64:
                raise ValueError
    except (OSError, ValueError, KeyError, TypeError):
        fail("Invalid vendor/credseal/SOURCE.json")
    return files


def verify(files):
    if DESTINATION.is_symlink():
        fail("The vendor directory must not be a symbolic link")
    paths = list(paths_in(DESTINATION))
    if any(path.is_symlink() for path in paths):
        fail("Vendor symbolic links are not supported")
    for name, expected in files.items():
        path = DESTINATION / name
        if not path.is_file() or digest(path.read_bytes()) != expected:
            fail(f"Vendored file differs from SOURCE.json: {name}")
    actual = {path.relative_to(DESTINATION).as_posix() for path in paths
              if path.is_file() and path.relative_to(DESTINATION).as_posix() != RECORD}
    if actual != set(files):
        fail("The vendor directory contains unrecorded files")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", nargs="?", type=Path)
    parser.add_argument("--check", action="store_true", help="Compare the snapshot with the source without writing")
    parser.add_argument("--verify", action="store_true", help="Check the snapshot without the source checkout")
    args = parser.parse_args()
    if args.verify and (args.source or args.check) or not args.verify and args.source is None:
        parser.error("supply a source directory, or --verify")
    files = recorded_files()
    if files:
        verify(files)
    if args.verify:
        if not files:
            fail("No credseal snapshot is recorded")
        print(f"Verified {len(files)} credseal source files")
        return
    version, contents = read_source(args.source)
    expected = {name: digest(data) for name, data in contents.items()}
    if args.check:
        if expected != files:
            fail("The credseal source differs from the vendored snapshot")
        print(f"Snapshot matches credseal {version}")
        return
    DESTINATION.mkdir(parents=True, exist_ok=True)
    for name in set(files) - set(contents):
        (DESTINATION / name).unlink()
    for name, data in contents.items():
        path = DESTINATION / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    record = {"package": "credseal", "version": version, "files": expected}
    (DESTINATION / RECORD).write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
    verify(expected)
    print(f"Synced credseal {version}: {len(contents)} files")


if __name__ == "__main__":
    main()
