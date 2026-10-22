//! CPU vs GPU parity. Tolerances are in each stage's own units and are the
//! ones `docs/GPU.md` documents.
//!
//! Without an adapter these tests print a note and pass, unless
//! `RAWSTOCK_REQUIRE_GPU=1`, which the GPU CI job sets.

use std::path::PathBuf;
use std::sync::OnceLock;

use super::*;
use crate::render::{NopControl, Pipeline};
use crate::stock::{Grain, Granularity, Stock};

/// Exposure is one multiply by the same f32 scale: bit for bit.
const EXPOSURE_TOLERANCE: f32 = 0.0;
/// Density, after log10 and the D-logH curve.
const DENSITY_TOLERANCE: f32 = 2.0e-5;
/// Scanned printing density, after 41 transmittances and a log10. Relative
/// above a density of 1: a dense patch sums tiny transmittances against an
/// observer with negative lobes, so the sum cancels.
const DYE_SCAN_TOLERANCE: f32 = 5.0e-5;
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

/// `name` without halation or grain, the stages the GPU does not run yet.
fn bare(name: &str) -> Parameters {
    let mut parameters = stock(name);
    parameters.set_halation(None).unwrap();
    parameters.set_grain(None).unwrap();
    parameters
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
    let pipeline = GpuPipeline::new(gpu, &parameters).unwrap();
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
        let pipeline = GpuPipeline::new(gpu, &parameters).unwrap();
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
    let pipeline = GpuPipeline::new(gpu, &parameters).unwrap();
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
    let pipeline = GpuPipeline::new(gpu, &parameters).unwrap();
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
fn print_scan_matches() {
    let Some(gpu) = gpu() else { return };
    let parameters = stock("velvia");
    let pipeline = GpuPipeline::new(gpu, &parameters).unwrap();
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
    let pipeline = GpuPipeline::new(gpu, &stock("neutral")).unwrap();
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
        let mut parameters = bare(name);
        parameters.set_exposure_ev(0.4).unwrap();
        let cpu = Pipeline::new(parameters.clone()).develop(&image);
        let developed = GpuPipeline::new(gpu, &parameters)
            .unwrap()
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
        let whole = GpuPipeline::new(gpu, &parameters).unwrap();
        let chunked = GpuPipeline::new(&gpu.with_chunk_limit(7), &parameters).unwrap();
        let expected = pollster::block_on(whole.run(&whole.sequence, &input)).unwrap();
        let actual = pollster::block_on(chunked.run(&chunked.sequence, &input)).unwrap();
        assert_eq!(expected, actual, "{name}");
    }
}

#[test]
fn unported_stages_are_refused() {
    let Some(gpu) = gpu() else { return };
    // Portra carries halation and grain.
    assert!(matches!(
        GpuPipeline::new(gpu, &stock("portra400")),
        Err(GpuError::Unsupported(Stage::Halation))
    ));
    let layer = Granularity::new([(0.0, 4.0), (3.0, 4.0)]).unwrap();
    let grain = Grain::new(Grain::FRAME_MM, 1, [layer.clone(), layer.clone(), layer]).unwrap();
    let grained = stock("velvia").with_grain(grain);
    assert!(matches!(
        GpuPipeline::new(gpu, &grained),
        Err(GpuError::Unsupported(Stage::Grain))
    ));
}
