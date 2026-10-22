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
| Grain | not yet | 4 |
| Dye scan | yes, always exact | 2 |
| Print/scan | yes | 1 |
| sRGB encode | yes | 1 |

`GpuPipeline::new` refuses parameters that need a stage marked "not yet",
with `GpuError::Unsupported`. Callers fall back to the CPU.

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
| Print/scan | 1e-4 relative transmission | 2e-7 |
| Encode | 2e-5, encoded `0..=1` | 1.3e-6 |
| Full develop | 8-bit codes: none off by more than 1, at most 0.1% off by 1 | Velvia 3, Neutral 4, Portra 6 (with halation), of 393,216 off by 1 |

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
```

Without an adapter, the tests print a note and pass. Set
`RAWSTOCK_REQUIRE_GPU=1` to make a missing adapter fail. `WGPU_BACKEND`
(`vulkan`, `metal`, `dx12`) picks a backend.

CI runs them on two software adapters, because GitHub's runners have no GPU:

- Linux: Mesa lavapipe, through Vulkan (`mesa-vulkan-drivers`).
- Windows: WARP, through DX12.

Metal has no software adapter in CI. Run the tests on a Mac.

The `wasm` CI job also checks `--features gpu` on `wasm32-unknown-unknown`,
which builds wgpu's WebGPU backend.
