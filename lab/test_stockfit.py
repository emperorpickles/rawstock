"""Synthetic checks for the stock fitter. No camera files."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import numpy as np
from scipy.ndimage import gaussian_filter, zoom

from stockfit.align import FrameAligner
from stockfit.fit import StockFitter, StockWriter
from stockfit.layout import ReferenceLayout
from stockfit.model import (
    DeltaE,
    NeutralStock,
    ParameterCodec,
    ParametricForward,
    WorkingSpace,
)
from stockfit.pipeline import (
    DisplayOrientation,
    JpegFrame,
    LinearDump,
    RawstockPipeline,
    SensorOrientation,
)


class OrientationTests(unittest.TestCase):
    def test_rotate_270_matches_the_core_fixture(self) -> None:
        pixels = np.array([[10.0, 20.0], [30.0, 40.0]], dtype=np.float32)
        rotated = SensorOrientation.apply(pixels, "rotate 90 CCW")
        self.assertTrue(
            np.array_equal(rotated, np.array([[20.0, 40.0], [10.0, 30.0]], dtype=np.float32))
        )

    def test_inspect_line_is_the_display_name(self) -> None:
        text = "dimensions: 2x2\norientation: rotate 90 CCW\n"
        self.assertEqual(SensorOrientation.from_inspect(text), "rotate 90 CCW")

    def test_jpeg_rotate_270_overrides_a_normal_raw_tag(self) -> None:
        # X-T2 frames tagged Rotate 270 CW: the JPEG carries EXIF 8 and the
        # RAF tag rawler reports can be normal. Display follows the JPEG.
        self.assertEqual(
            DisplayOrientation.choose("normal", "rotate 90 CCW"),
            "rotate 90 CCW",
        )
        self.assertTrue(DisplayOrientation.render_needs_turn("normal", "rotate 90 CCW"))
        self.assertFalse(
            DisplayOrientation.render_needs_turn("rotate 90 CCW", "rotate 90 CCW")
        )


class LayoutTests(unittest.TestCase):
    def test_heldout_is_not_training_data(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = self._tree(Path(tmp))
            layout = ReferenceLayout.load(root)
            training = {pair.stem for pair in layout.training_pairs()}
            heldout = {pair.stem for pair in layout.heldout_pairs()}
            self.assertEqual(training, {"DSCF0001"})
            self.assertEqual(heldout, {"DSCF4069", "DSCF4205", "DSCF6289"})
            self.assertTrue(training.isdisjoint(heldout))
            layout.require_simulation("Velvia")

    def test_a_heldout_match_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = self._tree(Path(tmp))
            prose = (root / "manifest.md").read_text().replace(
                "F0/Standard (Provia)",
                "F2/Fujichrome (Velvia)",
            )
            (root / "manifest.md").write_text(prose)
            layout = ReferenceLayout.load(root)
            with self.assertRaises(ValueError):
                layout.require_simulation("Velvia")

    def test_a_training_file_of_the_wrong_mode_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = self._tree(Path(tmp))
            layout = ReferenceLayout.load(root)
            with self.assertRaises(ValueError):
                layout.require_simulation("Provia")

    @staticmethod
    def _tree(root: Path) -> Path:
        root.joinpath("manifest.md").write_text(
            "\n".join(
                [
                    "| File | FilmMode | WB |",
                    "| --- | --- | --- |",
                    "| DSCF0001.JPG | F2/Fujichrome (Velvia) | Auto |",
                    "| DSCF4069.JPG | F0/Standard (Provia) | Auto |",
                    "| DSCF4205.JPG | Classic Chrome | Auto |",
                    "| DSCF6289.JPG | F1b/Studio Portrait Smooth Skin Tone (Astia) | Daylight |",
                    "",
                ]
            )
        )
        for split, stems in {
            "fit": ["DSCF0001"],
            "heldout": ["DSCF4069", "DSCF4205", "DSCF6289"],
        }.items():
            directory = root / split
            directory.mkdir()
            for stem in stems:
                directory.joinpath(f"{stem}.RAF").write_bytes(b"not a photo")
                directory.joinpath(f"{stem}.JPG").write_bytes(b"not a photo")
        return root


class ModelTests(unittest.TestCase):
    def test_acescg_white_encodes_to_srgb_white(self) -> None:
        encoded = WorkingSpace.encode(np.ones((1, 3)))
        self.assertTrue(np.allclose(encoded, 1.0, atol=1.0e-5), encoded)
        black = WorkingSpace.encode(np.full((1, 3), -1.0))
        self.assertTrue(np.allclose(black, 0.0, atol=1.0e-6), black)

    def test_neutral_gray_stays_neutral(self) -> None:
        rendered = ParametricForward.render(
            np.array([[0.18, 0.18, 0.18]]),
            NeutralStock.parameters(),
        )
        spread = float(rendered.max() - rendered.min())
        self.assertLess(spread, 0.01, rendered)

    def test_codec_round_trip_keeps_densities_monotone(self) -> None:
        parameters = NeutralStock.parameters()
        parameters.exposure_ev = -0.25
        parameters.coupling = np.array(
            [[1.05, -0.04, -0.01], [-0.02, 1.03, -0.01], [0.0, -0.03, 1.04]]
        )
        restored = ParameterCodec.unpack(ParameterCodec.pack(parameters))
        self.assertAlmostEqual(restored.exposure_ev, -0.25, places=5)
        np.testing.assert_allclose(restored.density, parameters.density, atol=1.0e-4)
        np.testing.assert_allclose(restored.coupling, parameters.coupling, atol=1.0e-5)
        for curve in (*restored.density, restored.print_density):
            self.assertTrue(np.all(np.diff(curve) > 0))

    def test_identical_srgb_has_zero_delta_e(self) -> None:
        color = np.array([[0.2, 0.4, 0.6], [0.8, 0.8, 0.8]])
        errors = DeltaE.ciede2000(color, color)
        self.assertTrue(np.allclose(errors, 0.0, atol=1.0e-6))


class AlignTests(unittest.TestCase):
    def test_recovers_a_known_scale_and_crop(self) -> None:
        generator = np.random.default_rng(1)
        base = generator.random((140, 180, 3))
        base = gaussian_filter(base, sigma=(1.6, 1.6, 0))
        scale = 1.07
        zoomed = zoom(base, (scale, scale, 1.0), order=1, prefilter=False)
        origin_y, origin_x = 9, 14
        reference = zoomed[origin_y : origin_y + 110, origin_x : origin_x + 140]
        _fitted, _target, placement = FrameAligner.align(base.astype(np.float32), reference)
        self.assertAlmostEqual(placement.scale, scale, delta=0.015)
        self.assertAlmostEqual(placement.origin_y, origin_y, delta=3)
        self.assertAlmostEqual(placement.origin_x, origin_x, delta=3)
        self.assertGreater(placement.score, 0.8)


class FitterTests(unittest.TestCase):
    def test_fit_reduces_delta_e_on_a_known_stock(self) -> None:
        generator = np.random.default_rng(2)
        scene = np.power(10.0, generator.uniform(-3.2, 0.4, size=(180, 3)))
        target = NeutralStock.parameters()
        target.exposure_ev = 0.4
        target.density = target.density.copy()
        target.density[0] *= 0.94
        target.density[2] *= 1.04
        jpeg = ParametricForward.render(scene, target)
        before = float(
            np.mean(
                DeltaE.ciede2000(
                    ParametricForward.render(scene, NeutralStock.parameters()),
                    jpeg,
                )
            )
        )
        fitted, _loss_before, _loss_after = StockFitter.fit(
            scene,
            jpeg,
            NeutralStock.parameters(),
            max_iter=12,
        )
        after = float(np.mean(DeltaE.ciede2000(ParametricForward.render(scene, fitted), jpeg)))
        self.assertLess(after, before * 0.7, f"before {before:.3f} after {after:.3f}")
        for curve in (*fitted.density, fitted.print_density):
            self.assertTrue(np.all(np.diff(curve) >= -1.0e-6))

    def test_written_stock_is_a_parametric_approximation_without_grain(self) -> None:
        text = StockWriter.render("Velvia", "Velvia", NeutralStock.parameters())
        self.assertIn('model = "parametric"', text)
        self.assertIn("Approximate parametric fit", text)
        self.assertIn("not a 3D LUT", text)
        self.assertIn("grain off", text)
        self.assertNotIn("[grain]", text)
        self.assertNotIn("[halation]", text)
        self.assertNotIn(".cube", text)


class PipelineParityTests(unittest.TestCase):
    def test_neutral_forward_model_matches_develop_linear(self) -> None:
        binary = Path(__file__).resolve().parents[1] / "target" / "release" / "rawstock"
        if not binary.is_file():
            binary = Path(__file__).resolve().parents[1] / "target" / "debug" / "rawstock"
        if not binary.is_file():
            self.skipTest("rawstock binary is not built")
        scene = np.array(
            [
                [[0.18, 0.18, 0.18], [0.05, 0.2, 0.4], [1.2, 0.4, 0.1]],
                [[0.0, 0.0, 0.0], [0.01, 0.01, 0.02], [-0.05, 0.3, 0.3]],
            ],
            dtype=np.float32,
        )
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            exr = directory / "scene.exr"
            tiff = directory / "scene.tif"
            LinearDump.write(exr, scene)
            stock = Path(__file__).resolve().parents[1] / "stocks" / "neutral.stock.toml"
            RawstockPipeline.develop_linear(binary, exr, stock, tiff)
            pipeline = JpegFrame.load_upright(tiff)
        model = ParametricForward.render(scene, NeutralStock.parameters())
        self.assertLess(float(np.max(np.abs(pipeline - model))), 0.02)


if __name__ == "__main__":
    unittest.main()
