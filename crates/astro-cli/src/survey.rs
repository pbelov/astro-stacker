//! Reading a run, with the command line's own reporting around it.
//!
//! The pass itself lives in `astro_core::pipeline`, because the window needs
//! exactly the same answer and a second copy of it would be the copy that
//! drifts. What is left here is the argument group and the lines it prints.

use anyhow::{Result, bail};
use astro_core::PluginHost;
use astro_core::calibrate::Method;
pub use astro_core::pipeline::{Flow, MasterSet, Step, Survey};
use astro_core::session::FrameId;
use astro_core::stars::DetectOptions;
use clap::{ArgMatches, Args};

use crate::format;
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

pub use astro_core::pipeline::Measured as Surveyed;

/// A run, read, with the masters it was calibrated with.
pub struct Read {
    pub survey: Survey,
    pub masters: MasterSet,
}

impl std::ops::Deref for Read {
    type Target = Survey;

    fn deref(&self) -> &Survey {
        &self.survey
    }
}

/// Reads a run: scans, builds masters, then calibrates and measures every
/// selected light.
pub fn read(
    host: &PluginHost,
    scan: &ScanArgs,
    matches: &ArgMatches,
    args: &SurveyArgs,
) -> Result<Read> {
    if args.step == 0 {
        bail!("--step 0 would read no frames");
    }
    let (options, tolerances) = options_from(scan, matches)?;
    let report = astro_core::session::scan(host, &options)?;
    let partition = report.session.partition(&tolerances);

    let chosen = crate::plan::choose(&report.session, &partition, scan.set)?;
    chosen.announce(&report.session);
    let (plan, lights) = (chosen.plan, chosen.lights);

    // Masters are built in memory and never written here. A command that reads
    // a run to report numbers should not quietly fill a directory with
    // gigabytes of FITS; `master` exists for when that is what was wanted.
    let masters = if args.raw {
        println!("note: detecting on raw frames, so hot photosites will be counted as stars");
        MasterSet::default()
    } else {
        astro_core::pipeline::masters(
            host,
            &report.session,
            &partition,
            plan,
            &Default::default(),
            &|step| announce(step, scan.quiet),
        )?
    };
    if !args.raw && masters.dark.is_none() {
        println!(
            "note: no master dark was matched, so hot photosites will be counted as stars - \
             --raw says so on purpose, and this is the same thing by accident"
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
    println!("\nreading {}", format::plural(selected.len(), "light"));

    let survey = astro_core::pipeline::survey(
        host,
        &report.session,
        &selected,
        &masters,
        &detect_options,
        &|step| announce(step, scan.quiet),
    );
    if !scan.quiet {
        eprint!("\r                                        \r");
    }
    if survey.frames.is_empty() {
        bail!("no frame could be read");
    }
    Ok(Read { survey, masters })
}

/// One line, rewritten in place. A master from 85 frames is two passes and
/// about half a minute, and a run of 226 lights is over two: silence for that
/// long reads as a hang.
fn announce(step: Step<'_>, quiet: bool) -> Flow {
    match step {
        Step::MasterMissing { kind } => {
            println!("{:<10} nothing matched, nothing built", kind.name())
        }
        Step::MasterReading { done, total, .. } => {
            if !quiet {
                eprint!("\r  reading {done} of {total}");
            }
        }
        Step::MasterBuilt { kind, master, seconds } => {
            if !quiet {
                eprint!("\r                                        \r");
            }
            println!();
            println!("{} from {}", kind.name(), format::plural(master.frames, "frame"));
            println!(
                "  combined   {} of {}, {} in {seconds:.1}s",
                master.method.name(),
                format::plural(master.frames, "frame"),
                if master.resident { "all held at once" } else { "streamed in two passes" }
            );
            // A healthy rejection is a fraction of a per cent. Whole
            // percentages mean either the threshold is wrong or the frames
            // disagree with each other, and both are the user's business.
            if matches!(master.method, Method::ClippedMean) {
                println!("  rejected   {:.4}% of samples", master.rejected_fraction() * 100.0);
            }
        }
        Step::FrameRead { done, total, .. } => {
            if !quiet && total > 8 {
                eprint!("\r  read {done} of {total}");
            }
        }
    }
    // The command line has no cancel of its own: Ctrl-C already ends the
    // process, and a second way to stop would be a second thing to keep in step.
    Flow::Continue
}
