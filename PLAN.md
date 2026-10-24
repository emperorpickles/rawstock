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
- 1.0 app open, filmstrip, and `.rawstock.toml` sidecars (#37): files and
  folders, capture order, per-image undo, and autosave beside the RAW.
- 1.0 app stock browser (#38): thumbnails of every installed stock drawn
  from the open photo, grouped by category in a 2-column grid, keyboard
  navigable, applied with undo. Thumbnails are the first real film
  renders in `apps/rawstock` (`thumbnails.rs`: one quick decode per photo,
  grain and halation off). Other develops are still the contract stub.
- 1.0 app real develop (#87): `develop.rs` replaces the stub for previews
  and full frames with a `PreviewSession` (quick X-Trans open, Markesteijn
  in the background, GPU with CPU fallback), cancelled between rows via
  the job board.
- 1.0 app export (#41, #101): Grok's dialog (quality, save path,
  overwrite confirmation, cancel) now writes a real JPEG. The develop
  thread renders the full frame with turns and crop, writes
  `<dest>.partial`, and the board renames it only while the job is live.
- 1.0 app CA in the fit preview (#106): the quick X-Trans develop now
  corrects lateral CA and defringes, and when Markesteijn lands the develop
  thread emits `image:refined` so the fit preview is developed again from
  the full frame.
- 1.0 app crop and straighten (#92): `rawstock_core::geometry::Framing`
  applies quarter turns, straighten (scaled to fill, no empty corners) and
  the crop. Export and thumbnails use it, and the viewer draws the same
  mapping at Fit, 100% and 200%, so all four frame the photo alike. Crop
  handles and draw-a-horizon work zoomed, by mouse and keyboard, with a
  grid while straightening. Geometry edits never develop again: frames
  stay in camera orientation and the viewer frames them.
- 1.0 app custom white balance (#88): temperature and tint map through the
  camera matrix (`white_balance.rs`), and custom at the file's as-shot
  reading is bit-identical to as shot. The session keeps the demosaiced
  camera RGB, so a change converts the fit proxy at once (about 2 ms on a
  24 MP RAF) and the full frame only when a 100% view or export needs it.
  Preview, full, export and thumbnails all use it. Files that cannot take
  one develop as shot and the panel says so.
- Unsigned alpha bundles of the 1.0 app (#93): `.github/workflows/alpha.yml`
  on a `v*` tag or by hand drafts a release with a Windows NSIS installer,
  a universal macOS `.dmg`, and checksums. Stocks ship inside the bundle.
- Ilford HP5 Plus, the first black and white stock (#76):
  `emulsion = "monochrome"` (one silver layer, silver grain, CPU and GPU),
  curves digitized from Ilford's datasheet charts by `lab/ilford_bw.py`.
- 1.0 app adjust panel (#39, #86): exposure, print, dye, contrast, grain,
  and halation, as-shot white balance, scan look, and crop, straighten, and
  rotate. Warm and neutral themes. A zoomed drag develops the full frame
  once on release, and preview events that beat the command are buffered.

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
- **#73** — Spectral Velvia brought in line with the parametric one
  (`agent: claude`, `stage: ready`, waiting on Caleb's look review).
  Exposure placed for camera files, a new spectral `neutral_scale` key
  (per-channel neutral scan balance, CPU and GPU), and an approximate
  saturation coupling.
- **#96** — Spectral Velvia matched closer to the camera's Velvia on
  DSCF7392 (`agent: claude`, `stage: ready`, waiting on Caleb's look
  review). New color-spectral key `violet_cutoff_nm` fades layer
  sensitivity below 440 nm (the upsampler's violet lobe turned reds
  magenta), a faint neutral halation, and the defringe no longer
  desaturates saturated reds into gray outlines. Its Status M scan was
  removed again (too-red skin, too-green foliage).
- **#79** — Scale-correct grain and silver grain (`agent: claude`, `stage:
  ready`, waiting on Caleb's look review). The field is calibrated through
  the 48 µm aperture, so a preview matches the export shrunk to its size;
  stock RMS rescaled so 100% exports keep their look. Monochrome stocks
  draw hard-edged, skewed silver clumps (CPU and GPU).
- **#81** — White grain from film scans (`agent: claude`, `stage: ready`,
  stacked on #79, waiting on Caleb's look review). `texture = "white"`
  with layer `correlation` and soft `clumping`, measured on V550 scans of
  Portra 400 and HP5 Plus; Portra, the Velvias, and HP5 use it. Portra
  grain is about 8× faster.
- **#40** — 1.0 app review view (`agent: claude`, `stage: ready`). Fit
  shows the proxy; 100% and 200% pan one `develop_full` per settings state
  as a client texture (`get_region` past 128 MB or under memory pressure),
  with no develop while panning. Split with a keyboard divider, A/B (`\`),
  and a clipping overlay (`J`). Logic in `apps/rawstock/src/lib/review/`.
  The backend still returns the stub's placeholder frames until the real
  develop service lands.
- **#91** — 1.0 app Darkroom layout (`agent: claude`, `stage: ready`,
  waiting on Caleb's look review). The filmstrip runs under the viewer and
  folds away (saved as `layout.filmstripCollapsed`). View > Split on `Y`.
  A resize while zoomed no longer re-develops the proxy.
- **#52, #53** — 1.0 app presets and export queue (`agent: claude`,
  `stage: holding`) until #39 and #41 land.
- **#63** — Lavapipe apt timeout and retry in CI (`agent: grok`). GPU
  parity jobs cap at 40 minutes, and a hung apt mirror fails fast and
  retries.

## Next

After the extra Provia pairs arrive, fit Provia with `lab/fit_stock.py`,
then the remaining Fuji simulations the same way. Other spectral_film_lut
negatives (Portra 160/800, Ektar, Gold) can reuse the #60 importer path.

## How to update this file

When you finish or stop, edit this file in the same PR. Move the issue
among Done, In flight, and Next. One or two lines, with the issue number.
Do not paste a commit log. Leave `docs/PLAN.md` alone unless a phase
decision actually changed.
