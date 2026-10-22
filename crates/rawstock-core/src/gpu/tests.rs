//! CPU vs GPU parity. Tolerances are in each stage's own units and are the
//! ones `docs/GPU.md` documents.
//!
//! Without an adapter these tests print a note and pass, unless
//! `RAWSTOCK_REQUIRE_GPU=1`, which the GPU CI job sets.

use std::path::PathBuf;
use std::sync::OnceLock;

use super::*;
use crate::decode::Orientation;
use crate::render::{NopControl, Pipeline};
use crate::session::{DisplayRect, PreviewSession, ScanMode};
use crate::stock::{Grain, GrainRegion, Halation, Stock};

/// Exposure is one multiply by the same f32 scale: bit for bit.
const EXPOSURE_TOLERANCE: f32 = 0.0;
/// Density, after log10 and the D-logH curve.
const DENSITY_TOLERANCE: f32 = 2.0e-5;
/// Scanned printing density, after 41 transmittances and a log10. Relative
/// above a density of 1: a dense patch sums tiny transmittances against an
/// observer with negative lobes, so the sum cancels.
const DYE_SCAN_TOLERANCE: f32 = 5.0e-5;
/// Scene-linear exposure after halation, relative above 1. The GPU sums each
/// box window directly in f32; the CPU slides an f64 running sum.
const HALATION_TOLERANCE: f32 = 1.0e-5;
/// Density after grain, per sample. The clouds are the same draws, so only
/// the Gaussian's `exp`, `sin`, and `cos` and the summation order differ.
const GRAIN_TOLERANCE: f32 = 1.0e-3;
/// The same, as an RMS over the frame: well under a thousandth of the grain.
const GRAIN_RMS_TOLERANCE: f64 = 2.0e-5;
/// Display-linear transmission from print/scan, relative to the value.
const PRINT_TOLERANCE: f32 = 1.0e-4;
/// Encoded sRGB in `0..=1`.
const ENCODE_TOLERANCE: f32 = 2.0e-5;

fn gpu() -> Option<&'static Gpu> {
    static GPU: OnceLock<Option<Gpu>> = OnceLock::new();
    let gpu = GPU.get_or_init(|| match Gpu::new_blocking() {
        Ok(gpu) => {
            eprintln!("GPU parity on {}", gpu.adapter_name());
            Some(gpu)
        }
        Err(error) => {
            assert!(
                std::env::var("RAWSTOCK_REQUIRE_GPU").as_deref() != Ok("1"),
                "RAWSTOCK_REQUIRE_GPU=1 but {error}"
            );
            None
        }
    });
    if gpu.is_none() {
        eprintln!("skipping GPU parity: no adapter");
    }
    gpu.as_ref()
}

fn stock(name: &str) -> Parameters {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stocks")
        .join(format!("{name}.stock.toml"));
    Stock::load(path).expect("stock loads").into_parameters()
}

/// `name` without halation or grain, for the per-pixel stage tests.
fn bare(name: &str) -> Parameters {
    let mut parameters = stock(name);
    parameters.set_halation(None).unwrap();
    parameters.set_grain(None).unwrap();
    parameters
}

/// A dark frame with hot points, a hot bar on the left edge, and a bright
/// lower-right corner, so the glow meets every border.
fn highlights(width: u32, height: u32) -> LinearImage {
    let mut pixels = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            let roll = unit(y * width + x, 3);
            let mut rgb = [0.02 + 0.1 * roll, 0.03, 0.01 + 0.05 * roll];
            let hot =
                (x % 37 == 5 && y % 23 == 7) || x < 3 || (x > width * 3 / 4 && y > height * 3 / 4);
            if hot {
                rgb = [6.0 + 20.0 * roll, 4.0, 2.0];
            }
            pixels.extend(rgb);
        }
    }
    LinearImage::new(width, height, pixels).unwrap()
}

/// Exposure and halation on the CPU, as `Pipeline::develop` runs them.
fn cpu_halation(parameters: &Parameters, image: &LinearImage, sigma: f32) -> Vec<f32> {
    let exposure =
        crate::render::exposure_buffer(image.pixels(), parameters.exposure_ev(), &mut NopControl)
            .unwrap();
    crate::render::halation_buffer(
        exposure,
        parameters.halation(),
        image.width(),
        image.height(),
        Some(sigma),
        &mut NopControl,
    )
    .unwrap()
}

/// A hash in `0..1`, so the inputs are the same on every run.
fn unit(index: u32, salt: u32) -> f32 {
    (Grain::hash(index, salt, 0, 0x5EED) >> 8) as f32 / 16_777_216.0
}

/// Scene-linear samples across 11 decades, plus zero, negatives, and
/// every channel order, so curve ends, the toe floor, and coupling all run.
fn scene_samples(pixels: u32) -> Vec<f32> {
    let mut samples = Vec::with_capacity(pixels as usize * 3);
    for index in 0..pixels {
        for channel in 0..3 {
            let roll = unit(index, channel);
            let value = match index % 17 {
                0 => 0.0,
                1 => -roll,
                _ => 10f32.powf(-8.0 + 11.0 * roll),
            };
            samples.push(value);
        }
    }
    samples
}

fn assert_close(stage: &str, cpu: &[f32], gpu: &[f32], tolerance: impl Fn(f32) -> f32) {
    assert_eq!(cpu.len(), gpu.len(), "{stage}: sample count");
    let mut worst = (0.0f32, 0usize);
    for (index, (expected, actual)) in cpu.iter().zip(gpu).enumerate() {
        let error = (expected - actual).abs();
        assert!(
            error <= tolerance(*expected),
            "{stage}: sample {index} cpu {expected} gpu {actual} (error {error})"
        );
        if error > worst.0 {
            worst = (error, index);
        }
    }
    eprintln!(
        "{stage}: worst error {} at sample {} (cpu {})",
        worst.0, worst.1, cpu[worst.1]
    );
}

fn cpu_print(parameters: &Parameters, density: &[f32]) -> Vec<f32> {
    crate::render::print_buffer(
        density,
        parameters.print_exposure(),
        parameters.print_curve(),
        &mut NopControl,
    )
    .unwrap()
}

#[test]
fn exposure_matches_bit_for_bit() {
    let Some(gpu) = gpu() else { return };
    let mut parameters = stock("velvia");
    parameters.set_exposure_ev(1.37).unwrap();
    let pipeline = GpuPipeline::new(gpu, &parameters);
    let input = scene_samples(4096);
    let cpu =
        crate::render::exposure_buffer(&input, parameters.exposure_ev(), &mut NopControl).unwrap();
    let gpu = pollster::block_on(pipeline.run(&[Kernel::Exposure], &input)).unwrap();
    assert_close("exposure", &cpu, &gpu, |_| EXPOSURE_TOLERANCE);
}

#[test]
fn parametric_density_matches_with_and_without_coupling() {
    let Some(gpu) = gpu() else { return };
    // Velvia has a coupling matrix. Neutral does not.
    for name in ["velvia", "neutral"] {
        let parameters = stock(name);
        let pipeline = GpuPipeline::new(gpu, &parameters);
        let input = scene_samples(65_536);
        let cpu = parameters.film().density(&input);
        let gpu = pollster::block_on(pipeline.run(&[Kernel::DensityParametric], &input)).unwrap();
        assert_close(&format!("{name} density"), &cpu, &gpu, |_| {
            DENSITY_TOLERANCE
        });
    }
}

#[test]
fn spectral_density_matches() {
    let Some(gpu) = gpu() else { return };
    let parameters = bare("portra400");
    let pipeline = GpuPipeline::new(gpu, &parameters);
    let mut input = scene_samples(65_536);
    // Neutrals take the flat-spectrum branch; values above 1 take the
    // scale extension.
    for gray in [0.0, 1.0e-3, 0.18, 1.0, 4.0] {
        input.extend([gray, gray, gray]);
    }
    let cpu = parameters.film().density(&input);
    let gpu = pollster::block_on(pipeline.run(&[Kernel::DensitySpectral], &input)).unwrap();
    assert_close("portra density", &cpu, &gpu, |_| DENSITY_TOLERANCE);
}

#[test]
fn dye_scan_matches() {
    let Some(gpu) = gpu() else { return };
    let parameters = bare("portra400");
    let pipeline = GpuPipeline::new(gpu, &parameters);
    let density: Vec<f32> = (0..65_536 * 3)
        .map(|index| -0.2 + 3.5 * unit(index, 13))
        .collect();
    let cpu = parameters
        .film()
        .printing_density(&density)
        .expect("Portra has dye curves");
    let gpu = pollster::block_on(pipeline.run(&[Kernel::DyeScan], &density)).unwrap();
    assert_close("dye scan", &cpu, &gpu, |value| {
        DYE_SCAN_TOLERANCE * value.abs().max(1.0)
    });
}

#[test]
fn halation_matches() {
    let Some(gpu) = gpu() else { return };
    let mut parameters = bare("velvia");
    // Strong enough that the glow is most of a dark pixel's value.
    parameters
        .set_halation(Some(Halation::new(1.0, 0.02, [0.6, 0.15, 0.02]).unwrap()))
        .unwrap();
    let image = highlights(301, 197);
    let halation = parameters.halation().unwrap().clone();
    let pipeline = GpuPipeline::new(gpu, &parameters);
    // Below half a pixel there is no blur, only the add-back. Then the
    // stock's own sigma, and a radius wider than the frame.
    for sigma in [0.3, halation.sigma_px(301, 197), 150.0] {
        let cpu = cpu_halation(&parameters, &image, sigma);
        let stages = Stages {
            pre: &[Kernel::Exposure],
            halation: Some((&halation, sigma)),
            mid: &[],
            grain: None,
            post: &[],
        };
        let gpu = pollster::block_on(pipeline.run_frame(image.pixels(), 301, 197, stages)).unwrap();
        assert_close(&format!("halation sigma {sigma}"), &cpu, &gpu, |value| {
            HALATION_TOLERANCE * value.abs().max(1.0)
        });
    }
}

/// Grain on its own: density in, density with grain out.
/// Grain over a density ramp raised by `shift`, on the CPU and on `gpu`.
fn grain_case(
    gpu: &Gpu,
    grain: &Grain,
    region: GrainRegion,
    shift: f32,
) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let pixels = region.width * region.height;
    let density: Vec<f32> = (0..pixels * 3)
        .map(|index| {
            let pixel = index / 3;
            let x = (pixel % region.width) as f32 / region.width as f32;
            let y = (pixel / region.width) as f32 / region.height as f32;
            shift + 0.2 + 2.3 * (0.5 * x + 0.5 * y) + 0.05 * (index % 3) as f32
        })
        .collect();
    let mut cpu = density.clone();
    grain
        .apply_region(&mut cpu, region, &mut NopControl)
        .unwrap();
    let pipeline = GpuPipeline::new(gpu, &bare("portra400"));
    let stages = Stages {
        pre: &[],
        halation: None,
        mid: &[],
        grain: Some((grain, region)),
        post: &[],
    };
    let gpu = pollster::block_on(pipeline.run_frame(&density, region.width, region.height, stages))
        .unwrap();
    (density, cpu, gpu)
}

#[test]
fn grain_matches() {
    let Some(gpu) = gpu() else { return };
    let grain = stock("portra400").grain().unwrap().clone();
    let cases = [
        // A coarse preview: every cloud is smaller than a pixel.
        (
            "preview",
            GrainRegion {
                width: 384,
                height: 256,
                origin_x: 0,
                origin_y: 0,
                frame_width: 384,
            },
        ),
        // Between: fine clouds are drawn per pixel, coarse ones are Gaussian
        // stamps.
        (
            "mixed",
            GrainRegion {
                width: 200,
                height: 120,
                origin_x: 900,
                origin_y: 600,
                frame_width: 2000,
            },
        ),
        // A 100% crop of a 24 MP frame: resolved clouds and the scanner blur.
        (
            "full",
            GrainRegion {
                width: 160,
                height: 96,
                origin_x: 2400,
                origin_y: 1500,
                frame_width: 6000,
            },
        ),
    ];
    for (label, region) in cases {
        assert_grain(label, grain_case(gpu, &grain, region, 0.0));
    }
}

fn assert_grain(label: &str, (density, cpu, gpu): (Vec<f32>, Vec<f32>, Vec<f32>)) {
    let mut worst = 0.0f32;
    let mut energy = 0.0f64;
    let mut error = 0.0f64;
    for ((input, expected), actual) in density.iter().zip(&cpu).zip(&gpu) {
        let delta = expected - input;
        energy += f64::from(delta * delta);
        let miss = (expected - actual).abs();
        error += f64::from(miss * miss);
        worst = worst.max(miss);
    }
    let rms = (energy / cpu.len() as f64).sqrt();
    let error_rms = (error / cpu.len() as f64).sqrt();
    eprintln!("grain {label}: grain rms {rms:.3e}, error rms {error_rms:.3e}, worst {worst:.3e}");
    assert!(rms > 1.0e-4, "{label}: no grain on the CPU");
    assert!(worst <= GRAIN_TOLERANCE, "{label}: worst {worst}");
    assert!(
        error_rms <= GRAIN_RMS_TOLERANCE,
        "{label}: error rms {error_rms}"
    );
}

#[test]
fn grain_field_is_reused_across_edits() {
    let Some(gpu) = gpu() else { return };
    // A fresh cache, and several chunks, each with its own cached band.
    let gpu = gpu.with_chunk_limit(4000);
    let grain = stock("portra400").grain().unwrap().clone();
    let region = GrainRegion {
        width: 160,
        height: 96,
        origin_x: 2400,
        origin_y: 1500,
        frame_width: 6000,
    };
    let cached = |gpu: &Gpu| gpu.inner.grain_fields.lock().unwrap().len();
    assert_grain("first", grain_case(&gpu, &grain, region, 0.0));
    assert_eq!(cached(&gpu), 1);
    // A different picture under the same grain reads the cached field.
    assert_grain("edited", grain_case(&gpu, &grain, region, 0.4));
    assert_eq!(cached(&gpu), 1);
    let fields = gpu.inner.grain_fields.lock().unwrap()[0].fields.len();
    assert!(fields > 1, "{fields} chunks");
    // Another crop is another field.
    let moved = GrainRegion {
        origin_x: 2410,
        ..region
    };
    assert_grain("moved", grain_case(&gpu, &grain, moved, 0.0));
    assert_eq!(cached(&gpu), 2);
}

#[test]
fn print_scan_matches() {
    let Some(gpu) = gpu() else { return };
    let parameters = stock("velvia");
    let pipeline = GpuPipeline::new(gpu, &parameters);
    let density: Vec<f32> = (0..65_536 * 3)
        .map(|index| -0.5 + 4.0 * unit(index, 7))
        .collect();
    let cpu = cpu_print(&parameters, &density);
    let gpu = pollster::block_on(pipeline.run(&[Kernel::Print], &density)).unwrap();
    assert_close("print", &cpu, &gpu, |value| {
        PRINT_TOLERANCE * value.abs().max(1.0e-3)
    });
}

#[test]
fn encode_matches() {
    let Some(gpu) = gpu() else { return };
    let pipeline = GpuPipeline::new(gpu, &stock("neutral"));
    let mut linear: Vec<f32> = (0..65_536 * 3)
        .map(|index| -0.25 + 1.75 * unit(index, 11))
        .collect();
    // The linear toe of the OETF, and non-finite samples.
    linear.extend([
        0.0,
        0.001,
        0.0031308,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ]);
    let cpu = crate::color::encode_srgb(&linear);
    let gpu = pollster::block_on(pipeline.run(&[Kernel::Encode], &linear)).unwrap();
    // A non-finite input makes the matrix output non-finite on both sides,
    // and the OETF then writes 0. Compare those pixels too.
    assert_close("encode", &cpu, &gpu, |_| ENCODE_TOLERANCE);
}

/// The 8-bit bound: at least 99.9% of codes equal, none off by more than 1.
fn assert_rgb8_parity(label: &str, cpu: &[u8], gpu: &[u8]) {
    assert_eq!(cpu.len(), gpu.len());
    let mut differ = 0usize;
    for (index, (expected, actual)) in cpu.iter().zip(gpu).enumerate() {
        let error = expected.abs_diff(*actual);
        assert!(
            error <= 1,
            "{label}: code {index} cpu {expected} gpu {actual}"
        );
        differ += usize::from(error != 0);
    }
    let fraction = differ as f64 / cpu.len() as f64;
    eprintln!("{label}: {differ} of {} codes differ by 1", cpu.len());
    assert!(fraction <= 1.0e-3, "{label}: {fraction:.5} of codes differ");
}

#[test]
fn develop_matches_the_cpu_pipeline() {
    let Some(gpu) = gpu() else { return };
    let (width, height) = (512, 256);
    // A smooth exposure field, so the comparison is on a picture rather than
    // noise: hue across, brightness down, 12 stops in all.
    let mut pixels = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        for x in 0..width {
            let stops = -9.0 + 12.0 * y as f32 / (height - 1) as f32;
            let level = 0.18 * 2f32.powf(stops);
            let hue = std::f32::consts::TAU * x as f32 / width as f32;
            for channel in 0..3 {
                let phase = hue - channel as f32 * std::f32::consts::TAU / 3.0;
                pixels.push(level * (1.0 + 0.8 * phase.cos()));
            }
        }
    }
    let image = LinearImage::new(width as u32, height as u32, pixels).unwrap();
    for name in ["velvia", "neutral", "portra400"] {
        let mut parameters = stock(name);
        parameters.set_exposure_ev(0.4).unwrap();
        let cpu = Pipeline::new(parameters.clone()).develop(&image);
        let developed = GpuPipeline::new(gpu, &parameters)
            .develop_blocking(&image)
            .unwrap();
        assert_eq!(developed.fidelity(), Fidelity::Exact);
        assert_eq!(
            (developed.width(), developed.height()),
            (cpu.width(), cpu.height())
        );
        assert_close(
            &format!("{name} develop"),
            cpu.pixels(),
            developed.pixels(),
            |_| 1.0e-3,
        );
        assert_rgb8_parity(
            &format!("{name} develop"),
            &cpu.to_rgb8(),
            &developed.to_rgb8(),
        );
    }
}

#[test]
fn chunks_do_not_change_the_result() {
    let Some(gpu) = gpu() else { return };
    let input = scene_samples(1000);
    for name in ["velvia", "portra400"] {
        let parameters = bare(name);
        let whole = GpuPipeline::new(gpu, &parameters);
        let chunked = GpuPipeline::new(&gpu.with_chunk_limit(7), &parameters);
        let expected = pollster::block_on(whole.run(&whole.sequence, &input)).unwrap();
        let actual = pollster::block_on(chunked.run(&chunked.sequence, &input)).unwrap();
        assert_eq!(expected, actual, "{name}");
    }
    // Halation and grain read across chunk borders.
    let parameters = stock("portra400");
    let image = highlights(97, 61);
    let whole = GpuPipeline::new(gpu, &parameters);
    let chunked = GpuPipeline::new(&gpu.with_chunk_limit(500), &parameters);
    assert_eq!(
        whole.develop_blocking(&image).unwrap().pixels(),
        chunked.develop_blocking(&image).unwrap().pixels(),
    );
}

/// A session on `gpu` and one on the CPU over the same frame and stock.
fn sessions(gpu: &Gpu, name: &str) -> (PreviewSession, PreviewSession) {
    let image = highlights(640, 427);
    let mut cpu = PreviewSession::new(image.clone(), Orientation::Normal, 256).unwrap();
    let mut on_gpu = PreviewSession::new(image, Orientation::Normal, 256).unwrap();
    cpu.set_parameters(stock(name));
    on_gpu.set_parameters(stock(name));
    on_gpu.set_gpu(Some(gpu.clone()));
    (cpu, on_gpu)
}

#[test]
fn session_renders_match_the_cpu() {
    let Some(gpu) = gpu() else { return };
    for name in ["velvia", "portra400"] {
        let (mut cpu, mut on_gpu) = sessions(gpu, name);
        // A crop away from the corner, so halation pads on every side and
        // grain sits at a full-frame offset.
        let rect = DisplayRect {
            x: 180,
            y: 120,
            width: 200,
            height: 150,
        };
        let expected = cpu
            .render_region(rect, ScanMode::Exact, &mut NopControl)
            .unwrap();
        let actual = on_gpu
            .render_region(rect, ScanMode::Preview, &mut NopControl)
            .unwrap();
        assert_eq!(actual.fidelity(), Fidelity::Exact);
        assert_eq!((actual.width(), actual.height()), (200, 150));
        assert_rgb8_parity(
            &format!("{name} crop"),
            &expected.to_rgb8(),
            &actual.to_rgb8(),
        );

        let expected = cpu.render_full(&mut NopControl).unwrap();
        let actual = on_gpu.render_full(&mut NopControl).unwrap();
        assert_rgb8_parity(
            &format!("{name} full"),
            &expected.to_rgb8(),
            &actual.to_rgb8(),
        );

        // The crop is the same pixels as the full frame.
        let full = actual.to_rgb8();
        let crop = on_gpu
            .render_region(rect, ScanMode::Exact, &mut NopControl)
            .unwrap()
            .to_rgb8();
        for row in 0..150usize {
            let start = ((120 + row) * 640 + 180) * 3;
            assert_eq!(&full[start..start + 600], &crop[row * 600..(row + 1) * 600]);
        }
    }
    // Velvia has no dye curves, so the CPU proxy is exact too.
    let (mut cpu, mut on_gpu) = sessions(gpu, "velvia");
    let expected = cpu.render_proxy(&mut NopControl).unwrap();
    let actual = on_gpu.render_proxy(&mut NopControl).unwrap();
    assert_rgb8_parity("velvia proxy", &expected.to_rgb8(), &actual.to_rgb8());
}

/// Wall time of a full Portra develop, CPU then GPU, on a frame the size of
/// `RAWSTOCK_TIMING_MP` megapixels (default 6). Not a parity test:
/// `cargo test --release -p rawstock-core --features gpu --lib timing -- --ignored --nocapture`
#[test]
#[ignore]
fn timing() {
    let Some(gpu) = gpu() else { return };
    let megapixels: f32 = std::env::var("RAWSTOCK_TIMING_MP")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(6.0);
    let height = (megapixels * 1.0e6 / 1.5).sqrt() as u32;
    let width = height * 3 / 2;
    let image = highlights(width, height);
    let frame_width: u32 = std::env::var("RAWSTOCK_TIMING_FRAME")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(width);
    let mut parameters = stock("portra400");
    if std::env::var("RAWSTOCK_TIMING_GRAIN").as_deref() == Ok("0") {
        parameters.set_grain(None).unwrap();
    }
    parameters.set_halation(None).unwrap();
    // A crop of a `frame_width` frame, so a small test can time full
    // resolution grain.
    let region = GrainRegion {
        width,
        height,
        origin_x: 0,
        origin_y: 0,
        frame_width,
    };
    let start = std::time::Instant::now();
    let mut cpu = Pipeline::new(parameters.clone())
        .develop(&image)
        .pixels()
        .to_vec();
    if let Some(grain) = parameters.grain() {
        // The CPU develop above used its own frame width; redo grain alone
        // for the comparison below.
        let mut density = parameters.film().density(image.pixels());
        grain
            .apply_region(&mut density, region, &mut NopControl)
            .unwrap();
        cpu = density;
    }
    let cpu_time = start.elapsed();
    let pipeline = GpuPipeline::new(gpu, &parameters);
    let stages = |grain| Stages {
        pre: &[],
        halation: None,
        mid: &pipeline.sequence[1..2],
        grain,
        post: &[],
    };
    let run = || {
        let grain = parameters.grain().map(|grain| (grain, region));
        pollster::block_on(pipeline.run_frame(image.pixels(), width, height, stages(grain)))
            .unwrap()
    };
    // The first run builds the shaders and the grain field; the second is
    // a preview edit, which reuses the field.
    let start = std::time::Instant::now();
    let first = run();
    let first_time = start.elapsed();
    let start = std::time::Instant::now();
    let developed = run();
    let gpu_time = start.elapsed();
    assert_eq!(first, developed);
    eprintln!(
        "{width}×{height} of a {frame_width} px frame, density + grain: CPU {cpu_time:.2?} \
         (develop and grain), GPU first {first_time:.2?}, then {gpu_time:.2?} on {}",
        gpu.adapter_name()
    );
    if parameters.grain().is_some() {
        assert_close("timing grain", &cpu, &developed, |_| GRAIN_TOLERANCE);
    }
}
