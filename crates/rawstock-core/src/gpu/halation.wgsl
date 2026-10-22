// Halation, from `Halation::apply_controlled` in `stock.rs`.
//
// The peak channel above the threshold is the excess. It is blurred with
// Kovesi's three box passes over the whole frame, then added back with the
// per-channel weight. `data` is one chunk of interleaved RGB; `excess` and
// `scratch` hold one sample per pixel for the whole frame.

struct Params {
    // x: frame width, y: frame height, z: box radius, w: the chunk's first pixel.
    dims: vec4<u32>,
    // x: threshold, yzw: red, green, and blue weight.
    scalars: vec4<f32>,
    // x: pixels in the chunk.
    counts: vec4<u32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> data: array<f32>;
@group(0) @binding(2) var<storage, read_write> excess: array<f32>;
@group(0) @binding(3) var<storage, read_write> scratch: array<f32>;

fn chunk_pixel(id: vec3<u32>, groups: vec3<u32>) -> u32 {
    return id.x + id.y * groups.x * 256u;
}

// The excess over the threshold, for each pixel in the chunk.
@compute @workgroup_size(256)
fn threshold(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let pixel = chunk_pixel(id, groups);
    if pixel >= params.counts.x {
        return;
    }
    let base = pixel * 3u;
    let peak = max(max(data[base], data[base + 1u]), data[base + 2u]);
    let over = peak - params.scalars.x;
    var value = 0.0;
    if over > 0.0 {
        value = over;
    }
    excess[params.dims.w + pixel] = value;
}

// `box_blur_horizontal`, `excess` into `scratch`. The CPU keeps a running
// f64 sum. Here each sample sums its own window, so no error accumulates
// along the row. Out-of-frame taps repeat the edge, as on the CPU.
@compute @workgroup_size(16, 16)
fn box_horizontal(@builtin(global_invocation_id) id: vec3<u32>) {
    let width = params.dims.x;
    let height = params.dims.y;
    if id.x >= width || id.y >= height {
        return;
    }
    let radius = i32(params.dims.z);
    let row = id.y * width;
    var sum = 0.0;
    for (var tap = -radius; tap <= radius; tap++) {
        let x = u32(clamp(i32(id.x) + tap, 0, i32(width) - 1));
        sum += excess[row + x];
    }
    scratch[row + id.x] = sum / f32(2 * radius + 1);
}

// `box_blur_vertical`, `scratch` back into `excess`.
@compute @workgroup_size(16, 16)
fn box_vertical(@builtin(global_invocation_id) id: vec3<u32>) {
    let width = params.dims.x;
    let height = params.dims.y;
    if id.x >= width || id.y >= height {
        return;
    }
    let radius = i32(params.dims.z);
    var sum = 0.0;
    for (var tap = -radius; tap <= radius; tap++) {
        let y = u32(clamp(i32(id.y) + tap, 0, i32(height) - 1));
        sum += scratch[y * width + id.x];
    }
    excess[id.y * width + id.x] = sum / f32(2 * radius + 1);
}

// Add the widened excess back with the per-channel weight.
@compute @workgroup_size(256)
fn add_glow(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let pixel = chunk_pixel(id, groups);
    if pixel >= params.counts.x {
        return;
    }
    let glow = excess[params.dims.w + pixel];
    let base = pixel * 3u;
    data[base] += params.scalars.y * glow;
    data[base + 1u] += params.scalars.z * glow;
    data[base + 2u] += params.scalars.w * glow;
}
