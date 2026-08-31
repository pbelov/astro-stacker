//! The `master` subcommand: turn a session's calibration sets into masters.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use astro_core::PluginHost;
use astro_core::calibrate::{CombineOptions, Master, build, fits};
use astro_core::session::{FrameKind, FrameSet, Partition, Session, StackPlan, scan};
use clap::{ArgMatches, Args};

use crate::format;
use crate::scan_command::{ScanArgs, options_from};

#[derive(Args, Debug)]
pub struct MasterArgs {
    #[command(flatten)]
    pub scan: ScanArgs,

    /// Where to write the masters. Created if it does not exist.
    #[arg(long, short = 'o', value_name = "DIR")]
    pub out: PathBuf,

    /// How much decoded pixel data may be held at once, in mebibytes. Below
    /// this the combination is exact and reads each frame once; above it the
    /// combination streams and reads each frame twice.
    #[arg(long = "resident-mb", value_name = "MB")]
    pub resident_mb: Option<u64>,
}

pub use astro_core::pipeline::{MasterSet, Wanted};

pub fn run(host: &PluginHost, args: &MasterArgs, matches: &ArgMatches) -> Result<()> {
    let (options, tolerances) = options_from(&args.scan, matches)?;
    let report = scan(host, &options)?;
    let partition = report.session.partition(&tolerances);

    let chosen = crate::plan::choose(&report.session, &partition, args.scan.set)?;
    chosen.announce(&report.session);
    std::fs::create_dir_all(&args.out)
        .with_context(|| format!("creating {}", args.out.display()))?;

    let mut combine = CombineOptions::default();
    if let Some(mb) = args.resident_mb {
        combine.resident_bytes = mb.saturating_mul(1 << 20);
    }

    let masters = build_masters(
        host,
        &report.session,
        &partition,
        chosen.plan,
        &combine,
        &args.out,
        // Writing them out is the whole point of this command, so a matched
        // bias is a deliverable here rather than dead weight.
        Wanted::Matched,
        true,
        args.scan.quiet,
    )?;

    if masters.bias.is_none() && masters.dark.is_none() && masters.flat.is_none() {
        bail!("no calibration set was matched to the lights, so no master was built");
    }
    Ok(())
}

/// Builds every master the plan matched, optionally writing each one out.
///
/// Shared with `calibrate` rather than duplicated: two commands that disagreed
/// about how a master is combined would produce two different images from one
/// session.
#[allow(clippy::too_many_arguments)]
pub fn build_masters(
    host: &PluginHost,
    session: &Session,
    partition: &Partition,
    plan: &StackPlan,
    options: &CombineOptions,
    out: &Path,
    wanted: Wanted,
    write: bool,
    quiet: bool,
) -> Result<MasterSet> {
    // The bias, then the dark, then the flat: the order they are applied in,
    // which is also the order the user will want to read them in.
    let roles = [
        (FrameKind::Bias, plan.bias.as_ref()),
        (FrameKind::Dark, plan.dark.as_ref()),
        (FrameKind::Flat, plan.flat.as_ref()),
    ];

    let mut built = MasterSet::default();
    for (kind, matched) in roles {
        let Some(matched) = matched else {
            if wanted.includes(kind) {
                println!("{:<10} nothing matched, nothing built", kind.name());
            }
            continue;
        };
        if !wanted.includes(kind) {
            println!(
                "{:<10} set {} matched, not built: a light subtracts a dark, never a bias",
                kind.name(),
                matched.set.index()
            );
            continue;
        }
        let Some(set) = partition.set(matched.set) else { continue };
        let master = build_one(host, session, set, options, out, write, quiet)?;
        match kind {
            FrameKind::Bias => built.bias = Some(master),
            FrameKind::Dark => built.dark = Some(master),
            _ => built.flat = Some(master),
        }
    }
    Ok(built)
}

fn build_one(
    host: &PluginHost,
    session: &Session,
    set: &FrameSet,
    options: &CombineOptions,
    out: &Path,
    write: bool,
    quiet: bool,
) -> Result<Master> {
    let kind = set.kind();
    let active = set.members.iter().filter(|id| session[**id].is_active()).count();
    println!();
    println!("{} from set {}, {}", kind.name(), set.id.index(), format::plural(active, "frame"));

    let started = Instant::now();
    let master = build(host, session, set, options, &|done, total| {
        // One line, rewritten. A master from 85 frames is two passes and about
        // half a minute; silence for that long reads as a hang.
        if !quiet {
            eprint!("\r  reading {done} of {total}");
        }
    })
    .with_context(|| format!("building the master {} from set {}", kind.name(), set.id.index()))?;
    if !quiet {
        eprint!("\r                                        \r");
    }

    print_master(&master, started.elapsed().as_secs_f64());

    if write {
        let path = out.join(format!("{}.fits", master.file_stem()));
        fits::write(
            &path,
            &master.pixels,
            master.layout.width as usize,
            master.layout.height as usize,
            &master.header(),
        )
        .with_context(|| format!("writing {}", path.display()))?;
        println!("  wrote      {}", path.display());
    }
    Ok(master)
}

fn print_master(master: &Master, seconds: f64) {
    println!(
        "  combined   {} of {}, {} in {seconds:.1}s",
        master.method.name(),
        format::plural(master.frames, "frame"),
        if master.resident { "all held at once" } else { "streamed in two passes" }
    );
    // A healthy rejection is a fraction of a per cent. Whole percentages mean
    // either the threshold is wrong or the frames disagree with each other, and
    // both are the user's business.
    if matches!(master.method, astro_core::calibrate::Method::ClippedMean) {
        println!("  rejected   {:.4}% of samples", master.rejected_fraction() * 100.0);
    }

    if let Some(pedestal) = &master.pedestal {
        let cells = (pedestal.cfa_width * pedestal.cfa_height) as usize;
        let listed: Vec<String> =
            pedestal.cells[..cells.min(4)].iter().map(|v| format!("{v:.1}")).collect();
        println!(
            "  pedestal   {} ADU per mosaic cell, from {}",
            listed.join(", "),
            pedestal.source.name()
        );
    }
    if let Some(divisors) = &master.normalisation {
        let listed: Vec<String> = divisors
            .iter()
            .enumerate()
            .filter_map(|(index, value)| {
                value.map(|v| format!("{} {v:.0}", "RGBE".as_bytes()[index] as char))
            })
            .collect();
        println!("  normalised {} ADU, per colour", listed.join("  "));
    }

    let mut shot = Vec::new();
    if let Some(exposure) = master.info.exposure_seconds {
        shot.push(format::exposure(exposure));
    }
    if let Some(iso) = master.info.iso {
        shot.push(format!("ISO {iso:.0}"));
    }
    if !shot.is_empty() {
        println!("  valid for  {} on {}", shot.join(", "), master.info.camera_model);
    }
}
