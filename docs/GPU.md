# GPU path

Phase 4 in [PLAN](PLAN.md). The film pipeline also runs on wgpu, in
`rawstock_core::gpu`, behind the `gpu` cargo feature. The CPU path in
`render.rs` is the reference. The plan and its slices are tracked in
[#21](https://github.com/emperorpickles/rawstock/issues/21).

## Status

| Stage | GPU | Slice |
| --- | --- | --- |
| Exposure | yes | 1 |
| Halation | yes | 3 |
| Parametric density (curves, coupling) | yes | 1 |
| Spectral density | yes | 2 |
| Grain | yes | 4 |
| Dye scan | yes, always exact | 2 |
| Print/scan | yes | 1 |
| sRGB encode | yes | 1 |

Every stage runs on the GPU, from the CLI, the preview session, and the
desktop tuner (slice 5). A frame too large for one storage binding,
for the halation plane or one row of the grain band, gets
`GpuError::FrameTooLarge`, and the caller falls back to the CPU.

The GPU dye scan is always the exact integral, 41 wavelengths per pixel,
so its output is `Fidelity::Exact`. The preview session's dye lookup table
exists only because that integral is slow on the CPU.

## Layout

- `crates/rawstock-core/src/gpu/mod.rs`: device setup, uploads, dispatch,
  readback.
- `crates/rawstock-core/src/gpu/pixel.wgsl`: one entry point per per-pixel
  stage. Each names the CPU function it mirrors. Spectral stocks upload
  their wavelength tables. The 9.4 MB Jakob-Hanika coefficient table is
  uploaded once per device, on the first spectral stock.
- `crates/rawstock-core/src/gpu/halation.wgsl`: threshold, the three Kovesi
  box passes, and the add-back.
- `crates/rawstock-core/src/gpu/grain.wgsl`: the dye-cloud and silver
  field, the aperture blur, and the RMS mix.
- `crates/rawstock-core/src/gpu/tests.rs`: the parity tests.

Buffers use the CPU layout: interleaved RGB, one `f32` per sample. A frame
larger than one storage binding is split into chunks. Per-pixel stages do not
read neighbors, so chunks cannot change the result. A test checks this.

Halation does read neighbors. Every chunk stays on the GPU through
exposure. The excess goes into one full-frame plane, one `f32` per pixel,
which is blurred and then added back to each chunk before the film model.
That plane must fit one storage binding: about 268 megapixels at the 1 GiB
cap. A larger frame gets `GpuError::FrameTooLarge`.

The CPU box blur slides an `f64` running sum along each row. The GPU sums
each output's window directly in `f32`, one thread per sample, so no error
builds up along a row. Out-of-frame taps repeat the edge, as on the CPU.

### Grain

The CPU splats each dye cloud into an `f64` field. The GPU inverts that:
each field sample gathers the clouds that can reach it, cell by cell. Every
draw is `Grain::hash(cell, sequence)`, which is integer-only, so any draw
can be fetched directly and the clouds are the CPU's clouds:

- Knuth's Poisson count compares a running product against `exp(-mu)`. The
  CPU passes that limit in, so both compare against the same bits. The
  products of exact 24-bit units are correctly rounded in WGSL, so the
  counts match.
- Center, size, aspect, angle, and strength come from the same draws.
- Per-process amplitude, mean, and variance come from the same function as
  the CPU (`Process::constants` in `grain.rs`), evaluated on the CPU and
  uploaded.

Only the Gaussian's `exp`, `sin`, and `cos`, the `f32` sum, and its order
differ. A silver clump's profile is a polynomial, so it differs only in the
sum. The profile is a per-process word in the upload, and the tile store
holds one process at a time, so the stored clouds need no profile field.

A process whose clouds are smaller than a pixel is not gathered. Each
sample draws its sum directly, as `Aggregate::sample` does on the CPU: the
cloud count from the same uploaded CDF table, compared against the same
24-bit unit, so the counts match, then a Box-Muller normal, whose `log`
and `cos` are the only difference.

Chunks are whole rows. Each chunk's field is evaluated on a band padded by
the aperture kernel's reach (the scanner, or the pixel aperture when some
clouds are drawn per pixel; its taps are computed on the CPU), so the blur
never clamps at a band edge for a
pixel that is kept, and chunking does not change the result.

The field is the slow part. Gathered per sample, each pixel would redraw
every cell that can reach it, about 150 cells at a full-resolution Portra.
So `grain_field` runs 8×8 tiles (#69): the tile's 64 lanes draw, one cell
per lane, the clouds of every cell that can reach the tile, keep those whose
footprint touches it, and a prefix sum places them in workgroup memory in
cell order. Each pixel then sums the stored clouds, in that order, so the
field is the one a per-pixel gather produces. The store holds 512 clouds
(28 bytes each, inside WebGPU's 16 KiB minimum). A tile that reaches more
sums them in batches, and a single round of 64 cells holding more than the
store, which a Poisson count makes vanishingly rare, falls back to per-pixel
draws. A pixel tests only the ellipse, not the CPU's square footprint: the
ellipse lies inside the square, and the band clip cannot exclude a pixel
inside the band.

The field depends only on the grain's geometry (frame size,
seed, cloud sizes) and the crop, not on its RMS or the picture. So the
device keeps the last two fields of frames up to 8 MP, and a preview edit
with the same grain and crop only runs the mix. The CPU session caches its
`CloudField` the same way.

## Where it runs

- CLI: `rawstock develop … --gpu` (also `develop-linear`). The flag is an
  explicit request, so without an adapter it fails rather than falling
  back. Without the flag the CLI develops on the CPU.
- Preview session: `PreviewSession::set_gpu`. The proxy, the 100% crop, and
  the export try the GPU first and fall back to the CPU on any error. A GPU
  render is always `Fidelity::Exact`.
- Desktop: the tuner opens the device once, on the first file, and hands it
  to every session. `RAWSTOCK_GPU=0` forces the CPU. The log names the
  adapter or the reason for the fallback.
- Web: the module builds for `wasm32-unknown-unknown` with the WebGPU
  backend. The web app does not use it yet.

A full-frame export keeps every chunk and its readback buffer on the GPU at
once, about 24 bytes per pixel: roughly 600 MB for 24 MP.

## Speed

On lavapipe, a software Vulkan rasterizer on 4 CPU cores, release build:

| 1224×816 | CPU | GPU |
| --- | --- | --- |
| Develop, no grain | 1.14 s | 0.41 s |
| Density and grain of a 1224 px frame, field built | 2.3 s | 1.7 s |
| Density and grain, a crop of a 6384 px frame, field built | 3.3 s | 3.4 s |
| Density and grain, field cached | not measured | 0.10 s |

lavapipe runs the shaders on the same CPU cores, so these show the shape of
the work, not hardware speed. The CPU runs grain on at most 4 threads (3 on
this machine). Before #64 and #69 the built field took 9.9 s on the CPU and
10.1 s on the GPU for the 1224 px frame, and 15.6 s on the GPU for the 6384
px crop. #64 draws a sub-pixel process per pixel, and #69 shares each tile's
clouds in workgroup memory.

Setup and readback are async because WebGPU in a browser cannot block.
`Gpu::new_blocking` and `GpuPipeline::develop_blocking` wrap them for native
callers.

## Parity

Tolerances are in each stage's own units, so an error cannot hide in a flat
part of a curve.

| Stage | Tolerance | Measured on lavapipe |
| --- | --- | --- |
| Exposure | bit for bit | 0 |
| Density, parametric and spectral | 2e-5 density | 5e-7 |
| Dye scan | 5e-5 density, relative above 1 | 8.4e-5 at density 4.6 |
| Halation | 1e-5 scene-linear, relative above 1 | 6.6e-7 relative |
| Grain | 1e-3 density per sample, 2e-5 RMS | worst 1.9e-5, RMS 1.1e-7, against grain RMS 2e-2 |
| Print/scan | 1e-4 relative transmission | 2e-7 |
| Encode | 2e-5, encoded `0..=1` | 1.3e-6 |
| Full develop | 8-bit codes: none off by more than 1, at most 0.1% off by 1 | Velvia 3, Neutral 4, Portra 9 (with halation and grain), of 393,216 off by 1 |

Exposure matches bit for bit because the CPU computes `2^ev` once and the
shader multiplies by that same `f32`. WGSL requires multiplication to be
correctly rounded. Other stages use `log2`, `exp2`, and `pow`, which WGSL
allows a few ulps of error. The CPU encode matrix is `f64`. The GPU's is
`f32`.

The dye scan has the loosest bound. A dense patch transmits very little.
The observer rows have negative lobes, so the 41-term sum cancels, and the
log10 of a small cancelled sum amplifies ulp differences. The error is still
invisible after the print curve: Portra's full develop stays within the 8-bit
bound.

Non-finite inputs follow the CPU rules: NaN exposure floors to the toe, a
non-finite curve input reads as the first knot, and a non-finite encode
writes 0. The shader tests the exponent bits rather than `x != x`, which a
compiler may fold away.

## Running the tests

```bash
cargo test -p rawstock-core --features gpu --lib gpu::
cargo test --release -p rawstock-core --features gpu --test gpu
```

The second runs the preview session on the CC0 RAW fixtures, every stock,
CPU against GPU. Time a frame with:

```bash
RAWSTOCK_TIMING_MP=1 cargo test --release -p rawstock-core --features gpu \
  --lib gpu::tests::timing -- --ignored --nocapture
```

`RAWSTOCK_TIMING_FRAME` sets the full frame's width, so a small crop can
time full-resolution grain. `RAWSTOCK_TIMING_GRAIN=0` drops grain.

Without an adapter, the tests print a note and pass. Set
`RAWSTOCK_REQUIRE_GPU=1` to make a missing adapter fail. `WGPU_BACKEND`
(`vulkan`, `metal`, `dx12`) picks a backend.

CI runs them on two software adapters, because GitHub's runners have no GPU:

- Linux: Mesa lavapipe, through Vulkan (`mesa-vulkan-drivers`).
- Windows: WARP, through DX12.

Metal has no software adapter in CI. Run the tests on a Mac.

The `wasm` CI job also checks `--features gpu` on `wasm32-unknown-unknown`,
which builds wgpu's WebGPU backend.
