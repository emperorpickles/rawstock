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

Two more optional spectral tables change how a negative is read and
printed. `[scan_response]` holds red, green, and blue densitometer
responses on the same grid (Status M for a published negative). With it,
the dye scan reads the negative through those responses instead of the
eye, so each channel sees mostly its own dye, as a printer does. It needs
`[dye_density]`. `[print_dye_density]` holds the print's own three dye
spectra. With it, the `[print]` curve gives each channel's paper density,
and the print is the transmittance of those three dyes together viewed
under D65 / CIE 1931, instead of three ideal ACEScg dyes. Zero print
density is the paper white, ACEScg `(1, 1, 1)`. Parametric stocks reject
both tables.

```toml
[scan_response]               # optional; needs [dye_density]
red = [0.0, 0.2, 1.0]
green = [0.0, 1.0, 0.2]
blue = [1.0, 0.2, 0.0]

[print_dye_density]           # optional; omitted keeps ideal ACEScg dyes
red = [0.1, 0.3, 1.0]
green = [0.2, 1.0, 0.3]
blue = [1.0, 0.4, 0.1]
```

`process` is an optional spectral key: `negative` (the default) or
`reversal`. A reversal (slide) stock's `red`, `green`, and `blue` curves
fall: density must not increase as log exposure rises. It needs
`[dye_density]`, because the developed dyes are the image. The scan hands
the print stage the negated slide density, so print log-exposure becomes
`print_exposure + slide density` and the `[print]` curve is a scan curve
for viewing the slide (identity shows it as it is). `[print]` itself
always rises. A parametric stock is always a negative and rejects the key.

`neutral_scale = true` is an optional spectral key that needs
`[dye_density]`. Without it, the scan is balanced at
`reference_log_exposure` only. With it, gray exposures across the curves are
scanned and each channel's scanned density is mapped onto the mean of the
three, so every gray prints neutral, like a scanner's per-channel curves.
Colors pass through the same three curves.

`emulsion = "monochrome"` is an optional spectral key for black and white
film. The default is `color`. A monochrome stock has one silver layer: one
`[sensitivity]` entry, `silver`, a single `[silver]` curve in place of
`[red]`, `[green]`, and `[blue]`, and `[grain.silver]` in place of the three
grain layers. Core copies that layer into all three channels, so the print
stage and the GPU run unchanged and every color develops to a neutral gray.
Silver grain puts every cloud in all three channels, so it has no chroma.
`[print]` inverts the silver density as usual; `[print_dye_density]` is
allowed, for a toned paper. Dye curves, scan responses, coupling,
`reference_log_exposure`, `neutral_scale`, and `process` are rejected, and a
parametric stock rejects the `emulsion` key.

```toml
model = "spectral"
emulsion = "monochrome"

[sensitivity]
silver = [0.0, 0.05, 0.0]

[silver]
log_exposure = [-3.0, 0.0]
density = [0.0, 1.7]

[grain]
[grain.silver]
density = [0.0, 1.0, 2.0]
rms = [8.0, 15.0, 14.0]
size_mm = 0.006               # silver clump sigma; one size for the layer
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
of their own. Optional `size_mm` is that layer's primary dye-cloud sigma, in
millimetres; omitted layers use the built-in sizes, with red coarser
than blue.
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

[portra400.stock.toml](portra400.stock.toml) is a spectral negative printed
the way a lab prints it. Its sensitivities, D-logH curves, and dye shapes
come from Kodak publication E-4050, and its print curve and paper dyes from
the KODAK PROFESSIONAL PORTRA ENDURA paper datasheet, both digitized by Jan
Lohse in [spectral_film_lut](https://github.com/JanLohse/spectral_film_lut),
which is MIT-licensed, copyright 2026 Jan Lohse:

<https://github.com/JanLohse/spectral_film_lut/blob/main/LICENSE>

`lab/import_spectral_film_lut.py` reads the Portra 400 and Portra Endura
modules and that package's Status M table, and writes every measured array
in the file. It does not vendor the package. `agx-emulsion` data is not
used.

- **Negative.** Sensitivities extend each traced end slope, floored three
  decades below the peak, as for Velvia. Dye shapes are the split of
  midscale-minus-D-min, each scaled to read density 1 through its own
  Status M channel.
- **Curves.** The datasheet curves are Status M readings of the whole dye
  stack. The stock stores dye amounts: readings above D-min (the orange
  mask), unmixed by the inverse of the dyes' Status M readings, so a Status
  M scan gives the datasheet curves back.
- **Scan.** `[scan_response]` is Status M. `reference_log_exposure` is the
  ISO middle gray, which the scan balances to neutral, the job of a
  printer's color trims.
- **Print.** `[print]` is the paper's green-layer curve from its white.
  `print_exposure` prints middle gray at an 18 % reflector's density, the
  paper's laboratory aim. `[print_dye_density]` is the paper's dyes, scaled
  so equal print densities at that aim view as neutral under D65.

There is no interlayer coupling. Grain and halation are an **approximate
aesthetic tuning**, marked in the TOML. Grain follows the shape of the
Vision3 250D tables spectral_film_lut carries for this film (blue grainiest,
cyan clouds coarsest). The paper multiplies negative grain by its contrast,
about 2.5, so the table sits near 5207's values, and a developed middle
gray shows about twice Velvia's luma noise. Halation is a faint red fringe,
because this film has an anti-halation layer.

Re-running the importer drops the header notes, grain, and halation; see
[lab/README.md](../lab/README.md).

Portra is a Kodak trademark. This file is an attributed transcription of
published curves, not an invented look. Invented looks still need original
names.

## Fuji Velvia 50 (spectral)

[velvia50-spectral.stock.toml](velvia50-spectral.stock.toml) is a spectral
reversal stock, kept separate from the parametric Velvia below so the two
can be compared. Its sensitivities, characteristic curves, and per-layer dye
densities are the Fujichrome Velvia 50 datasheet traces in the
`FUJI_VELVIA_50` module of the same MIT-licensed
[spectral_film_lut](https://github.com/JanLohse/spectral_film_lut),
written by `lab/import_spectral_film_lut.py`. The importer holds the last
few trace points at D-min (they rise by up to 0.035, digitizing noise) and
extends each sensitivity trace's end slope, floored three decades below the
peak, instead of clamping to the lowest traced value.

The rest is an **approximate tuning**:

- `exposure_ev` is placed for rawstock's camera decode, where Fuji RAFs put
  the metered middle gray near scene-linear 0.08, not the datasheet's 0.18.
  It matches the parametric Velvia's brightness.
- `neutral_scale` keeps grays neutral across the scale. The datasheet red
  layer has less contrast, so a single balance point left shadows warm and
  highlights cyan.
- `[coupling]` stands in for Velvia's interlayer saturation. Each row sums
  to 1, so neutrals are unchanged.
- The scan curve is identity through the midtones with a gentle shoulder
  past density 2.2, and `print_exposure` puts the clear base at white.
- The grain table is the parametric Velvia's, carried over unchanged.
  Fujifilm's quoted RMS granularity of 9 agrees with its peak.

There is no halation.

## Ilford HP5 Plus

[ilford-hp5-plus.stock.toml](ilford-hp5-plus.stock.toml) is the first black
and white stock, a monochrome spectral negative. Ilford publishes HP5
Plus's spectral sensitivity and characteristic curve only as figures, so
`lab/ilford_bw.py` holds readings of the two charts in the November 2018
HP5 PLUS technical information sheet, traced from the chart images and
calibrated to their ticks and grid, and writes the file from them.

- **Sensitivity.** The wedge spectrogram to tungsten (2850 K): native blue
  from 356 nm, a dip at 505 nm, the green peak at 575 nm, and a red edge
  at 655 nm, so deep reds print darker than the eye sees them. Its height
  is read as 3 decades of log sensitivity, the usual spectrograph wedge,
  and the tungsten spectrum is divided out. That scale is an assumption:
  the chart does not state it.
- **Curve.** The ILFOTEC HC 1+31 curve, net of its 0.18 base plus fog: a
  short toe and a straight line at gradient 0.64. The chart's exposure
  axis is relative, so the ISO speed point (net 0.10) is placed at
  `log10(0.8 / 400)` lux seconds.
- **Print.** Kodak Professional Polymax Fine-Art paper, grade 1, from the
  paper white. That curve is measured: the MIT-licensed `KODAK_POLYMAX`
  module in [spectral_film_lut](https://github.com/JanLohse/spectral_film_lut).
  Grade 1 suits a camera file, which has no lens flare to lift the shadows.
- **Placement.** `exposure_ev` puts the camera's metered middle gray
  (scene-linear about 0.08) on the ISO middle-gray log exposure, and
  `print_exposure` prints it at an 18 % reflector's density.
- **Grain and halation** are an approximate aesthetic tuning. Silver grain
  is about Tri-X's strength, rises with density, and has a 6 µm clump sigma.
  Halation is a faint neutral glow.

FP4 Plus, the Delta films, and Pan F Plus can be added the same way: a new
entry in the script's `FILMS` table. Ilford, HP5, and Polymax are
trademarks of their owners.

## Velvia

[velvia.stock.toml](velvia.stock.toml) is a parametric stock. It is an
**approximate** fit of the rawstock pipeline to Fujifilm X-T2 in-camera
Velvia JPEGs (film simulation F2/Fujichrome), not a datasheet of Fujichrome
Velvia and not a 3D LUT. The camera was set to DR100, Color 0, Highlight
Tone 0, Shadow Tone 0, Sharpness Normal, Noise Reduction 0, and White
Balance Auto. Grain Effect was Weak on those frames, and the fit was made
with grain off.

The `[grain]` table is a separate **approximate aesthetic tuning**, not part
of the fit and not a Fujifilm granularity table. Velvia 50 is a very fine
slide film, so rms is about a third of Portra 400's and the dye clouds are
smaller (6 to 10 µm). A developed gray ramp shows roughly half of Portra's
midtone luma noise.

`lab/fit_stock.py` develops each RAF, aligns it to the JPEG, and minimizes
CIEDE2000 on the tone curves, the print curve, and the 3×3 log-exposure
mix. Only the Velvia pairs in `fit/` are minimized. The Provia, Classic
Chrome, and Astia pairs are held-out checks and are not part of the loss.
The reference photographs are not in the repository.

Velvia is a Fujifilm trademark. This file is a labeled approximation of
that camera mode.
