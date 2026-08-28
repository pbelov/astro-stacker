//! The `register` subcommand: report how far each light of a run had to move.
//!
//! Like `stars`, this measures and does not filter. What it produces is the
//! transform per frame and the drift over the night, which is what says whether
//! the mount was tracking, whether the field rotated, and which frames are
//! somewhere else entirely.
//!
//! The alignment itself lives in `align`, because `stack` needs the same answer
//! and two copies of it would eventually disagree about which frame the stack
//! was built on.

use anyhow::{Result, bail};
use astro_core::PluginHost;
use astro_core::register::{Fitted, Registration};
use astro_core::stars::Star;
use clap::{ArgMatches, Args};

use crate::align::{AlignArgs, Aligned, Seed, align};
use crate::format;
use crate::scan_command::ScanArgs;
use crate::survey::SurveyArgs;

#[derive(Args, Debug)]
pub struct RegisterArgs {
    #[command(flatten)]
    pub scan: ScanArgs,

    #[command(flatten)]
    pub survey: SurveyArgs,

    #[command(flatten)]
    pub align: AlignArgs,

    /// Print one line per frame, not only the summary.
    #[arg(long)]
    pub each: bool,
}

pub fn run(host: &PluginHost, args: &RegisterArgs, matches: &ArgMatches) -> Result<()> {
    let survey = crate::survey::read(host, &args.scan, matches, &args.survey)?;
    if survey.frames.len() < 2 {
        bail!(
            "registration needs at least two lights, and this run offered {}",
            survey.frames.len()
        );
    }
    println!(
        "  found stars in {} in {:.1}s",
        format::plural(survey.frames.len(), "frame"),
        survey.seconds
    );

    let alignment = align(&survey.frames, &args.align)?;
    println!(
        "  reference  {} ({} stars, frame {} of {})",
        alignment.frames[alignment.reference].name(),
        alignment.frames[alignment.reference].stars().len(),
        alignment.reference + 1,
        alignment.frames.len()
    );
    println!(
        "  registered in {:.1}s, the chain of {} links reaching {} of {}",
        alignment.seconds,
        alignment.links,
        alignment.frames.len() - alignment.unreached,
        alignment.frames.len()
    );

    summarise(&alignment.frames, alignment.reference, args.each, &survey.failed);
    Ok(())
}

fn summarise(frames: &[Aligned<'_>], reference: usize, each: bool, unread: &[(String, String)]) {
    // The frame centre, from the reference's own stars: registration reports how
    // far a point moved, and the point that means anything is the middle of the
    // frame rather than the corner the coordinates start at.
    let (cx, cy) = centre_of(frames[reference].stars());

    if each {
        println!();
        for frame in frames {
            match &frame.registration {
                Some(found) => {
                    let (dx, dy) = found.transform.displacement_at(cx, cy);
                    // A rotation that was never fitted is not a rotation of
                    // zero, and printing it as one would report a run as
                    // steadier than anything measured it to be.
                    let (turn, scale) = match found.fitted {
                        Fitted::Similarity => (
                            format!("{:>7.3}'", found.transform.rotation_degrees() * 60.0),
                            format!("{:>+8.1}", (found.transform.scale() - 1.0) * 1e6),
                        ),
                        Fitted::Shift => ("      --".to_owned(), "      --".to_owned()),
                    };
                    println!(
                        "  {:<24} dx {:>8.2}  dy {:>8.2}  turn {turn}  scale {scale} ppm  \
                         {:>4} pairs at {:.2} px",
                        frame.name(), dx, dy, found.matched, found.residual
                    );
                }
                None => println!("  {:<24} did not register", frame.name()),
            }
        }
    }

    let registered: Vec<(&Aligned<'_>, &Registration)> = frames
        .iter()
        .filter_map(|frame| frame.registration.as_ref().map(|found| (frame, found)))
        .collect();
    let failed: Vec<&Aligned<'_>> = frames.iter().filter(|frame| frame.registration.is_none()).collect();

    println!();
    if !unread.is_empty() {
        // Distinct from a frame that registered badly: this one never produced
        // stars to register with.
        println!("{} could not be read at all:", format::plural(unread.len(), "frame"));
        for (name, why) in unread {
            println!("  {name:<24} {why}");
        }
        println!();
    }
    if !failed.is_empty() {
        // A frame that would not register is not a frame to stack, and it is
        // the one the user most needs named.
        println!(
            "{} did not register against the reference, which means cloud, a snag, or a \
             different target - not a small error:",
            format::plural(failed.len(), "frame")
        );
        for frame in &failed {
            println!("  {:<24} {} stars found", frame.name(), frame.stars().len());
        }
        println!();
    }
    if registered.is_empty() {
        println!("nothing registered, so there is no drift to report");
        return;
    }

    let mut shifts: Vec<f64> = Vec::new();
    let (mut min_x, mut max_x, mut min_y, mut max_y) =
        (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    let mut turns: Vec<f64> = Vec::new();
    let mut scales: Vec<f64> = Vec::new();
    let mut residuals: Vec<f64> = Vec::new();
    let mut pairs: Vec<usize> = Vec::new();
    for (_, found) in &registered {
        let (dx, dy) = found.transform.displacement_at(cx, cy);
        shifts.push((dx * dx + dy * dy).sqrt());
        min_x = min_x.min(dx);
        max_x = max_x.max(dx);
        min_y = min_y.min(dy);
        max_y = max_y.max(dy);
        // Only a frame that actually bought the four parameters may speak
        // about rotation and scale.
        if found.fitted == Fitted::Similarity {
            turns.push(found.transform.rotation_degrees() * 60.0);
            scales.push((found.transform.scale() - 1.0) * 1e6);
        }
        residuals.push(found.residual);
        pairs.push(found.matched);
    }
    let median = |values: &mut Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values[values.len() / 2]
    };

    println!("the run as a whole");
    println!(
        "  registered {} of {}, {} pairs at the median",
        registered.len(),
        frames.len(),
        {
            pairs.sort_unstable();
            pairs[pairs.len() / 2]
        }
    );
    println!(
        "  drift      {:.0} px across the run: x from {min_x:.0} to {max_x:.0}, y from {min_y:.0} to {max_y:.0}",
        ((max_x - min_x).powi(2) + (max_y - min_y).powi(2)).sqrt()
    );
    println!("  shift      {:.1} px from the reference at the median", median(&mut shifts));
    if turns.is_empty() {
        println!("  rotation   not measured: no frame matched enough stars to afford an angle");
    } else {
        let (turn_low, turn_high) = extremes(&turns);
        println!(
            "  rotation   {:.3}' at the median, {turn_low:.3}' to {turn_high:.3}' over the {} that bought one",
            median(&mut turns),
            turns.len()
        );
        let (scale_low, scale_high) = extremes(&scales);
        println!(
            "  scale      {:+.0} ppm at the median, {scale_low:+.0} to {scale_high:+.0} over the same",
            median(&mut scales)
        );
    }
    // The residual is the number that says whether four parameters were enough.
    // A residual near the centroid error means the model fits; one several times
    // larger means the field is doing something a similarity cannot describe.
    let chained = frames.iter().filter(|frame| frame.seeded == Seed::Chain).count();
    if chained > 0 {
        // Not a claim that these could not have been matched directly. The
        // chained guess is simply tried first, because it is the one that works
        // at a distance, so the direct vote was never run on them.
        println!(
            "  {chained} were seeded by the chain of neighbours rather than matched to the \
             reference directly"
        );
    }
    let shift_only = registered.iter().filter(|(_, found)| found.fitted == Fitted::Shift).count();
    if shift_only > 0 {
        println!(
            "  {shift_only} of them matched too few stars for an angle and were fitted as a shift alone"
        );
    }
    println!(
        "  residual   {:.2} px at the median, {:.2} at the worst frame",
        median(&mut residuals),
        residuals.last().copied().unwrap_or(f64::NAN)
    );

    // What the field costs, put as a choice rather than a verdict. A run that
    // was framed and refocused before it settled has a few frames a long way
    // from the reference, and keeping them costs most of the picture. Which
    // trade to make is the owner's.
    let width = cx * 2.0;
    println!("\nwhat keeping the far frames costs");
    for fraction in [1.0f64, 0.9, 0.75, 0.5] {
        let limit = shifts[((shifts.len() - 1) as f64 * fraction) as usize];
        let kept = shifts.iter().filter(|shift| **shift <= limit).count();
        println!(
            "  within {limit:>7.0} px  keeps {kept} of {} ({:.0}%), leaving {:.0}% of the frame",
            shifts.len(),
            kept as f64 / shifts.len() as f64 * 100.0,
            (1.0 - limit / width).max(0.0) * 100.0
        );
    }
}

/// The middle of the star field, which stands in for the middle of the frame
/// without needing the layout here.
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

fn extremes(values: &[f64]) -> (f64, f64) {
    let low = values.iter().copied().fold(f64::INFINITY, f64::min);
    let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (low, high)
}
