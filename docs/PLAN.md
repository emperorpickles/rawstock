# Plan

## Phase 0: Scaffolding
**Goal:** repo, workspace, docs, placeholders.
**Done when:** `cargo build` and `cargo test` pass; `rawstock develop` prints
"not implemented yet"; docs and READMEs exist; single initial commit.

## Phase 1: Look lab
**Goal:** decode Fuji RAFs in Python, build a linear pipeline, prototype 2–3
stocks, compare against camera JPEGs.
**Done when:** `lab/` decodes sample RAFs; a linear pipeline produces a neutral
developed image; 2–3 original stocks exist as LUT + parameters; side-by-side
comparisons with the in-camera JPEGs are reviewed; the pipeline is documented
well enough to port.

## Phase 2: Core + CLI
**Goal:** real decode, color pipeline, stock loading, CPU reference render,
JPEG/TIFF export.
**Done when:** `rawstock develop IMG.RAF --stock <name> -o out.jpg` works on
RAFs; stock files load and validate; CPU output closely matches the Phase 1
prototypes; tests cover decode, color, and stock parsing.

## Phase 3: GPU renderer (wgpu)
**Goal:** wgpu renderer running the same pipeline as the CPU path.
**Done when:** CPU/GPU parity tests pass within a documented tolerance on
Metal/DX12/Vulkan backends available in CI or locally; GPU path is used by
the CLI optionally.

## Phase 4: Desktop app (Tauri + Svelte)
**Goal:** open a RAW, pick a stock, preview, export.
**Done when:** app builds and runs on macOS, Windows, and Linux; preview uses
the GPU renderer; export matches CLI output.

## Phase 5: Web app (wasm + WebGPU)
**Goal:** the same experience in the browser, fully client-side.
**Done when:** core compiles to wasm; Svelte UI is shared with desktop; RAW
decode and rendering work in WebGPU browsers; verified that no image data is
sent to a server.

## Open questions
- **License:** which license (permissive vs copyleft)? LICENSE not yet added.
- **rawler vs LibRaw:** does rawler handle Fuji X-Trans/RAF well enough
  (demosaic quality, color matrices)? When to fall back to LibRaw on desktop?
- **Stock file format:** TOML vs other; how to express tone curve, grain, and
  halation; LUT input space; versioning; bundled vs user-installed stocks.
- **Export formats:** JPEG and TIFF are planned; 16-bit TIFF, PNG, HEIF/AVIF,
  and embedded ICC profiles TBD.
- **Working space:** exact wide-gamut primaries and LUT input encoding.
