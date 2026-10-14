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
