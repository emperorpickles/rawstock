// Per-pixel film stages. Each entry point rewrites `data` in place.
//
// `data` is interleaved RGB, one f32 per sample, the same layout as the CPU
// buffers in `render.rs`. Every function mirrors the CPU code it names, and
// the parity tests in `gpu/tests.rs` hold it to that.

struct Params {
    // x: pixel count, y: 1 when the film has a coupling matrix.
    counts: vec4<u32>,
    // x: exposure scale (2^ev, computed on the CPU), y: print exposure.
    scalars: vec4<f32>,
    coupling: array<vec4<f32>, 3>,
    srgb: array<vec4<f32>, 3>,
    // Red, green, and blue film curves, then the print curve.
    curve_offset: vec4<u32>,
    curve_len: vec4<u32>,
}

@group(0) @binding(0) var<uniform> params: Params;
// Each curve is `len` log-exposures, then `len` densities, from its offset.
@group(0) @binding(1) var<storage, read> knots: array<f32>;
@group(0) @binding(2) var<storage, read_write> data: array<f32>;

const EXPOSURE_FLOOR: f32 = 1.0e-6;
const LOG10_2: f32 = 0.30102999566398120;
const LOG2_10: f32 = 3.32192809488736235;

// Bit tests, so a shader compiler cannot fold them away the way it may
// fold `x != x`.
fn is_finite(x: f32) -> bool {
    return (bitcast<u32>(x) & 0x7f800000u) != 0x7f800000u;
}

fn is_nan(x: f32) -> bool {
    let bits = bitcast<u32>(x);
    return (bits & 0x7f800000u) == 0x7f800000u && (bits & 0x007fffffu) != 0u;
}

// `f32::max(x, floor)`: NaN gives the floor.
fn floor_exposure(x: f32) -> f32 {
    if is_nan(x) || x < EXPOSURE_FLOOR {
        return EXPOSURE_FLOOR;
    }
    return x;
}

fn log10(x: f32) -> f32 {
    return log2(x) * LOG10_2;
}

fn exp10(x: f32) -> f32 {
    return exp2(x * LOG2_10);
}

// `Curve::eval`.
fn curve_eval(curve: u32, value: f32) -> f32 {
    let offset = params.curve_offset[curve];
    let len = params.curve_len[curve];
    let first = knots[offset];
    let last = knots[offset + len - 1u];
    var x = value;
    if !is_finite(x) {
        x = first;
    }
    if x <= first {
        return knots[offset + len];
    }
    if x >= last {
        return knots[offset + 2u * len - 1u];
    }
    // `partition_point(|point| *point <= x)`.
    var lo = 0u;
    var hi = len;
    while lo < hi {
        let mid = (lo + hi) / 2u;
        if knots[offset + mid] <= x {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    let upper = lo;
    let lower = upper - 1u;
    let x0 = knots[offset + lower];
    let span = knots[offset + upper] - x0;
    let weight = (x - x0) / span;
    let d0 = knots[offset + len + lower];
    let d1 = knots[offset + len + upper];
    return d0 + weight * (d1 - d0);
}

fn pixel_index(id: vec3<u32>, groups: vec3<u32>) -> u32 {
    return id.x + id.y * groups.x * 256u;
}

fn load(pixel: u32) -> vec3<f32> {
    let base = pixel * 3u;
    return vec3<f32>(data[base], data[base + 1u], data[base + 2u]);
}

fn store(pixel: u32, value: vec3<f32>) {
    let base = pixel * 3u;
    data[base] = value.x;
    data[base + 1u] = value.y;
    data[base + 2u] = value.z;
}

// `apply_exposure`.
@compute @workgroup_size(256)
fn exposure(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let pixel = pixel_index(id, groups);
    if pixel >= params.counts.x {
        return;
    }
    store(pixel, load(pixel) * params.scalars.x);
}

// `Parametric::density_controlled`, and `mix` for the coupling.
@compute @workgroup_size(256)
fn density_parametric(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let pixel = pixel_index(id, groups);
    if pixel >= params.counts.x {
        return;
    }
    let exposure = load(pixel);
    var log_h = vec3<f32>(
        log10(floor_exposure(exposure.x)),
        log10(floor_exposure(exposure.y)),
        log10(floor_exposure(exposure.z)),
    );
    if params.counts.y != 0u {
        log_h = vec3<f32>(
            dot(params.coupling[0].xyz, log_h),
            dot(params.coupling[1].xyz, log_h),
            dot(params.coupling[2].xyz, log_h),
        );
    }
    store(pixel, vec3<f32>(curve_eval(0u, log_h.x), curve_eval(1u, log_h.y), curve_eval(2u, log_h.z)));
}

// `print_scan`.
@compute @workgroup_size(256)
fn print_scan(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let pixel = pixel_index(id, groups);
    if pixel >= params.counts.x {
        return;
    }
    let density = load(pixel);
    let print_exposure = params.scalars.y;
    store(pixel, vec3<f32>(
        exp10(-curve_eval(3u, print_exposure - density.x)),
        exp10(-curve_eval(3u, print_exposure - density.y)),
        exp10(-curve_eval(3u, print_exposure - density.z)),
    ));
}

// `srgb_oetf`.
fn srgb_oetf(value: f32) -> f32 {
    var linear = 0.0;
    if is_finite(value) {
        linear = clamp(value, 0.0, 1.0);
    }
    if linear <= 0.0031308 {
        return 12.92 * linear;
    }
    return 1.055 * pow(linear, 1.0 / 2.4) - 0.055;
}

// `color::encode_srgb`.
@compute @workgroup_size(256)
fn encode(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let pixel = pixel_index(id, groups);
    if pixel >= params.counts.x {
        return;
    }
    let linear = load(pixel);
    store(pixel, vec3<f32>(
        srgb_oetf(dot(params.srgb[0].xyz, linear)),
        srgb_oetf(dot(params.srgb[1].xyz, linear)),
        srgb_oetf(dot(params.srgb[2].xyz, linear)),
    ));
}
