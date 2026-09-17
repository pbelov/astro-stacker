//! What one decode is worth keeping.
//!
//! This module reports numbers and does not judge them. Deciding that a flat is
//! too dim or that a frame is not the kind it claims needs thresholds, and every
//! threshold worth having has to be argued against real frames rather than
//! guessed; those rules come later. What is settled is *which* numbers are worth
//! the decode, and there the measurements are unambiguous.
//!
//! Two of them, both learned from a real session rather than reasoned about:
//!
//! * Whole-frame minimum and maximum are useless. Across 370 frames of one
//!   night — lights, darks, flats and biases alike — every frame reported a
//!   maximum of 16310 to 16376 and a minimum between 1 and 333, because hot and
//!   cold photosites pin both ends everywhere. A 1/8000 s flat and a 30-second
//!   dark were indistinguishable by that measure.
//!
//! * The statistics have to be per colour-filter cell. On a GBRG sensor there
//!   are twice as many green photosites as red, and under broadband light they
//!   sit at roughly twice the level, so a whole-frame median lands between the
//!   two populations and describes neither.

use crate::frame::{CFA_MAX_CELLS, ImageLayout};

use crate::frame::Samples;

/// Sample values are 16 bit, so an exact histogram costs 256 KB per colour.
/// Paid once per frame and dropped with the pixels; what survives is
/// [`ColourStats`], about a hundred bytes.
const LEVELS: usize = 1 << 16;

/// The uniformity map is 3x3 and only ever compares blocks with each other, so
/// it does not need the full resolution: 1024 bins is about 64 ADU, far finer
/// than any real illumination ramp.
const BLOCK_LEVELS: usize = 1 << 10;
const BLOCK_SHIFT: u32 = 6;
const BLOCKS: usize = 9;

/// The residue of decoding one frame.
///
/// Small enough to hold for a whole session: a 500-frame session is about
/// 60 KB, against the 45 GB the frames themselves would be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStats {
    /// Indexed by colour-filter index — 0 red, 1 green, 2 blue, 3 a fourth
    /// filter. `None` where the mosaic has no photosites of that colour.
    pub colours: [Option<ColourStats>; 4],
    pub uniformity: Uniformity,
}

impl FrameStats {
    /// The colour with the most photosites, which on a Bayer sensor is green.
    /// The one to quote when a single number is wanted.
    pub fn dominant(&self) -> Option<ColourStats> {
        self.colours.iter().flatten().copied().max_by_key(|colour| colour.samples)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColourStats {
    pub samples: u64,
    /// Median sample in ADU, before anything is subtracted.
    pub median: f32,
    /// The black point taken from the layout, averaged over the cells of this
    /// colour. `NaN` when the frame did not record one.
    pub black: f32,
    /// The saturation point taken from the layout. `NaN` when unrecorded.
    pub white: f32,
    /// Median above black as a fraction of the distance from black to white.
    /// `NaN` when either level was unrecorded, never a stand-in number.
    pub level: f32,
    /// Median absolute deviation, in ADU. Robust: a field of stars or a hot
    /// column moves it far less than a standard deviation.
    pub spread: f32,
    /// Fraction of samples at or above the saturation point. `NaN` when the
    /// white level was unrecorded.
    pub clipped: f32,
}

/// How evenly the frame is lit, as a 3x3 map relative to the centre.
///
/// Only meaningful for a flat, where it measures the light source rather than
/// the sensor. A panel that is tilted, too small for the field, or lit off-axis
/// shows here as a ramp, and dividing lights by such a flat imprints that ramp
/// on every one of them.
///
/// Computed on signal above the black level, not on raw values. That is not a
/// refinement, it is the whole measurement: on a flat sitting at 9% of full
/// scale the pedestal is larger than the signal, and a raw ratio would report a
/// 24% illumination ramp as 8% — quietest exactly where a dim flat needs it
/// loudest. Every block is `NaN` when the frame recorded no black level, since
/// a ratio taken on a pedestal is not an evenness ratio at all.
///
/// The map is coarse by construction: each block is a ninth of the active area,
/// so a corner block is centred about two thirds of the way out and understates
/// the falloff at the very corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Uniformity {
    /// Row-major, top-left first, each the block's median divided by the centre
    /// block's. The centre is 1.0 by construction.
    pub blocks: [f32; BLOCKS],
}

impl Uniformity {
    /// Brightest block over dimmest. 1.0 is perfectly even.
    pub fn ramp(&self) -> f32 {
        let finite = || self.blocks.iter().copied().filter(|value| value.is_finite());
        let low = finite().fold(f32::INFINITY, f32::min);
        let high = finite().fold(f32::NEG_INFINITY, f32::max);
        if low > 0.0 && high.is_finite() { high / low } else { f32::NAN }
    }
}

/// Reduces one decoded frame to [`FrameStats`], then drops the pixels.
///
/// Returns `None` for a frame this cannot describe: floating-point samples, a
/// layout whose active area does not lie inside the frame, or no mosaic.
pub fn measure(samples: &Samples, layout: &ImageLayout) -> Option<FrameStats> {
    let Samples::U16(values) = samples else {
        return None;
    };
    let area = active_area(layout, values.len())?;
    let cfa = CfaGrid::new(layout)?;

    // One pass. Two accumulators, both indexed without a division in the inner
    // loop: the colour and block indices are advanced by counters that wrap,
    // because a modulo by a runtime divisor is a hardware divide per sample.
    let mut histograms = vec![0u32; 4 * LEVELS];
    let mut blocks = vec![0u32; BLOCKS * BLOCK_LEVELS];

    for y in area.y..area.y + area.height {
        let row = y * layout.width as usize;
        let block_row = (y - area.y) * 3 / area.height;
        let cfa_row = (y % cfa.height) * cfa.width;
        let mut cfa_col = area.x % cfa.width;
        let mut block_col = 0usize;
        // The x at which the block index next advances, so the loop never
        // divides.
        let mut next_block_at = area.x + area.width.div_ceil(3);

        for x in area.x..area.x + area.width {
            let value = values[row + x];
            let colour = cfa.pattern[cfa_row + cfa_col] as usize;
            histograms[colour * LEVELS + value as usize] += 1;

            if colour == cfa.dominant {
                let bin = (value >> BLOCK_SHIFT) as usize;
                blocks[(block_row * 3 + block_col) * BLOCK_LEVELS + bin] += 1;
            }

            cfa_col += 1;
            if cfa_col == cfa.width {
                cfa_col = 0;
            }
            if x + 1 == next_block_at && block_col < 2 {
                block_col += 1;
                next_block_at += area.width.div_ceil(3);
            }
        }
    }

    let mut colours = [None; 4];
    for (index, slot) in colours.iter_mut().enumerate() {
        *slot = summarise(&histograms[index * LEVELS..(index + 1) * LEVELS], layout, &cfa, index);
    }

    // The blocks hold the dominant colour, so they take that colour's black
    // point.
    let black = cfa.black_level(layout, cfa.dominant);
    Some(FrameStats { colours, uniformity: uniformity(&blocks, black) })
}

/// A finer illumination map, normalised to its own mean.
///
/// Deliberately not part of [`FrameStats`]: a session holds hundreds of those
/// and they have to stay tiny, while this is `grid * grid` floats and exists to
/// answer a question about one or two frames. Normalised to the mean rather
/// than to the centre so that two maps can be subtracted — comparing a flat
/// against another flat is how you find out whether anything in the optical
/// train moved between them.
///
/// Uses the dominant colour only, and signal above the black level. `None` when
/// the frame cannot be described, or recorded no black level.
pub fn illumination_map(samples: &Samples, layout: &ImageLayout, grid: usize) -> Option<Vec<f32>> {
    let Samples::U16(values) = samples else {
        return None;
    };
    if grid == 0 {
        return None;
    }
    let area = active_area(layout, values.len())?;
    let cfa = CfaGrid::new(layout)?;
    let black = cfa.black_level(layout, cfa.dominant);
    if !black.is_finite() {
        return None;
    }

    // Sum and count rather than a histogram per cell: at a fine grid the
    // histograms would dominate the memory, and a flat has no outliers worth
    // being robust against beyond the hot photosites, which a mean over tens of
    // thousands of samples absorbs.
    let cells = grid.checked_mul(grid)?;
    let mut totals = vec![0f64; cells];
    let mut counts = vec![0u64; cells];

    for y in area.y..area.y + area.height {
        let row = y * layout.width as usize;
        let cell_row = (y - area.y) * grid / area.height;
        let cfa_row = (y % cfa.height) * cfa.width;
        let mut cfa_col = area.x % cfa.width;

        for x in area.x..area.x + area.width {
            if cfa.pattern[cfa_row + cfa_col] as usize == cfa.dominant {
                let cell = cell_row * grid + ((x - area.x) * grid / area.width).min(grid - 1);
                totals[cell] += f64::from(values[row + x]);
                counts[cell] += 1;
            }
            cfa_col += 1;
            if cfa_col == cfa.width {
                cfa_col = 0;
            }
        }
    }

    let mut map: Vec<f32> = totals
        .iter()
        .zip(&counts)
        .map(|(total, count)| {
            if *count == 0 { f32::NAN } else { (total / *count as f64) as f32 - black }
        })
        .collect();

    let finite: Vec<f32> = map.iter().copied().filter(|value| value.is_finite()).collect();
    let mean = finite.iter().sum::<f32>() / finite.len().max(1) as f32;
    // NaN included: a mean that is not a positive number cannot normalise anything.
    if !mean.is_finite() || mean <= 0.0 {
        return None;
    }
    for value in &mut map {
        *value /= mean;
    }
    Some(map)
}

// ---------------------------------------------------------------------------

struct Area {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

/// The light-sensitive region, or the whole frame when the plugin did not say.
/// Refuses an area that does not fit, rather than reading past the buffer.
fn active_area(layout: &ImageLayout, samples: usize) -> Option<Area> {
    let (width, height) = (layout.width as usize, layout.height as usize);
    if width == 0 || height == 0 || layout.components != 1 || width * height != samples {
        return None;
    }
    let area = if layout.active_width == 0 || layout.active_height == 0 {
        Area { x: 0, y: 0, width, height }
    } else {
        Area {
            x: layout.active_x as usize,
            y: layout.active_y as usize,
            width: layout.active_width as usize,
            height: layout.active_height as usize,
        }
    };
    (area.x + area.width <= width && area.y + area.height <= height && area.width >= 3 && area.height >= 3)
        .then_some(area)
}

struct CfaGrid {
    width: usize,
    height: usize,
    pattern: [u8; CFA_MAX_CELLS],
    /// The colour with the most cells in the repeat: green on a Bayer sensor.
    dominant: usize,
}

impl CfaGrid {
    fn new(layout: &ImageLayout) -> Option<Self> {
        let (width, height) = (layout.cfa_width as usize, layout.cfa_height as usize);
        if width == 0 || height == 0 || width * height > CFA_MAX_CELLS {
            return None;
        }
        let mut counts = [0usize; 4];
        for &colour in &layout.cfa_pattern[..width * height] {
            *counts.get_mut(colour as usize)? += 1;
        }
        let dominant = counts.iter().enumerate().max_by_key(|(_, count)| **count)?.0;
        Some(Self { width, height, pattern: layout.cfa_pattern, dominant })
    }

    /// The black level for one colour: the mean of the grid cells that carry it.
    ///
    /// The two grids are both aligned to the frame origin but need not be the
    /// same shape, so the cells are walked over their least common multiple.
    fn black_level(&self, layout: &ImageLayout, colour: usize) -> f32 {
        let (bw, bh) = (layout.black_level_width as usize, layout.black_level_height as usize);
        if bw == 0 || bh == 0 || bw * bh > CFA_MAX_CELLS {
            return f32::NAN;
        }
        let width = lcm(self.width, bw);
        let height = lcm(self.height, bh);
        let mut total = 0.0f64;
        let mut count = 0u32;
        for y in 0..height {
            for x in 0..width {
                if self.pattern[(y % self.height) * self.width + (x % self.width)] as usize != colour
                {
                    continue;
                }
                let level = layout.black_level[(y % bh) * bw + (x % bw)];
                if !level.is_finite() {
                    return f32::NAN;
                }
                total += f64::from(level);
                count += 1;
            }
        }
        if count == 0 { f32::NAN } else { (total / f64::from(count)) as f32 }
    }
}

fn lcm(a: usize, b: usize) -> usize {
    fn gcd(a: usize, b: usize) -> usize {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    if a == 0 || b == 0 { a.max(b) } else { a / gcd(a, b) * b }
}

fn summarise(
    histogram: &[u32],
    layout: &ImageLayout,
    cfa: &CfaGrid,
    colour: usize,
) -> Option<ColourStats> {
    let samples: u64 = histogram.iter().map(|count| u64::from(*count)).sum();
    if samples == 0 {
        return None;
    }

    let median = percentile(histogram, samples, 0.5) as f32;
    let black = cfa.black_level(layout, colour);
    let white = layout.white_level.get(colour).copied().filter(|w| w.is_finite()).unwrap_or(f32::NAN);

    let clipped = if white.is_finite() {
        let first = white.max(0.0).ceil() as usize;
        let above: u64 = histogram[first.min(histogram.len())..]
            .iter()
            .map(|count| u64::from(*count))
            .sum();
        above as f32 / samples as f32
    } else {
        f32::NAN
    };

    let level = if black.is_finite() && white.is_finite() && white > black {
        (median - black) / (white - black)
    } else {
        f32::NAN
    };

    Some(ColourStats { samples, median, black, white, level, spread: mad(histogram, samples, median), clipped })
}

/// The value below which `fraction` of the samples lie.
fn percentile(histogram: &[u32], samples: u64, fraction: f64) -> usize {
    let target = (samples as f64 * fraction) as u64;
    let mut seen = 0u64;
    for (value, count) in histogram.iter().enumerate() {
        seen += u64::from(*count);
        if seen > target {
            return value;
        }
    }
    histogram.len().saturating_sub(1)
}

/// Median absolute deviation, computed from the same histogram by walking
/// outwards from the median until half the samples are covered.
fn mad(histogram: &[u32], samples: u64, median: f32) -> f32 {
    let centre = median.round().max(0.0) as usize;
    let half = samples / 2;
    let mut covered = u64::from(histogram.get(centre).copied().unwrap_or(0));
    let mut distance = 1usize;
    while covered < half && distance < histogram.len() {
        if let Some(count) = centre.checked_sub(distance).and_then(|index| histogram.get(index)) {
            covered += u64::from(*count);
        }
        if let Some(count) = histogram.get(centre + distance) {
            covered += u64::from(*count);
        }
        distance += 1;
    }
    distance as f32
}

fn uniformity(blocks: &[u32], black: f32) -> Uniformity {
    let mut medians = [f32::NAN; BLOCKS];
    if !black.is_finite() {
        return Uniformity { blocks: medians };
    }

    for (index, slot) in medians.iter_mut().enumerate() {
        let histogram = &blocks[index * BLOCK_LEVELS..(index + 1) * BLOCK_LEVELS];
        let samples: u64 = histogram.iter().map(|count| u64::from(*count)).sum();
        if samples > 0 {
            let median = ((percentile(histogram, samples, 0.5) as u32) << BLOCK_SHIFT) as f32;
            *slot = median - black;
        }
    }

    let centre = medians[4];
    if centre.is_finite() && centre > 0.0 {
        for value in &mut medians {
            *value /= centre;
        }
    } else {
        // A centre block at or below the black point measures nothing, and
        // dividing by it would manufacture a ratio out of noise.
        medians = [f32::NAN; BLOCKS];
    }
    Uniformity { blocks: medians }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{COLOR_BLUE, COLOR_GREEN, COLOR_RED};

    /// A 2x2 RGGB frame builder: every red photosite gets `r`, every green `g`,
    /// every blue `b`.
    fn frame(width: usize, height: usize, r: u16, g: u16, b: u16) -> (Samples, ImageLayout) {
        let mut layout = ImageLayout {
            width: width as u32,
            height: height as u32,
            components: 1,
            cfa_width: 2,
            cfa_height: 2,
            active_width: width as u32,
            active_height: height as u32,
            black_level_width: 1,
            black_level_height: 1,
            white_level: [16383.0; 4],
            ..Default::default()
        };
        layout.cfa_pattern[..4].copy_from_slice(&[COLOR_RED, COLOR_GREEN, COLOR_GREEN, COLOR_BLUE]);
        layout.black_level[0] = 2048.0;

        let mut values = vec![0u16; width * height];
        for y in 0..height {
            for x in 0..width {
                values[y * width + x] = match (y % 2, x % 2) {
                    (0, 0) => r,
                    (1, 1) => b,
                    _ => g,
                };
            }
        }
        (Samples::U16(values), layout)
    }

    #[test]
    fn each_colour_is_measured_separately() {
        // The reason this module exists: on a Bayer sensor the green population
        // sits at roughly twice red and blue, and a whole-frame median would
        // land between them and describe neither.
        let (samples, layout) = frame(64, 64, 3048, 8048, 4048);
        let stats = measure(&samples, &layout).expect("a describable frame");

        let red = stats.colours[0].expect("red photosites exist");
        let green = stats.colours[1].expect("green photosites exist");
        let blue = stats.colours[2].expect("blue photosites exist");

        assert_eq!(red.median, 3048.0);
        assert_eq!(green.median, 8048.0);
        assert_eq!(blue.median, 4048.0);
        // Green has twice the photosites of either other colour.
        assert_eq!(green.samples, red.samples * 2);
        assert_eq!(stats.dominant().map(|c| c.median), Some(8048.0));
    }

    #[test]
    fn level_is_measured_from_black_to_white_and_not_from_zero() {
        // Half way up a 2048..16383 range is 9215, not 8191.
        let (samples, layout) = frame(64, 64, 9215, 9215, 9215);
        let stats = measure(&samples, &layout).unwrap();
        let level = stats.colours[1].unwrap().level;
        assert!((level - 0.5).abs() < 0.001, "got {level}");
    }

    #[test]
    fn an_unrecorded_level_reports_nan_rather_than_a_stand_in() {
        let (samples, mut layout) = frame(64, 64, 5000, 5000, 5000);
        layout.black_level[0] = f32::NAN;
        let stats = measure(&samples, &layout).unwrap();
        let colour = stats.colours[1].unwrap();
        assert!(colour.level.is_nan(), "no black point means no level, not a guess");
        // The median is still real: it needs no calibration.
        assert_eq!(colour.median, 5000.0);
    }

    #[test]
    fn clipping_is_counted_against_the_white_level() {
        let (samples, layout) = frame(64, 64, 16383, 5000, 5000);
        let stats = measure(&samples, &layout).unwrap();
        // Every red photosite is at the saturation point, and red is a quarter
        // of the frame — but `clipped` is per colour, so it is all of red.
        assert_eq!(stats.colours[0].unwrap().clipped, 1.0);
        assert_eq!(stats.colours[1].unwrap().clipped, 0.0);
    }

    #[test]
    fn an_even_frame_has_no_ramp() {
        let (samples, layout) = frame(96, 96, 5000, 5000, 5000);
        let stats = measure(&samples, &layout).unwrap();
        let ramp = stats.uniformity.ramp();
        assert!((ramp - 1.0).abs() < 0.05, "got {ramp}");
    }

    #[test]
    fn a_tilted_source_shows_as_a_ramp() {
        // What a flat panel that is tilted, or too small for the field, does to
        // a frame — and it would be imprinted on every light divided by it.
        let (width, height) = (96usize, 96usize);
        let (_, layout) = frame(width, height, 0, 0, 0);
        let mut values = vec![0u16; width * height];
        for y in 0..height {
            for x in 0..width {
                // Bright on the left, dim on the right: a 2:1 ramp.
                values[y * width + x] = (8000 - (x * 4000 / width)) as u16;
            }
        }
        let stats = measure(&Samples::U16(values), &layout).unwrap();
        let ramp = stats.uniformity.ramp();
        assert!(ramp > 1.8, "a 2:1 illumination ramp must show at full size, got {ramp}");
        // And it must be left-to-right, not top-to-bottom.
        let blocks = stats.uniformity.blocks;
        assert!(blocks[3] > blocks[5], "left brighter than right: {blocks:?}");
        assert!((blocks[1] - blocks[7]).abs() < 0.05, "top and bottom alike: {blocks:?}");
    }

    #[test]
    fn a_pedestal_does_not_flatten_the_ramp() {
        // The bug this guards: on a flat sitting low above black, the black
        // level is larger than the signal, so a ratio taken on raw values
        // reports a real illumination ramp as a fraction of itself.
        let (width, height) = (96usize, 96usize);
        let (_, layout) = frame(width, height, 0, 0, 0);
        let mut values = vec![0u16; width * height];
        for y in 0..height {
            for x in 0..width {
                // Signal of 400 falling to 200 on a pedestal of 2048: a 2:1
                // ramp that a raw ratio would report as 1.09:1.
                values[y * width + x] = (2048 + 400 - (x * 200 / width)) as u16;
            }
        }
        let stats = measure(&Samples::U16(values), &layout).unwrap();
        let ramp = stats.uniformity.ramp();
        assert!(ramp > 1.5, "the pedestal must be out of the way, got {ramp}");
    }

    #[test]
    fn evenness_is_not_reported_without_a_black_level() {
        // A ratio taken on a pedestal is not an evenness ratio, so there is
        // nothing honest to print.
        let (samples, mut layout) = frame(96, 96, 5000, 5000, 5000);
        layout.black_level[0] = f32::NAN;
        let stats = measure(&samples, &layout).unwrap();
        assert!(stats.uniformity.blocks.iter().all(|value| value.is_nan()));
        assert!(stats.uniformity.ramp().is_nan());
    }

    #[test]
    fn a_fine_map_is_normalised_so_two_can_be_subtracted() {
        let (samples, layout) = frame(96, 96, 5000, 5000, 5000);
        let map = illumination_map(&samples, &layout, 8).expect("a describable frame");
        assert_eq!(map.len(), 64);
        // An even frame is 1.0 everywhere, whatever its absolute level, which is
        // what makes two maps comparable.
        assert!(map.iter().all(|value| (value - 1.0).abs() < 0.01), "{map:?}");

        let (brighter, layout) = frame(96, 96, 9000, 9000, 9000);
        let other = illumination_map(&brighter, &layout, 8).unwrap();
        let worst = map.iter().zip(&other).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst < 0.01, "two even frames must subtract to nothing, worst {worst}");
    }

    #[test]
    fn a_frame_with_no_mosaic_is_not_described() {
        let (samples, mut layout) = frame(64, 64, 5000, 5000, 5000);
        layout.cfa_width = 0;
        layout.cfa_height = 0;
        assert!(measure(&samples, &layout).is_none());
    }

    #[test]
    fn an_active_area_outside_the_frame_is_refused_rather_than_read() {
        let (samples, mut layout) = frame(64, 64, 5000, 5000, 5000);
        layout.active_x = 60;
        layout.active_width = 32;
        assert!(measure(&samples, &layout).is_none());
    }
}
