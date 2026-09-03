//! Reading a run: build its masters, then calibrate and measure every light.
//!
//! This lives in the core rather than in the command line because it is not
//! presentation. Three callers want the identical answer — `stars` reports the
//! shapes, `register` matches the star lists, `stack` needs both, and the window
//! needs all three — and the pass costs about 140 seconds of decoding on this
//! project's reference session. A second copy behind the window would be the
//! copy that drifts: a stack that applied the flat slightly differently from
//! the measurement that chose which frames to stack would be measuring one
//! thing and combining another.
//!
//! Nothing here prints. Progress arrives as [`Step`] values and the caller
//! decides whether that becomes a line on a terminal, a bar in a window, or
//! nothing at all.

pub mod align;
pub mod stack;
pub mod view;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use astro_plugin_abi::abi::ImageLayout;
use rayon::prelude::*;

use crate::calibrate::{CombineOptions, Master, apply_into, build};
use crate::error::{Error, Result};
use crate::session::{FrameId, FrameKind, Partition, SetId, Session, StackPlan};
use crate::stars::{DetectOptions, Detection, Scratch, detect_with};
use crate::{PluginHost, Samples};

/// The three masters a light needs, each `None` where nothing was matched.
///
/// `None` rather than a master of ones, because applying nothing and applying
/// an identity are indistinguishable in the pixels and must not be
/// indistinguishable in the report.
#[derive(Debug, Default)]
pub struct MasterSet {
    pub bias: Option<Master>,
    pub dark: Option<Master>,
    pub flat: Option<Master>,
}

impl MasterSet {
    /// Whether anything would actually be applied.
    pub fn is_empty(&self) -> bool {
        self.bias.is_none() && self.dark.is_none() && self.flat.is_none()
    }
}

/// Whether the caller wants the pass to carry on.
///
/// Returned from the progress callback rather than read from a flag the core
/// owns: the caller already knows when the user pressed cancel, and a second
/// place to look would be a second place to get it wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Stop,
}

/// What the pass is doing, as it does it.
#[derive(Debug, Clone, Copy)]
pub enum Step<'a> {
    /// Nothing was matched for this role, so nothing will be applied.
    MasterMissing { kind: FrameKind },
    /// A set was matched for this role, but this caller applies nothing from
    /// it, so no master was built. Reported rather than skipped in silence: a
    /// user who named a directory of biases has to learn that calibrating a
    /// light does not read them, or the absence looks like a fault.
    MasterUnused { kind: FrameKind, set: SetId },
    MasterReading { kind: FrameKind, set: SetId, done: usize, total: usize },
    MasterBuilt { kind: FrameKind, master: &'a Master, seconds: f64 },
    /// A light has been measured. `done` counts the frames that have
    /// *finished*, not the one that started: it is one-based, its last value is
    /// `total`, and `name` is whichever frame just finished. Lights are
    /// measured several at a time, so neither is a position in the run.
    FrameRead { done: usize, total: usize, name: &'a str },
    /// Depositing frames onto the output grid. Rejection needs two passes over
    /// them, and a progress bar that restarted without saying so reads as a
    /// crash rather than as the second pass.
    ///
    /// Unlike [`Step::FrameRead`], `done` here still counts frames *started*:
    /// depositing is one frame at a time, spread across the machine within a
    /// frame rather than across frames.
    Stacking { pass: usize, passes: usize, done: usize, total: usize, name: &'a str },
}

/// Which of the plan's masters a caller needs built.
///
/// The distinction is not a preference, it is the difference between a
/// deliverable and dead weight. [`crate::calibrate::apply`] takes a dark and a
/// flat and nothing else, because a dark that matches its lights *is* the
/// pedestal plus the dark current, so `l - D` removes both at once and the bias
/// cancels identically — see the equations in [`crate::calibrate`]. On a run
/// that only calibrates lights, a master bias is therefore a whole calibration
/// set decoded to produce a buffer nothing reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted {
    /// Only what will be applied: the dark and the flat.
    Applied,
    /// Every master the plan matched, for a caller whose purpose is to write
    /// them out rather than to use them.
    Matched,
}

impl Wanted {
    /// Whether a master of this kind is worth the frames it would read.
    pub fn includes(self, kind: FrameKind) -> bool {
        self == Wanted::Matched || kind != FrameKind::Bias
    }
}

/// Builds the masters a plan calls for.
///
/// The bias, then the dark, then the flat: the order they are applied in, which
/// is also the order a reader wants them in.
pub fn masters(
    host: &PluginHost,
    session: &Session,
    partition: &Partition,
    plan: &StackPlan,
    options: &CombineOptions,
    wanted: Wanted,
    on: &(dyn Fn(Step) -> Flow + Sync),
) -> Result<MasterSet> {
    let mut built = MasterSet::default();
    for (kind, matched) in [
        (FrameKind::Bias, plan.bias.as_ref()),
        (FrameKind::Dark, plan.dark.as_ref()),
        (FrameKind::Flat, plan.flat.as_ref()),
    ] {
        let Some(matched) = matched else {
            // A role this caller would not have used is not missing, so saying
            // nothing matched would be answering a question nobody asked.
            if wanted.includes(kind) {
                on(Step::MasterMissing { kind });
            }
            continue;
        };
        if !wanted.includes(kind) {
            on(Step::MasterUnused { kind, set: matched.set });
            continue;
        }
        let Some(set) = partition.set(matched.set) else {
            on(Step::MasterMissing { kind });
            continue;
        };

        let started = Instant::now();
        let master = build(host, session, set, options, &|done, total| {
            on(Step::MasterReading { kind, set: set.id, done, total });
        })?;
        if on(Step::MasterBuilt { kind, master: &master, seconds: started.elapsed().as_secs_f64() })
            == Flow::Stop
        {
            return Err(Error::Cancelled);
        }

        match kind {
            FrameKind::Bias => built.bias = Some(master),
            FrameKind::Dark => built.dark = Some(master),
            _ => built.flat = Some(master),
        }
    }
    Ok(built)
}

/// One light, read and measured.
///
/// Deliberately does NOT hold the pixels. A run of 225 frames is 16 GiB of
/// them, so stacking decodes a second time; what is kept is only what is small
/// and would otherwise have to be measured twice.
#[derive(Debug)]
pub struct Measured {
    pub id: FrameId,
    pub name: String,
    pub path: std::path::PathBuf,
    pub layout: ImageLayout,
    pub camera_model: String,
    pub exposure_seconds: Option<f64>,
    pub iso: Option<f64>,
    pub detection: Detection,
    /// Wall clock for this one frame, including whatever it spent sharing the
    /// machine with the others. It is a frame's share of a parallel pass and
    /// not the cost of a frame: these sum to roughly [`Survey::workers`] times
    /// [`Survey::seconds`], and the faster the pass gets the worse that grows.
    pub seconds: f64,
}

/// A whole run, read.
#[derive(Debug, Default)]
pub struct Survey {
    pub frames: Vec<Measured>,
    /// Whether the caller stopped it early. `frames` then holds what was
    /// measured before that, which is worth keeping and must not be mistaken
    /// for the whole run — nor for its beginning. Frames already under way when
    /// the stop was seen are finished and kept, so a stopped survey is the
    /// lights it managed, in the order they were given, with gaps in it. That
    /// makes it something to report and not something to align: `align` reads
    /// an index as a position in the night.
    pub stopped: bool,
    /// Frames that were selected but could not be read, with the reason.
    ///
    /// Carried rather than dropped: a report that says "225 of 226" has to be
    /// able to name the missing one, and a session that silently lost forty
    /// frames looks exactly like one that never had them.
    pub failed: Vec<(String, String)>,
    /// How many lights were measured at once. Reported because it is derived
    /// from a memory budget and a guess about the machine, not chosen.
    pub workers: usize,
    pub seconds: f64,
}

/// Cap on decoded pixel data held while a run is measured.
///
/// Deliberately not the scan's open budget, which bounds the metadata pass,
/// where a worker allocates one frame and frees it again. Measuring a light
/// holds nine times that, so the scan's budget would size this pool to a single
/// worker.
///
/// The number is a ceiling on harm rather than a search for the fastest answer,
/// because measurement says the two cannot be had from one constant. Measured
/// on this machine, a nineteen-megapixel body runs fastest at the core ceiling
/// and is still gaining there, while a forty-six-megapixel one peaks at three
/// workers, matches a single-threaded pass at six and is *slower* than one
/// beyond that. There is no byte figure that names both: the small body's best
/// holds more live bytes than the large body's worst.
///
/// What the large body is actually hitting is not settled, and this comment used
/// to name a mechanism it cannot support — that the pass saturates the rate whole
/// frames can be faulted in. It does not add up: at its optimum the small body
/// sustains several times the fresh mapping per second that the large body falls
/// over at, and a resource one case saturates harder is not what the other is
/// running into. The candidates that fit the shape are the cost of unmapping a
/// large region across many cores, and the asymmetry of this part's two core
/// complexes. Until one of them is shown, the constant stands on the measurement
/// rather than on an explanation.
///
/// Four gibibytes is therefore chosen as the largest value that was not a
/// regression on any body measured: it buys most of the available speed on a
/// small frame and a smaller but real gain on a large one, and never turns a
/// parallel pass into something slower than doing it one frame at a time.
pub const DEFAULT_SURVEY_BUDGET_BYTES: u64 = 4 << 30;

/// What measuring one light holds at its peak, as a multiple of its raw
/// samples.
///
/// What a worker holds is now resident rather than transient: the raw `u16`
/// samples at `2N`, the calibrated plane at `4N`, and the detection's scratch —
/// one plane it whitens and then filters in place, one intermediate for the
/// separable filter, and a byte per photosite for the flood fill — at `4N`,
/// `4N` and `N`. That is `15N`, which is seven and a half times the `2N` that
/// `required_bytes` reports for a mosaiced sixteen-bit frame.
///
/// Left at nine rather than tightened to eight, deliberately. Tightening it
/// would raise the derived worker count, and a change in how many frames are
/// measured at once does not belong in the same commit as a change that must be
/// shown to alter nothing: it would confound the very comparison that proves the
/// buffers are safe to reuse. It belongs behind its own sweep on both a small
/// and a large body.
///
/// Written as the buffers it counts rather than as a bare number, because
/// another full-frame buffer added to `detect` would make it wrong and nothing
/// else would notice.
const PEAK_MULTIPLE_OF_RAW: u64 = 9;

/// The progress callback, made safe to call from several workers at once.
///
/// The count and the call happen under one lock, and that is the point:
/// incrementing atomically and then calling is not the same thing, because two
/// workers can be reordered between the two and a bar would walk backwards by
/// up to the worker count. The one lock also stops sixteen workers interleaving
/// a carriage-returned terminal line into garbage, and keeps a channel's sends
/// in the order the frames actually finished. It is held for the length of the
/// callback, so a caller that does real work in there rather than printing or
/// sending makes itself the narrow point of the whole pass.
struct Reporter<'a> {
    on: &'a (dyn for<'s> Fn(Step<'s>) -> Flow + Sync),
    done: Mutex<usize>,
    stopped: AtomicBool,
}

impl<'a> Reporter<'a> {
    fn new(on: &'a (dyn for<'s> Fn(Step<'s>) -> Flow + Sync)) -> Self {
        Self { on, done: Mutex::new(0), stopped: AtomicBool::new(false) }
    }

    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    fn finished<'s>(&self, step: impl FnOnce(usize) -> Step<'s>) {
        // Into the inner value rather than unwrapping a poisoned lock: a
        // callback that panicked has already lost that one frame, and panicking
        // in every other worker would throw away the measurements they made.
        let mut done = self.done.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *done += 1;
        if (self.on)(step(*done)) == Flow::Stop {
            self.stopped.store(true, Ordering::Relaxed);
        }
    }
}

/// Calibrates and measures every light given, several at a time.
///
/// One unreadable frame in a night of two hundred does not lose the other
/// hundred and ninety-nine, and does not pass unmentioned either.
///
/// `frames` comes back in the order `lights` was given, whatever order the
/// workers finished in. That is a contract and not an accident: [`align`]
/// chooses its reference from the middle of the run and chains its seeds
/// between neighbours, so an index there means a position in the night.
pub fn survey(
    host: &PluginHost,
    session: &Session,
    lights: &[FrameId],
    masters: &MasterSet,
    options: &DetectOptions,
    on: &(dyn Fn(Step) -> Flow + Sync),
) -> Survey {
    survey_with_workers(host, session, lights, masters, options, None, on)
}

/// [`survey`], with the worker count named rather than derived.
///
/// `Some(1)` is a genuinely sequential pass, and it is kept reachable because
/// it is the only way to show that measuring a run several frames at a time
/// produces the same star lists as measuring it one frame at a time.
#[allow(clippy::too_many_arguments)]
pub fn survey_with_workers(
    host: &PluginHost,
    session: &Session,
    lights: &[FrameId],
    masters: &MasterSet,
    options: &DetectOptions,
    workers: Option<usize>,
    on: &(dyn Fn(Step) -> Flow + Sync),
) -> Survey {
    let started = Instant::now();
    let total = lights.len();
    let reporter = Reporter::new(on);
    // The lanes not currently measuring a frame. A free list rather than a slot
    // per thread: `rayon::current_thread_index` is `None` on the serial fallback
    // below, which runs on the caller's thread, and would name a stranger's slot
    // if a survey were ever run from inside another pool. At most one lane is
    // out per frame in flight, so what is resident is still the worker count
    // this pass was sized against.
    let idle: Mutex<Vec<Lane>> = Mutex::new(Vec::new());
    let mut workers = workers.map_or_else(
        || size_the_pool(session, lights, DEFAULT_SURVEY_BUDGET_BYTES),
        |named| named.max(1),
    );

    type Outcome = Option<std::result::Result<Measured, (String, String)>>;
    let read = |id: FrameId| -> Outcome {
        // Asked before the work rather than after it, because rayon has no
        // cancellation of its own: the cheapest honest stop is a frame that
        // declines to start. Frames already under way are finished and kept -
        // they are measurements, and throwing away a dozen decoded frames to
        // make the ending tidy is the worse trade.
        if reporter.stopped() {
            return None;
        }
        let name = session
            .path(id)
            .and_then(|path| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| format!("frame {}", id.index()));
        let mut lane = idle.lock().unwrap_or_else(|p| p.into_inner()).pop().unwrap_or_default();
        let outcome = read_one(host, session, id, &name, masters, options, &mut lane);
        // Handed back before the frame is reported, so a lane is never held for
        // the length of someone else's progress callback.
        idle.lock().unwrap_or_else(|p| p.into_inner()).push(lane);
        reporter.finished(|done| Step::FrameRead { done, total, name: &name });
        // Rendered here rather than carried out, so that `Error` never has to
        // cross a thread boundary for this to compile.
        Some(outcome.map_err(|error| (name, format!("{error}"))))
    };

    // A pool that will not build is not a reason to refuse to measure the run;
    // it only costs speed. The fallback runs genuinely serially, for the reason
    // `session::scan` gives at the same point: falling through to `par_iter`
    // outside an `install` would run on rayon's global pool, whose thread count
    // owes nothing to the budget this pass was sized against, and the number
    // reported to the user would be a fiction.
    let outcomes: Vec<Outcome> = match rayon::ThreadPoolBuilder::new().num_threads(workers).build()
    {
        Ok(pool) => pool.install(|| lights.par_iter().copied().map(read).collect()),
        Err(err) => {
            log::warn!("measuring the run on one thread: {err}");
            workers = 1;
            lights.iter().copied().map(read).collect()
        }
    };

    // Back into the order the night was shot in. Collecting an indexed parallel
    // iterator already preserves it; this loop only splits the outcomes apart,
    // so `failed` comes out chronological too. A `None` is a light the stop
    // reached first: never attempted, so neither measured nor failed.
    let mut frames = Vec::with_capacity(total);
    let mut failed = Vec::new();
    for outcome in outcomes.into_iter().flatten() {
        match outcome {
            Ok(measured) => frames.push(measured),
            Err(failure) => failed.push(failure),
        }
    }

    // A stopped pass returns what it measured rather than nothing. The caller
    // knows it stopped - it is the one that said so - and half a run of
    // measurements is still half a run of measurements.
    Survey {
        frames,
        stopped: reporter.stopped(),
        failed,
        workers,
        seconds: started.elapsed().as_secs_f64(),
    }
}

/// Turns the memory budget into a worker count, against the largest frame the
/// run actually holds.
///
/// Unlike `session::scan`'s equivalent this costs no open: the session has been
/// scanned already, so every layout is in hand.
fn size_the_pool(session: &Session, lights: &[FrameId], budget_bytes: u64) -> usize {
    let widest = lights
        .iter()
        .filter_map(|id| session.frame(*id))
        .filter_map(|record| record.layout.required_bytes())
        .max()
        .unwrap_or(1 << 26) as u64;
    let per_worker = (widest * PEAK_MULTIPLE_OF_RAW).max(1);
    (budget_bytes / per_worker).clamp(1, worker_ceiling() as u64) as usize
}

/// How many lights are worth measuring at once on this machine.
///
/// `available_parallelism` answers a different question: it reports logical
/// parallelism, which on a part with simultaneous multithreading is twice the
/// cores, and the standard library offers no way to ask for the physical count.
/// The second thread on a core brings no decoder of its own and shares the
/// cache and the memory pipe with the first, which on work that streams whole
/// frames costs more than it brings.
///
/// So this halves, which is a guess about the hardware rather than a fact about
/// it, and on a part without simultaneous multithreading it leaves speed
/// unclaimed. It is affordable because the curve is flat near its top, and it
/// is honest because the number arrived at is reported in [`Survey::workers`]
/// and [`survey_with_workers`] takes an override. On the frames this project is
/// measured against the budget binds first anyway, so this ceiling only decides
/// the small-frame case.
fn worker_ceiling() -> usize {
    let logical = std::thread::available_parallelism().map_or(4, |n| n.get());
    if logical >= 8 { logical / 2 } else { logical }
}

/// What one survey worker keeps for the length of the run.
///
/// The calibrated plane belongs here as much as the detection's own scratch
/// does: same size, and the same fresh mapping asked of the kernel and handed
/// straight back on every frame. What is still allocated per frame is the decode
/// buffer, which the plugin boundary owns.
#[derive(Debug, Default)]
struct Lane {
    pixels: Vec<f32>,
    scratch: Scratch,
}

#[allow(clippy::too_many_arguments)]
fn read_one(
    host: &PluginHost,
    session: &Session,
    id: FrameId,
    name: &str,
    masters: &MasterSet,
    options: &DetectOptions,
    lane: &mut Lane,
) -> Result<Measured> {
    let path = session
        .path(id)
        .ok_or_else(|| Error::Unmeasurable {
            name: name.to_owned(),
            reason: "the session holds no path for it".to_owned(),
        })?;

    let started = Instant::now();
    let frame = host.open(&path)?;
    let Samples::U16(raw) = frame.decode()? else {
        return Err(Error::Unmeasurable {
            name: name.to_owned(),
            reason: "floating-point sensor data is not measured yet".to_owned(),
        });
    };

    if masters.is_empty() {
        lane.pixels.clear();
        lane.pixels.extend(raw.iter().map(|value| f32::from(*value)));
    } else {
        apply_into(
            &raw,
            masters.dark.as_ref(),
            masters.flat.as_ref(),
            frame.layout(),
            &mut lane.pixels,
        );
    }

    // The saturation test inside `detect` reads the raw samples, whatever was
    // applied to the plane the stars were measured on: the white level is a
    // property of the sensor and means nothing after a pedestal has been
    // removed.
    let detection = detect_with(&lane.pixels, &raw, frame.layout(), options, &mut lane.scratch)
        .ok_or_else(|| {
        Error::Unmeasurable {
            name: name.to_owned(),
            reason: "no measurable sky to threshold against".to_owned(),
        }
    })?;

    let record = &session[id];
    Ok(Measured {
        id,
        name: name.to_owned(),
        path,
        layout: *frame.layout(),
        camera_model: record.info.camera_model.clone(),
        exposure_seconds: record.info.exposure_seconds,
        iso: record.info.iso,
        detection,
        seconds: started.elapsed().as_secs_f64(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{FrameKind, testing};

    fn a_run_of(count: usize) -> (Session, Vec<FrameId>) {
        let mut session = Session::new();
        let lights = (0..count)
            .map(|index| {
                let name = format!("IMG_{index:04}.CR3");
                testing::assigned(&mut session, FrameKind::Light, &name, testing::light())
            })
            .collect();
        (session, lights)
    }

    #[test]
    fn a_night_of_subframes_stays_in_the_order_it_was_shot() {
        // The property `align` depends on. It chooses its reference from the
        // middle of the run and chains its seeds between neighbours, so an
        // index is a position in the night; measuring the frames several at a
        // time must not reorder them by which finished first. The work is
        // deliberately slowest-first, so completion order is the reverse of
        // input order and a collect that did not preserve position would fail
        // rather than pass by luck.
        let given: Vec<usize> = (0..64).collect();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(8).build().expect("a pool");
        let measured: Vec<usize> = pool.install(|| {
            given
                .par_iter()
                .copied()
                .map(|index| {
                    std::thread::sleep(std::time::Duration::from_micros(
                        (64 - index) as u64 * 50,
                    ));
                    index
                })
                .collect()
        });
        assert_eq!(measured, given, "the run came back reordered by completion time");
    }

    #[test]
    fn a_budget_that_fits_one_frame_measures_one_frame_at_a_time() {
        // A hundred-megapixel body holds nearly two gibibytes per worker, and
        // the failure this guards is the pool opening one per core and asking
        // for thirty of them.
        let (session, lights) = a_run_of(8);
        let peak = 8280u64 * 5520 * 2 * PEAK_MULTIPLE_OF_RAW;
        assert_eq!(size_the_pool(&session, &lights, peak), 1);
        assert_eq!(size_the_pool(&session, &lights, peak * 3 / 2), 1);
        assert_eq!(size_the_pool(&session, &lights, peak * 2), 2);
    }

    #[test]
    fn a_generous_budget_stops_at_the_machine_rather_than_at_the_frame_count() {
        let (session, lights) = a_run_of(8);
        let workers = size_the_pool(&session, &lights, u64::MAX / 2);
        assert_eq!(workers, worker_ceiling());
        assert!(workers >= 1);
    }
}
