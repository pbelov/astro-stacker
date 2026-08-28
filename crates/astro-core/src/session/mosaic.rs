//! The colour-filter grid, and the part of a frame that saw light.
//!
//! Shared by everything that walks a frame photosite by photosite. Kept in one
//! place because the two facts it holds — which colour a photosite carries, and
//! where the light-sensitive area starts — are anchored to the sensor origin,
//! and two modules that disagreed about either would silently read every
//! photosite as the wrong colour.

use astro_plugin_abi::abi::{CFA_MAX_CELLS, ImageLayout};

/// A rectangle of photosites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// The colour-filter repeat, anchored to the frame origin.
#[derive(Debug, Clone, Copy)]
pub struct Mosaic {
    pub width: usize,
    pub height: usize,
    pub pattern: [u8; CFA_MAX_CELLS],
    /// How many distinct colour indices the pattern uses, which bounds every
    /// per-colour array.
    pub colours: usize,
    /// The colour with the most cells in the repeat: green on a Bayer sensor.
    pub dominant: usize,
    /// The light-sensitive region, or the whole frame when the plugin did not
    /// say.
    pub area: Area,
}

impl Mosaic {
    /// `None` for a frame this cannot describe: not mosaiced, more than one
    /// component, or an active area that does not lie inside the frame.
    pub fn new(layout: &ImageLayout) -> Option<Self> {
        let (width, height) = (layout.cfa_width as usize, layout.cfa_height as usize);
        if width == 0 || height == 0 || width * height > CFA_MAX_CELLS {
            return None;
        }
        if layout.components != 1 || layout.width == 0 || layout.height == 0 {
            return None;
        }

        let mut counts = [0usize; 4];
        for &colour in &layout.pattern_cells() {
            *counts.get_mut(colour as usize)? += 1;
        }
        let dominant = counts.iter().enumerate().max_by_key(|(_, count)| **count)?.0;
        let colours = counts.iter().filter(|count| **count > 0).count().max(1);

        Some(Self {
            width,
            height,
            pattern: layout.cfa_pattern,
            // Colour indices are dense from zero on every sensor this project
            // has met, so the count doubles as the bound.
            colours: counts.iter().rposition(|count| *count > 0).map_or(colours, |last| last + 1),
            dominant,
            area: active_area(layout)?,
        })
    }

    /// The colour-filter index at a photosite.
    pub fn colour_at(&self, x: usize, y: usize) -> usize {
        self.pattern[(y % self.height) * self.width + (x % self.width)] as usize
    }
}

/// The light-sensitive region, or the whole frame when the plugin did not say.
///
/// `None` when the area does not fit inside the frame, rather than clamping: a
/// plugin that reports an impossible rectangle has said something wrong, and
/// silently correcting it would read past the buffer or, worse, not.
pub fn active_area(layout: &ImageLayout) -> Option<Area> {
    let (width, height) = (layout.width as usize, layout.height as usize);
    if width == 0 || height == 0 {
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

/// The lowest common multiple, for walking two grids that are both anchored to
/// the frame origin but need not be the same size.
pub fn lcm(a: usize, b: usize) -> usize {
    fn gcd(a: usize, b: usize) -> usize {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    if a == 0 || b == 0 { a.max(b) } else { a / gcd(a, b) * b }
}

/// The cells of the colour-filter repeat that mean anything.
trait PatternCells {
    fn pattern_cells(&self) -> Vec<u8>;
}

impl PatternCells for ImageLayout {
    fn pattern_cells(&self) -> Vec<u8> {
        let cells = (self.cfa_width as usize) * (self.cfa_height as usize);
        self.cfa_pattern[..cells.min(CFA_MAX_CELLS)].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use astro_plugin_abi::abi::{COLOR_BLUE, COLOR_GREEN, COLOR_RED};

    fn gbrg() -> ImageLayout {
        // The reference body's pattern, which is not the RGGB most code assumes.
        let mut layout = ImageLayout {
            width: 5344,
            height: 3516,
            components: 1,
            cfa_width: 2,
            cfa_height: 2,
            active_x: 142,
            active_y: 51,
            active_width: 5202,
            active_height: 3465,
            ..Default::default()
        };
        layout.cfa_pattern[..4].copy_from_slice(&[COLOR_GREEN, COLOR_BLUE, COLOR_RED, COLOR_GREEN]);
        layout
    }

    #[test]
    fn a_gbrg_sensor_reads_as_gbrg() {
        let mosaic = Mosaic::new(&gbrg()).expect("a describable frame");
        assert_eq!(mosaic.colour_at(0, 0), COLOR_GREEN as usize);
        assert_eq!(mosaic.colour_at(1, 0), COLOR_BLUE as usize);
        assert_eq!(mosaic.colour_at(0, 1), COLOR_RED as usize);
        assert_eq!(mosaic.colour_at(1, 1), COLOR_GREEN as usize);
        // Green has two cells of the four, so it is the dominant colour.
        assert_eq!(mosaic.dominant, COLOR_GREEN as usize);
        assert_eq!(mosaic.colours, 3);
    }

    #[test]
    fn the_pattern_repeats_from_the_frame_origin_not_the_active_area() {
        // The active area starts at 142,51 - an even column and an odd row - so
        // a walk that anchored the mosaic to it would read every photosite as
        // the wrong colour on half the frame.
        let mosaic = Mosaic::new(&gbrg()).unwrap();
        assert_eq!(mosaic.colour_at(142, 51), mosaic.colour_at(0, 1));
        assert_eq!(mosaic.colour_at(143, 51), mosaic.colour_at(1, 1));
    }

    #[test]
    fn an_impossible_active_area_is_refused_rather_than_clamped() {
        let mut layout = gbrg();
        layout.active_x = 5000;
        layout.active_width = 5202;
        assert!(Mosaic::new(&layout).is_none());
    }

    #[test]
    fn a_frame_with_no_mosaic_is_not_described() {
        let mut layout = gbrg();
        layout.cfa_width = 0;
        assert!(Mosaic::new(&layout).is_none());
    }
}
