# Rawstock agent guide

Rawstock is a GPL-3.0-or-later Rust RAW developer with film stocks. A shared
core is the image path. Front ends are thin layers over it, in this order:
CLI, Tauri desktop, then a fully client-side web app (core as wasm). Never
upload images. First cameras: Fuji X-Trans RAF and iPhone ProRAW DNG. Looks
are stocks. Processing a file is developing.

## Layout

- `crates/rawstock-core` — decode, color, stock, render.
- `crates/rawstock-cli` — the `rawstock` binary.
- `apps/ui` — TypeScript + Svelte, shared by desktop and web.
- `apps/desktop` — Tauri 2 shell. `apps/desktop/src-tauri` is a workspace member.
- `apps/web` — future wasm + WebGPU shell. It does not decode yet.
- `lab/` — Python model math and the stock fitter. It does not decode.
- `stocks/` — versioned stock TOML.
- `docs/` — architecture, the film-pipeline writeup, the UI plan, and the
  older phase outline.

GPU, when it exists, is wgpu: one shader set on Metal, DX12, Vulkan, and
WebGPU, checked against a CPU reference. Decode stays on `rawler` (pure
Rust, wasm-friendly). LibRaw is a contingency outside core, never a core
dependency. Keep dependencies minimal and GPL-3.0-compatible (MIT, Apache,
BSD, LGPL). `rawler` is LGPL-2.1-only and is fine to link.

## Pipeline

Implemented order:

1. **Decode** with `rawler`. Bayer uses PPG. Linear DNG (ProRAW) is already
   demosaiced and passes through. X-Trans uses rawler's 3-pass Markesteijn.
2. **White balance, then ACEScg.** As-shot white balance is in camera RGB,
   baked into the camera → ACEScg matrix. ACEScg (scene-linear AP1) is the
   working space. **Exposure (`exposure_ev`) is a stop scale after that
   conversion, still in ACEScg, before halation.** It does not run before
   the working space. `dump-linear` does not apply it.
3. **Halation**, when the stock sets it.
4. **Film model.** Parametric reads scene RGB as three layer exposures.
   Spectral upsamples and integrates per-layer sensitivities. Both then
   apply D-logH curves and interlayer coupling. `FilmModel` is an enum,
   not a trait object, unless there is a stated reason.
5. **Grain** in density, when the stock sets it.
6. **Print/scan.** A spectral stock with dye curves converts to printing
   density first, read by the eye observer or by `[scan_response]`
   (Status M for a negative). Otherwise characteristic density is printed.
   The print curve gives print density; with `[print_dye_density]` the
   paper's own dyes are viewed under D65, otherwise ideal ACEScg dyes.
7. **Output.** ACEScg to encoded sRGB. The code clips out-of-gamut
   channels. There is no separate gamut-compression stage.

Stocks are parametric or spectral models, never LUTs. A 3D LUT is only an
optional `.cube` export bake or preview cache. **ACEScct is the shaper for
that bake.** Nothing in the tree encodes ACEScct or writes a `.cube` yet.
Halation and grain stay outside any lattice: they need neighbors and a
frame size.

Hand-tuned stock values must be labeled approximate, in the file's
`source` and in comments. Fuji simulation stocks are fitted against
in-camera JPEGs with `lab/fit_stock.py`, which minimizes CIEDE2000. That
fit has no LUT.

Grain must look like real film grain. It must never look like digital
noise, a pixel grid, or compression artifacts.

## Code

Prefer structs with associated functions over piles of module-level
helpers.

Never weaken a failing check or test to make it pass. Fix the root cause.
That includes clippy `-D warnings`, `RAWSTOCK_REQUIRE_FIXTURES`, and the
UI check.

## Checks

CI runs these. All of them must pass before pushing.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RAWSTOCK_REQUIRE_FIXTURES=1 cargo test --workspace
```

In `apps/ui` (Node 22):

```bash
npm ci && npm run check && npm run build
```

Wasm (needs the `wasm32-unknown-unknown` target):

```bash
cargo check -p rawstock-core --target wasm32-unknown-unknown
```

CI also runs `npm ci` in `apps/desktop`. Develop a file with:

```bash
cargo run -p rawstock-cli -- develop IMG.RAF --stock stocks/neutral.stock.toml -o out.jpg
```

Lab setup, only when model math or the fitter changes:

```bash
cd lab && python3 -m venv .venv && . .venv/bin/activate && pip install -r requirements.txt
python3 test_stockfit.py
```

## Privacy

Never commit Caleb's photos: RAF/JPEG reference pairs or full-size
renders. Never commit RAW files (`*.RAF`, `*.DNG`, `*.CR3`, `*.NEF`,
`*.ARW`, any case) or anything in `test-images/`. Check `git status`
before committing. Visual proof goes in the PR body as hosted images,
never as committed files.

## Coordination

Two agents share this repo. Claude uses Claude Code and `claude/`
branches. Grok uses Cursor cloud agents and `grok/` branches.

Every piece of work has one GitHub issue, with exactly one `agent: claude`
or `agent: grok` label and one `stage: working`, `stage: ready`, or
`stage: holding` label. Only the labeled agent edits code for that issue.
Do not touch files another agent's open PR is changing without
coordinating on the issue.

PRs are the hand-off. Link the issue (`Closes #N`). Either agent may
review the other's PR with comments. CI green plus a clean merge means
ready. Only Caleb, or Grok on his behalf, merges. Merge commits only,
never squash. Rebase onto `main` only when there is a real conflict.

When you finish or stop, update the root `PLAN.md`.

## License

GPL-3.0-or-later, so this is not reused in closed-source projects. Confirm
the `rawler` LGPL-2.1 link before a release.
