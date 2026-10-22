# apps/desktop

Tauri 2 shell for the stock tuner. The Svelte UI lives in [`apps/ui`](../ui)
and is shared with the future web app. This crate owns the window, the file
dialogs, and the one develop thread. `rawstock-core` stays a library: no
windowing crate and no `std::thread`.

Milestone 1 is the CPU preview. The GPU renderer replaces that preview when
Phase 3 lands. See [UI plan](../../docs/UI_PLAN.md).

## Run from a checkout

Install a stable Rust toolchain and Node 22. Then:

```sh
cd apps/ui && npm install
cd ../desktop && npm install
npm run dev
```

The first launch compiles the Rust shell. Open a Fuji RAF or an iPhone
ProRAW DNG, then pick a stock. Bundled stocks are read from the repo
`stocks/` directory.

`RAWSTOCK_OPEN` and `RAWSTOCK_STOCK` open a file on startup.
`RAWSTOCK_STOCK` may be a path or a file name in `stocks/`.
`RAWSTOCK_STOCKS` overrides that directory.

Previews and exports render on the GPU when one is found, and on the CPU
otherwise. `RAWSTOCK_GPU=0` forces the CPU. The adapter is printed to the
terminal on the first open.

### macOS

Xcode command line tools are enough. The system WebKit is the webview.

```sh
cd apps/ui && npm install
cd ../desktop && npm install
npm run dev
```

### Windows

WebView2 and the Visual Studio C++ build tools are required. From PowerShell
in the repo:

```powershell
cd apps/ui; npm install
cd ../desktop; npm install
npm run dev
```

`npm run dev` and `npm run build` start or build `apps/ui` with
`npm --prefix ../ui`. Those hooks run in PowerShell.

### Linux (Debian or Ubuntu)

```sh
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev patchelf
cd apps/ui && npm install
cd ../desktop && npm install
npm run dev
```
