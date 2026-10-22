// Dye-cloud grain, from `grain.rs` and `Grain::mix_field`.
//
// The CPU splats every cloud into an f64 field. Here each field sample
// gathers the clouds that reach it. Cells are addressed by the same integer
// hash, so a cloud's count, center, size, shape, and strength come from the
// same draws: every draw is `hash(cell, sequence)`, so any draw can be
// fetched directly. Only the Gaussian itself (`exp`, `sin`, `cos`) and the
// summation order differ.
//
// One dispatch covers one chunk of whole rows. The field is evaluated on that
// band plus `pad` rows and columns on each side, so the scanner blur never
// reads past the band for a pixel that is kept.

struct Params {
    // x: band width with padding, y: band height with padding, z: pad,
    // w: crop width.
    band: vec4<u32>,
    // x, y: the band's top-left pixel in the frame, padding included (may
    // be negative). z: process count. w: 1 when the scanner blurs.
    origin: vec4<i32>,
    // x: millimetres per pixel, y: the mix scale.
    scalars: vec4<f32>,
    mean: vec4<f32>,
    spread: vec4<f32>,
    // x: pixels in the chunk, y: where the scanner taps start in `tables`.
    counts: vec4<u32>,
    // Red, green, and blue granularity curves in `tables`.
    rms_offset: vec4<u32>,
    rms_len: vec4<u32>,
}

@group(0) @binding(0) var<uniform> params: Params;
// One chunk of density, interleaved RGB, whole rows.
@group(0) @binding(1) var<storage, read_write> data: array<f32>;
// The band's field, interleaved RGB.
@group(0) @binding(2) var<storage, read_write> field: array<f32>;
@group(0) @binding(3) var<storage, read_write> scratch: array<f32>;
// Process records of 12 words from 0, then the scanner taps, then the
// granularity curves.
@group(0) @binding(4) var<storage, read> tables: array<f32>;

const SUPPORT_D2: f32 = 12.25;
const STRENGTH_LO: f32 = 0.72;
const STRENGTH_HI: f32 = 1.0;
const TAU: f32 = 6.28318530717958648;
const MAX_POISSON: u32 = 64u;

// `Grain::hash`: PCG, after Jarzynski and Hill.
fn hash(x: u32, y: u32, layer: u32, seed: u32) -> u32 {
    var n = (seed + layer * 0x9E3779B9u + x * 0x85EBCA6Bu) ^ (y * 0xC2B2AE35u);
    n = n * 747796405u + 2891336453u;
    let word = ((n >> ((n >> 28u) + 4u)) ^ n) * 277803737u;
    return (word >> 22u) ^ word;
}

// `CellDraw::unit` for draw number `sequence`. Exact: 24 bits and a power
// of two.
fn unit(cell: vec2<i32>, sequence: u32, seed: u32) -> f32 {
    let word = hash(bitcast<u32>(cell.x), bitcast<u32>(cell.y), sequence, seed);
    return f32(word >> 8u) * (1.0 / 16777216.0);
}

struct Process {
    cell_mm: f32,
    limit: f32,
    sigma_lo: f32,
    sigma_hi: f32,
    aspect_max: f32,
    amplitude: f32,
    reach_mm: f32,
    seed: u32,
    layers: u32,
    resolved: bool,
}

fn process(index: u32) -> Process {
    let base = index * 12u;
    return Process(
        tables[base],
        tables[base + 1u],
        tables[base + 2u],
        tables[base + 3u],
        tables[base + 4u],
        tables[base + 5u],
        tables[base + 6u],
        bitcast<u32>(tables[base + 7u]),
        bitcast<u32>(tables[base + 8u]),
        bitcast<u32>(tables[base + 9u]) != 0u,
    );
}

// `CellDraw::poisson`: x = cloud count, y = draws used. The CPU passes
// `exp(-mu)` so the comparison sees the same bits. Products of exact units
// are correctly rounded in WGSL, so the count matches.
fn poisson(cell: vec2<i32>, seed: u32, limit: f32) -> vec2<u32> {
    var product = 1.0;
    var count = 0u;
    var sequence = 0u;
    while count < MAX_POISSON {
        sequence += 1u;
        product *= unit(cell, sequence, seed);
        if product <= limit {
            return vec2<u32>(count, sequence);
        }
        count += 1u;
    }
    return vec2<u32>(count, sequence);
}

fn log_lerp(lo: f32, hi: f32, value: f32) -> f32 {
    return lo * pow(hi / lo, value);
}

fn layer_mask(layers: u32) -> vec3<f32> {
    return vec3<f32>(f32(layers & 1u), f32((layers >> 1u) & 1u), f32((layers >> 2u) & 1u));
}

// One process's clouds at band pixel `pixel`.
fn gather(p: Process, pixel: vec2<i32>) -> f32 {
    let pitch = params.scalars.x;
    let origin = params.origin.xy;
    let width = i32(params.band.x);
    let rows = i32(params.band.y);
    let cell = p.cell_mm;
    // The pixel's own extent in millimetres, then every cell whose clouds
    // can reach it.
    let lo_mm = vec2<f32>(origin + pixel) * pitch;
    let hi_mm = lo_mm + vec2<f32>(pitch);
    // A little slack: the exact test below is in pixels, after a division
    // the CPU rounds differently near a pixel edge.
    var reach = 0.01 * pitch;
    if p.resolved {
        reach = p.reach_mm + 0.5 * pitch;
    }
    let first = vec2<i32>(floor((lo_mm - vec2<f32>(reach)) / cell));
    let last = vec2<i32>(floor((hi_mm + vec2<f32>(reach)) / cell));
    var sum = 0.0;
    for (var cy = first.y; cy <= last.y; cy++) {
        for (var cx = first.x; cx <= last.x; cx++) {
            let at = vec2<i32>(cx, cy);
            let drawn = poisson(at, p.seed, p.limit);
            for (var k = 0u; k < drawn.x; k++) {
                let base = drawn.y + 6u * k;
                let x_mm = (f32(cx) + unit(at, base + 1u, p.seed)) * cell;
                let y_mm = (f32(cy) + unit(at, base + 2u, p.seed)) * cell;
                // Quick reject: the cloud cannot reach this pixel.
                if x_mm < lo_mm.x - reach || x_mm >= hi_mm.x + reach
                    || y_mm < lo_mm.y - reach || y_mm >= hi_mm.y + reach {
                    continue;
                }
                let ur = unit(at, base + 3u, p.seed);
                let ua = unit(at, base + 4u, p.seed);
                let ut = unit(at, base + 5u, p.seed);
                let uf = unit(at, base + 6u, p.seed);
                let sigma = log_lerp(p.sigma_lo, p.sigma_hi, ur);
                let strength = STRENGTH_LO + (STRENGTH_HI - STRENGTH_LO) * uf;
                let amplitude = p.amplitude * strength;
                // Cloud center in band pixels, as the CPU computes it.
                let center = vec2<f32>(x_mm / pitch, y_mm / pitch) - vec2<f32>(origin);
                if p.resolved {
                    // `paint_gaussian`.
                    let aspect = log_lerp(1.0, p.aspect_max, ua);
                    let axis = sqrt(aspect);
                    let angle = ut * TAU;
                    let s = sin(angle);
                    let c = cos(angle);
                    let sx = sigma / axis / pitch;
                    let sy = sigma * axis / pitch;
                    if sx <= 0.0 || sy <= 0.0 {
                        continue;
                    }
                    let rad = 3.5 * max(sx, sy);
                    let x0 = max(i32(ceil(center.x - rad - 0.5)), 0);
                    let y0 = max(i32(ceil(center.y - rad - 0.5)), 0);
                    let x1 = min(i32(floor(center.x + rad - 0.5)), width - 1);
                    let y1 = min(i32(floor(center.y + rad - 0.5)), rows - 1);
                    if pixel.x < x0 || pixel.x > x1 || pixel.y < y0 || pixel.y > y1 {
                        continue;
                    }
                    let dx = f32(pixel.x) + 0.5 - center.x;
                    let dy = f32(pixel.y) + 0.5 - center.y;
                    let xr = dx * c + dy * s;
                    let yr = -dx * s + dy * c;
                    let inv_sx = 1.0 / sx;
                    let inv_sy = 1.0 / sy;
                    let d2 = xr * inv_sx * xr * inv_sx + yr * inv_sy * yr * inv_sy;
                    if d2 >= SUPPORT_D2 {
                        continue;
                    }
                    sum += amplitude * exp(-0.5 * d2);
                } else {
                    // `deposit_mass`: the whole cloud lands in the pixel
                    // that holds its center.
                    if i32(floor(center.x)) != pixel.x || i32(floor(center.y)) != pixel.y {
                        continue;
                    }
                    sum += amplitude * TAU * sigma * sigma / (pitch * pitch);
                }
            }
        }
    }
    return sum;
}

// The unblurred field over the padded band.
@compute @workgroup_size(16, 16)
fn grain_field(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.band.x || id.y >= params.band.y {
        return;
    }
    let pixel = vec2<i32>(id.xy);
    var value = vec3<f32>(0.0);
    for (var index = 0u; index < u32(params.origin.z); index++) {
        let p = process(index);
        value += gather(p, pixel) * layer_mask(p.layers);
    }
    let base = (id.y * params.band.x + id.x) * 3u;
    field[base] = value.x;
    field[base + 1u] = value.y;
    field[base + 2u] = value.z;
}

fn tap(offset: u32) -> f32 {
    return tables[params.counts.y + offset];
}

// `Scanner::blur`, horizontal: `field` into `scratch`, edges clamped.
@compute @workgroup_size(16, 16)
fn scan_horizontal(@builtin(global_invocation_id) id: vec3<u32>) {
    let width = params.band.x;
    if id.x >= width || id.y >= params.band.y {
        return;
    }
    let radius = i32(params.band.z);
    var acc = vec3<f32>(0.0);
    for (var offset = -radius; offset <= radius; offset++) {
        let x = u32(clamp(i32(id.x) + offset, 0, i32(width) - 1));
        let src = (id.y * width + x) * 3u;
        acc += tap(u32(offset + radius)) * vec3<f32>(field[src], field[src + 1u], field[src + 2u]);
    }
    let dst = (id.y * width + id.x) * 3u;
    scratch[dst] = acc.x;
    scratch[dst + 1u] = acc.y;
    scratch[dst + 2u] = acc.z;
}

// `Scanner::blur`, vertical: `scratch` back into `field`.
@compute @workgroup_size(16, 16)
fn scan_vertical(@builtin(global_invocation_id) id: vec3<u32>) {
    let width = params.band.x;
    let rows = params.band.y;
    if id.x >= width || id.y >= rows {
        return;
    }
    let radius = i32(params.band.z);
    var acc = vec3<f32>(0.0);
    for (var offset = -radius; offset <= radius; offset++) {
        let y = u32(clamp(i32(id.y) + offset, 0, i32(rows) - 1));
        let src = (y * width + id.x) * 3u;
        acc += tap(u32(offset + radius)) * vec3<f32>(scratch[src], scratch[src + 1u], scratch[src + 2u]);
    }
    let dst = (id.y * width + id.x) * 3u;
    field[dst] = acc.x;
    field[dst + 1u] = acc.y;
    field[dst + 2u] = acc.z;
}

fn is_finite(x: f32) -> bool {
    return (bitcast<u32>(x) & 0x7f800000u) != 0x7f800000u;
}

// `Granularity::eval`.
fn granularity(layer: u32, value: f32) -> f32 {
    let offset = params.rms_offset[layer];
    let len = params.rms_len[layer];
    let first = tables[offset];
    let last = tables[offset + len - 1u];
    var x = value;
    if !is_finite(x) {
        x = first;
    }
    if x <= first {
        return tables[offset + len];
    }
    if x >= last {
        return tables[offset + 2u * len - 1u];
    }
    var lo = 0u;
    var hi = len;
    while lo < hi {
        let mid = (lo + hi) / 2u;
        if tables[offset + mid] <= x {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    let upper = lo;
    let lower = upper - 1u;
    let x0 = tables[offset + lower];
    let weight = (x - x0) / (tables[offset + upper] - x0);
    let r0 = tables[offset + len + lower];
    let r1 = tables[offset + len + upper];
    return r0 + weight * (r1 - r0);
}

// `Grain::mix_field`, with the field normalized as `FilmGrain::render` does.
@compute @workgroup_size(256)
fn grain_mix(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let pixel = id.x + id.y * groups.x * 256u;
    if pixel >= params.counts.x {
        return;
    }
    let crop_width = params.band.w;
    let pad = params.band.z;
    let row = pixel / crop_width;
    let column = pixel % crop_width;
    let src = ((row + pad) * params.band.x + column + pad) * 3u;
    for (var layer = 0u; layer < 3u; layer++) {
        let spread = params.spread[layer];
        var normal = 0.0;
        if spread != 0.0 {
            normal = (field[src + layer] - params.mean[layer]) / spread;
        }
        let index = pixel * 3u + layer;
        let density = data[index];
        let rms = granularity(layer, density);
        if rms != 0.0 {
            data[index] = density + normal * rms * params.scalars.y;
        }
    }
}
