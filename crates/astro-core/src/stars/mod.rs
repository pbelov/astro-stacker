//! Finding stars, and measuring what the mount did to them.
//!
//! Detection runs on the **raw mosaic at full resolution**, with every photosite
//! expressed as signal above its own colour's local sky, divided by its own
//! colour's local noise. Nothing is binned, demosaiced or reduced to green, and
//! that is a measured decision rather than a taste.
//!
//! The obvious worry about detecting on a mosaic is that a small star landing on
//! a red photosite and the same star landing on a green one would be found in
//! different places. It turns out not to be so. The colour modulation of a Bayer
//! pattern is a signal at exactly the Nyquist frequency, and when a source is
//! centred on a photosite the modulated part is even about that centre and
//! contributes nothing to the first moment; at intermediate sub-pixel phases the
//! bias is real but small. Measured on 763 stars matched between two consecutive
//! frames of this project's reference session, the mosaic's own contribution to
//! the centroid is 0.005 to 0.018 photosites, while ordinary aperture truncation
//! — which has nothing to do with colour — is two to five times larger.
//!
//! Binning two by two was measured against it and is worse, not better: it turns
//! a 0.015 px problem into a 0.08 to 0.18 px one, because the cross-trail width
//! of a star on this rig is about 1.9 photosites, and binning pushes that
//! well-sampled axis below Nyquist.

pub mod shape;
pub mod sky;

use astro_plugin_abi::abi::ImageLayout;

use crate::session::mosaic::Mosaic;

pub use shape::{FrameShape, Moments};
pub use sky::Sky;

/// One source, in full-resolution mosaic photosites from the frame origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Star {
    pub x: f64,
    pub y: f64,
    /// Total flux above sky, in ADU, and independent of how blurred the source
    /// is.
    ///
    /// Twice the sum under the fitted window, which is not an approximation but
    /// an identity. For a Gaussian source of covariance `C` under a Gaussian
    /// window of covariance `W`, the weighted sum is
    /// `F * sqrt(det[(C^-1 + W^-1)^-1] / det C)`; the window fit converges to
    /// `W = C`, where `(C^-1 + C^-1)^-1 = C/2`, whose determinant is a quarter
    /// of `det C`, so the sum is exactly `F/2` whatever `C` was.
    ///
    /// That independence is the point. The obvious flux — the sum over the
    /// threshold footprint — is not a property of the star at all: the footprint
    /// grows with the blur while the wings it fails to reach grow too, so a
    /// trailed frame reads fainter at unchanged transparency. Anything that
    /// compares one frame's brightness to another's, which is what a stack must
    /// do before it combines them, would then read the trailing as cloud.
    pub flux: f64,
    /// How far above the noise this source was detected, in sigma, on the
    /// filtered plane.
    ///
    /// This and not `flux` is what ranks stars. `flux` is summed over a
    /// threshold footprint, so it grows with the footprint's area as much as
    /// with the source's brightness, and the largest footprints belong to merged
    /// pairs and to whatever the sky estimate left behind. Ranking by it puts
    /// the least star-like sources at the top of the list, and since matching
    /// takes the top of the list, two frames of the same field then propose
    /// different objects to each other and register against nothing.
    pub significance: f32,
    pub moments: Moments,
    /// How many photosites the footprint held, which is the honest measure of
    /// how much the moments rest on.
    pub footprint: usize,
}

impl Star {
    /// For tests and for callers that already have the shape.
    pub fn at(x: f64, y: f64, moments: Moments) -> Self {
        Self { x, y, flux: f64::NAN, significance: f32::NAN, moments, footprint: 0 }
    }

    pub fn orientation(&self) -> Option<(f64, f64)> {
        self.moments.orientation()
    }

    pub fn ellipticity(&self) -> f64 {
        self.moments.ellipticity()
    }
}

/// Everything one frame's detection produced.
#[derive(Debug, Clone)]
pub struct Detection {
    pub stars: Vec<Star>,
    pub shape: FrameShape,
    /// Sources dropped because a photosite in them was at or above the sensor's
    /// saturation point. Counted rather than silently kept: a flat-topped core
    /// has no centroid worth the name, and on this project's reference session
    /// about a tenth of a per cent of every frame's green photosites are there.
    pub saturated: usize,
    /// Sources dropped because their footprint ran past the cap, which is what a
    /// satellite trail or a badly deblended pair looks like.
    pub oversized: usize,
    /// The background this detection was measured against.
    ///
    /// Kept because stacking needs the same background the stars were found
    /// against, and measuring it twice is both a minute of decoding and a
    /// chance for the two answers to differ.
    pub sky: Sky,
}

#[derive(Debug, Clone, Copy)]
pub struct DetectOptions {
    /// How many times the filtered noise a peak must stand above the sky.
    pub detect_sigma: f32,
    /// The lower threshold a photosite must clear to join a source already
    /// found. Lower than the detection threshold, so a star's wings are measured
    /// rather than clipped.
    pub grow_sigma: f32,
    /// Width of the matched filter, in photosites. Close to the cross-trail
    /// width of a real star, which is what makes it a matched filter rather
    /// than a blur.
    pub filter_sigma: f32,
    /// Largest footprint worth measuring. Above it the source is a satellite, an
    /// aeroplane or two stars that should have been deblended, and its moments
    /// describe none of those usefully.
    pub max_footprint: usize,
    /// Smallest footprint worth measuring. Below it the source is one hot
    /// photosite the dark did not remove.
    pub min_footprint: usize,
    /// Stop after this many, brightest first. A cap rather than a target: the
    /// matching that comes later wants hundreds, not tens of thousands.
    pub max_stars: usize,
}

impl Default for DetectOptions {
    fn default() -> Self {
        Self {
            detect_sigma: 5.0,
            grow_sigma: 2.5,
            filter_sigma: 1.6,
            max_footprint: 400,
            min_footprint: 4,
            max_stars: 1000,
        }
    }
}

/// Finds the stars in one decoded frame.
///
/// `pixels` is the frame to measure — calibrated or not — and `raw` is what the
/// sensor read, used only for the saturation test. They must be the same length.
/// The saturation test has to see the raw samples: the white level is a
/// raw-sensor constant, and comparing it against a pedestal-removed plane would
/// mean the test never fires.
pub fn detect(
    pixels: &[f32],
    raw: &[u16],
    layout: &ImageLayout,
    options: &DetectOptions,
) -> Option<Detection> {
    detect_with(pixels, raw, layout, options, &mut Scratch::new())
}

/// Full-frame working memory for [`detect_with`], owned by whoever is measuring
/// a run rather than by one call of it.
///
/// A survey worker keeps one for a whole night: the buffers grow to the largest
/// frame that worker is handed and are never given back. What that saves is the
/// mapping and the kernel's zeroing of it, not the arithmetic — a plane returned
/// between frames is faulted back in one page at a time on the next one.
///
/// Deliberately not `Clone`. The only reason to clone one would be to give a
/// worker its own, and rayon's `map_with` clones its item into both halves of
/// every split, which would make that a deep copy of two full frames.
#[derive(Debug, Default)]
pub struct Scratch {
    /// Whitened on the way in and filtered on the way out. The filter's result
    /// lands back here because nothing reads a whitened value once the
    /// horizontal pass has consumed it, and a third resident plane per worker is
    /// exactly what would push the survey's per-worker estimate past what it
    /// claims.
    plane: Vec<f32>,
    /// The separable filter's horizontal intermediate.
    work: Vec<f32>,
    claimed: Vec<bool>,
    /// One source's samples, reused across the thousands in a frame.
    samples: Vec<(f64, f64, f64)>,
}

impl Scratch {
    pub fn new() -> Self {
        Self::default()
    }
}

/// [`detect`], with the working memory supplied by the caller.
///
/// `scratch` is working memory and never an output: nothing in it is read before
/// it is written, so one of the wrong length, or holding another frame's pixels,
/// is safe to pass.
pub fn detect_with(
    pixels: &[f32],
    raw: &[u16],
    layout: &ImageLayout,
    options: &DetectOptions,
    scratch: &mut Scratch,
) -> Option<Detection> {
    let mosaic = Mosaic::new(layout)?;
    let width = layout.width as usize;
    let height = layout.height as usize;
    if pixels.len() != width * height || raw.len() != pixels.len() {
        return None;
    }
    let sky = sky::measure(pixels, layout, &mosaic)?;
    let Scratch { plane, work, claimed, samples } = scratch;

    // Signal above this photosite's own colour's sky, in units of that colour's
    // own noise. Everything downstream thresholds on a plane whose noise is one
    // everywhere, which is what lets a single number be the threshold.
    let area = mosaic.area;
    // Cleared and regrown rather than resized in place: the loop below writes
    // only the measurable area while the filter reads the whole plane, so what
    // lies outside the area has to be the zero a fresh allocation used to give
    // for free. A border left over from the previous frame reaches the filter's
    // radius into the area, where it changes which photosites a footprint grows
    // through without changing anything a test has ever looked at.
    plane.clear();
    plane.resize(pixels.len(), 0.0);
    for y in area.y..area.y + area.height {
        let row = y * width;
        for x in area.x..area.x + area.width {
            let colour = mosaic.colour_at(x, y);
            let noise = sky.sigma_at(x, y, colour);
            let value = pixels[row + x] - sky.level_at(x, y, colour);
            plane[row + x] = if noise > 0.0 && value.is_finite() { value / noise } else { 0.0 };
        }
    }

    // Not cleared, unlike the plane: every element of it is assigned by the
    // horizontal pass before the vertical one reads any.
    work.resize(pixels.len(), 0.0);
    matched_filter(plane, work, width, height, options.filter_sigma);
    let found = extract(plane, pixels, raw, layout, &mosaic, &sky, options, claimed, samples);

    let (sky_level, noise) = sky_summary(&sky, &mosaic);
    let shape = FrameShape::of(&found.stars, sky_level, noise);
    Some(Detection {
        stars: found.stars,
        shape,
        saturated: found.saturated,
        oversized: found.oversized,
        sky,
    })
}

/// A separable Gaussian, scaled so that unit-noise input gives unit-noise
/// output.
///
/// Without that scaling the threshold would mean something different for every
/// filter width: convolving unit white noise with a kernel that sums to one
/// leaves noise of `sum(k^2)` in two dimensions, which is 0.176 at sigma 1.6, so
/// an unscaled five would really be a twenty-eight.
///
/// Filters `plane` in place, using `work` as the intermediate between the two
/// passes. Writing the result back over the input is safe and changes no value:
/// the vertical pass reads only `work`, so nothing it overwrites is read again.
/// Both must already be `width * height` long.
fn matched_filter(plane: &mut [f32], work: &mut [f32], width: usize, height: usize, sigma: f32) {
    let radius = (sigma * 3.0).ceil().max(1.0) as usize;
    let mut kernel: Vec<f32> = (0..=2 * radius)
        .map(|i| {
            let offset = i as f32 - radius as f32;
            (-(offset * offset) / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    let total: f32 = kernel.iter().sum();
    for value in &mut kernel {
        *value /= total;
    }
    // The two-dimensional kernel is the outer product, so its sum of squares is
    // the square of the one-dimensional one, and the noise scale is therefore
    // exactly that one-dimensional sum.
    let noise_scale: f32 = kernel.iter().map(|k| k * k).sum();

    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            let mut total = 0f32;
            for (i, weight) in kernel.iter().enumerate() {
                let sample = (x + i).saturating_sub(radius).min(width - 1);
                total += plane[row + sample] * weight;
            }
            work[row + x] = total;
        }
    }

    for y in 0..height {
        for x in 0..width {
            let mut total = 0f32;
            for (i, weight) in kernel.iter().enumerate() {
                let sample = (y + i).saturating_sub(radius).min(height - 1);
                total += work[sample * width + x] * weight;
            }
            plane[y * width + x] = total / noise_scale;
        }
    }
}

struct Found {
    stars: Vec<Star>,
    saturated: usize,
    oversized: usize,
}

#[allow(clippy::too_many_arguments)]
fn extract(
    filtered: &[f32],
    pixels: &[f32],
    raw: &[u16],
    layout: &ImageLayout,
    mosaic: &Mosaic,
    sky: &Sky,
    options: &DetectOptions,
    claimed: &mut Vec<bool>,
    samples: &mut Vec<(f64, f64, f64)>,
) -> Found {
    let width = layout.width as usize;
    let area = mosaic.area;
    // One byte per photosite rather than a hash set: a frame has nineteen
    // million of them and the sets would dominate both time and memory.
    //
    // Refilled and not merely resized, because nothing in this pass ever writes
    // a `false`: a photosite left claimed by the previous frame would silently
    // suppress every source standing on it.
    claimed.clear();
    claimed.resize(pixels.len(), false);
    let mut stars = Vec::new();
    let (mut saturated, mut oversized) = (0usize, 0usize);

    // A margin, so that a source touching the edge of the measurable area never
    // has its moments truncated on one side, which would report it as trailed.
    let margin = options.max_footprint.isqrt() + 2;
    let mut queue: Vec<usize> = Vec::with_capacity(options.max_footprint * 2);
    let mut footprint: Vec<usize> = Vec::with_capacity(options.max_footprint);

    for y in area.y + margin..area.y + area.height - margin {
        let row = y * width;
        for x in area.x + margin..area.x + area.width - margin {
            let here = row + x;
            if claimed[here] || filtered[here] < options.detect_sigma {
                continue;
            }
            // A local maximum, so a broad source yields one detection and not
            // a hundred. Strictly greater on one side and greater-or-equal on
            // the other breaks ties on a plateau without dropping it entirely.
            let peak = filtered[here];
            let neighbours = [
                filtered[here - width - 1],
                filtered[here - width],
                filtered[here - width + 1],
                filtered[here - 1],
            ];
            let after = [
                filtered[here + 1],
                filtered[here + width - 1],
                filtered[here + width],
                filtered[here + width + 1],
            ];
            if neighbours.iter().any(|value| *value > peak) || after.iter().any(|value| *value >= peak)
            {
                continue;
            }

            // Grow over the filtered plane, which is smooth, so that a chain of
            // noise photosites cannot walk the footprint across the frame.
            footprint.clear();
            queue.clear();
            queue.push(here);
            claimed[here] = true;
            let mut ran_over = false;
            while let Some(index) = queue.pop() {
                if footprint.len() < options.max_footprint {
                    footprint.push(index);
                } else {
                    // Past the cap the footprint is not worth recording, but the
                    // fill keeps going so that the whole source is claimed. Stop
                    // here instead and a satellite trail is counted once per
                    // local maximum along its length rather than once. Every
                    // photosite is still claimed at most once, so the work stays
                    // linear in the frame.
                    ran_over = true;
                }
                let (px, py) = (index % width, index / width);
                if px <= area.x || py <= area.y || px + 1 >= area.x + area.width
                    || py + 1 >= area.y + area.height
                {
                    continue;
                }
                for neighbour in
                    [index - width, index - 1, index + 1, index + width]
                {
                    if !claimed[neighbour] && filtered[neighbour] >= options.grow_sigma {
                        claimed[neighbour] = true;
                        queue.push(neighbour);
                    }
                }
            }

            if ran_over {
                oversized += 1;
                continue;
            }
            if footprint.len() < options.min_footprint {
                continue;
            }

            // The saturation test reads the RAW samples. The white level is a
            // property of the sensor, and `pixels` may already have had a
            // pedestal removed, so testing it there would mean never firing.
            let clipped = footprint.iter().any(|index| {
                let (px, py) = (index % width, index / width);
                let colour = mosaic.colour_at(px, py);
                let white = layout.white_level.get(colour).copied().unwrap_or(f32::NAN);
                white.is_finite() && f32::from(raw[*index]) >= white
            });
            if clipped {
                saturated += 1;
                continue;
            }

            // `peak` is the filtered value at the local maximum that started
            // this footprint, which is the source's detection significance in
            // sigma - the plane's noise is one by construction.
            if let Some(star) =
                measure_star(&footprint, pixels, layout, mosaic, sky, peak, samples)
            {
                stars.push(star);
            }
        }
    }

    // Most confidently detected first, then capped: what comes after this wants
    // hundreds of the best rather than everything, and "best" has to mean most
    // certainly a star rather than largest blob.
    stars.sort_by(|a, b| b.significance.total_cmp(&a.significance));
    stars.truncate(options.max_stars);
    Found { stars, saturated, oversized }
}

/// How many photosites the window may span before the fit is abandoned, as a
/// variance. Twenty photosites of sigma is far past any star, and a fit that
/// wanders out there is following the background rather than a source.
const MAX_WINDOW_VARIANCE: f64 = 400.0;

/// The narrowest window worth using, as a variance. Below about a third of a
/// photosite squared the window is narrower than the sampling itself and the
/// weighted sum stops describing anything.
const MIN_WINDOW_VARIANCE: f64 = 0.35;

const WINDOW_ITERATIONS: usize = 8;
const WINDOW_TOLERANCE: f64 = 1e-3;

/// Centroid and second moments of one source, measured under a Gaussian window
/// matched to its own shape.
///
/// The obvious estimator — flux times displacement squared, summed over the
/// threshold footprint — does not work here, and the reason is worth recording
/// because it looks harmless. The footprint is grown on the *filtered* plane, so
/// it reaches out to where the smoothed source clears 2.5 sigma, which is well
/// past where the unsmoothed star has any flux left. Its outer ring is noise.
/// And because the weight in a second moment is displacement squared, that outer
/// ring is exactly where a small error counts for most. Dropping the negative
/// half of that noise — which the natural `value <= 0.0` guard does — leaves a
/// one-sided positive residual at large radius and inflates every width.
/// Measured on this project's reference session it reported a cross-trail width
/// of 4.16 photosites where the stars are about 1.9.
///
/// So the window: weight each sample by a Gaussian of the source's own
/// covariance, which falls away faster than the noise can accumulate. For a
/// Gaussian source under a Gaussian window the measured covariance is the
/// harmonic combination of the two, so a window equal to the source measures
/// half of it — which makes twice the measurement the window to try next, and
/// the iteration's fixed point the source itself.
///
/// Measured on the sky-subtracted frame, never on the filtered plane: the filter
/// exists to decide *where* to look, and measuring on it would report the
/// filter's own width added to every star.
#[allow(clippy::too_many_arguments)]
fn measure_star(
    footprint: &[usize],
    pixels: &[f32],
    layout: &ImageLayout,
    mosaic: &Mosaic,
    sky: &Sky,
    significance: f32,
    samples: &mut Vec<(f64, f64, f64)>,
) -> Option<Star> {
    let width = layout.width as usize;
    // Negatives are kept throughout. They are half of the noise, and leaving
    // them out is what biases the widths.
    //
    // Borrowed rather than allocated: this is one allocation per source and a
    // frame holds thousands of them. The capacity is bounded by the footprint
    // cap and settles after the first few.
    samples.clear();

    for &index in footprint {
        let (x, y) = (index % width, index / width);
        let colour = mosaic.colour_at(x, y);
        let value = f64::from(pixels[index] - sky.level_at(x, y, colour));
        if !value.is_finite() {
            continue;
        }
        samples.push((x as f64, y as f64, value));
    }

    // A starting point for the window, not a reported measurement: the negative
    // half is dropped here only so that the first window is positive-definite.
    let (mut sum, mut cx, mut cy) = (0f64, 0f64, 0f64);
    for &(x, y, value) in samples.iter() {
        if value > 0.0 {
            sum += value;
            cx += value * x;
            cy += value * y;
        }
    }
    if sum <= 0.0 {
        return None;
    }
    let (cx, cy) = (cx / sum, cy / sum);
    let (mut s11, mut s22, mut s12) = (0f64, 0f64, 0f64);
    for &(x, y, value) in samples.iter() {
        if value > 0.0 {
            let (dx, dy) = (x - cx, y - cy);
            s11 += value * dx * dx;
            s22 += value * dy * dy;
            s12 += value * dx * dy;
        }
    }
    let seed = Moments { m11: s11 / sum, m22: s22 / sum, m12: s12 / sum };

    let Some((x, y, moments, under_window)) = window_moments(samples, cx, cy, seed) else {
        // The shape could not be fitted, but the position still stands, and
        // registration wants positions. `Moments::NONE` keeps this source out of
        // the frame's shape statistics rather than voting a made-up width into
        // them.
        // The window fit is what makes the flux blur-independent, so without it
        // there is no flux worth reporting either. The position still stands,
        // and registration wants positions.
        return Some(Star {
            x: cx,
            y: cy,
            flux: f64::NAN,
            significance,
            moments: Moments::NONE,
            footprint: footprint.len(),
        });
    };

    Some(Star { x, y, flux: 2.0 * under_window, significance, moments, footprint: footprint.len() })
}

/// Iterates a Gaussian window to the source's own shape.
///
/// Returns the windowed centroid, the deconvolved covariance, and the sum under
/// the converged window — which is half the source's total flux, whatever its
/// shape. See [`Star::flux`].
fn window_moments(
    samples: &[(f64, f64, f64)],
    mut cx: f64,
    mut cy: f64,
    seed: Moments,
) -> Option<(f64, f64, Moments, f64)> {
    let mut window = if seed.axes().is_some() {
        Moments {
            m11: seed.m11.max(MIN_WINDOW_VARIANCE),
            m22: seed.m22.max(MIN_WINDOW_VARIANCE),
            m12: seed.m12,
        }
    } else {
        // No usable seed, so start round and let the iteration find the shape
        // rather than inheriting a broken one.
        Moments { m11: MIN_WINDOW_VARIANCE, m22: MIN_WINDOW_VARIANCE, m12: 0.0 }
    };

    let mut under_window = f64::NAN;
    for _ in 0..WINDOW_ITERATIONS {
        let determinant = window.m11 * window.m22 - window.m12 * window.m12;
        if determinant.is_nan() || determinant <= 0.0 {
            return None;
        }
        let (i11, i22, i12) =
            (window.m22 / determinant, window.m11 / determinant, -window.m12 / determinant);

        let (mut weight, mut sx, mut sy) = (0f64, 0f64, 0f64);
        let (mut m11, mut m22, mut m12) = (0f64, 0f64, 0f64);
        for &(x, y, value) in samples {
            let (dx, dy) = (x - cx, y - cy);
            let distance = i11 * dx * dx + 2.0 * i12 * dx * dy + i22 * dy * dy;
            // Four window sigmas out the weight is under 1e-7, and every sample
            // past it costs an exponential to add nothing.
            if distance > 16.0 {
                continue;
            }
            let w = (-0.5 * distance).exp() * value;
            weight += w;
            sx += w * dx;
            sy += w * dy;
            m11 += w * dx * dx;
            m22 += w * dy * dy;
            m12 += w * dx * dy;
        }
        if weight.is_nan() || weight <= 0.0 {
            return None;
        }
        under_window = weight;

        let (ox, oy) = (sx / weight, sy / weight);
        cx += ox;
        cy += oy;
        let measured = Moments {
            m11: m11 / weight - ox * ox,
            m22: m22 / weight - oy * oy,
            m12: m12 / weight - ox * oy,
        };
        let next =
            Moments { m11: 2.0 * measured.m11, m22: 2.0 * measured.m22, m12: 2.0 * measured.m12 };
        if next.axes().is_none() || next.m11 > MAX_WINDOW_VARIANCE || next.m22 > MAX_WINDOW_VARIANCE
        {
            return None;
        }

        let moved = (next.m11 - window.m11).abs()
            + (next.m22 - window.m22).abs()
            + (next.m12 - window.m12).abs();
        window = next;
        if moved < WINDOW_TOLERANCE {
            break;
        }
    }

    Some((cx, cy, window, under_window))
}

/// The median sky and noise over the grid, for the report.
fn sky_summary(sky: &Sky, mosaic: &Mosaic) -> (f32, f32) {
    let mut levels = Vec::new();
    let mut noises = Vec::new();
    for cell_y in 0..sky.cells_y {
        for cell_x in 0..sky.cells_x {
            let level = sky.level_at(cell_x * sky::CELL, cell_y * sky::CELL, mosaic.dominant);
            let noise = sky.sigma_at(cell_x * sky::CELL, cell_y * sky::CELL, mosaic.dominant);
            if level.is_finite() {
                levels.push(level);
            }
            if noise.is_finite() {
                noises.push(noise);
            }
        }
    }
    let median = |values: &mut Vec<f32>| {
        if values.is_empty() {
            return f32::NAN;
        }
        let middle = values.len() / 2;
        let (_, value, _) = values.select_nth_unstable_by(middle, f32::total_cmp);
        *value
    };
    (median(&mut levels), median(&mut noises))
}

#[cfg(test)]
mod tests {
    use super::*;
    use astro_plugin_abi::abi::{COLOR_BLUE, COLOR_GREEN, COLOR_RED};

    fn layout(width: usize, height: usize) -> ImageLayout {
        let mut layout = ImageLayout {
            width: width as u32,
            height: height as u32,
            components: 1,
            cfa_width: 2,
            cfa_height: 2,
            active_width: width as u32,
            active_height: height as u32,
            white_level: [16383.0; 4],
            ..Default::default()
        };
        // GBRG, the reference body's pattern.
        layout.cfa_pattern[..4].copy_from_slice(&[COLOR_GREEN, COLOR_BLUE, COLOR_RED, COLOR_GREEN]);
        layout
    }

    /// A frame of sky with elliptical Gaussians painted on it, with the colour
    /// response of a real mosaic.
    fn frame(
        width: usize,
        height: usize,
        stars: &[(f64, f64, f64, f64, f64, f64)],
    ) -> (Vec<f32>, Vec<u16>, ImageLayout) {
        let layout = layout(width, height);
        let mosaic = Mosaic::new(&layout).unwrap();
        // Green twice red and blue, which is stronger modulation than the
        // reference body actually shows, so the test is the harder case.
        let response = [0.5f64, 1.0, 0.5, 1.0];
        // Real sky has noise, and a detection threshold is measured in units
        // of it. A noiseless test frame has no such unit, which is a fact about
        // the fixture rather than about the detector.
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut noise = move || {
            let mut total = 0f32;
            for _ in 0..3 {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                total += (state >> 40) as f32 / 16_777_216.0 - 0.5;
            }
            total * 40.0
        };
        let mut pixels: Vec<f32> = (0..width * height).map(|_| 300.0 + noise()).collect();

        for &(cx, cy, sx, sy, angle, amplitude) in stars {
            let (c, s) = (angle.cos(), angle.sin());
            for y in 0..height {
                for x in 0..width {
                    let (dx, dy) = (x as f64 - cx, y as f64 - cy);
                    let (u, v) = (dx * c + dy * s, -dx * s + dy * c);
                    let value = amplitude
                        * (-(u * u / (2.0 * sx * sx) + v * v / (2.0 * sy * sy))).exp();
                    if value > 0.01 {
                        let colour = mosaic.colour_at(x, y);
                        pixels[y * width + x] += (value * response[colour.min(3)]) as f32;
                    }
                }
            }
        }
        let raw: Vec<u16> = pixels.iter().map(|v| v.clamp(0.0, 65_535.0) as u16).collect();
        (pixels, raw, layout)
    }

    /// A frame whose measurable area stops short of its edges, which is what
    /// every real sensor looks like and what none of the other fixtures here do.
    fn inset(width: usize, height: usize, margin: usize) -> ImageLayout {
        let mut layout = layout(width, height);
        layout.active_x = margin as u32;
        layout.active_y = margin as u32;
        layout.active_width = (width - 2 * margin) as u32;
        layout.active_height = (height - 2 * margin) as u32;
        layout
    }

    #[test]
    fn a_reused_scratch_finds_exactly_the_stars_a_fresh_one_does() {
        // The trap this exists for: the whitening loop writes only the
        // measurable area, while the filter reads the whole plane, so what lies
        // outside the area used to be zero because the allocation was fresh. A
        // buffer carried between frames brings the previous frame's border with
        // it, and it reaches the filter's radius into the area — far enough to
        // change which photosites a footprint grows through, and so the moments
        // and the flux, without moving any star far enough to look wrong.
        //
        // Sizes deliberately go up as well as down, and an inset frame follows a
        // frame whose area runs edge to edge, so the border being tested is one
        // the previous frame actually wrote into.
        //ance third source sits a few photosites inside the measurable area, where
        // a stale border can still reach it.
        let sources = [
            (40.0, 34.0, 1.9, 3.6, 0.4, 9000.0),
            (78.0, 61.0, 1.7, 1.7, 0.0, 4000.0),
            (14.0, 15.0, 1.8, 1.8, 0.0, 7000.0),
        ];
        let mut carried = Scratch::new();
        let mut compared = 0usize;

        // The second options set narrows the scan margin below the filter's
        // radius. With the shipped `max_footprint` the scan starts far enough
        // inside the area that a stale border cannot seed or suppress a
        // detection, only bend a footprint that grows out to meet it; this one
        // puts the contaminated strip under the peak scan itself, which is where
        // the failure is visible rather than merely present.
        let narrow = DetectOptions { max_footprint: 8, ..DetectOptions::default() };
        for (width, height, margin, options) in [
            (128usize, 96usize, 0usize, DetectOptions::default()),
            (128, 96, 9, DetectOptions::default()),
            (128, 96, 6, narrow),
            (96, 72, 4, narrow),
            (160, 128, 11, DetectOptions::default()),
            (128, 96, 9, DetectOptions::default()),
        ] {
            let (pixels, raw, _) = frame(width, height, &sources);
            let layout = inset(width, height, margin);

            let fresh = detect(&pixels, &raw, &layout, &options);
            // Hostile rather than merely stale, and of exactly the right length
            // so that a `resize` alone would leave every byte of it in place.
            // Whatever the previous frame happened to leave behind is a subset
            // of this, so a pass here is a pass for any order of frames.
            carried.plane = vec![1.0e6; width * height];
            carried.work = vec![-1.0e6; width * height];
            carried.claimed = vec![true; width * height];
            carried.samples = vec![(f64::NAN, f64::NAN, f64::NAN); 32];
            let reused = detect_with(&pixels, &raw, &layout, &options, &mut carried);

            assert_eq!(
                fresh.is_none(),
                reused.is_none(),
                "{width}x{height}+{margin}: one run measured the frame and the other did not"
            );
            let (Some(fresh), Some(reused)) = (fresh, reused) else { continue };
            compared += fresh.stars.len();
            assert_eq!(fresh.stars.len(), reused.stars.len(), "{width}x{height}+{margin} count");
            assert_eq!(fresh.saturated, reused.saturated, "{width}x{height}+{margin} saturated");
            assert_eq!(fresh.oversized, reused.oversized, "{width}x{height}+{margin} oversized");
            for (index, (a, b)) in fresh.stars.iter().zip(&reused.stars).enumerate() {
                // By bits, so that a NaN counts as agreement only where the
                // fresh run also produced one.
                assert_eq!(a.x.to_bits(), b.x.to_bits(), "{width}x{height}+{margin} star {index} x");
                assert_eq!(a.y.to_bits(), b.y.to_bits(), "{width}x{height}+{margin} star {index} y");
                assert_eq!(a.flux.to_bits(), b.flux.to_bits(), "{width}x{height}+{margin} flux");
                assert_eq!(a.significance.to_bits(), b.significance.to_bits(), "significance");
                assert_eq!(a.footprint, b.footprint, "{width}x{height}+{margin} footprint");
                assert_eq!(a.moments.m11.to_bits(), b.moments.m11.to_bits(), "m11");
                assert_eq!(a.moments.m22.to_bits(), b.moments.m22.to_bits(), "m22");
                assert_eq!(a.moments.m12.to_bits(), b.moments.m12.to_bits(), "m12");
            }
        }
        assert!(compared > 0, "the fixtures produced no stars, so nothing was actually compared");
    }

    #[test]
    fn a_trailed_star_is_found_and_measured() {
        // The reference session's actual PSF: about 2 photosites across and 6
        // along, at 94.5 degrees.
        let sigma_major = 6.0 / 2.3548;
        let sigma_minor = 2.0 / 2.3548;
        let (pixels, raw, layout) =
            frame(128, 128, &[(64.0, 64.0, sigma_major, sigma_minor, 94.5f64.to_radians(), 4000.0)]);

        let found = detect(&pixels, &raw, &layout, &DetectOptions::default()).expect("describable");
        assert_eq!(found.stars.len(), 1, "one star, found once");

        let star = found.stars[0];
        assert!((star.x - 64.0).abs() < 0.3, "x {}", star.x);
        assert!((star.y - 64.0).abs() < 0.3, "y {}", star.y);
        // The width it was painted with, not merely "some trail". This is the
        // assertion that catches a moment estimator picking up the noise in the
        // outer ring of its own footprint, which reads as every star being wider
        // and rounder than it is.
        assert!(
            (star.moments.major_fwhm() - 6.0).abs() < 0.6,
            "along the trail: {}",
            star.moments.major_fwhm()
        );
        assert!(
            (star.moments.minor_fwhm() - 2.0).abs() < 0.4,
            "across the trail: {}",
            star.moments.minor_fwhm()
        );
        assert!((star.moments.trail() - 4.0).abs() < 0.8, "trail: {}", star.moments.trail());
        assert!(
            (star.moments.angle_degrees() - 94.5).abs() < 6.0,
            "angle: {}",
            star.moments.angle_degrees()
        );
        assert!(found.shape.direction_agreement.is_nan() || found.shape.stars == 1);
    }

    #[test]
    fn the_flux_of_one_star_does_not_change_when_it_is_blurred_or_trailed() {
        // The identity this rests on: under a window fitted to the source, the
        // sum is half the total flux whatever the source's shape. Without it a
        // trailed frame reads fainter at unchanged transparency, and a stack
        // comparing frame brightnesses would take the trailing for cloud and
        // scale the frame up - brightening exactly the worst frames.
        //
        // Same total flux, three shapes: round and tight, round and soft, and
        // trailed three and a half to one like this session's own frames.
        let total = 40_000.0f64;
        let shapes = [(1.0f64, 1.0f64, 0.0f64), (2.0, 2.0, 0.0), (3.5, 1.0, 94.5f64.to_radians())];
        let mut fluxes = Vec::new();
        for (sx, sy, angle) in shapes {
            let amplitude = total / (std::f64::consts::TAU * sx * sy);
            let (pixels, raw, layout) = frame(160, 160, &[(80.0, 80.0, sx, sy, angle, amplitude)]);
            let found = detect(&pixels, &raw, &layout, &DetectOptions::default()).unwrap();
            assert_eq!(found.stars.len(), 1, "one star at {sx}x{sy}");
            fluxes.push(found.stars[0].flux);
        }

        let reference = fluxes[0];
        for (flux, (sx, sy, _)) in fluxes.iter().zip(shapes) {
            let error = (flux - reference).abs() / reference;
            assert!(error < 0.10, "at {sx}x{sy} the flux moved by {:.1}%", error * 100.0);
        }
    }

    #[test]
    fn a_star_whose_shape_could_not_be_fitted_reports_no_flux_either
    () {
        // The flux is only blur-independent because the window converged. Where
        // it did not, a number would be a different measurement wearing the same
        // name.
        let star = Star::at(1.0, 2.0, Moments::NONE);
        assert!(star.flux.is_nan());
    }

    #[test]
    fn a_faint_star_is_not_widened_by_the_noise_around_it() {
        // The failure this exists for. A footprint is grown on the filtered
        // plane and reaches past where the star has any flux, so its outer ring
        // is noise; the weight in a second moment is displacement squared, which
        // is largest exactly there. Keeping only the positive half of that noise
        // inflated the reference session's stars from 1.9 photosites to 4.16.
        //
        // Same star, two brightnesses. If the noise were leaking in, the faint
        // one - where the outer ring is a larger share of the total - would read
        // as the wider of the two.
        let sigma = 2.2 / 2.3548;
        let mut widths = Vec::new();
        for amplitude in [8000.0, 700.0] {
            let (pixels, raw, layout) = frame(160, 160, &[(80.0, 80.0, sigma, sigma, 0.0, amplitude)]);
            let found = detect(&pixels, &raw, &layout, &DetectOptions::default()).unwrap();
            assert_eq!(found.stars.len(), 1, "at amplitude {amplitude}");
            let star = found.stars[0];
            assert!(
                (star.moments.minor_fwhm() - 2.2).abs() < 0.45,
                "at amplitude {amplitude} the width came back as {}",
                star.moments.minor_fwhm()
            );
            // And it stays round: a noise-driven width is isotropic, so it shows
            // up as a spurious trail too once the axes stop agreeing.
            assert!(star.moments.trail().abs() < 0.5, "at {amplitude}: {}", star.moments.trail());
            widths.push(star.moments.minor_fwhm());
        }
        assert!(
            (widths[1] - widths[0]).abs() < 0.4,
            "brightness must not change the width: {widths:?}"
        );
    }

    #[test]
    fn the_mosaic_does_not_move_the_centroid() {
        // The worry that shaped this module: a star centred on a green photosite
        // and the same star centred on a blue one must be found in the same
        // place. The response here is 2:1, stronger than the real sensor's.
        let sigma = 1.4 / 2.3548;
        for (cx, cy) in [(64.0, 64.0), (65.0, 64.0), (64.0, 65.0), (64.5, 64.5), (64.25, 64.75)] {
            let (pixels, raw, layout) = frame(128, 128, &[(cx, cy, sigma, sigma, 0.0, 6000.0)]);
            let found = detect(&pixels, &raw, &layout, &DetectOptions::default()).unwrap();
            assert_eq!(found.stars.len(), 1, "at {cx},{cy}");
            let star = found.stars[0];
            assert!(
                (star.x - cx).abs() < 0.25 && (star.y - cy).abs() < 0.25,
                "star at {cx},{cy} was found at {},{}",
                star.x,
                star.y
            );
        }
    }

    #[test]
    fn a_saturated_core_is_dropped_rather_than_centroided() {
        // A flat-topped core has no centroid worth the name. The test has to
        // read the raw samples: on the default path `pixels` has had a pedestal
        // removed, and a white level compared against that would never fire.
        let (mut pixels, mut raw, layout) =
            frame(128, 128, &[(64.0, 64.0, 1.0, 1.0, 0.0, 8000.0)]);
        for y in 62..67 {
            for x in 62..67 {
                pixels[y * 128 + x] = 20000.0;
                raw[y * 128 + x] = 20000;
            }
        }
        let found = detect(&pixels, &raw, &layout, &DetectOptions::default()).unwrap();
        assert_eq!(found.stars.len(), 0);
        assert_eq!(found.saturated, 1);
    }

    #[test]
    fn a_satellite_trail_is_counted_and_not_measured() {
        // Its moments describe a line across the frame, and a frame shape that
        // followed it would report the night as a disaster.
        let (pixels, raw, layout) =
            frame(256, 256, &[(128.0, 128.0, 60.0, 1.2, 0.3, 4000.0)]);
        let found = detect(&pixels, &raw, &layout, &DetectOptions::default()).unwrap();
        assert_eq!(found.oversized, 1, "the streak ran over the footprint cap");
        assert!(found.stars.is_empty());
    }

    #[test]
    fn empty_sky_yields_no_stars_rather_than_noise() {
        let (pixels, raw, layout) = frame(128, 128, &[]);
        let found = detect(&pixels, &raw, &layout, &DetectOptions::default()).unwrap();
        assert!(found.stars.is_empty());
        assert!(found.shape.moments.trail().is_nan(), "no stars, no shape, not a zero");
    }
}
