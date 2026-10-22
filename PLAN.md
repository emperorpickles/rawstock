# Plan

Current status for agents. `docs/PLAN.md` is the older phase outline. It
still says `develop` is a stub. Do not treat it as status.

## Done

On `main`, through the Velvia merge (#15):

- Workspace, GPL-3.0-or-later, `rawler` decode of Fuji X-Trans RAF and
  iPhone ProRAW DNG, scene-linear ACEScg, OpenEXR dump (#1–#3).
- Parametric pipeline, schema-1 stock TOML, JPEG/TIFF export (#4, #5).
- Halation and dye-cloud grain (#6). Spectral `FilmModel` and an eye-tuned
  Portra 400 (#7). X-Trans demosaic on this `main` is still bilinear.
- Desktop tuner: Svelte UI in `apps/ui`, Tauri 2 shell, orientation
  (#10–#12).
- Velvia parametric approximation, fitted with `lab/fit_stock.py`
  (CIEDE2000) against in-camera JPEGs (#15).

## In flight

- **#14** — Markesteijn X-Trans demosaic (`claude/edge-artifacts`). Being
  merged. Bayer and linear DNG stay as they are.
- **#13** — grain redesign, tracked by **#16**. CI is green and the branch
  merges cleanly. Waiting on Caleb's visual approval. It has to read as
  real film grain, not digital noise, a pixel grid, or compression
  artifacts.
- **#17** — Provia simulation fit. Waiting on more Provia RAF+JPEG pairs
  from Caleb. Pairs already in hand are held out. Do not commit them.
- **#18** — this guide (`AGENTS.md`, `CLAUDE.md`, `PLAN.md`).

## Next

After the extra Provia pairs arrive, fit Provia with `lab/fit_stock.py`,
then the remaining Fuji simulations the same way. Portra 400 stays paused
in favor of Fuji simulation matching.

## How to update this file

When you finish or stop, edit this file in the same PR. Move the issue
among Done, In flight, and Next. One or two lines, with the issue number.
Do not paste a commit log. Leave `docs/PLAN.md` alone unless a phase
decision actually changed.
