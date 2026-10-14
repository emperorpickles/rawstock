# Rawstock conventions

Cross-platform RAW developer with film simulations ("stocks"). Status: early
scaffolding (see docs/PLAN.md for phases).

## Decisions
- Shared Rust core; every platform is a thin layer over it.
- Front ends in order: CLI → Tauri desktop → web (core as wasm). Web is fully
  client-side; never upload images.
- GPU via wgpu (same shaders on Metal/DX12/Vulkan/WebGPU); keep a CPU
  reference path for testing and parity checks.
- RAW decode: `rawler` first; LibRaw is desktop fallback for Fuji X-Trans/RAF.
- Color: linear wide-gamut working space; convert to output space last.
- Stocks are data (base .cube LUT + tone/grain/halation params), not code.
  Names must be original: no trademarked names (Velvia, Portra, Classic
  Chrome, etc.).
- UI: TypeScript + Svelte, shared by desktop and web.
- Look development happens in the Python sandbox (`lab/`) before porting to Rust.
- Vocabulary: looks are "stocks"; processing a file is "developing".
- Keep dependencies minimal (currently clap only). No LICENSE yet: the
  license is undecided.

## Layout
`crates/rawstock-core` (decode, color, stock, render), `crates/rawstock-cli`
(binary `rawstock`), `apps/desktop`, `apps/web`, `stocks/`, `lab/`, `docs/`.

## Commands
- `cargo build` / `cargo test` (workspace, edition 2024)
- `cargo run -p rawstock-cli -- develop IMG.RAF --stock <name> -o out.jpg`
- Lab: `cd lab && python3 -m venv .venv && . .venv/bin/activate && pip install -r requirements.txt`

## Rules
- NEVER commit RAW files (*.RAF, *.DNG, *.CR3, *.NEF, *.ARW) or anything in
  `test-images/`. Check `git status` before committing.
