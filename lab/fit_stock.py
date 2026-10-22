#!/usr/bin/env python3
"""Fit a parametric stock to camera JPEG pairs.

`refs` is a directory with `fit/`, `heldout/`, and `manifest.md`. Only
`fit/` is minimized. `heldout/` is reported as a check and is not part of
the loss. The same command fits a later simulation by pointing it at that
simulation's pairs and passing `--simulation`.

Reference photographs are not written into the repository. Pass a path
outside the tree.

Example:
    cargo build -p rawstock-cli --release
    python3 lab/fit_stock.py /path/to/provia-ref \\
        --name Velvia --simulation Velvia \\
        -o stocks/velvia.stock.toml \\
        --report-dir /tmp/velvia-report
"""

from __future__ import annotations

import argparse
from pathlib import Path

from stockfit.fit import FitConfig, FitSession


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("refs", type=Path, help="Directory with fit/, heldout/, and manifest.md")
    parser.add_argument("-o", "--output", type=Path, required=True, help="Schema-1 stock to write")
    parser.add_argument("--name", default="Velvia", help="Stock name recorded in the file")
    parser.add_argument(
        "--simulation",
        default="Velvia",
        help="Manifest film mode to fit. Held-out rows must not be this mode.",
    )
    parser.add_argument("--binary", type=Path, help="rawstock binary (default: target/release/rawstock)")
    parser.add_argument("--long-edge", type=int, default=640, help="Working long edge after downsampling")
    parser.add_argument("--report-dir", type=Path, required=True, help="ΔE table and triptychs, not committed")
    parser.add_argument("--cache", type=Path, default=Path("/tmp/rawstock-fit"))
    parser.add_argument("--max-iter", type=int, default=40)
    parser.add_argument("--samples-per-image", type=int, default=400)
    parser.add_argument("--seed", type=int, default=0)
    args = parser.parse_args()
    FitSession.run(
        FitConfig(
            refs=args.refs,
            output=args.output,
            report_dir=args.report_dir,
            name=args.name,
            simulation=args.simulation,
            binary=args.binary,
            long_edge=args.long_edge,
            cache=args.cache,
            max_iter=args.max_iter,
            seed=args.seed,
            samples_per_image=args.samples_per_image,
        )
    )


if __name__ == "__main__":
    main()
