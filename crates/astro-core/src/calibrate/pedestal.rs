//! Where a frame's zero point comes from.
//!
//! Every calibration frame sits on a pedestal — the amplifier offset the sensor
//! reads with no light at all — and it has to come off before a flat can be
//! normalised or a dark scaled. There are three places to get it, and they are
//! not equal evidence.
//!
//! **A master bias at the same gain** is the best: it is per-photosite, so it
//! carries the column-to-column structure a scalar cannot.
//!
//! **The frame's own optically-masked columns** are the next best, and are what
//! this project reaches for when the bias is at another gain. They are measured
//! from the same exposure, at the same gain, on the same body, minutes apart —
//! and they cost nothing, because every frame carries them. What they give up
//! is the per-photosite pattern: a scalar per colour-filter cell, no more.
//!
//! **The black level the plugin reports** is deliberately not on that list for
//! calibration. It is four numbers Canon writes per frame, and measured against
//! an eighty-five frame master bias on this project's reference body they were
//! wrong by -15 to +4 ADU depending on the mosaic cell — a difference *between*
//! cells, which after normalisation is a colour cast rather than an offset. It
//! is right for a report and wrong for arithmetic.

use astro_plugin_abi::abi::{CFA_MAX_CELLS, ImageLayout};

/// Columns at the very edge of the sensor read a little high on some bodies.
const HEAD_GUARD: usize = 8;

/// Charge from the light-sensitive area spreads into the columns beside it, so
/// the last of the mask is not dark.
const TAIL_GUARD: usize = 12;

/// The fewest masked columns worth measuring from. Below this the estimate is
/// noisier than the thing it is meant to remove.
const MIN_COLUMNS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PedestalSource {
    /// The frame's own optically-masked columns: same gain, same exposure, same
    /// body, but a scalar per mosaic cell rather than a pattern.
    MaskedColumns,
    /// A master bias at the same gain: per-photosite, and the better answer
    /// where one exists.
    MasterBias,
}

impl PedestalSource {
    pub fn name(self) -> &'static str {
        match self {
            Self::MaskedColumns => "the frame's own masked columns",
            Self::MasterBias => "a master bias at the same gain",
        }
    }
}

/// A zero point, per colour-filter cell, and where it came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pedestal {
    pub source: PedestalSource,
    /// Row-major over the CFA grid, aligned to the frame origin. Only the first
    /// `cfa_width * cfa_height` entries mean anything.
    pub cells: [f32; CFA_MAX_CELLS],
    pub cfa_width: u32,
    pub cfa_height: u32,
    /// How many photosites the estimate came from, so a caller can tell a
    /// measurement from a guess.
    pub samples: u64,
}

impl Pedestal {
    /// The value to subtract at a photosite, indexed the way the frame is.
    pub fn at(&self, x: usize, y: usize) -> f32 {
        let width = self.cfa_width as usize;
        let height = self.cfa_height as usize;
        if width == 0 || height == 0 {
            return f32::NAN;
        }
        self.cells[(y % height) * width + (x % width)]
    }

    /// The mean over the cells, for a report. Never for arithmetic: the cells
    /// differ, and averaging them is what turns an offset into a colour cast.
    pub fn mean(&self) -> f32 {
        let cells = (self.cfa_width as usize) * (self.cfa_height as usize);
        if cells == 0 {
            return f32::NAN;
        }
        self.cells[..cells].iter().sum::<f32>() / cells as f32
    }
}

/// Measures the pedestal from the optically-masked columns to the left of the
/// active area.
///
/// The columns rather than the rows above it. On this project's reference body
/// the masked rows read ten to twenty ADU below the masked columns — they are
/// not a black reference there, whatever they are — and a pedestal taken from
/// them would over-subtract by that much on every frame.
///
/// `None` when the frame has no usable mask, which is honest: a plugin that
/// reports no active area, or one flush against the left edge, has not given
/// this anything to measure.
pub fn from_masked_columns(pixels: &[f32], layout: &ImageLayout) -> Option<Pedestal> {
    let width = layout.width as usize;
    let height = layout.height as usize;
    let (cfa_width, cfa_height) = (layout.cfa_width as usize, layout.cfa_height as usize);
    if width == 0 || height == 0 || pixels.len() != width * height {
        return None;
    }
    if cfa_width == 0 || cfa_height == 0 || cfa_width * cfa_height > CFA_MAX_CELLS {
        return None;
    }

    let mask_end = (layout.active_x as usize).min(width);
    let start = HEAD_GUARD;
    let end = mask_end.saturating_sub(TAIL_GUARD);
    if end <= start || end - start < MIN_COLUMNS {
        return None;
    }

    let mut totals = [0f64; CFA_MAX_CELLS];
    let mut counts = [0u64; CFA_MAX_CELLS];
    for y in 0..height {
        let row = y * width;
        let cell_row = (y % cfa_height) * cfa_width;
        let mut cell_col = start % cfa_width;
        for x in start..end {
            let cell = cell_row + cell_col;
            totals[cell] += f64::from(pixels[row + x]);
            counts[cell] += 1;
            cell_col += 1;
            if cell_col == cfa_width {
                cell_col = 0;
            }
        }
    }

    let mut cells = [f32::NAN; CFA_MAX_CELLS];
    let mut samples = 0u64;
    for cell in 0..cfa_width * cfa_height {
        if counts[cell] == 0 {
            return None;
        }
        cells[cell] = (totals[cell] / counts[cell] as f64) as f32;
        samples += counts[cell];
    }

    Some(Pedestal {
        source: PedestalSource::MaskedColumns,
        cells,
        cfa_width: layout.cfa_width,
        cfa_height: layout.cfa_height,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use astro_plugin_abi::abi::{COLOR_BLUE, COLOR_GREEN, COLOR_RED};

    /// A frame with a masked strip on the left whose four mosaic cells sit at
    /// four different levels, which is what a real sensor does.
    fn masked(width: usize, height: usize, active_x: u32, cells: [f32; 4]) -> (Vec<f32>, ImageLayout) {
        let mut layout = ImageLayout {
            width: width as u32,
            height: height as u32,
            components: 1,
            cfa_width: 2,
            cfa_height: 2,
            active_x,
            active_y: 0,
            active_width: width as u32 - active_x,
            active_height: height as u32,
            ..Default::default()
        };
        layout.cfa_pattern[..4].copy_from_slice(&[COLOR_RED, COLOR_GREEN, COLOR_GREEN, COLOR_BLUE]);

        let mut pixels = vec![0f32; width * height];
        for y in 0..height {
            for x in 0..width {
                pixels[y * width + x] = if x < active_x as usize {
                    cells[(y % 2) * 2 + (x % 2)]
                } else {
                    // The lit part, far above any pedestal.
                    9000.0
                };
            }
        }
        (pixels, layout)
    }

    #[test]
    fn each_mosaic_cell_gets_its_own_zero_point() {
        // The whole reason this is not one number: on a real body the cells of
        // one frame differed by twenty ADU, and averaging them turns an offset
        // into a colour cast.
        let (pixels, layout) = masked(200, 64, 142, [2038.0, 2039.0, 2057.0, 2058.0]);
        let pedestal = from_masked_columns(&pixels, &layout).expect("a usable mask");

        assert_eq!(pedestal.source, PedestalSource::MaskedColumns);
        assert!((pedestal.at(0, 0) - 2038.0).abs() < 0.01);
        assert!((pedestal.at(1, 0) - 2039.0).abs() < 0.01);
        assert!((pedestal.at(0, 1) - 2057.0).abs() < 0.01);
        assert!((pedestal.at(1, 1) - 2058.0).abs() < 0.01);
        assert!((pedestal.mean() - 2048.0).abs() < 0.01);
    }

    #[test]
    fn the_lit_area_never_reaches_the_estimate() {
        // The guard columns exist because charge spreads out of the active area
        // into the mask beside it. A pedestal contaminated by light is worse
        // than none: it over-subtracts everywhere.
        let (pixels, layout) = masked(200, 64, 142, [2048.0; 4]);
        let pedestal = from_masked_columns(&pixels, &layout).unwrap();
        assert!((pedestal.mean() - 2048.0).abs() < 0.01, "got {}", pedestal.mean());
    }

    #[test]
    fn a_frame_with_no_mask_measures_nothing_rather_than_guessing() {
        // A plugin that reports the active area flush against the left edge has
        // given this nothing, and the honest answer is that there is no
        // pedestal here — not a plausible 2048.
        let (pixels, layout) = masked(200, 64, 0, [2048.0; 4]);
        assert!(from_masked_columns(&pixels, &layout).is_none());

        // And a mask too narrow to measure from is the same answer.
        let (pixels, layout) = masked(200, 64, 24, [2048.0; 4]);
        assert!(from_masked_columns(&pixels, &layout).is_none());
    }

    #[test]
    fn a_real_canon_mask_is_wide_enough() {
        // The reference body: 142 masked columns before the active area, of
        // which the guards leave 122.
        let (pixels, layout) = masked(5344, 8, 142, [2038.0, 2039.0, 2057.0, 2058.0]);
        let pedestal = from_masked_columns(&pixels, &layout).expect("142 columns is plenty");
        assert_eq!(pedestal.samples, (142 - HEAD_GUARD - TAIL_GUARD) as u64 * 8);
    }
}
