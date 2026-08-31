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

use std::time::Instant;

use astro_plugin_abi::abi::ImageLayout;

use crate::calibrate::{CombineOptions, Master, apply, build};
use crate::error::{Error, Result};
use crate::session::{FrameId, FrameKind, Partition, SetId, Session, StackPlan};
use crate::stars::{DetectOptions, Detection, detect};
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
    FrameRead { done: usize, total: usize, name: &'a str },
    /// Depositing frames onto the output grid. Rejection needs two passes over
    /// them, and a progress bar that restarted without saying so reads as a
    /// crash rather than as the second pass.
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
    on: &dyn Fn(Step) -> Flow,
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
    pub seconds: f64,
}

/// A whole run, read.
#[derive(Debug, Default)]
pub struct Survey {
    pub frames: Vec<Measured>,
    /// Whether the caller stopped it early. `frames` then holds what was
    /// measured before that, which is worth keeping and must not be mistaken
    /// for the whole run.
    pub stopped: bool,
    /// Frames that were selected but could not be read, with the reason.
    ///
    /// Carried rather than dropped: a report that says "225 of 226" has to be
    /// able to name the missing one, and a session that silently lost forty
    /// frames looks exactly like one that never had them.
    pub failed: Vec<(String, String)>,
    pub seconds: f64,
}

/// Calibrates and measures every light given.
///
/// One unreadable frame in a night of two hundred does not lose the other
/// hundred and ninety-nine, and does not pass unmentioned either.
pub fn survey(
    host: &PluginHost,
    session: &Session,
    lights: &[FrameId],
    masters: &MasterSet,
    options: &DetectOptions,
    on: &dyn Fn(Step) -> Flow,
) -> Survey {
    let started = Instant::now();
    let mut frames = Vec::with_capacity(lights.len());
    let mut failed = Vec::new();
    let mut stopped = false;

    for (index, id) in lights.iter().copied().enumerate() {
        let name = session
            .path(id)
            .and_then(|path| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| format!("frame {}", id.index()));
        if on(Step::FrameRead { done: index, total: lights.len(), name: &name }) == Flow::Stop {
            stopped = true;
            break;
        }

        match read_one(host, session, id, &name, masters, options) {
            Ok(frame) => frames.push(frame),
            Err(error) => failed.push((name, format!("{error}"))),
        }
    }

    // A stopped pass returns what it measured rather than nothing. The caller
    // knows it stopped - it is the one that said so - and half a run of
    // measurements is still half a run of measurements.
    Survey { frames, stopped, failed, seconds: started.elapsed().as_secs_f64() }
}

fn read_one(
    host: &PluginHost,
    session: &Session,
    id: FrameId,
    name: &str,
    masters: &MasterSet,
    options: &DetectOptions,
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

    let pixels = if masters.is_empty() {
        raw.iter().map(|value| f32::from(*value)).collect()
    } else {
        apply(&raw, masters.dark.as_ref(), masters.flat.as_ref(), frame.layout()).0
    };

    // The saturation test inside `detect` reads the raw samples, whatever was
    // applied to the plane the stars were measured on: the white level is a
    // property of the sensor and means nothing after a pedestal has been
    // removed.
    let detection = detect(&pixels, &raw, frame.layout(), options).ok_or_else(|| {
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
