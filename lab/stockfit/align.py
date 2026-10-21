"""Align a developed frame to a camera JPEG.

Fuji JPEGs are oriented, slightly cropped, and lens-corrected relative to
the sensor dump. The placement is a uniform scale plus a translation,
estimated by normalized cross-correlation on high-pass luminance.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np
from scipy.ndimage import uniform_filter, zoom
from scipy.signal import fftconvolve


@dataclass(frozen=True)
class Placement:
    """Crop of `zoom(source, scale)` that lines up with `reference`."""

    scale: float
    origin_y: int
    origin_x: int
    score: float


class FrameAligner:
    """Scale and translation that put a develop on top of its JPEG."""

    _SCALES = np.linspace(0.90, 1.18, 15)

    @staticmethod
    def align(
        source: np.ndarray,
        reference: np.ndarray,
    ) -> tuple[np.ndarray, np.ndarray, Placement]:
        placement = FrameAligner.estimate(source, reference)
        fitted, target = FrameAligner.apply(source, reference, placement)
        return fitted, target, placement

    @staticmethod
    def estimate(source: np.ndarray, reference: np.ndarray) -> Placement:
        source_key = FrameAligner._key(source)
        reference_key = FrameAligner._key(reference)
        coarse = FrameAligner._search(source_key, reference_key, FrameAligner._SCALES)
        fine = np.linspace(coarse.scale - 0.02, coarse.scale + 0.02, 9)
        return FrameAligner._search(source_key, reference_key, fine)

    @staticmethod
    def apply(
        source: np.ndarray,
        reference: np.ndarray,
        placement: Placement,
    ) -> tuple[np.ndarray, np.ndarray]:
        scaled = FrameAligner._zoom_image(source, placement.scale)
        ref_h, ref_w = reference.shape[:2]
        crop = scaled[
            placement.origin_y : placement.origin_y + ref_h,
            placement.origin_x : placement.origin_x + ref_w,
        ]
        height = min(crop.shape[0], ref_h)
        width = min(crop.shape[1], ref_w)
        if height < 8 or width < 8:
            raise RuntimeError(
                f"alignment crop is {width}x{height}; scale {placement.scale:.3f} "
                f"score {placement.score:.3f}"
            )
        return (
            np.ascontiguousarray(crop[:height, :width]),
            np.ascontiguousarray(reference[:height, :width]),
        )

    @staticmethod
    def _search(
        source: np.ndarray,
        reference: np.ndarray,
        scales: np.ndarray,
    ) -> Placement:
        best: Placement | None = None
        for scale in scales:
            scaled = FrameAligner._zoom_image(source, float(scale))
            if scaled.shape[0] < reference.shape[0] or scaled.shape[1] < reference.shape[1]:
                continue
            origin_y, origin_x, score = FrameAligner._correlate(scaled, reference)
            candidate = Placement(float(scale), origin_y, origin_x, score)
            if best is None or candidate.score > best.score:
                best = candidate
        if best is None:
            raise RuntimeError("no scale left the developed frame large enough to cover the JPEG")
        return best

    @staticmethod
    def _correlate(image: np.ndarray, template: np.ndarray) -> tuple[int, int, float]:
        template = template - float(template.mean())
        template_norm = float(np.sqrt(np.sum(template * template))) + 1.0e-8
        ones = np.ones_like(template)
        area = float(template.size)
        total = fftconvolve(image, ones, mode="valid")
        total_sq = fftconvolve(image * image, ones, mode="valid")
        variance = np.maximum(total_sq - (total * total) / area, 0.0)
        cross = fftconvolve(image, template[::-1, ::-1], mode="valid")
        score = cross / (np.sqrt(variance) * template_norm + 1.0e-8)
        flat = int(np.nanargmax(score))
        origin_y, origin_x = np.unravel_index(flat, score.shape)
        return int(origin_y), int(origin_x), float(score[origin_y, origin_x])

    @staticmethod
    def _key(image: np.ndarray) -> np.ndarray:
        luma = (
            0.2126 * image[:, :, 0] + 0.7152 * image[:, :, 1] + 0.0722 * image[:, :, 2]
        ).astype(np.float64)
        return luma - uniform_filter(luma, size=11)

    @staticmethod
    def _zoom_image(image: np.ndarray, scale: float) -> np.ndarray:
        if abs(scale - 1.0) < 1.0e-4:
            return image
        factors = (scale, scale) if image.ndim == 2 else (scale, scale, 1.0)
        return zoom(image, factors, order=1, prefilter=False)
