// Dye-cloud grain, from `grain.rs` and `Grain::mix_field`.
//
// The CPU splats every cloud into an f64 field. Here each field sample
// gathers the clouds that reach it, from a store its tile fills once. Cells
// are addressed by the same integer hash, so a cloud's count, center, size,
// shape, and strength come from the same draws: every draw is
// `hash(cell, sequence)`, so any draw can be fetched directly. Only the
// Gaussian itself (`exp`, `sin`, `cos`) and the summation order differ.
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
// Process records of 16 words from 0, then the count tables of unresolved
// processes, then the scanner taps, then the granularity curves.
@group(0) @binding(4) var<storage, read> tables: array<f32>;

const SUPPORT_D2: f32 = 12.25;
const STRENGTH_LO: f32 = 0.72;
const STRENGTH_HI: f32 = 1.0;
const TAU: f32 = 6.28318530717958648;
const MAX_POISSON: u32 = 64u;
// `AGGREGATE_SALT` in `grain.rs`.
const AGGREGATE_SALT: u32 = 0x6A09E667u;

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
    // `Aggregate`: where the count table starts, its length (0 when the
    // whole sum is gaussian), and the mass mean and spread.
    cdf_offset: u32,
    cdf_len: u32,
    mass_mean: f32,
    mass_spread: f32,
}

fn process(index: u32) -> Process {
    let base = index * 16u;
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
        bitcast<u32>(tables[base + 10u]),
        bitcast<u32>(tables[base + 11u]),
        tables[base + 12u],
        tables[base + 13u],
    );
}

// `Aggregate::sample`: an unresolved process's sum at band pixel `pixel`,
// drawn per pixel from its full-frame position.
fn aggregate(p: Process, pixel: vec2<i32>) -> f32 {
    let at = params.origin.xy + pixel;
    let seed = p.seed ^ AGGREGATE_SALT;
    let pick = unit(at, 1u, seed);
    let radius = sqrt(-2.0 * log(1.0 - unit(at, 2u, seed)));
    let normal = radius * cos(unit(at, 3u, seed) * TAU);
    if p.cdf_len == 0u {
        return p.mass_mean + p.mass_spread * normal;
    }
    var count = 0u;
    while count < p.cdf_len && tables[p.cdf_offset + count] <= pick {
        count += 1u;
    }
    let n = f32(count);
    return n * p.mass_mean + sqrt(n) * p.mass_spread * normal;
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

// One resolved cloud, as `paint_gaussian` sees it. The center is in band
// pixels, as the CPU computes it, and the axes are in pixels.
struct Cloud {
    x: f32,
    y: f32,
    sx: f32,
    sy: f32,
    sin: f32,
    cos: f32,
    amplitude: f32,
}

// Cloud `k` of cell `at`, whose draws start after `drawn` Poisson draws.
fn cloud(p: Process, at: vec2<i32>, drawn: u32, k: u32) -> Cloud {
    let pitch = params.scalars.x;
    let origin = params.origin.xy;
    let base = drawn + 6u * k;
    let x_mm = (f32(at.x) + unit(at, base + 1u, p.seed)) * p.cell_mm;
    let y_mm = (f32(at.y) + unit(at, base + 2u, p.seed)) * p.cell_mm;
    let ur = unit(at, base + 3u, p.seed);
    let ua = unit(at, base + 4u, p.seed);
    let ut = unit(at, base + 5u, p.seed);
    let uf = unit(at, base + 6u, p.seed);
    let sigma = log_lerp(p.sigma_lo, p.sigma_hi, ur);
    let strength = STRENGTH_LO + (STRENGTH_HI - STRENGTH_LO) * uf;
    let axis = sqrt(log_lerp(1.0, p.aspect_max, ua));
    let angle = ut * TAU;
    return Cloud(
        x_mm / pitch - f32(origin.x),
        y_mm / pitch - f32(origin.y),
        sigma / axis / pitch,
        sigma * axis / pitch,
        sin(angle),
        cos(angle),
        p.amplitude * strength,
    );
}

// The pixels `paint_gaussian` visits for `c`, clipped to the band: x0, y0,
// x1, y1. Empty when x0 > x1 or y0 > y1.
fn footprint(c: Cloud) -> vec4<i32> {
    if c.sx <= 0.0 || c.sy <= 0.0 {
        return vec4<i32>(0, 0, -1, -1);
    }
    let rad = 3.5 * max(c.sx, c.sy);
    return vec4<i32>(
        max(i32(ceil(c.x - rad - 0.5)), 0),
        max(i32(ceil(c.y - rad - 0.5)), 0),
        min(i32(floor(c.x + rad - 0.5)), i32(params.band.x) - 1),
        min(i32(floor(c.y + rad - 0.5)), i32(params.band.y) - 1),
    );
}

// Whether `c` paints any pixel of the tile from `lo` to `hi`.
fn touches(c: Cloud, lo: vec2<i32>, hi: vec2<i32>) -> bool {
    let box = footprint(c);
    return box.x <= box.z && box.y <= box.w
        && box.x <= hi.x && box.z >= lo.x && box.y <= hi.y && box.w >= lo.y;
}

// What `c` adds at band pixel `pixel`, a pixel inside the band.
//
// The CPU also clips to `footprint`, the square of half-side 3.5 times the
// longer axis. The 3.5-sigma ellipse sits inside that square, and the band
// clip cannot exclude a pixel inside the band, so the ellipse test alone
// decides.
fn shade(c: Cloud, pixel: vec2<i32>) -> f32 {
    if c.sx <= 0.0 || c.sy <= 0.0 {
        return 0.0;
    }
    let dx = f32(pixel.x) + 0.5 - c.x;
    let dy = f32(pixel.y) + 0.5 - c.y;
    let xr = dx * c.cos + dy * c.sin;
    let yr = -dx * c.sin + dy * c.cos;
    let inv_sx = 1.0 / c.sx;
    let inv_sy = 1.0 / c.sy;
    let d2 = xr * inv_sx * xr * inv_sx + yr * inv_sy * yr * inv_sy;
    if d2 >= SUPPORT_D2 {
        return 0.0;
    }
    return c.amplitude * exp(-0.5 * d2);
}

// Every cloud of cell `at`, at band pixel `pixel`.
fn cell_sum(p: Process, at: vec2<i32>, pixel: vec2<i32>) -> f32 {
    let drawn = poisson(at, p.seed, p.limit);
    var sum = 0.0;
    for (var k = 0u; k < drawn.x; k++) {
        sum += shade(cloud(p, at, drawn.y, k), pixel);
    }
    return sum;
}

// A workgroup is one tile of the band. Each resolved process's clouds that
// reach the tile are drawn once, by the tile's lanes, one cell per lane,
// into workgroup memory, in cell order. Every pixel then sums them from
// there. Gathering per pixel instead redrew each cell for every pixel it
// reaches, about 150 cells per pixel at a full-resolution Portra.
const TILE: u32 = 8u;
const LANES: u32 = 64u;
// Clouds held at once. 28 bytes each keeps the tile store under WebGPU's
// 16 KiB minimum. A tile that reaches more is summed in batches.
const CAPACITY: u32 = 512u;

var<workgroup> clouds: array<Cloud, CAPACITY>;
// Clouds per lane in a round, then each lane's first slot.
var<workgroup> slots: array<u32, LANES>;
// First cell, cells per row, and cells of the tile's window. Zero cells for
// a process drawn per pixel.
var<workgroup> window_first: vec2<i32>;
var<workgroup> window_columns: u32;
var<workgroup> window_cells: u32;
// Clouds in the store, and clouds drawn this round.
var<workgroup> stored: u32;
var<workgroup> drawn_total: u32;

// Lane 0: the cells whose clouds can reach the tile, as `gather` once found
// them for each pixel.
fn plan_window(p: Process, lo: vec2<i32>, hi: vec2<i32>) {
    stored = 0u;
    if !p.resolved {
        window_cells = 0u;
        return;
    }
    let pitch = params.scalars.x;
    let origin = params.origin.xy;
    let reach = vec2<f32>(p.reach_mm + 0.5 * pitch);
    let lo_mm = vec2<f32>(origin + lo) * pitch;
    let hi_mm = vec2<f32>(origin + hi) * pitch + vec2<f32>(pitch);
    let first = vec2<i32>(floor((lo_mm - reach) / p.cell_mm));
    let last = vec2<i32>(floor((hi_mm + reach) / p.cell_mm));
    let size = vec2<u32>(last - first + vec2<i32>(1));
    window_first = first;
    window_columns = size.x;
    window_cells = size.x * size.y;
}

fn window_cell(index: u32) -> vec2<i32> {
    return window_first + vec2<i32>(i32(index % window_columns), i32(index / window_columns));
}

// The sum of every stored cloud at `pixel`.
fn stored_sum(pixel: vec2<i32>, count: u32) -> f32 {
    var sum = 0.0;
    for (var i = 0u; i < count; i++) {
        sum += shade(clouds[i], pixel);
    }
    return sum;
}

// The unblurred field over the padded band.
@compute @workgroup_size(8, 8)
fn grain_field(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_index) lane: u32,
    @builtin(workgroup_id) group: vec3<u32>,
) {
    // Every lane reaches every barrier. Lanes past the band edge draw
    // clouds but write nothing.
    let inside = id.x < params.band.x && id.y < params.band.y;
    let pixel = vec2<i32>(id.xy);
    let lo = vec2<i32>(group.xy * TILE);
    let hi = min(lo + vec2<i32>(i32(TILE) - 1), vec2<i32>(params.band.xy) - vec2<i32>(1));
    var value = vec3<f32>(0.0);
    for (var index = 0u; index < u32(params.origin.z); index++) {
        let p = process(index);
        if lane == 0u {
            plan_window(p, lo, hi);
        }
        let cells = workgroupUniformLoad(&window_cells);
        var sum = 0.0;
        if !p.resolved && inside {
            sum = aggregate(p, pixel);
        }
        let rounds = (cells + LANES - 1u) / LANES;
        for (var round = 0u; round < rounds; round++) {
            // This lane's cell, and how many of its clouds reach the tile.
            let cell_index = round * LANES + lane;
            let at = window_cell(cell_index);
            var drawn = vec2<u32>(0u);
            var reaching = 0u;
            if cell_index < cells {
                drawn = poisson(at, p.seed, p.limit);
                for (var k = 0u; k < drawn.x; k++) {
                    if touches(cloud(p, at, drawn.y, k), lo, hi) {
                        reaching += 1u;
                    }
                }
            }
            slots[lane] = reaching;
            workgroupBarrier();
            if lane == 0u {
                var total = 0u;
                for (var i = 0u; i < LANES; i++) {
                    let count = slots[i];
                    slots[i] = total;
                    total += count;
                }
                drawn_total = total;
            }
            let total = workgroupUniformLoad(&drawn_total);
            let held = workgroupUniformLoad(&stored);
            if held + total > CAPACITY {
                // The store is full: sum it, then start it over.
                if inside {
                    sum += stored_sum(pixel, held);
                }
                workgroupBarrier();
                if lane == 0u {
                    stored = 0u;
                }
            }
            let start = workgroupUniformLoad(&stored);
            if total > CAPACITY {
                // More than the store holds in one round, which a Poisson
                // count makes vanishingly rare: each pixel draws this
                // round's cells itself, still in cell order.
                for (var other = 0u; other < LANES; other++) {
                    let other_index = round * LANES + other;
                    if inside && other_index < cells {
                        sum += cell_sum(p, window_cell(other_index), pixel);
                    }
                }
            } else {
                if cell_index < cells {
                    var slot = start + slots[lane];
                    for (var k = 0u; k < drawn.x; k++) {
                        let c = cloud(p, at, drawn.y, k);
                        if touches(c, lo, hi) {
                            clouds[slot] = c;
                            slot += 1u;
                        }
                    }
                }
                workgroupBarrier();
                if lane == 0u {
                    stored = start + total;
                }
            }
        }
        let held = workgroupUniformLoad(&stored);
        if inside {
            sum += stored_sum(pixel, held);
        }
        // The next process reuses the store.
        workgroupBarrier();
        value += sum * layer_mask(p.layers);
    }
    if inside {
        let base = (id.y * params.band.x + id.x) * 3u;
        field[base] = value.x;
        field[base + 1u] = value.y;
        field[base + 2u] = value.z;
    }
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
