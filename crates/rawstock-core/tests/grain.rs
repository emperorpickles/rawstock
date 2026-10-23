//! Grain hash, density mean, resolution scaling, and halation energy.

use rawstock_core::render::NopControl;
use rawstock_core::stock::{Grain, GrainRegion, Granularity, Halation, ModelError};

fn flat(rms: f32) -> Granularity {
    Granularity::new([(0.0, rms), (4.0, rms)]).unwrap()
}

fn grain(frame_mm: f32, seed: u32, rms: f32) -> Grain {
    let layer = flat(rms);
    Grain::new(frame_mm, seed, [layer.clone(), layer.clone(), layer]).unwrap()
}

#[test]
fn grain_hash_is_deterministic() {
    // Locked so a WGSL port can match this exact u32. Inputs are
    // (x, y, layer, seed) = (3, 5, 1, 9).
    assert_eq!(Grain::hash(3, 5, 1, 9), 2_446_243_009);
    assert_eq!(Grain::hash(3, 5, 1, 9), Grain::hash(3, 5, 1, 9));
    assert_ne!(Grain::hash(3, 5, 1, 9), Grain::hash(4, 5, 1, 9));
    assert_ne!(Grain::hash(3, 5, 1, 9), Grain::hash(3, 6, 1, 9));
    assert_ne!(Grain::hash(3, 5, 1, 9), Grain::hash(3, 5, 2, 9));
    assert_ne!(Grain::hash(3, 5, 1, 9), Grain::hash(3, 5, 1, 10));

    let grain = grain(4.0, 9, 1000.0);
    let mut first = vec![1.25_f32; 64 * 48 * 3];
    let mut second = first.clone();
    grain.apply(&mut first, 64, 48);
    grain.apply(&mut second, 64, 48);
    assert_eq!(first, second);
}

#[test]
fn mean_density_is_preserved() {
    let grain = grain(4.0, 1, 1000.0);
    let width = 128_u32;
    let height = 96_u32;
    let density0 = 1.25_f32;
    let mut density = vec![density0; width as usize * height as usize * 3];
    grain.apply(&mut density, width, height);

    let (mean, std) = mean_std(&density);
    let samples = density.len() as f32;
    // Neighboring pixels share dye clouds, so the tolerance is several
    // standard errors of the pixel mean, which is the stricter figure.
    let stderr = std / samples.sqrt();
    assert!(
        (mean - density0).abs() < (8.0 * stderr).max(0.02),
        "mean {mean} density {density0} stderr {stderr}"
    );
    // rms 1000 is a 48 µm deviation of 1. A pixel reads the pixel ratio.
    let expected = grain.pixel_ratio(width)[1];
    assert!(
        (std / expected - 1.0).abs() < 0.08,
        "std {std} expected {expected}"
    );
}

#[test]
fn grain_statistics_scale_with_resolution() {
    // A 4 mm frame at 128 and 256 px: pixels of 31 and 16 µm.
    let small = grain(4.0, 3, 1000.0);
    let height = 96_u32;
    for width in [128_u32, 256] {
        let std = field_std(&small, width, height);
        let expect = small.pixel_ratio(width)[1];
        assert!(
            (std / expect - 1.0).abs() < 0.08,
            "width {width}: std {std} expected {expect}"
        );
        // The aperture is not a flat cell. Adjacent samples must not repeat.
        assert!(
            equal_neighbor_fraction(&small, width, height) < 0.01,
            "width {width} has flat runs"
        );
        assert_eq!(leading_run(&small, width, height), 1);
    }

    // Finer pixels see more grain, and the 48 µm figure sits in between:
    // a pixel of about the aperture reads close to rms.
    let ratio = |width: u32| small.pixel_ratio(width)[1];
    assert!(ratio(256) > ratio(128));
    let aperture_width = (4.0 / Grain::APERTURE_MM).round() as u32;
    assert!(
        (ratio(aperture_width) - 1.0).abs() < 0.15,
        "a 48 µm pixel reads {} of rms",
        ratio(aperture_width)
    );
    // Far coarser than the aperture, deviation falls nearly as the pixel
    // side; the coarsest clouds still straddle pixel edges.
    let full = grain(Grain::FRAME_MM, 5, 1000.0);
    let fall = full.pixel_ratio(160)[1] / full.pixel_ratio(80)[1];
    assert!(
        (1.75..2.05).contains(&fall),
        "deviation ratio {fall} for a 2x pixel"
    );
    let std_low = field_std(&full, 80, 60);
    let std_high = field_std(&full, 160, 60);
    assert!(
        (std_high / std_low / fall - 1.0).abs() < 0.06,
        "sub-pixel std ratio {} expected {fall}",
        std_high / std_low
    );
}

#[test]
fn a_preview_matches_the_export_shrunk_to_its_size() {
    // A 6 mm strip of a 35 mm frame: 1056 px at the full width, then a
    // quarter of that, each pixel the mean of a 4×4 block.
    for monochrome in [false, true] {
        let layer = flat(1000.0);
        let grain = if monochrome {
            Grain::monochrome(Grain::FRAME_MM, 7, layer, 0.006).unwrap()
        } else {
            Grain::new(Grain::FRAME_MM, 7, [layer.clone(), layer.clone(), layer]).unwrap()
        };
        let (frame, width, height, k) = (6160_u32, 1056_u32, 256_u32, 4_usize);
        let render = |frame_width: u32, width: u32, height: u32| {
            let mut density = vec![1.0_f32; width as usize * height as usize * 3];
            grain
                .apply_region(
                    &mut density,
                    GrainRegion {
                        width,
                        height,
                        origin_x: frame_width / 4,
                        origin_y: frame_width / 8,
                        frame_width,
                    },
                    &mut NopControl,
                )
                .unwrap();
            density
        };
        let full = render(frame, width, height);
        let (sw, sh) = (width as usize / k, height as usize / k);
        let mut shrunk = vec![0.0_f32; sw * sh * 3];
        for y in 0..sh {
            for x in 0..sw {
                for c in 0..3 {
                    let mut sum = 0.0;
                    for j in 0..k {
                        for i in 0..k {
                            sum += full[((y * k + j) * width as usize + x * k + i) * 3 + c];
                        }
                    }
                    shrunk[(y * sw + x) * 3 + c] = sum / (k * k) as f32;
                }
            }
        }
        let preview = render(frame / k as u32, sw as u32, sh as u32);
        let (_, std_shrunk) = mean_std(&shrunk);
        let (_, std_preview) = mean_std(&preview);
        assert!(
            (std_preview / std_shrunk - 1.0).abs() < 0.1,
            "monochrome {monochrome}: preview std {std_preview}, shrunk {std_shrunk}"
        );
        let lag_shrunk = mean_acf(&shrunk, sw as u32, 1);
        let lag_preview = mean_acf(&preview, sw as u32, 1);
        assert!(
            (lag_preview - lag_shrunk).abs() < 0.15,
            "monochrome {monochrome}: preview lag-1 {lag_preview}, shrunk {lag_shrunk}"
        );
    }
}

#[test]
fn silver_grain_is_skewed_toward_dense_specks() {
    let layer = flat(1000.0);
    let silver = Grain::monochrome(Grain::FRAME_MM, 9, layer.clone(), 0.006).unwrap();
    let dye = Grain::new(Grain::FRAME_MM, 9, [layer.clone(), layer.clone(), layer])
        .unwrap()
        .with_size_mm([0.006; 3])
        .unwrap();
    let skew = |grain: &Grain| {
        let mut density = vec![1.0_f32; 400 * 300 * 3];
        grain
            .apply_region(
                &mut density,
                GrainRegion {
                    width: 400,
                    height: 300,
                    origin_x: 2000,
                    origin_y: 1500,
                    frame_width: 6160,
                },
                &mut NopControl,
            )
            .unwrap();
        let green: Vec<f64> = density
            .iter()
            .skip(1)
            .step_by(3)
            .map(|v| f64::from(*v))
            .collect();
        let n = green.len() as f64;
        let mean = green.iter().sum::<f64>() / n;
        let var = green.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
        green.iter().map(|v| (v - mean).powi(3)).sum::<f64>() / n / var.powf(1.5)
    };
    let silver_skew = skew(&silver);
    let dye_skew = skew(&dye);
    assert!(silver_skew > 0.4, "silver skew {silver_skew}");
    assert!(dye_skew < 0.35, "dye skew {dye_skew}");
}

#[test]
fn zero_granularity_leaves_density_unchanged() {
    let silent = flat(0.0);
    let noisy = Granularity::new([(0.0, 0.0), (1.0, 1000.0)]).unwrap();
    let grain = Grain::new(4.0, 1, [noisy, silent.clone(), silent]).unwrap();
    let mut density = vec![0.0_f32; 32 * 24 * 3];
    // Green and blue sit at rms 0. Red is also rms 0 at density 0.
    let original = density.clone();
    grain.apply(&mut density, 32, 24);
    assert_eq!(density, original);

    for pixel in density.as_chunks_mut::<3>().0 {
        pixel[0] = 1.0;
    }
    grain.apply(&mut density, 32, 24);
    let mut red_moved = false;
    for pixel in density.as_chunks::<3>().0 {
        assert_eq!(pixel[1], 0.0);
        assert_eq!(pixel[2], 0.0);
        if pixel[0] != 1.0 {
            red_moved = true;
        }
    }
    assert!(red_moved, "red grain at rms 1000 did not move");
}

#[test]
fn halation_is_zero_below_the_threshold_and_energy_bounded() {
    let weights = [0.5_f32, 0.2, 0.05];
    let halation = Halation::new(1.0, 0.05, weights).unwrap();

    let mut below = vec![0.4_f32; 32 * 24 * 3];
    below[3] = 1.0; // exactly the threshold: still no excess
    let original = below.clone();
    halation.apply(&mut below, 32, 24);
    assert_eq!(below, original);

    // Uniform field above the threshold. The blur preserves a constant, so
    // the added energy is the excess times the weight sum.
    let mut flat = vec![2.0_f32; 32 * 24 * 3];
    halation.apply(&mut flat, 32, 24);
    let excess = 1.0_f32;
    let weight_sum = weights.iter().sum::<f32>();
    let mut added = 0.0_f32;
    for pixel in flat.as_chunks::<3>().0 {
        for channel in 0..3 {
            let delta = pixel[channel] - 2.0;
            assert!(delta >= -1.0e-4, "halation subtracted {delta}");
            assert!(
                delta <= weights[channel] * excess + 1.0e-3,
                "channel {channel} grew by {delta}"
            );
            added += delta;
        }
    }
    let pixels = 32.0 * 24.0;
    let expect = pixels * excess * weight_sum;
    assert!(
        (added - expect).abs() < expect * 1.0e-3,
        "uniform added {added}, expected {expect}"
    );

    // Inset highlight. Integral of a normalized blur stays within the excess.
    let mut spot = vec![0.2_f32; 48 * 48 * 3];
    for y in 20..28 {
        for x in 20..28 {
            let index = (y * 48 + x) * 3;
            spot[index] = 3.0;
            spot[index + 1] = 3.0;
            spot[index + 2] = 3.0;
        }
    }
    let halo = Halation::new(1.0, 2.0 / 48.0, weights).unwrap();
    let before = spot.clone();
    halo.apply(&mut spot, 48, 48);
    let mut excess_energy = 0.0_f32;
    let mut max_excess = 0.0_f32;
    for pixel in before.as_chunks::<3>().0 {
        let over = (pixel[0].max(pixel[1]).max(pixel[2]) - 1.0).max(0.0);
        excess_energy += over;
        max_excess = max_excess.max(over);
    }
    let mut added_energy = 0.0_f32;
    let mut neighbor = 0.0_f32;
    for y in 0..48 {
        for x in 0..48 {
            let index = (y * 48 + x) * 3;
            for channel in 0..3 {
                let delta = spot[index + channel] - before[index + channel];
                assert!(delta >= -1.0e-4, "subtracted at ({x},{y})");
                assert!(
                    delta <= weights[channel] * max_excess + 1.0e-3,
                    "channel {channel} at ({x},{y}) grew by {delta}, max excess {max_excess}"
                );
                added_energy += delta;
            }
            if (x, y) == (19, 24) {
                neighbor = spot[index] - before[index];
            }
        }
    }
    let bound = excess_energy * weight_sum;
    assert!(
        added_energy <= bound * 1.05 + 1.0e-3,
        "added {added_energy} bound {bound}"
    );
    assert!(
        added_energy >= bound * 0.9,
        "blur discarded energy: added {added_energy} bound {bound}"
    );
    assert!(neighbor > 0.0, "inset highlight did not spill");
    // Red weight is the largest, so the spill is red-heavy.
    let center = (24 * 48 + 24) * 3;
    let red = spot[center] - before[center];
    let green = spot[center + 1] - before[center + 1];
    let blue = spot[center + 2] - before[center + 2];
    assert!(red > green && green > blue, "{red} {green} {blue}");
}

#[test]
fn halation_radius_tracks_image_size() {
    let halation = Halation::new(1.0, 0.02, [1.0, 0.0, 0.0]).unwrap();
    assert!((halation.sigma_px(1000, 800) - 16.0).abs() < 1.0e-4);
    assert!((halation.sigma_px(2000, 1600) - 32.0).abs() < 1.0e-4);
}

#[test]
fn bad_grain_and_halation_parameters_are_rejected() {
    assert!(matches!(
        Granularity::new([]),
        Err(ModelError::EmptyGranularity)
    ));
    assert!(matches!(
        Granularity::new([(0.0, -1.0)]),
        Err(ModelError::GranularitySample)
    ));
    assert!(matches!(
        Halation::new(1.0, -0.1, [1.0, 0.0, 0.0]),
        Err(ModelError::HalationParameters)
    ));
    assert!(matches!(
        Halation::new(1.0, 0.1, [1.0, -0.2, 0.0]),
        Err(ModelError::HalationParameters)
    ));
    let layer = flat(1.0);
    assert!(matches!(
        Grain::new(0.0, 0, [layer.clone(), layer.clone(), layer.clone()]),
        Err(ModelError::FrameWidth)
    ));
    assert!(matches!(
        Grain::new(35.0, 0, [layer.clone(), layer.clone(), layer])
            .unwrap()
            .with_size_mm([0.02, 0.0, 0.01]),
        Err(ModelError::GrainSize)
    ));
}

#[test]
fn grain_has_no_grid_and_is_not_white() {
    // One millimetre across 256 pixels puts several pixels inside a dye cloud.
    let grain = grain(1.0, 11, 1000.0);
    let width = 256_u32;
    let height = 48_u32;
    let field = develop_field(&grain, width, height);
    assert!(
        equal_neighbor_fraction_of(&field, width) < 0.01,
        "exact neighbor copies"
    );
    let blocks = block_variance_fraction(&field, width, 8);
    assert!(blocks > 0.08, "8x8 blocks are flat ({blocks})");

    let low = mean_acf(&field, width, 1);
    let mid = mean_acf(&field, width, 8);
    assert!(low > 0.35, "lag-1 autocorrelation {low} is white");
    assert!(
        low > mid + 0.15,
        "lag-1 {low} did not fall by lag 8 ({mid})"
    );

    let (low_power, high_power) = row_spectrum_ratio(&field, width);
    assert!(
        low_power > high_power * 2.5,
        "low-frequency power {low_power} is not above high-frequency {high_power}"
    );

    let corr = channel_correlation(&field);
    assert!(
        corr > 0.85,
        "red/green correlation {corr} is not mostly shared luminance"
    );
}

#[test]
fn resolved_clouds_stay_soft_and_a_crop_matches() {
    // 35 mm across 6160 px is a 40 MP frame. Portra's dye clouds are then a
    // few pixels wide; a point-sampled kernel leaves hard pixel steps there.
    let layer = flat(1000.0);
    let grain = Grain::new(35.0, 400, [layer.clone(), layer.clone(), layer])
        .unwrap()
        .with_size_mm([0.070, 0.048, 0.034])
        .unwrap();
    let width = 200_u32;
    let height = 140_u32;
    let mut density = vec![1.0_f32; width as usize * height as usize * 3];
    let region = GrainRegion {
        width,
        height,
        origin_x: 1800,
        origin_y: 1200,
        frame_width: 6160,
    };
    grain
        .apply_region(&mut density, region, &mut NopControl)
        .unwrap();

    let rough = neighbor_roughness(&density, width);
    assert!(
        rough[0] < 0.155 && rough[1] < 0.22 && rough[2] < 0.27,
        "neighbor roughness {rough:?} is still a hard pixel step"
    );
    let std = channel_std(&density);
    let expect = grain.pixel_ratio(6160);
    for (channel, value) in std.iter().enumerate() {
        assert!(
            (value / expect[channel] - 1.0).abs() < 0.08,
            "channel {channel} std {value} expected {}",
            expect[channel]
        );
    }
    assert!(equal_neighbor_fraction_of(&density, width) < 0.01);

    let crop_w = 24_u32;
    let crop_h = 16_u32;
    let mut crop = vec![1.0_f32; crop_w as usize * crop_h as usize * 3];
    grain
        .apply_region(
            &mut crop,
            GrainRegion {
                width: crop_w,
                height: crop_h,
                origin_x: region.origin_x + 11,
                origin_y: region.origin_y + 7,
                frame_width: region.frame_width,
            },
            &mut NopControl,
        )
        .unwrap();
    for y in 0..crop_h as usize {
        for x in 0..crop_w as usize {
            for channel in 0..3 {
                let full_index = ((7 + y) * width as usize + (11 + x)) * 3 + channel;
                let crop_index = (y * crop_w as usize + x) * 3 + channel;
                assert_eq!(
                    crop[crop_index].to_bits(),
                    density[full_index].to_bits(),
                    "pixel {x},{y} channel {channel}"
                );
            }
        }
    }
}

#[test]
fn grain_variance_near_a_hard_edge_matches_the_flat_field() {
    // RMS peaks in the midtones, so the two sides of the step are different
    // and an edge halo would show up as extra variance on the seam.
    let curve = Granularity::new([(0.2, 250.0), (1.0, 1600.0), (2.4, 400.0)]).unwrap();
    let grain = Grain::new(35.0, 21, [curve.clone(), curve.clone(), curve]).unwrap();
    let width = 180_u32;
    let height = 240_u32;
    let region = GrainRegion {
        width,
        height,
        origin_x: 900,
        origin_y: 700,
        frame_width: 6160,
    };
    let left = 0.2_f32;
    let right = 1.0_f32;
    let mut edge = vec![0.0_f32; width as usize * height as usize * 3];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let density = if x < width as usize / 2 { left } else { right };
            let index = (y * width as usize + x) * 3;
            edge[index] = density;
            edge[index + 1] = density;
            edge[index + 2] = density;
        }
    }
    let clean = edge.clone();
    grain
        .apply_region(&mut edge, region, &mut NopControl)
        .unwrap();

    let left_flat = flat_residual_std(&grain, region, left);
    let right_flat = flat_residual_std(&grain, region, right);
    let left_band = band_residual_std(&edge, &clean, width, width as usize / 2 - 28, 20);
    let right_band = band_residual_std(&edge, &clean, width, width as usize / 2 + 4, 20);
    assert!(
        (left_band / left_flat - 1.0).abs() < 0.12,
        "left of edge {left_band} flat {left_flat}"
    );
    assert!(
        (right_band / right_flat - 1.0).abs() < 0.12,
        "right of edge {right_band} flat {right_flat}"
    );
    let seam = band_residual_std(&edge, &clean, width, width as usize / 2 - 1, 2);
    let cap = left_flat.max(right_flat) * 1.15;
    assert!(
        seam <= cap,
        "seam variance {seam} exceeds flat-field cap {cap}"
    );
}

#[test]
fn chroma_grain_is_a_fraction_of_luma_grain() {
    let grain = grain(35.0, 21, 1000.0);
    let width = 280_u32;
    let height = 160_u32;
    let mut density = vec![1.0_f32; width as usize * height as usize * 3];
    grain
        .apply_region(
            &mut density,
            GrainRegion {
                width,
                height,
                origin_x: 1400,
                origin_y: 900,
                frame_width: 6160,
            },
            &mut NopControl,
        )
        .unwrap();
    let mut luma = Vec::new();
    let mut chroma = Vec::new();
    for pixel in density.as_chunks::<3>().0 {
        let red = pixel[0] - 1.0;
        let green = pixel[1] - 1.0;
        let blue = pixel[2] - 1.0;
        luma.push(0.2126 * red + 0.7152 * green + 0.0722 * blue);
        chroma.push(red - green);
    }
    let luma_std = mean_std(&luma).1;
    let chroma_std = mean_std(&chroma).1;
    let ratio = chroma_std / luma_std;
    assert!(
        (0.12..0.50).contains(&ratio),
        "chroma/luma {ratio} (chroma {chroma_std}, luma {luma_std})"
    );
}

#[test]
fn grain_spectrum_has_no_grid_peak() {
    let grain = grain(35.0, 8, 1000.0);
    let width = 384_u32;
    let height = 48_u32;
    let mut density = vec![1.0_f32; width as usize * height as usize * 3];
    grain
        .apply_region(
            &mut density,
            GrainRegion {
                width,
                height,
                origin_x: 2000,
                origin_y: 1200,
                frame_width: 6160,
            },
            &mut NopControl,
        )
        .unwrap();
    // A smooth spectrum decays, so the loudest bin in a wide band is not a
    // grid. A lattice spike stands far above the bins next to it.
    let peak = narrow_spectral_peak(&density, width);
    assert!(
        peak < 14.0,
        "narrow periodogram peak {peak} looks like a grid"
    );
    for lag in 2..24 {
        let corr = mean_acf(&density, width, lag);
        assert!(
            corr > -0.20,
            "lag {lag} autocorrelation {corr} oscillates like a grid"
        );
    }
}

#[test]
fn dye_cloud_size_scales_with_the_frame() {
    let grain = grain(1.0, 13, 1000.0);
    let coarse = develop_field(&grain, 128, 32);
    let fine = develop_field(&grain, 256, 32);
    let lag_coarse = half_correlation_lag(&coarse, 128);
    let lag_fine = half_correlation_lag(&fine, 256);
    let ratio = lag_fine as f32 / lag_coarse as f32;
    assert!(
        (ratio - 2.0).abs() < 0.8,
        "correlation lag {lag_coarse} -> {lag_fine}, ratio {ratio}"
    );
}

fn field_std(grain: &Grain, width: u32, height: u32) -> f32 {
    let mut density = vec![1.0_f32; width as usize * height as usize * 3];
    grain.apply(&mut density, width, height);
    mean_std(&density).1
}

fn develop_field(grain: &Grain, width: u32, height: u32) -> Vec<f32> {
    let mut density = vec![1.0_f32; width as usize * height as usize * 3];
    grain.apply(&mut density, width, height);
    density
}

fn leading_run(grain: &Grain, width: u32, height: u32) -> usize {
    let density = develop_field(grain, width, height);
    let first = density[0];
    let mut run = 1;
    while run < width as usize && density[run * 3] == first {
        run += 1;
    }
    run
}

fn equal_neighbor_fraction(grain: &Grain, width: u32, height: u32) -> f32 {
    equal_neighbor_fraction_of(&develop_field(grain, width, height), width)
}

fn equal_neighbor_fraction_of(density: &[f32], width: u32) -> f32 {
    let width = width as usize;
    let pixels = density.len() / 3;
    let height = pixels / width;
    let mut equal = 0.0_f32;
    let mut total = 0.0_f32;
    for y in 0..height {
        for x in 0..width {
            let index = (y * width + x) * 3;
            if x + 1 < width {
                total += 1.0;
                if density[index] == density[index + 3] {
                    equal += 1.0;
                }
            }
            if y + 1 < height {
                total += 1.0;
                let below = index + width * 3;
                if density[index] == density[below] {
                    equal += 1.0;
                }
            }
        }
    }
    equal / total
}

fn block_variance_fraction(density: &[f32], width: u32, block: usize) -> f32 {
    let width = width as usize;
    let pixels = density.len() / 3;
    let height = pixels / width;
    let mut within = 0.0_f64;
    let mut blocks = 0.0_f64;
    let mut y = 0;
    while y + block <= height {
        let mut x = 0;
        while x + block <= width {
            let mut sum = 0.0_f64;
            let mut count = 0.0_f64;
            for dy in 0..block {
                for dx in 0..block {
                    sum += f64::from(density[((y + dy) * width + (x + dx)) * 3]);
                    count += 1.0;
                }
            }
            let mean = sum / count;
            let mut var = 0.0_f64;
            for dy in 0..block {
                for dx in 0..block {
                    let delta = f64::from(density[((y + dy) * width + (x + dx)) * 3]) - mean;
                    var += delta * delta;
                }
            }
            within += var / count;
            blocks += 1.0;
            x += block;
        }
        y += block;
    }
    let channel: Vec<f32> = density.iter().step_by(3).copied().collect();
    let total = f64::from(mean_std(&channel).1).powi(2);
    (within / blocks) as f32 / total as f32
}

fn mean_acf(density: &[f32], width: u32, lag: usize) -> f32 {
    let width = width as usize;
    let height = density.len() / 3 / width;
    let mut acc = 0.0_f64;
    for y in 0..height {
        let row: Vec<f32> = (0..width).map(|x| density[(y * width + x) * 3]).collect();
        acc += f64::from(autocorrelation(&row, lag));
    }
    (acc / height as f64) as f32
}

fn autocorrelation(samples: &[f32], lag: usize) -> f32 {
    let (mean, std) = mean_std(samples);
    if std == 0.0 || lag >= samples.len() {
        return 0.0;
    }
    let mut cov = 0.0_f64;
    let pairs = samples.len() - lag;
    for i in 0..pairs {
        cov += (f64::from(samples[i]) - f64::from(mean))
            * (f64::from(samples[i + lag]) - f64::from(mean));
    }
    (cov / pairs as f64) as f32 / (std * std)
}

fn half_correlation_lag(density: &[f32], width: u32) -> usize {
    let limit = (width as usize / 4).max(2);
    for lag in 1..limit {
        if mean_acf(density, width, lag) < 0.5 {
            return lag;
        }
    }
    limit
}

fn row_spectrum_ratio(density: &[f32], width: u32) -> (f32, f32) {
    let width = width as usize;
    let row: Vec<f64> = (0..width).map(|x| f64::from(density[x * 3])).collect();
    let mean = row.iter().sum::<f64>() / row.len() as f64;
    let n = row.len();
    let mut low = 0.0_f64;
    let mut high = 0.0_f64;
    let mut low_n = 0.0_f64;
    let mut high_n = 0.0_f64;
    for k in 1..n / 2 {
        let mut re = 0.0_f64;
        let mut im = 0.0_f64;
        for (t, sample) in row.iter().enumerate() {
            let angle = -std::f64::consts::TAU * k as f64 * t as f64 / n as f64;
            re += (sample - mean) * angle.cos();
            im += (sample - mean) * angle.sin();
        }
        let power = re * re + im * im;
        if k <= n / 16 {
            low += power;
            low_n += 1.0;
        }
        if k >= n / 4 {
            high += power;
            high_n += 1.0;
        }
    }
    ((low / low_n) as f32, (high / high_n) as f32)
}

fn channel_correlation(density: &[f32]) -> f32 {
    let mut red = Vec::with_capacity(density.len() / 3);
    let mut green = Vec::with_capacity(density.len() / 3);
    for pixel in density.as_chunks::<3>().0 {
        red.push(pixel[0]);
        green.push(pixel[1]);
    }
    let (red_mean, red_std) = mean_std(&red);
    let (green_mean, green_std) = mean_std(&green);
    let mut cov = 0.0_f64;
    for (r, g) in red.iter().zip(green.iter()) {
        cov += (f64::from(*r) - f64::from(red_mean)) * (f64::from(*g) - f64::from(green_mean));
    }
    (cov / red.len() as f64) as f32 / (red_std * green_std)
}

fn neighbor_roughness(density: &[f32], width: u32) -> [f32; 3] {
    let width = width as usize;
    let height = density.len() / 3 / width;
    let mut acc = [0.0_f64; 3];
    let mut count = [0.0_f64; 3];
    for y in 0..height {
        for x in 0..width {
            let index = (y * width + x) * 3;
            if x + 1 < width {
                for channel in 0..3 {
                    acc[channel] +=
                        f64::from((density[index + channel] - density[index + 3 + channel]).abs());
                    count[channel] += 1.0;
                }
            }
            if y + 1 < height {
                let below = index + width * 3;
                for channel in 0..3 {
                    acc[channel] +=
                        f64::from((density[index + channel] - density[below + channel]).abs());
                    count[channel] += 1.0;
                }
            }
        }
    }
    let std = channel_std(density);
    [0, 1, 2].map(|channel| (acc[channel] / count[channel]) as f32 / std[channel])
}

fn channel_std(density: &[f32]) -> [f32; 3] {
    let mut columns = [Vec::new(), Vec::new(), Vec::new()];
    for (index, sample) in density.iter().enumerate() {
        columns[index % 3].push(*sample);
    }
    columns.map(|column| mean_std(&column).1)
}

fn flat_residual_std(grain: &Grain, region: GrainRegion, density: f32) -> f32 {
    let mut field = vec![density; region.width as usize * region.height as usize * 3];
    grain
        .apply_region(&mut field, region, &mut NopControl)
        .unwrap();
    let mut acc = 0.0_f64;
    for sample in &field {
        let delta = f64::from(*sample - density);
        acc += delta * delta;
    }
    (acc / field.len() as f64).sqrt() as f32
}

fn band_residual_std(grained: &[f32], clean: &[f32], width: u32, x0: usize, cols: usize) -> f32 {
    let width = width as usize;
    let height = grained.len() / 3 / width;
    let mut acc = 0.0_f64;
    let mut count = 0.0_f64;
    for y in 0..height {
        for x in x0..x0 + cols {
            for channel in 0..3 {
                let index = (y * width + x) * 3 + channel;
                let delta = f64::from(grained[index] - clean[index]);
                acc += delta * delta;
                count += 1.0;
            }
        }
    }
    (acc / count).sqrt() as f32
}

fn narrow_spectral_peak(density: &[f32], width: u32) -> f32 {
    let width = width as usize;
    let y = density.len() / 3 / width / 2;
    let row: Vec<f64> = (0..width)
        .map(|x| f64::from(density[(y * width + x) * 3]))
        .collect();
    let mean = row.iter().sum::<f64>() / row.len() as f64;
    let n = row.len();
    let mut powers = Vec::new();
    for k in 1..n / 2 {
        let mut re = 0.0;
        let mut im = 0.0;
        for (t, sample) in row.iter().enumerate() {
            let angle = -std::f64::consts::TAU * k as f64 * t as f64 / n as f64;
            re += (sample - mean) * angle.cos();
            im += (sample - mean) * angle.sin();
        }
        powers.push(re * re + im * im);
    }
    let start = n / 10;
    let mut worst = 1.0_f64;
    for index in start..powers.len() {
        let mut neighbors = Vec::new();
        let lo = index.saturating_sub(4);
        let hi = (index + 4).min(powers.len() - 1);
        for (other, power) in powers.iter().enumerate().take(hi + 1).skip(lo) {
            if other != index && other >= start {
                neighbors.push(*power);
            }
        }
        if neighbors.is_empty() {
            continue;
        }
        neighbors.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = neighbors[neighbors.len() / 2];
        if median > 0.0 {
            worst = worst.max(powers[index] / median);
        }
    }
    worst as f32
}

fn mean_std(samples: &[f32]) -> (f32, f32) {
    let n = samples.len() as f64;
    let mean = samples.iter().map(|sample| f64::from(*sample)).sum::<f64>() / n;
    let variance = samples
        .iter()
        .map(|sample| {
            let delta = f64::from(*sample) - mean;
            delta * delta
        })
        .sum::<f64>()
        / n;
    (mean as f32, variance.sqrt() as f32)
}
