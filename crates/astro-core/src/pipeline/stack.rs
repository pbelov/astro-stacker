//! Choosing which frames go into the stack, what each is worth, and combining
//! them.
//!
//! The selection is deliberately not a filter with a hidden rule. Frames are
//! weighted by quality by default, so a slightly trailed one still contributes
//! what it carries; a threshold is available for when the answer wanted is the
//! sharp frames only. Both live here, and what a threshold would cost is
//! computable before it is set — a limit chosen without seeing that is chosen
//! blind.

use std::time::Instant;

use crate::calibrate::apply;
use crate::error::{Error, Result};
use crate::integrate::{Background, Canvas, Contribution, DEFAULT_PIXFRAC, Stack};
use crate::register::{Registration, Transform, matches};
use crate::session::mosaic::Mosaic;
use crate::stars::Star;
use crate::{PluginHost, Samples};

use super::align::Alignment;
use super::{Flow, MasterSet, Measured, Step};

/// The largest output grid worth allocating, in pixels.
pub const CANVAS_LIMIT: usize = 400_000_000;
/// How close a star must land to be the same star, once the transform is
/// fitted. Tighter than matching, because this is confirmation and not search.
const PHOTOMETRY_RADIUS: f64 = 3.0;
/// Below this many shared stars a brightness ratio is noise wearing a number.
const MIN_PHOTOMETRY_STARS: usize = 12;

#[derive(Debug, Clone, Copy)]
pub struct StackOptions {
    /// What the stack is optimised for. 0 weights by noise alone, which is
    /// optimal for anything larger than a star; 1 also divides by the PSF area,
    /// which is optimal for stars. Between them it interpolates.
    pub sharpness: f64,
    pub max_trail: Option<f64>,
    pub max_fwhm: Option<f64>,
    pub max_shift: Option<f64>,
    pub pixfrac: f64,
}

impl Default for StackOptions {
    fn default() -> Self {
        Self {
            sharpness: 0.5,
            max_trail: None,
            max_fwhm: None,
            max_shift: None,
            pixfrac: DEFAULT_PIXFRAC,
        }
    }
}

/// One frame, and what the selection made of it.
pub struct Chosen<'a> {
    pub read: &'a Measured,
    pub registration: Registration,
    /// Multiplies this frame's background-subtracted pixels to bring it onto
    /// the run's common brightness. `None` where too few stars were shared with
    /// the reference to measure it.
    pub scale: Option<f64>,
    /// NaN where the frame could not be weighed against the others. Not an
    /// average: that would be a guess dressed as a measurement.
    pub weight: f64,
    /// How far the middle of the picture moved, in photosites.
    pub shift: f64,
}

impl Chosen<'_> {
    pub fn usable(&self) -> bool {
        self.weight.is_finite() && self.weight > 0.0
    }
}

pub struct Selection<'a> {
    pub frames: Vec<Chosen<'a>>,
    pub refused: Vec<(String, String)>,
    /// The middle of the reference's star field, which stands in for the middle
    /// of the frame.
    pub centre: (f64, f64),
}

/// Picks the frames, measures their brightness against the reference and weighs
/// them.
pub fn select<'a>(alignment: &'a Alignment<'a>, options: &StackOptions) -> Selection<'a> {
    let anchor: Vec<Star> = alignment.frames[alignment.reference].stars().to_vec();
    let centre = centre_of(&anchor);

    let mut frames = Vec::new();
    let mut refused = Vec::new();
    for frame in &alignment.frames {
        let Some(registration) = frame.registration else {
            refused.push((frame.name().to_owned(), "did not register".to_owned()));
            continue;
        };
        let (dx, dy) = registration.transform.displacement_at(centre.0, centre.1);
        frames.push(Chosen {
            read: frame.read,
            registration,
            scale: brightness_of(&anchor, frame.stars(), &registration.transform),
            weight: f64::NAN,
            shift: (dx * dx + dy * dy).sqrt(),
        });
    }

    frames.retain(|frame| match refusal(frame, options) {
        Some(why) => {
            refused.push((frame.read.name.clone(), why));
            false
        }
        None => true,
    });
    weigh(&mut frames, options.sharpness);

    Selection { frames, refused, centre }
}

/// The brightness of one frame against the reference, from the stars they
/// share.
///
/// Measured on stars and never on sky level, because the two do not move
/// together: thin cirrus dims the stars while *raising* the sky by scattering
/// city light back down. A scale taken from the sky would multiply the clear
/// frames down and the hazy ones up, which is the opposite of the correction
/// wanted.
///
/// The flux each star carries is already independent of how blurred it is, so a
/// trailed frame does not read as a cloudy one.
fn brightness_of(reference: &[Star], frame: &[Star], transform: &Transform) -> Option<f64> {
    let pairs = matches(reference, frame, transform, PHOTOMETRY_RADIUS);
    let mut ratios: Vec<f64> = pairs
        .iter()
        .filter_map(|(f, r)| {
            let (a, b) = (reference[*r].flux, frame[*f].flux);
            (a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0).then_some(a / b)
        })
        .collect();
    if ratios.len() < MIN_PHOTOMETRY_STARS {
        return None;
    }
    ratios.sort_by(f64::total_cmp);
    Some(ratios[ratios.len() / 2])
}

fn refusal(frame: &Chosen<'_>, options: &StackOptions) -> Option<String> {
    let shape = &frame.read.detection.shape;
    if frame.scale.is_none() {
        return Some(format!(
            "shares fewer than {MIN_PHOTOMETRY_STARS} measurable stars with the reference, so its \
             brightness could not be put on the same scale"
        ));
    }
    if let Some(limit) = options.max_trail
        && over(shape.moments.trail(), limit)
    {
        return Some(describe("trail", shape.moments.trail(), limit));
    }
    if let Some(limit) = options.max_fwhm
        && over(shape.moments.minor_fwhm(), limit)
    {
        return Some(describe("fwhm", shape.moments.minor_fwhm(), limit));
    }
    if let Some(limit) = options.max_shift
        && frame.shift > limit
    {
        return Some(format!("moved {:.0} px, over the {limit:.0} limit", frame.shift));
    }
    None
}

/// Whether a measurement fails a limit.
///
/// An unmeasurable value fails it. Asked for frames under a trail of six, a
/// frame whose trail could not be measured is not a frame known to be under
/// six.
fn over(value: f64, limit: f64) -> bool {
    value.is_nan() || value > limit
}

fn describe(what: &str, value: f64, limit: f64) -> String {
    if value.is_nan() {
        format!("{what} could not be measured, so it cannot be shown to be under {limit:.2}")
    } else {
        format!("{what} {value:.2} px over the {limit:.2} limit")
    }
}

/// The weight of each frame, gauged so the median sits near one.
///
/// `(sigma_med / (scale * sigma))^2 * (area_med / area)^sharpness`. The scale
/// belongs inside the square because multiplying a frame up multiplies its
/// noise with it; leaving it out weights a hazy frame as though it had been
/// clear, and can make the result noisier than an unweighted mean of the good
/// frames alone.
fn weigh(frames: &mut [Chosen<'_>], sharpness: f64) {
    let area_of = |frame: &Chosen<'_>| {
        let m = frame.read.detection.shape.moments;
        // The registration residual is a real blur and belongs in the area.
        // Half the squared 2-D radius is the per-axis variance.
        let spread = frame.registration.residual * frame.registration.residual / 2.0;
        let determinant = (m.m11 + spread) * (m.m22 + spread) - m.m12 * m.m12;
        (determinant > 0.0).then(|| 4.0 * std::f64::consts::PI * determinant.sqrt())
    };
    let noise_of = |frame: &Chosen<'_>| {
        f64::from(frame.read.detection.shape.noise) * frame.scale.unwrap_or(f64::NAN)
    };

    let median = |values: &mut Vec<f64>| {
        if values.is_empty() {
            return f64::NAN;
        }
        values.sort_by(f64::total_cmp);
        values[values.len() / 2]
    };
    let mut noises: Vec<f64> =
        frames.iter().map(noise_of).filter(|value| value.is_finite()).collect();
    let mut areas: Vec<f64> = frames.iter().filter_map(area_of).collect();
    let (noise_gauge, area_gauge) = (median(&mut noises), median(&mut areas));

    for frame in frames.iter_mut() {
        let sigma = noise_of(frame);
        frame.weight = match (sigma.is_finite() && sigma > 0.0, area_of(frame)) {
            (true, Some(area)) if area > 0.0 => {
                (noise_gauge / sigma).powi(2) * (area_gauge / area).powf(sharpness)
            }
            _ => f64::NAN,
        };
    }
}

/// What a stack came out as.
pub struct Stacked {
    pub canvas: Canvas,
    pub colours: usize,
    /// One plane per colour, NaN where no frame reached.
    pub planes: Vec<Vec<f32>>,
    /// The level put back per colour, so the result reads like an exposure
    /// rather than like a difference.
    pub pedestal: Vec<f64>,
    /// Total weight per output pixel, per colour: how deep the stack is there.
    pub coverage: Vec<Vec<f32>>,
    pub frames: usize,
    pub seconds: f64,
}

/// Decodes each chosen frame a second time and deposits it.
///
/// A second decode rather than pixels held from the measuring pass: a run of a
/// couple of hundred frames is tens of gigabytes of them, and the budget is a
/// fraction of that.
pub fn combine(
    host: &PluginHost,
    selection: &Selection<'_>,
    masters: &MasterSet,
    options: &StackOptions,
    on: &dyn Fn(Step) -> Flow,
) -> Result<Stacked> {
    let usable: Vec<&Chosen<'_>> = selection.frames.iter().filter(|f| f.usable()).collect();
    if usable.is_empty() {
        return Err(Error::NothingToCombine);
    }
    let canvas = Canvas::covering(
        usable.iter().map(|f| (&f.read.layout, f.registration.transform)),
        CANVAS_LIMIT,
    )
    .ok_or(Error::NothingToCombine)?;
    let mosaic = Mosaic::new(&usable[0].read.layout).ok_or(Error::NothingToCombine)?;

    let started = Instant::now();
    let mut stack = Stack::new(canvas, mosaic.colours);
    let mut pedestal = vec![0f64; mosaic.colours];
    let mut pedestal_weight = 0f64;

    for (index, frame) in usable.iter().enumerate() {
        if on(Step::FrameRead { done: index, total: usable.len(), name: &frame.read.name })
            == Flow::Stop
        {
            return Err(Error::Cancelled);
        }

        let opened = host.open(&frame.read.path)?;
        let Samples::U16(raw) = opened.decode()? else {
            return Err(Error::Unmeasurable {
                name: frame.read.name.clone(),
                reason: "floating-point sensor data is not stacked yet".to_owned(),
            });
        };
        let pixels =
            apply(&raw, masters.dark.as_ref(), masters.flat.as_ref(), opened.layout()).0;

        let background = Background::fit(&frame.read.detection.sky, &mosaic);
        let scale = frame.scale.expect("a frame without a scale was refused");
        for (colour, level) in pedestal.iter_mut().enumerate() {
            *level += frame.weight * scale * background.centre(colour, opened.layout());
        }
        pedestal_weight += frame.weight;

        stack.add(
            &Contribution {
                pixels: &pixels,
                layout: opened.layout(),
                mosaic: &mosaic,
                background,
                scale,
                weight: frame.weight,
                transform: frame.registration.transform,
            },
            options.pixfrac,
        );
    }

    // The background was subtracted per frame so a moon rising does not enter
    // the stack as a gradient. One common level goes back, so the result reads
    // like an exposure rather than like a difference.
    for level in &mut pedestal {
        *level = if pedestal_weight > 0.0 { *level / pedestal_weight } else { 0.0 };
    }
    let planes = stack.finish(&pedestal);
    let coverage = (0..mosaic.colours).map(|colour| stack.coverage(colour)).collect();

    Ok(Stacked {
        canvas,
        colours: mosaic.colours,
        planes,
        pedestal,
        coverage,
        frames: stack.frames(),
        seconds: started.elapsed().as_secs_f64(),
    })
}

/// The middle of the star field, standing in for the middle of the frame.
fn centre_of(stars: &[Star]) -> (f64, f64) {
    if stars.is_empty() {
        return (0.0, 0.0);
    }
    let mut xs: Vec<f64> = stars.iter().map(|star| star.x).collect();
    let mut ys: Vec<f64> = stars.iter().map(|star| star.y).collect();
    xs.sort_by(f64::total_cmp);
    ys.sort_by(f64::total_cmp);
    ((xs[0] + xs[xs.len() - 1]) / 2.0, (ys[0] + ys[ys.len() - 1]) / 2.0)
}
