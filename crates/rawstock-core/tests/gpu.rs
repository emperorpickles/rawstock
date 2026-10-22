//! GPU parity on the CC0 fixtures: the preview session renders each stock on
//! the GPU and on the CPU, and the 8-bit output must agree.
//!
//! Without an adapter the tests print a note and pass, unless
//! `RAWSTOCK_REQUIRE_GPU` is set. Downloads follow the decode tests' rule.
#![cfg(feature = "gpu")]

use std::path::PathBuf;

use rawstock_core::decode::RawImage;
use rawstock_core::gpu::Gpu;
use rawstock_core::render::{Fidelity, NopControl};
use rawstock_core::session::{DisplayRect, PreviewSession, ScanMode};
use rawstock_core::stock::Stock;

#[path = "common/mod.rs"]
mod common;

use common::{Fixture, IPHONE_12_PRO, X_S10};

#[test]
fn fuji_xtrans_matches_the_cpu() {
    fixture_parity(&X_S10);
}

#[test]
fn iphone_proraw_matches_the_cpu() {
    fixture_parity(&IPHONE_12_PRO);
}

fn fixture_parity(fixture: &Fixture) {
    let Some(gpu) = gpu() else { return };
    let Some(path) = fixture.prepare() else {
        return;
    };
    let raw = RawImage::open(&path).unwrap();
    let linear = raw.develop_linear().unwrap();
    let mut cpu = PreviewSession::new(linear.clone(), raw.orientation(), 1024).unwrap();
    let mut on_gpu = PreviewSession::new(linear, raw.orientation(), 1024).unwrap();
    on_gpu.set_gpu(Some(gpu));
    let (width, height) = cpu.dimensions();
    // A 100% crop from the middle of the frame.
    let rect = DisplayRect {
        x: width / 2 - 128,
        y: height / 2 - 128,
        width: 256,
        height: 256,
    };
    for name in ["neutral", "velvia", "portra400"] {
        let label = format!("{} {name}", fixture.filename);
        cpu.set_parameters(stock(name));
        on_gpu.set_parameters(stock(name));

        let expected = cpu
            .render_region(rect, ScanMode::Exact, &mut NopControl)
            .unwrap();
        let actual = on_gpu
            .render_region(rect, ScanMode::Preview, &mut NopControl)
            .unwrap();
        // The GPU dye scan is always exact, so a GPU render reports it.
        assert_eq!(actual.fidelity(), Fidelity::Exact, "{label} crop");
        assert_rgb8_parity(
            &format!("{label} crop"),
            &expected.to_rgb8(),
            &actual.to_rgb8(),
        );

        // The CPU proxy of a spectral stock uses the dye lookup table, so
        // only an exact proxy is a reference.
        let expected = cpu.render_proxy(&mut NopControl).unwrap();
        let actual = on_gpu.render_proxy(&mut NopControl).unwrap();
        if expected.fidelity() == Fidelity::Exact {
            assert_rgb8_parity(
                &format!("{label} proxy"),
                &expected.to_rgb8(),
                &actual.to_rgb8(),
            );
        }
    }
}

fn gpu() -> Option<Gpu> {
    match Gpu::new_blocking() {
        Ok(gpu) => {
            eprintln!("gpu: {}", gpu.adapter_name());
            Some(gpu)
        }
        Err(err) if std::env::var_os("RAWSTOCK_REQUIRE_GPU").is_some() => {
            panic!("no GPU: {err}")
        }
        Err(err) => {
            eprintln!("skipping GPU fixture parity: {err}");
            None
        }
    }
}

fn stock(name: &str) -> rawstock_core::render::Parameters {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stocks")
        .join(format!("{name}.stock.toml"));
    Stock::load(path).unwrap().parameters().clone()
}

/// No code off by more than 1, and at most 0.1% of codes off by 1.
fn assert_rgb8_parity(label: &str, cpu: &[u8], gpu: &[u8]) {
    assert_eq!(cpu.len(), gpu.len(), "{label}");
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
