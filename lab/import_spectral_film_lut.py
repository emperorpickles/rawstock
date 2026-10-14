#!/usr/bin/env python3
"""Convert a spectral_film_lut negative module into a rawstock spectral stock.

The upstream package is MIT licensed (Copyright 2026 Jan Lohse):

    https://github.com/JanLohse/spectral_film_lut/blob/main/LICENSE

This script reads one ``FilmData(...)`` module as source text. It does not
import the package, and it does not copy grain. The Portra 400 module's RMS
tables are commented as copied from Kodak 5207, and its scalar ``rms`` is
commented as guessed, so the written stock leaves ``[grain]`` and
``[halation]`` out.

Usage:
    python3 lab/import_spectral_film_lut.py \\
        path/to/spectral_film_lut/src/spectral_film_lut/negative_film/kodak_portra_400.py \\
        -o stocks/portra400.stock.toml

What is copied
    ``log_sensitivity`` and ``sensiometric_curve``, in the module's layer
    order (red, green, blue). Wavelengths are resampled onto 380–780 nm at
    10 nm, the grid ``spectral_film_lut`` uses for ``SPECTRAL_SHAPE``.

What is derived
    The module has no per-layer ``spectral_density``. It has the combined
    midscale curve ``d_ref_sd`` and the base ``d_min_sd`` from the E-4050
    spectral-dye-density figure. Per-layer dye curves are that difference,
    split the way ``construct_spectral_density`` in
    ``spectral_film_lut/densiometry.py`` splits it (peaks in the red, green,
    and blue bands, then a Gaussian blend). Each layer is then scaled so its
    peak is 1. That scale is not Status M; a later calibration can replace it.

    Sensitivities are ``10`` to the power of the resampled log sensitivity,
    then scaled so each layer's trapezoidal integral is 1. A flat spectrum of
    level 1 therefore exposes every layer equally, and the datasheet D-logH
    curves keep the relative layer speed.

    ``exposure_ev`` places a scene-linear 0.18 on ``log10(12.5 / ISO)``, the
    middle-gray log exposure spectral_film_lut uses for an ISO speed. The
    print curve is the synthetic scan curve from ``stocks/neutral.stock.toml``,
    not a paper measurement. ``print_exposure`` is chosen so the mean
    scanned dye density at that log exposure sits on the same print
    log-exposure the neutral stock uses for a 0.18 gray (0.2).
"""

from __future__ import annotations

import argparse
import ast
import math
import re
from pathlib import Path

WAVELENGTH_MIN = 380
WAVELENGTH_MAX = 780
WAVELENGTH_STEP = 10
# spectral_film_lut construct_spectral_density defaults.
DYE_SIGMA_NM = 25.0
# Neutral stock's print stage. 0.2 is print log-exposure of its 0.18 gray.
NEUTRAL_PRINT_LOG = 0.2
PRINT_LOG_EXPOSURE = [-1.5, -0.5, 0.2, 1.0, 2.0]
PRINT_DENSITY = [0.04, 0.20, 0.70, 1.50, 2.20]
SCENE_GRAY = 0.18


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("module", type=Path, help="path to a spectral_film_lut FilmData module")
    parser.add_argument("-o", "--output", type=Path, required=True, help="stock TOML to write")
    args = parser.parse_args()
    film = load_film_data(args.module)
    text = render_stock(film)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(text)
    print(f"wrote {args.output}")


def load_film_data(path: Path) -> dict:
    tree = ast.parse(path.read_text())
    for node in ast.walk(tree):
        if isinstance(node, ast.Call) and getattr(node.func, "id", None) == "FilmData":
            return {keyword.arg: ast.literal_eval(keyword.value) for keyword in node.keywords}
    raise SystemExit(f"{path} has no FilmData(...) call")


def render_stock(film: dict) -> str:
    if film.get("name") != "Kodak Portra 400":
        raise SystemExit(f"this importer currently knows Kodak Portra 400, not {film.get('name')!r}")
    grid = list(range(WAVELENGTH_MIN, WAVELENGTH_MAX + 1, WAVELENGTH_STEP))
    sensitivities = []
    for layer in film["log_sensitivity"]:
        log_samples = resample_log(layer, grid)
        linear = [10.0**value for value in log_samples]
        sensitivities.append(normalize(grid, linear))
    curves = [sorted(layer.items()) for layer in film["sensiometric_curve"]]
    for curve in curves:
        logs = [point[0] for point in curve]
        densities = [point[1] for point in curve]
        if any(right <= left for left, right in zip(logs, logs[1:])):
            raise SystemExit("characteristic log exposure does not strictly increase")
        if any(right < left for left, right in zip(densities, densities[1:])):
            raise SystemExit("characteristic density decreases")
    dyes = split_dyes(grid, film["d_ref_sd"], film["d_min_sd"])
    iso = int(film["iso"])
    log_h_ref = math.log10(12.5 / iso)
    exposure_ev = math.log2((10.0**log_h_ref) / SCENE_GRAY)
    ref_density = [interp(log_h_ref, curve) for curve in curves]
    # Status-M density is not the density the print curve sees. The scan
    # integrates the dye spectra, the same way FilmModel::Spectral does, and
    # print_exposure places that mean on the neutral stock's 0.2 step.
    effective = scanned_density(grid, dyes, ref_density)
    print_exposure = sum(effective) / len(effective) + NEUTRAL_PRINT_LOG

    print(f"log H ref {log_h_ref:.4f}  exposure_ev {exposure_ev:.4f}")
    print("density at ref", " ".join(f"{value:.3f}" for value in ref_density))
    print("scanned density", " ".join(f"{value:.3f}" for value in effective))
    print(f"print_exposure {print_exposure:.4f}")
    for index, layer in enumerate(sensitivities):
        peak = max(range(len(layer)), key=layer.__getitem__)
        print(
            f"sensitivity {index} integral {trapezoid(grid, layer):.6f} "
            f"peak {grid[peak]} nm {layer[peak]:.5f} min {min(layer):.5f}"
        )
    for index, layer in enumerate(dyes):
        peak = max(range(len(layer)), key=layer.__getitem__)
        print(f"dye {index} peak {grid[peak]} nm {layer[peak]:.4f} min {min(layer):.4f}")

    lines = [
        "# Kodak Portra 400, schema 1 spectral stock.",
        "# Generated by lab/import_spectral_film_lut.py from the MIT-licensed",
        "# KODAK_PORTRA_400 module in Jan Lohse, spectral_film_lut",
        "# (https://github.com/JanLohse/spectral_film_lut). The curves were",
        "# digitized from Kodak publication E-4050. Grain and halation are",
        "# omitted: that module's RMS tables are copied from Kodak 5207 and its",
        "# scalar rms is marked guessed. The print curve is the synthetic scan",
        "# curve shared with stocks/neutral.stock.toml, not a paper measurement.",
        "# Dye curves are the importer's split of (midscale spectral density",
        "# minus D-min), peak-normalized. They are not separately traced dyes.",
        "",
        "schema_version = 1",
        "revision = 1",
        'name = "Kodak Portra 400"',
        'source = "spectral_film_lut, digitized from Kodak publication E-4050 (KODAK PROFESSIONAL PORTRA 400 Film)."',
        'license = "MIT"',
        'model = "spectral"',
        "",
        "# Stops that place scene-linear 0.18 on log10(12.5 / ISO), the",
        "# middle-gray log exposure spectral_film_lut uses for this ISO.",
        f"exposure_ev = {exposure_ev:.6f}",
        "# Synthetic scan curve. print_exposure puts the mean scanned dye",
        "# density at the reference log exposure on print log-exposure 0.2.",
        f"print_exposure = {print_exposure:.6f}",
        "# Log lux-seconds of that middle gray. The spectral print balances",
        "# this exposure to neutral. It is not a chart-scan white balance.",
        f"reference_log_exposure = {log_h_ref:.6f}",
        "",
        "wavelength_nm = [" + ", ".join(str(wave) for wave in grid) + "]",
        "",
        "[sensitivity]",
        f"red = {format_samples(sensitivities[0])}",
        f"green = {format_samples(sensitivities[1])}",
        f"blue = {format_samples(sensitivities[2])}",
        "",
        "[dye_density]",
        f"red = {format_samples(dyes[0])}",
        f"green = {format_samples(dyes[1])}",
        f"blue = {format_samples(dyes[2])}",
        "",
    ]
    names = ("red", "green", "blue")
    for name, curve in zip(names, curves):
        lines.append(f"[{name}]")
        lines.append("log_exposure = " + format_samples([point[0] for point in curve]))
        lines.append("density = " + format_samples([point[1] for point in curve]))
        lines.append("")
    lines.append("[print]")
    lines.append("log_exposure = " + format_samples(PRINT_LOG_EXPOSURE))
    lines.append("density = " + format_samples(PRINT_DENSITY))
    lines.append("")
    return "\n".join(lines)


def resample_log(samples: dict[float, float], grid: list[int]) -> list[float]:
    """Linear resample of log sensitivity, clamped outside the measured range.

    Inside the digitized trace, this is linear interpolation. Outside it, the
    end segment is extended and then clamped to the measured min and max, so
    a steep edge cannot run away and a gap cannot invent a peak.
    """
    points = sorted(samples.items())
    lows = [point[1] for point in points]
    floor = min(lows)
    ceiling = max(lows)
    resampled = []
    for wave in grid:
        resampled.append(min(ceiling, max(floor, extrapolate(points, float(wave)))))
    return resampled


def extrapolate(points: list[tuple[float, float]], wave: float) -> float:
    if wave <= points[0][0]:
        return project(points[0], points[1], wave)
    if wave >= points[-1][0]:
        return project(points[-2], points[-1], wave)
    for (left_x, left_y), (right_x, right_y) in zip(points, points[1:]):
        if left_x <= wave <= right_x:
            weight = (wave - left_x) / (right_x - left_x)
            return left_y + weight * (right_y - left_y)
    return points[-1][1]


def project(left: tuple[float, float], right: tuple[float, float], wave: float) -> float:
    span = right[0] - left[0]
    if span == 0.0:
        return left[1]
    slope = (right[1] - left[1]) / span
    return left[1] + slope * (wave - left[0])


def split_dyes(grid: list[int], reference: dict[float, float], minimum: dict[float, float]) -> list[list[float]]:
    """Split midscale-minus-D-min into red, green, and blue dye shapes.

    This follows ``construct_spectral_density``: the red, green, and blue
    peaks of the residual, the minima between those peaks, and a Gaussian of
    25 nm that softens the hard band edges. Samples outside either digitized
    trace are zero. Each dye is then divided by its peak.
    """
    residual = []
    for wave in grid:
        value = interp_map(float(wave), reference) - interp_map(float(wave), minimum)
        residual.append(max(0.0, value))
    waves = [float(wave) for wave in grid]

    def argmax(low: float, high: float) -> float:
        window = [index for index, wave in enumerate(waves) if low <= wave <= high]
        if not window:
            raise SystemExit(f"no dye samples between {low} and {high} nm")
        return waves[max(window, key=lambda index: residual[index])]

    def argmin(low: float, high: float) -> float:
        window = [index for index, wave in enumerate(waves) if low <= wave <= high]
        return waves[min(window, key=lambda index: residual[index])]

    red_peak = argmax(600.0, min(750.0, waves[-1]))
    green_peak = argmax(500.0, 600.0)
    blue_peak = argmax(max(400.0, waves[0]), 500.0)
    bg_cut = argmin(blue_peak, green_peak)
    gr_cut = argmin(green_peak, red_peak)
    bands = [
        [1.0 if wave >= gr_cut else 0.0 for wave in waves],
        [1.0 if bg_cut < wave < gr_cut else 0.0 for wave in waves],
        [1.0 if wave <= bg_cut else 0.0 for wave in waves],
    ]
    sigma = DYE_SIGMA_NM / WAVELENGTH_STEP
    bands = [smooth(band, sigma) for band in bands]
    dyes = []
    for band in bands:
        shaped = [weight * value for weight, value in zip(band, residual)]
        peak = max(shaped)
        if peak <= 0.0:
            raise SystemExit("dye split produced an empty layer")
        dyes.append([value / peak for value in shaped])
    return dyes


def interp_map(wave: float, samples: dict[float, float]) -> float:
    points = sorted(samples.items())
    if wave < points[0][0] or wave > points[-1][0]:
        return 0.0
    return extrapolate(points, wave)


def interp(wave: float, points: list[tuple[float, float]]) -> float:
    if wave <= points[0][0]:
        return points[0][1]
    if wave >= points[-1][0]:
        return points[-1][1]
    for (left_x, left_y), (right_x, right_y) in zip(points, points[1:]):
        if left_x <= wave <= right_x:
            weight = (wave - left_x) / (right_x - left_x)
            return left_y + weight * (right_y - left_y)
    return points[-1][1]


def smooth(values: list[float], sigma: float) -> list[float]:
    radius = max(1, math.ceil(3.0 * sigma))
    kernel = [math.exp(-0.5 * (offset / sigma) ** 2) for offset in range(-radius, radius + 1)]
    total = sum(kernel)
    kernel = [weight / total for weight in kernel]
    last = len(values) - 1
    smoothed = []
    for index in range(len(values)):
        acc = 0.0
        for offset, weight in enumerate(kernel):
            sample = index + offset - radius
            if sample < 0:
                sample = -sample
            elif sample > last:
                sample = last - (sample - last)
            sample = min(max(sample, 0), last)
            acc += weight * values[sample]
        smoothed.append(acc)
    return smoothed


def scanned_density(grid: list[int], dyes: list[list[float]], density: list[float]) -> list[float]:
    """ACEScg density of the dye sandwich, matching ``Observer::acescg_d65``.

    The colour-matching functions and D65 are the tables in
    ``crates/rawstock-core/src/cie1931.rs``. The matrix is the same
    Bradford D65 → ACES, then AP1, construction as ``xyz_d65_to_acescg``.
    """
    rows = acescg_rows(grid)
    transmittance = []
    for index in range(len(grid)):
        optical = sum(dyes[channel][index] * density[channel] for channel in range(3))
        transmittance.append(10.0 ** (-optical))
    rgb = [sum(row[index] * sample for index, sample in enumerate(transmittance)) for row in rows]
    return [-math.log10(max(channel, 1.0e-6)) for channel in rgb]


def acescg_rows(grid: list[int]) -> list[list[float]]:
    cie_x, cie_y, cie_z, cie_d65 = cie_tables()
    matrix = xyz_d65_to_acescg()
    weighted = []
    white = [0.0, 0.0, 0.0]
    for index, wave in enumerate(grid):
        left = 0.0 if index == 0 else float(wave - grid[index - 1])
        right = 0.0 if index + 1 == len(grid) else float(grid[index + 1] - wave)
        weight = 0.5 * (left + right)
        cmf = [cie_at(table, float(wave)) for table in (cie_x, cie_y, cie_z)]
        light = cie_at(cie_d65, float(wave))
        sample = [channel * light * weight for channel in cmf]
        for channel in range(3):
            white[channel] += sample[channel]
        weighted.append(sample)
    white_rgb = matvec(matrix, white)
    rows = [[], [], []]
    for sample in weighted:
        rgb = matvec(matrix, sample)
        for channel in range(3):
            rows[channel].append(rgb[channel] / white_rgb[channel])
    return rows


def cie_at(table: list[float], wavelength: float) -> float:
    scaled = (wavelength - 360.0) / (830.0 - 360.0) * 94.0
    scaled = min(94.0, max(0.0, scaled))
    index = min(int(scaled), 93)
    weight = scaled - index
    return table[index] * (1.0 - weight) + table[index + 1] * weight


def cie_tables() -> tuple[list[float], list[float], list[float], list[float]]:
    path = Path(__file__).resolve().parents[1] / "crates" / "rawstock-core" / "src" / "cie1931.rs"
    text = path.read_text()

    def load(name: str) -> list[float]:
        match = re.search(rf"static {name}: \[f32; 95\] = \[(.*?)\];", text, re.S)
        if match is None:
            raise SystemExit(f"{path} has no {name} table")
        values = [float(token) for token in re.findall(r"[-+]?(?:\d+\.\d*|\d*\.\d+|\d+)(?:[eE][-+]?\d+)?", match.group(1))]
        if len(values) != 95:
            raise SystemExit(f"{name} has {len(values)} samples, expected 95")
        return values

    return load("CIE_X"), load("CIE_Y"), load("CIE_Z"), load("CIE_D65")


def xyz_d65_to_acescg() -> list[list[float]]:
    """Match ``matrices::xyz_d65_to_acescg``: AP1 after Bradford D65 → ACES."""
    bradford = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ]
    d65 = xy_to_xyz((0.3127, 0.3290))
    aces = xy_to_xyz((0.32168, 0.33767))
    ap1 = [(0.713, 0.293), (0.165, 0.830), (0.128, 0.044)]
    adapted = bradford_adapt(bradford, d65, aces)
    return matmul(xyz_to_rgb(ap1, aces), adapted)


def xy_to_xyz(xy: tuple[float, float]) -> list[float]:
    x, y = xy
    return [x / y, 1.0, (1.0 - x - y) / y]


def bradford_adapt(bradford: list[list[float]], source: list[float], destination: list[float]) -> list[list[float]]:
    source_lms = matvec(bradford, source)
    destination_lms = matvec(bradford, destination)
    scale = [
        [destination_lms[0] / source_lms[0], 0.0, 0.0],
        [0.0, destination_lms[1] / source_lms[1], 0.0],
        [0.0, 0.0, destination_lms[2] / source_lms[2]],
    ]
    return matmul(invert(bradford), matmul(scale, bradford))


def xyz_to_rgb(primaries_xy: list[tuple[float, float]], white: list[float]) -> list[list[float]]:
    columns = [xy_to_xyz(primary) for primary in primaries_xy]
    primaries = [[columns[col][row] for col in range(3)] for row in range(3)]
    scales = matvec(invert(primaries), white)
    rgb_to_xyz = [[primaries[row][col] * scales[col] for col in range(3)] for row in range(3)]
    return invert(rgb_to_xyz)


def matvec(matrix: list[list[float]], value: list[float]) -> list[float]:
    return [sum(matrix[row][col] * value[col] for col in range(3)) for row in range(3)]


def matmul(left: list[list[float]], right: list[list[float]]) -> list[list[float]]:
    return [
        [sum(left[row][k] * right[k][col] for k in range(3)) for col in range(3)]
        for row in range(3)
    ]


def invert(matrix: list[list[float]]) -> list[list[float]]:
    a, b, c = matrix[0]
    d, e, f = matrix[1]
    g, h, i = matrix[2]
    det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g)
    if det == 0.0:
        raise SystemExit("colour matrix is singular")
    inv = 1.0 / det
    return [
        [(e * i - f * h) * inv, (c * h - b * i) * inv, (b * f - c * e) * inv],
        [(f * g - d * i) * inv, (a * i - c * g) * inv, (c * d - a * f) * inv],
        [(d * h - e * g) * inv, (b * g - a * h) * inv, (a * e - b * d) * inv],
    ]


def trapezoid(grid: list[int], values: list[float]) -> float:
    total = 0.0
    for index in range(len(grid) - 1):
        half = 0.5 * (grid[index + 1] - grid[index])
        total += half * (values[index] + values[index + 1])
    return total


def normalize(grid: list[int], values: list[float]) -> list[float]:
    total = trapezoid(grid, values)
    if total <= 0.0:
        raise SystemExit("sensitivity integral is not positive")
    return [value / total for value in values]


def format_samples(values: list[float]) -> str:
    parts = [format_number(value) for value in values]
    return "[" + ", ".join(parts) + "]"


def format_number(value: float) -> str:
    text = f"{value:.6f}".rstrip("0").rstrip(".")
    if text == "-0":
        return "0"
    return text


if __name__ == "__main__":
    main()
