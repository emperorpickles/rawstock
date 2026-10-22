# lab

Python sandbox for film-model math (numpy, colour-science). A prototype is
ported to Rust once it matches. Nothing here ships.

The lab reads linear dumps from core: scene-referred ACEScg after `rawler`
decode, as-shot white balance, and exposure. It does not decode with rawpy.
Lab and Rust compare the model math on those dumps, within a numeric
tolerance, so a decoder difference cannot show up as a model difference.

## Setup

```bash
cd lab
python3 -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
```

Put sample RAWs in `test-images/` at the repo root (git-ignored). Never commit
RAW files.

## Scene-linear dumps

The lab consumes scene-linear ACEScg OpenEXR files written by the Rust core,
so look work can share that decode instead of calling rawpy:

```bash
cargo run -p rawstock-cli --release -- dump-linear IMG.RAF out.exr
python read_linear.py out.exr
```

`read_linear.read_acescg_exr` returns a float32 NumPy array of shape
`(height, width, 3)`, RGB, scene-linear ACEScg. No white balance, tone
curve, or output encoding is applied on top of what the core already did.

## spectral_film_lut importer

`import_spectral_film_lut.py` turns the MIT-licensed Portra 400 and Velvia 50
modules from
[JanLohse/spectral_film_lut](https://github.com/JanLohse/spectral_film_lut)
into spectral stocks. Velvia is written as a reversal stock. Portra is a
negative printed onto the Portra Endura paper module, read through the
package's Status M table. It uses only the Python standard library and reads
the modules as source text, so the package does not have to be installed.
Grain and halation are left out. See the script's docstring and
[stocks/README.md](../stocks/README.md) for what is copied and what is derived.

The committed `stocks/portra400.stock.toml` is this script's output plus a
header note and an approximate grain and halation tuning. Running the
command below replaces that file and removes those additions.

```bash
python3 lab/import_spectral_film_lut.py \
    path/to/spectral_film_lut/src/spectral_film_lut/negative_film/kodak_portra_400.py \
    --paper path/to/spectral_film_lut/src/spectral_film_lut/print_film/kodak_portra_endura.py \
    -o stocks/portra400.stock.toml
```

`stocks/velvia50-spectral.stock.toml` works the same way: the importer's
traces plus an approximate scan curve and grain. Re-running the command
below drops the header notes, the scan-curve comment, and the grain.

```bash
python3 lab/import_spectral_film_lut.py \
    path/to/spectral_film_lut/src/spectral_film_lut/reversal_film/fuji_velvia_50.py \
    -o stocks/velvia50-spectral.stock.toml
```

## Parametric stock fitter

`fit_stock.py` fits a schema-1 parametric stock to camera JPEG pairs. It
develops each RAW through `rawstock` (decode, as-shot white balance, ACEScg,
then the film pipeline), aligns that develop to the JPEG (orientation, and
the crop from lens correction), downsamples both, and minimizes CIEDE2000
over sampled patches. The free parameters are the stock's own: exposure,
per-channel D-logH curves, the print curve, and a 3×3 log-exposure mix.
There is no LUT.

Only `fit/` is minimized. `heldout/` is measured and reported, and those
pairs never enter the loss. The same command is how a later simulation
(Provia, Astia, Classic Chrome, Eterna) gets its own file: point it at
that simulation's pairs and pass `--simulation`. Reference photographs stay
outside the repository. `python3 lab/test_stockfit.py` uses synthetic
images only.

```bash
cargo build -p rawstock-cli --release
python3 lab/fit_stock.py /path/to/pairs \
    --name Velvia --simulation Velvia \
    -o stocks/velvia.stock.toml \
    --report-dir /tmp/velvia-report
```
