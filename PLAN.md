# Plan

Current status for agents. `docs/PLAN.md` is the older phase outline. It
still says `develop` is a stub. Do not treat it as status.

## Done

On `main`, through the Markesteijn merge (#14, `06d863d`):

- Workspace, GPL-3.0-or-later, `rawler` decode of Fuji X-Trans RAF and
  iPhone ProRAW DNG, scene-linear ACEScg, OpenEXR dump (#1–#3).
- Parametric pipeline, schema-1 stock TOML, JPEG/TIFF export (#4, #5).
- Halation and the first dye-cloud grain (#6). Spectral `FilmModel` and an
  eye-tuned Portra 400 (#7).
- Desktop tuner: Svelte UI in `apps/ui`, Tauri 2 shell, orientation
  (#10–#12).
- Velvia parametric approximation, fitted with `lab/fit_stock.py`
  (CIEDE2000) against in-camera JPEGs (#15).
- Grain redesign (#13, closed #16).
- X-Trans demosaic is rawler's 3-pass Markesteijn (#14). Bayer stays PPG.
  Linear DNG still passes through.
- GPU rendering on wgpu (#21): every film stage, checked against the CPU
  (#23, #25, #27, #32), and used by the CLI (`--gpu`), the preview session,
  and the desktop tuner with a CPU fallback (#44). See `docs/GPU.md`.

## In flight

- **#17** — Provia simulation fit (`agent: grok`, `stage: holding`).
  Waiting on more Provia RAF+JPEG pairs from Caleb. Pairs already in hand
  are held out. Do not commit them.
- **#18** — this guide (`AGENTS.md`, `CLAUDE.md`, `PLAN.md`).
- **#20** — Add grain to the Velvia stock (`agent: claude`, `stage: ready`).
  PR #22.
- **Performance** (`agent: claude`). rawler optimized in dev builds (#61).
  Sub-pixel grain drawn per pixel, and grain painted in parallel on a pool
  capped at 4 threads with one core free (#64). X-Trans opens on a
  third-size develop while Markesteijn runs in the background, and the GPU
  opens at startup (#67). Next: GPU grain with tile-shared clouds, then the
  other CPU film stages on the capped pool.

## Next

After the extra Provia pairs arrive, fit Provia with `lab/fit_stock.py`,
then the remaining Fuji simulations the same way. Portra 400 stays paused
in favor of Fuji simulation matching.

## How to update this file

When you finish or stop, edit this file in the same PR. Move the issue
among Done, In flight, and Next. One or two lines, with the issue number.
Do not paste a commit log. Leave `docs/PLAN.md` alone unless a phase
decision actually changed.
