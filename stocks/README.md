# stocks

A **stock** is a film model, parametric or spectral, stored as data. A 3D LUT
is only an optional bake for `.cube` export or preview caching. The app
targets desktop hardware, so the model runs directly.

`FilmModel` in core maps scene exposure to layer densities. `Parametric` and
`Spectral` are enum variants. Prefer the enum over a trait object unless
there is a stated reason not to. Names must be original (no trademarked film
names).

`rawstock develop` loads one file into the existing
[`Parameters`](../crates/rawstock-core/src/render.rs) struct: exposure,
optional halation, a parametric film model, optional grain, print exposure,
and a print curve.

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
model = "parametric"          # enum tag; "spectral" is not accepted yet
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

`model` is an enum. The only tag schema 1 accepts is `parametric`. A later
`spectral` variant can be added without renaming `parametric`, so files
written today keep parsing. A spectral payload that needs new keys would
arrive with a new `schema_version`; this reader rejects both unknown
versions and unknown fields.

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

Schema 1 does not carry grain, halation, bloom, or spectral sensitivity and
dye density. The pipeline in [Architecture](../docs/ARCHITECTURE.md) still
fixes when those stages run. A later schema can add them. Until then the
loader rejects those keys.

[neutral.stock.toml](neutral.stock.toml) is a synthetic neutral reference.
Red, green, and blue share one curve and coupling is omitted, so a gray stays
gray. The license is CC0-1.0 because these numbers were written for rawstock.
They are not a measurement of any manufactured film.

## Kodak Portra 400

The first measured stock is spectral. Spectral sensitivity, characteristic
curves, and dye density come from Kodak publication E-4050. Grain, interlayer
coupling, and print/scan are calibrated from chart or step-wedge scans.

[agx-emulsion](https://github.com/andreavolpato/agx-emulsion) (GPL-3.0; the
repository is now spektrafilm) is reference prior art for this kind of model.
rawstock is GPL-3.0-or-later, so that license is compatible. Portra 400's
numbers come from E-4050 and our scans, with `source` and `license` filled in.

## spectral_film_lut

Research notes for a possible future importer. This tree does not vendor any
of its data, and there is no importer code.

[JanLohse/spectral_film_lut](https://github.com/JanLohse/spectral_film_lut) is
MIT-licensed. The license file is the standard MIT text, copyright 2026 Jan
Lohse:

<https://github.com/JanLohse/spectral_film_lut/blob/main/LICENSE>

`pyproject.toml` sets `license = "MIT"`, and GitHub reports the MIT License.
There is no second license file for the stock tables.

The stock data is not a document format of its own. Each stock is a Python
module that builds a `FilmData` dataclass
(`src/spectral_film_lut/film_data.py`). Those modules live in
`negative_film/`, `print_film/`, `reversal_film/`, `reversal_print/`,
`bw_negative_film/`, and `bw_print_film/`. The package `__init__.py` imports
the constants into `FILM_STOCKS`.

A curve is a list of dicts, one dict per layer, mapping a float key to a
float value in source order:

- `sensiometric_curve`: log exposure → density (the D-logH curve)
- `log_sensitivity`: wavelength → log sensitivity
- `spectral_density`: wavelength → dye density
- `d_ref_sd` and `d_min_sd`: one wavelength → density dict each
- `mtf`, `rms_curve`, and `rms_density`: more per-layer dicts
- `sensitivity`: an optional NumPy array

The dataclass also carries `name`, `manufacturer`, `film_type` (`positive` or
`negative`), `stage` (`camera` or `print`), `medium`, `density_measure` (for
example `status_m`), `year`, `iso`, and `rms`. An importer would read those
`FilmData` values. The repository does not ship them as TOML or JSON.

The MIT grant is the one in that LICENSE file: the software and associated
documentation files. The curves were digitized from manufacturer datasheets.
This repository does not copy them.
