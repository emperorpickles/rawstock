# Plan

Scaffolding is done: the workspace builds and tests, and `rawstock develop`
is a stub. What each pipeline stage does in the code, and which open pull
request adds it, is in [Film pipeline](FILM_PIPELINE.md). Phases from here:

## Phase 1: Decode
**Goal:** `rawler` decode for the first cameras, plus linear dumps for the lab.
**Done when:** Fuji X-Trans RAF and iPhone ProRAW DNG decode in core to
scene-referred linear RGB and metadata; LibRaw is not a core dependency.

## Phase 2: Shared pipeline (Parametric)
**Goal:** the CPU pipeline in [Architecture](ARCHITECTURE.md), driven by
`FilmModel::Parametric`.
**Done when:** `rawstock develop IMG.RAF --stock <name> -o out.jpg` runs that
pipeline through export on the Phase 1 cameras; a versioned parametric stock
loads and validates; tests cover decode, the parametric model, and stock
parsing.

## Phase 3: Portra 400 (Spectral)
**Goal:** Kodak Portra 400 as `FilmModel::Spectral`, from Kodak publication
E-4050.
**Done when:** spectral sensitivity, characteristic curves, and dye density
come from E-4050; grain, interlayer coupling, and print/scan are calibrated
from chart or step-wedge scans; on the same linear dumps, lab and Rust model
math agree within a numeric tolerance.

## Phase 4: GPU (wgpu)
**Goal:** the same pipeline on GPU, grain hash included.
**Done when:** CPU/GPU parity holds within a documented tolerance on the
Metal, DX12, and Vulkan backends available in CI or locally; preview and
export match the CPU grain; the CLI can select the GPU path.
**Status:** see [GPU path](GPU.md).

## Phase 5: Desktop (Tauri + Svelte)
**Goal:** open a RAW, pick a stock, preview, export.
**Done when:** the app builds and runs on macOS, Windows, and Linux; preview
uses the GPU renderer; export matches the CLI.

The first slice is a CPU stock tuner, before the GPU renderer. See
[UI plan](UI_PLAN.md).

## Phase 6: Web (wasm + WebGPU)
**Goal:** the same experience in the browser, fully client-side.
**Done when:** core compiles to wasm; the Svelte UI is shared with desktop;
decode and rendering work in WebGPU browsers; no image data is sent to a
server.

## Decisions
- Stocks are parametric or spectral film models. A 3D LUT is only an optional
  bake for `.cube` export or preview caching. Desktop hardware is the target,
  so compute is not a constraint.
- `FilmModel` maps scene exposure to layer densities. `Parametric` and
  `Spectral` are enum variants. Prefer the enum over a trait object unless
  there is a stated reason not to.
- Decode stays on `rawler` (pure Rust, wasm-friendly). LibRaw is a
  contingency only, never in core.
- The lab reads linear dumps from core and compares model math only, within
  a numeric tolerance.
- First cameras: Fuji X-Trans RAF and iPhone ProRAW DNG. First stock: Kodak
  Portra 400 from E-4050. Grain, interlayer coupling, and print/scan are
  calibrated from scans.
  [agx-emulsion](https://github.com/andreavolpato/agx-emulsion) (GPL-3.0; the
  repository is now spektrafilm) is reference prior art, compatible because
  rawstock is GPL-3.0-or-later.
- Stock files are versioned data (TOML or JSON plus numeric arrays) with
  `source` and `license` fields. Schema 1 is TOML and may include optional
  `[grain]` (RMS granularity versus density, per layer) and `[halation]`
  tables. See [stocks/](../stocks/README.md).

## Open questions
- **Export:** JPEG and TIFF first; 16-bit TIFF, PNG, HEIF/AVIF, and embedded
  ICC profiles are undecided.
- **Program license:** GPL-3.0-or-later stands. Revisit `-only` versus
  `-or-later`, and AGPL-3.0, only if hosted server-side use becomes a
  concern. `rawler` is LGPL-2.1-only; confirm the LGPL §6 link before the
  first release, and audit the tree (for example with `cargo deny`) once
  `rawler` is added.
- **Stock packaging:** LUT input space, and bundled versus user-installed
  stocks, are still open.
