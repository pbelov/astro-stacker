//! The sky each photosite sits on, and the noise around it.
//!
//! Two things force the shape of this.
//!
//! **The sky is not flat.** Over a three-degree field, light pollution, moonlight
//! and any residual vignetting give a gradient. One global median would put the
//! detection threshold too high on one side of the frame and too low on the
//! other, so the background is estimated on a coarse grid and interpolated.
//!
//! **The sky is not the same for every colour.** On a mosaic the red, green and
//! blue photosites sit at different levels — measured on this project's
//! reference body, a star's flux divides roughly 1 : 0.73 : 0.67 between green,
//! red and blue — so a background taken over all photosites together would be a
//! checkerboard, and a threshold against it would find the green photosites of
//! the sky before it found a faint star. Every estimate here is per
//! colour-filter index.

use astro_plugin_abi::abi::ImageLayout;

use crate::session::mosaic::Mosaic;

/// How wide a background cell is, in mosaic photosites.
///
/// Wide enough that a cell holds thousands of samples of each colour and a
/// bright star cannot move its median; narrow enough that a gradient across a
/// three-degree field is followed rather than averaged. On the reference sensor
/// this makes a 41 by 28 grid, each cell holding about 4000 photosites of each
/// colour.
pub const CELL: usize = 128;

/// A per-colour background and noise map over a coarse grid.
#[derive(Debug, Clone)]
pub struct Sky {
    pub cells_x: usize,
    pub cells_y: usize,
    colours: usize,
    /// `[cell_y][cell_x][colour]`, flattened. NaN where a cell held no samples
    /// of that colour, which never happens on a real mosaic but is not worth
    /// inventing a number for.
    level: Vec<f32>,
    sigma: Vec<f32>,
}

impl Sky {
    fn index(&self, cell_x: usize, cell_y: usize, colour: usize) -> usize {
        (cell_y * self.cells_x + cell_x) * self.colours + colour
    }

    /// The sky at a photosite, bilinear between the four nearest cell centres.
    pub fn level_at(&self, x: usize, y: usize, colour: usize) -> f32 {
        self.sample(&self.level, x, y, colour)
    }

    /// The noise at a photosite, interpolated the same way.
    pub fn sigma_at(&self, x: usize, y: usize, colour: usize) -> f32 {
        self.sample(&self.sigma, x, y, colour)
    }

    fn sample(&self, grid: &[f32], x: usize, y: usize, colour: usize) -> f32 {
        if colour >= self.colours || self.cells_x == 0 || self.cells_y == 0 {
            return f32::NAN;
        }
        // Cell centres sit at (i + 0.5) * CELL, so a photosite between them
        // interpolates; outside the outermost centres it clamps, which is what
        // keeps the corners from extrapolating a gradient into nonsense.
        let fx = (x as f32 / CELL as f32 - 0.5).clamp(0.0, (self.cells_x - 1) as f32);
        let fy = (y as f32 / CELL as f32 - 0.5).clamp(0.0, (self.cells_y - 1) as f32);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.cells_x - 1), (y0 + 1).min(self.cells_y - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);

        let at = |cx, cy| grid[self.index(cx, cy, colour)];
        let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
        let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
        top * (1.0 - ty) + bottom * ty
    }
}

/// Measures the sky and its noise over the active area.
///
/// The level is the median and the noise is the median absolute deviation
/// scaled to a Gaussian sigma. Both are robust for the same reason: a cell of a
/// star field is mostly sky with a few per cent of stars, and a mean or a
/// standard deviation would be pulled up by them — which is exactly backwards,
/// because a threshold inflated by the stars finds fewer of them.
pub fn measure(pixels: &[f32], layout: &ImageLayout, mosaic: &Mosaic) -> Option<Sky> {
    let width = layout.width as usize;
    let area = mosaic.area;
    let colours = mosaic.colours;

    let cells_x = area.width.div_ceil(CELL).max(1);
    let cells_y = area.height.div_ceil(CELL).max(1);
    let mut level = vec![f32::NAN; cells_x * cells_y * colours];
    let mut sigma = vec![f32::NAN; cells_x * cells_y * colours];

    // One reusable scratch per colour, so a 41 by 28 grid does not allocate
    // eleven hundred vectors.
    let mut samples: Vec<Vec<f32>> = vec![Vec::with_capacity(CELL * CELL / colours.max(1)); colours];

    for cell_y in 0..cells_y {
        let y0 = area.y + cell_y * CELL;
        let y1 = (y0 + CELL).min(area.y + area.height);
        for cell_x in 0..cells_x {
            let x0 = area.x + cell_x * CELL;
            let x1 = (x0 + CELL).min(area.x + area.width);

            for bucket in &mut samples {
                bucket.clear();
            }
            for y in y0..y1 {
                let row = y * width;
                for x in x0..x1 {
                    let colour = mosaic.colour_at(x, y);
                    if colour < colours {
                        let value = pixels[row + x];
                        if value.is_finite() {
                            samples[colour].push(value);
                        }
                    }
                }
            }

            for (colour, bucket) in samples.iter_mut().enumerate() {
                if bucket.is_empty() {
                    continue;
                }
                let index = (cell_y * cells_x + cell_x) * colours + colour;
                let median = median_of(bucket);
                level[index] = median;
                // MAD, scaled so that it estimates the same thing a standard
                // deviation would on Gaussian noise.
                for value in bucket.iter_mut() {
                    *value = (*value - median).abs();
                }
                sigma[index] = median_of(bucket) * 1.4826;
            }
        }
    }

    // A cell whose noise came out zero has more than half its samples
    // identical - dead, saturated, or synthetic - and a threshold in units of it
    // would divide by nothing. It borrows the frame's own median noise, which is
    // still a measurement rather than a constant. A frame with no measurable
    // noise anywhere has no scale to threshold against at all, and says so
    // instead of inventing one.
    let mut usable: Vec<f32> = sigma.iter().copied().filter(|value| *value > 0.0).collect();
    if usable.is_empty() {
        return None;
    }
    let fallback = median_of(&mut usable);
    for value in &mut sigma {
        if value.is_nan() || *value <= 0.0 {
            *value = fallback;
        }
    }

    Some(Sky { cells_x, cells_y, colours, level, sigma })
}

/// Median in place, by selection rather than a full sort.
fn median_of(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return f32::NAN;
    }
    let middle = values.len() / 2;
    let (_, value, _) = values.select_nth_unstable_by(middle, f32::total_cmp);
    *value
}

#[cfg(test)]
mod tests {
    use super::*;
    use astro_plugin_abi::abi::{COLOR_BLUE, COLOR_GREEN, COLOR_RED};

    /// Deterministic noise, because a threshold measured in units of the noise
    /// needs there to be some. Three uniforms summed is near enough Gaussian.
    fn noise(sigma: f32) -> impl FnMut() -> f32 {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        move || {
            let mut total = 0f32;
            for _ in 0..3 {
                state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                total += (state >> 40) as f32 / 16_777_216.0 - 0.5;
            }
            total * sigma * 2.0
        }
    }

    fn layout(width: usize, height: usize) -> ImageLayout {
        let mut layout = ImageLayout {
            width: width as u32,
            height: height as u32,
            components: 1,
            cfa_width: 2,
            cfa_height: 2,
            active_width: width as u32,
            active_height: height as u32,
            ..Default::default()
        };
        layout.cfa_pattern[..4].copy_from_slice(&[COLOR_RED, COLOR_GREEN, COLOR_GREEN, COLOR_BLUE]);
        layout
    }

    #[test]
    fn each_colour_gets_its_own_sky() {
        // The failure this prevents: a background taken over all photosites at
        // once is a checkerboard on a mosaic, and a threshold against it finds
        // the green sky before it finds a faint star.
        let (width, height) = (256, 256);
        let layout = layout(width, height);
        let mosaic = Mosaic::new(&layout).unwrap();
        let mut noise = noise(10.0);
        let mut pixels = vec![0f32; width * height];
        for y in 0..height {
            for x in 0..width {
                let level = match (y % 2, x % 2) {
                    (0, 0) => 300.0,
                    (1, 1) => 200.0,
                    _ => 400.0,
                };
                pixels[y * width + x] = level + noise();
            }
        }

        let sky = measure(&pixels, &layout, &mosaic).unwrap();
        assert!((sky.level_at(10, 10, 0) - 300.0).abs() < 2.0, "red");
        assert!((sky.level_at(10, 10, 1) - 400.0).abs() < 2.0, "green");
        assert!((sky.level_at(10, 10, 2) - 200.0).abs() < 2.0, "blue");
    }

    #[test]
    fn a_frame_with_no_measurable_noise_has_no_scale_to_threshold_against() {
        // What a frame of one repeated value looks like. A detection threshold
        // is expressed in units of the noise, so with no noise there is nothing
        // to express it in - and saying so beats inventing a sigma that would
        // make every photosite either invisible or a star.
        let layout = layout(256, 256);
        let mosaic = Mosaic::new(&layout).unwrap();
        assert!(measure(&vec![500f32; 256 * 256], &layout, &mosaic).is_none());
    }

    #[test]
    fn a_dead_cell_borrows_the_frame_noise_rather_than_dividing_by_nothing() {
        // A saturated or dead corner has zero spread of its own. Left at zero it
        // would make the whole cell either infinitely significant or invisible,
        // depending on which way the division fell.
        let (width, height) = (512, 256);
        let layout = layout(width, height);
        let mosaic = Mosaic::new(&layout).unwrap();
        let mut noise = noise(25.0);
        let mut pixels: Vec<f32> = (0..width * height).map(|_| 500.0 + noise()).collect();
        for y in 0..128 {
            for x in 0..128 {
                pixels[y * width + x] = 16_383.0;
            }
        }

        let sky = measure(&pixels, &layout, &mosaic).unwrap();
        let dead = sky.sigma_at(20, 20, 1);
        assert!(dead > 0.0 && dead.is_finite(), "the dead cell has a usable sigma: {dead}");
    }

    #[test]
    fn a_gradient_is_followed_rather_than_averaged() {
        // One global median would put the threshold too high on one side of a
        // three-degree field and too low on the other.
        let (width, height) = (512, 256);
        let layout = layout(width, height);
        let mosaic = Mosaic::new(&layout).unwrap();
        let mut pixels = vec![0f32; width * height];
        let mut noise = noise(5.0);
        for y in 0..height {
            for x in 0..width {
                pixels[y * width + x] = 100.0 + x as f32 + noise();
            }
        }

        let sky = measure(&pixels, &layout, &mosaic).unwrap();
        let left = sky.level_at(20, 128, 1);
        let right = sky.level_at(width - 20, 128, 1);
        // The outermost cell centres sit at 64 and 448 over a 512-wide frame,
        // and a sample beyond them clamps rather than extrapolating, so the
        // span that can be recovered is the distance between those centres.
        assert!(right - left > 350.0, "the gradient must survive: {left} to {right}");
        // And it interpolates smoothly rather than stepping at cell edges.
        let middle = sky.level_at(width / 2, 128, 1);
        assert!((middle - (left + right) / 2.0).abs() < 40.0, "got {middle}");
    }

    #[test]
    fn a_star_field_does_not_lift_the_sky() {
        // A cell is mostly sky with a few per cent of stars. A mean would be
        // pulled up by them, and a threshold inflated by the stars finds fewer
        // of them - exactly backwards.
        let (width, height) = (256, 256);
        let layout = layout(width, height);
        let mosaic = Mosaic::new(&layout).unwrap();
        let mut noise = noise(20.0);
        let mut pixels: Vec<f32> = (0..width * height).map(|_| 500.0 + noise()).collect();
        // Three per cent of the frame at a hundred times the sky.
        for index in (0..width * height).step_by(33) {
            pixels[index] = 50_000.0;
        }

        let sky = measure(&pixels, &layout, &mosaic).unwrap();
        assert!((sky.level_at(128, 128, 1) - 500.0).abs() < 5.0);
        // And the noise estimate is not inflated either. A standard deviation
        // over these samples is about 8600; the MAD sees the 20 that is really
        // there, and a threshold in units of it finds stars rather than hiding
        // them behind the brightest ones.
        let sigma = sky.sigma_at(128, 128, 1);
        assert!((15.0..30.0).contains(&sigma), "the stars must not inflate it: {sigma}");
    }
}
