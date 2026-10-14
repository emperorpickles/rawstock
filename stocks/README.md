# stocks

A **stock** is a film model, parametric or spectral, stored as data. A 3D LUT
is only an optional bake for `.cube` export or preview caching. The app
targets desktop hardware, so the model runs directly.

`FilmModel` in core maps scene exposure to layer densities. `Parametric` and
`Spectral` are enum variants. Prefer the enum over a trait object unless
there is a stated reason not to.

## File format

Versioned TOML or JSON, plus numeric array files. Every stock has `source`
and `license`.

```
stocks/
  example.stock.toml
  example/curves.json    # log H and per-layer density; not present yet
```

Fields:

- `format_version`, `name`, `description`, `model` (`parametric` or `spectral`).
- `source`: datasheet, scan, or other origin of the numbers.
- `license`: license of those numbers. Set it before committing measured data.
- Characteristic (D-logH) curves, as numeric arrays. Spectral stocks also
  carry spectral sensitivity and dye density.
- Interlayer coupling, grain (physical units and a coordinate-hash seed),
  and print/scan parameters. Halation and bloom parameters live here too;
  the pipeline in [Architecture](../docs/ARCHITECTURE.md) fixes when they run.

[example.stock.toml](example.stock.toml) is a format sketch. Its numbers are
placeholders, and the array file it names is not in the tree yet.

## Kodak Portra 400

The first stock is spectral. Spectral sensitivity, characteristic curves, and
dye density come from Kodak publication E-4050. Grain, interlayer coupling,
and print/scan are calibrated from chart or step-wedge scans.

[agx-emulsion](https://github.com/andreavolpato/agx-emulsion) (GPL-3.0; the
repository is now spektrafilm) is reference prior art for this kind of model.
rawstock is GPL-3.0-or-later, so that license is compatible. Portra 400's
numbers come from E-4050 and our scans, with `source` and `license` filled in.
