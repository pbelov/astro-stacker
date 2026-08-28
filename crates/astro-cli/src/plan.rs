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

    // The ranking itself lives in the core, so that the window and the command
    // line cannot come to different conclusions about which set is the session.
    let ranked = partition.plans_by_depth(session);

    let Some(&(plan, depth)) = ranked.first() else {
        bail!("every plan names a light set that is not in the partition");
    };
    let Some(lights) = partition.set(plan.lights) else {
        bail!("the deepest plan names a light set that is not in the partition");
    };

    // A tie is not a preference to encode. Two equally deep sets of lights are
    // two nights, or two targets, and only the user knows which. A window can
    // show both and let the user point; a command has to ask.
    if let Some((runner_up, tied)) = ranked.get(1)
        && *tied == depth
    {
        bail!(
            "sets {} and {} both hold {} — say which with --set",
            plan.lights.index(),
            runner_up.lights.index(),
            format::plural(depth, "light")
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
