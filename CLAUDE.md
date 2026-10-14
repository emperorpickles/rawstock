# Rawstock conventions

Cross-platform RAW developer with film simulations ("stocks"). Status: early
scaffolding (see docs/PLAN.md for phases).

## Decisions
- Shared Rust core; every platform is a thin layer over it.
- Front ends in order: CLI → Tauri desktop → web (core as wasm). Web is fully
  client-side; never upload images.
- GPU via wgpu (same shaders on Metal/DX12/Vulkan/WebGPU); keep a CPU
  reference path for testing and parity checks.
- RAW decode: `rawler` only in core (pure Rust, wasm-friendly). LibRaw is a
  contingency outside core, never a core dependency. First cameras: Fuji
  X-Trans RAF and iPhone ProRAW DNG.
- Color: linear ACEScg; gamut compression and the output transform last.
- Stocks are parametric or spectral film models (`FilmModel` enum: scene
  exposure → layer densities), not LUTs. A 3D LUT is only an optional bake
  for `.cube` export or preview caching. First stock: Kodak Portra 400 from
  the E-4050 datasheet.
- UI: TypeScript + Svelte, shared by desktop and web.
- The Python lab (`lab/`) reads linear dumps from core and compares model
  math only, within a numeric tolerance. It does not decode.
- Vocabulary: looks are "stocks"; processing a file is "developing".
- Keep dependencies minimal (currently clap only).
- License: GPL-3.0-or-later (goal: no reuse in closed-source projects). New
  dependencies must be GPL-3.0-compatible (MIT/Apache/BSD/LGPL are fine;
  check before adding anything else). `rawler` is LGPL-2.1-only: OK to link.

## Layout
`crates/rawstock-core` (decode, color, stock, render), `crates/rawstock-cli`
(binary `rawstock`), `apps/desktop`, `apps/web`, `stocks/`, `lab/`, `docs/`.

## Commands
- `cargo build` / `cargo test` (workspace, edition 2024)
- `cargo run -p rawstock-cli -- develop IMG.RAF --stock <stock.toml> -o out.jpg`
- Lab: `cd lab && python3 -m venv .venv && . .venv/bin/activate && pip install -r requirements.txt`

## Rules
- NEVER commit RAW files (*.RAF, *.DNG, *.CR3, *.NEF, *.ARW) or anything in
  `test-images/`. Check `git status` before committing.
