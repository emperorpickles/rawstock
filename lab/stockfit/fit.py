"""Fit a parametric stock and report ΔE2000 before and after.

Training pixels come only from `ReferenceLayout.training_pairs`. Held-out
pairs are measured with the same alignment and the same pipeline, and they
are not added to the loss.
"""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont
from scipy.optimize import minimize

from stockfit.align import FrameAligner, Placement
from stockfit.layout import ReferenceLayout, ScenePair
from stockfit.model import (
    DeltaE,
    NeutralStock,
    ParameterCodec,
    ParametricForward,
    StockParameters,
)
from stockfit.pipeline import (
    DisplayOrientation,
    JpegFrame,
    LinearDump,
    RawstockPipeline,
    SensorOrientation,
)


@dataclass
class PreparedScene:
    """Downsampled, oriented, aligned linear ACEScg and its camera JPEG."""

    pair: ScenePair
    linear: np.ndarray
    jpeg: np.ndarray
    placement: Placement


@dataclass
class ErrorRow:
    split: str
    stem: str
    film_mode: str
    white_balance: str
    before_median: float
    before_p95: float
    after_median: float
    after_p95: float
    align_score: float


@dataclass
class FitConfig:
    refs: Path
    output: Path
    report_dir: Path
    name: str
    simulation: str
    binary: Path | None
    long_edge: int
    cache: Path
    max_iter: int
    seed: int
    samples_per_image: int


class PatchSampler:
    """Stratified patches. A border is dropped so a one-pixel mis-crop
    does not dominate the loss or the reported percentiles."""

    _BORDER = 0.02

    @staticmethod
    def collect(
        scenes: list[PreparedScene],
        per_image: int,
        seed: int,
    ) -> tuple[np.ndarray, np.ndarray, list[str]]:
        generator = np.random.default_rng(seed)
        linear_parts: list[np.ndarray] = []
        jpeg_parts: list[np.ndarray] = []
        stems: list[str] = []
        for scene in scenes:
            linear, jpeg = PatchSampler._sample(scene, per_image, generator)
            linear_parts.append(linear)
            jpeg_parts.append(jpeg)
            stems.extend([scene.pair.stem] * linear.shape[0])
        return np.concatenate(linear_parts), np.concatenate(jpeg_parts), stems

    @staticmethod
    def _sample(
        scene: PreparedScene,
        count: int,
        generator: np.random.Generator,
    ) -> tuple[np.ndarray, np.ndarray]:
        linear, jpeg = PatchSampler.interior(scene.linear, scene.jpeg)
        luma = 0.2126 * jpeg[:, :, 0] + 0.7152 * jpeg[:, :, 1] + 0.0722 * jpeg[:, :, 2]
        order = np.argsort(luma.ravel(), kind="mergesort")
        bins = np.array_split(order, 5)
        chosen: list[np.ndarray] = []
        each = max(1, count // len(bins))
        for bin_index in bins:
            if bin_index.size == 0:
                continue
            take = min(each, bin_index.size)
            pick = generator.choice(bin_index, size=take, replace=False)
            chosen.append(np.asarray(pick, dtype=np.int64))
        index = np.concatenate(chosen)
        return linear.reshape(-1, 3)[index], jpeg.reshape(-1, 3)[index]

    @staticmethod
    def interior(linear: np.ndarray, jpeg: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        height, width = jpeg.shape[:2]
        margin_y = int(height * PatchSampler._BORDER)
        margin_x = int(width * PatchSampler._BORDER)
        if height - 2 * margin_y < 8 or width - 2 * margin_x < 8:
            return linear, jpeg
        window = np.s_[margin_y : height - margin_y, margin_x : width - margin_x]
        return linear[window], jpeg[window]


class StockFitter:
    """Minimize mean CIEDE2000 over training patches.

    The search variables are the parametric stock: exposure, per-channel
    D-logH densities, the print curve, and the 3×3 log-exposure mix.
    Densities stay monotone because each step is a softplus increment.
    """

    _SMOOTH = 0.35
    _COUPLING = 0.04
    _ANCHOR = 0.015

    @staticmethod
    def fit(
        linear: np.ndarray,
        jpeg: np.ndarray,
        start: StockParameters,
        max_iter: int,
    ) -> tuple[StockParameters, float, float]:
        reference_lab = DeltaE.lab(jpeg)
        start = StockFitter._exposure_start(linear, reference_lab, start)
        neutral = ParameterCodec.pack(NeutralStock.parameters())
        packed = ParameterCodec.pack(start)

        def loss(raw: np.ndarray) -> float:
            parameters = ParameterCodec.unpack(raw)
            rendered = ParametricForward.render(linear, parameters)
            data = float(np.mean(DeltaE.ciede2000_lab(rendered, reference_lab)))
            coupling_error = parameters.coupling - np.eye(3)
            anchor = ParameterCodec.pack(parameters) - neutral
            return (
                data
                + StockFitter._SMOOTH * ParametricForward.roughness(parameters)
                + StockFitter._COUPLING * float(np.mean(coupling_error * coupling_error))
                + StockFitter._ANCHOR * float(np.mean(anchor * anchor))
            )

        before = loss(neutral)
        print(f"sample loss before {before:.3f} ΔE (plus regularizers)", flush=True)
        result = minimize(
            loss,
            packed,
            method="L-BFGS-B",
            options={"maxiter": max_iter, "ftol": 1.0e-7, "maxfun": max_iter * 40},
        )
        after = float(result.fun)
        print(
            f"sample loss after {after:.3f}  iterations {result.nit}  {result.message}",
            flush=True,
        )
        return ParameterCodec.unpack(result.x), before, after

    @staticmethod
    def _exposure_start(
        linear: np.ndarray,
        reference_lab: np.ndarray,
        start: StockParameters,
    ) -> StockParameters:
        best = start.copy()
        best_error = float("inf")
        for exposure in np.linspace(-1.5, 1.5, 13):
            trial = start.copy()
            trial.exposure_ev = float(exposure)
            rendered = ParametricForward.render(linear, trial)
            error = float(np.mean(DeltaE.ciede2000_lab(rendered, reference_lab)))
            if error < best_error:
                best = trial
                best_error = error
        print(
            f"exposure start {best.exposure_ev:.2f} ev, mean ΔE {best_error:.2f}",
            flush=True,
        )
        return best


class StockWriter:
    """Schema-1 TOML. Grain and halation are omitted on purpose."""

    @staticmethod
    def render(name: str, simulation: str, parameters: StockParameters) -> str:
        source = StockWriter._source(simulation)
        lines = [
            f"# {name} film simulation, parametric approximation.",
            "# Fitted to Fujifilm X-T2 in-camera JPEGs. This is not a LUT and",
            "# not a datasheet of the reversal film. Grain is omitted: the",
            "# camera's Grain Effect was Weak, and the fit was made with grain off.",
            f"# {name} is a Fujifilm trademark. These numbers were produced by",
            "# rawstock's stock fitter from scene-linear ACEScg.",
            "",
            "schema_version = 1",
            "revision = 1",
            f"name = {StockWriter._string(name)}",
            f"source = {StockWriter._string(source)}",
            'license = "CC0-1.0"',
            'model = "parametric"',
            "",
            f"exposure_ev = {StockWriter._float(parameters.exposure_ev)}",
            f"print_exposure = {StockWriter._float(parameters.print_exposure)}",
            "",
        ]
        for index, channel in enumerate(("red", "green", "blue")):
            lines.append(f"[{channel}]")
            lines.append("log_exposure = " + StockWriter._literals(NeutralStock.LOG_EXPOSURE_TEXT))
            lines.append("density = " + StockWriter._array(parameters.density[index]))
            lines.append("")
        lines.append("[print]")
        lines.append("log_exposure = " + StockWriter._literals(NeutralStock.PRINT_LOG_TEXT))
        lines.append("density = " + StockWriter._array(parameters.print_density))
        lines.append("")
        lines.append("[coupling]")
        rows = ", ".join(StockWriter._array(row) for row in parameters.coupling)
        lines.append(f"matrix = [{rows}]")
        lines.append("")
        return "\n".join(lines)

    @staticmethod
    def _source(simulation: str) -> str:
        return (
            f"Approximate parametric fit to Fujifilm X-T2 in-camera {simulation} "
            "JPEGs. DR100, Color 0, Highlight Tone 0, Shadow Tone 0, Sharpness "
            "Normal, Noise Reduction 0, White Balance Auto. Grain Effect was Weak "
            "on the camera and is omitted, so the fit was made with grain off. "
            "Not a Fujifilm datasheet, not a 3D LUT, and not a measurement of the "
            "reversal film."
        )

    @staticmethod
    def _string(value: str) -> str:
        escaped = value.replace("\\", "\\\\").replace('"', '\\"')
        return f'"{escaped}"'

    @staticmethod
    def _array(values: np.ndarray) -> str:
        return "[" + ", ".join(StockWriter._float(value) for value in values) + "]"

    @staticmethod
    def _literals(values: tuple[str, ...]) -> str:
        return "[" + ", ".join(values) + "]"

    @staticmethod
    def _float(value: float) -> str:
        number = float(np.float32(value))
        text = f"{number:.8g}"
        if "e" not in text and "." not in text:
            text += ".0"
        return text


class ErrorTable:
    """Per-image median and 95th percentile, before the fit and after it."""

    @staticmethod
    def measure(developed: np.ndarray, jpeg: np.ndarray) -> tuple[float, float]:
        developed, jpeg = PatchSampler.interior(developed, jpeg)
        errors = DeltaE.ciede2000(developed.reshape(-1, 3), jpeg.reshape(-1, 3))
        return DeltaE.median_and_p95(errors)

    @staticmethod
    def markdown(rows: list[ErrorRow], name: str) -> str:
        header = (
            f"| Split | File | Camera mode | WB | Neutral median | Neutral p95 "
            f"| {name} median | {name} p95 |"
        )
        rule = "| --- | --- | --- | --- | ---: | ---: | ---: | ---: |"
        body = [header, rule]
        for row in rows:
            body.append(
                "| "
                + " | ".join(
                    [
                        row.split,
                        row.stem,
                        row.film_mode.replace("|", "/"),
                        row.white_balance,
                        f"{row.before_median:.2f}",
                        f"{row.before_p95:.2f}",
                        f"{row.after_median:.2f}",
                        f"{row.after_p95:.2f}",
                    ]
                )
                + " |"
            )
        return "\n".join(body) + "\n"

    @staticmethod
    def draw(rows: list[ErrorRow], name: str, path: Path) -> None:
        font = ErrorTable._font(18)
        small = ErrorTable._font(15)
        columns = [
            "Split",
            "File",
            "Camera mode",
            "WB",
            "Neutral median",
            "Neutral p95",
            f"{name} median",
            f"{name} p95",
        ]
        record = [
            [
                row.split,
                row.stem,
                row.film_mode,
                row.white_balance,
                f"{row.before_median:.2f}",
                f"{row.before_p95:.2f}",
                f"{row.after_median:.2f}",
                f"{row.after_p95:.2f}",
            ]
            for row in rows
        ]
        widths = []
        for index, title in enumerate(columns):
            widest = font.getlength(title)
            for line in record:
                widest = max(widest, small.getlength(line[index]))
            widths.append(int(widest) + 28)
        row_h = 32
        width = 24 + sum(widths)
        height = 24 + row_h * (len(rows) + 1)
        image = Image.new("RGB", (width, height), (248, 246, 242))
        draw = ImageDraw.Draw(image)
        y = 12
        x = 12
        for index, title in enumerate(columns):
            draw.text((x, y), title, fill=(30, 30, 30), font=font)
            x += widths[index]
        for line_index, line in enumerate(record):
            y = 12 + row_h * (line_index + 1)
            shade = (255, 255, 255) if line_index % 2 == 0 else (236, 233, 226)
            draw.rectangle((8, y - 4, width - 8, y + row_h - 6), fill=shade)
            x = 12
            for index, cell in enumerate(line):
                draw.text((x, y), cell, fill=(20, 20, 20), font=small)
                x += widths[index]
        path.parent.mkdir(parents=True, exist_ok=True)
        image.save(path)

    @staticmethod
    def _font(size: int) -> ImageFont.ImageFont:
        for candidate in (
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        ):
            if Path(candidate).is_file():
                return ImageFont.truetype(candidate, size=size)
        return ImageFont.load_default()


class Triptych:
    """Downscaled rawstock | camera JPEG | ΔE2000 heatmap."""

    @staticmethod
    def write(
        developed: np.ndarray,
        jpeg: np.ndarray,
        stem: str,
        median: float,
        p95: float,
        path: Path,
    ) -> None:
        developed, jpeg = PatchSampler.interior(developed, jpeg)
        errors = DeltaE.ciede2000(developed.reshape(-1, 3), jpeg.reshape(-1, 3))
        heat = Triptych._heatmap(errors.reshape(developed.shape[:2]))
        panels = [
            (Triptych._uint8(developed), "rawstock"),
            (Triptych._uint8(jpeg), "Fuji JPEG"),
            (heat, f"ΔE2000  med {median:.1f}  p95 {p95:.1f}"),
        ]
        gap = 6
        caption = 36
        height, width = developed.shape[:2]
        canvas = Image.new(
            "RGB",
            (width * 3 + gap * 4, height + caption + gap * 2),
            (28, 28, 28),
        )
        draw = ImageDraw.Draw(canvas)
        font = ErrorTable._font(18)
        for index, (panel, label) in enumerate(panels):
            left = gap + index * (width + gap)
            canvas.paste(Image.fromarray(panel, "RGB"), (left, gap))
            draw.text((left, gap + height + 8), label, fill=(240, 240, 240), font=font)
        path.parent.mkdir(parents=True, exist_ok=True)
        canvas.save(path)
        print(f"triptych {stem} -> {path}", flush=True)

    @staticmethod
    def _uint8(image: np.ndarray) -> np.ndarray:
        return np.clip(np.rint(image * 255.0), 0, 255).astype(np.uint8)

    @staticmethod
    def _heatmap(errors: np.ndarray) -> np.ndarray:
        t = np.clip(errors / 12.0, 0.0, 1.0)
        stops = np.array(
            [[32, 64, 120], [46, 150, 120], [236, 196, 64], [186, 48, 42]],
            dtype=np.float32,
        )
        positions = np.array([0.0, 0.28, 0.58, 1.0], dtype=np.float32)
        color = np.empty(errors.shape + (3,), dtype=np.float32)
        for channel in range(3):
            color[:, :, channel] = np.interp(t, positions, stops[:, channel])
        return np.clip(np.rint(color), 0, 255).astype(np.uint8)


class FitSession:
    """One fit. Held-out scenes are measured after the stock is written."""

    @staticmethod
    def run(config: FitConfig) -> list[ErrorRow]:
        layout = ReferenceLayout.load(config.refs)
        layout.require_simulation(config.simulation)
        binary = RawstockPipeline.resolve(config.binary)
        neutral_path = FitSession._neutral_stock()
        training = layout.training_pairs()
        heldout = layout.heldout_pairs()
        print(f"binary {binary}", flush=True)
        print(f"training {[pair.stem for pair in training]}", flush=True)
        print(f"held-out {[pair.stem for pair in heldout]} (not fit)", flush=True)
        prepared = [FitSession._prepare(pair, binary, config) for pair in training]
        linear, jpeg, stems = PatchSampler.collect(prepared, config.samples_per_image, config.seed)
        training_stems = {pair.stem for pair in training}
        heldout_stems = {pair.stem for pair in heldout}
        if any(stem not in training_stems for stem in stems):
            raise RuntimeError("training samples include a stem outside fit/")
        if any(stem in heldout_stems for stem in stems):
            raise RuntimeError("held-out stem leaked into the fit")
        print(f"fitting {linear.shape[0]} patches from fit/ only", flush=True)
        fitted, _before, _after = StockFitter.fit(
            linear, jpeg, NeutralStock.parameters(), config.max_iter
        )
        text = StockWriter.render(config.name, config.simulation, fitted)
        if "[grain]" in text or "[halation]" in text:
            raise RuntimeError("fitted stock must not enable grain or halation")
        config.output.parent.mkdir(parents=True, exist_ok=True)
        config.output.write_text(text)
        print(f"wrote {config.output}", flush=True)
        FitSession._check_model(binary, neutral_path, prepared[0], config.cache)
        rows = [FitSession._measure(pair, binary, neutral_path, config) for pair in training + heldout]
        config.report_dir.mkdir(parents=True, exist_ok=True)
        markdown = ErrorTable.markdown(rows, config.name)
        (config.report_dir / "delta-e.md").write_text(markdown)
        ErrorTable.draw(rows, config.name, config.report_dir / "delta-e.png")
        (config.report_dir / "delta-e.json").write_text(
            json.dumps([row.__dict__ for row in rows], indent=2) + "\n"
        )
        print(markdown, flush=True)
        return rows

    @staticmethod
    def _prepare(pair: ScenePair, binary: Path, config: FitConfig) -> PreparedScene:
        exr = config.cache / "linear" / f"{pair.stem}.exr"
        if not exr.is_file():
            print(f"dump-linear {pair.label()}", flush=True)
            RawstockPipeline.dump_linear(binary, pair.raw, exr, config.long_edge)
        raw_orientation = SensorOrientation.from_inspect(
            RawstockPipeline.inspect(binary, pair.raw)
        )
        orientation = DisplayOrientation.choose(
            raw_orientation, JpegFrame.orientation_name(pair.jpeg)
        )
        linear = SensorOrientation.apply(LinearDump.read(exr), orientation)
        jpeg = JpegFrame.downsample(JpegFrame.load_upright(pair.jpeg), config.long_edge)
        neutral = ParametricForward.render(linear, NeutralStock.parameters())
        _aligned_render, aligned_jpeg, placement = FrameAligner.align(neutral, jpeg)
        aligned_linear, aligned_jpeg = FrameAligner.apply(linear, jpeg, placement)
        print(
            f"align {pair.label()} {orientation} scale {placement.scale:.3f} "
            f"origin ({placement.origin_x}, {placement.origin_y}) "
            f"ncc {placement.score:.3f} -> {aligned_linear.shape[1]}x{aligned_linear.shape[0]}",
            flush=True,
        )
        if placement.score < 0.25:
            raise RuntimeError(
                f"{pair.label()} alignment NCC {placement.score:.3f} is too low to fit"
            )
        return PreparedScene(pair, aligned_linear, aligned_jpeg, placement)

    @staticmethod
    def _measure(
        pair: ScenePair,
        binary: Path,
        neutral_stock: Path,
        config: FitConfig,
    ) -> ErrorRow:
        before = FitSession._pipeline_pair(pair, binary, neutral_stock, "neutral", config)
        after = FitSession._pipeline_pair(pair, binary, config.output, "fitted", config)
        before_median, before_p95 = ErrorTable.measure(before[0], before[1])
        after_median, after_p95 = ErrorTable.measure(after[0], after[1])
        row = ErrorRow(
            split=pair.split,
            stem=pair.stem,
            film_mode=pair.film_mode,
            white_balance=pair.white_balance,
            before_median=before_median,
            before_p95=before_p95,
            after_median=after_median,
            after_p95=after_p95,
            align_score=after[2].score,
        )
        print(
            f"{pair.label()} neutral {before_median:.2f}/{before_p95:.2f}  "
            f"fitted {after_median:.2f}/{after_p95:.2f}",
            flush=True,
        )
        if pair.split == "fit":
            Triptych.write(
                after[0],
                after[1],
                pair.stem,
                after_median,
                after_p95,
                config.report_dir / f"triptych-{pair.stem}.png",
            )
        return row

    @staticmethod
    def _pipeline_pair(
        pair: ScenePair,
        binary: Path,
        stock: Path,
        tag: str,
        config: FitConfig,
    ) -> tuple[np.ndarray, np.ndarray, Placement]:
        digest = hashlib.sha256(stock.read_bytes()).hexdigest()[:12]
        output = config.cache / "develop" / f"{tag}-{digest}" / f"{pair.stem}.jpg"
        if not output.is_file():
            print(f"develop {tag} {pair.label()}", flush=True)
            RawstockPipeline.develop(binary, pair.raw, stock, output)
        raw_orientation = SensorOrientation.from_inspect(
            RawstockPipeline.inspect(binary, pair.raw)
        )
        display = DisplayOrientation.choose(raw_orientation, JpegFrame.orientation_name(pair.jpeg))
        developed = JpegFrame.load_upright(output)
        if DisplayOrientation.render_needs_turn(raw_orientation, display):
            developed = SensorOrientation.apply(developed, display)
        developed = JpegFrame.downsample(developed, config.long_edge)
        jpeg = JpegFrame.downsample(JpegFrame.load_upright(pair.jpeg), config.long_edge)
        aligned, reference, placement = FrameAligner.align(developed, jpeg)
        return aligned, reference, placement

    @staticmethod
    def _check_model(
        binary: Path,
        neutral_stock: Path,
        scene: PreparedScene,
        cache: Path,
    ) -> None:
        """The Python forward model has to match `develop-linear` on neutral."""
        crop = np.ascontiguousarray(scene.linear[:48, :48])
        directory = cache / "parity"
        directory.mkdir(parents=True, exist_ok=True)
        exr = directory / "neutral-crop.exr"
        tiff = directory / "neutral-crop.tif"
        LinearDump.write(exr, crop)
        RawstockPipeline.develop_linear(binary, exr, neutral_stock, tiff)
        pipeline = JpegFrame.load_upright(tiff)
        model = ParametricForward.render(crop, NeutralStock.parameters())
        height = min(pipeline.shape[0], model.shape[0])
        width = min(pipeline.shape[1], model.shape[1])
        error = float(np.max(np.abs(pipeline[:height, :width] - model[:height, :width])))
        print(f"neutral model vs develop-linear max abs {error:.5f}", flush=True)
        if error > 0.02:
            raise RuntimeError(f"parametric forward model disagrees with rawstock by {error:.4f}")

    @staticmethod
    def _neutral_stock() -> Path:
        return Path(__file__).resolve().parents[2] / "stocks" / "neutral.stock.toml"
