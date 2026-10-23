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
- 1.0 app (`apps/rawstock`): Preferences and a per-user settings store
  (#49), the Help menu with About, generated Licenses, Report an issue and
  a rotating log (#50), and the first-launch welcome with a shortcut cheat
  sheet (#51).
- Chromatic aberration correction for X-Trans and Bayer: red/blue
  registration (#62) and a blue/purple defringe, behind one switch in
  core, the CLI (`--no-ca-correction`) and the tuner (#71).

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
  opens at startup (#67). GPU grain draws each tile's clouds once in
  workgroup memory (#69). Next: the other CPU film stages on the capped
  pool.

- **#60** — Portra 400 refresh (`agent: claude`, `stage: ready`). Re-imported
  with a Status M scan, the Portra Endura paper curve and dyes, and grain
  retuned for the paper's contrast. Adds `[scan_response]` and
  `[print_dye_density]` to schema 1, CPU and GPU.
- **#52, #53** — 1.0 app presets and export queue (`agent: claude`,
  `stage: holding`) until #39 and #41 land.

## Next

After the extra Provia pairs arrive, fit Provia with `lab/fit_stock.py`,
then the remaining Fuji simulations the same way. Other spectral_film_lut
negatives (Portra 160/800, Ektar, Gold) can reuse the #60 importer path.

## How to update this file

When you finish or stop, edit this file in the same PR. Move the issue
among Done, In flight, and Next. One or two lines, with the issue number.
Do not paste a commit log. Leave `docs/PLAN.md` alone unless a phase
decision actually changed.
