//! Partitioning frames into sets that can be stacked, and matching calibration
//! sets to light sets.

use std::collections::HashMap;

use super::compat::{self, GeometryKey, Incompatibility, Mismatch, Pairing, Property, Tolerances};
use super::{BodyKey, FrameId, FrameKind, FrameRecord, Session, may_draw_from};

/// Handle to one set within one [`Partition`].
///
/// A **within-partition handle only**. `partition` is pure and reallocates ids
/// from zero every time it runs, so excluding a single frame can renumber every
/// set. Anything that outlives one partition — a cached master frame, a saved
/// selection — must be keyed by [`SetKey`], which is membership-independent
/// where it matters and is `Hash + Eq`. [`Partition::set_by_key`] is how a
/// caller rebinds after a re-partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SetId(u32);

impl SetId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// ISO as recorded, or the fact that it was not.
///
/// `Unknown` is its own bucket and never merges with a known one. A frame whose
/// gain was not recorded is not evidence that it matches anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GainBucket {
    Iso(u32),
    Unknown,
}

impl GainBucket {
    fn from_iso(iso: Option<f64>) -> Self {
        match iso {
            Some(iso) if iso.is_finite() && iso > 0.0 => Self::Iso(iso.round() as u32),
            _ => Self::Unknown,
        }
    }
}

/// Exposure quantised to microseconds — finer than any camera reports — plus the
/// unknown case, which likewise never merges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ExposureBucket {
    Micros(u64),
    Unknown,
}

impl ExposureBucket {
    fn from_seconds(seconds: Option<f64>) -> Self {
        match seconds {
            Some(seconds) if seconds.is_finite() && seconds > 0.0 => {
                Self::Micros((seconds * 1e6).round() as u64)
            }
            _ => Self::Unknown,
        }
    }

    pub fn seconds(self) -> Option<f64> {
        match self {
            Self::Micros(micros) => Some(micros as f64 / 1e6),
            Self::Unknown => None,
        }
    }
}

/// Shortest and longest exposure actually present in a set. Equal in the usual
/// case of a run shot at one shutter speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExposureSpan {
    pub shortest: ExposureBucket,
    pub longest: ExposureBucket,
}

impl ExposureSpan {
    /// The exposure a set is treated as having when it is matched against
    /// another. The longest, because dark current accumulates with time and
    /// under-removing it is the failure that shows.
    pub fn nominal(self) -> Option<f64> {
        self.longest.seconds()
    }
}

/// Everything that must match exactly before two frames are even considered for
/// the same set.
///
/// Hashable because every field is exact. Exposure is deliberately absent: it is
/// compared with a tolerance, and a tolerance is not an equivalence relation —
/// 100, 104 and 108 seconds are each within five per cent of a neighbour while
/// the ends are not within five per cent of each other — so it cannot key a hash
/// map. See [`partition`] for the second pass that handles it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PartitionKey {
    pub kind: FrameKind,
    /// The group's *name*, not its [`GroupId`][super::GroupId]. Ids are handed
    /// out in the order groups were first seen, so a key carrying one would
    /// rebind to a different night the moment the same session is described
    /// with the flags in another order. Every other field here is derived from
    /// the file; this one has to be too.
    pub group: Box<str>,
    pub body: BodyKey,
    pub geometry: GeometryKey,
    /// Gain is here even though mixed-ISO lights integrate together perfectly
    /// well, because a set is the unit calibration is matched to: one set
    /// spanning two gains could only be given one master dark, and it would be
    /// wrong for half of it. Merging the results back into one image is a
    /// stacking concern, and DeepSkyStacker draws the line in the same place.
    pub gain: GainBucket,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SetKey {
    pub partition: PartitionKey,
    pub exposure: ExposureSpan,
}

/// One run of frames that are the same frame, shot repeatedly.
///
/// A pure partition of [`FrameId`]s and nothing else. Master frames,
/// integration results and per-set caches attach later as tables keyed by
/// [`SetKey`], which is why this type carries no `master` field and will not
/// need one.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameSet {
    pub id: SetId,
    pub key: SetKey,
    /// In capture order where timestamps exist, otherwise in scan order, so a
    /// report and a later reference-frame choice are reproducible.
    pub members: Vec<FrameId>,
}

impl FrameSet {
    pub fn kind(&self) -> FrameKind {
        self.key.partition.kind
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// The member every other frame and every other set is judged against.
    ///
    /// The median by exposure, not the first scanned: one stray file must not
    /// become the reference that everything else is measured from.
    pub fn representative(&self, session: &Session) -> FrameId {
        let mut ordered = self.members.clone();
        ordered.sort_by(|a, b| {
            let key = |id: &FrameId| {
                (
                    ExposureBucket::from_seconds(session[*id].info.exposure_seconds),
                    session[*id].info.capture_time_unix,
                    *id,
                )
            };
            key(a).cmp(&key(b))
        });
        ordered[ordered.len() / 2]
    }
}

/// Why a particular calibration set was chosen, so that a report can say more
/// than "here is a master".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchQuality {
    /// Nothing differed at all.
    Exact,
    /// Nothing else was admissible.
    OnlyCandidate,
    /// Chosen because its exposure was nearest — how darks are matched.
    ClosestExposure,
    /// Chosen because its gain was nearest — how flats and biases are matched.
    ClosestGain,
}

/// A chosen calibration set and everything that does not fit about it.
///
/// A best match with a list of reasons, not a pass or a fail: every established
/// tool in this space degrades gracefully rather than refusing, and an empty
/// `mismatches` is what a perfect match looks like.
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationMatch {
    pub set: SetId,
    pub pairing: Pairing,
    pub quality: MatchQuality,
    /// How many other sets were admissible and lost. More than zero on a flat
    /// usually means flats from more than one night, where only the user can say
    /// which belongs to which.
    pub alternatives: usize,
    pub mismatches: Vec<Mismatch>,
}

/// A role the user supplied frames for, where every candidate was refused.
///
/// This is the difference between "you shot no darks" and "you shot darks and I
/// ignored them", and the second must never be silent: the user believes the
/// stack was calibrated.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockedRole {
    pub kind: FrameKind,
    /// The set the candidates were judged against. Not always the lights: a
    /// dark flat is measured against the flats, and a report that did not say so
    /// would show the flats' exposure as what the lights "expected".
    pub against: SetId,
    pub candidates: Vec<(SetId, Incompatibility)>,
}

/// One integration: a light set, and the calibration sets matched to it.
///
/// A graph over set ids rather than over frames. Registration attaches to the
/// light set, master generation to each calibration set — neither touches this
/// type.
#[derive(Debug, Clone, PartialEq)]
pub struct StackPlan {
    pub lights: SetId,
    pub bias: Option<CalibrationMatch>,
    pub dark: Option<CalibrationMatch>,
    pub flat: Option<CalibrationMatch>,
    /// Matched against `flat`, not against `lights`.
    pub dark_flat: Option<CalibrationMatch>,
    pub blocked: Vec<BlockedRole>,
}

impl StackPlan {
    /// The calibration this plan actually has, in report order.
    pub fn calibration(&self) -> impl Iterator<Item = (FrameKind, &CalibrationMatch)> {
        [
            (FrameKind::Bias, self.bias.as_ref()),
            (FrameKind::Dark, self.dark.as_ref()),
            (FrameKind::Flat, self.flat.as_ref()),
            (FrameKind::DarkFlat, self.dark_flat.as_ref()),
        ]
        .into_iter()
        .filter_map(|(kind, found)| found.map(|found| (kind, found)))
    }
}

/// Frames of one kind that could not share a set, and why.
///
/// A crop-mode frame among full-frame lights becomes its own one-frame set
/// rather than joining them. That is correct — they genuinely cannot be indexed
/// against each other — but silently showing two light sets would leave the user
/// to work out why. This names the reason against the largest set of that kind.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    pub kind: FrameKind,
    pub reference: SetId,
    pub other: SetId,
    pub reason: Incompatibility,
}

/// Something that is not wrong enough to refuse but is probably a mistake.
#[derive(Debug, Clone, PartialEq)]
pub enum Suspicion {
    /// Frames assigned Bias that are not the shortest exposure in the session.
    /// Almost always flats through a fast optic, filed in the wrong folder.
    BiasIsNotTheShortestExposure { set: SetId, seconds: f64, shortest: f64 },
    /// A flat long enough to accumulate dark current a bias alone cannot
    /// remove, with no dark flats to remove it.
    FlatNeedsDarkFlats { set: SetId, seconds: f64 },
    /// Consecutive lights spaced at roughly twice their own exposure, which is
    /// what Canon's long-exposure noise reduction does: the camera takes a
    /// second closed-shutter exposure of equal length and subtracts it. Such a
    /// light has already had its dark removed, so a master dark over-subtracts.
    ProbableInCameraDarkSubtraction { set: SetId, interval_seconds: f64, exposure_seconds: f64 },
    /// A set holding so few frames of its kind, beside so many, that it cannot
    /// be a series anybody meant to integrate.
    ///
    /// The finding is the arithmetic and nothing more. Why those frames exist -
    /// framing, focus, a cloud, a deliberate short series for bright stars - is
    /// a fact about the evening that no file records, and guessing at it in the
    /// message would be a confident claim about something the tool cannot see.
    /// Reported and never acted on, for the same reason.
    MinorityOfKind { set: SetId, members: usize, dominant: SetId, dominant_members: usize },
    /// A calibration set whose own frames were shot across more than one night.
    ///
    /// Flats especially. A flat records the optical train at one moment;
    /// averaging three nights of flats averages three different dust patterns
    /// and three focus positions into one, and what comes out matches none of
    /// the nights. Nothing in a file says which night it belongs to, so this
    /// can only be raised, not resolved.
    CalibrationSpansNights { set: SetId, kind: FrameKind, span_seconds: i64 },
}

/// The result of partitioning one session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Partition {
    pub sets: Vec<FrameSet>,
    pub plans: Vec<StackPlan>,
    /// Active frames that carry no assigned kind. Never guessed into a set.
    pub unassigned: Vec<FrameId>,
    pub splits: Vec<Split>,
    pub suspicions: Vec<Suspicion>,
    /// `None` marks a key two sets share, which happens when a bucket contains
    /// more than one frame of unrecorded exposure: each becomes its own set and
    /// every one of them keys as `ExposureSpan { Unknown, Unknown }`. Answering
    /// with whichever was inserted last would bind a caller's cached work to an
    /// arbitrary one of them.
    by_key: HashMap<SetKey, Option<SetId>>,
}

impl Partition {
    pub fn set(&self, id: SetId) -> Option<&FrameSet> {
        self.sets.get(id.index())
    }

    /// Every plan with the number of active lights it would stack, deepest
    /// first.
    ///
    /// Taking the plans in the order they happen to be in was a real bug and
    /// not a theoretical one. Set order is deterministic but arbitrary, and on
    /// this project's reference session the first plan is a two-frame set left
    /// over from framing — whose matched dark is a single one-second frame. A
    /// command built a master dark out of one frame and said so quietly, while
    /// its own scan output had already named the 226-frame set.
    ///
    /// Active lights rather than exposure or capture order: the set the user
    /// spent the night on is the one with the frames in it. Ties are left as
    /// ties, because two equally deep sets are two nights or two targets and
    /// only the caller knows which — a window can show both, a command has to
    /// ask.
    pub fn plans_by_depth(&self, session: &Session) -> Vec<(&StackPlan, usize)> {
        let mut ranked: Vec<(&StackPlan, usize)> = self
            .plans
            .iter()
            .filter_map(|plan| {
                self.set(plan.lights).map(|lights| {
                    let active =
                        lights.members.iter().filter(|id| session[**id].is_active()).count();
                    (plan, active)
                })
            })
            .collect();
        ranked.sort_by_key(|(plan, active)| (std::cmp::Reverse(*active), plan.lights.index()));
        ranked
    }

    /// Rebinds a set after a re-partition. The only safe way to carry anything
    /// across two calls to [`Session::partition`].
    pub fn set_by_key(&self, key: &SetKey) -> Option<SetId> {
        self.by_key.get(key).copied().flatten()
    }

    pub fn plan_for(&self, lights: SetId) -> Option<&StackPlan> {
        self.plans.iter().find(|plan| plan.lights == lights)
    }

    pub fn sets_of(&self, kind: FrameKind) -> impl Iterator<Item = &FrameSet> {
        self.sets.iter().filter(move |set| set.kind() == kind)
    }
}

/// Partitions a session's active, assigned frames into sets, then plans one
/// integration per light set.
///
/// Two passes, and the order is the whole trick. The first hashes on
/// [`PartitionKey`], which is exact. The second sorts each bucket by exposure
/// and cuts it into runs, extending a run only while the whole run's span stays
/// inside the tolerance of its own longest member — which is deterministic,
/// independent of the order the files were scanned in, and cannot chain a night
/// of sixty-second subs into a night of six-hundred-second ones the way a naive
/// pairwise merge can.
pub fn partition(session: &Session, tolerances: &Tolerances) -> Partition {
    let mut buckets: HashMap<PartitionKey, Vec<FrameId>> = HashMap::new();
    let mut unassigned = Vec::new();

    for (id, record) in session.active() {
        let Some(kind) = record.kind() else {
            unassigned.push(id);
            continue;
        };
        buckets.entry(partition_key(session, kind, record)).or_default().push(id);
    }

    // Sorted so a report lists sets in the same order every run, whatever order
    // the filesystem yielded.
    let mut buckets: Vec<_> = buckets.into_iter().collect();
    buckets.sort_by_key(|(key, _)| sort_key(key));

    let mut result = Partition::default();
    for (key, members) in buckets {
        for run in cut_into_exposure_runs(session, members, tolerances) {
            let id = SetId(result.sets.len() as u32);
            let span = exposure_span(session, &run);
            let set_key = SetKey { partition: key.clone(), exposure: span };
            result
                .by_key
                .entry(set_key.clone())
                .and_modify(|slot| *slot = None)
                .or_insert(Some(id));
            result.sets.push(FrameSet { id, key: set_key, members: in_capture_order(session, run) });
        }
    }

    result.splits = find_splits(&result);
    result.plans = plan_all(session, &result, tolerances);
    result.suspicions = find_suspicions(session, &result, tolerances);
    result.unassigned = unassigned;
    result
}

fn partition_key(session: &Session, kind: FrameKind, record: &FrameRecord) -> PartitionKey {
    PartitionKey {
        kind,
        group: session.group_name(record.group).into(),
        body: record.body.clone(),
        geometry: GeometryKey::from_layout(&record.layout),
        gain: GainBucket::from_iso(record.info.iso),
    }
}

fn sort_key(key: &PartitionKey) -> (FrameKind, String, String, u32, u32, GainBucket) {
    (
        key.kind,
        key.group.to_string(),
        key.body.display(),
        key.geometry.width,
        key.geometry.height,
        key.gain,
    )
}

/// Cuts one exact-key bucket into runs of the same exposure.
fn cut_into_exposure_runs(
    session: &Session,
    members: Vec<FrameId>,
    tolerances: &Tolerances,
) -> Vec<Vec<FrameId>> {
    let mut ordered: Vec<(ExposureBucket, FrameId)> = members
        .into_iter()
        .map(|id| (ExposureBucket::from_seconds(session[id].info.exposure_seconds), id))
        .collect();
    // Sorting first is what makes the result independent of scan order.
    ordered.sort();

    let mut runs: Vec<Vec<FrameId>> = Vec::new();
    let mut run: Vec<FrameId> = Vec::new();
    let mut run_shortest: Option<f64> = None;
    let mut run_longest = 0.0f64;

    for (bucket, id) in ordered {
        let Some(seconds) = bucket.seconds() else {
            // An unrecorded exposure joins nothing, including other unrecorded
            // ones: WBPP substitutes 1e10 here, and this project forbids a
            // plausible-looking stand-in.
            runs.push(vec![id]);
            continue;
        };
        match run_shortest {
            Some(shortest) if compat::same_exposure(shortest, seconds.max(run_longest), tolerances) => {
                run_longest = run_longest.max(seconds);
                run.push(id);
            }
            Some(_) => {
                runs.push(std::mem::take(&mut run));
                run_shortest = Some(seconds);
                run_longest = seconds;
                run.push(id);
            }
            None => {
                run_shortest = Some(seconds);
                run_longest = seconds;
                run.push(id);
            }
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    runs
}

fn exposure_span(session: &Session, members: &[FrameId]) -> ExposureSpan {
    let buckets =
        members.iter().map(|id| ExposureBucket::from_seconds(session[*id].info.exposure_seconds));
    let mut shortest = ExposureBucket::Unknown;
    let mut longest = ExposureBucket::Unknown;
    for (index, bucket) in buckets.enumerate() {
        if index == 0 {
            shortest = bucket;
            longest = bucket;
        } else {
            shortest = shortest.min(bucket);
            longest = longest.max(bucket);
        }
    }
    ExposureSpan { shortest, longest }
}

fn in_capture_order(session: &Session, mut members: Vec<FrameId>) -> Vec<FrameId> {
    members.sort_by_key(|id| (session[*id].info.capture_time_unix, *id));
    members
}

/// Names why frames of one kind ended up in more than one set.
fn find_splits(partition: &Partition) -> Vec<Split> {
    let mut splits = Vec::new();
    for kind in FrameKind::ALL {
        let mut of_kind: Vec<&FrameSet> = partition.sets_of(kind).collect();
        if of_kind.len() < 2 {
            continue;
        }
        // The largest set is the reference: it is what the user meant.
        of_kind.sort_by_key(|set| std::cmp::Reverse(set.len()));
        let reference = of_kind[0];
        for other in &of_kind[1..] {
            // Only a geometry difference is worth explaining here. Two sets that
            // differ by exposure or gain are two deliberate configurations.
            if let Err(reason) =
                compat::compatible(&reference.key.partition.geometry, &other.key.partition.geometry)
            {
                splits.push(Split { kind, reference: reference.id, other: other.id, reason });
            }
        }
    }
    splits
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

fn plan_all(session: &Session, partition: &Partition, tolerances: &Tolerances) -> Vec<StackPlan> {
    partition
        .sets_of(FrameKind::Light)
        .map(|lights| plan_one(session, partition, lights, tolerances))
        .collect()
}

fn plan_one(
    session: &Session,
    partition: &Partition,
    lights: &FrameSet,
    tolerances: &Tolerances,
) -> StackPlan {
    let mut blocked = Vec::new();

    let bias = resolve(session, partition, lights, FrameKind::Bias, tolerances, &mut blocked);
    let dark = resolve(session, partition, lights, FrameKind::Dark, tolerances, &mut blocked);
    let flat = resolve(session, partition, lights, FrameKind::Flat, tolerances, &mut blocked);

    // A dark flat calibrates the flats. With no flats there is nothing for it
    // to do, and matching it against the lights instead would subtract a 1/60 s
    // dark from a 300 s light and remove almost nothing.
    let dark_flat = flat
        .as_ref()
        .and_then(|matched| partition.set(matched.set))
        .and_then(|flats| {
            resolve(session, partition, flats, FrameKind::DarkFlat, tolerances, &mut blocked)
        });

    StackPlan { lights: lights.id, bias, dark, flat, dark_flat, blocked }
}

/// Matches one calibration role, recording a refusal that leaves the role empty.
fn resolve(
    session: &Session,
    partition: &Partition,
    against: &FrameSet,
    kind: FrameKind,
    tolerances: &Tolerances,
    blocked: &mut Vec<BlockedRole>,
) -> Option<CalibrationMatch> {
    let pairing = Pairing::for_calibration(kind).expect("every calibration kind has a pairing");
    let (found, refused) = best_match(session, against, partition, kind, pairing, tolerances);
    // "You shot no darks" and "you shot darks and I could not use them" must
    // not look the same: in the second case the user believes the stack was
    // calibrated.
    if found.is_none() && !refused.is_empty() {
        blocked.push(BlockedRole { kind, against: against.id, candidates: refused });
    }
    found
}

/// Picks the calibration set that fits `against` best.
///
/// Ordered by exposure first for darks and by gain first for flats and biases,
/// because the asymmetry is optical: dark signal is a function of exposure at a
/// given gain, a flat is normalised before division so its exposure carries
/// nothing, and a bias is exposure-free by construction. Sets that fail
/// [`compat::admissible`] are not candidates at all; among those that pass, the
/// closest wins and ties break on member count, so the deeper master is
/// preferred.
///
/// Returns the match and, separately, every candidate that was refused — the
/// caller needs the refusals to tell "you shot no darks" from "you shot darks
/// and I could not use them".
pub fn best_match(
    session: &Session,
    against: &FrameSet,
    partition: &Partition,
    kind: FrameKind,
    pairing: Pairing,
    tolerances: &Tolerances,
) -> (Option<CalibrationMatch>, Vec<(SetId, Incompatibility)>) {
    let target = &session[against.representative(session)];
    let mut admitted: Vec<&FrameSet> = Vec::new();
    let mut refused: Vec<(SetId, Incompatibility)> = Vec::new();

    for candidate in partition.sets_of(kind) {
        if !may_draw_from(&against.key.partition.group, &candidate.key.partition.group) {
            continue;
        }
        let source = &session[candidate.representative(session)];
        match compat::admissible(target, source, pairing) {
            Ok(()) => admitted.push(candidate),
            Err(reason) => refused.push((candidate.id, reason)),
        }
    }

    let exposure_first = matches!(pairing, Pairing::DarkToLight | Pairing::DarkFlatToFlat);
    admitted.sort_by(|a, b| {
        let rank = |set: &FrameSet| {
            // For a dark, exposure decides: dark signal accumulates with time.
            // For a flat or a bias, gain decides and exposure is deliberately
            // not consulted even as a tie-break — a flat is normalised before
            // it divides anything, so ranking flats by exposure would encode a
            // rule this module explicitly says is meaningless. Ties go to the
            // set with more frames, whose master will be the less noisy.
            let primary = if exposure_first {
                distance(set.key.exposure.nominal(), target.info.exposure_seconds)
            } else {
                distance(gain_of(set), target.info.iso)
            };
            (primary, std::cmp::Reverse(set.len()), set.id)
        };
        rank(a).partial_cmp(&rank(b)).unwrap_or(std::cmp::Ordering::Equal)
    });

    let Some(best) = admitted.first().copied() else {
        return (None, refused);
    };

    let source = &session[best.representative(session)];
    let mismatches = compat::differences(target, source, pairing, tolerances);
    // A rule that could not run is a third state, not a difference — but only
    // sensor temperature is discounted here, and only because no Canon body
    // records it, so counting it would make `Exact` unreachable on the hardware
    // this project targets. Any other unrun rule keeps `Exact` off the table:
    // "nothing differed" must not be printed over a comparison that never
    // happened.
    let differs = mismatches.iter().any(|finding| {
        !matches!(finding, Mismatch::Unrecorded { property: Property::SensorTemperature })
    });
    let quality = if !differs {
        MatchQuality::Exact
    } else if admitted.len() == 1 {
        MatchQuality::OnlyCandidate
    } else if exposure_first {
        MatchQuality::ClosestExposure
    } else {
        MatchQuality::ClosestGain
    };

    let matched = CalibrationMatch {
        set: best.id,
        pairing,
        quality,
        alternatives: admitted.len() - 1,
        mismatches,
    };
    (Some(matched), refused)
}

fn gain_of(set: &FrameSet) -> Option<f64> {
    match set.key.partition.gain {
        GainBucket::Iso(iso) => Some(f64::from(iso)),
        GainBucket::Unknown => None,
    }
}

/// How far apart two optional values are. An absent value sorts last, rather
/// than pretending to be a perfect match at zero distance.
fn distance(a: Option<f64>, b: Option<f64>) -> f64 {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs(),
        _ => f64::MAX,
    }
}

// ---------------------------------------------------------------------------
// Suspicions
// ---------------------------------------------------------------------------

/// Longer than this, a flat accumulates enough dark current that a bias alone
/// will not remove it and dark flats are needed.
const FLAT_NEEDS_DARK_FLATS_SECONDS: f64 = 1.0;

/// How much longer than the session's shortest frame something assigned Bias
/// may be before it is probably not a bias.
///
/// Consecutive shutter steps are factors of two, so four allows a session that
/// mixes two bodies with different minimum shutters while still separating a
/// 1/8000 s bias from a 1/250 s flat filed in the wrong folder.
const BIAS_EXPOSURE_RATIO: f64 = 4.0;

/// A set is a minority when the largest set of its kind holds at least this
/// many times as many frames.
const MINORITY_RATIO: usize = 10;

/// ...and holds fewer than this many frames itself.
///
/// Both conditions, because either alone is wrong. A ratio alone would flag a
/// deliberate 20-frame series beside 226. A count alone would flag the whole of
/// a short session. Together they describe the only case that is unambiguous:
/// too few frames to reject an outlier from, let alone to integrate, sitting
/// beside a series ten times deeper.
const MINORITY_CEILING: usize = 6;

/// Below this exposure, the in-camera dark subtraction test is not run.
///
/// The test is a ratio, and at short exposures ordinary overhead reaches it: a
/// 20-second sub on an intervalometer set to a round 45 seconds trips a
/// 1.8x-2.4x window with nothing wrong. Canon's long-exposure noise reduction
/// is a long-exposure feature, so restricting the test to long exposures costs
/// nothing real.
const IN_CAMERA_DARK_MIN_EXPOSURE_SECONDS: f64 = 60.0;

fn find_suspicions(
    session: &Session,
    partition: &Partition,
    tolerances: &Tolerances,
) -> Vec<Suspicion> {
    let mut suspicions = Vec::new();

    let shortest = partition
        .sets
        .iter()
        .filter_map(|set| set.key.exposure.shortest.seconds())
        .fold(f64::INFINITY, f64::min);

    for set in partition.sets_of(FrameKind::Bias) {
        // A ratio, not `same_exposure`: that comparison has a 10 ms floor so
        // that 1/8000 s and 1/4000 s count as one bias exposure, which means it
        // can never separate a bias from a 1/250 s flat. Both are under the
        // floor and the thing that distinguishes them is the factor between
        // them.
        // The longest frame in the set, not the shortest: a 1/250 s flat filed
        // as a bias merges into the same set as the real biases, because below
        // the floor they genuinely are one exposure as far as accumulated dark
        // current goes. What gives it away is the factor between the ends.
        if let Some(seconds) = set.key.exposure.longest.seconds()
            && shortest.is_finite()
            && seconds > shortest * BIAS_EXPOSURE_RATIO
        {
            suspicions.push(Suspicion::BiasIsNotTheShortestExposure { set: set.id, seconds, shortest });
        }
    }

    for set in partition.sets_of(FrameKind::Flat) {
        if let Some(seconds) = set.key.exposure.nominal()
            && seconds > FLAT_NEEDS_DARK_FLATS_SECONDS
            && !partition.plans.iter().any(|plan| {
                plan.flat.as_ref().is_some_and(|matched| matched.set == set.id)
                    && plan.dark_flat.is_some()
            })
        {
            suspicions.push(Suspicion::FlatNeedsDarkFlats { set: set.id, seconds });
        }
    }

    for set in partition.sets_of(FrameKind::Light) {
        if let Some(found) = in_camera_dark_subtraction(session, set) {
            suspicions.push(found);
        }
    }

    for kind in FrameKind::ALL {
        let mut of_kind: Vec<&FrameSet> = partition.sets_of(kind).collect();
        if of_kind.len() < 2 {
            continue;
        }
        of_kind.sort_by_key(|set| std::cmp::Reverse(set.len()));
        let dominant = of_kind[0];
        for other in &of_kind[1..] {
            if other.len() < MINORITY_CEILING && other.len() * MINORITY_RATIO <= dominant.len() {
                suspicions.push(Suspicion::MinorityOfKind {
                    set: other.id,
                    members: other.len(),
                    dominant: dominant.id,
                    dominant_members: dominant.len(),
                });
            }
        }
    }

    for set in &partition.sets {
        let kind = set.kind();
        // Flats only. The twelve-hour threshold is about the optical train —
        // dust and focus at one moment — and a dark has no optical train. A dark
        // run spanning two nights is ordinary practice, and warning about it
        // would train the user to ignore the line that matters.
        if !matches!(kind, FrameKind::Flat | FrameKind::DarkFlat) {
            continue;
        }
        if let Some(span) = capture_span(session, set)
            && span > tolerances.flat_age_note_seconds
        {
            suspicions.push(Suspicion::CalibrationSpansNights { set: set.id, kind, span_seconds: span });
        }
    }

    suspicions
}

/// How long the set took to shoot, end to end.
fn capture_span(session: &Session, set: &FrameSet) -> Option<i64> {
    let times: Vec<i64> =
        set.members.iter().filter_map(|id| session[*id].info.capture_time_unix).collect();
    let first = times.iter().copied().min()?;
    let last = times.iter().copied().max()?;
    Some(last - first)
}

/// Canon's long-exposure noise reduction shoots a second, closed-shutter frame
/// of equal length after every exposure. Nothing in EXIF records that it was on;
/// the interval between consecutive frames is the only evidence there is.
fn in_camera_dark_subtraction(session: &Session, set: &FrameSet) -> Option<Suspicion> {
    let exposure = set.key.exposure.nominal()?;
    if set.len() < 3 {
        return None;
    }
    let times: Vec<i64> =
        set.members.iter().filter_map(|id| session[*id].info.capture_time_unix).collect();
    if times.len() < 3 {
        return None;
    }
    let mut intervals: Vec<f64> =
        times.windows(2).map(|pair| (pair[1] - pair[0]) as f64).filter(|gap| *gap > 0.0).collect();
    if intervals.len() < 2 {
        return None;
    }
    intervals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = intervals[intervals.len() / 2];

    // Below a minute, ordinary per-frame overhead — download, dither, settle —
    // is itself comparable to the exposure, so the ratio alone would flag a
    // perfectly normal cadence. Long-exposure noise reduction is also neither
    // common nor distinguishable down there.
    if exposure < IN_CAMERA_DARK_MIN_EXPOSURE_SECONDS {
        return None;
    }
    // Twice the exposure, plus readout and a little slack for the intervalometer.
    (median > exposure * 1.8 && median < exposure * 2.4).then_some(
        Suspicion::ProbableInCameraDarkSubtraction {
            set: set.id,
            interval_seconds: median,
            exposure_seconds: exposure,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::super::testing::{self, bias, flat, info, light};
    use super::super::{ExclusionSource, FrameRole, Rejection};
    use super::*;

    fn partitioned(session: &Session) -> Partition {
        partition(session, &Tolerances::default())
    }

    fn members(partition: &Partition, id: SetId) -> usize {
        partition.set(id).map_or(0, FrameSet::len)
    }

    #[test]
    fn exposure_runs_do_not_chain_across_a_night() {
        // A naive pairwise merge would swallow this whole ladder into one set,
        // because every rung is within five per cent of its neighbour.
        let mut session = Session::new();
        let mut seconds = 60.0;
        while seconds <= 600.0 {
            testing::assigned(
                &mut session,
                FrameKind::Light,
                &format!("s{seconds}.CR3"),
                info(seconds, 1600.0),
            );
            seconds *= 1.03;
        }

        let partition = partitioned(&session);
        let runs: Vec<f64> =
            partition.sets_of(FrameKind::Light).filter_map(|set| set.key.exposure.nominal()).collect();
        assert!(runs.len() > 3, "the ladder must be cut into runs, got {runs:?}");
        // And the ends are certainly not together.
        let first = partition.sets_of(FrameKind::Light).next().unwrap();
        assert!(first.key.exposure.longest.seconds().unwrap() < 120.0);
    }

    #[test]
    fn exposure_grouping_does_not_depend_on_scan_order() {
        // The property that makes a tolerance safe to use as a grouping rule.
        let exposures = [300.0, 60.0, 305.0, 61.0, 297.0, 600.0];
        let shape = |order: &[usize]| {
            let mut session = Session::new();
            for index in order {
                let seconds = exposures[*index];
                testing::assigned(
                    &mut session,
                    FrameKind::Light,
                    &format!("f{index}.CR3"),
                    info(seconds, 1600.0),
                );
            }
            let partition = partitioned(&session);
            let mut sizes: Vec<usize> =
                partition.sets_of(FrameKind::Light).map(FrameSet::len).collect();
            sizes.sort_unstable();
            sizes
        };

        let forward = shape(&[0, 1, 2, 3, 4, 5]);
        assert_eq!(forward, shape(&[5, 4, 3, 2, 1, 0]));
        assert_eq!(forward, shape(&[2, 0, 5, 3, 1, 4]));
    }

    #[test]
    fn an_unknown_exposure_forms_its_own_set() {
        // PixInsight substitutes a huge stand-in value here. This project
        // forbids a plausible-looking default, so the frame stands alone.
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "a", 3, light());
        let mut unknown = light();
        unknown.exposure_seconds = None;
        testing::assigned(&mut session, FrameKind::Light, "odd.CR3", unknown);

        let partition = partitioned(&session);
        let sets: Vec<&FrameSet> = partition.sets_of(FrameKind::Light).collect();
        assert_eq!(sets.len(), 2);
        let lone = sets.iter().find(|set| set.len() == 1).expect("the odd frame stands alone");
        assert_eq!(lone.key.exposure.longest, ExposureBucket::Unknown);
    }

    #[test]
    fn rggb_and_grbg_frames_of_the_same_size_do_not_share_a_set() {
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "good", 5, light());
        let mut odd = testing::record("phase.CR3", light());
        odd.role = FrameRole::Assigned(FrameKind::Light);
        odd.layout.cfa_pattern[..4].copy_from_slice(&[1, 0, 2, 1]);
        session.insert(odd);

        let partition = partitioned(&session);
        assert_eq!(partition.sets_of(FrameKind::Light).count(), 2);
        // And the report can say why, rather than showing two sets and leaving
        // the user to work out the difference.
        let split = partition.splits.first().expect("the split is explained");
        assert!(matches!(split.reason, Incompatibility::CfaPattern { .. }), "{split:?}");
        assert_eq!(members(&partition, split.reference), 5);
    }

    #[test]
    fn matching_a_dark_prefers_the_nearest_exposure() {
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "L", 10, light());
        testing::run(&mut session, FrameKind::Dark, "D240", 20, info(240.0, 1600.0));
        testing::run(&mut session, FrameKind::Dark, "D300", 5, info(300.0, 1600.0));

        let partition = partitioned(&session);
        let plan = &partition.plans[0];
        let dark = plan.dark.as_ref().expect("a dark was matched");
        // Five well-matched darks beat twenty badly-matched ones.
        assert_eq!(members(&partition, dark.set), 5);
        assert_eq!(dark.alternatives, 1);
    }

    #[test]
    fn matching_a_flat_prefers_gain_and_ignores_exposure() {
        // A flat is normalised to unit mean before it divides anything, so its
        // exposure carries no information at all.
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "L", 10, light());
        testing::run(&mut session, FrameKind::Flat, "F400", 30, info(1.0 / 60.0, 400.0));
        testing::run(&mut session, FrameKind::Flat, "F1600", 20, info(2.0, 1600.0));

        let partition = partitioned(&session);
        let matched = partition.plans[0].flat.as_ref().expect("a flat was matched");
        // The ISO 1600 flats win on gain despite a wildly different exposure
        // and despite being the smaller set.
        assert_eq!(members(&partition, matched.set), 20);
        let findings = &matched.mismatches;
        assert!(
            !findings.iter().any(|m| matches!(m, Mismatch::Exposure { .. })),
            "a flat is never faulted for its exposure: {findings:?}"
        );
    }

    #[test]
    fn a_dark_at_the_wrong_iso_is_blocked_rather_than_silently_dropped() {
        // "You shot no darks" and "you shot darks and I ignored them" must not
        // look the same: in the second case the user believes the stack was
        // calibrated.
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "L", 10, light());
        testing::run(&mut session, FrameKind::Dark, "D", 20, info(300.0, 800.0));

        let partition = partitioned(&session);
        let plan = &partition.plans[0];
        assert!(plan.dark.is_none());
        let blocked =
            plan.blocked.iter().find(|role| role.kind == FrameKind::Dark).expect("named as refused");
        assert!(matches!(blocked.candidates[0].1, Incompatibility::Gain { .. }));
    }

    #[test]
    fn a_dark_flat_is_matched_against_the_flats_not_the_lights() {
        // Matching it against the lights would subtract a 1/60 s dark from a
        // 300 s light and remove almost nothing.
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "L", 10, light());
        testing::run(&mut session, FrameKind::Flat, "F", 30, flat());
        testing::run(&mut session, FrameKind::DarkFlat, "DF", 30, info(1.0 / 60.0, 400.0));

        let partition = partitioned(&session);
        let plan = &partition.plans[0];
        let dark_flat = plan.dark_flat.as_ref().expect("a dark flat was matched");
        assert_eq!(dark_flat.pairing, Pairing::DarkFlatToFlat);
        assert_eq!(dark_flat.quality, MatchQuality::Exact);
    }

    #[test]
    fn a_dark_flat_without_flats_is_not_matched_to_the_lights() {
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "L", 10, light());
        testing::run(&mut session, FrameKind::DarkFlat, "DF", 30, info(1.0 / 60.0, 1600.0));

        let partition = partitioned(&session);
        assert!(partition.plans[0].flat.is_none());
        assert!(partition.plans[0].dark_flat.is_none());
    }

    #[test]
    fn a_bias_library_in_the_main_group_reaches_every_night() {
        let mut session = Session::new();
        let monday = session.intern_group("mon");
        let tuesday = session.intern_group("tue");

        for (group, prefix) in [(monday, "mon"), (tuesday, "tue")] {
            for index in 0..5 {
                testing::assigned_in(
                    &mut session,
                    group,
                    FrameKind::Light,
                    &format!("{prefix}L{index}.CR3"),
                    light(),
                );
            }
        }
        for index in 0..10 {
            // The library, shot once and reused.
            testing::assigned(&mut session, FrameKind::Bias, &format!("B{index}.CR3"), bias());
        }
        for index in 0..10 {
            // Monday's darks, which must stay with Monday.
            testing::assigned_in(
                &mut session,
                monday,
                FrameKind::Dark,
                &format!("monD{index}.CR3"),
                light(),
            );
        }

        let partition = partitioned(&session);
        assert_eq!(partition.plans.len(), 2);
        assert!(partition.plans.iter().all(|plan| plan.bias.is_some()), "the library serves both");

        let group_of = |plan: &StackPlan| {
            partition.set(plan.lights).unwrap().key.partition.group.to_string()
        };
        let monday_plan = partition.plans.iter().find(|p| group_of(p) == "mon").unwrap();
        let tuesday_plan = partition.plans.iter().find(|p| group_of(p) == "tue").unwrap();
        assert!(monday_plan.dark.is_some());
        assert!(tuesday_plan.dark.is_none(), "Monday's darks do not reach Tuesday");
        assert!(tuesday_plan.blocked.is_empty(), "and are not reported as refused either");
    }

    #[test]
    fn a_handful_of_frames_beside_a_deep_series_is_named() {
        // The four framing and focus tests at the start of a real night, which
        // the tool used to present as peers of the 226-frame series.
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "main", 226, light());
        testing::run(&mut session, FrameKind::Light, "test", 2, info(1.0, 6400.0));

        let partition = partitioned(&session);
        let minorities: Vec<&Suspicion> = partition
            .suspicions
            .iter()
            .filter(|s| matches!(s, Suspicion::MinorityOfKind { .. }))
            .collect();
        assert_eq!(minorities.len(), 1, "{:?}", partition.suspicions);
        let Suspicion::MinorityOfKind { members, dominant_members, .. } = minorities[0] else {
            unreachable!()
        };
        assert_eq!((*members, *dominant_members), (2, 226));
    }

    #[test]
    fn a_deliberate_short_series_is_not_called_a_minority() {
        // Twenty frames shot on purpose for the bright stars, beside 226 long
        // subs. The ratio alone would condemn them; the frame count is what
        // saves them, and it has to, because no file records the difference
        // between a short series and a mistake.
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "main", 226, light());
        testing::run(&mut session, FrameKind::Light, "short", 20, info(2.0, 6400.0));

        let partition = partitioned(&session);
        assert!(
            !partition.suspicions.iter().any(|s| matches!(s, Suspicion::MinorityOfKind { .. })),
            "{:?}",
            partition.suspicions
        );
    }

    #[test]
    fn a_short_session_is_not_all_minority() {
        // Ten lights and nothing else: small, but it is the whole session, and
        // there is no deeper series for it to be a minority of.
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "L", 10, light());

        let partition = partitioned(&session);
        assert!(
            !partition.suspicions.iter().any(|s| matches!(s, Suspicion::MinorityOfKind { .. })),
            "{:?}",
            partition.suspicions
        );
    }

    #[test]
    fn a_bias_that_is_not_the_shortest_exposure_is_suspicious() {
        // Flats through a fast optic land at 1/2000 s and get filed as biases.
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "L", 5, light());
        testing::run(&mut session, FrameKind::Bias, "B", 10, bias());
        testing::run(&mut session, FrameKind::Bias, "X", 10, info(1.0 / 250.0, 1600.0));

        let partition = partitioned(&session);
        let flagged: Vec<&Suspicion> = partition
            .suspicions
            .iter()
            .filter(|s| matches!(s, Suspicion::BiasIsNotTheShortestExposure { .. }))
            .collect();
        assert_eq!(flagged.len(), 1, "only the odd set: {:?}", partition.suspicions);
    }

    #[test]
    fn a_long_flat_with_no_dark_flats_is_flagged() {
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "L", 5, light());
        testing::run(&mut session, FrameKind::Flat, "F", 20, info(4.0, 1600.0));

        let partition = partitioned(&session);
        assert!(
            partition.suspicions.iter().any(|s| matches!(s, Suspicion::FlatNeedsDarkFlats { .. })),
            "{:?}",
            partition.suspicions
        );
    }

    #[test]
    fn flats_shot_across_several_nights_are_flagged() {
        // The failure this catches: averaging three nights of flats averages
        // three dust patterns into one that matches none of the nights.
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "L", 5, light());
        for night in 0..3 {
            let mut when = flat();
            when.capture_time_unix = Some(1_787_834_096 + night * 86_400);
            testing::assigned(&mut session, FrameKind::Flat, &format!("F{night}.CR3"), when);
        }

        let partition = partitioned(&session);
        assert!(
            partition
                .suspicions
                .iter()
                .any(|s| matches!(s, Suspicion::CalibrationSpansNights { kind: FrameKind::Flat, .. })),
            "{:?}",
            partition.suspicions
        );
    }

    #[test]
    fn in_camera_dark_subtraction_is_noticed_from_the_frame_interval() {
        // Canon's long-exposure noise reduction leaves no EXIF trace; the gap
        // between consecutive frames is the only evidence there is.
        let mut session = Session::new();
        for index in 0..6 {
            let mut frame = info(300.0, 1600.0);
            // 300 s of exposure, then 300 s of closed-shutter dark.
            frame.capture_time_unix = Some(1_787_834_096 + 604 * index);
            testing::assigned(&mut session, FrameKind::Light, &format!("L{index}.CR3"), frame);
        }

        let partition = partitioned(&session);
        assert!(
            partition
                .suspicions
                .iter()
                .any(|s| matches!(s, Suspicion::ProbableInCameraDarkSubtraction { .. })),
            "{:?}",
            partition.suspicions
        );
    }

    #[test]
    fn an_ordinary_run_of_lights_is_not_mistaken_for_in_camera_dark_subtraction() {
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "L", 6, light());
        let partition = partitioned(&session);
        assert!(
            !partition
                .suspicions
                .iter()
                .any(|s| matches!(s, Suspicion::ProbableInCameraDarkSubtraction { .. })),
            "{:?}",
            partition.suspicions
        );
    }

    #[test]
    fn unassigned_frames_are_never_guessed_into_a_set() {
        // The policy sentence, enforced: evidence may propose, never elect.
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "L", 5, light());
        session.insert(testing::record("IMG_0001.CR3", light()));

        let partition = partitioned(&session);
        assert_eq!(partition.unassigned.len(), 1);
        assert_eq!(partition.sets_of(FrameKind::Light).map(FrameSet::len).sum::<usize>(), 5);
    }

    #[test]
    fn two_sets_that_share_a_key_are_not_answered_for() {
        // Every unrecorded exposure becomes its own set, and every one of them
        // keys as ExposureSpan { Unknown, Unknown }. Handing back whichever was
        // inserted last would bind a caller to an arbitrary one of them.
        let mut session = Session::new();
        for name in ["a.cr3", "b.cr3"] {
            let mut odd = light();
            odd.exposure_seconds = None;
            testing::assigned(&mut session, FrameKind::Light, name, odd);
        }

        let partition = partitioned(&session);
        assert_eq!(partition.sets_of(FrameKind::Light).count(), 2);
        let key = partition.sets_of(FrameKind::Light).next().unwrap().key.clone();
        assert_eq!(partition.set_by_key(&key), None, "an ambiguous key answers for nothing");
    }

    #[test]
    fn exact_is_not_claimed_when_the_exposure_could_not_be_compared() {
        // Sensor temperature is discounted because no Canon body records it.
        // An unrecorded exposure is a different matter: it is the property a
        // dark is ranked on.
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "L", 5, light());
        let mut dark = light();
        dark.exposure_seconds = None;
        testing::assigned(&mut session, FrameKind::Dark, "D.cr3", dark);

        let partition = partitioned(&session);
        let matched = partition.plans[0].dark.as_ref().expect("still matched, never refused");
        assert_ne!(matched.quality, MatchQuality::Exact);
    }

    #[test]
    fn a_blocked_dark_flat_records_that_it_was_judged_against_the_flats() {
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "L", 5, light());
        testing::run(&mut session, FrameKind::Flat, "F", 10, flat());
        // Right exposure for the flats, wrong gain, so it is refused.
        testing::run(&mut session, FrameKind::DarkFlat, "DF", 10, info(1.0 / 60.0, 1600.0));

        let partition = partitioned(&session);
        let plan = &partition.plans[0];
        let blocked = plan
            .blocked
            .iter()
            .find(|role| role.kind == FrameKind::DarkFlat)
            .expect("the refused dark flats are named");
        let against = partition.set(blocked.against).expect("a real set");
        assert_eq!(against.kind(), FrameKind::Flat, "never judged against the lights");
    }

    #[test]
    fn a_short_sub_on_a_round_intervalometer_is_not_called_in_camera_dark_subtraction() {
        // 20 s subs every 45 s: ordinary download, dither and settle, and a
        // ratio of 2.25 that the test used to fire on.
        let mut session = Session::new();
        for index in 0..6 {
            let mut frame = info(20.0, 1600.0);
            frame.capture_time_unix = Some(1_787_834_096 + 45 * index);
            testing::assigned(&mut session, FrameKind::Light, &format!("L{index}.CR3"), frame);
        }

        let partition = partitioned(&session);
        assert!(
            !partition
                .suspicions
                .iter()
                .any(|s| matches!(s, Suspicion::ProbableInCameraDarkSubtraction { .. })),
            "{:?}",
            partition.suspicions
        );
    }

    #[test]
    fn a_dark_run_across_two_nights_is_not_flagged_as_a_flat_would_be() {
        // The twelve-hour threshold is about dust and focus. A dark has neither,
        // and a dark library shot over several nights is ordinary practice.
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "L", 5, light());
        for night in 0..3 {
            let mut when = light();
            when.capture_time_unix = Some(1_787_834_096 + night * 86_400);
            testing::assigned(&mut session, FrameKind::Dark, &format!("D{night}.CR3"), when);
        }

        let partition = partitioned(&session);
        assert!(
            !partition
                .suspicions
                .iter()
                .any(|s| matches!(s, Suspicion::CalibrationSpansNights { .. })),
            "{:?}",
            partition.suspicions
        );
    }

    #[test]
    fn a_set_key_survives_the_flags_being_typed_in_another_order() {
        // Group ids are handed out in first-seen order, so a key carrying one
        // would rebind to the other night when the same session is described
        // the other way round.
        let key_for = |first: &str, second: &str| {
            let mut session = Session::new();
            let a = session.intern_group(first);
            let b = session.intern_group(second);
            for (group, name) in [(a, first), (b, second)] {
                testing::assigned_in(
                    &mut session,
                    group,
                    FrameKind::Light,
                    &format!("{name}.cr3"),
                    light(),
                );
            }
            let partition = partition(&session, &Tolerances::default());
            let mut keys: Vec<String> = partition
                .sets_of(FrameKind::Light)
                .map(|set| set.key.partition.group.to_string())
                .collect();
            keys.sort();
            keys
        };
        assert_eq!(key_for("mon", "tue"), key_for("tue", "mon"));
    }

    #[test]
    fn set_ids_are_rebound_by_key_after_a_repartition() {
        // SetId is a within-partition handle. Anything that outlives one
        // partition — a cached master — has to travel by SetKey.
        let mut session = Session::new();
        testing::run(&mut session, FrameKind::Light, "L", 5, light());
        testing::run(&mut session, FrameKind::Dark, "D", 5, light());

        let before = partitioned(&session);
        let dark_key = before.sets_of(FrameKind::Dark).next().unwrap().key.clone();

        // The user drops a bad light, and everything is partitioned again.
        let victim = before.sets_of(FrameKind::Light).next().unwrap().members[0];
        session.exclude(victim, Rejection::Unclassified, ExclusionSource::User);
        let after = partitioned(&session);

        let rebound = after.set_by_key(&dark_key).expect("the dark set is found again by key");
        assert_eq!(after.set(rebound).unwrap().len(), 5);
    }
}
