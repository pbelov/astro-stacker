//! The `stars` subcommand: find the stars in a session's lights and report what
//! the mount did to them.
//!
//! This is the answer to the first thing the owner asked for — "анализ и
//! исключение смаза" — and it is deliberately a *measurement* command rather
//! than a filtering one. It ranks the frames by how trailed they are and says
//! where the cut would fall; it does not yet drop anything, because the number
//! that decides is the owner's and not mine.
//!
//! Detection runs on the calibrated frame by default. That is not cosmetic: an
//! unsubtracted hot photosite is a five-sigma peak in every frame of the night,
//! and a hundred of them would enter the shape statistics as perfectly round
//! stars and dilute the trail they are supposed to reveal.

use anyhow::Result;
use astro_core::Formats;
use clap::{ArgMatches, Args};

use crate::format;
use crate::scan_command::ScanArgs;
use crate::survey::{Survey, SurveyArgs, Surveyed};

#[derive(Args, Debug)]
pub struct StarsArgs {
    #[command(flatten)]
    pub scan: ScanArgs,

    #[command(flatten)]
    pub survey: SurveyArgs,

    /// Print one line per frame as it is measured, not only the ranking.
    #[arg(long)]
    pub each: bool,

    /// How many of the worst frames to list at the end.
    #[arg(long, value_name = "N", default_value_t = 10)]
    pub worst: usize,
}

pub fn run(host: &Formats, args: &StarsArgs, matches: &ArgMatches) -> Result<()> {
    let survey = crate::survey::read(host, &args.scan, matches, &args.survey)?;
    if args.each {
        for frame in &survey.frames {
            print_one(frame);
        }
    }
    println!(
        "measured {} in {:.1}s",
        format::plural(survey.frames.len(), "frame"),
        survey.seconds
    );

    summarise(&survey, args.worst, args.survey.max_stars);
    Ok(())
}

fn print_one(one: &Surveyed) {
    let shape = &one.detection.shape;
    println!(
        "  {:<24} {:>5} stars  fwhm {:>5}  trail {:>5}  at {:>5}  agree {:>4}  {:.2}s",
        one.name,
        one.detection.stars.len(),
        number(shape.moments.minor_fwhm(), 2),
        number(shape.moments.trail(), 2),
        angle(shape.moments.angle_degrees()),
        number(shape.direction_agreement, 2),
        one.seconds
    );
}

/// A measured number, or a word saying it was not measured. Never a zero
/// standing in for an answer that was never reached.
fn number(value: f64, decimals: usize) -> String {
    if value.is_finite() { format!("{value:.decimals$}") } else { "--".to_owned() }
}

fn angle(degrees: f64) -> String {
    if degrees.is_finite() { format!("{degrees:.0}°") } else { "--".to_owned() }
}

fn summarise(survey: &Survey, worst: usize, cap: usize) {
    let measured: &[Surveyed] = &survey.frames;
    let mut trails: Vec<f64> =
        survey.frames.iter().map(|m| m.detection.shape.moments.trail()).filter(|v| v.is_finite()).collect();
    let mut minors: Vec<f64> = measured
        .iter()
        .map(|m| m.detection.shape.moments.minor_fwhm())
        .filter(|v| v.is_finite())
        .collect();
    let counts: Vec<usize> = survey.frames.iter().map(|m| m.detection.stars.len()).collect();

    // A frame whose shape could not be measured is not a badly trailed frame,
    // and ranking it among them would put the worst thing that can happen to a
    // light at the top of a list about mounts. It gets its own heading.
    let (shaped, unmeasured): (Vec<&Surveyed>, Vec<&Surveyed>) =
        survey.frames.iter().partition(|m| m.detection.shape.moments.axes().is_some());

    println!();
    if !survey.failed.is_empty() {
        // A frame that would not decode never reached the shape statistics at
        // all, so it cannot appear as a bad frame - only as a missing one.
        println!(
            "{} could not be read at all:",
            format::plural(survey.failed.len(), "frame")
        );
        for (name, why) in &survey.failed {
            println!("  {name:<24} {why}");
        }
        println!();
    }
    if !unmeasured.is_empty() {
        println!(
            "{} yielded no measurable shape at all, which is a ruined frame rather than a trailed one",
            format::plural(unmeasured.len(), "frame")
        );
        for one in &unmeasured {
            println!(
                "  {:<24} {} stars, {} with a clipped core, {} past the footprint cap",
                one.name,
                one.detection.stars.len(),
                one.detection.saturated,
                one.detection.oversized
            );
        }
        println!();
    }
    if trails.is_empty() {
        println!("no frame yielded a measurable shape, so there is nothing to rank");
        return;
    }
    trails.sort_by(f64::total_cmp);
    minors.sort_by(f64::total_cmp);
    let at = |values: &[f64], fraction: f64| values[((values.len() - 1) as f64 * fraction) as usize];

    println!("the run as a whole");
    // A count that is really the cap is not a count, and printed bare it reads
    // as one. Say which it is.
    let at_cap = counts.iter().filter(|count| **count >= cap).count();
    println!(
        "  stars      {} per frame, {} at worst and {} at best{}",
        counts.iter().sum::<usize>() / counts.len().max(1),
        counts.iter().min().copied().unwrap_or(0),
        counts.iter().max().copied().unwrap_or(0),
        if at_cap > 0 {
            format!(
                " - but {} of {} hit the --max-stars {cap} ceiling, so these are floors",
                at_cap,
                counts.len()
            )
        } else {
            String::new()
        }
    );
    let footprints: Vec<usize> =
        survey.frames.iter().flat_map(|m| m.detection.stars.iter().map(|s| s.footprint)).collect();
    if !footprints.is_empty() {
        let mut sorted = footprints.clone();
        sorted.sort_unstable();
        println!(
            "  footprint  {} photosites per star (median), {} at the largest kept",
            sorted[sorted.len() / 2],
            sorted[sorted.len() - 1]
        );
    }
    println!(
        "  fwhm       {:.2} px across the trail (median), {:.2} to {:.2} over the run",
        at(&minors, 0.5),
        minors.first().copied().unwrap_or(f64::NAN),
        minors.last().copied().unwrap_or(f64::NAN)
    );
    println!(
        "  trail      {:.2} px (median), {:.2} at the best frame and {:.2} at the worst",
        at(&trails, 0.5),
        trails.first().copied().unwrap_or(f64::NAN),
        trails.last().copied().unwrap_or(f64::NAN)
    );

    // A trail that points the same way in every frame is the mount tracking at
    // the wrong rate; one that points differently frame to frame is wind, or
    // touching the tripod. They call for opposite fixes, so the report has to
    // separate them rather than averaging them into "the stars are elongated".
    let (mut sum, mut weight) = ((0.0f64, 0.0f64), 0.0f64);
    for one in measured {
        let shape = &one.detection.shape;
        if let Some((cos2t, sin2t)) = shape.moments.orientation() {
            let trail = shape.moments.trail();
            if trail.is_finite() && trail > 0.0 {
                sum.0 += cos2t * trail;
                sum.1 += sin2t * trail;
                weight += trail;
            }
        }
    }
    if weight > 0.0 {
        let across_run = (sum.0 * sum.0 + sum.1 * sum.1).sqrt() / weight;
        let degrees = {
            let angle = sum.1.atan2(sum.0).to_degrees() / 2.0;
            if angle < 0.0 { angle + 180.0 } else { angle }
        };
        println!(
            "  direction  {degrees:.0}° across the whole run, agreement {across_run:.2} — {}",
            if across_run > 0.8 {
                "one direction in every frame, which is a tracking rate, not the wind"
            } else if across_run > 0.4 {
                "mostly one direction, with frames that wandered"
            } else {
                "no shared direction, so the elongation is not the mount's rate"
            }
        );
    }

    if worst == 0 {
        return;
    }
    let mut ranked = shaped.clone();
    ranked.sort_by(|a, b| {
        b.detection.shape.moments.trail().total_cmp(&a.detection.shape.moments.trail())
    });
    println!("\nthe {} most trailed", ranked.len().min(worst));
    for one in ranked.iter().take(worst) {
        let shape = &one.detection.shape;
        println!(
            "  {:<24} trail {:>5} px  fwhm {:>5}  at {:>5}  agree {:>4}  {} stars",
            one.name,
            number(shape.moments.trail(), 2),
            number(shape.moments.minor_fwhm(), 2),
            angle(shape.moments.angle_degrees()),
            number(shape.direction_agreement, 2),
            one.detection.stars.len()
        );
    }

    // What a cut would cost, without making it: the threshold belongs to whoever
    // owns the night, and the tool's job is to say what each choice throws away.
    println!("\nwhat a trail limit would drop");
    for fraction in [0.9f64, 0.75, 0.5] {
        let limit = at(&trails, fraction);
        let kept = trails.iter().filter(|v| **v <= limit).count();
        println!(
            "  under {limit:>5.2} px  keeps {kept} of {} ({:.0}%)",
            shaped.len(),
            kept as f64 / shaped.len() as f64 * 100.0
        );
    }

    let saturated: usize = survey.frames.iter().map(|m| m.detection.saturated).sum();
    let oversized: usize = survey.frames.iter().map(|m| m.detection.oversized).sum();
    if saturated + oversized > 0 {
        println!(
            "\ndropped along the way: {} with a clipped core, {} past the footprint cap \
             (satellites, aeroplanes, unresolved pairs)",
            saturated, oversized
        );
    }
}
