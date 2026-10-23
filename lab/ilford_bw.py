#!/usr/bin/env python3
"""Write a black and white (monochrome) Ilford stock from hand-drawn knots.

Ilford publishes its film curves only as figures: a wedge spectrogram to
tungsten light and a characteristic curve for one developer. There is no
numeric table and no MIT digitization in spectral_film_lut, so the knots in
``FILMS`` below are an **approximate** hand reading of the shapes on the
datasheet, not a measurement. Every stock this script writes says so in its
``source`` and its comments.

Only HP5 Plus is drawn so far. FP4 Plus, Delta 100/400/3200, and Pan F Plus
fit the same ``Film`` record: a log10 spectral sensitivity on a few
wavelengths, a net-density D-logH curve, an ISO speed, a grain table, and a
clump size. Add an entry, then run the script with its key.

The print is a measured black and white paper curve: one grade of the Kodak
Professional Polymax Fine-Art paper module in Jan Lohse, spectral_film_lut
(https://github.com/JanLohse/spectral_film_lut, MIT, copyright 2026 Jan
Lohse). The module is read as source text, so the package does not have to
be installed. The curve is stored from the paper white (D-min subtracted).

Placement:

- ``exposure_ev`` puts rawstock's metered camera middle gray (scene-linear
  about 0.08 for a Fuji RAF, not 0.18) on ``log10(12.5 / ISO)``, the ISO
  middle-gray log exposure spectral_film_lut uses.
- ``print_exposure`` prints that gray at an 18 % reflector's density.

Sensitivity knots are interpolated in log10 at 10 nm on 380-780 nm, floored
three decades below the peak, raised to linear, and scaled so the
trapezoidal integral is 1. Uses only the Python standard library.

Usage:
    python3 lab/ilford_bw.py hp5 \\
        --paper path/to/spectral_film_lut/src/spectral_film_lut/bw_print_film/kodak_polymax_fine_art.py \\
        -o stocks/ilford-hp5-plus.stock.toml
"""

from __future__ import annotations

import argparse
import ast
import math
from dataclasses import dataclass
from pathlib import Path

GRID = list(range(380, 781, 10))

# Decades below the peak where the sensitivity is floored.
FLOOR_DECADES = 3.0

# Density of an 18 % reflector, the print aim for middle gray.
GRAY_PRINT_DENSITY = -math.log10(0.18)

# Scene-linear middle gray of a metered camera RAW after rawstock's decode.
CAMERA_MIDDLE_GRAY = 0.08


@dataclass(frozen=True)
class Film:
    name: str
    iso: int
    # log10 relative spectral sensitivity (equal energy), (nm, value) knots.
    log_sensitivity: list[tuple[float, float]]
    # Net density above base + fog versus log10 exposure (lux seconds).
    curve: list[tuple[float, float]]
    developer: str
    paper_grade: int
    # (net density, Kodak-style rms) knots, and the clump sigma in mm.
    grain: list[tuple[float, float]]
    grain_size_mm: float
    seed: int
    # Faint neutral halation: threshold, radius, weight.
    halation: tuple[float, float, float] | None


FILMS = {
    "hp5": Film(
        name="Ilford HP5 Plus",
        iso=400,
        # Panchromatic: native silver halide blue, a shallow gap near 500 nm,
        # dye-sensitized green and red, falling off steeply past 640 nm with
        # the edge near 670 nm, as on Ilford's wedge spectrogram.
        log_sensitivity=[
            (380, 2.60),
            (400, 2.68),
            (420, 2.66),
            (440, 2.60),
            (460, 2.50),
            (480, 2.37),
            (500, 2.26),
            (510, 2.25),
            (530, 2.34),
            (550, 2.43),
            (570, 2.46),
            (590, 2.43),
            (610, 2.43),
            (625, 2.41),
            (640, 2.22),
            (650, 1.90),
            (660, 1.40),
            (670, 0.80),
            (680, 0.20),
            (690, -0.40),
        ],
        # ISO speed point (net 0.10) at log H = log10(0.8 / 400) = -2.70, a
        # gentle toe, a long straight line near G-bar 0.62, and a late,
        # soft shoulder: the shape of the ILFOTEC HC 1+31 curve.
        curve=[
            (-3.60, 0.00),
            (-3.30, 0.01),
            (-3.00, 0.04),
            (-2.70, 0.10),
            (-2.40, 0.22),
            (-2.10, 0.40),
            (-1.80, 0.59),
            (-1.50, 0.78),
            (-1.20, 0.97),
            (-0.90, 1.16),
            (-0.60, 1.34),
            (-0.30, 1.52),
            (0.00, 1.68),
            (0.30, 1.83),
            (0.60, 1.95),
            (0.90, 2.04),
            (1.20, 2.10),
        ],
        developer="ILFOTEC HC 1+31, 6.5 min at 20 C",
        paper_grade=1,
        # Silver granularity rises with density. HP5 Plus is a traditional
        # cubic-grain 400 film, about as grainy as Tri-X (Kodak rms 17).
        grain=[
            (0.0, 8),
            (0.3, 11),
            (0.7, 14),
            (1.1, 15),
            (1.5, 15),
            (2.0, 14),
        ],
        grain_size_mm=0.006,
        seed=5,
        # The 35 mm base has an anti-halation dye that clears in processing,
        # so only a faint glow.
        halation=(0.25, 0.008, 0.04),
    ),
}


def interpolate(knots: list[tuple[float, float]], x: float) -> float:
    if x <= knots[0][0]:
        (x0, y0), (x1, y1) = knots[0], knots[1]
    elif x >= knots[-1][0]:
        (x0, y0), (x1, y1) = knots[-2], knots[-1]
    else:
        index = next(i for i in range(1, len(knots)) if knots[i][0] >= x)
        (x0, y0), (x1, y1) = knots[index - 1], knots[index]
    return y0 + (y1 - y0) * (x - x0) / (x1 - x0)


def clamped(knots: list[tuple[float, float]], x: float) -> float:
    if x <= knots[0][0]:
        return knots[0][1]
    if x >= knots[-1][0]:
        return knots[-1][1]
    return interpolate(knots, x)


def inverse(knots: list[tuple[float, float]], y: float) -> float:
    """x where a rising curve reaches y."""
    for (x0, y0), (x1, y1) in zip(knots, knots[1:]):
        if y0 <= y <= y1 and y1 > y0:
            return x0 + (x1 - x0) * (y - y0) / (y1 - y0)
    raise ValueError(f"density {y} is outside the curve")


def trapezoid(xs: list[float], ys: list[float]) -> float:
    return sum((xs[i + 1] - xs[i]) * (ys[i] + ys[i + 1]) / 2 for i in range(len(xs) - 1))


def sensitivity(film: Film) -> list[float]:
    # Extend the end slopes, then floor below the peak.
    log_values = [interpolate(film.log_sensitivity, wave) for wave in GRID]
    floor = max(log_values) - FLOOR_DECADES
    linear = [10 ** max(value, floor) for value in log_values]
    area = trapezoid(GRID, linear)
    return [value / area for value in linear]


def paper_curve(path: Path, grade: int) -> list[tuple[float, float]]:
    tree = ast.parse(path.read_text())
    for node in tree.body:
        if (
            isinstance(node, ast.Assign)
            and isinstance(node.targets[0], ast.Name)
            and node.targets[0].id == "curve"
        ):
            curves = ast.literal_eval(node.value)
            break
    else:
        raise SystemExit(f"{path} has no curve table")
    points = sorted(curves[grade].items())
    base = points[0][1]
    # Hold density at its running maximum: digitizing noise must not fall.
    out = []
    top = 0.0
    for log_h, density in points:
        top = max(top, density - base)
        out.append((log_h, top))
    return out


def fmt(value: float) -> str:
    text = f"{value:.6f}".rstrip("0").rstrip(".")
    return text if text not in ("", "-0") else "0"


def array(values: list[float]) -> str:
    return "[" + ", ".join(fmt(value) for value in values) + "]"


def write(film: Film, paper: list[tuple[float, float]], grade: int) -> str:
    gray_log_h = math.log10(12.5 / film.iso)
    exposure_ev = math.log2(10**gray_log_h / CAMERA_MIDDLE_GRAY)
    gray_density = clamped(film.curve, gray_log_h)
    print_exposure = inverse(paper, GRAY_PRINT_DENSITY) + gray_density
    silver = sensitivity(film)
    lines = [
        f"# {film.name}, schema 1 monochrome spectral stock.",
        "# Written by lab/ilford_bw.py. APPROXIMATE: the sensitivity, the",
        "# characteristic curve, grain, and halation are hand-drawn from the",
        "# shapes in Ilford's published datasheet figures (wedge spectrogram",
        f"# and the {film.developer} curve), not digitized from tables.",
        "# Ilford publishes no numeric tables for these.",
        "#",
        f"# The print is Kodak Polymax Fine-Art paper, grade {grade}, measured,",
        "# from the MIT-licensed KODAK_POLYMAX module in Jan Lohse,",
        "# spectral_film_lut (https://github.com/JanLohse/spectral_film_lut).",
        "",
        "schema_version = 1",
        "revision = 1",
        f'name = "{film.name}"',
        (
            f'source = "Approximate hand reading of the {film.name} datasheet figures '
            f"(wedge spectrogram to tungsten, characteristic curve in {film.developer}); "
            "not a digitized table. Grain and halation are an approximate aesthetic tuning. "
            f"Print curve: Kodak Professional Polymax Fine-Art paper grade {grade} from "
            'spectral_film_lut (MIT, Jan Lohse)."'
        ),
        'license = "MIT"',
        'model = "spectral"',
        'emulsion = "monochrome"',
        "",
        "# Stops that place the camera's metered middle gray (scene-linear",
        f"# about {CAMERA_MIDDLE_GRAY}) on log10(12.5 / ISO).",
        f"exposure_ev = {fmt(exposure_ev)}",
        "# Prints that middle gray at an 18 % reflector's density.",
        f"print_exposure = {fmt(print_exposure)}",
        "",
        "wavelength_nm = [" + ", ".join(str(wave) for wave in GRID) + "]",
        "",
        "# Approximate. One panchromatic silver layer, equal-energy, integral 1.",
        "[sensitivity]",
        f"silver = {array(silver)}",
        "",
        "# Approximate. Net density above base + fog. The ISO speed point",
        "# (net 0.10) sits at log10(0.8 / ISO).",
        "[silver]",
        f"log_exposure = {array([x for x, _ in film.curve])}",
        f"density = {array([y for _, y in film.curve])}",
        "",
        f"# Kodak Polymax Fine-Art, grade {grade}, from the paper white.",
        "[print]",
        f"log_exposure = {array([x for x, _ in paper])}",
        f"density = {array([y for _, y in paper])}",
        "",
        "# Approximate aesthetic tuning, not a published granularity table.",
        "# Silver grain: one layer, the same clouds in every channel, coarser",
        "# than the color stocks. rms is 1000 times the density standard",
        "# deviation through a 48 um aperture, and rises with density.",
        "[grain]",
        "frame_mm = 35",
        f"seed = {film.seed}",
        "[grain.silver]",
        f"density = {array([x for x, _ in film.grain])}",
        f"rms = {array([y for _, y in film.grain])}",
        f"size_mm = {fmt(film.grain_size_mm)}",
    ]
    if film.halation is not None:
        threshold, radius, weight = film.halation
        lines += [
            "",
            "# Approximate. A faint neutral glow: the anti-halation dye clears in",
            "# processing. The film integrates all three channels, so equal",
            "# weights keep it gray.",
            "[halation]",
            f"threshold = {fmt(threshold)}",
            f"radius = {fmt(radius)}",
            f"weight = {array([weight] * 3)}",
        ]
    integral = trapezoid(GRID, silver)
    print(f"{film.name}: exposure_ev {exposure_ev:.4f}, middle gray density {gray_density:.3f}")
    print(f"print_exposure {print_exposure:.4f}, sensitivity integral {integral:.6f}")
    return "\n".join(lines) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("film", choices=sorted(FILMS))
    parser.add_argument("--paper", type=Path, required=True, help="kodak_polymax_fine_art.py")
    parser.add_argument("--grade", type=int, help="paper grade (default: the film's)")
    parser.add_argument("-o", "--output", type=Path, required=True)
    args = parser.parse_args()
    film = FILMS[args.film]
    grade = film.paper_grade if args.grade is None else args.grade
    args.output.write_text(write(film, paper_curve(args.paper, grade), grade))


if __name__ == "__main__":
    main()
