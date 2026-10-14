# Rawstock

Rawstock is a cross-platform app that develops RAW camera files and applies
film simulations, called **stocks**, in the spirit of in-camera film
simulations. A shared Rust core powers a CLI, a Tauri desktop app, and a web
app that runs entirely client-side via WebAssembly and WebGPU.

**Status:** early scaffolding. Nothing is implemented yet.

```
rawstock develop IMG.RAF --stock <name> -o out.jpg   # target shape
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
- [Stock format](stocks/README.md)
