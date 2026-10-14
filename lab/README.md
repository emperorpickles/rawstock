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
