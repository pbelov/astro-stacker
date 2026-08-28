//! The `calibrate` subcommand: apply a session's masters to its lights.
//!
//! Writes one FITS per light, which is a lot of disk — a 19-megapixel frame is
//! 76 MB as float, and a 226-frame session is 17 GB. So it calibrates one frame
//! by default: the point of this command is to *look* at a calibrated light and
//! see that the vignetting and the amp glow are gone. Integration will calibrate
//! on the way past and never write them at all.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use astro_core::calibrate::{apply, fits};
use astro_core::session::{FrameId, FrameKind, Session};
use astro_core::{PluginHost, Samples};
use clap::{ArgMatches, Args};

use crate::format;
use crate::master_command::{MasterSet, build_masters};
use crate::scan_command::{ScanArgs, options_from};

#[derive(Args, Debug)]
pub struct CalibrateArgs {
    #[command(flatten)]
    pub scan: ScanArgs,

    /// Where to write the calibrated lights. Created if it does not exist.
    #[arg(long, short = 'o', value_name = "DIR")]
    pub out: PathBuf,

    /// How many lights to calibrate, from the start of the run. One by default:
    /// a calibrated light is 76 MB and a whole session is many gigabytes.
    #[arg(long, value_name = "N", default_value_t = 1)]
    pub frames: usize,

    /// Skip this many lights first, to look at one from the middle of the run.
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub skip: usize,

    /// Also write the masters that were built along the way.
    #[arg(long)]
    pub keep_masters: bool,
}

pub fn run(host: &PluginHost, args: &CalibrateArgs, matches: &ArgMatches) -> Result<()> {
    let (options, tolerances) = options_from(&args.scan, matches)?;
    let report = astro_core::session::scan(host, &options)?;
    let partition = report.session.partition(&tolerances);

    let chosen = crate::plan::choose(&report.session, &partition, args.scan.set)?;
    chosen.announce(&report.session);
    let (plan, lights) = (chosen.plan, chosen.lights);
    std::fs::create_dir_all(&args.out)
        .with_context(|| format!("creating {}", args.out.display()))?;

    let masters = build_masters(
        host,
        &report.session,
        &partition,
        plan,
        &Default::default(),
        &args.out,
        args.keep_masters,
        args.scan.quiet,
    )?;

    if masters.dark.is_none() && masters.flat.is_none() {
        bail!("neither a dark nor a flat was matched, so calibration would change nothing");
    }

    let chosen: Vec<FrameId> = lights
        .members
        .iter()
        .copied()
        .filter(|id| report.session[*id].is_active())
        .skip(args.skip)
        .take(args.frames)
        .collect();
    if chosen.is_empty() {
        bail!("--skip {} left no lights to calibrate", args.skip);
    }

    println!();
    for id in chosen {
        calibrate_one(host, &report.session, id, &masters, &args.out)?;
    }
    Ok(())
}

fn calibrate_one(
    host: &PluginHost,
    session: &Session,
    id: FrameId,
    masters: &MasterSet,
    out: &std::path::Path,
) -> Result<()> {
    let path = session.path(id).context("a frame with no path")?;
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    println!("{name}");

    let started = Instant::now();
    let frame = host.open(&path).with_context(|| format!("opening {}", path.display()))?;
    let Samples::U16(raw) = frame.decode().with_context(|| format!("decoding {}", path.display()))?
    else {
        bail!("floating-point sensor data is not calibrated yet");
    };

    let (pixels, undefined) =
        apply(&raw, masters.dark.as_ref(), masters.flat.as_ref(), frame.layout());
    let elapsed = started.elapsed().as_secs_f64();

    println!(
        "  applied    {} in {elapsed:.2}s",
        [
            masters.dark.as_ref().map(|m| format!("a dark of {} frames", m.frames)),
            masters.flat.as_ref().map(|m| format!("a flat of {} frames", m.frames)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" and ")
    );
    // Undefined pixels are the masked border, where a normalised flat is zero
    // plus noise. Counted and named rather than filled in: a large number here
    // means the flat is wrong somewhere inside the frame too.
    println!(
        "  undefined  {} ({:.2}% of the frame, and the border alone is {:.2}%)",
        format::plural(undefined as usize, "pixel"),
        undefined as f64 / pixels.len() as f64 * 100.0,
        border_fraction(frame.layout()) * 100.0
    );
    report_levels(&pixels, frame.layout());

    let header = fits::Header {
        image_type: FrameKind::Light.name().to_owned(),
        instrument: session[id].info.camera_model.clone(),
        exposure: session[id].info.exposure_seconds,
        iso: session[id].info.iso,
        frames: 1,
        combination: "calibrated".to_owned(),
        bayer_pattern: astro_plugin_abi::safe::cfa_pattern_name(frame.layout()),
        notes: vec![describe(masters)],
    };
    let target = out.join(format!("{name}_calibrated.fits"));
    fits::write(
        &target,
        &pixels,
        frame.layout().width as usize,
        frame.layout().height as usize,
        &header,
    )
    .with_context(|| format!("writing {}", target.display()))?;
    println!("  wrote      {}", target.display());
    Ok(())
}

fn describe(masters: &MasterSet) -> String {
    let mut parts = Vec::new();
    if let Some(dark) = &masters.dark {
        parts.push(format!("dark of {} frames subtracted", dark.frames));
    }
    if let Some(flat) = &masters.flat {
        parts.push(format!("flat of {} frames divided", flat.frames));
    }
    if parts.is_empty() { "nothing applied".to_owned() } else { parts.join(", ") }
}

/// How much of the frame is outside the light-sensitive area, which is where a
/// normalised flat is zero and a calibrated value is undefined by construction.
fn border_fraction(layout: &astro_core::ImageLayout) -> f64 {
    let whole = (layout.width as f64) * (layout.height as f64);
    let active = (layout.active_width as f64) * (layout.active_height as f64);
    if whole <= 0.0 { 0.0 } else { (whole - active) / whole }
}

/// What the calibrated frame looks like, in the same terms `measure` uses, so
/// the two can be compared before and after.
fn report_levels(pixels: &[f32], layout: &astro_core::ImageLayout) {
    let mut sample: Vec<f32> = Vec::new();
    let x0 = layout.active_x as usize;
    let y0 = layout.active_y as usize;
    let width = layout.width as usize;
    // A coarse sample: the point is a number to compare, not a statistic.
    for y in (y0..y0 + layout.active_height as usize).step_by(37) {
        let row = y * width;
        for x in (x0..x0 + layout.active_width as usize).step_by(53) {
            let value = pixels[row + x];
            if value.is_finite() {
                sample.push(value);
            }
        }
    }
    if sample.is_empty() {
        println!("  levels     nothing finite in the active area");
        return;
    }
    sample.sort_by(f32::total_cmp);
    let at = |fraction: f64| sample[((sample.len() - 1) as f64 * fraction) as usize];
    println!(
        "  levels     median {:.1}, 1% {:.1}, 99% {:.1} ADU over {} sampled",
        at(0.5),
        at(0.01),
        at(0.99),
        sample.len()
    );
}
