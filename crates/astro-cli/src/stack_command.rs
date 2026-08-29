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

use anyhow::{Context, Result, bail};
use astro_core::PluginHost;
use astro_core::calibrate::{fits, tiff};
use astro_core::integrate::DEFAULT_PIXFRAC;
use astro_core::pipeline::stack::{Selection, StackOptions, Stacked, combine, select};
use astro_core::pipeline::view;
use astro_core::pipeline::{Flow, Step};
use astro_core::session::FrameKind;
use astro_core::session::mosaic::Mosaic;
use clap::{ArgMatches, Args};

use crate::align::{AlignArgs, align};
use crate::format;
use crate::survey::SurveyArgs;

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

pub fn run(host: &PluginHost, args: &StackArgs, matches_of: &ArgMatches) -> Result<()> {
    if !(0.0..=2.0).contains(&args.sharpness) {
        bail!("--sharpness {} is outside 0 to 2", args.sharpness);
    }
    let survey = crate::survey::read(host, &args.scan, matches_of, &args.survey)?;
    println!("  read {} in {:.1}s", format::plural(survey.frames.len(), "frame"), survey.seconds);

    let alignment = align(&survey.frames, &args.align)?;
    println!(
        "  reference  {} ({} stars)",
        alignment.frames[alignment.reference].name(),
        alignment.frames[alignment.reference].stars().len()
    );
    println!(
        "  registered {} of {} in {:.1}s",
        alignment.frames.iter().filter(|f| f.registration.is_some()).count(),
        alignment.frames.len(),
        alignment.seconds
    );

    let options = StackOptions {
        sharpness: args.sharpness,
        max_trail: args.max_trail,
        max_fwhm: args.max_fwhm,
        max_shift: args.max_shift,
        pixfrac: args.pixfrac,
    };
    let selection = select(&alignment, &options);
    if selection.frames.is_empty() {
        bail!("every frame was refused, so there is nothing to stack");
    }
    report_selection(&selection, args);

    std::fs::create_dir_all(&args.out)
        .with_context(|| format!("creating {}", args.out.display()))?;
    let stacked = combine(host, &selection, &survey.masters, &options, &|step| {
        if let Step::FrameRead { done, total, .. } = step
            && !args.scan.quiet
        {
            eprint!("\r  stacking {done} of {total}");
        }
        Flow::Continue
    })?;
    if !args.scan.quiet {
        eprint!("\r                                        \r");
    }
    report_stack(&stacked);

    for path in write_result(&stacked, &selection, args)? {
        println!("  wrote      {}", path.display());
    }
    Ok(())
}

fn report_selection(selection: &Selection<'_>, args: &StackArgs) {
    if !selection.refused.is_empty() {
        println!("\n{} refused:", format::plural(selection.refused.len(), "frame"));
        for (name, why) in &selection.refused {
            println!("  {name:<24} {why}");
        }
    }

    let usable = selection.frames.iter().filter(|f| f.usable()).count();
    println!("\nstacking {} of {}", usable, selection.frames.len() + selection.refused.len());
    if usable < selection.frames.len() {
        println!("  {} could not be weighed and will not contribute", selection.frames.len() - usable);
    }

    let mut weights: Vec<f64> =
        selection.frames.iter().map(|f| f.weight).filter(|w| w.is_finite()).collect();
    let mut scales: Vec<f64> = selection.frames.iter().filter_map(|f| f.scale).collect();
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
        for frame in &selection.frames {
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

fn report_stack(stacked: &Stacked) {
    println!(
        "\n  canvas     {} by {} ({:.1} Mpx)",
        stacked.canvas.width,
        stacked.canvas.height,
        (stacked.canvas.width * stacked.canvas.height) as f64 / 1e6
    );
    println!("  combined   {} frames in {:.1}s", stacked.frames, stacked.seconds);
    // Per plane, because they are not equally covered and reporting only the
    // dominant one would flatter the result: green photosites are half of a
    // Bayer mosaic and red and blue a quarter each, so the red plane is the one
    // that says whether the run dithered enough to fill the grid.
    println!("  coverage   per plane, and how evenly deep");
    for (colour, coverage) in stacked.coverage.iter().enumerate() {
        let mut depths: Vec<f32> = coverage.iter().copied().filter(|w| *w > 0.0).collect();
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
}

fn write_result(
    stacked: &Stacked,
    selection: &Selection<'_>,
    args: &StackArgs,
) -> Result<Vec<PathBuf>> {
    let first = selection.frames.first().context("nothing was stacked")?;
    let mosaic = Mosaic::new(&first.read.layout).context("the frames are not a mosaic")?;
    let (width, height) = (stacked.canvas.width, stacked.canvas.height);

    let header = fits::Header {
        image_type: FrameKind::Light.name().to_owned(),
        instrument: first.read.camera_model.clone(),
        exposure: first.read.exposure_seconds,
        iso: first.read.iso,
        frames: stacked.frames,
        combination: format!("weighted mean, sharpness {}", args.sharpness),
        // Deliberately absent: the planes are already separated by colour, so a
        // reader that debayered them would be debayering three images that have
        // each already been demosaiced by the stacking.
        bayer_pattern: None,
        notes: vec![
            format!("pedestal restored: {:?} ADU per plane", stacked.pedestal),
            format!("drizzle pixfrac {}", args.pixfrac),
            "NaN where no frame covered the pixel".to_owned(),
        ],
    };

    let mut written = Vec::new();
    let borrowed: Vec<&[f32]> = stacked.planes.iter().map(|plane| plane.as_slice()).collect();
    let target = args.out.join("stack.fits");
    fits::write_planes(&target, &borrowed, width, height, &header)
        .with_context(|| format!("writing {}", target.display()))?;
    written.push(target);

    // Linear, white balanced, background left where it is: this copy is meant
    // to be handed to another program, and the description says how to undo it.
    let linear = view::for_viewing(stacked, &first.read.layout, &mosaic, false);
    let planes: Vec<&[f32]> = linear.planes.iter().map(|plane| plane.as_slice()).collect();
    let image =
        tiff::Image { width, height, planes: &planes[..mosaic.colours.min(3)] };
    let target = args.out.join("stack.tif");
    tiff::write(&target, &image, &tiff::Mapping::Linear { full: linear.full }, &linear.note)
        .with_context(|| format!("writing {}", target.display()))?;
    written.push(target);

    if args.stretch {
        let levelled = view::for_viewing(stacked, &first.read.layout, &mosaic, true);
        let planes: Vec<&[f32]> = levelled.planes.iter().map(|plane| plane.as_slice()).collect();
        // A little below the sky rather than exactly on it: on it, half the
        // noise clips to zero, which reads as a clean background but has thrown
        // away the faintest half of everything.
        let floor = view::percentile(&levelled.planes[mosaic.dominant], 0.02).min(levelled.common);
        let target = args.out.join("stack_view.tif");
        tiff::write(
            &target,
            &tiff::Image { width, height, planes: &planes[..mosaic.colours.min(3)] },
            &tiff::Mapping::Asinh { black: floor, white: levelled.full, softening: 200.0 },
            &levelled.note,
        )
        .with_context(|| format!("writing {}", target.display()))?;
        written.push(target);
    }
    Ok(written)
}
