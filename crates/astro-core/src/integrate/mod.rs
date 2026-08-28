//! Combining registered frames onto one grid.
//!
//! Three decisions here were settled by measurement and by adversarial review,
//! and each of them rules out something that looks obviously right.
//!
//! **The mosaic is never interpolated.** Each photosite is deposited into the
//! plane of its own colour and nowhere else. The temptation is to resample the
//! frame bilinearly and combine the results, but on an undemosaiced frame the
//! neighbours of a red photosite are green, so that kernel produces a number
//! that is not a measurement of anything. Doing it per colour instead does not
//! rescue it: same-colour samples sit two photosites apart, and this session's
//! stars are 2.07 photosites across, so each colour plane is sampled at about
//! half the Nyquist rate and interpolating it interpolates the aliases. What
//! makes depositing work here is the drift: the field walks 3913 photosites
//! over the run, so the sub-pixel phases are effectively uniform and 225 frames
//! fill every output cell of every colour many times over.
//!
//! **Frames are brought onto a common photometric scale before they are
//! combined, and the model is affine.** Transparency multiplies everything that
//! came through the sky; airglow, moonlight and light pollution *add* to it, and
//! the two are unrelated — thin cirrus dims the stars while raising the sky. So
//! a frame is `scale * (pixel - sky)`, not `scale * pixel`, and the scale must
//! be measured from stars rather than from the sky, or a hazy frame would be
//! scaled the wrong way.
//!
//! **The scale enters the weight as its square.** Multiplying a frame by
//! `scale` multiplies its noise by `scale` too, so the inverse-variance weight
//! carries `1/(scale * sigma)^2`. Leaving the scale out of the weight is the
//! quiet failure: a hazy frame is multiplied up to full brightness, its noise is
//! amplified with it, and it is then weighted as though it had been clear —
//! which can make the result noisier than an unweighted mean of the good frames
//! alone. The invariance test at the bottom of this file exists for that one
//! mistake: the stack must not change when a frame is given an arbitrary gain,
//! because a gain of `g` sends `scale -> scale/g` and `sigma -> g*sigma`,
//! leaving `scale * sigma` and therefore the weight untouched.

use astro_plugin_abi::abi::ImageLayout;
use rayon::prelude::*;

use crate::register::Transform;
use crate::session::mosaic::Mosaic;
use crate::stars::Sky;

/// The output grid: the reference frame's coordinates, shifted so that every
/// contributing frame lands at a non-negative position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    /// Added to reference coordinates to reach output coordinates.
    pub origin_x: f64,
    pub origin_y: f64,
}

impl Canvas {
    /// The smallest grid holding every frame's active area.
    ///
    /// `None` when no frame was offered, or when the union comes out larger than
    /// `limit` pixels — a runaway transform should be refused rather than
    /// allocating for it.
    pub fn covering<'a>(
        frames: impl Iterator<Item = (&'a ImageLayout, Transform)>,
        limit: usize,
    ) -> Option<Canvas> {
        let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
        let mut any = false;

        for (layout, transform) in frames {
            let Some(mosaic) = Mosaic::new(layout) else { continue };
            let area = mosaic.area;
            let (x0, y0) = (area.x as f64, area.y as f64);
            let (x1, y1) = ((area.x + area.width - 1) as f64, (area.y + area.height - 1) as f64);
            // All four corners: with a rotation the axis-aligned box of the
            // transformed corners is not the transform of the box.
            for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
                let (u, v) = transform.apply(x, y);
                if !u.is_finite() || !v.is_finite() {
                    return None;
                }
                min_x = min_x.min(u);
                max_x = max_x.max(u);
                min_y = min_y.min(v);
                max_y = max_y.max(v);
                any = true;
            }
        }
        if !any {
            return None;
        }

        // Half a pixel each side, because a photosite's drop is centred on its
        // position and reaches beyond it.
        let width = (max_x - min_x).ceil() as usize + 2;
        let height = (max_y - min_y).ceil() as usize + 2;
        if width == 0 || height == 0 || width.checked_mul(height)? > limit {
            return None;
        }
        Some(Canvas { width, height, origin_x: 1.0 - min_x, origin_y: 1.0 - min_y })
    }

    fn place(&self, transform: &Transform, x: f64, y: f64) -> (f64, f64) {
        let (u, v) = transform.apply(x, y);
        (u + self.origin_x, v + self.origin_y)
    }
}

/// One frame, ready to be added.
pub struct Contribution<'a> {
    pub pixels: &'a [f32],
    pub layout: &'a ImageLayout,
    pub mosaic: &'a Mosaic,
    /// The frame's own background, subtracted before anything else. A plane
    /// rather than the full grid: a plane cannot eat real structure, and a
    /// background grid fine enough to follow the sky is also fine enough to
    /// follow a nebula.
    pub background: Background,
    /// Multiplies the background-subtracted value, bringing this frame onto the
    /// run's common brightness.
    pub scale: f64,
    /// How much this frame counts, already including `1/(scale * sigma)^2`.
    pub weight: f64,
    pub transform: Transform,
}

/// A per-colour linear background: `level = a*x + b*y + c`.
#[derive(Debug, Clone, Default)]
pub struct Background {
    pub planes: Vec<[f64; 3]>,
}

impl Background {
    /// Fits one plane per colour to a measured sky grid, by least squares over
    /// the cell medians.
    ///
    /// Least squares over robust cell medians rather than a robust fit over raw
    /// photosites: each cell median already ignores the stars inside it, and a
    /// thousand cells is few enough that the normal equations are exact and
    /// instant.
    pub fn fit(sky: &Sky, mosaic: &Mosaic) -> Background {
        let mut planes = Vec::with_capacity(mosaic.colours);
        for colour in 0..mosaic.colours {
            // Sums for the 3x3 normal equations of z = a*x + b*y + c.
            let (mut n, mut sx, mut sy, mut sz) = (0f64, 0f64, 0f64, 0f64);
            let (mut sxx, mut syy, mut sxy, mut sxz, mut syz) = (0f64, 0f64, 0f64, 0f64, 0f64);
            for cell_y in 0..sky.cells_y {
                for cell_x in 0..sky.cells_x {
                    let (x, y) = (
                        (cell_x * crate::stars::sky::CELL + crate::stars::sky::CELL / 2) as f64,
                        (cell_y * crate::stars::sky::CELL + crate::stars::sky::CELL / 2) as f64,
                    );
                    let z = f64::from(sky.level_at(x as usize, y as usize, colour));
                    if !z.is_finite() {
                        continue;
                    }
                    n += 1.0;
                    sx += x;
                    sy += y;
                    sz += z;
                    sxx += x * x;
                    syy += y * y;
                    sxy += x * y;
                    sxz += x * z;
                    syz += y * z;
                }
            }
            planes.push(solve3(
                [[sxx, sxy, sx], [sxy, syy, sy], [sx, sy, n]],
                [sxz, syz, sz],
            ));
        }
        Background { planes }
    }

    fn at(&self, colour: usize, x: f64, y: f64) -> f64 {
        match self.planes.get(colour) {
            Some([a, b, c]) => a * x + b * y + c,
            // A colour with no fitted plane has no background, and subtracting
            // zero would be a guess. NaN removes the photosite instead.
            None => f64::NAN,
        }
    }

    /// The background at the middle of a frame, which is the level to put back
    /// so the stack reads like an exposure rather than like a difference.
    pub fn centre(&self, colour: usize, layout: &ImageLayout) -> f64 {
        self.at(colour, f64::from(layout.width) / 2.0, f64::from(layout.height) / 2.0)
    }
}

/// Cramer's rule on a 3x3. Returns a flat plane through the mean when the system
/// is singular, which happens only for a grid of one cell.
fn solve3(m: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    let det = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let base = det(m);
    if !base.is_finite() || base.abs() < 1e-9 {
        let n = m[2][2];
        return [0.0, 0.0, if n > 0.0 { v[2] / n } else { f64::NAN }];
    }
    let mut out = [0f64; 3];
    for column in 0..3 {
        let mut swapped = m;
        for row in 0..3 {
            swapped[row][column] = v[row];
        }
        out[column] = det(swapped) / base;
    }
    out
}

/// The running weighted sum, one cell per output pixel per colour.
pub struct Stack {
    pub canvas: Canvas,
    pub colours: usize,
    /// `[((y * width + x) * colours + c) * 2 + {0: sum,1: weight}]`, interleaved
    /// so a band of output rows is one contiguous slice and rayon can own it
    /// without atomics.
    cells: Vec<f32>,
    frames: usize,
}

/// How wide the drop is, as a fraction of a photosite.
///
/// One means each photosite is spread over a full output cell, which costs a
/// box convolution of variance 1/12 per axis — on this session's 0.879-photosite
/// cross-trail sigma that is a five per cent widening. Smaller is sharper and
/// thinner: at `p` each output cell of the rarest colour collects about
/// `frames * p^2 / 4` contributions, so 225 frames give 56 at one and 14 at a
/// half, and below that the coverage starts to come out in holes.
pub const DEFAULT_PIXFRAC: f64 = 1.0;

impl Stack {
    pub fn new(canvas: Canvas, colours: usize) -> Stack {
        Stack { canvas, colours, cells: vec![0.0; canvas.width * canvas.height * colours * 2], frames: 0 }
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Deposits one frame.
    pub fn add(&mut self, frame: &Contribution, pixfrac: f64) {
        let pixfrac = pixfrac.clamp(0.05, 2.0);
        let half = pixfrac / 2.0;
        let width = frame.layout.width as usize;
        let area = frame.mosaic.area;
        let colours = self.colours;
        let canvas = self.canvas;
        let row_cells = canvas.width * colours * 2;

        // The transform is a near-identity similarity, so an output row is
        // reached by a narrow band of input rows. Each band recomputes the
        // photosites near its edges and writes only the cells it owns, which is
        // what keeps two threads off one cell without atomics.
        let shear = frame.transform.b.abs() * f64::from(frame.layout.width) + half + 1.0;
        let scale_y = if frame.transform.a.abs() > 1e-6 { frame.transform.a } else { 1.0 };
        let rows_per_band = (canvas.height / (rayon::current_num_threads() * 4).max(1)).max(64);

        self.cells
            .par_chunks_mut(row_cells * rows_per_band)
            .enumerate()
            .for_each(|(band, cells)| {
                let first_row = band * rows_per_band;
                let rows = cells.len() / row_cells;
                let (v_lo, v_hi) = (first_row as f64 - 0.5, (first_row + rows) as f64 - 0.5);

                // Invert v = b*x + a*y + ty + origin_y for the widest x.
                let base = frame.transform.ty + canvas.origin_y;
                let mut y_lo = ((v_lo - base - shear) / scale_y).floor();
                let mut y_hi = ((v_hi - base + shear) / scale_y).ceil();
                if scale_y < 0.0 {
                    std::mem::swap(&mut y_lo, &mut y_hi);
                }
                let y_start = y_lo.max(area.y as f64) as usize;
                let y_end = (y_hi.min((area.y + area.height) as f64).max(0.0)) as usize;

                for y in y_start..y_end {
                    let row = y * width;
                    for x in area.x..area.x + area.width {
                        let colour = frame.mosaic.colour_at(x, y);
                        if colour >= colours {
                            continue;
                        }
                        let raw = f64::from(frame.pixels[row + x]);
                        let value = (raw - frame.background.at(colour, x as f64, y as f64))
                            * frame.scale;
                        if !value.is_finite() {
                            continue;
                        }
                        let (u, v) = canvas.place(&frame.transform, x as f64, y as f64);

                        // The drop spans [u-half, u+half]; an output cell spans
                        // half a pixel each side of its own centre.
                        let i0 = (u - half + 0.5).floor() as i64;
                        let i1 = (u + half + 0.5).ceil() as i64;
                        let j0 = (v - half + 0.5).floor() as i64;
                        let j1 = (v + half + 0.5).ceil() as i64;

                        for j in j0..j1 {
                            if j < first_row as i64 || j >= (first_row + rows) as i64 {
                                continue;
                            }
                            let overlap_y = span(v - half, v + half, j as f64);
                            if overlap_y <= 0.0 {
                                continue;
                            }
                            for i in i0..i1 {
                                if i < 0 || i >= canvas.width as i64 {
                                    continue;
                                }
                                let overlap = overlap_y * span(u - half, u + half, i as f64);
                                if overlap <= 0.0 {
                                    continue;
                                }
                                // The drop area cancels between sum and weight,
                                // so it is left out of both.
                                let share = overlap * frame.weight;
                                let local = (j as usize - first_row) * row_cells
                                    + (i as usize * colours + colour) * 2;
                                cells[local] += (share * value) as f32;
                                cells[local + 1] += share as f32;
                            }
                        }
                    }
                }
            });

        self.frames += 1;
    }

    /// One plane per colour: the weighted mean, plus `pedestal`, and NaN where
    /// no frame reached.
    pub fn finish(&self, pedestal: &[f64]) -> Vec<Vec<f32>> {
        (0..self.colours)
            .map(|colour| {
                let back = pedestal.get(colour).copied().unwrap_or(0.0) as f32;
                (0..self.canvas.width * self.canvas.height)
                    .map(|pixel| {
                        let at = (pixel * self.colours + colour) * 2;
                        let weight = self.cells[at + 1];
                        // No frame reached here. NaN and not zero: a corner the
                        // run never covered is not a corner that was black.
                        if weight > 0.0 { self.cells[at] / weight + back } else { f32::NAN }
                    })
                    .collect()
            })
            .collect()
    }

    /// The total weight at each output pixel of one colour, which is how deep
    /// the stack is there.
    pub fn coverage(&self, colour: usize) -> Vec<f32> {
        (0..self.canvas.width * self.canvas.height)
            .map(|pixel| self.cells[(pixel * self.colours + colour) * 2 + 1])
            .collect()
    }
}

/// How much of `[lo, hi]` falls inside the output cell centred on `centre`.
fn span(lo: f64, hi: f64, centre: f64) -> f64 {
    (hi.min(centre + 0.5) - lo.max(centre - 0.5)).max(0.0)
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
            ..Default::default()
        };
        layout.cfa_pattern[..4].copy_from_slice(&[COLOR_RED, COLOR_GREEN, COLOR_GREEN, COLOR_BLUE]);
        layout
    }

    fn flat(colours: usize, level: f64) -> Background {
        Background { planes: vec![[0.0, 0.0, level]; colours] }
    }

    /// One frame of a constant scene, one value per colour.
    fn painted(layout: &ImageLayout, mosaic: &Mosaic, per_colour: &[f32]) -> Vec<f32> {
        let width = layout.width as usize;
        (0..width * layout.height as usize)
            .map(|index| per_colour[mosaic.colour_at(index % width, index / width)])
            .collect()
    }

    fn one(layout: &ImageLayout) -> Canvas {
        Canvas::covering(std::iter::once((layout, Transform::IDENTITY)), 1 << 24).unwrap()
    }

    #[test]
    fn a_constant_scene_comes_back_as_itself() {
        let layout = layout(64, 48);
        let mosaic = Mosaic::new(&layout).unwrap();
        let pixels = painted(&layout, &mosaic, &[1000.0, 2000.0, 3000.0]);

        let canvas = one(&layout);
        let mut stack = Stack::new(canvas, mosaic.colours);
        stack.add(
            &Contribution {
                pixels: &pixels,
                layout: &layout,
                mosaic: &mosaic,
                background: flat(mosaic.colours, 0.0),
                scale: 1.0,
                weight: 1.0,
                transform: Transform::IDENTITY,
            },
            DEFAULT_PIXFRAC,
        );

        // Every cell a photosite of that colour reached, and no other: one
        // undithered frame fills a quarter of the red plane and half the green,
        // and the rest is honestly empty rather than interpolated.
        let planes = stack.finish(&[0.0, 0.0, 0.0]);
        for (colour, expected) in [1000.0f32, 2000.0, 3000.0].iter().enumerate() {
            let filled: Vec<f32> =
                planes[colour].iter().copied().filter(|value| value.is_finite()).collect();
            assert!(!filled.is_empty(), "colour {colour} reached nothing");
            for value in &filled {
                assert!((value - expected).abs() < 0.5, "colour {colour} came back as {value}");
            }
            let share = filled.len() as f64 / planes[colour].len() as f64;
            let wanted = if colour == 1 { 0.5 } else { 0.25 };
            assert!(
                (share - wanted).abs() < 0.08,
                "colour {colour} filled {share:.2} of the plane, not {wanted}"
            );
        }
    }

    #[test]
    fn a_photosite_never_reaches_another_plane() {
        // The failure that makes an undemosaiced stack meaningless. A bilinear
        // resample of a mosaic reads a red photosite's green neighbours, and the
        // number it produces measures nothing at all.
        let layout = layout(64, 48);
        let mosaic = Mosaic::new(&layout).unwrap();
        let pixels = painted(&layout, &mosaic, &[5000.0, 0.0, 0.0]);

        let canvas = one(&layout);
        let mut stack = Stack::new(canvas, mosaic.colours);
        stack.add(
            &Contribution {
                pixels: &pixels,
                layout: &layout,
                mosaic: &mosaic,
                background: flat(mosaic.colours, 0.0),
                scale: 1.0,
                weight: 1.0,
                transform: Transform { a: 1.0, b: 0.0, tx: 0.37, ty: -0.61 },
            },
            DEFAULT_PIXFRAC,
        );

        let planes = stack.finish(&[0.0, 0.0, 0.0]);
        for colour in [1usize, 2] {
            for value in &planes[colour] {
                assert!(
                    value.is_nan() || value.abs() < 1e-3,
                    "red leaked into plane {colour}: {value}"
                );
            }
        }
    }

    #[test]
    fn a_frame_given_an_arbitrary_gain_changes_nothing() {
        // The one mistake this module exists to prevent. A hazy frame is scaled
        // up, which amplifies its noise with it, so the weight must carry
        // 1/(scale*sigma)^2 -- and under a gain g the scale goes to scale/g and
        // sigma to g*sigma, leaving scale*sigma and so the weight untouched. An
        // implementation that left the scale out of the weight passes every
        // other test here and fails this one.
        let layout = layout(48, 40);
        let mosaic = Mosaic::new(&layout).unwrap();
        let scene = painted(&layout, &mosaic, &[1000.0, 2000.0, 3000.0]);
        let sky = 300.0f64;
        let sigma = 40.0f64;

        let stacked = |gains: [f64; 3]| {
            let canvas = one(&layout);
            let mut stack = Stack::new(canvas, mosaic.colours);
            for (index, gain) in gains.iter().enumerate() {
                // What the camera would have recorded through that gain.
                let observed: Vec<f32> =
                    scene.iter().map(|value| ((f64::from(*value) + sky) * gain) as f32).collect();
                let scale = 1.0 / gain;
                let weight = 1.0 / (scale * (sigma * gain)).powi(2);
                stack.add(
                    &Contribution {
                        pixels: &observed,
                        layout: &layout,
                        mosaic: &mosaic,
                        background: flat(mosaic.colours, sky * gain),
                        scale,
                        weight,
                        transform: Transform {
                            a: 1.0,
                            b: 0.0,
                            tx: index as f64 * 0.31,
                            ty: index as f64 * -0.17,
                        },
                    },
                    DEFAULT_PIXFRAC,
                );
            }
            stack.finish(&[0.0, 0.0, 0.0])
        };

        let even = stacked([1.0, 1.0, 1.0]);
        let uneven = stacked([1.0, 0.4, 2.5]);
        let canvas = one(&layout);
        let mut compared = 0;
        for pixel in 0..canvas.width * canvas.height {
            for colour in 0..mosaic.colours {
                let (a, b) = (even[colour][pixel], uneven[colour][pixel]);
                if a.is_nan() || b.is_nan() {
                    continue;
                }
                assert!((a - b).abs() < 0.5, "a gain changed the stack: {a} against {b}");
                compared += 1;
            }
        }
        assert!(compared > 1000, "the test compared almost nothing: {compared}");
    }

    #[test]
    fn a_pixel_no_frame_reached_is_nan_and_not_zero() {
        // A corner the run never covered is not a corner that was black, and a
        // stack that said zero there would invite the next stage to average it
        // in as though it were a measurement.
        let layout = layout(32, 24);
        let mosaic = Mosaic::new(&layout).unwrap();
        let pixels = painted(&layout, &mosaic, &[1000.0, 1000.0, 1000.0]);

        let far = Transform { a: 1.0, b: 0.0, tx: 40.0, ty: 30.0 };
        let canvas =
            Canvas::covering([(&layout, Transform::IDENTITY), (&layout, far)].into_iter(), 1 << 24)
                .unwrap();
        let mut stack = Stack::new(canvas, mosaic.colours);
        for transform in [Transform::IDENTITY, far] {
            stack.add(
                &Contribution {
                    pixels: &pixels,
                    layout: &layout,
                    mosaic: &mosaic,
                    background: flat(mosaic.colours, 0.0),
                    scale: 1.0,
                    weight: 1.0,
                    transform,
                },
                DEFAULT_PIXFRAC,
            );
        }

        let planes = stack.finish(&[0.0, 0.0, 0.0]);
        let uncovered = planes[mosaic.dominant].iter().filter(|value| value.is_nan()).count();
        assert!(uncovered > 100, "two offset frames must leave corners empty: {uncovered}");
        assert!(
            planes[mosaic.dominant].iter().any(|value| value.is_finite()),
            "and must cover the middle"
        );
    }

    #[test]
    fn the_bands_the_work_is_split_over_leave_no_seam() {
        // The failure the obvious test is blind to. A deposit is split across
        // output rows, and a row on the boundary between two rayon bands can
        // lose the half belonging to its neighbour. Sum and weight lose it
        // together, so the pixel VALUE is still exactly right and only the depth
        // is halved -- a periodic banding artefact that would be blamed on the
        // sensor.
        let layout = layout(96, 600);
        let mosaic = Mosaic::new(&layout).unwrap();
        let pixels = painted(&layout, &mosaic, &[1000.0, 1000.0, 1000.0]);

        let canvas = one(&layout);
        let mut stack = Stack::new(canvas, mosaic.colours);
        // Half a photosite each way, so every drop straddles two output rows
        // and two output columns.
        stack.add(
            &Contribution {
                pixels: &pixels,
                layout: &layout,
                mosaic: &mosaic,
                background: flat(mosaic.colours, 0.0),
                scale: 1.0,
                weight: 1.0,
                transform: Transform { a: 1.0, b: 0.0, tx: 0.5, ty: 0.5 },
            },
            DEFAULT_PIXFRAC,
        );

        let coverage = stack.coverage(mosaic.dominant);
        let rows: Vec<f32> = (4..canvas.height - 4)
            .map(|y| {
                coverage[y * canvas.width + 8..y * canvas.width + canvas.width - 8].iter().sum()
            })
            .collect();
        let high = rows.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let low = rows.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(
            low > 0.0 && (high - low) / high < 0.02,
            "the depth must not dip at a band edge: {low} to {high}"
        );
    }

    #[test]
    fn dithered_frames_fill_every_cell_of_every_colour() {
        // What makes depositing on a mosaic work at all. A red photosite covers
        // one output cell in four, so a single frame leaves three quarters of
        // the red plane empty; only the run's own drift fills them.
        let layout = layout(64, 64);
        let mosaic = Mosaic::new(&layout).unwrap();
        let pixels = painted(&layout, &mosaic, &[1000.0, 1000.0, 1000.0]);

        let frames: Vec<Transform> = (0..24)
            .map(|i| Transform {
                a: 1.0,
                b: 0.0,
                tx: f64::from(i % 5) * 0.83 - 1.6,
                ty: f64::from(i / 5) * 0.71 - 1.4,
            })
            .collect();
        let canvas = Canvas::covering(frames.iter().map(|t| (&layout, *t)), 1 << 24).unwrap();

        let mut alone = Stack::new(canvas, mosaic.colours);
        let mut together = Stack::new(canvas, mosaic.colours);
        for (index, transform) in frames.iter().enumerate() {
            let contribution = Contribution {
                pixels: &pixels,
                layout: &layout,
                mosaic: &mosaic,
                background: flat(mosaic.colours, 0.0),
                scale: 1.0,
                weight: 1.0,
                transform: *transform,
            };
            if index == 0 {
                alone.add(&contribution, 0.4);
            }
            together.add(&contribution, 0.4);
        }

        // A window well inside every frame, so this is about the mosaic and not
        // about the edges.
        let empty = |stack: &Stack| {
            let planes = stack.finish(&[0.0, 0.0, 0.0]);
            let (mut holes, mut total) = (0usize, 0usize);
            for y in 8..canvas.height - 8 {
                for x in 8..canvas.width - 8 {
                    for plane in &planes {
                        total += 1;
                        if plane[y * canvas.width + x].is_nan() {
                            holes += 1;
                        }
                    }
                }
            }
            holes as f64 / total as f64
        };

        let single = empty(&alone);
        let all = empty(&together);
        assert!(single > 0.4, "one frame cannot fill a mosaic: {single:.2} empty");
        assert!(all < 0.02, "and twenty-four dithered ones must: {all:.2} empty");
    }
}
