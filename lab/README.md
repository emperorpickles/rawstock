# lab

Python look-development sandbox. Stocks are prototyped here (rawpy, numpy,
colour-science) before being ported to Rust. Nothing here ships.

## Setup

```bash
cd lab
python3 -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
```

Put sample RAWs in `test-images/` at the repo root (git-ignored). Never commit
RAW files.
