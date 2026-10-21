"""Reference-set layout: fit/ is training data, heldout/ is never fit."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class ScenePair:
    """One RAW file and the camera JPEG developed from it."""

    stem: str
    raw: Path
    jpeg: Path
    film_mode: str
    white_balance: str
    split: str

    def label(self) -> str:
        return f"{self.split}/{self.stem}"


class Manifest:
    """The markdown table shipped next to a reference set."""

    @staticmethod
    def parse(text: str) -> dict[str, dict[str, str]]:
        lines = [line.strip() for line in text.splitlines() if line.strip().startswith("|")]
        if len(lines) < 3:
            raise ValueError("manifest.md has no table")
        header = Manifest._cells(lines[0])
        try:
            file_index = header.index("File")
            mode_index = header.index("FilmMode")
            balance_index = header.index("WB")
        except ValueError as error:
            raise ValueError("manifest.md needs File, FilmMode, and WB columns") from error
        rows: dict[str, dict[str, str]] = {}
        for line in lines[2:]:
            cells = Manifest._cells(line)
            if len(cells) <= max(file_index, mode_index, balance_index):
                continue
            stem = Path(cells[file_index]).stem
            if not stem or set(cells[file_index]) <= {"-", ":"}:
                continue
            rows[stem] = {
                "film_mode": cells[mode_index],
                "white_balance": cells[balance_index],
            }
        if not rows:
            raise ValueError("manifest.md listed no files")
        return rows

    @staticmethod
    def _cells(line: str) -> list[str]:
        return [cell.strip() for cell in line.strip().strip("|").split("|")]


class ReferenceLayout:
    """fit/ is the only data a stock may be fit on. heldout/ is a check."""

    def __init__(
        self,
        root: Path,
        training: list[ScenePair],
        heldout: list[ScenePair],
    ) -> None:
        self.root = root
        self._training = training
        self._heldout = heldout

    @staticmethod
    def load(root: Path) -> ReferenceLayout:
        manifest_path = root / "manifest.md"
        if not manifest_path.is_file():
            raise FileNotFoundError(f"missing {manifest_path}")
        manifest = Manifest.parse(manifest_path.read_text())
        training = ReferenceLayout._pairs(root / "fit", manifest, "fit")
        heldout = ReferenceLayout._pairs(root / "heldout", manifest, "heldout")
        if not training:
            raise ValueError(f"{root / 'fit'} has no RAW/JPEG pairs")
        return ReferenceLayout(root, training, heldout)

    @staticmethod
    def _pairs(
        directory: Path,
        manifest: dict[str, dict[str, str]],
        split: str,
    ) -> list[ScenePair]:
        if not directory.is_dir():
            raise FileNotFoundError(f"missing {directory}")
        raws = sorted(directory.glob("*.RAF")) + sorted(directory.glob("*.raf"))
        pairs: list[ScenePair] = []
        for raw in raws:
            jpeg = ReferenceLayout._jpeg_for(raw)
            row = manifest.get(raw.stem)
            if row is None:
                raise ValueError(f"{raw.stem} is in {split}/ but not in manifest.md")
            pairs.append(
                ScenePair(
                    stem=raw.stem,
                    raw=raw,
                    jpeg=jpeg,
                    film_mode=row["film_mode"],
                    white_balance=row["white_balance"],
                    split=split,
                )
            )
        return pairs

    @staticmethod
    def _jpeg_for(raw: Path) -> Path:
        for suffix in (".JPG", ".jpg", ".JPEG", ".jpeg"):
            candidate = raw.with_suffix(suffix)
            if candidate.is_file():
                return candidate
        raise FileNotFoundError(f"{raw} has no JPEG pair")

    def training_pairs(self) -> list[ScenePair]:
        """Pairs the fitter is allowed to minimize against."""
        return list(self._training)

    def heldout_pairs(self) -> list[ScenePair]:
        """Pairs reported as checks. Never pass these to the minimizer."""
        return list(self._heldout)

    def require_simulation(self, name: str) -> None:
        """Every training row must be `name`, and no held-out row may be."""
        needle = name.casefold()
        if not needle:
            raise ValueError("simulation name is empty")
        for pair in self._training:
            if needle not in pair.film_mode.casefold():
                raise ValueError(
                    f"{pair.label()} is {pair.film_mode}, not {name}; refusing to fit it"
                )
        for pair in self._heldout:
            if needle in pair.film_mode.casefold():
                raise ValueError(
                    f"{pair.label()} is {pair.film_mode}; a held-out {name} pair must not sit beside the fit"
                )
