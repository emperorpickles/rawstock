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

`import_spectral_film_lut.py` turns the MIT-licensed Portra 400 module from
[JanLohse/spectral_film_lut](https://github.com/JanLohse/spectral_film_lut)
into a spectral stock. It uses only the Python standard library and reads
the module as source text, so the package does not have to be installed.
Grain and halation are left out. See the script's docstring and
[stocks/README.md](../stocks/README.md) for what is copied and what is derived.

The committed `stocks/portra400.stock.toml` keeps this script's sensitivities
and D-logH curves, then adds an approximate aesthetic tuning (dye scale,
print curve, coupling, grain, halation). Running the command below replaces
that file and removes the tuning.

```bash
python3 lab/import_spectral_film_lut.py \
    path/to/spectral_film_lut/src/spectral_film_lut/negative_film/kodak_portra_400.py \
    -o stocks/portra400.stock.toml
```
