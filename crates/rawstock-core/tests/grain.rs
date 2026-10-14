//! Grain hash, density mean, resolution scaling, and halation energy.

use rawstock_core::stock::{Grain, Granularity, Halation, ModelError};

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
    let expected = 1000.0 / 1000.0;
    assert!(
        (std - expected).abs() < 0.08,
        "std {std} expected {expected}"
    );
}

#[test]
fn grain_statistics_scale_with_resolution() {
    // frame_mm = 4 makes the 48 µm aperture 1.536 px at width 128 and 3.072 px
    // at width 256, so both images are in the "aperture covers a pixel" regime.
    let narrow = grain(4.0, 3, 1000.0);
    let wide_width = 256_u32;
    let narrow_width = 128_u32;
    assert!(
        (narrow.cell_px(wide_width) / narrow.cell_px(narrow_width) - 2.0).abs() < 1.0e-5,
        "cell size did not double with width"
    );
    assert!(
        (narrow.cell_px(narrow_width) - Grain::APERTURE_MM / 4.0 * narrow_width as f32).abs()
            < 1.0e-4
    );

    let height = 96_u32;
    let std_narrow = field_std(&narrow, narrow_width, height);
    let std_wide = field_std(&narrow, wide_width, height);
    // Specified: above one pixel per aperture, density std stays at rms/1000.
    assert!((std_narrow - 1.0).abs() < 0.08, "narrow std {std_narrow}");
    assert!((std_wide - 1.0).abs() < 0.08, "wide std {std_wide}");
    assert!(
        (std_wide / std_narrow - 1.0).abs() < 0.08,
        "std ratio {} was not invariant",
        std_wide / std_narrow
    );

    // The aperture is not a flat cell. Adjacent samples must not repeat.
    assert!(
        equal_neighbor_fraction(&narrow, narrow_width, height) < 0.01,
        "narrow frame still has flat runs"
    );
    assert!(
        equal_neighbor_fraction(&narrow, wide_width, height) < 0.01,
        "wide frame still has flat runs"
    );
    assert_eq!(leading_run(&narrow, narrow_width, height), 1);
    assert_eq!(leading_run(&narrow, wide_width, height), 1);

    // Coarser than the aperture: one pixel covers several cells, and the
    // density std scales with sqrt(cell_px), which is sqrt(width).
    let fine_frame = grain(Grain::FRAME_MM, 5, 1000.0);
    let low = 80_u32;
    let high = 160_u32;
    assert!(fine_frame.cell_px(high) < 1.0);
    let std_low = field_std(&fine_frame, low, 60);
    let std_high = field_std(&fine_frame, high, 60);
    let expect = (fine_frame.cell_px(high) / fine_frame.cell_px(low)).sqrt();
    assert!(
        (std_high / std_low - expect).abs() < 0.06,
        "sub-pixel std ratio {} expected {expect}",
        std_high / std_low
    );
    assert!(
        (std_low - fine_frame.cell_px(low).sqrt()).abs() < 0.05,
        "sub-pixel std {std_low}"
    );
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
    assert!(
        block_variance_fraction(&field, width, 8) > 0.25,
        "8x8 blocks are flat"
    );

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
        corr > 0.15 && corr < 0.85,
        "red/green correlation {corr} is not partial"
    );
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
