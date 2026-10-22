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

## In flight

- **#62** — Lateral chromatic aberration correction for X-Trans and Bayer
  (`agent: claude`, `stage: ready`). Fixes pink/blue edge fringes that
  Velvia amplified.

- **#17** — Provia simulation fit (`agent: grok`, `stage: holding`).
  Waiting on more Provia RAF+JPEG pairs from Caleb. Pairs already in hand
  are held out. Do not commit them.
- **#18** — this guide (`AGENTS.md`, `CLAUDE.md`, `PLAN.md`).
- **#20** — Add grain to the Velvia stock (`agent: claude`, `stage: ready`).
  PR #22.
- **#21** — GPU rendering with wgpu, Phase 4 plan (`agent: claude`,
  `stage: working`).
- **#23** — GPU slice 1: wgpu scaffold and per-pixel stages (`agent: claude`,
  `stage: working`).

## Next

After the extra Provia pairs arrive, fit Provia with `lab/fit_stock.py`,
then the remaining Fuji simulations the same way. Portra 400 stays paused
in favor of Fuji simulation matching.

## How to update this file

When you finish or stop, edit this file in the same PR. Move the issue
among Done, In flight, and Next. One or two lines, with the issue number.
Do not paste a commit log. Leave `docs/PLAN.md` alone unless a phase
decision actually changed.
