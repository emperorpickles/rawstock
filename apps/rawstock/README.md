# apps/rawstock

The rawstock 1.0 desktop shell. Tauri 2 owns the window, native menus, and
file associations. The Svelte UI in this directory is not the tuner in
`apps/ui`. `rawstock-core` stays a library: no windowing crate and no
`std::thread`.

The develop commands in `src-tauri` are the typed contract from issue #36.
They assign job ids, enforce the cancellation rules, and emit progress and
cache events. Previews and full frames run the real `rawstock-core` develop
(`develop.rs`, GPU with a CPU fallback), and stock thumbnails run in
`thumbnails.rs`. Export is still a stub. There is no onboarding sample.

Edits are written to `<file>.rawstock.toml` next to the photo. The RAW file
is never written, renamed, or touched. A sidecar that cannot be parsed is
left in place until Reset edits renames it to `.bak`.

## Alpha builds

`.github/workflows/alpha.yml` builds unsigned bundles: an NSIS installer for
Windows x64 and a universal `.dmg` for macOS. It runs when a `v*` tag is
pushed, which drafts a GitHub release with the bundles and `.sha256`
checksums for a person to publish, or by hand from the Actions tab, which
keeps them as workflow artifacts. It never runs on pull requests. The
bundles are not signed, so SmartScreen and Gatekeeper warn on first launch;
[`ALPHA_NOTES.md`](ALPHA_NOTES.md) (the release notes) says how to open the
app anyway. The stock files ship inside the bundle.

## Run from a checkout

Install a stable Rust toolchain and Node 22. The commands below are the same
in macOS Terminal, Linux, and Windows Git Bash or cmd. WebView2 and the
Visual Studio C++ build tools are required on Windows. On Debian or Ubuntu:

```sh
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev patchelf
```

```sh
cd apps/rawstock
npm ci
npm run tauri dev
```

The first launch compiles the Rust shell. `npm run dev` starts the UI on
port 1430, which is separate from the tuner. Open a Fuji RAF or an iPhone
ProRAW DNG from the File menu, or drop one on the window. `.raf` and `.dng`
are registered as file associations for a bundled build.

`npm run check` typechecks the UI and runs the `node --test` unit tests. `npm run build` writes `dist/`, which
`cargo` embeds for a release build. A dev compile creates an empty `dist/`
if that folder is missing so the shell can start from a fresh checkout.

Stocks are read from the repo `stocks/` directory. `RAWSTOCK_STOCKS`
overrides that path.

The app config and log directories use the identifier `dev.rawstock.app`,
so they do not collide with the tuner.

## Licenses and the log

`npm run dev` and `npm run build` first run `scripts/licenses.mjs`. It reads
`cargo metadata` for `rawstock-app` and the production packages in
`package-lock.json`, and writes `src/generated/licenses.json` (gitignored)
for Help > Licenses. It fails the build when a dependency's license is not
on the GPL-3.0-compatible allowlist in `scripts/spdx.mjs`. Run it alone with
`npm run licenses`.

The app writes `rawstock.log` to its log directory. Each line has a UTC time
and a level, and the file rotates to `rawstock.log.1` at 5 MB. Help > Show
Log File reveals it. Nothing is uploaded.
