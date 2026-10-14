# Architecture

## Shared core

All image logic lives in the Rust crate `rawstock-core`. Every front end is a
thin layer over it:

```
rawstock-core (Rust)
 ├─ rawstock-cli      native binary `rawstock`          (Phase 2)
 ├─ apps/desktop      Tauri, native core + wgpu         (Phase 5)
 └─ apps/web          core → WebAssembly, WebGPU        (Phase 6)
```

The desktop and web UIs are TypeScript + Svelte, shared between both builds.
The web app processes images entirely client-side; nothing is uploaded.

## Film model

`FilmModel` maps scene exposure to layer densities. `Parametric` and
`Spectral` are variants of one enum. Prefer the enum over a trait object
unless there is a stated reason not to.

A stock is that model, stored as data (see [stocks/](../stocks/README.md)).
A 3D LUT is only an optional bake for `.cube` export or preview caching.
Desktop hardware is the target, so the model runs directly.

## Pipeline

CPU, GPU, preview, and export share this order:

1. **Decode** (`decode`). `rawler` only: pure Rust, so native and wasm use the
   same decoder. LibRaw is a contingency outside core, never a core
   dependency. First cameras are Fuji X-Trans RAF and iPhone ProRAW DNG.
2. **As-shot white balance and exposure.**
3. **Linear ACEScg** (`color`). Later stages stay in this working space until
   the output transform.
4. **Halation**, when the stock sets it. A threshold, a wide blur, and a
   red-weighted add-back, still in scene-linear exposure.
5. **Film model** (`stock`). Characteristic (D-logH) curves produce per-layer
   density, then interlayer coupling.
6. **Grain**, when the stock sets it. Per-layer dye-cloud noise in density,
   seeded and sized to the physical frame so CPU, GPU, preview, and export
   match.
7. **Print/scan.**
8. **Gamut compression and output transform** (`color`), into the output
   encoding (sRGB, Display P3, and others).
9. **Export.**

## Lab

Core writes a linear ACEScg dump after as-shot white balance and exposure.
The Python lab reads that dump and compares model math with Rust, within a
numeric tolerance. See [lab/](../lab/README.md).

## Why wgpu

wgpu runs one set of shaders (WGSL) on Metal, DX12, and Vulkan natively and on
WebGPU in the browser, so desktop and web share the same renderer. The CPU
path is the source of truth; the GPU path is tested for parity against it,
including the grain hash.
