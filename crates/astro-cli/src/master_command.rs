//! The `master` subcommand: turn a session's calibration sets into masters.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use astro_core::calibrate::{CombineOptions, Master, build, fits};
use astro_core::session::{FrameKind, FrameSet, Session, scan};
use astro_core::PluginHost;
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

pub fn run(host: &PluginHost, args: &MasterArgs, matches: &ArgMatches) -> Result<()> {
    let (options, tolerances) = options_from(&args.scan, matches)?;
    let report = scan(host, &options)?;
    let partition = report.session.partition(&tolerances);

    let Some(plan) = partition.plans.first() else {
        bail!("no stackable set of lights was formed, so there is nothing to calibrate");
    };
    std::fs::create_dir_all(&args.out)
        .with_context(|| format!("creating {}", args.out.display()))?;

    let mut combine = CombineOptions::default();
    if let Some(mb) = args.resident_mb {
        combine.resident_bytes = mb.saturating_mul(1 << 20);
    }

    // The bias, then the dark, then the flat: the order they are applied in,
    // which is also the order the user will want to read them in.
    let roles = [
        (FrameKind::Bias, plan.bias.as_ref()),
        (FrameKind::Dark, plan.dark.as_ref()),
        (FrameKind::Flat, plan.flat.as_ref()),
    ];

    let mut built = 0usize;
    for (kind, matched) in roles {
        let Some(matched) = matched else {
            println!("{:<10} nothing matched, nothing built", kind.name());
            continue;
        };
        let Some(set) = partition.set(matched.set) else { continue };
        build_one(host, &report.session, set, &combine, &args.out, args.scan.quiet)?;
        built += 1;
    }

    if built == 0 {
        bail!("no calibration set was matched to the lights, so no master was built");
    }
    Ok(())
}

fn build_one(
    host: &PluginHost,
    session: &Session,
    set: &FrameSet,
    options: &CombineOptions,
    out: &std::path::Path,
    quiet: bool,
) -> Result<()> {
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

    Ok(())
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
