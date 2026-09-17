//! Choosing which frames go into the stack, what each is worth, and combining
//! them.
//!
//! The selection is deliberately not a filter with a hidden rule. Frames are
//! weighted by quality by default, so a slightly trailed one still contributes
//! what it carries; a threshold is available for when the answer wanted is the
//! sharp frames only. Both live here, and what a threshold would cost is
//! computable before it is set — a limit chosen without seeing that is chosen
//! blind.

use std::sync::mpsc;
use std::time::Instant;

use crate::frame::ImageLayout;

use crate::calibrate::apply_into;
use crate::error::{Error, Result};
use crate::integrate::{Background, Canvas, Contribution, DEFAULT_PIXFRAC, Guide, Rejection, Stack};
use crate::register::{Registration, Transform, matches};
use crate::session::mosaic::Mosaic;
use crate::stars::Star;
use crate::{Formats, Samples};

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
    /// `None` leaves every sample in. Rejection costs a second pass over the
    /// frames, which is a second decode, so it is the user's to ask for.
    pub rejection: Option<Rejection>,
}

impl Default for StackOptions {
    fn default() -> Self {
        Self {
            sharpness: 0.5,
            max_trail: None,
            max_fwhm: None,
            max_shift: None,
            pixfrac: DEFAULT_PIXFRAC,
            rejection: None,
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
    /// How many deposits the second pass threw away, and out of how many.
    /// `None` where rejection was not asked for.
    pub rejected: Option<(usize, usize)>,
    /// How many frames were decoded ahead of the deposit. Reported for the same
    /// reason [`super::Survey::workers`] is: derived from a memory budget and a
    /// guess about the machine, not chosen.
    pub look_ahead: usize,
    /// Frames that lost an unusual share of their samples, worst first.
    ///
    /// Reported rather than counted: a frame losing a large fraction is not a
    /// frame full of satellites, it is a sign the threshold does not fit the
    /// run, and only the user can tell those apart.
    pub heavy_losses: Vec<(String, f64)>,
}

/// Decodes each chosen frame a second time and deposits it.
///
/// A second decode rather than pixels held from the measuring pass: a run of a
/// couple of hundred frames is tens of gigabytes of them, and the budget is a
/// fraction of that.
/// Decodes each chosen frame again and deposits it, once or twice.
///
/// A second decode rather than pixels held from the measuring pass: a run of a
/// couple of hundred frames is tens of gigabytes of them, and the budget is a
/// fraction of that. Rejection costs one more decode again, which is why it is
/// asked for rather than assumed.
pub fn combine(
    host: &Formats,
    selection: &Selection<'_>,
    masters: &MasterSet,
    options: &StackOptions,
    on: &dyn Fn(Step) -> Flow,
) -> Result<Stacked> {
    combine_with_look_ahead(host, selection, masters, options, None, on)
}

/// [`combine`], with the look-ahead named rather than derived.
///
/// `Some(0)` decodes on the depositing thread. It is kept reachable because it
/// is the only way to show that a run stacked from frames arriving on several
/// threads produces the same bits as one stacked from frames arriving on none.
#[allow(clippy::too_many_arguments)]
pub fn combine_with_look_ahead(
    host: &Formats,
    selection: &Selection<'_>,
    masters: &MasterSet,
    options: &StackOptions,
    look_ahead: Option<usize>,
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
    let passes = if options.rejection.is_some() { 2 } else { 1 };
    let look_ahead =
        look_ahead.unwrap_or_else(|| look_ahead_lanes(&usable, DEFAULT_LOOK_AHEAD_BYTES));
    let mut pass =
        deposit(host, &usable, masters, options, canvas, &mosaic, 1, passes, look_ahead, None, on)?;

    let (rejected, heavy_losses) = match options.rejection {
        None => (None, Vec::new()),
        Some(rejection) => {
            // A single frame's sky noise, which is the scale a sample is judged
            // against. The median over the run rather than any one frame's,
            // because the guide's ceiling is a property of the stack while the
            // clip itself uses each frame's own.
            let mut noises: Vec<f64> =
                usable.iter().map(|frame| noise_of(frame)).filter(|n| n.is_finite()).collect();
            noises.sort_by(f64::total_cmp);
            let noise = if noises.is_empty() { f64::NAN } else { noises[noises.len() / 2] };
            // The first pass becomes the guide rather than being kept
            // beside one: its cells are most of a gigabyte at a full canvas.
            let guide = pass.stack.into_guide(noise, &rejection);

            pass = deposit(
                host,
                &usable,
                masters,
                options,
                canvas,
                &mosaic,
                2,
                passes,
                look_ahead,
                Some((&guide, &rejection)),
                on,
            )?;

            let mut heavy: Vec<(String, f64)> = pass
                .losses
                .iter()
                .filter(|(_, dropped, considered)| {
                    *considered > 0 && *dropped as f64 / *considered as f64 > HEAVY_LOSS
                })
                .map(|(name, dropped, considered)| {
                    (name.clone(), *dropped as f64 / *considered as f64)
                })
                .collect();
            heavy.sort_by(|a, b| b.1.total_cmp(&a.1));
            (Some((pass.dropped, pass.considered)), heavy)
        }
    };

    let planes = pass.stack.finish(&pass.pedestal);
    let coverage = (0..mosaic.colours).map(|colour| pass.stack.coverage(colour)).collect();

    Ok(Stacked {
        canvas,
        colours: mosaic.colours,
        planes,
        pedestal: pass.pedestal,
        coverage,
        frames: pass.stack.frames(),
        seconds: started.elapsed().as_secs_f64(),
        rejected,
        look_ahead,
        heavy_losses,
    })
}

/// A frame losing more than this share of its samples is named.
///
/// Well above what noise alone produces at three sigma, and well below what a
/// frame crossed by a satellite loses. Between the two it means the threshold
/// does not fit the run, which is the user's business and not something to
/// swallow.
const HEAVY_LOSS: f64 = 0.02;

struct Pass {
    stack: Stack,
    pedestal: Vec<f64>,
    dropped: usize,
    considered: usize,
    losses: Vec<(String, usize, usize)>,
}

/// Calibrated planes one lane owns.
///
/// Two, and the pair is the point: a lane fills one while the deposit is
/// reading the other. With one it would hand over its only buffer and wait for
/// it back, which is the sequential pass with a thread attached to it.
const PLANES_PER_LANE: usize = 2;

/// What one lane holds while a pass runs, as a multiple of one frame's raw
/// samples: [`PLANES_PER_LANE`] calibrated planes at four bytes a sample, plus
/// the two-byte raw its decode is holding at the time.
///
/// Unlike the survey's peak this is resident rather than transient — the planes
/// are allocated once and refilled for the length of the run.
const LANE_MULTIPLE_OF_RAW: u64 = 1 + 2 * PLANES_PER_LANE as u64;

/// How many frames may be decoded ahead of the deposit.
///
/// The tempting argument is that the deposit already spreads over every core, so
/// a lane must be a thread taken from it and two or three is all that can pay.
/// Measurement says eight, and it is worth saying plainly that the reason is not
/// known. The explanation this comment first offered was the deposit's band
/// split leaving the machine part idle. That has since been tested by cutting a
/// frame into anywhere from thirty-two bands to two hundred and fifty-six, which
/// moved the pass by less than its own run-to-run spread — so the band split is
/// not it, and the sentence is gone rather than reworded. What is left is that
/// the deposit is limited by something other than the cores it occupies, and a
/// thread that is only decoding does not compete with it for that.
///
/// Where the gain flattens depends on how a frame's cost divides between its
/// decode and its deposit, which moves with the body and with the canvas — hence
/// the override on [`combine_with_look_ahead`] and the number reported in
/// [`Stacked::look_ahead`]. Eight is where it flattened on the body this project
/// is measured against; it is a measurement on one machine, not a law.
const MAX_LOOK_AHEAD: usize = 8;

/// Cap on decoded pixel data held ahead of the deposit.
///
/// Deliberately below the survey's budget, and for a different reason than the
/// survey's is what it is. A lane runs beside a deposit that already holds the
/// accumulator — most of a gigabyte of cells at a full canvas, and a guide
/// beside it on the second pass — so what this bounds is what can be held *as
/// well as* that, rather than what a pass may hold on its own. It is also what
/// keeps a large body from opening as many lanes as a small one: the frames are
/// bigger, so fewer fit.
const DEFAULT_LOOK_AHEAD_BYTES: u64 = 2 << 30;

/// The deposit's end of one lane: where filled planes arrive, and where the
/// empty ones go back.
struct Lane {
    filled: mpsc::Receiver<Ready>,
    free: mpsc::SyncSender<Vec<f32>>,
}

/// One frame, decoded and calibrated, on its way to the stack.
struct Ready {
    /// Carried only so that a debug build says so if the rotation ever slips.
    /// The order is structural rather than checked — see [`deposit`].
    index: usize,
    /// Travels with the outcome rather than beside it, so that a frame which
    /// failed still hands back the buffer it was lent.
    plane: Vec<f32>,
    /// `Err` where this frame could not be read. Carried to its turn rather than
    /// raised where it happened: lanes run ahead, so a later frame can fail
    /// before an earlier one is even opened, and a run with two unreadable
    /// frames has to name the same one it named when it was sequential.
    outcome: Result<ImageLayout>,
}

/// Everything one frame needs before it can be deposited, into a buffer the
/// caller owns.
///
/// Pure, and per frame, which is the whole licence for running it ahead of the
/// deposit. The `OpenFrame` is made and dropped inside this call, so the plugin
/// handle that the ABI says belongs to one thread never leaves the thread that
/// made it.
fn prepare(
    host: &Formats,
    frame: &Chosen<'_>,
    masters: &MasterSet,
    plane: &mut Vec<f32>,
) -> Result<ImageLayout> {
    let opened = host.open(&frame.read.path)?;
    let Samples::U16(raw) = opened.decode()? else {
        return Err(Error::Unmeasurable {
            name: frame.read.name.clone(),
            reason: "floating-point sensor data is not stacked yet".to_owned(),
        });
    };
    apply_into(&raw, masters.dark.as_ref(), masters.flat.as_ref(), opened.layout(), plane);
    // `raw` dies here, on the lane. Shipping it onward instead would leave the
    // calibration in front of the deposit on one thread, and that is half of
    // what is being moved out of its way.
    Ok(*opened.layout())
}

/// How many frames to decode ahead, from the frame size and the machine.
fn look_ahead_lanes(usable: &[&Chosen<'_>], budget_bytes: u64) -> usize {
    let widest = usable
        .iter()
        .filter_map(|frame| frame.read.layout.required_bytes())
        .max()
        .unwrap_or(1 << 26) as u64;
    let per_lane = (widest * LANE_MULTIPLE_OF_RAW).max(1);
    // A quarter of the logical count, where the survey takes half: a lane is
    // filling the gaps the deposit's band split leaves, and there are fewer of
    // those than there are cores.
    let ceiling = std::thread::available_parallelism().map_or(1, |n| n.get() / 4).max(1);
    (budget_bytes / per_lane).clamp(1, ceiling.min(MAX_LOOK_AHEAD) as u64) as usize
}

#[allow(clippy::too_many_arguments)]
fn deposit(
    host: &Formats,
    usable: &[&Chosen<'_>],
    masters: &MasterSet,
    options: &StackOptions,
    canvas: Canvas,
    mosaic: &Mosaic,
    pass: usize,
    passes: usize,
    look_ahead: usize,
    judge: Option<(&Guide, &Rejection)>,
    on: &dyn Fn(Step) -> Flow,
) -> Result<Pass> {
    let mut stack = Stack::new(canvas, mosaic.colours);
    let mut pedestal = vec![0f64; mosaic.colours];
    let mut pedestal_weight = 0f64;
    let (mut dropped, mut considered) = (0usize, 0usize);
    let mut losses = Vec::with_capacity(usable.len());

    // Decoding runs ahead on its own threads; depositing does not. The stack's
    // cells are a running float sum per output pixel, and float addition is not
    // associative, so the order the frames are added in is part of the answer.
    //
    // Here that order is structural rather than restored: lane `l` takes frames
    // `l`, `l + lanes`, `l + 2 * lanes` and so on, in that order down its own
    // channel, and the loop asks lane `index % lanes` for its next frame at step
    // `index`. So the k-th thing a lane sends is the k-th thing the loop takes
    // from it, and nothing has to be sorted or buffered to put the run back in
    // order.
    //
    // Plain threads rather than rayon, and the loop stays on the thread that
    // called in. `Stack::add` sizes its bands from `rayon::current_num_threads`
    // and splits them over the global pool; a lane parked inside the plugin's
    // decode is not a task anything can steal from, so lanes on that pool would
    // leave the deposit cutting bands for workers that are inside an FFI call.
    std::thread::scope(|scope| -> Result<()> {
        // Declared here so that every way out of this closure — the `?` on an
        // unreadable frame included — drops them before the scope joins. A lane
        // waiting for a plane that will never come back is woken by its end of
        // the channel going away, and unwinds instead of parking a thread the
        // join is waiting on.
        let mut lanes: Vec<Lane> = Vec::with_capacity(look_ahead);
        for lane in 0..look_ahead {
            let (free, spare) = mpsc::sync_channel::<Vec<f32>>(PLANES_PER_LANE);
            let (ready, filled) = mpsc::sync_channel::<Ready>(PLANES_PER_LANE - 1);
            for _ in 0..PLANES_PER_LANE {
                // The channel holds exactly the planes this lane owns, so
                // priming it cannot block.
                free.send(Vec::new()).expect("the lane's own end is still open");
            }
            scope.spawn(move || {
                for index in (lane..usable.len()).step_by(look_ahead) {
                    let Ok(mut plane) = spare.recv() else { return };
                    let outcome = prepare(host, usable[index], masters, &mut plane);
                    if ready.send(Ready { index, plane, outcome }).is_err() {
                        return;
                    }
                }
            });
            lanes.push(Lane { filled, free });
        }

        // The no-lane path is this same loop with nothing to take from, and it
        // reuses one plane just as a lane does. So it is the pipeline with the
        // pipe removed rather than the pass that stood here before, which is
        // what makes it the reference the pipelined one has to match bit for
        // bit.
        let mut solo = Vec::new();

        for (index, frame) in usable.iter().enumerate() {
            let step = Step::Stacking {
                pass,
                passes,
                done: index,
                total: usable.len(),
                name: &frame.read.name,
            };
            // Still asked before the frame is waited for, so that a stop does
            // not have to sit out a decode already under way.
            if on(step) == Flow::Stop {
                return Err(Error::Cancelled);
            }

            let (mut plane, layout, lent) = if lanes.is_empty() {
                let plane = std::mem::take(&mut solo);
                let mut plane = plane;
                let layout = prepare(host, frame, masters, &mut plane)?;
                (plane, layout, None)
            } else {
                let lane = &lanes[index % lanes.len()];
                let Ok(ready) = lane.filled.recv() else {
                    // A lane stops sending its share only by panicking, and the
                    // scope re-raises that when it joins, so this is never the
                    // value that surfaces. Returning rather than panicking again
                    // leaves the original panic as the one the caller sees.
                    return Err(Error::Cancelled);
                };
                let Ready { index: sent, plane, outcome } = ready;
                debug_assert_eq!(sent, index, "the lane rotation slipped");
                (plane, outcome?, Some(lane))
            };

            let background = Background::fit(&frame.read.detection.sky, mosaic);
            let scale = frame.scale.expect("a frame without a scale was refused");
            for (colour, level) in pedestal.iter_mut().enumerate() {
                *level += frame.weight * scale * background.centre(colour, &layout);
            }
            pedestal_weight += frame.weight;

            let contribution = Contribution {
                pixels: &plane,
                layout: &layout,
                mosaic,
                background,
                scale,
                weight: frame.weight,
                noise: noise_of(frame),
                transform: frame.registration.transform,
            };
            match judge {
                None => stack.add(&contribution, options.pixfrac),
                Some((guide, rejection)) => {
                    let (lost, seen) =
                        stack.add_checked(&contribution, options.pixfrac, guide, rejection);
                    dropped += lost;
                    considered += seen;
                    losses.push((frame.read.name.clone(), lost, seen));
                }
            }

            // Back to the lane it came from, which is what keeps the pass to a
            // fixed number of full-frame buffers however long the run is.
            match lent {
                Some(lane) => {
                    let _ = lane.free.send(std::mem::take(&mut plane));
                }
                None => solo = plane,
            }
        }
        Ok(())
    })?;

    // The background was subtracted per frame so a moon rising does not enter
    // the stack as a gradient. One common level goes back, so the result reads
    // like an exposure rather than like a difference.
    for level in &mut pedestal {
        *level = if pedestal_weight > 0.0 { *level / pedestal_weight } else { 0.0 };
    }
    Ok(Pass { stack, pedestal, dropped, considered, losses })
}

/// One frame's sky noise once it has been brought onto the run's brightness.
fn noise_of(frame: &Chosen<'_>) -> f64 {
    f64::from(frame.read.detection.shape.noise) * frame.scale.unwrap_or(f64::NAN)
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
