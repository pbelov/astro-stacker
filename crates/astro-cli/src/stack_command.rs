//! The `stack` subcommand: combine a registered run into one image.
//!
//! The owner's choice, and the reason there are two modes: frames are weighted
//! by quality by default, so a slightly trailed frame still contributes the
//! signal it carries instead of being thrown away whole; and an explicit
//! threshold is available for when the answer wanted is the sharp frames only.
//! The report prints what each threshold would cost before it is set, because a
//! number chosen without that is chosen blind.
//!
//! One knob spans the two things a stack can be optimised for, and both ends of
//! it are exact rather than tuned. For structure larger than the star images —
//! nebulosity, dust, a galaxy's arms — blur conserves surface brightness, so a
//! trailed frame carries exactly as much signal per unit area as a sharp one
//! and only its noise matters: the optimal weight is `1/(scale*sigma)^2` and
//! the PSF does not enter at all. For point sources the signal is concentrated
//! into the PSF's noise area, so the optimal weight divides by that area.
//! `--sharpness` runs from 0 at the first to 1 at the second. Weighting by FWHM
//! as a matter of course, which is the conventional thing to do, silently
//! throws away good extended-structure signal.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use astro_core::calibrate::{apply, fits, tiff};
use astro_core::integrate::{Background, Canvas, Contribution, DEFAULT_PIXFRAC, Stack};
use astro_core::register::{Registration, Transform, matches};
use astro_core::session::FrameKind;
use astro_core::session::mosaic::Mosaic;
use astro_core::{PluginHost, Samples};
use clap::{ArgMatches, Args};

use crate::align::{AlignArgs, align};
use crate::format;
use crate::survey::{SurveyArgs, Surveyed};

#[derive(Args, Debug)]
pub struct StackArgs {
    #[command(flatten)]
    pub scan: crate::scan_command::ScanArgs,

    #[command(flatten)]
    pub survey: SurveyArgs,

    #[command(flatten)]
    pub align: AlignArgs,

    /// Where to write the result. Created if it does not exist.
    #[arg(long, short = 'o', value_name = "DIR")]
    pub out: PathBuf,

    /// What the stack is being optimised for. 0 weights by noise alone, which
    /// is optimal for nebulosity and anything larger than a star; 1 also
    /// divides by the PSF area, which is optimal for stars. Between them it
    /// interpolates.
    #[arg(long, value_name = "0..1", default_value_t = 0.5)]
    pub sharpness: f64,

    /// Drop frames whose stars are trailed by more than this, in photosites.
    /// Without it nothing is dropped for trailing and the weights do the work.
    #[arg(long, value_name = "PX")]
    pub max_trail: Option<f64>,

    /// Drop frames whose stars are wider than this across the trail, in
    /// photosites.
    #[arg(long, value_name = "PX")]
    pub max_fwhm: Option<f64>,

    /// Drop frames that moved further than this from the reference, in
    /// photosites. The far ones are usually the framing shots, and keeping them
    /// costs most of the field.
    #[arg(long, value_name = "PX")]
    pub max_shift: Option<f64>,

    /// How wide each photosite is spread over the output grid, as a fraction of
    /// a photosite. Smaller is sharper and thinner.
    #[arg(long, value_name = "0..2", default_value_t = DEFAULT_PIXFRAC)]
    pub pixfrac: f64,

    /// Also write a stretched TIFF for looking at, beside the linear one.
    #[arg(long)]
    pub stretch: bool,

    /// Say what each frame contributed.
    #[arg(long)]
    pub each: bool,
}

/// One frame, ready to be stacked.
struct Chosen<'a> {
    read: &'a Surveyed,
    registration: Registration,
    /// Multiplies this frame's background-subtracted pixels to bring it onto
    /// the run's common brightness. `None` where too few stars were shared with
    /// the reference to measure it.
    scale: Option<f64>,
    weight: f64,
    shift: f64,
}

/// The largest output grid worth allocating, in pixels.
const CANVAS_LIMIT: usize = 400_000_000;
/// How close a star must land to be the same star, when the transform is
/// already fitted. Tighter than matching, because this is confirmation and not
/// search.
const PHOTOMETRY_RADIUS: f64 = 3.0;
/// Below this many shared stars a brightness ratio is noise wearing a number.
const MIN_PHOTOMETRY_STARS: usize = 12;

pub fn run(host: &PluginHost, args: &StackArgs, matches_of: &ArgMatches) -> Result<()> {
    if !(0.0..=2.0).contains(&args.sharpness) {
        bail!("--sharpness {} is outside 0 to 2", args.sharpness);
    }
    let survey = crate::survey::read(host, &args.scan, matches_of, &args.survey)?;
    if survey.frames.len() < 2 {
        bail!("stacking needs at least two lights, and this run offered {}", survey.frames.len());
    }
    println!(
        "  read {} in {:.1}s",
        format::plural(survey.frames.len(), "frame"),
        survey.seconds
    );

    let alignment = align(&survey.frames, &args.align)?;
    let reference = alignment.reference;
    println!(
        "  reference  {} ({} stars)",
        alignment.frames[reference].name(),
        alignment.frames[reference].stars().len()
    );
    println!(
        "  registered {} of {} in {:.1}s",
        alignment.frames.iter().filter(|f| f.registration.is_some()).count(),
        alignment.frames.len(),
        alignment.seconds
    );

    // The frame centre, so a shift is the distance the picture moved rather
    // than a translation measured from a corner.
    let anchor: Vec<_> = alignment.frames[reference].stars().to_vec();
    let (cx, cy) = centre_of(&anchor);

    let mut chosen: Vec<Chosen> = Vec::new();
    let mut refused: Vec<(String, String)> = Vec::new();
    for frame in &alignment.frames {
        let Some(registration) = frame.registration else {
            refused.push((frame.name().to_owned(), "did not register".to_owned()));
            continue;
        };
        let (dx, dy) = registration.transform.displacement_at(cx, cy);
        let shift = (dx * dx + dy * dy).sqrt();
        let scale = brightness_of(&anchor, frame.stars(), &registration.transform);
        chosen.push(Chosen {
            read: frame.read,
            registration,
            scale,
            weight: f64::NAN,
            shift,
        });
    }

    apply_thresholds(&mut chosen, &mut refused, args);
    weigh(&mut chosen, args.sharpness);
    if chosen.is_empty() {
        bail!("every frame was refused, so there is nothing to stack");
    }

    report_selection(&chosen, &refused, args);
    let written = combine(host, &survey, &chosen, args)?;
    for path in written {
        println!("  wrote      {}", path.display());
    }
    Ok(())
}

/// The brightness of one frame against the reference, from the stars they share.
///
/// Measured on stars and never on the sky, because the two do not move together:
/// thin cirrus dims the stars while *raising* the sky, by scattering city light
/// back down. A scale taken from sky level would then multiply the clear frames
/// down and the hazy ones up, which is the opposite of the correction wanted.
///
/// The flux each star carries is already independent of how blurred it is, so a
/// trailed frame does not read as a cloudy one.
fn brightness_of(reference: &[astro_core::stars::Star], frame: &[astro_core::stars::Star], transform: &Transform) -> Option<f64> {
    let pairs = matches(reference, frame, transform, PHOTOMETRY_RADIUS);
    let mut ratios: Vec<f64> = pairs
        .iter()
        .filter_map(|(f, r)| {
            let (a, b) = (reference[*r].flux, frame[*f].flux);
            (a.is_finite() && b.is_finite() && b > 0.0 && a > 0.0).then_some(a / b)
        })
        .collect();
    if ratios.len() < MIN_PHOTOMETRY_STARS {
        return None;
    }
    ratios.sort_by(f64::total_cmp);
    Some(ratios[ratios.len() / 2])
}

/// Whether a measurement fails a limit.
///
/// An unmeasurable value fails it. When the owner has asked for frames under a
/// trail of six photosites, a frame whose trail could not be measured is not a
/// frame known to be under six.
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

fn apply_thresholds(chosen: &mut Vec<Chosen>, refused: &mut Vec<(String, String)>, args: &StackArgs) {
    chosen.retain(|frame| {
        let shape = &frame.read.detection.shape;
        let reason = if frame.scale.is_none() {
            Some(format!(
                "shares fewer than {MIN_PHOTOMETRY_STARS} measurable stars with the reference, \
                 so its brightness could not be put on the same scale"
            ))
        } else if let Some(limit) = args.max_trail
            && over(shape.moments.trail(), limit)
        {
            Some(describe("trail", shape.moments.trail(), limit))
        } else if let Some(limit) = args.max_fwhm
            && over(shape.moments.minor_fwhm(), limit)
        {
            Some(describe("fwhm", shape.moments.minor_fwhm(), limit))
        } else if let Some(limit) = args.max_shift
            && frame.shift > limit
        {
            Some(format!("moved {:.0} px, over the {limit:.0} limit", frame.shift))
        } else {
            None
        };
        match reason {
            Some(why) => {
                refused.push((frame.read.name.clone(), why));
                false
            }
            None => true,
        }
    });
}

/// The weight of each frame, gauged so the median frame sits near one.
///
/// `(sigma_med / (scale * sigma))^2 * (area_med / area)^sharpness`. The scale
/// belongs inside the square because multiplying a frame up multiplies its
/// noise with it; leaving it out weights a hazy frame as though it had been
/// clear, and can make the result noisier than an unweighted mean of the good
/// frames alone.
fn weigh(chosen: &mut [Chosen], sharpness: f64) {
    let area_of = |frame: &Chosen| {
        let m = frame.read.detection.shape.moments;
        // The registration residual is a real blur and belongs in the area: at
        // 0.34 px it costs 4% of the cross-trail width and at 1.56 px it costs
        // 60%. Half the squared 2-D radius is the per-axis variance.
        let spread = frame.registration.residual * frame.registration.residual / 2.0;
        let determinant =
            (m.m11 + spread) * (m.m22 + spread) - m.m12 * m.m12;
        (determinant > 0.0).then(|| 4.0 * std::f64::consts::PI * determinant.sqrt())
    };

    let mut noises: Vec<f64> = chosen
        .iter()
        .filter_map(|f| {
            let sigma = f64::from(f.read.detection.shape.noise) * f.scale.unwrap_or(f64::NAN);
            sigma.is_finite().then_some(sigma)
        })
        .collect();
    let mut areas: Vec<f64> = chosen.iter().filter_map(area_of).collect();
    let median = |values: &mut Vec<f64>| {
        if values.is_empty() {
            return f64::NAN;
        }
        values.sort_by(f64::total_cmp);
        values[values.len() / 2]
    };
    let (noise_gauge, area_gauge) = (median(&mut noises), median(&mut areas));

    for frame in chosen.iter_mut() {
        let sigma = f64::from(frame.read.detection.shape.noise) * frame.scale.unwrap_or(f64::NAN);
        let area = area_of(frame);
        frame.weight = match (sigma.is_finite() && sigma > 0.0, area) {
            (true, Some(area)) if area > 0.0 => {
                (noise_gauge / sigma).powi(2) * (area_gauge / area).powf(sharpness)
            }
            // A frame whose noise or shape could not be measured cannot be
            // weighed against the others. It is not given an average weight:
            // that would be a guess dressed as a measurement.
            _ => f64::NAN,
        };
    }
}

fn report_selection(chosen: &[Chosen], refused: &[(String, String)], args: &StackArgs) {
    if !refused.is_empty() {
        println!("\n{} refused:", format::plural(refused.len(), "frame"));
        for (name, why) in refused {
            println!("  {name:<24} {why}");
        }
    }

    let usable = chosen.iter().filter(|f| f.weight.is_finite() && f.weight > 0.0).count();
    println!("\nstacking {} of {}", usable, chosen.len() + refused.len());
    if usable < chosen.len() {
        println!(
            "  {} could not be weighed and will not contribute",
            chosen.len() - usable
        );
    }

    let mut weights: Vec<f64> = chosen.iter().map(|f| f.weight).filter(|w| w.is_finite()).collect();
    let mut scales: Vec<f64> = chosen.iter().filter_map(|f| f.scale).collect();
    weights.sort_by(f64::total_cmp);
    scales.sort_by(f64::total_cmp);
    if !weights.is_empty() {
        println!(
            "  weight     {:.2} at the median, {:.2} to {:.2} over the run (sharpness {})",
            weights[weights.len() / 2],
            weights[0],
            weights[weights.len() - 1],
            args.sharpness
        );
        // The effective count: how many frames of median quality this run is
        // worth. Lower than the frame count whenever the weights are uneven,
        // and the honest answer to "how deep is this".
        let total: f64 = weights.iter().sum();
        let squares: f64 = weights.iter().map(|w| w * w).sum();
        println!(
            "  depth      {} frames of median quality out of {} stacked",
            (total * total / squares).round() as usize,
            weights.len()
        );
    }
    if !scales.is_empty() {
        println!(
            "  brightness {:.3} at the median, {:.3} to {:.3} - the run's transparency",
            scales[scales.len() / 2],
            scales[0],
            scales[scales.len() - 1]
        );
    }

    if args.each {
        println!();
        for frame in chosen {
            let shape = &frame.read.detection.shape;
            println!(
                "  {:<24} weight {:>7.3}  brightness {:>6.3}  trail {:>5.2}  fwhm {:>5.2}  \
                 shift {:>7.0}",
                frame.read.name,
                frame.weight,
                frame.scale.unwrap_or(f64::NAN),
                shape.moments.trail(),
                shape.moments.minor_fwhm(),
                frame.shift
            );
        }
    }
}

fn combine(
    host: &PluginHost,
    survey: &crate::survey::Survey,
    chosen: &[Chosen],
    args: &StackArgs,
) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(&args.out)
        .with_context(|| format!("creating {}", args.out.display()))?;

    let usable: Vec<&Chosen> =
        chosen.iter().filter(|f| f.weight.is_finite() && f.weight > 0.0).collect();
    let canvas = Canvas::covering(
        usable.iter().map(|f| (&f.read.layout, f.registration.transform)),
        CANVAS_LIMIT,
    )
    .context("the frames do not describe a grid to stack onto")?;
    let mosaic = Mosaic::new(&usable[0].read.layout)
        .context("the reference frame is not a mosaic this can stack")?;

    println!(
        "\n  canvas     {} by {} ({:.1} Mpx), {:.2} GiB of accumulators",
        canvas.width,
        canvas.height,
        (canvas.width * canvas.height) as f64 / 1e6,
        (canvas.width * canvas.height * mosaic.colours * 2 * 4) as f64 / (1u64 << 30) as f64
    );

    let mut stack = Stack::new(canvas, mosaic.colours);
    let started = Instant::now();
    let mut pedestal = vec![0f64; mosaic.colours];
    let mut pedestal_weight = 0f64;

    for frame in &usable {
        let opened = host
            .open(&frame.read.path)
            .with_context(|| format!("opening {}", frame.read.path.display()))?;
        let Samples::U16(raw) = opened
            .decode()
            .with_context(|| format!("decoding {}", frame.read.path.display()))?
        else {
            bail!("{}: floating-point sensor data is not stacked yet", frame.read.name);
        };
        let pixels = apply(
            &raw,
            survey.masters.dark.as_ref(),
            survey.masters.flat.as_ref(),
            opened.layout(),
        )
        .0;

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
            args.pixfrac,
        );
    }

    // The background was subtracted per frame so that a moon rising does not
    // enter the stack as a gradient. One common level goes back, so the result
    // reads like an exposure rather than like a difference, and the FITS says
    // what it was.
    for level in &mut pedestal {
        *level = if pedestal_weight > 0.0 { *level / pedestal_weight } else { 0.0 };
    }
    let planes = stack.finish(&pedestal);
    println!(
        "  combined   {} frames in {:.1}s",
        stack.frames(),
        started.elapsed().as_secs_f64()
    );

    // Per plane, because they are not equally covered and reporting only the
    // dominant one would flatter the result: green photosites are half of a
    // Bayer mosaic and red and blue a quarter each, so the red plane is the one
    // that says whether the run dithered enough to fill the grid.
    println!("  coverage   per plane, and how evenly deep");
    for colour in 0..mosaic.colours {
        let coverage = stack.coverage(colour);
        let mut depths: Vec<f32> =
            coverage.iter().copied().filter(|weight| *weight > 0.0).collect();
        if depths.is_empty() {
            println!("    plane {colour}  nothing reached it at all");
            continue;
        }
        let filled = depths.len() as f64 / coverage.len() as f64 * 100.0;
        depths.sort_by(f32::total_cmp);
        let at = |f: f64| depths[((depths.len() - 1) as f64 * f) as usize];
        println!(
            "    plane {colour}  {filled:5.1}% filled, {:.2} frames deep at the median, \
             {:.2} at the thinnest tenth",
            at(0.5),
            at(0.1)
        );
    }

    write_result(&planes, canvas, &mosaic, chosen, &pedestal, args)
}

fn write_result(
    planes: &[Vec<f32>],
    canvas: Canvas,
    mosaic: &Mosaic,
    chosen: &[Chosen],
    pedestal: &[f64],
    args: &StackArgs,
) -> Result<Vec<PathBuf>> {
    let first = chosen.first().context("nothing was stacked")?;
    let header = fits::Header {
        image_type: FrameKind::Light.name().to_owned(),
        instrument: first.read.camera_model.clone(),
        exposure: first.read.exposure_seconds,
        iso: first.read.iso,
        frames: chosen.len(),
        combination: format!("weighted mean, sharpness {}", args.sharpness),
        // Deliberately absent: the planes are already separated by colour, so a
        // reader that debayered them would be debayering three images that have
        // each already been demosaiced by the stacking.
        bayer_pattern: None,
        notes: vec![
            format!("pedestal restored: {pedestal:?} ADU per plane"),
            format!("drizzle pixfrac {}", args.pixfrac),
            "NaN where no frame covered the pixel".to_owned(),
        ],
    };

    let mut written = Vec::new();
    let borrowed: Vec<&[f32]> = planes.iter().map(|plane| plane.as_slice()).collect();
    let target = args.out.join("stack.fits");
    fits::write_planes(&target, &borrowed, canvas.width, canvas.height, &header)
        .with_context(|| format!("writing {}", target.display()))?;
    written.push(target);

    // White balance goes on the TIFF and never on the FITS. A raw green
    // photosite collects roughly twice what a red or blue one does from the same
    // white light, so an unbalanced stack is violently green — true, and useless
    // to look at. The FITS keeps the sensor's own numbers because that is the
    // measurement; the TIFF exists to be looked at, and says in its description
    // what was applied.
    let (balanced, multipliers, balance) = white_balance(planes, &first.read.layout, mosaic);
    let shown: Vec<&[f32]> = balanced.iter().map(|plane| plane.as_slice()).collect();
    // The pedestal was measured before the balance and has to move with it, or
    // the levelling below would subtract the wrong sky.
    let pedestal: Vec<f64> = pedestal
        .iter()
        .enumerate()
        .map(|(colour, level)| level * f64::from(multipliers.get(colour).copied().unwrap_or(1.0)))
        .collect();
    let pedestal = &pedestal[..];

    // Full scale is the brightest thing the stack actually holds, rather than a
    // round number the data would clip against.
    let mut full = 0f32;
    for plane in &shown {
        for value in *plane {
            if value.is_finite() && *value > full {
                full = *value;
            }
        }
    }
    let image = tiff::Image {
        width: canvas.width,
        height: canvas.height,
        planes: &shown[..mosaic.colours.min(3)],
    };
    let target = args.out.join("stack.tif");
    tiff::write(&target, &image, &tiff::Mapping::Linear { full }, &balance)
        .with_context(|| format!("writing {}", target.display()))?;
    written.push(target);

    if args.stretch {
        // The viewing copy also has its background levelled between the
        // channels, which the linear one deliberately does not. The sky is not
        // a white source - light pollution is orange and airglow is green - so
        // a white balance taken from daylight leaves it coloured, and a single
        // black point across three coloured skies renders the whole frame in
        // whichever channel sits highest above it. Levelling is a decision
        // about how to look at the picture, not a measurement, so it goes only
        // in the file that says it is not to be measured from.
        let (levelled, common) = level_background(&balanced, pedestal, mosaic);
        // A little below the sky rather than exactly on it. Put the black point
        // on the sky and half the noise clips to zero, which reads as a clean
        // background but has thrown away the faintest half of everything. The
        // percentile is taken from the data so it follows the run rather than a
        // number chosen once.
        let floor = percentile(&levelled[mosaic.dominant], 0.02).min(common);
        let shown: Vec<&[f32]> = levelled.iter().map(|plane| plane.as_slice()).collect();
        let target = args.out.join("stack_view.tif");
        tiff::write(
            &target,
            &tiff::Image {
                width: canvas.width,
                height: canvas.height,
                planes: &shown[..mosaic.colours.min(3)],
            },
            &tiff::Mapping::Asinh { black: floor, white: full, softening: 200.0 },
            "astro-stacker stack, white balanced and its background levelled between the \
             channels, then stretched. For viewing only.",
        )
        .with_context(|| format!("writing {}", target.display()))?;
        written.push(target);
    }
    Ok(written)
}

/// The planes with the camera's as-shot white balance applied, and a sentence
/// saying what was done.
///
/// Gauged on the dominant colour so green is left alone and the other two are
/// lifted to meet it: that keeps the numbers near the ones in the FITS, and it
/// is the multiplier the camera itself recorded rather than one chosen to make
/// the picture look right.
fn white_balance(
    planes: &[Vec<f32>],
    layout: &astro_core::ImageLayout,
    mosaic: &Mosaic,
) -> (Vec<Vec<f32>>, Vec<f32>, String) {
    let gauge = layout.wb_coeffs.get(mosaic.dominant).copied().unwrap_or(f32::NAN);
    let usable = gauge.is_finite()
        && gauge > 0.0
        && (0..mosaic.colours)
            .all(|c| layout.wb_coeffs.get(c).is_some_and(|v| v.is_finite() && *v > 0.0));
    if !usable {
        // Not recorded, so nothing is applied and nothing is invented. A green
        // picture that says why beats a neutral one built on a guess.
        return (
            planes.to_vec(),
            vec![1.0; planes.len()],
            "astro-stacker stack. The camera recorded no white balance, so none was applied \
             and the raw green cast is the sensor's own."
                .to_owned(),
        );
    }

    let multipliers: Vec<f32> =
        (0..planes.len()).map(|c| layout.wb_coeffs[c.min(3)] / gauge).collect();
    let balanced = planes
        .iter()
        .zip(&multipliers)
        .map(|(plane, gain)| plane.iter().map(|value| value * gain).collect())
        .collect();
    let listed: Vec<String> = multipliers.iter().map(|m| format!("{m:.3}")).collect();
    (
        balanced,
        multipliers,
        format!(
            "astro-stacker stack, with the camera's as-shot white balance applied \
             ({}). The FITS beside it has not been balanced.",
            listed.join(", ")
        ),
    )
}

/// One quantile of a plane, from a sample: the exact answer would mean sorting
/// forty million values to choose a black point.
fn percentile(plane: &[f32], fraction: f64) -> f32 {
    let mut sample: Vec<f32> =
        plane.iter().copied().step_by(97).filter(|value| value.is_finite()).collect();
    if sample.is_empty() {
        return 0.0;
    }
    sample.sort_by(f32::total_cmp);
    sample[((sample.len() - 1) as f64 * fraction) as usize]
}

/// Every channel's sky moved to one level, so the background is grey and the
/// stars keep their colours.
///
/// Returns the level everything was moved to, which is where a stretch must put
/// its black point.
fn level_background(
    planes: &[Vec<f32>],
    pedestal: &[f64],
    mosaic: &Mosaic,
) -> (Vec<Vec<f32>>, f32) {
    // Green is left where it is and the others are brought to meet it, so the
    // numbers stay near the ones in the FITS.
    let common = pedestal.get(mosaic.dominant).copied().unwrap_or(0.0) as f32;
    let levelled = planes
        .iter()
        .enumerate()
        .map(|(colour, plane)| {
            let own = pedestal.get(colour).copied().unwrap_or(0.0) as f32;
            plane.iter().map(|value| value - own + common).collect()
        })
        .collect();
    (levelled, common)
}

/// The middle of the star field, standing in for the middle of the frame.
fn centre_of(stars: &[astro_core::stars::Star]) -> (f64, f64) {
    if stars.is_empty() {
        return (0.0, 0.0);
    }
    let mut xs: Vec<f64> = stars.iter().map(|star| star.x).collect();
    let mut ys: Vec<f64> = stars.iter().map(|star| star.y).collect();
    xs.sort_by(f64::total_cmp);
    ys.sort_by(f64::total_cmp);
    ((xs[0] + xs[xs.len() - 1]) / 2.0, (ys[0] + ys[ys.len() - 1]) / 2.0)
}
