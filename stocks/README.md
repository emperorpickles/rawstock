# stocks

A **stock** is a film look defined as data, not code. Names must be original
(no trademarked film names).

## Proposed format (draft, see Open questions in [PLAN](../docs/PLAN.md))

One definition file per stock, `<id>.stock.toml`, next to its LUT:

```
stocks/
  example.stock.toml
  example.cube        # base 3D LUT, .cube format
```

Fields:

- `name`, `description`, `version`: metadata.
- `[lut]`: `file` (relative path to the `.cube`), `input_space` (the linear
  working space or log encoding the LUT expects).
- `[tone]`: tone curve, as control points in linear light.
- `[grain]`: `amount`, `size`, `roughness`.
- `[halation]`: `strength`, `radius`, `tint` (RGB).

All values in [example.stock.toml](example.stock.toml) are placeholders; the
referenced `example.cube` does not exist yet.
