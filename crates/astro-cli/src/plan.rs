//! Choosing which integration a command acts on.
//!
//! A session normally holds one, but a night that begins with framing and focus
//! tests holds several: this project's reference session partitions into four
//! light sets of 226, 2, 1 and 1 frames, and each gets its own plan with its own
//! matched calibration.
//!
//! Taking the first was a real bug and not a theoretical one. Set order is
//! deterministic but arbitrary, and on that session the first plan was the
//! two-frame one-second set — whose matched dark is the single one-second dark.
//! `master` therefore built a master dark out of one frame and said so quietly,
//! while its own `scan` output had already named the 226-frame set and flagged
//! the small ones as too few to stack.

use anyhow::{Result, bail};
use astro_core::session::{FrameSet, Partition, Session, StackPlan};

use crate::format;

/// The plan a command should act on, and why it was chosen.
pub struct Chosen<'a> {
    pub plan: &'a StackPlan,
    pub lights: &'a FrameSet,
    /// `true` when the session held more than one integration and this one was
    /// picked rather than being the only candidate.
    pub picked: bool,
    pub of: usize,
}

/// Picks the integration with the most light frames, or the one the user named.
///
/// Depth rather than exposure or capture order: the set the user spent the night
/// on is the one with the frames in it, and a tie between two equally deep sets
/// is a real ambiguity the user has to resolve rather than a preference to
/// encode.
pub fn choose<'a>(
    session: &Session,
    partition: &'a Partition,
    requested: Option<usize>,
) -> Result<Chosen<'a>> {
    if partition.plans.is_empty() {
        bail!("no stackable set of lights was formed, so there is nothing to act on");
    }

    let active = |set: &FrameSet| set.members.iter().filter(|id| session[**id].is_active()).count();

    if let Some(wanted) = requested {
        let found = partition
            .plans
            .iter()
            .find(|plan| plan.lights.index() == wanted)
            .and_then(|plan| partition.set(plan.lights).map(|lights| (plan, lights)));
        let Some((plan, lights)) = found else {
            let available: Vec<String> = partition
                .plans
                .iter()
                .filter_map(|plan| partition.set(plan.lights))
                .map(|set| format!("{} ({})", set.id.index(), format::plural(active(set), "frame")))
                .collect();
            bail!("set {wanted} is not a light set with a plan. Try one of: {}", available.join(", "));
        };
        return Ok(Chosen { plan, lights, picked: partition.plans.len() > 1, of: partition.plans.len() });
    }

    let mut ranked: Vec<(&StackPlan, &FrameSet)> = partition
        .plans
        .iter()
        .filter_map(|plan| partition.set(plan.lights).map(|lights| (plan, lights)))
        .collect();
    ranked.sort_by_key(|(_, lights)| std::cmp::Reverse(active(lights)));

    let Some(&(plan, lights)) = ranked.first() else {
        bail!("every plan names a light set that is not in the partition");
    };

    // A tie is not a preference to encode. Two equally deep sets of lights are
    // two nights, or two targets, and only the user knows which.
    if let Some((_, runner_up)) = ranked.get(1)
        && active(runner_up) == active(lights)
    {
        bail!(
            "sets {} and {} both hold {} — say which with --set",
            lights.id.index(),
            runner_up.id.index(),
            format::plural(active(lights), "light")
        );
    }

    Ok(Chosen { plan, lights, picked: ranked.len() > 1, of: ranked.len() })
}

impl Chosen<'_> {
    /// Says which integration was acted on when there was more than one, so a
    /// command that quietly did the wrong set cannot look like one that had no
    /// choice.
    pub fn announce(&self, session: &Session) {
        if !self.picked {
            return;
        }
        let active =
            self.lights.members.iter().filter(|id| session[**id].is_active()).count();
        println!(
            "acting on set {} of {} plans, the deepest at {}",
            self.lights.id.index(),
            self.of,
            format::plural(active, "light")
        );
    }
}
