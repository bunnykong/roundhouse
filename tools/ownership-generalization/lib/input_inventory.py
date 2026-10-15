"""Corpus identity helpers; no Ruby, dependencies, or diagnostic text."""
import hashlib
import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parent
FRONTIER = ROOT.parent.parent


def file_sha(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def inventory(root):
    """Hash paths, executable bits, link targets and bytes; omit only Git metadata."""
    root = Path(root)
    rows = []
    for base, dirs, files in os.walk(root, followlinks=False):
        dirs[:] = sorted(d for d in dirs if d != ".git")
        links = [d for d in dirs if (Path(base) / d).is_symlink()]
        dirs[:] = [d for d in dirs if d not in links]
        for name in sorted(files + links):
            if name == ".git":
                continue
            path = Path(base) / name
            row = {"path": path.relative_to(root).as_posix()}
            if path.is_symlink():
                row.update(kind="symlink", target=os.readlink(path))
            else:
                row.update(kind="file", sha256=file_sha(path), bytes=path.stat().st_size,
                           executable=bool(path.stat().st_mode & 0o111))
            rows.append(row)
    rows.sort(key=lambda row: row["path"])
    digest = hashlib.sha256(json.dumps(rows, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    return digest, rows


def write_json(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(data, indent=2, sort_keys=True, allow_nan=False) + "\n")
    temporary.replace(path)


def verify(manifest, selected=None):
    failures = []
    for item in manifest["inputs"]:
        if selected and item["set"] not in selected:
            continue
        digest, _ = inventory(ROOT / item["path"])
        if digest != item["tree_sha256"] or not (ROOT / item["path"] / "app").is_dir():
            failures.append(item["id"])
    return failures
