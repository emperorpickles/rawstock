# Rawstock

Rawstock is a cross-platform app that develops RAW camera files and applies
film simulations, called **stocks**, in the spirit of in-camera film
simulations. A shared Rust core powers a CLI, a Tauri desktop app, and a web
app that runs entirely client-side via WebAssembly and WebGPU.

**Status:** `develop` loads a schema-1 stock into the film pipeline
(exposure, optional halation, D-logH curves, optional grain, print/scan,
sRGB) and writes 8-bit JPEG or 16-bit TIFF. `parametric` reads scene RGB as
layer exposure. `spectral` upsamples that RGB and integrates per-layer
sensitivities first. [stocks/portra400.stock.toml](stocks/portra400.stock.toml)
is the first spectral stock.

```
rawstock develop IMG.RAF --stock stocks/neutral.stock.toml --out out.jpg
rawstock dump-linear IMG.RAF out.exr
```

## Layout

- `crates/rawstock-core`: shared core (decode, color, stock, render)
- `crates/rawstock-cli`: the `rawstock` command-line tool
- `apps/desktop`: future Tauri app
- `apps/web`: future wasm + WebGPU app
- `stocks/`: stock definitions and format notes
- `lab/`: Python look-development sandbox
- `docs/`: design and roadmap

## Docs

- [Architecture](docs/ARCHITECTURE.md)
- [Plan](docs/PLAN.md)
- [Film pipeline](docs/FILM_PIPELINE.md)
- [UI plan](docs/UI_PLAN.md)
- [Stock format](stocks/README.md)

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
