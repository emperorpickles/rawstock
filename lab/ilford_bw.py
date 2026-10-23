#!/usr/bin/env python3
"""Write a black and white (monochrome) Ilford stock from its datasheet charts.

Ilford publishes its film curves only as figures: a wedge spectrogram to
tungsten light (2850 K) and a characteristic curve for one developer. The
``FILMS`` records below hold readings of those two figures, traced from the
chart images in the datasheet PDF and calibrated to their tick marks and
grid lines, then rounded to the precision a figure supports. HP5 Plus was
read from the November 2018 HP5 PLUS technical information sheet.

Two steps turn the readings into a stock, and both are assumptions the
figures do not state:

- **Spectrogram.** A wedge spectrogram's height is the log of the
  exposure that reaches threshold, so the plotted 0 to 1 scale is read as
  ``WEDGE_DECADES`` of log sensitivity. The source was tungsten at 2850 K,
  so its Planck spectrum is divided out to give an equal-energy
  sensitivity. Outside the traced ends the sensitivity is floored three
  decades below its peak.
- **Placement.** The curve's relative log exposure is placed so the ISO
  speed point, 0.10 above base plus fog, falls at ``log10(0.8 / ISO)`` lux
  seconds. Density is stored net of base plus fog.

Grain and halation are an **approximate** aesthetic tuning. Ilford
publishes no granularity for these films.

FP4 Plus, Delta 100/400/3200, and Pan F Plus fit the same ``Film`` record:
read their two figures the same way, add an entry, then run the script with
its key.

The print is a measured black and white paper curve: one grade of the Kodak
Professional Polymax Fine-Art paper module in Jan Lohse, spectral_film_lut
(https://github.com/JanLohse/spectral_film_lut, MIT, copyright 2026 Jan
Lohse). The module is read as source text, so the package does not have to
be installed. The curve is stored from the paper white (D-min subtracted).

- ``exposure_ev`` puts rawstock's metered camera middle gray (scene-linear
  about 0.08 for a Fuji RAF, not 0.18) on ``log10(12.5 / ISO)``, the ISO
  middle-gray log exposure spectral_film_lut uses.
- ``print_exposure`` prints that gray at an 18 % reflector's density.

Sensitivity is evaluated at 10 nm on 380-780 nm and scaled so the
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

# Log sensitivity spanned by the spectrogram's 0 to 1 scale. Read as the
# usual 3.0 density spectrograph wedge, so threshold meets the floor.
WEDGE_DECADES = 3.0

# Second radiation constant, nm kelvin, for the tungsten source.
PLANCK_C2 = 1.4388e7

# Net density of the ISO speed point above base plus fog.
SPEED_POINT_DENSITY = 0.10

# Density of an 18 % reflector, the print aim for middle gray.
GRAY_PRINT_DENSITY = -math.log10(0.18)

# Scene-linear middle gray of a metered camera RAW after rawstock's decode.
CAMERA_MIDDLE_GRAY = 0.08


@dataclass(frozen=True)
class Film:
    name: str
    iso: int
    datasheet: str
    # Wedge spectrogram: (nm, height on the chart's 0 to 1 scale).
    spectrogram: list[tuple[float, float]]
    # Spectrogram light source, kelvin.
    source_kelvin: float
    # Characteristic curve: (relative log exposure, gross density).
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
        datasheet="HP5 PLUS technical information, Nov 2018",
        # Rising blue edge from 356 nm, a flat top near 460 nm, a dip at
        # 505 nm, the green peak at 575 nm, a shoulder to 630 nm, and the red
        # edge at 655 nm. The ends were read along rows, the rest along
        # columns.
        spectrogram=[
            (356, 0.0),
            (360, 0.11),
            (365, 0.20),
            (370, 0.29),
            (375, 0.35),
            (380, 0.43),
            (385, 0.51),
            (390, 0.58),
            (395, 0.63),
            (400, 0.68),
            (405, 0.71),
            (410, 0.73),
            (415, 0.75),
            (420, 0.76),
            (430, 0.79),
            (440, 0.81),
            (450, 0.82),
            (460, 0.825),
            (470, 0.825),
            (480, 0.815),
            (490, 0.80),
            (500, 0.78),
            (505, 0.78),
            (510, 0.79),
            (520, 0.82),
            (530, 0.86),
            (540, 0.90),
            (550, 0.935),
            (560, 0.965),
            (570, 0.978),
            (575, 0.982),
            (580, 0.974),
            (590, 0.943),
            (600, 0.912),
            (610, 0.908),
            (620, 0.93),
            (625, 0.934),
            (630, 0.912),
            (633, 0.842),
            (636, 0.737),
            (640, 0.526),
            (643, 0.421),
            (646, 0.316),
            (650, 0.158),
            (654, 0.053),
            (656, 0.0),
        ],
        source_kelvin=2850.0,
        # ILFOTEC HC (1+31), 6.5 min at 20 C. Base plus fog 0.18, a short
        # toe, and a straight line at gradient 0.64 to the end of the chart.
        curve=[
            (0.0, 0.18),
            (0.5, 0.18),
            (0.7, 0.195),
            (0.8, 0.21),
            (0.9, 0.23),
            (1.0, 0.25),
            (1.1, 0.28),
            (1.2, 0.32),
            (1.3, 0.36),
            (1.4, 0.40),
            (1.5, 0.46),
            (1.6, 0.52),
            (1.7, 0.58),
            (1.8, 0.65),
            (1.9, 0.72),
            (2.0, 0.79),
            (2.2, 0.91),
            (2.4, 1.05),
            (2.6, 1.18),
            (2.8, 1.31),
            (3.0, 1.43),
            (3.2, 1.56),
            (3.4, 1.68),
            (3.6, 1.81),
            (3.8, 1.94),
            (4.0, 2.06),
            (4.1, 2.12),
        ],
        developer="ILFOTEC HC 1+31, 6.5 min at 20 C",
        paper_grade=1,
        # Approximate. Silver granularity rises with density. HP5 Plus is a
        # traditional cubic-grain 400 film, close to Tri-X in grain. Tuned by
        # eye on 100% exports of a 6160 px frame, where a pixel reads about
        # 1.5 times the 48 um figure for 6 um clumps.
        grain=[
            (0.0, 5.3),
            (0.3, 7.2),
            (0.7, 9.2),
            (1.1, 9.9),
            (1.5, 9.9),
            (2.0, 9.2),
        ],
        grain_size_mm=0.006,
        seed=5,
        # Approximate. The 35 mm base has an anti-halation dye that clears in
        # processing, so only a faint glow.
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


def planck(wavelength_nm: float, kelvin: float) -> float:
    """Relative spectral radiance of a black body."""
    return wavelength_nm**-5 / math.expm1(PLANCK_C2 / (wavelength_nm * kelvin))


def sensitivity(film: Film) -> list[float]:
    """Equal-energy sensitivity from the spectrogram, integral 1."""
    first, last = film.spectrogram[0][0], film.spectrogram[-1][0]
    log_values = []
    for wave in GRID:
        if first <= wave <= last:
            height = interpolate(film.spectrogram, wave)
            log_values.append(
                WEDGE_DECADES * height - math.log10(planck(wave, film.source_kelvin))
            )
        else:
            log_values.append(-math.inf)
    floor = max(log_values) - FLOOR_DECADES
    linear = [10 ** max(value, floor) for value in log_values]
    area = trapezoid(GRID, linear)
    return [value / area for value in linear]


def net_curve(film: Film) -> list[tuple[float, float]]:
    """Net density versus log10 lux seconds, speed point on log10(0.8 / ISO)."""
    base = film.curve[0][1]
    net = [(log_e, density - base) for log_e, density in film.curve]
    shift = math.log10(0.8 / film.iso) - inverse(net, SPEED_POINT_DENSITY)
    return [(round(log_e + shift, 4), round(density, 4)) for log_e, density in net]


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
    curve = net_curve(film)
    gray_log_h = math.log10(12.5 / film.iso)
    exposure_ev = math.log2(10**gray_log_h / CAMERA_MIDDLE_GRAY)
    gray_density = clamped(curve, gray_log_h)
    print_exposure = inverse(paper, GRAY_PRINT_DENSITY) + gray_density
    silver = sensitivity(film)
    lines = [
        f"# {film.name}, schema 1 monochrome spectral stock.",
        "# Written by lab/ilford_bw.py. The sensitivity and characteristic",
        f"# curve are digitized from the figures in Ilford's {film.datasheet}:",
        f"# the wedge spectrogram to tungsten ({film.source_kelvin:.0f} K) and the",
        f"# {film.developer} curve. Reading the spectrogram as",
        f"# {WEDGE_DECADES:g} decades of log sensitivity and placing the curve by ISO",
        "# speed are assumptions; see the script. Grain and halation are an",
        "# approximate aesthetic tuning.",
        "#",
        f"# The print is Kodak Polymax Fine-Art paper, grade {grade}, measured,",
        "# from the MIT-licensed KODAK_POLYMAX module in Jan Lohse,",
        "# spectral_film_lut (https://github.com/JanLohse/spectral_film_lut).",
        "",
        "schema_version = 1",
        "revision = 3",
        f'name = "{film.name}"',
        (
            f'source = "Digitized from the figures in Ilford {film.datasheet} '
            f"(wedge spectrogram to tungsten {film.source_kelvin:.0f} K, characteristic "
            f"curve in {film.developer}). The spectrogram is read as {WEDGE_DECADES:g} "
            "decades of log sensitivity with the tungsten source divided out, and the "
            "curve is placed by ISO speed: approximate. Grain and halation are an "
            "approximate aesthetic tuning. Print curve: Kodak Professional Polymax "
            f'Fine-Art paper grade {grade} from spectral_film_lut (MIT, Jan Lohse)."'
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
        "# The datasheet spectrogram as equal-energy sensitivity, integral 1.",
        "[sensitivity]",
        f"silver = {array(silver)}",
        "",
        "# The datasheet curve, net of base + fog. The ISO speed point",
        f"# (net {SPEED_POINT_DENSITY:.2f}) is placed at log10(0.8 / ISO) lux seconds.",
        "[silver]",
        f"log_exposure = {array([x for x, _ in curve])}",
        f"density = {array([y for _, y in curve])}",
        "",
        f"# Kodak Polymax Fine-Art, grade {grade}, from the paper white.",
        "[print]",
        f"log_exposure = {array([x for x, _ in paper])}",
        f"density = {array([y for _, y in paper])}",
        "",
        "# Approximate aesthetic tuning, not a published granularity table.",
        "# Silver grain: one layer of hard-edged silver clumps, the same in",
        "# every channel. rms is 1000 times the density standard deviation",
        "# through a 48 um aperture, and rises with density. A 6160 px frame",
        "# reads about 1.5 times that per pixel with 6 um clumps.",
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
