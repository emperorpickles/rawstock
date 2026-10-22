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
    // x: wavelength count, y: 1 when the stock has dye spectra, z: rgb2spec
    // cube resolution.
    spectral: vec4<u32>,
    scan_balance: vec4<f32>,
    ap1_to_ap0: array<vec4<f32>, 3>,
}

@group(0) @binding(0) var<uniform> params: Params;
// Each curve is `len` log-exposures, then `len` densities, from its offset.
@group(0) @binding(1) var<storage, read> knots: array<f32>;
@group(0) @binding(2) var<storage, read_write> data: array<f32>;
// Spectral stock tables, each `spectral.x` long: wavelengths, the three
// weighted sensitivities, the three dye spectra, then the three observer rows.
@group(0) @binding(3) var<storage, read> spectral: array<f32>;
// rgb2spec: `spectral.z` z-scale entries, then the coefficient cube.
@group(0) @binding(4) var<storage, read> rgb2spec: array<f32>;

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

// Jakob-Hanika upsampling, from `spectrum.rs`.

struct Upsample {
    coeff: vec3<f32>,
    scale: f32,
}

// `mono_coeff`.
fn mono_coeff(value: f32) -> vec3<f32> {
    var constant: f32;
    if value <= 0.0 {
        constant = -8192.0;
    } else if value >= 1.0 {
        constant = 8192.0;
    } else {
        constant = (value - 0.5) / sqrt(value * (1.0 - value));
    }
    return vec3<f32>(0.0, 0.0, constant);
}

// `find_interval` over the z scale.
fn find_interval(value: f32) -> u32 {
    let last = params.spectral.z - 2u;
    var left = 0u;
    var size = last;
    while size > 0u {
        let half = size >> 1u;
        let middle = left + half + 1u;
        if rgb2spec[middle] <= value {
            left = middle;
            size -= half + 1u;
        } else {
            size = half;
        }
    }
    return min(left, last);
}

// `Rgb2Spec::fetch`.
fn fetch(value: vec3<f32>) -> vec3<f32> {
    let rgb = clamp(value, vec3<f32>(0.0), vec3<f32>(1.0));
    if rgb.x == rgb.y && rgb.y == rgb.z {
        return mono_coeff(rgb.x);
    }
    let res = params.spectral.z;
    let data = res;
    var max_channel = 0u;
    for (var channel = 1u; channel < 3u; channel++) {
        if rgb[channel] >= rgb[max_channel] {
            max_channel = channel;
        }
    }
    let z = rgb[max_channel];
    if z <= 0.0 {
        return vec3<f32>(rgb2spec[data], rgb2spec[data + 1u], rgb2spec[data + 2u]);
    }
    let scale = f32(res - 1u) / z;
    let x = rgb[(max_channel + 1u) % 3u] * scale;
    let y = rgb[(max_channel + 2u) % 3u] * scale;
    let xi = min(u32(x), res - 2u);
    let yi = min(u32(y), res - 2u);
    let zi = find_interval(z);
    let base = data + (((max_channel * res + zi) * res + yi) * res + xi) * 3u;
    let dx = 3u;
    let dy = 3u * res;
    let dz = 3u * res * res;
    let x1 = x - f32(xi);
    let y1 = y - f32(yi);
    let x0 = 1.0 - x1;
    let y0 = 1.0 - y1;
    let z1 = (z - rgb2spec[zi]) / (rgb2spec[zi + 1u] - rgb2spec[zi]);
    let z0 = 1.0 - z1;
    var coeff = vec3<f32>(0.0);
    for (var term = 0u; term < 3u; term++) {
        let at = base + term;
        coeff[term] = ((rgb2spec[at] * x0 + rgb2spec[at + dx] * x1) * y0
            + (rgb2spec[at + dy] * x0 + rgb2spec[at + dy + dx] * x1) * y1) * z0
            + ((rgb2spec[at + dz] * x0 + rgb2spec[at + dz + dx] * x1) * y0
            + (rgb2spec[at + dz + dy] * x0 + rgb2spec[at + dz + dy + dx] * x1) * y1) * z1;
    }
    return coeff;
}

// Negative and non-finite channels are zero before upsampling.
fn reflectance(value: f32) -> f32 {
    if is_finite(value) {
        return max(value, 0.0);
    }
    return 0.0;
}

// `upsample`.
fn upsample(value: vec3<f32>) -> Upsample {
    let rgb = vec3<f32>(reflectance(value.x), reflectance(value.y), reflectance(value.z));
    let high = max(max(rgb.x, rgb.y), rgb.z);
    if high == 0.0 {
        return Upsample(vec3<f32>(0.0), 0.0);
    }
    let low = min(min(rgb.x, rgb.y), rgb.z);
    if high - low <= 1.0e-4 * max(high, 1.0) {
        return Upsample(mono_coeff(1.0), (rgb.x + rgb.y + rgb.z) / 3.0);
    }
    let m = params.ap1_to_ap0;
    let ap0 = max(
        vec3<f32>(
            m[0].x * rgb.x + m[0].y * rgb.y + m[0].z * rgb.z,
            m[1].x * rgb.x + m[1].y * rgb.y + m[1].z * rgb.z,
            m[2].x * rgb.x + m[2].y * rgb.y + m[2].z * rgb.z,
        ),
        vec3<f32>(0.0),
    );
    let peak = max(max(ap0.x, ap0.y), ap0.z);
    if peak == 0.0 {
        return Upsample(vec3<f32>(0.0), 0.0);
    }
    // Every color at a fixed AP0 peak of 1/2, so exposure scales the
    // spectrum without changing its shape (#42).
    let scale = 2.0 * peak;
    return Upsample(fetch(ap0 / scale), scale);
}

// `eval`: the sigmoid of a quadratic in nanometres.
fn sigmoid(coeff: vec3<f32>, lambda: f32) -> f32 {
    let polynomial = (coeff.x * lambda + coeff.y) * lambda + coeff.z;
    return 0.5 * polynomial / sqrt(polynomial * polynomial + 1.0) + 0.5;
}

// `Spectral::density_controlled`: upsample, integrate each layer's
// sensitivity, then the same log, coupling, and curves as parametric.
@compute @workgroup_size(256)
fn density_spectral(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let pixel = pixel_index(id, groups);
    if pixel >= params.counts.x {
        return;
    }
    let count = params.spectral.x;
    let up = upsample(load(pixel));
    let empty = all(up.coeff == vec3<f32>(0.0)) && up.scale == 0.0;
    let flat = up.coeff.x == 0.0 && up.coeff.y == 0.0;
    let level = sigmoid(up.coeff, 0.0) * up.scale;
    var exposure = vec3<f32>(0.0);
    for (var index = 0u; index < count; index++) {
        var sample = 0.0;
        if empty {
            sample = 0.0;
        } else if flat {
            sample = level;
        } else {
            sample = sigmoid(up.coeff, spectral[index]) * up.scale;
        }
        exposure += sample * vec3<f32>(
            spectral[count + index],
            spectral[2u * count + index],
            spectral[3u * count + index],
        );
    }
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

// `Spectral::printing_density_controlled`: the exact dye scan. Transmittance
// of the summed dye density, integrated against D65 and CIE 1931 in ACEScg,
// back to density, less the neutral balance.
@compute @workgroup_size(256)
fn dye_scan(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let pixel = pixel_index(id, groups);
    if pixel >= params.counts.x {
        return;
    }
    let count = params.spectral.x;
    let density = load(pixel);
    var rgb = vec3<f32>(0.0);
    for (var index = 0u; index < count; index++) {
        let optical = spectral[4u * count + index] * density.x
            + spectral[5u * count + index] * density.y
            + spectral[6u * count + index] * density.z;
        let transmittance = exp10(-optical);
        rgb += transmittance * vec3<f32>(
            spectral[7u * count + index],
            spectral[8u * count + index],
            spectral[9u * count + index],
        );
    }
    store(pixel, vec3<f32>(
        -log10(floor_exposure(rgb.x)),
        -log10(floor_exposure(rgb.y)),
        -log10(floor_exposure(rgb.z)),
    ) - params.scan_balance.xyz);
}
