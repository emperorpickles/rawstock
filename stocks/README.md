# stocks

A **stock** is a film model, parametric or spectral, stored as data. A 3D LUT
is only an optional bake for `.cube` export or preview caching. The app
targets desktop hardware, so the model runs directly.

`FilmModel` in core maps scene exposure to layer densities. `Parametric` and
`Spectral` are enum variants. Prefer the enum over a trait object unless
there is a stated reason not to.

`rawstock develop` loads one file into the existing
[`Parameters`](../crates/rawstock-core/src/render.rs) struct: exposure,
optional halation, a film model, optional grain, print exposure, and a print
curve. An invented look needs an original name. A file that transcribes one
published film may use that film's name when `source` and `license` attribute
the measurement.

## Schema 1

One TOML file per stock. `schema_version` is the file format. `revision` is
the stock's own revision and is a different number. Unknown fields are
rejected. `schema_version` must be `1`.

```toml
schema_version = 1
revision = 1
name = "Neutral"
source = "Where the numbers came from."
license = "CC0-1.0"
model = "parametric"          # or "spectral"; see below
exposure_ev = 0.0             # stops, before the film model
print_exposure = 0.9          # print log-exposure = print_exposure - density

[red]                         # green, blue, and print use the same keys
log_exposure = [-3.5, -2.0]   # log10 exposure, strictly increasing
density = [0.15, 0.30]        # density, must not decrease

# Optional. Omit the table for independent layers. Row-major; it multiplies
# the log-exposure column vector before the curves.
# [coupling]
# matrix = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]

# Optional. Omit the whole table for no grain. frame_mm is the physical
# width of the image, default 35. seed feeds the grain hash. Each layer is
# an RMS-granularity-versus-density table: density may not decrease, rms may.
# rms is the Kodak number, 1000 times the density standard deviation through
# a 48 µm aperture (read Sigma D on a Vision3 sheet and multiply by 1000).
# [grain]
# frame_mm = 35
# seed = 1
# [grain.red]                     # green and blue use the same keys
# density = [0.0, 1.0, 2.0]
# rms = [3.0, 5.0, 4.0]
# size_mm = 0.028                 # optional dye-cloud size; red > green > blue

# Optional. Omit the whole table for no halation. radius is the Gaussian
# sigma as a fraction of the shorter image side. weight is the red, green,
# and blue add-back; red is the large one.
# [halation]
# threshold = 1.0
# radius = 0.02
# weight = [0.6, 0.12, 0.04]
```

`model` is an enum. Schema 1 accepts `parametric` and `spectral`. Existing
parametric files keep parsing: the spectral keys are optional, and a
parametric file that sets one is rejected. Unknown versions and unknown
fields are still rejected.

`spectral` replaces only the step from scene RGB to per-layer log exposure.
ACEScg is upsampled with the Jakob and Hanika sigmoid (see
`crates/rawstock-core/data/NOTICE`) and integrated against `sensitivity`.
The same D-logH curves, coupling, and grain then run. When `dye_density` is
present, the print stage turns those densities into display RGB by
integrating transmittance `10^(-Σ density · dye)` against CIE 1931 under
D65. Without dye curves the print stage is the existing per-channel
inversion. `wavelength_nm` is the shared grid, in nanometres, strictly
increasing. Each sensitivity is non-negative and its trapezoidal integral
is 1, so a flat spectrum of level 1 exposes every layer equally. Dye
samples are non-negative. `reference_log_exposure`, with dye curves, is the
log exposure the scan balances to a neutral.

```toml
model = "spectral"
reference_log_exposure = -1.5
wavelength_nm = [380, 390, 400]

[sensitivity]                 # red, green, and blue, one sample per wavelength
red = [0.0, 0.05, 0.0]
green = [0.0, 0.05, 0.0]
blue = [0.0, 0.05, 0.0]

[dye_density]                 # optional; omitted keeps the simple print
red = [0.0, 1.0, 0.0]
green = [0.0, 1.0, 0.0]
blue = [0.0, 1.0, 0.0]
```

`[grain]` and `[halation]` are optional schema 1 tables. Leaving them out
is the same file older stocks already use, so `schema_version` stays 1.
`[grain]` holds one RMS-granularity-versus-density table per layer, the
shape of a Kodak Vision3 datasheet: read density, then the Sigma D scale,
and store Sigma D × 1000 as `rms`. The loader does not resample it. Density
must strictly increase; rms may rise and fall. The 48 µm figure is the
measurement aperture that sets the amplitude. It is not a tile. Grain is a
Poisson field of overlapping dye clouds, placed in millimetres on
`frame_mm` (default 35), so a wider export grows the clouds in pixels
instead of revealing a finer noise. Layers share some clouds and keep some
of their own. Optional `size_mm` is that layer's characteristic cloud size;
omitted layers use the built-in sizes, with red coarser than blue.
`[halation]` is a threshold on the peak scene-linear channel, a blur whose
sigma is `radius` times the shorter side, and a red-weighted add-back.

Each curve is two arrays. Index `i` is one step-wedge reading, the pair
`(log_exposure[i], density[i])`. Paste a measured table into those two
columns; the loader does not resample it. The arrays must be the same length.
Log-exposure must strictly increase and density must not decrease, which is
the same rule as the in-memory characteristic curve (a flat shoulder is
allowed). `red`, `green`, and `blue` are the negative. `print` is the
print/scan curve. Integers and decimals are both numbers, so a densitometer
table can be copied without rewriting whole values as floats.

`source` is the datasheet, scan, or other origin of the numbers. `license`
is the license of those numbers. Set both before committing measured data.

[neutral.stock.toml](neutral.stock.toml) is a synthetic neutral reference.
Red, green, and blue share one curve and coupling is omitted, so a gray stays
gray. The license is CC0-1.0 because these numbers were written for rawstock.
They are not a measurement of any manufactured film.

## Kodak Portra 400

[portra400.stock.toml](portra400.stock.toml) is a spectral stock. The
sensitivities and D-logH curves were digitized from Kodak publication E-4050
by Jan Lohse in
[spectral_film_lut](https://github.com/JanLohse/spectral_film_lut), which is
MIT-licensed, copyright 2026 Jan Lohse:

<https://github.com/JanLohse/spectral_film_lut/blob/main/LICENSE>

`lab/import_spectral_film_lut.py` reads that package's Portra 400 module and
writes a stock. It does not vendor the package. The committed
`portra400.stock.toml` keeps that importer's sensitivities and D-logH
curves. Re-running the importer overwrites the file and drops the tuning
below. `agx-emulsion` data is not used.

The dye scales, print curve, interlayer coupling, grain, and halation are an
**approximate aesthetic tuning**, shaped by eye. They are not densitometer
readings, a paper curve, or a published granularity table. Comments in the
TOML mark each of those values the same way. The dye wavelengths still come
from the importer's split of midscale-minus-D-min. Grain uses a
Vision3-style RMS-versus-density shape (rises, then falls; blue grainiest)
and per-layer `size_mm` dye-cloud sizes, chosen by eye for a fine ISO 400
color negative. Halation is a faint red fringe, because this film has an
anti-halation layer.

Portra is a Kodak trademark. This file is an attributed transcription of
published curves, not an invented look. Invented looks still need original
names.

## Velvia

[velvia.stock.toml](velvia.stock.toml) is a parametric stock. It is an
**approximate** fit of the rawstock pipeline to Fujifilm X-T2 in-camera
Velvia JPEGs (film simulation F2/Fujichrome), not a datasheet of Fujichrome
Velvia and not a 3D LUT. The camera was set to DR100, Color 0, Highlight
Tone 0, Shadow Tone 0, Sharpness Normal, Noise Reduction 0, and White
Balance Auto. Grain Effect was Weak on those frames, so this file has no
`[grain]` table: the fit was made with grain off.

`lab/fit_stock.py` develops each RAF, aligns it to the JPEG, and minimizes
CIEDE2000 on the tone curves, the print curve, and the 3×3 log-exposure
mix. Only the Velvia pairs in `fit/` are minimized. The Provia, Classic
Chrome, and Astia pairs are held-out checks and are not part of the loss.
The reference photographs are not in the repository.

Velvia is a Fujifilm trademark. This file is a labeled approximation of
that camera mode.
