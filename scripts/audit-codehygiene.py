#!/usr/bin/env python3
"""Read-only, bounded CodeHygiene scan of tracked WM source; no source execution.

Run with a Python containing NumPy and an inspected CodeHygiene checkout.
Window scores are heuristics, NOT security/performance/architecture verdicts.
"""
from __future__ import annotations

import argparse
from collections import Counter
from datetime import datetime, timezone
import hashlib
import importlib
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import time

for key in ("OPENBLAS_NUM_THREADS", "OMP_NUM_THREADS", "VECLIB_MAXIMUM_THREADS", "MKL_NUM_THREADS"):
    os.environ[key] = "1"
sys.dont_write_bytecode = True

EXTENSIONS = {".rs", ".py", ".js", ".mjs", ".cjs", ".ts", ".tsx", ".jsx", ".sh"}
GENERATED = ("web/dist/", "target/", "dist/", "node_modules/")


def git(root: Path, *args: str) -> bytes:
    return subprocess.check_output(["git", "-C", str(root), *args])


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def windows(count: int, size: int, overlap: int):
    start = 0
    while True:
        end = min(count, start + size)
        yield start, end
        if end >= count:
            return
        start = end - overlap


def require_finite(value):
    if isinstance(value, float) and not math.isfinite(value):
        raise ValueError("Nonfinite model output; reject this runtime/result")
    if isinstance(value, dict):
        for item in value.values():
            require_finite(item)
    elif isinstance(value, (tuple, list)):
        for item in value:
            require_finite(item)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tool-path", required=True, type=Path)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--output", required=True, type=Path, help="New directory; refuses overwrite")
    parser.add_argument("--window", type=int, default=256)
    parser.add_argument("--overlap", type=int, default=64)
    parser.add_argument("--threshold", type=float, default=0.55)
    args = parser.parse_args()
    if not 64 <= args.window <= 512 or not 0 <= args.overlap < args.window:
        parser.error("window must be 64..512; overlap must be 0..window-1")
    if not 0 <= args.threshold <= 1:
        parser.error("threshold must be 0..1")
    root = args.root.resolve(strict=True)
    tool = args.tool_path.resolve(strict=True)
    sys.path.insert(0, str(tool))
    codehygiene = importlib.import_module("codehygiene")
    if Path(codehygiene.__file__).resolve() != tool / "codehygiene" / "__init__.py":
        raise RuntimeError("Unexpected CodeHygiene import location")
    import numpy as np
    # Fail before producing evidence if this numerical runtime is inconsistent.
    sample = np.arange(258 * 16, dtype=np.float32).reshape(258, 16) % 7
    kernel = np.arange(16 * 16, dtype=np.float32).reshape(16, 16) % 3 - 1
    if not np.allclose(sample @ kernel, np.einsum("ni,io->no", sample, kernel, optimize=False)):
        raise RuntimeError("NumPy matmul/reference sanity check failed")
    require_finite(codehygiene.score('fn main() {\n    println!("hello");\n}\n').to_dict())
    names = sorted(p.decode("utf-8") for p in git(root, "ls-files", "-z").split(b"\0") if p)
    selected, excluded = [], []
    for name in names:
        path = root / name
        if name.startswith(GENERATED):
            excluded.append({"path": name, "reason": "generated/distribution"})
        elif path.suffix.lower() in EXTENSIONS or name.endswith("/debug-discover"):
            selected.append(name)
        else:
            excluded.append({"path": name, "reason": "not scored source (config/docs/assets/binary)"})
    # Scan metadata contains no credentials or source contents.
    output = args.output.resolve()
    if output != root and root not in output.parents:
        parser.error("output must be inside the audited workspace")
    output.mkdir(parents=True, exist_ok=False)
    started = time.monotonic()
    metadata = {
        "started_at": datetime.now(timezone.utc).isoformat(),
        "root": str(root), "git_commit": git(root, "rev-parse", "HEAD").decode().strip(),
        "git_status_at_start": git(root, "status", "--short").decode(),
        "tool_path": str(tool), "tool_version": codehygiene.__version__,
        "tool_commit": git(tool, "rev-parse", "HEAD").decode().strip(),
        "tool_sha256": digest((tool / "codehygiene/__init__.py").read_bytes()),
        "weights_sha256": digest((tool / "codehygiene/weights.npz").read_bytes()),
        "python": sys.version, "numpy": np.__version__,
        "runner_sha256": digest(Path(__file__).read_bytes()),
        "window_lines": args.window, "overlap_lines": args.overlap,
        "threshold": args.threshold, "columns_seen_by_model": codehygiene.MAX_COLS,
        "semantics": "All tracked selected source covered by overlapping bounded windows. Scores are window heuristics, not native full-file scores. Diagnostics remapped and deduplicated by original line+kind. No code execution, no automatic fixes, no security/performance certification. Shell is auxiliary heuristic coverage. Config/docs/assets require manual review.",
        "selected": selected, "excluded": excluded,
    }
    (output / "manifest.json").write_text(json.dumps(metadata, indent=2) + "\n")
    results, errors = [], []
    with (output / "results.jsonl").open("x") as stream:
        for index, name in enumerate(selected, 1):
            try:
                path = (root / name).resolve(strict=True)
                if root not in path.parents or (root / name).is_symlink():
                    raise ValueError("Refuse source symlink/outside-root path")
                if path.stat().st_size > 2_000_000:
                    raise ValueError("File exceeds 2 MB bounded input limit")
                data = path.read_bytes()
                text = data.decode("utf-8")
                lines = text.splitlines()
                diagnostics, chunks = {}, []
                for start, end in windows(len(lines), args.window, args.overlap):
                    result = codehygiene.score("\n".join(lines[start:end]), threshold=args.threshold).to_dict()
                    require_finite(result)
                    for issue in result.pop("squiggles"):
                        issue["line"] += start
                        key = (issue["line"], issue["kind"])
                        if key not in diagnostics or issue["probability"] > diagnostics[key]["probability"]:
                            diagnostics[key] = issue
                    chunks.append({"start_line": start + 1, "end_line": end, **result})
                record = {
                    "path": name, "sha256": digest(data), "lines": len(lines),
                    "lines_over_128_columns": sum(len(line.expandtabs(2)) > codehygiene.MAX_COLS for line in lines),
                    "window_score_mean": round(sum(c["score"] for c in chunks) / len(chunks), 4),
                    "windows": chunks, "squiggles": sorted(diagnostics.values(), key=lambda d: (d["line"], d["kind"])),
                }
                stream.write(json.dumps(record, allow_nan=False) + "\n")
                stream.flush()
                results.append(record)
                print(f"[{index}/{len(selected)}] {name}: {len(lines)} lines, {len(diagnostics)} signals", flush=True)
            except Exception as exc:
                errors.append({"path": name, "error": f"{type(exc).__name__}: {exc}"})
                print(f"ERROR {name}: {exc}", file=sys.stderr, flush=True)
    kinds = Counter(issue["kind"] for record in results for issue in record["squiggles"])
    summary = {
        "files_selected": len(selected), "files_scored": len(results),
        "source_lines": sum(record["lines"] for record in results),
        "windows": sum(len(record["windows"]) for record in results),
        "signals": sum(kinds.values()), "signal_kinds": dict(kinds), "errors": errors,
        "elapsed_seconds": round(time.monotonic() - started, 2),
        "most_signals": [{"path": r["path"], "lines": r["lines"], "signals": len(r["squiggles"]), "window_score_mean": r["window_score_mean"]} for r in sorted(results, key=lambda r: len(r["squiggles"]), reverse=True)[:25]],
    }
    (output / "summary.json").write_text(json.dumps(summary, indent=2, allow_nan=False) + "\n")
    print(json.dumps({k: v for k, v in summary.items() if k != "most_signals"}, indent=2))
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
