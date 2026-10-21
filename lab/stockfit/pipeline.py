"""Call the rawstock binary and honor EXIF orientation."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import numpy as np
import OpenEXR
from PIL import Image, ImageOps


class SensorOrientation:
    """The same flips rawstock applies at export, keyed by `rawstock inspect`."""

    # (transpose, flip horizontal, flip vertical). Flips happen first.
    _FLIPS = {
        "normal": (False, False, False),
        "unknown": (False, False, False),
        "flip vertical": (False, False, True),
        "flip horizontal": (False, True, False),
        "rotate 180": (False, True, True),
        "transpose": (True, False, False),
        "rotate 90 CW": (True, False, True),
        "rotate 90 CCW": (True, True, False),
        "transverse": (True, True, True),
    }

    @staticmethod
    def from_inspect(text: str) -> str:
        for line in text.splitlines():
            if line.startswith("orientation:"):
                name = line.split(":", 1)[1].strip()
                if name not in SensorOrientation._FLIPS:
                    raise ValueError(f"unrecognized orientation {name!r}")
                return name
        raise ValueError("inspect output has no orientation")

    @staticmethod
    def apply(image: np.ndarray, name: str) -> np.ndarray:
        transpose, flip_horizontal, flip_vertical = SensorOrientation._FLIPS[name]
        out = image
        if flip_horizontal:
            out = out[:, ::-1]
        if flip_vertical:
            out = out[::-1]
        if transpose:
            out = np.swapaxes(out, 0, 1)
        return np.ascontiguousarray(out)


class LinearDump:
    """Scene-linear ACEScg OpenEXR, the same files `dump-linear` writes."""

    @staticmethod
    def read(path: Path) -> np.ndarray:
        from read_linear import read_acescg_exr

        return read_acescg_exr(path)

    @staticmethod
    def write(path: Path, image: np.ndarray) -> None:
        pixels = np.ascontiguousarray(image, dtype=np.float32)
        header = {"compression": OpenEXR.ZIP_COMPRESSION}
        OpenEXR.File(
            header,
            {
                "R": np.ascontiguousarray(pixels[:, :, 0]),
                "G": np.ascontiguousarray(pixels[:, :, 1]),
                "B": np.ascontiguousarray(pixels[:, :, 2]),
            },
        ).write(str(path))


class DisplayOrientation:
    """Which way the camera asked for the picture to be shown.

    `rawstock inspect` reads the RAF tag. On some X-T2 frames that tag is
    missing while the paired JPEG is Rotate 270 CW. The JPEG tag is the one
    the manifest lists, so a non-identity JPEG orientation wins. `develop`
    only bakes the RAF tag, so a render still has to be turned when the two
    disagree.
    """

    @staticmethod
    def choose(raw_name: str, jpeg_name: str) -> str:
        if jpeg_name not in ("normal", "unknown"):
            return jpeg_name
        return raw_name

    @staticmethod
    def render_needs_turn(raw_name: str, display_name: str) -> bool:
        baked = "normal" if raw_name == "unknown" else raw_name
        return baked != display_name


class JpegFrame:
    """Camera or developed JPEG, upright, as float RGB in 0..1."""

    _EXIF = {
        1: "normal",
        2: "flip horizontal",
        3: "rotate 180",
        4: "flip vertical",
        5: "transpose",
        6: "rotate 90 CW",
        7: "transverse",
        8: "rotate 90 CCW",
    }

    @staticmethod
    def orientation_name(path: Path) -> str:
        with Image.open(path) as image:
            value = int(image.getexif().get(274, 1))
        return JpegFrame._EXIF.get(value, "unknown")

    @staticmethod
    def load_upright(path: Path) -> np.ndarray:
        with Image.open(path) as image:
            upright = ImageOps.exif_transpose(image).convert("RGB")
            pixels = np.asarray(upright)
        if pixels.dtype == np.uint8:
            return pixels.astype(np.float32) / 255.0
        if pixels.dtype == np.uint16:
            return pixels.astype(np.float32) / 65535.0
        raise ValueError(f"{path} is {pixels.dtype}, expected 8-bit or 16-bit RGB")

    @staticmethod
    def downsample(image: np.ndarray, long_edge: int) -> np.ndarray:
        height, width = image.shape[:2]
        out_w, out_h = JpegFrame._long_edge(width, height, long_edge)
        if (out_w, out_h) == (width, height):
            return np.ascontiguousarray(image, dtype=np.float32)
        uint8 = np.clip(np.rint(image * 255.0), 0, 255).astype(np.uint8)
        resized = Image.fromarray(uint8, "RGB").resize((out_w, out_h), Image.Resampling.BOX)
        return np.asarray(resized).astype(np.float32) / 255.0

    @staticmethod
    def _long_edge(width: int, height: int, long_edge: int) -> tuple[int, int]:
        if width <= 0 or height <= 0 or long_edge <= 0:
            return 0, 0
        if max(width, height) <= long_edge:
            return width, height
        if width >= height:
            short = max(1, (height * long_edge + width // 2) // width)
            return long_edge, short
        short = max(1, (width * long_edge + height // 2) // height)
        return short, long_edge


class RawstockPipeline:
    """Develop and dump through the rawstock CLI."""

    @staticmethod
    def resolve(explicit: Path | None) -> Path:
        if explicit is not None:
            binary = explicit
        elif os.environ.get("RAWSTOCK_BIN"):
            binary = Path(os.environ["RAWSTOCK_BIN"])
        else:
            binary = Path(__file__).resolve().parents[2] / "target" / "release" / "rawstock"
        if not binary.is_file():
            raise FileNotFoundError(
                f"rawstock binary not found at {binary}. Build it with "
                "`cargo build -p rawstock-cli --release` or pass --binary."
            )
        return binary

    @staticmethod
    def inspect(binary: Path, raw: Path) -> str:
        return RawstockPipeline._run(binary, ["inspect", str(raw)])

    @staticmethod
    def dump_linear(binary: Path, raw: Path, output: Path, long_edge: int) -> None:
        output.parent.mkdir(parents=True, exist_ok=True)
        RawstockPipeline._run(
            binary,
            ["dump-linear", str(raw), str(output), "--long-edge", str(long_edge)],
        )

    @staticmethod
    def develop(binary: Path, raw: Path, stock: Path, output: Path) -> None:
        output.parent.mkdir(parents=True, exist_ok=True)
        RawstockPipeline._run(
            binary,
            ["develop", str(raw), "--stock", str(stock), "--out", str(output)],
        )

    @staticmethod
    def develop_linear(binary: Path, scene: Path, stock: Path, output: Path) -> None:
        output.parent.mkdir(parents=True, exist_ok=True)
        RawstockPipeline._run(
            binary,
            ["develop-linear", str(scene), "--stock", str(stock), "--out", str(output)],
        )

    @staticmethod
    def _run(binary: Path, args: list[str]) -> str:
        completed = subprocess.run(
            [str(binary), *args],
            check=False,
            capture_output=True,
            text=True,
        )
        if completed.returncode != 0:
            raise RuntimeError(
                f"rawstock {' '.join(args)} failed\n{completed.stdout}\n{completed.stderr}"
            )
        return completed.stdout
