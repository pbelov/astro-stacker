//! The `register` subcommand: put every light of a run onto one set of
//! coordinates and report how far each had to move.
//!
//! Like `stars`, this measures and does not filter. What it produces is the
//! transform per frame and the drift over the night, which is the thing that
//! says whether the mount was tracking, whether the field rotated, and which
//! frames are somewhere else entirely.

use std::collections::VecDeque;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use astro_core::PluginHost;
use astro_core::register::{Fitted, MatchOptions, Registration, Transform, refine, register};
use astro_core::stars::Star;
use clap::{ArgMatches, Args};

use crate::format;
use crate::scan_command::ScanArgs;
use crate::survey::{SurveyArgs, Surveyed};

#[derive(Args, Debug)]
pub struct RegisterArgs {
    #[command(flatten)]
    pub scan: ScanArgs,

    #[command(flatten)]
    pub survey: SurveyArgs,

    /// Register against this frame rather than one chosen from the middle of
    /// the run. Give the file stem, as printed.
    #[arg(long, value_name = "NAME")]
    pub reference: Option<String>,

    /// How many of the brightest stars to match with. Generous on purpose: the
    /// top of a brightness ranking is the least stable part of it, because
    /// which stars saturate moves with the trailing.
    #[arg(long, value_name = "N", default_value_t = 1000)]
    pub stars: usize,

    /// How far along the run to look for a frame to chain through. Larger
    /// steps over more ruined frames and costs more matching.
    #[arg(long, value_name = "N", default_value_t = 3)]
    pub span: usize,

    /// Print one line per frame, not only the summary.
    #[arg(long)]
    pub each: bool,
}

/// One surveyed light, plus where registering it landed.
struct Frame<'a> {
    read: &'a Surveyed,
    registration: Option<Registration>,
    /// How the frame was seeded, for the report: a run where most frames needed
    /// the chain is a run that drifted, and that is worth knowing.
    seeded: Seed,
}

impl Frame<'_> {
    fn name(&self) -> &str {
        &self.read.name
    }

    fn stars(&self) -> &[Star] {
        &self.read.detection.stars
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seed {
    /// Matched against the reference directly, by voting for the most popular
    /// offset. What a frame near the reference can afford.
    Vote,
    /// Seeded by carrying the answer along the run from neighbour to
    /// neighbour, then fitted against the reference directly.
    Chain,
    None,
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

    let mut frames: Vec<Frame> = survey
        .frames
        .iter()
        .map(|read| Frame { read, registration: None, seeded: Seed::None })
        .collect();

    let reference = choose_reference(&frames, args.reference.as_deref())?;
    println!(
        "  reference  {} ({} stars, frame {} of {})",
        frames[reference].name(),
        frames[reference].stars().len(),
        reference + 1,
        frames.len()
    );

    let match_options = MatchOptions { brightest: args.stars, ..Default::default() };
    let started = Instant::now();
    let (seeds, links) = chain_seeds(&frames, reference, args.span, &match_options);
    let unreached = seeds.iter().filter(|seed| seed.is_none()).count();

    // Cloned so the reference can be read while the rest are written.
    let anchor: Vec<Star> = frames[reference].stars().to_vec();
    for index in 0..frames.len() {
        if index == reference {
            frames[index].registration = Some(Registration {
                transform: Transform::IDENTITY,
                fitted: Fitted::Similarity,
                matched: frames[index].stars().len(),
                residual: 0.0,
            });
            frames[index].seeded = Seed::Vote;
            continue;
        }
        // The chained guess first, since it is the one that works at a
        // distance, and the vote as the fallback for a frame the chain could
        // not reach.
        let chained = seeds[index]
            .and_then(|seed| refine(&anchor, frames[index].stars(), seed, &match_options));
        if let Some(found) = chained {
            frames[index].registration = Some(found);
            frames[index].seeded = Seed::Chain;
            continue;
        }
        frames[index].registration = register(&anchor, frames[index].stars(), &match_options);
        frames[index].seeded =
            if frames[index].registration.is_some() { Seed::Vote } else { Seed::None };
    }
    println!(
        "  registered in {:.1}s, the chain of {links} links reaching {} of {}",
        started.elapsed().as_secs_f64(),
        frames.len() - unreached,
        frames.len()
    );

    summarise(&frames, reference, args.each, &survey.failed);
    Ok(())
}

/// A transform for every frame, built by registering each against a nearby
/// frame and carrying the result back to the reference.
///
/// Consecutive subframes share nearly all their sky, so the offset vote finds
/// them easily; the ends of a drifting night do not, which is why this exists.
/// On this project's reference session the field walks two thousand photosites,
/// and a frame a hundred exposures from the reference has too little in common
/// with it for the most popular offset to mean anything.
///
/// What it produces is only a seed. Every frame is then fitted against the
/// reference directly, so the error of a long chain of links never reaches the
/// answer — only its guess.
///
/// It spreads outward from the reference rather than walking the run in order,
/// and it will step over a frame as well as to it. A single ruined frame — cloud,
/// a bump, a passing car — would otherwise cut the run in half and lose
/// everything past it, which is the failure that a chain of strict neighbours
/// actually shows on real data. Reaching each frame in the fewest hops also
/// keeps the guess as good as it can be.
fn chain_seeds(
    frames: &[Frame<'_>],
    reference: usize,
    span: usize,
    options: &MatchOptions,
) -> (Vec<Option<Transform>>, usize) {
    let mut seeds: Vec<Option<Transform>> = vec![None; frames.len()];
    seeds[reference] = Some(Transform::IDENTITY);

    let mut queue = VecDeque::from([reference]);
    let mut links = 0usize;
    while let Some(here) = queue.pop_front() {
        let onward = seeds[here].expect("a frame is queued only once it has a seed");
        let first = here.saturating_sub(span);
        for there in first..(here + span + 1).min(frames.len()) {
            if there == here || seeds[there].is_some() {
                continue;
            }
            // The transform that maps `there` onto `here`, then `here` onto the
            // reference. Composed this way round, no inverse is ever needed.
            let Some(link) = register(frames[here].stars(), frames[there].stars(), options) else {
                continue;
            };
            links += 1;
            seeds[there] = Some(link.transform.then(&onward));
            queue.push_back(there);
        }
    }
    (seeds, links)
}

/// The frame everything else is measured against.
///
/// From the middle of the run unless the user names one, because drift
/// accumulates and the middle is the frame with the least of it between itself
/// and the ends. Among the middle third, the one with the most stars: a
/// reference thin on stars limits every match made against it.
fn choose_reference(frames: &[Frame<'_>], named: Option<&str>) -> Result<usize> {
    if let Some(wanted) = named {
        return frames
            .iter()
            .position(|frame| frame.name() == wanted)
            .with_context(|| format!("no light called {wanted} was measured"));
    }
    let third = frames.len() / 3;
    let window = third..frames.len().saturating_sub(third).max(third + 1);
    window
        .clone()
        .max_by_key(|index| frames[*index].stars().len())
        .with_context(|| format!("the run has no middle to choose from: {window:?}"))
}

fn summarise(frames: &[Frame<'_>], reference: usize, each: bool, unread: &[(String, String)]) {
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

    let registered: Vec<(&Frame, &Registration)> = frames
        .iter()
        .filter_map(|frame| frame.registration.as_ref().map(|found| (frame, found)))
        .collect();
    let failed: Vec<&Frame> = frames.iter().filter(|frame| frame.registration.is_none()).collect();

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
