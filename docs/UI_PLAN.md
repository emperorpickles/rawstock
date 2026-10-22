# UI plan

Caleb needs a desktop tool that opens his own Fuji X-Trans RAF and iPhone
ProRAW DNG files, develops them with a stock, and lets him move that stock's
parameters while looking at the picture. This is the first slice of the
desktop app in [PLAN.md](PLAN.md) Phase 5. It runs the CPU pipeline already
in `rawstock-core`. Phase 4 (wgpu) replaces the preview implementation
later, on the same controls. Phase 6 (wasm + WebGPU) reuses the same Svelte
UI. Those phases stay as written. The tuner exists so stock work can happen
before the GPU renderer does.

The picture is the pipeline in [ARCHITECTURE.md](ARCHITECTURE.md), in this
order:

1. Decode and demosaic (`rawler`: bilinear X-Trans, PPG Bayer, linear DNG
   passed through).
2. As-shot white balance and camera RGB → scene-linear ACEScg.
3. `exposure_ev`.
4. Halation, when the stock has it.
5. Film density. Parametric reads scene RGB as three layer exposures.
   Spectral upsamples ACEScg and integrates per-layer sensitivities. Both
   then apply the D-logH curves and the coupling matrix.
6. Grain, when the stock has it: dye-cloud shot noise in density. The
   model is the one in [FILM_PIPELINE.md](FILM_PIPELINE.md).
7. Dye scan, when a spectral stock has `dye_density`. Otherwise this step
   is a no-op and the print sees characteristic density.
8. Print/scan (`print_exposure - density` through the print curve).
9. ACEScg → encoded sRGB, then JPEG or TIFF.

`Pipeline::develop` runs steps 3–9 as one call. It has no cache, no
progress, and no cancel. The API work below splits that call so a tweak
re-runs only the tail.

Two files this plan is written against: the X-S10 fixture is 6384×4182, and
the iPhone 12 Pro ProRAW fixture is 4032×3024. A 40 MP RAF is about one and
a half times the X-S10, and one full ACEScg buffer of it is about 460 MB.

## Recommendation

Use **Tauri 2 + TypeScript/Svelte**, the stack Phase 5 and Phase 6 already
name. The Svelte components live in `apps/ui`. The Tauri shell lives in
`apps/desktop`. `rawstock-core` stays a library: no windowing crate, no
`wgpu` yet, no `std::thread`. The desktop shell owns the one worker thread.
The web app later calls the same core from a Web Worker.

The wasm constraint does not pick the toolkit. All three options below can
keep the core wasm-safe, as long as the UI stays out of the crate and the
render session does not spawn threads or read files itself. The web phase
is what picks the toolkit: Phase 6 is the same Svelte UI on wasm, and a
Rust-native widget crate would be a second interface to build and then
leave behind.

| | egui / eframe | Tauri 2 + Svelte | iced |
| --- | --- | --- | --- |
| Fits Phase 5 and Phase 6 | A separate UI. eframe can run on wasm, which would retire the Svelte decision. | This is the Phase 5 shell, and `apps/ui` is what Phase 6 ships. | A separate UI. Its wasm port is younger than the Svelte one this roadmap already chose. |
| Time to a first window | Shortest. One Rust crate, sliders are the easy part, the preview is a texture. | A Node/Svelte/Tauri scaffold, then a thin shell over the same core API. Milestone 1 keeps that shell small. | More widget work than egui for a curve editor and a split view. |
| Calling the core | Direct, in-process. | In-process. The picture has to cross into the webview as bytes. | Direct, in-process. |
| Later GPU preview | A texture upload from the CPU, or a rewrite into eframe's wgpu. | Unchanged. Phase 4 swaps the stage functions the worker calls. | iced's own renderer is wgpu, which is a different path from the shared WGSL renderer Phase 4 is for. |
| License | MIT / Apache-2.0. | MIT / Apache-2.0, including the shell. | MIT. |

egui is the right choice for a throwaway lab window. This tuner is the
start of the desktop app, so that head start would be paid back when the
controls are written again for Phase 6. iced has the same problem, and it
adds less of a widget set for curves and a before/after wipe.

Tauri's cost is the webview boundary. A 1440×944 RGB8 preview is about
4 MB. The worker returns it through `tauri::ipc::Response` (raw bytes).
A JSON array of numbers would be the wrong transport and would dominate
the render. The window must not call `Pipeline::develop` on the UI
thread: a spectral frame can take seconds, and the window would freeze.
The UI sends a job and paints whatever bitmap comes back with a matching
generation id.

GPL-3.0-or-later can link this shell. No new crate goes into
`rawstock-core` for the UI.

## How a tweak stays live

### What is cached

Opening a file runs decode, demosaic, and ACEScg once. An X-Trans file
opens on a third-size develop first (`RawImage::develop_quick_linear`, each
3×3 block's per-color means), which takes about 0.2 s where Markesteijn
takes about 5 s. The fit proxy is built from it, so the first preview does
not wait. Markesteijn runs on a thread of its own, on its own rayon pool so
the preview's grain is not queued behind its tiles, and
`PreviewSession::install_full` swaps it in when the worker is idle or when a
100% view or export needs it. That full-resolution ACEScg image stays for
the life of the file. Changing a stock does not
touch it. White balance is already baked into that image, which is the
CLI's behavior.

The fit view is a box-downsampled proxy of that ACEScg image, built in
core (`LinearImage` area downsample, after demosaic, so X-Trans and
ProRAW share one path). The shell asks for a long edge:

- **720** while a control is moving. The X-S10 frame becomes about
  720×472 (0.34 MP). The iPhone frame becomes about 720×540.
- **1440** once the worker has caught up and been idle for a short beat
  (~150 ms). The X-S10 frame becomes 1440×944 (1.36 MP).

The widget scales that bitmap to the window. That scale is display only.
It is not another pass of the film pipeline.

100% is a crop of the full ACEScg frame, the visible rectangle plus a pad
for the halation blur. While the pointer is down, that crop uses the same
lookup-table preview as the fit view and is marked `preview`. When the
pointer stops, the worker re-renders the crop with the exact dye scan and
the mark changes to `exact`. Panning therefore stays responsive, and the
picture you leave on screen is the one export will agree with. Halation
sizes its blur from the full frame (`sigma = radius × shorter side`).
For Portra's `radius = 0.008` on the X-S10, sigma is about 33 px, so the
pad is a few dozen pixels. Grain is placed in millimetres on `frame_mm`,
with pixel pitch `frame_mm / full_width`, and the crop's origin is a
millimetre offset into that frame. A crop that used its own width as the
frame would enlarge every cloud and disagree with `rawstock develop`.
The film model, grain, dye scan, and print then run only on the visible
crop. Grain walks only the Poisson cells that overlap that crop, plus
the cloud margin.

Grain is why 100% exists. `size_mm` is the primary dye-cloud sigma
(Portra is 0.018, 0.013, and 0.010 mm). On the X-S10 those clouds are
about 3.3, 2.4, and 1.8 pixels, and the shared field uses their geometric
mean. On the 1440 proxy the same clouds are smaller than a pixel, so the
proxy draws each pixel's summed grain directly, with the export's
statistics but not its individual clouds. The fit view is for tone and color. The 100% view is for grain. The fit view
says so on screen. The 48 µm aperture only sets amplitude (`rms / 1000`
while that window covers at least one pixel). It is not a tile. The 720 px
drag proxy puts that window just under one pixel, so amplitude there is a
hair lower than at 1440; the settled proxy is the one to trust for amount.

### Which stage re-runs

The session stores the proxy buffers between stages. An edit dirties the
first stage it affects, and everything after that stage. Earlier buffers
stay.

| Edit | First stage that re-runs | Kept as-is |
| --- | --- | --- |
| New file | Decode, demosaic, ACEScg | — |
| `exposure_ev` | Exposure scale | ACEScg, both sizes |
| Halation threshold, radius, weights, on/off | Halation | Exposure-scaled image |
| Layer curves, coupling, sensitivities | Film density | Post-halation exposure |
| Grain, including the amount multiplier | Grain | Characteristic density |
| Dye curves or scales, `reference_log_exposure` | Dye scan | Grained characteristic density |
| `print_exposure`, print curve | Print/scan | Dye-scan output, or grained density when the stock has no dyes |
| Output encoding | sRGB encode | Print-linear RGB |

A grain drag does not redo decode, demosaic, ACEScg, exposure, halation,
or film density. A print-exposure drag does not redo the dye scan. A dye
drag does not re-grain. This matches the order in `Pipeline::develop`:
grain is applied to characteristic density, and the dye scan reads the
grained density.

The full-resolution cache keeps the ACEScg frame and the downsampled
proxy. It does not keep every intermediate at full resolution. Six full
X-S10 buffers would be about 1.8 GB. A 100% crop is small enough to
recompute from ACEScg, using the same dirty rules on the crop.

The grain cache boundary stays after characteristic density and before
the dye scan and the print. `Grain::apply` walks every Poisson cell on
the 35 mm frame, so a smaller proxy does not make a rebuild cheaper. The
session caches the unit-variance field for the proxy and for the current
crop. The amount slider multiplies that field by `rms` and does not
rebuild clouds. `seed`, `frame_mm`, and `size_mm` do rebuild it, and a
fit-view rebuild waits until the pointer stops. A 100% rebuild walks
only the cells over that crop. The UI edits a `Grain` value and
`Stock::to_toml` writes it. Milestone 1 does not grow a knot editor for
the RMS tables.

### The worker

`apps/desktop` starts one worker thread. That thread owns the session.
The UI thread sends messages (open this path, set this parameter, render
this crop, export) and receives bitmaps and progress. It does not share
the pixel buffers.

Each edit bumps a generation counter and sets a cancel flag. The worker
checks the flag between stages and between rows of the long stages
(spectral density, dye scan, halation, grain). A cancelled job returns
`Cancelled` and no image. The previous bitmap stays on screen until a job
with the latest generation finishes. A partial buffer is never displayed
and never exported.

Core does not start that thread. The cancel hook is a trait method,
`fn cancelled(&self) -> bool`, so the desktop shell can back it with an
`AtomicBool` and a wasm caller can back it with a `Cell<bool>` or a
worker message. Progress is the same trait: stage id plus rows done. The
CLI passes a no-op and keeps calling `Pipeline::develop`.

### CPU latency

Measured with Kodak Portra 400 on a release build, one core of a 4-vCPU
Intel Xeon under KVM. The clock is `RAWSTOCK_FIXTURES=/tmp/rawstock-fixtures
cargo test -p rawstock-core --release --test session release_preview_timings
-- --ignored --nocapture`. The same `PreviewSession` calls are what the
desktop worker makes. Decode is once per open. A proxy render is the fit
view. Full export is the exact frame plus the JPEG write.

| | X-S10 RAF, 6384×4182 | iPhone 12 Pro DNG, 4032×3024 |
| --- | --- | --- |
| Decode, demosaic, ACEScg | 1.07 s | 0.35 s |
| 720 proxy, cold (dye-cloud field, density, dye table) | 3.02 s | 3.49 s |
| 720 proxy, grain amount (cached field) | 0.04 s | 0.05 s |
| 720 proxy, exposure (field reused) | 0.09 s | 0.11 s |
| 1440 proxy, cold (new field) | 4.98 s | 5.77 s |
| 1440 proxy, print exposure | 0.09 s | 0.11 s |
| Full exact export, JPEG | 48.2 s | 25.3 s |

The Portra grid is 41 wavelengths (380–780 nm at 10 nm). The exact dye
scan calls `10f32.powf` once per wavelength per pixel. The fit view, and a
100% crop while a control is moving, look the dye scan up in a 48³ table.
Grain stays exact: the dye-cloud field is splatted first, then the grained
density looks up the table. Building that table is skipped until the dye
curves or the reference log exposure change. Parametric stocks skip both
spectral stages.

The cold proxy rows are the first splat of the dye-cloud field. That walk
is in the film plane, so a 720 px proxy and a 1440 px proxy cost a few
seconds each rather than scaling with pixel count. The amount slider keeps
the field and only remultiplies RMS, which is the 0.04 s row. An exposure
drag recomputes spectral density and reuses the field, which is the 0.09 s
row at 720. A `size_mm` change rebuilds the field, and a 100% view rebuilds
only the crop. A print-exposure drag stays on the exact print curve and is
the 0.09 s row at 1440. Halation is three box passes. The full-export row
is the exact dye scan plus a full-frame splat and the JPEG write.

The table is a function of the dye curves and `reference_log_exposure`
only. A dye-scale drag rebuilds it. The cancel flag keeps the newest drag
as the one that finishes. The settled 100% crop and every export use the
exact dye scan. The widget marks the bitmap `preview` or `exact` so a
proxy is not judged as the file that will be saved.

Print-only edits skip the table rebuild and the film model. On this
machine that is under a tenth of a second at 1440. Full export stays on
the exact path, on demand, with the progress events on screen. The JPEG
row in the table above is that export for Portra 400. A parametric stock
skips the spectral stages, so its export is well under those times.
Decode and demosaic happen once per open and are then cached. ProRAW
skips demosaic. X-Trans pays for bilinear, which is the 1.07 s RAF open.

### What the GPU adds later

Phase 4 runs the same stages. The session, the dirty rules, the
controls, and the TOML writer stay. The CPU functions remain the
reference and the thing the parity tests call.

Once the ACEScg image is on the GPU, a fit preview or a 100% crop is one
dispatch of a megapixel or two: a few milliseconds on an integrated GPU,
faster on a discrete one. Exposure is a uniform. The dye `powf` and the
spectral upsample move into the shader, which is the whole reason the
CPU proxy needs a lookup table. Halation is a separable blur. The cloud
draws come from an integer hash, so a WGSL port can match them; the
splat is the part that moves onto the GPU.

A full export becomes a dispatch plus a readback, on the order of tens
to a couple of hundred milliseconds for the shader, then the existing
CPU JPEG or TIFF encode. Decode and demosaic stay on the CPU, once per
file. The upload of a 300–460 MB ACEScg buffer is a one-time cost at
open. The CPU lookup table goes away for the interactive path when this
lands. Export parity is still against the CPU.

## Controls

The panel follows pipeline order. A control writes the stock field it is
named after. The shell does not format TOML; it edits core values and
`Stock::to_toml` writes them.

Suggested ranges are starting points. A file value outside the range still
displays, and the slider grows to include it.

| Control | Stock field | When | Starts at |
| --- | --- | --- | --- |
| Stock list, plus Open stock… | the whole file | Milestone 1 | `stocks/*.stock.toml` and a chosen path. Shows `name`, model, revision. |
| Exposure | `exposure_ev` | Milestone 1 | −3 to +3 stops, step 0.01. Portra is −2.53. |
| Halation on/off | omit `[halation]` when off | Milestone 1 | On when the file has the table. |
| Halation threshold | `halation.threshold` | Milestone 1 | Log slider, about 0.01–4.0 in scene-linear units after exposure. Portra is 0.25, and middle gray after its exposure is about 0.03. |
| Halation radius | `halation.radius` | Milestone 1 | 0–0.05 of the shorter side. Portra is 0.008. |
| Halation red weight | `halation.weight[0]` | Milestone 1 | 0–1. Green and blue stay as loaded until Milestone 2. |
| Halation green and blue | `halation.weight[1..]` | Milestone 2 | 0–1. |
| Red, green, blue curves | `[red]`, `[green]`, `[blue]` | Milestone 2 | Knot chart plus a numeric list. Portra has 31 knots; the chart has to handle that, not only Neutral's 6. |
| Coupling | `[coupling].matrix` | Milestone 2 | 3×3 fields. Cleared means omit the table. A "normalize rows" button is offered because Portra's rows sum to 1; the loader does not require it. |
| Dye scale, three sliders | baked into `[dye_density]` | Milestone 1, spectral only | 0.25–2.5, step 0.01, starting at 1 on the file as loaded. Hidden on a parametric stock. |
| Reference log exposure | `reference_log_exposure` | Milestone 2, spectral only | Numeric. Portra is −1.505. |
| Grain amount | scales each layer's `rms` | Milestone 1 | 0–2, step 0.01. 1 is the file. 0 omits `[grain]` on save. Multiplies the cached field. |
| Grain seed, `frame_mm`, `size_mm` | `[grain]` | Milestone 2 | Seed, frame width, and the three dye-cloud sizes. Rebuilds the field. |
| Print exposure | `print_exposure` | Milestone 1 | 0–2.5. Neutral is 0.9, Portra is 1.28. |
| Print curve | `[print]` | Milestone 2 | Same knot editor as the layer curves. |

Sensitivities and `wavelength_nm` are measured data. The panel shows them
as read-only. The model tag is a read-only badge, `parametric` or
`spectral`.

The Portra file already has the eye-tuned dye scales 1.45, 0.98, and 0.95
baked into `dye_density`. Opening it shows 1.0 on the sliders, meaning
"the file as written." The session keeps the dyes from load as the base
and the three sliders as multipliers. Save writes `base × scale`. After a
successful save the session stores that product as the new base and resets
the sliders to 1, so a second save does not square the scale.

Curve drags are clamped to the loader's rule: log exposure strictly
increases, density does not decrease. The chart states that rule. An
illegal edit never reaches the renderer.

### Before / after, zoom, save

**Before / after.** "Before" is the stock as opened, or as last saved.
Milestone 1 is an A/B toggle of the current view, fit or 100%. The before
bitmap is its own cached render, invalidated by the same stage rules.
Milestone 2 adds a horizontal wipe. Neutral is a stock in the picker, not
a third compare mode.

**Zoom.** Fit, and 100% with pan. The 100% rectangle is in display space;
the session maps it back to sensor space. Scroll-wheel zoom can wait.
There is no need for a loupe: grain wants a field of cells, which the pan
view is.

**Export.** The Export button runs the exact full frame and calls
`SrgbImage::write` with the file's orientation, the same call the CLI
makes. It does not write the on-screen bitmap. Milestone 1 writes JPEG.
Milestone 3 adds 16-bit TIFF, which the writer already supports.

**Orientation.** Caches stay in sensor orientation so the grain cell
matches the CLI. The preview bitmap is rotated with the same transform as
export (`Orientation::apply_rgb`, made public). Export of a full frame
uses the original orientation value, so a portrait file is not rotated
twice.

**Save as a new stock.** A save dialog. The app does not overwrite a
bundled file on its own. Fields in the dialog: name, source, license,
revision. Revision starts at the opened revision + 1. Source is pre-filled
with `Tuned from <name> in rawstock.` and can be edited. License is copied
from the opened stock and can be edited. The name rule from
[stocks/README.md](../stocks/README.md) is helper text in the dialog:
an invented look needs an original name; a transcription of a published
film may keep that film's name when source and license attribute it.

The writer emits schema 1 in the key order the README shows, with
`[coupling]`, `[grain]`, and `[halation]` omitted when they are off.
Comments from the source file are not kept. The file starts with one line
saying it was written by rawstock and that comments are not preserved.
Fidelity means:

- `Stock::parse` of the saved text succeeds.
- Every numeric value matches the in-memory `f32` bit for bit, including
  curves, coupling, dyes, sensitivities, grain, and halation.
- Omitted optional tables come back as `None`.
- `rawstock develop` of the same RAW with the saved file matches the
  tuner's exact full export.

Whitespace and comments are not part of fidelity. Floats are written with
a decimal that round-trips through `f32`.

## Core API

All of this stays in `rawstock-core`. `#![forbid(unsafe_code)]` stays. No
egui, Tauri, wgpu, tokio, or rayon dependency. No `std::thread`. File
dialogs and the worker live in `apps/desktop`. The session takes an owned
`LinearImage` and a `Parameters` value, so a wasm host can build both
from memory.

`Pipeline::develop` remains the CLI entry. It calls the same stage
functions as the session, with a no-op control. Its results stay as they
are.

### Editing parameters

`Parameters` today has getters, `with_halation`, and `with_grain`. Add
setters that validate through the existing `ModelError` rules and return
which stage they dirty:

- `set_exposure_ev`, `set_print_exposure`
- `set_halation`, `set_grain` (`None` clears the stage)
- `set_print_curve`
- `set_film` for a rebuilt `FilmModel` (curves, coupling, sensitivities,
  dyes, reference log exposure)

`Curve` gains `points()`, the same idea as `Granularity::points()`, so
the editor and the writer can see the knots. `Spectral` gains a method
that returns a new model with dye curves multiplied by three finite
non-negative scales and the scan balance recomputed. That is the dye
slider.

### Session

`PreviewSession` holds the full ACEScg image, the proxy, the orientation,
the parameters, and the stage buffers.

- `new(linear, orientation, proxy_long_edge)`
- `set_parameters` / the setters above, each marking the dirty stage
- `set_proxy_long_edge`
- `render_proxy`, `render_region` (display-space rectangle), `render_full`
- each takes `&mut dyn DevelopControl` and returns
  `Result<SrgbImage, Cancelled>`

`render_region` slices sensor space, runs halation with sigma from the
full frame, and runs grain with the full frame's pitch and the crop's
millimetre origin, walking only the cells that overlap the crop.
`Grain::apply` and `Halation::apply` keep their current signatures for the
full frame. The crop path is new methods (`apply_region`, and halation
with an explicit sigma), and the full-frame methods call them.

The dye lookup table is an internal detail of the proxy render. `render_full`,
`render_region` after the pointer settles, and export call the exact
`printing_density`. A core test compares the lookup against the exact scan
on a density ramp that covers the stock's range, and requires each encoded
sRGB channel to land within one 8-bit level. If a 48³ table misses that,
the resolution goes up before the approximation ships. Export has no switch
that selects the table.

### Progress and cancel

```rust
trait DevelopControl {
    fn cancelled(&self) -> bool;
    fn on_progress(&mut self, stage: Stage, done_rows: u32, rows: u32);
}
```

`Stage` is `Exposure`, `Halation`, `Density`, `Grain`, `DyeScan`,
`Print`, `Encode`. `Cancelled` is its own error, not a `ModelError`. The
long stages poll `cancelled` every row or every few rows.

A unit test records `on_progress` and checks the dirty rules: after a
grain edit the log starts at `Grain` and does not contain `Density`;
after a print-exposure edit it starts at `Print`; after an exposure edit
it starts at `Exposure` and does not imply a new decode (the session has
no decode stage). A second test cancels during `Density` and expects
`Err(Cancelled)` with no image. A third test renders a crop with the
full frame's pitch and millimetre origin and checks those pixels against
the same region of a full-frame `Grain::apply`, so a future change
cannot size the clouds from the crop width.

### Stock text

- `Stock::parse(&str) -> Result<Stock, StockError>` — what `load` uses
  after it reads the file. wasm and the save dialog's preview both need
  this. `load` stays for the CLI.
- `Stock::from_parts(meta, parameters) -> Result<Stock, StockError>` —
  the save dialog builds a stock without a path. The model tag has to
  match the `FilmModel` variant.
- `Stock::to_toml(&self) -> String` and `write(&self, path)`.

Round-trip tests load `stocks/neutral.stock.toml` and
`stocks/portra400.stock.toml`, write them, parse them, and compare every
public field. `cargo check -p rawstock-core --target wasm32-unknown-unknown`
stays part of the milestone. `RawImage::open` may keep using `std::fs`;
the session never calls it. A bytes-in decoder is Phase 6 work, when the
web app cannot hand the core a path. The session API does not need to
change for that, because it already accepts a `LinearImage`.

`SrgbImage` gains an 8-bit RGB view that uses the same rounding as the
JPEG writer, so the preview and the export quantize once. The shell
orients that buffer for display.

### Decode on the worker

The open message carries a path. The worker calls `RawImage::open` and
`develop_linear`, builds the session, and drops the `RawImage`. For X-Trans
it builds a staged session on the quick develop and moves the `RawImage`
to the demosaic thread, so `RawImage` has to be `Send`. The UI thread never
holds sensor samples. The GPU device opens on its own thread when the
worker starts, so the first decode does not wait for it.
`LinearImage` and `Parameters` do, because the worker owns them; they
already are, and new fields need to stay `Send` (no `Rc`, no UI handles).

## Milestones

Each milestone is usable on its own. Milestone 1 is the one to build
first, and it does not wait for Phase 4. Phase 5's existing done-when
(macOS, Windows, and Linux; GPU preview; export matches the CLI) is
Milestone 4 plus the packaging the phase already describes.
Milestone 1 is one desktop OS, from a checkout, via the Tauri dev command.

### Milestone 1 — A tuner that opens a file

A window with a file button, the stock list, the picture, and the
Milestone 1 sliders from the table above. Fit proxy, A/B toggle, 100% pan,
JPEG export, Save as.

Done when:

- From a checkout, the app opens a Fuji X-Trans RAF and an iPhone ProRAW
  DNG. Each file is decoded once. Switching stocks does not decode again.
- Exposure, print exposure, dye scale, grain amount, and the Milestone 1
  halation controls update the fit view. Dye scale is absent on Neutral.
- The core tests for dirty stages, cancel, crop grain coordinates, dye
  lookup tolerance, and TOML round-trip pass.
- Moving a control during a render cancels the in-flight job. The bitmap
  that stays up matches the latest generation.
- The fit view is labeled as the wrong place to judge grain. At 100%, a
  grain-amount change updates the texture without rebuilding clouds, and
  a crop matches the same pixels of a CLI develop of that frame.
- Export JPEG of an unmodified stock matches `rawstock develop` of that
  file and stock.
- Save as writes a new schema-1 file. `rawstock develop --stock` on that
  file matches the tuner's exact export of the tweaked parameters.
- `rawstock-core` still has no UI dependency and still checks for
  `wasm32-unknown-unknown`.
- No RAW file is committed. Release timings for the proxy and for one
  full export are written back into the latency section above.

### Milestone 2 — Curves, coupling, and the rest of the panel

The knot editors, the coupling matrix, the remaining halation and grain
fields, reference log exposure, and the before/after wipe.

Done when:

- Editing a knot, the matrix, or the print curve updates the proxy and
  dirties only the stage in the table.
- A drag cannot produce a curve the loader would reject.
- The wipe compares the opened stock with the current edit at fit and at
  100%.
- A stock saved after a curve edit round-trips those knots, and a cleared
  coupling matrix is omitted from the file.

### Milestone 3 — Full-frame export you can trust

16-bit TIFF, progress on the full spectral export, and a remembered stock
directory. The latency table gets real numbers if Milestone 1's machine
was not the machine Caleb tunes on.

Done when:

- TIFF export matches the CLI for an unmodified stock, including
  orientation and the sRGB profile.
- A tweaked stock, saved and reloaded, matches the CLI on both JPEG and
  TIFF.
- The progress line names the stage during a full export, and Cancel
  returns to the previous picture without writing a file.
- The app reopens on the last stock directory.

### Milestone 4 — GPU preview, which is Phase 4 inside this shell

Starts when the wgpu renderer exists. The controls do not change.

Done when:

- The worker's preview stages run on wgpu. Export and a CPU toggle still
  run the reference path.
- CPU/GPU parity holds to the tolerance Phase 4 documents, on the proxy
  and on a 100% crop, including the dye-cloud field.
- The app runs on macOS, Windows, and Linux, which is the rest of
  Phase 5's done-when.

### Milestone 5 — The same UI on the web, which is Phase 6

`apps/ui` is shared. The core session runs in a Web Worker. Decode grows
a bytes-in entry so the browser can pass a file the user picked. No image
and no stock file is uploaded. The phase's done-when is unchanged.

## Risks

- **The spectral dye scan is the expensive stage.** If the lookup table
  misses the one-level tolerance and has to grow a lot, drag updates slip
  from a tenth of a second toward the raw `powf` cost. The fallback is the
  720 px proxy, cancel, and an exact image once the pointer stops. Export
  never uses the table, so a preview error cannot reach a saved file.
- **A grain rebuild walks the whole frame.** Proxy resolution does not
  shrink the Poisson-cell loop. The amount slider stays live only because
  it multiplies a cached unit field. A `size_mm` drag that rebuilds on
  every tick will stall the fit view. The plan keeps that rebuild for
  pointer-up, and for the 100% crop while that view is open.
- **A crop that uses its own width will lie about grain.** Pitch has to
  be `frame_mm / full_width`. The crop test is the guard. Halation has
  the same trap if sigma is computed from the crop instead of the full
  frame, and a short pad will darken the edge of a 100% view.
- **Preview bytes through JSON.** A 4 MB frame encoded as a number array
  will feel worse than the render. The shell uses a raw response.
- **Full-resolution intermediates.** Caching every stage of a 40 MP file
  is a memory problem. The session keeps the ACEScg frame and proxy-sized
  buffers.
- **Save drops comments.** `portra400.stock.toml` documents which numbers
  are datasheet traces and which are eye-tuned. A saved copy keeps the
  numbers and the source string, and loses those comments. Saving a new
  file, rather than overwriting the bundled one, is the mitigation.
- **Applying a dye scale twice.** The base-times-slider rule has to reset
  after save. A test saves twice without moving a slider and checks the
  dyes are unchanged the second time.
- **The webview is not a color-managed display.** Encoded sRGB in a
  wide-gamut webview will not match a profiled viewer. Export embeds an
  sRGB profile. Display P3 is still open in the roadmap, and it should
  not block Milestone 1.
- **Bilinear X-Trans is soft.** Stock tweaks on a RAF are judged through
  that demosaic. A stock that looks right on ProRAW can look dull on a
  Fuji file for reasons that are not the stock.
- **Orientation applied twice.** Export has to call `SrgbImage::write`
  with the camera orientation on sensor-oriented pixels. Writing the
  already-rotated preview with `Orientation::Normal` is the other legal
  path, and mixing them is the bug.
- **A cancel that yields a short image.** `Cancelled` means no image.
  The shell keeps the last good bitmap.

## Questions for Caleb

1. **Preview approximation.** The fit view uses a dye-scan lookup table
   and is marked `preview`. The settled 100% view and export use the
   exact scan. Are you willing to judge color on that preview and grain
   on the exact 100% refine? The alternative is exact pixels on every
   drag, which on Portra means the picture updates a few times a second
   at 720 px and slower at 1440.
2. **Dye scale in the file.** Recommended: bake `base × scale` into
   `dye_density` and keep schema 1 unchanged. The other option is an
   optional schema-1 key so the multiplier round-trips as a multiplier.
   Opening today's Portra file shows 1.0 either way, because 1.45 / 0.98 /
   0.95 is already baked.
3. **White balance.** As-shot is baked into the ACEScg image, as in the
   CLI. A temp/tint slider would be session state, not a stock field, and
   it would rebuild that image. Do you want it in the tuner?
4. **X-Trans demosaic.** Is rawler's bilinear good enough to tune
   against, or is the LibRaw desktop fallback a prerequisite before Fuji
   files are worth judging?
5. **Frame size.** The fixtures in mind are the X-S10 (6384×4182, about
   305 MB for one ACEScg buffer) and the iPhone 12 Pro (4032×3024, about
   140 MB). Are 40 MP RAFs and 48 MP ProRAW in the first set of files?
   A 40 MP buffer is about 460 MB, and the spectral export estimate
   scales with pixel count.
6. **License on save.** Recommended: copy the opened stock's license
   into the dialog, let you edit it, and always go through Save as.
   Should a saved derivative keep the source license automatically
   instead of asking?
7. **Screen color.** Is sRGB in the webview enough to trust while you
   tune, on the display you will actually use? A wide-gamut screen will
   show that webview unmanaged until a later color-managed output path
   exists.
