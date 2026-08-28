//! Putting a surveyed run onto one set of coordinates.
//!
//! Extracted from the `register` command because `stack` needs exactly the same
//! answer and must not compute it a second, slightly different way. The
//! reference frame, the chain of neighbours and the direct refit are one
//! decision, and two copies of it would eventually disagree about which frame
//! the stack was built on.

use std::collections::VecDeque;
use std::time::Instant;

use anyhow::{Context, Result};
use astro_core::register::{Fitted, MatchOptions, Registration, Transform, refine, register};
use astro_core::stars::Star;
use clap::Args;

use crate::survey::Surveyed;

#[derive(Args, Debug, Clone)]
pub struct AlignArgs {
    /// Align against this frame rather than one chosen from the middle of the
    /// run. Give the file stem, as printed.
    #[arg(long, value_name = "NAME")]
    pub reference: Option<String>,

    /// How many of the brightest stars to match with. Generous on purpose: the
    /// top of a brightness ranking is the least stable part of it, because
    /// which stars saturate moves with the trailing.
    #[arg(long, value_name = "N", default_value_t = 1000)]
    pub stars: usize,

    /// How far along the run to look for a frame to chain through. Larger steps
    /// over more ruined frames and costs more matching.
    #[arg(long, value_name = "N", default_value_t = 3)]
    pub span: usize,
}

/// How a frame reached the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seed {
    /// Matched against the reference directly, by voting for the most popular
    /// offset. What a frame near the reference can afford.
    Vote,
    /// Seeded by carrying the answer along the run from neighbour to
    /// neighbour, then fitted against the reference directly.
    Chain,
    None,
}

/// One surveyed light, and where aligning it landed.
pub struct Aligned<'a> {
    pub read: &'a Surveyed,
    pub registration: Option<Registration>,
    pub seeded: Seed,
}

impl Aligned<'_> {
    pub fn name(&self) -> &str {
        &self.read.name
    }

    pub fn stars(&self) -> &[Star] {
        &self.read.detection.stars
    }
}

pub struct Alignment<'a> {
    pub frames: Vec<Aligned<'a>>,
    pub reference: usize,
    pub links: usize,
    pub unreached: usize,
    pub seconds: f64,
}

pub fn align<'a>(read: &'a [Surveyed], args: &AlignArgs) -> Result<Alignment<'a>> {
    let mut frames: Vec<Aligned<'a>> = read
        .iter()
        .map(|read| Aligned { read, registration: None, seeded: Seed::None })
        .collect();
    let reference = choose_reference(&frames, args.reference.as_deref())?;

    let options = MatchOptions { brightest: args.stars, ..Default::default() };
    let started = Instant::now();
    let (seeds, links) = chain_seeds(&frames, reference, args.span, &options);
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
        // The chained guess first, since it is the one that works at a distance,
        // and the vote as the fallback for a frame the chain could not reach.
        let chained =
            seeds[index].and_then(|seed| refine(&anchor, frames[index].stars(), seed, &options));
        if let Some(found) = chained {
            frames[index].registration = Some(found);
            frames[index].seeded = Seed::Chain;
            continue;
        }
        frames[index].registration = register(&anchor, frames[index].stars(), &options);
        frames[index].seeded =
            if frames[index].registration.is_some() { Seed::Vote } else { Seed::None };
    }

    Ok(Alignment {
        frames,
        reference,
        links,
        unreached,
        seconds: started.elapsed().as_secs_f64(),
    })
}

/// The frame everything else is measured against.
///
/// From the middle of the run unless the user names one, because drift
/// accumulates and the middle is the frame with the least of it between itself
/// and the ends. Among the middle third, the one with the most stars: a
/// reference thin on stars limits every match made against it.
fn choose_reference(frames: &[Aligned<'_>], named: Option<&str>) -> Result<usize> {
    if let Some(wanted) = named {
        return frames
            .iter()
            .position(|frame| frame.name() == wanted)
            .with_context(|| format!("no light called {wanted} was read"));
    }
    let third = frames.len() / 3;
    let window = third..frames.len().saturating_sub(third).max(third + 1);
    window
        .clone()
        .max_by_key(|index| frames[*index].stars().len())
        .with_context(|| format!("the run has no middle to choose from: {window:?}"))
}

/// A transform for every frame, built by registering each against a nearby
/// frame and carrying the result back to the reference.
///
/// Consecutive subframes share nearly all their sky, so the offset vote finds
/// them easily; the ends of a drifting night do not, which is why this exists.
/// On this project's reference session the field walks nearly four thousand
/// photosites, and a frame a hundred exposures from the reference has too
/// little in common with it for the most popular offset to mean anything.
///
/// What it produces is only a seed. Every frame is then fitted against the
/// reference directly, so the error of a long chain of links never reaches the
/// answer — only its guess.
///
/// It spreads outward from the reference rather than walking the run in order,
/// and it will step over a frame as well as to it. A single ruined frame —
/// cloud, a bump, a passing car — would otherwise cut the run in half and lose
/// everything past it, which is the failure a chain of strict neighbours
/// actually shows on real data. Reaching each frame in the fewest hops also
/// keeps the guess as good as it can be.
fn chain_seeds(
    frames: &[Aligned<'_>],
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
