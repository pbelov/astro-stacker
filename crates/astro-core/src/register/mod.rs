//! Putting every frame of a run onto one set of coordinates.
//!
//! The shape of this is set by what actually varies between two subframes of the
//! same night on a tracked mount, and by what does not.
//!
//! **Four parameters, not six or eight.** Between two frames the field can shift,
//! turn about the pole, and change plate scale if the focus drew breath. It
//! cannot shear, and it cannot change aspect: those are properties of the sensor
//! and the optics, and they are the same in both frames. An affine fit has two
//! spare parameters, and spare parameters do not sit idle — they absorb
//! whatever the star matching got wrong and hide it in a lower residual. So the
//! fit is a similarity: rotation, uniform scale, translation.
//!
//! **A translation vote, not triangles.** The usual answer to "which star is
//! which" is to build similar triangles and match their shape descriptors, which
//! is invariant to everything and expensive. It is also solving a harder problem
//! than the one here: consecutive subframes of a tracked run differ by a shift
//! and a fraction of a degree, so every star has the *same* offset, and simply
//! voting for the most popular offset among all pairs finds it. Two hundred stars
//! against two hundred is forty thousand votes, and the right answer is the only
//! one that can be cast more than a handful of times.
//!
//! **Mutual nearest neighbours.** After the seed, a frame star matches a
//! reference star only when each is the other's nearest. A one-sided match lets
//! a crowded region point three frame stars at one reference star, and the fit
//! then quietly drags itself toward that region.

use std::collections::HashMap;

use crate::stars::Star;

/// One matched star: where it sits in the frame, and where the same star sits
/// in the reference.
type Pair = ((f64, f64), (f64, f64));

/// A similarity: rotation, uniform scale and translation, mapping frame
/// coordinates onto the reference's.
///
/// Held as `a = s·cos θ` and `b = s·sin θ` rather than as scale and angle,
/// because that is the form the least-squares fit produces and the form in which
/// two of them compose without trigonometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub a: f64,
    pub b: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Transform {
    pub const IDENTITY: Transform = Transform { a: 1.0, b: 0.0, tx: 0.0, ty: 0.0 };

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x - self.b * y + self.tx, self.b * x + self.a * y + self.ty)
    }

    /// Plate scale relative to the reference. One means the two frames image the
    /// same sky onto the same number of photosites.
    pub fn scale(&self) -> f64 {
        (self.a * self.a + self.b * self.b).sqrt()
    }

    pub fn rotation_degrees(&self) -> f64 {
        self.b.atan2(self.a).to_degrees()
    }

    /// This transform followed by another.
    ///
    /// Two similarities compose into a similarity, which is what lets a run be
    /// registered frame to neighbouring frame and the result carried back to a
    /// distant reference. As complex numbers `t·z + c`, composition is one
    /// multiplication and one addition.
    pub fn then(&self, next: &Transform) -> Transform {
        Transform {
            a: next.a * self.a - next.b * self.b,
            b: next.b * self.a + next.a * self.b,
            tx: next.a * self.tx - next.b * self.ty + next.tx,
            ty: next.b * self.tx + next.a * self.ty + next.ty,
        }
    }

    /// The transform that undoes this one. `None` for a degenerate one, which
    /// has collapsed the plane and cannot be undone.
    pub fn inverse(&self) -> Option<Transform> {
        let determinant = self.a * self.a + self.b * self.b;
        if determinant.is_nan() || determinant <= 0.0 {
            return None;
        }
        Some(Transform {
            a: self.a / determinant,
            b: -self.b / determinant,
            tx: -(self.tx * self.a + self.ty * self.b) / determinant,
            ty: -(self.ty * self.a - self.tx * self.b) / determinant,
        })
    }

    /// How far a given point moves, in photosites.
    ///
    /// The bare translation is not that number and reporting it as one is
    /// misleading: with any rotation at all, `tx` is measured from the origin in
    /// the corner and grows with the rotation rather than with the drift. Ask
    /// about the frame centre instead.
    pub fn displacement_at(&self, x: f64, y: f64) -> (f64, f64) {
        let (px, py) = self.apply(x, y);
        (px - x, py - y)
    }
}

/// How many parameters the pairs could afford.
///
/// Rotation and scale need a long baseline and many pairs to mean anything: a
/// dozen stars in one corner will happily fit a rotation of several arcminutes
/// that is really just their own centroid noise. Below the threshold the fit
/// spends two parameters instead of four and says so, rather than reporting an
/// angle it did not measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fitted {
    /// Shift only. `rotation_degrees` and `scale` on the transform are the
    /// values that were assumed, not measured.
    Shift,
    Similarity,
}

/// What registering one frame produced.
#[derive(Debug, Clone, Copy)]
pub struct Registration {
    pub transform: Transform,
    pub fitted: Fitted,
    /// How many stars were matched to the reference.
    pub matched: usize,
    /// Root mean square distance between a matched pair after the transform, in
    /// photosites. The honest measure of whether the four parameters were
    /// enough.
    pub residual: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct MatchOptions {
    /// How many of the brightest stars from each frame to use.
    ///
    /// Generous on purpose, and this was measured rather than guessed. The
    /// instinct is that a short list of the brightest stars is the reliable
    /// part, but the top of a brightness ranking is exactly where it is least
    /// stable: the brightest stars saturate and are dropped, and which ones
    /// saturate moves with the trailing, which on this project's reference
    /// session runs from 0.2 to 16 photosites. Two frames of identical pointing
    /// shared only 16 of their top 300, and 571 of their top 1000.
    pub brightest: usize,
    /// Width of a vote bin, in photosites.
    ///
    /// This is what sets how much rotation the seed survives, and it is worth
    /// being precise about why. Under a rotation the offset between two matched
    /// stars is not constant: two stars a distance `D` apart differ by about
    /// `D·θ`. So the votes spread over that much, and a bin narrower than it
    /// splits the one real answer into a dozen small piles that the accidental
    /// background can outvote. At 24 photosites across a 6700-photosite diagonal
    /// the seed survives about a fifth of a degree, which is far more rotation
    /// than a tracked run shows between subframes; the cost is a seed good only
    /// to a dozen photosites, which the first matching pass is sized to absorb.
    pub vote_bin: f64,
    /// How close a pair must be to count, at the first pass. Halves each pass
    /// afterwards: wide enough at first to cover the seed's error and the
    /// rotation it could not see, tight enough by the end to exclude a star's
    /// neighbour. Even the widest is well under the typical spacing between the
    /// few hundred brightest stars, which is why a wide first pass does not
    /// simply pair everything with whatever is nearby.
    pub match_radius: f64,
    pub iterations: usize,
    /// Below this many pairs the fit is not a fit.
    pub min_matches: usize,
    /// Below this many pairs, only the shift is fitted. Rotation and scale from
    /// a handful of stars are centroid noise wearing an angle.
    pub similarity_minimum: usize,
    /// How far the winning offset must stand above the best unrelated one.
    ///
    /// Two frames that share a field give a peak of a hundred or two against a
    /// background of four or five accidental agreements; two that do not give
    /// four against four. Measured on this project's reference session the two
    /// cases are separated by a factor of ten or more, so any threshold in the
    /// middle is safe, and a ratio is the right form because the background
    /// scales with how many stars were offered.
    pub vote_margin: f64,
    /// The largest residual that still counts as a registration, in photosites.
    ///
    /// Not a matter of taste. A real fit on this rig lands between 0.2 and 0.5
    /// photosites, which is the centroid error; anything approaching this is a
    /// fit to pairs that are not the same stars. It matters because the
    /// iteration keeps the best fit it reached, so a run that matched a dozen
    /// stars at the widest radius and then lost them would otherwise report that
    /// first loose pass as the answer.
    pub max_residual: f64,
}

impl Default for MatchOptions {
    fn default() -> Self {
        Self {
            brightest: 1000,
            vote_bin: 24.0,
            match_radius: 48.0,
            iterations: 5,
            min_matches: 12,
            similarity_minimum: 25,
            vote_margin: 3.0,
            max_residual: 3.0,
        }
    }
}

/// Registers one frame's stars against a reference frame's.
///
/// Returns `None` when the frames could not be matched at all, which is a real
/// answer about the frame — cloud, a cable snag, a different target — and not a
/// reason to return an identity transform that would silently stack it wrong.
pub fn register(
    reference: &[Star],
    frame: &[Star],
    options: &MatchOptions,
) -> Option<Registration> {
    // `detect` returns brightest first, so this is the bright end of each.
    let reference: Vec<(f64, f64)> =
        reference.iter().take(options.brightest).map(|star| (star.x, star.y)).collect();
    let frame: Vec<(f64, f64)> =
        frame.iter().take(options.brightest).map(|star| (star.x, star.y)).collect();
    if reference.len() < options.min_matches || frame.len() < options.min_matches {
        return None;
    }

    let (dx, dy) = vote_for_offset(&reference, &frame, options.vote_bin, options.vote_margin)?;
    refine_points(&reference, &frame, Transform { a: 1.0, b: 0.0, tx: dx, ty: dy }, options)
}

/// Registers a frame that already has a good guess, skipping the vote.
///
/// This is what makes a long run registerable at all. The vote needs the two
/// frames to share most of their sky, which consecutive subframes do and the
/// ends of a drifting night do not: on this project's reference session the
/// field walks two thousand photosites, and a frame a hundred exposures from
/// the reference has too little in common with it for the most popular offset to
/// mean anything. Registering neighbour to neighbour and carrying the answer
/// along the run gives every frame a guess good to a few photosites, and from
/// there the match against the distant reference is direct — which is what
/// stacking needs, since errors chained through two hundred links would not be.
pub fn refine(
    reference: &[Star],
    frame: &[Star],
    seed: Transform,
    options: &MatchOptions,
) -> Option<Registration> {
    let reference: Vec<(f64, f64)> =
        reference.iter().take(options.brightest).map(|star| (star.x, star.y)).collect();
    let frame: Vec<(f64, f64)> =
        frame.iter().take(options.brightest).map(|star| (star.x, star.y)).collect();
    if reference.len() < options.min_matches || frame.len() < options.min_matches {
        return None;
    }
    refine_points(&reference, &frame, seed, options)
}

fn refine_points(
    reference: &[(f64, f64)],
    frame: &[(f64, f64)],
    seed: Transform,
    options: &MatchOptions,
) -> Option<Registration> {
    let mut transform = seed;
    let index = Grid::build(reference, options.match_radius.max(1.0));
    let mut best: Option<Registration> = None;
    for step in 0..options.iterations.max(1) {
        // Halve each pass: the first has to survive the seed's error and the
        // rotation the seed could not see, the last has to exclude a star's
        // neighbour.
        let radius = options.match_radius * 0.5f64.powi(step as i32);
        let pairs = mutual_pairs(reference, frame, &transform, &index, radius);
        log::debug!(
            "pass {step}: {} pairs within {radius:.1} px of a shift of ({:.1}, {:.1})",
            pairs.len(),
            transform.tx,
            transform.ty
        );
        if pairs.len() < options.min_matches {
            break;
        }
        // Four parameters or two, according to what the pairs can carry.
        let fitted =
            if pairs.len() >= options.similarity_minimum { Fitted::Similarity } else { Fitted::Shift };
        let Some(next) = (match fitted {
            Fitted::Similarity => fit_similarity(&pairs),
            Fitted::Shift => fit_shift(&pairs),
        }) else {
            break;
        };
        transform = next;

        let residual = rms(&pairs, &transform);
        if residual.is_finite() && residual <= options.max_residual {
            best = Some(Registration { transform, fitted, matched: pairs.len(), residual });
        }
    }
    best
}

/// The most popular offset between any reference star and any frame star.
///
/// Every real pair casts the same vote, up to the rotation; every accidental
/// pair casts a vote of its own. With a few hundred stars each that is one bin
/// holding tens of votes against a background of ones and twos.
fn vote_for_offset(
    reference: &[(f64, f64)],
    frame: &[(f64, f64)],
    bin: f64,
    margin: f64,
) -> Option<(f64, f64)> {
    let bin = if bin > 0.0 { bin } else { 1.0 };
    let mut votes: HashMap<(i32, i32), u32> = HashMap::new();
    for &(rx, ry) in reference {
        for &(fx, fy) in frame {
            let key = (((rx - fx) / bin).round() as i32, ((ry - fy) / bin).round() as i32);
            *votes.entry(key).or_insert(0) += 1;
        }
    }

    // The winner and its eight neighbours together, so that a peak straddling a
    // bin edge is not split in half and beaten by a solid accident.
    let (&(bx, by), &best) = votes.iter().max_by_key(|(key, count)| (**count, **key))?;
    // How far the winner stands above the field of accidental agreements is the
    // whole question, so it is worth being able to see it.
    let runner_up = votes
        .iter()
        .filter(|((x, y), _)| (x - bx).abs() > 1 || (y - by).abs() > 1)
        .map(|(_, count)| *count)
        .max()
        .unwrap_or(0);
    log::debug!(
        "offset vote: {best} at ({}, {}) px against a next best of {runner_up}, over {} bins",
        f64::from(bx) * bin,
        f64::from(by) * bin,
        votes.len()
    );
    // No peak worth the name. Two frames of different sky agree by accident a
    // few times whatever you do, and calling that a registration would stack a
    // frame of somewhere else into the result without a word.
    if f64::from(best) < margin * f64::from(runner_up.max(1)) {
        return None;
    }

    let (mut sum_x, mut sum_y, mut total) = (0f64, 0f64, 0u32);
    for oy in -1..=1 {
        for ox in -1..=1 {
            if let Some(&count) = votes.get(&(bx + ox, by + oy)) {
                sum_x += f64::from(bx + ox) * bin * f64::from(count);
                sum_y += f64::from(by + oy) * bin * f64::from(count);
                total += count;
            }
        }
    }
    (total > 0).then(|| (sum_x / f64::from(total), sum_y / f64::from(total)))
}

/// A uniform grid over the reference stars, so matching does not cost every
/// frame star a scan of every reference star.
struct Grid {
    cell: f64,
    buckets: HashMap<(i32, i32), Vec<usize>>,
}

impl Grid {
    fn build(points: &[(f64, f64)], cell: f64) -> Self {
        let mut buckets: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (index, &(x, y)) in points.iter().enumerate() {
            buckets.entry(Self::key(x, y, cell)).or_default().push(index);
        }
        Self { cell, buckets }
    }

    fn key(x: f64, y: f64, cell: f64) -> (i32, i32) {
        ((x / cell).floor() as i32, (y / cell).floor() as i32)
    }

    /// The nearest point within `radius`, and how far it was.
    fn nearest(&self, points: &[(f64, f64)], x: f64, y: f64, radius: f64) -> Option<(usize, f64)> {
        let (cx, cy) = Self::key(x, y, self.cell);
        let reach = (radius / self.cell).ceil() as i32;
        let mut best: Option<(usize, f64)> = None;
        for oy in -reach..=reach {
            for ox in -reach..=reach {
                let Some(bucket) = self.buckets.get(&(cx + ox, cy + oy)) else { continue };
                for &index in bucket {
                    let (px, py) = points[index];
                    let distance = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
                    if distance <= radius && best.is_none_or(|(_, d)| distance < d) {
                        best = Some((index, distance));
                    }
                }
            }
        }
        best
    }
}

/// Pairs where each star is the other's nearest, as `(frame point, reference
/// point)`.
///
/// One-sided matching lets a crowded region point three frame stars at one
/// reference star, and a least-squares fit then drags itself toward that region
/// while reporting a residual that looks fine.
fn mutual_pairs(
    reference: &[(f64, f64)],
    frame: &[(f64, f64)],
    transform: &Transform,
    index: &Grid,
    radius: f64,
) -> Vec<Pair> {
    // Where each frame star lands under the current transform.
    let projected: Vec<(f64, f64)> =
        frame.iter().map(|&(x, y)| transform.apply(x, y)).collect();
    let forward: Vec<Option<usize>> = projected
        .iter()
        .map(|&(x, y)| index.nearest(reference, x, y, radius).map(|(index, _)| index))
        .collect();

    // And the reverse, over the projected frame stars.
    let back = Grid::build(&projected, radius.max(1.0));
    let mut pairs = Vec::new();
    for (frame_index, target) in forward.iter().enumerate() {
        let Some(reference_index) = *target else { continue };
        let (rx, ry) = reference[reference_index];
        if let Some((nearest, _)) = back.nearest(&projected, rx, ry, radius)
            && nearest == frame_index
        {
            pairs.push((frame[frame_index], reference[reference_index]));
        }
    }
    pairs
}

/// Least-squares similarity from matched pairs, in closed form.
///
/// Written as one complex division. Minimising `sum |z·p + c - q|^2` over a
/// complex `z` and `c` is what a similarity *is*, and in that form the answer
/// falls out without a matrix decomposition or an iteration: centre both sets,
/// and `z` is the sum of `conj(p)·q` over the sum of `|p|^2`.
fn fit_similarity(pairs: &[Pair]) -> Option<Transform> {
    let count = pairs.len() as f64;
    if pairs.len() < 2 {
        return None;
    }
    let (mut px, mut py, mut qx, mut qy) = (0f64, 0f64, 0f64, 0f64);
    for &((fx, fy), (rx, ry)) in pairs {
        px += fx;
        py += fy;
        qx += rx;
        qy += ry;
    }
    let (px, py, qx, qy) = (px / count, py / count, qx / count, qy / count);

    let (mut real, mut imaginary, mut norm) = (0f64, 0f64, 0f64);
    for &((fx, fy), (rx, ry)) in pairs {
        let (ax, ay) = (fx - px, fy - py);
        let (bx, by) = (rx - qx, ry - qy);
        real += ax * bx + ay * by;
        imaginary += ax * by - ay * bx;
        norm += ax * ax + ay * ay;
    }
    // Every matched star at the same place: no baseline, so no rotation and no
    // scale can be measured from them.
    if norm.is_nan() || norm <= 0.0 {
        return None;
    }
    let (a, b) = (real / norm, imaginary / norm);
    if !a.is_finite() || !b.is_finite() {
        return None;
    }
    Some(Transform { a, b, tx: qx - (a * px - b * py), ty: qy - (b * px + a * py) })
}

/// Translation alone: the mean displacement over the pairs.
///
/// What is fitted when there are too few pairs to afford an angle.
fn fit_shift(pairs: &[Pair]) -> Option<Transform> {
    if pairs.is_empty() {
        return None;
    }
    let count = pairs.len() as f64;
    let (mut tx, mut ty) = (0f64, 0f64);
    for &((fx, fy), (rx, ry)) in pairs {
        tx += rx - fx;
        ty += ry - fy;
    }
    Some(Transform { a: 1.0, b: 0.0, tx: tx / count, ty: ty / count })
}

fn rms(pairs: &[Pair], transform: &Transform) -> f64 {
    if pairs.is_empty() {
        return f64::NAN;
    }
    let total: f64 = pairs
        .iter()
        .map(|&((fx, fy), (rx, ry))| {
            let (px, py) = transform.apply(fx, fy);
            (px - rx).powi(2) + (py - ry).powi(2)
        })
        .sum();
    (total / pairs.len() as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stars::Moments;

    /// A star list with descending flux, which is the order `detect` produces.
    fn stars(points: &[(f64, f64)]) -> Vec<Star> {
        points
            .iter()
            .enumerate()
            .map(|(index, &(x, y))| {
                let mut star = Star::at(x, y, Moments { m11: 1.0, m22: 1.0, m12: 0.0 });
                star.flux = 10_000.0 - index as f64;
                star
            })
            .collect()
    }

    /// A pseudo-random field, deterministic so a failure can be reproduced.
    fn field(count: usize, extent: f64) -> Vec<(f64, f64)> {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        (0..count).map(|_| (next() * extent, next() * extent)).collect()
    }

    fn moved(points: &[(f64, f64)], transform: &Transform) -> Vec<(f64, f64)> {
        points.iter().map(|&(x, y)| transform.apply(x, y)).collect()
    }

    #[test]
    fn a_shifted_field_is_registered_to_the_photosite() {
        // The ordinary case: a tracked mount drifting between subframes.
        let reference = field(200, 5000.0);
        let truth = Transform { a: 1.0, b: 0.0, tx: 37.4, ty: -12.9 };
        // The frame's stars are where the reference's are, seen from the frame,
        // so the transform that takes frame to reference is the one applied.
        let inverse = Transform { a: 1.0, b: 0.0, tx: -37.4, ty: 12.9 };
        let frame = moved(&reference, &inverse);

        let found = register(&stars(&reference), &stars(&frame), &MatchOptions::default())
            .expect("a shifted field registers");
        assert!(found.matched > 150, "matched {}", found.matched);
        assert!(found.residual < 0.01, "residual {}", found.residual);
        let (dx, dy) = found.transform.displacement_at(2500.0, 2500.0);
        assert!((dx - truth.tx).abs() < 0.05 && (dy - truth.ty).abs() < 0.05, "{dx},{dy}");
    }

    #[test]
    fn rotation_and_scale_are_recovered_and_not_absorbed_into_the_shift() {
        // Field rotation and a focus drift. Four parameters exist so that these
        // are reported rather than smeared into the translation.
        let reference = field(200, 5000.0);
        let angle = 0.35f64.to_radians();
        let scale = 1.0008;
        let truth = Transform {
            a: scale * angle.cos(),
            b: scale * angle.sin(),
            tx: 21.0,
            ty: 44.0,
        };
        // Build the frame by inverting the truth, so registering it must return
        // the truth itself.
        let determinant = truth.a * truth.a + truth.b * truth.b;
        let inverse = Transform {
            a: truth.a / determinant,
            b: -truth.b / determinant,
            tx: (-truth.a * truth.tx - truth.b * truth.ty) / determinant,
            ty: (truth.b * truth.tx - truth.a * truth.ty) / determinant,
        };
        let frame = moved(&reference, &inverse);

        let found = register(&stars(&reference), &stars(&frame), &MatchOptions::default())
            .expect("a turned field registers");
        assert!(found.residual < 0.05, "residual {}", found.residual);
        assert!(
            (found.transform.rotation_degrees() - 0.35).abs() < 0.01,
            "rotation {}",
            found.transform.rotation_degrees()
        );
        assert!(
            (found.transform.scale() - scale).abs() < 0.0005,
            "scale {}",
            found.transform.scale()
        );
    }

    #[test]
    fn stars_missing_from_one_frame_and_invented_in_the_other_do_not_break_it() {
        // What a real pair of frames looks like: cloud takes some, noise adds
        // others, and neither list is a permutation of the other.
        let all = field(240, 5000.0);
        let reference: Vec<(f64, f64)> = all[..180].to_vec();
        let inverse = Transform { a: 1.0, b: 0.0, tx: -60.0, ty: 25.0 };
        let mut frame = moved(&all[40..], &inverse);
        // Forty spurious detections, a fifth of the list, at their own places.
        frame.extend(moved(&field(40, 5000.0), &inverse));

        let found = register(&stars(&reference), &stars(&frame), &MatchOptions::default())
            .expect("a partly shared field registers");
        assert!(found.matched > 100, "matched {}", found.matched);
        assert!(found.residual < 0.05, "residual {}", found.residual);
        let (dx, dy) = found.transform.displacement_at(2500.0, 2500.0);
        assert!((dx - 60.0).abs() < 0.1 && (dy + 25.0).abs() < 0.1, "{dx},{dy}");
    }

    #[test]
    fn a_handful_of_pairs_buys_a_shift_and_not_an_angle() {
        // Twelve stars in one corner will fit a rotation of several arcminutes
        // out of their own centroid noise, and it will look like a measurement.
        // Two parameters are all they can carry.
        let reference: Vec<(f64, f64)> = field(14, 400.0);
        let inverse = Transform { a: 1.0, b: 0.0, tx: -30.0, ty: 12.0 };
        let frame = moved(&reference, &inverse);

        let found = register(&stars(&reference), &stars(&frame), &MatchOptions::default())
            .expect("fourteen stars still register");
        assert_eq!(found.fitted, Fitted::Shift, "{} pairs bought an angle", found.matched);
        assert_eq!(found.transform.rotation_degrees(), 0.0, "an angle that was never measured");
        assert_eq!(found.transform.scale(), 1.0);
        let (dx, dy) = found.transform.displacement_at(200.0, 200.0);
        assert!((dx - 30.0).abs() < 0.05 && (dy + 12.0).abs() < 0.05, "{dx},{dy}");
    }

    #[test]
    fn enough_pairs_buys_the_whole_similarity() {
        let reference = field(200, 5000.0);
        let inverse = Transform { a: 1.0, b: 0.0, tx: -30.0, ty: 12.0 };
        let frame = moved(&reference, &inverse);
        let found = register(&stars(&reference), &stars(&frame), &MatchOptions::default()).unwrap();
        assert_eq!(found.fitted, Fitted::Similarity);
    }

    #[test]
    fn transforms_compose_and_undo_each_other() {
        // The algebra a chained run rests on. If `then` or `inverse` is wrong,
        // every frame away from the reference is seeded into the wrong place and
        // the failure looks like bad data.
        let angle = 0.7f64.to_radians();
        let first = Transform { a: 1.0002 * angle.cos(), b: 1.0002 * angle.sin(), tx: 12.0, ty: -5.0 };
        let second = Transform { a: 0.9995, b: -0.004, tx: -30.0, ty: 44.0 };

        let both = first.then(&second);
        let (x, y) = (1234.0, 5678.0);
        let stepwise = {
            let (px, py) = first.apply(x, y);
            second.apply(px, py)
        };
        let together = both.apply(x, y);
        assert!((stepwise.0 - together.0).abs() < 1e-9, "{stepwise:?} vs {together:?}");
        assert!((stepwise.1 - together.1).abs() < 1e-9, "{stepwise:?} vs {together:?}");

        let back = both.inverse().expect("a similarity with scale is invertible");
        let (rx, ry) = back.apply(together.0, together.1);
        assert!((rx - x).abs() < 1e-6 && (ry - y).abs() < 1e-6, "{rx},{ry}");

        // A collapsed transform cannot be undone, and says so.
        assert!(Transform { a: 0.0, b: 0.0, tx: 1.0, ty: 2.0 }.inverse().is_none());
    }

    #[test]
    fn a_seeded_refit_registers_what_the_vote_alone_cannot() {
        // Two frames sharing only a corner: the vote has too few real pairs to
        // outvote the accidental ones, but a guess from the neighbouring frames
        // costs nothing and lands it.
        let all = field(400, 5000.0);
        let reference: Vec<(f64, f64)> = all.iter().copied().filter(|(x, _)| *x < 2200.0).collect();
        let shift = Transform { a: 1.0, b: 0.0, tx: -1800.0, ty: 40.0 };
        let frame: Vec<(f64, f64)> =
            moved(&all.iter().copied().filter(|(x, _)| *x > 1600.0).collect::<Vec<_>>(), &shift);

        let seeded = refine(
            &stars(&reference),
            &stars(&frame),
            Transform { a: 1.0, b: 0.0, tx: 1795.0, ty: -37.0 },
            &MatchOptions::default(),
        )
        .expect("a seeded refit registers");
        assert!(seeded.matched > 20, "matched {}", seeded.matched);
        assert!(seeded.residual < 0.05, "residual {}", seeded.residual);
        let (dx, dy) = seeded.transform.displacement_at(2500.0, 2500.0);
        assert!((dx - 1800.0).abs() < 0.1 && (dy + 40.0).abs() < 0.1, "{dx},{dy}");
    }

    #[test]
    fn an_unrelated_field_is_refused_rather_than_fitted() {
        // The failure that matters. Returning an identity transform for a frame
        // that does not overlap would stack it in silently, and a hundred good
        // frames cannot outvote one that is simply somewhere else.
        let reference = field(200, 5000.0);
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let elsewhere: Vec<(f64, f64)> =
            (0..200).map(|_| (next() * 5000.0, next() * 5000.0)).collect();

        // Two unrelated fields of 200 stars over five thousand photosites always
        // share a few accidental coincidences. The peak has to stand above them
        // by a margin, not merely be the largest number present.
        assert!(
            register(&stars(&reference), &stars(&elsewhere), &MatchOptions::default()).is_none(),
            "an unrelated field must be refused, not fitted"
        );
    }

    #[test]
    fn a_frame_with_too_few_stars_says_so() {
        let reference = field(200, 5000.0);
        let frame = field(4, 5000.0);
        assert!(register(&stars(&reference), &stars(&frame), &MatchOptions::default()).is_none());
    }

    #[test]
    fn the_reported_shift_is_the_frame_centre_and_not_the_origin() {
        // With any rotation the translation term is measured from the corner and
        // grows with the angle rather than with the drift, so reporting it as
        // "how far the frame moved" would be wrong in a way that looks right.
        let angle = 1.0f64.to_radians();
        let turned = Transform { a: angle.cos(), b: angle.sin(), tx: 0.0, ty: 0.0 };
        let (dx, dy) = turned.displacement_at(0.0, 0.0);
        assert!(dx.abs() < 1e-12 && dy.abs() < 1e-12, "the origin does not move under a rotation");
        let (dx, dy) = turned.displacement_at(2600.0, 1730.0);
        assert!(dx.abs() > 25.0 || dy.abs() > 25.0, "but the middle of the frame does: {dx},{dy}");
    }
}
