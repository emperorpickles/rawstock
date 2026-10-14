# Architecture

## Shared core

All image logic lives in the Rust crate `rawstock-core`. Every front end is a
thin layer over it:

```
rawstock-core (Rust)
 ├─ rawstock-cli      native binary `rawstock`          (Phase 2)
 ├─ apps/desktop      Tauri, native core + wgpu         (Phase 4)
 └─ apps/web          core → WebAssembly, WebGPU        (Phase 5)
```

The desktop and web UIs are TypeScript + Svelte, shared between both builds.
The web app processes images entirely client-side; nothing is uploaded.

## Pipeline

1. **Decode** (`decode`): RAW file → sensor data and metadata. Starts with the
   pure-Rust `rawler` (easiest for wasm); LibRaw is the desktop fallback if
   Fuji X-Trans/RAF support falls short. Includes demosaicing.
2. **Linear color** (`color`): camera RGB → a linear, wide-gamut working
   space. All creative work happens in linear light.
3. **Stock** (`stock`, `render`): apply the stock: base 3D LUT (.cube), tone
   curve, grain, halation. Stocks are data files, not code (see
   [stocks/](../stocks/README.md)).
4. **Output transform** (`color`): working space → output space (sRGB,
   Display P3, ...) with encoding, then export (JPEG/TIFF).

## Why wgpu

wgpu runs one set of shaders (WGSL) on Metal, DX12, and Vulkan natively and on
WebGPU in the browser, so desktop and web share the same renderer. A CPU
reference path in `render` is the source of truth for correctness; the GPU path
is tested for parity against it within a tolerance.
