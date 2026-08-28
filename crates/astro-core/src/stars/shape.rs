//! The shape of a star, and of a whole frame's worth of them.
//!
//! Two rules here are not stylistic.
//!
//! **A failed measurement is NaN, never zero.** `f32::max` returns the non-NaN
//! operand, so the idiomatic `.max(0.0)` on a moment turns "this could not be
//! measured" into "this is perfectly round" — a confident answer to a question
//! that was never answered. Every accessor below either produces a real number
//! or NaN.
//!
//! **A position angle is a spin-2 quantity and is never averaged as an angle.**
//! An ellipse at 179 degrees and one at 1 degree point almost the same way, and
//! their arithmetic mean is 90 — perpendicular to both. Frame shape is therefore
//! aggregated by taking the median of the moment *matrix* and deriving the angle
//! from that, which is a tensor average and cannot cross a branch cut; the
//! consistency of the direction is measured separately as the length of the mean
//! of `(cos 2t, sin 2t)`.

/// The second moments of one source about its own centroid, in photosites.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moments {
    pub m11: f64,
    pub m22: f64,
    pub m12: f64,
}

/// A Gaussian's full width at half maximum is this many standard deviations.
const FWHM_PER_SIGMA: f64 = 2.354_820_045;

impl Moments {
    pub const NONE: Moments = Moments { m11: f64::NAN, m22: f64::NAN, m12: f64::NAN };

    /// The eigenvalues of the moment matrix: the variances along the major and
    /// minor axes. `None` when the moments are not a usable matrix.
    pub fn axes(self) -> Option<(f64, f64)> {
        if !(self.m11.is_finite() && self.m22.is_finite() && self.m12.is_finite()) {
            return None;
        }
        let half_trace = (self.m11 + self.m22) / 2.0;
        let root = (((self.m11 - self.m22) / 2.0).powi(2) + self.m12 * self.m12).sqrt();
        let (major, minor) = (half_trace + root, half_trace - root);
        // A negative eigenvalue means the moments are not a covariance at all,
        // which happens when the sky was over-subtracted under a faint source.
        (major > 0.0 && minor >= 0.0).then_some((major, minor))
    }

    /// Full width at half maximum along the long axis, in photosites.
    pub fn major_fwhm(self) -> f64 {
        self.axes().map_or(f64::NAN, |(major, _)| major.sqrt() * FWHM_PER_SIGMA)
    }

    /// Full width at half maximum across the short axis. On a trailed frame this
    /// is the one that carries the seeing and the focus; the long one carries
    /// what the mount did.
    pub fn minor_fwhm(self) -> f64 {
        self.axes().map_or(f64::NAN, |(_, minor)| minor.sqrt() * FWHM_PER_SIGMA)
    }

    /// One minus the ratio of the axes: 0 for a round source, approaching 1 for
    /// a line.
    pub fn ellipticity(self) -> f64 {
        self.axes().map_or(f64::NAN, |(major, minor)| 1.0 - (minor / major).sqrt())
    }

    /// How much longer the source is than it is wide, in photosites: the excess
    /// of the long axis over the short one.
    ///
    /// This is the number a trail is measured in. A round star of any width
    /// reads zero, so it separates what the mount did from what the seeing did —
    /// unlike a plain FWHM, which grows with both.
    pub fn trail(self) -> f64 {
        self.axes().map_or(f64::NAN, |(major, minor)| {
            (major.sqrt() - minor.sqrt()) * FWHM_PER_SIGMA
        })
    }

    /// The direction of the long axis, as the spin-2 unit vector
    /// `(cos 2t, sin 2t)`.
    ///
    /// Returned this way rather than as an angle because that is the only form
    /// in which directions may be averaged. Convert to degrees only to print.
    pub fn orientation(self) -> Option<(f64, f64)> {
        let (major, minor) = self.axes()?;
        let difference = self.m11 - self.m22;
        let length = (difference * difference + 4.0 * self.m12 * self.m12).sqrt();
        if length <= 0.0 || major <= minor {
            // Perfectly round: the long axis has no direction, and inventing
            // one would put a number where there is genuinely none.
            return None;
        }
        Some((difference / length, 2.0 * self.m12 / length))
    }

    /// The long axis in degrees, in `[0, 180)`, measured from the x axis.
    ///
    /// A half-open interval of 180 degrees rather than 360, because an ellipse
    /// has no head and no tail.
    pub fn angle_degrees(self) -> f64 {
        match self.orientation() {
            Some((cos2t, sin2t)) => {
                let degrees = sin2t.atan2(cos2t).to_degrees() / 2.0;
                if degrees < 0.0 { degrees + 180.0 } else { degrees }
            }
            None => f64::NAN,
        }
    }
}

/// What a frame's stars say about the frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameShape {
    /// The median moment matrix over the stars, from which everything else is
    /// derived. A tensor median, so it cannot cross the branch cut an angle
    /// median would.
    pub moments: Moments,
    pub stars: usize,
    /// How consistently the stars point the same way: the length of the mean of
    /// `(cos 2t, sin 2t)`, from 0 for random to 1 for perfectly aligned.
    ///
    /// This is what separates trailing from everything else. Trailing points one
    /// way across the whole frame; bad seeing and noise do not point at all;
    /// field rotation points in directions that turn about a point, so it lands
    /// in between and needs the positions to tell apart.
    pub direction_agreement: f64,
    /// Median sky level and noise over the frame, in the units the pixels came
    /// in.
    pub sky: f32,
    pub noise: f32,
}

impl FrameShape {
    /// Aggregates a frame's stars.
    ///
    /// The median of each moment rather than the mean: one badly deblended pair
    /// or one satellite streak has moments an order of magnitude out, and a mean
    /// would follow it.
    pub fn of(stars: &[super::Star], sky: f32, noise: f32) -> Self {
        let usable: Vec<Moments> =
            stars.iter().map(|star| star.moments).filter(|m| m.axes().is_some()).collect();
        if usable.is_empty() {
            return Self {
                moments: Moments::NONE,
                stars: 0,
                direction_agreement: f64::NAN,
                sky,
                noise,
            };
        }

        let median = |pick: fn(&Moments) -> f64| {
            let mut values: Vec<f64> = usable.iter().map(pick).collect();
            let middle = values.len() / 2;
            let (_, value, _) = values.select_nth_unstable_by(middle, f64::total_cmp);
            *value
        };
        let moments =
            Moments { m11: median(|m| m.m11), m22: median(|m| m.m22), m12: median(|m| m.m12) };

        // Weighted by how elongated each star is, so that the round ones — which
        // have no direction to contribute — do not dilute the answer toward zero.
        let mut sum = (0.0f64, 0.0f64);
        let mut weight = 0.0f64;
        for star in &usable {
            if let Some((cos2t, sin2t)) = star.orientation() {
                let elongation = star.ellipticity();
                if elongation.is_finite() {
                    sum.0 += cos2t * elongation;
                    sum.1 += sin2t * elongation;
                    weight += elongation;
                }
            }
        }
        let direction_agreement =
            if weight > 0.0 { (sum.0 * sum.0 + sum.1 * sum.1).sqrt() / weight } else { f64::NAN };

        Self { moments, stars: usable.len(), direction_agreement, sky, noise }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elongated(sigma_major: f64, sigma_minor: f64, degrees: f64) -> Moments {
        let (a, b) = (sigma_major * sigma_major, sigma_minor * sigma_minor);
        let t = degrees.to_radians();
        Moments {
            m11: a * t.cos().powi(2) + b * t.sin().powi(2),
            m22: a * t.sin().powi(2) + b * t.cos().powi(2),
            m12: (a - b) * t.sin() * t.cos(),
        }
    }

    #[test]
    fn a_trail_is_measured_as_the_excess_of_the_long_axis() {
        // The reference session's stars: about 2 px across and 6 px along.
        let star = elongated(6.0 / 2.3548, 2.0 / 2.3548, 94.5);
        assert!((star.major_fwhm() - 6.0).abs() < 0.01, "{}", star.major_fwhm());
        assert!((star.minor_fwhm() - 2.0).abs() < 0.01, "{}", star.minor_fwhm());
        assert!((star.trail() - 4.0).abs() < 0.01, "{}", star.trail());
    }

    #[test]
    fn a_round_star_of_any_width_reads_no_trail() {
        // What separates what the mount did from what the seeing did: a plain
        // FWHM grows with both, and this does not.
        for fwhm in [1.0, 2.0, 6.0] {
            let star = elongated(fwhm / 2.3548, fwhm / 2.3548, 0.0);
            assert!(star.trail().abs() < 0.01, "at {fwhm}: {}", star.trail());
            assert!(star.ellipticity().abs() < 0.01);
            // And it has no direction at all, rather than an invented one.
            assert!(star.orientation().is_none());
            assert!(star.angle_degrees().is_nan());
        }
    }

    #[test]
    fn a_failed_measurement_is_nan_and_not_a_confident_zero() {
        // The trap: `f32::max` returns the non-NaN operand, so `.max(0.0)` on a
        // moment reports an unmeasurable star as perfectly round.
        assert!(Moments::NONE.trail().is_nan());
        assert!(Moments::NONE.minor_fwhm().is_nan());
        assert!(Moments::NONE.ellipticity().is_nan());
        assert!(Moments::NONE.axes().is_none());

        // A negative eigenvalue is not a covariance, and says so.
        let broken = Moments { m11: 1.0, m22: 1.0, m12: 4.0 };
        assert!(broken.axes().is_none());
        assert!(broken.trail().is_nan());
    }

    #[test]
    fn the_angle_lives_in_a_half_circle_because_an_ellipse_has_no_head() {
        for (given, expected) in [(0.0, 0.0), (45.0, 45.0), (94.5, 94.5), (170.0, 170.0)] {
            let star = elongated(3.0, 1.0, given);
            let angle = star.angle_degrees();
            assert!((angle - expected).abs() < 0.01, "{given} came back as {angle}");
        }
    }

    #[test]
    fn directions_are_averaged_as_spin_two_and_not_as_angles() {
        // Two ellipses at 179 and 1 degree point almost the same way. Their
        // arithmetic mean is 90 - perpendicular to both - and this session's
        // own trail sits at 94.5 degrees, right on the branch cut of the
        // obvious implementation.
        let stars: Vec<super::super::Star> = [179.0, 1.0, 178.0, 2.0]
            .iter()
            .map(|degrees| super::super::Star::at(0.0, 0.0, elongated(3.0, 1.0, *degrees)))
            .collect();
        let shape = FrameShape::of(&stars, 0.0, 0.0);

        let angle = shape.moments.angle_degrees();
        assert!(
            angle < 5.0 || angle > 175.0,
            "the aggregate must point where the stars do, got {angle}"
        );
        assert!(shape.direction_agreement > 0.99, "they agree: {}", shape.direction_agreement);
    }

    #[test]
    fn stars_pointing_every_way_agree_about_nothing() {
        // Which is what noise and seeing look like, as opposed to trailing.
        let stars: Vec<super::super::Star> = (0..36)
            .map(|i| super::super::Star::at(0.0, 0.0, elongated(1.6, 1.0, f64::from(i) * 5.0)))
            .collect();
        let shape = FrameShape::of(&stars, 0.0, 0.0);
        assert!(shape.direction_agreement < 0.1, "got {}", shape.direction_agreement);
    }

    #[test]
    fn one_satellite_streak_does_not_move_the_frame() {
        // A median of the moment matrix, not a mean: a streak has moments an
        // order of magnitude out and a mean would follow it.
        let mut stars: Vec<super::super::Star> = (0..40)
            .map(|_| super::super::Star::at(0.0, 0.0, elongated(6.0 / 2.3548, 2.0 / 2.3548, 94.5)))
            .collect();
        stars.push(super::super::Star::at(0.0, 0.0, elongated(300.0, 1.0, 30.0)));

        let shape = FrameShape::of(&stars, 0.0, 0.0);
        assert!((shape.moments.major_fwhm() - 6.0).abs() < 0.1);
    }
}
