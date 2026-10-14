"""Read a scene-linear ACEScg OpenEXR dump from `rawstock dump-linear`."""

from __future__ import annotations

import argparse
from pathlib import Path

import numpy as np
import OpenEXR


def read_acescg_exr(path: str | Path) -> np.ndarray:
    """Return float32 RGB, shape `(height, width, 3)`, scene-linear ACEScg.

    Channel order is R, G, B. Values are scene-referred: they are not
    display-encoded and are not clipped to ``0..1``.
    """
    image = OpenEXR.File(str(path))
    pixels = np.asarray(image.channels()["RGB"].pixels)
    if pixels.ndim != 3 or pixels.shape[2] != 3:
        raise ValueError(f"{path} is not an RGB OpenEXR image (shape {pixels.shape})")
    return np.array(pixels, dtype=np.float32, copy=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", type=Path, help="ACEScg OpenEXR written by rawstock")
    args = parser.parse_args()
    image = read_acescg_exr(args.path)
    height, width, _channels = image.shape
    print(f"{width}x{height} RGB float32  min {image.min():.5f}  max {image.max():.5f}")


if __name__ == "__main__":
    main()
