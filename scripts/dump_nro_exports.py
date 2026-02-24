#!/usr/bin/env python3
"""Dump exported symbols from a Nintendo Switch .nro into a sidecar .txt file.

Usage examples:
  python scripts/dump_nro_exports.py path/to/game.nro
  python scripts/dump_nro_exports.py --target-dir target
  python scripts/dump_nro_exports.py --target-dir target --profile release
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="List exported symbols from a .nro and write them to a .txt file."
    )
    parser.add_argument(
        "nro",
        nargs="?",
        type=Path,
        help="Path to the .nro file. If omitted, the newest .nro in --target-dir is used.",
    )
    parser.add_argument(
        "--target-dir",
        type=Path,
        default=Path("target"),
        help="Cargo target dir used to auto-discover .nro files (default: target).",
    )
    parser.add_argument(
        "--profile",
        default=None,
        help="Optional build profile filter (for example: debug, release).",
    )
    parser.add_argument(
        "--tool",
        default=None,
        help="Explicit symbol tool to use (for example: llvm-nm).",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=None,
        help="Output path for the symbol list. Default: <nro>.exports.txt",
    )
    return parser.parse_args()


def find_newest_nro(target_dir: Path, profile: str | None) -> Path:
    if not target_dir.exists():
        raise FileNotFoundError(f"target dir does not exist: {target_dir}")

    candidates = []
    for nro in target_dir.rglob("*.nro"):
        if profile and profile not in nro.parts:
            continue
        candidates.append(nro)

    if not candidates:
        raise FileNotFoundError(f"no .nro files found under: {target_dir}")

    return max(candidates, key=lambda p: p.stat().st_mtime)


def pick_tool(explicit: str | None) -> str:
    if explicit:
        return explicit

    for name in ("llvm-nm", "nm", "rust-nm", "aarch64-none-elf-nm"):
        if shutil.which(name):
            return name
    raise RuntimeError(
        "No symbol tool found. Install one of: llvm-nm, nm, rust-nm, aarch64-none-elf-nm"
    )


def run_nm(tool: str, nro_path: Path) -> list[str]:
    cmd = [tool, "-g", "--defined-only", str(nro_path)]
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip() or f"{tool} failed on {nro_path}")

    symbols: list[str] = []
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        parts = line.split()
        if len(parts) < 2:
            continue
        symbol = parts[-1]
        if symbol and symbol not in symbols:
            symbols.append(symbol)
    return symbols


def main() -> int:
    args = parse_args()

    try:
        nro_path = args.nro if args.nro else find_newest_nro(args.target_dir, args.profile)
        nro_path = nro_path.resolve()
        if not nro_path.exists():
            raise FileNotFoundError(f".nro not found: {nro_path}")

        tool = pick_tool(args.tool)
        symbols = run_nm(tool, nro_path)

        if args.output:
            out_path = args.output.resolve()
        else:
            out_path = (nro_path.parent / f"{nro_path.name}.exports.txt").resolve()

        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text("\n".join(symbols) + ("\n" if symbols else ""), encoding="utf-8")

        print(f"nro: {nro_path}")
        print(f"tool: {tool}")
        print(f"symbols: {len(symbols)}")
        print(f"output: {out_path}")
        return 0
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
