"""Parametric film model, matching `Pipeline::develop` with grain and halation off.

The matrix is the core ACEScg → linear sRGB transform: AP1 back to XYZ,
Bradford from the ACES white onto the sRGB matrix white, then the IEC
sRGB matrix. Curve evaluation matches `Curve::eval`: linear in log
exposure, clamped outside the knots, with exposures lifted to 1e-6.
"""

from __future__ import annotations

from dataclasses import dataclass

import colour
import numpy as np

_EXPOSURE_FLOOR = 1.0e-6


@dataclass
class StockParameters:
    """One schema-1 parametric stock, grain and halation omitted."""

    exposure_ev: float
    print_exposure: float
    density: np.ndarray
    print_density: np.ndarray
    coupling: np.ndarray

    def copy(self) -> StockParameters:
        return StockParameters(
            self.exposure_ev,
            self.print_exposure,
            self.density.copy(),
            self.print_density.copy(),
            self.coupling.copy(),
        )


class NeutralStock:
    """Starting point: `stocks/neutral.stock.toml`."""

    LOG_EXPOSURE_TEXT = ("-3.8", "-2.6", "-1.6", "-0.7447", "0.0", "0.5", "1.1")
    PRINT_LOG_TEXT = ("-1.6", "-0.7", "0.15", "0.85", "1.55", "2.3")
    LOG_EXPOSURE = np.array([-3.8, -2.6, -1.6, -0.7447, 0.0, 0.5, 1.1], dtype=np.float64)
    DENSITY = np.array([0.18, 0.42, 0.72, 1.10, 1.70, 2.02, 2.28], dtype=np.float64)
    PRINT_LOG = np.array([-1.6, -0.7, 0.15, 0.85, 1.55, 2.3], dtype=np.float64)
    PRINT_DENSITY = np.array([0.04, 0.12, 0.74, 1.70, 2.30, 2.55], dtype=np.float64)

    @staticmethod
    def parameters() -> StockParameters:
        return StockParameters(
            exposure_ev=0.0,
            print_exposure=1.25,
            density=np.tile(NeutralStock.DENSITY, (3, 1)),
            print_density=NeutralStock.PRINT_DENSITY.copy(),
            coupling=np.eye(3, dtype=np.float64),
        )


class WorkingSpace:
    """ACEScg to encoded sRGB, using the same constants as `matrices.rs`."""

    _BRADFORD = np.array(
        [
            [0.8951, 0.2664, -0.1614],
            [-0.7502, 1.7135, 0.0367],
            [0.0389, -0.0685, 1.0296],
        ],
        dtype=np.float64,
    )
    _ACES_WHITE_XY = np.array([0.32168, 0.33767], dtype=np.float64)
    _AP1_XY = np.array(
        [[0.713, 0.293], [0.165, 0.830], [0.128, 0.044]],
        dtype=np.float64,
    )
    _SRGB_TO_XYZ = np.array(
        [
            [0.4124564, 0.3575761, 0.1804375],
            [0.2126729, 0.7151522, 0.0721750],
            [0.0193339, 0.1191920, 0.9503041],
        ],
        dtype=np.float64,
    )
    _MATRIX: np.ndarray | None = None

    @staticmethod
    def acescg_to_linear_srgb() -> np.ndarray:
        if WorkingSpace._MATRIX is None:
            WorkingSpace._MATRIX = WorkingSpace._build_matrix()
        return WorkingSpace._MATRIX

    @staticmethod
    def _build_matrix() -> np.ndarray:
        aces_white = WorkingSpace._xy_to_xyz(WorkingSpace._ACES_WHITE_XY)
        xyz_to_ap1 = WorkingSpace._xyz_to_rgb(WorkingSpace._AP1_XY, aces_white)
        acescg_to_xyz = np.linalg.inv(xyz_to_ap1)
        srgb_white = WorkingSpace._SRGB_TO_XYZ @ np.ones(3)
        adapt = WorkingSpace._bradford(aces_white, srgb_white)
        xyz_to_srgb = np.linalg.inv(WorkingSpace._SRGB_TO_XYZ)
        return xyz_to_srgb @ adapt @ acescg_to_xyz

    @staticmethod
    def _xy_to_xyz(xy: np.ndarray) -> np.ndarray:
        x, y = float(xy[0]), float(xy[1])
        return np.array([x / y, 1.0, (1.0 - x - y) / y], dtype=np.float64)

    @staticmethod
    def _xyz_to_rgb(primaries_xy: np.ndarray, white: np.ndarray) -> np.ndarray:
        primaries = np.stack([WorkingSpace._xy_to_xyz(xy) for xy in primaries_xy], axis=1)
        scales = np.linalg.inv(primaries) @ white
        return np.linalg.inv(primaries * scales)

    @staticmethod
    def _bradford(source_xyz: np.ndarray, destination_xyz: np.ndarray) -> np.ndarray:
        source = WorkingSpace._BRADFORD @ source_xyz
        destination = WorkingSpace._BRADFORD @ destination_xyz
        scale = np.diag(destination / source)
        inverse = np.linalg.inv(WorkingSpace._BRADFORD)
        return inverse @ scale @ WorkingSpace._BRADFORD

    @staticmethod
    def encode(linear_acescg: np.ndarray) -> np.ndarray:
        """Encoded sRGB. `linear_acescg` is (..., 3)."""
        linear = np.matmul(linear_acescg, WorkingSpace.acescg_to_linear_srgb().T)
        linear = np.nan_to_num(linear, nan=0.0, posinf=1.0, neginf=0.0)
        linear = np.clip(linear, 0.0, 1.0)
        encoded = np.empty_like(linear)
        small = linear <= 0.0031308
        encoded[small] = 12.92 * linear[small]
        encoded[~small] = 1.055 * np.power(linear[~small], 1.0 / 2.4) - 0.055
        return encoded


class ParametricForward:
    """Scene-linear ACEScg to encoded sRGB for one parametric stock."""

    @staticmethod
    def render(scene: np.ndarray, parameters: StockParameters) -> np.ndarray:
        shape = scene.shape
        pixels = np.reshape(np.asarray(scene, dtype=np.float64), (-1, 3))
        exposed = pixels * np.float64(2.0 ** parameters.exposure_ev)
        log_h = np.log10(np.maximum(exposed, _EXPOSURE_FLOOR))
        mixed = log_h @ np.asarray(parameters.coupling, dtype=np.float64).T
        density = np.empty_like(mixed)
        for channel in range(3):
            density[:, channel] = np.interp(
                mixed[:, channel],
                NeutralStock.LOG_EXPOSURE,
                parameters.density[channel],
            )
        print_log = parameters.print_exposure - density
        print_density = np.interp(print_log, NeutralStock.PRINT_LOG, parameters.print_density)
        linear = np.power(10.0, -print_density)
        return WorkingSpace.encode(linear).reshape(shape)

    @staticmethod
    def roughness(parameters: StockParameters) -> float:
        """Mean square second difference of the four monotone curves."""
        curves = [parameters.density[channel] for channel in range(3)]
        curves.append(parameters.print_density)
        total = 0.0
        count = 0
        for curve in curves:
            if curve.size < 3:
                continue
            second = np.diff(curve, n=2)
            total += float(np.mean(second * second))
            count += 1
        return total / max(count, 1)


class DeltaE:
    """CIEDE2000 between encoded sRGB images."""

    @staticmethod
    def lab(encoded_srgb: np.ndarray) -> np.ndarray:
        xyz = colour.sRGB_to_XYZ(np.clip(encoded_srgb, 0.0, 1.0))
        return colour.XYZ_to_Lab(xyz)

    @staticmethod
    def ciede2000(encoded: np.ndarray, reference: np.ndarray) -> np.ndarray:
        return colour.delta_E(DeltaE.lab(encoded), DeltaE.lab(reference), method="CIE 2000")

    @staticmethod
    def ciede2000_lab(encoded: np.ndarray, reference_lab: np.ndarray) -> np.ndarray:
        return colour.delta_E(DeltaE.lab(encoded), reference_lab, method="CIE 2000")

    @staticmethod
    def median_and_p95(errors: np.ndarray) -> tuple[float, float]:
        flat = np.asarray(errors, dtype=np.float64).ravel()
        if flat.size == 0:
            raise ValueError("no ΔE samples")
        return float(np.median(flat)), float(np.percentile(flat, 95))


class ParameterCodec:
    """Unconstrained vector ↔ monotone densities and a 3×3 mix."""

    @staticmethod
    def pack(parameters: StockParameters) -> np.ndarray:
        parts = [
            np.array([parameters.exposure_ev, parameters.print_exposure], dtype=np.float64),
            ParameterCodec._density_to_raw(parameters.density[0]),
            ParameterCodec._density_to_raw(parameters.density[1]),
            ParameterCodec._density_to_raw(parameters.density[2]),
            ParameterCodec._density_to_raw(parameters.print_density),
            parameters.coupling.reshape(-1),
        ]
        return np.concatenate(parts)

    @staticmethod
    def unpack(raw: np.ndarray) -> StockParameters:
        raw = np.asarray(raw, dtype=np.float64)
        knots = NeutralStock.LOG_EXPOSURE.size
        print_knots = NeutralStock.PRINT_LOG.size
        cursor = 2
        density = np.stack(
            [
                ParameterCodec._raw_to_density(raw[cursor : cursor + knots]),
                ParameterCodec._raw_to_density(raw[cursor + knots : cursor + 2 * knots]),
                ParameterCodec._raw_to_density(raw[cursor + 2 * knots : cursor + 3 * knots]),
            ]
        )
        cursor += 3 * knots
        print_density = ParameterCodec._raw_to_density(raw[cursor : cursor + print_knots])
        cursor += print_knots
        coupling = raw[cursor : cursor + 9].reshape(3, 3)
        return StockParameters(
            exposure_ev=float(raw[0]),
            print_exposure=float(raw[1]),
            density=density,
            print_density=print_density,
            coupling=coupling,
        )

    @staticmethod
    def _density_to_raw(density: np.ndarray) -> np.ndarray:
        raw = np.empty(density.shape, dtype=np.float64)
        raw[0] = density[0]
        increments = np.maximum(np.diff(np.asarray(density, dtype=np.float64)), 1.0e-5)
        raw[1:] = ParameterCodec._softplus_inverse(increments)
        return raw

    @staticmethod
    def _raw_to_density(raw: np.ndarray) -> np.ndarray:
        increments = ParameterCodec._softplus(raw[1:])
        return np.cumsum(np.concatenate([raw[:1], increments]))

    @staticmethod
    def _softplus(value: np.ndarray) -> np.ndarray:
        return np.maximum(value, 0.0) + np.log1p(np.exp(-np.abs(value)))

    @staticmethod
    def _softplus_inverse(increment: np.ndarray) -> np.ndarray:
        return np.log(np.expm1(increment))
