//! Reading a run once: calibrate every light, find its stars, measure its
//! shape.
//!
//! Three commands need exactly this and nothing else before they can start —
//! `stars` reports the shapes, `register` matches the star lists, and `stack`
//! needs both — and the pass costs about 140 seconds of decoding for this
//! project's reference session. Having it written once means the three cannot
//! drift apart on what "calibrated" means, which is the failure that matters
//! here: a `stack` that applied the flat slightly differently from the `stars`
//! that chose which frames to stack would be measuring one thing and combining
//! another.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use astro_core::calibrate::apply;
use astro_core::session::{FrameId, Session};
use astro_core::stars::{Detection, DetectOptions, detect};
use astro_core::{PluginHost, Samples};
use clap::{ArgMatches, Args};

use crate::format;
use crate::master_command::{MasterSet, build_masters};
use crate::scan_command::{ScanArgs, options_from};

/// The options every command that reads a run shares.
#[derive(Args, Debug, Clone)]
pub struct SurveyArgs {
    /// How many lights to read, from the start of the run. All of them by
    /// default.
    #[arg(long, value_name = "N")]
    pub frames: Option<usize>,

    /// Skip this many lights first.
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub skip: usize,

    /// Read every Nth light rather than every one, to sample a long run
    /// quickly.
    #[arg(long, value_name = "N", default_value_t = 1)]
    pub step: usize,

    /// Detect on the raw frame instead of calibrating first. Faster, and wrong
    /// in a specific way: hot photosites the dark would have removed are
    /// counted as round stars.
    #[arg(long)]
    pub raw: bool,

    /// How many times the noise a peak must stand above the sky to count.
    #[arg(long, value_name = "SIGMA", default_value_t = 5.0)]
    pub sigma: f32,

    /// Keep at most this many stars per frame, most confidently detected first.
    #[arg(long, value_name = "N", default_value_t = 1000)]
    pub max_stars: usize,

    /// Largest footprint worth measuring, in photosites. Above it a source is a
    /// satellite, an aeroplane or two stars that should have been deblended.
    #[arg(long, value_name = "N", default_value_t = 400)]
    pub max_footprint: usize,
}

/// One light, read and measured.
pub struct Surveyed {
    pub name: String,
    pub detection: Detection,
    pub seconds: f64,
}

/// A whole run, read.
pub struct Survey {
    pub frames: Vec<Surveyed>,
    /// Frames that were selected but could not be read, with the reason.
    /// Carried rather than only printed to stderr: a command that says "225 of
    /// 226" has to be able to name the missing one in the same report.
    pub failed: Vec<(String, String)>,
    pub seconds: f64,
}

/// Reads a run: scans, builds masters, then calibrates and measures every
/// selected light.
pub fn read(
    host: &PluginHost,
    scan: &ScanArgs,
    matches: &ArgMatches,
    args: &SurveyArgs,
) -> Result<Survey> {
    if args.step == 0 {
        bail!("--step 0 would read no frames");
    }
    let (options, tolerances) = options_from(scan, matches)?;
    let report = astro_core::session::scan(host, &options)?;
    let partition = report.session.partition(&tolerances);

    let chosen = crate::plan::choose(&report.session, &partition, scan.set)?;
    chosen.announce(&report.session);
    let (plan, lights) = (chosen.plan, chosen.lights);

    let mut notes = Vec::new();
    // Masters are built in memory and never written here. A command that reads
    // a run to report numbers should not quietly fill a directory with
    // gigabytes of FITS; `master` exists for when that is what was wanted.
    let masters = if args.raw {
        notes.push("detecting on raw frames: hot photosites will be counted as stars".to_owned());
        MasterSet::default()
    } else {
        build_masters(
            host,
            &report.session,
            &partition,
            plan,
            &Default::default(),
            &std::env::temp_dir(),
            false,
            scan.quiet,
        )?
    };
    if !args.raw && masters.dark.is_none() {
        notes.push(
            "no master dark was matched, so hot photosites will be counted as stars - \
             --raw says so on purpose, and this is the same thing by accident"
                .to_owned(),
        );
    }

    let selected: Vec<FrameId> = lights
        .members
        .iter()
        .copied()
        .filter(|id| report.session[*id].is_active())
        .skip(args.skip)
        .step_by(args.step)
        .take(args.frames.unwrap_or(usize::MAX))
        .collect();
    if selected.is_empty() {
        bail!("--skip {} left no lights to read", args.skip);
    }

    let detect_options = DetectOptions {
        detect_sigma: args.sigma,
        max_stars: args.max_stars,
        max_footprint: args.max_footprint,
        ..Default::default()
    };
    for note in &notes {
        println!("note: {note}");
    }
    println!("\nreading {}", format::plural(selected.len(), "light"));

    let started = Instant::now();
    let mut frames = Vec::with_capacity(selected.len());
    let mut failed = Vec::new();
    for id in selected {
        match read_one(host, &report.session, id, &masters, &detect_options) {
            Ok(frame) => frames.push(frame),
            // One unreadable frame in a night of two hundred must not lose the
            // other hundred and ninety-nine, and must not pass unmentioned
            // either.
            Err(error) => {
                let name = report
                    .session
                    .path(id)
                    .and_then(|path| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| format!("frame {}", id.index()));
                failed.push((name, format!("{error:#}")));
            }
        }
    }
    if frames.is_empty() {
        bail!("no frame could be read");
    }

    Ok(Survey { frames, failed, seconds: started.elapsed().as_secs_f64() })
}

fn read_one(
    host: &PluginHost,
    session: &Session,
    id: FrameId,
    masters: &MasterSet,
    options: &DetectOptions,
) -> Result<Surveyed> {
    let path = session.path(id).context("a frame with no path")?;
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();

    let started = Instant::now();
    let frame = host.open(&path).with_context(|| format!("opening {}", path.display()))?;
    let Samples::U16(raw) = frame.decode().with_context(|| format!("decoding {}", path.display()))?
    else {
        bail!("{name}: floating-point sensor data is not read yet");
    };

    let pixels = if masters.dark.is_some() || masters.flat.is_some() {
        apply(&raw, masters.dark.as_ref(), masters.flat.as_ref(), frame.layout()).0
    } else {
        raw.iter().map(|value| f32::from(*value)).collect()
    };

    // The saturation test inside `detect` reads the raw samples, whatever was
    // applied to the plane the stars were measured on: the white level is a
    // property of the sensor and means nothing after a pedestal has been
    // removed.
    let detection = detect(&pixels, &raw, frame.layout(), options)
        .with_context(|| format!("{name}: no measurable sky to threshold against"))?;

    Ok(Surveyed { name, detection, seconds: started.elapsed().as_secs_f64() })
}
